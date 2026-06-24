"""FastMCP server for powerio.

The advertised MCP surface is semantic and format neutral:

``convert``, ``save``, ``summary``, ``parse``, ``normalize``, ``matrix``,
``display``.

Network tools route transmission cases, distribution cases, PyPSA CSV folders,
and gridfm datasets through the lower level powerio APIs. Transmission parses
serialize through the ``powerio-json`` transport. Distribution parses serialize
through canonical ``bmopf-json``.
"""

from __future__ import annotations

from dataclasses import dataclass
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
_ALLOWED_ROOTS_ENV = "POWERIO_MCP_ALLOWED_ROOTS"
_LEGACY_ALLOWED_ROOT_ENV = "POWERIO_MCP_ROOT"

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


def _fmt(value: Optional[str]) -> Optional[str]:
    return value.strip().lower().replace("_", "-") if value is not None else None


def _opts(options: Optional[Dict[str, Any]]) -> Dict[str, Any]:
    return dict(options or {})


def _one_input(path: Optional[str], content: Optional[str]) -> None:
    if (path is None) == (content is None):
        raise ValueError("provide exactly one of `path` or `content`")


def _one_network_input(
    path: Optional[str], content: Optional[str], transport: Optional[str]
) -> None:
    if sum(v is not None for v in (path, content, transport)) != 1:
        raise ValueError("provide exactly one of `path`, `content`, or `json`")


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
        if parsed.netloc not in ("", "localhost"):
            raise ValueError(f"`{purpose}` file URI must be local")
        return Path(unquote(parsed.path)).expanduser()
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
            f"ambiguous JSON markers{where}; pass `format` or `json_format`"
        )
    raise ValueError(f"cannot infer JSON format{where}; pass `format` or `json_format`")


def _transport_kind(text: str, json_format: Optional[str]) -> str:
    fmt = _fmt(json_format)
    if fmt in _POWERIO_JSON_FORMATS:
        return "powerio-json"
    if fmt in _BMOPF_JSON_FORMATS:
        return "bmopf-json"
    if fmt is not None:
        raise ValueError(
            "`json_format` must be `powerio-json` or `bmopf-json`, "
            f"got {json_format!r}"
        )
    domain, format = _format_from_json_class(*_json_class(text))
    if domain == "distribution":
        return format
    if format == "powerio-json":
        return "powerio-json"
    raise ValueError(
        "`json` transport must be `powerio-json` or `bmopf-json`; "
        "pass case JSON as `content` with `format`"
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
            raise ValueError("ambiguous JSON markers; pass `format`")
        else:
            raise ValueError("`format` is required for inline distribution content")
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
            domain, inferred = _format_from_json_class(*_json_path_class(path), path=path)
            if domain == "distribution":
                return _parse_distribution(path, content, inferred)
            return _parse_transmission(path, content, inferred, options)
    elif format is None and _jsonish(content):
        domain, inferred = _format_from_json_class(*_json_class(content))
        if domain == "distribution":
            return _parse_distribution(path, content, inferred)
        return _parse_transmission(path, content, inferred, options)
    return _parse_transmission(path, content, format, options)


def _load_transport(text: str, json_format: Optional[str]) -> _Loaded:
    kind = _transport_kind(text, json_format)
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
    format: Optional[str],
    json_format: Optional[str],
    options: Optional[Dict[str, Any]] = None,
) -> _Loaded:
    _one_network_input(path, content, transport)
    if transport is not None:
        return _load_transport(transport, json_format)
    return _parse_any(path, content, format, options)


