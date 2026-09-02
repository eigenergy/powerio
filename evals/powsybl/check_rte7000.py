#!/usr/bin/env python3
"""Run the external RTE 7k XIIDM comparison without vendoring the dataset."""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import io
import json
import math
import subprocess
import time
from collections import Counter
from collections.abc import Callable
from pathlib import Path
from typing import Any

import numpy as np
import pandas as pd
import pypowsybl as pp


EXPECTED_SOURCE = {
    "name": "recollement-auto-20210103-0000-enrichi.xiidm",
    "sha256": "cd1cbd8c49c367ca366dd83bb05ead72f984a35de21e135e01f86b74d810a244",
    "bytes": 33_054_521,
    "lines": 351_970,
    "license": "CDLA-Permissive-2.0",
    "dataset": "OpenSynth/D-GITT-RTE7000-2021",
    "url": "https://huggingface.co/datasets/OpenSynth/D-GITT-RTE7000-2021",
}
EXPECTED_PYPOWSYBL_VERSION = "1.16.1"
EXPECTED_POWSYBL_CORE_VERSION = "7.3.0"
EXPECTED_POWSYBL_CORE_COMMIT = "0939bfcc2c0c094de907dc818dd688b4cbfb7281"
EXPECTED_COUNTS = {
    "identifiables": 130_725,
    "substations": 4_811,
    "voltage_levels": 5_867,
    "buses": 6_470,
    "bus_breaker_buses": 14_555,
    "busbar_sections": 11_864,
    "switches": 85_704,
    "terminals": 43_860,
    "lines": 7_745,
    "two_winding_transformers": 1_773,
    "three_winding_transformers": 0,
    "generators": 5_625,
    "loads": 6_876,
    "batteries": 0,
    "shunts": 407,
    "linear_shunt_sections": 407,
    "nonlinear_shunt_sections": 0,
    "static_var_compensators": 7,
    "boundary_lines": 45,
    "boundary_line_generation": 0,
    "tie_lines": 0,
    "ratio_tap_changers": 1_435,
    "ratio_tap_steps": 26_985,
    "phase_tap_changers": 14,
    "phase_tap_steps": 451,
    "operational_limits": 37_148,
    "voltage_angle_limits": 0,
    "reactive_capability_points": 11_268,
    "properties": 41_290,
}
RELATIVE_TOLERANCE = 1e-9
ABSOLUTE_TOLERANCE = 1e-8


def getter(name: str, *, all_attributes: bool = True) -> Callable[[Any], pd.DataFrame]:
    def get(network: Any) -> pd.DataFrame:
        method = getattr(network, name)
        return method(all_attributes=True) if all_attributes else method()

    return get


FRAMES = {
    "identifiables": getter("get_identifiables"),
    "substations": getter("get_substations"),
    "voltage_levels": getter("get_voltage_levels"),
    "buses": getter("get_buses"),
    "bus_breaker_buses": getter("get_bus_breaker_view_buses"),
    "busbar_sections": getter("get_busbar_sections"),
    "switches": getter("get_switches"),
    "terminals": getter("get_terminals"),
    "lines": getter("get_lines"),
    "two_winding_transformers": getter("get_2_windings_transformers"),
    "three_winding_transformers": getter("get_3_windings_transformers"),
    "generators": getter("get_generators"),
    "loads": getter("get_loads"),
    "batteries": getter("get_batteries"),
    "shunts": getter("get_shunt_compensators"),
    "linear_shunt_sections": getter("get_linear_shunt_compensator_sections"),
    "nonlinear_shunt_sections": getter("get_non_linear_shunt_compensator_sections"),
    "static_var_compensators": getter("get_static_var_compensators"),
    "boundary_lines": getter("get_boundary_lines"),
    "boundary_line_generation": getter("get_boundary_lines_generation"),
    "tie_lines": getter("get_tie_lines"),
    "ratio_tap_changers": getter("get_ratio_tap_changers"),
    "ratio_tap_steps": getter("get_ratio_tap_changer_steps"),
    "phase_tap_changers": getter("get_phase_tap_changers"),
    "phase_tap_steps": getter("get_phase_tap_changer_steps"),
    "operational_limits": getter("get_operational_limits"),
    "voltage_angle_limits": getter("get_voltage_angle_limits"),
    "reactive_capability_points": getter("get_reactive_capability_curve_points"),
    "properties": getter("get_elements_properties", all_attributes=False),
}

