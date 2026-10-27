"""MCP tools backed directly by the public :mod:`powerio` Python API.

Every input becomes a :class:`powerio.PioModule`. ``powerio_ir`` is serialized
PowerIO IR; external grid exchange data enters through ``path`` or ``content``.
Tools select collection entries by time index or scenario ID. Matrix tools
use the network carried by a balanced operating point.
"""

from __future__ import annotations

import base64
import functools
import io
import json
import operator
import os
import stat
import sys
from pathlib import Path
from typing import (
    Annotated,
    Any,
    Callable,
    Dict,
    Literal,
    Mapping,
    MutableMapping,
    Optional,
    TypeVar,
    cast,
)

from mcp.server.mcpserver import MCPServer
from mcp.server.mcpserver.exceptions import ToolError
from pydantic import Field

import powerio
from powerio import diagnostic_records, dist
from powerio.mcp import sandbox

mcp = MCPServer("powerio")

_DIRECTORY_FORMATS = frozenset({"dss", "gridfm", "opendss", "pypsa", "pypsa-csv"})
_MATRIX_NAMES = frozenset(
    {
        "bprime",
        "bdoubleprime",
        "admittance_real",
        "admittance_imag",
        "adjacency",
        "ptdf",
        "lodf",
        "weighted_laplacian",
        "lacpf",
        "incidence",
        "branch_susceptances",
        "bus_susceptance",
        "branch_flow",
        "branch_phase_shift_injection",
        "bus_phase_shift_injection",
    }
)
# Axis names: "bus" rows or columns map to `bus_ids`, "branch" to `branch_ids`
# of the calculation index map. LACPF maps voltage deviations to power injections.
_MATRIX_AXES = {
    "bprime": ("bus", "bus"),
    "bdoubleprime": ("bus", "bus"),
    "admittance_real": ("bus", "bus"),
    "admittance_imag": ("bus", "bus"),
    "adjacency": ("bus", "bus"),
    "ptdf": ("branch", "bus"),
    "lodf": ("branch", "branch"),
    "weighted_laplacian": ("bus", "bus"),
    "lacpf": ("bus_power", "bus_voltage"),
    "incidence": ("branch", "bus"),
    "branch_susceptances": ("branch", None),
    "bus_susceptance": ("bus", "bus"),
    "branch_flow": ("branch", "bus"),
    "branch_phase_shift_injection": ("branch", None),
    "bus_phase_shift_injection": ("bus", None),
}
_MATRIX_HELP = ", ".join(sorted(_MATRIX_NAMES))
# The DC calculations `calc_matrix` reaches that take `skip_zero_impedance`.
# `calc_branch_flow_dc` and `calc_bus_injection_dc` also take it but need
# voltage angles, so they are not matrix names. `ptdf` and `lodf` factor the
# reference grounded DC matrix, `adjacency` and `weighted_laplacian` build on
# the graph, and none of the four has such an option.
_SKIP_ZERO_IMPEDANCE_DC_NAMES = (
    "incidence",
    "branch_susceptances",
    "bus_susceptance",
    "branch_flow",
    "branch_phase_shift_injection",
    "bus_phase_shift_injection",
)
_SKIP_ZERO_IMPEDANCE_DC_HELP = ", ".join(_SKIP_ZERO_IMPEDANCE_DC_NAMES)
# Matrix names whose calculation has no such option; the flag is refused so the
# payload never echoes a choice the result ignored.
_SKIP_ZERO_IMPEDANCE_REFUSED = ("ptdf", "lodf", "adjacency", "weighted_laplacian")
_MATRIX_KIND_FIELD = Field(
    json_schema_extra=cast(Any, {"enum": sorted(_MATRIX_NAMES)})
)
_SKIP_ZERO_IMPEDANCE_FIELD = Field(
    description=(
        "Drop zero impedance branches instead of failing the build. Six DC "
        f"calculations take it ({_SKIP_ZERO_IMPEDANCE_DC_HELP}), as do "
        "bprime, bdoubleprime, admittance_real, admittance_imag and lacpf. "
        "ptdf, lodf, adjacency and weighted_laplacian reject it."
    )
)
_Scheme = Literal["bx", "xb"]
_BranchSusceptanceFormula = Literal[
    "series_susceptance", "tap_adjusted_reactance", "reactance_only"
]

_SOURCE_FORMAT_HELP = (
    "Use `format` for an explicit grid exchange format; omit it to let "
    "PowerIO inspect a path, extension, or content markers. Raw text belongs "
    "in `content`; serialized PowerIO IR belongs in `powerio_ir`."
)

