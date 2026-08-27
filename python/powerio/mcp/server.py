"""MCP server for powerio.

The advertised MCP surface is semantic and format neutral:

``convert``, ``save``, ``summary``, ``parse``, ``normalize``, ``matrix``,
``diagnostics``, ``display``.

The tools route balanced transmission models, multiconductor distribution
models, PyPSA CSV folders, and gridfm datasets through the lower level powerio
APIs. Transmission parses serialize through the ``model-json`` transport.
Distribution parses serialize through canonical ``bmopf-json``. The stored
module transport serializes either family through `.pio.json`.

The filesystem containment policy for ``path`` and ``out_path`` lives in
``powerio.mcp.sandbox``, which imports no MCP SDK; the private helpers here
are wrappers over it. A dss parse passes the allowed root that admitted the
path as the reader's include root, so includes may span sibling directories
under that root; with no roots configured the reader's case directory default
applies.
"""

from __future__ import annotations

import json as jsonlib
import os
from dataclasses import dataclass
from pathlib import Path
from typing import Annotated, Any, Dict, Optional, TypeAlias

from mcp.server.mcpserver import MCPServer
from pydantic import Field

import powerio
from powerio import dist
from powerio.mcp import sandbox

mcp = MCPServer("powerio")

_DIST_FORMATS = frozenset(
    {
        "dss",
        "opendss",
        "pmd",
        "pmd-json",
        "pmd_json",
        "engineering",
        "bmopf",
        "bmopf-json",
        "bmopf_json",
    }
)
_GRIDFM_FORMATS = frozenset({"gridfm"})
_PYPSA_FORMATS = frozenset({"pypsa", "pypsa-csv"})
# The serial transport for a balanced model. ``powerio-json`` and its two
# short spellings were the token this transport carried before 0.9 and stay
# accepted here: the MCP schema is versioned with the Python package rather
# than the C ABI, so an input alias costs nothing and spares a client.
_MODEL_JSON_FORMATS = frozenset({"model-json", "powerio", "powerio-json", "json"})
_BMOPF_JSON_FORMATS = frozenset({"bmopf", "bmopf-json", "bmopf_json"})
_PACKAGE_JSON_FORMATS = frozenset(
    {"package", "pio", "pio-json", "pio_json", "pio-package", "pio_package"}
)
_VERSION_KEY = "powerio_version"

_MATRIX_KIND_ALIASES = {
    "b": "bprime",
    "b1": "bprime",
    "bprime": "bprime",
    "b2": "bdoubleprime",
    "bpp": "bdoubleprime",
    "bdoubleprime": "bdoubleprime",
    "g": "ybus_real",
    "ybus_real": "ybus_real",
    "negb": "ybus_imag",
    "b_lap": "ybus_imag",
    "ybus_imag": "ybus_imag",
    "adj": "adjacency",
    "adjacency": "adjacency",
    "ptdf": "ptdf",
    "lodf": "lodf",
    "laplacian": "laplacian",
    "lacpf": "lacpf",
}

_MATRIX_HELP = (
    "bprime/b/b1 (MATPOWER Bp), bdoubleprime/b2/bpp (MATPOWER Bpp), "
    "ybus_real/g, ybus_imag/negB/b_lap, adjacency/adj, ptdf, lodf, "
    "laplacian, lacpf"
)

# JSON schema `enum` entries the tool surface advertises. `json_schema_extra`
# documents without validating, so the historical aliases stay accepted.
_JsonFormatArg: TypeAlias = Optional[
    Annotated[
        str,
        Field(json_schema_extra={"enum": ["package", "model-json", "bmopf-json"]}),
    ]
]
# A FieldInfo constant rather than an `Annotated[str, ...]` alias: stubtest
# probes a str-aliased Annotated differently across python versions. The
# enum list is Any-typed for pydantic's invariant JsonValue list.
_MATRIX_KIND_ENUM: "list[Any]" = sorted(_MATRIX_KIND_ALIASES)
_MATRIX_KIND_FIELD = Field(json_schema_extra={"enum": _MATRIX_KIND_ENUM})

# Tool description prose for the open ended format name sets.
_SOURCE_FORMAT_HELP = (
    "Accepted `from_format` names — transmission: matpower, powermodels-json, "
    "egret-json, pandapower-json, psse, powerworld, pslf, goc3-json, "
    "surge-json, opfdata-json, pypsa-csv, gridfm; distribution: dss, pmd-json, "
    "bmopf-json. Omit it to infer from the file extension or JSON markers."
)
_TARGET_FORMAT_HELP = (
    "Accepted `to_format` names — transmission: matpower, psse, "
    "powermodels-json, egret-json, pandapower-json, powerworld, pslf; "
    "distribution: dss, pmd-json, bmopf-json. The folder targets pypsa-csv "
    "and gridfm go through `save`."
)


@dataclass
class _Loaded:
    domain: str
    network: Any
    warnings: list[str]
    json_format: str
    scenario: Optional[int] = None
    package_json: Optional[str] = None


def _fmt(value: Optional[str]) -> Optional[str]:
    return value.strip().lower().replace("_", "-") if value is not None else None


def _opts(options: Optional[Dict[str, Any]]) -> Dict[str, Any]:
    return dict(options or {})


def _one_input(path: Optional[str], content: Optional[str]) -> None:
    if (path is None) == (content is None):
        raise ValueError("provide exactly one of `path` or `content`")


def _coded_error(prefix: str, exc: Exception) -> ValueError:
    """Lead with the diagnostic code when the failure carries one, so a
    consumer that splits on the first colon reads a code, never prose."""
    code = getattr(exc, "code", None)
    if code:
        return ValueError(f"{code}: {exc}")
    return ValueError(f"{prefix}: {exc}")


def _required(value: Optional[str], name: str) -> str:
    """Narrow an argument an earlier check already settled.

    The input guards run at the top of each entry point, so a call site
    reaches this with the value set. The raise states the invariant for a
    type checker, which cannot see across the guard.
    """
    if value is None:
        raise ValueError(f"`{name}` is not set")
    return value


def _one_network_input(
    path: Optional[str],
    content: Optional[str],
    transport: Optional[str],
    package_json: Optional[str],
) -> None:
    if sum(v is not None for v in (path, content, transport, package_json)) != 1:
        raise ValueError(
            "provide exactly one of `path`, `content`, `json`, or `package_json`"
        )


def _is_dist_format(format: Optional[str]) -> bool:
    return _fmt(format) in _DIST_FORMATS


def _is_gridfm_format(format: Optional[str]) -> bool:
    return _fmt(format) in _GRIDFM_FORMATS


def _is_pypsa_format(format: Optional[str]) -> bool:
    return _fmt(format) in _PYPSA_FORMATS


def _looks_like_gridfm_dir(path: str) -> bool:
    p = Path(path)
    return (
        p.joinpath("bus_data.parquet").is_file()
        or p.joinpath("raw", "bus_data.parquet").is_file()
        or len(list(p.glob("*/raw/bus_data.parquet"))) == 1
    )


