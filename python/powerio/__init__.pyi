from typing import Any, Dict, List, Literal, NamedTuple, Optional, Tuple, TypedDict

__version__: str

Scheme = Literal["bx", "xb"]
Convention = Literal["series", "matpower", "reactance-only"]
SensitivitySolver = Literal["auto", "dense", "sparse"]
Units = Literal["perunit", "native"]
GridfmOutputs = Dict[str, Any]

class PowerIOError(ValueError):
    """Base error from the powerio parser, converter, or matrix builders.

    Failures mapped from the Rust core carry the diagnostic code string as
    ``code``; it is set at raise time, so it is instance-only.
    """

    code: str

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
    g_fr: float
    b_fr: float
    g_to: float
    b_to: float
    rate_a: float
    rate_b: float
    rate_c: float
    rating_sets: List[BranchRatingSet]
    c_rating_a: Optional[float]
    c_rating_b: Optional[float]
    c_rating_c: Optional[float]
    tap: float
    shift: float
    in_service: bool
    angmin: float
    angmax: float
    pf: Optional[float]
    qf: Optional[float]
    pt: Optional[float]
    qt: Optional[float]
    uid: Optional[str]

class Switch(TypedDict):
    from_id: int
    to_id: int
    closed: bool
    thermal_rating: Optional[float]
    current_rating: Optional[float]
    pf: Optional[float]
    qf: Optional[float]
    pt: Optional[float]
    qt: Optional[float]
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
    # MATPOWER gen columns past PMIN, in column order: pc1, pc2, qc1min,
    # qc1max, qc2min, qc2max, ramp_agc, ramp_10, ramp_30, ramp_q, apf.
    caps: List[Optional[float]]
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
    network: "BalancedNetwork"
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