# Tool arguments come from the client, so the server holds them to service
# sized byte limits rather than the 1 GiB library defaults meant for local use.
_PRIMARY_BYTES_ENV = "POWERIO_MAX_PRIMARY_BYTES"
_REFERENCED_BYTES_ENV = "POWERIO_MAX_REFERENCED_BYTES"
_SERVICE_BYTE_LIMIT = 256 << 20
# Characters encoded at a time while counting an argument's UTF-8 bytes.
_UTF8_COUNT_CHUNK = 1 << 20


def _apply_service_limits(environ: MutableMapping[str, str] = os.environ) -> None:
    """Default both acquisition limits to 256 MiB for the server process.

    Only a setting the operator left unset or empty is filled in; any other
    value stays as it is, and PowerIO validates it when it reads a source.
    """
    for name in (_PRIMARY_BYTES_ENV, _REFERENCED_BYTES_ENV):
        if not environ.get(name):
            environ[name] = str(_SERVICE_BYTE_LIMIT)


def _primary_byte_limit() -> int:
    """The byte limit for inline arguments and display files.

    ``POWERIO_MAX_PRIMARY_BYTES`` when it holds a positive decimal byte count
    that PowerIO accepts; otherwise the 256 MiB service default.
    """
    value = os.environ.get(_PRIMARY_BYTES_ENV, "")
    if value.isascii() and value.isdigit():
        limit = int(value)
        if 0 < limit <= sys.maxsize:
            return limit
    return _SERVICE_BYTE_LIMIT


def _over_limit(name: str, limit: int) -> ValueError:
    return ValueError(
        f"READ.IO.PRIMARY_BUDGET: `{name}` exceeds the {limit} byte input limit; "
        f"set {_PRIMARY_BYTES_ENV} to raise it"
    )


def _check_inline_size(name: str, value: str) -> None:
    """Refuse an inline argument whose UTF-8 encoding exceeds the limit.

    A string of ``n`` characters encodes to between ``n`` and ``4n`` bytes,
    so only a string between those bounds is counted, a chunk at a time,
    stopping once the count passes the limit.
    """
    limit = _primary_byte_limit()
    if len(value) > limit:
        raise _over_limit(name, limit)
    if value.isascii() or len(value) * 4 <= limit:
        return
    total = 0
    for start in range(0, len(value), _UTF8_COUNT_CHUNK):
        chunk = value[start : start + _UTF8_COUNT_CHUNK]
        total += len(chunk.encode("utf-8", "surrogatepass"))
        if total > limit:
            raise _over_limit(name, limit)


def _fmt(value: Optional[str]) -> Optional[str]:
    return value.strip().lower().replace("_", "-") if value is not None else None


def _coded_error(prefix: str, exc: Exception) -> ValueError:
    code = getattr(exc, "code", None)
    text = str(exc)
    if code and not text.startswith(f"{code}: "):
        text = f"{code}: {text}"
    elif not code:
        text = f"{prefix}: {text}"
    diagnostics = getattr(exc, "diagnostics", None)
    if diagnostics:
        records = [
            item if isinstance(item, dict) else powerio.diagnostic_record(item)
            for item in diagnostics
        ]
        text = f"{text} | {json.dumps(records)}"
    return ValueError(text)


def _checked_source_path(value: str) -> str:
    path = sandbox.checked_path(value, purpose="path")
    if Path(path).is_dir():
        sandbox.check_allowed_read_tree(Path(path), purpose="path")
    return path


def _one_module_input(
    path: Optional[str], content: Optional[str], powerio_ir: Optional[str]
) -> tuple[Optional[str], Optional[str], Optional[str]]:
    content = content or None
    powerio_ir = powerio_ir or None
    if sum(item is not None for item in (path, content, powerio_ir)) != 1:
        raise ValueError("provide exactly one of `path`, `content`, or `powerio_ir`")
    return path, content, powerio_ir


def _load_module(
    *,
    path: Optional[str] = None,
    content: Optional[str] = None,
    powerio_ir: Optional[str] = None,
    format: Optional[str] = None,
) -> "powerio.PioModule":
    path, content, powerio_ir = _one_module_input(path, content, powerio_ir)
    if content is not None:
        _check_inline_size("content", content)
    if powerio_ir is not None:
        _check_inline_size("powerio_ir", powerio_ir)
    try:
        if powerio_ir is not None:
            return powerio.deserialize(io.StringIO(powerio_ir))
        if path is not None:
            return powerio.parse(_checked_source_path(path), format=_fmt(format))
        return powerio.parse(
            io.StringIO(content or ""), format=_fmt(format), name="mcp-input"
        )
    except powerio.PowerIOError as exc:
        raise _coded_error("PowerIO input", exc) from exc
    except sandbox.PathNotAllowed:
        raise
    except (OSError, ValueError, TypeError) as exc:
        raise _coded_error("PowerIO input", exc) from exc


