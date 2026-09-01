"""Parse, transform, and emit power system data.

``parse`` returns a module whose ``value`` is the typed power system object
and whose ``diagnostics`` record what the parser found::

    import powerio as pio

    module = pio.parse("case9.m")
    net = module.value
    print(net.n_buses, net.base_mva)         # 9 100.0
    matpower = pio.emit(module, "matpower")
    emitted = pio.emit(module, "psse", "case9.raw")

    B = net.calc_bprime_matrix()             # scipy.sparse, MATPOWER Bp
    Y = net.calc_admittance_matrix()         # complex csr, G + jB
    G = net.to_networkx()                    # networkx.Graph keyed by bus id

PyPSA CSV folders carry static network topology. NetCDF and HDF5 time series
are tracked in https://github.com/eigenergy/powerio/issues/107.

A source that defines a calculation parses to that calculation's typed value.
Use ``isinstance(module.value, ...)`` to branch on the result type.

``import powerio`` and the base parse and emit paths require no
third party Python package. Matrix methods require SciPy and NumPy. Graph
methods require NetworkX. Install them with ``powerio[matrix]``,
``powerio[graph]``, or ``powerio[all]``. Missing extras raise ``ImportError``.
"""

from __future__ import annotations

import importlib
import io as _io
import json as _json
import operator as _operator
import os as _os
from collections import namedtuple
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from typing import Any, Iterable, Optional, Union

from . import _powerio
from ._powerio import (
    ActivePower,
    ApparentPower,
    CalculationUpdate,
    ComponentId,
    Diagnostic,
    NetworkUpdate,
    OperatingPointUpdate,
    PowerIODataError,
    PowerIOError,
    PowerIOParseError,
    ReactivePower,
    Residuals,
    ScucActiveReserveZone,
    ScucBranchSwitchingCost,
    ScucContingency,
    ScucDevice,
    ScucDeviceOutputs,
    ScucDevicePeriod,
    ScucEnergyCostBlock,
    ScucEnergyRequirement,
    ScucInitialCommitment,
    ScucInputs,
    ScucNetworkOutputs,
    ScucRampLimits,
    ScucReactiveCapability,
    ScucReactiveReserveZone,
    ScucReserveCosts,
    ScucReserveLimits,
    ScucShunt,
    ScucStartupCostAdjustment,
    ScucStartupLimit,
    ScucTransformerControl,
    ScucViolationCosts,
    SourceSpan,
    UpdateChange,
    UpdateReport,
    __version__,
)

__all__ = [
    "AcOpfInstance",
    "AcOpfSolution",
    "AcPfInstance",
    "AcPfSolution",
    "AcScucInstance",
    "AcScucSolution",
    "ActivePower",
    "ApparentPower",
    "Artifact",
    "BalancedNetwork",
    "CalculationUpdate",
    "ComponentId",
    "DcOpfInstance",
    "DcOpfSolution",
    "DcPfInstance",
    "DcPfSolution",
    "Diagnostic",
    "DisplayData",
    "EmitResult",
    "FormatInfo",
    "McAcOpfInstance",
    "McAcOpfSolution",
    "McAcPfInstance",
    "McAcPfSolution",
    "MulticonductorNetwork",
    "NetworkUpdate",
    "OperatingPoint",
    "OperatingPointUpdate",
    "PioModule",
    "PowerIODataError",
    "PowerIOError",
    "PowerIOParseError",
    "PwdDisplay",
    "PwdSubstation",
    "ReactivePower",
    "Residuals",
    "ScenarioSet",
    "Scenario",
    "ScucActiveReserveZone",
    "ScucBranchSwitchingCost",
    "ScucContingency",
    "ScucDevice",
    "ScucDeviceOutputs",
    "ScucDevicePeriod",
    "ScucEnergyCostBlock",
    "ScucEnergyRequirement",
    "ScucInitialCommitment",
    "ScucInputs",
    "ScucNetworkOutputs",
    "ScucRampLimits",
    "ScucReactiveCapability",
    "ScucReactiveReserveZone",
    "ScucReserveCosts",
    "ScucReserveLimits",
    "ScucShunt",
    "ScucStartupCostAdjustment",
    "ScucStartupLimit",
    "ScucTransformerControl",
    "ScucViolationCosts",
    "SocwrOpfSolution",
    "SourceSpan",
    "TimeSeries",
    "TimePoint",
    "UpdateChange",
    "UpdateReport",
    "__version__",
    "deserialize",
    "dist",
    "emit",
    "apply_bus_load_active_power",
    "apply_updates",
    "features",
    "from_ppc",
    "parse",
    "parse_display",
    "parse_geo",
    "resolve_format",
    "serialize",
    "versions",
]

@dataclass(frozen=True)
class Artifact:
    """One artifact produced by :func:`emit` or :func:`serialize`.

    ``data`` is set for an in-memory result. ``path`` is set after committing
    to a filesystem destination.
    """

    name: str
    data: Optional[bytes]
    path: Optional[str]

    @property
    def text(self) -> str:
        """Decode an in-memory UTF-8 artifact."""
        if self.data is None:
            raise ValueError("this artifact was committed to a destination")
        return self.data.decode("utf-8")


@dataclass(frozen=True)
class EmitResult:
    """Artifact inventory and diagnostics from an emission or serialization."""

    artifacts: tuple[Artifact, ...]
    layout: str
    fidelity: str
    diagnostics: tuple[Diagnostic, ...]

    @property
    def text(self) -> Optional[str]:
        """The sole UTF-8 memory artifact, or ``None`` for other inventories."""
        if len(self.artifacts) != 1 or self.artifacts[0].data is None:
            return None
        return self.artifacts[0].text

FormatInfo = namedtuple(
    "FormatInfo", ["token", "extension", "is_directory", "can_emit"]
)
FormatInfo.__doc__ = """Canonical metadata returned by :func:`resolve_format`.

``extension`` is the conventional filename suffix without a leading dot; it
can be compound and is ``None`` when a directory format has no primary case
file. ``can_emit`` reports whether a fresh universal emitter exists for the
format. It is not a promise for every concrete module value or a feature probe. A
false value neither promises nor forbids a same format retained source echo.
"""

