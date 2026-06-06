from typing import Any, Dict, List, Literal, NamedTuple, Optional, Tuple, TypedDict

__version__: str

Scheme = Literal["bx", "xb"]
Convention = Literal["paper", "matpower"]
Units = Literal["perunit", "native"]

class CasematError(Exception): ...

class GenCost(TypedDict):
    model: int
    startup: float
    shutdown: float
    ncost: int
    coeffs: List[float]

class Bus(TypedDict):
    id: int
    type: Literal["PQ", "PV", "REF", "ISOLATED"]
    pd: float
    qd: float
    gs: float
    bs: float
    area: int
    vm: float
    va: float
    base_kv: float
    zone: int
    vmax: float
    vmin: float

class Branch(TypedDict):
    from_id: int
    to_id: int
    r: float
    x: float
    b: float
    rate_a: float
    rate_b: float
    rate_c: float
    tap: float
    shift: float
    status: float
    angmin: float
    angmax: float

class Gen(TypedDict):
    bus_id: int
    pg: float
    qg: float
    qmax: float
    qmin: float
    vg: float
    mbase: float
    status: float
    pmax: float
    pmin: float
    cost: Optional[GenCost]

class Incidence(NamedTuple):
    A: Any  # scipy.sparse.csr_matrix, (n, m)
    b: Any  # numpy.ndarray, (m,)
    p_shift: Any  # numpy.ndarray, (n,)
    branch_of_col: Any  # numpy.ndarray, (m,)

class YbusParts(NamedTuple):
    g: Any  # scipy.sparse.csr_matrix, Re(Y_bus)
    b: Any  # scipy.sparse.csr_matrix, Im(Y_bus)

class Case:
    name: str
    base_mva: float
    n: int
    n_branches: int
    n_gens: int
    is_radial: bool
    n_connected_components: int
    buses: List[Bus]
    branches: List[Branch]
    gens: List[Gen]
    def reference_bus_index(self) -> int: ...
    def connectivity_report(self) -> Dict[str, Any]: ...
    def bprime(self, scheme: Scheme = ...) -> Any: ...
    def bdoubleprime(self, scheme: Scheme = ...) -> Any: ...
    def lacpf(self, include_taps: bool = ..., include_shifts: bool = ...) -> Any: ...
    def adjacency(self) -> Any: ...
    def ybus_parts(
        self, include_taps: bool = ..., include_shifts: bool = ...
    ) -> YbusParts: ...
    def ybus(self, include_taps: bool = ..., include_shifts: bool = ...) -> Any: ...
    def ptdf(self, convention: Convention = ...) -> Any: ...
    def lodf(self, convention: Convention = ...) -> Any: ...
    def weighted_laplacian(self, convention: Convention = ...) -> Any: ...
    def incidence(self, convention: Convention = ...) -> Incidence: ...
    def to_networkx(self) -> Any: ...
    def write_dcopf_bundle(
        self, out_dir: str, convention: Convention = ..., units: Units = ...
    ) -> Dict[str, Any]: ...

class Conversion(NamedTuple):
    text: str
    warnings: List[str]

Format = Literal["matpower", "powermodels-json", "egret-json", "psse", "powerworld"]

def parse_matpower(path: Any) -> Case: ...
def parse_matpower_string(content: str, name: Optional[str] = ...) -> Case: ...
def convert(path: Any, to: Format, from_: Optional[Format] = ...) -> Conversion: ...