def _serialize_module_text(module: "powerio.PioModule") -> str:
    result = powerio.serialize(module)
    if result.text is None:
        raise RuntimeError("PowerIO serialization returned no text")
    return result.text


def _diagnostics_summary(records: list[Dict[str, Any]]) -> Dict[str, Any]:
    counts = {name: 0 for name in ("error", "warning", "remark", "note")}
    for record in records:
        severity = record.get("severity")
        if severity in counts:
            counts[severity] += 1
    if counts["error"]:
        status = "error"
    elif counts["warning"]:
        status = "warning"
    elif counts["remark"] or counts["note"]:
        status = "info"
    else:
        status = "ok"
    nonzero = [f"{name}={count}" for name, count in counts.items() if count]
    text = "ok: no diagnostics" if not nonzero else f"{status}: " + ", ".join(nonzero)
    return {"status": status, "counts": counts, "text": text}


def _diagnostics_payload(powerio_ir: str) -> Dict[str, Any]:
    _check_inline_size("powerio_ir", powerio_ir)
    try:
        module = powerio.deserialize(io.StringIO(powerio_ir))
    except (powerio.PowerIOError, ValueError, TypeError) as exc:
        raise _coded_error("PowerIO IR", exc) from exc
    records = diagnostic_records(module.diagnostics)
    return {
        "value_type": module.type_name,
        "summary": _diagnostics_summary(records),
        "diagnostics": records,
    }


class _TimeIndexOutOfRange(IndexError):
    """A ``time_index`` outside the selected TimeSeries."""


class _UnknownScenarioId(KeyError):
    """A ``scenario_id`` the selected ScenarioSet does not hold."""

    def __str__(self) -> str:
        # KeyError quotes its argument; this message is already a sentence.
        return str(self.args[0])


# Scenario IDs an unknown `scenario_id` refusal names before it counts the rest.
_LISTED_SCENARIO_IDS = 20


def _time_index_out_of_range(index: int, length: int) -> _TimeIndexOutOfRange:
    if length == 0:
        held = "the TimeSeries has no time points"
    elif length == 1:
        held = "the TimeSeries has 1 time point, index 0"
    else:
        held = f"the TimeSeries has {length} time points, indices 0 to {length - 1}"
    return _TimeIndexOutOfRange(f"`time_index` {index} is out of range: {held}")


def _unknown_scenario_id(
    scenario_id: str, scenarios: "powerio.ScenarioSet"
) -> _UnknownScenarioId:
    ids = list(scenarios)
    if not ids:
        held = "the ScenarioSet has no scenarios"
    else:
        listed = ", ".join(repr(item) for item in ids[:_LISTED_SCENARIO_IDS])
        if len(ids) > _LISTED_SCENARIO_IDS:
            listed += f", and {len(ids) - _LISTED_SCENARIO_IDS} more"
        held = f"the ScenarioSet holds {listed}"
    return _UnknownScenarioId(f"unknown `scenario_id` {scenario_id!r}: {held}")


def _integer_index(value: Any) -> Optional[int]:
    try:
        return operator.index(value)
    except TypeError:
        return None


def _select_value(
    value: Any,
    *,
    time_index: Optional[int] = None,
    scenario_id: Optional[str] = None,
) -> tuple[Any, Dict[str, Any]]:
    remaining_time = time_index
    remaining_scenario = scenario_id
    selection: Dict[str, Any] = {}
    while remaining_time is not None or remaining_scenario is not None:
        if isinstance(value, powerio.TimeSeries) and remaining_time is not None:
            # A selector that is not an integer goes on to the TimeSeries,
            # which raises its own TypeError.
            position = _integer_index(remaining_time)
            length = len(value)
            if position is not None and not -length <= position < length:
                raise _time_index_out_of_range(position, length)
            selection["time_index"] = remaining_time
            value = value[remaining_time]
            remaining_time = None
            continue
        if isinstance(value, powerio.ScenarioSet) and remaining_scenario is not None:
            # A selector that is not a string goes on to the ScenarioSet,
            # which raises its own TypeError.
            if isinstance(remaining_scenario, str) and remaining_scenario not in value:
                raise _unknown_scenario_id(remaining_scenario, value)
            selection["scenario_id"] = remaining_scenario
            value = value[remaining_scenario]
            remaining_scenario = None
            continue
        if remaining_time is not None:
            raise ValueError("`time_index` requires a TimeSeries at that position")
        raise ValueError("`scenario_id` requires a ScenarioSet at that position")
    return value, selection


