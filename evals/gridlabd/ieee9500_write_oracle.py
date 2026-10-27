"""Full IEEE9500 canonical GLM/IR/edits and repeated physical-port validation."""

import argparse
import hashlib
import json
import re
from collections import defaultdict
from pathlib import Path

from feeder_balance import current_balance
from ieee8500_import_oracle import grouped
from ieee8500_write_oracle import record
from ieee9500_import_oracle import (
    LINKS,
    NODES,
    SOURCE_HASHES,
    imported_snapshot,
    inventory,
)
from oracle import (
    FEEDER_TOLERANCES,
    LOCK,
    add,
    command,
    compare,
    parse_complex,
    run_glm,
)


def unwrap(value):
    value = value.get("value", value)
    return value.get("network", value)


def root(name, objects):
    seen = set()
    while "parent" in objects[name]:
        if name in seen:
            raise ValueError("electrical parent cycle")
        seen.add(name)
        name = objects[name]["parent"]
    return name


def load_fields(text):
    fields = {}
    for body in re.findall(r"object load\s*\{([^{}]*)\}", text):
        name = re.search(r'name "([^"]+)";', body)[1]
        fields[name] = {
            key: parse_complex(value)
            for key, value in re.findall(
                r"constant_impedance_([ABC][ABCN]?) ([^;]+);", body
            )
        }
    return fields


def canonical_snapshot(rows, actual, original, fields):
    result = {}
    expanded = defaultdict(list)
    for name in actual:
        if "__glm_shunt_" in name:
            line, side = name.split("__glm_shunt_", 1)
            expanded[line, side.split("_")[0]].append(name)

    def shunt_current(name, phase):
        row = rows[name]
        if "measured_power_" + phase in row:
            voltage = row["voltage_" + phase]
            power = row["measured_power_" + phase]
            if voltage == 0:
                if power != 0:
                    raise ValueError("nonzero shunt power at zero voltage")
                return 0j
            return (power / voltage).conjugate()
        # Arras has no measured load power. Apply the declared immutable
        # impedance law to its independently recorded terminal voltages.
        current = 0j
        for suffix, impedance in fields[name].items():
            a, b = suffix[0], suffix[1] if len(suffix) == 2 else "N"
            va = row["voltage_" + a]
            vb = 0j if b == "N" else row["voltage_" + b]
            value = (va - vb) / impedance
            if phase == a:
                current += value
            if phase == b:
                current -= value
        return current

    def endpoint_voltage(bus, phase):
        row = rows[bus]
        if original[bus]["class"].startswith("triplex"):
            value = row["voltage_" + {"A": "1", "B": "2", "C": "N"}[phase]]
            return -value if phase == "B" else value
        return row["voltage_" + phase]

    for name, obj in original.items():
        if obj["class"] in NODES:
            row = rows[root(name, original)]
            props = "12N" if obj["class"].startswith("triplex") else "ABC"
            for phase, prop in zip("ABC", props, strict=True):
                add(result, f"bus.{name}.{phase}", "V", row["voltage_" + prop])
            continue
        if name not in actual:
            raise ValueError(f"missing original link {name}")
        target = actual[name]
        phases = "AB" if obj["class"] == "triplex_line" else "ABC"
        losses = 0j
        aux = target["to"]
        for end, side in [("in", "from"), ("out", "to")]:
            bus = root(obj[side], original)
            if "__glm_current" in aux and aux not in original:
                branch = name if end == "in" else aux + "__to"
                if root(actual[branch]["from"], actual) != bus:
                    raise ValueError("current-variable physical endpoint changed")
                currents = {
                    p: rows[branch]["current_in_" + p]
                    + shunt_current(aux + "__" + side + "_shunt", p)
                    for p in phases
                }
            else:
                forward = root(target["from"], actual) == root(obj["from"], original)
                field = end if forward else ("out" if end == "in" else "in")
                sign = 1 if field == "in" else -1
                currents = {
                    p: sign * rows[name]["current_" + field + "_" + p] for p in phases
                }
                for shunt in expanded[name, side]:
                    if root(shunt, actual) != bus:
                        raise ValueError("line shunt physical endpoint changed")
                    for p in phases:
                        currents[p] += shunt_current(shunt, p)
            for phase in phases:
                power = endpoint_voltage(bus, phase) * currents[phase].conjugate()
                add(result, f"link.{name}.{end}.{phase}.current", "A", currents[phase])
                add(result, f"link.{name}.{end}.{phase}.power", "VA", power)
                losses += power
        add(result, f"link.{name}.loss", "VA", losses)
    return result


