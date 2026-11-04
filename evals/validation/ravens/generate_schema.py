#!/usr/bin/env python3
"""Generate the unmodified schema with the pinned MG-RAVENS implementation."""

import argparse
import json
import subprocess
import sys
from pathlib import Path

UPSTREAM_REVISION = "094ad1163cb111df676884635aa03faa0538b688"


def pinned_checkout(path: Path) -> Path:
    path = path.resolve()
    revision = subprocess.check_output(
        ["git", "-C", str(path), "rev-parse", "HEAD"], text=True
    ).strip()
    if revision != UPSTREAM_REVISION:
        raise ValueError(f"expected MG-RAVENS {UPSTREAM_REVISION}, got {revision}")
    if subprocess.check_output(
        ["git", "-C", str(path), "status", "--porcelain", "--untracked-files=no"],
        text=True,
    ).strip():
        raise ValueError("the MG-RAVENS oracle checkout has modified tracked files")
    return path


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("upstream", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    sys.path.insert(0, str(pinned_checkout(args.upstream)))
    from ravens.schema import RavensSchema
    from ravens.uml import UMLExclusions

    # This is the upstream schema generation entry point and its normal
    # exclusion of market classes, not a PowerIO reconstruction of the UML.
    schema = RavensSchema(
        uml_exclusions=UMLExclusions().exclude_by_name_startswith(["Mkt"])
    ).schema
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(schema, indent=2) + "\n", encoding="utf-8")
    print(f"Generated MG-RAVENS {UPSTREAM_REVISION}: {args.output}")


if __name__ == "__main__":
    main()