def _balanced_summary(net: "powerio.BalancedNetwork") -> Dict[str, Any]:
    return {
        "domain": "transmission",
        "electrical_model": "balanced",
        "name": net.name,
        "source_format": net.source_format,
        "base_mva": net.base_mva,
        "elements": {
            "buses": net.n_buses,
            "branches": net.n_branches,
            "generators": net.n_generators,
            "loads": net.n_loads,
            "shunts": net.n_shunts,
        },
        "topology": {
            "connected_components": net.n_islands,
            "is_radial": net.is_radial,
            "reference_buses": net.reference_bus_indices(),
            "connectivity_report": net.calc_connectivity_report(),
        },
    }


def _multiconductor_summary(net: "dist.MulticonductorNetwork") -> Dict[str, Any]:
    return {
        "domain": "distribution",
        "electrical_model": "multiconductor",
        "name": net.name,
        "source_format": net.source_format,
        "elements": {
            "buses": net.n_buses,
            "lines": net.n_lines,
            "transformers": net.n_transformers,
            "generators": net.n_generators,
            "loads": net.n_loads,
            "voltage_sources": net.n_voltage_sources,
        },
    }


def _value_summary(value: Any) -> Dict[str, Any]:
    if isinstance(value, powerio.BalancedNetwork):
        summary = _balanced_summary(value)
    elif isinstance(value, dist.MulticonductorNetwork):
        summary = _multiconductor_summary(value)
    elif isinstance(value, powerio.TimeSeries):
        summary = {
            "collection": "TimeSeries",
            "length": len(value),
            "time_points": [
                {
                    "index": index,
                    "label": point.label,
                    "duration_seconds": point.duration_seconds,
                }
                for index, point in enumerate(value.time_points)
            ],
        }
    elif isinstance(value, powerio.ScenarioSet):
        summary = {
            "collection": "ScenarioSet",
            "length": len(value),
            "scenarios": [
                {"id": scenario.id, "probability": scenario.probability}
                for scenario in value.scenarios
            ],
        }
    elif isinstance(value, powerio.OperatingPoint):
        summary = {
            "operating_point": True,
            "network": _balanced_summary(value.network)
            if value.module.type_name == "powerio.OperatingPoint<powerio.BalancedNetwork>"
            else None,
        }
    elif isinstance(value, powerio.GeoLayer):
        layer = json.loads(value.geojson)
        summary = {
            "layer": "GeoLayer",
            "features": len(layer.get("features", [])),
        }
    elif isinstance(
        value, (powerio.ContingencySet, powerio.SubsystemSet, powerio.MonitoredSet)
    ):
        summary = {"lines": len(value.text.splitlines())}
    else:
        summary = {"calculation": type(value).__name__}
    return {"value_type": type(value).__name__, **summary}


def _summary_payload(
    module: "powerio.PioModule",
    *,
    time_index: Optional[int] = None,
    scenario_id: Optional[str] = None,
) -> Dict[str, Any]:
    value, selection = _select_value(
        module.value, time_index=time_index, scenario_id=scenario_id
    )
    payload = {
        "module_value_type": module.type_name,
        **_value_summary(value),
        "diagnostics": diagnostic_records(module.diagnostics),
    }
    if selection:
        payload["selection"] = selection
    return payload


def _artifact_payload(artifact: "powerio.Artifact") -> Dict[str, Any]:
    item: Dict[str, Any] = {
        "name": artifact.name,
        "path": artifact.path,
        "size": len(artifact.data) if artifact.data is not None else None,
    }
    if artifact.data is not None:
        try:
            item["text"] = artifact.data.decode("utf-8")
        except UnicodeDecodeError:
            item["data_base64"] = base64.b64encode(artifact.data).decode("ascii")
    return item


def _emit_result_payload(result: "powerio.EmitResult") -> Dict[str, Any]:
    artifacts = [_artifact_payload(artifact) for artifact in result.artifacts]
    payload: Dict[str, Any] = {
        "layout": result.layout,
        "fidelity": result.fidelity,
        "artifacts": artifacts,
        "diagnostics": diagnostic_records(result.diagnostics),
    }
    if len(artifacts) == 1 and "text" in artifacts[0]:
        payload["text"] = artifacts[0]["text"]
    return payload


def _emit_module(
    module: "powerio.PioModule",
    format: str,
    destination: Optional[str],
    overwrite: bool,
) -> Dict[str, Any]:
    if not format:
        raise ValueError("`format` is required")
    try:
        if destination is None:
            return _emit_result_payload(powerio.emit(module, format))
        checked = sandbox.checked_path(
            destination, purpose="destination", for_write=True
        )
        if _fmt(format) in _DIRECTORY_FORMATS:

            def write_directory(staging: str) -> Dict[str, Any]:
                result = powerio.emit(module, format, staging)
                files = sorted(
                    str(path) for path in Path(staging).rglob("*") if path.is_file()
                )
                return {
                    "dir": staging,
                    "files": files,
                    **_emit_result_payload(result),
                }

            return sandbox.staged_directory_write(checked, overwrite, write_directory)
        def write_file(staging: str) -> Dict[str, Any]:
            result = powerio.emit(module, format, staging)
            return {
                "path": staging,
                **_emit_result_payload(result),
            }

        return sandbox.staged_file_write(checked, overwrite, write_file)
    except powerio.PowerIOError as exc:
        raise _coded_error("emission failed", exc) from exc
    except OSError as exc:
        raise _coded_error("emission failed", exc) from exc


