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
    "DcOpfInstance",
    "DcOpfSolution",
    "DcPfInstance",
    "DcPfSolution",
    "Diagnostic",
    "DisplayData",
    "EmitResult",
    "FormatInfo",
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
    "__version__",
    "dist",
    "emit_gridfm_batch",
    "features",
    "from_json",
    "from_ppc",
    "parse_display_file",
    "parse_file",
    "parse_geo",
    "parse_text",
    "resolve_format",
    "versions",
]

__version__: str

Scheme = Literal["bx", "xb"]
BranchSusceptanceFormula = Literal[
    "series_susceptance",
    "tap_adjusted_reactance",
    "reactance_only",
]
SensitivitySolver = Literal["auto", "dense", "sparse"]
Units = Literal["perunit", "native"]
GridfmOutputs = Dict[str, Any]

class PowerIOError(ValueError):
    """Base error from the powerio parser, emitter, or matrix calculations.

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
    n_buses: int
    n_branches: int
    n_generators: int
    n_loads: int
    n_shunts: int
    n_switches: int
    n_storage: int
    n_hvdc: int
    n_transformers_3w: int
    n_areas: int
    is_radial: bool
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
    def calc_connectivity_report(self) -> Dict[str, Any]: ...
    def to_json(self) -> str: ...
    def to_geo_layer(self) -> Dict[str, Any]: ...
    def apply_geo_layer(
        self, text: str, name_hint: Optional[str] = ...
    ) -> Tuple["BalancedNetwork", Dict[str, Any]]: ...
    def calc_bprime_matrix(
        self, scheme: Scheme = ..., *, skip_zero_impedance: bool = ...
    ) -> Any: ...
    def calc_incidence_matrix(self, formula: str = ...) -> Any: ...
    def calc_bus_susceptance_matrix(self, formula: str = ...) -> Any: ...
    def calc_branch_susceptance_matrix(self, formula: str = ...) -> Any: ...
    def calc_phase_shift_injection(self, formula: str = ...) -> Any: ...
    def calc_branch_flow_dc(self, voltage_angles: Any, formula: str = ...) -> Any: ...
    def calc_bus_injection_dc(self, voltage_angles: Any, formula: str = ...) -> Any: ...
    def calc_bdoubleprime_matrix(
        self, scheme: Scheme = ..., *, skip_zero_impedance: bool = ...
    ) -> Any: ...
    def calc_lacpf_matrix(
        self,
        *,
        include_taps: bool = ...,
        include_shifts: bool = ...,
        skip_zero_impedance: bool = ...,
    ) -> Any: ...
    def calc_adjacency_matrix(self) -> Any: ...
    def calc_admittance_matrix(
        self,
        *,
        include_taps: bool = ...,
        include_shifts: bool = ...,
        skip_zero_impedance: bool = ...,
    ) -> Any: ...
    def calc_ptdf(
        self,
        formula: BranchSusceptanceFormula = ...,
        solver: SensitivitySolver = ...,
    ) -> Any: ...
    def calc_lodf(
        self,
        formula: BranchSusceptanceFormula = ...,
        solver: SensitivitySolver = ...,
    ) -> Any: ...
    def calc_weighted_laplacian(
        self,
        formula: BranchSusceptanceFormula = ...,
    ) -> Any: ...
    def emit_gridfm(
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
    def to_normalized(
        self,
        *,
        clamp_angle_bounds: bool = ...,
        angle_bound_pad: Optional[float] = ...,
    ) -> "BalancedNetwork": ...
    def to_ppc(self) -> Dict[str, Any]: ...
    def to_networkx(self) -> Any: ...
    def emit_dcopf_bundle(
        self,
        out_dir: str,
        formula: BranchSusceptanceFormula = ...,
        units: Units = ...,
        missing_gen_cost: Optional[str] = ...,
        default_gen_cost: Optional[str] = ...,
        gen_cost_csv: Optional[Any] = ...,
    ) -> Dict[str, Any]: ...

class EmitResult(NamedTuple):
    text: Optional[str]
    diagnostics: List[Diagnostic]

class FormatInfo(NamedTuple):
    token: str
    extension: Optional[str]
    is_directory: bool
    can_emit: bool

# Any format name or alias the Rust hub accepts (e.g. "matpower"/"m",
# "psse"/"raw"). Kept as `str` so aliases type-check; the binding validates it.
Format = str

from . import dist as dist

def parse_display_file(path: Any, format: Optional[Format] = ...) -> DisplayData: ...
def resolve_format(name: str) -> Optional[FormatInfo]: ...
def versions() -> Any: ...
def from_json(text: str) -> BalancedNetwork: ...
def from_ppc(ppc: Dict[str, Any]) -> BalancedNetwork: ...
def parse_geo(text: str, name_hint: Optional[str] = ...) -> Dict[str, Any]: ...

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

class PioModule(Generic[_T]):
    _inner: Any
    def __init__(self, inner: Any) -> None: ...
    @classmethod
    @overload
    def from_value(cls, value: BalancedNetwork) -> PioModule[BalancedNetwork]: ...
    @classmethod
    @overload
    def from_value(
        cls, value: dist.MulticonductorNetwork
    ) -> PioModule[dist.MulticonductorNetwork]: ...
    @property
    def value(self) -> _T: ...
    def as_balanced_network(self) -> BalancedNetwork: ...
    def as_multiconductor_network(self) -> dist.MulticonductorNetwork: ...
    def emit(self, format: Format, destination: Optional[Any] = ...) -> EmitResult: ...
    @property
    def kind(self) -> str: ...
    def inspect(self) -> Any: ...
    @property
    def diagnostics(self) -> List[Diagnostic]: ...
    def list_states(self) -> Any: ...
    def inspect_state(
        self, time_position: Optional[int] = ..., scenario: Optional[str] = ...
    ) -> Any: ...
    def export_state(
        self, time_position: Optional[int] = ..., scenario: Optional[str] = ...
    ) -> PioModule[Any]: ...
    def to_balanced_report(self, base_mva: float = ...) -> Any: ...
    def to_balanced(self, base_mva: float = ...) -> PioModule[BalancedNetwork]: ...

@overload
def parse_file(
    path: Any,
    format: Optional[Format] = ...,
    *,
    include_root: Optional[Any] = ...,
    value_type: type[PioModule[Any]],
) -> PioModule[Any]: ...
@overload
def parse_file(
    path: Any,
    format: Optional[Format] = ...,
    *,
    include_root: Optional[Any] = ...,
    value_type: type[_T],
) -> PioModule[_T]: ...
@overload
def parse_file(
    path: Any,
    format: Optional[Format] = ...,
    *,
    include_root: Optional[Any] = ...,
    value_type: None = ...,
) -> PioModule[Any]: ...
@overload
def parse_text(
    text: str,
    *,
    name: str,
    format: Optional[Format] = ...,
    include_root: Optional[Any] = ...,
    value_type: type[PioModule[Any]],
) -> PioModule[Any]: ...
@overload
def parse_text(
    text: str,
    *,
    name: str,
    format: Optional[Format] = ...,
    include_root: Optional[Any] = ...,
    value_type: type[_T],
) -> PioModule[_T]: ...
@overload
def parse_text(
    text: str,
    *,
    name: str,
    format: Optional[Format] = ...,
    include_root: Optional[Any] = ...,
    value_type: None = ...,
) -> PioModule[Any]: ...
def emit_gridfm_batch(
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
def features() -> Dict[str, bool]: ...
