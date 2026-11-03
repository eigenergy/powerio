"""Native Thevenin emission, fallback circuits and source-port identity checks."""
import argparse
import copy
import hashlib
import json
import re
from pathlib import Path

from european_lv_import_oracle import node_inventory, snapshot, source_inventory
from ieee4_import_oracle import evaluate, run
from oracle import command, compare


def cases():
    for frequency in (50, 60):
        for sequence in ("positive", "negative", "zero"):
            for angle in (0, 17):
                name = f"{frequency}-{sequence}-{angle}"
                yield name, frequency, f"angle={angle} sequence={sequence} r1=1 x1=2 r0=3 x0=4", "", True
    for frequency in (50, 60):
        for angle in (0, 17):
            name = "mva-ieee4" if (frequency, angle) == (60, 0) else f"mva-{frequency}-{angle}"
            yield name, frequency, f"angle={angle} mvasc3=200000 mvasc1=200000 x1r1=4 x0r0=3", "", True
    yield "permuted", 60, "bus1=feed.3.1.2 r1=1 x1=2 r0=3 x0=4", "", True
    yield "ieee-secondary", 60, "r1=1.036 x1=10.1433 r0=5.462 x0=34.1897", "", True
    yield "mva", 60, "mvasc3=25 mvasc1=30 x1r1=5 x0r0=4", "", None
    yield "unequal-sequences", 60, "z1=[1 2] z2=[2 3] z0=[4 5]", "", False
    for return_bus in ("return.4.4.4", "return.1.2.3"):
        anchors = "\n".join(f"new line.return{node} phases=1 bus1=return.{node} bus2=return.0 r1=5 x1=0 r0=5 x0=0 c1=0 c0=0" for node in sorted(set(return_bus.split(".")[1:])))
        yield "reference-" + return_bus, 60, "r1=1 x1=2 r0=3 x0=4 bus2=" + return_bus, anchors, False


