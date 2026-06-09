"""powerio: lossless power system case file IO, conversion, and matrices.

Parse MATPOWER, PSS/E, PowerWorld, and PowerModels JSON into one format-neutral
case; write it back byte-exact; convert between formats; and pull the sparse
matrices and graph views solvers need::

    import powerio

    case = powerio.parse("case9.m")          # format inferred from the extension
    print(case.n, case.base_mva)             # 9 100.0
    text = case.write()                      # byte-exact MATPOWER echo
    raw, warnings = powerio.convert("case9.m", "psse")

    B = case.bprime()                        # scipy.sparse, the FDPF B'
    Y = case.ybus()                          # complex csr, G + jB
    G = case.to_networkx()                   # networkx.Graph keyed by bus id

``import powerio`` and parse/write/convert pull in nothing but the interpreter.
The matrix methods need scipy/numpy and the graph view needs networkx; add them
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
    "Case",
    "Incidence",
    "YbusParts",
    "Conversion",
    "PowerIOError",
    "PowerIOParseError",
    "PowerIODataError",
    "parse",
    "parse_str",
    "parse_matpower",
    "parse_matpower_string",
    "write",
    "convert",
    "write_gridfm_batch",
    "__version__",
]

Conversion = namedtuple("Conversion", ["text", "warnings"])
Conversion.__doc__ = """Output of :func:`convert`.

``text`` is the converted file contents; ``warnings`` lists the fields the
target format could not represent (empty for a faithful conversion).
"""

Incidence = namedtuple("Incidence", ["A", "b", "p_shift", "branch_of_col"])
Incidence.__doc__ = """Output of :meth:`Case.incidence`.

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
    "Output of :meth:`Case.ybus_parts`: ``g`` = Re(Y_bus), ``b`` = Im(Y_bus), "
    "each a real csr_matrix. ``Case.ybus()`` returns ``g + 1j*b``."
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

    The gridfm write path pulls arrow + parquet into the native module, so it is
    an opt-in build (the ``powerio[gridfm]`` extra); the default wheel stays
    interpreter-only.
    """
    if not getattr(_powerio, "_has_gridfm", False):
        raise ImportError(
            "powerio was built without the gridfm Parquet surface; install the "
            "gridfm build with `pip install 'powerio[gridfm]'` (or rebuild with "
            "`maturin develop --features gridfm`)."
        )


class Case:
    """A parsed power network case.

    The data attributes (``buses``, ``branches``, ``gens``, ``loads``,
    ``shunts``) and the non-matrix methods (``write``, ``reference_bus_index``,
    ``connectivity_report``, ``write_dcopf_bundle``) delegate to the compiled
    handle; the matrix methods below return ``scipy.sparse`` objects.

    Errors: a bad file path raises the standard ``OSError`` subclass
    (``FileNotFoundError``); a malformed case raises :class:`PowerIOParseError`
    and an unmet builder precondition (no generators, no reference bus) raises
    :class:`PowerIODataError` — both subclass :class:`PowerIOError`, so
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
        return repr(self._inner).replace("PyCase", "Case", 1)

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
        ``degenerate_cost_gens``. Requires the gridfm build of the extension
        (``pip install 'powerio[gridfm]'``); otherwise raises ``ImportError``.
        For many perturbed snapshots in one dataset, see
        :func:`write_gridfm_batch`.
        """
        _require_gridfm()
        return self._inner.write_gridfm(
            str(out_dir), scenario, include_y_bus, include_taps, include_shifts
        )

    def to_normalized(self) -> "Case":
        """A normalized, computation-ready copy of this case: per unit, radians,
        out-of-service filtered, densely reindexed (1-based), bus types
        canonicalized. The original case is unchanged; the result carries no
        retained source, so :meth:`write` serializes the per-unit model rather
        than echoing it. Raises :class:`PowerIODataError` if no reference bus can
        be chosen.
        """
        return Case(self._inner.to_normalized())

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


def parse(path: Any) -> Case:
    """Parse a case file from a path, inferring the format from the extension."""
    return Case(_powerio.parse(str(path)))


def parse_str(text: str, format: str = "matpower") -> Case:
    """Parse a case from in-memory text in the named ``format``."""
    return Case(_powerio.parse_str(text, format))


def parse_matpower(path: Any) -> Case:
    """Parse a MATPOWER ``.m`` case from a file path."""
    return Case(_powerio.parse_matpower(str(path)))


def parse_matpower_string(content: str, name: Optional[str] = None) -> Case:
    """Parse a MATPOWER case from in-memory ``.m`` text; ``name`` overrides the
    parsed case name."""
    return Case(_powerio.parse_matpower_string(content, name))


def write(case: Case) -> str:
    """Serialize ``case`` to MATPOWER ``.m`` (byte-exact echo when it was parsed
    from MATPOWER)."""
    return _powerio.write(case._inner)


def convert(path: Any, to: str, from_: Optional[str] = None) -> Conversion:
    """Convert a case file to another format through the neutral hub.

    ``to`` / ``from_`` are format names: ``matpower``, ``powermodels-json``,
    ``egret-json``, ``psse``, ``powerworld`` (aliases ``m``, ``pm``, ``egret``,
    ``raw``, ``aux``). The input format is inferred from the file extension
    unless ``from_`` overrides it. Returns a :class:`Conversion` with the text
    and any fidelity warnings.
    """
    text, warnings = _powerio.convert(str(path), to, from_)
    return Conversion(text, warnings)


def write_gridfm_batch(
    cases: "list[Case]",
    out_dir: Any,
    base_scenario: int = 0,
    include_y_bus: bool = True,
    include_taps: bool = True,
    include_shifts: bool = True,
) -> dict:
    """Write several cases as one gridfm-datakit dataset, row-stacked and keyed by
    the ``scenario`` column.

    Each case is one snapshot; the k-th is stamped ``base_scenario + k``. The
    cases must share a base element set — the same bus/branch/gen counts and
    bus-id order (otherwise :class:`PowerIODataError` is raised) — but load, dispatch,
    branch status, and costs may vary per scenario. Returns the same dict as
    :meth:`Case.write_gridfm`. Requires the gridfm build
    (``pip install 'powerio[gridfm]'``); otherwise raises ``ImportError``.
    """
    _require_gridfm()
    inners = [c._inner for c in cases]
    return _powerio.write_gridfm_batch(
        inners, str(out_dir), base_scenario, include_y_bus, include_taps, include_shifts
    )