DisplayData = namedtuple("DisplayData", ["kind", "data"])
DisplayData.__doc__ = """Output of :func:`parse_display`.

``kind`` names the display format. For PowerWorld PWD data,
``kind == "powerworld"`` and
``data`` is a :class:`PwdDisplay`.
"""

PwdDisplay = namedtuple(
    "PwdDisplay", ["canvas_width", "canvas_height", "stamp", "substations"]
)
PwdDisplay.__doc__ = """Decoded PowerWorld ``.pwd`` display metadata."""

PwdSubstation = namedtuple("PwdSubstation", ["number", "name", "x", "y"])
PwdSubstation.__doc__ = """One decoded PowerWorld display substation."""

def _require(module: str, extra: str):
    """Import ``module`` or raise a clear ImportError naming the extra to install."""
    try:
        return importlib.import_module(module)
    except ImportError as exc:
        # Only rewrite "module is absent". A present-but-broken install (e.g. a
        # failed C-extension load) raises ImportError from a sub-import; let its
        # own traceback through instead of misdirecting the user to reinstall.
        if getattr(exc, "name", None) not in (module, module.split(".")[0]):
            raise
        raise ImportError(
            f"powerio needs {module!r} for this call; install it with "
            f"`pip install 'powerio[{extra}]'`"
        ) from exc


def _to_csr(coo):
    """Assemble a ``(data, row, col, shape)`` COO tuple into a csr_matrix."""
    sparse = _require("scipy.sparse", "matrix")
    data, row, col, shape = coo
    return sparse.coo_matrix((data, (row, col)), shape=shape).tocsr()


def _dc_angles(n_buses: int, voltage_angles):
    np = _require("numpy", "matrix")
    angles = np.asarray(voltage_angles, dtype=float)
    if angles.ndim != 1 or angles.shape[0] != n_buses:
        raise ValueError(
            f"voltage_angles must be a one dimensional array of length {n_buses}"
        )
    return np, angles


def _wrap_display(raw) -> DisplayData:
    kind, payload = raw
    if kind == "powerworld":
        substations = [
            PwdSubstation(
                row["number"],
                row["name"],
                row["x"],
                row["y"],
            )
            for row in payload["substations"]
        ]
        payload = PwdDisplay(
            payload["canvas_width"],
            payload["canvas_height"],
            payload["stamp"],
            substations,
        )
    return DisplayData(kind, payload)


_BALANCED_DELEGATED_NAMES = frozenset(
    {
        "areas",
        "base_frequency",
        "base_mva",
        "branches",
        "buses",
        "detailed_connectivity",
        "generators",
        "hvdc",
        "is_radial",
        "loads",
        "n_areas",
        "n_branches",
        "n_buses",
        "n_generators",
        "n_hvdc",
        "n_islands",
        "n_loads",
        "n_shunts",
        "n_static_var_compensators",
        "n_storage",
        "n_switches",
        "n_transformers_3w",
        "name",
        "reference_bus_index",
        "reference_bus_indices",
        "shunts",
        "static_var_compensators",
        "source_format",
        "storage",
        "switches",
        "transformers_3w",
    }
)