def _decode_local_path(value: str, *, purpose: str) -> Path:
    return sandbox.decode_local_path(value, purpose=purpose)


def _local_path(value: str, *, purpose: str, for_write: bool = False) -> str:
    return sandbox.checked_path(value, purpose=purpose, for_write=for_write)


def _jsonish(text: str) -> bool:
    return text.lstrip().startswith(("{", "["))


def _json_object(text: str, *, purpose: str) -> Dict[str, Any]:
    try:
        value = jsonlib.loads(text)
    except jsonlib.JSONDecodeError as exc:
        raise ValueError(f"{purpose} is not valid JSON: {exc}") from exc
    if not isinstance(value, dict):
        raise ValueError(f"{purpose} must be a JSON object")
    return value


def _package_value(text: str) -> Optional[Dict[str, Any]]:
    """Recognize a `.pio.json` package by the same markers the Rust classifier uses.

    Deliberately does not test `powerio_version`: a package written before
    0.9.0 states none, and it has to be recognized before it can be rejected
    with a message that says so. `powerio.StoredModule.from_json` owns the
    version gate.
    """
    try:
        value = jsonlib.loads(text)
    except jsonlib.JSONDecodeError:
        return None
    if not isinstance(value, dict):
        return None
    model = value.get("model")
    if not isinstance(model, dict):
        return None
    if value.get("model_kind") not in ("balanced", "multiconductor"):
        return None
    if not isinstance(model.get("kind"), str):
        return None
    return value


def _looks_like_package_json(text: str) -> bool:
    """Both stored generations: the version 1 module and the legacy 0.9 package."""
    return _module_header(text) or _package_value(text) is not None


def _package_model_kind(value: Dict[str, Any]) -> str:
    kind = value.get("model_kind")
    model = value.get("model")
    payload_kind = model.get("kind") if isinstance(model, dict) else None
    if kind not in ("balanced", "multiconductor"):
        raise ValueError("package `model_kind` must be `balanced` or `multiconductor`")
    if payload_kind != kind:
        raise ValueError("package `model_kind` does not match `model.kind`")
    return kind


def _json_class(text: str) -> tuple[str, Optional[str], Optional[str]]:
    return powerio._powerio.classify_json_text(text)


def _json_path_class(path: str) -> tuple[str, Optional[str], Optional[str]]:
    path = _local_path(path, purpose="path")
    try:
        text = Path(path).read_text(encoding="utf-8")
    except OSError as exc:
        raise ValueError(f"cannot read input: {exc}") from exc
    return _json_class(text)


def _format_from_json_class(
    status: str,
    domain: Optional[str],
    format: Optional[str],
    *,
    path: Optional[str] = None,
) -> tuple[str, str]:
    where = f" in {path}" if path is not None else ""
    if status == "known" and domain is not None and format is not None:
        return domain, format
    if status == "package":
        raise ValueError(
            f"JSON{where} is a .pio.json package; pass it as "
            "`json_format=\"package\"` or read it with the package tools"
        )
    if status == "model-json":
        return "transmission", "model-json"
    if status == "ambiguous":
        raise ValueError(
            f"ambiguous JSON markers{where}; pass `from_format` or `json_format`"
        )
    raise ValueError(
        f"cannot infer JSON format{where}; pass `from_format` or `json_format`"
    )


def _transport_kind(text: str, json_format: Optional[str]) -> str:
    if _looks_like_package_json(text):
        return "package"
    fmt = _fmt(json_format)
    if fmt in _PACKAGE_JSON_FORMATS:
        return "package"
    if fmt in _MODEL_JSON_FORMATS:
        return "model-json"
    if fmt in _BMOPF_JSON_FORMATS:
        return "bmopf-json"
    if fmt is not None:
        raise ValueError(
            "`json_format` must be `package`, `model-json`, or `bmopf-json`, "
            f"got {json_format!r}"
        )
    domain, format = _format_from_json_class(*_json_class(text))
    if domain == "distribution":
        return format
    if format == "model-json":
        return "model-json"
    raise ValueError(
        "`json` transport must be `model-json` or `bmopf-json`; "
        "pass case JSON as `content` with `from_format`"
    )


def _header(schema: str) -> Dict[str, Any]:
    """The two keys every tool response opens with: what it is, and which
    powerio release wrote it."""
    return {"schema": schema, _VERSION_KEY: powerio.__version__}


def _severity_counts(diagnostics: list[Dict[str, Any]]) -> Dict[str, int]:
    counts = {key: 0 for key in ("fatal", "error", "warning", "info", "debug")}
    for item in diagnostics:
        severity = item.get("severity")
        if severity in counts:
            counts[severity] += 1
    return counts


def _diagnostic_message(item: Dict[str, Any]) -> Optional[str]:
    """Format one diagnostic dict as `code: message`, or `message` alone when
    code is absent."""
    code = item.get("code")
    message = item.get("message")
    if code and message:
        return f"{code}: {message}"
    if message:
        return str(message)
    return None


def _diagnostic_messages(
    diagnostics: list[Any], keep_severities: frozenset[str]
) -> list[str]:
    messages = []
    for item in diagnostics:
        if not isinstance(item, dict):
            continue
        if item.get("severity") not in keep_severities:
            continue
        formatted = _diagnostic_message(item)
        if formatted is not None:
            messages.append(formatted)
    return messages


# A package's diagnostics carry the older severity set, including `fatal`.
_PACKAGE_WARNING_SEVERITIES = frozenset({"warning", "error", "fatal"})
# A stored module's `DiagnosticV1` severity is `error`, `warning`, `remark`,
# or `note`; only the first two are surfaced as warnings.
_MODULE_WARNING_SEVERITIES = frozenset({"warning", "error"})


def _package_diagnostic_messages(value: Dict[str, Any]) -> list[str]:
    return _diagnostic_messages(value.get("diagnostics", []), _PACKAGE_WARNING_SEVERITIES)


def _module_diagnostic_messages(module: "powerio.StoredModule") -> list[str]:
    return _diagnostic_messages(module.diagnostics(), _MODULE_WARNING_SEVERITIES)


