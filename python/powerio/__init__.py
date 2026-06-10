"""powerio: lossless power system case file IO, conversion, and matrices.

Parse MATPOWER, PSS/E, PowerWorld, PowerModels JSON, and egret JSON into one
format-neutral case; write it back byte exact; convert between formats; and
pull the sparse matrices and graph views solvers need::

    import powerio as pio

    net = pio.parse_file("case9.m")          # format inferred from the extension
    print(net.n_buses, net.base_mva)         # 9 100.0
    text = net.to_matpower()                 # byte-exact MATPOWER echo
    raw, warnings = pio.convert_file("case9.m", "psse")

    B = net.bprime()                         # scipy.sparse, the FDPF B'
    Y = net.ybus()                           # complex csr, G + jB
    G = net.to_networkx()                    # networkx.Graph keyed by bus id

``import powerio`` and parsing/writing/converting pull in nothing but the
interpreter. The matrix methods need scipy/numpy and the graph view needs networkx; add them
with ``pip install 'powerio[matrix]'``, ``[graph]``, or ``[all]``. A missing
extra raises a clear ImportError, never a link error: the compiled core
(``powerio._powerio``) returns COO triplets as plain Python lists, and the
wrappers here assemble scipy matrices and networkx graphs lazily.
"""

from __future__ import annotations

import importlib
from collections import namedtuple
from typing import Any, Optional

from . import _powerio
from ._powerio import PowerIODataError, PowerIOError, PowerIOParseError, __version__

__all__ = [
    "Network",
    "Case",
    "Incidence",
    "YbusParts",
    "Conversion",
    "DenseNetwork",
    "DenseBranch",
    "DenseGen",
    "DenseDemand",
    "DenseShunt",
    "PowerIOError",
    "PowerIOParseError",
    "PowerIODataError",
    "parse_file",
    "parse_str",
    "from_json",
    "convert_file",
    "to_format",
    "to_matpower",
    "to_json",
    "to_dense",
    "write_gridfm_batch",
    "dist",
    "__version__",
]

Conversion = namedtuple("Conversion", ["text", "warnings"])
Conversion.__doc__ = """Output of :func:`convert_file`.

``text`` is the converted file contents; ``warnings`` lists the fields the
target format could not represent (empty for a faithful conversion).
"""

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
DenseNetwork.__doc__ = """Dense NumPy table view of a parsed :class:`Network`."""


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


class Network:
    """A parsed power network case.

    The data attributes (``buses``, ``branches``, ``gens``, ``loads``,
    ``shunts``) and the non-matrix methods (``write``, ``reference_bus_index``,
    ``connectivity_report``, ``write_dcopf_bundle``) delegate to the compiled
    handle; the matrix methods below return ``scipy.sparse`` objects.

    Errors: a bad file path raises the standard ``OSError`` subclass
    (``FileNotFoundError``); a malformed case raises :class:`PowerIOParseError`
    and an unmet builder precondition (no generators, no reference bus) raises
    :class:`PowerIODataError`; both subclass :class:`PowerIOError`, so
    ``except PowerIOError`` catches either; an unknown
    ``scheme``/``convention``/``units`` string raises ``ValueError``.
    """

    def __init__(self, inner: "_powerio.PyCase"):
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
        return repr(self._inner).replace("PyCase", "Network", 1)

    # --- canonical format and table views -------------------------------

    def to_matpower(self) -> str:
        """Serialize to MATPOWER ``.m`` text.

        A case parsed from MATPOWER keeps its original source, so this returns a
        byte-exact echo. Derived cases serialize from the format-neutral model.
        """
        return self._inner.to_matpower()

    def to_json(self) -> str:
        """Serialize to the JSON transport."""
        return self._inner.to_json()

    def to_format(self, to: str) -> Conversion:
        """Serialize this parsed case to another format.

        ``to`` is one of the format names accepted by :func:`convert_file`.
        Returns a :class:`Conversion` with output text and fidelity warnings.
        """
        text, warnings = self._inner.to_format(to)
        return Conversion(text, warnings)

    def to_dense(self) -> DenseNetwork:
        """Dense NumPy arrays for solver and adapter code.

        This view preserves bus and branch source order. Loads and shunts are
        summed per bus, matching the Rust indexed analysis view.
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
        """FDPF B' (shuntless). ``scheme`` is ``"bx"`` or ``"xb"``."""
        return _to_csr(self._inner.bprime(scheme))

    def bdoubleprime(self, scheme: str = "bx"):
        """FDPF B'' (with shunts and taps; shifts zeroed). ``scheme`` is
        ``"bx"`` or ``"xb"``; taps are always kept (MATPOWER ``makeB``)."""
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
            str(out_dir), scenario, include_y_bus, include_taps, include_shifts
        )

    def to_normalized(self) -> "Network":
        """A normalized, computation-ready copy of this case: per unit, radians,
        out-of-service filtered, densely reindexed (1-based), bus types
        canonicalized. The original case is unchanged; the result carries no
        retained source, so :meth:`write` serializes the per-unit model rather
        than echoing it. Raises :class:`PowerIODataError` if the case can't be
        normalized (no reference bus can be chosen, or a non-positive base MVA).
        """
        return Network(self._inner.to_normalized())

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


