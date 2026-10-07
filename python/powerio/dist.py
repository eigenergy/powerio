"""Multiconductor distribution network values.

The typed model uses wire coordinates. Supported formats are OpenDSS ``.dss``,
PowerModelsDistribution ENGINEERING JSON (``pmd-json``), and BMOPF JSON
(``bmopf-json``), and the explicit ``sincal-multiconductor`` profile. Same format emissions can return retained source bytes. Cross
format emissions report unsupported fields as diagnostics.

    import powerio

    module = powerio.parse("feeder.dss")
    net = module.value
    for diagnostic in module.diagnostics:
        print("parse:", diagnostic)
    conv = powerio.emit(module, "pmd-json")
"""

from __future__ import annotations

import json as _json
from dataclasses import dataclass
from typing import Any, Optional

from ._guard import guard_class as _guard_class

__all__ = ["MulticonductorNetwork", "SincalReadOptions"]


@dataclass(frozen=True)
class SincalReadOptions:
    """Explicit conductor-resolved SINCAL selection for :func:`powerio.parse`.

    ``variant`` is a native variant ID. ``snapshot_hours`` selects an explicit
    daily snapshot. ``acquired_tables`` is a relative companion name for the
    optional Access acquisition helper's output; the primary remains the MDB.
    The original MDB length and hash must match the acquisition metadata.
    No external acquisition tool is run by parsing.
    """

    variant: Optional[int] = None
    snapshot_hours: Optional[float] = None
    acquired_tables: Optional[str] = None

    def __post_init__(self):
        if self.variant is not None and (
            isinstance(self.variant, bool) or not isinstance(self.variant, int)
        ):
            raise TypeError("variant must be an integer or None")
        if self.snapshot_hours is not None and (
            isinstance(self.snapshot_hours, bool)
            or not isinstance(self.snapshot_hours, (int, float))
        ):
            raise TypeError("snapshot_hours must be a number or None")
        if self.acquired_tables is not None and not isinstance(self.acquired_tables, str):
            raise TypeError("acquired_tables must be a relative companion name or None")



@_guard_class
class MulticonductorNetwork:
    """A parsed multiconductor distribution network in wire coordinates.

    Buses carry named terminals, lines carry conductor impedance matrices, and
    transformers carry per winding connections. This type is distinct from the
    positive sequence :class:`powerio.BalancedNetwork`; balanced matrix calculations do not
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
        """Native format identity: ``dss``, ``pmd-json``, ``bmopf-json``, or ``sincal``."""
        return self._inner.source_format()

    @property
    def base_frequency(self) -> float:
        """System base frequency in hertz."""
        return self._inner.base_frequency()

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
    def n_voltage_sources(self) -> int:
        """Number of grid forming voltage sources."""
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
    def untyped_objects(self) -> "list[dict[str, Any]]":
        return self._inner.untyped_objects()

    def to_graph(self) -> Any:
        """Transform the network to collapsed bus and terminal graph data."""
        return _json.loads(self._inner.graph_json())

    def to_geo_layer(self) -> Any:
        """Transform coordinates to a canonical GeoJSON FeatureCollection.

        A network without coordinates produces an empty feature collection.
        """
        return _json.loads(self._inner.to_geo_layer_json())

    def apply_geo_layer(
        self, text: str, name_hint: Optional[str] = None
    ) -> tuple["MulticonductorNetwork", Any]:
        """Apply a geographic sidecar and return ``(placed, report)``.

        ``text`` is any form :func:`powerio.parse_geo` accepts. This network
        is unchanged; the placed copy drops the retained source text, so a
        same-format emission re-serializes.
        """
        inner, report = self._inner.apply_geo_layer(text, name_hint)
        return MulticonductorNetwork(inner), report

    def __repr__(self) -> str:
        return self._inner.__repr__()