def _diagnostics_payload(package_json: str, verbose: bool = False) -> Dict[str, Any]:
    value = _json_object(package_json, purpose="package_json")
    if _module_header(package_json):
        raw_value = value.get("value")
        payload_kind = raw_value.get("kind") if isinstance(raw_value, dict) else None
        if not isinstance(payload_kind, str):
            raise ValueError("the stored module document has no `value.kind`")
        kind = {
            "balanced_network": "balanced",
            "multiconductor_network": "multiconductor",
        }.get(payload_kind, payload_kind)
    elif _package_value(package_json) is not None:
        kind = _package_model_kind(value)
    else:
        raise ValueError("package_json is not a stored .pio.json document")
    # Validate with the stored reader (which upgrades a released 0.9
    # package) so schema version and consistency checks stay in one place.
    powerio.StoredModule.from_json(package_json)
    raw = value.get("diagnostics", [])
    diagnostics = [item for item in raw if isinstance(item, dict)]
    if not verbose:
        keep = {"code", "severity", "stage", "message", "element_path", "target"}
        diagnostics = [{k: v for k, v in item.items() if k in keep} for item in diagnostics]
    raw_validation = value.get("validation")
    validation = raw_validation if isinstance(raw_validation, dict) else {}
    raw_counts = validation.get("counts")
    counts = (
        dict(raw_counts)
        if isinstance(raw_counts, dict)
        else _severity_counts(diagnostics)
    )
    status = validation.get("status") or (
        "fatal"
        if counts.get("fatal", 0)
        else "error"
        if counts.get("error", 0)
        else "warning"
        if counts.get("warning", 0)
        else "info"
        if counts.get("info", 0)
        else "ok"
    )
    total = sum(int(counts.get(key, 0)) for key in ("fatal", "error", "warning", "info", "debug"))
    if total == 0:
        text = "ok: no diagnostics"
    else:
        parts = [
            f"{key}={int(counts.get(key, 0))}"
            for key in ("fatal", "error", "warning", "info", "debug")
            if int(counts.get(key, 0))
        ]
        text = f"{status}: " + ", ".join(parts)
    return {
        **_header("powerio.diagnostics"),
        "model_kind": kind,
        "summary": {
            "status": status,
            "counts": counts,
            "text": text,
        },
        "diagnostics": diagnostics,
    }


def _module_header(text: str) -> bool:
    """Whether the text is a stored version 1 module document."""
    if not _jsonish(text):
        return False
    try:
        value = jsonlib.loads(text)
    except ValueError:
        return False
    return isinstance(value, dict) and value.get("schema") == "powerio.module"


def _load_module(module_json: str) -> _Loaded:
    """Load a stored module's static network value as a network handle."""
    try:
        module = powerio.StoredModule.from_json(module_json)
    except ValueError as exc:
        raise _coded_error("module input", exc) from exc
    kind = module.kind
    warnings = _module_diagnostic_messages(module)
    if kind == "balanced_network":
        return _Loaded(
            domain="transmission",
            network=module.as_balanced_network(),
            warnings=warnings,
            json_format="module",
        )
    if kind == "multiconductor_network":
        return _Loaded(
            domain="distribution",
            network=module.as_multiconductor_network(),
            warnings=warnings,
            json_format="module",
        )
    raise ValueError(
        f"the module carries a {kind} value; select and export one static item "
        "with export_state first"
    )


def _parse_transmission(
    path: Optional[str],
    content: Optional[str],
    format: Optional[str],
    options: Optional[Dict[str, Any]] = None,
) -> _Loaded:
    opts = _opts(options)
    try:
        if _is_gridfm_format(format):
            if path is None:
                raise ValueError("gridfm input is a dataset directory; provide `path`")
            result = powerio.read_gridfm(path, int(opts.get("scenario", 0)))
            return _Loaded(
                "transmission",
                result.network,
                list(result.warnings),
                "model-json",
                int(result.scenario),
            )
        if path is not None:
            net = powerio.parse(path, format, value_type=powerio.BalancedNetwork)
        else:
            net = powerio.parse(
                _required(content, "content").encode(),
                format or "matpower",
                value_type=powerio.BalancedNetwork,
            )
    except powerio.PowerIOError as exc:
        raise _coded_error("parse failed", exc) from exc
    except FileNotFoundError as exc:
        raise ValueError(f"file not found: {exc}") from exc
    except ImportError as exc:
        raise ValueError(str(exc)) from exc
    except OSError as exc:
        raise ValueError(f"cannot read input: {exc}") from exc
    return _Loaded("transmission", net, list(net.read_warnings), "model-json")


def _parse_distribution(
    path: Optional[str],
    content: Optional[str],
    format: Optional[str],
    include_root: Optional[str] = None,
) -> _Loaded:
    if content is not None and not format:
        status, domain, inferred = _json_class(content)
        if status == "known" and domain == "distribution":
            format = inferred
        elif status == "ambiguous":
            raise ValueError("ambiguous JSON markers; pass `from_format`")
        else:
            raise ValueError("`from_format` is required for inline distribution content")
    try:
        if path is not None:
            net = dist.parse_file(path, format, include_root=include_root)
        else:
            # The block above settles `format` whenever `content` is set.
            net = dist.parse_str(
                _required(content, "content"), _required(format, "from_format")
            )
    except powerio.PowerIOError as exc:
        raise _coded_error("parse failed", exc) from exc
    except FileNotFoundError as exc:
        raise ValueError(f"file not found: {exc}") from exc
    except OSError as exc:
        raise ValueError(f"cannot read input: {exc}") from exc
    return _Loaded("distribution", net, list(net.warnings), "bmopf-json")


def _parse_any(
    path: Optional[str],
    content: Optional[str],
    format: Optional[str],
    options: Optional[Dict[str, Any]] = None,
) -> _Loaded:
    _one_input(path, content)
    include_root: Optional[str] = None
    if path is not None:
        path = _local_path(path, purpose="path")
        if Path(path).is_dir():
            sandbox.check_allowed_read_tree(Path(path), purpose="path")
        # A dss parse widens include confinement to the root that admitted the
        # path, so the operator's configured containment is the one policy in
        # force. Unconfined, the case directory default stands.
        root = sandbox.admitting_root(Path(path))
        include_root = str(root) if root is not None else None
    if _fmt(format) in _PACKAGE_JSON_FORMATS:
        if path is not None:
            try:
                text = Path(path).read_text(encoding="utf-8")
            except OSError as exc:
                raise ValueError(f"cannot read input: {exc}") from exc
            return _load_module(text)
        return _load_module(_required(content, "content"))
    if _is_gridfm_format(format):
        return _parse_transmission(path, content, format, options)
    if _is_dist_format(format):
        return _parse_distribution(path, content, format, include_root)
    if path is not None:
        p = Path(path)
        suffix = p.suffix.lower()
        if format is None and p.is_dir() and _looks_like_gridfm_dir(path):
            return _parse_transmission(path, content, "gridfm", options)
        if format is None and suffix == ".dss":
            return _parse_distribution(path, content, format, include_root)
        if format is None and suffix == ".json":
            try:
                text = Path(path).read_text(encoding="utf-8")
            except OSError as exc:
                raise ValueError(f"cannot read input: {exc}") from exc
            if _looks_like_package_json(text):
                return _load_module(text)
            domain, inferred = _format_from_json_class(*_json_path_class(path), path=path)
            if domain == "distribution":
                return _parse_distribution(path, content, inferred, include_root)
            return _parse_transmission(path, content, inferred, options)
    else:
        text = _required(content, "content")
        if format is None and _jsonish(text):
            if _looks_like_package_json(text):
                return _load_module(text)
            domain, inferred = _format_from_json_class(*_json_class(text))
            if domain == "distribution":
                return _parse_distribution(path, text, inferred)
            return _parse_transmission(path, text, inferred, options)
    return _parse_transmission(path, content, format, options)


