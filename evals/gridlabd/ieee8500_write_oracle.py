"""Full IEEE8500 canonical GLM, IR, edits and repeated physical-port checks."""
import argparse
import hashlib
import json
import re
from collections import deque
from pathlib import Path

import numpy as np
from group_recorders import recorders
from ieee8500_import_oracle import (
    LINK_CLASSES,
    NODE_CLASSES,
    grouped,
    imported_snapshot,
    inventory,
    kcl,
    native_snapshot,
)
from ieee8500_reference import SOURCE_SHA256, prepare
from oracle import LOCK, add, command, compare, run_glm


def objects_in(text):
    result = {}
    for cls, body in re.findall(r"object (\w+)\s*\{([^{}]*)\}", text):
        if cls not in NODE_CLASSES + LINK_CLASSES:
            continue
        fields = {k: a or b for k, a, b in re.findall(r'\b(name|parent|phases|from|to)\s+(?:"([^"]+)"|([^;\s]+))\s*;', body)}
        name = fields.pop("name")
        if name in result:
            raise ValueError("duplicate canonical object identity")
        result[name] = dict(fields, **{"class": cls})
    return result


def root(name, objects):
    seen = set()
    while "parent" in objects[name]:
        if name in seen:
            raise ValueError("electrical parent cycle")
        seen.add(name)
        name = objects[name]["parent"]
    return name


def path_between(edges, start, stop):
    neighbors = {}
    for name, a, b in edges:
        neighbors.setdefault(a, []).append((b, name, True))
        neighbors.setdefault(b, []).append((a, name, False))
    queue = deque([(start, [])])
    seen = {start}
    while queue:
        bus, path = queue.popleft()
        if bus == stop:
            return path
        for other, name, forward in neighbors.get(bus, []):
            if other not in seen:
                seen.add(other)
                queue.append((other, path + [(name, forward)]))
    raise ValueError(f"missing regulator path {start}->{stop}")


def record(text, profile, objects, folder, *, node_classes=NODE_CLASSES, link_classes=LINK_CLASSES):
    props = {cls: [f"voltage_{p}" for p in ("12N" if cls.startswith("triplex") else "ABC")] for cls in node_classes}
    props.update({cls: [f"{q}_{end}_{p}" for end in ("in", "out") for p in "ABC" for q in ("current", "power")] + ["power_losses"] for cls in link_classes})
    props = {cls: fields for cls, fields in props.items() if any(o["class"] == cls for o in objects.values())}
    text += "\n#set complex_format=%.15g%+.15gj\n#set complex_output_format=RECT\nmodule tape;\n"
    declarations, spec, outputs = recorders(props, objects, profile)
    if profile == "arras":
        (folder / "records").mkdir()
    text += declarations
    return text, spec, outputs


def canonical_snapshot(rows, actual, original):
    result = {}
    regulator_edges = [(name, obj["from"], obj["to"]) for name, obj in actual.items() if obj["class"] == "regulator"]
    for name, obj in original.items():
        if obj["class"] in NODE_CLASSES:
            bus = root(name, original)
            row = rows[bus]
            for p, prop in zip("ABC", "12N" if obj["class"].startswith("triplex") else "ABC", strict=True):
                add(result, f"bus.{name}.{p}", "V", row[f"voltage_{prop}"])
            continue
        if obj["class"] == "regulator":
            path = path_between(regulator_edges, root(obj["from"], original), root(obj["to"], original))
        else:
            native_obj = actual[name]
            forward = root(native_obj["from"], actual) == root(obj["from"], original)
            path = [(name, forward)]
        end_powers = np.zeros((2, 3), complex)
        for end, (link, forward) in enumerate((path[0], path[-1])):
            native_end = "in" if (end == 0) == forward else "out"
            sign = 1 if native_end == "in" else -1
            row = rows[link]
            for index, p in enumerate("ABC"):
                for q, unit in (("current", "A"), ("power", "VA")):
                    value = sign * row[f"{q}_{native_end}_{p}"]
                    add(result, f"link.{name}.{'in' if end == 0 else 'out'}.{p}.{q}", unit, value)
                    if q == "power":
                        end_powers[end, index] = value
        add(result, f"link.{name}.loss", "VA", end_powers.sum())
        add(result, f"link.{name}.reported_loss", "VA", sum(rows[link]["power_losses"] for link, _ in path))
    return result


