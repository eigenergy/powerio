"""Parse and convert multiconductor distribution networks.

The typed model uses wire coordinates. Supported formats are OpenDSS ``.dss``,
PowerModelsDistribution ENGINEERING JSON (``pmd-json``), and BMOPF JSON
(``bmopf-json``). Same format writes can return retained source bytes. Cross
format writes report unsupported fields in :class:`~powerio.Conversion`.

    import powerio.dist as dist

    net = dist.parse_file("feeder.dss")
    for w in net.warnings:
        print("parse:", w)
    conv = net.to_format("pmd-json")
"""

from __future__ import annotations

import json as _json
from typing import Any, Optional

from . import Conversion, _powerio

__all__ = [
    "MulticonductorNetwork",
    "convert_file",
    "convert_str",
    "parse_file",
    "parse_str",
]


class MulticonductorNetwork:
    """A parsed multiconductor distribution network in wire coordinates.

    Buses carry named terminals, lines carry conductor impedance matrices, and
    transformers carry per winding connections. This type is distinct from the
    positive sequence :class:`powerio.BalancedNetwork`; balanced matrix builders do not
    accept it.
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
    def base_frequency(self) -> float:
        """System base frequency in hertz."""
        return self._inner.base_frequency()

    @property
    def warnings(self) -> "list[str]":
        """Return source fields not represented and assumptions made while parsing."""
        return self._inner.warnings()

    def diagnostics(self) -> Any:
        """The same findings as ``warnings``, structured: a list of dicts
        carrying ``code``, ``severity``, ``message``, and ``target``."""
        return _json.loads(self._inner.diagnostics_json())

    @property
    def n_buses(self) -> int:
        return self._inner.n_buses()

    @property
    def n_lines(self) -> int:
        return self._inner.n_lines()

    @property
    def n_line_codes(self) -> int:
        return self._inner.n_line_codes()

    @property
    def n_switches(self) -> int:
        return self._inner.n_switches()

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
    def n_ibrs(self) -> int:
        return self._inner.n_ibrs()

    @property
    def n_control_profiles(self) -> int:
        return self._inner.n_control_profiles()

    @property
    def n_shunts(self) -> int:
        return self._inner.n_shunts()

    @property
    def n_capacitors(self) -> int:
        return self._inner.n_capacitors()

    @property
    def n_sources(self) -> int:
        return self._inner.n_sources()

    @property
    def n_voltage_sources(self) -> int:
        """Number of grid forming voltage sources.

        ``n_sources`` remains as a compatibility spelling.
        """
        return self._inner.n_voltage_sources()

    @property
    def n_untyped_objects(self) -> int:
        return self._inner.n_untyped_objects()

    # These properties are copies of the native model tables. Nested field
    # names come from the Rust model's serialization, so this wrapper does not
    # maintain a second distribution schema.

    @property
    def buses(self) -> "list[dict[str, Any]]":
        return self._inner.buses()

    @property
    def line_codes(self) -> "list[dict[str, Any]]":
        return self._inner.line_codes()

    @property
    def linecodes(self) -> "list[dict[str, Any]]":
        """Compatibility spelling for :attr:`line_codes`."""
        return self.line_codes

    @property
    def lines(self) -> "list[dict[str, Any]]":
        return self._inner.lines()

    @property
    def switches(self) -> "list[dict[str, Any]]":
        return self._inner.switches()

    @property
    def transformers(self) -> "list[dict[str, Any]]":
        return self._inner.transformers()

    @property
    def loads(self) -> "list[dict[str, Any]]":
        return self._inner.loads()

    @property
    def generators(self) -> "list[dict[str, Any]]":
        return self._inner.generators()

    @property
    def ibrs(self) -> "list[dict[str, Any]]":
        return self._inner.ibrs()

    @property
    def control_profiles(self) -> "list[dict[str, Any]]":
        return self._inner.control_profiles()

    @property
    def shunts(self) -> "list[dict[str, Any]]":
        return self._inner.shunts()

    @property
    def capacitors(self) -> "list[dict[str, Any]]":
        return self._inner.capacitors()

    @property
    def voltage_sources(self) -> "list[dict[str, Any]]":
        return self._inner.voltage_sources()

    @property
    def sources(self) -> "list[dict[str, Any]]":
        """Compatibility spelling for :attr:`voltage_sources`."""
        return self.voltage_sources

    @property
    def untyped_objects(self) -> "list[dict[str, Any]]":
        return self._inner.untyped_objects()

    @property
    def untyped(self) -> "list[dict[str, Any]]":
        """Compatibility spelling for :attr:`untyped_objects`."""
        return self.untyped_objects

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

    def write_file(self, path: Any, to: str) -> list[str]:
        """Serialize to ``to`` and write it to ``path`` byte exact.

        Any sidecar the writer produces goes beside ``path``: a dss write of a
        network with bus coordinates emits a ``Buscoords`` directive, and the
        CSV it names is written too. Returns the fidelity warnings. See
        :meth:`powerio.BalancedNetwork.write_file` for why this beats writing
        :meth:`to_format` text through ``open(path, "w")`` on Windows.
        """
        return self._inner.write_file(str(path), to)

    def graph(self) -> Any:
        """Compatibility alias for :meth:`to_graph`."""
        return self.to_graph()

    def to_graph(self) -> Any:
        """Transform the network to collapsed bus and terminal graph data."""
        return _json.loads(self._inner.graph_json())

    def geo_layer(self) -> Any:
        """This case's coordinates as a canonical GeoJSON FeatureCollection.

        Raises when the case carries none.
        """
        return _json.loads(self._inner.geo_layer_json())

    def apply_geo_layer(
        self, text: str, name_hint: Optional[str] = None
    ) -> tuple["MulticonductorNetwork", Any]:
        """Apply a geographic sidecar and return ``(placed, report)``.

        ``text`` is any form :func:`powerio.parse_geo` accepts. This network
        is unchanged; the placed copy drops the retained source text, so a
        same-format write re-serializes.
        """
        inner, report = self._inner.apply_geo_layer(text, name_hint)
        return MulticonductorNetwork(inner), report

    def __repr__(self) -> str:
        return self._inner.__repr__()




def parse_file(
    path: Any, from_: Optional[str] = None, include_root: Any = None
) -> MulticonductorNetwork:
    """Parse a distribution network file.

    The format comes from ``from_`` when given, else from the file itself:
    ``.dss`` is OpenDSS, and ``.json`` holding the ENGINEERING ``data_model``
    key is PMD JSON, otherwise BMOPF JSON.

    ``include_root`` widens dss include confinement from the case directory to
    the given directory: the case file must sit under it, and
    ``Redirect``/``Compile``/``Buscoords`` includes resolve anywhere beneath
    it. Unset, includes stay confined to the case directory.
    """
    return MulticonductorNetwork(
        _powerio.dist_parse_file(
            str(path),
            from_,
            str(include_root) if include_root is not None else None,
        )
    )


def parse_str(text: str, from_: str) -> MulticonductorNetwork:
    """Parse an in-memory distribution network of the named source format ``from_``."""
    return MulticonductorNetwork(_powerio.dist_parse_str(text, from_))


def convert_file(path: Any, to: str, from_: Optional[str] = None) -> Conversion:
    """Convert a distribution network file to ``to`` in one call.

    The warnings carry both the parse warnings and the writer's fidelity
    losses (there is no :class:`MulticonductorNetwork` to query them from).
    """
    text, warnings = _powerio.dist_convert_file(str(path), to, from_)
    return Conversion(text, warnings)


def convert_str(text: str, to: str, from_: str) -> Conversion:
    """Convert an in-memory distribution network of the named source format ``from_`` to ``to``.

    The signature matches :func:`powerio.convert_str`: input, target, source,
    except ``from_`` is required (there is no extension to infer from and no
    default). The warnings carry both the parse warnings and the writer's
    fidelity losses (there is no :class:`MulticonductorNetwork` to query them from).
    """
    text, warnings = _powerio.dist_convert_str(text, to, from_)
    return Conversion(text, warnings)
