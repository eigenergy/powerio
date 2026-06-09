"""A FastMCP server exposing powerio's lossless converter and case summary.

Two tools for LLM agent tooling, both accepting either a filesystem ``path`` or
inline ``content``:

- ``convert_case`` — convert a case file between formats, returning the text and
  any fidelity warnings.
- ``case_summary`` — counts, base MVA, source format, and connectivity, with no
  scipy/numpy in the loop.

Run over stdio with the ``powerio-mcp`` console script (or ``python -m
powerio.mcp``). The server reuses ``powerio.convert``/``parse``/``parse_str`` —
it never reimplements parsing.
"""

from __future__ import annotations

import os
import tempfile
from typing import Optional

import powerio
from mcp.server.fastmcp import FastMCP

mcp = FastMCP("powerio")

# Format name (and alias) → file extension, for staging inline content to a temp
# file. ``convert`` is path-only, so inline conversion writes the text to disk
# first; a matching extension keeps the format obvious even though we always
# pass ``from_`` explicitly for inline input. Mirrors the canonical Rust tables
# (`target_format_from_name` + `TargetFormat::extension` in
# `powerio/src/format/mod.rs`); the temp file goes away once powerio grows an
# in-memory `convert_str` (issue #66) and this map with it.
_EXT = {
    "matpower": ".m",
    "m": ".m",
    "powermodels-json": ".json",
    "powermodels": ".json",
    "pm": ".json",
    "egret-json": ".json",
    "egret": ".json",
    "psse": ".raw",
    "raw": ".raw",
    "powerworld": ".aux",
    "aux": ".aux",
}


def _unlink_quietly(path: str) -> None:
    """Remove ``path``, ignoring a missing or locked file. Cleanup runs next to
    an in-flight exception (a failed write, a conversion error), so it must
    never raise and mask the error the caller actually cares about."""
    try:
        os.unlink(path)
    except OSError:
        pass


def _stage(content: str, fmt: str) -> str:
    """Write ``content`` to a temp file whose extension matches ``fmt``.

    Returns the path; the caller is responsible for deleting it. Writes UTF-8
    regardless of the platform's default text encoding, because the case
    readers decode as UTF-8 (a non-UTF-8 locale would otherwise corrupt
    non-ASCII content or fail the parse). If the write fails, the temp file
    `mkstemp` already created on disk is removed before re-raising — the caller
    only learns the path on success, so it can't clean up after a failed stage.
    """
    suffix = _EXT.get(fmt.strip().lower(), ".txt")
    fd, path = tempfile.mkstemp(suffix=suffix)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            fh.write(content)
    except Exception:
        _unlink_quietly(path)
        raise
    return path


def _one_input(path: Optional[str], content: Optional[str]) -> None:
    if (path is None) == (content is None):
        raise ValueError("provide exactly one of `path` or `content`")


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
    ``powermodels-json`` (``pm``), ``egret-json`` (``egret``), ``psse``
    (``raw``), ``powerworld`` (``aux``). The input format is inferred from the
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
            conv = powerio.convert(path, to, from_)
        else:
            tmp = _stage(content, from_)
            try:
                conv = powerio.convert(tmp, to, from_)
            finally:
                _unlink_quietly(tmp)
    except powerio.PowerIOError as exc:
        raise ValueError(f"conversion failed: {exc}") from exc
    except FileNotFoundError as exc:
        raise ValueError(f"file not found: {exc}") from exc
    return {"text": conv.text, "warnings": list(conv.warnings)}


@mcp.tool()
def case_summary(
    path: Optional[str] = None,
    content: Optional[str] = None,
    format: str = "matpower",
) -> dict:
    """Summarize a power system case: name, base MVA, source format, element
    counts, and connectivity.

    Provide exactly one of ``path`` or ``content``. For inline ``content``,
    ``format`` names the input format (default ``matpower``). Pulls in no
    scipy/numpy.
    """
    _one_input(path, content)
    try:
        case = (
            powerio.parse(path) if path is not None else powerio.parse_str(content, format)
        )
    except powerio.PowerIOError as exc:
        raise ValueError(f"parse failed: {exc}") from exc
    except FileNotFoundError as exc:
        raise ValueError(f"file not found: {exc}") from exc
    return {
        "name": case.name,
        "base_mva": case.base_mva,
        "source_format": case.source_format,
        "n_buses": case.n,
        "n_branches": case.n_branches,
        "n_gens": case.n_gens,
        "n_loads": case.n_loads,
        "n_shunts": case.n_shunts,
        "is_radial": case.is_radial,
        "n_connected_components": case.n_connected_components,
        "connectivity_report": case.connectivity_report(),
    }


def main() -> None:
    """Console-script entry point: serve the two tools over stdio."""
    mcp.run()
