#!/usr/bin/env python3
"""Validate fresh RAVENS output, including the IR boundary, against upstream."""

import argparse
import importlib.util
import json
import shutil
import subprocess
import tempfile
from copy import deepcopy
from pathlib import Path

from balanced_profile import balanced_schema
from generate_schema import UPSTREAM_REVISION, pinned_checkout
from jsonschema import Draft202012Validator

ROOT = Path(__file__).resolve().parents[3]
# The archived ENWL document already fails PowerIO's source semantic check.
# Require this precise refusal instead of describing it as valid RAVENS output.
SOURCE_REFUSALS = {
    "tests/data/dist/bmopf/example_enwl_n1_f2.json": "READ.BMOPF.SEMANTIC_INVALID: bus.sourcebus: a perfectly grounded terminal is absent from terminal_names",
}


def run(*args: str | Path) -> str:
    result = subprocess.run(
        [str(a) for a in args], capture_output=True, text=True, check=False
    )
    if result.returncode:
        raise RuntimeError(f"{' '.join(map(str, args))}\n{result.stderr}")
    return result.stderr


def load(path: Path) -> dict:
    def reject_nonfinite(value: str) -> None:
        raise ValueError(f"nonfinite JSON literal: {value}")

    return json.loads(path.read_text(encoding="utf-8"), parse_constant=reject_nonfinite)


def failures(validator: Draft202012Validator, document: dict) -> list[str]:
    return [
        f"/{'/'.join(map(str, error.absolute_path))}: {error.message}"
        for error in validator.iter_errors(document)
    ]


def require_valid(validator: Draft202012Validator, document: dict) -> None:
    errors = failures(validator, document)
    if errors:
        raise ValueError("\n".join(errors))


def require_rejected(
    validator: Draft202012Validator, document: dict, label: str
) -> None:
    if not failures(validator, document):
        raise ValueError(f"false pass: validator accepted {label}")


