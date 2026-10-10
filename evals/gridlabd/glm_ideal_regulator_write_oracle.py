"""Canonical ideal FBS regulator banks, physical ports and repeated output."""
import argparse
import hashlib
import json
import re
from pathlib import Path

import numpy as np
from oracle import LOCK, add, command, compare, recorded, run_glm
from regulator_oracle import CASES, deck, expected
from transformer_roundtrip_oracle import header

# Wide manual ratios exercise the native Type A output branch.
CASES["type_a_wide_ratio"] = ("A", "ABC", [160, 320, 480], 0.0, False)


def add_open_tie(text, phases, direction):
    a, b = ("source", "sink") if direction == "forward" else ("sink", "source")
    return text + f'\nobject node {{ name tie_from; parent {a}; phases {phases}N; nominal_voltage 240; }}\nobject node {{ name tie_to; parent {b}; phases {phases}N; nominal_voltage 240; }}\nobject switch {{ name tie; from tie_from; to tie_to; phases {phases}N; status OPEN; switch_impedance 0+0j; latitude -27; longitude 153; }}\n'


def expected_tie(snapshot, phases, direction):
    for p in phases:
        for end in ("from", "to"):
            bus = ("source" if end == "from" else "sink") if direction == "forward" else ("sink" if end == "from" else "source")
            snapshot[f"tie.{end}.{p}.voltage"] = snapshot[f"{bus}.{p}"]
            for quantity, unit in (("current", "A"), ("power", "VA")):
                add(snapshot, f"tie.{end}.{p}.{quantity}", unit, 0j)
    add(snapshot, "tie.loss", "VA", 0j)
    return snapshot


def outputs(objects, tie):
    return ("source.csv", "sink.csv", *(f"reg{i}.csv" for i in range(len(objects))), *(("tie.csv", "alias_from.csv", "alias_to.csv") if tie else ()))


def instrument(text, phases, clock=True, tie=False):
    objects = []
    for body in re.findall(r"object regulator\s*\{([^}]+)\}", text):
        values = dict(re.findall(r'(name|from|to)\s+"?([^";\s]+)"?\s*;', body))
        objects.append(values)
    props = ",".join(f"voltage_{p}" for p in phases)
    text = (header() if clock else "") + text + "\nmodule tape;\n"
    for bus in ("source", "sink"):
        text += f'object recorder {{ parent {bus}; property {props}; interval 1; file "{bus}.csv"; }}\n'
    props = ",".join(f"{q}_{end}_{p}" for p in phases for end in ("in", "out") for q in ("current", "power")) + ",power_losses"
    for index, obj in enumerate(objects):
        text += f'object recorder {{ parent "{obj["name"]}"; property {props}; interval 1; file "reg{index}.csv"; }}\n'
    if tie:
        body = re.search(r"object switch\s*\{([^}]+)\}", text)[1]
        ends = dict(re.findall(r'(from|to)\s+"?([^";\s]+)"?\s*;', body))
        props = ",".join(f"{q}_{end}_{p}" for p in phases for end in ("in", "out") for q in ("current", "power")) + ",power_losses"
        text += f'object recorder {{ parent tie; property {props}; interval 1; file "tie.csv"; }}\n'
        for side, parent in ends.items():
            props = ",".join(f"voltage_{p}" for p in phases)
            text += f'object recorder {{ parent "{parent}"; property {props}; interval 1; file "alias_{side}.csv"; }}\n'
    return text, objects


