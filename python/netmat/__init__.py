"""netmat: power network case files into sparse matrices and graph views.

Parse a MATPOWER ``.m`` case, then pull matrices as ``scipy.sparse`` or a
networkx graph::

    import netmat as nm

    case = nm.parse_matpower("case9.m")
    B = case.bprime()        # scipy.sparse.csr_matrix, the FDPF B'
    Y = case.ybus()          # complex csr_matrix, G + jB
    G = case.to_networkx()   # networkx.Graph keyed by MATPOWER bus id

The compiled core (``netmat._netmat``) returns COO triplets of numpy arrays;
the wrappers here assemble them into scipy matrices, so a missing scipy or
networkx surfaces as a clear ImportError rather than a link error.
"""

from __future__ import annotations

import importlib
from collections import namedtuple
from typing import Any, List, Optional

from . import _netmat
from ._netmat import NetmatError, __version__

__all__ = [
    "Case",
    "Incidence",
    "YbusParts",
    "NetmatError",
    "parse_matpower",
    "parse_matpower_string",
    "__version__",
]

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
    try:
        return importlib.import_module(module)
    except ImportError as exc:
        # Only rewrite "module is absent". A present-but-broken install (e.g. a
        # failed C-extension load) raises ImportError from a sub-import; let its
        # own traceback through instead of misdirecting the user to reinstall.
        if getattr(exc, "name", None) not in (module, module.split(".")[0]):
            raise
        raise ImportError(
            f"netmat needs {module!r} for this call; install it with "
            f"`pip install 'netmat[{extra}]'` or `pip install {extra}`"
        ) from exc


def _to_csr(coo):
    """Assemble a ``(data, row, col, shape)`` COO tuple into a csr_matrix."""
    sparse = _require("scipy.sparse", "scipy")
    data, row, col, shape = coo
    return sparse.coo_matrix((data, (row, col)), shape=shape).tocsr()


class Case:
    """A parsed MATPOWER case.

    The data attributes and the non-matrix methods (``reference_bus_index``,
    ``connectivity_report``, ``write_dcopf_bundle``) delegate to the compiled
    handle; the matrix methods below return ``scipy.sparse`` objects.

    Errors: a bad file path raises the standard ``OSError`` subclass
    (``FileNotFoundError``); malformed cases and unmet builder preconditions
    (no generators, no reference bus) raise :class:`NetmatError`; an unknown
    ``scheme``/``convention``/``units`` string raises ``ValueError``.
    """

    def __init__(self, inner: "_netmat.PyCase"):
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
        a, b, p_shift, branch_of_col = self._inner.incidence(convention)
        return Incidence(
            A=_to_csr(a), b=b, p_shift=p_shift, branch_of_col=branch_of_col
        )

    def to_networkx(self):
        """Undirected networkx graph keyed by MATPOWER bus id.

        In-service branches become edges carrying ``branch`` (index), ``r``,
        ``x``, and ``b``.
        """
        nx = _require("networkx", "networkx")
        g = nx.Graph()
        g.add_nodes_from(bus["id"] for bus in self._inner.buses)
        for k, br in enumerate(self._inner.branches):
            if br["status"] == 1.0:
                g.add_edge(
                    br["from_id"],
                    br["to_id"],
                    branch=k,
                    r=br["r"],
                    x=br["x"],
                    b=br["b"],
                )
        return g


def parse_matpower(path: Any) -> Case:
    """Parse a MATPOWER ``.m`` case from a file path."""
    return Case(_netmat.parse_matpower(str(path)))


def parse_matpower_string(content: str, name: Optional[str] = None) -> Case:
    """Parse a MATPOWER case from in-memory ``.m`` text."""
    return Case(_netmat.parse_matpower_string(content, name))