def check_balanced_oracle(upstream: Path, scratch: Path, native, balanced) -> None:
    # Load the actual converter directly: its package __init__ eagerly imports
    # all of the distribution export tools, which this oracle does not need.
    spec = importlib.util.spec_from_file_location(
        "upstream_mpc2ravens", upstream / "ravens/parsers/mpc2ravens.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    shutil.copyfile(ROOT / "tests/data/case9.m", scratch / "case9.m")
    module.parse_mpc_to_ravens(str(scratch), "case9")
    document = load(scratch / "case9.json")
    require_rejected(
        native, document, "upstream balanced profile with schema omissions"
    )
    require_valid(balanced, document)
    # Prove the discrepancy is exactly the two documented fields, and no
    # other native-schema failure was hidden by the supplement.
    stripped = deepcopy(document)
    del stripped["MySettings"]
    for node in stripped["ConnectivityNode"].values():
        del node["ConnectivityNode.SvVoltage"]
    require_valid(native, stripped)
    invalid = deepcopy(document)
    next(iter(invalid["ConnectivityNode"].values()))["ConnectivityNode.SvVoltage"][0][
        "SvVoltage.v"
    ] = "bad"
    require_rejected(balanced, invalid, "a nonnumeric state voltage")
    invalid = deepcopy(document)
    invalid["MySettings"]["unknown_setting"] = 1
    require_rejected(balanced, invalid, "an unknown balanced settings field")
    invalid = deepcopy(document)
    invalid["unknown_top_level_class"] = {}
    require_rejected(balanced, invalid, "an unknown balanced class")


def fixtures() -> list[tuple[Path, str]]:
    data = ROOT / "tests/data"
    cases = [(data / f"case{n}.m", "balanced") for n in (9, 14, 30, 57, 118)]
    # Deliberately include every micro case, including unsupported terminal
    # identities. Loss diagnostics do not permit schema-invalid output.
    for pattern in (
        "dist/micro/*.dss",
        "dist/pmd/*.json",
        "dist/pmd/math/*.json",
        "dist/ravens/*.json",
        "dist/bmopf/example_*.json",
    ):
        matched = sorted(data.glob(pattern))
        if not matched:
            raise ValueError(f"fixture group is empty: {pattern}")
        cases.extend((path, "distribution") for path in matched)
    return cases


def snapshots(ir: Path) -> list[tuple[str, Path]]:
    document = load(ir)
    if document["value"]["type"] != "powerio.TimeSeries<powerio.MulticonductorNetwork>":
        return [("network", ir)]
    series = document["value"]["data"]
    if not series["values"] or len(series["values"]) != len(series["time_points"]):
        raise ValueError("empty or mismatched time series")
    result = []
    for index, (point, value) in enumerate(
        zip(series["time_points"], series["values"], strict=True)
    ):
        snapshot = deepcopy(document)
        snapshot["value"] = {"type": "powerio.MulticonductorNetwork", "data": value}
        path = ir.with_name(f"{ir.stem}-{index}.pio.json")
        path.write_text(json.dumps(snapshot), encoding="utf-8")
        result.append((f"snapshot {point['label']}", path))
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--powerio", required=True, type=Path)
    parser.add_argument("--upstream", required=True, type=Path)
    parser.add_argument("--schema", required=True, type=Path)
    parser.add_argument("--report", required=True, type=Path)
    args = parser.parse_args()
    upstream = pinned_checkout(args.upstream)
    schema = load(args.schema)
    Draft202012Validator.check_schema(schema)
    native = Draft202012Validator(schema)
    supplement = balanced_schema(schema)
    Draft202012Validator.check_schema(supplement)
    balanced = Draft202012Validator(supplement)
    report = [
        "# MG-RAVENS schema validation",
        "",
        f"Upstream: `lanl-ansi/MG-RAVENS@{UPSTREAM_REVISION}`.",
        "",
        "Distribution: unmodified generated upstream schema. Balanced: the same schema",
        "plus the two strict upstream MATPOWER converter fields (`ConnectivityNode.SvVoltage`",
        "and `MySettings`). This is converter-profile validation, not a claim that the",
        "unmodified upstream schema accepts balanced output.",
        "",
        "Every emitted document is regenerated through PowerIO IR, without retained source bytes.",
        "Diagnostics are listed separately; schema validity does not establish electrical fidelity.",
        "",
        "| Case | Profile | Result |",
        "|---|---|---|",
    ]
    errors = []
    details = []
    with tempfile.TemporaryDirectory(prefix="powerio-ravens-schema-") as directory:
        scratch = Path(directory)
        check_balanced_oracle(upstream, scratch, native, balanced)
        for index, (source, profile) in enumerate(fixtures()):
            label = str(source.relative_to(ROOT))
            validator = balanced if profile == "balanced" else native
            try:
                ir = scratch / f"{index}.pio.json"
                try:
                    diagnostics = run(args.powerio, "serialize", source, "-o", ir)
                except RuntimeError as error:
                    expected_refusal = [
                        SOURCE_REFUSALS.get(label),
                        f"wrote {ir}",
                        "Error: PARSE.CLI.ERRORS_REPORTED: the reader reported 1 error(s); the output is incomplete",
                    ]
                    if (
                        label in SOURCE_REFUSALS
                        and str(error).splitlines()[1:] == expected_refusal
                    ):
                        report.append(
                            f"| `{label}` | {profile} | REFUSED: invalid source grounding |"
                        )
                        details.extend(
                            [f"### {label}", "", "```text", str(error), "```", ""]
                        )
                        continue
                    raise
                if label in SOURCE_REFUSALS:
                    raise ValueError(
                        "expected source refusal disappeared; review the source baseline"
                    )
                for snapshot_label, snapshot in snapshots(ir):
                    output = snapshot.with_suffix(".ravens.json")
                    diagnostics += run(
                        args.powerio,
                        "convert",
                        snapshot,
                        "--to",
                        "ravens-json",
                        "-o",
                        output,
                    )
                    document = load(output)
                    require_valid(validator, document)
                    if not document.get("ConnectivityNode"):
                        raise ValueError("empty emitted network")
                    invalid = deepcopy(document)
                    invalid["unknown_top_level_class"] = {}
                    require_rejected(validator, invalid, f"unknown class in {label}")
                    report.append(
                        f"| `{label}` ({snapshot_label}) | {profile} | PASS |"
                    )
                if diagnostics.strip():
                    details.extend(
                        [f"### {label}", "", "```text", diagnostics.strip(), "```", ""]
                    )
            except (ValueError, RuntimeError, OSError) as error:
                report.append(f"| `{label}` | {profile} | FAIL |")
                errors.append(f"{label}: {error}")
                details.extend([f"### {label}", "", "```text", str(error), "```", ""])
    report.extend(["", "Native converter and false-pass controls: PASS.", "", *details])
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text("\n".join(report) + "\n", encoding="utf-8")
    print(
        f"Checked {len(fixtures())} cases; {len(errors)} failures. Report: {args.report}"
    )
    if errors:
        raise SystemExit("\n".join(errors))


if __name__ == "__main__":
    main()