DUPLICATE_INDEX_KEYS = {
    "terminals": ("element_id", "element_side"),
    "properties": ("id", "type", "key"),
}


def source_metadata(path: Path) -> dict[str, Any]:
    digest = hashlib.sha256()
    byte_count = 0
    line_count = 0
    with path.open("rb") as source:
        while chunk := source.read(1024 * 1024):
            digest.update(chunk)
            byte_count += len(chunk)
            line_count += chunk.count(b"\n")
    return {
        "path": str(path.resolve()),
        "sha256": digest.hexdigest(),
        "bytes": byte_count,
        "lines": line_count,
        "license": EXPECTED_SOURCE["license"],
        "dataset": EXPECTED_SOURCE["dataset"],
        "url": EXPECTED_SOURCE["url"],
    }


def powsybl_version() -> dict[str, str]:
    output = io.StringIO()
    with contextlib.redirect_stdout(output):
        pp.print_version()
    for line in output.getvalue().splitlines():
        cells = [cell.strip() for cell in line.strip().strip("|").split("|")]
        if cells and cells[0] == "powsybl-core":
            if len(cells) < 4:
                break
            return {"version": cells[1], "commit": cells[3]}
    raise RuntimeError("PyPowSybl did not report its PowSybl Core version")


def powerio_revision(repository: Path) -> dict[str, Any]:
    commit = subprocess.run(
        ["git", "-C", str(repository), "rev-parse", "HEAD"],
        text=True,
        capture_output=True,
        check=True,
    ).stdout.strip()
    dirty = subprocess.run(
        ["git", "-C", str(repository), "status", "--porcelain"],
        text=True,
        capture_output=True,
        check=True,
    ).stdout.strip()
    return {"repository": str(repository.resolve()), "commit": commit, "dirty": bool(dirty)}


def diagnostic_counts(ir_path: Path, emission_log: Path) -> dict[str, Any]:
    ir = json.loads(ir_path.read_text(encoding="utf-8"))
    stored = Counter(diagnostic["code"] for diagnostic in ir.get("diagnostics", []))
    emission_lines = [
        line.strip()
        for line in emission_log.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]
    emitted = Counter(
        line.split(":", maxsplit=1)[0]
        for line in emission_lines
        if ":" in line
    )
    return {
        "stored": dict(sorted(stored.items())),
        "emission": dict(sorted(emitted.items())),
        "emission_messages": emission_lines,
    }


def run_command(command: list[str], log: Path) -> float:
    started = time.perf_counter()
    completed = subprocess.run(command, text=True, capture_output=True, check=False)
    wall_time = time.perf_counter() - started
    log.write_text(
        completed.stdout + ("\n" if completed.stdout and completed.stderr else "") + completed.stderr,
        encoding="utf-8",
    )
    if completed.returncode:
        raise RuntimeError(
            f"command failed with status {completed.returncode}: {' '.join(command)}; see {log}"
        )
    return wall_time


def timed_load(path: Path) -> tuple[Any, float]:
    started = time.perf_counter()
    network = pp.network.load(path)
    return network, time.perf_counter() - started


def validation(network: Any) -> dict[str, str]:
    try:
        return {"result": network.validate().name}
    except Exception as error:  # PyPowSybl exposes Java validation failures as exceptions.
        return {"error": str(error)}


def json_scalar(value: Any) -> Any:
    if isinstance(value, (np.bool_, bool)):
        return bool(value)
    if isinstance(value, (np.integer, int)):
        return int(value)
    if isinstance(value, (np.floating, float)):
        return None if math.isnan(float(value)) else float(value)
    if pd.isna(value):
        return None
    return str(value)


def row_identity(values: list[Any]) -> str:
    return json.dumps([json_scalar(value) for value in values], ensure_ascii=False)


def keyed_frame(label: str, frame: pd.DataFrame) -> tuple[pd.DataFrame, list[str]]:
    value = frame.copy()
    index_names = [name or f"index_{position}" for position, name in enumerate(value.index.names)]
    value.index = value.index.set_names(index_names)
    value = value.reset_index()
    keys = list(DUPLICATE_INDEX_KEYS.get(label, tuple(index_names)))
    missing = [key for key in keys if key not in value.columns]
    if missing:
        raise AssertionError(f"{label}: identity columns are absent: {missing}")
    identities = [row_identity([row[key] for key in keys]) for _, row in value.iterrows()]
    if len(identities) != len(set(identities)):
        raise AssertionError(f"{label}: row identity {keys} is not unique")
    value.insert(0, "__identity", identities)
    value = value.drop(columns=keys).set_index("__identity").sort_index()
    return value, keys