class BalancedNetwork:
    """A parsed balanced power network.

    The data attributes (``buses``, ``branches``, ``generators``, ``loads``,
    ``shunts``) and reference bus queries delegate to the compiled handle; the
    matrix methods below return ``scipy.sparse`` objects. Parse and transform
    diagnostics belong to the owning :class:`PioModule`.

    Errors: a bad file path raises the standard ``OSError`` subclass
    (``FileNotFoundError``); a malformed case raises :class:`PowerIOParseError`
    and an unmet calculation precondition (no generators, no reference bus) raises
    :class:`PowerIODataError`; both subclass :class:`PowerIOError`, so
    ``except PowerIOError`` catches either; an unknown
    ``scheme``/``formula``/``units`` string raises ``ValueError``.
    """

    def __init__(self, inner: "_powerio._BalancedNetwork"):
        self._inner = inner

    def __dir__(self):
        # The data attributes arrive through __getattr__, so name them here or
        # they stay invisible to tab completion.
        return sorted(set(super().__dir__()) | _BALANCED_DELEGATED_NAMES)

    def __getattr__(self, name: str):
        # Reached only when normal lookup misses, so the matrix methods below
        # win. Guard underscore names so a lookup before _inner exists raises
        # AttributeError instead of recursing forever.
        if name not in _BALANCED_DELEGATED_NAMES:
            raise AttributeError(
                f"{type(self).__name__!r} object has no attribute {name!r}"
            )
        return getattr(self._inner, name)

    def __repr__(self) -> str:
        # The inner handle's __repr__ already renders the public ``BalancedNetwork(...)``
        # form, so this is a straight delegate.
        return repr(self._inner)

    def calc_connectivity_report(self) -> dict[str, Any]:
        """Calculate the in-service topology summary."""
        return self._inner.calc_connectivity_report()

    def to_geo_layer(self) -> dict[str, Any]:
        """Transform coordinates to a canonical GeoJSON FeatureCollection.

        A case without coordinates produces an empty feature collection.
        """
        return _json.loads(self._inner.to_geo_layer_json())

    def apply_geo_layer(
        self, text: str, name_hint: Optional[str] = None
    ) -> tuple["BalancedNetwork", dict[str, Any]]:
        """Apply a geographic sidecar and return ``(placed, report)``.

        ``text`` is any form :func:`parse_geo` accepts; this case is
        unchanged. The report carries ``matched_buses``, ``matched_branches``,
        ``unmatched_features``, ``unlocated_buses``, ``unlocated_branches``,
        and ``notes``. The two unlocated counts cover the whole case when the
        pass ends, so a layer that matched nothing reads apart from a case
        that needed nothing. The placed copy drops the retained source text,
        so a same-format emission re-serializes.
        """
        inner, report = self._inner.apply_geo_layer(text, name_hint)
        return BalancedNetwork(inner), report

    # --- matrix calculations (scipy.sparse) -----------------------------

    def calc_bprime_matrix(
        self, scheme: str = "bx", *, skip_zero_impedance: bool = False
    ):
        """MATPOWER FDPF Bp matrix. ``scheme`` is ``"bx"`` or ``"xb"``.

        ``skip_zero_impedance=False`` refuses a zero impedance branch
        (``r`` and ``x`` both zero); pass ``True`` to drop it instead.
        """
        return _to_csr(
            self._inner.bprime(scheme, skip_zero_impedance=skip_zero_impedance)
        )

    def calc_incidence_matrix(self, formula: str = "series_susceptance"):
        """Return PowerModels incidence ``A`` (branches by buses)."""
        return _to_csr(self._inner.calc_incidence_matrix(formula))

    def calc_branch_susceptances(self, formula: str = "series_susceptance"):
        """Return per branch susceptances in active branch order."""
        np = _require("numpy", "matrix")
        return np.asarray(self._inner.calc_branch_susceptances(formula), dtype=float)

    def calc_branch_flow_matrix(self, formula: str = "series_susceptance"):
        """Return ``Bf = diag(b) A`` as a CSR matrix."""
        return _to_csr(self._inner.calc_branch_flow_matrix(formula))

    def calc_bus_susceptance_matrix(self, formula: str = "series_susceptance"):
        """Return ``B = A.T diag(b) A`` as a CSR matrix."""
        return _to_csr(self._inner.calc_bus_susceptance_matrix(formula))

    def calc_branch_phase_shift_injection(
        self, formula: str = "series_susceptance"
    ):
        """Return ``b * shift`` in active branch order."""
        np = _require("numpy", "matrix")
        return np.asarray(
            self._inner.calc_branch_phase_shift_injection(formula), dtype=float
        )

    def calc_bus_phase_shift_injection(self, formula: str = "series_susceptance"):
        """Return ``A.T @ (b * shift)`` in bus order."""
        np = _require("numpy", "matrix")
        return np.asarray(
            self._inner.calc_bus_phase_shift_injection(formula), dtype=float
        )

    def calc_branch_flow_dc(self, voltage_angles, formula: str = "series_susceptance"):
        """Compute ``-Bf @ va + b * shift`` in active branch order."""
        np, angles = _dc_angles(self.n_buses, voltage_angles)
        return np.asarray(
            self._inner.calc_branch_flow_dc(angles.tolist(), formula), dtype=float
        )

    def calc_bus_injection_dc(
        self, voltage_angles, formula: str = "series_susceptance"
    ):
        """Compute ``-B @ va + p_shift`` in bus order."""
        np, angles = _dc_angles(self.n_buses, voltage_angles)
        return np.asarray(
            self._inner.calc_bus_injection_dc(angles.tolist(), formula), dtype=float
        )

    def calc_bdoubleprime_matrix(
        self, scheme: str = "bx", *, skip_zero_impedance: bool = False
    ):
        """MATPOWER FDPF Bpp matrix. ``scheme`` is ``"bx"`` or ``"xb"``.
        ``skip_zero_impedance`` as in :meth:`calc_bprime_matrix`.
        """
        return _to_csr(
            self._inner.bdoubleprime(scheme, skip_zero_impedance=skip_zero_impedance)
        )

    def calc_lacpf_matrix(
        self,
        *,
        include_taps: bool = True,
        include_shifts: bool = True,
        skip_zero_impedance: bool = False,
    ):
        """LACPF 2n×2n block ``[[G, -B], [-B, -G]]``. ``skip_zero_impedance``
        as in :meth:`calc_bprime_matrix`."""
        return _to_csr(
            self._inner.lacpf(
                include_taps=include_taps,
                include_shifts=include_shifts,
                skip_zero_impedance=skip_zero_impedance,
            )
        )

    def calc_adjacency_matrix(self):
        """0/1 bus adjacency matrix."""
        return _to_csr(self._inner.adjacency())

    def calc_admittance_matrix(
        self,
        *,
        include_taps: bool = True,
        include_shifts: bool = True,
        skip_zero_impedance: bool = False,
    ):
        """``Y_bus = G + jB`` as a complex csr_matrix. ``skip_zero_impedance``
        as in :meth:`calc_bprime_matrix`."""
        g, b = self._inner.ybus_parts(
            include_taps=include_taps,
            include_shifts=include_shifts,
            skip_zero_impedance=skip_zero_impedance,
        )
        g, b = _to_csr(g), _to_csr(b)
        return (g + 1j * b).tocsr()

    def calc_ptdf(self, formula: str = "series_susceptance", solver: str = "auto"):
        """DC PTDF (m×n). ``formula`` is ``"series_susceptance"``,
        ``"tap_adjusted_reactance"``, or ``"reactance_only"``.

        ``solver`` is ``"auto"``, ``"dense"``, or ``"sparse"``. ``"auto"``
        uses the dense factorization on small cases and the sparse Cholesky
        path on large ones, the same policy as the CLI.
        """
        return _to_csr(self._inner.ptdf(formula, solver))

    def calc_lodf(self, formula: str = "series_susceptance", solver: str = "auto"):
        """DC LODF (m×m). ``formula`` and ``solver`` as in :meth:`calc_ptdf`."""
        return _to_csr(self._inner.lodf(formula, solver))

    def calc_weighted_laplacian(
        self,
        formula: str = "series_susceptance",
    ):
        """Weighted Laplacian ``L = -B``. ``formula`` as in :meth:`calc_ptdf`."""
        return _to_csr(self._inner.weighted_laplacian(formula))

    def to_normalized(
        self,
        *,
        clamp_angle_bounds: bool = False,
        angle_bound_pad: Optional[float] = None,
    ) -> "BalancedNetwork":
        """Return a normalized copy with per unit power and radian angles.

        The result removes out of service elements, preserves source bus IDs,
        and normalizes bus types. It carries no retained source, so
        :func:`powerio.emit` produces a grid exchange representation from the
        derived module. Raises
        :class:`PowerIODataError` if the network cannot be
        normalized (no reference bus can be chosen, or a non-positive base MVA).

        ``clamp_angle_bounds=True`` applies the PowerModels angle difference
        bound repair: limits at or beyond ``+/-pi/2`` and zero/zero windows
        become ``[-angle_bound_pad, angle_bound_pad]``. A repair that would
        invert the interval widens to that same window. The default pad is
        1.0472 radians.
        """
        if not clamp_angle_bounds and angle_bound_pad is None:
            return BalancedNetwork(self._inner.to_normalized())
        return BalancedNetwork(
            self._inner.to_normalized_with_options(
                clamp_angle_bounds=clamp_angle_bounds, angle_bound_pad=angle_bound_pad
            )
        )

    def to_ppc(self):
        """PYPOWER case dict (``ppc``) with MATPOWER-style numpy tables.

        Values are emitted as the model holds them, so a case read from a
        file carries MW, MVAr, and degrees. A network from
        :meth:`to_normalized` holds per unit and radians, and those are what
        its tables carry — PYPOWER reads a ppc dict as MW and degrees, so
        build this from the raw network unless the consumer expects per unit.

        Loads and shunts are summed onto their bus in the
        ``PD``/``QD``/``GS``/``BS`` columns, the same aggregation as the
        MATPOWER emitter. The bus table has no per element status
        column, so an element the model marks out of service still
        contributes its value, and a de-energized bus is carried as type 4.
        ``gencost`` is present only when every generator carries cost data,
        because MATPOWER requires cost rows for all generators or none.
        :func:`from_ppc` reads the tables back.
        """
        np = _require("numpy", "matrix")
        buses = self._inner.buses
        bus = np.array(
            [
                (
                    b["id"],
                    _PPC_BUS_TYPE.get(b["kind"], 1.0),
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    b["area"],
                    b["vm"],
                    b["va"],
                    b["base_kv"],
                    b["zone"],
                    b["vmax"],
                    b["vmin"],
                )
                for b in buses
            ],
            dtype=float,
        ).reshape(len(buses), 13)
        bus[:, 2], bus[:, 3], bus[:, 4], bus[:, 5] = _bus_sums(
            np, buses, self._inner.loads, self._inner.shunts
        )

        # The capability and ramp columns past PMIN are an OPF extension that a
        # source need not carry. Widen to the full 21 only when a generator
        # actually states one: a table of zeros there reads back as eleven
        # explicit zero limits, which a ramp aware solver takes as a generator
        # that cannot move.
        gens = self._inner.generators
        caps = [g["caps"] for g in gens]
        width = 21 if any(c is not None for row in caps for c in row) else 10
        gen = np.array(
            [
                [
                    g["bus"],
                    g["pg"],
                    g["qg"],
                    g["qmax"],
                    g["qmin"],
                    g["vg"],
                    g["mbase"],
                    float(g["in_service"]),
                    g["pmax"],
                    g["pmin"],
                ]
                + ([0.0 if c is None else c for c in row] if width == 21 else [])
                for g, row in zip(gens, caps)
            ],
            dtype=float,
        ).reshape(len(gens), width)

        branches = self._inner.branches
        branch = np.array(
            [
                (
                    br["from_id"],
                    br["to_id"],
                    br["r"],
                    br["x"],
                    br["b"],
                    br["rate_a"],
                    br["rate_b"],
                    br["rate_c"],
                    br["tap"],
                    br["shift"],
                    float(br["in_service"]),
                    br["angmin"],
                    br["angmax"],
                )
                for br in branches
            ],
            dtype=float,
        ).reshape(len(branches), 13)

        ppc = {
            "version": "2",
            "baseMVA": float(self._inner.base_mva),
            "bus": bus,
            "gen": gen,
            "branch": branch,
        }

        # Coefficients sit left-aligned after ncost, padded to the widest
        # row, which is the layout PYPOWER's own loadcase produces.
        costs = [g["cost"] for g in gens]
        if costs and all(c is not None for c in costs):
            gencost = np.zeros((len(costs), 4 + max(len(c["coeffs"]) for c in costs)))
            for i, c in enumerate(costs):
                gencost[i, :4] = (
                    c["model"],
                    c["startup"],
                    c["shutdown"],
                    c["ncost"],
                )
                gencost[i, 4 : 4 + len(c["coeffs"])] = c["coeffs"]
            ppc["gencost"] = gencost
        return ppc

    def to_networkx(self):
        """Undirected networkx graph keyed by bus id.

        In-service branches become edges carrying ``branch`` (index), ``r``,
        ``x``, and ``b``.
        """
        nx = _require("networkx", "graph")
        g = nx.Graph()
        g.add_nodes_from(bus["id"] for bus in self._inner.buses)
        for k, br in enumerate(self._inner.branches):
            if br["in_service"]:
                g.add_edge(
                    br["from_id"],
                    br["to_id"],
                    branch=k,
                    r=br["r"],
                    x=br["x"],
                    b=br["b"],
                )
        return g


