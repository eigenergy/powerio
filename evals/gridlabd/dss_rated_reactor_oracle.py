"""Rated shunt reactors with explicit R and Rp, checked against native DSS."""

import argparse
import hashlib
import json
from pathlib import Path

from european_lv_import_oracle import node_inventory, snapshot, source_inventory
from ieee4_import_oracle import evaluate, run
from oracle import compare


def ratings(engine):
    engine.Circuit.SetActiveElement("reactor.bank")
    return [engine.CktElement.NormalAmps(), engine.CktElement.EmergAmps()]


def close_ratings(expected, actual):
    return all(abs(a - b) <= 1e-10 + 1e-12 * abs(a) for a, b in zip(expected, actual, strict=True))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bridge", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--check-edits", action="store_true")
    args = parser.parse_args()
    if args.check_edits:
        check_edits(args.output)
        return
    args.output.mkdir(parents=True, exist_ok=False)
    report = dict(bridge_sha256=hashlib.sha256(args.bridge.read_bytes()).hexdigest(), cases={})
    for phases in (1, 2, 3):
        for frequency in (50, 60):
            for resistance, parallel in ((0, 0), (2, 0), (2, 300), (0, 300), (None, 300)):
                name = f"p{phases}-f{frequency}-r{resistance}-rp{parallel}"
                folder = args.output / name
                folder.mkdir()
                source = folder / "native.dss"
                kv = 7.2 if phases == 1 else 12.47
                resistance_property = "" if resistance is None else f"r={resistance}"
                source.write_text(
                    f"New Circuit.c basekv=12.47 pu=1.03 frequency={frequency} basefreq=60 r1=.1 x1=.2 r0=.1 x0=.2\n"
                    "New Line.feed phases=3 bus1=sourcebus bus2=loadbus length=1 units=m r1=.1 x1=.2 r0=.1 x0=.2 c1=0 c0=0 basefreq=60\n"
                    f"New Reactor.bank bus1=loadbus phases={phases} kv={kv} kvar=120 {resistance_property} rp={parallel} basefreq=60\n"
                    f"Set frequency={frequency} voltagebases=[12.47]\nCalcvoltagebases\n"
                )
                row = evaluate(source.resolve(), args.bridge.resolve(), folder.resolve(), 1e-11)
                values = [ratings(run(path, 1e-11)) for path in
                          (source, folder / "imported/dss/case.dss", folder / "reimported/dss/case.dss")]
                row["current_ratings"] = dict(native=values[0], canonical=values[1], reimported=values[2],
                                              passed=all(close_ratings(values[0], value) for value in values[1:]))
                row["passed"] &= row["current_ratings"]["passed"]
                report["cases"][name] = row
                (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
                print(name, row["passed"], row.get("error", ""), flush=True)
    (args.output / "cases.json").write_text(json.dumps(list(report["cases"]), indent=2) + "\n")
    report["passed"] = all(row["passed"] for row in report["cases"].values())
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")


def check_edits(root):
    report = dict(cases={})
    for name in json.loads((root / "cases.json").read_text()):
        folder = root / name
        resistance = 0.0 if "-rNone-" in name else float(name.split("-r")[1])
        parallel = float(name.split("-rp")[1])
        normal, emergency = ratings(run(folder / "native.dss", 1e-11))
        path = folder / "native-edited.dss"
        path.write_text((folder / "native.dss").read_text()
                        + f"Edit Reactor.bank normamps={normal:.17g} emergamps={emergency:.17g} kvar={120 * 1.17:.17g} r={resistance / 1.17:.17g} rp={parallel / 1.17:.17g}\n")
        native = run(path, 1e-11)
        buses = [bus.lower() for bus in native.Circuit.AllBusNames()]
        elements = ["line.feed", "reactor.bank"]
        expected = snapshot(native, buses, elements, canonical=False)
        canonical = run(folder / "typed-edited/case.dss", 1e-11)
        ratings_match = close_ratings(ratings(native), ratings(canonical))
        actual = snapshot(canonical, buses, elements,
                          canonical=True, bus_nodes=node_inventory(native, buses), source_port=source_inventory(native))
        comparison = compare(expected, actual)
        unchanged = snapshot(run(folder / "native.dss", 1e-11), buses, elements, canonical=False)
        negative = compare(unchanged, actual)
        report["cases"][name] = dict(comparison=comparison, unchanged_negative=negative,
                                    current_ratings_passed=ratings_match,
                                    passed=comparison["passed"] and not negative["passed"] and ratings_match)
        print(name, report["cases"][name]["passed"], flush=True)
    report["passed"] = all(row["passed"] for row in report["cases"].values())
    (root / "edited-report.json").write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
