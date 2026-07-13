from typing import Any, Dict, List, Literal, NamedTuple, Optional, Tuple, TypedDict

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
    uid: Optional[str]

class Load(TypedDict):
    bus: int
    p: float
    q: float
    in_service: bool
    uid: Optional[str]

class Shunt(TypedDict):
    bus: int
    g: float
    b: float
    in_service: bool
    uid: Optional[str]

class BranchRatingSet(TypedDict):
    name: str
    rate_mva: float

class Branch(TypedDict):
    from_id: int
    to_id: int
    r: float
    x: float
    b: float
    rate_a: float
    rate_b: float
    rate_c: float
    rating_sets: List[BranchRatingSet]
    tap: float
    shift: float
    in_service: bool
    angmin: float
    angmax: float
    uid: Optional[str]

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
    uid: Optional[str]

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

class PwdSubstation(NamedTuple):
    number: int
    name: str
    x: float
    y: float

class PwdDisplay(NamedTuple):
    canvas_width: int
    canvas_height: int
    stamp: int
    substations: List[PwdSubstation]

class DisplayData(NamedTuple):
    kind: Literal["powerworld"]
    data: PwdDisplay

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
    # `_powerio.PyNetwork` handle at runtime via `Network.__getattr__`.
    name: str
    base_mva: float
    source_format: Literal[
        "Matpower",
        "PowerModelsJson",
        "OpfDataJson",
        "EgretJson",
        "Psse",
        "PowerWorld",
        "PandapowerJson",
        "Pslf",
        "PowerWorldBinary",
        "InMemory",
        "Normalized",
        "Gridfm",
        "PypsaCsv",
        "Goc3Json",
        "SurgeJson",
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
    def geo_layer(self) -> Dict[str, Any]: ...
    def apply_geo_layer(
        self, text: str, name_hint: Optional[str] = ...
    ) -> Tuple["Network", Dict[str, Any]]: ...
    def acopf_instance(self, units: Optional[str] = ...) -> Dict[str, Any]: ...
    def to_format(
        self,
        to: str,
        missing_gen_cost: Optional[str] = ...,
        default_gen_cost: Optional[str] = ...,
        gen_cost_csv: Optional[Any] = ...,
    ) -> "Conversion": ...
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
        missing_gen_cost: Optional[str] = ...,
        default_gen_cost: Optional[str] = ...,
        gen_cost_csv: Optional[Any] = ...,
    ) -> GridfmOutputs: ...
    def write_pypsa_csv_folder(self, out_dir: Any) -> Dict[str, Any]: ...
    def to_normalized(self) -> "Network": ...
    def to_normalized_with_options(
        self,
        clamp_angle_bounds: bool = ...,
        angle_bound_pad: Optional[float] = ...,
    ) -> "Network": ...
    def to_networkx(self) -> Any: ...
    def write_dcopf_bundle(
        self,
        out_dir: str,
        convention: Convention = ...,
        units: Units = ...,
        missing_gen_cost: Optional[str] = ...,
        default_gen_cost: Optional[str] = ...,
        gen_cost_csv: Optional[Any] = ...,
    ) -> Dict[str, Any]: ...

BalancedNetwork = Network

class Conversion(NamedTuple):
    text: str
    warnings: List[str]

# Any reader/writer name or alias the Rust hub accepts (e.g. "matpower"/"m",
# "psse"/"raw"). Kept as `str` so aliases type-check; the binding validates it.
Format = str

from . import dist as dist

def parse_file(path: Any, from_: Optional[Format] = ...) -> Network: ...
def parse_display_file(path: Any, from_: Optional[Format] = ...) -> DisplayData: ...
def parse_display_bytes(data: bytes, format: Format) -> DisplayData: ...
def parse_str(text: str, format: Format = ...) -> Network: ...
def parse_scopf(text: str, from_: Format = ...) -> Dict[str, Any]: ...
def parse_geo(text: str, name_hint: Optional[str] = ...) -> Dict[str, Any]: ...
def from_json(text: str) -> Network: ...
def convert_file(
    path: Any,
    to: Format,
    from_: Optional[Format] = ...,
    missing_gen_cost: Optional[str] = ...,
    default_gen_cost: Optional[str] = ...,
    gen_cost_csv: Optional[Any] = ...,
) -> Conversion: ...
def convert_str(
    text: str,
    to: Format,
    format: Format = ...,
    missing_gen_cost: Optional[str] = ...,
    default_gen_cost: Optional[str] = ...,
    gen_cost_csv: Optional[Any] = ...,
) -> Conversion: ...
def to_format(
    network: Network,
    to: Format,
    missing_gen_cost: Optional[str] = ...,
    default_gen_cost: Optional[str] = ...,
    gen_cost_csv: Optional[Any] = ...,
) -> Conversion: ...
def to_matpower(network: Network) -> str: ...
def to_json(network: Network) -> str: ...
def to_dense(network: Network) -> DenseNetwork: ...
class Package:
    @classmethod
    def from_file(
        cls, path: Any, from_: Optional[Format] = ..., scenario: int = ...
    ) -> Package: ...
    @classmethod
    def from_str(cls, text: str, from_: Optional[Format] = ...) -> Package: ...
    @classmethod
    def from_json(cls, text: str) -> Package: ...
    @classmethod
    def from_balanced(
        cls, network: Network, include_solver_metadata: bool = ...
    ) -> Package: ...
    @classmethod
    def from_multiconductor(cls, network: Any) -> Package: ...
    @property
    def model_kind(self) -> Literal["balanced", "multiconductor", "unknown"]: ...
    def to_json(self) -> str: ...
    def as_balanced(self) -> Network: ...
    def as_multiconductor(self) -> Any: ...
    def operating_points(self) -> Optional[Dict[str, Any]]: ...
    def materialize_operating_point(self, index: int) -> Package: ...
    def validate(self) -> None: ...
    def validation(self) -> Dict[str, Any]: ...
    def diagnostics(self) -> List[Dict[str, Any]]: ...
    def multiconductor_to_balanced_preflight(
        self, base_mva: float = ...
    ) -> Dict[str, Any]: ...
    def lower_multiconductor_to_balanced(self, base_mva: float = ...) -> Package: ...
    def __repr__(self) -> str: ...
def write_gridfm_batch(
    networks: List[Network],
    out_dir: Any,
    base_scenario: int = ...,
    include_y_bus: bool = ...,
    include_taps: bool = ...,
    include_shifts: bool = ...,
    missing_gen_cost: Optional[str] = ...,
    default_gen_cost: Optional[str] = ...,
    gen_cost_csv: Optional[Any] = ...,
) -> GridfmOutputs: ...
def read_gridfm(dir: Any, scenario: int = ...) -> GridfmRead: ...
def read_gridfm_scenarios(dir: Any) -> List[GridfmRead]: ...
def read_pypsa_csv_folder(path: Any) -> Network: ...
