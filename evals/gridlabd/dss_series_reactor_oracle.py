"""Scalar series-reactor physical ports, canonical output and IR against OpenDSS."""

import argparse
import hashlib
import json
from pathlib import Path

from ieee4_import_oracle import evaluate, run


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
    for phases, nodes in ((1, "2"), (2, "3.1"), (3, "1.2.3")):
        for resistance, base_frequency, ratings in (
            (0, 60, "explicit"),
            (0.3, 60, "explicit"),
            (0.3, 50, "explicit"),
            (0.3, 60, "default"),
            (0.3, 60, "earlier_edit"),
        ):
            name = f"{phases}ph-r{resistance}-base{base_frequency}-{ratings}"
            folder = args.output / name
            folder.mkdir()
            source = folder / "input.dss"
            source.write_text(
                "clear\nnew circuit.c basekv=12.47 r1=.01 x1=.02 r0=.02 x0=.03\n"
                f"new reactor.series phases={phases} bus1=sourcebus.{nodes} bus2=middle.{nodes} "
                f"r={resistance} x=14.798307 basefreq={base_frequency} normamps=400 emergamps=500\n"
                f"new line.line1 phases={phases} bus1=middle.{nodes} bus2=end.{nodes} "
                "length=1 units=m r1=.01 r0=.01 x1=.02 x0=.02 c1=0 c0=0\n"
                + "\n".join(
                    f"new load.l{i} phases=1 bus1=end.{node}.0 kv=7.199557856794634 kw={40 + 15 * i} kvar={20 + 5 * i}"
                    for i, node in enumerate(nodes.split("."))
                )
                + "\nset controlmode=off tolerance=1e-11 maxiterations=1000\nsolve\n"
            )
            if ratings != "explicit":
                text = source.read_text().replace(" normamps=400 emergamps=500", "")
                if ratings == "earlier_edit":
                    text = text.replace(f"r={resistance} x=", f"\n~ r={resistance} x=")
                source.write_text(text)
            row = evaluate(
                source.resolve(), args.bridge.resolve(), folder.resolve(), 1e-11
            )
            native = run(source.resolve(), 1e-11)
            native.Circuit.SetActiveElement("Reactor.series")
            network = json.loads((folder / "imported/bridge.json").read_text())[
                "network"
            ]
            line = next(
                candidate
                for candidate in network["lines"]
                if candidate.get("extras", {}).get("dss_original_element")
                == "reactor.series"
            )
            ratings_pair = [
                native.CktElement.NormalAmps(),
                native.CktElement.EmergAmps(),
            ]
            typed_pair = [line["extras"]["normamps"], line["i_max"][0]]
            row["current_ratings"] = dict(
                native=ratings_pair,
                typed=typed_pair,
                passed=all(
                    abs(a - b) < 1e-10
                    for a, b in zip(ratings_pair, typed_pair, strict=True)
                ),
            )
            row["passed"] = row["passed"] and row["current_ratings"]["passed"]
            row["input_sha256"] = hashlib.sha256(source.read_bytes()).hexdigest()
            report["cases"][name] = row
            print(name, "PASS" if row["passed"] else "FAIL", row["stage"], flush=True)
    report["passed"] = all(row["passed"] for row in report["cases"].values())
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    if not report["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
