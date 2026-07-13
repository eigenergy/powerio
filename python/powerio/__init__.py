"""Parse, convert, and project power system data.

Readers produce a format neutral network model. Writers return retained source
bytes where supported or report fields that a target format cannot represent.
Packages, sparse matrices, graphs, and problem instances use the same parsed
data::

    import powerio as pio

    net = pio.parse_file("case9.m")          # format inferred from the extension
    print(net.n_buses, net.base_mva)         # 9 100.0
    text = net.to_matpower()                 # byte-exact MATPOWER echo
    raw, warnings = pio.convert_file("case9.m", "psse")
    pp_json, warnings = pio.convert_file("case9.m", "pandapower-json")
    pypsa_out = net.write_pypsa_csv_folder("case9-pypsa")
    pkg = pio.Package.from_file("goc3_case.json", from_="goc3-json")
    points = pkg.operating_points()

    B = net.bprime()                         # scipy.sparse, MATPOWER Bp
    Y = net.ybus()                           # complex csr, G + jB
    G = net.to_networkx()                    # networkx.Graph keyed by bus id

PyPSA CSV folders carry static network topology. NetCDF and HDF5 time series
are tracked in https://github.com/eigenergy/powerio/issues/107.

GO Challenge 3 JSON is read as a static balanced network using the first
interval. When it is parsed as a ``.pio.json`` package, the full source time
series is exposed as replayable operating points.

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
from ._powerio import PowerIODataError, PowerIOError, PowerIOParseError, __version__

__all__ = [
    "Network",
    "BalancedNetwork",
    "Incidence",
    "YbusParts",
    "Conversion",
    "DisplayData",
    "PwdDisplay",
    "PwdSubstation",
    "DenseNetwork",
    "DenseBranch",
    "DenseGen",
    "DenseDemand",
    "DenseShunt",
    "PowerIOError",
    "PowerIOParseError",
    "PowerIODataError",
    "parse_file",
    "parse_display_file",
    "parse_display_bytes",
    "parse_str",
    "parse_scopf",
    "parse_geo",
    "from_json",
    "convert_file",
    "convert_str",
    "to_format",
    "to_matpower",
    "to_json",
    "to_dense",
    "Package",
    "write_gridfm_batch",
    "read_gridfm",
    "read_gridfm_scenarios",
    "read_pypsa_csv_folder",
    "GridfmRead",
    "dist",
    "__version__",
]

Conversion = namedtuple("Conversion", ["text", "warnings"])
Conversion.__doc__ = """Output of :func:`convert_file`.

``text`` is the converted file contents; ``warnings`` lists the fields the
target format could not represent (empty for a faithful conversion).
"""

GridfmRead = namedtuple("GridfmRead", ["network", "scenario", "warnings"])
GridfmRead.__doc__ = """Output of :func:`read_gridfm` / :func:`read_gridfm_scenarios`.

``network`` is the reconstructed :class:`Network`; ``scenario`` is the source
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
Incidence.__doc__ = """Output of :meth:`Network.incidence`.

Shapes, with ``n`` buses and ``m`` in-service branches:
- ``A``: signed incidence csr_matrix, ``(n, m)``.
- ``b``: branch susceptances, ``(m,)``; ``b[k]`` is column ``k``.
- ``p_shift``: phase-shift injection, ``(n,)`` (all zero unless
  ``convention="matpower"``).
- ``branch_of_col``: column→branch index map, ``(m,)``; ``branch_of_col[k]``
  and ``b[k]`` are co-indexed by incidence column ``k``.
"""

YbusParts = namedtuple("YbusParts", ["g", "b"])
YbusParts.__doc__ = (
    "Output of :meth:`Network.ybus_parts`: ``g`` = Re(Y_bus), ``b`` = Im(Y_bus), "
    "each a real csr_matrix. ``Network.ybus()`` returns ``g + 1j*b``."
)

DenseBranch = namedtuple(
    "DenseBranch", ["from_id", "to_id", "r", "x", "b", "tap", "shift", "in_service"]
)
DenseBranch.__doc__ = """Branch arrays in source order."""

DenseGen = namedtuple("DenseGen", ["bus", "pg", "pmax", "pmin", "in_service"])
DenseGen.__doc__ = """Generator arrays in source order."""

