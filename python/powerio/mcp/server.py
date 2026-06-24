"""A FastMCP server exposing powerio: case conversion, summaries, the JSON
transport, and sparse matrix views.

Tools for LLM agents, accepting a filesystem ``path``, inline ``content``, or
(for ``save_case``, ``compute_matrix``, and ``dense_view``) the JSON transport
string:

- ``convert_case``: convert a case between formats, returning the text and any
  fidelity warnings.
- ``save_case``: convert and write the result to a file on disk, staging input
  for path-only consumers.
- ``case_summary``: counts, base MVA, source format, and connectivity, with no
  scipy/numpy in the loop.
- ``parse_case`` / ``normalize_case`` / ``case_to_json``: emit the JSON
  transport (``Network.to_json``), the cheap handoff between tool calls.
- ``compute_matrix``: the sparse matrix views in COO form as plain lists.
- ``dense_view``: the dense table view as plain lists and dicts.
- ``read_pypsa_csv_folder`` / ``write_pypsa_csv_folder``: the PyPSA static CSV
  folder format, which has no single-file text form.
- ``read_gridfm`` / ``write_gridfm``: the gridfm-datakit Parquet datasets.
- ``convert_dist_case`` / ``dist_case_summary`` / ``save_dist_case``: the
  multiconductor distribution surface (``powerio.dist``), with its own format
  set (OpenDSS ``.dss``, PowerModelsDistribution ENGINEERING JSON, IEEE BMOPF
  JSON); a distribution case keeps wire detail and has no JSON transport.
- ``read_display_file``: the PowerWorld ``.pwd`` one-line display geometry,
  which travels separately from the network case.

Run over stdio with the ``powerio-mcp`` console script (or ``python -m
powerio.mcp``). The server is a thin wrapper over the powerio Python API; it
never reimplements parsing or math, and inline content converts in memory with
no temp file staging.

This file is canonical for the tool surface. The PowerMCP bundle ships a
standalone copy (``powerio/powerio_mcp.py`` in Power-Agent/PowerMCP); land
changes here first and sync that copy.
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


def _one_input(path: Optional[str], content: Optional[str]) -> None:
    if (path is None) == (content is None):
        raise ValueError("provide exactly one of `path` or `content`")


def _parse(
    path: Optional[str], content: Optional[str], format: Optional[str]
) -> "powerio.Network":
    """Parse from exactly one of ``path`` or inline ``content``, mapping powerio
    and filesystem errors to ValueError so MCP clients see one error shape.
    ``format`` forwards to the parser; ``None`` infers from the path extension
    or means ``matpower`` for inline content."""
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
        # e.g. an unreadable file (permissions); keep the one error shape.
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
        # The Rust layer already maps malformed and wrong-schema JSON to
        # PowerIOParseError; this guards future Python-side paths so the tool
        # keeps its one error shape.
        raise ValueError(f"parse failed: {exc}") from exc


def _summary(case: "powerio.Network") -> Dict[str, Any]:
    return {
        "name": case.name,
        "base_mva": case.base_mva,
        "source_format": case.source_format,
        "n_buses": case.n_buses,
        "n_branches": case.n_branches,
        "n_gens": case.n_gens,
        "n_loads": case.n_loads,
        "n_shunts": case.n_shunts,
        "is_radial": case.is_radial,
        "n_connected_components": case.n_connected_components,
        "connectivity_report": case.connectivity_report(),
        "read_warnings": list(case.read_warnings),
    }


@mcp.tool()
def convert_case(
    to: str,
    path: Optional[str] = None,
    content: Optional[str] = None,
    from_: Optional[str] = None,
) -> dict:
    """Convert a power system case file to another format, losslessly where the
    target allows.

    Provide exactly one of ``path`` (a file on disk) or ``content`` (inline file
    text). ``to``/``from_`` are format names or aliases: ``matpower`` (``m``),
    ``powermodels-json`` (``pm``), ``egret-json`` (``egret``),
    ``pandapower-json`` (``pp``), ``psse`` (``raw``), ``powerworld`` (``aux``).
    PyPSA CSV folders are accepted as path inputs with ``from_="pypsa-csv"``,
    but are not returned as inline text. The input format is inferred from the
    file extension for ``path``; ``from_`` is REQUIRED with inline ``content``.

    Returns ``{"text": <converted file>, "warnings": [<fidelity notes: data the
    target can't represent, defaults synthesized, or blocks mapped to the nearest
    supported target representation>]}`` (empty for a faithful conversion).
    """
    _one_input(path, content)
    if content is not None and not from_:
        raise ValueError("`from_` is required when converting inline `content`")
    try:
        if path is not None:
            conv = powerio.convert_file(path, to, from_)
        else:
            conv = powerio.convert_str(content, to, from_)
    except powerio.PowerIOError as exc:
        raise ValueError(f"conversion failed: {exc}") from exc
    except FileNotFoundError as exc:
        raise ValueError(f"file not found: {exc}") from exc
    except OSError as exc:
        # e.g. an unreadable file (permissions); keep the one error shape.
        raise ValueError(f"cannot read file: {exc}") from exc
    return {"text": conv.text, "warnings": list(conv.warnings)}


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
    """Convert a case and write the result to a file on disk.

    Use this to stage input for consumers that only accept file paths: convert
    any case (or the JSON transport from ``parse_case``) to the target format
    and point the other program at ``out_path``. Pick an ``out_path`` extension
    matching ``to`` (``.m``, ``.json``, ``.raw``, ``.aux``).

    ``to`` is a format name or alias: ``matpower`` (``m``), ``powermodels-json``
    (``pm``), ``egret-json`` (``egret``), ``pandapower-json`` (``pp``),
    ``psse`` (``raw``), ``powerworld`` (``aux``). Provide exactly one of
    ``path``, ``content``, or ``json`` (the transport string). ``format`` is
    the source format name; default: inferred from the path extension, or
    ``matpower`` for inline ``content``. An existing ``out_path`` is not
    overwritten unless ``overwrite`` is true.

    Returns ``{"path": <absolute path written>, "bytes_written": <count>,
    "warnings": [<read fidelity notes, then write fidelity notes>]}``. Read
    notes are always included, even when the output format matches the source
    (where ``convert_case`` reports none because the text is a byte exact
    echo): this tool's warnings describe the written file end to end.
    """
    case = _load(path, content, json, format)
    try:
        conv = case.to_format(to)
    except powerio.PowerIOError as exc:
        raise ValueError(f"conversion failed: {exc}") from exc
    try:
        # newline="" disables newline translation so the file is byte-identical
        # to the converter output (and to the CLI) on every platform, and
        # bytes_written below is exact on Windows.
        mode = "w" if overwrite else "x"
        with open(out_path, mode, encoding="utf-8", newline="") as fh:
            fh.write(conv.text)
    except FileExistsError:
        raise ValueError(
            f"refusing to overwrite existing file: {out_path}; pass overwrite=true"
        ) from None
    except OSError as exc:
        raise ValueError(f"write failed: {exc}") from exc
    return {
        "path": os.path.abspath(out_path),
        "bytes_written": len(conv.text.encode("utf-8")),
        # to_format bypasses the hub's convert fold, so prepend the read side
        # here. Deliberately unconditional: the hub suppresses read warnings on
        # a byte exact echo, but this report covers the written file end to
        # end (pinned in test_mcp.py).
        "warnings": list(case.read_warnings) + list(conv.warnings),
    }


@mcp.tool()
def case_summary(
    path: Optional[str] = None,
    content: Optional[str] = None,
    format: Optional[str] = None,
) -> dict:
    """Summarize a power system case: name, base MVA, source format, element
    counts, connectivity, and read fidelity warnings.

    Provide exactly one of ``path`` or ``content``. ``format`` is the source
    format name; default: inferred from the path extension, or ``matpower``
    for inline ``content``. Pulls in no scipy/numpy.
    """
    return _summary(_parse(path, content, format))


@mcp.tool()
def parse_case(
    path: Optional[str] = None,
    content: Optional[str] = None,
    format: Optional[str] = None,
) -> dict:
    """Parse a power system case once and return its JSON transport plus a
    summary.

    Provide exactly one of ``path`` or ``content``. ``format`` is the source
    format name; default: inferred from the path extension, or ``matpower``
    for inline ``content``. Formats: ``matpower``, ``powermodels-json``,
    ``egret-json``, ``pandapower-json``, ``psse``, ``powerworld``, and
    ``pypsa-csv`` for path inputs.

    The returned ``json`` string is the exchange format between tool calls:
    pass it to ``compute_matrix``, ``dense_view``, and ``save_case`` here, or
    to any downstream tool that ingests the transport (e.g. PowerMCP's
    pandapower, egret, and PyPSA bridges), instead of parsing the file again
    on every call.

    Returns ``{"json": <transport string>, "summary": <case_summary fields>}``.
    """
    case = _parse(path, content, format)
    return {"json": case.to_json(), "summary": _summary(case)}


@mcp.tool()
def normalize_case(
    path: Optional[str] = None,
    content: Optional[str] = None,
    format: Optional[str] = None,
) -> dict:
    """Parse a case and return the JSON transport of its normalized form: per
    unit, radians, out of service elements filtered, source bus ids preserved,
    bus types canonicalized.

    Use this instead of ``parse_case`` when downstream math wants a computation
    ready case rather than the verbatim source tables. Provide exactly one of
    ``path`` or ``content``. ``format`` is the source format name; default:
    inferred from the path extension, or ``matpower`` for inline ``content``.

    Returns ``{"json": <transport string>, "summary": <fields of the normalized
    case>}``; the ``json`` is accepted everywhere the ``parse_case`` transport
    is.
    """
    case = _parse(path, content, format)
    try:
        norm = case.to_normalized()
    except powerio.PowerIOError as exc:
        raise ValueError(f"normalization failed: {exc}") from exc
    return {"json": norm.to_json(), "summary": _summary(norm)}


@mcp.tool()
def case_to_json(
    path: Optional[str] = None,
    content: Optional[str] = None,
    format: Optional[str] = None,
) -> dict:
    """Convert a case file (or inline text) to the powerio JSON transport
    string.

    Provide exactly one of ``path`` or ``content``. ``format`` is the source
    format name; default: inferred from the path extension, or ``matpower``
    for inline ``content``. The returned ``json`` is accepted by
    ``compute_matrix``, ``dense_view``, ``save_case``, and any downstream tool
    that ingests the transport. Use ``parse_case`` instead if you also want
    the summary.

    Returns ``{"json": <transport string>}``.
    """
    return {"json": _parse(path, content, format).to_json()}


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
    """Build a sparse matrix view of a case and return it in COO form.

    ``kind`` is one of: ``bprime`` (FDPF B', shuntless), ``bdoubleprime`` (FDPF
    B'' with shunts and taps), ``ybus_real`` / ``ybus_imag`` (Re/Im of Y_bus),
    ``adjacency`` (0/1 bus adjacency), ``ptdf`` (DC PTDF, m×n), ``lodf`` (DC
    LODF, m×m), ``laplacian`` (weighted Laplacian L = A diag(b) Aᵀ), ``lacpf``
    (linearized AC 2n×2n block [[G, -B], [-B, -G]], taps and shifts included).
    ``scheme`` ("bx"|"xb") applies to bprime/bdoubleprime; ``convention``
    ("paper"|"matpower") to ptdf/lodf/laplacian.

    Provide exactly one of ``path``, ``content``, or ``json``, the transport
    string from ``parse_case`` / ``normalize_case`` / ``case_to_json``; passing
    it skips parsing again. ``format`` is the source format name; default:
    inferred from the path extension, or ``matpower`` for inline ``content``.

    Returns ``{"format": "coo", "shape": [rows, cols], "nnz": <count>,
    "data": [...], "row": [...], "col": [...]}`` with plain Python lists.
    Requires scipy (``pip install 'powerio[matrix]'``).
    """
    if kind not in _MATRIX_KINDS:
        raise ValueError(
            f"unknown matrix kind {kind!r}; expected one of: {', '.join(_MATRIX_KINDS)}"
        )
    case = _load(path, content, json, format)
    try:
        if kind == "bprime":
            m = case.bprime(scheme)
        elif kind == "bdoubleprime":
            m = case.bdoubleprime(scheme)
        elif kind in ("ybus_real", "ybus_imag"):
            parts = case.ybus_parts()
            m = parts.g if kind == "ybus_real" else parts.b
        elif kind == "adjacency":
            m = case.adjacency()
        elif kind == "ptdf":
            m = case.ptdf(convention)
        elif kind == "lodf":
            m = case.lodf(convention)
        elif kind == "lacpf":
            m = case.lacpf()
        elif kind == "laplacian":
            m = case.weighted_laplacian(convention)
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
    """Dense table view of a case as plain lists and dicts: counts, base MVA,
    bus ids, branch arrays (from_id, to_id, r, x, b, tap, shift, in_service),
    generator arrays (bus, pg, pmax, pmin, in_service), nodal demand and shunt
    arrays, the reference bus index, connected component count, and radial
    flag.

    Provide exactly one of ``path``, ``content``, or ``json`` (the transport
    string from ``parse_case`` / ``normalize_case`` / ``case_to_json``).
    ``format`` is the source format name; default: inferred from the path
    extension, or ``matpower`` for inline ``content``. Requires numpy
    (``pip install 'powerio[matrix]'``).
    """
    case = _load(path, content, json, format)
    try:
        d = case.to_dense()
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
def read_pypsa_csv_folder(folder: str) -> dict:
    """Read a PyPSA static CSV folder into the JSON transport plus a summary.

    ``folder`` is a directory of PyPSA component CSVs (``buses.csv``,
    ``generators.csv``, ``lines.csv``, ...). PyPSA CSV is a folder format with
    no single-file text form; ``convert_case`` / ``case_summary`` accept such a
    folder as a ``path`` input, but this tool returns the JSON transport in one
    call so the case can be handed to ``compute_matrix`` / ``dense_view`` or any
    downstream consumer without re-reading the folder.

    Returns ``{"json": <transport string>, "summary": <case_summary fields>,
    "warnings": [<read fidelity notes>]}``.
    """
    try:
        case = powerio.read_pypsa_csv_folder(folder)
    except powerio.PowerIOError as exc:
        raise ValueError(f"parse failed: {exc}") from exc
    except FileNotFoundError as exc:
        raise ValueError(f"file not found: {exc}") from exc
    except OSError as exc:
        raise ValueError(f"cannot read folder: {exc}") from exc
    return {
        "json": case.to_json(),
        "summary": _summary(case),
        "warnings": list(getattr(case, "read_warnings", []) or []),
    }


@mcp.tool()
def write_pypsa_csv_folder(
    out_dir: str,
    path: Optional[str] = None,
    content: Optional[str] = None,
    json: Optional[str] = None,
    format: Optional[str] = None,
) -> dict:
    """Write a case out as a PyPSA static CSV folder.

    Converts any case — a file ``path``, inline ``content`` (with ``format``),
    or the ``json`` transport from ``parse_case`` — to PyPSA's CSV component
    tables under ``out_dir`` (created if needed). The PyPSA-CSV counterpart of
    ``save_case`` for the folder format. ``format`` is the source format name;
    default: inferred from the path extension, or ``matpower`` for inline
    ``content``.

    Returns ``{"dir": <folder written>, "files": [<csv paths>],
    "warnings": [<fidelity notes>]}``.
    """
    case = _load(path, content, json, format)
    try:
        result = case.write_pypsa_csv_folder(out_dir)
    except powerio.PowerIOError as exc:
        raise ValueError(f"conversion failed: {exc}") from exc
    except OSError as exc:
        raise ValueError(f"write failed: {exc}") from exc
    return {
        "dir": result.get("dir", out_dir),
        "files": list(result.get("files", [])),
        "warnings": list(result.get("warnings", [])),
    }


@mcp.tool()
def read_gridfm(dir: str, scenario: int = 0) -> dict:
    """Read one scenario of a gridfm-datakit Parquet dataset into the transport.

    ``dir`` is resolved leniently: the ``raw/`` directory holding the parquet
    files, a ``<case>/`` directory with a ``raw/`` child, or a parent with one
    ``*/raw/`` child all work. ``scenario`` selects one snapshot from a batch
    (``0``, the base case, by default). The read is lossy but recovers
    everything a power flow needs; what it can't recover is in ``warnings``.

    Returns ``{"json": <transport string>, "summary": <case_summary fields>,
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
    case = result.network
    return {
        "json": case.to_json(),
        "summary": _summary(case),
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
    """Write a case as a gridfm-datakit Parquet dataset under ``out_dir``.

    Converts any case — a file ``path``, inline ``content`` (with ``format``),
    or the ``json`` transport — and writes the gridfm layout
    (``<case>/raw/*.parquet`` plus ``gridfm_meta.json``). ``scenario`` tags the
    snapshot id; the ``include_*`` flags toggle the Y-bus, tap, and shift
    columns. ``format`` is the source format name; default: inferred from the
    path extension, or ``matpower`` for inline ``content``.

    Returns the writer's report ``{"dir": ..., "files": [...], ...}``. Requires
    a powerio build with the native gridfm writer (published wheels include it).
    """
    case = _load(path, content, json, format)
    try:
        result = case.write_gridfm(
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


# ---------------------------------------------------------------------------
# Distribution cases: multiconductor unbalanced networks (powerio.dist), with
# their own three-way format set — OpenDSS .dss, PowerModelsDistribution
# ENGINEERING JSON, and IEEE BMOPF JSON. Distinct from the transmission Network
# above: a DistCase keeps multiconductor wire detail, has no JSON transport (so
# these tools take only path/content, never json), and is not accepted by the
# matrix builders. dist.parse_str / dist.convert_str also require an explicit
# format — there is no `matpower` default to fall back to for inline content.
# ---------------------------------------------------------------------------


def _one_dist_input(path: Optional[str], content: Optional[str]) -> None:
    if (path is None) == (content is None):
        raise ValueError("provide exactly one of `path` or `content`")


def _parse_dist(
    path: Optional[str], content: Optional[str], format: Optional[str]
) -> "dist.DistCase":
    """Parse a distribution case from exactly one of ``path`` or inline
    ``content``, mapping powerio and filesystem errors to ValueError so MCP
    clients see one error shape. ``format`` forwards to the parser; for a
    ``path`` it may be ``None`` (inferred from the extension), but it is
    REQUIRED for inline ``content`` (``dist.parse_str`` has no default)."""
    _one_dist_input(path, content)
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


def _dist_summary(case: "dist.DistCase") -> Dict[str, Any]:
    return {
        "source_format": case.source_format,
        "n_buses": case.n_buses,
        "n_lines": case.n_lines,
        "n_transformers": case.n_transformers,
        "n_loads": case.n_loads,
        "n_generators": case.n_generators,
        "warnings": list(case.warnings),
    }


@mcp.tool()
def convert_dist_case(
    to: str,
    path: Optional[str] = None,
    content: Optional[str] = None,
    from_: Optional[str] = None,
) -> dict:
    """Convert a multiconductor distribution case to another format, losslessly
    where the target allows.

    The distribution counterpart of ``convert_case`` (which handles
    transmission cases). Provide exactly one of ``path`` (a file on disk) or
    ``content`` (inline file text). ``to``/``from_`` are distribution format
    names: ``dss`` (OpenDSS), ``pmd-json`` (PowerModelsDistribution ENGINEERING
    JSON), or ``bmopf-json`` (IEEE BMOPF task-force JSON). For a ``path`` the
    input format is inferred from the extension (``.dss`` is OpenDSS; a
    ``.json`` holding a top-level ENGINEERING ``data_model`` key is PMD JSON,
    otherwise BMOPF JSON); ``from_`` is REQUIRED with inline ``content``.

    Returns ``{"text": <converted file>, "warnings": [<fidelity notes: both the
    parse warnings and any data the target format could not represent>]}``
    (empty for a faithful conversion; writing back to the source format echoes
    the retained source text byte for byte).
    """
    _one_dist_input(path, content)
    if content is not None and not from_:
        raise ValueError("`from_` is required when converting inline `content`")
    try:
        if path is not None:
            conv = dist.convert_file(path, to, from_)
        else:
            conv = dist.convert_str(content, to, from_)
    except powerio.PowerIOError as exc:
        raise ValueError(f"conversion failed: {exc}") from exc
    except FileNotFoundError as exc:
        raise ValueError(f"file not found: {exc}") from exc
    except OSError as exc:
        raise ValueError(f"cannot read file: {exc}") from exc
    return {"text": conv.text, "warnings": list(conv.warnings)}


@mcp.tool()
def dist_case_summary(
    path: Optional[str] = None,
    content: Optional[str] = None,
    format: Optional[str] = None,
) -> dict:
    """Summarize a multiconductor distribution case: source format, element
    counts, and parse fidelity warnings.

    The distribution counterpart of ``case_summary`` (which handles
    transmission cases). Provide exactly one of ``path`` or ``content``.
    ``format`` is the distribution format name (``dss``, ``pmd-json``,
    ``bmopf-json``); for a ``path`` it is inferred from the extension when
    omitted, but it is REQUIRED for inline ``content``.

    Returns ``{"source_format": <str>, "n_buses": <int>, "n_lines": <int>,
    "n_transformers": <int>, "n_loads": <int>, "n_generators": <int>,
    "warnings": [<everything the reader could not represent or had to assume>]}``.
    Distribution cases keep multiconductor wire detail and have no positive
    sequence base MVA or connectivity summary, so those ``case_summary`` fields
    are absent here.
    """
    return _dist_summary(_parse_dist(path, content, format))


@mcp.tool()
def save_dist_case(
    to: str,
    out_path: str,
    path: Optional[str] = None,
    content: Optional[str] = None,
    format: Optional[str] = None,
    overwrite: bool = False,
) -> dict:
    """Convert a distribution case and write the result to a file on disk.

    The distribution counterpart of ``save_case``. Use this to stage input for
    consumers that only accept file paths — for example a distribution solver
    or PowerMCP's OpenDSS engine, which compiles a ``.dss`` file: convert any
    distribution case to ``.dss`` here and point the other program at
    ``out_path``. Distribution cases have no JSON transport, so a file is the
    only handoff. Pick an ``out_path`` extension matching ``to`` (``.dss`` for
    OpenDSS, ``.json`` for ``pmd-json``/``bmopf-json``).

    ``to`` is a distribution format name (``dss``, ``pmd-json``,
    ``bmopf-json``). Provide exactly one of ``path`` or ``content``. ``format``
    is the source format name; for a ``path`` it is inferred from the extension
    when omitted, but it is REQUIRED for inline ``content``. An existing
    ``out_path`` is not overwritten unless ``overwrite`` is true.

    Returns ``{"path": <absolute path written>, "bytes_written": <count>,
    "warnings": [<parse fidelity notes, then write fidelity notes — the full
    fidelity story of the written file>]}``. The warnings already fold the
    parse side in (``dist.convert_*`` carries both), so they describe the
    written file end to end.
    """
    _one_dist_input(path, content)
    if content is not None and not format:
        raise ValueError("`format` is required when converting inline `content`")
    try:
        if path is not None:
            conv = dist.convert_file(path, to, format)
        else:
            conv = dist.convert_str(content, to, format)
    except powerio.PowerIOError as exc:
        raise ValueError(f"conversion failed: {exc}") from exc
    except FileNotFoundError as exc:
        raise ValueError(f"file not found: {exc}") from exc
    except OSError as exc:
        raise ValueError(f"cannot read file: {exc}") from exc
    try:
        # newline="" disables newline translation so the file is byte-identical
        # to the converter output on every platform, and bytes_written is exact.
        mode = "w" if overwrite else "x"
        with open(out_path, mode, encoding="utf-8", newline="") as fh:
            fh.write(conv.text)
    except FileExistsError:
        raise ValueError(
            f"refusing to overwrite existing file: {out_path}; pass overwrite=true"
        ) from None
    except OSError as exc:
        raise ValueError(f"write failed: {exc}") from exc
    return {
        "path": os.path.abspath(out_path),
        "bytes_written": len(conv.text.encode("utf-8")),
        "warnings": list(conv.warnings),
    }


# ---------------------------------------------------------------------------
# Display artifacts: the one-line diagram geometry that travels separately from
# the network case (powerio.parse_display_file), exposed so a client can place
# buses on a one-line or map without the vendor tool installed.
# ---------------------------------------------------------------------------


@mcp.tool()
def read_display_file(path: str) -> dict:
    """Decode a PowerWorld ``.pwd`` display file into canvas + substation layout.

    A ``.pwd`` is the one-line *display* artifact (diagram geometry), separate
    from the network case in a ``.pwb`` / ``.aux``. This reads the diagram's
    canvas size, its stamp, and each substation's display coordinates, so a
    client can place buses on a one-line or map without PowerWorld installed.

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
    # powerio's DisplayData is generic (kind + data); only "powerworld" yields a
    # PwdDisplay. Reject any other kind with a clean error instead of an opaque
    # AttributeError if a future powerio adds one.
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


def main() -> None:
    """Console-script entry point: serve the tools over stdio."""
    mcp.run()