def _load_transport(text: str, json_format: Optional[str]) -> _Loaded:
    kind = _transport_kind(text, json_format)
    if kind == "package":
        return _load_module(text)
    if kind in _BMOPF_JSON_FORMATS or kind in {"pmd-json", "pmd_json", "pmd", "engineering"}:
        return _parse_distribution(None, text, kind)
    try:
        net = powerio.from_json(text)
    except powerio.PowerIOError as exc:
        raise _coded_error("parse failed", exc) from exc
    except (ValueError, KeyError, TypeError) as exc:
        raise ValueError(f"parse failed: {exc}") from exc
    return _Loaded("transmission", net, list(net.read_warnings), "model-json")


def _load_any(
    path: Optional[str],
    content: Optional[str],
    transport: Optional[str],
    package_json: Optional[str],
    format: Optional[str],
    json_format: Optional[str],
    options: Optional[Dict[str, Any]] = None,
) -> _Loaded:
    # The tools spell an unset text argument as "" (a bare `str` annotation
    # keeps the SDK from re-parsing JSON text before validation); empty
    # transport text is invalid anyway, so "" means absent here.
    content, transport, package_json = content or None, transport or None, package_json or None
    _one_network_input(path, content, transport, package_json)
    if package_json is not None:
        # The stored reader upgrades a released 0.9 package one way, so both
        # document generations load through the module path.
        return _load_module(package_json)
    if transport is not None:
        return _load_transport(transport, json_format)
    return _parse_any(path, content, format, options)


def _transmission_summary(net: "powerio.BalancedNetwork") -> Dict[str, Any]:
    refs = net.reference_bus_indices()
    return {
        **_header("powerio.summary"),
        "domain": "transmission",
        "model": "balanced",
        "name": net.name,
        "source_format": net.source_format,
        "json_format": "model-json",
        "base_mva": net.base_mva,
        "elements": {
            "buses": net.n_buses,
            "branches": net.n_branches,
            "generators": net.n_gens,
            "loads": net.n_loads,
            "shunts": net.n_shunts,
            "lines": None,
            "transformers": None,
            "sources": None,
        },
        "topology": {
            "connected_components": net.n_connected_components,
            "is_radial": net.is_radial,
            "reference_buses": refs,
            "connectivity_report": net.connectivity_report(),
        },
        "warnings": list(net.read_warnings),
    }


def _distribution_summary(net: "dist.MulticonductorNetwork") -> Dict[str, Any]:
    return {
        **_header("powerio.summary"),
        "domain": "distribution",
        "model": "multiconductor",
        "name": net.name,
        "source_format": net.source_format,
        "json_format": "bmopf-json",
        "base_mva": None,
        "elements": {
            "buses": net.n_buses,
            "branches": None,
            "generators": net.n_generators,
            "loads": net.n_loads,
            "shunts": None,
            "lines": net.n_lines,
            "transformers": net.n_transformers,
            "sources": net.n_sources,
        },
        "topology": {
            "connected_components": None,
            "is_radial": None,
            "reference_buses": None,
            "connectivity_report": None,
        },
        "warnings": list(net.warnings),
    }


def _summary(loaded: _Loaded) -> Dict[str, Any]:
    if loaded.domain == "distribution":
        summary = _distribution_summary(loaded.network)
    else:
        summary = _transmission_summary(loaded.network)
    summary["warnings"] = list(loaded.warnings)
    return summary


def _dist_json(net: "dist.MulticonductorNetwork") -> tuple[str, list[str]]:
    conv = net.to_format("bmopf-json")
    return conv.text, list(net.warnings) + list(conv.warnings)


def _write_text(
    out_path: str, text: str, warnings: list[str], overwrite: bool
) -> Dict[str, Any]:
    try:
        mode = "w" if overwrite else "x"
        with open(out_path, mode, encoding="utf-8", newline="") as fh:
            fh.write(text)
    except FileExistsError:
        raise ValueError(
            f"refusing to overwrite existing file: {out_path}; pass overwrite=true"
        ) from None
    except OSError as exc:
        raise ValueError(f"write failed: {exc}") from exc
    return {
        "path": os.path.abspath(out_path),
        "bytes_written": len(text.encode("utf-8")),
        "warnings": warnings,
    }


def _choose_from_format(
    from_format: Optional[str] = None,
    *,
    format: Optional[str] = None,
    from_: Optional[str] = None,
) -> Optional[str]:
    values = [
        ("from_format", from_format),
        ("format", format),
        ("from_", from_),
    ]
    chosen_name: Optional[str] = None
    chosen: Optional[str] = None
    for name, value in values:
        if value is None:
            continue
        if chosen is None:
            chosen_name, chosen = name, value
            continue
        if _fmt(value) != _fmt(chosen):
            raise ValueError(f"`{chosen_name}` and `{name}` disagree")
    return chosen


def _choose_to_format(
    to_format: Optional[str] = None,
    *,
    to: Optional[str] = None,
) -> Optional[str]:
    """The target format from either spelling, or None when neither is set."""
    if to_format is not None and to is not None and _fmt(to_format) != _fmt(to):
        raise ValueError("`to_format` and `to` disagree")
    return to_format or to


def _require_to_format(to_format: Optional[str] = None, *, to: Optional[str] = None) -> str:
    """[`_choose_to_format`] where the caller has no target to fall back on."""
    target = _choose_to_format(to_format, to=to)
    if target is None:
        raise ValueError("`to_format` is required")
    return target


def _infer_to_format_from_out_path(out_path: str) -> str:
    suffix = Path(out_path).suffix.lower()
    inferred = {
        ".m": "matpower",
        ".raw": "psse",
        ".aux": "powerworld",
        ".epc": "pslf",
        ".dss": "dss",
    }.get(suffix)
    if inferred is not None:
        return inferred
    raise ValueError(
        "cannot infer `to_format` from `out_path`; pass `to_format` explicitly"
    )