def parse_display(path: Any, format: Optional[str] = None) -> DisplayData:
    """Parse a display artifact such as a PowerWorld ``.pwd`` file."""
    return _wrap_display(_powerio.parse_display(str(path), format))


def resolve_format(name: str) -> Optional[FormatInfo]:
    """Resolve a format token or common alias to its canonical metadata."""
    resolved = _powerio.resolve_format(name)
    return None if resolved is None else FormatInfo(*resolved)


def parse_geo(text: str, name_hint: Optional[str] = None) -> dict[str, Any]:
    """Tolerantly read a geographic sidecar and return its canonical form.

    Accepts headerless buscoords CSV, aliased CSV/JSON records, and GeoJSON
    Point/LineString features. Returns ``{"geojson": <FeatureCollection dict>,
    "diagnostics": [...]}``; ``name_hint`` (a file name) picks CSV against JSON
    when the content alone is ambiguous. Input with no usable coordinates
    raises :class:`PowerIOParseError`.
    """
    parsed = _powerio.parse_geo(text, name_hint)
    parsed["geojson"] = _json.loads(parsed["geojson"])
    return parsed


# powerio bus kind -> MATPOWER/PYPOWER BUS_TYPE code.
def _bus_sums(np, buses, loads, shunts):
    """Per bus `(pd, qd, gs, bs)` in bus order.

    :meth:`BalancedNetwork.to_ppc` folds the element
    tables onto their bus the way the Rust indexed analysis view does. This is
    that fold, once.
    """
    row_of = {b["id"]: i for i, b in enumerate(buses)}
    pd, qd, gs, bs = (np.zeros(len(buses), dtype=float) for _ in range(4))
    for load in loads:
        i = row_of.get(load["bus"])
        if i is not None:
            pd[i] += load["p"]
            qd[i] += load["q"]
    for shunt in shunts:
        i = row_of.get(shunt["bus"])
        if i is not None:
            gs[i] += shunt["g"]
            bs[i] += shunt["b"]
    return pd, qd, gs, bs


