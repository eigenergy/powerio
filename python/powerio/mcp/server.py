"""FastMCP server for powerio.

The advertised MCP surface is semantic and format neutral:

``convert``, ``save``, ``summary``, ``parse``, ``normalize``, ``matrix``,
``diagnostics``, ``capabilities``, ``display``.

Network tools route balanced transmission models, multiconductor distribution
models, PyPSA CSV folders, and gridfm datasets through the lower level powerio
APIs. Transmission parses serialize through the ``powerio-json`` transport.
Distribution parses serialize through canonical ``bmopf-json``. Package
transport serializes either family through the ``.pio.json`` compiler package.
"""

from __future__ import annotations

from dataclasses import dataclass
import json as jsonlib
import os
from pathlib import Path
from typing import Any, Dict, Optional
from urllib.parse import unquote, urlparse

import powerio
from powerio import dist
from mcp.server.fastmcp import FastMCP

mcp = FastMCP("powerio")

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
_POWERIO_JSON_FORMATS = frozenset({"powerio", "powerio-json", "json"})
_BMOPF_JSON_FORMATS = frozenset({"bmopf", "bmopf-json", "bmopf_json"})
_PACKAGE_JSON_FORMATS = frozenset(
    {"package", "pio", "pio-json", "pio_json", "pio-package", "pio_package"}
)
_ALLOWED_ROOTS_ENV = "POWERIO_MCP_ALLOWED_ROOTS"
_LEGACY_ALLOWED_ROOT_ENV = "POWERIO_MCP_ROOT"
_SCHEMA_VERSION = "0.1"

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
    "bprime/b/b1 (FDPF B'), bdoubleprime/b2/bpp (FDPF B''), "
    "ybus_real/g, ybus_imag/negB/b_lap, adjacency/adj, ptdf, lodf, "
    "laplacian, lacpf"
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


def _allowed_roots() -> tuple[Path, ...]:
    raw = os.environ.get(_ALLOWED_ROOTS_ENV) or os.environ.get(_LEGACY_ALLOWED_ROOT_ENV)
    if not raw:
        return ()
    roots = []
    for item in raw.split(os.pathsep):
        item = item.strip()
        if item:
            roots.append(Path(item).expanduser().resolve(strict=False))
    return tuple(roots)


def _decode_local_path(value: str, *, purpose: str) -> Path:
    parsed = urlparse(str(value))
    windows_drive = os.name == "nt" and len(parsed.scheme) == 1
    if parsed.scheme and not windows_drive:
        if parsed.scheme != "file":
            raise ValueError(f"`{purpose}` must be a local path or file:// URI")
        netloc = unquote(parsed.netloc)
        path = unquote(parsed.path)
        if len(netloc) == 2 and netloc[0].isalpha() and netloc[1] == ":":
            return Path(f"{netloc}{path}").expanduser()
        if netloc.lower() not in ("", "localhost"):
            raise ValueError(f"`{purpose}` file URI must be local")
        if (
            len(path) >= 3
            and path[0] == "/"
            and path[1].isalpha()
            and path[2] == ":"
            and (len(path) == 3 or path[3] in "/\\")
        ):
            path = path[1:]
        return Path(path).expanduser()
    return Path(str(value)).expanduser()


def _path_for_policy(path: Path, *, for_write: bool) -> Path:
    try:
        if for_write and not path.exists():
            parent = path.parent if path.parent != Path("") else Path(".")
            return parent.resolve(strict=True) / path.name
        return path.resolve(strict=True)
    except FileNotFoundError:
        if for_write:
            raise
        return path.resolve(strict=False)


def _check_allowed_path(path: Path, *, for_write: bool, purpose: str) -> None:
    roots = _allowed_roots()
    if not roots:
        return
    try:
        resolved = _path_for_policy(path, for_write=for_write)
    except OSError as exc:
        raise ValueError(
            f"cannot resolve `{purpose}` against allowed MCP roots: {exc}"
        ) from exc
    for root in roots:
        if resolved == root or root in resolved.parents:
            return
    root_list = ", ".join(str(root) for root in roots)
    raise ValueError(f"`{purpose}` is outside allowed MCP roots: {root_list}")