def _convert_impl(
    to_format: str,
    path: Optional[str] = None,
    content: Optional[str] = None,
    json: Optional[str] = None,
    package_json: Optional[str] = None,
    from_format: Optional[str] = None,
    json_format: Optional[str] = None,
    options: Optional[Dict[str, Any]] = None,
) -> dict:
    to_l = _fmt(to_format)
    if _is_pypsa_format(to_l):
        raise ValueError(
            "`pypsa-csv` writes a folder; use save(to_format='pypsa-csv')"
        )
    if _is_gridfm_format(to_l):
        raise ValueError("`gridfm` writes a dataset; use save(to_format='gridfm')")
    loaded = _load_any(
        path, content, json, package_json, from_format, json_format, options
    )
    try:
        if _is_dist_format(to_l):
            if loaded.domain != "distribution":
                raise ValueError(
                    "no conversion path between transmission and distribution formats"
                )
            conv = loaded.network.to_format(to_format)
            warnings = loaded.warnings + list(conv.warnings)
        else:
            if loaded.domain != "transmission":
                raise ValueError(
                    "no conversion path between distribution and transmission formats"
                )
            conv = loaded.network.to_format(to_format)
            warnings = loaded.warnings + list(conv.warnings)
    except powerio.PowerIOError as exc:
        raise _coded_error("conversion failed", exc) from exc
    return {"text": conv.text, "warnings": warnings}


def _save_impl(
    out_path: str,
    path: Optional[str] = None,
    content: Optional[str] = None,
    json: Optional[str] = None,
    package_json: Optional[str] = None,
    to_format: Optional[str] = None,
    from_format: Optional[str] = None,
    json_format: Optional[str] = None,
    options: Optional[Dict[str, Any]] = None,
    overwrite: bool = False,
) -> dict:
    opts = _opts(options)
    out_path = _local_path(out_path, purpose="out_path", for_write=True)
    target = to_format or _infer_to_format_from_out_path(out_path)
    loaded = _load_any(
        path, content, json, package_json, from_format, json_format, options
    )
    to_l = _fmt(target)

    if _is_gridfm_format(to_l):
        if loaded.domain != "transmission":
            raise ValueError("gridfm export needs a transmission network")

        def write_gridfm(staging: str) -> Dict[str, Any]:
            return dict(
                loaded.network.write_gridfm(
                    staging,
                    scenario=int(opts.get("scenario", 0)),
                    include_y_bus=bool(opts.get("include_y_bus", True)),
                    include_taps=bool(opts.get("include_taps", True)),
                    include_shifts=bool(opts.get("include_shifts", True)),
                )
            )

        try:
            return sandbox.staged_directory_write(out_path, overwrite, write_gridfm)
        except ImportError as exc:
            raise ValueError(str(exc)) from exc
        except powerio.PowerIOError as exc:
            raise _coded_error("conversion failed", exc) from exc
        except OSError as exc:
            raise ValueError(f"write failed: {exc}") from exc

    if _is_pypsa_format(to_l):
        if loaded.domain != "transmission":
            raise ValueError("pypsa-csv export needs a transmission network")
        try:
            result = sandbox.staged_directory_write(
                out_path,
                overwrite,
                lambda staging: dict(loaded.network.write_pypsa_csv_folder(staging)),
            )
        except powerio.PowerIOError as exc:
            raise _coded_error("conversion failed", exc) from exc
        except OSError as exc:
            raise ValueError(f"write failed: {exc}") from exc
        return {
            "dir": result.get("dir", out_path),
            "files": list(result.get("files", [])),
            "warnings": loaded.warnings + list(result.get("warnings", [])),
        }

    if _is_dist_format(to_l):
        if loaded.domain != "distribution":
            raise ValueError("target is a distribution format but source is transmission")
        try:
            conv = loaded.network.to_format(target)
        except powerio.PowerIOError as exc:
            raise _coded_error("conversion failed", exc) from exc
        return _write_text(out_path, conv.text, loaded.warnings + list(conv.warnings), overwrite)

    if loaded.domain != "transmission":
        raise ValueError("target is a transmission format but source is distribution")
    try:
        conv = loaded.network.to_format(target)
    except powerio.PowerIOError as exc:
        raise _coded_error("conversion failed", exc) from exc
    return _write_text(out_path, conv.text, loaded.warnings + list(conv.warnings), overwrite)


def _summary_impl(
    path: Optional[str] = None,
    content: Optional[str] = None,
    json: Optional[str] = None,
    package_json: Optional[str] = None,
    from_format: Optional[str] = None,
    json_format: Optional[str] = None,
    options: Optional[Dict[str, Any]] = None,
) -> dict:
    return _summary(
        _load_any(path, content, json, package_json, from_format, json_format, options)
    )


def _parse_impl(
    path: Optional[str] = None,
    content: Optional[str] = None,
    from_format: Optional[str] = None,
    options: Optional[Dict[str, Any]] = None,
    transport: str = "json",
) -> dict:
    # "" means absent, as in `_load_any`.
    content = content or None
    transport_l = _fmt(transport or "json")
    if transport_l in _PACKAGE_JSON_FORMATS:
        module = _stored_module(path=path, content=content, from_format=from_format)
        package_json = module.to_json()
        loaded = _load_module(package_json)
        summary = _summary(loaded)
        diag = _diagnostics_payload(package_json, verbose=True)
        return {
            **_header("powerio.parse"),
            "transport": "package",
            "domain": loaded.domain,
            "model": summary["model"],
            "source_format": summary["source_format"],
            "json_format": "package",
            "package_json": package_json,
            "summary": summary,
            "diagnostics": diag["diagnostics"],
            "diagnostics_summary": diag["summary"],
            "warnings": loaded.warnings,
        }
    if transport_l not in {"json", "legacy"}:
        raise ValueError("`transport` must be `json` or `package`")
    loaded = _parse_any(path, content, from_format, options)
    if loaded.domain == "distribution":
        text, warnings = _dist_json(loaded.network)
    else:
        text, warnings = loaded.network.to_json(), loaded.warnings
    summary = _summary(loaded)
    return {
        **_header("powerio.parse"),
        "domain": loaded.domain,
        "model": summary["model"],
        "source_format": summary["source_format"],
        "json_format": loaded.json_format,
        "json": text,
        "summary": summary,
        "warnings": warnings,
    }


def _normalize_impl(
    path: Optional[str] = None,
    content: Optional[str] = None,
    json: Optional[str] = None,
    package_json: Optional[str] = None,
    from_format: Optional[str] = None,
    json_format: Optional[str] = None,
    options: Optional[Dict[str, Any]] = None,
) -> dict:
    loaded = _load_any(
        path, content, json, package_json, from_format, json_format, options
    )
    if loaded.domain != "transmission":
        raise ValueError("normalization is not defined for distribution networks")
    try:
        norm = loaded.network.to_normalized()
    except powerio.PowerIOError as exc:
        raise _coded_error("normalization failed", exc) from exc
    normalized = _Loaded("transmission", norm, list(norm.read_warnings), "model-json")
    summary = _summary(normalized)
    return {
        **_header("powerio.normalize"),
        "domain": "transmission",
        "model": "balanced",
        "source_format": summary["source_format"],
        "json_format": "model-json",
        "json": norm.to_json(),
        "summary": summary,
        "warnings": list(norm.read_warnings),
    }


