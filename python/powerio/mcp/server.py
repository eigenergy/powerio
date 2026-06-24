"""A FastMCP server exposing powerio: one set of network tools that cover
transmission and distribution, plus the JSON transport and sparse matrix views.

The tool names are the bare powerio verbs (`convert`, `save`, `summary`,
`parse`, ...), matching the CLI (`powerio convert`). Transmission and
distribution share the same tools — `convert`/`save`/`summary` route to the
right engine by format, because the two format sets are disjoint:

- transmission text formats: ``matpower`` (``m``), ``powermodels-json`` (``pm``),
  ``egret-json``, ``pandapower-json`` (``pp``), ``psse`` (``raw``),
  ``powerworld`` (``aux``);
- distribution text formats (``powerio.dist``): ``dss`` (OpenDSS), ``pmd-json``
  (PowerModelsDistribution ENGINEERING JSON), ``bmopf-json`` (IEEE BMOPF JSON);
- directory formats: ``pypsa-csv`` (a folder of CSVs) and ``gridfm`` (a Parquet
  dataset).

Tools:

- ``convert``: convert a single-file text format to another, either domain.
- ``save``: convert and write to disk — a file for text formats, a folder for
  ``pypsa-csv``. ``save(to="dss")`` stages a distribution case for OpenDSS.
- ``summary``: element counts and fidelity warnings, either domain.
- ``parse`` / ``to_json`` / ``normalize``: emit the powerio JSON transport — the
  in-memory handoff between tool calls. Transmission only (a distribution
  network has no JSON transport; serialize it with ``convert``/``save`` to a
  real format instead).
- ``compute_matrix`` / ``dense_view``: sparse and dense views. Transmission only.
- ``read_gridfm`` / ``write_gridfm``: the gridfm Parquet dataset — its own tools
  because it carries dataset options (scenario selection, column toggles) that
  the format-only tools don't.
- ``read_display_file``: the PowerWorld ``.pwd`` one-line display geometry.

Deprecated aliases (removed in 0.4.0): ``convert_case``, ``save_case``,
``case_summary``, ``parse_case``, ``normalize_case``, ``case_to_json``,
``read_pypsa_csv_folder``, ``write_pypsa_csv_folder``. They forward to the new
tools (a ``pypsa-csv`` folder now flows through ``parse``/``summary`` via a
folder ``path`` and ``save(to="pypsa-csv")``).

Run over stdio with the ``powerio-mcp`` console script (or ``python -m
powerio.mcp``). The server is a thin wrapper over the powerio Python API; it
never reimplements parsing or math, and inline content converts in memory with
no temp file staging.

This file is canonical for the tool surface. The PowerMCP bundle re-exports it
(``powerio/powerio_mcp.py`` in Power-Agent/PowerMCP); land changes here first.
"""

from __future__ import annotations

import os
from typing import Any, Dict, Optional

import powerio
from powerio import dist
from mcp.server.fastmcp import FastMCP

mcp = FastMCP("powerio")

_MATRIX_KINDS = (
    "bprime", "bdoubleprime", "ybus_real", "ybus_imag",
    "adjacency", "ptdf", "lodf", "laplacian", "lacpf",
)

# Distribution format names. Disjoint from the transmission format names, so a
# tool can route on the format alone.
_DIST_FORMATS = frozenset({"dss", "pmd-json", "bmopf-json"})

_DIST_NO_TRANSPORT = (
    "distribution networks have no JSON transport; serialize one with "
    "convert(to='bmopf-json'/'pmd-json'/'dss') or save(...), or call summary() "
    "for counts"
)


def _one_input(path: Optional[str], content: Optional[str]) -> None:
    if (path is None) == (content is None):
        raise ValueError("provide exactly one of `path` or `content`")


def _is_dist_format(fmt: Optional[str]) -> bool:
    return fmt is not None and fmt.lower() in _DIST_FORMATS