def native(path, profile, folder, original, pnnl_root, execution_timeout):
    text = path.read_text()
    objects = inventory(text)
    text, spec, outputs = record(
        "clock { timezone UTC0; starttime '2000-01-01 00:00:00'; stoptime '2000-01-01 00:00:01'; }\n"
        + text,
        profile,
        objects,
        folder,
        node_classes=NODES,
        link_classes=LINKS,
    )
    fields = load_fields(path.read_text())
    if profile == "pnnl":
        for phase in "ABC":
            prop = "measured_power_" + phase
            spec["load"].append(prop)
            text += f'object group_recorder {{ group "class=load"; property {prop}; file "load-{prop}.csv"; interval 1; strict true; print_units false; }}\n'
        outputs = tuple(
            f"{cls}-{prop}.csv" for cls, props in spec.items() for prop in props
        )
    (folder / "model.glm").write_text(text)
    run_glm(
        profile,
        folder,
        pnnl_root,
        outputs=outputs,
        execution_timeout=execution_timeout,
        open_files=65536,
    )
    rows = grouped(folder, spec)
    return canonical_snapshot(rows, objects, original, fields)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ["source", "bridge", "reference", "pnnl-root", "output"]:
        p.add_argument("--" + name, type=Path, required=True)
    p.add_argument("--case", choices=["bal", "unbal"], action="append")
    p.add_argument("--profile", choices=["pnnl", "arras"], action="append")
    p.add_argument(
        "--stage",
        choices=["fresh", "restored", "edited", "rebased", "grounded_reference"],
        action="append",
    )
    p.add_argument("--execution-timeout", type=float, default=120)
    args = p.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    report = {
        "engines": LOCK,
        "bridge_sha256": hashlib.sha256(args.bridge.read_bytes()).hexdigest(),
        "budgets": FEEDER_TOLERANCES,
        "cases": {},
    }
    reference = json.loads((args.reference / "report.json").read_text())
    if reference["engines"] != LOCK or not reference["passed"]:
        raise ValueError("reference engine profile or qualification mismatch")
    for case in args.case or ["bal", "unbal"]:
        source = args.source / f"ieee9500{case}_glm/glm/ieee9500{case}_run.glm"
        for suffix, digest in SOURCE_HASHES[case].items():
            data = (source.parent / f"ieee9500{case}_{suffix}.glm").read_bytes()
            if hashlib.sha256(data).hexdigest() != digest:
                raise ValueError("source pin mismatch")
        original = inventory(
            source.read_text()
            + (source.parent / f"ieee9500{case}_base.glm").read_text()
        )
        if reference["cases"][case]["source_sha256"] != SOURCE_HASHES[case]:
            raise ValueError("reference source pin mismatch")
        expected = json.loads((args.reference / case / "native.json").read_text())
        folder = args.output / case
        folder.mkdir()
        emitted = folder / "emitted"
        bridge = json.loads(
            command(
                [
                    str(args.bridge.resolve()),
                    str(source.resolve()),
                    str(emitted.resolve()),
                    "pnnl",
                ],
                folder,
            )
        )
        (folder / "bridge.json").write_text(json.dumps(bridge))
        for stage in args.stage or [
            "fresh",
            "restored",
            "edited",
            "rebased",
            "grounded_reference",
        ]:
            net = unwrap(bridge[stage]["network"])
            wanted = expected
            if stage == "edited":
                wanted, _ = imported_snapshot(
                    emitted / f"{stage}.dss/case.dss", net, original
                )
            repeat = folder / ("reimported-" + stage)
            repeated = json.loads(
                command(
                    [
                        str(args.bridge.resolve()),
                        str((emitted / f"{stage}.glm").resolve()),
                        str(repeat.resolve()),
                        "pnnl",
                    ],
                    folder,
                )
            )
            (repeat / "bridge.json").write_text(json.dumps(repeated))
            for profile in args.profile or ["pnnl", "arras"]:
                key = f"{case}/{stage}/{profile}"
                row = {"passed": False, "comparisons": {}, "current_balance": {}}
                try:
                    for variant, path in [
                        ("canonical", emitted / f"{stage}.glm"),
                        ("reimported", repeat / "fresh.glm"),
                    ]:
                        run = folder / stage / profile / variant
                        run.mkdir(parents=True)
                        actual = native(
                            path,
                            profile,
                            run,
                            original,
                            args.pnnl_root,
                            args.execution_timeout,
                        )
                        row["comparisons"][variant] = compare(
                            wanted, actual, tolerances=FEEDER_TOLERANCES
                        )
                        row["current_balance"][variant] = current_balance(
                            actual, net, original, LINKS
                        )
                        (run / "physical.json").write_text(json.dumps(actual))
                        if stage == "edited" and variant == "canonical":
                            row["unchanged_negative"] = compare(
                                expected, actual, tolerances=FEEDER_TOLERANCES
                            )
                    for variant, path, state in [
                        ("dss", emitted / f"{stage}.dss/case.dss", net),
                        (
                            "reimported-dss",
                            repeat / "fresh.dss/case.dss",
                            unwrap(repeated["fresh"]["network"]),
                        ),
                    ]:
                        if stage == "edited" and variant == "dss":
                            continue  # Reference solve is not an independent comparison.
                        actual, _ = imported_snapshot(path, state, original)
                        row["comparisons"][variant] = compare(
                            wanted, actual, tolerances=FEEDER_TOLERANCES
                        )
                    row["passed"] = (
                        all(c["passed"] for c in row["comparisons"].values())
                        and all(c["passed"] for c in row["current_balance"].values())
                        and (
                            stage != "edited" or not row["unchanged_negative"]["passed"]
                        )
                    )
                except Exception as error:
                    row["error"] = str(error)
                report["cases"][key] = row
                (args.output / "report.json").write_text(
                    json.dumps(report, indent=2) + "\n"
                )
                print(
                    key,
                    row["passed"],
                    row.get("error", ""),
                    {
                        k: c["failures"][:2]
                        for k, c in row["comparisons"].items()
                        if not c["passed"]
                    },
                    flush=True,
                )
    report["passed"] = all(c["passed"] for c in report["cases"].values())
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
