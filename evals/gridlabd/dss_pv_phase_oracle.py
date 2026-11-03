"""PV phase/return-conductor mappings against the pinned native engine."""

import argparse
import hashlib
import json
from pathlib import Path

from ieee4_import_oracle import evaluate


def circuits():
    for phases, conn, nodes, kv in (
        (1, "wye", "2.0", 7.199557856794634),
        (1, "delta", "3.1", 12.47),
        (2, "wye", "3.1.0", 12.47),
        (2, "delta", "3.1.2", 12.47),
        (3, "wye", "1.2.3.0", 12.47),
        (3, "delta", "3.1.2", 12.47),
    ):
        for pf in (1, 0.95):
            yield (
                f"{phases}ph-{conn}-pf{pf}",
                (
                    "clear\nnew circuit.c basekv=12.47 r1=.01 x1=.02 r0=.02 x0=.03\n"
                    "new line.line1 phases=3 bus1=sourcebus.1.2.3 bus2=end.1.2.3 "
                    "r1=.4 r0=.6 x1=.3 x0=.5 c1=0 c0=0 length=1 units=km\n"
                    f"new pvsystem.pv phases={phases} bus1=end.{nodes} conn={conn} "
                    f"kv={kv} kva=80 pmpp=50 irradiance=1 pf={pf}\n"
                    "set controlmode=off tolerance=1e-11 maxiterations=1000\nsolve\n"
                ),
            )
    yield (
        "split-phase-wye",
        (
            "clear\nnew circuit.c basekv=12.47 r1=.01 x1=.02 r0=.02 x0=.03\n"
            "new line.line1 phases=1 bus1=sourcebus.1 bus2=primary.1 r1=.4 r0=.4 x1=.3 x0=.3 c1=0 c0=0 length=1 units=km\n"
            "new transformer.t phases=1 windings=3 xhl=2 xht=2 xlt=1\n"
            "~ wdg=1 bus=primary.1.0 conn=wye kv=7.199557856794634 kva=50 %r=.2\n"
            "~ wdg=2 bus=end.1.0 conn=wye kv=.12 kva=25 %r=.2\n"
            "~ wdg=3 bus=end.0.2 conn=wye kv=.12 kva=25 %r=.2\n"
            "new pvsystem.pv phases=2 bus1=end.1.2.0 conn=wye kv=.20784609690826528 kva=20 pmpp=12 irradiance=1 pf=1\n"
            "new load.l phases=1 bus1=end.1.0 kv=.12 kw=4 kvar=1\n"
            "set controlmode=off tolerance=1e-11 maxiterations=1000\nsolve\n"
        ),
    )


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
    for name, text in circuits():
        folder = args.output / name
        folder.mkdir()
        source = folder / "input.dss"
        source.write_text(text)
        row = evaluate(source.resolve(), args.bridge.resolve(), folder.resolve(), 1e-11)
        row["input_sha256"] = hashlib.sha256(source.read_bytes()).hexdigest()
        report["cases"][name] = row
        print(name, "PASS" if row["passed"] else "FAIL", row["stage"], flush=True)
    report["passed"] = all(row["passed"] for row in report["cases"].values())
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    if not report["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
