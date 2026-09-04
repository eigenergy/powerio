from __future__ import annotations

from dataclasses import dataclass
from typing import (
    Any,
    Dict,
    Generic,
    Iterable,
    Iterator,
    List,
    Literal,
    Mapping,
    NamedTuple,
    Optional,
    Sequence,
    Tuple,
    TypedDict,
    TypeVar,
    Union,
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
    "ActivePower",
    "ApparentPower",
    "Artifact",
    "BalancedNetwork",
    "CalculationUpdate",
    "ComponentId",
    "DcOpfInstance",
    "DcOpfSolution",
    "DcPfInstance",
    "DcPfSolution",
    "Diagnostic",
    "DisplayData",
    "EmitResult",
    "FormatInfo",
    "GeoLayer",
    "McAcOpfInstance",
    "McAcOpfSolution",
    "McAcPfInstance",
    "McAcPfSolution",
    "MulticonductorNetwork",
    "NetworkUpdate",
    "OperatingPoint",
    "OperatingPointUpdate",
    "PioModule",
    "PowerIODataError",
    "PowerIOError",
    "PowerIOParseError",
    "PwdDisplay",
    "PwdSubstation",
    "ReactivePower",
    "Residuals",
    "Scenario",
    "ScenarioSet",
    "ScucActiveReserveZone",
    "ScucBranchSwitchingCost",
    "ScucContingency",
    "ScucDevice",
    "ScucDeviceOutputs",
    "ScucDevicePeriod",
    "ScucEnergyCostBlock",
    "ScucEnergyRequirement",
    "ScucInitialCommitment",
    "ScucInputs",
    "ScucNetworkOutputs",
    "ScucRampLimits",
    "ScucReactiveCapability",
    "ScucReactiveReserveZone",
    "ScucReserveCosts",
    "ScucReserveLimits",
    "ScucShunt",
    "ScucStartupCostAdjustment",
    "ScucStartupLimit",
    "ScucTransformerControl",
    "ScucViolationCosts",
    "SocwrOpfSolution",
    "SourceSpan",
    "TimePoint",
    "TimeSeries",
    "UpdateChange",
    "UpdateReport",
    "__version__",
    "apply_bus_load_active_power",
    "apply_updates",
    "deserialize",
    "dist",
    "emit",
    "features",
    "from_ppc",
    "parse",
    "parse_display",
    "parse_geo",
    "resolve_format",
    "serialize",
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
from ._powerio import ActivePower as ActivePower
from ._powerio import ApparentPower as ApparentPower
from ._powerio import CalculationUpdate as CalculationUpdate
from ._powerio import ComponentId as ComponentId
from ._powerio import Diagnostic as Diagnostic
from ._powerio import NetworkUpdate as NetworkUpdate
from ._powerio import OperatingPointUpdate as OperatingPointUpdate
from ._powerio import ReactivePower as ReactivePower
from ._powerio import Residuals as Residuals
from ._powerio import ScucActiveReserveZone as ScucActiveReserveZone
from ._powerio import ScucBranchSwitchingCost as ScucBranchSwitchingCost
from ._powerio import ScucContingency as ScucContingency
from ._powerio import ScucDevice as ScucDevice
from ._powerio import ScucDeviceOutputs as ScucDeviceOutputs
from ._powerio import ScucDevicePeriod as ScucDevicePeriod
from ._powerio import ScucEnergyCostBlock as ScucEnergyCostBlock
from ._powerio import ScucEnergyRequirement as ScucEnergyRequirement
from ._powerio import ScucInitialCommitment as ScucInitialCommitment
from ._powerio import ScucInputs as ScucInputs
from ._powerio import ScucNetworkOutputs as ScucNetworkOutputs
from ._powerio import ScucRampLimits as ScucRampLimits
from ._powerio import ScucReactiveCapability as ScucReactiveCapability
from ._powerio import ScucReactiveReserveZone as ScucReactiveReserveZone
from ._powerio import ScucReserveCosts as ScucReserveCosts
from ._powerio import ScucReserveLimits as ScucReserveLimits
from ._powerio import ScucShunt as ScucShunt
from ._powerio import ScucStartupCostAdjustment as ScucStartupCostAdjustment
from ._powerio import ScucStartupLimit as ScucStartupLimit
from ._powerio import ScucTransformerControl as ScucTransformerControl
from ._powerio import ScucViolationCosts as ScucViolationCosts
from ._powerio import SourceSpan as SourceSpan
from ._powerio import UpdateChange as UpdateChange
from ._powerio import UpdateReport as UpdateReport

class GenCost(TypedDict):
    model: int
    startup: float
    shutdown: float
    ncost: int
    coeffs: List[float]

class ActivePowerControl(TypedDict, total=False):
    participate: bool
    droop_percent: Optional[float]
    participation_factor: Optional[float]
    minimum_target_active_power_mw: Optional[float]
    maximum_target_active_power_mw: Optional[float]

class ComponentIdentity(TypedDict):
    component_type: str
    local_id: str

class ComponentAlias(TypedDict, total=False):
    value: str
    alias_type: str

class ExternalIdentifier(TypedDict, total=False):
    value: str
    authority: str

class ComponentMetadata(TypedDict, total=False):
    component: ComponentIdentity
    name: str
    equipment_container: ComponentIdentity
    aliases: List[ComponentAlias]
    external_identifiers: List[ExternalIdentifier]
    properties: Dict[str, str]
    fictitious: bool

class TerminalReference(TypedDict):
    equipment: ComponentIdentity
    terminal: int

class TransformerControl(TypedDict):
    mode: Literal[
        "fixed",
        "voltage",
        "reactive_flow",
        "active_flow",
        "dc_line_quantity",
        "asymmetric_active_flow",
        "unknown",
    ]
    enabled: bool
    controlled_bus: Optional[int]
    controlled_bus_on_winding_side: bool
    regulating_terminal: Optional[TerminalReference]
    tap_min: float
    tap_max: float
    band_min: float
    band_max: float
    tap_position_count: int
    mva_base: float
    winding_connection_angle: Optional[float]

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
    section_count: Optional[int]
    uid: Optional[str]

class StaticVarCompensator(TypedDict):
    bus: int
    b_min_siemens: float
    b_max_siemens: float
    voltage_setpoint_kv: float
    reactive_power_setpoint_mvar: float
    regulation_mode: Literal["voltage", "reactive_power"]
    regulating: bool
    regulating_terminal: Optional[Dict[str, Any]]
    p: float
    q: float
    in_service: bool
    uid: Optional[str]
    extras: Dict[str, Any]

class BranchRatingSet(TypedDict):
    name: str
    rate_mva: float

class Branch(TypedDict):
    name: Optional[str]
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
    control: Optional[TransformerControl]
    pf: Optional[float]
    qf: Optional[float]
    pt: Optional[float]
    qt: Optional[float]
    uid: Optional[str]

class ThreeWindingTransformerWinding(TypedDict):
    bus: int
    tap: float
    shift: float
    nominal_kv: float
    rate_a: float
    rate_b: float
    rate_c: float
    control: Optional[TransformerControl]

class ThreeWindingTransformerImpedance(TypedDict):
    r: float
    x: float
    base_mva: float

class ThreeWindingTransformer(TypedDict):
    windings: List[ThreeWindingTransformerWinding]
    z: List[ThreeWindingTransformerImpedance]
    star_vm: float
    star_va: float
    mag_g: float
    mag_b: float
    in_service: bool
    name: Optional[str]
    uid: Optional[str]
    extras: Dict[str, Any]

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
    energy_source: Literal["hydro", "nuclear", "wind", "thermal", "solar", "other"]
    pg: float
    qg: float
    pmax: float
    pmin: float
    qmax: float
    qmin: float
    vg: float
    mbase: float
    in_service: bool
    voltage_regulation_on: bool
    regulated_bus: Optional[int]
    regulating_terminal: Optional[TerminalReference]
    # MATPOWER gen columns past PMIN, in column order: pc1, pc2, qc1min,
    # qc1max, qc2min, qc2max, ramp_agc, ramp_10, ramp_30, ramp_q, apf.
    caps: List[Optional[float]]
    cost: Optional[GenCost]
    active_power_control: Optional[ActivePowerControl]
    uid: Optional[str]

class Storage(TypedDict, total=False):
    bus: int
    ps: float
    qs: float
    energy: float
    energy_rating: float
    charge_rating: float
    discharge_rating: float
    charge_efficiency: float
    discharge_efficiency: float
    thermal_rating: float
    current_rating: Optional[float]
    qmin: float
    qmax: float
    r: float
    x: float
    p_loss: float
    q_loss: float
    in_service: bool
    active_power_control: Optional[ActivePowerControl]
    uid: Optional[str]
    extras: Dict[str, Any]

class CaseMetadata(TypedDict, total=False):
    case_date: str
    forecast_distance: int
    source_model_format: str
    minimum_validation_level: str

class Subnetwork(TypedDict):
    component: ComponentIdentity
    parent: ComponentIdentity
    case_metadata: CaseMetadata
    components: List[ComponentIdentity]

class MinMaxReactiveLimits(TypedDict):
    minimum_reactive_power_mvar: float
    maximum_reactive_power_mvar: float
    properties: Dict[str, str]

class ReactiveCapabilityCurvePoint(TypedDict):
    active_power_mw: float
    minimum_reactive_power_mvar: float
    maximum_reactive_power_mvar: float
    properties: Dict[str, str]

class ReactiveCapabilityCurve(TypedDict):
    curve_style: Literal["constant_y_value", "straight_line_y_values"]
    properties: Dict[str, str]
    points: List[ReactiveCapabilityCurvePoint]

class MinMaxReactiveLimitsRecord(TypedDict):
    kind: Literal["min_max"]
    limits: MinMaxReactiveLimits

class ReactiveCapabilityCurveRecord(TypedDict):
    kind: Literal["capability_curve"]
    limits: ReactiveCapabilityCurve

ReactiveLimits = Union[MinMaxReactiveLimitsRecord, ReactiveCapabilityCurveRecord]

class EquipmentReactiveLimits(TypedDict):
    equipment: ComponentIdentity
    limits: ReactiveLimits

class BoundaryLineGeneration(TypedDict, total=False):
    voltage_regulation_on: bool
    minimum_active_power_mw: float
    maximum_active_power_mw: float
    target_active_power_mw: float
    target_reactive_power_mvar: float
    target_voltage_kv: float
    reactive_limits: ReactiveLimits

class BoundaryLine(TypedDict, total=False):
    component: ComponentIdentity
    voltage_level: ComponentIdentity
    active_power_setpoint_mw: float
    reactive_power_setpoint_mvar: float
    resistance_ohm: float
    reactance_ohm: float
    conductance_siemens: float
    susceptance_siemens: float
    pairing_key: str
    generation: BoundaryLineGeneration
    calculation_load: ComponentIdentity
    calculation_generator: ComponentIdentity

class TieLine(TypedDict, total=False):
    component: ComponentIdentity
    boundary_line1: ComponentIdentity
    boundary_line2: ComponentIdentity
    calculation_branch: ComponentIdentity

class TapChanger(TypedDict, total=False):
    component: ComponentIdentity
    transformer: ComponentIdentity
    winding: int
    kind: Literal["ratio", "phase"]
    tap_position: int
    solved_tap_position: int
    low_tap_position: int
    neutral_tap_position: int
    normal_tap_position: int
    voltage_step_increment_percent: float
    load_tap_changing_capabilities: bool
    regulating: bool
    regulation_mode: str
    regulation_value: float
    target_deadband: float
    regulation_terminal: TerminalReference
    steps: List["TapChangerStep"]

class TapChangerStep(TypedDict):
    position: int
    rho: float
    alpha_degrees: float
    resistance_deviation_percent: float
    reactance_deviation_percent: float
    conductance_deviation_percent: float
    susceptance_deviation_percent: float

class Substation(TypedDict, total=False):
    component: ComponentIdentity
    country: str
    operator: str
    geographical_tags: List[str]

class VoltageLevel(TypedDict, total=False):
    component: ComponentIdentity
    substation: ComponentIdentity
    nominal_kv: float
    low_voltage_limit_kv: float
    high_voltage_limit_kv: float
    topology_kind: Literal["bus_breaker", "node_breaker"]
    buses: List[int]

class ConnectivityNode(TypedDict, total=False):
    component: ComponentIdentity
    voltage_level: ComponentIdentity
    node_number: Optional[int]
    calculated_bus: Optional[int]

class Junction(TypedDict):
    component: ComponentIdentity

class OmittedField(TypedDict):
    component: ComponentIdentity
    field: Literal[
        "active_power",
        "reactive_power",
        "voltage_setpoint",
        "rated_apparent_power",
        "shunt_conductance_per_section",
    ]

class BusBreakerBus(TypedDict, total=False):
    component: ComponentIdentity
    voltage_level: ComponentIdentity
    calculated_bus: int
    voltage_kv: float
    angle_degrees: float

class CalculatedBus(TypedDict, total=False):
    voltage_level: ComponentIdentity
    calculated_bus: int
    nodes: List[ComponentIdentity]
    voltage_kv: float
    angle_degrees: float

class BusbarSection(TypedDict):
    component: ComponentIdentity
    voltage_level: ComponentIdentity
    node: ComponentIdentity

class DetailedTerminal(TypedDict, total=False):
    component: ComponentIdentity
    equipment: ComponentIdentity
    terminal: int
    voltage_level: ComponentIdentity
    bus: ComponentIdentity
    connectable_bus: ComponentIdentity
    node: ComponentIdentity
    connected: bool
    active_power_mw: float
    reactive_power_mvar: float

class TopologyEndpoint(TypedDict):
    kind: Literal["bus", "node"]
    component: ComponentIdentity

class TopologySwitch(TypedDict):
    component: ComponentIdentity
    voltage_level: ComponentIdentity
    kind: Literal["breaker", "disconnector", "load_break_switch"]
    endpoint1: TopologyEndpoint
    endpoint2: TopologyEndpoint
    open: bool
    retained: bool

class InternalConnection(TypedDict):
    voltage_level: ComponentIdentity
    node1: ComponentIdentity
    node2: ComponentIdentity

class TemporaryLimit(TypedDict):
    name: str
    value: float
    acceptable_duration_seconds: int
    fictitious: bool

class LoadingLimits(TypedDict, total=False):
    permanent_limit: float
    permanent_limit_name: str
    temporary_limits: List[TemporaryLimit]

class OperationalLimitGroup(TypedDict, total=False):
    equipment: ComponentIdentity
    terminal: int
    id: str
    properties: Dict[str, str]
    selected: bool
    current_limits: LoadingLimits
    active_power_limits: LoadingLimits
    apparent_power_limits: LoadingLimits

class DcConverterUnit(TypedDict, total=False):
    component: ComponentIdentity
    substation: ComponentIdentity
    operation_mode: Literal[
        "bipolar", "monopolar_ground_return", "monopolar_metallic_return"
    ]

class DcTopologicalNode(TypedDict, total=False):
    component: ComponentIdentity
    dc_converter_unit: ComponentIdentity

class DcTerminal(TypedDict, total=False):
    component: ComponentIdentity
    sequence_number: int
    dc_node: ComponentIdentity
    dc_topological_node: ComponentIdentity
    polarity: Literal["positive", "middle", "negative"]
    connected: bool
    active_power_mw: float
    current_a: float

class DcNode(TypedDict, total=False):
    component: ComponentIdentity
    nominal_voltage_kv: float
    dc_converter_unit: ComponentIdentity
    dc_topological_node: ComponentIdentity
    voltage_kv: float

class DcGround(TypedDict, total=False):
    component: ComponentIdentity
    equipment_container: ComponentIdentity
    dc_terminal: DcTerminal
    rated_dc_voltage_kv: float
    resistance_ohm: float
    inductance_h: float

class DcBusbar(TypedDict, total=False):
    component: ComponentIdentity
    equipment_container: ComponentIdentity
    dc_terminal: DcTerminal
    rated_dc_voltage_kv: float

class DcLine(TypedDict, total=False):
    component: ComponentIdentity
    equipment_container: ComponentIdentity
    dc_terminal1: DcTerminal
    dc_terminal2: DcTerminal
    rated_dc_voltage_kv: float
    resistance_ohm: float
    inductance_h: float
    capacitance_f: float
    length_km: float

class DcSeriesDevice(TypedDict, total=False):
    component: ComponentIdentity
    equipment_container: ComponentIdentity
    dc_terminal1: DcTerminal
    dc_terminal2: DcTerminal
    rated_dc_voltage_kv: float
    resistance_ohm: float
    inductance_h: float

class DcSwitch(TypedDict, total=False):
    component: ComponentIdentity
    equipment_container: ComponentIdentity
    dc_terminal1: DcTerminal
    dc_terminal2: DcTerminal
    kind: Literal["switch", "breaker", "disconnector"]
    rated_dc_voltage_kv: float
    open: bool
    resistance_ohm: float

class DroopCurveSegment(TypedDict):
    minimum_voltage_kv: float
    maximum_voltage_kv: float
    k: float

class DroopCurve(TypedDict):
    segments: List[DroopCurveSegment]

AcDcConverterControlMode = Literal[
    "active_power_at_pcc",
    "dc_voltage",
    "dc_current",
    "active_power_at_pcc_and_dc_voltage_droop_curve",
    "active_power_at_pcc_and_dc_voltage_droop",
    "active_power_at_pcc_and_dc_voltage_droop_with_compensation",
    "active_power_at_pcc_and_dc_voltage_droop_pilot",
]

class VoltageSourceConverter(TypedDict, total=False):
    component: ComponentIdentity
    dc_converter_unit: ComponentIdentity
    dc_terminal1: DcTerminal
    dc_terminal2: DcTerminal
    base_apparent_power_mva: float
    minimum_active_power_mw: float
    maximum_active_power_mw: float
    minimum_dc_voltage_kv: float
    maximum_dc_voltage_kv: float
    rated_dc_voltage_kv: float
    valve_u0_kv: float
    number_of_valves: int
    idle_loss_mw: float
    switching_loss_mw_per_ampere: float
    resistive_loss_ohm: float
    control_mode: AcDcConverterControlMode
    active_power_at_pcc_mw: float
    reactive_power_at_pcc_mvar: float
    target_active_power_mw: float
    target_dc_voltage_kv: float
    pcc_terminal: TerminalReference
    droop_curve: DroopCurve
    droop: float
    droop_compensation: float
    q_share: float
    maximum_modulation_index: float
    maximum_valve_current_a: float
    voltage_regulator_on: bool
    voltage_setpoint_kv: float
    reactive_power_setpoint_mvar: float
    reactive_limits: ReactiveLimits
    pole_loss_active_power_mw: float
    dc_current_a: float
    ac_voltage_kv: float
    dc_voltage_kv: float
    delta_degrees: float
    uf_kv: float
    uv_kv: float

class LineCommutatedConverter(TypedDict, total=False):
    component: ComponentIdentity
    dc_converter_unit: ComponentIdentity
    dc_terminal1: DcTerminal
    dc_terminal2: DcTerminal
    base_apparent_power_mva: float
    minimum_active_power_mw: float
    maximum_active_power_mw: float
    minimum_dc_voltage_kv: float
    maximum_dc_voltage_kv: float
    rated_dc_voltage_kv: float
    valve_u0_kv: float
    number_of_valves: int
    idle_loss_mw: float
    switching_loss_mw_per_ampere: float
    resistive_loss_ohm: float
    control_mode: AcDcConverterControlMode
    active_power_at_pcc_mw: float
    reactive_power_at_pcc_mvar: float
    target_active_power_mw: float
    target_dc_voltage_kv: float
    pcc_terminal: TerminalReference
    droop_curve: DroopCurve
    reactive_model: Literal["fixed_power_factor", "calculated_power_factor"]
    power_factor: float
    operating_mode: Literal["rectifier", "inverter"]
    rated_dc_current_a: float
    minimum_alpha_degrees: float
    maximum_alpha_degrees: float
    minimum_gamma_degrees: float
    maximum_gamma_degrees: float
    target_alpha_degrees: float
    target_gamma_degrees: float
    target_dc_current_a: float
    pole_loss_active_power_mw: float
    dc_current_a: float
    ac_voltage_kv: float
    dc_voltage_kv: float
    alpha_degrees: float
    gamma_degrees: float

class DetailedConnectivity(TypedDict, total=False):
    omitted_fields: List[OmittedField]
    component_metadata: List[ComponentMetadata]
    subnetworks: List[Subnetwork]
    substations: List[Substation]
    voltage_levels: List[VoltageLevel]
    bus_breaker_buses: List[BusBreakerBus]
    calculated_buses: List[CalculatedBus]
    connectivity_nodes: List[ConnectivityNode]
    busbar_sections: List[BusbarSection]
    junctions: List[Junction]
    terminals: List[DetailedTerminal]
    switches: List[TopologySwitch]
    internal_connections: List[InternalConnection]
    operational_limit_groups: List[OperationalLimitGroup]
    tap_changers: List[TapChanger]
    equipment_reactive_limits: List[EquipmentReactiveLimits]
    boundary_lines: List[BoundaryLine]
    tie_lines: List[TieLine]
    dc_converter_units: List[DcConverterUnit]
    dc_topological_nodes: List[DcTopologicalNode]
    dc_nodes: List[DcNode]
    dc_grounds: List[DcGround]
    dc_busbars: List[DcBusbar]
    dc_lines: List[DcLine]
    dc_series_devices: List[DcSeriesDevice]
    dc_switches: List[DcSwitch]
    voltage_source_converters: List[VoltageSourceConverter]
    line_commutated_converters: List[LineCommutatedConverter]

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
        "psse-rawx",
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
        "xiidm",
        "jiidm",
        "cgmes",
        "ucte",
        "ieee-cdf",
    ]
    n_buses: int
    n_branches: int
    n_generators: int
    n_loads: int
    n_shunts: int
    n_static_var_compensators: int
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
    static_var_compensators: List[StaticVarCompensator]
    branches: List[Branch]
    switches: List[Switch]
    generators: List[Gen]
    storage: List[Storage]
    hvdc: List[Dict[str, Any]]
    transformers_3w: List[ThreeWindingTransformer]
    areas: List[Dict[str, Any]]
    detailed_connectivity: Optional[DetailedConnectivity]
    def reference_bus_index(self) -> int: ...
    def reference_bus_indices(self) -> List[int]: ...
    def calc_connectivity_report(self) -> Dict[str, Any]: ...
    def to_geo_layer(self) -> Dict[str, Any]: ...
    def apply_geo_layer(
        self, text: str, name_hint: Optional[str] = ...
    ) -> Tuple["BalancedNetwork", Dict[str, Any]]: ...
    def calc_bprime_matrix(
        self, scheme: Scheme = ..., *, skip_zero_impedance: bool = ...
    ) -> Any: ...
    def calc_incidence_matrix(self, formula: str = ...) -> Any: ...
    def calc_bus_susceptance_matrix(self, formula: str = ...) -> Any: ...
    def calc_branch_susceptances(self, formula: str = ...) -> Any: ...
    def calc_branch_flow_matrix(self, formula: str = ...) -> Any: ...
    def calc_branch_phase_shift_injection(self, formula: str = ...) -> Any: ...
    def calc_bus_phase_shift_injection(self, formula: str = ...) -> Any: ...
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
    def to_normalized(
        self,
        *,
        clamp_angle_bounds: bool = ...,
        angle_bound_pad: Optional[float] = ...,
    ) -> "BalancedNetwork": ...
    def to_ppc(self) -> Dict[str, Any]: ...
    def to_networkx(self) -> Any: ...