def _source_is_dist(path: Optional[str], format: Optional[str]) -> bool:
    """Whether the source is a distribution case. An explicit ``format`` decides
    it; otherwise a ``.dss`` path is the only unambiguous distribution
    extension (a bare ``.json`` defaults to transmission and needs an explicit
    ``format`` to be read as ``pmd-json``/``bmopf-json``)."""
    if format is not None:
        return _is_dist_format(format)
    return path is not None and str(path).lower().endswith(".dss")


def _parse(
    path: Optional[str], content: Optional[str], format: Optional[str]
) -> "powerio.Network":
    """Parse a transmission network from one of ``path`` or inline ``content``,
    mapping powerio and filesystem errors to ValueError. ``format`` infers from
    the path extension or means ``matpower`` for inline content."""
    _one_input(path, content)
    try:
        if path is not None:
            return powerio.parse_file(path, format)
        return powerio.parse_str(content, format or "matpower")
    except powerio.PowerIOError as exc:
        raise ValueError(f"parse failed: {exc}") from exc
    except FileNotFoundError as exc:
        raise ValueError(f"file not found: {exc}") from exc
    except OSError as exc:
        raise ValueError(f"cannot read file: {exc}") from exc


def _load(
    path: Optional[str], content: Optional[str], json: Optional[str], format: Optional[str]
) -> "powerio.Network":
    """Like ``_parse`` but also accepts the JSON transport string."""
    if sum(v is not None for v in (path, content, json)) != 1:
        raise ValueError("provide exactly one of `path`, `content`, or `json`")
    if json is None:
        return _parse(path, content, format)
    try:
        return powerio.from_json(json)
    except powerio.PowerIOError as exc:
        raise ValueError(f"parse failed: {exc}") from exc
    except (ValueError, KeyError, TypeError) as exc:
        raise ValueError(f"parse failed: {exc}") from exc


def _summary(net: "powerio.Network") -> Dict[str, Any]:
    return {
        "name": net.name,
        "base_mva": net.base_mva,
        "source_format": net.source_format,
        "n_buses": net.n_buses,
        "n_branches": net.n_branches,
        "n_gens": net.n_gens,
        "n_loads": net.n_loads,
        "n_shunts": net.n_shunts,
        "is_radial": net.is_radial,
        "n_connected_components": net.n_connected_components,
        "connectivity_report": net.connectivity_report(),
        "read_warnings": list(net.read_warnings),
    }


def _parse_dist(
    path: Optional[str], content: Optional[str], format: Optional[str]
) -> "dist.DistNetwork":
    """Parse a distribution network, mapping errors to ValueError. ``format`` is
    inferred from a ``path`` extension but REQUIRED for inline ``content``."""
    _one_input(path, content)
    if content is not None and not format:
        raise ValueError("`format` is required when parsing inline `content`")
    try:
        if path is not None:
            return dist.parse_file(path, format)
        return dist.parse_str(content, format)
    except powerio.PowerIOError as exc:
        raise ValueError(f"parse failed: {exc}") from exc
    except FileNotFoundError as exc:
        raise ValueError(f"file not found: {exc}") from exc
    except OSError as exc:
        raise ValueError(f"cannot read file: {exc}") from exc


def _dist_summary(net: "dist.DistNetwork") -> Dict[str, Any]:
    return {
        "source_format": net.source_format,
        "n_buses": net.n_buses,
        "n_lines": net.n_lines,
        "n_transformers": net.n_transformers,
        "n_loads": net.n_loads,
        "n_generators": net.n_generators,
        # Keyed `read_warnings` to match the transmission summary.
        "read_warnings": list(net.warnings),
    }