class BalancedNetwork:
    # Data attributes and the non-matrix methods delegate to the compiled
    # `_powerio._BalancedNetwork` handle at runtime via `BalancedNetwork.__getattr__`.
    _inner: Any
    def __init__(self, inner: Any) -> None: ...
    name: str
    base_mva: float
    source_format: Literal[
        "matpower",
        "powermodels-json",
        "opfdata-json",
        "egret-json",
        "psse",
        "powerworld",
        "pandapower-json",
        "pslf",
        "powerworld-pwb",
        "in-memory",
        "normalized",
        "gridfm",
        "pypsa-csv",
        "goc3-json",
        "surge-json",
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
    switches: List[Switch]
    generators: List[Gen]
    def reference_bus_index(self) -> int: ...
    def reference_bus_indices(self) -> List[int]: ...
    def connectivity_report(self) -> Dict[str, Any]: ...
    def to_matpower(self) -> str: ...
    def to_json(self) -> str: ...
    def geo_layer(self) -> Dict[str, Any]: ...
    def apply_geo_layer(
        self, text: str, name_hint: Optional[str] = ...
    ) -> Tuple["BalancedNetwork", Dict[str, Any]]: ...
    def to_format(
        self,
        to: str,
        missing_gen_cost: Optional[str] = ...,
        default_gen_cost: Optional[str] = ...,
        gen_cost_csv: Optional[Any] = ...,
    ) -> "Conversion": ...
    def to_canonical_format(self, to: str) -> "Conversion": ...
    def write_file(
        self,
        path: Any,
        to: str,
        missing_gen_cost: Optional[str] = ...,
        default_gen_cost: Optional[str] = ...,
        gen_cost_csv: Optional[Any] = ...,
    ) -> List[str]: ...
    def bprime(self, scheme: Scheme = ...) -> Any: ...
    def dc_data(self, formula: str = ...) -> Dict[str, Any]: ...
    def bdoubleprime(self, scheme: Scheme = ...) -> Any: ...
    def lacpf(self, *, include_taps: bool = ..., include_shifts: bool = ...) -> Any: ...
    def adjacency(self) -> Any: ...
    def ybus_parts(
        self, *, include_taps: bool = ..., include_shifts: bool = ...
    ) -> YbusParts: ...
    def ybus(self, *, include_taps: bool = ..., include_shifts: bool = ...) -> Any: ...
    def ptdf(self, convention: Convention = ..., solver: SensitivitySolver = ...) -> Any: ...
    def lodf(self, convention: Convention = ..., solver: SensitivitySolver = ...) -> Any: ...
    def weighted_laplacian(self, convention: Convention = ...) -> Any: ...
    def incidence(self, convention: Convention = ...) -> Incidence: ...
    def write_gridfm(
        self,
        out_dir: Any,
        *,
        scenario: int = ...,
        include_y_bus: bool = ...,
        include_taps: bool = ...,
        include_shifts: bool = ...,
        missing_gen_cost: Optional[str] = ...,
        default_gen_cost: Optional[str] = ...,
        gen_cost_csv: Optional[Any] = ...,
    ) -> GridfmOutputs: ...
    def write_pypsa_csv_folder(self, out_dir: Any) -> Dict[str, Any]: ...
    def to_normalized(self) -> "BalancedNetwork": ...
    def to_normalized_with_options(
        self,
        *,
        clamp_angle_bounds: bool = ...,
        angle_bound_pad: Optional[float] = ...,
    ) -> "BalancedNetwork": ...
    def to_ppc(self) -> Dict[str, Any]: ...
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


class Conversion(NamedTuple):
    text: str
    warnings: List[str]

# Any reader/writer name or alias the Rust hub accepts (e.g. "matpower"/"m",
# "psse"/"raw"). Kept as `str` so aliases type-check; the binding validates it.
Format = str

from . import dist as dist

def parse_display_file(path: Any, from_: Optional[Format] = ...) -> DisplayData: ...
def parse_display_bytes(data: bytes, from_: Format) -> DisplayData: ...
def to_json(network: BalancedNetwork) -> str: ...
def from_json(text: str) -> BalancedNetwork: ...
def from_ppc(ppc: Dict[str, Any]) -> BalancedNetwork: ...
def parse_geo(text: str, name_hint: Optional[str] = ...) -> Dict[str, Any]: ...
def convert_file(
    path: Any,
    to: Format,
    from_: Optional[Format] = ...,
    missing_gen_cost: Optional[str] = ...,
    default_gen_cost: Optional[str] = ...,
    gen_cost_csv: Optional[Any] = ...,
    out: Optional[Any] = ...,
) -> Conversion: ...
def convert_str(
    text: str,
    to: Format,
    from_: Format = ...,
    missing_gen_cost: Optional[str] = ...,
    default_gen_cost: Optional[str] = ...,
    gen_cost_csv: Optional[Any] = ...,
) -> Conversion: ...
def to_format(
    network: BalancedNetwork,
    to: Format,
    missing_gen_cost: Optional[str] = ...,
    default_gen_cost: Optional[str] = ...,
    gen_cost_csv: Optional[Any] = ...,
) -> Conversion: ...
def to_matpower(network: BalancedNetwork) -> str: ...
class StoredModule:
    _inner: Any
    def __init__(self, inner: Any) -> None: ...
    @classmethod
    def from_json(cls, text: str) -> StoredModule: ...
    @classmethod
    def from_file(
        cls,
        path: Any,
        from_: Optional[str] = ...,
        *,
        include_root: Optional[Any] = ...,
    ) -> StoredModule: ...
    @classmethod
    def from_str(cls, text: str, from_: Optional[str] = ...) -> StoredModule: ...
    @classmethod
    def from_bytes(cls, data: bytes, from_: Optional[str] = ...) -> StoredModule: ...
    def as_balanced_network(self) -> BalancedNetwork: ...
    def as_multiconductor_network(self) -> dist.MulticonductorNetwork: ...
    def to_json(self) -> str: ...
    @property
    def kind(self) -> str: ...
    def inspect(self) -> Any: ...
    def diagnostics(self) -> Any: ...
    def state_inventory(self) -> Any: ...
    def select_state(
        self, time_position: Optional[int] = ..., scenario: Optional[str] = ...
    ) -> Any: ...
    def export_state(
        self, time_position: Optional[int] = ..., scenario: Optional[str] = ...
    ) -> StoredModule: ...
    def to_balanced_inspect(self, base_mva: float = ...) -> Any: ...
    def to_balanced(self, base_mva: float = ...) -> StoredModule: ...

def parse(
    source: Any,
    from_: Optional[Format] = ...,
    *,
    include_root: Optional[Any] = ...,
    value_type: Optional[type] = ...,
) -> Any: ...
def write_gridfm_batch(
    networks: List[BalancedNetwork],
    out_dir: Any,
    *,
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
