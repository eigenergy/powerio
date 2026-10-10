"""Canonical grounded service GLM, checked on both forks and native DSS.

Original, IR-restored, edited, rebased and reimported circuits retain all
transformer and grounded cable ports. No retained-source replay is used.
"""
import argparse
import hashlib
import json
import re
from pathlib import Path

from oracle import LOCK, command, compare, run_glm
from split_phase_import_oracle import cases as center_cases
from split_phase_import_oracle import read_dss, read_native
from triplex_import_oracle import OUTPUTS, imported_snapshot, native_snapshot
from triplex_import_oracle import cases as cable_cases


def instrument(text, original):
    prelude = "\n".join(re.findall(r"(?m)^#set[^\n]*|clock\s*\{[^{}]*\}|module\s+tape\s*;", original))
    records = "\n".join(re.findall(r"object\s+(?:recorder|impedance_dump)\s*\{[^{}]*\}", original))
    return prelude + "\n" + text + "\n" + records + "\n"


def cable_losses(snapshot):
    phase = [complex(*snapshot[f"service.from.{leg}.power"]["value"])
             + complex(*snapshot[f"service.to.{leg}.power"]["value"]) for leg in ("L1", "L2", "N")]
    physical = sum(phase)
    reported = complex(sum(abs(value.real) for value in phase), physical.imag)
    return physical, reported


def loss_snapshot(value):
    return {"service.native_reported_loss": dict(unit="VA", value=[value.real, value.imag])}


def physical_native(directory, phase):
    snapshot = native_snapshot(directory, phase)
    raw = complex(*snapshot["service.loss"]["value"])
    physical, convention = cable_losses(snapshot)
    snapshot["service.loss"] = dict(unit="VA", value=[physical.real, physical.imag])
    return snapshot, raw, compare(loss_snapshot(convention), loss_snapshot(raw))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bridge", type=Path, required=True)
    parser.add_argument("--pnnl-root", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--case", action="append")
    parser.add_argument("--profile", action="append", choices=("pnnl", "arras"))
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    report = dict(scope=__doc__, engines=LOCK, bridge_sha256=hashlib.sha256(args.bridge.read_bytes()).hexdigest(), cases={})
    cases = [("center-" + label, phase, text, False, False) for label, phase, text, _ in center_cases()]
    cases += [("cable-" + label, phase, text, True, False) for label, phase, text in cable_cases()]
    _, phase, original = next(cable_cases())
    for label, sign in (("ieee9500", 1), ("reciprocal", -1)):
        direct = re.sub(r"object triplex_line_configuration\s*\{[^{}]*\}",
                        f"object triplex_line_configuration {{ name cable; z11 2.16454+.880800j; z12 .623542+.673688j; z21 {sign * .623542}{sign * .673688:+}j; z22 {sign * 2.16454}{sign * .880800:+}j; }}", original)
        for solver in ("NR", "FBS"):
            cases.append((f"direct-{label}-{solver}", phase, re.sub(r"solver_method (NR|FBS)", "solver_method " + solver, direct), True, True))
    for label, phase, text, cable, direct in cases:
        if args.case and label not in args.case:
            continue
        for profile in args.profile or ("pnnl", "arras"):
            folder = args.output / label / profile
            reference = folder / "reference"
            reference.mkdir(parents=True)
            source = reference / "model.glm"
            source.write_text(text)
            native_observe = native_snapshot if cable else read_native
            def dss_observe(path, phase, cable=cable, direct=direct):
                return imported_snapshot(path, phase, direct=direct) if cable else read_dss(path, phase)
            outputs = OUTPUTS if cable else ("source.csv", "sink.csv", "line.csv", "impedance.xml")
            row = dict(passed=False, input_sha256=hashlib.sha256(text.encode()).hexdigest(), comparisons={})
            try:
                emitted = folder / "emitted"
                bridge = json.loads(command([str(args.bridge.resolve()), str(source.resolve()), str(emitted.resolve()), profile], folder))
                (folder / "bridge.json").write_text(json.dumps(bridge, indent=2) + "\n")
                if not all(stage["ir_equal"] and stage["reimport_ir_equal"] for stage in bridge.values()):
                    raise ValueError("IR network equality failed")
                row["engine"] = run_glm(profile, reference, args.pnnl_root, outputs)
                if cable:
                    expected, raw_reference, reporting = physical_native(reference, phase)
                    row["native_reporting"] = {"reference": reporting}
                else:
                    expected = native_observe(reference, phase)
                for stage in ("fresh", "restored", "edited", "rebased"):
                    directory = emitted / stage
                    goal = dss_observe(directory / "dss/case.dss", phase) if stage == "edited" else expected
                    for variant, filename in (("canonical", "case.glm"), ("reimported", "repeated.glm")):
                        run_directory = directory / variant
                        run_directory.mkdir()
                        (run_directory / "model.glm").write_text(instrument((directory / filename).read_text(), text))
                        run_glm(profile, run_directory, args.pnnl_root, outputs)
                        key = f"{stage}/{variant}"
                        if cable:
                            actual, raw, reporting = physical_native(run_directory, phase)
                            row["native_reporting"][key + "/law"] = reporting
                            wanted = cable_losses(goal)[1] if stage == "edited" else raw_reference
                            row["native_reporting"][key + "/original_or_edited"] = compare(loss_snapshot(wanted), loss_snapshot(raw))
                        else:
                            actual = native_observe(run_directory, phase)
                        row["comparisons"][key] = compare(goal, actual)
                        (run_directory / "measurements.json").write_text(json.dumps(dict(expected=goal, actual=actual), indent=2) + "\n")
                    row["comparisons"][f"{stage}/dss"] = compare(goal, dss_observe(directory / "dss/case.dss", phase))
                    row["comparisons"][f"{stage}/reimported-dss"] = compare(goal, dss_observe(directory / "repeated-dss/case.dss", phase))
                edited = physical_native(emitted / "edited/canonical", phase)[0] if cable else native_observe(emitted / "edited/canonical", phase)
                negative = compare(expected, edited)
                row["unchanged_negative"] = negative
                row["passed"] = all(c["passed"] for c in row["comparisons"].values()) and all(c["passed"] for c in row.get("native_reporting", {}).values()) and not negative["passed"]
            except Exception as error:
                row["error"] = str(error)
            report["cases"][f"{label}/{profile}"] = row
            (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
            failed = {key: c["failures"][:2] for key, c in row["comparisons"].items() if not c["passed"]}
            print(label, profile, row["passed"], row.get("error", ""), failed, flush=True)
    report["passed"] = bool(report["cases"]) and all(c["passed"] for c in report["cases"].values())
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