@mcp.tool()
def convert(
    to: str,
    path: Optional[str] = None,
    content: Optional[str] = None,
    format: Optional[str] = None,
) -> dict:
    """Convert a power network from one single-file text format to another,
    losslessly where the target allows. Covers both transmission and
    distribution — the engine is chosen by format.

    Provide exactly one of ``path`` (a file on disk) or ``content`` (inline file
    text). ``to``/``format`` are format names: transmission ``matpower`` (``m``),
    ``powermodels-json`` (``pm``), ``egret-json``, ``pandapower-json`` (``pp``),
    ``psse`` (``raw``), ``powerworld`` (``aux``); distribution ``dss``,
    ``pmd-json``, ``bmopf-json``. PSLF EPC is a read-only source (``.epc`` or
    ``format="pslf"``), not a write target. ``format`` is inferred from the path
    extension and REQUIRED for inline ``content`` (and for a distribution
    ``.json``).

    Directory formats are not single-file text: for ``pypsa-csv`` use
    ``save(to="pypsa-csv", ...)``, for ``gridfm`` use ``write_gridfm``. Crossing
    the transmission/distribution boundary (e.g. ``matpower`` → ``dss``) is not
    supported and raises.

    Returns ``{"text": <converted file>, "warnings": [<fidelity notes>]}``
    (empty for a faithful conversion).
    """
    _one_input(path, content)
    to_l = to.lower()
    if to_l == "pypsa-csv":
        raise ValueError("`pypsa-csv` is a directory format; use save(to='pypsa-csv', out_path=...)")
    if to_l == "gridfm":
        raise ValueError("`gridfm` is a dataset format; use write_gridfm(...)")
    if content is not None and not format:
        raise ValueError("`format` is required when converting inline `content`")
    target_dist = _is_dist_format(to)
    source_dist = _source_is_dist(path, format)
    if target_dist != source_dist:
        raise ValueError(
            "cannot convert across the transmission/distribution boundary "
            f"(source {'distribution' if source_dist else 'transmission'}, "
            f"target {'distribution' if target_dist else 'transmission'})"
        )
    try:
        if target_dist:
            conv = dist.convert_file(path, to, format) if path is not None else dist.convert_str(content, to, format)
        else:
            conv = powerio.convert_file(path, to, format) if path is not None else powerio.convert_str(content, to, format)
    except powerio.PowerIOError as exc:
        raise ValueError(f"conversion failed: {exc}") from exc
    except FileNotFoundError as exc:
        raise ValueError(f"file not found: {exc}") from exc
    except OSError as exc:
        raise ValueError(f"cannot read file: {exc}") from exc
    return {"text": conv.text, "warnings": list(conv.warnings)}