def native(folder, phases, objects, tie=False):
    result = {}
    for bus in ("source", "sink"):
        row = recorded(folder / f"{bus}.csv")
        for p in phases:
            add(result, f"{bus}.{p}", "V", row[f"voltage_{p}"])
    ports = {(bus, p, q): 0j for bus in ("source", "sink") for p in phases for q in ("current", "power")}
    loss = 0j
    for index, obj in enumerate(objects):
        row = recorded(folder / f"reg{index}.csv")
        loss += row["power_losses"]
        for side, end, sign in (("from", "in", 1), ("to", "out", -1)):
            if obj[side] in ("source", "sink"):
                for p in phases:
                    for q in ("current", "power"):
                        ports[obj[side], p, q] += sign * row[f"{q}_{end}_{p}"]
    for (bus, p, q), value in ports.items():
        add(result, f"{'from' if bus == 'source' else 'to'}.{p}.{q}", "A" if q == "current" else "VA", value)
    add(result, "loss", "VA", loss)
    if tie:
        row = recorded(folder / "tie.csv")
        for side, end, sign in (("from", "in", 1), ("to", "out", -1)):
            alias = recorded(folder / f"alias_{side}.csv")
            for p in phases:
                add(result, f"tie.{side}.{p}.voltage", "V", alias[f"voltage_{p}"])
                for q, unit in (("current", "A"), ("power", "VA")):
                    add(result, f"tie.{side}.{p}.{q}", unit, sign * row[f"{q}_{end}_{p}"])
        add(result, "tie.loss", "VA", row["power_losses"])
    return result