def _matrix_impl(
    kind: str,
    path: Optional[str] = None,
    content: Optional[str] = None,
    json: Optional[str] = None,
    package_json: Optional[str] = None,
    from_format: Optional[str] = None,
    json_format: Optional[str] = None,
    options: Optional[Dict[str, Any]] = None,
    scheme: str = "bx",
    convention: str = "series",
) -> dict:
    canonical = _MATRIX_KIND_ALIASES.get(kind.lower())
    if canonical is None:
        raise ValueError(f"unknown matrix kind {kind!r}; expected one of: {_MATRIX_HELP}")
    loaded = _load_any(
        path, content, json, package_json, from_format, json_format, options
    )
    if loaded.domain != "transmission":
        raise ValueError("matrix outputs need a transmission network")
    net = loaded.network
    try:
        if canonical == "bprime":
            mat = net.bprime(scheme)
        elif canonical == "bdoubleprime":
            mat = net.bdoubleprime(scheme)
        elif canonical in ("ybus_real", "ybus_imag"):
            parts = net.ybus_parts()
            mat = parts.g if canonical == "ybus_real" else parts.b
        elif canonical == "adjacency":
            mat = net.adjacency()
        elif canonical == "ptdf":
            mat = net.ptdf(convention)
        elif canonical == "lodf":
            mat = net.lodf(convention)
        elif canonical == "lacpf":
            mat = net.lacpf()
        elif canonical == "laplacian":
            mat = net.weighted_laplacian(convention)
        else:  # pragma: no cover
            raise ValueError(f"unhandled matrix kind {canonical!r}")
    except ImportError as exc:
        raise ValueError(str(exc)) from exc
    except powerio.PowerIOError as exc:
        raise _coded_error("matrix build failed", exc) from exc
    coo = mat.tocoo()
    return {
        **_header("powerio.matrix"),
        "domain": "transmission",
        "model": "balanced",
        "source_format": net.source_format,
        "json_format": loaded.json_format,
        "warnings": loaded.warnings,
        "format": "coo",
        "kind": canonical,
        "shape": [int(coo.shape[0]), int(coo.shape[1])],
        "nnz": int(coo.nnz),
        "data": coo.data.tolist(),
        "row": coo.row.tolist(),
        "col": coo.col.tolist(),
    }


def _display_impl(path: str, from_format: Optional[str] = None) -> dict:
    path = _local_path(path, purpose="path")
    try:
        data = powerio.parse_display_file(path, from_format)
    except powerio.PowerIOError as exc:
        raise _coded_error("parse failed", exc) from exc
    except FileNotFoundError as exc:
        raise ValueError(f"file not found: {exc}") from exc
    except OSError as exc:
        raise ValueError(f"cannot read file: {exc}") from exc
    if data.kind != "powerworld":
        raise ValueError(f"unsupported display format: {data.kind!r}")
    pwd = data.data
    return {
        **_header("powerio.display"),
        "domain": "display",
        "model": "display",
        "source_format": "powerworld-pwd",
        "canvas": {
            "width": pwd.canvas_width,
            "height": pwd.canvas_height,
        },
        "stamp": pwd.stamp,
        "substations": [
            {"number": s.number, "name": s.name, "x": s.x, "y": s.y}
            for s in pwd.substations
        ],
    }


# The tool text arguments that can carry JSON (`content`, `json`,
# `package_json`) are annotated bare `str`: under any other annotation the
# SDK re-parses a string that reads as JSON, destroying the text. "" means
# unset.
@mcp.tool(
    name="convert",
    description="Convert a network to a single text format. "
    + _TARGET_FORMAT_HELP
    + " "
    + _SOURCE_FORMAT_HELP,
)
def _convert_tool(
    to_format: str,
    path: Optional[str] = None,
    content: str = "",
    json: str = "",
    package_json: str = "",
    from_format: Optional[str] = None,
    json_format: _JsonFormatArg = None,
    options: Optional[Dict[str, Any]] = None,
) -> dict:
    return _convert_impl(
        to_format,
        path=path,
        content=content,
        json=json,
        package_json=package_json,
        from_format=from_format,
        json_format=json_format,
        options=options,
    )


@mcp.tool(
    name="save",
    description="Write a converted network to disk. "
    + _TARGET_FORMAT_HELP
    + " "
    + _SOURCE_FORMAT_HELP,
)
def _save_tool(
    out_path: str,
    path: Optional[str] = None,
    content: str = "",
    json: str = "",
    package_json: str = "",
    to_format: Optional[str] = None,
    from_format: Optional[str] = None,
    json_format: _JsonFormatArg = None,
    options: Optional[Dict[str, Any]] = None,
    overwrite: bool = False,
) -> dict:
    return _save_impl(
        out_path,
        path=path,
        content=content,
        json=json,
        package_json=package_json,
        to_format=to_format,
        from_format=from_format,
        json_format=json_format,
        options=options,
        overwrite=overwrite,
    )


@mcp.tool(
    name="summary",
    description="Return canonical summary JSON for a balanced or "
    "multiconductor model. " + _SOURCE_FORMAT_HELP,
)
def _summary_tool(
    path: Optional[str] = None,
    content: str = "",
    json: str = "",
    package_json: str = "",
    from_format: Optional[str] = None,
    json_format: _JsonFormatArg = None,
    options: Optional[Dict[str, Any]] = None,
) -> dict:
    return _summary_impl(
        path, content, json, package_json, from_format, json_format, options
    )


@mcp.tool(
    name="parse",
    description="Parse a model and return legacy JSON or a `.pio.json` "
    "package. " + _SOURCE_FORMAT_HELP,
)
def _parse_tool(
    path: Optional[str] = None,
    content: str = "",
    from_format: Optional[str] = None,
    options: Optional[Dict[str, Any]] = None,
    transport: str = "json",
) -> dict:
    return _parse_impl(path, content, from_format, options, transport)


@mcp.tool(
    name="normalize",
    description="Normalize a transmission network and return the powerio "
    "JSON transport. " + _SOURCE_FORMAT_HELP,
)
def _normalize_tool(
    path: Optional[str] = None,
    content: str = "",
    json: str = "",
    package_json: str = "",
    from_format: Optional[str] = None,
    json_format: _JsonFormatArg = None,
    options: Optional[Dict[str, Any]] = None,
) -> dict:
    return _normalize_impl(
        path, content, json, package_json, from_format, json_format, options
    )


@mcp.tool(
    name="matrix",
    description="Build a transmission matrix output in COO form. "
    + _SOURCE_FORMAT_HELP,
)
def _matrix_tool(
    kind: Annotated[str, _MATRIX_KIND_FIELD],
    path: Optional[str] = None,
    content: str = "",
    json: str = "",
    package_json: str = "",
    from_format: Optional[str] = None,
    json_format: _JsonFormatArg = None,
    options: Optional[Dict[str, Any]] = None,
    scheme: str = "bx",
    convention: str = "series",
) -> dict:
    return _matrix_impl(
        kind,
        path=path,
        content=content,
        json=json,
        package_json=package_json,
        from_format=from_format,
        json_format=json_format,
        options=options,
        scheme=scheme,
        convention=convention,
    )


