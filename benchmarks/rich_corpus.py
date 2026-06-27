#!/usr/bin/env python3
"""Scan local corpora for rich typed field coverage.

This is intentionally an opt in report generator, not a CI gate. Roots come
from ``--root`` arguments or the generic ``POWERIO_RICH_ROOTS`` path list.
Output paths are relative to the root label, so generated reports do not record
local absolute paths.
"""

from __future__ import annotations

import argparse
import csv
import json
import os
import sys
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Iterable

import powerio


SUPPORTED_FILES = {
    ".m": "matpower",
    ".raw": "psse",
    ".aux": "powerworld",
    ".epc": "pslf",
    ".json": "json",
}
SKIP_DIRS = {".git", ".hg", ".svn", ".venv", ".venv-validate", "target", "__pycache__"}


@dataclass
class Row:
    root: str
    path: str
    format: str
    status: str
    message: str
    warnings: int = 0
    buses: int = 0
    branches: int = 0
    loads: int = 0
    switches: int = 0
    storage: int = 0
    hvdc: int = 0
    terminal_admittance: int = 0
    branch_current_ratings: int = 0
    branch_solutions: int = 0
    load_voltage_models: int = 0
    storage_current_ratings: int = 0
    hvdc_costs: int = 0


def env_paths(name: str) -> list[Path]:
    raw = os.environ.get(name, "")
    return [Path(p).expanduser() for p in raw.split(os.pathsep) if p]


def roots_from_args(args: argparse.Namespace) -> list[Path]:
    roots = [Path(p).expanduser() for p in args.root]
    roots.extend(env_paths("POWERIO_RICH_ROOTS"))
    seen: set[Path] = set()
    unique: list[Path] = []
    for root in roots:
        resolved = root.resolve()
        if resolved not in seen and resolved.exists():
            seen.add(resolved)
            unique.append(resolved)
    return unique


def iter_files(root: Path) -> Iterable[tuple[Path, str]]:
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = sorted(d for d in dirnames if d not in SKIP_DIRS)
        path = Path(dirpath)
        if (path / "network.csv").is_file() and (path / "buses.csv").is_file():
            yield path, "pypsa-csv-folder"
            dirnames[:] = []
            continue
        for name in sorted(filenames):
            file = path / name
            fmt = SUPPORTED_FILES.get(file.suffix.lower())
            if fmt is not None:
                yield file, fmt


def rel_display(root_label: str, root: Path, path: Path) -> str:
    try:
        rel = path.relative_to(root)
    except ValueError:
        rel = Path(path.name)
    return f"{root_label}/{rel.as_posix()}"


def count_features(case) -> dict[str, int]:
    data = json.loads(case.to_json())
    branches = data.get("branches", [])
    loads = data.get("loads", [])
    storage = data.get("storage", [])
    hvdc = data.get("hvdc", [])

    def has_terminal_admittance(branch: dict) -> bool:
        charging = branch.get("charging")
        if not charging:
            return False
        g_fr = float(charging.get("g_fr", 0.0))
        b_fr = float(charging.get("b_fr", 0.0))
        g_to = float(charging.get("g_to", 0.0))
        b_to = float(charging.get("b_to", 0.0))
        return abs(g_fr) > 0.0 or abs(g_to) > 0.0 or abs(b_fr - b_to) > 0.0

    return {
        "buses": len(data.get("buses", [])),
        "branches": len(branches),
        "loads": len(loads),
        "switches": len(data.get("switches", [])),
        "storage": len(storage),
        "hvdc": len(hvdc),
        "terminal_admittance": sum(has_terminal_admittance(b) for b in branches),
        "branch_current_ratings": sum(1 for b in branches if b.get("current_ratings")),
        "branch_solutions": sum(1 for b in branches if b.get("solution")),
        "load_voltage_models": sum(1 for l in loads if l.get("voltage_model")),
        "storage_current_ratings": sum(1 for s in storage if s.get("current_rating") is not None),
        "hvdc_costs": sum(1 for h in hvdc if h.get("cost")),
    }


def parse_case(path: Path, fmt: str):
    if fmt == "pypsa-csv-folder":
        return powerio.read_pypsa_csv_folder(str(path))
    return powerio.parse_file(str(path))


def scan_one(root_label: str, root: Path, path: Path, fmt: str) -> Row:
    row = Row(root=root_label, path=rel_display(root_label, root, path), format=fmt, status="ok", message="")
    try:
        case = parse_case(path, fmt)
    except Exception as exc:  # noqa: BLE001 - report corpus parser failures
        row.message = f"{type(exc).__name__}: {exc}".replace("\n", " ")[:500]
        if fmt == "json" and (
            "use the distribution parser" in row.message
            or "cannot infer JSON format" in row.message
        ):
            row.status = "SKIP"
        else:
            row.status = "FAIL"
        return row

    row.warnings = len(getattr(case, "read_warnings", []))
    for key, value in count_features(case).items():
        setattr(row, key, value)
    return row


def write_reports(rows: list[Row], out_dir: Path) -> None:
    out_dir.mkdir(parents=True, exist_ok=True)
    fields = list(Row.__dataclass_fields__)
    tsv_path = out_dir / "rich_corpus.tsv"
    with tsv_path.open("w", newline="") as f:
        writer = csv.DictWriter(f, fieldnames=fields, delimiter="\t")
        writer.writeheader()
        for row in rows:
            writer.writerow(asdict(row))

    summary = {
        "rows": len(rows),
        "ok": sum(r.status == "ok" for r in rows),
        "fail": sum(r.status == "FAIL" for r in rows),
        "skip": sum(r.status == "SKIP" for r in rows),
        "feature_totals": {
            field: sum(getattr(r, field) for r in rows)
            for field in [
                "terminal_admittance",
                "branch_current_ratings",
                "branch_solutions",
                "load_voltage_models",
                "switches",
                "storage_current_ratings",
                "hvdc_costs",
            ]
        },
    }
    json_path = out_dir / "rich_corpus.json"
    json_path.write_text(
        json.dumps({"summary": summary, "rows": [asdict(r) for r in rows]}, indent=2) + "\n"
    )
    print(f"wrote {tsv_path} and {json_path}")
    print(
        "summary: "
        f"{summary['ok']} ok, {summary['fail']} failed, {summary['skip']} skipped, "
        f"{summary['feature_totals']}"
    )


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--root", action="append", default=[], help="corpus root to scan; repeatable")
    p.add_argument(
        "--output-dir",
        default="benchmarks/results",
        help="directory for rich_corpus.tsv/json",
    )
    p.add_argument("--limit", type=int, default=0, help="maximum cases to scan, 0 means no limit")
    return p.parse_args()


def main() -> int:
    args = parse_args()
    roots = roots_from_args(args)
    if not roots:
        print(
            "no corpus roots; set POWERIO_RICH_ROOTS or pass --root",
            file=sys.stderr,
        )
        write_reports([], Path(args.output_dir))
        return 0

    rows: list[Row] = []
    for i, root in enumerate(roots, start=1):
        root_label = root.name or f"root{i}"
        for path, fmt in iter_files(root):
            rows.append(scan_one(root_label, root, path, fmt))
            if args.limit and len(rows) >= args.limit:
                write_reports(rows, Path(args.output_dir))
                return 0
    write_reports(rows, Path(args.output_dir))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