@mcp.tool()
def save(
    to: str,
    out_path: str,
    path: Optional[str] = None,
    content: Optional[str] = None,
    json: Optional[str] = None,
    format: Optional[str] = None,
    overwrite: bool = False,
) -> dict:
    """Convert a network and write the result to disk — a file for text formats,
    a folder for ``pypsa-csv``. Use this to stage input for programs that only
    read paths (a solver, or PowerMCP's OpenDSS engine: ``save(to="dss")``
    writes a ``.dss`` an OpenDSS engine can compile).

    ``to`` is any transmission or distribution format, or ``pypsa-csv``. Provide
    exactly one of ``path``, ``content``, or ``json`` (the transmission
    transport; not valid for a distribution target). ``format`` is the source
    format — inferred from a path extension, REQUIRED for inline ``content``. An
    existing ``out_path`` is not overwritten unless ``overwrite`` is true. For
    ``gridfm`` use ``write_gridfm``.

    Returns ``{"path": <absolute file>, "bytes_written": <count>, "warnings":
    [...]}`` for a text format, or ``{"dir": <folder>, "files": [...],
    "warnings": [...]}`` for ``pypsa-csv``. Warnings cover the written artifact
    end to end (read fidelity notes included).
    """
    to_l = to.lower()
    if to_l == "gridfm":
        raise ValueError("`gridfm` is a dataset format; use write_gridfm(...)")

    if to_l == "pypsa-csv":
        net = _load(path, content, json, format)
        try:
            result = net.write_pypsa_csv_folder(out_path)
        except powerio.PowerIOError as exc:
            raise ValueError(f"conversion failed: {exc}") from exc
        except OSError as exc:
            raise ValueError(f"write failed: {exc}") from exc
        return {
            "dir": result.get("dir", out_path),
            "files": list(result.get("files", [])),
            # Read warnings included, like the text branch: this report covers
            # the written folder end to end.
            "warnings": list(net.read_warnings) + list(result.get("warnings", [])),
        }

    if _is_dist_format(to):
        if json is not None:
            raise ValueError("`json` is the transmission transport; a distribution target takes `path` or `content`")
        _one_input(path, content)
        if content is not None and not format:
            raise ValueError("`format` is required when converting inline `content`")
        if not _source_is_dist(path, format):
            raise ValueError("source is a transmission case but the target is a distribution format")
        try:
            conv = dist.convert_file(path, to, format) if path is not None else dist.convert_str(content, to, format)
        except powerio.PowerIOError as exc:
            raise ValueError(f"conversion failed: {exc}") from exc
        except FileNotFoundError as exc:
            raise ValueError(f"file not found: {exc}") from exc
        except OSError as exc:
            raise ValueError(f"cannot read file: {exc}") from exc
        text, warnings = conv.text, list(conv.warnings)
    else:
        if json is None and _source_is_dist(path, format):
            raise ValueError(
                "cannot save across the transmission/distribution boundary "
                "(distribution source, transmission target)"
            )
        net = _load(path, content, json, format)
        try:
            conv = net.to_format(to)
        except powerio.PowerIOError as exc:
            raise ValueError(f"conversion failed: {exc}") from exc
        # to_format bypasses the hub's read-warning fold, so prepend the read
        # side: this report covers the written file end to end (pinned in tests).
        text, warnings = conv.text, list(net.read_warnings) + list(conv.warnings)

    try:
        # newline="" disables newline translation so the file is byte-identical
        # to the converter output on every platform, and bytes_written is exact.
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
def summary(
    path: Optional[str] = None,
    content: Optional[str] = None,
    format: Optional[str] = None,
) -> dict:
    """Summarize a power network: element counts and read fidelity warnings,
    transmission or distribution.

    Provide exactly one of ``path`` or ``content``. ``format`` is inferred from
    a path extension, REQUIRED for inline ``content`` (and a distribution
    ``.json``). A transmission summary adds name, base MVA, source format, and
    connectivity; a distribution summary reports multiconductor element counts
    (lines/transformers) and has no positive-sequence fields.
    """
    if _source_is_dist(path, format):
        return _dist_summary(_parse_dist(path, content, format))
    return _summary(_parse(path, content, format))


@mcp.tool()
def parse(
    path: Optional[str] = None,
    content: Optional[str] = None,
    format: Optional[str] = None,
) -> dict:
    """Parse a transmission network and return its JSON transport plus a summary.

    The ``json`` string is the in-memory handoff between tool calls: pass it to
    ``compute_matrix``/``dense_view``/``save`` here, or to a downstream tool
    that ingests the transport, instead of re-reading the file. Provide exactly
    one of ``path`` or ``content``; ``format`` is inferred from a path
    extension, ``matpower`` for inline ``content``. A ``pypsa-csv`` folder is a
    valid ``path``.

    Distribution networks have no JSON transport — this rejects a distribution
    format; serialize one with ``convert``/``save`` instead.

    Returns ``{"json": <transport string>, "summary": <summary fields>}``.
    """
    if _source_is_dist(path, format):
        raise ValueError(_DIST_NO_TRANSPORT)
    net = _parse(path, content, format)
    return {"json": net.to_json(), "summary": _summary(net)}