@mcp.tool(name="diagnostics")
def _diagnostics_tool(package_json: str, verbose: bool = False) -> dict:
    """Return package diagnostics as structured JSON plus a concise summary."""
    return _diagnostics_payload(package_json, verbose)


@mcp.tool(name="display")
def _display_tool(path: str, from_format: Optional[str] = None) -> dict:
    """Parse a display artifact and return canonical display JSON."""
    return _display_impl(path, from_format)


def _stored_module(
    module_json: Optional[str] = None,
    path: Optional[str] = None,
    content: Optional[str] = None,
    from_format: Optional[str] = None,
) -> "powerio.StoredModule":
    """One module input: stored `.pio.json` text, a case path, or case text."""
    supplied = [value for value in (module_json, path, content) if value]
    if len(supplied) != 1:
        raise ValueError("pass exactly one of module_json, path, and content")
    try:
        if module_json:
            return powerio.StoredModule.from_json(module_json)
        if path is not None:
            return powerio.StoredModule.from_file(
                _local_path(path, purpose="module input"), _fmt(from_format)
            )
        return powerio.StoredModule.from_str(content or "", _fmt(from_format))
    except ValueError as exc:
        raise _coded_error("module input", exc) from exc


def _selected_args(
    time_position: Optional[int], scenario: Optional[str]
) -> Dict[str, Any]:
    if (time_position is None) == (scenario is None):
        raise ValueError("pass exactly one of time_position and scenario")
    return {"time_position": time_position, "scenario": scenario}


_MODULE_INPUT_HELP = (
    "Input is exactly one of `module_json` (stored `.pio.json` text; a "
    "released 0.9 package upgrades one way on read), `path`, or `content` "
    "(case data parsed into a module; `from_format` selects the reader)."
)


@mcp.tool(
    name="inspect",
    description="Inspect a module's typed value and discover the operations "
    "that apply to it. " + _MODULE_INPUT_HELP,
)
def _inspect_tool(
    module_json: Optional[str] = None,
    path: Optional[str] = None,
    content: Optional[str] = None,
    from_format: Optional[str] = None,
) -> dict:
    module = _stored_module(module_json, path, content, from_format)
    return {**_header("powerio.inspect"), **module.inspect()}


@mcp.tool(
    name="state_inventory",
    description="List the exact typed time point labels or scenario IDs a "
    "stored module's value can select. " + _MODULE_INPUT_HELP,
)
def _state_inventory_tool(
    module_json: Optional[str] = None,
    path: Optional[str] = None,
    content: Optional[str] = None,
    from_format: Optional[str] = None,
) -> dict:
    module = _stored_module(module_json, path, content, from_format)
    try:
        inventory = module.state_inventory()
    except ValueError as exc:
        raise _coded_error("state inventory", exc) from exc
    return {**_header("powerio.state_inventory"), "kind": module.kind, **inventory}


@mcp.tool(
    name="select_state",
    description="Select one existing typed item by time position or scenario "
    "ID and describe it. Selection never clones the collection or "
    "serializes it; `export_state` is the separate materialization. "
    + _MODULE_INPUT_HELP,
)
def _select_state_tool(
    module_json: Optional[str] = None,
    path: Optional[str] = None,
    content: Optional[str] = None,
    from_format: Optional[str] = None,
    time_position: Optional[int] = None,
    scenario: Optional[str] = None,
) -> dict:
    module = _stored_module(module_json, path, content, from_format)
    keys = _selected_args(time_position, scenario)
    try:
        selected = module.select_state(**keys)
    except ValueError as exc:
        raise _coded_error("state selection", exc) from exc
    return {**_header("powerio.select_state"), **keys, "selected": selected}


@mcp.tool(
    name="export_state",
    description="Export one selected time point or scenario as an "
    "independent static module document, with the selection stated in its "
    "history. " + _MODULE_INPUT_HELP,
)
def _export_state_tool(
    module_json: Optional[str] = None,
    path: Optional[str] = None,
    content: Optional[str] = None,
    from_format: Optional[str] = None,
    time_position: Optional[int] = None,
    scenario: Optional[str] = None,
) -> dict:
    module = _stored_module(module_json, path, content, from_format)
    keys = _selected_args(time_position, scenario)
    try:
        exported = module.export_state(**keys)
    except ValueError as exc:
        raise _coded_error("state export", exc) from exc
    return {
        **_header("powerio.export_state"),
        **keys,
        "kind": exported.kind,
        "module_json": exported.to_json(),
    }


@mcp.tool(
    name="to_balanced_inspect",
    description="Inspect whether a multiconductor module can lower to a "
    "balanced network: blockers, assumptions, approximations, and "
    "unrepresentable fields. " + _MODULE_INPUT_HELP,
)
def _to_balanced_inspect_tool(
    module_json: Optional[str] = None,
    path: Optional[str] = None,
    content: Optional[str] = None,
    from_format: Optional[str] = None,
    base_mva: float = 100.0,
) -> dict:
    module = _stored_module(module_json, path, content, from_format)
    try:
        readiness = module.to_balanced_inspect(base_mva)
    except ValueError as exc:
        raise _coded_error("lowering inspection", exc) from exc
    return {**_header("powerio.to_balanced_inspect"), **readiness}


@mcp.tool(
    name="about",
    description="Version and schema identity of this powerio build: the "
    "release, the stored module schema, the BMOPF schema, and the tool "
    "names this server exposes.",
)
def _about_tool() -> dict:
    return {
        **_header("powerio.about"),
        **powerio.versions(),
        "tools": sorted(
            tool.name for tool in mcp._tool_manager.list_tools()
        ),
    }


@mcp.tool(
    name="dc_data",
    description="DC branch data under one named susceptance formula: "
    "incidence row endpoints, susceptance, the phase shift injection, and "
    "stable element mappings for included rows and omitted branches. "
    "Formulas: series_susceptance, tap_adjusted_reactance, reactance_only. "
    + _MODULE_INPUT_HELP,
)
def _dc_data_tool(
    module_json: Optional[str] = None,
    path: Optional[str] = None,
    content: Optional[str] = None,
    from_format: Optional[str] = None,
    formula: str = "series_susceptance",
) -> dict:
    module = _stored_module(module_json, path, content, from_format)
    if module.kind != "balanced_network":
        raise ValueError(
            f"the module carries a {module.kind} value; DC data takes a "
            "balanced network"
        )
    net = module.as_balanced_network()
    try:
        data = net.dc_data(formula)
    except ValueError as exc:
        raise _coded_error("dc data", exc) from exc
    return {**_header("powerio.dc_data"), **data}