@dataclass(frozen=True)
class Artifact:
    name: str
    data: Optional[bytes]
    path: Optional[str]
    @property
    def text(self) -> str: ...

@dataclass(frozen=True)
class EmitResult:
    artifacts: Tuple[Artifact, ...]
    layout: Literal["file", "directory"]
    fidelity: Literal["exact_same_format", "canonical"]
    diagnostics: Tuple[Diagnostic, ...]
    @property
    def text(self) -> Optional[str]: ...

class FormatInfo(NamedTuple):
    token: str
    extension: Optional[str]
    is_directory: bool
    can_emit: bool

# Any format name or alias the Rust hub accepts (e.g. "matpower"/"m",
# "psse"/"raw"). Kept as `str` so aliases type-check; the binding validates it.
Format = str

from . import dist as dist
from .dist import MulticonductorNetwork as MulticonductorNetwork

def parse_display(path: Any, format: Optional[Format] = ...) -> DisplayData: ...
def resolve_format(name: str) -> Optional[FormatInfo]: ...
def versions() -> Any: ...
def from_ppc(ppc: Dict[str, Any]) -> BalancedNetwork: ...
def parse_geo(text: str, name_hint: Optional[str] = ...) -> Dict[str, Any]: ...

class _TypedValue:
    module: PioModule[Any]
    def __init__(self, module: PioModule[Any]) -> None: ...