def _parse_impl(
    path: Optional[str] = None,
    content: Optional[str] = None,
    format: Optional[str] = None,
) -> Dict[str, Any]:
    module = _load_module(path=path, content=content, format=format)
    records = diagnostic_records(module.diagnostics)
    return {
        "value_type": module.type_name,
        "powerio_ir": _serialize_module_text(module),
        "summary": _summary_payload(module),
        "diagnostics": records,
        "diagnostics_summary": _diagnostics_summary(records),
    }


def _summarize_impl(
    *,
    path: Optional[str] = None,
    content: Optional[str] = None,
    powerio_ir: Optional[str] = None,
    format: Optional[str] = None,
    time_index: Optional[int] = None,
    scenario_id: Optional[str] = None,
) -> Dict[str, Any]:
    module = _load_module(
        path=path, content=content, powerio_ir=powerio_ir, format=format
    )
    return _summary_payload(module, time_index=time_index, scenario_id=scenario_id)


def _normalize_impl(
    *,
    path: Optional[str] = None,
    content: Optional[str] = None,
    powerio_ir: Optional[str] = None,
    format: Optional[str] = None,
) -> Dict[str, Any]:
    module = _load_module(
        path=path, content=content, powerio_ir=powerio_ir, format=format
    )
    value = module.value
    if not isinstance(value, powerio.BalancedNetwork):
        raise ValueError("to_normalized requires a BalancedNetwork")
    try:
        normalized = powerio.PioModule.from_value(value.to_normalized())
    except powerio.PowerIOError as exc:
        raise _coded_error("normalization failed", exc) from exc
    return {
        "value_type": normalized.type_name,
        "powerio_ir": _serialize_module_text(normalized),
        "summary": _summary_payload(normalized),
    }


def _axis_ids(axis: Optional[str], index_map: Mapping[str, Any]) -> Optional[list]:
    if axis == "bus":
        return list(index_map["bus_ids"])
    if axis == "branch":
        return list(index_map["branch_ids"])
    if axis in ("bus_power", "bus_voltage"):
        buses = list(index_map["bus_ids"])
        quantities = ("p", "q") if axis == "bus_power" else ("vm", "va")
        return [f"{bus}:{quantity}" for quantity in quantities for bus in buses]
    return None


