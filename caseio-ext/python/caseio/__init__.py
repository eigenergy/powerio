"""caseio: fast, lossless power-system case-file parsing and conversion.

No numpy/scipy. Parse, write, and convert return plain dicts and strings::

    import caseio

    case = caseio.parse("case9.m")
    print(case.n, case.base_mva)         # 9 100.0
    text = case.write()                  # byte-exact MATPOWER echo
    raw, warnings = caseio.convert("case9.m", "psse")

The compiled core (``caseio._caseio``) does the parsing/conversion in Rust.
The matrix builders (B', Y_bus, PTDF, DC-OPF, …) live in the separate
``casemat`` package, which pulls in scipy; this package deliberately does not.
"""

from __future__ import annotations

from collections import namedtuple
from typing import Optional

from . import _caseio
from ._caseio import CaseioError, PyCase as Case, __version__

__all__ = [
    "Case",
    "Conversion",
    "CaseioError",
    "parse",
    "parse_string",
    "write",
    "convert",
    "__version__",
]

Conversion = namedtuple("Conversion", ["text", "warnings"])
Conversion.__doc__ = """Output of :func:`convert`.

``text`` is the converted file contents; ``warnings`` lists the fields the
target format could not represent (empty for a faithful conversion).
"""


def parse(path) -> Case:
    """Parse a MATPOWER ``.m`` file from a path into a :class:`Case`."""
    return _caseio.parse(str(path))


def parse_string(text: str, name: Optional[str] = None) -> Case:
    """Parse a MATPOWER case from in-memory ``.m`` text."""
    return _caseio.parse_string(text, name)


def write(case: Case) -> str:
    """Serialize ``case`` to MATPOWER ``.m`` (byte-exact echo when it was
    parsed from MATPOWER)."""
    return _caseio.write(case)


def convert(path, to: str, from_: Optional[str] = None) -> Conversion:
    """Convert a case file to another format through the neutral hub.

    ``to``/``from_`` are format names (``matpower``, ``powermodels``, ``psse``,
    ``powerworld``, ``egret``); the source format is inferred from the file
    extension when ``from_`` is omitted.
    """
    text, warnings = _caseio.convert(str(path), to, from_)
    return Conversion(text, warnings)