def _transmission_summary(net: "powerio.Network") -> Dict[str, Any]:
    refs = net.reference_bus_indices()
    return {
        "domain": "transmission",
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
        "domain": "distribution",
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
        return _distribution_summary(loaded.network)
    return _transmission_summary(loaded.network)


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


@mcp.tool()
def convert(
    to: str,
    path: Optional[str] = None,
    content: Optional[str] = None,
    json: Optional[str] = None,
    format: Optional[str] = None,
    json_format: Optional[str] = None,
    options: Optional[Dict[str, Any]] = None,
) -> dict:
    """Convert a network to a single text format.

    Inputs can be a file/folder/dataset ``path``, inline ``content``, or a
    transport ``json`` from ``parse``/``normalize``. ``format`` names the input
    format when inference is not enough; ``json_format`` is ``powerio-json`` or
    ``bmopf-json``. Folder and dataset targets write through ``save``.
    """
    to_l = _fmt(to)
    if _is_pypsa_format(to_l):
        raise ValueError("`pypsa-csv` writes a folder; use save(to='pypsa-csv')")
    if _is_gridfm_format(to_l):
        raise ValueError("`gridfm` writes a dataset; use save(to='gridfm')")
    loaded = _load_any(path, content, json, format, json_format, options)
    try:
        if _is_dist_format(to_l):
            if loaded.domain != "distribution":
                raise ValueError(
                    "no conversion path between transmission and distribution formats"
                )
            conv = loaded.network.to_format(to)
            warnings = loaded.warnings + list(conv.warnings)
        else:
            if loaded.domain != "transmission":
                raise ValueError(
                    "no conversion path between distribution and transmission formats"
                )
            conv = loaded.network.to_format(to)
            warnings = loaded.warnings + list(conv.warnings)
    except powerio.PowerIOError as exc:
        raise ValueError(f"conversion failed: {exc}") from exc
    return {"text": conv.text, "warnings": warnings}


@mcp.tool()
def save(
    to: str,
    out_path: str,
    path: Optional[str] = None,
    content: Optional[str] = None,
    json: Optional[str] = None,
    format: Optional[str] = None,
    json_format: Optional[str] = None,
    options: Optional[Dict[str, Any]] = None,
    overwrite: bool = False,
) -> dict:
    """Write a converted network to disk.

    Text targets write files. ``pypsa-csv`` writes a folder. ``gridfm`` writes a
    dataset. ``options`` carries format-specific fields such as gridfm
    ``scenario`` and column toggles.
    """
    opts = _opts(options)
    out_path = _local_path(out_path, purpose="out_path", for_write=True)
    loaded = _load_any(path, content, json, format, json_format, options)
    to_l = _fmt(to)

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
            conv = loaded.network.to_format(to)
        except powerio.PowerIOError as exc:
            raise ValueError(f"conversion failed: {exc}") from exc
        return _write_text(out_path, conv.text, loaded.warnings + list(conv.warnings), overwrite)

    if loaded.domain != "transmission":
        raise ValueError("target is a transmission format but source is distribution")
    try:
        conv = loaded.network.to_format(to)
    except powerio.PowerIOError as exc:
        raise ValueError(f"conversion failed: {exc}") from exc
    return _write_text(out_path, conv.text, loaded.warnings + list(conv.warnings), overwrite)


@mcp.tool()
def summary(
    path: Optional[str] = None,
    content: Optional[str] = None,
    json: Optional[str] = None,
    format: Optional[str] = None,
    json_format: Optional[str] = None,
    options: Optional[Dict[str, Any]] = None,
) -> dict:
    """Return the canonical network summary JSON."""
    return _summary(_load_any(path, content, json, format, json_format, options))


@mcp.tool()
def parse(
    path: Optional[str] = None,
    content: Optional[str] = None,
    format: Optional[str] = None,
    options: Optional[Dict[str, Any]] = None,
) -> dict:
    """Parse a network and return its serial JSON transport plus summary."""
    loaded = _parse_any(path, content, format, options)
    if loaded.domain == "distribution":
        text, warnings = _dist_json(loaded.network)
    else:
        text, warnings = loaded.network.to_json(), loaded.warnings
    return {
        "domain": loaded.domain,
        "json_format": loaded.json_format,
        "json": text,
        "summary": _summary(loaded),
        "warnings": warnings,
    }


@mcp.tool()
def normalize(
    path: Optional[str] = None,
    content: Optional[str] = None,
    json: Optional[str] = None,
    format: Optional[str] = None,
    json_format: Optional[str] = None,
    options: Optional[Dict[str, Any]] = None,
) -> dict:
    """Normalize a transmission network and return the powerio JSON transport."""
    loaded = _load_any(path, content, json, format, json_format, options)
    if loaded.domain != "transmission":
        raise ValueError("normalization is not defined for distribution networks")
    try:
        norm = loaded.network.to_normalized()
    except powerio.PowerIOError as exc:
        raise ValueError(f"normalization failed: {exc}") from exc
    normalized = _Loaded("transmission", norm, list(norm.read_warnings), "powerio-json")
    return {
        "domain": "transmission",
        "json_format": "powerio-json",
        "json": norm.to_json(),
        "summary": _summary(normalized),
        "warnings": list(norm.read_warnings),
    }


@mcp.tool()
def matrix(
    kind: str,
    path: Optional[str] = None,
    content: Optional[str] = None,
    json: Optional[str] = None,
    format: Optional[str] = None,
    json_format: Optional[str] = None,
    options: Optional[Dict[str, Any]] = None,
    scheme: str = "bx",
    convention: str = "paper",
) -> dict:
    """Build a transmission matrix output in COO form.

    ``kind`` accepts common names and intent aliases: ``bprime``/``b``/``b1``,
    ``bdoubleprime``/``b2``/``bpp``, ``ybus_real``/``g``,
    ``ybus_imag``/``negB``/``b_lap``, ``adjacency``/``adj``, ``ptdf``,
    ``lodf``, ``laplacian``, or ``lacpf``.
    """
    canonical = _MATRIX_KIND_ALIASES.get(kind.lower())
    if canonical is None:
        raise ValueError(f"unknown matrix kind {kind!r}; expected one of: {_MATRIX_HELP}")
    loaded = _load_any(path, content, json, format, json_format, options)
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
        "format": "coo",
        "kind": canonical,
        "shape": [int(coo.shape[0]), int(coo.shape[1])],
        "nnz": int(coo.nnz),
        "data": coo.data.tolist(),
        "row": coo.row.tolist(),
        "col": coo.col.tolist(),
    }