DenseDemand = namedtuple("DenseDemand", ["pd", "qd"])
DenseDemand.__doc__ = """Nodal active and reactive demand arrays in bus order."""

DenseShunt = namedtuple("DenseShunt", ["gs", "bs"])
DenseShunt.__doc__ = """Nodal shunt conductance and susceptance arrays in bus order."""

DenseNetwork = namedtuple(
    "DenseNetwork",
    [
        "n",
        "m",
        "ng",
        "base_mva",
        "bus_ids",
        "branch",
        "gen",
        "demand",
        "shunt",
        "reference_bus",
        "n_components",
        "is_radial",
    ],
)
DenseNetwork.__doc__ = """Copied dense NumPy table export of a parsed :class:`Network`."""


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


class Network:
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

    def __init__(self, inner: "_powerio.PyNetwork"):
        self._inner = inner

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
        # The inner handle's __repr__ already renders the public ``Network(...)``
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
    ) -> tuple["Network", dict[str, Any]]:
        """Apply a geographic sidecar and return ``(placed, report)``.

        ``text`` is any form :func:`parse_geo` accepts; this case is
        unchanged. The report carries ``matched_buses``, ``matched_branches``,
        ``unmatched_features``, and ``notes``. The placed copy drops the
        retained source text, so a same-format write re-serializes.
        """
        inner, report = self._inner.apply_geo_layer(text, name_hint)
        return Network(inner), report

    def acopf_instance(self, units: Optional[str] = None) -> dict[str, Any]:
        """The matrix free AC OPF problem instance as Python data.

        Dense 0-based indices; ``units`` is ``"perunit"`` (default) or
        ``"native"``.
        """
        return _json.loads(self._inner.acopf_json(units))

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

    def to_dense(self) -> DenseNetwork:
        """Dense NumPy arrays for solver and adapter code.

        This allocates new arrays, preserves bus and branch source order, and
        sums loads and shunts per bus to match the Rust indexed analysis view.
        """
        np = _require("numpy", "matrix")
        buses = self._inner.buses
        branches = self._inner.branches
        generators = self._inner.generators
        bus_ids = np.asarray([b["id"] for b in buses], dtype=np.int64)
        id_to_idx = {int(bus_id): idx for idx, bus_id in enumerate(bus_ids)}

        pd = np.zeros(len(buses), dtype=float)
        qd = np.zeros(len(buses), dtype=float)
        for load in self._inner.loads:
            idx = id_to_idx.get(load["bus"])
            if idx is not None:
                pd[idx] += load["p"]
                qd[idx] += load["q"]

        gs = np.zeros(len(buses), dtype=float)
        bs = np.zeros(len(buses), dtype=float)
        for shunt in self._inner.shunts:
            idx = id_to_idx.get(shunt["bus"])
            if idx is not None:
                gs[idx] += shunt["g"]
                bs[idx] += shunt["b"]

        branch = DenseBranch(
            from_id=np.asarray([br["from_id"] for br in branches], dtype=np.int64),
            to_id=np.asarray([br["to_id"] for br in branches], dtype=np.int64),
            r=np.asarray([br["r"] for br in branches], dtype=float),
            x=np.asarray([br["x"] for br in branches], dtype=float),
            b=np.asarray([br["b"] for br in branches], dtype=float),
            tap=np.asarray([br["tap"] for br in branches], dtype=float),
            shift=np.asarray([br["shift"] for br in branches], dtype=float),
            in_service=np.asarray([br["in_service"] for br in branches], dtype=bool),
        )
        gen = DenseGen(
            bus=np.asarray([g["bus"] for g in generators], dtype=np.int64),
            pg=np.asarray([g["pg"] for g in generators], dtype=float),
            pmax=np.asarray([g["pmax"] for g in generators], dtype=float),
            pmin=np.asarray([g["pmin"] for g in generators], dtype=float),
            in_service=np.asarray([g["in_service"] for g in generators], dtype=bool),
        )
        refs = self.reference_bus_indices()
        return DenseNetwork(
            n=len(buses),
            m=len(branches),
            ng=len(generators),
            base_mva=self.base_mva,
            bus_ids=bus_ids,
            branch=branch,
            gen=gen,
            demand=DenseDemand(pd=pd, qd=qd),
            shunt=DenseShunt(gs=gs, bs=bs),
            reference_bus=refs[0] if len(refs) == 1 else None,
            n_components=self.n_connected_components,
            is_radial=self.is_radial,
        )

    # --- matrix builders (scipy.sparse) ---------------------------------

    def bprime(self, scheme: str = "bx"):
        """MATPOWER FDPF Bp matrix. ``scheme`` is ``"bx"`` or ``"xb"``."""
        return _to_csr(self._inner.bprime(scheme))

    def bdoubleprime(self, scheme: str = "bx"):
        """MATPOWER FDPF Bpp matrix. ``scheme`` is ``"bx"`` or ``"xb"``."""
        return _to_csr(self._inner.bdoubleprime(scheme))

    def lacpf(self, include_taps: bool = True, include_shifts: bool = True):
        """LACPF 2n×2n block ``[[G, -B], [-B, -G]]``."""
        return _to_csr(self._inner.lacpf(include_taps, include_shifts))

    def adjacency(self):
        """0/1 bus adjacency matrix."""
        return _to_csr(self._inner.adjacency())

    def ybus_parts(self, include_taps: bool = True, include_shifts: bool = True):
        """:class:`YbusParts` ``(g, b)`` = ``(Re(Y_bus), Im(Y_bus))``, two real
        csr_matrix."""
        g, b = self._inner.ybus_parts(include_taps, include_shifts)
        return YbusParts(g=_to_csr(g), b=_to_csr(b))

    def ybus(self, include_taps: bool = True, include_shifts: bool = True):
        """``Y_bus = G + jB`` as a complex csr_matrix."""
        g, b = self.ybus_parts(include_taps, include_shifts)
        return (g + 1j * b).tocsr()

    def ptdf(self, convention: str = "paper"):
        """DC PTDF (m×n). ``convention`` is ``"paper"`` or ``"matpower"``."""
        return _to_csr(self._inner.ptdf(convention))

    def lodf(self, convention: str = "paper"):
        """DC LODF (m×m)."""
        return _to_csr(self._inner.lodf(convention))

    def weighted_laplacian(self, convention: str = "paper"):
        """Weighted Laplacian ``L = A diag(b) Aᵀ``."""
        return _to_csr(self._inner.weighted_laplacian(convention))

    def incidence(self, convention: str = "paper") -> "Incidence":
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
            scenario,
            include_y_bus,
            include_taps,
            include_shifts,
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

    def to_normalized(self) -> "Network":
        """Return a normalized copy with per unit power and radian angles.

        The result removes out of service elements, preserves source bus IDs,
        and normalizes bus types. It carries no retained source, so
        :meth:`write` serializes the derived model. Raises
        :class:`PowerIODataError` if the network cannot be
        normalized (no reference bus can be chosen, or a non-positive base MVA).
        """
        return Network(self._inner.to_normalized())

    def to_normalized_with_options(
        self,
        clamp_angle_bounds: bool = False,
        angle_bound_pad: Optional[float] = None,
    ) -> "Network":
        """Return a normalized copy with explicit normalization options.

        ``clamp_angle_bounds=True`` applies the PowerModels angle difference
        bound repair: limits at or beyond ``+/-pi/2`` and zero/zero windows
        become ``[-angle_bound_pad, angle_bound_pad]``. A repair that would
        invert the interval widens to that same window. The default pad is
        1.0472 radians.
        """
        return Network(
            self._inner.to_normalized_with_options(
                clamp_angle_bounds, angle_bound_pad
            )
        )

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


