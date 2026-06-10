"""A FastMCP server exposing powerio's lossless converter and case summary.

Two tools for LLM agent tooling, both accepting either a filesystem ``path`` or
inline ``content``:

- ``convert_case``: convert a case file between formats, returning the text and
  any fidelity warnings.
- ``case_summary``: counts, base MVA, source format, and connectivity, with no
  scipy/numpy in the loop.
- ``generate_case``: a synthetic tree/lattice/pegase-like case as the JSON
  transport, deterministic per seed.

Run over stdio with the ``powerio-mcp`` console script (or ``python -m
powerio.mcp``). The server reuses ``powerio.convert_file``/``convert_str``/
``parse_file``/``parse_str``; it never reimplements parsing, and inline
content converts in memory with no temp file staging.
"""

from __future__ import annotations

from typing import Optional

import powerio
from mcp.server.fastmcp import FastMCP

mcp = FastMCP("powerio")


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
            conv = powerio.convert_file(path, to, from_)
        else:
            conv = powerio.convert_str(content, to, from_)
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
            powerio.parse_file(path)
            if path is not None
            else powerio.parse_str(content, format)
        )
    except powerio.PowerIOError as exc:
        raise ValueError(f"parse failed: {exc}") from exc
    except FileNotFoundError as exc:
        raise ValueError(f"file not found: {exc}") from exc
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
    }


@mcp.tool()
def generate_case(
    topology: str = "tree",
    n: int = 64,
    r_over_x: float = 0.1,
    mean_x: float = 0.05,
    seed: int = 0x00C0_FFEE,
) -> dict:
    """Generate a synthetic power system case and return its JSON transport
    plus a summary.

    ``topology`` is ``tree`` (radial spanning tree), ``lattice`` (2-D grid;
    ``n`` rounds up to a perfect square), or ``pegase-like`` (tree plus ~30%
    extra edges, transmission-like meshing). ``n`` below 2 is raised to 2
    (lattice: at least a 2×2 grid). Identical arguments (including ``seed``)
    generate the identical case; bus 1 is the reference. The case carries
    buses and branches only — no loads, shunts, or generators.

    The returned ``json`` is the same transport ``Network.to_json`` emits, so
    any tool that accepts a parsed case accepts it.
    """
    case = powerio.generate_case(topology, n, r_over_x, mean_x, seed)
    return {
        "json": case.to_json(),
        "summary": {
            "name": case.name,
            "base_mva": case.base_mva,
            "n_buses": case.n_buses,
            "n_branches": case.n_branches,
            "is_radial": case.is_radial,
            "n_connected_components": case.n_connected_components,
        },
    }


def main() -> None:
    """Console-script entry point: serve the tools over stdio."""
    mcp.run()
