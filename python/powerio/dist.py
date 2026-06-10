"""Multiconductor distribution cases in wire coordinates.

Three formats, lossless three way conversion: OpenDSS ``.dss``,
PowerModelsDistribution ENGINEERING JSON (``pmd-json``), and the draft BMOPF
task force JSON (``bmopf-json``). The fidelity contract matches the
transmission surface: writing back to the source format echoes the retained
source text byte for byte, and every cross format write reports each loss in
the :class:`~powerio.Conversion` warnings instead of dropping it silently.

    import powerio.dist as dist

    case = dist.parse_file("feeder.dss")
    for w in case.warnings:
        print("parse:", w)
    conv = case.to_format("pmd-json")
"""

from __future__ import annotations

from typing import Any, Optional

from . import Conversion, _powerio

__all__ = [
    "DistCase",
    "parse_file",
    "parse_str",
    "convert_file",
    "convert_str",
]


class DistCase:
    """A parsed multiconductor distribution case.

    Buses carry named terminals, lines carry conductor impedance matrices, and
    transformers carry per winding connections; nothing is collapsed to
    positive sequence. Distinct from :class:`powerio.Network` (the
    transmission model); the matrix builders do not accept it.
    """

    def __init__(self, inner) -> None:
        self._inner = inner

    @property
    def source_format(self) -> Optional[str]:
        """Format the case was parsed from: ``dss``, ``pmd-json``, or ``bmopf-json``."""
        return self._inner.source_format()

    @property
    def warnings(self) -> "list[str]":
        """Parse warnings: everything the reader could not represent or had to assume."""
        return self._inner.warnings()

    @property
    def n_buses(self) -> int:
        return self._inner.n_buses()

    @property
    def n_lines(self) -> int:
        return self._inner.n_lines()

    @property
    def n_transformers(self) -> int:
        return self._inner.n_transformers()

    @property
    def n_loads(self) -> int:
        return self._inner.n_loads()

    @property
    def n_generators(self) -> int:
        return self._inner.n_generators()

    def to_format(self, to: str) -> Conversion:
        """Serialize to ``to`` (``dss``, ``pmd-json``, ``bmopf-json``).

        Writing back to the source format echoes the retained source text byte
        for byte; a cross format write regenerates from the typed model and
        reports every fidelity loss in the warnings.
        """
        text, warnings = self._inner.to_format(to)
        return Conversion(text, warnings)

    def __repr__(self) -> str:
        return self._inner.__repr__()


def parse_file(path: Any, from_: Optional[str] = None) -> DistCase:
    """Parse a distribution case file.

    The format comes from ``from_`` when given, else from the file itself:
    ``.dss`` is OpenDSS, and ``.json`` holding the ENGINEERING ``data_model``
    key is PMD JSON, otherwise BMOPF JSON.
    """
    return DistCase(_powerio.dist_parse_file(str(path), from_))


def parse_str(text: str, format: str) -> DistCase:
    """Parse an in-memory distribution case of the named ``format``."""
    return DistCase(_powerio.dist_parse_str(text, format))


def convert_file(path: Any, to: str, from_: Optional[str] = None) -> Conversion:
    """Convert a distribution case file to ``to`` in one call.

    The warnings carry both the parse warnings and the writer's fidelity
    losses (there is no :class:`DistCase` to query them from).
    """
    text, warnings = _powerio.dist_convert_file(str(path), to, from_)
    return Conversion(text, warnings)


def convert_str(text: str, from_: str, to: str) -> Conversion:
    """Convert an in-memory distribution case from ``from_`` to ``to`` in one call.

    The warnings carry both the parse warnings and the writer's fidelity
    losses (there is no :class:`DistCase` to query them from).
    """
    text, warnings = _powerio.dist_convert_str(text, from_, to)
    return Conversion(text, warnings)
