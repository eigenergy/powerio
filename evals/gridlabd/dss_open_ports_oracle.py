"""Fixed-open Line equivalents retain every native external port observation."""

import argparse
import copy
import hashlib
import json
from pathlib import Path

from ieee4_import_oracle import evaluate, run
from physical_ports import observe


def cases():
    for count, nodes, load, suffix in (
        (1, "1", "phases=1 bus1=end.1.0 kv=7.199557856794634 kw=30 kvar=8", "single"),
        (3, "3.1.2", "phases=3 bus1=end.1.2.3.0 kv=12.47 kw=90 kvar=24", "permuted"),
        (4, "3.1.2.0", "phases=3 bus1=end.1.2.3.4 kv=12.47 kw=90 kvar=24", "neutral"),
    ):
        dest = "3.1.2.4" if count == 4 else nodes
        for states in (
            "open line.sw 1 1",
            "open line.sw 2 0",
            "open line.sw 1 0\nopen line.sw 2 0",
        ):
            index = {"open line.sw 1 1": "one", "open line.sw 2 0": "end"}.get(
                states, "both"
            )
            text = (
                "clear\nnew circuit.c basekv=12.47 r1=.1 x1=.2 r0=.2 x0=.3\n"
                f"new line.line1 phases={count} bus1=sourcebus.{nodes} bus2=end.{dest} r1=.4 r0=.4 x1=.3 x0=.3 c1=0 c0=0 length=1 units=km\n"
                f"new line.sw phases={count} bus1=sourcebus.{nodes} bus2=end.{dest} r1=2 r0=3 x1=.5 x0=.8 c1=3 c0=1 length=1 units=km\n"
                # Force generated-name collision handling; this is source metadata.
                "new load._pio_open_line_sw_from phases=1 bus1=end.1.0 kv=7.199557856794634 kw=.1 kvar=0\n"
                f"new load.l {load}\n{states}\n"
                "set controlmode=off tolerance=1e-12 maxiterations=1000\nsolve\n"
            )
            yield f"{suffix}-{index}", text
    yield (
        "reactor-return-permutation",
        (
            "clear\nnew circuit.c basekv=12.47 r1=.1 x1=.2 r0=.2 x0=.3\n"
            "new line.line1 phases=3 bus1=sourcebus.1.2.3 bus2=end.1.2.3 r1=.4 r0=.6 x1=.3 x0=.5 c1=0 c0=0 length=1 units=km\n"
            "new reactor.r phases=3 bus1=end.1.2.3 bus2=end.0 r=3 x=4\n"
            "new load.l phases=3 bus1=end.1.2.3.0 kv=12.47 kw=90 kvar=24\n"
            "set controlmode=off tolerance=1e-12 maxiterations=1000\nsolve\n"
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
    for name, text in cases():
        folder = args.output / name
        folder.mkdir()
        source = folder / "input.dss"
        source.write_text(text)
        row = evaluate(source.resolve(), args.bridge.resolve(), folder.resolve(), 1e-12)
        if row["passed"]:
            engine = run(folder / "imported/dss/case.dss", 1e-12)
            ports = next(iter(row["physical_port_maps"].values()))
            rejected = {}
            for label in ("bus", "nodes", "element"):
                wrong = copy.deepcopy(ports)
                if label == "nodes":
                    wrong[0]["nodes"] = wrong[0]["nodes"][:-1]
                else:
                    wrong[0][label] = "_missing_interface_"
                try:
                    observe(engine, wrong)
                except ValueError:
                    rejected[label] = True
                else:
                    rejected[label] = False
            row["mapping_negative_controls"] = rejected
            row["passed"] = all(rejected.values())
        row["input_sha256"] = hashlib.sha256(source.read_bytes()).hexdigest()
        report["cases"][name] = row
        print(
            name, "PASS" if row["passed"] else "FAIL", row.get("error", ""), flush=True
        )
    report["passed"] = all(row["passed"] for row in report["cases"].values())
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    if not report["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
