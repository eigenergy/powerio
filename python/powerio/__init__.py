"""Parse, transform, and emit power system data.

``parse_file`` returns a module whose ``value`` is the typed power system
object and whose ``diagnostics`` record what the parser found. ``emit`` produces
the module to another format while preserving its source and diagnostic
records::

    import powerio as pio

    module = pio.parse_file("case9.m", value_type=pio.BalancedNetwork)
    net = module.value
    print(net.n_buses, net.base_mva)         # 9 100.0
    matpower = module.emit("matpower")       # EmitResult(text, diagnostics)
    emitted = module.emit("psse", "case9.raw")

    B = net.calc_bprime_matrix()             # scipy.sparse, MATPOWER Bp
    Y = net.calc_admittance_matrix()         # complex csr, G + jB
    G = net.to_networkx()                    # networkx.Graph keyed by bus id

PyPSA CSV folders carry static network topology. NetCDF and HDF5 time series
are tracked in https://github.com/eigenergy/powerio/issues/107.

A source that defines a calculation parses to that calculation's typed value.
:func:`parse_file` always returns a :class:`PioModule`; ``module.kind`` names
the value and ``module.value`` reads it.

``import powerio`` and the base parse and emit paths require no
third party Python package. Matrix methods require SciPy and NumPy. Graph
methods require NetworkX. Install them with ``powerio[matrix]``,
``powerio[graph]``, or ``powerio[all]``. Missing extras raise ``ImportError``.
"""

from __future__ import annotations

import importlib
import json as _json
import operator as _operator
import os as _os
from collections import namedtuple
from typing import Any, Optional

from . import _powerio
from ._powerio import (
    Diagnostic,
    PowerIODataError,
    PowerIOError,
    PowerIOParseError,
    SourceSpan,
    __version__,
)

__all__ = [
    "AcOpfInstance",
    "AcOpfSolution",
    "AcPfInstance",
    "AcPfSolution",
    "AcScucInstance",
    "AcScucSolution",
    "BalancedNetwork",
    "DcOpfInstance",
    "DcOpfSolution",
    "DcPfInstance",
    "DcPfSolution",
    "Diagnostic",
    "DisplayData",
    "EmitResult",
    "McAcOpfInstance",
    "McAcOpfSolution",
    "McAcPfInstance",
    "McAcPfSolution",
    "PioModule",
    "PowerIODataError",
    "PowerIOError",
    "PowerIOParseError",
    "PwdDisplay",
    "PwdSubstation",
    "ScenarioSet",
    "SourceSpan",
    "TimeSeries",
    "UnknownValue",
    "__version__",
    "dist",
    "emit_gridfm_batch",
    "features",
    "from_json",
    "from_ppc",
    "parse_display_file",
    "parse_file",
    "parse_geo",
    "parse_text",
    "versions",
]

EmitResult = namedtuple("EmitResult", ["text", "diagnostics"])
EmitResult.__doc__ = """Output of :meth:`PioModule.emit`.

``text`` is the emitted file contents when no destination was supplied and
``None`` after committing a file or directory. ``diagnostics`` lists the
fields the target format could not represent (empty for a faithful emission).
"""