_TypedT = TypeVar("_TypedT", bound=_TypedValue)

@dataclass(frozen=True)
class TimePoint:
    label: str
    duration_seconds: Optional[float] = ...

@dataclass(frozen=True)
class Scenario:
    id: str
    probability: Optional[float] = ...

class TimeSeries(_TypedValue, Sequence[_T]):
    def __init__(
        self, values: Sequence[_T], *, time_points: Sequence[TimePoint]
    ) -> None: ...
    @property
    def time_points(self) -> Tuple[TimePoint, ...]: ...
    def __len__(self) -> int: ...
    @overload
    def __getitem__(self, index: int) -> _T: ...
    @overload
    def __getitem__(self, index: slice) -> list[_T]: ...
    def __iter__(self) -> Iterator[_T]: ...

class ScenarioSet(_TypedValue, Mapping[str, _T]):
    def __init__(
        self,
        values: Mapping[str, _T],
        *,
        probabilities: Optional[Mapping[str, float]] = ...,
    ) -> None: ...
    @property
    def scenarios(self) -> Tuple[Scenario, ...]: ...
    def __len__(self) -> int: ...
    def __iter__(self) -> Iterator[str]: ...
    def __contains__(self, scenario: object) -> bool: ...
    def __getitem__(self, scenario: str) -> _T: ...

