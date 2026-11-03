"""Fixed-slip motor law, state ordering, and edited typed matrices against IndMach012a.

Run prepare first, export typed edits using the ignored Rust integration test,
then run check-edits. All data and generated circuits stay in the output folder.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path

from european_lv_import_oracle import node_inventory, pairs, snapshot, source_inventory
from ieee4_import_oracle import evaluate, run
from oracle import LOCK, compare
from physical_ports import interfaces

STATES = {
    "public": "userdata=(rs=.03359375 xs=.07999132 rr=.03940972 xr=.07999132 xm=2.1 slip=.035 option=fixed)",
    "defaults": "userdata=(option=fixed)",
    "zero": "userdata=(slip=0 option=fixed)",
    "negative": "userdata=(slip=-.035 option=fixed)",
    "clamp_first": "userdata=(slip=.2 maxslip=.3 option=fixed)",
    "limit_first": "userdata=(maxslip=.3 slip=.2 option=fixed)",
    "split_edit": "userdata=(rs=.03)\nEdit Generator.m userdata=(slip=.04 option=fixed)",
    "reset": "userdata=(rs=.9 slip=.05 option=fixed)\nEdit Generator.m usermodel=IndMach012a userdata=(option=fixed)",
    "late_base": "userdata=(slip=.035 option=fixed)\nEdit Generator.m kv=.25 kva=25",
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("prepare", "check-edits"))
    parser.add_argument("--bridge", type=Path, required=True)
    parser.add_argument("--indmach-manifest", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    manifest = json.loads(args.indmach_manifest.read_text())
    library = Path(manifest["library"])
    if (manifest["commit"] != LOCK["indmach012a"]["commit"]
            or manifest["engine_version"] != LOCK["dss"]["version"]
            or hashlib.sha256(library.read_bytes()).hexdigest() != manifest["library_sha256"]
            or str(library.parent) != os.environ.get("DYLD_LIBRARY_PATH", "").split(os.pathsep)[0]
            or (library.parent / "IndMach012a").resolve() != library.resolve()):
        raise ValueError("motor library pin mismatch")
    if args.mode == "prepare":
        args.output.mkdir(exist_ok=False, parents=True)
        report = dict(manifest=manifest, bridge_sha256=hashlib.sha256(args.bridge.read_bytes()).hexdigest(), cases={})
        for state, tail in STATES.items():
            for pu in (0.9, 1.0, 1.1):
                name = f"{state}-{pu}"
                folder = args.output / name
                folder.mkdir()
                source = folder / "native.dss"
                source.write_text(
                    f"New Circuit.c basekv=.24 pu={pu} mvasc3=100 mvasc1=100\n"
                    "New Line.feed phases=3 bus1=sourcebus bus2=loadbus length=1 units=m rmatrix=[.003 | .0003 .004 | .0002 .0001 .005] xmatrix=[.004 | .001 .006 | .0005 .0008 .008] cmatrix=[0 | 0 0 | 0 0 0]\n"
                    f"New Generator.m bus1=loadbus kv=.24 kva=25 kw=-18.65 conn=delta model=6 usermodel=IndMach012a {tail}\n"
                    "New Load.unbalance phases=1 bus1=loadbus.1.0 kv=.1385640646055102 kw=.4 kvar=.15\n"
                    "Set voltagebases=[.24]\nCalcvoltagebases\n"
                )
                row = evaluate(source.resolve(), args.bridge.resolve(), folder.resolve(), 1e-11)
                native = run(source, 1e-11)
                native.Circuit.SetActiveElement("generator.m")
                network = json.loads((folder / "imported/bridge.json").read_text())["network"]
                shunt = network["shunts"][0]
                voltages = pairs(native.CktElement.Voltages())
                currents = pairs(native.CktElement.Currents())
                actual = [sum(complex(g, b) * v for g, b, v in zip(gr, br, voltages, strict=True))
                          for gr, br in zip(shunt["g"], shunt["b"], strict=True)]
                errors = [abs(a - b) for a, b in zip(currents, actual, strict=True)]
                row["typed_current_law"] = dict(passed=all(e <= 1e-7 + 1e-9 * abs(i) for e, i in zip(errors, currents, strict=True)), max_error_A=max(errors))
                row["passed"] &= row["typed_current_law"]["passed"]
                report["cases"][name] = row
                print(name, row["passed"], row.get("error", ""), flush=True)
        (args.output / "cases.json").write_text(json.dumps(list(report["cases"]), indent=2) + "\n")
        report["passed"] = all(row["passed"] for row in report["cases"].values())
        (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    else:
        report = dict(manifest=manifest, cases={})
        for name in json.loads((args.output / "cases.json").read_text()):
            folder = args.output / name
            source = folder / "native-edited.dss"
            source.write_text((folder / "native.dss").read_text() + "\nEdit Generator.m kva=29.25\n")
            native = run(source, 1e-11)
            network = json.loads((folder / "imported/bridge.json").read_text())["network"]
            # A uniform 1.17 admittance edit preserves the declared subcircuit
            # boundary and corresponds to the independent native kVA-base edit.
            ports = interfaces(native, network, {})
            buses = [b.lower() for b in native.Circuit.AllBusNames()]
            elements = ["line.feed", "generator.m", "load.unbalance"]
            expected = snapshot(native, buses, elements, canonical=False)
            candidates = list((folder / "typed-edited").glob("*.dss"))
            if len(candidates) != 1:
                raise ValueError(f"missing edited typed export: {name}")
            actual = snapshot(run(candidates[0], 1e-11), buses, elements, canonical=True,
                              port_maps=ports, bus_nodes=node_inventory(native, buses), source_port=source_inventory(native))
            result = compare(expected, actual)
            # The typed edit must also differ from the original native circuit.
            unchanged = snapshot(run(folder / "native.dss", 1e-11), buses, elements, canonical=False)
            negative = compare(unchanged, actual)
            report["cases"][name] = dict(passed=result["passed"] and not negative["passed"], comparison=result, unchanged_negative=negative)
            print(name, report["cases"][name]["passed"], flush=True)
        report["passed"] = all(row["passed"] for row in report["cases"].values())
        (args.output / "edited-report.json").write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