def _matrix_impl(
    matrix: str,
    *,
    path: Optional[str] = None,
    content: Optional[str] = None,
    powerio_ir: Optional[str] = None,
    format: Optional[str] = None,
    time_index: Optional[int] = None,
    scenario_id: Optional[str] = None,
    scheme: _Scheme = "bx",
    formula: _BranchSusceptanceFormula = "series_susceptance",
    skip_zero_impedance: bool = False,
) -> Dict[str, Any]:
    canonical = matrix.lower()
    if canonical not in _MATRIX_NAMES:
        raise ValueError(f"unknown matrix {matrix!r}; expected one of: {_MATRIX_HELP}")
    # These calculations have no such option, so the flag would be echoed
    # back in the payload while the result ignored it.
    if skip_zero_impedance and canonical in _SKIP_ZERO_IMPEDANCE_REFUSED:
        raise ValueError(
            f"{canonical} does not take skip_zero_impedance; the DC "
            f"calculations that do are: {_SKIP_ZERO_IMPEDANCE_DC_HELP}"
        )
    module = _load_module(
        path=path, content=content, powerio_ir=powerio_ir, format=format
    )
    value, selection = _select_value(
        module.value, time_index=time_index, scenario_id=scenario_id
    )
    if isinstance(value, powerio.OperatingPoint):
        if value.module.type_name != "powerio.OperatingPoint<powerio.BalancedNetwork>":
            raise ValueError("matrix calculations require a BalancedNetwork")
        value = value.network
    if not isinstance(value, powerio.BalancedNetwork):
        raise ValueError("matrix calculations require a BalancedNetwork")
    skip = skip_zero_impedance
    try:
        if canonical == "bprime":
            result = value.calc_bprime_matrix(scheme, skip_zero_impedance=skip)
        elif canonical == "bdoubleprime":
            result = value.calc_bdoubleprime_matrix(scheme, skip_zero_impedance=skip)
        elif canonical in ("admittance_real", "admittance_imag"):
            admittance = value.calc_admittance_matrix(skip_zero_impedance=skip)
            result = (
                admittance.real
                if canonical == "admittance_real"
                else admittance.imag
            )
        elif canonical == "adjacency":
            result = value.calc_adjacency_matrix()
        elif canonical == "ptdf":
            result = value.calc_ptdf(formula)
        elif canonical == "lodf":
            result = value.calc_lodf(formula)
        elif canonical == "lacpf":
            result = value.calc_lacpf_matrix(skip_zero_impedance=skip)
        elif canonical == "weighted_laplacian":
            result = value.calc_weighted_laplacian(formula)
        elif canonical == "incidence":
            result = value.calc_incidence_matrix(formula, skip_zero_impedance=skip)
        elif canonical == "bus_susceptance":
            result = value.calc_bus_susceptance_matrix(
                formula, skip_zero_impedance=skip
            )
        elif canonical == "branch_flow":
            result = value.calc_branch_flow_matrix(formula, skip_zero_impedance=skip)
        elif canonical == "branch_susceptances":
            result = value.calc_branch_susceptances(formula, skip_zero_impedance=skip)
        elif canonical == "branch_phase_shift_injection":
            result = value.calc_branch_phase_shift_injection(
                formula, skip_zero_impedance=skip
            )
        elif canonical == "bus_phase_shift_injection":
            result = value.calc_bus_phase_shift_injection(
                formula, skip_zero_impedance=skip
            )
        else:
            raise AssertionError(f"unhandled matrix name: {canonical}")
        if canonical in _SKIP_ZERO_IMPEDANCE_DC_NAMES or canonical == "weighted_laplacian":
            index_map = value.calc_dc_index_map(formula, skip_zero_impedance=skip)
        else:
            index_map = value._inner._matrix_index_map(
                zero_resistance=(canonical, scheme) in (("bprime", "xb"), ("bdoubleprime", "bx")),
                skip_zero_impedance=skip,
            )
    except ImportError as exc:
        raise ValueError(str(exc)) from exc
    except powerio.PowerIOError as exc:
        raise _coded_error("matrix calculation failed", exc) from exc
    row_axis, col_axis = _MATRIX_AXES[canonical]
    payload: Dict[str, Any] = {
        "value_type": type(value).__name__,
        "matrix": canonical,
        "formula": formula,
        "skip_zero_impedance": skip,
    }
    if col_axis is None:
        values = [float(item) for item in result]
        payload.update(
            {
                "format": "vector",
                "shape": [len(values)],
                "data": values,
                "row_ids": _axis_ids(row_axis, index_map),
            }
        )
    else:
        coo = result.tocoo()
        payload.update(
            {
                "format": "coo",
                "shape": [int(coo.shape[0]), int(coo.shape[1])],
                "nnz": int(coo.nnz),
                "data": coo.data.tolist(),
                "row": coo.row.tolist(),
                "col": coo.col.tolist(),
                "row_ids": _axis_ids(row_axis, index_map),
                "col_ids": _axis_ids(col_axis, index_map),
            }
        )
    payload["skipped_branch_rows"] = list(index_map["skipped_branch_rows"])
    payload["diagnostics"] = diagnostic_records(module.diagnostics)
    if selection:
        payload["selection"] = selection
    return payload


def _checked_display_file(path: str) -> str:
    """Confine a display path and hold it to the primary byte limit.

    The display reader reads the whole file without a byte limit of its own,
    so the server requires a regular file no larger than the limit first.
    """
    checked = sandbox.checked_path(path, purpose="path")
    try:
        info = os.stat(checked)
    except OSError as exc:
        raise _coded_error("display parse failed", exc) from exc
    if not stat.S_ISREG(info.st_mode):
        raise ValueError(f"display `path` must be a regular file: {checked}")
    limit = _primary_byte_limit()
    if info.st_size > limit:
        raise _over_limit("path", limit)
    return checked


def _display_impl(path: str, format: Optional[str] = None) -> Dict[str, Any]:
    checked = _checked_display_file(path)
    try:
        data = powerio.parse_display(checked, format)
    except (powerio.PowerIOError, OSError, ValueError) as exc:
        raise _coded_error("display parse failed", exc) from exc
    if data.kind != "powerworld":
        raise ValueError(f"unsupported display format: {data.kind!r}")
    display = data.data
    return {
        "format": "powerworld-pwd",
        "canvas": {"width": display.canvas_width, "height": display.canvas_height},
        "stamp": display.stamp,
        "substations": [
            {"number": row.number, "name": row.name, "x": row.x, "y": row.y}
            for row in display.substations
        ],
    }


_MODULE_INPUT_HELP = (
    "Provide one of `powerio_ir`, `path`, or `content`. `powerio_ir` is "
    "serialized PowerIO IR; `path` and `content` are grid exchange data."
)