@mcp.tool()
def to_json(
    path: Optional[str] = None,
    content: Optional[str] = None,
    format: Optional[str] = None,
) -> dict:
    """Convert a transmission network to the powerio JSON transport string.

    Provide exactly one of ``path`` or ``content``. ``format`` is inferred from
    a path extension, ``matpower`` for inline ``content``. The returned ``json``
    is accepted by ``compute_matrix``/``dense_view``/``save`` and downstream
    transport consumers; use ``parse`` if you also want the summary. Rejects a
    distribution format (no JSON transport).

    Returns ``{"json": <transport string>}``.
    """
    if _source_is_dist(path, format):
        raise ValueError(_DIST_NO_TRANSPORT)
    return {"json": _parse(path, content, format).to_json()}


@mcp.tool()
def normalize(
    path: Optional[str] = None,
    content: Optional[str] = None,
    format: Optional[str] = None,
) -> dict:
    """Parse a transmission network and return the JSON transport of its
    normalized form: per unit, radians, out-of-service elements filtered, source
    bus ids preserved, bus types canonicalized.

    Use this instead of ``parse`` when downstream math wants a computation-ready
    network. Provide exactly one of ``path`` or ``content``; ``format`` is
    inferred from a path extension, ``matpower`` for inline ``content``. Rejects
    a distribution format (no JSON transport).

    Returns ``{"json": <transport string>, "summary": <fields of the normalized
    network>}``.
    """
    if _source_is_dist(path, format):
        raise ValueError(_DIST_NO_TRANSPORT)
    net = _parse(path, content, format)
    try:
        norm = net.to_normalized()
    except powerio.PowerIOError as exc:
        raise ValueError(f"normalization failed: {exc}") from exc
    return {"json": norm.to_json(), "summary": _summary(norm)}


@mcp.tool()
def compute_matrix(
    kind: str,
    path: Optional[str] = None,
    content: Optional[str] = None,
    json: Optional[str] = None,
    format: Optional[str] = None,
    scheme: str = "bx",
    convention: str = "paper",
) -> dict:
    """Build a sparse matrix view of a transmission network in COO form.

    ``kind`` is one of: ``bprime`` (FDPF B', shuntless), ``bdoubleprime`` (FDPF
    B'' with shunts and taps), ``ybus_real`` / ``ybus_imag`` (Re/Im of Y_bus),
    ``adjacency`` (0/1 bus adjacency), ``ptdf`` (DC PTDF, m×n), ``lodf`` (DC
    LODF, m×m), ``laplacian`` (weighted Laplacian L = A diag(b) Aᵀ), ``lacpf``
    (linearized AC 2n×2n block [[G, -B], [-B, -G]], taps and shifts included).
    ``scheme`` ("bx"|"xb") applies to bprime/bdoubleprime; ``convention``
    ("paper"|"matpower") to ptdf/lodf/laplacian.

    Provide exactly one of ``path``, ``content``, or ``json`` (the transport
    from ``parse``/``normalize``/``to_json``). ``format`` is inferred from a path
    extension, ``matpower`` for inline ``content``. Distribution networks aren't
    positive-sequence, so a distribution format is rejected.

    Returns ``{"format": "coo", "shape": [rows, cols], "nnz": <count>,
    "data": [...], "row": [...], "col": [...]}`` with plain Python lists.
    Requires scipy (``pip install 'powerio[matrix]'``).
    """
    if kind not in _MATRIX_KINDS:
        raise ValueError(
            f"unknown matrix kind {kind!r}; expected one of: {', '.join(_MATRIX_KINDS)}"
        )
    if json is None and _source_is_dist(path, format):
        raise ValueError("compute_matrix needs a transmission network; distribution networks aren't positive-sequence")
    net = _load(path, content, json, format)
    try:
        if kind == "bprime":
            m = net.bprime(scheme)
        elif kind == "bdoubleprime":
            m = net.bdoubleprime(scheme)
        elif kind in ("ybus_real", "ybus_imag"):
            parts = net.ybus_parts()
            m = parts.g if kind == "ybus_real" else parts.b
        elif kind == "adjacency":
            m = net.adjacency()
        elif kind == "ptdf":
            m = net.ptdf(convention)
        elif kind == "lodf":
            m = net.lodf(convention)
        elif kind == "lacpf":
            m = net.lacpf()
        elif kind == "laplacian":
            m = net.weighted_laplacian(convention)
        else:  # pragma: no cover - unreachable; _MATRIX_KINDS is checked above
            raise ValueError(f"unhandled matrix kind {kind!r}")
    except ImportError as exc:
        raise ValueError(str(exc)) from exc
    except powerio.PowerIOError as exc:
        raise ValueError(f"matrix build failed: {exc}") from exc
    coo = m.tocoo()
    return {
        "format": "coo",
        "shape": [int(coo.shape[0]), int(coo.shape[1])],
        "nnz": int(coo.nnz),
        "data": coo.data.tolist(),
        "row": coo.row.tolist(),
        "col": coo.col.tolist(),
    }