def _local_path(value: str, *, purpose: str, for_write: bool = False) -> str:
    path = _decode_local_path(value, purpose=purpose)
    _check_allowed_path(path, for_write=for_write, purpose=purpose)
    return str(path)


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
    try:
        value = jsonlib.loads(text)
    except jsonlib.JSONDecodeError:
        return None
    if not isinstance(value, dict):
        return None
    model = value.get("model")
    if not isinstance(model, dict):
        return None
    required = ("schema", "schema_version", "model_kind")
    if not all(isinstance(value.get(key), str) for key in required):
        return None
    if not isinstance(model.get("kind"), str):
        return None
    return value


def _looks_like_package_json(text: str) -> bool:
    return _package_value(text) is not None


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
    if fmt in _POWERIO_JSON_FORMATS:
        return "powerio-json"
    if fmt in _BMOPF_JSON_FORMATS:
        return "bmopf-json"
    if fmt is not None:
        raise ValueError(
            "`json_format` must be `package`, `powerio-json`, or `bmopf-json`, "
            f"got {json_format!r}"
        )
    domain, format = _format_from_json_class(*_json_class(text))
    if domain == "distribution":
        return format
    if format == "powerio-json":
        return "powerio-json"
    raise ValueError(
        "`json` transport must be `powerio-json` or `bmopf-json`; "
        "pass case JSON as `content` with `from_format`"
    )


def _severity_counts(diagnostics: list[Dict[str, Any]]) -> Dict[str, int]:
    counts = {key: 0 for key in ("fatal", "error", "warning", "info", "debug")}
    for item in diagnostics:
        severity = item.get("severity")
        if severity in counts:
            counts[severity] += 1
    return counts


def _package_diagnostic_messages(value: Dict[str, Any]) -> list[str]:
    messages = []
    for item in value.get("diagnostics", []):
        if not isinstance(item, dict):
            continue
        if item.get("severity") not in ("warning", "error", "fatal"):
            continue
        code = item.get("code")
        message = item.get("message")
        if code and message:
            messages.append(f"{code}: {message}")
        elif message:
            messages.append(str(message))
    return messages


def _diagnostics_payload(package_json: str, verbose: bool = False) -> Dict[str, Any]:
    value = _json_object(package_json, purpose="package_json")
    if _package_value(package_json) is None:
        raise ValueError("package_json is not a .pio.json package envelope")
    kind = _package_model_kind(value)
    # Validate with the Rust package reader so schema version and payload
    # consistency checks stay in one place.
    powerio._powerio.package_model_kind(package_json)
    raw = value.get("diagnostics", [])
    diagnostics = [item for item in raw if isinstance(item, dict)]
    if not verbose:
        keep = {"code", "severity", "stage", "message", "element_path"}
        diagnostics = [{k: v for k, v in item.items() if k in keep} for item in diagnostics]
    validation = value.get("validation") if isinstance(value.get("validation"), dict) else {}
    counts = validation.get("counts") if isinstance(validation.get("counts"), dict) else None
    counts = dict(counts) if counts is not None else _severity_counts(diagnostics)
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
        "schema": "powerio.diagnostics",
        "schema_version": _SCHEMA_VERSION,
        "model_kind": kind,
        "summary": {
            "status": status,
            "counts": counts,
            "text": text,
        },
        "diagnostics": diagnostics,
    }


def _load_package(package_json: str) -> _Loaded:
    value = _json_object(package_json, purpose="package_json")
    if _package_value(package_json) is None:
        raise ValueError("package_json is not a .pio.json package envelope")
    kind = _package_model_kind(value)
    try:
        if kind == "multiconductor":
            inner = powerio._powerio.package_as_multiconductor(package_json)
            net = dist.DistNetwork(inner)
            return _Loaded(
                "distribution",
                net,
                _package_diagnostic_messages(value),
                "bmopf-json",
                package_json=package_json,
            )
        inner = powerio._powerio.package_as_balanced(package_json)
        net = powerio.Network(inner)
        return _Loaded(
            "transmission",
            net,
            _package_diagnostic_messages(value),
            "powerio-json",
            package_json=package_json,
        )
    except powerio.PowerIOError as exc:
        raise ValueError(f"parse failed: {exc}") from exc
    except (ValueError, KeyError, TypeError) as exc:
        raise ValueError(f"parse failed: {exc}") from exc


