"""Parse, convert, and project power system data.

Readers produce a format neutral network model. Writers return retained source
bytes where supported or report fields that a target format cannot represent.
Modules, sparse matrices, graphs, and problem instances use the same parsed
data::

    import powerio as pio

    net = pio.parse("case9.m", value_type=pio.BalancedNetwork).value
    print(net.n_buses, net.base_mva)         # 9 100.0
    text = net.to_matpower()                 # byte-exact MATPOWER echo
    raw, warnings = pio.convert_file("case9.m", "psse")
    pp_json, warnings = pio.convert_file("case9.m", "pandapower-json")
    pypsa_out = net.write_pypsa_csv_folder("case9-pypsa")

    B = net.bprime()                         # scipy.sparse, MATPOWER Bp
    Y = net.ybus()                           # complex csr, G + jB
    G = net.to_networkx()                    # networkx.Graph keyed by bus id

PyPSA CSV folders carry static network topology. NetCDF and HDF5 time series
are tracked in https://github.com/eigenergy/powerio/issues/107.

A source that defines a calculation parses to that calculation's typed
value: :func:`parse` returns a :class:`PioModule` whose kind names it, and
whose ``.value`` property reads the typed value back out.

``import powerio`` and the base parse, write, and conversion paths require no
third party Python package. Matrix methods require SciPy and NumPy. Graph
methods require NetworkX. Install them with ``powerio[matrix]``,
``powerio[graph]``, or ``powerio[all]``. Missing extras raise ``ImportError``.
"""

from __future__ import annotations

import importlib
import json as _json
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
    "Conversion",
    "DcOpfInstance",
    "DcOpfSolution",
    "DcPfInstance",
    "DcPfSolution",
    "Diagnostic",
    "DisplayData",
    "GridfmRead",
    "Incidence",
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
    "YbusParts",
    "__version__",
    "convert_file",
    "convert_str",
    "dist",
    "from_json",
    "from_ppc",
    "parse",
    "parse_display_bytes",
    "parse_display_file",
    "parse_geo",
    "read_gridfm",
    "read_gridfm_scenarios",
    "to_format",
    "to_json",
    "to_matpower",
    "versions",
    "write_gridfm_batch",
]

Conversion = namedtuple("Conversion", ["text", "warnings"])
Conversion.__doc__ = """Output of :func:`convert_file`.

``text`` is the converted file contents; ``warnings`` lists the fields the
target format could not represent (empty for a faithful conversion).
"""

GridfmRead = namedtuple("GridfmRead", ["network", "scenario", "warnings"])
GridfmRead.__doc__ = """Output of :func:`read_gridfm` / :func:`read_gridfm_scenarios`.

``network`` is the reconstructed :class:`BalancedNetwork`; ``scenario`` is the source
scenario ID; ``warnings`` lists fields the GridFM schema cannot retain,
including source bus IDs, per element load and shunt rows, HVDC, storage, and
piecewise costs.
"""

DisplayData = namedtuple("DisplayData", ["kind", "data"])
DisplayData.__doc__ = """Output of :func:`parse_display_file` / :func:`parse_display_bytes`.

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

Incidence = namedtuple("Incidence", ["A", "b", "p_shift", "branch_of_col"])
Incidence.__doc__ = """Output of :meth:`BalancedNetwork.incidence`.

Shapes, with ``n`` buses and ``m`` in-service branches:
- ``A``: signed incidence csr_matrix, ``(n, m)``.
- ``b``: positive Laplacian edge weights, ``(m,)``; ``b[k]`` is column ``k``.
  These are the factor weights a sparse solver uses; PowerModels sign
  susceptances live on :meth:`BalancedNetwork.dc_data`.
- ``p_shift``: phase-shift injection, ``(n,)`` (all zero unless
  ``convention="matpower"``).
- ``branch_of_col``: column→branch index map, ``(m,)``; ``branch_of_col[k]``
  and ``b[k]`` are co-indexed by incidence column ``k``.