@mcp.tool()
def dense_view(
    path: Optional[str] = None,
    content: Optional[str] = None,
    json: Optional[str] = None,
    format: Optional[str] = None,
) -> dict:
    """Dense table view of a transmission network as plain lists and dicts:
    counts, base MVA, bus ids, branch arrays (from_id, to_id, r, x, b, tap,
    shift, in_service), generator arrays (bus, pg, pmax, pmin, in_service),
    nodal demand and shunt arrays, the reference bus index, connected component
    count, and radial flag.

    Provide exactly one of ``path``, ``content``, or ``json`` (the transport
    from ``parse``/``normalize``/``to_json``). ``format`` is inferred from a path
    extension, ``matpower`` for inline ``content``. A distribution format is
    rejected (not positive-sequence). Requires numpy (``pip install
    'powerio[matrix]'``).
    """
    if json is None and _source_is_dist(path, format):
        raise ValueError("dense_view needs a transmission network; distribution networks aren't positive-sequence")
    net = _load(path, content, json, format)
    try:
        d = net.to_dense()
    except ImportError as exc:
        raise ValueError(str(exc)) from exc
    except powerio.PowerIOError as exc:
        raise ValueError(f"dense view failed: {exc}") from exc
    return {
        "n": int(d.n),
        "m": int(d.m),
        "ng": int(d.ng),
        "base_mva": float(d.base_mva),
        "bus_ids": d.bus_ids.tolist(),
        "branch": {
            "from_id": d.branch.from_id.tolist(),
            "to_id": d.branch.to_id.tolist(),
            "r": d.branch.r.tolist(),
            "x": d.branch.x.tolist(),
            "b": d.branch.b.tolist(),
            "tap": d.branch.tap.tolist(),
            "shift": d.branch.shift.tolist(),
            "in_service": d.branch.in_service.tolist(),
        },
        "gen": {
            "bus": d.gen.bus.tolist(),
            "pg": d.gen.pg.tolist(),
            "pmax": d.gen.pmax.tolist(),
            "pmin": d.gen.pmin.tolist(),
            "in_service": d.gen.in_service.tolist(),
        },
        "demand": {"pd": d.demand.pd.tolist(), "qd": d.demand.qd.tolist()},
        "shunt": {"gs": d.shunt.gs.tolist(), "bs": d.shunt.bs.tolist()},
        "reference_bus": None if d.reference_bus is None else int(d.reference_bus),
        "n_components": int(d.n_components),
        "is_radial": bool(d.is_radial),
    }


