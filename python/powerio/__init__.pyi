from typing import Any, Dict, List, Literal, NamedTuple, Optional, TypedDict

__version__: str

Scheme = Literal["bx", "xb"]
Convention = Literal["paper", "matpower"]
Units = Literal["perunit", "native"]
GridfmOutputs = Dict[str, Any]

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
    kind: Literal["PQ", "PV", "REF", "ISOLATED"]
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

class GridfmRead(NamedTuple):
    network: "Network"
    scenario: int
    warnings: List[str]

class DenseBranch(NamedTuple):
    from_id: Any  # numpy.ndarray
    to_id: Any  # numpy.ndarray
    r: Any  # numpy.ndarray
    x: Any  # numpy.ndarray
    b: Any  # numpy.ndarray
    tap: Any  # numpy.ndarray
    shift: Any  # numpy.ndarray
    in_service: Any  # numpy.ndarray

class DenseGen(NamedTuple):
    bus: Any  # numpy.ndarray
    pg: Any  # numpy.ndarray
    pmax: Any  # numpy.ndarray
    pmin: Any  # numpy.ndarray
    in_service: Any  # numpy.ndarray

class DenseDemand(NamedTuple):
    pd: Any  # numpy.ndarray
    qd: Any  # numpy.ndarray

class DenseShunt(NamedTuple):
    gs: Any  # numpy.ndarray
    bs: Any  # numpy.ndarray

class DenseNetwork(NamedTuple):
    n: int
    m: int
    ng: int
    base_mva: float
    bus_ids: Any  # numpy.ndarray
    branch: DenseBranch
    gen: DenseGen
    demand: DenseDemand
    shunt: DenseShunt
    reference_bus: Optional[int]
    n_components: int
    is_radial: bool

class Network:
    # Data attributes and the non-matrix methods delegate to the compiled
    # `_powerio.PyCase` handle at runtime via `Network.__getattr__`.
    name: str
    base_mva: float
    source_format: Literal[
        "Matpower",
        "PowerModelsJson",
        "EgretJson",
        "Psse",
        "PowerWorld",
        "PandapowerJson",
        "PowerWorldBinary",
        "InMemory",
        "Normalized",
        "Gridfm",
        "PypsaCsv",
    ]
    read_warnings: List[str]
    n_buses: int
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
    generators: List[Gen]
    def reference_bus_index(self) -> int: ...
    def reference_bus_indices(self) -> List[int]: ...
    def connectivity_report(self) -> Dict[str, Any]: ...
    def to_matpower(self) -> str: ...
    def to_json(self) -> str: ...
    def to_format(self, to: str) -> "Conversion": ...
    def to_dense(self) -> DenseNetwork: ...
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
    def write_gridfm(
        self,
        out_dir: Any,
        scenario: int = ...,
        include_y_bus: bool = ...,
        include_taps: bool = ...,
        include_shifts: bool = ...,
    ) -> GridfmOutputs: ...
    def write_pypsa_csv_folder(self, out_dir: Any) -> Dict[str, Any]: ...
    def to_normalized(self) -> "Network": ...
    def to_networkx(self) -> Any: ...
    def write_dcopf_bundle(
        self, out_dir: str, convention: Convention = ..., units: Units = ...
    ) -> Dict[str, Any]: ...

Case = Network

class Conversion(NamedTuple):
    text: str
    warnings: List[str]

# Any reader/writer name or alias the Rust hub accepts (e.g. "matpower"/"m",
# "psse"/"raw"). Kept as `str` so aliases type-check; the binding validates it.
Format = str

def parse_file(path: Any, from_: Optional[Format] = ...) -> Network: ...
def parse_str(text: str, format: Format = ...) -> Network: ...
def from_json(text: str) -> Network: ...
def convert_file(path: Any, to: Format, from_: Optional[Format] = ...) -> Conversion: ...
def convert_str(text: str, to: Format, format: Format = ...) -> Conversion: ...
def to_format(case: Network, to: Format) -> Conversion: ...
def to_matpower(case: Network) -> str: ...
def to_json(case: Network) -> str: ...
def to_dense(case: Network) -> DenseNetwork: ...
def write_gridfm_batch(
    cases: List[Network],
    out_dir: Any,
    base_scenario: int = ...,
    include_y_bus: bool = ...,
    include_taps: bool = ...,
    include_shifts: bool = ...,
) -> GridfmOutputs: ...
def read_gridfm(dir: Any, scenario: int = ...) -> GridfmRead: ...
def read_gridfm_scenarios(dir: Any) -> List[GridfmRead]: ...
def read_pypsa_csv_folder(path: Any) -> Network: ...