# v1 name for the scalar positive sequence model. ``Network`` remains the
# existing Python handle name in 0.4.
BalancedNetwork = Network


def parse_file(path: Any, from_: Optional[str] = None) -> Network:
    """Parse a case file from a path, inferring the format from the extension.

    Read fidelity warnings are on ``Network.read_warnings`` (empty for readers
    that don't report any; currently pandapower JSON, PyPSA CSV, and PSLF EPC
    report them).
    """
    return Network(_powerio.parse_file(str(path), from_))


def parse_display_file(path: Any, from_: Optional[str] = None) -> DisplayData:
    """Parse a display artifact such as a PowerWorld ``.pwd`` file."""
    return _wrap_display(_powerio.parse_display_file(str(path), from_))


def parse_display_bytes(data: bytes, format: str) -> DisplayData:
    """Parse display bytes in the named display format."""
    return _wrap_display(_powerio.parse_display_bytes(data, format))


def parse_str(text: str, format: str = "matpower") -> Network:
    """Parse a case from in-memory text in the named ``format``."""
    return Network(_powerio.parse_str(text, format))


def parse_scopf(text: str, from_: str = "goc3-json") -> dict[str, Any]:
    """Return a versioned SCOPF problem instance document.

    ``from_`` currently accepts ``"goc3-json"``. The returned dictionary uses
    the wire schema's declared 1-based indices and retains source identities in
    separate fields. Parse and assembly failures raise :class:`PowerIOError`.
    """
    return _json.loads(_powerio.parse_scopf(text, from_))


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