DisplayData = namedtuple("DisplayData", ["kind", "data"])
DisplayData.__doc__ = """Output of :func:`parse_display_file`.

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


def _native_attr(owner: Any, name: str) -> Any:
    """Read a private extension entry omitted from the public native stub."""
    return getattr(owner, name)


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


def _require_gridfm() -> None:
    """Raise a clear ImportError if the extension lacks the gridfm Parquet surface.

    Published wheels include this surface. A custom source build can omit the
    Rust feature, in which case the method names still raise a direct error
    instead of failing with ``AttributeError``.
    """
    if not getattr(_powerio, "_has_gridfm", False):
        raise ImportError(
            "powerio was built without the gridfm Parquet surface; reinstall a "
            "wheel built with gridfm support or rebuild from source with "
            "`maturin develop --features gridfm`."
        )


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
        "n_storage",
        "n_switches",
        "n_transformers_3w",
        "name",
        "reference_bus_index",
        "reference_bus_indices",
        "shunts",
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

    def to_json(self) -> str:
        """Serialize to the JSON transport."""
        return self._inner.to_json()

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

    def calc_branch_susceptance_matrix(self, formula: str = "series_susceptance"):
        """Return ``Bf = diag(b) A`` as a CSR matrix."""
        return _to_csr(self._inner.calc_branch_susceptance_matrix(formula))

    def calc_bus_susceptance_matrix(self, formula: str = "series_susceptance"):
        """Return ``B = A.T diag(b) A`` as a CSR matrix."""
        return _to_csr(self._inner.calc_bus_susceptance_matrix(formula))

    def calc_phase_shift_injection(self, formula: str = "series_susceptance"):
        """Return ``A.T @ (b * shift)`` in bus order."""
        np = _require("numpy", "matrix")
        return np.asarray(self._inner.calc_phase_shift_injection(formula), dtype=float)

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

    def emit_gridfm(
        self,
        out_dir: Any,
        *,
        scenario: int = 0,
        include_y_bus: bool = True,
        include_taps: bool = True,
        include_shifts: bool = True,
        missing_gen_cost: Optional[str] = None,
        default_gen_cost: Optional[str] = None,
        gen_cost_csv: Optional[Any] = None,
    ) -> dict:
        """Emit the gridfm-datakit Parquet dataset for this case under
        ``<out_dir>/<case>/raw/``.

        Returns a dict with ``dir``, ``files``, ``dropped_zero_impedance``, and
        ``degenerate_cost_gens``. Published wheels include the native emitter;
        custom source builds without the Rust ``gridfm`` feature raise
        ``ImportError``. For many perturbed snapshots in one dataset, see
        :func:`emit_gridfm_batch`.
        """
        _require_gridfm()
        return self._inner.write_gridfm(
            str(out_dir),
            scenario=scenario,
            include_y_bus=include_y_bus,
            include_taps=include_taps,
            include_shifts=include_shifts,
            missing_gen_cost=missing_gen_cost,
            default_gen_cost=default_gen_cost,
            gen_cost_csv=None if gen_cost_csv is None else str(gen_cost_csv),
        )

    def emit_dcopf_bundle(
        self,
        out_dir: Any,
        formula: str = "series_susceptance",
        units: str = "perunit",
        missing_gen_cost: Optional[str] = None,
        default_gen_cost: Optional[str] = None,
        gen_cost_csv: Optional[Any] = None,
    ) -> dict[str, Any]:
        """Emit the DC OPF matrix bundle under ``out_dir``."""
        return self._inner.write_dcopf_bundle(
            str(out_dir),
            formula,
            units,
            missing_gen_cost,
            default_gen_cost,
            None if gen_cost_csv is None else str(gen_cost_csv),
        )

    def to_normalized(
        self,
        *,
        clamp_angle_bounds: bool = False,
        angle_bound_pad: Optional[float] = None,
    ) -> "BalancedNetwork":
        """Return a normalized copy with per unit power and radian angles.

        The result removes out of service elements, preserves source bus IDs,
        and normalizes bus types. It carries no retained source, so
        :meth:`PioModule.emit` serializes the derived model. Raises
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


def parse_display_file(path: Any, format: Optional[str] = None) -> DisplayData:
    """Parse a display artifact such as a PowerWorld ``.pwd`` file."""
    return _wrap_display(_powerio.parse_display_file(str(path), format))


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


def from_json(text: str) -> BalancedNetwork:
    """Rebuild a case from JSON produced by :meth:`BalancedNetwork.to_json`."""
    return BalancedNetwork(_powerio.from_json(text))


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
    module = PioModule(
        _native_attr(_powerio._PioModule, "from_str")(
            _ppc_to_matpower_text(ppc), "matpower"
        )
    )
    return module.as_balanced_network()