@mcp.tool()
def display(path: str, format: Optional[str] = None) -> dict:
    """Parse a display artifact and return canonical display JSON."""
    path = _local_path(path, purpose="path")
    try:
        data = powerio.parse_display_file(path, format)
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
        "domain": "display",
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


# Non-advertised compatibility callables for direct Python imports.
def compute_matrix(*args: Any, **kwargs: Any) -> dict:
    return matrix(*args, **kwargs)


def convert_case(
    to: str,
    path: Optional[str] = None,
    content: Optional[str] = None,
    from_: Optional[str] = None,
) -> dict:
    return convert(to=to, path=path, content=content, format=from_)


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
        to=to,
        out_path=out_path,
        path=path,
        content=content,
        json=json,
        format=format,
        overwrite=overwrite,
    )


def case_summary(
    path: Optional[str] = None,
    content: Optional[str] = None,
    json: Optional[str] = None,
    format: Optional[str] = None,
) -> dict:
    return summary(path=path, content=content, json=json, format=format)


def parse_case(
    path: Optional[str] = None,
    content: Optional[str] = None,
    format: Optional[str] = None,
) -> dict:
    return parse(path=path, content=content, format=format)


def normalize_case(
    path: Optional[str] = None,
    content: Optional[str] = None,
    format: Optional[str] = None,
) -> dict:
    return normalize(path=path, content=content, format=format)


def case_to_json(
    path: Optional[str] = None,
    content: Optional[str] = None,
    format: Optional[str] = None,
) -> dict:
    result = parse(path=path, content=content, format=format)
    return {"json": result["json"], "json_format": result["json_format"]}


def write_pypsa_csv_folder(
    out_dir: str,
    path: Optional[str] = None,
    content: Optional[str] = None,
    json: Optional[str] = None,
    format: Optional[str] = None,
) -> dict:
    return save(
        to="pypsa-csv",
        out_path=out_dir,
        path=path,
        content=content,
        json=json,
        format=format,
    )


def read_pypsa_csv_folder(folder: str) -> dict:
    return parse(path=folder)


def main() -> None:
    """Console-script entry point: serve the tools over stdio."""
    mcp.run()
