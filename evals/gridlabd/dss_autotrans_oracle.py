"""Qualify static AutoTrans coil equivalents with the pinned native engine.

Synthetic cases use ordinary source/line impedances. The public IEEE cases
remain in the separate unmodified feeder suite, including strict failures.
"""

import argparse
import copy
import hashlib
import json
from pathlib import Path

from autotrans_ports import interfaces, mapping, pairs, select_bank
from ieee4_import_oracle import evaluate, run


def primitive_error(native, canonical, interface):
    native.Circuit.SetActiveElement("autotrans.t")
    count = interface["windings"]
    size = 6 * count
    expected = pairs(native.CktElement.YPrim())
    actual = [0j] * (size * size)
    for phase, bank in enumerate(interface["banks"]):
        select_bank(canonical, bank)
        values = pairs(canonical.CktElement.YPrim())
        indices = mapping(phase, count)
        for i, target_i in enumerate(indices):
            for j, target_j in enumerate(indices):
                actual[target_i + size * target_j] += values[i + 2 * count * j]
    return max(abs(a - b) for a, b in zip(expected, actual, strict=True))


def case(windings, permutation, losses, frequency):
    buses = ["high", "low", "tert"]
    if permutation:
        buses = ["high.2.3.1", "low.3.1.2", "tert.3.2.1"]
    text = (f"clear\nnew circuit.c basekv=12.47 basefreq={frequency} frequency={frequency} r1=.1 x1=.2 r0=.1 x0=.2\n"
            "new line.line1 bus1=sourcebus bus2=high phases=3 length=1 units=m "
            "r1=.05 x1=.1 r0=.15 x0=.2 c1=0 c0=0\n"
            f"new autotrans.t phases=3 windings={windings} xhx=7.23 xht=24.45 xxt=28.45 "
            f"%imag={.033 if losses else 0} %noloadloss={.024 if losses else 0} "
            f"ppm_antifloat={1 if losses or windings == 3 else 0}\n"
            f"~ wdg=1 bus={buses[0]} conn=s kv=12.47 kva=1000 %r=.0493\n"
            f"~ wdg=2 bus={buses[1]} conn=w kv=4.16 kva=1000 %r=.0556\n")
    if windings == 3:
        text += f"~ wdg=3 bus={buses[2]} conn=d kv=.48 kva=240 %r=1.4503\n"
        text += "new load.tertiary phases=3 bus1=tert conn=delta kv=.48 kw=10 kvar=2\n"
    text += ("new load.test phases=3 bus1=low conn=wye kv=4.16 kw=500 kvar=100\n"
             "new load.single phases=1 bus1=low.2.0 kv=2.401777119 kw=5 kvar=2\n"
             f"set frequency={frequency} voltagebases=[12.47 4.16 .48]\n"
             "calcvoltagebases\nset controlmode=off tolerance=1e-11\nsolve\n")
    return text


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bridge", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    rows = {}
    for windings in (2, 3):
        for permute, losses, frequency in ((False, True, 60), (True, True, 60),
                                          (False, False, 60), (True, True, 50)):
            name = f"w{windings}-permuted{permute}-losses{losses}-f{frequency}"
            folder = args.output / name
            folder.mkdir()
            source = folder / "native.dss"
            source.write_text(case(windings, permute, losses, frequency))
            row = evaluate(source, args.bridge.resolve(), folder, 1e-11)
            network = json.loads((folder / "imported/bridge.json").read_text())["network"]
            native = run(source, 1e-11)
            port = interfaces(native, network)["autotrans.t"]
            canonical = run(folder / "imported/dss/case.dss", 1e-11)
            error = primitive_error(native, canonical, port)
            row["primitive_max_error_S"] = error
            row["primitive_passed"] = error < 1e-12
            controls = {}
            for change in ("contact_node", "coil_bus", "missing_bank"):
                wrong = copy.deepcopy(network)
                bank = next(t for t in wrong["transformers"]
                            if "dss_autotrans_original" in t.get("extras", {}))
                if change == "contact_node":
                    name_contact = bank["extras"]["dss_autotrans_contacts"][0]
                    contact = next(s for s in wrong["switches"] if s["name"] == name_contact)
                    contact["terminal_map_from"][0] = "77"
                elif change == "coil_bus":
                    bank["windings"][1]["bus"] = "high"
                else:
                    missing = bank["extras"]["dss_autotrans_banks"][1]
                    wrong["transformers"] = [t for t in wrong["transformers"] if t["name"] != missing]
                try:
                    interfaces(native, wrong)
                    controls[change] = False
                except (ValueError, KeyError):
                    controls[change] = True
            row["mapping_negative_controls"] = controls
            row["passed"] &= row["primitive_passed"] and all(controls.values())
            rows[name] = row
            print(name, row["passed"], row.get("comparison", {}).get("max_absolute_error"))
    report = dict(cases=rows, passed=all(row["passed"] for row in rows.values()),
                  bridge_sha256=hashlib.sha256(args.bridge.read_bytes()).hexdigest())
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    if not report["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