def emit_gridfm_batch(
    networks: "list[BalancedNetwork]",
    out_dir: Any,
    *,
    base_scenario: int = 0,
    include_y_bus: bool = True,
    include_taps: bool = True,
    include_shifts: bool = True,
    missing_gen_cost: Optional[str] = None,
    default_gen_cost: Optional[str] = None,
    gen_cost_csv: Optional[Any] = None,
) -> dict:
    """Emit several networks as one gridfm-datakit dataset, row stacked and
    keyed by the ``scenario`` column.

    Each network is one snapshot; the k-th is stamped ``base_scenario + k``. The
    networks must share a base element set: the same bus/branch/gen counts and
    bus id order (otherwise :class:`PowerIODataError` is raised). Load, dispatch,
    branch status, and costs may vary per scenario. Returns the same dict as
    :meth:`BalancedNetwork.emit_gridfm`. Published wheels include the native
    emitter; custom source builds without the Rust ``gridfm`` feature raise
    ``ImportError``.
    """
    _require_gridfm()
    inners = [c._inner for c in networks]
    return _native_attr(_powerio, "write_gridfm_batch")(
        inners,
        str(out_dir),
        base_scenario=base_scenario,
        include_y_bus=include_y_bus,
        include_taps=include_taps,
        include_shifts=include_shifts,
        missing_gen_cost=missing_gen_cost,
        default_gen_cost=default_gen_cost,
        gen_cost_csv=None if gen_cost_csv is None else str(gen_cost_csv),
    )


from . import dist  # noqa: E402  (needs EmitResult defined above)


def versions() -> Any:
    """Version and schema identity of this build.

    The release API discovery document: the powerio release, the stored
    module schema name and version, and the BMOPF schema this build speaks.
    Keys agree with the C ``pio_schema_versions_json`` report where both
    apply.
    """
    return _json.loads(_powerio.versions_json())


class _TypedValue:
    """Base for a thin typed wrapper around a :class:`PioModule` value that
    has no dedicated handle type of its own.

    Holds the owning module and its kind; this release exposes no per field
    accessors for these kinds, so read a value back from ``module`` (its
    ``to_json``, ``inspect``, or — for a series or scenario set —
    ``list_states``/``inspect_state``/``export_state``).
    """

    __slots__ = ("module", "kind")

    def __init__(self, module: "PioModule", kind: str) -> None:
        self.module = module
        self.kind = kind

    def __repr__(self) -> str:
        return f"{type(self).__name__}(kind={self.kind!r})"


class TimeSeries(_TypedValue):
    """A typed time series whose items are independently usable modules."""

    def _points(self):
        return self.module.list_states().get("time_points", [])

    def __len__(self) -> int:
        return len(self._points())

    def __getitem__(self, index: int) -> "PioModule":
        try:
            position = _operator.index(index)
        except TypeError:
            raise TypeError("time series indices must be integers") from None
        length = len(self)
        if position < 0:
            position += length
        if position < 0 or position >= length:
            raise IndexError("time series index out of range")
        return self.module.export_state(time_position=position)

    def __iter__(self):
        for position in range(len(self)):
            yield self[position]


class ScenarioSet(_TypedValue):
    """A typed scenario mapping whose values are independently usable modules."""

    def keys(self) -> "tuple[str, ...]":
        return tuple(
            scenario["id"]
            for scenario in self.module.list_states().get("scenarios", [])
        )

    def __len__(self) -> int:
        return len(self.keys())

    def __iter__(self):
        return iter(self.keys())

    def __contains__(self, scenario: object) -> bool:
        return isinstance(scenario, str) and scenario in self.keys()

    def __getitem__(self, scenario: str) -> "PioModule":
        if not isinstance(scenario, str):
            raise TypeError("scenario keys must be strings")
        if scenario not in self:
            raise KeyError(scenario)
        return self.module.export_state(scenario=scenario)


class DcPfInstance(_TypedValue):
    """A DC power flow problem instance."""


class AcPfInstance(_TypedValue):
    """An AC power flow problem instance."""


class DcOpfInstance(_TypedValue):
    """A DC OPF problem instance."""


class AcOpfInstance(_TypedValue):
    """An AC OPF problem instance."""


class McAcPfInstance(_TypedValue):
    """A multiconductor AC power flow problem instance."""


class McAcOpfInstance(_TypedValue):
    """A multiconductor AC OPF problem instance."""


