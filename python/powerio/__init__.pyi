from typing import (
    Any,
    Dict,
    Generic,
    Iterator,
    List,
    Literal,
    NamedTuple,
    Optional,
    Tuple,
    TypedDict,
    TypeVar,
    overload,
)

_T = TypeVar("_T")
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
    "features",
    "from_json",
    "from_ppc",
    "parse",
    "parse_display_bytes",
    "parse_display_file",
    "parse_file",
    "parse_geo",
    "read_gridfm",
    "read_gridfm_scenarios",
    "to_format",
    "to_json",
    "to_matpower",
    "versions",
    "write_gridfm_batch",
]

__version__: str

Scheme = Literal["bx", "xb"]
Convention = Literal[
    "series_susceptance",
    "tap_adjusted_reactance",
    "reactance_only",
    "series",
    "series-impedance",
    "matpower",
    "mp",
    "reactance-only",
]
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
    """A well-formed case cannot satisfy a requested operation.

    A refused pass (e.g. :meth:`PioModule.to_balanced`) additionally sets
    ``diagnostics``: the pass's structured findings, each a dict with
    ``code``, ``severity``, ``message``, and ``target``. Absent on a
    ``PowerIODataError`` raised elsewhere.
    """

    diagnostics: List[Dict[str, Any]]

# The runtime re-exports the native record classes; the stub does the same so
# a `_powerio.Diagnostic` and a `powerio.Diagnostic` are one type to a checker.
from ._powerio import Diagnostic as Diagnostic
from ._powerio import SourceSpan as SourceSpan

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
    base_frequency: float
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
    n_generators: int
    n_loads: int
    n_shunts: int
    n_switches: int
    n_storage: int
    n_hvdc: int
    n_transformers_3w: int
    n_areas: int
    is_radial: bool
    n_connected_components: int
    n_islands: int
    buses: List[Bus]
    loads: List[Load]
    shunts: List[Shunt]
    branches: List[Branch]
    switches: List[Switch]
    generators: List[Gen]
    storage: List[Dict[str, Any]]
    hvdc: List[Dict[str, Any]]
    transformers_3w: List[Dict[str, Any]]
    areas: List[Dict[str, Any]]
    def reference_bus_index(self) -> int: ...
    def reference_bus_indices(self) -> List[int]: ...
    def connectivity_report(self) -> Dict[str, Any]: ...
    def to_matpower(self) -> str: ...
    def to_json(self) -> str: ...
    def diagnostics(self) -> Any: ...
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
    ) -> List[Diagnostic]: ...
    def bprime(
        self, scheme: Scheme = ..., *, skip_zero_impedance: bool = ...
    ) -> Any: ...
    def calc_bprime_matrix(
        self, scheme: Scheme = ..., *, skip_zero_impedance: bool = ...
    ) -> Any: ...
    def dc_data(self, formula: str = ...) -> Dict[str, Any]: ...
    def calc_incidence_matrix(self, formula: str = ...) -> Any: ...
    def calc_bus_susceptance_matrix(self, formula: str = ...) -> Any: ...
    def calc_branch_susceptance_matrix(self, formula: str = ...) -> Any: ...
    def calc_phase_shift_injection(self, formula: str = ...) -> Any: ...
    def calc_branch_flow_dc(
        self, voltage_angles: Any, formula: str = ...
    ) -> Any: ...
    def bdoubleprime(
        self, scheme: Scheme = ..., *, skip_zero_impedance: bool = ...
    ) -> Any: ...
    def calc_bdoubleprime_matrix(
        self, scheme: Scheme = ..., *, skip_zero_impedance: bool = ...
    ) -> Any: ...
    def lacpf(
        self,
        *,
        include_taps: bool = ...,
        include_shifts: bool = ...,
        skip_zero_impedance: bool = ...,
    ) -> Any: ...
    def calc_lacpf_matrix(
        self,
        *,
        include_taps: bool = ...,
        include_shifts: bool = ...,
        skip_zero_impedance: bool = ...,
    ) -> Any: ...
    def adjacency(self) -> Any: ...
    def calc_adjacency_matrix(self) -> Any: ...
    def ybus_parts(
        self,
        *,
        include_taps: bool = ...,
        include_shifts: bool = ...,
        skip_zero_impedance: bool = ...,
    ) -> YbusParts: ...
    def calc_admittance_matrix_parts(
        self,
        *,
        include_taps: bool = ...,
        include_shifts: bool = ...,
        skip_zero_impedance: bool = ...,
    ) -> YbusParts: ...
    def ybus(
        self,
        *,
        include_taps: bool = ...,
        include_shifts: bool = ...,
        skip_zero_impedance: bool = ...,
    ) -> Any: ...
    def calc_admittance_matrix(
        self,
        *,
        include_taps: bool = ...,
        include_shifts: bool = ...,
        skip_zero_impedance: bool = ...,
    ) -> Any: ...
    def ptdf(self, convention: Convention = ..., solver: SensitivitySolver = ...) -> Any: ...
    def calc_ptdf(
        self, convention: Convention = ..., solver: SensitivitySolver = ...
    ) -> Any: ...
    def lodf(self, convention: Convention = ..., solver: SensitivitySolver = ...) -> Any: ...
    def calc_lodf(
        self, convention: Convention = ..., solver: SensitivitySolver = ...
    ) -> Any: ...
    def weighted_laplacian(
        self, convention: Convention = ..., *, skip_zero_impedance: bool = ...
    ) -> Any: ...
    def calc_weighted_laplacian(
        self, convention: Convention = ..., *, skip_zero_impedance: bool = ...
    ) -> Any: ...
    def incidence(
        self, convention: Convention = ..., *, skip_zero_impedance: bool = ...
    ) -> Incidence: ...
    def calc_incidence_factors(
        self, convention: Convention = ..., *, skip_zero_impedance: bool = ...
    ) -> Incidence: ...
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
    warnings: List[Diagnostic]
    @property
    def diagnostics(self) -> List[Diagnostic]: ...