# What this module raises for input the caller can correct. Every such refusal
# is a ValueError (PowerIOError and sandbox.PathNotAllowed subclass it), apart
# from the two collection selection errors.
_REFUSALS = (ValueError, _TimeIndexOutOfRange, _UnknownScenarioId)

_F = TypeVar("_F", bound=Callable[..., Any])


def _tool(**options: Any) -> Callable[[_F], _F]:
    """Register a function as an MCP tool whose refusals reach the client.

    The SDK sends the client the message of a ``ToolError``. From mcp 2.1 it
    sends only ``Error executing tool <name>`` for any other exception, so a
    refusal raised as ``ValueError`` would lose its diagnostic code and
    reason. The registered callable raises each refusal again as a
    ``ToolError`` with the same message. Any other exception stays a failure
    the SDK logs with its traceback. The decorated function itself is returned
    unchanged, so a Python caller still receives the original exception.
    """

    def register(fn: _F) -> _F:
        @functools.wraps(fn)
        def call(*args: Any, **kwargs: Any) -> Any:
            try:
                return fn(*args, **kwargs)
            except _REFUSALS as exc:
                raise ToolError(str(exc)) from exc

        mcp.tool(**options)(call)
        return fn

    return register


@_tool(
    name="parse",
    description="Parse grid exchange data and return serialized PowerIO IR. "
    + _SOURCE_FORMAT_HELP,
)
def _parse_tool(
    path: Optional[str] = None,
    content: str = "",
    format: Optional[str] = None,
) -> dict:
    return _parse_impl(path, content, format)


@_tool(
    name="emit",
    description="Emit a PowerIO module in one grid exchange format. "
    + _MODULE_INPUT_HELP,
)
def _emit_tool(
    format: str,
    destination: Optional[str] = None,
    path: Optional[str] = None,
    content: str = "",
    powerio_ir: str = "",
    source_format: Optional[str] = None,
    overwrite: bool = False,
) -> dict:
    module = _load_module(
        path=path, content=content, powerio_ir=powerio_ir, format=source_format
    )
    return {
        **_emit_module(module, format, destination, overwrite),
    }


@_tool(
    name="summarize",
    description="Summarize a typed PowerIO value. Collection entries use a "
    "normal time index or scenario ID. "
    + _MODULE_INPUT_HELP,
)
def _summarize_tool(
    path: Optional[str] = None,
    content: str = "",
    powerio_ir: str = "",
    format: Optional[str] = None,
    time_index: Optional[int] = None,
    scenario_id: Optional[str] = None,
) -> dict:
    return _summarize_impl(
        path=path,
        content=content,
        powerio_ir=powerio_ir,
        format=format,
        time_index=time_index,
        scenario_id=scenario_id,
    )


@_tool(name="diagnostics")
def _diagnostics_tool(powerio_ir: str) -> dict:
    """Return the diagnostics stored on a serialized PowerIO module."""
    return _diagnostics_payload(powerio_ir)


@_tool(
    name="to_normalized",
    description="Normalize a BalancedNetwork and return serialized PowerIO IR. "
    + _MODULE_INPUT_HELP,
)
def _to_normalized_tool(
    path: Optional[str] = None,
    content: str = "",
    powerio_ir: str = "",
    format: Optional[str] = None,
) -> dict:
    return _normalize_impl(
        path=path, content=content, powerio_ir=powerio_ir, format=format
    )


@_tool(
    name="calc_matrix",
    description="Calculate a BalancedNetwork matrix in COO form, or a DC "
    "vector, with the bus ids and branch identities of every axis. "
    + _MODULE_INPUT_HELP,
)
def _calc_matrix_tool(
    matrix: Annotated[str, _MATRIX_KIND_FIELD],
    path: Optional[str] = None,
    content: str = "",
    powerio_ir: str = "",
    format: Optional[str] = None,
    time_index: Optional[int] = None,
    scenario_id: Optional[str] = None,
    scheme: _Scheme = "bx",
    formula: _BranchSusceptanceFormula = "series_susceptance",
    skip_zero_impedance: Annotated[bool, _SKIP_ZERO_IMPEDANCE_FIELD] = False,
) -> dict:
    return _matrix_impl(
        matrix,
        path=path,
        content=content,
        powerio_ir=powerio_ir,
        format=format,
        time_index=time_index,
        scenario_id=scenario_id,
        scheme=scheme,
        formula=formula,
        skip_zero_impedance=skip_zero_impedance,
    )


@_tool(name="display")
def _display_tool(path: str, format: Optional[str] = None) -> dict:
    """Return the raw record of a PowerWorld display file.

    Kept only for 0.11 compatibility and removed in 0.12; `parse` reads the
    same file to a GeoLayer with its diagnostics.
    """
    return _display_impl(path, format)