class AcScucInstance(_TypedValue):
    """An AC security constrained unit commitment problem instance."""


class DcPfSolution(_TypedValue):
    """A DC power flow solution."""


class AcPfSolution(_TypedValue):
    """An AC power flow solution."""


class DcOpfSolution(_TypedValue):
    """A DC OPF solution."""


class AcOpfSolution(_TypedValue):
    """An AC OPF solution."""


class McAcPfSolution(_TypedValue):
    """A multiconductor AC power flow solution."""


class McAcOpfSolution(_TypedValue):
    """A multiconductor AC OPF solution."""


class AcScucSolution(_TypedValue):
    """An AC security constrained unit commitment solution."""


class UnknownValue(_TypedValue):
    """A module kind this release does not expose as a typed Python value.

    This covers a kind newer than this release and a known value whose state
    cannot yet be exported without inventing semantics, such as a
    multiconductor operating point series. ``module`` and ``kind`` still work,
    so a caller can inspect, store, or emit the module unchanged.
    """


# kind string (PioModule.kind) -> the .value wrapper it reads back as. The two
# network kinds ("balanced_network", "multiconductor_network") are not here:
# PioModule.value special-cases them to the real network handle instead of one
# of these thin wrappers.
_VALUE_CLASSES: "dict[str, type]" = {
    "balanced_network_time_series": TimeSeries,
    "balanced_operating_point_time_series": TimeSeries,
    "multiconductor_operating_point_time_series": UnknownValue,
    "balanced_network_scenario_set": ScenarioSet,
    "dc_pf_instance": DcPfInstance,
    "ac_pf_instance": AcPfInstance,
    "dc_opf_instance": DcOpfInstance,
    "ac_opf_instance": AcOpfInstance,
    "mc_ac_pf_instance": McAcPfInstance,
    "mc_ac_opf_instance": McAcOpfInstance,
    "ac_scuc_instance": AcScucInstance,
    "dc_pf_solution": DcPfSolution,
    "ac_pf_solution": AcPfSolution,
    "dc_opf_solution": DcOpfSolution,
    "ac_opf_solution": AcOpfSolution,
    "mc_ac_pf_solution": McAcPfSolution,
    "mc_ac_opf_solution": McAcOpfSolution,
    "ac_scuc_solution": AcScucSolution,
}

# The inverse is not one-to-one because TimeSeries covers two stable value
# kinds. These sets make ``value_type`` an assertion for every typed value the
# runtime already returns through ``PioModule.value``.
_VALUE_TYPE_KINDS = {
    TimeSeries: frozenset(
        {
            "balanced_network_time_series",
            "balanced_operating_point_time_series",
        }
    ),
    ScenarioSet: frozenset({"balanced_network_scenario_set"}),
    DcPfInstance: frozenset({"dc_pf_instance"}),
    AcPfInstance: frozenset({"ac_pf_instance"}),
    DcOpfInstance: frozenset({"dc_opf_instance"}),
    AcOpfInstance: frozenset({"ac_opf_instance"}),
    McAcPfInstance: frozenset({"mc_ac_pf_instance"}),
    McAcOpfInstance: frozenset({"mc_ac_opf_instance"}),
    AcScucInstance: frozenset({"ac_scuc_instance"}),
    DcPfSolution: frozenset({"dc_pf_solution"}),
    AcPfSolution: frozenset({"ac_pf_solution"}),
    DcOpfSolution: frozenset({"dc_opf_solution"}),
    AcOpfSolution: frozenset({"ac_opf_solution"}),
    McAcPfSolution: frozenset({"mc_ac_pf_solution"}),
    McAcOpfSolution: frozenset({"mc_ac_opf_solution"}),
    AcScucSolution: frozenset({"ac_scuc_solution"}),
}


