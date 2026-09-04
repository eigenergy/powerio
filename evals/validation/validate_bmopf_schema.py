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

import powerio


SCHEMAS = {
    "0.1.0": Path("tests/data/dist/bmopf/draft_bmopf_schema.json"),
    "0.2.0": Path("tests/data/dist/bmopf/bmopf-0.2.0.schema.json"),
}
SCHEMA_ALIASES = {
    # The original ENWL case names the draft schema's repository directory,
    # rather than the schema document's canonical raw-content $id.
    "https://github.com/frederikgeth/bmopf-report/draft_schema_and_networks": "0.1.0",
}
SCHEMA_PATHS = frozenset(SCHEMAS.values())
BMOPF_CASES = [
    path
    for path in sorted(Path("tests/data/dist/bmopf").glob("*.json"))
    if path not in SCHEMA_PATHS
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


def declared_schema_version(doc: dict, schema_ids: dict[str, str]) -> str:
    meta = doc.get("meta")
    if not isinstance(meta, dict):
        raise ValueError("emitted document has no meta object naming its BMOPF schema")
    version = meta.get("schema_version")
    if version in SCHEMAS:
        return version
    schema_id = meta.get("$schema")
    for candidate, expected_id in schema_ids.items():
        if schema_id == expected_id:
            return candidate
    if schema_id in SCHEMA_ALIASES:
        return SCHEMA_ALIASES[schema_id]
    raise ValueError(
        f"emitted document names unsupported BMOPF schema version {version!r} "
        f"and schema {schema_id!r}"
    )


def validate_case(validators_by_version, schema_ids: dict[str, str], case: Path) -> list[str]:
    module = powerio.parse(case)
    out = powerio.emit(module, "bmopf-json")
    if not out.text.strip():
        return ["writer returned an empty document"]

    doc = json.loads(out.text)
    version = declared_schema_version(doc, schema_ids)
    validator = validators_by_version[version]
    errors = sorted(
        validator.iter_errors(doc),
        key=lambda err: (tuple(err.absolute_path), err.message),
    )
    return [f"{error_path(err)}: {err.message}" for err in errors]


def check_paths(paths: Iterable[Path]) -> list[str]:
    missing = [path.as_posix() for path in paths if not path.is_file()]
    missing.extend(path.as_posix() for path in SCHEMA_PATHS if not path.is_file())
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

    schema_documents = {
        version: json.loads(path.read_text(encoding="utf-8"))
        for version, path in SCHEMAS.items()
    }
    validators_by_version = {
        version: schema_validator(path) for version, path in SCHEMAS.items()
    }
    schema_ids = {
        version: document["$id"] for version, document in schema_documents.items()
    }
    failures: list[str] = []
    for case in CASES:
        try:
            case_failures = validate_case(validators_by_version, schema_ids, case)
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