_PPC_BUS_TYPE = {"PQ": 1.0, "PV": 2.0, "REF": 3.0, "ISOLATED": 4.0}

# MATPOWER case-input table widths. PYPOWER result tables append columns
# (LAM_P, MU_*) past these; from_ppc drops them.
_PPC_INPUT_WIDTH = {"bus": 13, "gen": 21, "branch": 13}

# Columns a table must carry, which is what the MATPOWER reader requires. The
# gen table's capability and ramp columns are an OPF extension, so a 10 column
# gen table is a complete case and passes through at its own width; padding it
# would hand the reader eleven explicit zero limits the source never stated. A
# bus or branch row below 13 is truncated data, and zero padding it would
# invent a bus at 0 p.u. and 0 kV, so it is refused here as the reader refuses
# it in a `.m` file.
_PPC_MIN_WIDTH = {"bus": 13, "gen": 10, "branch": 13}


def _ppc_rows(name, table):
    """The table's rows as float lists, trimmed to the MATPOWER input width."""
    width = _PPC_INPUT_WIDTH.get(name)
    minimum = _PPC_MIN_WIDTH.get(name)
    out = []
    for i, row in enumerate(table):
        try:
            vals = [float(v) for v in row]
        except TypeError as e:
            raise ValueError(
                f"ppc table {name!r} row {i} is not a sequence of numbers: "
                f"pass a 2-D array, one row per element"
            ) from e
        except ValueError as e:
            raise ValueError(
                f"ppc table {name!r} row {i} has a non-numeric value: {e}"
            ) from e
        if minimum is not None and len(vals) < minimum:
            raise ValueError(
                f"ppc table {name!r} row {i} has {len(vals)} columns; "
                f"MATPOWER requires at least {minimum}"
            )
        out.append(vals[:width] if width is not None else vals)
    return out


def _ppc_to_matpower_text(ppc) -> str:
    missing = [k for k in ("baseMVA", "bus", "gen", "branch") if k not in ppc]
    if missing:
        raise ValueError(f"ppc dict is missing required keys: {missing}")
    lines = [
        "function mpc = from_ppc",
        f"mpc.version = '{ppc.get('version', '2')}';",
        f"mpc.baseMVA = {float(ppc['baseMVA'])!r};",
    ]
    names = ["bus", "gen", "branch"] + (["gencost"] if "gencost" in ppc else [])
    for name in names:
        rows = _ppc_rows(name, ppc[name])
        lines.append(f"mpc.{name} = [")
        for vals in rows:
            lines.append("  " + "  ".join(repr(v) for v in vals) + ";")
        lines.append("];")
    return "\n".join(lines) + "\n"


def from_ppc(ppc) -> BalancedNetwork:
    """Case from a PYPOWER dict (``ppc``); the inverse of :meth:`BalancedNetwork.to_ppc`.

    The tables route through the MATPOWER reader, so the semantics match a
    ``.m`` case exactly: bus ``PD``/``QD`` become loads, ``GS``/``BS`` become
    shunts, and ``gencost`` is read when present. Result columns past the
    MATPOWER input widths are dropped. A 10 column ``gen`` table (the layout
    without the OPF capability columns) passes through at its own width, so
    the generators come back with no capability limits rather than eleven
    zero ones. Raises :class:`ValueError` when a required table is absent,
    when a ``bus`` or ``branch`` row is below its 13 column width, when a row
    is not a sequence of numbers, or when a cell is not numeric; the message
    names the table and the row.
    """
    value = parse(
        _io.StringIO(_ppc_to_matpower_text(ppc)),
        format="matpower",
        name="from_ppc.m",
    ).value
    assert isinstance(value, BalancedNetwork)
    return value


from . import dist  # noqa: E402  (needs EmitResult defined above)

MulticonductorNetwork = dist.MulticonductorNetwork


def versions() -> Any:
    """Return the PowerIO release, sole IR identity, and BMOPF schema."""
    return _json.loads(_powerio.versions_json())


class _TypedValue:
    """Typed view rooted in its owning :class:`PioModule`."""

    __slots__ = ("module", "_collection_entry")

    def __init__(self, module: "PioModule") -> None:
        self.module = module
        self._collection_entry = None

    def __repr__(self) -> str:
        return f"{type(self).__name__}()"


@dataclass(frozen=True)
class TimePoint:
    label: str
    duration_seconds: Optional[float] = None


@dataclass(frozen=True)
class Scenario:
    id: str
    probability: Optional[float] = None


@dataclass(frozen=True)
class _CollectionEntry:
    root: "PioModule"
    time_index: Optional[int] = None
    scenario_id: Optional[str] = None


def _bind_collection_entry(
    value: Any,
    location: _CollectionEntry,
) -> Any:
    setattr(value, "_collection_entry", location)
    return value


class TimeSeries(_TypedValue):
    """Values of one type ordered in time."""

    def __init__(
        self,
        values: Sequence[Any],
        *,
        time_points: Sequence[TimePoint],
    ) -> None:
        if isinstance(values, (str, bytes, bytearray)) or not isinstance(
            values, Sequence
        ):
            raise TypeError("TimeSeries values must be a sequence of PowerIO values")
        if not isinstance(time_points, Sequence):
            raise TypeError("time_points must be a sequence of TimePoint values")
        points = tuple(time_points)
        if not all(isinstance(point, TimePoint) for point in points):
            raise TypeError("time_points must contain only TimePoint values")
        modules = [PioModule.from_value(value)._inner for value in values]
        inner = _powerio._PioModule._from_time_series(
            modules,
            [(point.label, point.duration_seconds) for point in points],
        )
        super().__init__(PioModule(inner))

    @classmethod
    def _from_module(cls, module: "PioModule") -> "TimeSeries":
        value = object.__new__(cls)
        _TypedValue.__init__(value, module)
        return value

    @property
    def time_points(self) -> tuple[TimePoint, ...]:
        return tuple(TimePoint(*point) for point in self.module._inner._time_series_points())

    def __len__(self) -> int:
        return self.module._inner._time_series_len()

    def __getitem__(self, index: int) -> Any:
        try:
            position = _operator.index(index)
        except TypeError:
            raise TypeError("time series indices must be integers") from None
        if position < 0:
            position += len(self)
        if position < 0 or position >= len(self):
            raise IndexError("time series index out of range")
        current = self._collection_entry or _CollectionEntry(self.module)
        if current.time_index is not None:
            raise TypeError("nested TimeSeries values are not supported")
        value = PioModule(self.module._inner._time_series_get(position)).value
        return _bind_collection_entry(
            value,
            _CollectionEntry(
                root=current.root,
                time_index=position,
                scenario_id=current.scenario_id,
            ),
        )

    def __iter__(self):
        return (self[position] for position in range(len(self)))