class PioModule:
    """A runtime module handle: one typed value with its common records.

    The stored form is ``.pio.json`` version 1; released 0.9 packages upgrade
    one way on read. Selection returns the existing typed item; export is the
    separate explicit materialization.
    """

    def __init__(self, inner: "_powerio._PioModule"):
        self._inner = inner

    @classmethod
    def from_value(cls, value: Any) -> "PioModule":
        """Wrap an existing network value without serializing or reparsing it.

        Common records and retained source remain attached, so a parsed value
        still emits a byte exact same format echo. A generated value gains the
        ordinary module ``emit`` path.
        """
        if isinstance(value, BalancedNetwork):
            return cls(_powerio._PioModule.from_balanced_network(value._inner))
        if isinstance(value, dist.MulticonductorNetwork):
            return cls(_powerio._PioModule.from_multiconductor_network(value._inner))
        raise TypeError(
            "PioModule.from_value expects a BalancedNetwork or MulticonductorNetwork"
        )

    @property
    def value(self) -> Any:
        """The typed value ``kind`` names, as the ordinary Python object for it.

        ``balanced_network`` and ``multiconductor_network`` read back as the
        network handle (:class:`BalancedNetwork` /
        :class:`dist.MulticonductorNetwork`). Every other kind reads back as
        a thin wrapper — :class:`TimeSeries`, :class:`ScenarioSet`, or one of
        the calculation instance/solution classes (:class:`DcPfInstance`,
        :class:`AcOpfSolution`, and so on) — holding this module; a kind this
        release cannot expose without loss reads back as
        :class:`UnknownValue`. This is the ordinary way to read the value
        :func:`parse_file` narrowed to with ``value_type``.
        """
        kind = self.kind
        if kind == "balanced_network":
            return self.as_balanced_network()
        if kind == "multiconductor_network":
            return self.as_multiconductor_network()
        return _VALUE_CLASSES.get(kind, UnknownValue)(self, kind)

    def as_balanced_network(self) -> "BalancedNetwork":
        """The balanced network value as a network handle (cheap table
        share). Raises when the module carries another kind. ``.value`` is
        the ordinary spelling; call this directly only when the static
        ``BalancedNetwork`` return type (rather than ``Any``) matters."""
        return BalancedNetwork(self._inner.as_balanced_network())

    def as_multiconductor_network(self) -> "dist.MulticonductorNetwork":
        """The multiconductor network value as a network handle. Raises when
        the module carries another kind. ``.value`` is the ordinary
        spelling; call this directly only when the static
        ``MulticonductorNetwork`` return type (rather than ``Any``) matters."""
        return dist.MulticonductorNetwork(self._inner.as_multiconductor_network())

    def emit(self, format: str, destination: Optional[Any] = None):
        """Emit the module in a target format.

        With no ``destination``, the result contains emitted text and
        diagnostics. With a path destination, the complete artifact inventory
        is committed there and ``text`` is ``None``. The return type is always
        :class:`EmitResult`. For a directory format such as ``dss`` or
        ``pypsa-csv``, ``destination`` names the output directory; ``dss``
        stores its primary case as ``case.dss`` beside any companion files.

        """
        if destination is None:
            text, diagnostics = _native_attr(self._inner, "to_format")(format)
            return EmitResult(text, diagnostics)
        diagnostics = _native_attr(self._inner, "write_file")(str(destination), format)
        return EmitResult(None, diagnostics)

    @property
    def kind(self) -> str:
        """The value's permanent kind identifier."""
        return self._inner.kind()

    def inspect(self) -> Any:
        """Value inspection and supported operation discovery."""
        return _json.loads(self._inner.inspect_json())

    @property
    def diagnostics(self) -> "list[_powerio.Diagnostic]":
        """The diagnostics stored on this module, in encounter order."""
        return _native_attr(self._inner, "diagnostics")()

    def list_states(self) -> Any:
        """List the typed time points or scenarios stored by this module."""
        return _json.loads(self._inner.list_states_json())

    def inspect_state(
        self,
        time_position: Optional[int] = None,
        scenario: Optional[str] = None,
    ) -> Any:
        """Describe the selected existing typed item without materializing.

        ``time_position`` is zero based, the C convention: the first point in
        the series or scenario set is position ``0``.
        """
        return _json.loads(self._inner.inspect_state_json(time_position, scenario))

    def export_state(
        self,
        time_position: Optional[int] = None,
        scenario: Optional[str] = None,
    ) -> "PioModule":
        """Export the selected item as an independent static module.

        ``time_position`` is zero based, the C convention: the first point in
        the series or scenario set is position ``0``.
        """
        return PioModule(self._inner.export_state(time_position, scenario))

    def to_balanced_report(self, base_mva: float = 100.0) -> Any:
        """Report whether a multiconductor value can become balanced."""
        return _json.loads(self._inner.lowering_readiness_json(base_mva))

    def to_balanced(self, base_mva: float = 100.0) -> "PioModule":
        """Transform the multiconductor value to a balanced module.

        On refusal, raises :class:`PowerIODataError` with the refusal's
        diagnostic code as ``.code`` and its structured findings as
        ``.diagnostics`` (a list of dicts with ``code``, ``severity``,
        ``message``, and ``target``).
        """
        return PioModule(self._inner.lower_to_balanced(base_mva))

    def __repr__(self) -> str:
        return repr(self._inner)