def _package_json_from_input(
    path: Optional[str],
    content: Optional[str],
    from_format: Optional[str],
    options: Optional[Dict[str, Any]] = None,
) -> str:
    _one_input(path, content)
    opts = _opts(options)
    from_l = _fmt(from_format)
    try:
        if path is not None:
            path = _local_path(path, purpose="path")
            if from_l in _PACKAGE_JSON_FORMATS or Path(path).suffix.lower() == ".json":
                try:
                    text = Path(path).read_text(encoding="utf-8")
                except OSError as exc:
                    raise ValueError(f"cannot read input: {exc}") from exc
                if not _looks_like_package_json(text):
                    if from_l in _PACKAGE_JSON_FORMATS:
                        raise ValueError("input is not a .pio.json package envelope")
                else:
                    powerio._powerio.package_model_kind(text)
                    return text
            return powerio._powerio.package_parse_file(
                path, from_format, int(opts.get("scenario", 0))
            )
        if _looks_like_package_json(content):
            powerio._powerio.package_model_kind(content)
            return content
        if from_l in _PACKAGE_JSON_FORMATS:
            raise ValueError("content is not a .pio.json package envelope")
        return powerio._powerio.package_parse_str(content, from_format)
    except powerio.PowerIOError as exc:
        raise ValueError(f"parse failed: {exc}") from exc
    except FileNotFoundError as exc:
        raise ValueError(f"file not found: {exc}") from exc
    except ImportError as exc:
        raise ValueError(str(exc)) from exc
    except OSError as exc:
        raise ValueError(f"cannot read input: {exc}") from exc


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
                "powerio-json",
                int(result.scenario),
            )
        if path is not None:
            net = powerio.parse_file(path, format)
        else:
            net = powerio.parse_str(content, format or "matpower")
    except powerio.PowerIOError as exc:
        raise ValueError(f"parse failed: {exc}") from exc
    except FileNotFoundError as exc:
        raise ValueError(f"file not found: {exc}") from exc
    except ImportError as exc:
        raise ValueError(str(exc)) from exc
    except OSError as exc:
        raise ValueError(f"cannot read input: {exc}") from exc
    return _Loaded("transmission", net, list(net.read_warnings), "powerio-json")