class OperatingPoint(_TypedValue): ...

class _BalancedCalculation(_TypedValue):
    @property
    def network(self) -> BalancedNetwork: ...

class _MulticonductorCalculation(_TypedValue):
    @property
    def network(self) -> MulticonductorNetwork: ...

class _CalculationSolution(_TypedValue):
    @property
    def instance(self) -> _TypedValue: ...

class DcPfInstance(_BalancedCalculation): ...
class AcPfInstance(_BalancedCalculation): ...
class DcOpfInstance(_BalancedCalculation): ...
class AcOpfInstance(_BalancedCalculation): ...
class McAcPfInstance(_MulticonductorCalculation): ...
class McAcOpfInstance(_MulticonductorCalculation): ...
class AcScucInstance(_BalancedCalculation):
    @property
    def inputs(self) -> ScucInputs: ...
class DcPfSolution(_BalancedCalculation, _CalculationSolution):
    @property
    def instance(self) -> DcPfInstance: ...
class AcPfSolution(_BalancedCalculation, _CalculationSolution):
    @property
    def instance(self) -> AcPfInstance: ...
class DcOpfSolution(_BalancedCalculation, _CalculationSolution):
    @property
    def instance(self) -> DcOpfInstance: ...

class GeoLayer(_TypedValue):
    @property
    def geojson(self) -> str: ...