@mcp.tool()
def read_gridfm(dir: str, scenario: int = 0) -> dict:
    """Read one scenario of a gridfm-datakit Parquet dataset into the transport.

    gridfm has its own read/write tools (not ``parse``/``save``) because it is a
    multi-scenario dataset, not a single case. ``dir`` is resolved leniently:
    the ``raw/`` directory holding the parquet files, a ``<case>/`` directory
    with a ``raw/`` child, or a parent with one ``*/raw/`` child all work.
    ``scenario`` selects one snapshot (``0``, the base case, by default). The
    read is lossy but recovers everything a power flow needs; what it can't
    recover is in ``warnings``.

    Returns ``{"json": <transport string>, "summary": <summary fields>,
    "scenario": <int>, "warnings": [<fidelity notes>]}``. Requires a powerio
    build with the native gridfm reader (published wheels include it).
    """
    try:
        result = powerio.read_gridfm(dir, scenario)
    except powerio.PowerIOError as exc:
        raise ValueError(f"parse failed: {exc}") from exc
    except FileNotFoundError as exc:
        raise ValueError(f"file not found: {exc}") from exc
    except ImportError as exc:
        raise ValueError(str(exc)) from exc
    except OSError as exc:
        raise ValueError(f"cannot read dataset: {exc}") from exc
    net = result.network
    return {
        "json": net.to_json(),
        "summary": _summary(net),
        "scenario": int(result.scenario),
        "warnings": list(result.warnings),
    }


@mcp.tool()
def write_gridfm(
    out_dir: str,
    path: Optional[str] = None,
    content: Optional[str] = None,
    json: Optional[str] = None,
    format: Optional[str] = None,
    scenario: int = 0,
    include_y_bus: bool = True,
    include_taps: bool = True,
    include_shifts: bool = True,
) -> dict:
    """Write a transmission network as a gridfm-datakit Parquet dataset under
    ``out_dir``.

    Converts any transmission network — a file ``path``, inline ``content`` (with
    ``format``), or the ``json`` transport — and writes the gridfm layout
    (``<case>/raw/*.parquet`` plus ``gridfm_meta.json``). ``scenario`` tags the
    snapshot id; the ``include_*`` flags toggle the Y-bus, tap, and shift
    columns — the dataset-specific options that earn gridfm its own tool.

    Returns the writer's report ``{"dir": ..., "files": [...], ...}``. Requires
    a powerio build with the native gridfm writer (published wheels include it).
    """
    net = _load(path, content, json, format)
    try:
        result = net.write_gridfm(
            out_dir,
            scenario,
            include_y_bus=include_y_bus,
            include_taps=include_taps,
            include_shifts=include_shifts,
        )
    except powerio.PowerIOError as exc:
        raise ValueError(f"conversion failed: {exc}") from exc
    except ImportError as exc:
        raise ValueError(str(exc)) from exc
    except OSError as exc:
        raise ValueError(f"write failed: {exc}") from exc
    return dict(result)


@mcp.tool()
def read_display_file(path: str) -> dict:
    """Decode a PowerWorld ``.pwd`` display file into canvas + substation layout.

    A ``.pwd`` is the one-line *display* artifact (diagram geometry), separate
    from the network in a ``.pwb`` / ``.aux``. This reads the diagram's canvas
    size, its stamp, and each substation's display coordinates, so a client can
    place buses on a one-line or map without PowerWorld installed.

    Returns ``{"kind": "powerworld", "canvas_width": <int>,
    "canvas_height": <int>, "stamp": <int>, "substations":
    [{"number": <int>, "name": <str>, "x": <float>, "y": <float>}, ...]}``.
    """
    try:
        display = powerio.parse_display_file(path)
    except powerio.PowerIOError as exc:
        raise ValueError(f"parse failed: {exc}") from exc
    except FileNotFoundError as exc:
        raise ValueError(f"file not found: {exc}") from exc
    except OSError as exc:
        raise ValueError(f"cannot read file: {exc}") from exc
    if display.kind != "powerworld":
        raise ValueError(f"unsupported display format: {display.kind!r}")
    pwd = display.data
    return {
        "kind": display.kind,
        "canvas_width": pwd.canvas_width,
        "canvas_height": pwd.canvas_height,
        "stamp": pwd.stamp,
        "substations": [
            {"number": s.number, "name": s.name, "x": s.x, "y": s.y}
            for s in pwd.substations
        ],
    }