def from_json(text: str) -> Network:
    """Rebuild a case from JSON produced by :meth:`Network.to_json`."""
    return Network(_powerio.from_json(text))


def convert_file(
    path: Any,
    to: str,
    from_: Optional[str] = None,
    missing_gen_cost: Optional[str] = None,
    default_gen_cost: Optional[str] = None,
    gen_cost_csv: Optional[Any] = None,
) -> Conversion:
    """Convert a case file to another format through the network model.

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
    :meth:`Network.write_pypsa_csv_folder`. Returns a :class:`Conversion` with
    the text and any fidelity warnings.
    """
    text, warnings = _powerio.convert_file(
        str(path),
        to,
        from_,
        missing_gen_cost=missing_gen_cost,
        default_gen_cost=default_gen_cost,
        gen_cost_csv=None if gen_cost_csv is None else str(gen_cost_csv),
    )
    return Conversion(text, warnings)


def convert_str(
    text: str,
    to: str,
    format: str = "matpower",
    missing_gen_cost: Optional[str] = None,
    default_gen_cost: Optional[str] = None,
    gen_cost_csv: Optional[Any] = None,
) -> Conversion:
    """Convert in-memory case ``text`` through the network model without a
    temporary file.

    ``to`` and ``format`` are format names as in :func:`convert_file`;
    ``format`` names the input (default ``matpower``). Returns a
    :class:`Conversion` with the converted text and any fidelity warnings.
    """
    out, warnings = _powerio.convert_str(
        text,
        to,
        format,
        missing_gen_cost=missing_gen_cost,
        default_gen_cost=default_gen_cost,
        gen_cost_csv=None if gen_cost_csv is None else str(gen_cost_csv),
    )
    return Conversion(out, warnings)