class ScenarioSet(_TypedValue):
    """Named alternatives of one type, with optional probabilities."""

    def __init__(
        self,
        values: Mapping[str, Any],
        *,
        probabilities: Optional[Mapping[str, float]] = None,
    ) -> None:
        if not isinstance(values, Mapping):
            raise TypeError("ScenarioSet values must be a mapping from IDs to values")
        if probabilities is not None and not isinstance(probabilities, Mapping):
            raise TypeError("probabilities must be a mapping from scenario IDs to numbers")
        ids = list(values)
        modules = [PioModule.from_value(values[id])._inner for id in ids]
        inner = _powerio._PioModule._from_scenario_set(
            modules,
            ids,
            None if probabilities is None else dict(probabilities),
        )
        super().__init__(PioModule(inner))

    @classmethod
    def _from_module(cls, module: "PioModule") -> "ScenarioSet":
        value = object.__new__(cls)
        _TypedValue.__init__(value, module)
        return value

    @property
    def scenarios(self) -> tuple[Scenario, ...]:
        return tuple(Scenario(*entry) for entry in self.module._inner._scenario_entries())

    def keys(self) -> tuple[str, ...]:
        return tuple(scenario.id for scenario in self.scenarios)

    def __len__(self) -> int:
        return len(self.scenarios)

    def __iter__(self):
        return iter(self.keys())

    def __contains__(self, scenario: object) -> bool:
        return isinstance(scenario, str) and scenario in self.keys()

    def __getitem__(self, scenario: str) -> Any:
        if not isinstance(scenario, str):
            raise TypeError("scenario keys must be strings")
        if scenario not in self:
            raise KeyError(scenario)
        current = self._collection_entry or _CollectionEntry(self.module)
        if current.scenario_id is not None:
            raise TypeError("nested ScenarioSet values are not supported")
        value = PioModule(self.module._inner._scenario_get(scenario)).value
        return _bind_collection_entry(
            value,
            _CollectionEntry(
                root=current.root,
                time_index=current.time_index,
                scenario_id=scenario,
            ),
        )


class OperatingPoint(_TypedValue):
    """A possibly partial assignment over fixed equipment identities."""


class _BalancedCalculation(_TypedValue):
    """A calculation over one shared balanced network."""

    @property
    def network(self) -> BalancedNetwork:
        """The balanced network used by this calculation."""
        return BalancedNetwork(self.module._inner._balanced_calculation_network())


class _MulticonductorCalculation(_TypedValue):
    """A calculation over one shared multiconductor network."""

    @property
    def network(self) -> MulticonductorNetwork:
        """The multiconductor network used by this calculation."""
        return MulticonductorNetwork(
            self.module._inner._multiconductor_calculation_network()
        )


class _CalculationSolution(_TypedValue):
    """A solution that retains the exact typed instance it solves."""

    @property
    def instance(self) -> _TypedValue:
        """The calculation instance solved by this result."""
        return PioModule(self.module._inner._calculation_solution_instance()).value


class DcPfInstance(_BalancedCalculation):
    """A DC power flow calculation instance."""


class AcPfInstance(_BalancedCalculation):
    """An AC power flow calculation instance."""


class DcOpfInstance(_BalancedCalculation):
    """A DC optimal power flow calculation instance."""


class AcOpfInstance(_BalancedCalculation):
    """An AC optimal power flow calculation instance."""


class McAcPfInstance(_MulticonductorCalculation):
    """A multiconductor AC power flow calculation instance."""


class McAcOpfInstance(_MulticonductorCalculation):
    """A multiconductor AC optimal power flow calculation instance."""


class AcScucInstance(_BalancedCalculation):
    """An AC security constrained unit commitment calculation instance."""

    @property
    def inputs(self) -> ScucInputs:
        """Scheduling, reserve, and contingency inputs."""
        return self.module._inner._ac_scuc_inputs()


class DcPfSolution(_BalancedCalculation, _CalculationSolution):
    """A DC power flow solution."""


class AcPfSolution(_BalancedCalculation, _CalculationSolution):
    """An AC power flow solution."""


class DcOpfSolution(_BalancedCalculation, _CalculationSolution):
    """A DC optimal power flow solution."""


class AcOpfSolution(_BalancedCalculation, _CalculationSolution):
    """An AC optimal power flow solution."""


class SocwrOpfSolution(_BalancedCalculation, _CalculationSolution):
    """A PowerModels SOCWR relaxation solution and objective lower bound."""


class McAcPfSolution(_MulticonductorCalculation, _CalculationSolution):
    """A multiconductor AC power flow solution."""


class McAcOpfSolution(_MulticonductorCalculation, _CalculationSolution):
    """A multiconductor AC optimal power flow solution."""


class AcScucSolution(_BalancedCalculation, _CalculationSolution):
    """An AC security constrained unit commitment solution."""

    @property
    def termination(self) -> str:
        """How the calculation ended."""
        return self.module._inner._ac_scuc_solution_termination()

    @property
    def residuals(self) -> Residuals:
        """Reported active and reactive power balance residuals."""
        return self.module._inner._ac_scuc_solution_residuals()

    @property
    def producer(self) -> Optional[str]:
        """Producer or solver identity, when recorded."""
        return self.module._inner._ac_scuc_solution_producer()

    @property
    def network_outputs(self) -> ScucNetworkOutputs:
        """Per interval network outputs."""
        return self.module._inner._ac_scuc_solution_network_outputs()

    @property
    def device_outputs(self) -> ScucDeviceOutputs:
        """Per interval dispatchable device outputs."""
        return self.module._inner._ac_scuc_solution_device_outputs()

    @property
    def objective(self) -> Optional[float]:
        """Reported objective value, when present."""
        return self.module._inner._ac_scuc_solution_objective()