def dss_snapshot(path, network, original):
    # The existing qualified observer covers lines, switches and all service
    # windings. Observe regulator chain boundaries independently of generated IDs.
    nonreg = {name: obj for name, obj in original.items() if obj["class"] != "regulator"}
    result, solve = imported_snapshot(path, network, nonreg)
    import opendssdirect as dss
    e = dss.NewContext()
    e.Basic.AllowChangeDir(False)
    if LOCK["dss"]["version"] not in e.Basic.Version():
        raise ValueError("OpenDSS pin mismatch")
    e.Text.Command(f'redirect "{path}"')
    e.Text.Command("set tolerance=1e-12 maxiterations=1000 controlmode=off")
    e.Solution.Solve()
    if not e.Solution.Converged() or e.Solution.ControlMode() != -1:
        raise ValueError("OpenDSS regulator observer solve failed")
    coils = {t["name"]: t for t in network["transformers"] if t["xsc_pct"] == [0.0]}
    def terminal(name, primary):
        if e.Circuit.SetActiveElement(("transformer." if primary else "line._pio_xfmr_") + name) < 0:
            raise ValueError("missing ideal transformer equivalent")
        n = e.CktElement.NumConductors()
        offset = 0 if primary else n
        current, power = e.CktElement.Currents(), e.CktElement.Powers()
        return complex(*current[2*offset:2*offset+2]), 1000 * complex(*power[2*offset:2*offset+2])
    for name, obj in original.items():
        if obj["class"] != "regulator":
            continue
        phases = np.zeros((2, 3), complex)
        for index, p in enumerate("ABC"):
            edges = [(n, t["windings"][0]["bus"], t["windings"][1]["bus"]) for n, t in coils.items() if t["windings"][0]["terminal_map"][0] == str(index+1)]
            path = path_between(edges, root(obj["from"], original), root(obj["to"], original))
            for end, (coil, forward) in enumerate((path[0], path[-1])):
                current, power = terminal(coil, (end == 0) == forward)
                add(result, f"link.{name}.{'in' if end == 0 else 'out'}.{p}.current", "A", current)
                add(result, f"link.{name}.{'in' if end == 0 else 'out'}.{p}.power", "VA", power)
                phases[end, index] = power
        loss = phases.sum()
        add(result, f"link.{name}.loss", "VA", loss)
        add(result, f"link.{name}.reported_loss", "VA", sum(complex(abs(v.real), v.imag) for v in phases.sum(axis=0)))
    return result, solve


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--bridge", type=Path, required=True)
    parser.add_argument("--pnnl-root", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--profile", action="append", choices=("pnnl", "arras"))
    parser.add_argument("--stage", action="append", choices=("fresh", "restored", "edited", "rebased", "grounded_reference"))
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    data = args.source.read_bytes()
    if hashlib.sha256(data).hexdigest() != SOURCE_SHA256:
        raise ValueError("IEEE8500 source pin mismatch")
    report = dict(scope=__doc__, source_sha256=SOURCE_SHA256, engines=LOCK, bridge_sha256=hashlib.sha256(args.bridge.read_bytes()).hexdigest(), cases={})
    for profile in args.profile or ("pnnl", "arras"):
        folder = args.output / profile
        folder.mkdir()
        source_text = prepare(data, profile, True).replace("default_maximum_voltage_error 1e-4", "default_maximum_voltage_error 1e-12")
        original = inventory(source_text)
        source = folder / "input.glm"
        source.write_text(source_text)
        emitted = folder / "emitted"
        bridge = json.loads(command([str(args.bridge.resolve()), str(source.resolve()), str(emitted.resolve()), profile], folder))
        (folder / "bridge.json").write_text(json.dumps(bridge, indent=2) + "\n")
        reference = folder / "reference"
        reference.mkdir()
        text, spec, outputs = record(source_text, profile, original, reference)
        (reference / "model.glm").write_text(text)
        run_glm(profile, reference, args.pnnl_root, outputs=outputs, open_files=65536)
        expected = native_snapshot(grouped(reference, spec), original)
        for stage in args.stage or ("fresh", "restored", "edited", "rebased", "grounded_reference"):
            row = dict(passed=False, comparisons={})
            try:
                if stage == "edited":
                    wanted, reference_solve = dss_snapshot(emitted / f"{stage}.dss/case.dss", bridge[stage]["network"], original)
                    row["reference_solve"] = reference_solve
                    row["reference_balance"] = kcl(wanted, bridge[stage]["network"], original)
                else:
                    wanted = expected
                repeat = folder / ("reimported-" + stage)
                case = emitted / f"{stage}.glm"
                repeated = json.loads(command([str(args.bridge.resolve()), str(case.resolve()), str(repeat.resolve()), profile], folder))
                (repeat / "bridge.json").write_text(json.dumps(repeated, indent=2) + "\n")
                for variant, path in (("canonical", case), ("reimported", repeat / "fresh.glm")):
                    run = folder / stage / variant
                    run.mkdir(parents=True)
                    actual_objects = objects_in(path.read_text())
                    clock = "\n".join(re.findall(r"clock\s*\{[^{}]*\}", source_text))
                    text, spec, outputs = record(clock + "\n" + path.read_text(), profile, actual_objects, run)
                    (run / "model.glm").write_text(text)
                    run_glm(profile, run, args.pnnl_root, outputs=outputs, open_files=65536)
                    actual = canonical_snapshot(grouped(run, spec), actual_objects, original)
                    row["comparisons"][variant] = compare(wanted, actual)
                    row.setdefault("current_balance", {})[variant] = kcl(actual, bridge[stage]["network"], original)
                    (run / "measurements.json").write_text(json.dumps(dict(expected=wanted, actual=actual), indent=2) + "\n")
                    if stage == "edited" and variant == "canonical":
                        row["unchanged_negative"] = compare(expected, actual)
                for variant, path, net in (("dss", emitted / f"{stage}.dss/case.dss", bridge[stage]["network"]), ("reimported-dss", repeat / "fresh.dss/case.dss", repeated["fresh"]["network"])):
                    if stage == "edited" and variant == "dss":
                        continue  # This solve is the reference, not an independent comparison.
                    actual, solve = dss_snapshot(path, net, original)
                    row["comparisons"][variant] = compare(wanted, actual)
                    row.setdefault("solves", {})[variant] = solve
                row["passed"] = all(c["passed"] for c in row["comparisons"].values()) and all(c["passed"] for c in row["current_balance"].values()) and (stage != "edited" or (row["reference_balance"]["passed"] and not row["unchanged_negative"]["passed"]))
            except Exception as error:
                row["error"] = str(error)
            report["cases"][f"{profile}/{stage}"] = row
            (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
            print(profile, stage, row["passed"], row.get("error", ""), {k: c["failures"][:3] for k, c in row["comparisons"].items() if not c["passed"]}, flush=True)
    report["passed"] = bool(report["cases"]) and all(c["passed"] for c in report["cases"].values())
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