def _assert_value_type(module: "PioModule", value_type: Optional[type]) -> "PioModule":
    """Apply the optional typed-value assertion shared by parse entries."""
    if value_type is None or value_type is PioModule:
        return module
    if value_type is BalancedNetwork:
        expected = frozenset({"balanced_network"})
    elif value_type is dist.MulticonductorNetwork:
        expected = frozenset({"multiconductor_network"})
    elif value_type in _VALUE_TYPE_KINDS:
        expected = _VALUE_TYPE_KINDS[value_type]
    else:
        raise TypeError(
            "value_type must be powerio.PioModule or a public powerio value class"
        )
    if module.kind not in expected:
        expected_text = (
            repr(next(iter(expected)))
            if len(expected) == 1
            else "one of " + repr(tuple(sorted(expected)))
        )
        raise ValueError(
            f"parsed value has kind {module.kind!r}; value_type="
            f"{value_type.__name__} asserts {expected_text}"
        )
    return module


def parse_file(
    path: Any,
    format: Optional[str] = None,
    *,
    include_root: Optional[Any] = None,
    value_type: Optional[type] = None,
) -> "PioModule":
    """Parse a file into a :class:`PioModule`.

    ``format`` overrides format detection. ``include_root`` widens the
    acquisition root for formats whose includes reference sibling files.
    """
    try:
        path = _os.fspath(path)
    except TypeError as error:
        raise TypeError(
            "parse_file path must be a string or path-like object"
        ) from error
    if isinstance(path, bytes):
        raise TypeError("parse_file path must be text, not bytes")
    root = None if include_root is None else str(include_root)
    module = PioModule(
        _native_attr(_powerio._PioModule, "from_file")(path, format, root)
    )
    return _assert_value_type(module, value_type)


def parse_text(
    text: str,
    *,
    name: str,
    format: Optional[str] = None,
    include_root: Optional[Any] = None,
    value_type: Optional[type] = None,
) -> "PioModule":
    """Parse text held in memory into a :class:`PioModule`.

    ``name`` identifies the source in diagnostics and enables extension based
    format detection. In-memory sources never acquire referenced files, so a
    non-``None`` ``include_root`` is refused; use :func:`parse_file` for a case
    that reads includes from disk.
    """
    if not isinstance(text, str):
        raise TypeError("parse_text text must be a string")
    if not isinstance(name, str):
        raise TypeError("parse_text name must be a string")
    if include_root is not None:
        raise ValueError(
            "parse_text cannot acquire files from include_root; use parse_file"
        )
    module = PioModule(
        _native_attr(_powerio._PioModule, "from_bytes")(
            text.encode("utf-8"), format, name
        )
    )
    return _assert_value_type(module, value_type)


def features() -> dict[str, bool]:
    """Optional build-time features compiled into this powerio installation.

    ``matrix``, ``dist``, and ``prob`` are unconditional dependencies of the
    extension and are always ``True``. ``gridfm`` reflects whether the
    GridFM Parquet parser and emitter (``BalancedNetwork.emit_gridfm`` and
    ``emit_gridfm_batch``) were compiled in; the published wheel always includes them,
    but a custom source build can omit it (see :func:`emit_gridfm_batch`).
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
