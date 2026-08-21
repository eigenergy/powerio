#!/usr/bin/env python3
"""Validate release archive notices and checkout-only source exclusions."""

from __future__ import annotations

import argparse
import sys
import tarfile
from pathlib import PurePosixPath

REQUIRED_LICENSES = ("LICENSE-MIT", "LICENSE-APACHE")
OPENDSS_DERIVED_EXAMPLES = (
    "examples/bmopf/4bus_dy.json",
    "examples/bmopf/ieee34.json",
    "examples/bmopf/ieee123.json",
)
OPENDSS_SOURCE_NOTICE = "examples/bmopf/NOTICE-OPENDSS-BSD-3-CLAUSE"
CHECKOUT_ONLY = (
    "powerio-prob/tests/data/goc3_14bus_20220707.json",
    "tests/data/dist/bmopf/draft_bmopf_schema.json",
    "tests/data/dist/bmopf/example_enwl_n1_f2.json",
    "tests/data/dist/bmopf/example_ieee13.json",
)


def suffix_match(name: str, suffix: str) -> bool:
    normalized = str(PurePosixPath(name))
    return normalized == suffix or normalized.endswith("/" + suffix)


def audit(path: str) -> list[str]:
    errors: list[str] = []
    try:
        with tarfile.open(path, "r:*") as archive:
            members = [member for member in archive.getmembers() if member.isfile()]
    except (OSError, tarfile.TarError) as exc:
        return [f"cannot read tar archive: {exc}"]

    names = [member.name for member in members]
    for license_name in REQUIRED_LICENSES:
        if not any(PurePosixPath(name).name == license_name for name in names):
            errors.append(f"missing {license_name}")
    for checkout_only in CHECKOUT_ONLY:
        if any(suffix_match(name, checkout_only) for name in names):
            errors.append(f"contains checkout-only source fixture {checkout_only}")
    if any(
        any(suffix_match(name, example) for example in OPENDSS_DERIVED_EXAMPLES)
        for name in names
    ):
        if not any(suffix_match(name, OPENDSS_SOURCE_NOTICE) for name in names):
            errors.append(
                "contains OpenDSS-derived examples without " + OPENDSS_SOURCE_NOTICE
            )
    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("archives", nargs="+", help=".crate, sdist, or binary tarballs")
    args = parser.parse_args()
    failed = False
    for path in args.archives:
        errors = audit(path)
        if errors:
            failed = True
            for error in errors:
                print(f"{path}: {error}", file=sys.stderr)
        else:
            print(f"checked {path}")
    return int(failed)


if __name__ == "__main__":
    raise SystemExit(main())
