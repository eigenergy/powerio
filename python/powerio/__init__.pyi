from typing import Any, Dict, List, Literal, NamedTuple, Optional, TypedDict

__version__: str

Scheme = Literal["bx", "xb"]
Convention = Literal["paper", "matpower"]
Units = Literal["perunit", "native"]

class PowerIOError(Exception):
    """Base error from the powerio parser, converter, or matrix builders."""

class PowerIOParseError(PowerIOError):
    """A case file is malformed or unparseable."""

class PowerIODataError(PowerIOError):
    """A well-formed case cannot satisfy a requested operation."""

class GenCost(TypedDict):
    model: int
    startup: float
    shutdown: float
    ncost: int
    coeffs: List[float]

class Bus(TypedDict):
    id: int
    type: Literal["PQ", "PV", "REF", "ISOLATED"]
    vm: float
    va: float
    base_kv: float
    area: int
    zone: int
    vmax: float
    vmin: float

class Load(TypedDict):
    bus: int
    p: float
    q: float
    in_service: bool

class Shunt(TypedDict):
    bus: int
    g: float
    b: float
    in_service: bool

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
    in_service: bool
    angmin: float
    angmax: float

class Gen(TypedDict):
    bus: int
    pg: float
    qg: float
    pmax: float
    pmin: float
    qmax: float
    qmin: float
    vg: float
    mbase: float
    in_service: bool
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
    # Data attributes and the non-matrix methods delegate to the compiled
    # `_powerio.PyCase` handle at runtime via `Case.__getattr__`.
    name: str
    base_mva: float
    source_format: str
    n: int
    n_branches: int
    n_gens: int
    n_loads: int
    n_shunts: int
    is_radial: bool
    n_connected_components: int
    buses: List[Bus]
    loads: List[Load]
    shunts: List[Shunt]
    branches: List[Branch]
    gens: List[Gen]
    def reference_bus_index(self) -> int: ...
    def connectivity_report(self) -> Dict[str, Any]: ...
    def write(self) -> str: ...
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

# Any reader/writer name or alias the Rust hub accepts (e.g. "matpower"/"m",
# "psse"/"raw"). Kept as `str` so aliases type-check; the binding validates it.
Format = str

def parse(path: Any) -> Case: ...
def parse_str(text: str, format: Format = ...) -> Case: ...
def parse_matpower(path: Any) -> Case: ...
def parse_matpower_string(content: str, name: Optional[str] = ...) -> Case: ...
def write(case: Case) -> str: ...
def convert(path: Any, to: Format, from_: Optional[Format] = ...) -> Conversion: ...