Case = Network


def parse_file(path: Any, from_: Optional[str] = None) -> Network:
    """Parse a case file from a path, inferring the format from the extension."""
    return Network(_powerio.parse_file(str(path), from_))


def parse_str(text: str, format: str = "matpower") -> Network:
    """Parse a case from in-memory text in the named ``format``."""
    return Network(_powerio.parse_str(text, format))


def from_json(text: str) -> Network:
    """Rebuild a case from JSON produced by :meth:`Network.to_json`."""
    return Network(_powerio.from_json(text))


def convert_file(path: Any, to: str, from_: Optional[str] = None) -> Conversion:
    """Convert a case file to another format through the neutral hub.

    ``to`` / ``from_`` are format names: ``matpower``, ``powermodels-json``,
    ``egret-json``, ``psse``, ``powerworld`` (aliases ``m``, ``pm``, ``egret``,
    ``raw``, ``aux``). The input format is inferred from the file extension
    unless ``from_`` overrides it. Returns a :class:`Conversion` with the text
    and any fidelity warnings.
    """
    text, warnings = _powerio.convert_file(str(path), to, from_)
    return Conversion(text, warnings)


def to_format(case: Network, to: str) -> Conversion:
    """Serialize ``case`` to another format."""
    return case.to_format(to)


def to_matpower(case: Network) -> str:
    """Serialize ``case`` to MATPOWER ``.m`` text."""
    return case.to_matpower()


def to_json(case: Network) -> str:
    """Serialize ``case`` to the JSON transport."""
    return case.to_json()


def to_dense(case: Network) -> DenseNetwork:
    """Return the dense NumPy table view of ``case``."""
    return case.to_dense()


def write_gridfm_batch(
    cases: "list[Network]",
    out_dir: Any,
    base_scenario: int = 0,
    include_y_bus: bool = True,
    include_taps: bool = True,
    include_shifts: bool = True,
) -> dict:
    """Write several cases as one gridfm-datakit dataset, row-stacked and keyed by
    the ``scenario`` column.

    Each case is one snapshot; the k-th is stamped ``base_scenario + k``. The
    cases must share a base element set: the same bus/branch/gen counts and
    bus id order (otherwise :class:`PowerIODataError` is raised). Load, dispatch,
    branch status, and costs may vary per scenario. Returns the same dict as
    :meth:`Network.write_gridfm`. Published wheels include the native writer;
    custom source builds without the Rust ``gridfm`` feature raise
    ``ImportError``.
    """
    _require_gridfm()
    inners = [c._inner for c in cases]
    return _powerio.write_gridfm_batch(
        inners, str(out_dir), base_scenario, include_y_bus, include_taps, include_shifts
    )


from . import dist  # noqa: E402  (needs Conversion defined above)