_VALUE_CLASSES: dict[str, type[_TypedValue]] = {
    "powerio.OperatingPoint<powerio.BalancedNetwork>": OperatingPoint,
    "powerio.OperatingPoint<powerio.MulticonductorNetwork>": OperatingPoint,
    "powerio.DcPfInstance": DcPfInstance,
    "powerio.AcPfInstance": AcPfInstance,
    "powerio.DcOpfInstance": DcOpfInstance,
    "powerio.AcOpfInstance": AcOpfInstance,
    "powerio.McAcPfInstance": McAcPfInstance,
    "powerio.McAcOpfInstance": McAcOpfInstance,
    "powerio.AcScucInstance": AcScucInstance,
    "powerio.DcPfSolution": DcPfSolution,
    "powerio.AcPfSolution": AcPfSolution,
    "powerio.DcOpfSolution": DcOpfSolution,
    "powerio.AcOpfSolution": AcOpfSolution,
    "powerio.SocwrOpfSolution": SocwrOpfSolution,
    "powerio.McAcPfSolution": McAcPfSolution,
    "powerio.McAcOpfSolution": McAcOpfSolution,
    "powerio.AcScucSolution": AcScucSolution,
}


class PioModule:
    """One typed value with diagnostics, producer, sources, source mappings,
    history, and extensions.
    """

    def __init__(self, inner: "_powerio._PioModule"):
        self._inner = inner

    @classmethod
    def from_value(cls, value: Any) -> "PioModule":
        """Wrap an existing typed value without serializing it."""
        if isinstance(value, BalancedNetwork):
            return cls(_powerio._PioModule.from_balanced_network(value._inner))
        if isinstance(value, dist.MulticonductorNetwork):
            return cls(_powerio._PioModule.from_multiconductor_network(value._inner))
        if isinstance(value, _TypedValue):
            location = value._collection_entry
            inner = value.module._inner
            if location is not None:
                inner = location.root._inner
                if location.scenario_id is not None:
                    inner = inner._scenario_get(location.scenario_id)
                if location.time_index is not None:
                    inner = inner._time_series_get(location.time_index)
            return cls(inner._copy())
        raise TypeError("PioModule.from_value expects a typed PowerIO value")

    @property
    def value(self) -> Any:
        """The contained typed value."""
        type_name = self._inner._type_name
        if type_name == "powerio.BalancedNetwork":
            return BalancedNetwork(self._inner.as_balanced_network())
        if type_name == "powerio.MulticonductorNetwork":
            return dist.MulticonductorNetwork(self._inner.as_multiconductor_network())
        if type_name.startswith("powerio.TimeSeries<"):
            return TimeSeries._from_module(self)
        if type_name.startswith("powerio.ScenarioSet<"):
            return ScenarioSet._from_module(self)
        value_class = _VALUE_CLASSES.get(type_name)
        if value_class is None:
            raise RuntimeError(f"this binding has no Python class for {type_name}")
        return value_class(self)

    @property
    def diagnostics(self) -> list[Diagnostic]:
        """The diagnostics stored on this module, in encounter order."""
        return list(self._inner.diagnostics)

    def to_balanced_report(self, base_mva: float = 100.0) -> Any:
        """Report whether a multiconductor network can become balanced."""
        return _json.loads(self._inner.lowering_readiness_json(base_mva))

    def to_balanced(self, base_mva: float = 100.0) -> "PioModule":
        """Transform a multiconductor network to a balanced module."""
        return PioModule(self._inner.lower_to_balanced(base_mva))

    def to_dc_pf_instance(self) -> "PioModule":
        """Build a DC power flow instance from a balanced network module."""
        return PioModule(self._inner._to_dc_pf_instance())

    def to_ac_pf_instance(self) -> "PioModule":
        """Build an AC power flow instance from a balanced network module."""
        return PioModule(self._inner._to_ac_pf_instance())

    def to_dc_opf_instance(self) -> "PioModule":
        """Build a DC optimal power flow instance from a balanced network module."""
        return PioModule(self._inner._to_dc_opf_instance())

    def to_ac_opf_instance(self) -> "PioModule":
        """Build an AC optimal power flow instance from a balanced network module."""
        return PioModule(self._inner._to_ac_opf_instance())

    def to_mc_ac_pf_instance(self) -> "PioModule":
        """Build a multiconductor AC power flow instance from a network module."""
        return PioModule(self._inner._to_mc_ac_pf_instance())

    def to_mc_ac_opf_instance(self) -> "PioModule":
        """Build a multiconductor AC optimal power flow instance from a network module."""
        return PioModule(self._inner._to_mc_ac_opf_instance())

    def __repr__(self) -> str:
        return repr(self._inner)


def _selected_collection_value(location: _CollectionEntry) -> Any:
    value = location.root.value
    time_index = location.time_index
    scenario_id = location.scenario_id
    while time_index is not None or scenario_id is not None:
        if isinstance(value, TimeSeries) and time_index is not None:
            value = value[time_index]
            time_index = None
        elif isinstance(value, ScenarioSet) and scenario_id is not None:
            value = value[scenario_id]
            scenario_id = None
        elif time_index is not None:
            raise TypeError("the selected value is not a TimeSeries")
        else:
            raise TypeError("the selected value is not a ScenarioSet")
    return value


def _refresh_collection_entry(target: Any, location: _CollectionEntry) -> None:
    refreshed = _selected_collection_value(location)
    if type(refreshed) is not type(target):
        raise RuntimeError("a collection update changed the entry type")
    if isinstance(target, BalancedNetwork):
        target._inner = refreshed._inner
    elif isinstance(target, dist.MulticonductorNetwork):
        target._inner = refreshed._inner
    elif isinstance(target, _TypedValue):
        target.module = refreshed.module
        target._collection_entry = refreshed._collection_entry
    else:
        raise TypeError("the selected value does not support typed updates")