@_tool(
    name="to_balanced_report",
    description="Report whether a MulticonductorNetwork can become a "
    "BalancedNetwork. "
    + _MODULE_INPUT_HELP,
)
def _to_balanced_report_tool(
    powerio_ir: str = "",
    path: Optional[str] = None,
    content: str = "",
    format: Optional[str] = None,
    base_mva: float = 100.0,
) -> dict:
    module = _load_module(
        powerio_ir=powerio_ir, path=path, content=content, format=format
    )
    try:
        report = module.to_balanced_report(base_mva)
    except (powerio.PowerIOError, ValueError) as exc:
        raise _coded_error("balanced conversion report", exc) from exc
    return report


@_tool(
    name="to_balanced",
    description="Convert a MulticonductorNetwork to a BalancedNetwork and "
    "return serialized PowerIO IR. "
    + _MODULE_INPUT_HELP,
)
def _to_balanced_tool(
    powerio_ir: str = "",
    path: Optional[str] = None,
    content: str = "",
    format: Optional[str] = None,
    base_mva: float = 100.0,
) -> dict:
    module = _load_module(
        powerio_ir=powerio_ir, path=path, content=content, format=format
    )
    try:
        converted = module.to_balanced(base_mva)
    except (powerio.PowerIOError, ValueError) as exc:
        raise _coded_error("balanced conversion", exc) from exc
    return {
        "value_type": converted.type_name,
        "powerio_ir": _serialize_module_text(converted),
        "summary": _summary_payload(converted),
    }


@_tool(
    name="about",
    description="Return the PowerIO version, IR version, feature set, and MCP tools.",
)
def _about_tool() -> dict:
    return {
        **powerio.versions(),
        "features": powerio.features(),
        "tools": sorted(tool.name for tool in mcp._tool_manager.list_tools()),
    }


def parse(
    path: Optional[str] = None,
    content: Optional[str] = None,
    format: Optional[str] = None,
) -> dict:
    return _parse_impl(path, content, format)


def emit(
    format: str,
    destination: Optional[str] = None,
    path: Optional[str] = None,
    content: Optional[str] = None,
    powerio_ir: Optional[str] = None,
    source_format: Optional[str] = None,
    overwrite: bool = False,
) -> dict:
    module = _load_module(
        path=path, content=content, powerio_ir=powerio_ir, format=source_format
    )
    return {
        **_emit_module(module, format, destination, overwrite),
    }


def summarize(
    path: Optional[str] = None,
    content: Optional[str] = None,
    powerio_ir: Optional[str] = None,
    format: Optional[str] = None,
    time_index: Optional[int] = None,
    scenario_id: Optional[str] = None,
) -> dict:
    return _summarize_impl(
        path=path,
        content=content,
        powerio_ir=powerio_ir,
        format=format,
        time_index=time_index,
        scenario_id=scenario_id,
    )


def to_normalized(
    path: Optional[str] = None,
    content: Optional[str] = None,
    powerio_ir: Optional[str] = None,
    format: Optional[str] = None,
) -> dict:
    return _normalize_impl(
        path=path, content=content, powerio_ir=powerio_ir, format=format
    )


def calc_matrix(
    matrix: str,
    path: Optional[str] = None,
    content: Optional[str] = None,
    powerio_ir: Optional[str] = None,
    format: Optional[str] = None,
    time_index: Optional[int] = None,
    scenario_id: Optional[str] = None,
    scheme: _Scheme = "bx",
    formula: _BranchSusceptanceFormula = "series_susceptance",
    skip_zero_impedance: bool = False,
) -> dict:
    return _matrix_impl(
        matrix,
        path=path,
        content=content,
        powerio_ir=powerio_ir,
        format=format,
        time_index=time_index,
        scenario_id=scenario_id,
        scheme=scheme,
        formula=formula,
        skip_zero_impedance=skip_zero_impedance,
    )


def display(path: str, format: Optional[str] = None) -> dict:
    return _display_impl(path, format)


def diagnostics(powerio_ir: str) -> dict:
    return _diagnostics_payload(powerio_ir)


def main() -> None:
    """Serve the PowerIO MCP tools over stdio.

    The server process defaults ``POWERIO_MAX_PRIMARY_BYTES`` and
    ``POWERIO_MAX_REFERENCED_BYTES`` to 256 MiB when the operator leaves them
    unset or empty. Importing this module leaves the environment alone, so a
    program that calls the tool functions directly keeps the library defaults
    for what PowerIO reads; the checks this module makes itself, on inline
    arguments and display files, still default to 256 MiB.
    """
    _apply_service_limits()
    sandbox._DEFAULT_ROOT = Path.cwd().resolve(strict=True)
    mcp.run()
