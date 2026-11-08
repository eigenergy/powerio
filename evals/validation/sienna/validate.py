#!/usr/bin/env python3
"""Validate PowerIO exports against each pinned SiennaSchemas release.

Generate exports with POWERIO_SIENNA_OUTPUT_DIR=<dir> cargo test -p powerio-tx
--test sienna export_reference_cases. Pass the schema checkout and that directory.
Requires jsonschema. Schemas are read from immutable commits, outside this repo.
"""
from __future__ import annotations
import argparse
import json
from pathlib import Path
import subprocess
import tempfile
import warnings

with warnings.catch_warnings():
    warnings.simplefilter("ignore", DeprecationWarning)
    from jsonschema import Draft202012Validator, Draft7Validator, RefResolver

RELEASES = {
    "0.1.0": "9de1405a8574dd6939da5c156015ed9398014252",
    "0.2.0": "faf0640728da1a9ea735e0a5dc2d399e9f9d2366",
    "0.3.0": "ee93996970c616cc29f39cd0ce94e9def08cfe6d",
    "0.4.0": "53cbffec27376b23a9ced996131e0fcf8f1ee1f9",
}

def validate(checkout: Path, outputs: Path, strict: Path | None = None) -> int:
    checked = 0
    for version, commit in RELEASES.items():
        paths = subprocess.check_output(["git", "-C", str(checkout), "ls-tree", "-r", "--name-only", commit], text=True).splitlines()
        with tempfile.TemporaryDirectory(prefix="powerio-sienna-schema-") as directory:
            root = Path(directory)
            for name in paths:
                if not name.endswith(".json") or name.startswith("tests/"):
                    continue
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(subprocess.check_output(["git", "-C", str(checkout), "show", f"{commit}:{name}"]))
            schemas = {path.stem: path for path in root.rglob("*.json") if not path.name.startswith("openapi")}
            files = sorted((outputs / version).glob("*.json"))
            if not files:
                raise ValueError(f"No exports for {version}")
            for output in files:
                document = json.loads(output.read_text())
                check(schemas["SystemDocument"], document, output.name)
                for kind, rows in document["components"].items():
                    if kind not in schemas:
                        raise ValueError(f"{output}: unknown component type {kind}")
                    for row in rows:
                        check(schemas[kind], row, f"{output.name}/{kind}/{row.get('id')}")
                if strict is not None:
                    schema = json.loads((strict / version / "SystemDocument.json").read_text())
                    Draft7Validator(schema).validate(document)
                checked += 1
    return checked

def check(path: Path, instance: dict, label: str) -> None:
    schema = json.loads(path.read_text())
    validator = Draft202012Validator(schema, resolver=RefResolver(path.as_uri(), schema))
    failures = list(validator.iter_errors(instance))
    if failures:
        raise ValueError(label + ": " + "; ".join(f"{list(e.path)}: {e.message}" for e in failures))

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("schemas", type=Path)
    parser.add_argument("outputs", type=Path)
    parser.add_argument("--strict-bundles", type=Path)
    args = parser.parse_args()
    print(f"Validated {validate(args.schemas, args.outputs, args.strict_bundles)} documents and all component records.")