def to_format(
    network: Network,
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


def to_matpower(network: Network) -> str:
    """Serialize ``network`` to MATPOWER ``.m`` text."""
    return network.to_matpower()


def to_json(network: Network) -> str:
    """Serialize ``network`` to the JSON transport."""
    return network.to_json()


def to_dense(network: Network) -> DenseNetwork:
    """Return copied dense NumPy tables for ``network``."""
    return network.to_dense()


def write_gridfm_batch(
    networks: "list[Network]",
    out_dir: Any,
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
    :meth:`Network.write_gridfm`. Published wheels include the native writer;
    custom source builds without the Rust ``gridfm`` feature raise
    ``ImportError``.
    """
    _require_gridfm()
    inners = [c._inner for c in networks]
    return _powerio.write_gridfm_batch(
        inners,
        str(out_dir),
        base_scenario,
        include_y_bus,
        include_taps,
        include_shifts,
        missing_gen_cost=missing_gen_cost,
        default_gen_cost=default_gen_cost,
        gen_cost_csv=None if gen_cost_csv is None else str(gen_cost_csv),
    )


def read_gridfm(dir: Any, scenario: int = 0) -> GridfmRead:
    """Read one scenario of a gridfm-datakit Parquet dataset back into a case.

    The inverse of :meth:`Network.write_gridfm`. ``dir`` is resolved leniently:
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
    return GridfmRead(Network(inner), scen, warnings)


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
        GridfmRead(Network(inner), scen, warnings)
        for inner, scen, warnings in _powerio.read_gridfm_scenarios(str(dir))
    ]


def read_pypsa_csv_folder(path: Any) -> Network:
    """Read a PyPSA CSV folder into a :class:`Network`."""
    return Network(_powerio.read_pypsa_csv_folder(str(path)))


from . import dist  # noqa: E402  (needs Conversion defined above)


class Package:
    """A parsed ``.pio.json`` package.

    Parsing occurs once; every accessor reuses the native handle.
    """

    def __init__(self, inner: "_powerio._Package"):
        self._inner = inner

    @classmethod
    def from_file(
        cls, path: Any, from_: Optional[str] = None, scenario: int = 0
    ) -> "Package":
        """Build a package from a case file or folder."""
        return cls(_powerio._Package.from_file(str(path), from_, scenario))

    @classmethod
    def from_str(cls, text: str, from_: Optional[str] = None) -> "Package":
        """Build a package from in-memory case text."""
        return cls(_powerio._Package.from_str(text, from_))

    @classmethod
    def from_json(cls, text: str) -> "Package":
        """Parse a ``.pio.json`` document."""
        return cls(_powerio._Package.from_json(text))

    @classmethod
    def from_balanced(
        cls, network: Network, include_solver_metadata: bool = False
    ) -> "Package":
        """Wrap a balanced :class:`Network` in a package."""
        return cls(
            _powerio._Package.from_balanced(network._inner, include_solver_metadata)
        )

    @classmethod
    def from_multiconductor(cls, network: "dist.MulticonductorNetwork") -> "Package":
        """Wrap a multiconductor network in a package."""
        return cls(_powerio._Package.from_multiconductor(network._inner))

    @property
    def model_kind(self) -> str:
        """``"balanced"`` or ``"multiconductor"``."""
        return self._inner.model_kind()

    def to_json(self) -> str:
        """Serialize to pretty ``.pio.json``."""
        return self._inner.to_json()

    def as_balanced(self) -> Network:
        """Return the balanced payload as a :class:`Network`."""
        return Network(self._inner.as_balanced())

    def as_multiconductor(self) -> "dist.MulticonductorNetwork":
        """Return the multiconductor payload."""
        return dist.MulticonductorNetwork(self._inner.as_multiconductor())

    def operating_points(self) -> Any:
        """The operating point series as Python data, or ``None``.

        GOC3 packages populate this from the source time series. Each point is
        a set of field updates over the package's static payload.
        """
        return _json.loads(self._inner.operating_points_json())

    def set_operating_points(self, points: Any) -> None:
        """Replace the operating point series and rerun package validation.

        ``None`` or an empty series clears it.
        """
        self._inner.set_operating_points_json(_json.dumps(points))

    def study(self) -> Any:
        """The study block as Python data, or ``None``."""
        return _json.loads(self._inner.study_json())

    def materialize_operating_point(self, index: int) -> "Package":
        """Materialize one operating point into a new static package."""
        return Package(self._inner.materialize_operating_point(index))

    def materialize_study_commit(self, index: int) -> "Package":
        """Materialize one study commit into a new static package."""
        return Package(self._inner.materialize_study_commit(index))

    def validate(self) -> None:
        """Run the package semantic validation profile in place."""
        self._inner.validate()

    def validation(self) -> Any:
        """The validation summary as Python data."""
        return _json.loads(self._inner.validation_json())

    def diagnostics(self) -> Any:
        """The structured diagnostics as a list of Python dicts."""
        return _json.loads(self._inner.diagnostics_json())

    def multiconductor_to_balanced_preflight(self, base_mva: float = 100.0) -> Any:
        """Readiness report for multiconductor to balanced lowering."""
        return _json.loads(
            self._inner.multiconductor_to_balanced_preflight_json(base_mva)
        )

    def lower_multiconductor_to_balanced(self, base_mva: float = 100.0) -> "Package":
        """Lower a multiconductor package to a new balanced package."""
        return Package(self._inner.lower_multiconductor_to_balanced(base_mva))

    def __repr__(self) -> str:
        return repr(self._inner)