# Any reader/writer name or alias the Rust hub accepts (e.g. "matpower"/"m",
# "psse"/"raw"). Kept as `str` so aliases type-check; the binding validates it.
Format = str

from . import dist as dist

def parse_display_file(path: Any, from_: Optional[Format] = ...) -> DisplayData: ...
def parse_display_bytes(data: bytes, from_: Format) -> DisplayData: ...
def to_json(network: BalancedNetwork) -> str: ...
def versions() -> Any: ...

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

class _TypedValue:
    module: PioModule[Any]
    kind: str
    def __init__(self, module: PioModule[Any], kind: str) -> None: ...

class TimeSeries(_TypedValue):
    def __len__(self) -> int: ...
    def __getitem__(self, index: int) -> PioModule[Any]: ...
    def __iter__(self) -> Iterator[PioModule[Any]]: ...

class ScenarioSet(_TypedValue):
    def keys(self) -> Tuple[str, ...]: ...
    def __len__(self) -> int: ...
    def __iter__(self) -> Iterator[str]: ...
    def __contains__(self, scenario: object) -> bool: ...
    def __getitem__(self, scenario: str) -> PioModule[Any]: ...
class DcPfInstance(_TypedValue): ...
class AcPfInstance(_TypedValue): ...
class DcOpfInstance(_TypedValue): ...
class AcOpfInstance(_TypedValue): ...
class McAcPfInstance(_TypedValue): ...
class McAcOpfInstance(_TypedValue): ...
class AcScucInstance(_TypedValue): ...
class DcPfSolution(_TypedValue): ...
class AcPfSolution(_TypedValue): ...
class DcOpfSolution(_TypedValue): ...
class AcOpfSolution(_TypedValue): ...
class McAcPfSolution(_TypedValue): ...
class McAcOpfSolution(_TypedValue): ...
class AcScucSolution(_TypedValue): ...
class UnknownValue(_TypedValue): ...

