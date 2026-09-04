"""MCP tools backed directly by the public :mod:`powerio` Python API.

Every input becomes a :class:`powerio.PioModule`. ``powerio_ir`` is serialized
PowerIO IR; external grid exchange data enters through ``path`` or ``content``.
Collections remain collections. Tools that inspect a collection use its normal
time index or scenario ID and never turn an operating point into a network.
"""

from __future__ import annotations

import base64
import io
import json
from pathlib import Path
from typing import Annotated, Any, Dict, Literal, Optional, cast

from mcp.server.mcpserver import MCPServer
from pydantic import Field

import powerio
from powerio import dist
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
    }
)
_MATRIX_HELP = ", ".join(sorted(_MATRIX_NAMES))
_MATRIX_KIND_FIELD = Field(
    json_schema_extra=cast(Any, {"enum": sorted(_MATRIX_NAMES)})
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
        text = f"{text} | {json.dumps(diagnostics)}"
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


def _canonical_type(module: "powerio.PioModule") -> str:
    return str(module._inner._type_name)


def _diagnostic_record(item: Any) -> Dict[str, Any]:
    record: Dict[str, Any] = {
        "code": item.code,
        "severity": item.severity,
        "message": item.message,
        "target": getattr(item, "target", None),
    }
    identity = getattr(item, "id", None)
    if identity:
        record["id"] = identity
    spans = getattr(item, "spans", None)
    if spans:
        record["spans"] = [
            {
                "source": span.source,
                "byte_start": span.byte_start,
                "byte_end": span.byte_end,
            }
            for span in spans
        ]
    return record


def _diagnostic_records(items: Any) -> list[Dict[str, Any]]:
    return [_diagnostic_record(item) for item in items]


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
    try:
        module = powerio.deserialize(io.StringIO(powerio_ir))
    except (powerio.PowerIOError, ValueError, TypeError) as exc:
        raise _coded_error("PowerIO IR", exc) from exc
    records = _diagnostic_records(module.diagnostics)
    return {
        "value_type": _canonical_type(module),
        "summary": _diagnostics_summary(records),
        "diagnostics": records,
    }


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
            selection["time_index"] = remaining_time
            value = value[remaining_time]
            remaining_time = None
            continue
        if isinstance(value, powerio.ScenarioSet) and remaining_scenario is not None:
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
        "module_value_type": _canonical_type(module),
        **_value_summary(value),
        "diagnostics": _diagnostic_records(module.diagnostics),
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
        "diagnostics": _diagnostic_records(result.diagnostics),
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
    records = _diagnostic_records(module.diagnostics)
    return {
        "value_type": _canonical_type(module),
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
        "value_type": _canonical_type(normalized),
        "powerio_ir": _serialize_module_text(normalized),
        "summary": _summary_payload(normalized),
    }


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
) -> Dict[str, Any]:
    canonical = matrix.lower()
    if canonical not in _MATRIX_NAMES:
        raise ValueError(f"unknown matrix {matrix!r}; expected one of: {_MATRIX_HELP}")
    module = _load_module(
        path=path, content=content, powerio_ir=powerio_ir, format=format
    )
    value, selection = _select_value(
        module.value, time_index=time_index, scenario_id=scenario_id
    )
    if not isinstance(value, powerio.BalancedNetwork):
        raise ValueError("matrix calculations require a BalancedNetwork")
    try:
        if canonical == "bprime":
            result = value.calc_bprime_matrix(scheme)
        elif canonical == "bdoubleprime":
            result = value.calc_bdoubleprime_matrix(scheme)
        elif canonical in ("admittance_real", "admittance_imag"):
            admittance = value.calc_admittance_matrix()
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
            result = value.calc_lacpf_matrix()
        elif canonical == "weighted_laplacian":
            result = value.calc_weighted_laplacian(formula)
        else:
            raise AssertionError(f"unhandled matrix name: {canonical}")
    except ImportError as exc:
        raise ValueError(str(exc)) from exc
    except powerio.PowerIOError as exc:
        raise _coded_error("matrix calculation failed", exc) from exc
    coo = result.tocoo()
    payload: Dict[str, Any] = {
        "value_type": type(value).__name__,
        "matrix": canonical,
        "format": "coo",
        "shape": [int(coo.shape[0]), int(coo.shape[1])],
        "nnz": int(coo.nnz),
        "data": coo.data.tolist(),
        "row": coo.row.tolist(),
        "col": coo.col.tolist(),
        "diagnostics": _diagnostic_records(module.diagnostics),
    }
    if selection:
        payload["selection"] = selection
    return payload


def _display_impl(path: str, format: Optional[str] = None) -> Dict[str, Any]:
    checked = sandbox.checked_path(path, purpose="path")
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


@mcp.tool(
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


@mcp.tool(
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


@mcp.tool(
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


@mcp.tool(name="diagnostics")
def _diagnostics_tool(powerio_ir: str) -> dict:
    """Return the diagnostics stored on a serialized PowerIO module."""
    return _diagnostics_payload(powerio_ir)


@mcp.tool(
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


@mcp.tool(
    name="calc_matrix",
    description="Calculate a BalancedNetwork matrix in COO form. "
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
    )


@mcp.tool(name="display")
def _display_tool(path: str, format: Optional[str] = None) -> dict:
    """Parse a PowerWorld display file."""
    return _display_impl(path, format)


@mcp.tool(
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


@mcp.tool(
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
        "value_type": _canonical_type(converted),
        "powerio_ir": _serialize_module_text(converted),
        "summary": _summary_payload(converted),
    }


@mcp.tool(
    name="about",
    description="Return the PowerIO version, IR version, feature set, and MCP tools.",
)
def _about_tool() -> dict:
    return {
        **powerio.versions(),
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
    )


def display(path: str, format: Optional[str] = None) -> dict:
    return _display_impl(path, format)


def diagnostics(powerio_ir: str) -> dict:
    return _diagnostics_payload(powerio_ir)


def main() -> None:
    """Serve the PowerIO MCP tools over stdio."""
    mcp.run()