@mcp.tool(
    name="to_balanced",
    description="Explicitly lower a multiconductor module to a balanced "
    "module document. Records and source ownership carry over; the pass "
    "appends its findings and a Transform history entry. "
    + _MODULE_INPUT_HELP,
)
def _to_balanced_tool(
    module_json: Optional[str] = None,
    path: Optional[str] = None,
    content: Optional[str] = None,
    from_format: Optional[str] = None,
    base_mva: float = 100.0,
) -> dict:
    module = _stored_module(module_json, path, content, from_format)
    try:
        lowered = module.to_balanced(base_mva)
    except ValueError as exc:
        raise _coded_error("balanced lowering", exc) from exc
    return {
        **_header("powerio.to_balanced"),
        "kind": lowered.kind,
        "module_json": lowered.to_json(),
    }


# Non-advertised compatibility callables for direct Python imports.
def convert(
    to_format: Optional[str] = None,
    path: Optional[str] = None,
    content: Optional[str] = None,
    json: Optional[str] = None,
    from_format: Optional[str] = None,
    json_format: Optional[str] = None,
    options: Optional[Dict[str, Any]] = None,
    *,
    to: Optional[str] = None,
    format: Optional[str] = None,
    from_: Optional[str] = None,
    package_json: Optional[str] = None,
) -> dict:
    target = _require_to_format(to_format, to=to)
    source = _choose_from_format(from_format, format=format, from_=from_)
    return _convert_impl(
        target,
        path=path,
        content=content,
        json=json,
        package_json=package_json,
        from_format=source,
        json_format=json_format,
        options=options,
    )


def save(
    out_path: str,
    path: Optional[str] = None,
    content: Optional[str] = None,
    json: Optional[str] = None,
    to_format: Optional[str] = None,
    from_format: Optional[str] = None,
    json_format: Optional[str] = None,
    options: Optional[Dict[str, Any]] = None,
    overwrite: bool = False,
    *,
    to: Optional[str] = None,
    format: Optional[str] = None,
    from_: Optional[str] = None,
    package_json: Optional[str] = None,
) -> dict:
    target = _choose_to_format(to_format, to=to)
    source = _choose_from_format(from_format, format=format, from_=from_)
    return _save_impl(
        out_path,
        path=path,
        content=content,
        json=json,
        package_json=package_json,
        to_format=target,
        from_format=source,
        json_format=json_format,
        options=options,
        overwrite=overwrite,
    )


def summary(
    path: Optional[str] = None,
    content: Optional[str] = None,
    json: Optional[str] = None,
    from_format: Optional[str] = None,
    json_format: Optional[str] = None,
    options: Optional[Dict[str, Any]] = None,
    *,
    format: Optional[str] = None,
    from_: Optional[str] = None,
    package_json: Optional[str] = None,
) -> dict:
    source = _choose_from_format(from_format, format=format, from_=from_)
    return _summary_impl(path, content, json, package_json, source, json_format, options)


def parse(
    path: Optional[str] = None,
    content: Optional[str] = None,
    from_format: Optional[str] = None,
    options: Optional[Dict[str, Any]] = None,
    transport: str = "json",
    *,
    format: Optional[str] = None,
    from_: Optional[str] = None,
) -> dict:
    source = _choose_from_format(from_format, format=format, from_=from_)
    return _parse_impl(path, content, source, options, transport)


def normalize(
    path: Optional[str] = None,
    content: Optional[str] = None,
    json: Optional[str] = None,
    from_format: Optional[str] = None,
    json_format: Optional[str] = None,
    options: Optional[Dict[str, Any]] = None,
    *,
    format: Optional[str] = None,
    from_: Optional[str] = None,
    package_json: Optional[str] = None,
) -> dict:
    source = _choose_from_format(from_format, format=format, from_=from_)
    return _normalize_impl(path, content, json, package_json, source, json_format, options)


def matrix(
    kind: str,
    path: Optional[str] = None,
    content: Optional[str] = None,
    json: Optional[str] = None,
    from_format: Optional[str] = None,
    json_format: Optional[str] = None,
    options: Optional[Dict[str, Any]] = None,
    scheme: str = "bx",
    convention: str = "series",
    *,
    format: Optional[str] = None,
    from_: Optional[str] = None,
    package_json: Optional[str] = None,
) -> dict:
    source = _choose_from_format(from_format, format=format, from_=from_)
    return _matrix_impl(
        kind,
        path=path,
        content=content,
        json=json,
        package_json=package_json,
        from_format=source,
        json_format=json_format,
        options=options,
        scheme=scheme,
        convention=convention,
    )


def display(
    path: str,
    from_format: Optional[str] = None,
    *,
    format: Optional[str] = None,
    from_: Optional[str] = None,
) -> dict:
    source = _choose_from_format(from_format, format=format, from_=from_)
    return _display_impl(path, source)


def diagnostics(package_json: str, verbose: bool = False) -> dict:
    return _diagnostics_payload(package_json, verbose)


def compute_matrix(*args: Any, **kwargs: Any) -> dict:
    return matrix(*args, **kwargs)


def convert_case(
    to: str,
    path: Optional[str] = None,
    content: Optional[str] = None,
    from_: Optional[str] = None,
) -> dict:
    return convert(to_format=to, path=path, content=content, from_format=from_)


def save_case(
    to: str,
    out_path: str,
    path: Optional[str] = None,
    content: Optional[str] = None,
    json: Optional[str] = None,
    format: Optional[str] = None,
    overwrite: bool = False,
) -> dict:
    return save(
        out_path=out_path,
        path=path,
        content=content,
        json=json,
        to_format=to,
        from_format=format,
        overwrite=overwrite,
    )


def case_summary(
    path: Optional[str] = None,
    content: Optional[str] = None,
    json: Optional[str] = None,
    format: Optional[str] = None,
) -> dict:
    return summary(path=path, content=content, json=json, from_format=format)


def parse_case(
    path: Optional[str] = None,
    content: Optional[str] = None,
    format: Optional[str] = None,
) -> dict:
    return parse(path=path, content=content, from_format=format)


def normalize_case(
    path: Optional[str] = None,
    content: Optional[str] = None,
    format: Optional[str] = None,
) -> dict:
    return normalize(path=path, content=content, from_format=format)


def case_to_json(
    path: Optional[str] = None,
    content: Optional[str] = None,
    format: Optional[str] = None,
) -> dict:
    result = parse(path=path, content=content, from_format=format)
    return {"json": result["json"], "json_format": result["json_format"]}


def write_pypsa_csv_folder(
    out_dir: str,
    path: Optional[str] = None,
    content: Optional[str] = None,
    json: Optional[str] = None,
    format: Optional[str] = None,
) -> dict:
    return save(
        out_path=out_dir,
        path=path,
        content=content,
        json=json,
        to_format="pypsa-csv",
        from_format=format,
    )


def read_pypsa_csv_folder(folder: str) -> dict:
    return parse(path=folder)


def main() -> None:
    """Console-script entry point: serve the tools over stdio."""
    mcp.run()