def apply_updates(
    target: Any,
    updates: Union[
        Iterable[OperatingPointUpdate],
        Iterable[NetworkUpdate],
        Iterable[CalculationUpdate],
    ],
) -> UpdateReport:
    """Validate and apply one batch of typed updates atomically.

    ``updates`` contains one update class: :class:`OperatingPointUpdate`,
    :class:`NetworkUpdate`, or :class:`CalculationUpdate`. Values are absolute
    replacements and power quantities carry their units in the typed value.
    ``target`` is a module or a value obtained by indexing a :class:`TimeSeries`
    or :class:`ScenarioSet`. If validation fails, the module is unchanged.
    """
    batch = list(updates)
    if isinstance(target, PioModule):
        return target._inner._apply_updates(batch)
    location = getattr(target, "_collection_entry", None)
    if not isinstance(location, _CollectionEntry):
        raise TypeError(
            "target must be a PioModule or a TimeSeries/ScenarioSet entry"
        )
    report = location.root._inner._apply_collection_updates(
        batch,
        time_index=location.time_index,
        scenario_id=location.scenario_id,
    )
    _refresh_collection_entry(target, location)
    return report


def apply_bus_load_active_power(
    module: PioModule,
    bus_id: int,
    total: ActivePower,
    *,
    allocation: str = "proportional_to_current_active_power",
) -> UpdateReport:
    """Replace aggregate bus demand through an explicit PowerIO allocation rule.

    ``"proportional_to_current_active_power"`` preserves each participating
    load's current share. ``"equal"`` gives every participating load the same
    share, including when their current aggregate demand is zero. PowerIO
    requires stable load IDs and reports each load changed.
    """
    if not isinstance(module, PioModule):
        raise TypeError("module must be a PioModule")
    if not isinstance(total, ActivePower):
        raise TypeError("total must be an ActivePower")
    return module._inner._apply_bus_load_active_power(
        bus_id,
        total,
        allocation=allocation,
    )


def _path_from_source(source: Any) -> Optional[str]:
    if isinstance(source, str):
        return source
    if isinstance(source, (bytes, bytearray, memoryview)):
        return None
    try:
        path = _os.fspath(source)
    except TypeError:
        return None
    if isinstance(path, bytes):
        raise TypeError("a path-like source must return str, not bytes")
    return path


def _memory_from_source(source: Any, name: Optional[str]) -> tuple[bytes, str]:
    if isinstance(source, (bytes, bytearray, memoryview)):
        data = bytes(source)
    else:
        read = getattr(source, "read", None)
        if read is None:
            raise TypeError(
                "source must be a path, file object, or bytes-like object"
            )
        data = read()
        if isinstance(data, str):
            data = data.encode("utf-8")
        elif isinstance(data, (bytes, bytearray, memoryview)):
            data = bytes(data)
        else:
            raise TypeError("source.read() must return str or bytes-like data")
    if name is None:
        candidate = getattr(source, "name", None)
        try:
            candidate = _os.fspath(candidate) if candidate is not None else None
        except TypeError:
            candidate = None
        name = candidate if isinstance(candidate, str) else "<memory>"
    if not isinstance(name, str):
        raise TypeError("name must be a string")
    return data, name


def parse(
    source: Any,
    *,
    format: Optional[str] = None,
    name: Optional[str] = None,
) -> PioModule:
    """Parse a path, file object, or bytes-like source.

    A string is always a path. Pass raw text through ``io.StringIO`` or
    another file object.
    """
    path = _path_from_source(source)
    if path is not None:
        if name is not None:
            raise ValueError("name is only valid for memory and file object sources")
        return PioModule(_powerio._PioModule._parse_path(path, format))
    data, source_name = _memory_from_source(source, name)
    return PioModule(_powerio._PioModule._parse_memory(data, source_name, format))


def _result_from_native(result: dict[str, Any]) -> EmitResult:
    return EmitResult(
        artifacts=tuple(Artifact(**artifact) for artifact in result["artifacts"]),
        layout=result["layout"],
        fidelity=result["fidelity"],
        diagnostics=tuple(result["diagnostics"]),
    )


def _emit_to_destination(
    module: PioModule,
    destination: Optional[Any],
    memory_call: Any,
    path_call: Any,
) -> EmitResult:
    if not isinstance(module, PioModule):
        raise TypeError("module must be a PioModule")
    if destination is None:
        return _result_from_native(memory_call())
    path = _path_from_source(destination)
    if path is not None:
        return _result_from_native(path_call(path))
    write = getattr(destination, "write", None)
    if write is None:
        raise TypeError("destination must be a path or writable file object")
    result = _result_from_native(memory_call())
    if result.layout != "file" or len(result.artifacts) != 1:
        raise ValueError("a directory emission requires a path destination")
    data = result.artifacts[0].data
    assert data is not None
    try:
        write(data)
    except TypeError:
        write(data.decode("utf-8"))
    return result


def emit(module: PioModule, format: str, destination: Optional[Any] = None) -> EmitResult:
    """Emit a module as one grid exchange format."""
    return _emit_to_destination(
        module,
        destination,
        lambda: module._inner._emit_memory(format),
        lambda path: module._inner._emit_path(format, path),
    )


def serialize(module: PioModule, destination: Optional[Any] = None) -> EmitResult:
    """Serialize a module as PowerIO IR."""
    return _emit_to_destination(
        module,
        destination,
        module._inner._serialize_memory,
        module._inner._serialize_path,
    )


def deserialize(source: Any) -> PioModule:
    """Deserialize PowerIO IR from a path, file object, or bytes-like source."""
    path = _path_from_source(source)
    if path is not None:
        return PioModule(_powerio._PioModule._deserialize_path(path))
    data, _ = _memory_from_source(source, None)
    return PioModule(_powerio._PioModule._deserialize_memory(data))


def features() -> dict[str, bool]:
    """Optional build-time features compiled into this powerio installation.

    ``matrix``, ``dist``, and ``prob`` are unconditional dependencies of the
    extension and are always ``True``. ``gridfm`` reflects whether the
    GridFM Parquet parsing and emission were compiled in; the published wheel
    always includes them, while a custom source build can omit them.
    ``arrow`` is always ``False``: unlike the C ABI and the Julia binding,
    this binding calls into the Rust core directly and does not expose the
    Arrow C Data Interface.
    """
    return {
        "arrow": False,
        "matrix": True,
        "gridfm": bool(getattr(_powerio, "_has_gridfm", False)),
        "dist": True,
        "prob": True,
    }