def dss_snapshot(path, phases, direction=None):
    import opendssdirect as dss
    e = dss.NewContext()
    e.Basic.AllowChangeDir(False)
    if LOCK["dss"]["version"] not in e.Basic.Version():
        raise ValueError("OpenDSS pin mismatch")
    e.Text.Command(f'redirect "{path}"')
    e.Text.Command("set tolerance=1e-12 maxiterations=100 controlmode=off")
    e.Solution.Solve()
    if not e.Solution.Converged() or e.Solution.ControlMode() != -1:
        raise ValueError("OpenDSS failed to converge with controls disabled")
    def unpack(raw):
        return np.array(raw[::2]) + 1j * np.array(raw[1::2])
    result = {}
    for bus in ("source", "sink"):
        e.Circuit.SetActiveBus(bus)
        voltages = dict(zip(e.Bus.Nodes(), unpack(e.Bus.Voltages()), strict=True))
        for p in phases:
            add(result, f"{bus}.{p}", "V", voltages["ABC".index(p) + 1])
    ports = {(bus, p, q): 0j for bus in ("source", "sink") for p in phases for q in ("current", "power")}
    for name in e.Circuit.AllElementNames():
        cls = name.split(".")[0].lower()
        if cls != "transformer" and not name.lower().startswith("line._pio_xfmr_"):
            continue
        e.Circuit.SetActiveElement(name)
        nodes = e.CktElement.NodeOrder()
        currents = unpack(e.CktElement.Currents())
        powers = 1000 * unpack(e.CktElement.Powers())
        nc = e.CktElement.NumConductors()
        for terminal, raw_bus in enumerate(e.CktElement.BusNames()):
            bus = raw_bus.split(".")[0].lower()
            if bus not in ("source", "sink"):
                continue
            for wire in range(nc):
                index = terminal * nc + wire
                if nodes[index] not in (1, 2, 3):
                    continue
                p = "ABC"[nodes[index] - 1]
                if p in phases:
                    ports[bus, p, "current"] += currents[index]
                    ports[bus, p, "power"] += powers[index]
    for (bus, p, q), value in ports.items():
        add(result, f"{'from' if bus == 'source' else 'to'}.{p}.{q}", "A" if q == "current" else "VA", value)
    add(result, "loss", "VA", sum(value for (_, _, q), value in ports.items() if q == "power"))
    if direction:
        if e.Circuit.SetActiveElement("line.tie") < 0 or e.CktElement.Enabled():
            raise ValueError("open tie is missing or enabled")
        expected_tie(result, phases, direction)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bridge", type=Path, required=True)
    parser.add_argument("--pnnl-root", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--profile", action="append", choices=("pnnl", "arras"))
    parser.add_argument("--case", action="append")
    parser.add_argument("--open-tie", choices=("forward", "reverse"), help="Include an unloaded alias reference for a physical open tie.")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    report = dict(engines=LOCK, bridge_sha256=hashlib.sha256(args.bridge.read_bytes()).hexdigest(), cases={})
    for label in list(CASES):
        if args.case and label not in args.case:
            continue
        kind, phases, taps, _, override = CASES[label]
        CASES[label] = (kind, phases, taps, 0.0, override)
        # The nonzero declaration is intentionally unused by native FBS.
        original = deck(label).replace("solver_method NR", "solver_method FBS").replace("regulator_resistance 0.0;", "regulator_resistance 7.3;")
        reference_text = add_open_tie(original, phases, args.open_tie) if args.open_tie else original
        for profile in args.profile or ("pnnl", "arras"):
            folder = args.output / label / profile
            reference = folder / "reference"
            reference.mkdir(parents=True)
            text, objects = instrument(reference_text, phases, clock=False, tie=bool(args.open_tie))
            source = reference / "model.glm"
            source.write_text(text)
            row = dict(passed=False, input_sha256=hashlib.sha256(text.encode()).hexdigest(), comparisons={})
            try:
                run_glm(profile, reference, args.pnnl_root, outputs=outputs(objects, args.open_tie))
                baseline = native(reference, phases, objects, args.open_tie)
                row["comparisons"]["analytic/reference"] = compare(expected_tie(expected(label), phases, args.open_tie) if args.open_tie else expected(label), baseline)
                emitted = folder / "emitted"
                data = json.loads(command([str(args.bridge.resolve()), str(source.resolve()), str(emitted.resolve()), profile], folder))
                (folder / "bridge.json").write_text(json.dumps(data, indent=2) + "\n")
                for stage in ("fresh", "restored", "edited", "rebased", "grounded_reference"):
                    wanted = expected(label, edited=stage == "edited")
                    if args.open_tie:
                        expected_tie(wanted, phases, args.open_tie)
                    repeated = folder / ("reimported-" + stage)
                    path = emitted / f"{stage}.glm"
                    command([str(args.bridge.resolve()), str(path.resolve()), str(repeated.resolve()), profile], folder)
                    for variant, case in (("canonical", path), ("reimported", repeated / "fresh.glm")):
                        run = folder / stage / variant
                        run.mkdir(parents=True)
                        text, objects = instrument(case.read_text(), phases, tie=bool(args.open_tie))
                        (run / "model.glm").write_text(text)
                        run_glm(profile, run, args.pnnl_root, outputs=outputs(objects, args.open_tie))
                        actual = native(run, phases, objects, args.open_tie)
                        if stage == "edited" and variant == "canonical":
                            edited_actual = actual
                        row["comparisons"][f"{stage}/{variant}"] = compare(wanted, actual)
                        (run / "measurements.json").write_text(json.dumps(dict(expected=wanted, actual=actual), indent=2) + "\n")
                    row["comparisons"][f"{stage}/dss"] = compare(wanted, dss_snapshot(emitted / f"{stage}.dss/case.dss", phases, args.open_tie))
                    row["comparisons"][f"{stage}/reimported-dss"] = compare(wanted, dss_snapshot(repeated / "fresh.dss/case.dss", phases, args.open_tie))
                row["wrong_ratio_negative"] = compare(expected_tie(expected(label, negative=True), phases, args.open_tie) if args.open_tie else expected(label, negative=True), baseline)
                row["unchanged_negative"] = compare(baseline, edited_actual)
                row["passed"] = all(c["passed"] for c in row["comparisons"].values()) and not row["wrong_ratio_negative"]["passed"] and not row["unchanged_negative"]["passed"]
            except Exception as error:
                row["error"] = str(error)
            report["cases"][f"{label}/{profile}"] = row
            (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
            print(label, profile, row["passed"], row.get("error", ""), {k: c["failures"][:2] for k, c in row["comparisons"].items() if not c["passed"]}, flush=True)
    report["passed"] = bool(report["cases"]) and all(c["passed"] for c in report["cases"].values())
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
