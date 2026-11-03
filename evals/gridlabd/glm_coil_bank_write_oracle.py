"""Canonical delta-grounded-wye banks on both pinned GridLAB-D forks and DSS."""
import argparse
import hashlib
import json
from pathlib import Path

from delta_grounded_import_oracle import cases, imported_snapshot, native_snapshot
from glm_service_write_oracle import instrument
from oracle import LOCK, command, compare, run_glm


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bridge", type=Path, required=True)
    parser.add_argument("--pnnl-root", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--case", action="append")
    parser.add_argument("--profile", action="append", choices=("pnnl", "arras"))
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    report = dict(engines=LOCK, bridge_sha256=hashlib.sha256(args.bridge.read_bytes()).hexdigest(), cases={})
    for label, text, _ in cases():
        if args.case and label not in args.case:
            continue
        for profile in args.profile or ("pnnl", "arras"):
            folder = args.output / label / profile
            reference = folder / "reference"
            reference.mkdir(parents=True)
            source = reference / "model.glm"
            source.write_text(text)
            row = dict(passed=False, input_sha256=hashlib.sha256(text.encode()).hexdigest(), comparisons={})
            try:
                emitted = folder / "emitted"
                data = json.loads(command([str(args.bridge.resolve()), str(source.resolve()), str(emitted.resolve()), profile], folder))
                (folder / "bridge.json").write_text(json.dumps(data, indent=2) + "\n")
                row["engine"] = run_glm(profile, reference, args.pnnl_root)
                original = native_snapshot(reference)
                for stage in ("fresh", "restored", "edited", "rebased", "grounded_reference"):
                    case = emitted / f"{stage}.glm"
                    dss = emitted / f"{stage}.dss/case.dss"
                    wanted = imported_snapshot(dss) if stage == "edited" else original
                    repeated = folder / ("reimported-" + stage)
                    command([str(args.bridge.resolve()), str(case.resolve()), str(repeated.resolve()), profile], folder)
                    for variant, path in (("canonical", case), ("reimported", repeated / "fresh.glm")):
                        run = emitted / stage / variant
                        run.mkdir(parents=True)
                        (run / "model.glm").write_text(instrument(path.read_text(), text))
                        run_glm(profile, run, args.pnnl_root)
                        actual = native_snapshot(run)
                        key = f"{stage}/{variant}"
                        row["comparisons"][key] = compare(wanted, actual)
                        (run / "measurements.json").write_text(json.dumps(dict(expected=wanted, actual=actual), indent=2) + "\n")
                    row["comparisons"][f"{stage}/dss"] = compare(wanted, imported_snapshot(dss))
                    row["comparisons"][f"{stage}/reimported-dss"] = compare(wanted, imported_snapshot(repeated / "fresh.dss/case.dss"))
                negative = compare(original, native_snapshot(emitted / "edited/canonical"))
                row["unchanged_negative"] = negative
                row["passed"] = all(c["passed"] for c in row["comparisons"].values()) and not negative["passed"]
            except Exception as error:
                row["error"] = str(error)
            report["cases"][f"{label}/{profile}"] = row
            (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
            failures = {key: row["failures"][:2] for key, row in row["comparisons"].items() if not row["passed"]}
            print(label, profile, row["passed"], row.get("error", ""), failures, flush=True)
    report["passed"] = bool(report["cases"]) and all(c["passed"] for c in report["cases"].values())
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