def equal_column(left: pd.Series, right: pd.Series) -> pd.Series:
    if pd.api.types.is_numeric_dtype(left.dtype) and pd.api.types.is_numeric_dtype(right.dtype):
        left_values = pd.to_numeric(left, errors="coerce").to_numpy(dtype=float, na_value=np.nan)
        right_values = pd.to_numeric(right, errors="coerce").to_numpy(dtype=float, na_value=np.nan)
        return pd.Series(
            np.isclose(
                left_values,
                right_values,
                rtol=RELATIVE_TOLERANCE,
                atol=ABSOLUTE_TOLERANCE,
                equal_nan=True,
            ),
            index=left.index,
        )
    return left.astype("string").fillna("<NA>") == right.astype("string").fillna("<NA>")


def compare_frame(label: str, source: pd.DataFrame, fresh: pd.DataFrame) -> dict[str, Any]:
    source_count = len(source)
    fresh_count = len(fresh)
    source, identity_columns = keyed_frame(label, source)
    fresh, fresh_identity_columns = keyed_frame(label, fresh)
    if identity_columns != fresh_identity_columns:
        raise AssertionError(
            f"{label}: source identity {identity_columns} differs from fresh identity {fresh_identity_columns}"
        )
    source_ids = set(source.index)
    fresh_ids = set(fresh.index)
    result: dict[str, Any] = {
        "source_count": source_count,
        "fresh_count": fresh_count,
        "identity_columns": identity_columns,
        "missing_ids": sorted(source_ids - fresh_ids)[:20],
        "extra_ids": sorted(fresh_ids - source_ids)[:20],
        "source_only_columns": sorted(set(source.columns) - set(fresh.columns)),
        "fresh_only_columns": sorted(set(fresh.columns) - set(source.columns)),
        "field_mismatches": {},
        "identity_comparisons": len(source_ids),
        "field_comparisons": 0,
    }
    if source_ids != fresh_ids:
        return result
    source = source.loc[sorted(source_ids)]
    fresh = fresh.loc[sorted(source_ids)]
    common_columns = sorted(set(source.columns) & set(fresh.columns))
    result["field_comparisons"] = len(source_ids) * len(common_columns)
    for column in common_columns:
        equal = equal_column(source[column], fresh[column])
        mismatch_ids = equal.index[~equal].tolist()
        if mismatch_ids:
            result["field_mismatches"][column] = {
                "count": len(mismatch_ids),
                "samples": [
                    {
                        "id": identity,
                        "source": json_scalar(source.at[identity, column]),
                        "fresh": json_scalar(fresh.at[identity, column]),
                    }
                    for identity in mismatch_ids[:10]
                ],
            }
    return result