class AcOpfSolution(_BalancedCalculation, _CalculationSolution):
    @property
    def instance(self) -> AcOpfInstance: ...
class SocwrOpfSolution(_BalancedCalculation, _CalculationSolution):
    @property
    def instance(self) -> AcOpfInstance: ...
class McAcPfSolution(_MulticonductorCalculation, _CalculationSolution):
    @property
    def instance(self) -> McAcPfInstance: ...
class McAcOpfSolution(_MulticonductorCalculation, _CalculationSolution):
    @property
    def instance(self) -> McAcOpfInstance: ...
class AcScucSolution(_BalancedCalculation, _CalculationSolution):
    @property
    def instance(self) -> AcScucInstance: ...
    @property
    def termination(
        self,
    ) -> Literal[
        "converged",
        "iteration_limit",
        "infeasible",
        "unbounded",
        "failed",
        "not_reported",
    ]: ...
    @property
    def residuals(self) -> Residuals: ...
    @property
    def producer(self) -> Optional[str]: ...
    @property
    def network_outputs(self) -> ScucNetworkOutputs: ...
    @property
    def device_outputs(self) -> ScucDeviceOutputs: ...
    @property
    def objective(self) -> Optional[float]: ...

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
    @classmethod
    @overload
    def from_value(cls, value: _TypedT) -> PioModule[_TypedT]: ...
    @property
    def value(self) -> _T: ...
    @property
    def diagnostics(self) -> List[Diagnostic]: ...
    def to_balanced_report(self, base_mva: float = ...) -> Any: ...
    def to_balanced(self, base_mva: float = ...) -> PioModule[BalancedNetwork]: ...
    def to_dc_pf_instance(self) -> PioModule[DcPfInstance]: ...
    def to_ac_pf_instance(self) -> PioModule[AcPfInstance]: ...
    def to_dc_opf_instance(self) -> PioModule[DcOpfInstance]: ...
    def to_ac_opf_instance(self) -> PioModule[AcOpfInstance]: ...
    def to_mc_ac_pf_instance(self) -> PioModule[McAcPfInstance]: ...
    def to_mc_ac_opf_instance(self) -> PioModule[McAcOpfInstance]: ...

def apply_updates(
    target: Union[
        PioModule[Any],
        BalancedNetwork,
        dist.MulticonductorNetwork,
        _TypedValue,
    ],
    updates: Union[
        Iterable[OperatingPointUpdate],
        Iterable[NetworkUpdate],
        Iterable[CalculationUpdate],
    ],
) -> UpdateReport: ...

def apply_bus_load_active_power(
    module: PioModule[Any],
    bus_id: int,
    total: ActivePower,
    *,
    allocation: Literal["equal", "proportional_to_current_active_power"] = ...,
) -> UpdateReport: ...

def parse(
    source: Any, *, format: Optional[Format] = ..., name: Optional[str] = ...
) -> PioModule[Any]: ...
def emit(
    module: PioModule[Any], format: Format, destination: Optional[Any] = ...
) -> EmitResult: ...
def serialize(module: PioModule[Any], destination: Optional[Any] = ...) -> EmitResult: ...
def deserialize(source: Any) -> PioModule[Any]: ...
def features() -> Dict[str, bool]: ...