def edited_cases(root, bridge, powerio):
    rows = {}
    for name in ("edited-voltage-resistance", "edited-source-length", "edited-mva-voltage"):
        base = root / ("mva-ieee4" if name == "edited-mva-voltage" else "60-positive-0")
        original_ir = json.loads((base / "imported/network.pio.json").read_text())
        original_text = (base / "input.dss").read_text()
        original_observations = json.loads((base / "native.json").read_text())
        folder = root / name
        folder.mkdir()
        ir = copy.deepcopy(original_ir)
        network = ir["value"]["data"]
        if name == "edited-voltage-resistance":
            for source in network["sources"]:
                source["v_magnitude"] = [value * 1.125 for value in source["v_magnitude"]]
            code = next(c for c in network["linecodes"] if c["name"] == "_pio_vsource_source_impedance")
            code["r_series"] = [[value * 2 for value in row] for row in code["r_series"]]
            reference = original_text.replace("basekv=12.47", f"basekv={12.47 * 1.125:.17g}").replace("r1=1 x1=2 r0=3 x0=4", "r1=2 x1=2 r0=6 x0=4")
        elif name == "edited-mva-voltage":
            for source in network["sources"]:
                source["v_magnitude"] = [value * 2 for value in source["v_magnitude"]]
            reference = original_text.replace("basekv=12.47", "basekv=24.94").replace("mvasc3=200000 mvasc1=200000", "mvasc3=800000 mvasc1=800000")
        else:
            line = next(line for line in network["lines"] if line["name"] == "_pio_vsource_source_impedance")
            line["length"] *= 2
            reference = original_text.replace("r1=1 x1=2 r0=3 x0=4", "r1=2 x1=4 r0=6 x0=8")
        edited_ir = folder / "edited.pio.json"
        edited_ir.write_text(json.dumps(ir) + "\n")
        canonical = folder / "edited.dss"
        command([str(powerio.resolve()), "convert", str(edited_ir), "--to", "opendss", "-o", str(canonical)], folder)
        native_path = folder / "reference.dss"
        native_path.write_text(reference)
        native = run(native_path, 1e-11)
        buses = [bus.lower() for bus in native.Circuit.AllBusNames()]
        elements = ["line.line1", "load.a", "load.b", "load.c"]
        expected = snapshot(native, buses, elements, canonical=False)
        kwargs = dict(canonical=True, bus_nodes=node_inventory(native, buses), source_port=source_inventory(native))
        actual = snapshot(run(canonical, 1e-11), buses, elements, **kwargs)
        reimported = folder / "reimported"
        command([str(bridge.resolve()), str(canonical), str(reimported)], folder)
        again = snapshot(run(reimported / "dss/case.dss", 1e-11), buses, elements, **kwargs)
        first, repeated, unedited = (compare(expected, values) for values in (actual, again, original_observations))
        native_emission = "_pio_ideal_" not in canonical.read_text()
        row = dict(comparison=first, reimport_comparison=repeated, unedited_negative=unedited,
                   native_thevenin_emission=native_emission,
                   passed=first["passed"] and repeated["passed"] and not unedited["passed"] and native_emission)
        for label, values in (("native", expected), ("canonical", actual), ("reimported", again)):
            (folder / f"{label}.json").write_text(json.dumps(values, indent=2) + "\n")
        rows[name] = row
        print(name, "PASS" if row["passed"] else "FAIL", flush=True)
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bridge", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--powerio", type=Path, required=True)
    args = parser.parse_args()
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    rows = {}
    for name, frequency, source, anchors, native_expected in cases():
        folder = root / name
        folder.mkdir()
        path = folder / "input.dss"
        path.write_text(f"clear\nset defaultbasefrequency={frequency}\nnew circuit.c phases=3 bus1=feed basekv=12.47 frequency={frequency} {source}\n"
                        "new line.line1 phases=3 bus1=feed bus2=end r1=.1 x1=.2 r0=.15 x0=.25 c1=0 c0=0 length=1 units=m\n"
                        "new load.a phases=1 bus1=end.1.0 kv=7.199557856794634 kw=3 kvar=1\n"
                        "new load.b phases=1 bus1=end.2.0 kv=7.199557856794634 kw=2 kvar=.5\n"
                        "new load.c phases=1 bus1=end.3.0 kv=7.199557856794634 kw=1 kvar=.25\n"
                        + anchors + f"\nset frequency={frequency} controlmode=off\nsolve\n")
        row = evaluate(path, args.bridge.resolve(), folder, 1e-11)
        canonical = folder / "imported/dss/case.dss"
        if canonical.exists():
            text = canonical.read_text()
            native = "_pio_ideal_" not in text
            row["native_thevenin_emission"] = native
            if native_expected is not None:
                row["passed"] &= native == native_expected
            if native:
                # A moved source can still solve, but its external bus/conductor
                # identity must not be confused with the original feeder port.
                wrong = re.sub(r"(?im)^(New Circuit\.[^\n]*?bus1=)feed", r"\1end", text, count=1)
                wrong_path = canonical.with_name("wrong-source-port.dss")
                wrong_path.write_text(wrong)
                engine = run(wrong_path, 1e-11)
                expected = json.loads((folder / "native.json").read_text())
                buses = ["feed", "end"]
                elements = ["line.line1", "load.a", "load.b", "load.c"]
                actual = snapshot(engine, buses, elements, canonical=True)
                negative = compare(expected, actual)
                missing = sorted(key for key in expected.keys() - actual.keys() if key.startswith("source.feeder."))
                row["wrong_source_port_negative"] = dict(passed=not negative["passed"] and len(missing) == 6, missing_original_ports=missing)
                row["passed"] &= row["wrong_source_port_negative"]["passed"]
        rows[name] = row
        print(name, "PASS" if row["passed"] else "FAIL", row.get("error", ""), flush=True)
    rows.update(edited_cases(root, args.bridge, args.powerio))
    report = dict(powerio_sha256=hashlib.sha256(args.powerio.read_bytes()).hexdigest(), cases=rows, passed=all(row["passed"] for row in rows.values()), bridge_sha256=hashlib.sha256(args.bridge.read_bytes()).hexdigest())
    (root / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    if not report["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