def frame_has_difference(result: dict[str, Any]) -> bool:
    return any(
        result.get(key)
        for key in (
            "missing_ids",
            "extra_ids",
            "source_only_columns",
            "fresh_only_columns",
            "field_mismatches",
            "comparison_error",
        )
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("source", type=Path, help="local RTE 7k XIIDM file")
    parser.add_argument("--powerio", type=Path, default=Path("target/debug/powerio"))
    parser.add_argument("--powerio-repository", type=Path, default=Path.cwd())
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()

    args.output_dir.mkdir(parents=True, exist_ok=True)
    metadata = source_metadata(args.source)
    metadata_errors = {
        field: {"expected": EXPECTED_SOURCE[field], "actual": metadata[field]}
        for field in ("sha256", "bytes", "lines")
        if metadata[field] != EXPECTED_SOURCE[field]
    }
    if metadata_errors:
        raise SystemExit(f"RTE 7k source does not match the pinned case: {metadata_errors}")
    if pp.__version__ != EXPECTED_PYPOWSYBL_VERSION:
        raise SystemExit(
            f"PyPowSybl {pp.__version__} is installed; expected {EXPECTED_PYPOWSYBL_VERSION}"
        )
    core = powsybl_version()
    if core != {
        "version": EXPECTED_POWSYBL_CORE_VERSION,
        "commit": EXPECTED_POWSYBL_CORE_COMMIT,
    }:
        raise SystemExit(
            f"PowSybl Core {core} is installed; expected version "
            f"{EXPECTED_POWSYBL_CORE_VERSION} at {EXPECTED_POWSYBL_CORE_COMMIT}"
        )

    ir = args.output_dir / "rte7000.pio.json"
    fresh = args.output_dir / "rte7000-fresh.xiidm"
    report_path = args.output_dir / "rte7000-report.json"
    started = time.perf_counter()
    parse_and_serialize = run_command(
        [
            str(args.powerio),
            "serialize",
            str(args.source),
            "--from",
            "xiidm",
            "-o",
            str(ir),
        ],
        args.output_dir / "parse-and-serialize.log",
    )
    deserialize_and_emit = run_command(
        [str(args.powerio), "convert", str(ir), "--to", "xiidm", "-o", str(fresh)],
        args.output_dir / "deserialize-and-emit.log",
    )
    source_network, source_load = timed_load(args.source)
    fresh_network, fresh_load = timed_load(fresh)

    report: dict[str, Any] = {
        "source": metadata,
        "powerio": {
            "binary": str(args.powerio.resolve()),
            **powerio_revision(args.powerio_repository),
        },
        "powsybl": {"pypowsybl": pp.__version__, "core": core},
        "artifacts": {"ir": str(ir.resolve()), "fresh_xiidm": str(fresh.resolve())},
        "wall_time_seconds": {
            "parse_and_serialize": parse_and_serialize,
            "deserialize_and_emit": deserialize_and_emit,
            "pypowsybl_load_source": source_load,
            "pypowsybl_load_fresh": fresh_load,
            "total": time.perf_counter() - started,
        },
        "validation": {
            "source": validation(source_network),
            "fresh": validation(fresh_network),
        },
        "tolerance": {
            "relative": RELATIVE_TOLERANCE,
            "absolute": ABSOLUTE_TOLERANCE,
        },
        "frames": {},
        "expected_source_counts": EXPECTED_COUNTS,
        "diagnostics": diagnostic_counts(
            ir,
            args.output_dir / "deserialize-and-emit.log",
        ),
    }
    for label, function in FRAMES.items():
        try:
            report["frames"][label] = compare_frame(
                label, function(source_network), function(fresh_network)
            )
        except Exception as error:
            report["frames"][label] = {"comparison_error": str(error)}

    count_errors = {
        label: {
            "expected": expected,
            "actual": report["frames"].get(label, {}).get("source_count"),
        }
        for label, expected in EXPECTED_COUNTS.items()
        if report["frames"].get(label, {}).get("source_count") != expected
    }
    differences = {
        label: result
        for label, result in report["frames"].items()
        if frame_has_difference(result)
    }
    report["source_count_errors"] = count_errors
    report["frames_with_differences"] = sorted(differences)
    assertion_counts = {
        "pypowsybl_version": 1,
        "pinned_source_metadata": 3,
        "pinned_source_counts": len(EXPECTED_COUNTS),
        "validation_parity": 1,
        "row_identities": sum(
            result.get("identity_comparisons", 0) for result in report["frames"].values()
        ),
        "field_values": sum(
            result.get("field_comparisons", 0) for result in report["frames"].values()
        ),
    }
    assertion_counts["total"] = sum(assertion_counts.values())
    report["assertion_counts"] = assertion_counts
    report["validation_matches_source"] = (
        report["validation"]["source"] == report["validation"]["fresh"]
    )
    failures = []
    if count_errors:
        failures.append("source counts differ from the pinned RTE 7k case")
    if differences:
        failures.append("fresh XIIDM differs from the source network")
    if not report["validation_matches_source"]:
        failures.append("fresh XIIDM validation result differs from the source")
    report["failures"] = failures
    report_path.write_text(json.dumps(report, indent=2, sort_keys=True), encoding="utf-8")

    summary = {
        "source": metadata,
        "wall_time_seconds": report["wall_time_seconds"],
        "validation": report["validation"],
        "frames_compared": len(report["frames"]),
        "frames_with_differences": report["frames_with_differences"],
        "source_count_errors": count_errors,
        "assertion_counts": assertion_counts,
        "validation_matches_source": report["validation_matches_source"],
        "powerio": report["powerio"],
        "powsybl": report["powsybl"],
        "diagnostics": report["diagnostics"],
        "failures": failures,
        "report": str(report_path.resolve()),
    }
    print(json.dumps(summary, indent=2, sort_keys=True))
    return int(
        bool(count_errors)
        or bool(differences)
        or not report["validation_matches_source"]
    )


if __name__ == "__main__":
    raise SystemExit(main())