"""

YbusParts = namedtuple("YbusParts", ["g", "b"])
YbusParts.__doc__ = (
    "Output of :meth:`BalancedNetwork.ybus_parts`: ``g`` = Re(Y_bus), ``b`` = Im(Y_bus), "
    "each a real csr_matrix. ``BalancedNetwork.ybus()`` returns ``g + 1j*b``."
)

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


class BalancedNetwork:
    """A parsed balanced power network.

    The data attributes (``buses``, ``branches``, ``gens``, ``loads``,
    ``shunts``) and the non-matrix methods (``write``, ``reference_bus_index``,
    ``connectivity_report``, ``write_dcopf_bundle``) delegate to the compiled
    handle; the matrix methods below return ``scipy.sparse`` objects. Read
    fidelity warnings from parse time are on ``read_warnings``. Readers use this
    for source data they cannot model or assumptions they had to make.

    Errors: a bad file path raises the standard ``OSError`` subclass
    (``FileNotFoundError``); a malformed case raises :class:`PowerIOParseError`
    and an unmet builder precondition (no generators, no reference bus) raises
    :class:`PowerIODataError`; both subclass :class:`PowerIOError`, so
    ``except PowerIOError`` catches either; an unknown
    ``scheme``/``convention``/``units`` string raises ``ValueError``.
    """

    def __init__(self, inner: "_powerio._BalancedNetwork"):
        self._inner = inner

    def __dir__(self):
        # The data attributes arrive through __getattr__, so name them here or
        # they stay invisible to tab completion.
        return sorted(set(super().__dir__()) | set(dir(self._inner)))

    def __getattr__(self, name: str):
        # Reached only when normal lookup misses, so the matrix methods below
        # win. Guard underscore names so a lookup before _inner exists raises
        # AttributeError instead of recursing forever.
        if name.startswith("_"):
            raise AttributeError(
                f"{type(self).__name__!r} object has no attribute {name!r}"
            )
        return getattr(self._inner, name)

    def __repr__(self) -> str:
        # The inner handle's __repr__ already renders the public ``BalancedNetwork(...)``
        # form, so this is a straight delegate.
        return repr(self._inner)

    # --- canonical format and table exports -----------------------------

    def to_matpower(self) -> str:
        """Serialize to MATPOWER ``.m`` text.

        A case parsed from MATPOWER keeps its original source, so this returns a
        byte-exact echo. Derived cases serialize from the format neutral model.
        """
        return self._inner.to_matpower()

    def to_json(self) -> str:
        """Serialize to the JSON transport."""
        return self._inner.to_json()

    def geo_layer(self) -> dict[str, Any]:
        """This case's coordinates as a canonical GeoJSON FeatureCollection.

        Raises :class:`PowerIOError` when the case carries none.
        """
        return _json.loads(self._inner.geo_layer_json())

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
        so a same-format write re-serializes.
        """
        inner, report = self._inner.apply_geo_layer(text, name_hint)
        return BalancedNetwork(inner), report

    def to_format(
        self,
        to: str,
        missing_gen_cost: Optional[str] = None,
        default_gen_cost: Optional[str] = None,
        gen_cost_csv: Optional[Any] = None,
    ) -> Conversion:
        """Serialize this parsed case to another format.

        ``to`` is one of the format names accepted by :func:`convert_file`.
        Returns a :class:`Conversion` with output text and fidelity warnings.
        """
        text, warnings = self._inner.to_format(
            to,
            missing_gen_cost=missing_gen_cost,
            default_gen_cost=default_gen_cost,
            gen_cost_csv=None if gen_cost_csv is None else str(gen_cost_csv),
        )
        return Conversion(text, warnings)

    def to_canonical_format(self, to: str) -> Conversion:
        """Serialize to ``to`` from the typed model, bypassing source echo."""
        text, warnings = self._inner.to_canonical_format(to)
        return Conversion(text, warnings)

    def write_file(
        self,
        path: Any,
        to: str,
        missing_gen_cost: Optional[str] = None,
        default_gen_cost: Optional[str] = None,
        gen_cost_csv: Optional[Any] = None,
    ) -> list[str]:
        r"""Serialize this case to ``to`` and write it to ``path`` byte exact.

        Returns the fidelity warnings. Prefer this over writing
        :meth:`to_format` text through ``open(path, "w")``: Python's text mode
        translates newlines on Windows, so a case whose retained source has
        CRLF line endings comes out with doubled carriage returns
        (``\r\r\n``), which PSS/E family tools reject.
        """
        return self._inner.write_file(
            str(path),
            to,
            missing_gen_cost=missing_gen_cost,
            default_gen_cost=default_gen_cost,
            gen_cost_csv=None if gen_cost_csv is None else str(gen_cost_csv),
        )

    # --- matrix builders (scipy.sparse) ---------------------------------

    def bprime(self, scheme: str = "bx"):
        """MATPOWER FDPF Bp matrix. ``scheme`` is ``"bx"`` or ``"xb"``."""
        return _to_csr(self._inner.bprime(scheme))

    def dc_data(self, formula: str = "series_susceptance"):
        """DC branch data under one named susceptance formula.

        Incidence row endpoints, susceptance, the phase shift injection,
        stable element mappings for included rows and omitted branches, and
        the selected formula. Key spellings match the C ``pio_dc_data_*``
        accessors, so every language reads the same names in the same
        element order.
        """
        return self._inner.dc_data(formula)

    def bdoubleprime(self, scheme: str = "bx"):
        """MATPOWER FDPF Bpp matrix. ``scheme`` is ``"bx"`` or ``"xb"``."""
        return _to_csr(self._inner.bdoubleprime(scheme))

    def lacpf(self, *, include_taps: bool = True, include_shifts: bool = True):
        """LACPF 2n×2n block ``[[G, -B], [-B, -G]]``."""
        return _to_csr(
            self._inner.lacpf(include_taps=include_taps, include_shifts=include_shifts)
        )

    def adjacency(self):
        """0/1 bus adjacency matrix."""
        return _to_csr(self._inner.adjacency())

    def ybus_parts(self, *, include_taps: bool = True, include_shifts: bool = True):
        """:class:`YbusParts` ``(g, b)`` = ``(Re(Y_bus), Im(Y_bus))``, two real
        csr_matrix."""
        g, b = self._inner.ybus_parts(
            include_taps=include_taps, include_shifts=include_shifts
        )
        return YbusParts(g=_to_csr(g), b=_to_csr(b))

    def ybus(self, *, include_taps: bool = True, include_shifts: bool = True):
        """``Y_bus = G + jB`` as a complex csr_matrix."""
        g, b = self.ybus_parts(
            include_taps=include_taps, include_shifts=include_shifts
        )
        return (g + 1j * b).tocsr()

    def ptdf(self, convention: str = "series", solver: str = "auto"):
        """DC PTDF (m×n). ``convention`` is ``"series"`` or ``"matpower"``.

        ``solver`` is ``"auto"``, ``"dense"``, or ``"sparse"``. ``"auto"``
        uses the dense factorization on small cases and the sparse Cholesky
        path on large ones, the same policy as the CLI.
        """
        return _to_csr(self._inner.ptdf(convention, solver))

    def lodf(self, convention: str = "series", solver: str = "auto"):
        """DC LODF (m×m). ``solver`` as in :meth:`ptdf`."""
        return _to_csr(self._inner.lodf(convention, solver))

    def weighted_laplacian(self, convention: str = "series"):
        """Weighted Laplacian ``L = A diag(b) Aᵀ``."""
        return _to_csr(self._inner.weighted_laplacian(convention))

    def incidence(self, convention: str = "series") -> "Incidence":
        """Signed incidence factorization as an :data:`Incidence` tuple."""
        np = _require("numpy", "matrix")
        a, b, p_shift, branch_of_col = self._inner.incidence(convention)
        return Incidence(
            A=_to_csr(a),
            b=np.asarray(b, dtype=float),
            p_shift=np.asarray(p_shift, dtype=float),
            branch_of_col=np.asarray(branch_of_col, dtype=np.int64),
        )

    def write_gridfm(
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
        """Write the gridfm-datakit Parquet dataset for this case under
        ``<out_dir>/<case>/raw/``.

        Returns a dict with ``dir``, ``files``, ``dropped_zero_impedance``, and
        ``degenerate_cost_gens``. Published wheels include the native writer;
        custom source builds without the Rust ``gridfm`` feature raise
        ``ImportError``. For many perturbed snapshots in one dataset, see
        :func:`write_gridfm_batch`.
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

    def write_pypsa_csv_folder(self, out_dir: Any) -> dict:
        """Write this case as a PyPSA CSV folder.

        The folder contains static PyPSA component CSVs and can be imported with
        ``pypsa.Network().import_from_csv_folder(path)``. Returns a dict with
        ``dir``, ``files``, and fidelity ``warnings``.
        """
        return self._inner.write_pypsa_csv_folder(str(out_dir))

    def to_normalized(self) -> "BalancedNetwork":
        """Return a normalized copy with per unit power and radian angles.

        The result removes out of service elements, preserves source bus IDs,
        and normalizes bus types. It carries no retained source, so
        :meth:`write` serializes the derived model. Raises
        :class:`PowerIODataError` if the network cannot be
        normalized (no reference bus can be chosen, or a non-positive base MVA).
        """
        return BalancedNetwork(self._inner.to_normalized())

    def to_normalized_with_options(
        self,
        *,
        clamp_angle_bounds: bool = False,
        angle_bound_pad: Optional[float] = None,
    ) -> "BalancedNetwork":
        """Return a normalized copy with explicit normalization options.

        ``clamp_angle_bounds=True`` applies the PowerModels angle difference
        bound repair: limits at or beyond ``+/-pi/2`` and zero/zero windows
        become ``[-angle_bound_pad, angle_bound_pad]``. A repair that would
        invert the interval widens to that same window. The default pad is
        1.0472 radians.
        """
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
        ``PD``/``QD``/``GS``/``BS`` columns, the same aggregation
        :meth:`to_matpower` writes. The bus table has no per element status
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
                    b["id"], _PPC_BUS_TYPE.get(b["kind"], 1.0), 0.0, 0.0, 0.0, 0.0,
                    b["area"], b["vm"], b["va"], b["base_kv"], b["zone"],
                    b["vmax"], b["vmin"],
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
                    g["bus"], g["pg"], g["qg"], g["qmax"], g["qmin"], g["vg"],
                    g["mbase"], float(g["in_service"]), g["pmax"], g["pmin"],
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
                    br["from_id"], br["to_id"], br["r"], br["x"], br["b"],
                    br["rate_a"], br["rate_b"], br["rate_c"], br["tap"],
                    br["shift"], float(br["in_service"]), br["angmin"],
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
            gencost = np.zeros(
                (len(costs), 4 + max(len(c["coeffs"]) for c in costs))
            )
            for i, c in enumerate(costs):
                gencost[i, :4] = (
                    c["model"], c["startup"], c["shutdown"], c["ncost"],
                )
                gencost[i, 4:4 + len(c["coeffs"])] = c["coeffs"]
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


def parse_display_file(path: Any, from_: Optional[str] = None) -> DisplayData:
    """Parse a display artifact such as a PowerWorld ``.pwd`` file."""
    return _wrap_display(_powerio.parse_display_file(str(path), from_))


def parse_display_bytes(data: bytes, from_: str) -> DisplayData:
    """Parse display bytes in the named display format ``from_``."""
    return _wrap_display(_powerio.parse_display_bytes(data, from_))


def parse_geo(text: str, name_hint: Optional[str] = None) -> dict[str, Any]:
    """Tolerantly read a geographic sidecar and return its canonical form.

    Accepts headerless buscoords CSV, aliased CSV/JSON records, and GeoJSON
    Point/LineString features. Returns ``{"geojson": <FeatureCollection dict>,
    "warnings": [...]}``; ``name_hint`` (a file name) picks CSV against JSON
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
    module = PioModule.from_str(_ppc_to_matpower_text(ppc), "matpower")
    return module.as_balanced_network()


def convert_file(
    path: Any,
    to: str,
    from_: Optional[str] = None,
    missing_gen_cost: Optional[str] = None,
    default_gen_cost: Optional[str] = None,
    gen_cost_csv: Optional[Any] = None,
    out: Optional[Any] = None,
) -> Conversion:
    r"""Convert a case file to another format through the network model.

    ``to`` / ``from_`` are format names: ``matpower``, ``powermodels-json``,
    ``egret-json``, ``pandapower-json``, ``psse``, ``powerworld``, ``pslf``,
    ``goc3-json``, ``surge-json``, and ``opfdata-json`` (aliases ``m``, ``pm``,
    ``egret``, ``pp``, ``raw``, ``aux``, ``epc``, ``goc3``, ``surge``,
    ``opfdata``, and ``gridopt``). The input format is
    inferred from the file extension unless ``from_`` overrides it. GO Challenge
    3 and OPFData JSON are read only. An OPFData input may be an extracted
    FullTop or N-1 example of any published grid size; its element counts are
    read from the document. PyPSA CSV folders are read with
    ``from_="pypsa-csv"`` and written with
    :meth:`BalancedNetwork.write_pypsa_csv_folder`. Returns a :class:`Conversion` with
    the text and any fidelity warnings. ``out`` writes the text to a file
    exactly as produced; prefer it over ``open(out, "w").write(text)``, whose
    text mode newline translation on Windows doubles the carriage returns of
    a CRLF source echo into ``\r\r\n``, which PSS/E family tools reject.
    """
    text, warnings = _powerio.convert_file(
        str(path),
        to,
        from_,
        missing_gen_cost=missing_gen_cost,
        default_gen_cost=default_gen_cost,
        gen_cost_csv=None if gen_cost_csv is None else str(gen_cost_csv),
        out=None if out is None else str(out),
    )
    return Conversion(text, warnings)


def convert_str(
    text: str,
    to: str,
    from_: str = "matpower",
    missing_gen_cost: Optional[str] = None,
    default_gen_cost: Optional[str] = None,
    gen_cost_csv: Optional[Any] = None,
) -> Conversion:
    """Convert in-memory case ``text`` through the network model without a
    temporary file.

    ``to`` and ``from_`` are format names as in :func:`convert_file`;
    ``from_`` names the input (default ``matpower``). Returns a
    :class:`Conversion` with the converted text and any fidelity warnings.
    """
    out, warnings = _powerio.convert_str(
        text,
        to,
        from_,
        missing_gen_cost=missing_gen_cost,
        default_gen_cost=default_gen_cost,
        gen_cost_csv=None if gen_cost_csv is None else str(gen_cost_csv),
    )
    return Conversion(out, warnings)


def to_format(
    network: BalancedNetwork,
    to: str,
    missing_gen_cost: Optional[str] = None,
    default_gen_cost: Optional[str] = None,
    gen_cost_csv: Optional[Any] = None,
) -> Conversion:
    """Serialize ``network`` to another format."""
    return network.to_format(
        to,
        missing_gen_cost=missing_gen_cost,
        default_gen_cost=default_gen_cost,
        gen_cost_csv=gen_cost_csv,
    )


def to_matpower(network: BalancedNetwork) -> str:
    """Serialize ``network`` to MATPOWER ``.m`` text."""
    return network.to_matpower()


def to_json(network: BalancedNetwork) -> str:
    """Serialize ``network`` to the JSON transport."""
    return network.to_json()


def write_gridfm_batch(
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
    """Write several networks as one gridfm-datakit dataset, row stacked and
    keyed by the ``scenario`` column.

    Each network is one snapshot; the k-th is stamped ``base_scenario + k``. The
    networks must share a base element set: the same bus/branch/gen counts and
    bus id order (otherwise :class:`PowerIODataError` is raised). Load, dispatch,
    branch status, and costs may vary per scenario. Returns the same dict as
    :meth:`BalancedNetwork.write_gridfm`. Published wheels include the native writer;
    custom source builds without the Rust ``gridfm`` feature raise
    ``ImportError``.
    """
    _require_gridfm()
    inners = [c._inner for c in networks]
    return _powerio.write_gridfm_batch(
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


def read_gridfm(dir: Any, scenario: int = 0) -> GridfmRead:
    """Read one scenario of a gridfm-datakit Parquet dataset back into a case.

    The inverse of :meth:`BalancedNetwork.write_gridfm`. ``dir`` is resolved leniently:
    the ``raw/`` directory holding the parquet files, a ``<case>/`` directory with
    a ``raw/`` child, or a parent directory with one ``*/raw/`` child all work.
    ``scenario`` selects one snapshot from a batch (``0``, the base case, by
    default). Returns a :class:`GridfmRead` ``(network, scenario, warnings)``.

    The read recovers bus types, voltages and limits, nodal load and shunt
    totals, generator dispatch and bounds, branch
    ``r/x/b/tap/shift/rate_a`` values, angle limits, and ``baseMVA``. It cannot
    recover source bus IDs, per element load/shunt granularity, piecewise or
    cubic costs, HVDC, or storage;
    what it can't recover is listed in ``warnings``. Published wheels include the
    native reader; custom source builds without the Rust ``gridfm`` feature raise
    ``ImportError``.
    """
    _require_gridfm()
    inner, scen, warnings = _powerio.read_gridfm(str(dir), scenario)
    return GridfmRead(BalancedNetwork(inner), scen, warnings)


def read_gridfm_scenarios(dir: Any) -> "list[GridfmRead]":
    """Read every scenario of a gridfm dataset, one :class:`GridfmRead` per
    scenario id (ascending) over the shared topology, the read side of
    :func:`write_gridfm_batch`.

    Each scenario is rebuilt independently, so two scenarios may differ in branch
    status, bus types, and reference bus. See :func:`read_gridfm` for the lenient
    directory resolution and the fidelity behavior.
    """
    _require_gridfm()
    return [
        GridfmRead(BalancedNetwork(inner), scen, warnings)
        for inner, scen, warnings in _powerio.read_gridfm_scenarios(str(dir))
    ]


from . import dist  # noqa: E402  (needs Conversion defined above)


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
    ``state_inventory``/``select_state``/``export_state``).
    """

    __slots__ = ("module", "kind")

    def __init__(self, module: "PioModule", kind: str) -> None:
        self.module = module
        self.kind = kind

    def __repr__(self) -> str:
        return f"{type(self).__name__}(kind={self.kind!r})"


class TimeSeries(_TypedValue):
    """A balanced network, balanced operating point, or multiconductor
    operating point time series (kind ``*_time_series``)."""


class ScenarioSet(_TypedValue):
    """A balanced network scenario set (kind ``balanced_network_scenario_set``)."""


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
    """A module kind this release of powerio does not wrap in a typed class.

    Reached only for a kind newer than this release recognizes; ``module``
    and ``kind`` still work, so a caller can still inspect and re-export it.
    """


# kind string (PioModule.kind) -> the .value wrapper it reads back as. The two
# network kinds ("balanced_network", "multiconductor_network") are not here:
# PioModule.value special-cases them to the real network handle instead of one
# of these thin wrappers.
_VALUE_CLASSES: "dict[str, type]" = {
    "balanced_network_time_series": TimeSeries,
    "balanced_operating_point_time_series": TimeSeries,
    "multiconductor_operating_point_time_series": TimeSeries,
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


class PioModule:
    """A runtime module handle: one typed value with its common records.

    The stored form is ``.pio.json`` version 1; released 0.9 packages upgrade
    one way on read. Selection returns the existing typed item; export is the
    separate explicit materialization.
    """

    def __init__(self, inner: "_powerio._PioModule"):
        self._inner = inner

    @classmethod
    def from_json(cls, text: str) -> "PioModule":
        """Read stored ``.pio.json`` text."""
        return cls(_powerio._PioModule.from_json(text))

    @classmethod
    def from_file(
        cls,
        path: Any,
        from_: Optional[str] = None,
        *,
        include_root: Optional[Any] = None,
    ) -> "PioModule":
        """Parse a case file into a module of whichever family claims it.

        ``include_root`` widens the acquisition root for formats whose
        includes reference sibling files (OpenDSS redirects above all).
        """
        root = None if include_root is None else str(include_root)
        return cls(_powerio._PioModule.from_file(str(path), from_, root))

    @classmethod
    def from_str(cls, text: str, from_: Optional[str] = None) -> "PioModule":
        """Parse in-memory case text into a module."""
        return cls(_powerio._PioModule.from_str(text, from_))

    @classmethod
    def from_bytes(cls, data: bytes, from_: Optional[str] = None) -> "PioModule":
        """Parse in-memory case bytes into a module. The only in-memory way
        to read a binary format; text formats must be UTF-8."""
        return cls(_powerio._PioModule.from_bytes(data, from_))

    @property
    def value(self) -> Any:
        """The typed value ``kind`` names, as the ordinary Python object for it.

        ``balanced_network`` and ``multiconductor_network`` read back as the
        network handle (:class:`BalancedNetwork` /
        :class:`dist.MulticonductorNetwork`). Every other kind reads back as
        a thin wrapper — :class:`TimeSeries`, :class:`ScenarioSet`, or one of
        the calculation instance/solution classes (:class:`DcPfInstance`,
        :class:`AcOpfSolution`, and so on) — holding this module; a kind this
        release does not recognize reads back as :class:`UnknownValue`. This
        is the ordinary way to read the value :func:`parse` narrowed to with
        ``value_type``.
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

    def to_json(self) -> str:
        """Serialize to the stored version 1 document."""
        return self._inner.to_json()

    @property
    def kind(self) -> str:
        """The value's permanent kind identifier."""
        return self._inner.kind()

    def inspect(self) -> Any:
        """Value inspection and supported operation discovery."""
        return _json.loads(self._inner.inspect_json())

    def diagnostics(self) -> "list[_powerio.Diagnostic]":
        """The module's diagnostics as native :class:`Diagnostic` objects, in
        encounter order."""
        return self._inner.diagnostics()

    def state_inventory(self) -> Any:
        """The typed time or scenario inventory."""
        return _json.loads(self._inner.state_inventory_json())

    def select_state(
        self,
        time_position: Optional[int] = None,
        scenario: Optional[str] = None,
    ) -> Any:
        """Describe the selected existing typed item without materializing.

        ``time_position`` is zero based, the C convention: the first point in
        the series or scenario set is position ``0``.
        """
        return _json.loads(self._inner.select_json(time_position, scenario))

    def export_state(
        self,
        time_position: Optional[int] = None,
        scenario: Optional[str] = None,
    ) -> "PioModule":
        """Export the selected item as an independent static module.

        ``time_position`` is zero based, the C convention: the first point in
        the series or scenario set is position ``0``.
        """
        return PioModule(self._inner.export_selected(time_position, scenario))

    def to_balanced_inspect(self, base_mva: float = 100.0) -> Any:
        """Readiness of the multiconductor value for the balanced lowering."""
        return _json.loads(self._inner.lowering_readiness_json(base_mva))

    def to_balanced(self, base_mva: float = 100.0) -> "PioModule":
        """Lower the multiconductor value to a balanced module."""
        return PioModule(self._inner.lower_to_balanced(base_mva))

    def __repr__(self) -> str:
        return repr(self._inner)


def parse(
    source: Any,
    from_: Optional[str] = None,
    *,
    include_root: Optional[Any] = None,
    value_type: Optional[type] = None,
) -> "PioModule":
    """Parse one source into a module of whichever family claims it.

    ``source`` is a filesystem path (``str`` or path-like) or in-memory
    ``bytes`` (the only way to read a binary format without a file). The
    result is always a :class:`PioModule` carrying the source's typed value;
    ``module.kind`` names it, and ``module.value`` reads the typed value
    (a calculation defining source produces that calculation rather than a
    bare network).

    ``value_type`` is an optional assertion, not a different return: pass
    :class:`BalancedNetwork` or :class:`dist.MulticonductorNetwork` to assert
    the parsed value's kind, raising :class:`ValueError` naming both the
    detected and the requested kind on a mismatch. Either way — assertion
    passed, or ``value_type`` left at its default ``None`` — the call returns
    the :class:`PioModule`; read `.value` to get the typed value.
    ``include_root`` widens the acquisition root for formats whose includes
    reference sibling files.
    """
    if isinstance(source, (bytes, bytearray, memoryview)):
        module = PioModule.from_bytes(bytes(source), from_)
    else:
        module = PioModule.from_file(source, from_, include_root=include_root)
    if value_type is None or value_type is PioModule:
        return module
    if value_type is BalancedNetwork:
        expected = "balanced_network"
    elif value_type is dist.MulticonductorNetwork:
        expected = "multiconductor_network"
    else:
        raise TypeError(
            "value_type must be powerio.PioModule, powerio.BalancedNetwork, or "
            "powerio.dist.MulticonductorNetwork"
        )
    if module.kind != expected:
        raise ValueError(
            f"parsed value has kind {module.kind!r}; value_type="
            f"{value_type.__name__} asserts {expected!r}"
        )
    return module