class _DiagnosticList(List[Diagnostic]):
    def __call__(self) -> _DiagnosticList: ...

class PioModule(Generic[_T]):
    _inner: Any
    def __init__(self, inner: Any) -> None: ...
    @classmethod
    def from_json(cls, text: str) -> PioModule[Any]: ...
    @classmethod
    def from_file(
        cls,
        path: Any,
        from_: Optional[str] = ...,
        *,
        include_root: Optional[Any] = ...,
    ) -> PioModule[Any]: ...
    @classmethod
    def from_str(cls, text: str, from_: Optional[str] = ...) -> PioModule[Any]: ...
    @classmethod
    def from_bytes(
        cls, data: bytes, from_: Optional[str] = ..., *, name: Optional[str] = ...
    ) -> PioModule[Any]: ...
    @property
    def value(self) -> _T: ...
    def as_balanced_network(self) -> BalancedNetwork: ...
    def as_multiconductor_network(self) -> dist.MulticonductorNetwork: ...
    def to_json(self) -> str: ...
    def to_format(
        self,
        to: Format,
        missing_gen_cost: Optional[str] = ...,
        default_gen_cost: Optional[str] = ...,
        gen_cost_csv: Optional[Any] = ...,
    ) -> Conversion: ...
    def write_file(
        self, path: Any, format: Optional[Format] = ...
    ) -> List[Diagnostic]: ...
    @overload
    def emit(self, format: Format, destination: None = ...) -> Conversion: ...
    @overload
    def emit(self, format: Format, destination: Any) -> List[Diagnostic]: ...
    @property
    def kind(self) -> str: ...
    def inspect(self) -> Any: ...
    @property
    def diagnostics(self) -> _DiagnosticList: ...
    def state_inventory(self) -> Any: ...
    def select_state(
        self, time_position: Optional[int] = ..., scenario: Optional[str] = ...
    ) -> Any: ...
    def inspect_state(
        self, time_position: Optional[int] = ..., scenario: Optional[str] = ...
    ) -> Any: ...
    def export_state(
        self, time_position: Optional[int] = ..., scenario: Optional[str] = ...
    ) -> PioModule[Any]: ...
    def to_balanced_report(self, base_mva: float = ...) -> Any: ...
    def to_balanced_inspect(self, base_mva: float = ...) -> Any: ...
    def to_balanced(self, base_mva: float = ...) -> PioModule[BalancedNetwork]: ...

@overload
def parse(
    source: Any,
    from_: Optional[Format] = ...,
    *,
    include_root: Optional[Any] = ...,
    value_type: type[PioModule[Any]],
    name: Optional[str] = ...,
) -> PioModule[Any]: ...
@overload
def parse(
    source: Any,
    from_: Optional[Format] = ...,
    *,
    include_root: Optional[Any] = ...,
    value_type: type[_T],
    name: Optional[str] = ...,
) -> PioModule[_T]: ...
@overload
def parse(
    source: Any,
    from_: Optional[Format] = ...,
    *,
    include_root: Optional[Any] = ...,
    value_type: None = ...,
    name: Optional[str] = ...,
) -> PioModule[Any]: ...

@overload
def parse_file(
    path: Any,
    format: Optional[Format] = ...,
    *,
    include_root: Optional[Any] = ...,
    value_type: type[PioModule[Any]],
    from_: Optional[Format] = ...,
) -> PioModule[Any]: ...
@overload
def parse_file(
    path: Any,
    format: Optional[Format] = ...,
    *,
    include_root: Optional[Any] = ...,
    value_type: type[_T],
    from_: Optional[Format] = ...,
) -> PioModule[_T]: ...
@overload
def parse_file(
    path: Any,
    format: Optional[Format] = ...,
    *,
    include_root: Optional[Any] = ...,
    value_type: None = ...,
    from_: Optional[Format] = ...,
) -> PioModule[Any]: ...
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
def features() -> Dict[str, bool]: ...
