"""Multiconductor distribution networks in wire coordinates.

Three formats, lossless three way conversion: OpenDSS ``.dss``,
PowerModelsDistribution ENGINEERING JSON (``pmd-json``), and the draft BMOPF
task force JSON (``bmopf-json``). The fidelity rules match the
transmission surface: writing back to the source format echoes the retained
source text byte for byte, and every cross format write reports each loss in
the :class:`~powerio.Conversion` warnings instead of dropping it silently.

    import powerio.dist as dist

    net = dist.parse_file("feeder.dss")
    for w in net.warnings:
        print("parse:", w)
    conv = net.to_format("pmd-json")
"""

from __future__ import annotations

from typing import Any, Optional

from . import Conversion, _powerio

__all__ = [
    "DistNetwork",
    "MulticonductorNetwork",
    "parse_file",
    "parse_str",
    "convert_file",
    "convert_str",
]


class DistNetwork:
    """A parsed multiconductor distribution network.

    Buses carry named terminals, lines carry conductor impedance matrices, and
    transformers carry per winding connections; nothing is collapsed to
    positive sequence. Distinct from :class:`powerio.Network` (the
    transmission model); the matrix builders do not accept it.
    """

    def __init__(self, inner) -> None:
        self._inner = inner

    @property
    def name(self) -> Optional[str]:
        """Distribution network name when the source format carries one."""
        return self._inner.name()

    @property
    def source_format(self) -> Optional[str]:
        """Format parsed from: ``dss``, ``pmd-json``, or ``bmopf-json``."""
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

    @property
    def n_sources(self) -> int:
        return self._inner.n_sources()

    def to_format(self, to: str) -> Conversion:
        """Serialize to ``to`` (``dss``, ``pmd-json``, ``bmopf-json``).

        Writing back to the source format echoes the retained source text byte
        for byte; a cross format write regenerates from the typed model and
        reports every fidelity loss in the warnings.
        """
        text, warnings = self._inner.to_format(to)
        return Conversion(text, warnings)

    def to_canonical_format(self, to: str) -> Conversion:
        """Serialize to ``to`` from the typed model, bypassing source echo."""
        text, warnings = self._inner.to_canonical_format(to)
        return Conversion(text, warnings)

    def __repr__(self) -> str:
        return self._inner.__repr__()


# v1 name for the wire coordinate distribution model. ``DistNetwork`` remains
# available in 0.4 because it is the existing handle name.
MulticonductorNetwork = DistNetwork


def parse_file(path: Any, from_: Optional[str] = None) -> MulticonductorNetwork:
    """Parse a distribution network file.

    The format comes from ``from_`` when given, else from the file itself:
    ``.dss`` is OpenDSS, and ``.json`` holding the ENGINEERING ``data_model``
    key is PMD JSON, otherwise BMOPF JSON.
    """
    return DistNetwork(_powerio.dist_parse_file(str(path), from_))


def parse_str(text: str, format: str) -> MulticonductorNetwork:
    """Parse an in-memory distribution network of the named ``format``."""
    return DistNetwork(_powerio.dist_parse_str(text, format))


def convert_file(path: Any, to: str, from_: Optional[str] = None) -> Conversion:
    """Convert a distribution network file to ``to`` in one call.

    The warnings carry both the parse warnings and the writer's fidelity
    losses (there is no :class:`DistNetwork` to query them from).
    """
    text, warnings = _powerio.dist_convert_file(str(path), to, from_)
    return Conversion(text, warnings)


def convert_str(text: str, to: str, format: str) -> Conversion:
    """Convert an in-memory distribution network of the named ``format`` to ``to``.

    The signature matches :func:`powerio.convert_str`: input, target, source,
    except ``format`` is required (there is no extension to infer from and no
    default). The warnings carry both the parse warnings and the writer's
    fidelity losses (there is no :class:`DistNetwork` to query them from).
    """
    text, warnings = _powerio.dist_convert_str(text, to, format)
    return Conversion(text, warnings)
