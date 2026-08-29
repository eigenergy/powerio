#!/usr/bin/env python3
"""Validate emitted BMOPF JSON against the task force schema."""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path
from typing import Iterable

from jsonschema import validators

import powerio.dist as dist


SCHEMA = Path("tests/data/dist/bmopf/draft_bmopf_schema.json")
BMOPF_CASES = [
    path
    for path in sorted(Path("tests/data/dist/bmopf").glob("*.json"))
    if path.name != SCHEMA.name
] + sorted(Path("powerio-dist/examples/bmopf").glob("*.json"))
DSS_CASES = sorted(Path("tests/data/dist/micro").glob("*.dss")) + [
    Path("tests/data/dist/opendss/ieee13/IEEE13Nodeckt.dss"),
    Path("tests/data/dist/opendss/ieee34/ieee34Mod1.dss"),
    Path("tests/data/dist/opendss/ieee123/IEEE123Master.dss"),
]
PMD_CASES = sorted(Path("tests/data/dist/pmd").glob("*.json"))
CASES = BMOPF_CASES + DSS_CASES + PMD_CASES


def append_result(case: Path, mark: str) -> None:
    out = os.environ.get("PIO_RESULTS_TSV")
    if out:
        with open(out, "a", encoding="utf-8") as fh:
            fh.write(f"{case.as_posix()}\tbmopf_schema\t{mark}\n")


def schema_validator(schema_path: Path):
    schema = json.loads(schema_path.read_text(encoding="utf-8"))
    validator_cls = validators.validator_for(schema)
    validator_cls.check_schema(schema)
    return validator_cls(schema)


def error_path(error) -> str:
    path = "".join(f"[{part!r}]" for part in error.absolute_path)
    return path or "$"


def validate_case(validator, case: Path) -> list[str]:
    net = dist.parse_file(case)
    out = net.to_canonical_format("bmopf-json")
    if not out.text.strip():
        return ["writer returned an empty document"]

    doc = json.loads(out.text)
    errors = sorted(
        validator.iter_errors(doc),
        key=lambda err: (tuple(err.absolute_path), err.message),
    )
    return [f"{error_path(err)}: {err.message}" for err in errors]


def check_paths(paths: Iterable[Path]) -> list[str]:
    missing = [path.as_posix() for path in paths if not path.is_file()]
    if not SCHEMA.is_file():
        missing.append(SCHEMA.as_posix())
    return missing


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--count", action="store_true", help="print the number of validated cases")
    args = parser.parse_args()

    if args.count:
        print(len(CASES))
        return 0

    missing = check_paths(CASES)
    if missing:
        for path in missing:
            print(f"missing fixture: {path}", file=sys.stderr)
        return 2

    validator = schema_validator(SCHEMA)
    failures: list[str] = []
    for case in CASES:
        try:
            case_failures = validate_case(validator, case)
        except Exception as err:  # noqa: BLE001
            case_failures = [str(err)]

        mark = "ok" if not case_failures else "FAIL"
        append_result(case, mark)
        print(f"{case}: {mark}")
        for failure in case_failures[:10]:
            print(f"  {failure}")
        if case_failures:
            failures.append(f"{case}: {len(case_failures)} validation error(s)")

    if failures:
        print("\nBMOPF schema validation failed:")
        for failure in failures:
            print(f"  {failure}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
