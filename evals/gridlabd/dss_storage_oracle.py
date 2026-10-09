"""Idle storage laws through native voltage sweeps, IR and canonical DSS.

The ignored Rust storage-current test consumes report.json to independently
compare typed voltage-law powers with the native terminal powers.
"""

import argparse
import hashlib
import json
import math
from pathlib import Path

from european_lv_import_oracle import pairs
from ieee4_import_oracle import evaluate, run


def circuits():
    for phases, conn, nodes, kv in (
        (1, "wye", "2.0", 0.48 / math.sqrt(3)),
        (2, "wye", "3.1.0", 0.48),
        (2, "delta", "3.1.2", 0.48),
        (3, "wye", "1.2.3.0", 0.48),
        (3, "delta", "3.1.2", 0.48),
    ):
        for limited in ("yes", "no"):
            for pu in (0.45, 0.899, 0.901, 1.0, 1.101, 1.344, 1.5):
                yield (
                    f"{phases}ph-{conn}-{limited}-{pu}",
                    phases,
                    conn,
                    pu,
                    f"phases={phases} conn={conn} bus1=end.{nodes} kv={kv:.17g} "
                    f"kwrated=250 kva=250 %idlingkw=1 state=idling pf=-.95 "
                    f"vminpu=.9 vmaxpu=1.1 limitcurrent={limited}",
                )
    for name, props in (
        ("kvar", "phases=3 kv=.48 kwrated=100 kva=100 kvar=2"),
        ("reactive-last", "phases=3 kv=.48 kwrated=100 kva=100 kvar=2 pf=.95"),
        ("rating-history", "phases=3 kv=.48 kwrated=100 kva=100 kwrated=250"),
        ("phase-after-voltage", "kv=.48 phases=1"),
        ("constructor-voltage", "phases=1"),
        ("load-name-collision", "phases=3 kv=.48\nnew load.s bus1=end.1.2.3.0 kv=.48 kw=1 kvar=0"),
    ):
        phases = 1 if "phases=1" in props else 3
        yield name, phases, "wye", 15.0 if name == "constructor-voltage" else 1.0, f"bus1=end.1.2.3.0 {props}"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bridge", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    report = dict(
        scope=__doc__,
        bridge_sha256=hashlib.sha256(args.bridge.read_bytes()).hexdigest(),
        cases={},
    )
    for name, phases, conn, pu, props in circuits():
        folder = args.output / name
        folder.mkdir()
        source = folder / "input.dss"
        source.write_text(
            f"clear\nnew circuit.c basekv=.48 pu={pu} r1=.001 x1=.001 r0=.001 x0=.001\n"
            "new line.line1 phases=3 bus1=sourcebus.1.2.3 bus2=end.1.2.3 "
            "r1=.001 r0=.002 x1=.001 x0=.002 c1=0 c0=0 length=1 units=km\n"
            f"new storage.s {props}\n"
            "set controlmode=off tolerance=1e-12 maxiterations=1000\nsolve\n"
        )
        row = evaluate(source.resolve(), args.bridge.resolve(), folder.resolve(), 1e-12)
        row["input_sha256"] = hashlib.sha256(source.read_bytes()).hexdigest()
        native = run(source, 1e-12)
        native.Circuit.SetActiveElement("storage.s")
        volts = pairs(native.CktElement.Voltages())
        branches = [
            volts[i] - (volts[-1] if conn == "wye" else volts[(i + 1) % len(volts)])
            for i in range(phases)
        ]
        power = sum(pairs(native.CktElement.Powers())) * 1000
        row["typed_sample"] = dict(
            ir=str((folder / "imported/network.pio.json").resolve()),
            branch_voltage=[[v.real, v.imag] for v in branches],
            total_power=[power.real, power.imag],
        )
        report["cases"][name] = row
        print(name, "PASS" if row["passed"] else "FAIL", row.get("error", ""), flush=True)
    report["passed"] = all(row["passed"] for row in report["cases"].values())
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    if not report["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