def _parse_distribution(
    path: Optional[str], content: Optional[str], format: Optional[str]
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
            net = dist.parse_file(path, format)
        else:
            net = dist.parse_str(content, format)
    except powerio.PowerIOError as exc:
        raise ValueError(f"parse failed: {exc}") from exc
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
    if path is not None:
        path = _local_path(path, purpose="path")
    if _fmt(format) in _PACKAGE_JSON_FORMATS:
        if path is not None:
            try:
                text = Path(path).read_text(encoding="utf-8")
            except OSError as exc:
                raise ValueError(f"cannot read input: {exc}") from exc
            return _load_package(text)
        return _load_package(content)
    if _is_gridfm_format(format):
        return _parse_transmission(path, content, format, options)
    if _is_dist_format(format):
        return _parse_distribution(path, content, format)
    if path is not None:
        p = Path(path)
        suffix = p.suffix.lower()
        if format is None and p.is_dir() and _looks_like_gridfm_dir(path):
            return _parse_transmission(path, content, "gridfm", options)
        if format is None and suffix == ".dss":
            return _parse_distribution(path, content, format)
        if format is None and suffix == ".json":
            try:
                text = Path(path).read_text(encoding="utf-8")
            except OSError as exc:
                raise ValueError(f"cannot read input: {exc}") from exc
            if _looks_like_package_json(text):
                return _load_package(text)
            domain, inferred = _format_from_json_class(*_json_path_class(path), path=path)
            if domain == "distribution":
                return _parse_distribution(path, content, inferred)
            return _parse_transmission(path, content, inferred, options)
    elif format is None and _jsonish(content):
        if _looks_like_package_json(content):
            return _load_package(content)
        domain, inferred = _format_from_json_class(*_json_class(content))
        if domain == "distribution":
            return _parse_distribution(path, content, inferred)
        return _parse_transmission(path, content, inferred, options)
    return _parse_transmission(path, content, format, options)


def _load_transport(text: str, json_format: Optional[str]) -> _Loaded:
    kind = _transport_kind(text, json_format)
    if kind == "package":
        return _load_package(text)
    if kind in _BMOPF_JSON_FORMATS or kind in {"pmd-json", "pmd_json", "pmd", "engineering"}:
        return _parse_distribution(None, text, kind)
    try:
        net = powerio.from_json(text)
    except powerio.PowerIOError as exc:
        raise ValueError(f"parse failed: {exc}") from exc
    except (ValueError, KeyError, TypeError) as exc:
        raise ValueError(f"parse failed: {exc}") from exc
    return _Loaded("transmission", net, list(net.read_warnings), "powerio-json")


def _load_any(
    path: Optional[str],
    content: Optional[str],
    transport: Optional[str],
    package_json: Optional[str],
    format: Optional[str],
    json_format: Optional[str],
    options: Optional[Dict[str, Any]] = None,
) -> _Loaded:
    _one_network_input(path, content, transport, package_json)
    if package_json is not None:
        return _load_package(package_json)
    if transport is not None:
        return _load_transport(transport, json_format)
    return _parse_any(path, content, format, options)


def _transmission_summary(net: "powerio.Network") -> Dict[str, Any]:
    refs = net.reference_bus_indices()
    return {
        "schema": "powerio.summary",
        "schema_version": _SCHEMA_VERSION,
        "domain": "transmission",
        "model": "balanced",
        "name": net.name,
        "source_format": net.source_format,
        "json_format": "powerio-json",
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


def _distribution_summary(net: "dist.DistNetwork") -> Dict[str, Any]:
    return {
        "schema": "powerio.summary",
        "schema_version": _SCHEMA_VERSION,
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


def _dist_json(net: "dist.DistNetwork") -> tuple[str, list[str]]:
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
    required: bool = True,
) -> Optional[str]:
    if to_format is not None and to is not None and _fmt(to_format) != _fmt(to):
        raise ValueError("`to_format` and `to` disagree")
    target = to_format or to
    if required and target is None:
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
        raise ValueError(f"conversion failed: {exc}") from exc
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
        try:
            return dict(
                loaded.network.write_gridfm(
                    out_path,
                    int(opts.get("scenario", 0)),
                    include_y_bus=bool(opts.get("include_y_bus", True)),
                    include_taps=bool(opts.get("include_taps", True)),
                    include_shifts=bool(opts.get("include_shifts", True)),
                )
            )
        except ImportError as exc:
            raise ValueError(str(exc)) from exc
        except powerio.PowerIOError as exc:
            raise ValueError(f"conversion failed: {exc}") from exc
        except OSError as exc:
            raise ValueError(f"write failed: {exc}") from exc

    if _is_pypsa_format(to_l):
        if loaded.domain != "transmission":
            raise ValueError("pypsa-csv export needs a transmission network")
        try:
            result = loaded.network.write_pypsa_csv_folder(out_path)
        except powerio.PowerIOError as exc:
            raise ValueError(f"conversion failed: {exc}") from exc
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
            raise ValueError(f"conversion failed: {exc}") from exc
        return _write_text(out_path, conv.text, loaded.warnings + list(conv.warnings), overwrite)

    if loaded.domain != "transmission":
        raise ValueError("target is a transmission format but source is distribution")
    try:
        conv = loaded.network.to_format(target)
    except powerio.PowerIOError as exc:
        raise ValueError(f"conversion failed: {exc}") from exc
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
    transport_l = _fmt(transport or "json")
    if transport_l in _PACKAGE_JSON_FORMATS:
        package_json = _package_json_from_input(path, content, from_format, options)
        loaded = _load_package(package_json)
        summary = _summary(loaded)
        diag = _diagnostics_payload(package_json, verbose=True)
        return {
            "schema": "powerio.parse",
            "schema_version": _SCHEMA_VERSION,
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
        "schema": "powerio.parse",
        "schema_version": _SCHEMA_VERSION,
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
        raise ValueError(f"normalization failed: {exc}") from exc
    normalized = _Loaded("transmission", norm, list(norm.read_warnings), "powerio-json")
    summary = _summary(normalized)
    return {
        "schema": "powerio.normalize",
        "schema_version": _SCHEMA_VERSION,
        "domain": "transmission",
        "model": "balanced",
        "source_format": summary["source_format"],
        "json_format": "powerio-json",
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
    convention: str = "paper",
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
        raise ValueError(f"matrix build failed: {exc}") from exc
    coo = mat.tocoo()
    return {
        "schema": "powerio.matrix",
        "schema_version": _SCHEMA_VERSION,
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
        raise ValueError(f"parse failed: {exc}") from exc
    except FileNotFoundError as exc:
        raise ValueError(f"file not found: {exc}") from exc
    except OSError as exc:
        raise ValueError(f"cannot read file: {exc}") from exc
    if data.kind != "powerworld":
        raise ValueError(f"unsupported display format: {data.kind!r}")
    pwd = data.data
    return {
        "schema": "powerio.display",
        "schema_version": _SCHEMA_VERSION,
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


def _capabilities_impl() -> dict:
    source_formats = {
        "transmission": [
            "matpower",
            "powermodels-json",
            "egret-json",
            "psse",
            "psse34",
            "psse35",
            "powerworld",
            "pandapower-json",
            "powerio-json",
            "pypsa-csv",
            "pslf",
            "pwb",
            "gridfm",
        ],
        "distribution": ["dss", "pmd-json", "bmopf-json"],
        "package": ["package", "pio-json", "pio-package"],
    }
    target_formats = {
        "transmission": [
            "matpower",
            "powermodels-json",
            "egret-json",
            "psse",
            "psse34",
            "psse35",
            "powerworld",
            "pandapower-json",
            "powerio-json",
            "pypsa-csv",
            "pslf",
            "gridfm",
        ],
        "distribution": ["dss", "pmd-json", "bmopf-json"],
    }
    return {
        "schema": "powerio.capabilities",
        "schema_version": _SCHEMA_VERSION,
        "model_kinds": ["balanced", "multiconductor"],
        "source_formats": source_formats,
        "target_formats": target_formats,
        "matrix_kinds": sorted(set(_MATRIX_KIND_ALIASES.values())),
        "transports": ["powerio-json", "bmopf-json", "package"],
        "optional_features": {
            "gridfm": bool(getattr(powerio._powerio, "_has_gridfm", False)),
            "mcp": True,
            "distribution": True,
            "package": True,
        },
    }


@mcp.tool(name="convert")
def _convert_tool(
    to_format: str,
    path: Optional[str] = None,
    content: Optional[str] = None,
    json: Optional[str] = None,
    package_json: Optional[str] = None,
    from_format: Optional[str] = None,
    json_format: Optional[str] = None,
    options: Optional[Dict[str, Any]] = None,
) -> dict:
    """Convert a network to a single text format."""
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


@mcp.tool(name="save")
def _save_tool(
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
    """Write a converted network to disk."""
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


@mcp.tool(name="summary")
def _summary_tool(
    path: Optional[str] = None,
    content: Optional[str] = None,
    json: Optional[str] = None,
    package_json: Optional[str] = None,
    from_format: Optional[str] = None,
    json_format: Optional[str] = None,
    options: Optional[Dict[str, Any]] = None,
) -> dict:
    """Return canonical summary JSON for a balanced or multiconductor model."""
    return _summary_impl(
        path, content, json, package_json, from_format, json_format, options
    )


@mcp.tool(name="parse")
def _parse_tool(
    path: Optional[str] = None,
    content: Optional[str] = None,
    from_format: Optional[str] = None,
    options: Optional[Dict[str, Any]] = None,
    transport: str = "json",
) -> dict:
    """Parse a model and return legacy JSON or a `.pio.json` package."""
    return _parse_impl(path, content, from_format, options, transport)


@mcp.tool(name="normalize")
def _normalize_tool(
    path: Optional[str] = None,
    content: Optional[str] = None,
    json: Optional[str] = None,
    package_json: Optional[str] = None,
    from_format: Optional[str] = None,
    json_format: Optional[str] = None,
    options: Optional[Dict[str, Any]] = None,
) -> dict:
    """Normalize a transmission network and return the powerio JSON transport."""
    return _normalize_impl(
        path, content, json, package_json, from_format, json_format, options
    )


@mcp.tool(name="matrix")
def _matrix_tool(
    kind: str,
    path: Optional[str] = None,
    content: Optional[str] = None,
    json: Optional[str] = None,
    package_json: Optional[str] = None,
    from_format: Optional[str] = None,
    json_format: Optional[str] = None,
    options: Optional[Dict[str, Any]] = None,
    scheme: str = "bx",
    convention: str = "paper",
) -> dict:
    """Build a transmission matrix output in COO form."""
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


@mcp.tool(name="capabilities")
def _capabilities_tool() -> dict:
    """List supported model kinds, formats, matrix kinds, and optional features."""
    return _capabilities_impl()


@mcp.tool(name="display")
def _display_tool(path: str, from_format: Optional[str] = None) -> dict:
    """Parse a display artifact and return canonical display JSON."""
    return _display_impl(path, from_format)


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
    target = _choose_to_format(to_format, to=to)
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
    target = _choose_to_format(to_format, to=to, required=False)
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
    convention: str = "paper",
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


def capabilities() -> dict:
    return _capabilities_impl()


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