# ---------------------------------------------------------------------------
# Deprecated aliases (removed in 0.4.0). The MCP surface was unified in 0.3.3 to
# the bare verbs; these forward so clients on the old names keep working for one
# release. Each is marked DEPRECATED so an agent prefers the canonical tool.
# ---------------------------------------------------------------------------


@mcp.tool()
def convert_case(
    to: str,
    path: Optional[str] = None,
    content: Optional[str] = None,
    from_: Optional[str] = None,
) -> dict:
    """DEPRECATED (removed in 0.4.0): use ``convert``. The old ``from_`` maps to
    ``format``."""
    return convert(to=to, path=path, content=content, format=from_)


@mcp.tool()
def save_case(
    to: str,
    out_path: str,
    path: Optional[str] = None,
    content: Optional[str] = None,
    json: Optional[str] = None,
    format: Optional[str] = None,
    overwrite: bool = False,
) -> dict:
    """DEPRECATED (removed in 0.4.0): use ``save``."""
    return save(
        to=to, out_path=out_path, path=path, content=content,
        json=json, format=format, overwrite=overwrite,
    )


@mcp.tool()
def case_summary(
    path: Optional[str] = None,
    content: Optional[str] = None,
    format: Optional[str] = None,
) -> dict:
    """DEPRECATED (removed in 0.4.0): use ``summary``."""
    return summary(path=path, content=content, format=format)


@mcp.tool()
def parse_case(
    path: Optional[str] = None,
    content: Optional[str] = None,
    format: Optional[str] = None,
) -> dict:
    """DEPRECATED (removed in 0.4.0): use ``parse``."""
    return parse(path=path, content=content, format=format)


@mcp.tool()
def normalize_case(
    path: Optional[str] = None,
    content: Optional[str] = None,
    format: Optional[str] = None,
) -> dict:
    """DEPRECATED (removed in 0.4.0): use ``normalize``."""
    return normalize(path=path, content=content, format=format)


@mcp.tool()
def case_to_json(
    path: Optional[str] = None,
    content: Optional[str] = None,
    format: Optional[str] = None,
) -> dict:
    """DEPRECATED (removed in 0.4.0): use ``to_json``."""
    return to_json(path=path, content=content, format=format)


@mcp.tool()
def write_pypsa_csv_folder(
    out_dir: str,
    path: Optional[str] = None,
    content: Optional[str] = None,
    json: Optional[str] = None,
    format: Optional[str] = None,
) -> dict:
    """DEPRECATED (removed in 0.4.0): use ``save(to="pypsa-csv", out_path=...)``."""
    return save(to="pypsa-csv", out_path=out_dir, path=path, content=content, json=json, format=format)


@mcp.tool()
def read_pypsa_csv_folder(folder: str) -> dict:
    """DEPRECATED (removed in 0.4.0): read a PyPSA CSV folder with
    ``parse(path=<folder>)`` / ``summary(path=<folder>)``.

    Returns ``{"json": <transport>, "summary": <summary fields>,
    "warnings": [<read fidelity notes>]}``.
    """
    try:
        net = powerio.read_pypsa_csv_folder(folder)
    except powerio.PowerIOError as exc:
        raise ValueError(f"parse failed: {exc}") from exc
    except FileNotFoundError as exc:
        raise ValueError(f"file not found: {exc}") from exc
    except OSError as exc:
        raise ValueError(f"cannot read folder: {exc}") from exc
    return {
        "json": net.to_json(),
        "summary": _summary(net),
        "warnings": list(getattr(net, "read_warnings", []) or []),
    }


def main() -> None:
    """Console-script entry point: serve the tools over stdio."""
    mcp.run()
