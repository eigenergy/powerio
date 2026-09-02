//! PowSybl XIIDM 1.12 through 1.17 XML reader and 1.17 writer.

#![allow(
    clippy::format_push_string,
    reason = "XML emission appends fixed record fragments to one owned output buffer"
)]
#![allow(
    clippy::too_many_lines,
    reason = "the streaming event and table mappers keep XIIDM fields beside their source records"
)]

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use powerio_core::ComponentId;
use quick_xml::NsReader;
use quick_xml::events::{BytesDecl, BytesStart, Event};
use quick_xml::name::ResolveResult;

use crate::diagnostics::{Diagnostics, codes};
use crate::format::TextEmission;
use crate::network::{
    AcDcConverterControlMode, ActivePowerControl, Area, BalancedNetwork, BoundaryLine,
    BoundaryLineGeneration, Branch, BranchCharging, BranchCurrentRatings, BranchRatingSet,
    BranchSolution, Bus, BusBreakerBus, BusId, BusType, BusbarSection, CalculatedBus, CaseMetadata,
    ComponentAlias, ComponentMetadata, ConnectivityNode, CurveStyle, DcGround, DcLine, DcNode,
    DcSwitch, DcSwitchKind, DcTerminal, DetailedConnectivity, DroopCurve, DroopCurveSegment,
    EquipmentReactiveLimits, GeneratorEnergySource, Hvdc, HvdcConverter, HvdcConverterKind,
    HvdcConvertersMode, Impedance, InternalConnection, LineCommutatedConverter,
    LineCommutatedConverterReactiveModel, Load, LoadVoltageModel, LoadingLimits,
    MinMaxReactiveLimits, OmittedField, OmittedFieldName, OperationalLimitGroup,
    ReactiveCapabilityCurve, ReactiveCapabilityCurvePoint, ReactiveLimits, Shunt, ShuntBlock,
    SourceFormat, StaticVarCompensator, StaticVarCompensatorRegulationMode, Storage, Subnetwork,
    Substation, Switch, SwitchKind, SwitchedShuntControl, SwitchedShuntMode,
    TapChanger as NetworkTapChanger, TapChangerKind, TapChangerRegulationMode,
    TapChangerStep as NetworkTapChangerStep, TemporaryLimit, Terminal, TerminalReference, TieLine,
    TopologyEndpoint, TopologyKind, TopologySwitch, Transformer3W, TransformerControlMode,
    VoltageLevel, VoltageSourceConverter, Winding,
};
use crate::{Error, Generator, Result};

const FORMAT: &str = "XIIDM XML";
/// Retained three winding transformer `ratedU0` when it differs from
/// `ratedU1`: the voltage base the source stated its leg impedances on.
const XIIDM_RATED_U0_EXTRA: &str = "xiidm_rated_u0";

const NAMESPACE: &str = "http://www.powsybl.org/schema/iidm/1_17";
const EQUIPMENT_NAMESPACE: &str = "http://www.powsybl.org/schema/iidm/equipment/1_17";
const NAMESPACE_PREFIX: &str = "http://www.powsybl.org/schema/iidm/";
const EQUIPMENT_NAMESPACE_PREFIX: &str = "http://www.powsybl.org/schema/iidm/equipment/";
const ACTIVE_POWER_CONTROL_NAMESPACE_V1_0: &str =
    "http://www.itesla_project.eu/schema/iidm/ext/active_power_control/1_0";
const ACTIVE_POWER_CONTROL_NAMESPACE_V1_1: &str =
    "http://www.powsybl.org/schema/iidm/ext/active_power_control/1_1";
const ACTIVE_POWER_CONTROL_NAMESPACE_V1_2: &str =
    "http://www.powsybl.org/schema/iidm/ext/active_power_control/1_2";
const DEFAULT_BASE_MVA: f64 = 100.0;
const DEFAULT_CASE_DATE: &str = "1970-01-01T00:00:00.000Z";
const DEFAULT_SOURCE_MODEL_FORMAT: &str = "PowerIO";
const DEFAULT_VALIDATION_LEVEL: &str = "STEADY_STATE_HYPOTHESIS";
const MAX_XIIDM_BYTES: usize = 64 << 20;
const MAX_XIIDM_ELEMENT_DEPTH: usize = 256;

pub(crate) fn looks_like_xiidm(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(16_384)];
    let text = String::from_utf8_lossy(head);
    (text.contains(NAMESPACE_PREFIX) || text.contains(EQUIPMENT_NAMESPACE_PREFIX))
        && (text.contains(":network") || text.contains("<network"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum XiidmVersion {
    V1_12,
    V1_13,
    V1_14,
    V1_15,
    V1_16,
    V1_17,
}

impl XiidmVersion {
    fn from_suffix(suffix: &str) -> Option<Self> {
        Some(match suffix {
            "1_12" => Self::V1_12,
            "1_13" => Self::V1_13,
            "1_14" => Self::V1_14,
            "1_15" => Self::V1_15,
            "1_16" => Self::V1_16,
            "1_17" => Self::V1_17,
            _ => return None,
        })
    }

    fn label(self) -> &'static str {
        match self {
            Self::V1_12 => "1.12",
            Self::V1_13 => "1.13",
            Self::V1_14 => "1.14",
            Self::V1_15 => "1.15",
            Self::V1_16 => "1.16",
            Self::V1_17 => "1.17",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum XiidmValidation {
    Valid,
    Equipment,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActivePowerControlVersion {
    V1_0,
    V1_1,
    V1_2,
}

impl ActivePowerControlVersion {
    fn from_namespace(namespace: Option<&str>) -> Option<Self> {
        Some(match namespace? {
            ACTIVE_POWER_CONTROL_NAMESPACE_V1_0 => Self::V1_0,
            ACTIVE_POWER_CONTROL_NAMESPACE_V1_1 => Self::V1_1,
            ACTIVE_POWER_CONTROL_NAMESPACE_V1_2 => Self::V1_2,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct XiidmNamespace {
    version: XiidmVersion,
    validation: XiidmValidation,
}

impl XiidmNamespace {
    fn from_uri(uri: &str) -> Option<Self> {
        let (validation, suffix) =
            if let Some(suffix) = uri.strip_prefix(EQUIPMENT_NAMESPACE_PREFIX) {
                (XiidmValidation::Equipment, suffix)
            } else {
                (XiidmValidation::Valid, uri.strip_prefix(NAMESPACE_PREFIX)?)
            };
        Some(Self {
            version: XiidmVersion::from_suffix(suffix)?,
            validation,
        })
    }

    fn is_equipment(self) -> bool {
        self.validation == XiidmValidation::Equipment
    }

    fn matches_uri(self, uri: Option<&str>) -> bool {
        uri.and_then(Self::from_uri) == Some(self)
    }
}

fn is_xiidm_namespace_uri(uri: &str) -> bool {
    uri.strip_prefix(EQUIPMENT_NAMESPACE_PREFIX)
        .or_else(|| uri.strip_prefix(NAMESPACE_PREFIX))
        .is_some_and(|version| {
            let Some((major, minor)) = version.split_once('_') else {
                return false;
            };
            !major.is_empty()
                && !minor.is_empty()
                && major.bytes().all(|value| value.is_ascii_digit())
                && minor.bytes().all(|value| value.is_ascii_digit())
        })
}

type Attrs = BTreeMap<String, String>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RawTopologyKind {
    BusBreaker,
    NodeBreaker,
}

#[derive(Clone, Debug)]
struct RawSubstation {
    id: String,
    country: Option<String>,
    operator: Option<String>,
    geographical_tags: Vec<String>,
}

#[derive(Clone, Debug)]
struct RawSubnetwork {
    component: ComponentId,
    parent: ComponentId,
    case_metadata: CaseMetadata,
    components: Vec<ComponentId>,
}

#[derive(Clone, Debug)]
struct RawVoltageLevel {
    id: String,
    substation: Option<String>,
    nominal_v: f64,
    low_voltage_limit: Option<f64>,
    high_voltage_limit: Option<f64>,
    topology: RawTopologyKind,
}

#[derive(Clone, Debug)]
struct RawArea {
    id: String,
    name: Option<String>,
    area_type: String,
    interchange_target: Option<f64>,
    voltage_levels: Vec<String>,
    boundary_count: usize,
}

#[derive(Clone, Debug)]
struct RawBus {
    id: Option<String>,
    voltage_level: String,
    nodes: Vec<i32>,
    v: Option<f64>,
    angle: Option<f64>,
}

#[derive(Clone, Debug)]
enum RawEndpoint {
    Bus(String),
    Node(i32),
}

#[derive(Clone, Debug)]
struct RawTerminalReference {
    id: String,
    terminal: u8,
}

#[derive(Clone, Debug)]
struct RawSwitch {
    id: String,
    voltage_level: String,
    kind: SwitchKind,
    endpoint1: RawEndpoint,
    endpoint2: RawEndpoint,
    open: bool,
    retained: bool,
}

#[derive(Clone, Debug)]
struct RawBusbarSection {
    id: String,
    voltage_level: String,
    node: i32,
}

#[derive(Clone, Debug)]
struct RawInternalConnection {
    voltage_level: String,
    node1: i32,
    node2: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EquipmentKind {
    Load,
    Generator,
    Battery,
    Shunt,
    StaticVarCompensator,
    BoundaryLine,
    Line,
    Transformer,
    ThreeWindingTransformer,
    VscConverterStation,
    LccConverterStation,
}

impl EquipmentKind {
    fn component_type(self) -> &'static str {
        match self {
            Self::Load => "load",
            Self::Generator => "generator",
            Self::Battery => "storage",
            Self::Shunt => "shunt",
            Self::StaticVarCompensator => "static_var_compensator",
            Self::BoundaryLine => "boundary_line",
            Self::Line | Self::Transformer => "branch",
            Self::ThreeWindingTransformer => "transformer_3w",
            Self::VscConverterStation | Self::LccConverterStation => "hvdc_converter",
        }
    }

    fn terminal_count(self) -> u8 {
        match self {
            Self::Load
            | Self::Generator
            | Self::Battery
            | Self::Shunt
            | Self::StaticVarCompensator
            | Self::BoundaryLine
            | Self::VscConverterStation
            | Self::LccConverterStation => 1,
            Self::Line | Self::Transformer => 2,
            Self::ThreeWindingTransformer => 3,
        }
    }
}

fn tap_tag(tag: &str) -> Option<ActiveTap> {
    let (kind, winding) = match tag {
        "ratioTapChanger" | "ratioTapChanger1" => (TapKind::Ratio, 0),
        "phaseTapChanger" | "phaseTapChanger1" => (TapKind::Phase, 0),
        "ratioTapChanger2" => (TapKind::Ratio, 1),
        "phaseTapChanger2" => (TapKind::Phase, 1),
        "ratioTapChanger3" => (TapKind::Ratio, 2),
        "phaseTapChanger3" => (TapKind::Phase, 2),
        _ => return None,
    };
    Some(ActiveTap { kind, winding })
}

fn operational_limits_side(tag: &str) -> Option<u8> {
    match tag {
        "operationalLimitsGroup" | "operationalLimitsGroup1" => Some(1),
        "operationalLimitsGroup2" => Some(2),
        "operationalLimitsGroup3" => Some(3),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TapKind {
    Ratio,
    Phase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ActiveTap {
    kind: TapKind,
    winding: usize,
}

#[derive(Clone, Copy, Debug)]
struct TapStep {
    rho: f64,
    alpha: f64,
    r_percent: f64,
    x_percent: f64,
    g_percent: f64,
    b_percent: f64,
}

#[derive(Clone, Debug)]
struct TapChanger {
    tap_position: Option<i32>,
    solved_tap_position: Option<i32>,
    low_tap_position: i32,
    load_tap_changing_capabilities: bool,
    regulating: bool,
    regulation_mode: Option<TapChangerRegulationMode>,
    regulation_value: Option<f64>,
    target_deadband: Option<f64>,
    regulation_terminal: Option<RawTerminalReference>,
    steps: Vec<TapStep>,
}

impl TapChanger {
    fn assigned_step(&self) -> Option<TapStep> {
        let offset = self.tap_position?.checked_sub(self.low_tap_position)?;
        usize::try_from(offset)
            .ok()
            .and_then(|index| self.steps.get(index).copied())
    }

    #[allow(clippy::float_cmp)]
    fn calculation_step(&self, kind: TapKind) -> Option<TapStep> {
        self.assigned_step().or_else(|| {
            self.tap_position.is_none().then(|| {
                self.steps
                    .iter()
                    .copied()
                    .find(|step| match kind {
                        TapKind::Ratio => step.rho == 1.0,
                        TapKind::Phase => step.rho == 1.0 && step.alpha == 0.0,
                    })
                    .or_else(|| self.steps.first().copied())
            })?
        })
    }
}

#[derive(Clone, Debug)]
enum RawLoadModel {
    Zip {
        c0p: f64,
        c1p: f64,
        c2p: f64,
        c0q: f64,
        c1q: f64,
        c2q: f64,
    },
    Exponential {
        np: f64,
        nq: f64,
    },
}

#[derive(Clone, Debug)]
struct RawEquipment {
    kind: EquipmentKind,
    id: String,
    voltage_level: Option<String>,
    attrs: Attrs,
    reactive_limits: Option<ReactiveLimits>,
    load_model: Option<RawLoadModel>,
    shunt_linear: Option<(f64, f64, u32)>,
    shunt_sections: Vec<(f64, f64)>,
    shunt_conductance_omitted: bool,
    ratio_tap: Option<TapChanger>,
    phase_tap: Option<TapChanger>,
    winding_ratio_taps: [Option<TapChanger>; 3],
    winding_phase_taps: [Option<TapChanger>; 3],
    active_power_control: Option<ActivePowerControl>,
    regulating_terminal: Option<RawTerminalReference>,
    operational_limits: Vec<RawOperationalLimitsGroup>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LoadingLimitKind {
    ActivePower,
    ApparentPower,
    Current,
}

#[derive(Clone, Debug)]
struct RawTemporaryLimit {
    name: String,
    acceptable_duration: Option<i32>,
    value: Option<f64>,
    fictitious: bool,
}

#[derive(Clone, Debug, Default)]
struct RawLoadingLimits {
    permanent_limit: Option<f64>,
    permanent_name: Option<String>,
    temporary_limits: Vec<RawTemporaryLimit>,
}

#[derive(Clone, Debug)]
struct RawOperationalLimitsGroup {
    id: String,
    side: u8,
    properties: BTreeMap<String, String>,
    active_power: Option<RawLoadingLimits>,
    apparent_power: Option<RawLoadingLimits>,
    current: Option<RawLoadingLimits>,
}

#[derive(Clone, Debug)]
struct RawHvdcLine {
    id: String,
    attrs: Attrs,
}

#[derive(Clone, Debug)]
struct RawTieLine {
    id: String,
    boundary_line1: String,
    boundary_line2: String,
}

#[derive(Clone, Debug)]
struct RawAcDcConverter {
    id: String,
    voltage_level: String,
    attrs: Attrs,
    pcc_terminal: Option<RawTerminalReference>,
    droop_curve: Option<DroopCurve>,
}

#[derive(Clone, Debug)]
struct RawVoltageSourceConverter {
    common: RawAcDcConverter,
    reactive_limits: Option<ReactiveLimits>,
}

#[derive(Clone, Debug)]
struct RawLineCommutatedConverter {
    common: RawAcDcConverter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RawAcDcConverterIndex {
    VoltageSource(usize),
    LineCommutated(usize),
}

#[derive(Clone, Debug)]
struct Frame {
    tag: String,
    component: Option<ComponentId>,
    equipment: Option<usize>,
    ac_dc_converter: Option<RawAcDcConverterIndex>,
}

#[derive(Clone, Debug)]
struct PendingAlias {
    component: ComponentId,
    alias_type: Option<String>,
    value: String,
}

#[derive(Default)]
struct ParsedXiidm {
    id: Option<String>,
    case_metadata: CaseMetadata,
    namespace: Option<XiidmNamespace>,
    root_seen: bool,
    current_subnetwork: Option<usize>,
    current_substation: Option<String>,
    current_voltage_level: Option<String>,
    current_area: Option<usize>,
    current_tap: Option<ActiveTap>,
    current_loading_limit: Option<LoadingLimitKind>,
    current_extension_target: Option<String>,
    frames: Vec<Frame>,
    pending_alias: Option<PendingAlias>,
    ids: HashSet<String>,
    metadata: BTreeMap<ComponentId, ComponentMetadata>,
    subnetworks: Vec<RawSubnetwork>,
    substations: Vec<RawSubstation>,
    voltage_levels: Vec<RawVoltageLevel>,
    areas: Vec<RawArea>,
    buses: Vec<RawBus>,
    switches: Vec<RawSwitch>,
    busbar_sections: Vec<RawBusbarSection>,
    internal_connections: Vec<RawInternalConnection>,
    equipment: Vec<RawEquipment>,
    equipment_by_id: HashMap<String, usize>,
    tie_lines: Vec<RawTieLine>,
    hvdc_lines: Vec<RawHvdcLine>,
    dc_nodes: Vec<DcNode>,
    dc_grounds: Vec<DcGround>,
    dc_lines: Vec<DcLine>,
    dc_switches: Vec<DcSwitch>,
    voltage_source_converters: Vec<RawVoltageSourceConverter>,
    line_commutated_converters: Vec<RawLineCommutatedConverter>,
    slack_terminal: Option<String>,
}

pub(crate) fn parse_xiidm_source(
    text: &str,
    diagnostics: &mut Diagnostics,
) -> Result<BalancedNetwork> {
    parse_xiidm_bytes(text.as_bytes(), diagnostics)
}

pub(crate) fn parse_xiidm_bytes(
    bytes: &[u8],
    diagnostics: &mut Diagnostics,
) -> Result<BalancedNetwork> {
    if bytes.len() > MAX_XIIDM_BYTES {
        return Err(format_error(format!(
            "XIIDM XML exceeds the {MAX_XIIDM_BYTES} byte input limit"
        )));
    }
    reject_xml_entities(bytes)?;
    let mut reader = NsReader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    reader.config_mut().check_end_names = true;
    reader.config_mut().expand_empty_elements = false;
    let mut parsed = ParsedXiidm::default();
    let mut skipped_extension_depth = 0_usize;
    let mut element_depth = 0_usize;
    let mut encoding_checked = false;
    let mut buffer = Vec::new();
    loop {
        let (resolved, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| format_error(format!("malformed XML: {error}")))?;
        let namespace = resolved_namespace(resolved)?;
        if !encoding_checked && !matches!(&event, Event::Decl(_)) {
            validate_xml_encoding(None, reader.decoder())?;
            encoding_checked = true;
        }
        match event {
            Event::Start(element) => {
                element_depth = element_depth
                    .checked_add(1)
                    .ok_or_else(|| format_error("XIIDM XML element depth overflow"))?;
                if element_depth > MAX_XIIDM_ELEMENT_DEPTH {
                    return Err(format_error(format!(
                        "XIIDM XML exceeds the {MAX_XIIDM_ELEMENT_DEPTH} element nesting limit"
                    )));
                }
                if skipped_extension_depth > 0 {
                    skipped_extension_depth += 1;
                    buffer.clear();
                    continue;
                }
                let tag = local_name(element.name().as_ref())?;
                let active_power_control = tag == "activePowerControl"
                    && ActivePowerControlVersion::from_namespace(namespace.as_deref()).is_some();
                if parsed.current_extension_target.is_some()
                    && tag != "extension"
                    && !active_power_control
                {
                    if namespace.as_deref().is_none_or(is_xiidm_namespace_uri) {
                        return Err(format_error(format!(
                            "XIIDM model element `{tag}` appears inside an extension"
                        )));
                    }
                    diagnostics.push(
                        &codes::READ_XIIDM_ELEMENT_UNMAPPED,
                        format!(
                            "XIIDM extension element `{tag}` on `{}` is retained only by exact same format emission",
                            parsed.current_extension_target.as_deref().unwrap_or_default()
                        ),
                    );
                    skipped_extension_depth = 1;
                    buffer.clear();
                    continue;
                }
                parsed.start(
                    &tag,
                    namespace.as_deref(),
                    attributes(&element, reader.decoder())?,
                    false,
                    diagnostics,
                )?;
            }
            Event::Empty(element) => {
                if element_depth >= MAX_XIIDM_ELEMENT_DEPTH {
                    return Err(format_error(format!(
                        "XIIDM XML exceeds the {MAX_XIIDM_ELEMENT_DEPTH} element nesting limit"
                    )));
                }
                if skipped_extension_depth > 0 {
                    buffer.clear();
                    continue;
                }
                let tag = local_name(element.name().as_ref())?;
                let active_power_control = tag == "activePowerControl"
                    && ActivePowerControlVersion::from_namespace(namespace.as_deref()).is_some();
                if parsed.current_extension_target.is_some()
                    && tag != "extension"
                    && !active_power_control
                {
                    if namespace.as_deref().is_none_or(is_xiidm_namespace_uri) {
                        return Err(format_error(format!(
                            "XIIDM model element `{tag}` appears inside an extension"
                        )));
                    }
                    diagnostics.push(
                        &codes::READ_XIIDM_ELEMENT_UNMAPPED,
                        format!(
                            "XIIDM extension element `{tag}` on `{}` is retained only by exact same format emission",
                            parsed.current_extension_target.as_deref().unwrap_or_default()
                        ),
                    );
                    buffer.clear();
                    continue;
                }
                parsed.start(
                    &tag,
                    namespace.as_deref(),
                    attributes(&element, reader.decoder())?,
                    true,
                    diagnostics,
                )?;
            }
            Event::End(element) => {
                if skipped_extension_depth > 0 {
                    skipped_extension_depth -= 1;
                    element_depth = element_depth.saturating_sub(1);
                    buffer.clear();
                    continue;
                }
                parsed.end(&local_name(element.name().as_ref())?)?;
                element_depth = element_depth.saturating_sub(1);
            }
            Event::Text(value) => {
                if skipped_extension_depth == 0
                    && let Some(alias) = &mut parsed.pending_alias
                {
                    alias.value.push_str(
                        &value
                            .decode()
                            .map_err(|error| format_error(error.to_string()))?,
                    );
                }
            }
            Event::CData(value) => {
                if skipped_extension_depth == 0
                    && let Some(alias) = &mut parsed.pending_alias
                {
                    alias.value.push_str(
                        &value
                            .decode()
                            .map_err(|error| format_error(error.to_string()))?,
                    );
                }
            }
            Event::DocType(_) | Event::GeneralRef(_) => {
                return Err(format_error(
                    "DTD declarations and entity references are not accepted",
                ));
            }
            Event::Decl(declaration) => {
                validate_xml_encoding(Some(&declaration), reader.decoder())?;
                encoding_checked = true;
            }
            Event::PI(_) | Event::Comment(_) => {}
            Event::Eof => break,
        }
        buffer.clear();
    }
    parsed.finish(diagnostics)
}

fn validate_xml_encoding(
    declaration: Option<&BytesDecl<'_>>,
    decoder: quick_xml::encoding::Decoder,
) -> Result<()> {
    let declared_label = declaration
        .and_then(BytesDecl::encoding)
        .transpose()
        .map_err(|error| format_error(format!("malformed XML encoding declaration: {error}")))?
        .map(|label| {
            std::str::from_utf8(&label)
                .map(str::to_owned)
                .map_err(|error| {
                    format_error(format!("XML encoding name is not valid ASCII: {error}"))
                })
        })
        .transpose()?;
    let encoding = decoder.encoding();
    let supported = encoding.name() == "UTF-8" || encoding.is_single_byte();
    let unknown =
        declared_label.is_some() && declaration.is_some_and(|value| value.encoder().is_none());
    if unknown || !supported {
        let name = declared_label.as_deref().unwrap_or_else(|| encoding.name());
        return Err(format_error(format!(
            "unsupported XIIDM XML encoding `{name}`; use UTF-8 or a recognized single-byte encoding"
        )));
    }
    Ok(())
}

fn reject_xml_entities(bytes: &[u8]) -> Result<()> {
    let upper = String::from_utf8_lossy(bytes).to_ascii_uppercase();
    if upper.contains("<!DOCTYPE") || upper.contains("<!ENTITY") {
        return Err(format_error(
            "DTD declarations and entity definitions are not accepted",
        ));
    }
    Ok(())
}

impl ParsedXiidm {
    fn start(
        &mut self,
        tag: &str,
        namespace: Option<&str>,
        attrs: Attrs,
        empty: bool,
        diagnostics: &mut Diagnostics,
    ) -> Result<()> {
        if !self.root_seen && tag != "network" {
            return Err(format_error("root element is not `network`"));
        }
        if self.root_seen {
            let recognized_extension = self.current_extension_target.is_some()
                && tag == "activePowerControl"
                && ActivePowerControlVersion::from_namespace(namespace).is_some();
            if !recognized_extension
                && !self
                    .namespace
                    .is_some_and(|selected| selected.matches_uri(namespace))
            {
                return Err(format_error(format!(
                    "XIIDM element `{tag}` uses XML namespace `{}` instead of the root network namespace",
                    namespace.unwrap_or("(none)")
                )));
            }
        }
        let mut frame = Frame {
            tag: tag.to_owned(),
            component: None,
            equipment: None,
            ac_dc_converter: None,
        };
        match tag {
            "network" => {
                let root = !self.root_seen;
                self.read_network(&attrs, &mut frame, diagnostics)?;
                if root
                    && !self
                        .namespace
                        .is_some_and(|selected| selected.matches_uri(namespace))
                {
                    return Err(format_error(format!(
                        "XIIDM root element uses XML namespace `{}` instead of its declared XIIDM namespace",
                        namespace.unwrap_or("(none)")
                    )));
                }
            }
            "extension" => {
                if self
                    .frames
                    .last()
                    .is_none_or(|frame| frame.tag != "network")
                {
                    return Err(format_error(
                        "XIIDM extension must be a direct child of a network",
                    ));
                }
                if self.current_extension_target.is_some() {
                    return Err(format_error(
                        "nested XIIDM extension elements are not supported",
                    ));
                }
                self.current_extension_target = Some(required_text(&attrs, "id")?.to_owned());
            }
            "activePowerControl" => {
                let version =
                    ActivePowerControlVersion::from_namespace(namespace).ok_or_else(|| {
                        format_error("activePowerControl uses an unsupported XML namespace")
                    })?;
                self.read_active_power_control(&attrs, version)?;
            }
            "dcNode" => {
                let id = required_text(&attrs, "id")?.to_owned();
                let component = self.register_component("dc_node", &id, &attrs)?;
                frame.component = Some(component.clone());
                let nominal_voltage_kv = required_f64(&attrs, "nominalV")?;
                if nominal_voltage_kv <= 0.0 {
                    return Err(format_error(format!(
                        "DC node `{id}` has nonpositive nominalV"
                    )));
                }
                self.dc_nodes.push(DcNode {
                    component,
                    nominal_voltage_kv: Some(nominal_voltage_kv),
                    dc_converter_unit: None,
                    dc_topological_node: None,
                    voltage_kv: optional_f64(&attrs, "v")?,
                });
            }
            "dcGround" => {
                let id = required_text(&attrs, "id")?.to_owned();
                let component = self.register_component("dc_ground", &id, &attrs)?;
                frame.component = Some(component.clone());
                let resistance_ohm = required_f64(&attrs, "r")?;
                if resistance_ohm < 0.0 {
                    return Err(format_error(format!(
                        "DC ground `{id}` has negative resistance"
                    )));
                }
                self.dc_grounds.push(DcGround {
                    component,
                    equipment_container: None,
                    dc_terminal: DcTerminal {
                        component: None,
                        sequence_number: None,
                        dc_node: Some(component_id("dc_node", required_text(&attrs, "dcNode")?)?),
                        dc_topological_node: None,
                        polarity: None,
                        connected: Some(required_bool(&attrs, "connected")?),
                        active_power_mw: optional_f64(&attrs, "dcP")?,
                        current_a: optional_f64(&attrs, "dcI")?,
                    },
                    rated_dc_voltage_kv: None,
                    resistance_ohm: Some(resistance_ohm),
                    inductance_h: None,
                });
            }
            "dcLine" => {
                let id = required_text(&attrs, "id")?.to_owned();
                let component = self.register_component("dc_line", &id, &attrs)?;
                frame.component = Some(component.clone());
                let resistance_ohm = required_f64(&attrs, "r")?;
                if resistance_ohm < 0.0 {
                    return Err(format_error(format!(
                        "DC line `{id}` has negative resistance"
                    )));
                }
                self.dc_lines.push(DcLine {
                    component,
                    equipment_container: None,
                    dc_terminal1: DcTerminal {
                        component: None,
                        sequence_number: None,
                        dc_node: Some(component_id("dc_node", required_text(&attrs, "dcNode1")?)?),
                        dc_topological_node: None,
                        polarity: None,
                        connected: Some(required_bool(&attrs, "connected1")?),
                        active_power_mw: optional_f64(&attrs, "dcP1")?,
                        current_a: optional_f64(&attrs, "dcI1")?,
                    },
                    dc_terminal2: DcTerminal {
                        component: None,
                        sequence_number: None,
                        dc_node: Some(component_id("dc_node", required_text(&attrs, "dcNode2")?)?),
                        dc_topological_node: None,
                        polarity: None,
                        connected: Some(required_bool(&attrs, "connected2")?),
                        active_power_mw: optional_f64(&attrs, "dcP2")?,
                        current_a: optional_f64(&attrs, "dcI2")?,
                    },
                    rated_dc_voltage_kv: None,
                    resistance_ohm: Some(resistance_ohm),
                    inductance_h: None,
                    capacitance_f: None,
                    length_km: None,
                });
            }
            "dcSwitch" => {
                let id = required_text(&attrs, "id")?.to_owned();
                let component = self.register_component("dc_switch", &id, &attrs)?;
                frame.component = Some(component.clone());
                let resistance_ohm = if self
                    .namespace
                    .is_some_and(|namespace| namespace.version >= XiidmVersion::V1_17)
                {
                    required_f64(&attrs, "r")?
                } else {
                    optional_f64(&attrs, "r")?.unwrap_or(0.0)
                };
                if resistance_ohm < 0.0 {
                    return Err(format_error(format!(
                        "DC switch `{id}` has negative resistance"
                    )));
                }
                let kind = match required_text(&attrs, "kind")? {
                    "BREAKER" => DcSwitchKind::Breaker,
                    "DISCONNECTOR" => DcSwitchKind::Disconnector,
                    other => {
                        return Err(format_error(format!(
                            "DC switch `{id}` has unknown kind `{other}`"
                        )));
                    }
                };
                self.dc_switches.push(DcSwitch {
                    component,
                    equipment_container: None,
                    dc_terminal1: DcTerminal {
                        component: None,
                        sequence_number: None,
                        dc_node: Some(component_id("dc_node", required_text(&attrs, "dcNode1")?)?),
                        dc_topological_node: None,
                        polarity: None,
                        connected: None,
                        active_power_mw: None,
                        current_a: None,
                    },
                    dc_terminal2: DcTerminal {
                        component: None,
                        sequence_number: None,
                        dc_node: Some(component_id("dc_node", required_text(&attrs, "dcNode2")?)?),
                        dc_topological_node: None,
                        polarity: None,
                        connected: None,
                        active_power_mw: None,
                        current_a: None,
                    },
                    kind,
                    rated_dc_voltage_kv: None,
                    open: Some(required_bool(&attrs, "open")?),
                    resistance_ohm: Some(resistance_ohm),
                });
            }
            "area" => {
                let id = required_text(&attrs, "id")?.to_owned();
                frame.component = Some(self.register_component("area", &id, &attrs)?);
                self.areas.push(RawArea {
                    id,
                    name: attrs.get("name").cloned(),
                    area_type: required_text(&attrs, "areaType")?.to_owned(),
                    interchange_target: optional_f64(&attrs, "interchangeTarget")?,
                    voltage_levels: Vec::new(),
                    boundary_count: 0,
                });
                self.current_area = Some(self.areas.len() - 1);
            }
            "voltageLevelRef" if self.current_area.is_some() => {
                let index = self.current_area.expect("matched area parent");
                self.areas[index]
                    .voltage_levels
                    .push(required_text(&attrs, "id")?.to_owned());
            }
            "areaBoundary" if self.current_area.is_some() => {
                let index = self.current_area.expect("matched area parent");
                self.areas[index].boundary_count += 1;
                diagnostics.push(
                    &codes::READ_XIIDM_FIELD_UNMAPPED,
                    format!(
                        "XIIDM area `{}` boundary `{}` is not represented in the balanced area table",
                        self.areas[index].id,
                        required_text(&attrs, "id")?
                    ),
                );
            }
            "substation" => {
                let id = required_text(&attrs, "id")?.to_owned();
                frame.component = Some(self.register_component("substation", &id, &attrs)?);
                self.current_substation = Some(id.clone());
                self.substations.push(RawSubstation {
                    id,
                    country: attrs.get("country").cloned(),
                    operator: attrs.get("tso").cloned(),
                    geographical_tags: attrs
                        .get("geographicalTags")
                        .map(|value| value.split(',').map(str::trim).map(str::to_owned).collect())
                        .unwrap_or_default(),
                });
            }
            "voltageLevel" => {
                let id = required_text(&attrs, "id")?.to_owned();
                frame.component = Some(self.register_component("voltage_level", &id, &attrs)?);
                let topology = match required_text(&attrs, "topologyKind")? {
                    "BUS_BREAKER" => RawTopologyKind::BusBreaker,
                    "NODE_BREAKER" => RawTopologyKind::NodeBreaker,
                    other => {
                        return Err(format_error(format!(
                            "voltage level `{id}` has unknown topologyKind `{other}`"
                        )));
                    }
                };
                self.current_voltage_level = Some(id.clone());
                self.voltage_levels.push(RawVoltageLevel {
                    id,
                    substation: self.current_substation.clone(),
                    nominal_v: required_f64(&attrs, "nominalV")?,
                    low_voltage_limit: optional_f64(&attrs, "lowVoltageLimit")?,
                    high_voltage_limit: optional_f64(&attrs, "highVoltageLimit")?,
                    topology,
                });
            }
            "bus" => {
                self.read_bus(&attrs, &mut frame)?;
                report_unmapped_bus_attributes(&attrs, diagnostics)?;
            }
            "busbarSection" => {
                let voltage_level = self.require_voltage_level(tag)?;
                let id = required_text(&attrs, "id")?.to_owned();
                frame.component = Some(self.register_component("busbar_section", &id, &attrs)?);
                self.busbar_sections.push(RawBusbarSection {
                    id,
                    voltage_level,
                    node: required_i32(&attrs, "node")?,
                });
            }
            "switch" => self.read_switch(&attrs, &mut frame)?,
            "internalConnection" => self.internal_connections.push(RawInternalConnection {
                voltage_level: self.require_voltage_level(tag)?,
                node1: required_i32(&attrs, "node1")?,
                node2: required_i32(&attrs, "node2")?,
            }),
            "load" => self.read_equipment(EquipmentKind::Load, attrs, &mut frame)?,
            "generator" => self.read_equipment(EquipmentKind::Generator, attrs, &mut frame)?,
            "battery" => self.read_equipment(EquipmentKind::Battery, attrs, &mut frame)?,
            "shunt"
                if self
                    .namespace
                    .is_some_and(|namespace| namespace.version <= XiidmVersion::V1_15) =>
            {
                self.read_equipment(EquipmentKind::Shunt, attrs, &mut frame)?;
            }
            "shuntCompensator"
                if self
                    .namespace
                    .is_some_and(|namespace| namespace.version >= XiidmVersion::V1_16) =>
            {
                self.read_equipment(EquipmentKind::Shunt, attrs, &mut frame)?;
            }
            "staticVarCompensator" => {
                self.read_equipment(EquipmentKind::StaticVarCompensator, attrs, &mut frame)?;
            }
            "boundaryLine"
                if self
                    .namespace
                    .is_some_and(|namespace| namespace.version >= XiidmVersion::V1_16) =>
            {
                self.read_equipment(EquipmentKind::BoundaryLine, attrs, &mut frame)?;
            }
            "danglingLine"
                if self
                    .namespace
                    .is_some_and(|namespace| namespace.version <= XiidmVersion::V1_15) =>
            {
                self.read_equipment(EquipmentKind::BoundaryLine, attrs, &mut frame)?;
            }
            "shunt" | "shuntCompensator" | "boundaryLine" | "danglingLine" => {
                return Err(format_error(format!(
                    "XIIDM element `{tag}` is not valid for the detected XIIDM version"
                )));
            }
            "vscConverterStation" => {
                self.read_equipment(EquipmentKind::VscConverterStation, attrs, &mut frame)?;
            }
            "lccConverterStation" => {
                self.read_equipment(EquipmentKind::LccConverterStation, attrs, &mut frame)?;
            }
            "voltageSourceConverter" => {
                self.read_voltage_source_converter(attrs, &mut frame)?;
            }
            "lineCommutatedConverter" => {
                self.read_line_commutated_converter(attrs, &mut frame)?;
            }
            "line" => self.read_equipment(EquipmentKind::Line, attrs, &mut frame)?,
            "twoWindingsTransformer" => {
                self.read_equipment(EquipmentKind::Transformer, attrs, &mut frame)?;
            }
            "threeWindingsTransformer" => {
                self.read_equipment(EquipmentKind::ThreeWindingTransformer, attrs, &mut frame)?;
            }
            "hvdcLine" => {
                let id = required_text(&attrs, "id")?.to_owned();
                frame.component = Some(self.register_component("hvdc", &id, &attrs)?);
                self.hvdc_lines.push(RawHvdcLine { id, attrs });
            }
            "tieLine" => {
                let id = required_text(&attrs, "id")?.to_owned();
                frame.component = Some(self.register_component("tie_line", &id, &attrs)?);
                let modern = self
                    .namespace
                    .is_some_and(|namespace| namespace.version >= XiidmVersion::V1_16);
                self.tie_lines.push(RawTieLine {
                    id,
                    boundary_line1: required_text(
                        &attrs,
                        if modern {
                            "boundaryLineId1"
                        } else {
                            "danglingLineId1"
                        },
                    )?
                    .to_owned(),
                    boundary_line2: required_text(
                        &attrs,
                        if modern {
                            "boundaryLineId2"
                        } else {
                            "danglingLineId2"
                        },
                    )?
                    .to_owned(),
                });
            }
            "minMaxReactiveLimits" if self.current_ac_dc_converter().is_some() => {
                let limits = ReactiveLimits::MinMax(MinMaxReactiveLimits {
                    minimum_reactive_power_mvar: required_f64(&attrs, "minQ")?,
                    maximum_reactive_power_mvar: required_f64(&attrs, "maxQ")?,
                    properties: BTreeMap::new(),
                });
                self.set_voltage_source_converter_reactive_limits(limits)?;
            }
            "minMaxReactiveLimits" => {
                self.set_equipment_reactive_limits(ReactiveLimits::MinMax(MinMaxReactiveLimits {
                    minimum_reactive_power_mvar: required_f64(&attrs, "minQ")?,
                    maximum_reactive_power_mvar: required_f64(&attrs, "maxQ")?,
                    properties: BTreeMap::new(),
                }))?;
            }
            "reactiveCapabilityCurve" if self.current_ac_dc_converter().is_some() => {
                self.set_voltage_source_converter_reactive_limits(
                    ReactiveLimits::CapabilityCurve(ReactiveCapabilityCurve {
                        curve_style: CurveStyle::StraightLineYValues,
                        properties: BTreeMap::new(),
                        points: Vec::new(),
                    }),
                )?;
            }
            "reactiveCapabilityCurve" => {
                self.set_equipment_reactive_limits(ReactiveLimits::CapabilityCurve(
                    ReactiveCapabilityCurve {
                        curve_style: CurveStyle::StraightLineYValues,
                        properties: BTreeMap::new(),
                        points: Vec::new(),
                    },
                ))?;
            }
            "point" if self.current_ac_dc_converter().is_some() => {
                let converter = self.current_voltage_source_converter_mut()?;
                let Some(ReactiveLimits::CapabilityCurve(curve)) =
                    converter.reactive_limits.as_mut()
                else {
                    return Err(format_error(
                        "XIIDM reactive capability point has no curve parent",
                    ));
                };
                curve.points.push(ReactiveCapabilityCurvePoint {
                    active_power_mw: required_f64(&attrs, "p")?,
                    minimum_reactive_power_mvar: required_f64(&attrs, "minQ")?,
                    maximum_reactive_power_mvar: required_f64(&attrs, "maxQ")?,
                    properties: BTreeMap::new(),
                });
            }
            "point" => {
                let equipment = self.current_equipment()?;
                let Some(ReactiveLimits::CapabilityCurve(curve)) =
                    self.equipment[equipment].reactive_limits.as_mut()
                else {
                    return Err(format_error(
                        "XIIDM reactive capability point has no curve parent",
                    ));
                };
                curve.points.push(ReactiveCapabilityCurvePoint {
                    active_power_mw: required_f64(&attrs, "p")?,
                    minimum_reactive_power_mvar: required_f64(&attrs, "minQ")?,
                    maximum_reactive_power_mvar: required_f64(&attrs, "maxQ")?,
                    properties: BTreeMap::new(),
                });
            }
            "pccTerminal" => {
                let reference = parse_terminal_reference(&attrs)?;
                let common = self.current_ac_dc_converter_mut()?;
                if common.pcc_terminal.replace(reference).is_some() {
                    return Err(format_error(
                        "AC/DC converter has more than one pccTerminal",
                    ));
                }
            }
            "droopCurve" => {
                let common = self.current_ac_dc_converter_mut()?;
                if common
                    .droop_curve
                    .replace(DroopCurve {
                        segments: Vec::new(),
                    })
                    .is_some()
                {
                    return Err(format_error("AC/DC converter has more than one droopCurve"));
                }
            }
            "segment" => {
                let common = self.current_ac_dc_converter_mut()?;
                let curve = common
                    .droop_curve
                    .as_mut()
                    .ok_or_else(|| format_error("droop segment has no droopCurve parent"))?;
                curve.segments.push(DroopCurveSegment {
                    minimum_voltage_kv: required_f64(&attrs, "minV")?,
                    maximum_voltage_kv: required_f64(&attrs, "maxV")?,
                    k: required_f64(&attrs, "k")?,
                });
            }
            "zipModel" => {
                let index = self.current_equipment()?;
                self.equipment[index].load_model = Some(RawLoadModel::Zip {
                    c0p: required_f64(&attrs, "c0p")?,
                    c1p: required_f64(&attrs, "c1p")?,
                    c2p: required_f64(&attrs, "c2p")?,
                    c0q: required_f64(&attrs, "c0q")?,
                    c1q: required_f64(&attrs, "c1q")?,
                    c2q: required_f64(&attrs, "c2q")?,
                });
            }
            "exponentialModel" => {
                let index = self.current_equipment()?;
                self.equipment[index].load_model = Some(RawLoadModel::Exponential {
                    np: required_f64(&attrs, "np")?,
                    nq: required_f64(&attrs, "nq")?,
                });
            }
            "shuntLinearModel" => {
                let index = self.current_equipment()?;
                self.equipment[index].shunt_conductance_omitted =
                    !attrs.contains_key("gPerSection");
                self.equipment[index].shunt_linear = Some((
                    optional_f64(&attrs, "gPerSection")?.unwrap_or(0.0),
                    required_f64(&attrs, "bPerSection")?,
                    required_u32(&attrs, "maximumSectionCount")?,
                ));
            }
            "section" if self.current_equipment_kind() == Some(EquipmentKind::Shunt) => {
                let index = self.current_equipment()?;
                self.equipment[index].shunt_conductance_omitted |= !attrs.contains_key("g");
                self.equipment[index].shunt_sections.push((
                    optional_f64(&attrs, "g")?.unwrap_or(0.0),
                    required_f64(&attrs, "b")?,
                ));
            }
            tag if tap_tag(tag).is_some() => {
                let index = self.current_equipment()?;
                let active = tap_tag(tag).expect("matched tap tag");
                let legacy_fixed_phase_tap = active.kind == TapKind::Phase
                    && attrs
                        .get("regulationMode")
                        .is_some_and(|mode| mode == "FIXED_TAP")
                    && self
                        .namespace
                        .is_some_and(|namespace| namespace.version <= XiidmVersion::V1_13);
                let regulation_mode = if legacy_fixed_phase_tap {
                    diagnostics.push(
                        &codes::READ_XIIDM_VERSION_COMPATIBILITY,
                        format!(
                            "XIIDM {} phase tap changer on `{}` uses legacy FIXED_TAP mode; it is read as CURRENT_LIMITER with regulation disabled",
                            self.namespace.expect("version checked").version.label(),
                            self.equipment[index].id,
                        ),
                    );
                    Some(TapChangerRegulationMode::Current)
                } else {
                    match active.kind {
                        TapKind::Ratio => attrs
                            .get("regulationMode")
                            .map(|mode| parse_tap_regulation_mode(active.kind, mode))
                            .transpose()?,
                        TapKind::Phase => attrs
                            .get("regulationMode")
                            .map(|mode| parse_tap_regulation_mode(active.kind, mode))
                            .transpose()?,
                    }
                };
                let load_tap_changing_capabilities = match active.kind {
                    TapKind::Phase
                        if self
                            .namespace
                            .is_some_and(|namespace| namespace.version <= XiidmVersion::V1_13) =>
                    {
                        true
                    }
                    TapKind::Ratio | TapKind::Phase => {
                        required_bool(&attrs, "loadTapChangingCapabilities")?
                    }
                };
                let source_regulating = optional_bool(&attrs, "regulating")?.unwrap_or(false);
                let regulating = if legacy_fixed_phase_tap {
                    false
                } else if active.kind == TapKind::Ratio
                    && self
                        .namespace
                        .is_some_and(|namespace| namespace.version <= XiidmVersion::V1_13)
                    && !load_tap_changing_capabilities
                    && source_regulating
                {
                    diagnostics.push(
                        &codes::READ_XIIDM_VERSION_COMPATIBILITY,
                        format!(
                            "XIIDM {} ratio tap changer on `{}` declares `regulating=true` without load tap changing capability; PowSybl treats regulation as disabled",
                            self.namespace.expect("version checked").version.label(),
                            self.equipment[index].id,
                        ),
                    );
                    false
                } else {
                    source_regulating
                };
                let tap = TapChanger {
                    tap_position: optional_i32(&attrs, "tapPosition")?,
                    solved_tap_position: optional_i32(&attrs, "solvedTapPosition")?,
                    low_tap_position: optional_i32(&attrs, "lowTapPosition")?.unwrap_or(0),
                    load_tap_changing_capabilities,
                    regulating,
                    regulation_mode,
                    regulation_value: optional_f64(&attrs, "regulationValue")?,
                    target_deadband: optional_f64(&attrs, "targetDeadband")?,
                    regulation_terminal: None,
                    steps: Vec::new(),
                };
                match (self.equipment[index].kind, active.kind) {
                    (EquipmentKind::ThreeWindingTransformer, TapKind::Ratio) => {
                        self.equipment[index].winding_ratio_taps[active.winding] = Some(tap);
                    }
                    (EquipmentKind::ThreeWindingTransformer, TapKind::Phase) => {
                        self.equipment[index].winding_phase_taps[active.winding] = Some(tap);
                    }
                    (_, TapKind::Ratio) => self.equipment[index].ratio_tap = Some(tap),
                    (_, TapKind::Phase) => self.equipment[index].phase_tap = Some(tap),
                }
                self.current_tap = Some(active);
            }
            "step" if self.current_tap.is_some() => self.read_tap_step(&attrs)?,
            tag if operational_limits_side(tag).is_some() => {
                let equipment = self.current_equipment()?;
                self.equipment[equipment]
                    .operational_limits
                    .push(RawOperationalLimitsGroup {
                        id: required_text(&attrs, "id")?.to_owned(),
                        side: operational_limits_side(tag).expect("matched limit group"),
                        properties: BTreeMap::new(),
                        active_power: None,
                        apparent_power: None,
                        current: None,
                    });
            }
            "activePowerLimits" | "apparentPowerLimits" | "currentLimits" => {
                let kind = match tag {
                    "activePowerLimits" => LoadingLimitKind::ActivePower,
                    "apparentPowerLimits" => LoadingLimitKind::ApparentPower,
                    _ => LoadingLimitKind::Current,
                };
                let limits = RawLoadingLimits {
                    permanent_limit: optional_f64(&attrs, "permanentLimit")?,
                    permanent_name: attrs.get("permanentLimitName").cloned(),
                    temporary_limits: Vec::new(),
                };
                let group = self.current_operational_limits_group_mut()?;
                match kind {
                    LoadingLimitKind::ActivePower => group.active_power = Some(limits),
                    LoadingLimitKind::ApparentPower => group.apparent_power = Some(limits),
                    LoadingLimitKind::Current => group.current = Some(limits),
                }
                self.current_loading_limit = Some(kind);
            }
            "temporaryLimit" if self.current_loading_limit.is_some() => {
                let limit = RawTemporaryLimit {
                    name: required_text(&attrs, "name")?.to_owned(),
                    acceptable_duration: optional_i32(&attrs, "acceptableDuration")?,
                    value: optional_f64(&attrs, "value")?,
                    fictitious: optional_bool(&attrs, "fictitious")?.unwrap_or(false),
                };
                let kind = self.current_loading_limit.expect("matched loading limit");
                let group = self.current_operational_limits_group_mut()?;
                let limits = match kind {
                    LoadingLimitKind::ActivePower => group.active_power.as_mut(),
                    LoadingLimitKind::ApparentPower => group.apparent_power.as_mut(),
                    LoadingLimitKind::Current => group.current.as_mut(),
                }
                .expect("loading limit exists");
                limits.temporary_limits.push(limit);
            }
            "alias" => {
                self.pending_alias = Some(PendingAlias {
                    component: self.current_component()?.clone(),
                    alias_type: attrs.get("type").cloned(),
                    value: String::new(),
                });
            }
            "property" => self.read_property(&attrs, diagnostics)?,
            "slackTerminal" => self.slack_terminal = Some(required_text(&attrs, "id")?.to_owned()),
            "terminalRef" if self.current_tap.is_some() => {
                let reference = parse_terminal_reference(&attrs)?;
                let equipment = self.current_equipment()?;
                let active = self.current_tap.expect("tap parent exists");
                let tap = self.current_tap_mut(equipment, active)?;
                tap.regulation_terminal = Some(reference);
            }
            "regulatingTerminal" => {
                let equipment = self.current_equipment()?;
                self.equipment[equipment].regulating_terminal =
                    Some(parse_terminal_reference(&attrs)?);
            }
            "busBreakerTopology"
            | "nodeBreakerTopology"
            | "terminalRef"
            | "shuntNonLinearModel" => {}
            "selectedOperationalLimitsGroup" | "permanentLimit" => {
                diagnostics.push(
                    &codes::READ_XIIDM_FIELD_UNMAPPED,
                    format!("XIIDM `{tag}` data is not yet mapped"),
                );
            }
            other => diagnostics.push(
                &codes::READ_XIIDM_ELEMENT_UNMAPPED,
                format!("XIIDM element `{other}` is not mapped"),
            ),
        }
        if empty {
            if tag == "alias" {
                self.finish_alias()?;
            }
            if tap_tag(tag).is_some() {
                self.current_tap = None;
            }
            if matches!(
                tag,
                "activePowerLimits" | "apparentPowerLimits" | "currentLimits"
            ) {
                self.current_loading_limit = None;
            }
            if tag == "area" {
                self.current_area = None;
            }
            if tag == "extension" {
                self.current_extension_target = None;
            }
            if tag == "network"
                && frame
                    .component
                    .as_ref()
                    .is_some_and(|component| component.component_type() == "subnetwork")
            {
                self.current_subnetwork = None;
            }
        } else {
            self.frames.push(frame);
        }
        Ok(())
    }

    fn read_network(
        &mut self,
        attrs: &Attrs,
        frame: &mut Frame,
        diagnostics: &mut Diagnostics,
    ) -> Result<()> {
        if self.root_seen {
            if self.current_subnetwork.is_some()
                || self.frames.len() != 1
                || self.frames[0].tag != "network"
            {
                return Err(format_error(
                    "only one level of XIIDM subnetworks is supported",
                ));
            }
            let id = required_text(attrs, "id")?.to_owned();
            let component = self.register_component("subnetwork", &id, attrs)?;
            let parent = component_id(
                "balanced_network",
                self.id
                    .as_deref()
                    .ok_or_else(|| format_error("subnetwork has no root balanced network"))?,
            )?;
            let case_metadata = CaseMetadata {
                case_date: Some(required_text(attrs, "caseDate")?.to_owned()),
                forecast_distance: Some(required_i32(attrs, "forecastDistance")?),
                source_model_format: Some(required_text(attrs, "sourceFormat")?.to_owned()),
                minimum_validation_level: Some(
                    required_text(attrs, "minimumValidationLevel")?.to_owned(),
                ),
            };
            frame.component = Some(component.clone());
            self.subnetworks.push(RawSubnetwork {
                component,
                parent,
                case_metadata,
                components: Vec::new(),
            });
            self.current_subnetwork = Some(self.subnetworks.len() - 1);
            return Ok(());
        }
        let mut namespaces = attrs
            .iter()
            .filter(|(key, value)| key.starts_with("xmlns") && is_xiidm_namespace_uri(value));
        let (_, uri) = namespaces
            .next()
            .ok_or_else(|| format_error("network has no recognized XIIDM namespace"))?;
        if namespaces.next().is_some() {
            return Err(format_error(
                "network declares more than one XIIDM namespace",
            ));
        }
        let Some(namespace) = XiidmNamespace::from_uri(uri) else {
            diagnostics.push(
                &codes::PARSE_XIIDM_VERSION_UNSUPPORTED,
                format!(
                    "XIIDM namespace `{uri}` is unsupported; PowerIO reads XIIDM 1.12 through 1.17"
                ),
            );
            return Err(format_error(format!(
                "unsupported XIIDM namespace `{uri}`; supported input versions are 1.12 through 1.17"
            )));
        };
        if namespace.version != XiidmVersion::V1_17 {
            diagnostics.push(
                &codes::READ_XIIDM_VERSION_COMPATIBILITY,
                format!(
                    "XIIDM {} input was read; fresh XIIDM output uses 1.17",
                    namespace.version.label()
                ),
            );
        }
        self.namespace = Some(namespace);
        let id = required_text(attrs, "id")?.to_owned();
        frame.component = Some(self.register_component("balanced_network", &id, attrs)?);
        self.id = Some(id);
        self.case_metadata = CaseMetadata {
            case_date: Some(required_text(attrs, "caseDate")?.to_owned()),
            forecast_distance: Some(required_i32(attrs, "forecastDistance")?),
            source_model_format: Some(required_text(attrs, "sourceFormat")?.to_owned()),
            minimum_validation_level: Some(
                required_text(attrs, "minimumValidationLevel")?.to_owned(),
            ),
        };
        self.root_seen = true;
        Ok(())
    }

    fn read_bus(&mut self, attrs: &Attrs, frame: &mut Frame) -> Result<()> {
        let voltage_level = self.require_voltage_level("bus")?;
        let topology = self
            .voltage_levels
            .iter()
            .find(|value| value.id == voltage_level)
            .ok_or_else(|| format_error("bus has no declared voltage level"))?
            .topology;
        let (id, nodes) = if topology == RawTopologyKind::NodeBreaker {
            (None, parse_nodes(required_text(attrs, "nodes")?)?)
        } else {
            let id = required_text(attrs, "id")?.to_owned();
            frame.component = Some(self.register_component("bus", &id, attrs)?);
            (Some(id), Vec::new())
        };
        self.buses.push(RawBus {
            id,
            voltage_level,
            nodes,
            v: optional_f64(attrs, "v")?,
            angle: optional_f64(attrs, "angle")?,
        });
        Ok(())
    }

    fn read_switch(&mut self, attrs: &Attrs, frame: &mut Frame) -> Result<()> {
        let voltage_level = self.require_voltage_level("switch")?;
        let id = required_text(attrs, "id")?.to_owned();
        frame.component = Some(self.register_component("switch", &id, attrs)?);
        let kind = match required_text(attrs, "kind")? {
            "BREAKER" => SwitchKind::Breaker,
            "DISCONNECTOR" => SwitchKind::Disconnector,
            "LOAD_BREAK_SWITCH" => SwitchKind::LoadBreakSwitch,
            other => {
                return Err(format_error(format!(
                    "switch `{id}` has unknown kind `{other}`"
                )));
            }
        };
        let (endpoint1, endpoint2) = if attrs.contains_key("bus1") {
            (
                RawEndpoint::Bus(required_text(attrs, "bus1")?.to_owned()),
                RawEndpoint::Bus(required_text(attrs, "bus2")?.to_owned()),
            )
        } else {
            (
                RawEndpoint::Node(required_i32(attrs, "node1")?),
                RawEndpoint::Node(required_i32(attrs, "node2")?),
            )
        };
        self.switches.push(RawSwitch {
            id,
            voltage_level,
            kind,
            endpoint1,
            endpoint2,
            open: optional_bool(attrs, "open")?.unwrap_or(false),
            retained: optional_bool(attrs, "retained")?.unwrap_or(false),
        });
        Ok(())
    }

    fn read_equipment(
        &mut self,
        kind: EquipmentKind,
        attrs: Attrs,
        frame: &mut Frame,
    ) -> Result<()> {
        let id = required_text(&attrs, "id")?.to_owned();
        frame.component = Some(self.register_component(kind.component_type(), &id, &attrs)?);
        let voltage_level = (kind.terminal_count() == 1)
            .then(|| self.require_voltage_level(kind.component_type()))
            .transpose()?;
        let equipment_index = self.equipment.len();
        frame.equipment = Some(equipment_index);
        if self
            .equipment_by_id
            .insert(id.clone(), equipment_index)
            .is_some()
        {
            return Err(format_error(format!("duplicate XIIDM equipment ID `{id}`")));
        }
        self.equipment.push(RawEquipment {
            kind,
            id,
            voltage_level,
            attrs,
            reactive_limits: None,
            load_model: None,
            shunt_linear: None,
            shunt_sections: Vec::new(),
            shunt_conductance_omitted: false,
            ratio_tap: None,
            phase_tap: None,
            winding_ratio_taps: std::array::from_fn(|_| None),
            winding_phase_taps: std::array::from_fn(|_| None),
            active_power_control: None,
            regulating_terminal: None,
            operational_limits: Vec::new(),
        });
        Ok(())
    }

    fn read_active_power_control(
        &mut self,
        attrs: &Attrs,
        version: ActivePowerControlVersion,
    ) -> Result<()> {
        let target = self
            .current_extension_target
            .as_deref()
            .ok_or_else(|| format_error("activePowerControl appears outside an extension"))?;
        let equipment_index = self.equipment_by_id.get(target).copied().ok_or_else(|| {
            format_error(format!(
                "activePowerControl references unknown equipment `{target}`"
            ))
        })?;
        let equipment = &mut self.equipment[equipment_index];
        if !matches!(
            equipment.kind,
            EquipmentKind::Generator | EquipmentKind::Battery
        ) {
            return Err(format_error(format!(
                "activePowerControl target `{target}` is not a generator or battery"
            )));
        }
        if equipment.active_power_control.is_some() {
            return Err(format_error(format!(
                "equipment `{target}` has more than one activePowerControl extension"
            )));
        }
        let droop_percent = match version {
            ActivePowerControlVersion::V1_0 => Some(required_f64(attrs, "droop")?),
            ActivePowerControlVersion::V1_1 | ActivePowerControlVersion::V1_2 => {
                optional_f64(attrs, "droop")?
            }
        };
        let participation_factor = match version {
            ActivePowerControlVersion::V1_0 => None,
            ActivePowerControlVersion::V1_1 | ActivePowerControlVersion::V1_2 => {
                optional_f64(attrs, "participationFactor")?
            }
        };
        if participation_factor.is_some_and(|value| value < 0.0) {
            return Err(format_error(format!(
                "activePowerControl on `{target}` has a negative participationFactor"
            )));
        }
        let (minimum_target_active_power_mw, maximum_target_active_power_mw) = match version {
            ActivePowerControlVersion::V1_2 => (
                optional_f64(attrs, "minTargetP")?,
                optional_f64(attrs, "maxTargetP")?,
            ),
            ActivePowerControlVersion::V1_0 | ActivePowerControlVersion::V1_1 => (None, None),
        };
        validate_active_power_control_target_limits(
            target,
            minimum_target_active_power_mw,
            maximum_target_active_power_mw,
            required_f64(&equipment.attrs, "minP")?,
            required_f64(&equipment.attrs, "maxP")?,
        )?;
        equipment.active_power_control = Some(ActivePowerControl {
            participate: required_bool(attrs, "participate")?,
            droop_percent,
            participation_factor,
            minimum_target_active_power_mw,
            maximum_target_active_power_mw,
        });
        Ok(())
    }

    fn read_voltage_source_converter(&mut self, attrs: Attrs, frame: &mut Frame) -> Result<()> {
        let id = required_text(&attrs, "id")?.to_owned();
        let component = self.register_component("voltage_source_converter", &id, &attrs)?;
        let voltage_level = self.require_voltage_level("voltageSourceConverter")?;
        frame.component = Some(component);
        frame.ac_dc_converter = Some(RawAcDcConverterIndex::VoltageSource(
            self.voltage_source_converters.len(),
        ));
        self.voltage_source_converters
            .push(RawVoltageSourceConverter {
                common: RawAcDcConverter {
                    id,
                    voltage_level,
                    attrs,
                    pcc_terminal: None,
                    droop_curve: None,
                },
                reactive_limits: None,
            });
        Ok(())
    }

    fn read_line_commutated_converter(&mut self, attrs: Attrs, frame: &mut Frame) -> Result<()> {
        let id = required_text(&attrs, "id")?.to_owned();
        let component = self.register_component("line_commutated_converter", &id, &attrs)?;
        let voltage_level = self.require_voltage_level("lineCommutatedConverter")?;
        frame.component = Some(component);
        frame.ac_dc_converter = Some(RawAcDcConverterIndex::LineCommutated(
            self.line_commutated_converters.len(),
        ));
        self.line_commutated_converters
            .push(RawLineCommutatedConverter {
                common: RawAcDcConverter {
                    id,
                    voltage_level,
                    attrs,
                    pcc_terminal: None,
                    droop_curve: None,
                },
            });
        Ok(())
    }

    fn current_ac_dc_converter(&self) -> Option<RawAcDcConverterIndex> {
        self.frames
            .iter()
            .rev()
            .find_map(|frame| frame.ac_dc_converter)
    }

    fn current_ac_dc_converter_mut(&mut self) -> Result<&mut RawAcDcConverter> {
        match self.current_ac_dc_converter() {
            Some(RawAcDcConverterIndex::VoltageSource(index)) => Ok(&mut self
                .voltage_source_converters
                .get_mut(index)
                .expect("registered voltage source converter")
                .common),
            Some(RawAcDcConverterIndex::LineCommutated(index)) => Ok(&mut self
                .line_commutated_converters
                .get_mut(index)
                .expect("registered line commutated converter")
                .common),
            None => Err(format_error("element has no AC/DC converter parent")),
        }
    }

    fn current_voltage_source_converter_mut(&mut self) -> Result<&mut RawVoltageSourceConverter> {
        let Some(RawAcDcConverterIndex::VoltageSource(index)) = self.current_ac_dc_converter()
        else {
            return Err(format_error(
                "reactive limits have no voltageSourceConverter parent",
            ));
        };
        Ok(self
            .voltage_source_converters
            .get_mut(index)
            .expect("registered voltage source converter"))
    }

    fn set_voltage_source_converter_reactive_limits(
        &mut self,
        limits: ReactiveLimits,
    ) -> Result<()> {
        let converter = self.current_voltage_source_converter_mut()?;
        if converter.reactive_limits.replace(limits).is_some() {
            return Err(format_error(
                "voltageSourceConverter has more than one reactive limits record",
            ));
        }
        Ok(())
    }

    fn set_equipment_reactive_limits(&mut self, limits: ReactiveLimits) -> Result<()> {
        let index = self.current_equipment()?;
        if self.equipment[index]
            .reactive_limits
            .replace(limits)
            .is_some()
        {
            return Err(format_error(format!(
                "{} `{}` has more than one reactive limits record",
                self.equipment[index].kind.component_type(),
                self.equipment[index].id
            )));
        }
        Ok(())
    }

    fn read_property(&mut self, attrs: &Attrs, diagnostics: &mut Diagnostics) -> Result<()> {
        let name = required_text(attrs, "name")?.to_owned();
        let value = required_text(attrs, "value")?.to_owned();
        let parent = self
            .frames
            .last()
            .map(|frame| frame.tag.clone())
            .unwrap_or_default();
        if matches!(
            parent.as_str(),
            "activePowerLimits" | "apparentPowerLimits" | "currentLimits" | "temporaryLimit"
        ) {
            let group = self.current_operational_limits_group_mut()?.id.clone();
            diagnostics.push(
                &codes::READ_XIIDM_FIELD_UNMAPPED,
                format!(
                    "XIIDM operational limits group `{group}` has property `{name}={value}` on `{parent}`; the balanced operational limit model does not represent properties at that level and fresh XIIDM output omits it"
                ),
            );
            return Ok(());
        }
        if self.frames.iter().rev().any(|frame| {
            matches!(
                frame.tag.as_str(),
                "operationalLimitsGroup"
                    | "operationalLimitsGroup1"
                    | "operationalLimitsGroup2"
                    | "operationalLimitsGroup3"
            )
        }) {
            self.current_operational_limits_group_mut()?
                .properties
                .insert(name, value);
            return Ok(());
        }
        if matches!(
            self.current_ac_dc_converter(),
            Some(RawAcDcConverterIndex::VoltageSource(_))
        ) {
            let parent = self
                .frames
                .iter()
                .rev()
                .find_map(|frame| match frame.tag.as_str() {
                    "point" => Some(0_u8),
                    "reactiveCapabilityCurve" => Some(1),
                    "minMaxReactiveLimits" => Some(2),
                    _ => None,
                });
            if let Some(parent) = parent {
                let converter = self.current_voltage_source_converter_mut()?;
                let limits = converter
                    .reactive_limits
                    .as_mut()
                    .ok_or_else(|| format_error("reactive limit property has no parent"))?;
                match (parent, limits) {
                    (0, ReactiveLimits::CapabilityCurve(curve)) => {
                        curve
                            .points
                            .last_mut()
                            .ok_or_else(|| format_error("point property has no point parent"))?
                            .properties
                            .insert(name, value);
                    }
                    (1, ReactiveLimits::CapabilityCurve(curve)) => {
                        curve.properties.insert(name, value);
                    }
                    (2, ReactiveLimits::MinMax(limits)) => {
                        limits.properties.insert(name, value);
                    }
                    _ => {
                        return Err(format_error(
                            "reactive limit property does not match its XML parent",
                        ));
                    }
                }
                return Ok(());
            }
        }
        let reactive_limits_parent =
            self.frames
                .iter()
                .rev()
                .find_map(|frame| match frame.tag.as_str() {
                    "point" => Some(0_u8),
                    "reactiveCapabilityCurve" => Some(1),
                    "minMaxReactiveLimits" => Some(2),
                    _ => None,
                });
        if let Some(parent) = reactive_limits_parent
            && let Ok(index) = self.current_equipment()
        {
            let limits = self.equipment[index]
                .reactive_limits
                .as_mut()
                .ok_or_else(|| format_error("reactive limit property has no parent"))?;
            match (parent, limits) {
                (0, ReactiveLimits::CapabilityCurve(curve)) => {
                    curve
                        .points
                        .last_mut()
                        .ok_or_else(|| format_error("point property has no point parent"))?
                        .properties
                        .insert(name, value);
                }
                (1, ReactiveLimits::CapabilityCurve(curve)) => {
                    curve.properties.insert(name, value);
                }
                (2, ReactiveLimits::MinMax(limits)) => {
                    limits.properties.insert(name, value);
                }
                _ => {
                    return Err(format_error(
                        "reactive limit property does not match its XML parent",
                    ));
                }
            }
            return Ok(());
        }
        if self
            .frames
            .last()
            .is_some_and(|frame| frame.component.is_none())
        {
            diagnostics.push(
                &codes::READ_XIIDM_FIELD_UNMAPPED,
                format!(
                    "XIIDM `{parent}` has property `{name}={value}`; the corresponding source neutral record does not represent properties and fresh XIIDM output omits it"
                ),
            );
            return Ok(());
        }
        let component = self.current_component()?.clone();
        self.metadata
            .get_mut(&component)
            .expect("registered component")
            .properties
            .insert(name, value);
        Ok(())
    }

    fn read_tap_step(&mut self, attrs: &Attrs) -> Result<()> {
        let active = self.current_tap.expect("tap step has tap parent");
        let step = TapStep {
            rho: required_f64(attrs, "rho")?,
            alpha: if active.kind == TapKind::Phase {
                required_f64(attrs, "alpha")?
            } else {
                0.0
            },
            r_percent: optional_f64(attrs, "r")?.unwrap_or(0.0),
            x_percent: optional_f64(attrs, "x")?.unwrap_or(0.0),
            g_percent: optional_f64(attrs, "g")?.unwrap_or(0.0),
            b_percent: optional_f64(attrs, "b")?.unwrap_or(0.0),
        };
        let index = self.current_equipment()?;
        let equipment = &mut self.equipment[index];
        let tap = match (equipment.kind, active.kind) {
            (EquipmentKind::ThreeWindingTransformer, TapKind::Ratio) => {
                equipment.winding_ratio_taps[active.winding].as_mut()
            }
            (EquipmentKind::ThreeWindingTransformer, TapKind::Phase) => {
                equipment.winding_phase_taps[active.winding].as_mut()
            }
            (_, TapKind::Ratio) => equipment.ratio_tap.as_mut(),
            (_, TapKind::Phase) => equipment.phase_tap.as_mut(),
        }
        .expect("tap exists");
        tap.steps.push(step);
        Ok(())
    }

    fn current_operational_limits_group_mut(&mut self) -> Result<&mut RawOperationalLimitsGroup> {
        let equipment = self.current_equipment()?;
        self.equipment[equipment]
            .operational_limits
            .last_mut()
            .ok_or_else(|| format_error("loading limits have no operational limits group parent"))
    }

    fn current_tap_mut(&mut self, equipment: usize, active: ActiveTap) -> Result<&mut TapChanger> {
        let equipment = &mut self.equipment[equipment];
        match (equipment.kind, active.kind) {
            (EquipmentKind::ThreeWindingTransformer, TapKind::Ratio) => {
                equipment.winding_ratio_taps[active.winding].as_mut()
            }
            (EquipmentKind::ThreeWindingTransformer, TapKind::Phase) => {
                equipment.winding_phase_taps[active.winding].as_mut()
            }
            (_, TapKind::Ratio) => equipment.ratio_tap.as_mut(),
            (_, TapKind::Phase) => equipment.phase_tap.as_mut(),
        }
        .ok_or_else(|| format_error("tap changer child has no tap changer parent"))
    }

    fn register_id(&mut self, id: &str) -> Result<()> {
        if id.is_empty() || !self.ids.insert(id.to_owned()) {
            return Err(format_error(format!(
                "duplicate or empty XIIDM identifier `{id}`"
            )));
        }
        Ok(())
    }

    fn register_component(&mut self, kind: &str, id: &str, attrs: &Attrs) -> Result<ComponentId> {
        self.register_id(id)?;
        let component = component_id(kind, id)?;
        self.metadata.insert(
            component.clone(),
            ComponentMetadata {
                component: component.clone(),
                name: attrs.get("name").cloned(),
                equipment_container: None,
                aliases: Vec::new(),
                external_identifiers: Vec::new(),
                properties: BTreeMap::new(),
                fictitious: optional_bool(attrs, "fictitious")?.unwrap_or(false),
            },
        );
        if let Some(subnetwork) = self.current_subnetwork {
            self.subnetworks[subnetwork]
                .components
                .push(component.clone());
        }
        Ok(component)
    }

    fn current_component(&self) -> Result<&ComponentId> {
        self.frames
            .iter()
            .rev()
            .find_map(|frame| frame.component.as_ref())
            .ok_or_else(|| format_error("alias or property has no identifiable parent"))
    }

    fn current_equipment(&self) -> Result<usize> {
        self.frames
            .iter()
            .rev()
            .find_map(|frame| frame.equipment)
            .ok_or_else(|| format_error("equipment child has no equipment parent"))
    }

    fn current_equipment_kind(&self) -> Option<EquipmentKind> {
        self.frames
            .iter()
            .rev()
            .find_map(|frame| frame.equipment)
            .map(|index| self.equipment[index].kind)
    }

    fn require_voltage_level(&self, tag: &str) -> Result<String> {
        self.current_voltage_level
            .clone()
            .ok_or_else(|| format_error(format!("`{tag}` appears outside a voltage level")))
    }

    fn end(&mut self, tag: &str) -> Result<()> {
        if tag == "alias" {
            self.finish_alias()?;
        }
        if tap_tag(tag).is_some() {
            self.current_tap = None;
        }
        if matches!(
            tag,
            "activePowerLimits" | "apparentPowerLimits" | "currentLimits"
        ) {
            self.current_loading_limit = None;
        }
        let frame = self
            .frames
            .pop()
            .ok_or_else(|| format_error(format!("unexpected closing element `{tag}`")))?;
        if frame.tag != tag {
            return Err(format_error(format!(
                "closing element `{tag}` does not match `{}`",
                frame.tag
            )));
        }
        if tag == "voltageLevel" {
            self.current_voltage_level = None;
        } else if tag == "substation" {
            self.current_substation = None;
        } else if tag == "area" {
            self.current_area = None;
        } else if tag == "extension" {
            self.current_extension_target = None;
        } else if tag == "network"
            && frame
                .component
                .as_ref()
                .is_some_and(|component| component.component_type() == "subnetwork")
        {
            self.current_subnetwork = None;
        }
        Ok(())
    }

    fn finish_alias(&mut self) -> Result<()> {
        let alias = self
            .pending_alias
            .take()
            .ok_or_else(|| format_error("closing alias without an open alias"))?;
        if alias.value.is_empty() {
            return Err(format_error("XIIDM alias is empty"));
        }
        self.metadata
            .get_mut(&alias.component)
            .expect("registered component")
            .aliases
            .push(ComponentAlias {
                value: alias.value,
                alias_type: alias.alias_type,
            });
        Ok(())
    }

    fn finish(self, diagnostics: &mut Diagnostics) -> Result<BalancedNetwork> {
        if !self.root_seen || !self.frames.is_empty() || self.pending_alias.is_some() {
            return Err(format_error("incomplete XIIDM document"));
        }
        build_network(&self, diagnostics)
    }
}

const EXACT_ASSIGNMENT_DIAGNOSTIC_LIMIT: usize = 8;
const ASSIGNMENT_DIAGNOSTIC_SAMPLE_LIMIT: usize = 5;

#[derive(Default)]
struct MissingAssignmentGroup {
    count: usize,
    sample_ids: Vec<String>,
}

type MissingAssignmentGroups = BTreeMap<(&'static str, &'static str), MissingAssignmentGroup>;

fn assignment_attributes(kind: EquipmentKind) -> &'static [&'static str] {
    match kind {
        EquipmentKind::Load | EquipmentKind::BoundaryLine => &["p0", "q0"],
        EquipmentKind::Generator => &["targetP", "targetQ", "targetV"],
        EquipmentKind::Battery => &["targetP", "targetQ"],
        EquipmentKind::Shunt => &["sectionCount"],
        _ => &[],
    }
}

fn omitted_assignment_name(kind: EquipmentKind, attribute: &str) -> Option<OmittedFieldName> {
    match (kind, attribute) {
        (
            EquipmentKind::Load
            | EquipmentKind::Generator
            | EquipmentKind::Battery
            | EquipmentKind::BoundaryLine,
            "p0" | "targetP",
        ) => Some(OmittedFieldName::ActivePower),
        (
            EquipmentKind::Load
            | EquipmentKind::Generator
            | EquipmentKind::Battery
            | EquipmentKind::BoundaryLine,
            "q0" | "targetQ",
        ) => Some(OmittedFieldName::ReactivePower),
        (EquipmentKind::Generator, "targetV") => Some(OmittedFieldName::VoltageSetpoint),
        _ => None,
    }
}

fn collect_omitted_fields(parsed: &ParsedXiidm) -> Result<Vec<OmittedField>> {
    let mut omitted = Vec::new();
    for equipment in &parsed.equipment {
        let component = component_id(equipment.kind.component_type(), &equipment.id)?;
        for &attribute in assignment_attributes(equipment.kind) {
            let Some(field) = omitted_assignment_name(equipment.kind, attribute) else {
                continue;
            };
            if !equipment.attrs.contains_key(attribute) {
                omitted.push(OmittedField {
                    component: component.clone(),
                    field,
                });
            }
        }
        if equipment.kind == EquipmentKind::Generator && !equipment.attrs.contains_key("ratedS") {
            omitted.push(OmittedField {
                component: component.clone(),
                field: OmittedFieldName::RatedApparentPower,
            });
        }
        if equipment.kind == EquipmentKind::Shunt && equipment.shunt_conductance_omitted {
            omitted.push(OmittedField {
                component,
                field: OmittedFieldName::ShuntConductancePerSection,
            });
        }
    }
    Ok(omitted)
}

fn omission_requires_equipment_validation(
    network: &BalancedNetwork,
    omitted: &OmittedField,
) -> bool {
    match (omitted.component.component_type(), omitted.field) {
        ("generator", OmittedFieldName::ReactivePower) => network
            .generators()
            .iter()
            .find(|generator| generator.uid.as_deref() == Some(omitted.component.local_id()))
            .is_none_or(|generator| !generator.voltage_regulation_on),
        ("generator", OmittedFieldName::VoltageSetpoint) => network
            .generators()
            .iter()
            .find(|generator| generator.uid.as_deref() == Some(omitted.component.local_id()))
            .is_none_or(|generator| generator.voltage_regulation_on),
        ("generator", OmittedFieldName::RatedApparentPower)
        | ("shunt", OmittedFieldName::ShuntConductancePerSection) => false,
        _ => true,
    }
}

fn parse_generator_energy_source(attrs: &Attrs) -> Result<GeneratorEnergySource> {
    match required_text(attrs, "energySource")? {
        "HYDRO" => Ok(GeneratorEnergySource::Hydro),
        "NUCLEAR" => Ok(GeneratorEnergySource::Nuclear),
        "WIND" => Ok(GeneratorEnergySource::Wind),
        "THERMAL" => Ok(GeneratorEnergySource::Thermal),
        "SOLAR" => Ok(GeneratorEnergySource::Solar),
        "OTHER" => Ok(GeneratorEnergySource::Other),
        value => Err(format_error(format!(
            "unknown XIIDM generator energySource `{value}`"
        ))),
    }
}

const fn generator_energy_source_text(source: GeneratorEnergySource) -> &'static str {
    match source {
        GeneratorEnergySource::Hydro => "HYDRO",
        GeneratorEnergySource::Nuclear => "NUCLEAR",
        GeneratorEnergySource::Wind => "WIND",
        GeneratorEnergySource::Thermal => "THERMAL",
        GeneratorEnergySource::Solar => "SOLAR",
        GeneratorEnergySource::Other => "OTHER",
    }
}

fn collect_missing_assignments(parsed: &ParsedXiidm) -> MissingAssignmentGroups {
    let mut groups = MissingAssignmentGroups::new();
    if !parsed.namespace.is_some_and(XiidmNamespace::is_equipment) {
        return groups;
    }
    for equipment in &parsed.equipment {
        let kind = equipment.kind.component_type();
        for &attribute in assignment_attributes(equipment.kind) {
            if equipment.attrs.contains_key(attribute) {
                continue;
            }
            let group = groups.entry((kind, attribute)).or_default();
            group.count += 1;
            if group.sample_ids.len() < ASSIGNMENT_DIAGNOSTIC_SAMPLE_LIMIT {
                group.sample_ids.push(equipment.id.clone());
            }
        }
    }

    groups
}

fn report_missing_assignment_summaries(
    groups: &MissingAssignmentGroups,
    diagnostics: &mut Diagnostics,
) {
    for (&(kind, attribute), group) in groups {
        if group.count <= EXACT_ASSIGNMENT_DIAGNOSTIC_LIMIT {
            continue;
        }
        let samples = group
            .sample_ids
            .iter()
            .map(|id| format!("`{id}`"))
            .collect::<Vec<_>>()
            .join(", ");
        diagnostics.push(
            &codes::READ_XIIDM_VALUE_DEFAULTED,
            format!(
                "{} XIIDM equipment-mode {kind} records have no `{attribute}`; the balanced calculation view uses 0 (sample IDs: {samples})",
                group.count
            ),
        );
    }
}

fn assigned_power_or_zero(
    parsed: &ParsedXiidm,
    equipment: &RawEquipment,
    attribute: &'static str,
    missing: &MissingAssignmentGroups,
    diagnostics: &mut Diagnostics,
) -> Result<f64> {
    if let Some(value) = optional_f64(&equipment.attrs, attribute)? {
        return Ok(value);
    }
    if parsed.namespace.is_some_and(XiidmNamespace::is_equipment) {
        if missing
            .get(&(equipment.kind.component_type(), attribute))
            .is_none_or(|group| group.count <= EXACT_ASSIGNMENT_DIAGNOSTIC_LIMIT)
        {
            diagnostics.push(
                &codes::READ_XIIDM_VALUE_DEFAULTED,
                format!(
                    "XIIDM equipment-mode `{}` `{}` has no `{attribute}`; the balanced calculation view uses 0",
                    equipment.kind.component_type(), equipment.id
                ),
            );
        }
        return Ok(0.0);
    }
    Err(format_error(format!(
        "{} `{}` is missing required numeric attribute `{attribute}`",
        equipment.kind.component_type(),
        equipment.id
    )))
}

fn assigned_section_count_or_zero(
    parsed: &ParsedXiidm,
    equipment: &RawEquipment,
    missing: &MissingAssignmentGroups,
    diagnostics: &mut Diagnostics,
) -> Result<Option<u32>> {
    if let Some(value) = optional_u32(&equipment.attrs, "sectionCount")? {
        return Ok(Some(value));
    }
    if parsed.namespace.is_some_and(XiidmNamespace::is_equipment) {
        if missing
            .get(&(equipment.kind.component_type(), "sectionCount"))
            .is_none_or(|group| group.count <= EXACT_ASSIGNMENT_DIAGNOSTIC_LIMIT)
        {
            diagnostics.push(
                &codes::READ_XIIDM_VALUE_DEFAULTED,
                format!(
                    "XIIDM equipment-mode shunt `{}` has no `sectionCount`; the balanced calculation view uses 0",
                    equipment.id
                ),
            );
        }
        return Ok(None);
    }
    Err(format_error(format!(
        "shunt `{}` is missing required numeric attribute `sectionCount`",
        equipment.id
    )))
}

#[allow(clippy::float_cmp)]
fn report_unmapped_bus_attributes(attrs: &Attrs, diagnostics: &mut Diagnostics) -> Result<()> {
    let id = attrs
        .get("id")
        .map_or_else(|| "calculated bus".to_owned(), |id| format!("bus `{id}`"));
    for attribute in ["fictitiousP0", "fictitiousQ0"] {
        if let Some(value) = optional_f64(attrs, attribute)?
            && value != 0.0
        {
            diagnostics.push(
                &codes::READ_XIIDM_FIELD_UNMAPPED,
                format!(
                    "XIIDM {id} has `{attribute}={value}`; the balanced bus table does not represent fictitious bus injection and fresh XIIDM output omits it"
                ),
            );
        }
    }
    Ok(())
}

fn report_unmapped_equipment_attributes(
    parsed: &ParsedXiidm,
    diagnostics: &mut Diagnostics,
) -> Result<()> {
    for equipment in &parsed.equipment {
        match equipment.kind {
            EquipmentKind::Generator => {
                if optional_bool(&equipment.attrs, "isCondenser")? == Some(true) {
                    diagnostics.push(
                        &codes::READ_XIIDM_FIELD_UNMAPPED,
                        format!(
                            "XIIDM generator `{}` has `isCondenser=true`; the balanced generator table does not represent synchronous condenser mode and fresh XIIDM output omits it",
                            equipment.id
                        ),
                    );
                }
                if let Some(value) = optional_f64(&equipment.attrs, "equivalentLocalTargetV")? {
                    diagnostics.push(
                        &codes::READ_XIIDM_FIELD_UNMAPPED,
                        format!(
                            "XIIDM generator `{}` has `equivalentLocalTargetV={value}`; the balanced generator table does not represent the equivalent local voltage target and fresh XIIDM output omits it",
                            equipment.id
                        ),
                    );
                }
            }
            EquipmentKind::Load => {
                if let Some(load_type) = equipment
                    .attrs
                    .get("loadType")
                    .filter(|value| value.as_str() != "UNDEFINED")
                {
                    diagnostics.push(
                        &codes::READ_XIIDM_FIELD_UNMAPPED,
                        format!(
                            "XIIDM load `{}` has `loadType={load_type}`; the balanced load table does not represent the XIIDM load type and fresh XIIDM output uses `UNDEFINED`",
                            equipment.id
                        ),
                    );
                }
            }
            EquipmentKind::Shunt => {
                if let Some(value) = optional_u32(&equipment.attrs, "solvedSectionCount")? {
                    diagnostics.push(
                        &codes::READ_XIIDM_FIELD_UNMAPPED,
                        format!(
                            "XIIDM shunt `{}` has `solvedSectionCount={value}`; the balanced shunt table does not distinguish solved and assigned section counts and fresh XIIDM output omits the solved count",
                            equipment.id
                        ),
                    );
                }
            }
            EquipmentKind::Transformer => {
                if let Some(value) = optional_f64(&equipment.attrs, "ratedS")? {
                    diagnostics.push(
                        &codes::READ_XIIDM_FIELD_UNMAPPED,
                        format!(
                            "XIIDM two winding transformer `{}` has `ratedS={value}`; the balanced branch table does not represent transformer nameplate apparent power and fresh XIIDM output omits it",
                            equipment.id
                        ),
                    );
                }
            }
            EquipmentKind::ThreeWindingTransformer => {
                for side in 1..=3 {
                    let attribute = format!("ratedS{side}");
                    if let Some(value) = optional_f64(&equipment.attrs, &attribute)? {
                        diagnostics.push(
                            &codes::READ_XIIDM_FIELD_UNMAPPED,
                            format!(
                                "XIIDM three winding transformer `{}` has `{attribute}={value}`; the balanced three winding transformer table does not represent winding nameplate apparent power and fresh XIIDM output omits it",
                                equipment.id
                            ),
                        );
                    }
                }
            }
            EquipmentKind::Battery
            | EquipmentKind::StaticVarCompensator
            | EquipmentKind::BoundaryLine
            | EquipmentKind::Line
            | EquipmentKind::VscConverterStation
            | EquipmentKind::LccConverterStation => {}
        }
    }
    Ok(())
}

fn build_network(parsed: &ParsedXiidm, diagnostics: &mut Diagnostics) -> Result<BalancedNetwork> {
    let has_dc_equipment = !parsed.dc_nodes.is_empty()
        || !parsed.dc_grounds.is_empty()
        || !parsed.dc_lines.is_empty()
        || !parsed.dc_switches.is_empty();
    if parsed.voltage_levels.is_empty() && !has_dc_equipment {
        return Err(format_error("network has no voltage levels"));
    }
    validate_dc_references(parsed)?;
    report_unmapped_equipment_attributes(parsed, diagnostics)?;
    let voltage_levels: HashMap<&str, &RawVoltageLevel> = parsed
        .voltage_levels
        .iter()
        .map(|level| (level.id.as_str(), level))
        .collect();
    for level in &parsed.voltage_levels {
        if let Some(substation) = &level.substation
            && !parsed
                .substations
                .iter()
                .any(|value| value.id == *substation)
        {
            return Err(format_error(format!(
                "voltage level `{}` references missing substation `{substation}`",
                level.id
            )));
        }
        if level.nominal_v <= 0.0 {
            return Err(format_error(format!(
                "voltage level `{}` has nonpositive nominalV",
                level.id
            )));
        }
    }

    let mut bus_builder = BusBuilder::new(parsed);
    bus_builder.build(diagnostics)?;
    let mut buses = bus_builder.buses.clone();
    let areas = map_areas(parsed, &bus_builder, &mut buses, diagnostics)?;
    let missing_assignments = collect_missing_assignments(parsed);
    report_missing_assignment_summaries(&missing_assignments, diagnostics);
    let inverted_curve_count = parsed
        .equipment
        .iter()
        .filter(|equipment| {
            matches!(
                equipment.reactive_limits.as_ref(),
                Some(ReactiveLimits::CapabilityCurve(curve))
                    if curve.points.iter().any(|point| {
                        point.minimum_reactive_power_mvar
                            > point.maximum_reactive_power_mvar
                    })
            )
        })
        .count();
    if inverted_curve_count > 0 {
        diagnostics.push(
            &codes::READ_XIIDM_CALCULATION_VIEW,
            format!(
                "{inverted_curve_count} reactive capability curves contain minQ greater than maxQ; exact curve points are retained, and an evaluated inverted range collapses to its midpoint in the balanced calculation view"
            ),
        );
    }
    let unset_tap_positions = parsed
        .equipment
        .iter()
        .flat_map(|equipment| {
            equipment
                .ratio_tap
                .iter()
                .chain(equipment.phase_tap.iter())
                .chain(equipment.winding_ratio_taps.iter().flatten())
                .chain(equipment.winding_phase_taps.iter().flatten())
                .filter(move |tap| tap.tap_position.is_none())
                .map(move |_| equipment.id.as_str())
        })
        .collect::<Vec<_>>();
    if !unset_tap_positions.is_empty() {
        let samples = unset_tap_positions
            .iter()
            .take(ASSIGNMENT_DIAGNOSTIC_SAMPLE_LIMIT)
            .map(|id| format!("`{id}`"))
            .collect::<Vec<_>>()
            .join(", ");
        diagnostics.push(
            &codes::READ_XIIDM_VALUE_DEFAULTED,
            format!(
                "{} XIIDM tap changers have no assigned tap position; the balanced calculation view uses the neutral step when present and the low step otherwise (sample transformer IDs: {samples})",
                unset_tap_positions.len()
            ),
        );
    }

    let mut loads = Vec::new();
    let mut generators = Vec::new();
    let mut storage = Vec::new();
    let mut shunts = Vec::new();
    let mut static_var_compensators = Vec::new();
    let mut branches = Vec::new();
    let mut transformers_3w = Vec::new();
    let mut terminals = Vec::new();
    let mut converters = HashMap::new();
    let mut generator_bus_by_id = HashMap::new();
    let mut boundary_bus_by_id = HashMap::new();
    for equipment in &parsed.equipment {
        let terminal_records = equipment_terminals(equipment, &bus_builder, &voltage_levels)?;
        terminals.extend(
            terminal_records
                .iter()
                .map(|(_, terminal)| terminal.clone()),
        );
        let bus1 = terminal_records[0].0;
        let connected1 = terminal_records[0].1.connected;
        match equipment.kind {
            EquipmentKind::Load => {
                let p = assigned_power_or_zero(
                    parsed,
                    equipment,
                    "p0",
                    &missing_assignments,
                    diagnostics,
                )?;
                let q = assigned_power_or_zero(
                    parsed,
                    equipment,
                    "q0",
                    &missing_assignments,
                    diagnostics,
                )?;
                let mut load = Load::new(bus1, p, q);
                load.uid = Some(equipment.id.clone());
                load.in_service = connected1;
                if let Some(RawLoadModel::Zip {
                    c0p,
                    c1p,
                    c2p,
                    c0q,
                    c1q,
                    c2q,
                }) = equipment.load_model.as_ref()
                {
                    if p.abs() <= f64::EPSILON
                        && [*c0p, *c1p, *c2p]
                            .iter()
                            .any(|value| value.abs() > f64::EPSILON)
                    {
                        diagnostics.push(
                            &codes::READ_XIIDM_FIELD_UNMAPPED,
                            format!(
                                "XIIDM load `{}` has zero p0 with nonzero active power ZIP coefficients; the balanced load stores zero active power components and fresh XIIDM output uses zero active coefficients",
                                equipment.id
                            ),
                        );
                    }
                    if q.abs() <= f64::EPSILON
                        && [*c0q, *c1q, *c2q]
                            .iter()
                            .any(|value| value.abs() > f64::EPSILON)
                    {
                        diagnostics.push(
                            &codes::READ_XIIDM_FIELD_UNMAPPED,
                            format!(
                                "XIIDM load `{}` has zero q0 with nonzero reactive power ZIP coefficients; the balanced load stores zero reactive power components and fresh XIIDM output uses zero reactive coefficients",
                                equipment.id
                            ),
                        );
                    }
                }
                load.voltage_model = equipment.load_model.as_ref().map(|model| match model {
                    RawLoadModel::Zip {
                        c0p,
                        c1p,
                        c2p,
                        c0q,
                        c1q,
                        c2q,
                    } => LoadVoltageModel::Zip {
                        p_constant_power: p * c0p,
                        q_constant_power: q * c0q,
                        p_constant_current: p * c1p,
                        q_constant_current: q * c1q,
                        p_constant_impedance: p * c2p,
                        q_constant_impedance: q * c2q,
                        v_nom: None,
                        load_type: None,
                        scaling: None,
                    },
                    RawLoadModel::Exponential { np, nq } => LoadVoltageModel::Exponential {
                        p,
                        q,
                        v_nom: None,
                        gamma_p: *np,
                        gamma_q: *nq,
                    },
                });
                loads.push(load);
            }
            EquipmentKind::Generator => {
                let mut generator = Generator::new(bus1);
                generator.uid = Some(equipment.id.clone());
                generator.energy_source = parse_generator_energy_source(&equipment.attrs)?;
                generator.pg = assigned_power_or_zero(
                    parsed,
                    equipment,
                    "targetP",
                    &missing_assignments,
                    diagnostics,
                )?;
                generator.qg = optional_f64(&equipment.attrs, "targetQ")?.unwrap_or(0.0);
                generator.pmin = required_f64(&equipment.attrs, "minP")?;
                generator.pmax = required_f64(&equipment.attrs, "maxP")?;
                let (qmin, qmax) = if let Some(limits) = &equipment.reactive_limits {
                    reactive_limits_at_active_power(
                        &format!("generator `{}`", equipment.id),
                        limits,
                        generator.pg,
                    )?
                } else {
                    diagnostics.push(
                        &codes::READ_XIIDM_VALUE_DEFAULTED,
                        format!(
                            "generator `{}` has no reactive limits; both default to targetQ",
                            equipment.id
                        ),
                    );
                    (generator.qg, generator.qg)
                };
                generator.qmin = qmin;
                generator.qmax = qmax;
                generator.voltage_regulation_on =
                    required_bool(&equipment.attrs, "voltageRegulatorOn")?;
                generator.regulating_terminal = equipment
                    .regulating_terminal
                    .as_ref()
                    .map(|reference| resolve_terminal_reference(parsed, reference))
                    .transpose()?;
                let regulated_bus = equipment
                    .regulating_terminal
                    .as_ref()
                    .map(|reference| {
                        resolve_regulating_bus(parsed, reference, &bus_builder, &voltage_levels)
                    })
                    .transpose()?
                    .flatten();
                generator.regulated_bus = regulated_bus.filter(|bus| *bus != generator.bus);
                let nominal = regulated_bus
                    .and_then(|bus| {
                        bus_builder
                            .buses
                            .iter()
                            .find(|candidate| candidate.id == bus)
                            .map(|candidate| candidate.base_kv)
                    })
                    .unwrap_or(
                        voltage_levels[terminal_records[0].1.voltage_level.local_id()].nominal_v,
                    );
                generator.vg = optional_f64(&equipment.attrs, "targetV")?
                    .map_or(1.0, |target| target / nominal);
                generator.mbase =
                    optional_f64(&equipment.attrs, "ratedS")?.unwrap_or(DEFAULT_BASE_MVA);
                generator.in_service = connected1;
                generator
                    .active_power_control
                    .clone_from(&equipment.active_power_control);
                generator_bus_by_id.insert(equipment.id.as_str(), bus1);
                generators.push(generator);
            }
            EquipmentKind::Battery => {
                let mut item = Storage::new(bus1);
                item.uid = Some(equipment.id.clone());
                item.ps = assigned_power_or_zero(
                    parsed,
                    equipment,
                    "targetP",
                    &missing_assignments,
                    diagnostics,
                )?;
                item.qs = assigned_power_or_zero(
                    parsed,
                    equipment,
                    "targetQ",
                    &missing_assignments,
                    diagnostics,
                )?;
                item.charge_rating = (-required_f64(&equipment.attrs, "minP")?).max(0.0);
                item.discharge_rating = required_f64(&equipment.attrs, "maxP")?.max(0.0);
                if let Some(limits) = &equipment.reactive_limits {
                    let (qmin, qmax) = reactive_limits_at_active_power(
                        &format!("battery `{}`", equipment.id),
                        limits,
                        item.ps,
                    )?;
                    item.qmin = qmin;
                    item.qmax = qmax;
                }
                item.in_service = connected1;
                item.active_power_control
                    .clone_from(&equipment.active_power_control);
                storage.push(item);
            }
            EquipmentKind::BoundaryLine => {
                boundary_bus_by_id.insert(equipment.id.as_str(), (bus1, connected1));
            }
            EquipmentKind::Shunt => {
                let nominal =
                    voltage_levels[terminal_records[0].1.voltage_level.local_id()].nominal_v;
                let assigned_section_count = assigned_section_count_or_zero(
                    parsed,
                    equipment,
                    &missing_assignments,
                    diagnostics,
                )?;
                let section_count = assigned_section_count.unwrap_or(0) as usize;
                let (g_si, b_si, blocks) = if let Some((g, b, maximum)) = equipment.shunt_linear {
                    if section_count > maximum as usize {
                        return Err(format_error(format!(
                            "shunt `{}` sectionCount exceeds maximumSectionCount",
                            equipment.id
                        )));
                    }
                    (
                        g * section_count as f64,
                        b * section_count as f64,
                        vec![ShuntBlock::with_admittance(
                            maximum,
                            g * nominal * nominal,
                            b * nominal * nominal,
                        )],
                    )
                } else if !equipment.shunt_sections.is_empty() {
                    if section_count > equipment.shunt_sections.len() {
                        return Err(format_error(format!(
                            "shunt `{}` sectionCount exceeds its nonlinear section count",
                            equipment.id
                        )));
                    }
                    let (g_si, b_si) = equipment
                        .shunt_sections
                        .iter()
                        .take(section_count)
                        .fold((0.0, 0.0), |(ga, ba), (g, b)| (ga + g, ba + b));
                    let blocks = equipment
                        .shunt_sections
                        .iter()
                        .map(|(g, b)| {
                            ShuntBlock::with_admittance(
                                1,
                                g * nominal * nominal,
                                b * nominal * nominal,
                            )
                        })
                        .collect();
                    (g_si, b_si, blocks)
                } else {
                    return Err(format_error(format!(
                        "shunt `{}` has no shunt model",
                        equipment.id
                    )));
                };
                let mut shunt =
                    Shunt::new(bus1, g_si * nominal * nominal, b_si * nominal * nominal);
                shunt.uid = Some(equipment.id.clone());
                shunt.in_service = connected1;
                shunt.section_count = assigned_section_count;
                let regulating = required_bool(&equipment.attrs, "voltageRegulatorOn")?;
                if blocks
                    .iter()
                    .map(|block| block.steps as usize)
                    .sum::<usize>()
                    > 1
                    || regulating
                    || section_count == 0
                {
                    let target = optional_f64(&equipment.attrs, "targetV")?
                        .map_or(1.0, |value| value / nominal);
                    let deadband =
                        optional_f64(&equipment.attrs, "targetDeadband")?.unwrap_or(0.0) / nominal;
                    let control_bus = equipment
                        .regulating_terminal
                        .as_ref()
                        .map(|reference| {
                            resolve_regulating_bus(parsed, reference, &bus_builder, &voltage_levels)
                        })
                        .transpose()?
                        .flatten();
                    shunt.control = Some(SwitchedShuntControl {
                        mode: if regulating {
                            SwitchedShuntMode::Discrete
                        } else {
                            SwitchedShuntMode::Locked
                        },
                        vhigh: target + deadband / 2.0,
                        vlow: target - deadband / 2.0,
                        control_bus,
                        regulating_terminal: equipment
                            .regulating_terminal
                            .as_ref()
                            .map(|reference| resolve_terminal_reference(parsed, reference))
                            .transpose()?,
                        rmpct: 100.0,
                        blocks,
                    });
                }
                shunts.push(shunt);
            }
            EquipmentKind::StaticVarCompensator => {
                let (mode, legacy_off) = match equipment
                    .attrs
                    .get("regulationMode")
                    .map(String::as_str)
                {
                    Some("VOLTAGE") => (StaticVarCompensatorRegulationMode::Voltage, false),
                    Some("REACTIVE_POWER") => {
                        (StaticVarCompensatorRegulationMode::ReactivePower, false)
                    }
                    Some("OFF")
                        if parsed
                            .namespace
                            .is_some_and(|namespace| namespace.version <= XiidmVersion::V1_13) =>
                    {
                        (StaticVarCompensatorRegulationMode::Voltage, true)
                    }
                    Some(other) => {
                        return Err(format_error(format!(
                            "static VAR compensator `{}` has unknown regulationMode `{other}`",
                            equipment.id
                        )));
                    }
                    None => {
                        diagnostics.push(
                            &codes::READ_XIIDM_VALUE_DEFAULTED,
                            format!(
                                "static VAR compensator `{}` has no regulationMode; it defaults to VOLTAGE with regulation disabled",
                                equipment.id
                            ),
                        );
                        (StaticVarCompensatorRegulationMode::Voltage, false)
                    }
                };
                let mut svc = StaticVarCompensator::new(
                    bus1,
                    required_f64(&equipment.attrs, "bMin")?,
                    required_f64(&equipment.attrs, "bMax")?,
                );
                svc.uid = Some(equipment.id.clone());
                svc.voltage_setpoint_kv =
                    optional_f64(&equipment.attrs, "voltageSetpoint")?.unwrap_or(0.0);
                svc.reactive_power_setpoint_mvar =
                    optional_f64(&equipment.attrs, "reactivePowerSetpoint")?.unwrap_or(0.0);
                svc.regulation_mode = mode;
                svc.regulating = if parsed
                    .namespace
                    .is_some_and(|namespace| namespace.version <= XiidmVersion::V1_13)
                {
                    !legacy_off && equipment.attrs.contains_key("regulationMode")
                } else {
                    optional_bool(&equipment.attrs, "regulating")?.unwrap_or(false)
                };
                svc.regulating_terminal = equipment
                    .regulating_terminal
                    .as_ref()
                    .map(|reference| resolve_terminal_reference(parsed, reference))
                    .transpose()?;
                svc.p = optional_f64(&equipment.attrs, "p")?.unwrap_or(0.0);
                svc.q = optional_f64(&equipment.attrs, "q")?.unwrap_or(0.0);
                svc.in_service = connected1;
                static_var_compensators.push(svc);
            }
            EquipmentKind::Line | EquipmentKind::Transformer => {
                let bus2 = terminal_records[1].0;
                let level1 = voltage_levels[terminal_records[0].1.voltage_level.local_id()];
                let level2 = voltage_levels[terminal_records[1].1.voltage_level.local_id()];
                let mut branch = if equipment.kind == EquipmentKind::Line {
                    read_line(equipment, bus1, bus2, level1.nominal_v, level2.nominal_v)?
                } else {
                    read_transformer(
                        equipment,
                        bus1,
                        bus2,
                        level1.nominal_v,
                        level2.nominal_v,
                        diagnostics,
                    )?
                };
                branch.uid = Some(equipment.id.clone());
                branch.in_service = connected1 && terminal_records[1].1.connected;
                apply_branch_operational_limits(equipment, &mut branch, diagnostics)?;
                branches.push(branch);
            }
            EquipmentKind::ThreeWindingTransformer => {
                let transformer = read_three_winding_transformer(
                    equipment,
                    &terminal_records,
                    &voltage_levels,
                    diagnostics,
                )?;
                transformers_3w.push(transformer);
            }
            EquipmentKind::VscConverterStation | EquipmentKind::LccConverterStation => {
                converters.insert(equipment.id.as_str(), (bus1, connected1, equipment));
            }
        }
    }

    let boundary_mapping = map_boundary_lines(
        parsed,
        &boundary_bus_by_id,
        &voltage_levels,
        &missing_assignments,
        &mut loads,
        &mut generators,
        &mut branches,
        diagnostics,
    )?;

    for converter in &parsed.voltage_source_converters {
        terminals.extend(ac_dc_converter_terminals(
            &converter.common,
            "voltage_source_converter",
            &bus_builder,
            &voltage_levels,
        )?);
    }
    for converter in &parsed.line_commutated_converters {
        terminals.extend(ac_dc_converter_terminals(
            &converter.common,
            "line_commutated_converter",
            &bus_builder,
            &voltage_levels,
        )?);
    }

    let hvdc = parsed
        .hvdc_lines
        .iter()
        .map(|line| read_hvdc_line(line, &converters, &voltage_levels, parsed, diagnostics))
        .collect::<Result<Vec<_>>>()?;

    let mut switches = Vec::new();
    for raw in &parsed.switches {
        let from = bus_builder.endpoint_bus(&raw.voltage_level, &raw.endpoint1)?;
        let to = bus_builder.endpoint_bus(&raw.voltage_level, &raw.endpoint2)?;
        if from != to {
            let mut switch = Switch::new(from, to, !raw.open);
            switch.uid = Some(raw.id.clone());
            switches.push(switch);
        }
    }

    if let Some(slack) = parsed.slack_terminal.as_deref() {
        if let Some(bus) = generator_bus_by_id.get(slack).copied() {
            set_bus_kind(&mut buses, bus, BusType::Ref);
        } else {
            diagnostics.push(
                &codes::READ_XIIDM_FIELD_UNMAPPED,
                format!("slack terminal `{slack}` does not identify a mapped generator"),
            );
        }
    }
    for generator in &generators {
        if buses
            .iter()
            .find(|bus| bus.id == generator.bus)
            .is_some_and(|bus| bus.kind != BusType::Ref)
        {
            set_bus_kind(&mut buses, generator.bus, BusType::Pv);
        }
    }
    if !buses.is_empty() && !buses.iter().any(|bus| bus.kind == BusType::Ref) {
        let reference = generators
            .iter()
            .find(|generator| generator.in_service)
            .map_or(buses[0].id, |generator| generator.bus);
        set_bus_kind(&mut buses, reference, BusType::Ref);
        diagnostics.push(
            &codes::READ_XIIDM_VALUE_DEFAULTED,
            format!(
                "XIIDM declares no mapped slack terminal; bus {reference} is the calculation reference"
            ),
        );
    }
    diagnostics.push(
        &codes::READ_XIIDM_VALUE_DEFAULTED,
        "XIIDM has no system MVA base; the balanced calculation view uses 100 MVA",
    );

    let detailed = build_detailed_connectivity(
        parsed,
        &bus_builder,
        terminals,
        boundary_mapping.boundary_lines,
        boundary_mapping.tie_lines,
    )?;
    let mut network = BalancedNetwork::new(
        parsed.id.clone().unwrap_or_else(|| "network".to_owned()),
        DEFAULT_BASE_MVA,
    );
    *network.case_metadata_mut() = parsed.case_metadata.clone();
    *network.source_format_mut() = SourceFormat::Xiidm;
    *network.buses_mut() = buses;
    *network.loads_mut() = loads;
    *network.shunts_mut() = shunts;
    *network.static_var_compensators_mut() = static_var_compensators;
    *network.branches_mut() = branches;
    *network.transformers_3w_mut() = transformers_3w;
    *network.switches_mut() = switches;
    *network.generators_mut() = generators;
    *network.storage_mut() = storage;
    *network.hvdc_mut() = hvdc;
    *network.areas_mut() = areas;
    *network.detailed_connectivity_mut() = Some(std::sync::Arc::new(detailed));
    network.assign_missing_component_ids();
    network.check_references(FORMAT)?;
    Ok(network)
}

fn reactive_limits_at_active_power(
    owner: &str,
    limits: &ReactiveLimits,
    active_power_mw: f64,
) -> Result<(f64, f64)> {
    crate::network::calc_reactive_limits_at_active_power(owner, limits, active_power_mw)
        .map_err(format_error)
}

struct BoundaryMapping {
    boundary_lines: Vec<BoundaryLine>,
    tie_lines: Vec<TieLine>,
}

#[allow(clippy::too_many_arguments)]
fn map_boundary_lines(
    parsed: &ParsedXiidm,
    boundary_bus_by_id: &HashMap<&str, (BusId, bool)>,
    voltage_levels: &HashMap<&str, &RawVoltageLevel>,
    missing_assignments: &MissingAssignmentGroups,
    loads: &mut Vec<Load>,
    generators: &mut Vec<Generator>,
    branches: &mut Vec<Branch>,
    diagnostics: &mut Diagnostics,
) -> Result<BoundaryMapping> {
    let by_id = parsed
        .equipment
        .iter()
        .filter(|equipment| equipment.kind == EquipmentKind::BoundaryLine)
        .map(|equipment| (equipment.id.as_str(), equipment))
        .collect::<HashMap<_, _>>();
    let mut paired = HashSet::new();
    for tie in &parsed.tie_lines {
        if tie.boundary_line1 == tie.boundary_line2 {
            return Err(format_error(format!(
                "tie line `{}` references the same boundary line twice",
                tie.id
            )));
        }
        for id in [&tie.boundary_line1, &tie.boundary_line2] {
            if !by_id.contains_key(id.as_str()) {
                return Err(format_error(format!(
                    "tie line `{}` references missing boundary line `{id}`",
                    tie.id
                )));
            }
            if !paired.insert(id.as_str()) {
                return Err(format_error(format!(
                    "boundary line `{id}` belongs to more than one tie line"
                )));
            }
        }
    }

    let mut boundary_lines = Vec::with_capacity(by_id.len());
    let mut unpaired_count = 0_usize;
    for equipment in parsed
        .equipment
        .iter()
        .filter(|equipment| equipment.kind == EquipmentKind::BoundaryLine)
    {
        let voltage_level_id = equipment
            .voltage_level
            .as_deref()
            .ok_or_else(|| format_error("boundary line has no voltage level"))?;
        let level = voltage_levels
            .get(voltage_level_id)
            .copied()
            .ok_or_else(|| {
                format_error(format!(
                    "boundary line `{}` references missing voltage level `{voltage_level_id}`",
                    equipment.id
                ))
            })?;
        let (bus, connected) = boundary_bus_by_id
            .get(equipment.id.as_str())
            .copied()
            .ok_or_else(|| format_error("boundary line has no calculated terminal bus"))?;
        let p0 = assigned_power_or_zero(parsed, equipment, "p0", missing_assignments, diagnostics)?;
        let q0 = assigned_power_or_zero(parsed, equipment, "q0", missing_assignments, diagnostics)?;
        let has_generation = [
            "generationVoltageRegulationOn",
            "generationMinP",
            "generationMaxP",
            "generationTargetP",
            "generationTargetQ",
            "generationTargetV",
        ]
        .iter()
        .any(|attribute| equipment.attrs.contains_key(*attribute))
            || equipment.reactive_limits.is_some();
        let generation = if has_generation {
            Some(BoundaryLineGeneration {
                voltage_regulation_on: optional_bool(
                    &equipment.attrs,
                    "generationVoltageRegulationOn",
                )?
                .unwrap_or(false),
                minimum_active_power_mw: optional_f64(&equipment.attrs, "generationMinP")?,
                maximum_active_power_mw: optional_f64(&equipment.attrs, "generationMaxP")?,
                target_active_power_mw: optional_f64(&equipment.attrs, "generationTargetP")?,
                target_reactive_power_mvar: optional_f64(&equipment.attrs, "generationTargetQ")?,
                target_voltage_kv: optional_f64(&equipment.attrs, "generationTargetV")?,
                reactive_limits: equipment.reactive_limits.clone(),
            })
        } else {
            if equipment.reactive_limits.is_some() {
                return Err(format_error(format!(
                    "boundary line `{}` has reactive limits without generation",
                    equipment.id
                )));
            }
            None
        };
        let is_paired = paired.contains(equipment.id.as_str());
        let calculation_load = (!is_paired)
            .then(|| component_id("load", &equipment.id))
            .transpose()?;
        let calculation_generator = (!is_paired && generation.is_some())
            .then(|| component_id("generator", &equipment.id))
            .transpose()?;
        if !is_paired {
            unpaired_count += 1;
            let mut load = Load::new(bus, p0, q0);
            load.uid = Some(equipment.id.clone());
            load.in_service = connected;
            loads.push(load);
            if let Some(generation) = &generation {
                let mut generator = Generator::new(bus);
                generator.uid = Some(equipment.id.clone());
                generator.pg = generation.target_active_power_mw.unwrap_or(0.0);
                generator.qg = generation.target_reactive_power_mvar.unwrap_or(0.0);
                generator.pmin = generation.minimum_active_power_mw.unwrap_or(generator.pg);
                generator.pmax = generation.maximum_active_power_mw.unwrap_or(generator.pg);
                if let Some(limits) = &generation.reactive_limits {
                    let (qmin, qmax) = reactive_limits_at_active_power(
                        &format!("boundary line `{}` generation", equipment.id),
                        limits,
                        generator.pg,
                    )?;
                    generator.qmin = qmin;
                    generator.qmax = qmax;
                } else {
                    generator.qmin = generator.qg;
                    generator.qmax = generator.qg;
                }
                generator.vg = generation
                    .target_voltage_kv
                    .map_or(1.0, |voltage| voltage / level.nominal_v);
                generator.mbase = DEFAULT_BASE_MVA;
                generator.in_service = connected;
                generators.push(generator);
            }
        }
        boundary_lines.push(BoundaryLine {
            component: component_id("boundary_line", &equipment.id)?,
            voltage_level: component_id("voltage_level", voltage_level_id)?,
            active_power_setpoint_mw: p0,
            reactive_power_setpoint_mvar: q0,
            resistance_ohm: required_f64(&equipment.attrs, "r")?,
            reactance_ohm: required_f64(&equipment.attrs, "x")?,
            conductance_siemens: optional_f64(&equipment.attrs, "g")?.unwrap_or(0.0),
            susceptance_siemens: optional_f64(&equipment.attrs, "b")?.unwrap_or(0.0),
            pairing_key: equipment.attrs.get("pairingKey").cloned(),
            generation,
            calculation_load,
            calculation_generator,
        });
    }
    if unpaired_count > 0 {
        diagnostics.push(
            &codes::READ_XIIDM_CALCULATION_VIEW,
            format!(
                "{unpaired_count} unpaired boundary lines use their p0/q0 and optional generation in the balanced calculation view; their line impedance is retained in detailed connectivity"
            ),
        );
    }

    let records = boundary_lines
        .iter()
        .map(|line| (line.component.local_id(), line))
        .collect::<HashMap<_, _>>();
    let mut tie_lines = Vec::with_capacity(parsed.tie_lines.len());
    let mut ignored_boundary_assignments = 0_usize;
    for tie in &parsed.tie_lines {
        let first_raw = by_id[tie.boundary_line1.as_str()];
        let second_raw = by_id[tie.boundary_line2.as_str()];
        let first = records[tie.boundary_line1.as_str()];
        let second = records[tie.boundary_line2.as_str()];
        let (from, connected1) = boundary_bus_by_id[tie.boundary_line1.as_str()];
        let (to, connected2) = boundary_bus_by_id[tie.boundary_line2.as_str()];
        let nominal_v1 = voltage_levels[first_raw
            .voltage_level
            .as_deref()
            .expect("boundary voltage level")]
        .nominal_v;
        let nominal_v2 = voltage_levels[second_raw
            .voltage_level
            .as_deref()
            .expect("boundary voltage level")]
        .nominal_v;
        let mut attrs = Attrs::new();
        for (name, value) in [
            ("r", first.resistance_ohm + second.resistance_ohm),
            ("x", first.reactance_ohm + second.reactance_ohm),
            ("g1", first.conductance_siemens),
            ("b1", first.susceptance_siemens),
            ("g2", second.conductance_siemens),
            ("b2", second.susceptance_siemens),
        ] {
            attrs.insert(name.to_owned(), value.to_string());
        }
        for (source, target) in [
            ((first_raw, "p"), "p1"),
            ((first_raw, "q"), "q1"),
            ((second_raw, "p"), "p2"),
            ((second_raw, "q"), "q2"),
        ] {
            if let Some(value) = source.0.attrs.get(source.1) {
                attrs.insert(target.to_owned(), value.clone());
            }
        }
        let mut branch = read_line_attributes(&attrs, from, to, nominal_v1, nominal_v2)?;
        branch.uid = Some(tie.id.clone());
        branch.in_service = connected1 && connected2;
        apply_tie_line_operational_limits(
            first_raw,
            second_raw,
            &tie.id,
            &mut branch,
            diagnostics,
        )?;
        branches.push(branch);
        if first.active_power_setpoint_mw != 0.0
            || first.reactive_power_setpoint_mvar != 0.0
            || first.generation.is_some()
            || second.active_power_setpoint_mw != 0.0
            || second.reactive_power_setpoint_mvar != 0.0
            || second.generation.is_some()
        {
            ignored_boundary_assignments += 1;
        }
        tie_lines.push(TieLine {
            component: component_id("tie_line", &tie.id)?,
            boundary_line1: first.component.clone(),
            boundary_line2: second.component.clone(),
            calculation_branch: Some(component_id("branch", &tie.id)?),
        });
    }
    if ignored_boundary_assignments > 0 {
        diagnostics.push(
            &codes::READ_XIIDM_CALCULATION_VIEW,
            format!(
                "{ignored_boundary_assignments} tie lines have boundary p0/q0 or generation assignments; the balanced calculation branch omits those assignments while detailed connectivity retains them"
            ),
        );
    }
    Ok(BoundaryMapping {
        boundary_lines,
        tie_lines,
    })
}

fn apply_tie_line_operational_limits(
    first: &RawEquipment,
    second: &RawEquipment,
    tie_id: &str,
    branch: &mut Branch,
    diagnostics: &mut Diagnostics,
) -> Result<()> {
    let mut combined = first.clone();
    combined.kind = EquipmentKind::Line;
    tie_id.clone_into(&mut combined.id);
    combined.attrs.clear();
    combined.operational_limits.clear();
    for group in &first.operational_limits {
        let mut group = group.clone();
        group.side = 1;
        combined.operational_limits.push(group);
    }
    for group in &second.operational_limits {
        let mut group = group.clone();
        group.side = 2;
        combined.operational_limits.push(group);
    }
    for (equipment, side) in [(first, 1_u8), (second, 2_u8)] {
        let ids = selected_operational_limit_group_ids(equipment, 1)?;
        if !ids.is_empty() {
            let ids = ids.iter().map(String::as_str).collect::<Vec<_>>();
            combined.attrs.insert(
                format!("selectedOperationalLimitsGroupIds{side}"),
                format_string_array(&ids),
            );
        }
    }
    apply_branch_operational_limits(&combined, branch, diagnostics)
}

fn map_areas(
    parsed: &ParsedXiidm,
    bus_builder: &BusBuilder<'_>,
    buses: &mut [Bus],
    diagnostics: &mut Diagnostics,
) -> Result<Vec<Area>> {
    let voltage_levels = parsed
        .voltage_levels
        .iter()
        .map(|level| level.id.as_str())
        .collect::<HashSet<_>>();
    let mut used_numbers = HashSet::new();
    let mut assigned = HashMap::<BusId, (usize, &str)>::new();
    let mut next_number = 1_usize;
    let mut areas = Vec::with_capacity(parsed.areas.len());

    for raw in &parsed.areas {
        let preferred = raw
            .id
            .strip_prefix('A')
            .or_else(|| {
                raw.id
                    .chars()
                    .all(|value| value.is_ascii_digit())
                    .then_some(raw.id.as_str())
            })
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value != 0 && !used_numbers.contains(value));
        let number = preferred.unwrap_or_else(|| {
            while used_numbers.contains(&next_number) {
                next_number += 1;
            }
            next_number
        });
        used_numbers.insert(number);
        next_number = next_number.max(number.saturating_add(1));

        for voltage_level in &raw.voltage_levels {
            if !voltage_levels.contains(voltage_level.as_str()) {
                return Err(format_error(format!(
                    "area `{}` references missing voltage level `{voltage_level}`",
                    raw.id
                )));
            }
            let mut members = bus_builder
                .bus_map
                .iter()
                .filter_map(|((level, _), bus)| (level == voltage_level).then_some(*bus))
                .chain(
                    bus_builder
                        .node_map
                        .iter()
                        .filter_map(|((level, _), bus)| (level == voltage_level).then_some(*bus)),
                )
                .collect::<BTreeSet<_>>();
            for bus_id in &members {
                if let Some((existing, existing_type)) = assigned.get(bus_id).copied()
                    && existing != number
                {
                    diagnostics.push(
                        &codes::READ_XIIDM_FIELD_UNMAPPED,
                        format!(
                            "bus {bus_id} belongs to both XIIDM `{existing_type}` and `{}` areas; the balanced bus table keeps one area number",
                            raw.area_type
                        ),
                    );
                    if raw.area_type != "ControlArea" || existing_type == "ControlArea" {
                        continue;
                    }
                }
                assigned.insert(*bus_id, (number, raw.area_type.as_str()));
                if let Some(bus) = buses.iter_mut().find(|bus| bus.id == *bus_id) {
                    bus.area = number;
                }
            }
            members.clear();
        }

        areas.push(Area {
            number,
            slack_bus: None,
            net_interchange: raw.interchange_target.unwrap_or(0.0),
            tolerance: 0.0,
            name: raw.name.clone(),
            uid: Some(raw.id.clone()),
            area_type: Some(raw.area_type.clone()),
        });
    }
    Ok(areas)
}

fn set_bus_kind(buses: &mut [Bus], id: BusId, kind: BusType) {
    if let Some(bus) = buses.iter_mut().find(|bus| bus.id == id) {
        bus.kind = kind;
    }
}

#[derive(Clone)]
struct BusBuilder<'a> {
    parsed: &'a ParsedXiidm,
    buses: Vec<Bus>,
    bus_map: HashMap<(String, String), BusId>,
    node_map: HashMap<(String, i32), BusId>,
    empty_node_breaker_levels: Vec<String>,
    derived_node_breaker_levels: Vec<String>,
    joined_bus_breaker_levels: Vec<(String, usize)>,
}

struct TerminalConnection {
    bus_id: BusId,
    connected: bool,
    bus: Option<ComponentId>,
    connectable_bus: Option<ComponentId>,
    node: Option<ComponentId>,
}

fn report_voltage_level_group(
    diagnostics: &mut Diagnostics,
    levels: &[String],
    singular: &str,
    plural: &str,
) {
    if levels.len() <= EXACT_ASSIGNMENT_DIAGNOSTIC_LIMIT {
        for level in levels {
            diagnostics.push(
                &codes::READ_XIIDM_VALUE_DEFAULTED,
                format!("node breaker voltage level `{level}` {singular}"),
            );
        }
    } else {
        let samples = levels
            .iter()
            .take(ASSIGNMENT_DIAGNOSTIC_SAMPLE_LIMIT)
            .map(|level| format!("`{level}`"))
            .collect::<Vec<_>>()
            .join(", ");
        diagnostics.push(
            &codes::READ_XIIDM_VALUE_DEFAULTED,
            format!(
                "{} node breaker voltage levels {plural} (sample IDs: {samples})",
                levels.len()
            ),
        );
    }
}

impl<'a> BusBuilder<'a> {
    fn new(parsed: &'a ParsedXiidm) -> Self {
        Self {
            parsed,
            buses: Vec::new(),
            bus_map: HashMap::new(),
            node_map: HashMap::new(),
            empty_node_breaker_levels: Vec::new(),
            derived_node_breaker_levels: Vec::new(),
            joined_bus_breaker_levels: Vec::new(),
        }
    }

    fn build(&mut self, diagnostics: &mut Diagnostics) -> Result<()> {
        for level in &self.parsed.voltage_levels {
            match level.topology {
                RawTopologyKind::BusBreaker => self.build_bus_breaker(level, diagnostics)?,
                RawTopologyKind::NodeBreaker => self.build_node_breaker(level, diagnostics)?,
            }
        }
        self.report_topology_summaries(diagnostics);
        Ok(())
    }

    fn report_topology_summaries(&self, diagnostics: &mut Diagnostics) {
        report_voltage_level_group(
            diagnostics,
            &self.empty_node_breaker_levels,
            "is empty and has no calculated bus",
            "are empty and have no calculated bus",
        );
        report_voltage_level_group(
            diagnostics,
            &self.derived_node_breaker_levels,
            "has no calculated bus records; buses were derived from closed switches and internal connections",
            "have no calculated bus records; buses were derived from closed switches and internal connections",
        );
        if self.joined_bus_breaker_levels.len() <= EXACT_ASSIGNMENT_DIAGNOSTIC_LIMIT {
            for (level, count) in &self.joined_bus_breaker_levels {
                diagnostics.push(
                    &codes::READ_XIIDM_FIELD_UNMAPPED,
                    format!(
                        "{count} configured buses in voltage level `{level}` form one energized calculation bus"
                    ),
                );
            }
        } else {
            let samples = self
                .joined_bus_breaker_levels
                .iter()
                .take(ASSIGNMENT_DIAGNOSTIC_SAMPLE_LIMIT)
                .map(|(level, _)| format!("`{level}`"))
                .collect::<Vec<_>>()
                .join(", ");
            diagnostics.push(
                &codes::READ_XIIDM_FIELD_UNMAPPED,
                format!(
                    "{} bus breaker voltage levels join configured buses into energized calculation buses (sample IDs: {samples})",
                    self.joined_bus_breaker_levels.len()
                ),
            );
        }
    }

    fn build_bus_breaker(
        &mut self,
        level: &RawVoltageLevel,
        _diagnostics: &mut Diagnostics,
    ) -> Result<()> {
        let raw_buses: Vec<_> = self
            .parsed
            .buses
            .iter()
            .filter(|bus| bus.voltage_level == level.id)
            .collect();
        if raw_buses.is_empty() {
            return Err(format_error(format!(
                "bus breaker voltage level `{}` has no buses",
                level.id
            )));
        }
        let ids: Vec<String> = raw_buses
            .iter()
            .map(|bus| bus.id.clone().expect("bus breaker bus has id"))
            .collect();
        let index: HashMap<&str, usize> = ids
            .iter()
            .enumerate()
            .map(|(position, id)| (id.as_str(), position))
            .collect();
        let mut union = UnionFind::new(ids.len());
        for switch in self
            .parsed
            .switches
            .iter()
            .filter(|switch| switch.voltage_level == level.id && !switch.open)
        {
            let (RawEndpoint::Bus(first), RawEndpoint::Bus(second)) =
                (&switch.endpoint1, &switch.endpoint2)
            else {
                return Err(format_error(format!(
                    "bus breaker switch `{}` does not name bus endpoints",
                    switch.id
                )));
            };
            let first = *index.get(first.as_str()).ok_or_else(|| {
                format_error(format!(
                    "switch `{}` references missing bus `{first}`",
                    switch.id
                ))
            })?;
            let second = *index.get(second.as_str()).ok_or_else(|| {
                format_error(format!(
                    "switch `{}` references missing bus `{second}`",
                    switch.id
                ))
            })?;
            union.union(first, second);
        }
        let mut groups = BTreeMap::<usize, Vec<usize>>::new();
        for position in 0..ids.len() {
            groups
                .entry(union.find(position))
                .or_default()
                .push(position);
        }
        for positions in groups.values() {
            let raw = raw_buses[positions[0]];
            let id = self.push_bus(level, raw, raw.id.clone());
            for position in positions {
                self.bus_map
                    .insert((level.id.clone(), ids[*position].clone()), id);
            }
            if positions.len() > 1 {
                self.joined_bus_breaker_levels
                    .push((level.id.clone(), positions.len()));
            }
        }
        Ok(())
    }

    fn build_node_breaker(
        &mut self,
        level: &RawVoltageLevel,
        _diagnostics: &mut Diagnostics,
    ) -> Result<()> {
        let mut nodes = BTreeSet::new();
        for bus in self
            .parsed
            .buses
            .iter()
            .filter(|bus| bus.voltage_level == level.id)
        {
            nodes.extend(bus.nodes.iter().copied());
        }
        for busbar in self
            .parsed
            .busbar_sections
            .iter()
            .filter(|value| value.voltage_level == level.id)
        {
            nodes.insert(busbar.node);
        }
        for connection in self
            .parsed
            .internal_connections
            .iter()
            .filter(|value| value.voltage_level == level.id)
        {
            nodes.insert(connection.node1);
            nodes.insert(connection.node2);
        }
        for switch in self
            .parsed
            .switches
            .iter()
            .filter(|value| value.voltage_level == level.id)
        {
            if let RawEndpoint::Node(node) = switch.endpoint1 {
                nodes.insert(node);
            }
            if let RawEndpoint::Node(node) = switch.endpoint2 {
                nodes.insert(node);
            }
        }
        for equipment in &self.parsed.equipment {
            for side in 1..=equipment.kind.terminal_count() {
                if equipment_voltage_level(equipment, side)? == level.id
                    && let Some(node) = terminal_i32(&equipment.attrs, "node", side)?
                {
                    nodes.insert(node);
                }
            }
        }
        if nodes.is_empty() {
            self.empty_node_breaker_levels.push(level.id.clone());
            return Ok(());
        }
        let values: Vec<_> = nodes.into_iter().collect();
        let index: HashMap<_, _> = values
            .iter()
            .enumerate()
            .map(|(position, node)| (*node, position))
            .collect();
        let mut union = UnionFind::new(values.len());
        for connection in self
            .parsed
            .internal_connections
            .iter()
            .filter(|value| value.voltage_level == level.id)
        {
            union.union(index[&connection.node1], index[&connection.node2]);
        }
        for switch in self
            .parsed
            .switches
            .iter()
            .filter(|value| value.voltage_level == level.id && !value.open)
        {
            let (RawEndpoint::Node(first), RawEndpoint::Node(second)) =
                (&switch.endpoint1, &switch.endpoint2)
            else {
                return Err(format_error(format!(
                    "node breaker switch `{}` does not name node endpoints",
                    switch.id
                )));
            };
            union.union(index[first], index[second]);
        }
        let mut calculated_buses = HashMap::<usize, &RawBus>::new();
        for bus in self
            .parsed
            .buses
            .iter()
            .filter(|bus| bus.voltage_level == level.id)
        {
            let Some(first) = bus.nodes.first() else {
                return Err(format_error(format!(
                    "calculated bus in voltage level `{}` has no nodes",
                    level.id
                )));
            };
            let root = union.find(index[first]);
            if bus
                .nodes
                .iter()
                .skip(1)
                .any(|node| union.find(index[node]) != root)
            {
                return Err(format_error(format!(
                    "calculated bus in voltage level `{}` lists nodes from distinct connected components",
                    level.id
                )));
            }
            if calculated_buses.insert(root, bus).is_some() {
                return Err(format_error(format!(
                    "voltage level `{}` has more than one calculated bus for the same connected component",
                    level.id
                )));
            }
        }
        let mut groups = BTreeMap::<usize, Vec<i32>>::new();
        for (position, node) in values.iter().enumerate() {
            groups.entry(union.find(position)).or_default().push(*node);
        }
        for (root, group) in &groups {
            let raw = calculated_buses
                .get(root)
                .copied()
                .cloned()
                .unwrap_or(RawBus {
                    id: None,
                    voltage_level: level.id.clone(),
                    nodes: group.clone(),
                    v: None,
                    angle: None,
                });
            let local_id = self
                .parsed
                .busbar_sections
                .iter()
                .find(|value| value.voltage_level == level.id && group.contains(&value.node))
                .map_or_else(
                    || format!("{}/{}", level.id, group[0]),
                    |value| value.id.clone(),
                );
            let id = self.push_bus(level, &raw, Some(local_id));
            for node in group {
                self.node_map.insert((level.id.clone(), *node), id);
            }
        }
        if self
            .parsed
            .buses
            .iter()
            .all(|bus| bus.voltage_level != level.id)
        {
            self.derived_node_breaker_levels.push(level.id.clone());
        }
        Ok(())
    }

    fn push_bus(
        &mut self,
        level: &RawVoltageLevel,
        raw: &RawBus,
        local_id: Option<String>,
    ) -> BusId {
        let id = BusId::new(self.buses.len() + 1);
        let mut bus = Bus::new(id, BusType::Pq, level.nominal_v);
        bus.uid.clone_from(&local_id);
        bus.name = local_id;
        bus.vm = raw.v.map_or(1.0, |value| value / level.nominal_v);
        bus.va = raw.angle.unwrap_or(0.0);
        bus.vmin = level
            .low_voltage_limit
            .map_or(0.9, |value| value / level.nominal_v);
        bus.vmax = level
            .high_voltage_limit
            .map_or(1.1, |value| value / level.nominal_v);
        self.buses.push(bus);
        id
    }

    fn endpoint_bus(&self, voltage_level: &str, endpoint: &RawEndpoint) -> Result<BusId> {
        match endpoint {
            RawEndpoint::Bus(bus) => self
                .bus_map
                .get(&(voltage_level.to_owned(), bus.clone()))
                .copied()
                .ok_or_else(|| {
                    format_error(format!(
                        "voltage level `{voltage_level}` references missing bus `{bus}`"
                    ))
                }),
            RawEndpoint::Node(node) => self
                .node_map
                .get(&(voltage_level.to_owned(), *node))
                .copied()
                .ok_or_else(|| {
                    format_error(format!(
                        "voltage level `{voltage_level}` references missing node `{node}`"
                    ))
                }),
        }
    }

    fn terminal_bus(
        &self,
        voltage_level: &str,
        attrs: &Attrs,
        side: u8,
    ) -> Result<TerminalConnection> {
        let bus = terminal_text(attrs, "bus", side);
        let connectable = terminal_text(attrs, "connectableBus", side);
        let node = terminal_i32(attrs, "node", side)?;
        if let Some(bus) = bus {
            return Ok(TerminalConnection {
                bus_id: self.endpoint_bus(voltage_level, &RawEndpoint::Bus(bus.to_owned()))?,
                connected: true,
                bus: Some(component_id("bus", bus)?),
                connectable_bus: connectable
                    .map(|value| component_id("bus", value))
                    .transpose()?,
                node: None,
            });
        }
        if let Some(node) = node {
            return Ok(TerminalConnection {
                bus_id: self.endpoint_bus(voltage_level, &RawEndpoint::Node(node))?,
                connected: true,
                bus: None,
                connectable_bus: None,
                node: Some(node_component_id(voltage_level, node)?),
            });
        }
        if let Some(connectable) = connectable {
            return Ok(TerminalConnection {
                bus_id: self
                    .endpoint_bus(voltage_level, &RawEndpoint::Bus(connectable.to_owned()))?,
                connected: false,
                bus: None,
                connectable_bus: Some(component_id("bus", connectable)?),
                node: None,
            });
        }
        Err(format_error(format!(
            "terminal {side} has no bus, connectableBus, or node"
        )))
    }
}

fn equipment_terminals(
    equipment: &RawEquipment,
    buses: &BusBuilder<'_>,
    voltage_levels: &HashMap<&str, &RawVoltageLevel>,
) -> Result<Vec<(BusId, Terminal)>> {
    let mut terminals = Vec::with_capacity(equipment.kind.terminal_count() as usize);
    for side in 1..=equipment.kind.terminal_count() {
        let voltage_level = equipment_voltage_level(equipment, side)?;
        if !voltage_levels.contains_key(voltage_level.as_str()) {
            return Err(format_error(format!(
                "equipment `{}` terminal {side} references missing voltage level `{voltage_level}`",
                equipment.id
            )));
        }
        let connection = buses.terminal_bus(&voltage_level, &equipment.attrs, side)?;
        terminals.push((
            connection.bus_id,
            Terminal {
                component: None,
                equipment: component_id(equipment.kind.component_type(), &equipment.id)?,
                terminal: side,
                voltage_level: component_id("voltage_level", &voltage_level)?,
                bus: connection.bus,
                connectable_bus: connection.connectable_bus,
                node: connection.node,
                connected: connection.connected,
                active_power_mw: terminal_f64(&equipment.attrs, "p", side)?,
                reactive_power_mvar: terminal_f64(&equipment.attrs, "q", side)?,
            },
        ));
    }
    Ok(terminals)
}

fn equipment_voltage_level(equipment: &RawEquipment, side: u8) -> Result<String> {
    if equipment.kind.terminal_count() == 1 {
        return equipment
            .voltage_level
            .clone()
            .ok_or_else(|| format_error("single terminal equipment has no voltage level"));
    }
    Ok(required_text(&equipment.attrs, &format!("voltageLevelId{side}"))?.to_owned())
}

fn validate_dc_references(parsed: &ParsedXiidm) -> Result<()> {
    let nodes = parsed
        .dc_nodes
        .iter()
        .map(|node| &node.component)
        .collect::<HashSet<_>>();
    let require_node = |equipment: &ComponentId, node: &ComponentId| -> Result<()> {
        if nodes.contains(node) {
            Ok(())
        } else {
            Err(format_error(format!(
                "DC equipment `{}` references missing DC node `{}`",
                equipment.local_id(),
                node.local_id()
            )))
        }
    };
    let require_terminal_node = |equipment: &ComponentId, terminal: &DcTerminal| -> Result<()> {
        let node = terminal.dc_node.as_ref().ok_or_else(|| {
            format_error(format!(
                "DC equipment `{}` has no physical DC node",
                equipment.local_id()
            ))
        })?;
        require_node(equipment, node)
    };
    for ground in &parsed.dc_grounds {
        require_terminal_node(&ground.component, &ground.dc_terminal)?;
    }
    for line in &parsed.dc_lines {
        require_terminal_node(&line.component, &line.dc_terminal1)?;
        require_terminal_node(&line.component, &line.dc_terminal2)?;
    }
    for switch in &parsed.dc_switches {
        require_terminal_node(&switch.component, &switch.dc_terminal1)?;
        require_terminal_node(&switch.component, &switch.dc_terminal2)?;
    }
    for converter in &parsed.voltage_source_converters {
        let component = component_id("voltage_source_converter", &converter.common.id)?;
        require_terminal_node(&component, &converter_dc_terminal(&converter.common, 1)?)?;
        require_terminal_node(&component, &converter_dc_terminal(&converter.common, 2)?)?;
    }
    for converter in &parsed.line_commutated_converters {
        let component = component_id("line_commutated_converter", &converter.common.id)?;
        require_terminal_node(&component, &converter_dc_terminal(&converter.common, 1)?)?;
        require_terminal_node(&component, &converter_dc_terminal(&converter.common, 2)?)?;
    }
    Ok(())
}

fn converter_dc_terminal(converter: &RawAcDcConverter, side: u8) -> Result<DcTerminal> {
    Ok(DcTerminal {
        component: None,
        sequence_number: None,
        dc_node: Some(component_id(
            "dc_node",
            required_text(&converter.attrs, &format!("dcNode{side}"))?,
        )?),
        dc_topological_node: None,
        polarity: None,
        connected: Some(required_bool(
            &converter.attrs,
            &format!("dcConnected{side}"),
        )?),
        active_power_mw: optional_f64(&converter.attrs, &format!("dcP{side}"))?,
        current_a: optional_f64(&converter.attrs, &format!("dcI{side}"))?,
    })
}

fn ac_dc_converter_control_mode(converter: &RawAcDcConverter) -> Result<AcDcConverterControlMode> {
    let mode = match required_text(&converter.attrs, "controlMode")? {
        "P_PCC" => AcDcConverterControlMode::ActivePowerAtPcc,
        "V_DC" => AcDcConverterControlMode::DcVoltage,
        "P_PCC_DROOP" => AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroopCurve,
        other => {
            return Err(format_error(format!(
                "AC/DC converter `{}` has unknown controlMode `{other}`",
                converter.id
            )));
        }
    };
    match mode {
        AcDcConverterControlMode::ActivePowerAtPcc if !converter.attrs.contains_key("targetP") => {
            return Err(format_error(format!(
                "AC/DC converter `{}` uses P_PCC control without targetP",
                converter.id
            )));
        }
        AcDcConverterControlMode::DcVoltage if !converter.attrs.contains_key("targetVdc") => {
            return Err(format_error(format!(
                "AC/DC converter `{}` uses V_DC control without targetVdc",
                converter.id
            )));
        }
        _ => {}
    }
    Ok(mode)
}

fn validate_ac_dc_converter(converter: &RawAcDcConverter) -> Result<()> {
    for (name, value) in [
        ("idleLoss", required_f64(&converter.attrs, "idleLoss")?),
        (
            "switchingLoss",
            required_f64(&converter.attrs, "switchingLoss")?,
        ),
        (
            "resistiveLoss",
            required_f64(&converter.attrs, "resistiveLoss")?,
        ),
    ] {
        if value < 0.0 {
            return Err(format_error(format!(
                "AC/DC converter `{}` has negative {name}",
                converter.id
            )));
        }
    }
    ac_dc_converter_control_mode(converter)?;
    if converter.pcc_terminal.is_none() {
        return Err(format_error(format!(
            "AC/DC converter `{}` has no pccTerminal",
            converter.id
        )));
    }
    if converter
        .droop_curve
        .as_ref()
        .is_some_and(|curve| curve.segments.is_empty())
    {
        return Err(format_error(format!(
            "AC/DC converter `{}` has an empty droopCurve",
            converter.id
        )));
    }
    if converter.droop_curve.as_ref().is_some_and(|curve| {
        curve
            .segments
            .iter()
            .any(|segment| segment.minimum_voltage_kv > segment.maximum_voltage_kv)
    }) {
        return Err(format_error(format!(
            "AC/DC converter `{}` has a droop segment with minV greater than maxV",
            converter.id
        )));
    }
    Ok(())
}

fn ac_dc_converter_terminals(
    converter: &RawAcDcConverter,
    component_type: &str,
    buses: &BusBuilder<'_>,
    voltage_levels: &HashMap<&str, &RawVoltageLevel>,
) -> Result<Vec<Terminal>> {
    if !voltage_levels.contains_key(converter.voltage_level.as_str()) {
        return Err(format_error(format!(
            "AC/DC converter `{}` references missing voltage level `{}`",
            converter.id, converter.voltage_level
        )));
    }
    let component = component_id(component_type, &converter.id)?;
    let mut terminals = Vec::with_capacity(2);
    for side in 1..=2 {
        let present = terminal_text(&converter.attrs, "bus", side).is_some()
            || terminal_text(&converter.attrs, "connectableBus", side).is_some()
            || terminal_i32(&converter.attrs, "node", side)?.is_some();
        if !present {
            if side == 1 {
                return Err(format_error(format!(
                    "AC/DC converter `{}` has no AC terminal 1",
                    converter.id
                )));
            }
            continue;
        }
        let connection = buses.terminal_bus(&converter.voltage_level, &converter.attrs, side)?;
        terminals.push(Terminal {
            component: None,
            equipment: component.clone(),
            terminal: side,
            voltage_level: component_id("voltage_level", &converter.voltage_level)?,
            bus: connection.bus,
            connectable_bus: connection.connectable_bus,
            node: connection.node,
            connected: connection.connected,
            active_power_mw: terminal_f64(&converter.attrs, "p", side)?,
            reactive_power_mvar: terminal_f64(&converter.attrs, "q", side)?,
        });
    }
    Ok(terminals)
}

fn map_voltage_source_converter(
    parsed: &ParsedXiidm,
    converter: &RawVoltageSourceConverter,
) -> Result<VoltageSourceConverter> {
    validate_ac_dc_converter(&converter.common)?;
    let reactive_limits = converter.reactive_limits.clone().ok_or_else(|| {
        format_error(format!(
            "voltageSourceConverter `{}` has no reactive limits",
            converter.common.id
        ))
    })?;
    match &reactive_limits {
        ReactiveLimits::MinMax(limits)
            if limits.minimum_reactive_power_mvar > limits.maximum_reactive_power_mvar =>
        {
            return Err(format_error(format!(
                "voltageSourceConverter `{}` has minQ greater than maxQ",
                converter.common.id
            )));
        }
        ReactiveLimits::CapabilityCurve(curve) if curve.points.len() < 2 => {
            return Err(format_error(format!(
                "voltageSourceConverter `{}` reactiveCapabilityCurve has fewer than two points",
                converter.common.id
            )));
        }
        ReactiveLimits::CapabilityCurve(curve)
            if curve.points.iter().any(|point| {
                point.minimum_reactive_power_mvar > point.maximum_reactive_power_mvar
            }) =>
        {
            return Err(format_error(format!(
                "voltageSourceConverter `{}` reactiveCapabilityCurve has minQ greater than maxQ",
                converter.common.id
            )));
        }
        _ => {}
    }
    let voltage_regulator_on = required_bool(&converter.common.attrs, "voltageRegulatorOn")?;
    let voltage_setpoint_kv = optional_f64(&converter.common.attrs, "voltageSetpoint")?;
    let reactive_power_setpoint_mvar =
        optional_f64(&converter.common.attrs, "reactivePowerSetpoint")?;
    if voltage_regulator_on && voltage_setpoint_kv.is_none() {
        return Err(format_error(format!(
            "voltageSourceConverter `{}` regulates voltage without voltageSetpoint",
            converter.common.id
        )));
    }
    if !voltage_regulator_on && reactive_power_setpoint_mvar.is_none() {
        return Err(format_error(format!(
            "voltageSourceConverter `{}` regulates reactive power without reactivePowerSetpoint",
            converter.common.id
        )));
    }
    Ok(VoltageSourceConverter {
        component: component_id("voltage_source_converter", &converter.common.id)?,
        dc_converter_unit: None,
        dc_terminal1: converter_dc_terminal(&converter.common, 1)?,
        dc_terminal2: converter_dc_terminal(&converter.common, 2)?,
        base_apparent_power_mva: None,
        minimum_active_power_mw: None,
        maximum_active_power_mw: None,
        minimum_dc_voltage_kv: None,
        maximum_dc_voltage_kv: None,
        rated_dc_voltage_kv: None,
        valve_u0_kv: None,
        number_of_valves: None,
        idle_loss_mw: Some(required_f64(&converter.common.attrs, "idleLoss")?),
        switching_loss_mw_per_ampere: Some(required_f64(&converter.common.attrs, "switchingLoss")?),
        resistive_loss_ohm: Some(required_f64(&converter.common.attrs, "resistiveLoss")?),
        control_mode: Some(ac_dc_converter_control_mode(&converter.common)?),
        active_power_at_pcc_mw: None,
        reactive_power_at_pcc_mvar: None,
        target_active_power_mw: optional_f64(&converter.common.attrs, "targetP")?,
        target_dc_voltage_kv: optional_f64(&converter.common.attrs, "targetVdc")?,
        pcc_terminal: Some(resolve_terminal_reference(
            parsed,
            converter
                .common
                .pcc_terminal
                .as_ref()
                .expect("validated pccTerminal"),
        )?),
        droop_curve: converter.common.droop_curve.clone(),
        droop: None,
        droop_compensation: None,
        q_share: None,
        maximum_modulation_index: None,
        maximum_valve_current_a: None,
        voltage_regulator_on: Some(voltage_regulator_on),
        voltage_setpoint_kv,
        reactive_power_setpoint_mvar,
        reactive_limits: Some(reactive_limits),
        pole_loss_active_power_mw: None,
        dc_current_a: None,
        ac_voltage_kv: None,
        dc_voltage_kv: None,
        delta_degrees: None,
        uf_kv: None,
        uv_kv: None,
    })
}

fn map_line_commutated_converter(
    parsed: &ParsedXiidm,
    converter: &RawLineCommutatedConverter,
) -> Result<LineCommutatedConverter> {
    validate_ac_dc_converter(&converter.common)?;
    let reactive_model = match required_text(&converter.common.attrs, "reactiveModel")? {
        "FIXED_POWER_FACTOR" => LineCommutatedConverterReactiveModel::FixedPowerFactor,
        "CALCULATED_POWER_FACTOR" => LineCommutatedConverterReactiveModel::CalculatedPowerFactor,
        other => {
            return Err(format_error(format!(
                "lineCommutatedConverter `{}` has unknown reactiveModel `{other}`",
                converter.common.id
            )));
        }
    };
    let power_factor = required_f64(&converter.common.attrs, "powerFactor")?;
    if !(0.0..=1.0).contains(&power_factor) {
        return Err(format_error(format!(
            "lineCommutatedConverter `{}` has powerFactor outside [0, 1]",
            converter.common.id
        )));
    }
    Ok(LineCommutatedConverter {
        component: component_id("line_commutated_converter", &converter.common.id)?,
        dc_converter_unit: None,
        dc_terminal1: converter_dc_terminal(&converter.common, 1)?,
        dc_terminal2: converter_dc_terminal(&converter.common, 2)?,
        base_apparent_power_mva: None,
        minimum_active_power_mw: None,
        maximum_active_power_mw: None,
        minimum_dc_voltage_kv: None,
        maximum_dc_voltage_kv: None,
        rated_dc_voltage_kv: None,
        valve_u0_kv: None,
        number_of_valves: None,
        idle_loss_mw: Some(required_f64(&converter.common.attrs, "idleLoss")?),
        switching_loss_mw_per_ampere: Some(required_f64(&converter.common.attrs, "switchingLoss")?),
        resistive_loss_ohm: Some(required_f64(&converter.common.attrs, "resistiveLoss")?),
        control_mode: Some(ac_dc_converter_control_mode(&converter.common)?),
        active_power_at_pcc_mw: None,
        reactive_power_at_pcc_mvar: None,
        target_active_power_mw: optional_f64(&converter.common.attrs, "targetP")?,
        target_dc_voltage_kv: optional_f64(&converter.common.attrs, "targetVdc")?,
        pcc_terminal: Some(resolve_terminal_reference(
            parsed,
            converter
                .common
                .pcc_terminal
                .as_ref()
                .expect("validated pccTerminal"),
        )?),
        droop_curve: converter.common.droop_curve.clone(),
        reactive_model: Some(reactive_model),
        power_factor: Some(power_factor),
        operating_mode: None,
        rated_dc_current_a: None,
        minimum_alpha_degrees: None,
        maximum_alpha_degrees: None,
        minimum_gamma_degrees: None,
        maximum_gamma_degrees: None,
        target_alpha_degrees: None,
        target_gamma_degrees: None,
        target_dc_current_a: None,
        pole_loss_active_power_mw: None,
        dc_current_a: None,
        ac_voltage_kv: None,
        dc_voltage_kv: None,
        alpha_degrees: None,
        gamma_degrees: None,
    })
}

fn read_line(
    equipment: &RawEquipment,
    from: BusId,
    to: BusId,
    nominal_v1: f64,
    nominal_v2: f64,
) -> Result<Branch> {
    read_line_attributes(&equipment.attrs, from, to, nominal_v1, nominal_v2)
}

fn read_line_attributes(
    attrs: &Attrs,
    from: BusId,
    to: BusId,
    nominal_v1: f64,
    nominal_v2: f64,
) -> Result<Branch> {
    let r = required_f64(attrs, "r")?;
    let x = required_f64(attrs, "x")?;
    let scale = DEFAULT_BASE_MVA / (nominal_v1 * nominal_v2);
    let mut branch = Branch::new(from, to, r * scale, x * scale);
    let denominator = r * r + x * x;
    let y_real = if denominator == 0.0 {
        0.0
    } else {
        r / denominator
    };
    let y_imag = if denominator == 0.0 {
        0.0
    } else {
        -x / denominator
    };
    let convert = |shunt: f64, at: f64, other: f64, transmission: f64| {
        (shunt * at * at + (at - other) * at * transmission) / DEFAULT_BASE_MVA
    };
    branch.charging = Some(BranchCharging::new(
        convert(
            optional_f64(attrs, "g1")?.unwrap_or(0.0),
            nominal_v1,
            nominal_v2,
            y_real,
        ),
        convert(
            optional_f64(attrs, "b1")?.unwrap_or(0.0),
            nominal_v1,
            nominal_v2,
            y_imag,
        ),
        convert(
            optional_f64(attrs, "g2")?.unwrap_or(0.0),
            nominal_v2,
            nominal_v1,
            y_real,
        ),
        convert(
            optional_f64(attrs, "b2")?.unwrap_or(0.0),
            nominal_v2,
            nominal_v1,
            y_imag,
        ),
    ));
    branch.b = branch.charging.expect("assigned").calc_total_b();
    branch.solution = branch_solution(attrs)?;
    Ok(branch)
}

fn read_transformer(
    equipment: &RawEquipment,
    from: BusId,
    to: BusId,
    nominal_v1: f64,
    nominal_v2: f64,
    diagnostics: &mut Diagnostics,
) -> Result<Branch> {
    let rated_u1 = required_f64(&equipment.attrs, "ratedU1")?;
    let rated_u2 = required_f64(&equipment.attrs, "ratedU2")?;
    if (rated_u2 - nominal_v2).abs() > f64::EPSILON {
        diagnostics.push(
            &codes::READ_XIIDM_FIELD_UNMAPPED,
            format!(
                "XIIDM two winding transformer `{}` has ratedU1={rated_u1} kV and ratedU2={rated_u2} kV while terminal 2 has nominal voltage {nominal_v2} kV; fresh XIIDM output preserves the ratio but normalizes ratedU2 to the terminal voltage level",
                equipment.id
            ),
        );
    }
    let mut r = required_f64(&equipment.attrs, "r")?;
    let mut x = required_f64(&equipment.attrs, "x")?;
    let mut g = optional_f64(&equipment.attrs, "g")?.unwrap_or(0.0);
    let mut b = optional_f64(&equipment.attrs, "b")?.unwrap_or(0.0);
    let mut rho = (rated_u2 / nominal_v2) / (rated_u1 / nominal_v1);
    let mut alpha = 0.0;
    for (name, kind, tap) in [
        ("ratio", TapKind::Ratio, equipment.ratio_tap.as_ref()),
        ("phase", TapKind::Phase, equipment.phase_tap.as_ref()),
    ] {
        if let Some(tap) = tap {
            if let Some(step) = tap.calculation_step(kind) {
                rho *= step.rho;
                r *= 1.0 + step.r_percent / 100.0;
                x *= 1.0 + step.x_percent / 100.0;
                g *= 1.0 + step.g_percent / 100.0;
                b *= 1.0 + step.b_percent / 100.0;
                if name == "phase" {
                    alpha = step.alpha;
                }
            } else {
                diagnostics.push(
                    &codes::READ_XIIDM_FIELD_UNMAPPED,
                    format!(
                        "transformer `{}` {name} tap position is outside its step table",
                        equipment.id
                    ),
                );
            }
        }
    }
    if rho == 0.0 {
        return Err(format_error(format!(
            "transformer `{}` has zero effective voltage ratio",
            equipment.id
        )));
    }
    let zbase = nominal_v2 * nominal_v2 / DEFAULT_BASE_MVA;
    let mut branch = Branch::new(from, to, r / zbase, x / zbase);
    branch.tap = 1.0 / rho;
    branch.shift = -alpha;
    branch.b = b * zbase;
    branch.charging = Some(BranchCharging::new(g * zbase, branch.b, 0.0, 0.0));
    branch.solution = branch_solution(&equipment.attrs)?;
    Ok(branch)
}

#[derive(Default)]
struct LoadingLimitProjection {
    permanent: f64,
    temporary: Vec<(i32, String, f64)>,
}

fn selected_operational_limits_groups(
    equipment: &RawEquipment,
    side: u8,
) -> Result<Vec<&RawOperationalLimitsGroup>> {
    let selected = selected_operational_limit_group_ids(equipment, side)?;
    let selected = (!selected.is_empty()).then(|| selected.into_iter().collect::<HashSet<_>>());
    let candidates = equipment
        .operational_limits
        .iter()
        .filter(|group| group.side == side);
    Ok(selected.map_or_else(Vec::new, |selected| {
        candidates
            .filter(|group| selected.contains(group.id.as_str()))
            .collect()
    }))
}

fn selected_operational_limit_group_ids(equipment: &RawEquipment, side: u8) -> Result<Vec<String>> {
    let (plural_key, singular_key) = if equipment.kind == EquipmentKind::BoundaryLine && side == 1 {
        (
            "selectedOperationalLimitsGroupIds".to_owned(),
            "selectedOperationalLimitsGroupId".to_owned(),
        )
    } else {
        (
            format!("selectedOperationalLimitsGroupIds{side}"),
            format!("selectedOperationalLimitsGroupId{side}"),
        )
    };
    if let Some(value) = equipment.attrs.get(&plural_key) {
        return parse_operational_limits_group_ids(value);
    }
    if let Some(value) = equipment.attrs.get(&singular_key) {
        return Ok(vec![value.clone()]);
    }
    Ok(Vec::new())
}

fn parse_operational_limits_group_ids(value: &str) -> Result<Vec<String>> {
    let mut values = Vec::new();
    let mut current = String::new();
    let mut characters = value.chars().peekable();
    let mut quoted = false;
    let mut at_field_start = true;
    while let Some(character) = characters.next() {
        if quoted {
            if character == '"' {
                if characters.peek() == Some(&'"') {
                    characters.next();
                    current.push('"');
                } else {
                    quoted = false;
                }
            } else {
                current.push(character);
            }
            continue;
        }
        match character {
            ',' => {
                values.push(std::mem::take(&mut current));
                at_field_start = true;
            }
            '"' if at_field_start => {
                quoted = true;
                at_field_start = false;
            }
            '"' => {
                return Err(format_error(
                    "selected operational limits group list contains an unexpected quote",
                ));
            }
            _ => {
                current.push(character);
                at_field_start = false;
            }
        }
    }
    if quoted {
        return Err(format_error(
            "selected operational limits group list contains an unclosed quote",
        ));
    }
    values.push(current);
    Ok(values)
}

fn validate_selected_operational_limits_groups(equipment: &RawEquipment, side: u8) -> Result<()> {
    for selected in selected_operational_limit_group_ids(equipment, side)? {
        if !equipment
            .operational_limits
            .iter()
            .any(|group| group.side == side && group.id == selected)
        {
            return Err(format_error(format!(
                "equipment `{}` selects missing operational limits group `{selected}` on side {side}",
                equipment.id
            )));
        }
    }
    Ok(())
}

fn calc_loading_limit_projection(
    equipment: &RawEquipment,
    side: u8,
    select: impl Fn(&RawOperationalLimitsGroup) -> Option<&RawLoadingLimits>,
) -> Result<LoadingLimitProjection> {
    validate_selected_operational_limits_groups(equipment, side)?;
    let groups = selected_operational_limits_groups(equipment, side)?;
    let mut permanent = Vec::new();
    let mut temporary = BTreeMap::<i32, (String, f64)>::new();
    for group in groups {
        let Some(limits) = select(group) else {
            continue;
        };
        if let Some(value) = limits.permanent_limit {
            if value < 0.0 {
                return Err(format_error(format!(
                    "equipment `{}` has a negative permanent loading limit",
                    equipment.id
                )));
            }
            permanent.push(value);
        }
        for limit in &limits.temporary_limits {
            let duration = limit.acceptable_duration.unwrap_or(i32::MAX);
            if duration < 0 {
                return Err(format_error(format!(
                    "equipment `{}` temporary limit `{}` has a negative acceptableDuration",
                    equipment.id, limit.name
                )));
            }
            let Some(value) = limit.value else {
                continue;
            };
            if value < 0.0 {
                return Err(format_error(format!(
                    "equipment `{}` temporary limit `{}` has a negative value",
                    equipment.id, limit.name
                )));
            }
            temporary
                .entry(duration)
                .and_modify(|(_, existing)| *existing = existing.min(value))
                .or_insert_with(|| (limit.name.clone(), value));
        }
    }
    let mut temporary = temporary
        .into_iter()
        .map(|(duration, (name, value))| (duration, name, value))
        .collect::<Vec<_>>();
    temporary.sort_by_key(|(duration, _, _)| std::cmp::Reverse(*duration));
    Ok(LoadingLimitProjection {
        permanent: permanent.into_iter().reduce(f64::min).unwrap_or(0.0),
        temporary,
    })
}

fn combined_temporary_limits(projections: &[LoadingLimitProjection]) -> Vec<(i32, f64)> {
    let mut by_duration = BTreeMap::<i32, f64>::new();
    for (duration, _, value) in projections
        .iter()
        .flat_map(|projection| projection.temporary.iter())
    {
        by_duration
            .entry(*duration)
            .and_modify(|existing| *existing = existing.min(*value))
            .or_insert(*value);
    }
    let mut limits = by_duration.into_iter().collect::<Vec<_>>();
    limits.sort_by_key(|(duration, _)| std::cmp::Reverse(*duration));
    limits
}

fn apply_branch_operational_limits(
    equipment: &RawEquipment,
    branch: &mut Branch,
    diagnostics: &mut Diagnostics,
) -> Result<()> {
    let mut apparent_by_side = Vec::new();
    let mut current_by_side = Vec::new();
    let mut named_apparent_ratings = BTreeMap::<String, f64>::new();
    for side in 1..=2 {
        let apparent =
            calc_loading_limit_projection(equipment, side, |group| group.apparent_power.as_ref())?;
        let current =
            calc_loading_limit_projection(equipment, side, |group| group.current.as_ref())?;
        if apparent.permanent > 0.0 || !apparent.temporary.is_empty() {
            apparent_by_side.push(apparent);
        }
        if current.permanent > 0.0 || !current.temporary.is_empty() {
            current_by_side.push(current);
        }
        if selected_operational_limits_groups(equipment, side)?
            .iter()
            .any(|group| group.active_power.is_some())
        {
            diagnostics.push(
                &codes::READ_XIIDM_FIELD_UNMAPPED,
                format!(
                    "branch `{}` side {side} active power limits have no source neutral branch field",
                    equipment.id
                ),
            );
        }
        let selected_ids = selected_operational_limit_group_ids(equipment, side)?;
        let selected =
            (!selected_ids.is_empty()).then(|| selected_ids.into_iter().collect::<HashSet<_>>());
        for group in equipment.operational_limits.iter().filter(|group| {
            group.side == side
                && !selected
                    .as_ref()
                    .is_some_and(|selected| selected.contains(group.id.as_str()))
        }) {
            if let Some(limits) = &group.apparent_power {
                if let Some(value) = limits.permanent_limit {
                    let name = limits
                        .permanent_name
                        .clone()
                        .unwrap_or_else(|| group.id.clone());
                    named_apparent_ratings
                        .entry(name)
                        .and_modify(|existing| *existing = existing.min(value))
                        .or_insert(value);
                }
                for limit in &limits.temporary_limits {
                    if let Some(value) = limit.value {
                        let name = format!(
                            "{} {} ({} s)",
                            group.id,
                            limit.name,
                            limit.acceptable_duration.unwrap_or(i32::MAX)
                        );
                        named_apparent_ratings
                            .entry(name)
                            .and_modify(|existing| *existing = existing.min(value))
                            .or_insert(value);
                    }
                }
            }
        }
    }
    branch.rating_sets.extend(
        named_apparent_ratings
            .into_iter()
            .map(|(name, value)| BranchRatingSet::new(name, value)),
    );
    branch.rate_a = apparent_by_side
        .iter()
        .map(|limits| limits.permanent)
        .filter(|value| *value > 0.0)
        .reduce(f64::min)
        .unwrap_or(0.0);
    let apparent_temporary = combined_temporary_limits(&apparent_by_side);
    branch.rate_b = apparent_temporary.first().map_or(0.0, |(_, value)| *value);
    branch.rate_c = apparent_temporary.get(1).map_or(0.0, |(_, value)| *value);

    if !current_by_side.is_empty() {
        let permanent = current_by_side
            .iter()
            .map(|limits| limits.permanent)
            .filter(|value| *value > 0.0)
            .reduce(f64::min)
            .unwrap_or(0.0);
        let temporary = combined_temporary_limits(&current_by_side);
        branch.current_ratings = Some(BranchCurrentRatings::new(
            permanent,
            temporary.first().map_or(0.0, |(_, value)| *value),
            temporary.get(1).map_or(0.0, |(_, value)| *value),
        ));
    }
    Ok(())
}

fn read_three_winding_transformer(
    equipment: &RawEquipment,
    terminals: &[(BusId, Terminal)],
    voltage_levels: &HashMap<&str, &RawVoltageLevel>,
    diagnostics: &mut Diagnostics,
) -> Result<Transformer3W> {
    let rated_u0 = required_f64(&equipment.attrs, "ratedU0")?;
    if rated_u0 <= 0.0 {
        return Err(format_error(format!(
            "three winding transformer `{}` has nonpositive ratedU0",
            equipment.id
        )));
    }
    let zbase = rated_u0 * rated_u0 / DEFAULT_BASE_MVA;
    let mut star_r = [0.0; 3];
    let mut star_x = [0.0; 3];
    let mut star_g = [0.0; 3];
    let mut star_b = [0.0; 3];
    let mut rated_u = [0.0; 3];
    let mut windings = std::array::from_fn(|side| Winding::new(terminals[side].0));
    for side in 0..3 {
        let suffix = side + 1;
        star_r[side] = required_f64(&equipment.attrs, &format!("r{suffix}"))?;
        star_x[side] = required_f64(&equipment.attrs, &format!("x{suffix}"))?;
        star_g[side] = optional_f64(&equipment.attrs, &format!("g{suffix}"))?.unwrap_or(0.0);
        star_b[side] = optional_f64(&equipment.attrs, &format!("b{suffix}"))?.unwrap_or(0.0);
        let winding_rated_u = required_f64(&equipment.attrs, &format!("ratedU{suffix}"))?;
        rated_u[side] = winding_rated_u;
        let level = voltage_levels[terminals[side].1.voltage_level.local_id()];
        let mut rho = 1.0;
        let mut alpha = 0.0;
        for (kind, tap) in [
            (TapKind::Ratio, equipment.winding_ratio_taps[side].as_ref()),
            (TapKind::Phase, equipment.winding_phase_taps[side].as_ref()),
        ] {
            if let Some(tap) = tap {
                if let Some(step) = tap.calculation_step(kind) {
                    rho *= step.rho;
                    star_r[side] *= 1.0 + step.r_percent / 100.0;
                    star_x[side] *= 1.0 + step.x_percent / 100.0;
                    star_g[side] *= 1.0 + step.g_percent / 100.0;
                    star_b[side] *= 1.0 + step.b_percent / 100.0;
                    if kind == TapKind::Phase {
                        alpha = step.alpha;
                    }
                } else {
                    diagnostics.push(
                        &codes::READ_XIIDM_FIELD_UNMAPPED,
                        format!(
                            "three winding transformer `{}` winding {suffix} tap position is outside its step table",
                            equipment.id
                        ),
                    );
                }
            }
        }
        windings[side].tap = winding_rated_u / level.nominal_v * rho;
        windings[side].shift = -alpha;
        windings[side].nominal_kv = winding_rated_u;
        let apparent = calc_loading_limit_projection(equipment, suffix as u8, |group| {
            group.apparent_power.as_ref()
        })?;
        windings[side].rate_a = apparent.permanent;
        windings[side].rate_b = apparent
            .temporary
            .first()
            .map_or(0.0, |(_, _, value)| *value);
        windings[side].rate_c = apparent
            .temporary
            .get(1)
            .map_or(0.0, |(_, _, value)| *value);
        if selected_operational_limits_groups(equipment, suffix as u8)?
            .iter()
            .any(|group| group.current.is_some() || group.active_power.is_some())
        {
            diagnostics.push(
                &codes::READ_XIIDM_FIELD_UNMAPPED,
                format!(
                    "three winding transformer `{}` winding {suffix} current or active power limits have no Transformer3W field",
                    equipment.id
                ),
            );
        }
    }
    let pair = |first: usize, second: usize| {
        Impedance::new(
            (star_r[first] + star_r[second]) / zbase,
            (star_x[first] + star_x[second]) / zbase,
            DEFAULT_BASE_MVA,
        )
    };
    let connected = terminals
        .iter()
        .filter(|(_, terminal)| terminal.connected)
        .count();
    if connected != 0 && connected != 3 {
        diagnostics.push(
            &codes::READ_XIIDM_FIELD_UNMAPPED,
            format!(
                "three winding transformer `{}` has {connected} connected terminals; its calculation record is out of service while detailed terminals retain each connection",
                equipment.id
            ),
        );
    }
    // `ratedU0` is the voltage base the leg impedances are stated on and has
    // no electrical meaning once they are per unit. Fresh output derives it
    // from winding one, so only a differing source base is retained, and the
    // writer restates the legs on it.
    let mut extras = BTreeMap::new();
    if (rated_u0 - rated_u[0]).abs() > f64::EPSILON {
        extras.insert(
            XIIDM_RATED_U0_EXTRA.to_string(),
            serde_json::Value::from(rated_u0),
        );
    }
    if star_g[1] != 0.0 || star_g[2] != 0.0 || star_b[1] != 0.0 || star_b[2] != 0.0 {
        diagnostics.push(
            &codes::READ_XIIDM_FIELD_UNMAPPED,
            format!(
                "three winding transformer `{}` leg shunt admittances were combined at the star point",
                equipment.id
            ),
        );
    }
    Ok(Transformer3W {
        windings,
        z: [pair(0, 1), pair(1, 2), pair(2, 0)],
        star_vm: 1.0,
        star_va: 0.0,
        mag_g: star_g.iter().sum::<f64>() * zbase,
        mag_b: star_b.iter().sum::<f64>() * zbase,
        in_service: connected == 3,
        name: equipment.attrs.get("name").cloned(),
        uid: Some(equipment.id.clone()),
        extras,
    })
}

fn read_hvdc_line(
    line: &RawHvdcLine,
    converters: &HashMap<&str, (BusId, bool, &RawEquipment)>,
    voltage_levels: &HashMap<&str, &RawVoltageLevel>,
    parsed: &ParsedXiidm,
    diagnostics: &mut Diagnostics,
) -> Result<Hvdc> {
    let first_id = required_text(&line.attrs, "converterStation1")?;
    let second_id = required_text(&line.attrs, "converterStation2")?;
    let first = converters.get(first_id).ok_or_else(|| {
        format_error(format!(
            "HVDC line `{}` references missing converter station `{first_id}`",
            line.id
        ))
    })?;
    let second = converters.get(second_id).ok_or_else(|| {
        format_error(format!(
            "HVDC line `{}` references missing converter station `{second_id}`",
            line.id
        ))
    })?;
    let mode = required_text(&line.attrs, "convertersMode")?;
    let (from, to) = match mode {
        "SIDE_1_RECTIFIER_SIDE_2_INVERTER" => (first, second),
        "SIDE_1_INVERTER_SIDE_2_RECTIFIER" => (second, first),
        other => {
            return Err(format_error(format!(
                "HVDC line `{}` has unknown convertersMode `{other}`",
                line.id
            )));
        }
    };
    let setpoint = if let Some(value) = optional_f64(&line.attrs, "activePowerSetpoint")? {
        value
    } else if parsed.namespace.is_some_and(XiidmNamespace::is_equipment) {
        diagnostics.push(
            &codes::READ_XIIDM_VALUE_DEFAULTED,
            format!(
                "XIIDM equipment-mode HVDC line `{}` has no `activePowerSetpoint`; the balanced calculation view uses 0",
                line.id
            ),
        );
        0.0
    } else {
        return Err(format_error(format!(
            "HVDC line `{}` is missing required numeric attribute `activePowerSetpoint`",
            line.id
        )));
    };
    let max_p = required_f64(&line.attrs, "maxP")?;
    let nominal_v = required_f64(&line.attrs, "nominalV")?;
    let resistance = required_f64(&line.attrs, "r")?;
    if setpoint < 0.0 || max_p < 0.0 || nominal_v <= 0.0 || resistance < 0.0 {
        return Err(format_error(format!(
            "HVDC line `{}` has a negative power/resistance or nonpositive nominal voltage",
            line.id
        )));
    }
    let loss_factor = |equipment: &RawEquipment| required_f64(&equipment.attrs, "lossFactor");
    let loss1 = (loss_factor(from.2)? + loss_factor(to.2)?) / 100.0;
    let current_ka = setpoint / nominal_v;
    let loss0 = resistance * current_ka * current_ka;
    if resistance != 0.0 && (max_p - setpoint).abs() > f64::EPSILON {
        diagnostics.push(
            &codes::READ_XIIDM_FIELD_UNMAPPED,
            format!(
                "HVDC line `{}` resistance is retained for XIIDM emission; its typed affine loss matches I²R loss at the active power setpoint only",
                line.id
            ),
        );
    }
    let level = |equipment: &RawEquipment| -> Result<&RawVoltageLevel> {
        let id = equipment
            .voltage_level
            .as_deref()
            .ok_or_else(|| format_error("converter station has no voltage level"))?;
        voltage_levels.get(id).copied().ok_or_else(|| {
            format_error(format!(
                "converter station references missing voltage level `{id}`"
            ))
        })
    };
    let converter_voltage = |equipment: &RawEquipment| -> Result<f64> {
        Ok(
            optional_f64(&equipment.attrs, "voltageSetpoint")?.map_or(1.0, |value| {
                value / level(equipment).expect("checked voltage level").nominal_v
            }),
        )
    };
    let converter_q = |equipment: &RawEquipment| -> Result<(f64, f64, f64)> {
        let q = optional_f64(&equipment.attrs, "q")?
            .or(optional_f64(&equipment.attrs, "reactivePowerSetpoint")?)
            .unwrap_or(0.0);
        let (qmin, qmax) = equipment
            .reactive_limits
            .as_ref()
            .map(|limits| {
                reactive_limits_at_active_power(
                    &format!("converter station `{}`", equipment.id),
                    limits,
                    optional_f64(&equipment.attrs, "p")?.unwrap_or(0.0),
                )
            })
            .transpose()?
            .unwrap_or((q, q));
        Ok((q, qmin, qmax))
    };
    let (qf, qminf, qmaxf) = converter_q(from.2)?;
    let (qt, qmint, qmaxt) = converter_q(to.2)?;
    let mut hvdc = Hvdc::new(from.0, to.0);
    hvdc.uid = Some(line.id.clone());
    hvdc.in_service = from.1 && to.1;
    hvdc.pf = setpoint;
    hvdc.pt = Hvdc::calc_delivered_power(setpoint, loss0, loss1);
    hvdc.qf = qf;
    hvdc.qt = qt;
    hvdc.vf = converter_voltage(from.2)?;
    hvdc.vt = converter_voltage(to.2)?;
    hvdc.pmin = 0.0;
    hvdc.pmax = max_p;
    hvdc.qminf = qminf;
    hvdc.qmaxf = qmaxf;
    hvdc.qmint = qmint;
    hvdc.qmaxt = qmaxt;
    hvdc.loss0 = loss0;
    hvdc.loss1 = loss1;
    hvdc.resistance_ohm = Some(resistance);
    hvdc.nominal_voltage_kv = Some(nominal_v);
    hvdc.converters_mode = Some(match mode {
        "SIDE_1_RECTIFIER_SIDE_2_INVERTER" => HvdcConvertersMode::Side1RectifierSide2Inverter,
        "SIDE_1_INVERTER_SIDE_2_RECTIFIER" => HvdcConvertersMode::Side1InverterSide2Rectifier,
        _ => unreachable!("mode was checked above"),
    });
    hvdc.converter1 = Some(read_hvdc_converter(first.2, parsed)?);
    hvdc.converter2 = Some(read_hvdc_converter(second.2, parsed)?);
    Ok(hvdc)
}

fn read_hvdc_converter(equipment: &RawEquipment, parsed: &ParsedXiidm) -> Result<HvdcConverter> {
    let kind = match equipment.kind {
        EquipmentKind::VscConverterStation => HvdcConverterKind::Vsc,
        EquipmentKind::LccConverterStation => HvdcConverterKind::Lcc,
        _ => {
            return Err(format_error(
                "HVDC line references a non-converter equipment",
            ));
        }
    };
    Ok(HvdcConverter {
        component: component_id("hvdc_converter", &equipment.id)?,
        kind,
        loss_factor_percent: required_f64(&equipment.attrs, "lossFactor")?,
        voltage_regulator_on: (kind == HvdcConverterKind::Vsc)
            .then(|| required_bool(&equipment.attrs, "voltageRegulatorOn"))
            .transpose()?,
        voltage_setpoint_kv: optional_f64(&equipment.attrs, "voltageSetpoint")?,
        reactive_power_setpoint_mvar: optional_f64(&equipment.attrs, "reactivePowerSetpoint")?,
        power_factor: if kind == HvdcConverterKind::Lcc {
            Some(required_f64(&equipment.attrs, "powerFactor")?)
        } else {
            None
        },
        regulating_terminal: equipment
            .regulating_terminal
            .as_ref()
            .map(|reference| resolve_terminal_reference(parsed, reference))
            .transpose()?,
    })
}

fn branch_solution(attrs: &Attrs) -> Result<Option<BranchSolution>> {
    let values = [
        optional_f64(attrs, "p1")?,
        optional_f64(attrs, "q1")?,
        optional_f64(attrs, "p2")?,
        optional_f64(attrs, "q2")?,
    ];
    if values.iter().all(Option::is_none) {
        return Ok(None);
    }
    let [Some(pf), Some(qf), Some(pt), Some(qt)] = values else {
        return Ok(None);
    };
    Ok(Some(BranchSolution::new(pf, qf, pt, qt)))
}

fn map_loading_limits(raw: &RawLoadingLimits) -> Result<LoadingLimits> {
    let temporary_limits = raw
        .temporary_limits
        .iter()
        .map(|limit| {
            let duration = limit.acceptable_duration.unwrap_or(i32::MAX);
            if duration < 0 {
                return Err(format_error(format!(
                    "temporary limit `{}` has a negative acceptableDuration",
                    limit.name
                )));
            }
            let value = limit.value.unwrap_or(f64::MAX);
            if value < 0.0 {
                return Err(format_error(format!(
                    "temporary limit `{}` has a negative value",
                    limit.name
                )));
            }
            Ok(TemporaryLimit {
                name: limit.name.clone(),
                value,
                acceptable_duration_seconds: duration as u64,
                fictitious: limit.fictitious,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(LoadingLimits {
        permanent_limit: raw.permanent_limit,
        permanent_limit_name: raw.permanent_name.clone(),
        temporary_limits,
    })
}

fn map_operational_limit_groups(parsed: &ParsedXiidm) -> Result<Vec<OperationalLimitGroup>> {
    let mut result = Vec::new();
    for equipment in &parsed.equipment {
        let component = component_id(equipment.kind.component_type(), &equipment.id)?;
        for group in &equipment.operational_limits {
            let selected = selected_operational_limit_group_ids(equipment, group.side)?
                .iter()
                .any(|candidate| candidate == &group.id);
            result.push(OperationalLimitGroup {
                equipment: component.clone(),
                terminal: group.side,
                id: group.id.clone(),
                properties: group.properties.clone(),
                selected,
                current_limits: group.current.as_ref().map(map_loading_limits).transpose()?,
                active_power_limits: group
                    .active_power
                    .as_ref()
                    .map(map_loading_limits)
                    .transpose()?,
                apparent_power_limits: group
                    .apparent_power
                    .as_ref()
                    .map(map_loading_limits)
                    .transpose()?,
            });
        }
    }
    Ok(result)
}

fn map_tap_changer(
    parsed: &ParsedXiidm,
    equipment: &RawEquipment,
    raw: &TapChanger,
    winding: u8,
    kind: TapChangerKind,
) -> Result<NetworkTapChanger> {
    if raw.steps.is_empty() {
        return Err(format_error(format!(
            "transformer `{}` tap changer has no step",
            equipment.id
        )));
    }
    let steps = raw
        .steps
        .iter()
        .enumerate()
        .map(|(offset, step)| {
            let offset = i32::try_from(offset)
                .map_err(|_| format_error("tap changer has too many steps"))?;
            let position = raw
                .low_tap_position
                .checked_add(offset)
                .ok_or_else(|| format_error("tap changer step position overflows i32"))?;
            Ok(NetworkTapChangerStep {
                position,
                rho: step.rho,
                alpha_degrees: step.alpha,
                resistance_deviation_percent: step.r_percent,
                reactance_deviation_percent: step.x_percent,
                conductance_deviation_percent: step.g_percent,
                susceptance_deviation_percent: step.b_percent,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(NetworkTapChanger {
        component: None,
        transformer: component_id(equipment.kind.component_type(), &equipment.id)?,
        winding,
        kind,
        tap_position: raw.tap_position,
        solved_tap_position: raw.solved_tap_position,
        low_tap_position: raw.low_tap_position,
        neutral_tap_position: Some(raw.low_tap_position),
        normal_tap_position: raw.tap_position,
        voltage_step_increment_percent: None,
        load_tap_changing_capabilities: raw.load_tap_changing_capabilities,
        regulating: raw.regulating,
        regulation_mode: raw.regulation_mode,
        regulation_value: raw.regulation_value,
        target_deadband: raw.target_deadband,
        regulation_terminal: raw
            .regulation_terminal
            .as_ref()
            .map(|reference| resolve_terminal_reference(parsed, reference))
            .transpose()?,
        steps,
    })
}

fn map_tap_changers(parsed: &ParsedXiidm) -> Result<Vec<NetworkTapChanger>> {
    let mut result = Vec::new();
    for equipment in &parsed.equipment {
        if let Some(tap) = &equipment.ratio_tap {
            result.push(map_tap_changer(
                parsed,
                equipment,
                tap,
                1,
                TapChangerKind::Ratio,
            )?);
        }
        if let Some(tap) = &equipment.phase_tap {
            result.push(map_tap_changer(
                parsed,
                equipment,
                tap,
                1,
                TapChangerKind::Phase,
            )?);
        }
        for winding in 0..3 {
            if let Some(tap) = &equipment.winding_ratio_taps[winding] {
                result.push(map_tap_changer(
                    parsed,
                    equipment,
                    tap,
                    winding as u8 + 1,
                    TapChangerKind::Ratio,
                )?);
            }
            if let Some(tap) = &equipment.winding_phase_taps[winding] {
                result.push(map_tap_changer(
                    parsed,
                    equipment,
                    tap,
                    winding as u8 + 1,
                    TapChangerKind::Phase,
                )?);
            }
        }
    }
    Ok(result)
}

fn build_detailed_connectivity(
    parsed: &ParsedXiidm,
    buses: &BusBuilder<'_>,
    mut terminals: Vec<Terminal>,
    boundary_lines: Vec<BoundaryLine>,
    tie_lines: Vec<TieLine>,
) -> Result<DetailedConnectivity> {
    let substations = parsed
        .substations
        .iter()
        .map(|value| {
            Ok(Substation {
                component: component_id("substation", &value.id)?,
                country: value.country.clone(),
                operator: value.operator.clone(),
                geographical_tags: value.geographical_tags.clone(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let voltage_levels = parsed
        .voltage_levels
        .iter()
        .map(|level| {
            let mut level_buses = buses
                .bus_map
                .iter()
                .filter_map(|((vl, _), bus)| (vl == &level.id).then_some(*bus))
                .chain(
                    buses
                        .node_map
                        .iter()
                        .filter_map(|((vl, _), bus)| (vl == &level.id).then_some(*bus)),
                )
                .collect::<Vec<_>>();
            level_buses.sort_unstable();
            level_buses.dedup();
            Ok(VoltageLevel {
                component: component_id("voltage_level", &level.id)?,
                substation: level
                    .substation
                    .as_deref()
                    .map(|id| component_id("substation", id))
                    .transpose()?,
                nominal_kv: level.nominal_v,
                low_voltage_limit_kv: level.low_voltage_limit,
                high_voltage_limit_kv: level.high_voltage_limit,
                topology_kind: match level.topology {
                    RawTopologyKind::BusBreaker => TopologyKind::BusBreaker,
                    RawTopologyKind::NodeBreaker => TopologyKind::NodeBreaker,
                },
                buses: level_buses,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let connectivity_nodes = buses
        .node_map
        .iter()
        .map(|((voltage_level, node), bus)| {
            Ok(ConnectivityNode {
                component: node_component_id(voltage_level, *node)?,
                voltage_level: component_id("voltage_level", voltage_level)?,
                node_number: Some(*node),
                calculated_bus: Some(*bus),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let mut bus_breaker_buses = buses
        .bus_map
        .iter()
        .map(|((voltage_level, bus), calculated_bus)| {
            let source_bus = parsed.buses.iter().find(|source_bus| {
                source_bus.voltage_level == *voltage_level
                    && source_bus.id.as_deref() == Some(bus.as_str())
            });
            Ok(BusBreakerBus {
                component: component_id("bus", bus)?,
                voltage_level: component_id("voltage_level", voltage_level)?,
                calculated_bus: Some(*calculated_bus),
                voltage_kv: source_bus.and_then(|source_bus| source_bus.v),
                angle_degrees: source_bus.and_then(|source_bus| source_bus.angle),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    bus_breaker_buses.sort_by(|first, second| first.component.cmp(&second.component));
    let calculated_buses = parsed
        .buses
        .iter()
        .filter(|bus| bus.id.is_none())
        .map(|bus| {
            let first_node = *bus
                .nodes
                .first()
                .ok_or_else(|| format_error("calculated bus has no nodes"))?;
            let calculated_bus = buses
                .node_map
                .get(&(bus.voltage_level.clone(), first_node))
                .copied()
                .ok_or_else(|| format_error("calculated bus references an unknown node"))?;
            Ok(CalculatedBus {
                voltage_level: component_id("voltage_level", &bus.voltage_level)?,
                calculated_bus,
                nodes: bus
                    .nodes
                    .iter()
                    .map(|node| node_component_id(&bus.voltage_level, *node))
                    .collect::<Result<Vec<_>>>()?,
                voltage_kv: bus.v,
                angle_degrees: bus.angle,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let busbar_sections = parsed
        .busbar_sections
        .iter()
        .map(|value| {
            Ok(BusbarSection {
                component: component_id("busbar_section", &value.id)?,
                voltage_level: component_id("voltage_level", &value.voltage_level)?,
                node: node_component_id(&value.voltage_level, value.node)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    terminals.extend(busbar_sections.iter().map(|busbar| Terminal {
        component: None,
        equipment: busbar.component.clone(),
        terminal: 1,
        voltage_level: busbar.voltage_level.clone(),
        bus: None,
        connectable_bus: None,
        node: Some(busbar.node.clone()),
        connected: true,
        active_power_mw: None,
        reactive_power_mvar: None,
    }));
    let switches = parsed
        .switches
        .iter()
        .map(|value| {
            Ok(TopologySwitch {
                component: component_id("switch", &value.id)?,
                voltage_level: component_id("voltage_level", &value.voltage_level)?,
                kind: value.kind,
                endpoint1: detailed_endpoint(&value.voltage_level, &value.endpoint1)?,
                endpoint2: detailed_endpoint(&value.voltage_level, &value.endpoint2)?,
                open: value.open,
                retained: value.retained,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let internal_connections = parsed
        .internal_connections
        .iter()
        .map(|value| {
            Ok(InternalConnection {
                voltage_level: component_id("voltage_level", &value.voltage_level)?,
                node1: node_component_id(&value.voltage_level, value.node1)?,
                node2: node_component_id(&value.voltage_level, value.node2)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(DetailedConnectivity {
        omitted_fields: collect_omitted_fields(parsed)?,
        component_metadata: parsed.metadata.values().cloned().collect(),
        subnetworks: parsed
            .subnetworks
            .iter()
            .map(|value| Subnetwork {
                component: value.component.clone(),
                parent: value.parent.clone(),
                case_metadata: value.case_metadata.clone(),
                components: value.components.clone(),
            })
            .collect(),
        substations,
        voltage_levels,
        bus_breaker_buses,
        calculated_buses,
        connectivity_nodes,
        busbar_sections,
        junctions: Vec::new(),
        terminals,
        switches,
        internal_connections,
        operational_limit_groups: map_operational_limit_groups(parsed)?,
        tap_changers: map_tap_changers(parsed)?,
        equipment_reactive_limits: parsed
            .equipment
            .iter()
            .filter_map(|equipment| {
                equipment.reactive_limits.as_ref().map(|limits| {
                    Ok(EquipmentReactiveLimits {
                        equipment: component_id(equipment.kind.component_type(), &equipment.id)?,
                        limits: limits.clone(),
                    })
                })
            })
            .collect::<Result<Vec<_>>>()?,
        boundary_lines,
        tie_lines,
        dc_converter_units: Vec::new(),
        dc_topological_nodes: Vec::new(),
        dc_nodes: parsed.dc_nodes.clone(),
        dc_busbars: Vec::new(),
        dc_grounds: parsed.dc_grounds.clone(),
        dc_series_devices: Vec::new(),
        dc_lines: parsed.dc_lines.clone(),
        dc_switches: parsed.dc_switches.clone(),
        voltage_source_converters: parsed
            .voltage_source_converters
            .iter()
            .map(|converter| map_voltage_source_converter(parsed, converter))
            .collect::<Result<Vec<_>>>()?,
        line_commutated_converters: parsed
            .line_commutated_converters
            .iter()
            .map(|converter| map_line_commutated_converter(parsed, converter))
            .collect::<Result<Vec<_>>>()?,
    })
}

fn detailed_endpoint(voltage_level: &str, endpoint: &RawEndpoint) -> Result<TopologyEndpoint> {
    match endpoint {
        RawEndpoint::Bus(bus) => Ok(TopologyEndpoint::Bus(component_id("bus", bus)?)),
        RawEndpoint::Node(node) => Ok(TopologyEndpoint::Node(node_component_id(
            voltage_level,
            *node,
        )?)),
    }
}

struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    fn new(len: usize) -> Self {
        Self {
            parent: (0..len).collect(),
        }
    }

    fn find(&mut self, value: usize) -> usize {
        let parent = self.parent[value];
        if parent == value {
            value
        } else {
            let root = self.find(parent);
            self.parent[value] = root;
            root
        }
    }

    fn union(&mut self, first: usize, second: usize) {
        let first = self.find(first);
        let second = self.find(second);
        if first != second {
            self.parent[second] = first;
        }
    }
}

fn attributes(element: &BytesStart<'_>, decoder: quick_xml::encoding::Decoder) -> Result<Attrs> {
    let mut values = Attrs::new();
    for attribute in element.attributes().with_checks(true) {
        let attribute = attribute.map_err(|error| format_error(error.to_string()))?;
        let key = std::str::from_utf8(attribute.key.as_ref())
            .map_err(|error| format_error(error.to_string()))?
            .to_owned();
        let value = attribute
            .decode_and_unescape_value(decoder)
            .map_err(|error| format_error(error.to_string()))?
            .into_owned();
        if values.insert(key.clone(), value).is_some() {
            return Err(format_error(format!("duplicate XML attribute `{key}`")));
        }
    }
    Ok(values)
}

fn local_name(name: &[u8]) -> Result<String> {
    let name = std::str::from_utf8(name).map_err(|error| format_error(error.to_string()))?;
    Ok(name
        .rsplit_once(':')
        .map_or(name, |(_, local)| local)
        .to_owned())
}

fn resolved_namespace(resolved: ResolveResult<'_>) -> Result<Option<String>> {
    match resolved {
        ResolveResult::Unbound => Ok(None),
        ResolveResult::Bound(namespace) => std::str::from_utf8(namespace.as_ref())
            .map(str::to_owned)
            .map(Some)
            .map_err(|error| format_error(format!("XML namespace is not valid UTF-8: {error}"))),
        ResolveResult::Unknown(prefix) => Err(format_error(format!(
            "XML namespace prefix `{}` is not declared",
            String::from_utf8_lossy(&prefix)
        ))),
    }
}

fn required_text<'a>(attrs: &'a Attrs, name: &str) -> Result<&'a str> {
    attrs
        .get(name)
        .map(String::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format_error(format!("missing required attribute `{name}`")))
}

fn required_f64(attrs: &Attrs, name: &str) -> Result<f64> {
    optional_f64(attrs, name)?
        .ok_or_else(|| format_error(format!("missing required numeric attribute `{name}`")))
}

fn optional_f64(attrs: &Attrs, name: &str) -> Result<Option<f64>> {
    attrs
        .get(name)
        .map(|value| {
            let parsed = value.parse::<f64>().map_err(|_| {
                format_error(format!("attribute `{name}` is not a number: `{value}`"))
            })?;
            if !parsed.is_finite() {
                return Err(format_error(format!(
                    "attribute `{name}` is not finite: `{value}`"
                )));
            }
            Ok(parsed)
        })
        .transpose()
}

fn validate_active_power_control_target_limits(
    equipment: &str,
    minimum: Option<f64>,
    maximum: Option<f64>,
    equipment_minimum: f64,
    equipment_maximum: f64,
) -> Result<()> {
    for (name, value) in [("minTargetP", minimum), ("maxTargetP", maximum)] {
        if let Some(value) = value {
            if !value.is_finite() {
                return Err(format_error(format!(
                    "activePowerControl `{name}` on `{equipment}` is not finite"
                )));
            }
            if value < equipment_minimum || value > equipment_maximum {
                return Err(format_error(format!(
                    "activePowerControl `{name}` on `{equipment}` is outside equipment minP/maxP [{equipment_minimum}, {equipment_maximum}]"
                )));
            }
        }
    }
    if minimum.zip(maximum).is_some_and(|(min, max)| min > max) {
        return Err(format_error(format!(
            "activePowerControl on `{equipment}` has minTargetP greater than maxTargetP"
        )));
    }
    Ok(())
}

fn required_i32(attrs: &Attrs, name: &str) -> Result<i32> {
    optional_i32(attrs, name)?
        .ok_or_else(|| format_error(format!("missing required integer attribute `{name}`")))
}

fn optional_i32(attrs: &Attrs, name: &str) -> Result<Option<i32>> {
    attrs
        .get(name)
        .map(|value| {
            value.parse::<i32>().map_err(|_| {
                format_error(format!("attribute `{name}` is not an integer: `{value}`"))
            })
        })
        .transpose()
}

fn required_u32(attrs: &Attrs, name: &str) -> Result<u32> {
    let value = required_text(attrs, name)?;
    value.parse::<u32>().map_err(|_| {
        format_error(format!(
            "attribute `{name}` is not a nonnegative integer: `{value}`"
        ))
    })
}

fn optional_u32(attrs: &Attrs, name: &str) -> Result<Option<u32>> {
    attrs
        .get(name)
        .map(|value| {
            value.parse::<u32>().map_err(|_| {
                format_error(format!(
                    "attribute `{name}` is not a nonnegative integer: `{value}`"
                ))
            })
        })
        .transpose()
}

fn optional_bool(attrs: &Attrs, name: &str) -> Result<Option<bool>> {
    attrs
        .get(name)
        .map(|value| match value.as_str() {
            "true" | "1" => Ok(true),
            "false" | "0" => Ok(false),
            _ => Err(format_error(format!(
                "attribute `{name}` is not a boolean: `{value}`"
            ))),
        })
        .transpose()
}

fn required_bool(attrs: &Attrs, name: &str) -> Result<bool> {
    optional_bool(attrs, name)?
        .ok_or_else(|| format_error(format!("missing required `{name}` attribute")))
}

fn parse_tap_regulation_mode(kind: TapKind, value: &str) -> Result<TapChangerRegulationMode> {
    match (kind, value) {
        (TapKind::Ratio, "VOLTAGE") => Ok(TapChangerRegulationMode::Voltage),
        (TapKind::Ratio, "REACTIVE_POWER") => Ok(TapChangerRegulationMode::ReactivePower),
        (TapKind::Phase, "ACTIVE_POWER_CONTROL") => Ok(TapChangerRegulationMode::ActivePower),
        (TapKind::Phase, "CURRENT_LIMITER") => Ok(TapChangerRegulationMode::Current),
        _ => Err(format_error(format!(
            "unknown XIIDM tap changer regulation mode `{value}`"
        ))),
    }
}

fn parse_terminal_reference(attrs: &Attrs) -> Result<RawTerminalReference> {
    let side = attrs
        .get("side")
        .map(|value| parse_terminal_number("side", value))
        .transpose()?;
    let number = attrs
        .get("number")
        .map(|value| parse_terminal_number("number", value))
        .transpose()?;
    if side.is_some() && number.is_some() {
        return Err(format_error(
            "an XIIDM terminal reference cannot specify both `side` and `number`",
        ));
    }
    Ok(RawTerminalReference {
        id: required_text(attrs, "id")?.to_owned(),
        terminal: side.or(number).unwrap_or(1),
    })
}

fn parse_terminal_number(attribute: &str, value: &str) -> Result<u8> {
    match value {
        "ONE" => Ok(1),
        "TWO" => Ok(2),
        "THREE" if attribute == "side" => Ok(3),
        _ => Err(format_error(format!(
            "unknown XIIDM terminal {attribute} `{value}`"
        ))),
    }
}

fn parse_nodes(value: &str) -> Result<Vec<i32>> {
    let nodes = value
        .split(',')
        .map(str::trim)
        .map(|node| {
            node.parse::<i32>()
                .map_err(|_| format_error(format!("invalid node `{node}` in `nodes`")))
        })
        .collect::<Result<Vec<_>>>()?;
    if nodes.is_empty() {
        return Err(format_error("calculated bus has an empty node list"));
    }
    Ok(nodes)
}

fn terminal_text<'a>(attrs: &'a Attrs, name: &str, side: u8) -> Option<&'a str> {
    let sided = format!("{name}{side}");
    if side == 1 && !attrs.contains_key(&sided) {
        attrs.get(name).map(String::as_str)
    } else {
        attrs.get(&sided).map(String::as_str)
    }
}

fn terminal_i32(attrs: &Attrs, name: &str, side: u8) -> Result<Option<i32>> {
    let sided = format!("{name}{side}");
    let key = if side == 1 && !attrs.contains_key(&sided) {
        name
    } else {
        &sided
    };
    optional_i32(attrs, key)
}

fn terminal_f64(attrs: &Attrs, name: &str, side: u8) -> Result<Option<f64>> {
    let sided = format!("{name}{side}");
    let key = if side == 1 && !attrs.contains_key(&sided) {
        name
    } else {
        &sided
    };
    optional_f64(attrs, key)
}

fn component_id(kind: &str, id: &str) -> Result<ComponentId> {
    ComponentId::new(kind, id).map_err(|error| format_error(error.to_string()))
}

fn resolve_terminal_reference(
    parsed: &ParsedXiidm,
    reference: &RawTerminalReference,
) -> Result<TerminalReference> {
    if let Some(equipment) = parsed
        .equipment
        .iter()
        .find(|equipment| equipment.id == reference.id)
    {
        if reference.terminal == 0 || reference.terminal > equipment.kind.terminal_count() {
            return Err(format_error(format!(
                "terminal reference `{}` names terminal {} but the equipment has {} terminal(s)",
                reference.id,
                reference.terminal,
                equipment.kind.terminal_count()
            )));
        }
        return Ok(TerminalReference {
            equipment: component_id(equipment.kind.component_type(), &reference.id)?,
            terminal: reference.terminal,
        });
    }
    if let Some(converter) = parsed
        .voltage_source_converters
        .iter()
        .find(|converter| converter.common.id == reference.id)
    {
        let terminal_count = ac_dc_converter_ac_terminal_count(&converter.common)?;
        if reference.terminal == 0 || reference.terminal > terminal_count {
            return Err(format_error(format!(
                "terminal reference `{}` names terminal {} but the converter has {terminal_count} terminal(s)",
                reference.id, reference.terminal
            )));
        }
        return Ok(TerminalReference {
            equipment: component_id("voltage_source_converter", &reference.id)?,
            terminal: reference.terminal,
        });
    }
    if let Some(converter) = parsed
        .line_commutated_converters
        .iter()
        .find(|converter| converter.common.id == reference.id)
    {
        let terminal_count = ac_dc_converter_ac_terminal_count(&converter.common)?;
        if reference.terminal == 0 || reference.terminal > terminal_count {
            return Err(format_error(format!(
                "terminal reference `{}` names terminal {} but the converter has {terminal_count} terminal(s)",
                reference.id, reference.terminal
            )));
        }
        return Ok(TerminalReference {
            equipment: component_id("line_commutated_converter", &reference.id)?,
            terminal: reference.terminal,
        });
    }
    if parsed
        .busbar_sections
        .iter()
        .any(|busbar| busbar.id == reference.id)
        && reference.terminal == 1
    {
        return Ok(TerminalReference {
            equipment: component_id("busbar_section", &reference.id)?,
            terminal: 1,
        });
    }
    Err(format_error(format!(
        "terminal reference names unsupported or missing equipment `{}`",
        reference.id
    )))
}

fn resolve_regulating_bus(
    parsed: &ParsedXiidm,
    reference: &RawTerminalReference,
    buses: &BusBuilder<'_>,
    voltage_levels: &HashMap<&str, &RawVoltageLevel>,
) -> Result<Option<BusId>> {
    let resolved = resolve_terminal_reference(parsed, reference)?;
    if resolved.equipment.component_type() == "busbar_section" {
        let busbar = parsed
            .busbar_sections
            .iter()
            .find(|busbar| busbar.id == reference.id)
            .expect("resolved busbar section exists");
        return Ok(buses
            .node_map
            .get(&(busbar.voltage_level.clone(), busbar.node))
            .copied());
    }
    let Some(equipment) = parsed.equipment.iter().find(|equipment| {
        equipment.id == reference.id
            && equipment.kind.component_type() == resolved.equipment.component_type()
    }) else {
        return Ok(None);
    };
    let terminals = equipment_terminals(equipment, buses, voltage_levels)?;
    Ok(terminals
        .get(reference.terminal.saturating_sub(1) as usize)
        .map(|(bus, _)| *bus))
}

fn ac_dc_converter_ac_terminal_count(converter: &RawAcDcConverter) -> Result<u8> {
    let second = terminal_text(&converter.attrs, "bus", 2).is_some()
        || terminal_text(&converter.attrs, "connectableBus", 2).is_some()
        || terminal_i32(&converter.attrs, "node", 2)?.is_some();
    Ok(if second { 2 } else { 1 })
}

fn node_component_id(voltage_level: &str, node: i32) -> Result<ComponentId> {
    component_id("connectivity_node", &format!("{voltage_level}/{node}"))
}

fn format_error(message: impl Into<String>) -> Error {
    Error::FormatRead {
        format: FORMAT,
        message: message.into(),
    }
}

fn has_missing_component_ids(network: &BalancedNetwork) -> bool {
    network.buses().iter().any(|value| value.uid.is_none())
        || network.loads().iter().any(|value| value.uid.is_none())
        || network.shunts().iter().any(|value| value.uid.is_none())
        || network.branches().iter().any(|value| value.uid.is_none())
        || network.switches().iter().any(|value| value.uid.is_none())
        || network.generators().iter().any(|value| value.uid.is_none())
        || network.storage().iter().any(|value| value.uid.is_none())
        || network.hvdc().iter().any(|value| value.uid.is_none())
        || network
            .transformers_3w()
            .iter()
            .any(|value| value.uid.is_none())
}

fn push_derived_terminal(
    detailed: &mut DetailedConnectivity,
    bus_connections: &HashMap<BusId, (ComponentId, ComponentId)>,
    equipment: ComponentId,
    terminal: u8,
    bus: BusId,
    connected: bool,
) {
    let (voltage_level, configured_bus) = bus_connections.get(&bus).expect("checked bus reference");
    detailed.terminals.push(Terminal {
        component: None,
        equipment,
        terminal,
        voltage_level: voltage_level.clone(),
        bus: Some(configured_bus.clone()),
        connectable_bus: Some(configured_bus.clone()),
        node: None,
        connected,
        active_power_mw: None,
        reactive_power_mvar: None,
    });
}

fn bus_group_root(parent: &mut [usize], mut index: usize) -> usize {
    let mut root = index;
    while parent[root] != root {
        root = parent[root];
    }
    while parent[index] != index {
        let next = parent[index];
        parent[index] = root;
        index = next;
    }
    root
}

fn join_bus_groups(parent: &mut [usize], first: usize, second: usize) {
    let first = bus_group_root(parent, first);
    let second = bus_group_root(parent, second);
    if first != second {
        let (root, child) = if first < second {
            (first, second)
        } else {
            (second, first)
        };
        parent[child] = root;
    }
}

fn derive_detailed_connectivity(
    network: &BalancedNetwork,
    diagnostics: &mut Diagnostics,
) -> Result<DetailedConnectivity> {
    let substation =
        component_id("substation", "powerio-substation").expect("static component identity");
    let mut detailed = DetailedConnectivity::default();
    detailed.substations.push(Substation {
        component: substation.clone(),
        country: None,
        operator: None,
        geographical_tags: Vec::new(),
    });
    let bus_positions = network
        .buses()
        .iter()
        .enumerate()
        .map(|(index, bus)| (bus.id, index))
        .collect::<HashMap<_, _>>();
    let mut bus_groups = (0..network.buses().len()).collect::<Vec<_>>();
    for switch in network.switches() {
        let from = *bus_positions
            .get(&switch.from)
            .expect("network validation checked the switch bus");
        let to = *bus_positions
            .get(&switch.to)
            .expect("network validation checked the switch bus");
        let from_kv = network.buses()[from].base_kv;
        let to_kv = network.buses()[to].base_kv;
        if from_kv.to_bits() != to_kv.to_bits() {
            return Err(Error::Emit {
                format: FORMAT,
                message: format!(
                    "switch `{}` joins buses {} ({from_kv} kV) and {} ({to_kv} kV); one XIIDM switch cannot join different voltage levels",
                    switch.uid.as_deref().expect("prepared identity"),
                    switch.from,
                    switch.to,
                ),
            });
        }
        join_bus_groups(&mut bus_groups, from, to);
    }
    let mut grouped_buses = BTreeMap::<usize, Vec<&Bus>>::new();
    for (index, bus) in network.buses().iter().enumerate() {
        let root = bus_group_root(&mut bus_groups, index);
        grouped_buses.entry(root).or_default().push(bus);
    }
    let mut bus_connections = HashMap::new();
    for buses in grouped_buses.values() {
        let first_bus = buses.first().expect("a bus group is not empty");
        let voltage_level = component_id("voltage_level", &format!("powerio-vl-{}", first_bus.id))
            .expect("derived voltage level identity");
        let low_voltage_limit_kv = buses
            .iter()
            .map(|bus| bus.vmin * bus.base_kv)
            .reduce(f64::max)
            .expect("a bus group is not empty");
        let high_voltage_limit_kv = buses
            .iter()
            .map(|bus| bus.vmax * bus.base_kv)
            .reduce(f64::min)
            .expect("a bus group is not empty");
        if low_voltage_limit_kv > high_voltage_limit_kv {
            return Err(Error::Emit {
                format: FORMAT,
                message: format!(
                    "switch-connected buses {} have no common voltage limit range for XIIDM voltage level `{}`",
                    buses
                        .iter()
                        .map(|bus| bus.id.to_string())
                        .collect::<Vec<_>>()
                        .join(", "),
                    voltage_level.local_id(),
                ),
            });
        }
        if buses.len() > 1
            && buses.iter().any(|bus| {
                (bus.vmin * bus.base_kv).to_bits() != low_voltage_limit_kv.to_bits()
                    || (bus.vmax * bus.base_kv).to_bits() != high_voltage_limit_kv.to_bits()
            })
        {
            diagnostics.push(
                &codes::EMIT_XIIDM.value_collapsed,
                format!(
                    "switch-connected buses {} have distinct voltage limits; XIIDM voltage level `{}` uses their common range [{low_voltage_limit_kv}, {high_voltage_limit_kv}] kV",
                    buses
                        .iter()
                        .map(|bus| bus.id.to_string())
                        .collect::<Vec<_>>()
                        .join(", "),
                    voltage_level.local_id(),
                ),
            );
        }
        detailed.voltage_levels.push(VoltageLevel {
            component: voltage_level.clone(),
            substation: Some(substation.clone()),
            nominal_kv: first_bus.base_kv,
            low_voltage_limit_kv: Some(low_voltage_limit_kv),
            high_voltage_limit_kv: Some(high_voltage_limit_kv),
            topology_kind: TopologyKind::BusBreaker,
            buses: buses.iter().map(|bus| bus.id).collect(),
        });
        for bus in buses {
            let configured_bus = component_id("bus", &format!("powerio-bus-{}", bus.id))
                .expect("derived bus identity");
            detailed.bus_breaker_buses.push(BusBreakerBus {
                component: configured_bus.clone(),
                voltage_level: voltage_level.clone(),
                calculated_bus: Some(bus.id),
                voltage_kv: Some(bus.vm * bus.base_kv),
                angle_degrees: Some(bus.va),
            });
            bus_connections.insert(bus.id, (voltage_level.clone(), configured_bus));
        }
    }
    for load in network.loads() {
        push_derived_terminal(
            &mut detailed,
            &bus_connections,
            component_id("load", load.uid.as_deref().expect("prepared identity"))
                .expect("valid stored identity"),
            1,
            load.bus,
            load.in_service,
        );
    }
    for generator in network.generators() {
        push_derived_terminal(
            &mut detailed,
            &bus_connections,
            component_id(
                "generator",
                generator.uid.as_deref().expect("prepared identity"),
            )
            .expect("valid stored identity"),
            1,
            generator.bus,
            generator.in_service,
        );
    }
    for storage in network.storage() {
        push_derived_terminal(
            &mut detailed,
            &bus_connections,
            component_id(
                "storage",
                storage.uid.as_deref().expect("prepared identity"),
            )
            .expect("valid stored identity"),
            1,
            storage.bus,
            storage.in_service,
        );
    }
    for shunt in network.shunts() {
        push_derived_terminal(
            &mut detailed,
            &bus_connections,
            component_id("shunt", shunt.uid.as_deref().expect("prepared identity"))
                .expect("valid stored identity"),
            1,
            shunt.bus,
            shunt.in_service,
        );
    }
    for branch in network.branches() {
        let component = component_id("branch", branch.uid.as_deref().expect("prepared identity"))
            .expect("valid stored identity");
        push_derived_terminal(
            &mut detailed,
            &bus_connections,
            component.clone(),
            1,
            branch.from,
            branch.in_service,
        );
        push_derived_terminal(
            &mut detailed,
            &bus_connections,
            component,
            2,
            branch.to,
            branch.in_service,
        );
    }
    if !network.switches().is_empty() {
        diagnostics.push(
            &codes::EMIT_XIIDM.value_defaulted,
            format!(
                "{} balanced switch record(s) carry no physical switch kind; fresh XIIDM emits them as breakers",
                network.switches().len()
            ),
        );
    }
    for switch in network.switches() {
        let component = component_id("switch", switch.uid.as_deref().expect("prepared identity"))
            .expect("valid stored identity");
        let (voltage_level, first_bus) = bus_connections
            .get(&switch.from)
            .expect("network validation checked the switch bus");
        let (second_voltage_level, second_bus) = bus_connections
            .get(&switch.to)
            .expect("network validation checked the switch bus");
        debug_assert_eq!(voltage_level, second_voltage_level);
        detailed.switches.push(TopologySwitch {
            component: component.clone(),
            voltage_level: voltage_level.clone(),
            kind: SwitchKind::Breaker,
            endpoint1: TopologyEndpoint::Bus(first_bus.clone()),
            endpoint2: TopologyEndpoint::Bus(second_bus.clone()),
            open: !switch.closed,
            retained: true,
        });
        let fields = [
            switch.thermal_rating.map(|_| "thermal rating"),
            switch.current_rating.map(|_| "current rating"),
            switch.pf.map(|_| "from-side active power"),
            switch.qf.map(|_| "from-side reactive power"),
            switch.pt.map(|_| "to-side active power"),
            switch.qt.map(|_| "to-side reactive power"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        diagnose_xiidm_dropped_fields(&component, &fields, diagnostics);
    }
    for transformer in network.transformers_3w() {
        let component = component_id(
            "transformer_3w",
            transformer.uid.as_deref().expect("prepared identity"),
        )
        .expect("valid stored identity");
        for (index, winding) in transformer.windings.iter().enumerate() {
            push_derived_terminal(
                &mut detailed,
                &bus_connections,
                component.clone(),
                u8::try_from(index + 1).expect("three windings"),
                winding.bus,
                transformer.in_service,
            );
        }
    }
    for line in network.hvdc() {
        for original_side in 1..=2 {
            let is_from = hvdc_original_side_is_from(line, original_side);
            let bus = if is_from { line.from } else { line.to };
            push_derived_terminal(
                &mut detailed,
                &bus_connections,
                component_id("hvdc_converter", &hvdc_station_id(line, original_side))
                    .expect("valid converter identity"),
                1,
                bus,
                line.in_service,
            );
        }
    }
    Ok(detailed)
}

/// Lookup tables used only while writing XIIDM. The parsed model keeps source
/// order in its vectors; each grouped vector below is filled in that same
/// order so indexing does not change emitted XML ordering.
struct XiidmWriteIndex<'a> {
    /// `true` when the caller supplied detailed terminals. In that case an
    /// absent terminal power value stays absent instead of being filled from
    /// the balanced calculation view.
    supplied_detailed_connectivity: bool,
    omitted_fields: HashSet<(ComponentId, OmittedFieldName)>,
    metadata: HashMap<ComponentId, &'a ComponentMetadata>,
    terminals: HashMap<(ComponentId, u8), &'a Terminal>,
    node_numbers: HashMap<ComponentId, i32>,
    voltage_levels: HashMap<ComponentId, &'a VoltageLevel>,
    buses: HashMap<BusId, &'a Bus>,
    levels_by_substation: HashMap<ComponentId, Vec<&'a VoltageLevel>>,
    bus_breaker_buses_by_level: HashMap<ComponentId, Vec<&'a BusBreakerBus>>,
    bus_breaker_bus_by_calculated_bus: HashMap<(ComponentId, BusId), &'a BusBreakerBus>,
    connectivity_node_by_calculated_bus: HashMap<(ComponentId, BusId), &'a ConnectivityNode>,
    calculated_buses_by_level: HashMap<ComponentId, Vec<&'a CalculatedBus>>,
    busbars_by_level: HashMap<ComponentId, Vec<&'a BusbarSection>>,
    switches_by_level: HashMap<ComponentId, Vec<&'a TopologySwitch>>,
    internal_connections_by_level: HashMap<ComponentId, Vec<&'a InternalConnection>>,
    loads_by_level: HashMap<ComponentId, Vec<&'a Load>>,
    generators_by_level: HashMap<ComponentId, Vec<&'a Generator>>,
    storage_by_level: HashMap<ComponentId, Vec<&'a Storage>>,
    shunts_by_level: HashMap<ComponentId, Vec<&'a Shunt>>,
    svcs_by_level: HashMap<ComponentId, Vec<&'a StaticVarCompensator>>,
    boundaries_by_level: HashMap<ComponentId, Vec<&'a BoundaryLine>>,
    vscs_by_level: HashMap<ComponentId, Vec<&'a VoltageSourceConverter>>,
    lccs_by_level: HashMap<ComponentId, Vec<&'a LineCommutatedConverter>>,
    converter_dc_terminals: HashMap<ComponentId, (&'a DcTerminal, &'a DcTerminal)>,
    hvdc_converters_by_level: HashMap<ComponentId, Vec<(&'a Hvdc, u8)>>,
    transformers_by_substation: HashMap<ComponentId, Vec<&'a Branch>>,
    transformers_3w_by_substation: HashMap<ComponentId, Vec<&'a Transformer3W>>,
    transformer_substations: HashMap<ComponentId, ComponentId>,
    transformer_3w_substations: HashMap<ComponentId, ComponentId>,
    tap_changers: HashMap<(ComponentId, u8), Vec<&'a NetworkTapChanger>>,
    operational_limits: HashMap<ComponentId, Vec<&'a OperationalLimitGroup>>,
    reactive_limits: HashMap<ComponentId, &'a EquipmentReactiveLimits>,
    terminal_solution: HashSet<ComponentId>,
    tie_calculation_branches: HashSet<ComponentId>,
    boundary_calculation_loads: HashSet<ComponentId>,
    boundary_calculation_generators: HashSet<ComponentId>,
}

impl<'a> XiidmWriteIndex<'a> {
    fn new(network: &'a BalancedNetwork, detailed: &'a DetailedConnectivity) -> Result<Self> {
        let metadata = detailed
            .component_metadata
            .iter()
            .map(|value| (value.component.clone(), value))
            .collect();
        let terminals = detailed
            .terminals
            .iter()
            .map(|value| ((value.equipment.clone(), value.terminal), value))
            .collect::<HashMap<_, _>>();
        let mut node_numbers = HashMap::new();
        let mut used_node_numbers: HashMap<ComponentId, HashSet<i32>> = HashMap::new();
        for value in &detailed.connectivity_nodes {
            let Some(number) = value.node_number else {
                continue;
            };
            if !used_node_numbers
                .entry(value.voltage_level.clone())
                .or_default()
                .insert(number)
            {
                return Err(Error::Emit {
                    format: FORMAT,
                    message: format!(
                        "node breaker voltage level `{}` repeats node number {number}",
                        value.voltage_level
                    ),
                });
            }
            node_numbers.insert(value.component.clone(), number);
        }
        let mut next_node_number: HashMap<ComponentId, i32> = HashMap::new();
        for value in &detailed.connectivity_nodes {
            if value.node_number.is_some() {
                continue;
            }
            let used = used_node_numbers
                .entry(value.voltage_level.clone())
                .or_default();
            let next = next_node_number
                .entry(value.voltage_level.clone())
                .or_insert(0);
            while used.contains(next) {
                *next = next.checked_add(1).ok_or_else(|| Error::Emit {
                    format: FORMAT,
                    message: format!(
                        "node breaker voltage level `{}` has no available XIIDM node number",
                        value.voltage_level
                    ),
                })?;
            }
            let number = *next;
            used.insert(number);
            node_numbers.insert(value.component.clone(), number);
            *next = next.checked_add(1).unwrap_or(i32::MAX);
        }
        let voltage_levels = detailed
            .voltage_levels
            .iter()
            .map(|value| (value.component.clone(), value))
            .collect::<HashMap<_, _>>();
        let buses = network
            .buses()
            .iter()
            .map(|value| (value.id, value))
            .collect();
        let fallback_substation =
            component_id("substation", "powerio-substation").expect("static component identity");
        let mut levels_by_substation: HashMap<ComponentId, Vec<&VoltageLevel>> = HashMap::new();
        let mut levels_by_bus: HashMap<BusId, Vec<ComponentId>> = HashMap::new();
        for level in &detailed.voltage_levels {
            levels_by_substation
                .entry(
                    level
                        .substation
                        .clone()
                        .unwrap_or_else(|| fallback_substation.clone()),
                )
                .or_default()
                .push(level);
            for bus in &level.buses {
                levels_by_bus
                    .entry(*bus)
                    .or_default()
                    .push(level.component.clone());
            }
        }

        let mut index = Self {
            supplied_detailed_connectivity: network.detailed_connectivity().is_some(),
            omitted_fields: detailed
                .omitted_fields
                .iter()
                .map(|value| (value.component.clone(), value.field))
                .collect(),
            metadata,
            terminals,
            node_numbers,
            voltage_levels,
            buses,
            levels_by_substation,
            bus_breaker_buses_by_level: HashMap::new(),
            bus_breaker_bus_by_calculated_bus: HashMap::new(),
            connectivity_node_by_calculated_bus: HashMap::new(),
            calculated_buses_by_level: HashMap::new(),
            busbars_by_level: HashMap::new(),
            switches_by_level: HashMap::new(),
            internal_connections_by_level: HashMap::new(),
            loads_by_level: HashMap::new(),
            generators_by_level: HashMap::new(),
            storage_by_level: HashMap::new(),
            shunts_by_level: HashMap::new(),
            svcs_by_level: HashMap::new(),
            boundaries_by_level: HashMap::new(),
            vscs_by_level: HashMap::new(),
            lccs_by_level: HashMap::new(),
            converter_dc_terminals: HashMap::new(),
            hvdc_converters_by_level: HashMap::new(),
            transformers_by_substation: HashMap::new(),
            transformers_3w_by_substation: HashMap::new(),
            transformer_substations: HashMap::new(),
            transformer_3w_substations: HashMap::new(),
            tap_changers: HashMap::new(),
            operational_limits: HashMap::new(),
            reactive_limits: HashMap::new(),
            terminal_solution: HashSet::new(),
            tie_calculation_branches: HashSet::new(),
            boundary_calculation_loads: HashSet::new(),
            boundary_calculation_generators: HashSet::new(),
        };

        for value in &detailed.bus_breaker_buses {
            index
                .bus_breaker_buses_by_level
                .entry(value.voltage_level.clone())
                .or_default()
                .push(value);
            if let Some(bus) = value.calculated_bus {
                index
                    .bus_breaker_bus_by_calculated_bus
                    .entry((value.voltage_level.clone(), bus))
                    .or_insert(value);
            }
        }
        for value in &detailed.connectivity_nodes {
            if let Some(bus) = value.calculated_bus {
                index
                    .connectivity_node_by_calculated_bus
                    .entry((value.voltage_level.clone(), bus))
                    .or_insert(value);
            }
        }
        for value in &detailed.calculated_buses {
            index
                .calculated_buses_by_level
                .entry(value.voltage_level.clone())
                .or_default()
                .push(value);
        }
        for value in &detailed.busbar_sections {
            index
                .busbars_by_level
                .entry(value.voltage_level.clone())
                .or_default()
                .push(value);
        }
        for value in &detailed.switches {
            index
                .switches_by_level
                .entry(value.voltage_level.clone())
                .or_default()
                .push(value);
        }
        for value in &detailed.internal_connections {
            index
                .internal_connections_by_level
                .entry(value.voltage_level.clone())
                .or_default()
                .push(value);
        }
        for value in &detailed.boundary_lines {
            index
                .boundaries_by_level
                .entry(value.voltage_level.clone())
                .or_default()
                .push(value);
            if let Some(component) = &value.calculation_load {
                index.boundary_calculation_loads.insert(component.clone());
            }
            if let Some(component) = &value.calculation_generator {
                index
                    .boundary_calculation_generators
                    .insert(component.clone());
            }
        }
        for value in &detailed.voltage_source_converters {
            index.converter_dc_terminals.insert(
                value.component.clone(),
                (&value.dc_terminal1, &value.dc_terminal2),
            );
            if let Some(terminal) = index.terminal(&value.component, 1) {
                index
                    .vscs_by_level
                    .entry(terminal.voltage_level.clone())
                    .or_default()
                    .push(value);
            }
        }
        for value in &detailed.line_commutated_converters {
            index.converter_dc_terminals.insert(
                value.component.clone(),
                (&value.dc_terminal1, &value.dc_terminal2),
            );
            if let Some(terminal) = index.terminal(&value.component, 1) {
                index
                    .lccs_by_level
                    .entry(terminal.voltage_level.clone())
                    .or_default()
                    .push(value);
            }
        }
        for value in &detailed.tap_changers {
            index
                .tap_changers
                .entry((value.transformer.clone(), value.winding))
                .or_default()
                .push(value);
        }
        for value in &detailed.operational_limit_groups {
            index
                .operational_limits
                .entry(value.equipment.clone())
                .or_default()
                .push(value);
        }
        for value in &detailed.equipment_reactive_limits {
            index
                .reactive_limits
                .entry(value.equipment.clone())
                .or_insert(value);
        }
        for value in &detailed.terminals {
            if value.active_power_mw.is_some() || value.reactive_power_mvar.is_some() {
                index.terminal_solution.insert(value.equipment.clone());
            }
        }
        for value in &detailed.tie_lines {
            if let Some(component) = &value.calculation_branch {
                index.tie_calculation_branches.insert(component.clone());
            }
        }

        macro_rules! group_by_bus {
            ($values:expr, $field:ident) => {
                for value in $values {
                    if let Some(levels) = levels_by_bus.get(&value.bus) {
                        for level in levels {
                            index.$field.entry(level.clone()).or_default().push(value);
                        }
                    }
                }
            };
        }
        group_by_bus!(network.loads(), loads_by_level);
        group_by_bus!(network.generators(), generators_by_level);
        group_by_bus!(network.storage(), storage_by_level);
        group_by_bus!(network.shunts(), shunts_by_level);
        group_by_bus!(network.static_var_compensators(), svcs_by_level);

        for line in network.hvdc() {
            for side in 1..=2 {
                let bus = if hvdc_original_side_is_from(line, side) {
                    line.from
                } else {
                    line.to
                };
                if let Some(levels) = levels_by_bus.get(&bus) {
                    for level in levels {
                        index
                            .hvdc_converters_by_level
                            .entry(level.clone())
                            .or_default()
                            .push((line, side));
                    }
                }
            }
        }

        for branch in network
            .branches()
            .iter()
            .filter(|value| value.is_transformer())
        {
            let Some(id) = branch.uid.as_deref() else {
                continue;
            };
            let component = component_id("branch", id).expect("prepared identity");
            if let Some(substation) = index.common_substation(&component, 2) {
                index
                    .transformer_substations
                    .insert(component, substation.clone());
                index
                    .transformers_by_substation
                    .entry(substation)
                    .or_default()
                    .push(branch);
            }
        }
        for transformer in network.transformers_3w() {
            let Some(id) = transformer.uid.as_deref() else {
                continue;
            };
            let component = component_id("transformer_3w", id).expect("prepared identity");
            if let Some(substation) = index.common_substation(&component, 3) {
                index
                    .transformer_3w_substations
                    .insert(component, substation.clone());
                index
                    .transformers_3w_by_substation
                    .entry(substation)
                    .or_default()
                    .push(transformer);
            }
        }
        Ok(index)
    }

    fn metadata(&self, component: &ComponentId) -> Option<&'a ComponentMetadata> {
        self.metadata.get(component).copied()
    }

    fn is_omitted(&self, component: &ComponentId, field: OmittedFieldName) -> bool {
        self.omitted_fields.contains(&(component.clone(), field))
    }

    fn terminal(&self, component: &ComponentId, side: u8) -> Option<&'a Terminal> {
        self.terminals.get(&(component.clone(), side)).copied()
    }

    fn node_number(&self, component: &ComponentId) -> Option<i32> {
        self.node_numbers.get(component).copied()
    }

    fn bus(&self, id: BusId) -> &'a Bus {
        self.buses.get(&id).copied().expect("checked bus reference")
    }

    fn common_substation(&self, component: &ComponentId, sides: u8) -> Option<ComponentId> {
        let first_level = &self.terminal(component, 1)?.voltage_level;
        let first = self.voltage_levels.get(first_level)?.substation.clone()?;
        (2..=sides)
            .all(|side| {
                self.terminal(component, side)
                    .and_then(|terminal| self.voltage_levels.get(&terminal.voltage_level))
                    .and_then(|level| level.substation.as_ref())
                    == Some(&first)
            })
            .then_some(first)
    }
}

pub(crate) fn write_xiidm(network: &BalancedNetwork) -> Result<TextEmission> {
    let prepared_network = has_missing_component_ids(network).then(|| {
        let mut prepared = network.clone();
        prepared.assign_missing_component_ids();
        prepared
    });
    let network = prepared_network.as_ref().unwrap_or(network);
    network.validate().map_err(|error| Error::Emit {
        format: FORMAT,
        message: format!("network validation failed before XIIDM emission: {error}"),
    })?;
    validate_xiidm_hvdc_emission(network)?;
    let mut diagnostics = Diagnostics::new();
    let metadata = network.case_metadata();
    let case_date = metadata.case_date.as_deref().unwrap_or_else(|| {
        diagnostics.push(
            &codes::EMIT_XIIDM.value_defaulted,
            format!("XIIDM requires `case_date`; emitted `{DEFAULT_CASE_DATE}`"),
        );
        DEFAULT_CASE_DATE
    });
    let forecast_distance = metadata.forecast_distance.unwrap_or_else(|| {
        diagnostics.push(
            &codes::EMIT_XIIDM.value_defaulted,
            "XIIDM requires `forecast_distance`; emitted `0`",
        );
        0
    });
    let source_model_format = metadata.source_model_format.as_deref().unwrap_or_else(|| {
        diagnostics.push(
            &codes::EMIT_XIIDM.value_defaulted,
            format!(
                "XIIDM requires `source_model_format`; emitted `{DEFAULT_SOURCE_MODEL_FORMAT}`"
            ),
        );
        DEFAULT_SOURCE_MODEL_FORMAT
    });
    let declared_validation = metadata
        .minimum_validation_level
        .as_deref()
        .unwrap_or_else(|| {
            diagnostics.push(
                &codes::EMIT_XIIDM.value_defaulted,
                format!(
                    "XIIDM requires `minimum_validation_level`; emitted `{DEFAULT_VALIDATION_LEVEL}`"
                ),
            );
            DEFAULT_VALIDATION_LEVEL
        });
    let detailed = network.detailed_connectivity().as_deref();
    let has_omitted_required_fields = detailed.is_some_and(|value| {
        value
            .omitted_fields
            .iter()
            .any(|omitted| omission_requires_equipment_validation(network, omitted))
    });
    let equipment_mode = declared_validation == "EQUIPMENT"
        || has_omitted_required_fields
        || detailed.is_some_and(|value| {
            value.subnetworks.iter().any(|subnetwork| {
                subnetwork.case_metadata.minimum_validation_level.as_deref() == Some("EQUIPMENT")
            })
        });
    if has_omitted_required_fields && declared_validation != "EQUIPMENT" {
        diagnostics.push(
            &codes::EMIT_XIIDM.value_defaulted,
            "XIIDM fields recorded as omitted require equipment validation; emitted `minimumValidationLevel=\"EQUIPMENT\"`",
        );
    }
    let validation = if equipment_mode {
        "EQUIPMENT"
    } else {
        declared_validation
    };
    let namespace = if equipment_mode {
        EQUIPMENT_NAMESPACE
    } else {
        NAMESPACE
    };
    let active_power_control_namespace = network
        .generators()
        .iter()
        .any(|value| value.active_power_control.is_some())
        || network
            .storage()
            .iter()
            .any(|value| value.active_power_control.is_some());
    let active_power_control_namespace = active_power_control_namespace.then_some(format!(
        " xmlns:apc=\"{ACTIVE_POWER_CONTROL_NAMESPACE_V1_2}\""
    ));
    let mut output = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<iidm:network xmlns:iidm=\"{namespace}\"{} id=\"{}\" caseDate=\"{}\" forecastDistance=\"{forecast_distance}\" sourceFormat=\"{}\" minimumValidationLevel=\"{}\">\n",
        active_power_control_namespace.as_deref().unwrap_or(""),
        xml(network.name()),
        xml(case_date),
        xml(source_model_format),
        xml(validation),
    );
    if let Some(detailed) = network.detailed_connectivity() {
        diagnose_xiidm_projection(detailed, &mut diagnostics)?;
        validate_xiidm_tap_changers(detailed)?;
        validate_xiidm_reactive_limits(detailed)?;
        validate_xiidm_dc_emission(detailed)?;
        let index = XiidmWriteIndex::new(network, detailed)?;
        let network_component = component_id("balanced_network", network.name())?;
        write_identifiable_children_at(index.metadata(&network_component), 2, &mut output);
        let mut contained = HashSet::new();
        for subnetwork in &detailed.subnetworks {
            if subnetwork.parent != network_component {
                return Err(Error::Emit {
                    format: FORMAT,
                    message: format!(
                        "subnetwork `{}` is not contained directly by `{network_component}`",
                        subnetwork.component.local_id()
                    ),
                });
            }
            for component in &subnetwork.components {
                if !contained.insert(component.clone()) {
                    return Err(Error::Emit {
                        format: FORMAT,
                        message: format!(
                            "component `{component}` belongs to more than one XIIDM subnetwork"
                        ),
                    });
                }
            }
            write_subnetwork(
                network,
                detailed,
                &index,
                subnetwork,
                &mut output,
                &mut diagnostics,
            )?;
        }
        let root_components = |component: &ComponentId| !contained.contains(component);
        write_detailed_body(
            network,
            detailed,
            &index,
            &root_components,
            &mut output,
            &mut diagnostics,
        );
        write_active_power_control_extensions(network, &root_components, 2, &mut output)?;
        output.push_str("</iidm:network>\n");
        return Ok(TextEmission::new(output, diagnostics));
    }
    diagnostics.push(
        &codes::EMIT_XIIDM.value_defaulted,
        "the network has no substation or voltage level hierarchy; XIIDM hierarchy was derived from buses",
    );
    let detailed = derive_detailed_connectivity(network, &mut diagnostics)?;
    validate_xiidm_tap_changers(&detailed)?;
    validate_xiidm_reactive_limits(&detailed)?;
    validate_xiidm_dc_emission(&detailed)?;
    let index = XiidmWriteIndex::new(network, &detailed)?;
    write_detailed_body(
        network,
        &detailed,
        &index,
        &|_| true,
        &mut output,
        &mut diagnostics,
    );
    write_active_power_control_extensions(network, &|_| true, 2, &mut output)?;
    output.push_str("</iidm:network>\n");
    Ok(TextEmission::new(output, diagnostics))
}

fn write_subnetwork(
    network: &BalancedNetwork,
    detailed: &DetailedConnectivity,
    index: &XiidmWriteIndex<'_>,
    subnetwork: &Subnetwork,
    output: &mut String,
    diagnostics: &mut Diagnostics,
) -> Result<()> {
    let metadata = index.metadata(&subnetwork.component);
    let case_date = subnetwork
        .case_metadata
        .case_date
        .as_deref()
        .unwrap_or(DEFAULT_CASE_DATE);
    let forecast_distance = subnetwork.case_metadata.forecast_distance.unwrap_or(0);
    let source_model_format = subnetwork
        .case_metadata
        .source_model_format
        .as_deref()
        .unwrap_or(DEFAULT_SOURCE_MODEL_FORMAT);
    let validation = subnetwork
        .case_metadata
        .minimum_validation_level
        .as_deref()
        .unwrap_or(DEFAULT_VALIDATION_LEVEL);
    output.push_str(&format!(
        "  <iidm:network id=\"{}\"{} caseDate=\"{}\" forecastDistance=\"{forecast_distance}\" sourceFormat=\"{}\" minimumValidationLevel=\"{}\">\n",
        xml(subnetwork.component.local_id()),
        identifiable_attributes(metadata),
        xml(case_date),
        xml(source_model_format),
        xml(validation),
    ));
    write_identifiable_children_at(metadata, 4, output);
    let members = subnetwork.components.iter().collect::<HashSet<_>>();
    write_detailed_body(
        network,
        detailed,
        index,
        &|component| members.contains(component),
        output,
        diagnostics,
    );
    write_active_power_control_extensions(
        network,
        &|component| members.contains(component),
        4,
        output,
    )?;
    output.push_str("  </iidm:network>\n");
    Ok(())
}

fn write_active_power_control_extensions(
    network: &BalancedNetwork,
    included: &dyn Fn(&ComponentId) -> bool,
    indent: usize,
    output: &mut String,
) -> Result<()> {
    let parent_indent = " ".repeat(indent);
    let child_indent = " ".repeat(indent + 2);
    for generator in network.generators() {
        let Some(control) = &generator.active_power_control else {
            continue;
        };
        let id = generator.uid.as_deref().unwrap_or("generator");
        let component = component_id("generator", id).expect("prepared identity");
        if included(&component) {
            validate_active_power_control_for_emission(
                id,
                control,
                generator.pmin,
                generator.pmax,
            )?;
            write_active_power_control_extension(
                id,
                control,
                &parent_indent,
                &child_indent,
                output,
            );
        }
    }
    for storage in network.storage() {
        let Some(control) = &storage.active_power_control else {
            continue;
        };
        let id = storage.uid.as_deref().unwrap_or("battery");
        let component = component_id("storage", id).expect("prepared identity");
        if included(&component) {
            validate_active_power_control_for_emission(
                id,
                control,
                -storage.charge_rating,
                storage.discharge_rating,
            )?;
            write_active_power_control_extension(
                id,
                control,
                &parent_indent,
                &child_indent,
                output,
            );
        }
    }
    Ok(())
}

fn validate_active_power_control_for_emission(
    equipment: &str,
    control: &ActivePowerControl,
    equipment_minimum: f64,
    equipment_maximum: f64,
) -> Result<()> {
    for (name, value) in [
        ("droop", control.droop_percent),
        ("participationFactor", control.participation_factor),
        ("minTargetP", control.minimum_target_active_power_mw),
        ("maxTargetP", control.maximum_target_active_power_mw),
    ] {
        if value.is_some_and(|value| !value.is_finite()) {
            return Err(Error::Emit {
                format: FORMAT,
                message: format!(
                    "cannot emit activePowerControl on `{equipment}`: `{name}` is not finite"
                ),
            });
        }
    }
    if control
        .participation_factor
        .is_some_and(|value| value < 0.0)
    {
        return Err(Error::Emit {
            format: FORMAT,
            message: format!(
                "cannot emit activePowerControl on `{equipment}`: participationFactor is negative"
            ),
        });
    }
    for (name, value) in [
        ("minTargetP", control.minimum_target_active_power_mw),
        ("maxTargetP", control.maximum_target_active_power_mw),
    ] {
        if value.is_some_and(|value| value < equipment_minimum || value > equipment_maximum) {
            return Err(Error::Emit {
                format: FORMAT,
                message: format!(
                    "cannot emit activePowerControl on `{equipment}`: `{name}` is outside equipment minP/maxP [{equipment_minimum}, {equipment_maximum}]"
                ),
            });
        }
    }
    if control
        .minimum_target_active_power_mw
        .zip(control.maximum_target_active_power_mw)
        .is_some_and(|(minimum, maximum)| minimum > maximum)
    {
        return Err(Error::Emit {
            format: FORMAT,
            message: format!(
                "cannot emit activePowerControl on `{equipment}`: minTargetP is greater than maxTargetP"
            ),
        });
    }
    Ok(())
}

fn write_active_power_control_extension(
    equipment: &str,
    control: &ActivePowerControl,
    parent_indent: &str,
    child_indent: &str,
    output: &mut String,
) {
    output.push_str(&format!(
        "{parent_indent}<iidm:extension id=\"{}\">\n{child_indent}<apc:activePowerControl participate=\"{}\"{}{}{}{}/>\n{parent_indent}</iidm:extension>\n",
        xml(equipment),
        control.participate,
        optional_number_attribute("droop", control.droop_percent),
        optional_number_attribute("participationFactor", control.participation_factor),
        optional_number_attribute("maxTargetP", control.maximum_target_active_power_mw),
        optional_number_attribute("minTargetP", control.minimum_target_active_power_mw),
    ));
}

fn xiidm_hvdc_emission_error(line: &Hvdc, field: &str) -> Error {
    Error::Emit {
        format: FORMAT,
        message: format!(
            "cannot emit HVDC line `{}`: XIIDM requires `{field}`",
            line.uid.as_deref().unwrap_or("<unnamed>")
        ),
    }
}

fn validate_xiidm_hvdc_emission(network: &BalancedNetwork) -> Result<()> {
    for line in network.hvdc() {
        if line.resistance_ohm.is_none() {
            return Err(xiidm_hvdc_emission_error(line, "r"));
        }
        if line.nominal_voltage_kv.is_none() {
            return Err(xiidm_hvdc_emission_error(line, "nominalV"));
        }
        if line.converters_mode.is_none() {
            return Err(xiidm_hvdc_emission_error(line, "convertersMode"));
        }
        for (side, converter) in [(1, line.converter1.as_ref()), (2, line.converter2.as_ref())] {
            let Some(converter) = converter else {
                return Err(xiidm_hvdc_emission_error(
                    line,
                    &format!("converterStation{side}"),
                ));
            };
            match converter.kind {
                HvdcConverterKind::Vsc if converter.voltage_regulator_on.is_none() => {
                    return Err(xiidm_hvdc_emission_error(
                        line,
                        &format!("converterStation{side}.voltageRegulatorOn"),
                    ));
                }
                HvdcConverterKind::Lcc if converter.power_factor.is_none() => {
                    return Err(xiidm_hvdc_emission_error(
                        line,
                        &format!("converterStation{side}.powerFactor"),
                    ));
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn xiidm_emission_error(component: &ComponentId, field: &str) -> Error {
    Error::Emit {
        format: FORMAT,
        message: format!("cannot emit `{component}`: XIIDM requires `{field}`"),
    }
}

fn ordered_xiidm_tap_steps(tap: &NetworkTapChanger) -> Result<Vec<&NetworkTapChangerStep>> {
    let mut steps = tap.steps.iter().collect::<Vec<_>>();
    steps.sort_unstable_by_key(|step| step.position);
    for (offset, step) in steps.iter().enumerate() {
        let offset = i32::try_from(offset).map_err(|_| Error::Emit {
            format: FORMAT,
            message: format!(
                "cannot emit transformer `{}` tap changer on winding {}: too many tap steps",
                tap.transformer.local_id(),
                tap.winding
            ),
        })?;
        let expected = tap
            .low_tap_position
            .checked_add(offset)
            .ok_or_else(|| Error::Emit {
                format: FORMAT,
                message: format!(
                    "cannot emit transformer `{}` tap changer on winding {}: tap step position overflows i32",
                    tap.transformer.local_id(),
                    tap.winding
                ),
            })?;
        if step.position != expected {
            return Err(Error::Emit {
                format: FORMAT,
                message: format!(
                    "cannot emit transformer `{}` tap changer on winding {}: XIIDM assigns consecutive step positions from lowTapPosition {}, but found position {} where {} was required",
                    tap.transformer.local_id(),
                    tap.winding,
                    tap.low_tap_position,
                    step.position,
                    expected
                ),
            });
        }
    }
    for (field, position) in [
        ("tapPosition", tap.tap_position),
        ("solvedTapPosition", tap.solved_tap_position),
    ] {
        if let Some(position) = position
            && !steps.iter().any(|step| step.position == position)
        {
            return Err(Error::Emit {
                format: FORMAT,
                message: format!(
                    "cannot emit transformer `{}` tap changer on winding {}: {field} {position} has no matching step",
                    tap.transformer.local_id(),
                    tap.winding
                ),
            });
        }
    }
    Ok(steps)
}

fn validate_xiidm_tap_changers(detailed: &DetailedConnectivity) -> Result<()> {
    for tap in &detailed.tap_changers {
        if !tap.steps.is_empty() {
            ordered_xiidm_tap_steps(tap)?;
        }
    }
    Ok(())
}

fn validate_xiidm_reactive_limits(detailed: &DetailedConnectivity) -> Result<()> {
    let validate = |component: &ComponentId, limits: &ReactiveLimits| -> Result<()> {
        if matches!(
            limits,
            ReactiveLimits::CapabilityCurve(ReactiveCapabilityCurve {
                curve_style: CurveStyle::ConstantYValue,
                ..
            })
        ) {
            return Err(Error::Emit {
                format: FORMAT,
                message: format!(
                    "cannot emit `{component}`: XIIDM reactiveCapabilityCurve uses CurveStyle.straightLineYValues, not CurveStyle.constantYValue"
                ),
            });
        }
        Ok(())
    };
    for record in &detailed.equipment_reactive_limits {
        validate(&record.equipment, &record.limits)?;
    }
    for boundary in &detailed.boundary_lines {
        if let Some(limits) = boundary
            .generation
            .as_ref()
            .and_then(|generation| generation.reactive_limits.as_ref())
        {
            validate(&boundary.component, limits)?;
        }
    }
    for converter in &detailed.voltage_source_converters {
        if let Some(limits) = converter.reactive_limits.as_ref() {
            validate(&converter.component, limits)?;
        }
    }
    Ok(())
}

fn diagnose_xiidm_dropped_fields(
    component: &ComponentId,
    fields: &[&str],
    diagnostics: &mut Diagnostics,
) {
    if fields.is_empty() {
        return;
    }
    diagnostics.push(
        &codes::EMIT_XIIDM.field_dropped,
        format!(
            "`{component}` has no XIIDM 1.17 representation for: {}",
            fields.join(", ")
        ),
    );
}

fn diagnose_xiidm_dc_terminal(
    equipment: &ComponentId,
    side: &str,
    terminal: &DcTerminal,
    diagnostics: &mut Diagnostics,
) {
    let fields = [
        terminal.component.as_ref().map(|_| "terminal identity"),
        terminal.sequence_number.map(|_| "terminal sequence number"),
        terminal
            .dc_topological_node
            .as_ref()
            .map(|_| "DC topological node"),
        terminal.polarity.map(|_| "terminal polarity"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    if !fields.is_empty() {
        diagnostics.push(
            &codes::EMIT_XIIDM.field_dropped,
            format!(
                "`{equipment}` {side} has no XIIDM 1.17 representation for: {}",
                fields.join(", ")
            ),
        );
    }
}

#[allow(clippy::too_many_lines)]
fn diagnose_xiidm_projection(
    detailed: &DetailedConnectivity,
    diagnostics: &mut Diagnostics,
) -> Result<()> {
    let metadata_by_component = detailed
        .component_metadata
        .iter()
        .map(|metadata| (&metadata.component, metadata))
        .collect::<HashMap<_, _>>();
    for terminal in &detailed.terminals {
        let Some(component) = terminal.component.as_ref() else {
            continue;
        };
        let has_metadata = metadata_by_component
            .get(component)
            .is_some_and(|metadata| {
                metadata.name.is_some()
                    || metadata.equipment_container.is_some()
                    || !metadata.aliases.is_empty()
                    || !metadata.external_identifiers.is_empty()
                    || !metadata.properties.is_empty()
                    || metadata.fictitious
            });
        diagnostics.push(
            &codes::EMIT_XIIDM.field_dropped,
            format!(
                "`{}` terminal {} identity `{component}`{} has no XIIDM 1.17 representation",
                terminal.equipment,
                terminal.terminal,
                if has_metadata {
                    " and its attached metadata"
                } else {
                    ""
                },
            ),
        );
    }
    for node in &detailed.connectivity_nodes {
        let xiidm_identity = node
            .node_number
            .and_then(|number| node_component_id(node.voltage_level.local_id(), number).ok());
        let metadata = metadata_by_component.get(&node.component);
        let has_unrepresentable_metadata = metadata.is_some_and(|metadata| {
            metadata.name.is_some()
                || metadata.equipment_container.is_some()
                || !metadata.aliases.is_empty()
                || !metadata.external_identifiers.is_empty()
                || !metadata.properties.is_empty()
                || metadata.fictitious
        });
        if xiidm_identity.as_ref() != Some(&node.component) || has_unrepresentable_metadata {
            diagnostics.push(
                &codes::EMIT_XIIDM.field_dropped,
                format!(
                    "connectivity node `{}`{} is emitted only as an XIIDM local node number; its source identity{} cannot be retained",
                    node.component,
                    if node.node_number.is_none() {
                        " has no source node number and receives an allocated number"
                    } else {
                        ""
                    },
                    if has_unrepresentable_metadata {
                        " and attached metadata"
                    } else {
                        ""
                    },
                ),
            );
        }
    }
    for tap in &detailed.tap_changers {
        let mut fields = Vec::new();
        if tap.component.is_some() {
            fields.push("tap changer identity");
        }
        if tap.neutral_tap_position != Some(tap.low_tap_position) {
            fields.push("neutral tap position distinct from low tap position");
        }
        if tap.normal_tap_position != tap.tap_position {
            fields.push("normal tap position distinct from assigned tap position");
        }
        if tap.voltage_step_increment_percent.is_some() {
            fields.push("voltage step increment");
        }
        diagnose_xiidm_dropped_fields(&tap.transformer, &fields, diagnostics);
    }
    for metadata in &detailed.component_metadata {
        if !metadata.external_identifiers.is_empty() {
            diagnostics.push(
                &codes::EMIT_XIIDM.value_collapsed,
                format!(
                    "`{}` external identifiers are emitted as XIIDM aliases; XIIDM does not preserve the distinction between an alias and an external identifier",
                    metadata.component
                ),
            );
        }
        if let Some(container) = &metadata.equipment_container {
            diagnostics.push(
                &codes::EMIT_XIIDM.field_dropped,
                format!(
                    "`{}` explicit equipment container `{container}` is expressed through XIIDM nesting where possible; the source metadata relationship is not retained",
                    metadata.component
                ),
            );
        }
    }
    for (kind, components) in [
        (
            "DC busbar",
            detailed
                .dc_busbars
                .iter()
                .map(|value| &value.component)
                .collect::<Vec<_>>(),
        ),
        (
            "DC series device",
            detailed
                .dc_series_devices
                .iter()
                .map(|value| &value.component)
                .collect::<Vec<_>>(),
        ),
        (
            "junction",
            detailed
                .junctions
                .iter()
                .map(|value| &value.component)
                .collect::<Vec<_>>(),
        ),
    ] {
        if !components.is_empty() {
            return Err(Error::Emit {
                format: FORMAT,
                message: format!(
                    "cannot emit {kind} records as XIIDM 1.17 without changing connectivity: {}",
                    components
                        .iter()
                        .map(std::string::ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            });
        }
    }
    for (kind, components) in [
        (
            "DC converter unit",
            detailed
                .dc_converter_units
                .iter()
                .map(|value| &value.component)
                .collect::<Vec<_>>(),
        ),
        (
            "DC topological node",
            detailed
                .dc_topological_nodes
                .iter()
                .map(|value| &value.component)
                .collect::<Vec<_>>(),
        ),
    ] {
        if !components.is_empty() {
            diagnostics.push(
                &codes::EMIT_XIIDM.field_dropped,
                format!(
                    "XIIDM 1.17 has no {kind} record; omitted {}",
                    components
                        .iter()
                        .map(std::string::ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            );
        }
    }
    for node in &detailed.dc_nodes {
        diagnose_xiidm_dropped_fields(
            &node.component,
            &[
                node.dc_converter_unit
                    .as_ref()
                    .map(|_| "DC converter unit")
                    .into_iter(),
                node.dc_topological_node
                    .as_ref()
                    .map(|_| "DC topological node")
                    .into_iter(),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>(),
            diagnostics,
        );
    }
    for ground in &detailed.dc_grounds {
        let fields = [
            ground
                .equipment_container
                .as_ref()
                .map(|_| "equipment container"),
            ground.rated_dc_voltage_kv.map(|_| "rated DC voltage"),
            ground.inductance_h.map(|_| "inductance"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        diagnose_xiidm_dropped_fields(&ground.component, &fields, diagnostics);
        diagnose_xiidm_dc_terminal(
            &ground.component,
            "DC terminal",
            &ground.dc_terminal,
            diagnostics,
        );
    }
    for line in &detailed.dc_lines {
        let fields = [
            line.equipment_container
                .as_ref()
                .map(|_| "equipment container"),
            line.rated_dc_voltage_kv.map(|_| "rated DC voltage"),
            line.inductance_h.map(|_| "inductance"),
            line.capacitance_f.map(|_| "capacitance"),
            line.length_km.map(|_| "length"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        diagnose_xiidm_dropped_fields(&line.component, &fields, diagnostics);
        diagnose_xiidm_dc_terminal(
            &line.component,
            "first DC terminal",
            &line.dc_terminal1,
            diagnostics,
        );
        diagnose_xiidm_dc_terminal(
            &line.component,
            "second DC terminal",
            &line.dc_terminal2,
            diagnostics,
        );
    }
    for switch in &detailed.dc_switches {
        let fields = [
            switch
                .equipment_container
                .as_ref()
                .map(|_| "equipment container"),
            switch.rated_dc_voltage_kv.map(|_| "rated DC voltage"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        diagnose_xiidm_dropped_fields(&switch.component, &fields, diagnostics);
        diagnose_xiidm_dc_terminal(
            &switch.component,
            "first DC terminal",
            &switch.dc_terminal1,
            diagnostics,
        );
        diagnose_xiidm_dc_terminal(
            &switch.component,
            "second DC terminal",
            &switch.dc_terminal2,
            diagnostics,
        );
    }
    for converter in &detailed.voltage_source_converters {
        let fields = [
            converter
                .dc_converter_unit
                .as_ref()
                .map(|_| "DC converter unit"),
            converter
                .base_apparent_power_mva
                .map(|_| "base apparent power"),
            converter
                .minimum_dc_voltage_kv
                .map(|_| "minimum DC voltage"),
            converter
                .maximum_dc_voltage_kv
                .map(|_| "maximum DC voltage"),
            converter.rated_dc_voltage_kv.map(|_| "rated DC voltage"),
            converter.valve_u0_kv.map(|_| "valve threshold voltage"),
            converter.number_of_valves.map(|_| "number of valves"),
            converter.active_power_at_pcc_mw.map(|_| "PCC active power"),
            converter
                .reactive_power_at_pcc_mvar
                .map(|_| "PCC reactive power"),
            converter.droop.map(|_| "scalar droop"),
            converter.droop_compensation.map(|_| "droop compensation"),
            converter.q_share.map(|_| "reactive power sharing factor"),
            converter
                .maximum_modulation_index
                .map(|_| "maximum modulation index"),
            converter
                .maximum_valve_current_a
                .map(|_| "maximum valve current"),
            converter
                .pole_loss_active_power_mw
                .map(|_| "pole loss active power"),
            converter.dc_current_a.map(|_| "solved DC current"),
            converter.ac_voltage_kv.map(|_| "solved AC voltage"),
            converter.dc_voltage_kv.map(|_| "solved DC voltage"),
            converter.delta_degrees.map(|_| "solved converter angle"),
            converter.uf_kv.map(|_| "solved filter voltage"),
            converter.uv_kv.map(|_| "solved valve voltage"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        diagnose_xiidm_dropped_fields(&converter.component, &fields, diagnostics);
        diagnose_xiidm_dc_terminal(
            &converter.component,
            "first DC terminal",
            &converter.dc_terminal1,
            diagnostics,
        );
        diagnose_xiidm_dc_terminal(
            &converter.component,
            "second DC terminal",
            &converter.dc_terminal2,
            diagnostics,
        );
    }
    for converter in &detailed.line_commutated_converters {
        let fields = [
            converter
                .dc_converter_unit
                .as_ref()
                .map(|_| "DC converter unit"),
            converter
                .base_apparent_power_mva
                .map(|_| "base apparent power"),
            converter
                .minimum_dc_voltage_kv
                .map(|_| "minimum DC voltage"),
            converter
                .maximum_dc_voltage_kv
                .map(|_| "maximum DC voltage"),
            converter.rated_dc_voltage_kv.map(|_| "rated DC voltage"),
            converter.valve_u0_kv.map(|_| "valve threshold voltage"),
            converter.number_of_valves.map(|_| "number of valves"),
            converter.active_power_at_pcc_mw.map(|_| "PCC active power"),
            converter
                .reactive_power_at_pcc_mvar
                .map(|_| "PCC reactive power"),
            converter.operating_mode.map(|_| "operating mode"),
            converter.rated_dc_current_a.map(|_| "rated DC current"),
            converter.minimum_alpha_degrees.map(|_| "minimum alpha"),
            converter.maximum_alpha_degrees.map(|_| "maximum alpha"),
            converter.minimum_gamma_degrees.map(|_| "minimum gamma"),
            converter.maximum_gamma_degrees.map(|_| "maximum gamma"),
            converter.target_alpha_degrees.map(|_| "target alpha"),
            converter.target_gamma_degrees.map(|_| "target gamma"),
            converter.target_dc_current_a.map(|_| "target DC current"),
            converter
                .pole_loss_active_power_mw
                .map(|_| "pole loss active power"),
            converter.dc_current_a.map(|_| "solved DC current"),
            converter.ac_voltage_kv.map(|_| "solved AC voltage"),
            converter.dc_voltage_kv.map(|_| "solved DC voltage"),
            converter.alpha_degrees.map(|_| "solved alpha"),
            converter.gamma_degrees.map(|_| "solved gamma"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        diagnose_xiidm_dropped_fields(&converter.component, &fields, diagnostics);
        diagnose_xiidm_dc_terminal(
            &converter.component,
            "first DC terminal",
            &converter.dc_terminal1,
            diagnostics,
        );
        diagnose_xiidm_dc_terminal(
            &converter.component,
            "second DC terminal",
            &converter.dc_terminal2,
            diagnostics,
        );
    }
    Ok(())
}

fn validate_xiidm_dc_terminal(
    equipment: &ComponentId,
    terminal: &DcTerminal,
    connected_required: bool,
) -> Result<()> {
    if terminal.dc_node.is_none() {
        return Err(xiidm_emission_error(equipment, "dcNode"));
    }
    if connected_required && terminal.connected.is_none() {
        return Err(xiidm_emission_error(equipment, "connected"));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_xiidm_ac_dc_converter(
    component: &ComponentId,
    dc_terminal1: &DcTerminal,
    dc_terminal2: &DcTerminal,
    idle_loss_mw: Option<f64>,
    switching_loss_mw_per_ampere: Option<f64>,
    resistive_loss_ohm: Option<f64>,
    control_mode: Option<AcDcConverterControlMode>,
    pcc_terminal: Option<&TerminalReference>,
) -> Result<()> {
    validate_xiidm_dc_terminal(component, dc_terminal1, true)?;
    validate_xiidm_dc_terminal(component, dc_terminal2, true)?;
    for (name, present) in [
        ("idleLoss", idle_loss_mw.is_some()),
        ("switchingLoss", switching_loss_mw_per_ampere.is_some()),
        ("resistiveLoss", resistive_loss_ohm.is_some()),
        ("controlMode", control_mode.is_some()),
        ("pccTerminal", pcc_terminal.is_some()),
    ] {
        if !present {
            return Err(xiidm_emission_error(component, name));
        }
    }
    match control_mode {
        Some(AcDcConverterControlMode::DcCurrent) => {
            return Err(Error::Emit {
                format: FORMAT,
                message: format!(
                    "cannot emit `{component}`: XIIDM has no DC current converter control mode"
                ),
            });
        }
        Some(
            AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroop
            | AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroopWithCompensation
            | AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroopPilot,
        ) => {
            return Err(Error::Emit {
                format: FORMAT,
                message: format!(
                    "cannot emit `{component}`: CGMES scalar droop control is not XIIDM P_PCC_DROOP"
                ),
            });
        }
        _ => {}
    }
    Ok(())
}

fn validate_xiidm_dc_emission(detailed: &DetailedConnectivity) -> Result<()> {
    for node in &detailed.dc_nodes {
        if node.nominal_voltage_kv.is_none() {
            return Err(xiidm_emission_error(&node.component, "nominalV"));
        }
    }
    for ground in &detailed.dc_grounds {
        validate_xiidm_dc_terminal(&ground.component, &ground.dc_terminal, true)?;
        if ground.resistance_ohm.is_none() {
            return Err(xiidm_emission_error(&ground.component, "r"));
        }
    }
    for line in &detailed.dc_lines {
        validate_xiidm_dc_terminal(&line.component, &line.dc_terminal1, true)?;
        validate_xiidm_dc_terminal(&line.component, &line.dc_terminal2, true)?;
        if line.resistance_ohm.is_none() {
            return Err(xiidm_emission_error(&line.component, "r"));
        }
    }
    for switch in &detailed.dc_switches {
        validate_xiidm_dc_terminal(&switch.component, &switch.dc_terminal1, false)?;
        validate_xiidm_dc_terminal(&switch.component, &switch.dc_terminal2, false)?;
        if switch.kind == DcSwitchKind::Switch {
            return Err(Error::Emit {
                format: FORMAT,
                message: format!(
                    "cannot emit `{}`: XIIDM supports only breaker and disconnector DC switch kinds",
                    switch.component
                ),
            });
        }
        if switch.open.is_none() {
            return Err(xiidm_emission_error(&switch.component, "open"));
        }
        if switch.resistance_ohm.is_none() {
            return Err(xiidm_emission_error(&switch.component, "r"));
        }
    }
    for converter in &detailed.voltage_source_converters {
        validate_xiidm_ac_dc_converter(
            &converter.component,
            &converter.dc_terminal1,
            &converter.dc_terminal2,
            converter.idle_loss_mw,
            converter.switching_loss_mw_per_ampere,
            converter.resistive_loss_ohm,
            converter.control_mode,
            converter.pcc_terminal.as_ref(),
        )?;
        if converter.voltage_regulator_on.is_none() {
            return Err(xiidm_emission_error(
                &converter.component,
                "voltageRegulatorOn",
            ));
        }
        if converter.reactive_limits.is_none() {
            return Err(xiidm_emission_error(
                &converter.component,
                "reactive limits",
            ));
        }
        if matches!(
            converter.reactive_limits.as_ref(),
            Some(ReactiveLimits::CapabilityCurve(ReactiveCapabilityCurve {
                curve_style: CurveStyle::ConstantYValue,
                ..
            }))
        ) {
            return Err(Error::Emit {
                format: FORMAT,
                message: format!(
                    "cannot emit `{}`: XIIDM reactiveCapabilityCurve uses CurveStyle.straightLineYValues, not CurveStyle.constantYValue",
                    converter.component
                ),
            });
        }
    }
    for converter in &detailed.line_commutated_converters {
        validate_xiidm_ac_dc_converter(
            &converter.component,
            &converter.dc_terminal1,
            &converter.dc_terminal2,
            converter.idle_loss_mw,
            converter.switching_loss_mw_per_ampere,
            converter.resistive_loss_ohm,
            converter.control_mode,
            converter.pcc_terminal.as_ref(),
        )?;
        if converter.reactive_model.is_none() {
            return Err(xiidm_emission_error(&converter.component, "reactiveModel"));
        }
        if converter.power_factor.is_none() {
            return Err(xiidm_emission_error(&converter.component, "powerFactor"));
        }
    }
    Ok(())
}

fn write_dc_equipment(
    detailed: &DetailedConnectivity,
    index: &XiidmWriteIndex<'_>,
    included: &dyn Fn(&ComponentId) -> bool,
    output: &mut String,
) {
    for node in detailed
        .dc_nodes
        .iter()
        .filter(|node| included(&node.component))
    {
        let metadata = index.metadata(&node.component);
        let children = has_identifiable_children(metadata);
        output.push_str(&format!(
            "  <iidm:dcNode id=\"{}\"{} nominalV=\"{}\"{}{}>\n",
            xml(node.component.local_id()),
            identifiable_attributes(metadata),
            number(node.nominal_voltage_kv.expect("validated XIIDM nominalV")),
            optional_number_attribute("v", node.voltage_kv),
            if children { "" } else { "/" },
        ));
        if children {
            write_identifiable_children(metadata, output);
            output.push_str("  </iidm:dcNode>\n");
        }
    }
    for switch in detailed
        .dc_switches
        .iter()
        .filter(|switch| included(&switch.component))
    {
        let metadata = index.metadata(&switch.component);
        let children = has_identifiable_children(metadata);
        output.push_str(&format!(
            "  <iidm:dcSwitch id=\"{}\"{} dcNode1=\"{}\" dcNode2=\"{}\" kind=\"{}\" open=\"{}\" r=\"{}\"{}>\n",
            xml(switch.component.local_id()),
            identifiable_attributes(metadata),
            xml(switch
                .dc_terminal1
                .dc_node
                .as_ref()
                .expect("validated XIIDM dcNode1")
                .local_id()),
            xml(switch
                .dc_terminal2
                .dc_node
                .as_ref()
                .expect("validated XIIDM dcNode2")
                .local_id()),
            match switch.kind {
                DcSwitchKind::Switch => unreachable!("validated XIIDM switch kind"),
                DcSwitchKind::Breaker => "BREAKER",
                DcSwitchKind::Disconnector => "DISCONNECTOR",
            },
            switch.open.expect("validated XIIDM open"),
            number(switch.resistance_ohm.expect("validated XIIDM r")),
            if children { "" } else { "/" },
        ));
        if children {
            write_identifiable_children(metadata, output);
            output.push_str("  </iidm:dcSwitch>\n");
        }
    }
    for ground in detailed
        .dc_grounds
        .iter()
        .filter(|ground| included(&ground.component))
    {
        let metadata = index.metadata(&ground.component);
        let children = has_identifiable_children(metadata);
        output.push_str(&format!(
            "  <iidm:dcGround id=\"{}\"{} dcNode=\"{}\" r=\"{}\" connected=\"{}\"{}{}{}>\n",
            xml(ground.component.local_id()),
            identifiable_attributes(metadata),
            xml(ground
                .dc_terminal
                .dc_node
                .as_ref()
                .expect("validated XIIDM dcNode")
                .local_id()),
            number(ground.resistance_ohm.expect("validated XIIDM r")),
            ground
                .dc_terminal
                .connected
                .expect("validated XIIDM connected"),
            optional_number_attribute("dcP", ground.dc_terminal.active_power_mw),
            optional_number_attribute("dcI", ground.dc_terminal.current_a),
            if children { "" } else { "/" },
        ));
        if children {
            write_identifiable_children(metadata, output);
            output.push_str("  </iidm:dcGround>\n");
        }
    }
    for line in detailed
        .dc_lines
        .iter()
        .filter(|line| included(&line.component))
    {
        let metadata = index.metadata(&line.component);
        let children = has_identifiable_children(metadata);
        output.push_str(&format!(
            "  <iidm:dcLine id=\"{}\"{} dcNode1=\"{}\" dcNode2=\"{}\" r=\"{}\" connected1=\"{}\" connected2=\"{}\"{}{}{}{}{}>\n",
            xml(line.component.local_id()),
            identifiable_attributes(metadata),
            xml(line
                .dc_terminal1
                .dc_node
                .as_ref()
                .expect("validated XIIDM dcNode1")
                .local_id()),
            xml(line
                .dc_terminal2
                .dc_node
                .as_ref()
                .expect("validated XIIDM dcNode2")
                .local_id()),
            number(line.resistance_ohm.expect("validated XIIDM r")),
            line.dc_terminal1
                .connected
                .expect("validated XIIDM connected1"),
            line.dc_terminal2
                .connected
                .expect("validated XIIDM connected2"),
            optional_number_attribute("dcP1", line.dc_terminal1.active_power_mw),
            optional_number_attribute("dcI1", line.dc_terminal1.current_a),
            optional_number_attribute("dcP2", line.dc_terminal2.active_power_mw),
            optional_number_attribute("dcI2", line.dc_terminal2.current_a),
            if children { "" } else { "/" },
        ));
        if children {
            write_identifiable_children(metadata, output);
            output.push_str("  </iidm:dcLine>\n");
        }
    }
}

fn write_ac_dc_converters(
    index: &XiidmWriteIndex<'_>,
    level: &VoltageLevel,
    included: &dyn Fn(&ComponentId) -> bool,
    output: &mut String,
    diagnostics: &mut Diagnostics,
) {
    for converter in index
        .vscs_by_level
        .get(&level.component)
        .into_iter()
        .flatten()
        .copied()
        .filter(|converter| included(&converter.component))
    {
        let metadata = index.metadata(&converter.component);
        diagnose_converter_active_power_limits(
            &converter.component,
            converter.minimum_active_power_mw,
            converter.maximum_active_power_mw,
            diagnostics,
        );
        output.push_str(&format!(
            "      <iidm:voltageSourceConverter id=\"{}\"{} dcNode1=\"{}\" dcConnected1=\"{}\" dcNode2=\"{}\" dcConnected2=\"{}\" idleLoss=\"{}\" switchingLoss=\"{}\" resistiveLoss=\"{}\" controlMode=\"{}\"{}{}{} voltageRegulatorOn=\"{}\"{}{}>\n",
            xml(converter.component.local_id()),
            identifiable_attributes(metadata),
            xml(converter
                .dc_terminal1
                .dc_node
                .as_ref()
                .expect("validated XIIDM dcNode1")
                .local_id()),
            converter.dc_terminal1
                .connected
                .expect("validated XIIDM dcConnected1"),
            xml(converter
                .dc_terminal2
                .dc_node
                .as_ref()
                .expect("validated XIIDM dcNode2")
                .local_id()),
            converter.dc_terminal2
                .connected
                .expect("validated XIIDM dcConnected2"),
            number(converter.idle_loss_mw.expect("validated XIIDM idleLoss")),
            number(converter
                .switching_loss_mw_per_ampere
                .expect("validated XIIDM switchingLoss")),
            number(converter
                .resistive_loss_ohm
                .expect("validated XIIDM resistiveLoss")),
            ac_dc_control_mode_name(converter
                .control_mode
                .expect("validated XIIDM controlMode")),
            optional_number_attribute("targetP", converter.target_active_power_mw),
            optional_number_attribute("targetVdc", converter.target_dc_voltage_kv),
            ac_dc_terminal_attributes(index, &converter.component),
            converter
                .voltage_regulator_on
                .expect("validated XIIDM voltageRegulatorOn"),
            optional_number_attribute("voltageSetpoint", converter.voltage_setpoint_kv),
            optional_number_attribute(
                "reactivePowerSetpoint",
                converter.reactive_power_setpoint_mvar,
            ),
        ));
        write_identifiable_children(metadata, output);
        write_pcc_terminal(
            &converter.component,
            converter
                .pcc_terminal
                .as_ref()
                .expect("validated XIIDM pccTerminal"),
            output,
        );
        write_droop_curve(converter.droop_curve.as_ref(), output);
        write_reactive_limits(
            converter
                .reactive_limits
                .as_ref()
                .expect("validated XIIDM reactive limits"),
            output,
        );
        output.push_str("      </iidm:voltageSourceConverter>\n");
    }
    for converter in index
        .lccs_by_level
        .get(&level.component)
        .into_iter()
        .flatten()
        .copied()
        .filter(|converter| included(&converter.component))
    {
        let metadata = index.metadata(&converter.component);
        diagnose_converter_active_power_limits(
            &converter.component,
            converter.minimum_active_power_mw,
            converter.maximum_active_power_mw,
            diagnostics,
        );
        output.push_str(&format!(
            "      <iidm:lineCommutatedConverter id=\"{}\"{} dcNode1=\"{}\" dcConnected1=\"{}\" dcNode2=\"{}\" dcConnected2=\"{}\" idleLoss=\"{}\" switchingLoss=\"{}\" resistiveLoss=\"{}\" controlMode=\"{}\"{}{}{} reactiveModel=\"{}\" powerFactor=\"{}\">\n",
            xml(converter.component.local_id()),
            identifiable_attributes(metadata),
            xml(converter
                .dc_terminal1
                .dc_node
                .as_ref()
                .expect("validated XIIDM dcNode1")
                .local_id()),
            converter.dc_terminal1
                .connected
                .expect("validated XIIDM dcConnected1"),
            xml(converter
                .dc_terminal2
                .dc_node
                .as_ref()
                .expect("validated XIIDM dcNode2")
                .local_id()),
            converter.dc_terminal2
                .connected
                .expect("validated XIIDM dcConnected2"),
            number(converter.idle_loss_mw.expect("validated XIIDM idleLoss")),
            number(converter
                .switching_loss_mw_per_ampere
                .expect("validated XIIDM switchingLoss")),
            number(converter
                .resistive_loss_ohm
                .expect("validated XIIDM resistiveLoss")),
            ac_dc_control_mode_name(converter
                .control_mode
                .expect("validated XIIDM controlMode")),
            optional_number_attribute("targetP", converter.target_active_power_mw),
            optional_number_attribute("targetVdc", converter.target_dc_voltage_kv),
            ac_dc_terminal_attributes(index, &converter.component),
            match converter
                .reactive_model
                .expect("validated XIIDM reactiveModel")
            {
                LineCommutatedConverterReactiveModel::FixedPowerFactor => "FIXED_POWER_FACTOR",
                LineCommutatedConverterReactiveModel::CalculatedPowerFactor => {
                    "CALCULATED_POWER_FACTOR"
                }
            },
            number(converter.power_factor.expect("validated XIIDM powerFactor")),
        ));
        write_identifiable_children(metadata, output);
        write_pcc_terminal(
            &converter.component,
            converter
                .pcc_terminal
                .as_ref()
                .expect("validated XIIDM pccTerminal"),
            output,
        );
        write_droop_curve(converter.droop_curve.as_ref(), output);
        output.push_str("      </iidm:lineCommutatedConverter>\n");
    }
}

fn diagnose_converter_active_power_limits(
    component: &ComponentId,
    minimum_active_power_mw: Option<f64>,
    maximum_active_power_mw: Option<f64>,
    diagnostics: &mut Diagnostics,
) {
    let fields = [
        minimum_active_power_mw.map(|_| "minimum active power"),
        maximum_active_power_mw.map(|_| "maximum active power"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    if !fields.is_empty() {
        diagnostics.push(
            &codes::EMIT_XIIDM.field_dropped,
            format!(
                "converter `{}` {} {} no XIIDM 1.17 field",
                component.local_id(),
                fields.join(" and "),
                if fields.len() == 1 {
                    "limit has"
                } else {
                    "limits have"
                },
            ),
        );
    }
}

fn ac_dc_control_mode_name(mode: AcDcConverterControlMode) -> &'static str {
    match mode {
        AcDcConverterControlMode::ActivePowerAtPcc => "P_PCC",
        AcDcConverterControlMode::DcVoltage => "V_DC",
        AcDcConverterControlMode::DcCurrent => unreachable!("validated XIIDM control mode"),
        AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroopCurve => "P_PCC_DROOP",
        AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroop
        | AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroopWithCompensation
        | AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroopPilot => {
            unreachable!("CGMES converter control mode cannot be emitted as XIIDM")
        }
    }
}

fn ac_dc_terminal_attributes(index: &XiidmWriteIndex<'_>, component: &ComponentId) -> String {
    let mut attributes = String::new();
    for side in 1..=2 {
        let Some(terminal) = index.terminal(component, side) else {
            continue;
        };
        if let Some(node) = terminal
            .node
            .as_ref()
            .and_then(|node| index.node_number(node))
        {
            attributes.push_str(&format!(" node{side}=\"{node}\""));
        } else {
            if terminal.connected
                && let Some(bus) = terminal.bus.as_ref()
            {
                attributes.push_str(&format!(" bus{side}=\"{}\"", xml(bus.local_id())));
            }
            if let Some(bus) = terminal.connectable_bus.as_ref().or(terminal.bus.as_ref()) {
                attributes.push_str(&format!(
                    " connectableBus{side}=\"{}\"",
                    xml(bus.local_id())
                ));
            }
        }
        attributes.push_str(&optional_number_attribute(
            &format!("p{side}"),
            terminal.active_power_mw,
        ));
        attributes.push_str(&optional_number_attribute(
            &format!("q{side}"),
            terminal.reactive_power_mvar,
        ));
    }
    if let Some((first, second)) = index.converter_dc_terminals.get(component).copied() {
        attributes.push_str(&optional_number_attribute("dcP1", first.active_power_mw));
        attributes.push_str(&optional_number_attribute("dcI1", first.current_a));
        attributes.push_str(&optional_number_attribute("dcP2", second.active_power_mw));
        attributes.push_str(&optional_number_attribute("dcI2", second.current_a));
    }
    attributes
}

fn write_pcc_terminal(converter: &ComponentId, reference: &TerminalReference, output: &mut String) {
    let (attribute, terminal) = if reference.equipment == *converter {
        (
            "number",
            match reference.terminal {
                2 => "TWO",
                _ => "ONE",
            },
        )
    } else {
        (
            "side",
            match reference.terminal {
                2 => "TWO",
                3 => "THREE",
                _ => "ONE",
            },
        )
    };
    output.push_str(&format!(
        "        <iidm:pccTerminal id=\"{}\" {attribute}=\"{terminal}\"/>\n",
        xml(reference.equipment.local_id())
    ));
}

fn write_droop_curve(curve: Option<&DroopCurve>, output: &mut String) {
    let Some(curve) = curve else {
        return;
    };
    output.push_str("        <iidm:droopCurve>\n");
    for segment in &curve.segments {
        output.push_str(&format!(
            "          <iidm:segment minV=\"{}\" maxV=\"{}\" k=\"{}\"/>\n",
            number(segment.minimum_voltage_kv),
            number(segment.maximum_voltage_kv),
            number(segment.k),
        ));
    }
    output.push_str("        </iidm:droopCurve>\n");
}

fn write_reactive_limits(limits: &ReactiveLimits, output: &mut String) {
    match limits {
        ReactiveLimits::MinMax(limits) => {
            if limits.properties.is_empty() {
                output.push_str(&format!(
                    "        <iidm:minMaxReactiveLimits minQ=\"{}\" maxQ=\"{}\"/>\n",
                    number(limits.minimum_reactive_power_mvar),
                    number(limits.maximum_reactive_power_mvar),
                ));
            } else {
                output.push_str(&format!(
                    "        <iidm:minMaxReactiveLimits minQ=\"{}\" maxQ=\"{}\">\n",
                    number(limits.minimum_reactive_power_mvar),
                    number(limits.maximum_reactive_power_mvar),
                ));
                write_properties(&limits.properties, 10, output);
                output.push_str("        </iidm:minMaxReactiveLimits>\n");
            }
        }
        ReactiveLimits::CapabilityCurve(curve) => {
            output.push_str("        <iidm:reactiveCapabilityCurve>\n");
            write_properties(&curve.properties, 10, output);
            for point in &curve.points {
                if point.properties.is_empty() {
                    output.push_str(&format!(
                        "          <iidm:point p=\"{}\" minQ=\"{}\" maxQ=\"{}\"/>\n",
                        number(point.active_power_mw),
                        number(point.minimum_reactive_power_mvar),
                        number(point.maximum_reactive_power_mvar),
                    ));
                } else {
                    output.push_str(&format!(
                        "          <iidm:point p=\"{}\" minQ=\"{}\" maxQ=\"{}\">\n",
                        number(point.active_power_mw),
                        number(point.minimum_reactive_power_mvar),
                        number(point.maximum_reactive_power_mvar),
                    ));
                    write_properties(&point.properties, 12, output);
                    output.push_str("          </iidm:point>\n");
                }
            }
            output.push_str("        </iidm:reactiveCapabilityCurve>\n");
        }
    }
}

fn write_properties(
    properties: &BTreeMap<String, String>,
    indentation: usize,
    output: &mut String,
) {
    let indentation = " ".repeat(indentation);
    for (name, value) in properties {
        output.push_str(&format!(
            "{indentation}<iidm:property name=\"{}\" value=\"{}\"/>\n",
            xml(name),
            xml(value),
        ));
    }
}

fn write_detailed_body(
    network: &BalancedNetwork,
    detailed: &DetailedConnectivity,
    index: &XiidmWriteIndex<'_>,
    included: &dyn Fn(&ComponentId) -> bool,
    output: &mut String,
    diagnostics: &mut Diagnostics,
) {
    write_dc_equipment(detailed, index, included, output);
    let mut substations = detailed
        .substations
        .iter()
        .filter(|substation| included(&substation.component))
        .cloned()
        .collect::<Vec<_>>();
    let fallback_substation =
        component_id("substation", "powerio-substation").expect("static component identity");
    if detailed
        .voltage_levels
        .iter()
        .any(|level| included(&level.component) && level.substation.is_none())
        && !substations
            .iter()
            .any(|value| value.component == fallback_substation)
    {
        substations.push(Substation {
            component: fallback_substation.clone(),
            country: None,
            operator: None,
            geographical_tags: Vec::new(),
        });
        diagnostics.push(
            &codes::EMIT_XIIDM.value_defaulted,
            "voltage levels without a substation were placed in `powerio-substation`",
        );
    }
    for substation in &substations {
        let metadata = index.metadata(&substation.component);
        output.push_str(&format!(
            "  <iidm:substation id=\"{}\"{}{}{}{}>\n",
            xml(substation.component.local_id()),
            identifiable_attributes(metadata),
            optional_text_attribute("country", substation.country.as_deref()),
            optional_text_attribute("tso", substation.operator.as_deref()),
            optional_text_attribute(
                "geographicalTags",
                (!substation.geographical_tags.is_empty())
                    .then(|| substation.geographical_tags.join(","))
                    .as_deref(),
            ),
        ));
        write_identifiable_children(metadata, output);
        for level in index
            .levels_by_substation
            .get(&substation.component)
            .into_iter()
            .flatten()
            .filter(|level| included(&level.component))
        {
            write_detailed_voltage_level(index, level, included, output, diagnostics);
        }
        for branch in index
            .transformers_by_substation
            .get(&substation.component)
            .into_iter()
            .flatten()
            .filter(|branch| component_is_included(included, "branch", branch.uid.as_deref()))
        {
            write_transformer(index, branch, output, diagnostics);
        }
        for transformer in index
            .transformers_3w_by_substation
            .get(&substation.component)
            .into_iter()
            .flatten()
            .filter(|transformer| {
                component_is_included(included, "transformer_3w", transformer.uid.as_deref())
            })
        {
            write_three_winding_transformer(index, transformer, output, diagnostics);
        }
        output.push_str("  </iidm:substation>\n");
    }
    for branch in network.branches() {
        if is_tie_calculation_branch(index, branch)
            || !component_is_included(included, "branch", branch.uid.as_deref())
        {
            continue;
        }
        let component = branch
            .uid
            .as_deref()
            .and_then(|id| component_id("branch", id).ok());
        if !branch.is_transformer()
            || component
                .as_ref()
                .is_none_or(|component| !index.transformer_substations.contains_key(component))
        {
            if branch.is_transformer() {
                diagnostics.push(
                    &codes::EMIT_XIIDM.element_relabeled,
                    format!(
                        "transformer `{}` is not contained by one substation and was emitted as a line",
                        branch.uid.as_deref().unwrap_or("branch")
                    ),
                );
            }
            write_line(index, branch, output, diagnostics);
        }
    }
    write_tie_lines(detailed, index, included, output);
    for transformer in network.transformers_3w() {
        if !component_is_included(included, "transformer_3w", transformer.uid.as_deref()) {
            continue;
        }
        let component = transformer
            .uid
            .as_deref()
            .and_then(|id| component_id("transformer_3w", id).ok());
        if component
            .as_ref()
            .is_none_or(|component| !index.transformer_3w_substations.contains_key(component))
        {
            diagnostics.push(
                &codes::EMIT_XIIDM.record_dropped,
                format!(
                    "three winding transformer `{}` is not contained by one substation",
                    transformer.uid.as_deref().unwrap_or("transformer")
                ),
            );
        }
    }
    write_hvdc_lines(network, index, included, output, diagnostics);
    write_areas(network, detailed, index, included, output, diagnostics);
}

fn component_is_included(
    included: &dyn Fn(&ComponentId) -> bool,
    component_type: &str,
    local_id: Option<&str>,
) -> bool {
    local_id
        .and_then(|local_id| component_id(component_type, local_id).ok())
        .is_some_and(|component| included(&component))
}

fn is_tie_calculation_branch(index: &XiidmWriteIndex<'_>, branch: &Branch) -> bool {
    let Some(local_id) = branch.uid.as_deref() else {
        return false;
    };
    component_id("branch", local_id)
        .is_ok_and(|component| index.tie_calculation_branches.contains(&component))
}

fn write_tie_lines(
    detailed: &DetailedConnectivity,
    index: &XiidmWriteIndex<'_>,
    included: &dyn Fn(&ComponentId) -> bool,
    output: &mut String,
) {
    for tie in detailed
        .tie_lines
        .iter()
        .filter(|tie| included(&tie.component))
    {
        let metadata = index.metadata(&tie.component);
        let children = has_identifiable_children(metadata);
        output.push_str(&format!(
            "  <iidm:tieLine id=\"{}\"{} boundaryLineId1=\"{}\" boundaryLineId2=\"{}\"{}>\n",
            xml(tie.component.local_id()),
            identifiable_attributes(metadata),
            xml(tie.boundary_line1.local_id()),
            xml(tie.boundary_line2.local_id()),
            if children { "" } else { "/" },
        ));
        if children {
            write_identifiable_children(metadata, output);
            output.push_str("  </iidm:tieLine>\n");
        }
    }
}

fn write_areas(
    network: &BalancedNetwork,
    detailed: &DetailedConnectivity,
    index: &XiidmWriteIndex<'_>,
    included: &dyn Fn(&ComponentId) -> bool,
    output: &mut String,
    diagnostics: &mut Diagnostics,
) {
    for area in network.areas().iter().filter(|area| {
        let fallback_id = format!("A{}", area.number);
        component_is_included(
            included,
            "area",
            Some(area.uid.as_deref().unwrap_or(&fallback_id)),
        )
    }) {
        let fallback_id = format!("A{}", area.number);
        let id = area.uid.as_deref().unwrap_or(&fallback_id);
        let component = component_id("area", id).expect("valid stored area identity");
        let metadata = index.metadata(&component);
        let name = area
            .name
            .as_deref()
            .or_else(|| metadata.and_then(|value| value.name.as_deref()));
        let fictitious = metadata.is_some_and(|value| value.fictitious);
        output.push_str(&format!(
            "  <iidm:area id=\"{}\"{} areaType=\"{}\" interchangeTarget=\"{}\"{}>\n",
            xml(id),
            optional_text_attribute("name", name),
            xml(area.area_type.as_deref().unwrap_or("ControlArea")),
            number(area.net_interchange),
            if fictitious {
                " fictitious=\"true\""
            } else {
                ""
            },
        ));
        write_identifiable_children(metadata, output);
        for level in &detailed.voltage_levels {
            if level.buses.iter().any(|bus_id| {
                index
                    .buses
                    .get(bus_id)
                    .is_some_and(|bus| bus.area == area.number)
            }) {
                output.push_str(&format!(
                    "    <iidm:voltageLevelRef id=\"{}\"/>\n",
                    xml(level.component.local_id())
                ));
            }
        }
        output.push_str("  </iidm:area>\n");
        if area.slack_bus.is_some() {
            diagnostics.push(
                &codes::EMIT_XIIDM.field_dropped,
                format!("area `{id}` swing bus has no XIIDM area field"),
            );
        }
        if area.tolerance != 0.0 {
            diagnostics.push(
                &codes::EMIT_XIIDM.field_dropped,
                format!("area `{id}` interchange tolerance has no XIIDM area field"),
            );
        }
    }
}

fn write_detailed_voltage_level(
    index: &XiidmWriteIndex<'_>,
    level: &VoltageLevel,
    included: &dyn Fn(&ComponentId) -> bool,
    output: &mut String,
    diagnostics: &mut Diagnostics,
) {
    let metadata = index.metadata(&level.component);
    output.push_str(&format!(
        "    <iidm:voltageLevel id=\"{}\"{} nominalV=\"{}\" topologyKind=\"{}\"{}{}>\n",
        xml(level.component.local_id()),
        identifiable_attributes(metadata),
        number(level.nominal_kv),
        match level.topology_kind {
            TopologyKind::BusBreaker => "BUS_BREAKER",
            TopologyKind::NodeBreaker => "NODE_BREAKER",
        },
        optional_number_attribute("lowVoltageLimit", level.low_voltage_limit_kv),
        optional_number_attribute("highVoltageLimit", level.high_voltage_limit_kv),
    ));
    write_identifiable_children(metadata, output);
    match level.topology_kind {
        TopologyKind::BusBreaker => {
            output.push_str("      <iidm:busBreakerTopology>\n");
            for configured in index
                .bus_breaker_buses_by_level
                .get(&level.component)
                .into_iter()
                .flatten()
                .copied()
                .filter(|bus| included(&bus.component))
            {
                let metadata = index.metadata(&configured.component);
                let values = format!(
                    "{}{}",
                    optional_number_attribute("v", configured.voltage_kv),
                    optional_number_attribute("angle", configured.angle_degrees),
                );
                let children = has_identifiable_children(metadata);
                output.push_str(&format!(
                    "        <iidm:bus id=\"{}\"{}{}{}>\n",
                    xml(configured.component.local_id()),
                    identifiable_attributes(metadata),
                    values,
                    if children { "" } else { "/" },
                ));
                if children {
                    write_identifiable_children(metadata, output);
                    output.push_str("        </iidm:bus>\n");
                }
            }
            for switch in index
                .switches_by_level
                .get(&level.component)
                .into_iter()
                .flatten()
                .copied()
                .filter(|switch| included(&switch.component))
            {
                if let (TopologyEndpoint::Bus(first), TopologyEndpoint::Bus(second)) =
                    (&switch.endpoint1, &switch.endpoint2)
                {
                    let metadata = index.metadata(&switch.component);
                    let children = has_identifiable_children(metadata);
                    output.push_str(&format!(
                        "        <iidm:switch id=\"{}\"{} kind=\"{}\" open=\"{}\" retained=\"{}\" bus1=\"{}\" bus2=\"{}\"{}>\n",
                        xml(switch.component.local_id()), identifiable_attributes(metadata),
                        switch_kind(switch.kind), switch.open, switch.retained,
                        xml(first.local_id()), xml(second.local_id()),
                        if children { "" } else { "/" },
                    ));
                    if children {
                        write_identifiable_children(metadata, output);
                        output.push_str("        </iidm:switch>\n");
                    }
                }
            }
            output.push_str("      </iidm:busBreakerTopology>\n");
        }
        TopologyKind::NodeBreaker => {
            output.push_str("      <iidm:nodeBreakerTopology>\n");
            for busbar in index
                .busbars_by_level
                .get(&level.component)
                .into_iter()
                .flatten()
                .copied()
                .filter(|busbar| included(&busbar.component))
            {
                if let Some(node) = index.node_number(&busbar.node) {
                    let metadata = index.metadata(&busbar.component);
                    let children = has_identifiable_children(metadata);
                    output.push_str(&format!(
                        "        <iidm:busbarSection id=\"{}\"{} node=\"{node}\"{}>\n",
                        xml(busbar.component.local_id()),
                        identifiable_attributes(metadata),
                        if children { "" } else { "/" },
                    ));
                    if children {
                        write_identifiable_children(metadata, output);
                        output.push_str("        </iidm:busbarSection>\n");
                    }
                }
            }
            for switch in index
                .switches_by_level
                .get(&level.component)
                .into_iter()
                .flatten()
                .copied()
                .filter(|switch| included(&switch.component))
            {
                if let (TopologyEndpoint::Node(first), TopologyEndpoint::Node(second)) =
                    (&switch.endpoint1, &switch.endpoint2)
                    && let (Some(first), Some(second)) =
                        (index.node_number(first), index.node_number(second))
                {
                    let metadata = index.metadata(&switch.component);
                    let children = has_identifiable_children(metadata);
                    output.push_str(&format!(
                        "        <iidm:switch id=\"{}\"{} kind=\"{}\" open=\"{}\" retained=\"{}\" node1=\"{first}\" node2=\"{second}\"{}>\n",
                        xml(switch.component.local_id()), identifiable_attributes(metadata),
                        switch_kind(switch.kind), switch.open, switch.retained,
                        if children { "" } else { "/" },
                    ));
                    if children {
                        write_identifiable_children(metadata, output);
                        output.push_str("        </iidm:switch>\n");
                    }
                }
            }
            for connection in index
                .internal_connections_by_level
                .get(&level.component)
                .into_iter()
                .flatten()
                .copied()
            {
                if let (Some(first), Some(second)) = (
                    index.node_number(&connection.node1),
                    index.node_number(&connection.node2),
                ) {
                    output.push_str(&format!(
                        "        <iidm:internalConnection node1=\"{first}\" node2=\"{second}\"/>\n"
                    ));
                }
            }
            for calculated in index
                .calculated_buses_by_level
                .get(&level.component)
                .into_iter()
                .flatten()
                .copied()
            {
                let mut nodes = calculated
                    .nodes
                    .iter()
                    .filter_map(|node| index.node_number(node))
                    .collect::<Vec<_>>();
                nodes.sort_unstable();
                output.push_str(&format!(
                    "        <iidm:bus nodes=\"{}\"{}{} />\n",
                    nodes
                        .iter()
                        .map(i32::to_string)
                        .collect::<Vec<_>>()
                        .join(","),
                    optional_number_attribute("v", calculated.voltage_kv),
                    optional_number_attribute("angle", calculated.angle_degrees),
                ));
            }
            output.push_str("      </iidm:nodeBreakerTopology>\n");
        }
    }
    for load in index
        .loads_by_level
        .get(&level.component)
        .into_iter()
        .flatten()
        .copied()
        .filter(|value| {
            component_is_included(included, "load", value.uid.as_deref())
                && !is_boundary_calculation_component(
                    &index.boundary_calculation_loads,
                    "load",
                    value.uid.as_deref(),
                )
        })
    {
        write_load(index, load, output);
    }
    for generator in index
        .generators_by_level
        .get(&level.component)
        .into_iter()
        .flatten()
        .copied()
        .filter(|value| {
            component_is_included(included, "generator", value.uid.as_deref())
                && !is_boundary_calculation_component(
                    &index.boundary_calculation_generators,
                    "generator",
                    value.uid.as_deref(),
                )
        })
    {
        write_generator(index, level, generator, output, diagnostics);
    }
    for storage in index
        .storage_by_level
        .get(&level.component)
        .into_iter()
        .flatten()
        .copied()
        .filter(|value| component_is_included(included, "storage", value.uid.as_deref()))
    {
        write_storage(index, storage, output, diagnostics);
    }
    for shunt in index
        .shunts_by_level
        .get(&level.component)
        .into_iter()
        .flatten()
        .copied()
        .filter(|value| component_is_included(included, "shunt", value.uid.as_deref()))
    {
        write_shunt(index, level, shunt, output, diagnostics);
    }
    for svc in index
        .svcs_by_level
        .get(&level.component)
        .into_iter()
        .flatten()
        .copied()
        .filter(|value| {
            component_is_included(included, "static_var_compensator", value.uid.as_deref())
        })
    {
        write_static_var_compensator(index, svc, output);
    }
    write_hvdc_converter_stations(index, level, included, output, diagnostics);
    write_ac_dc_converters(index, level, included, output, diagnostics);
    for boundary in index
        .boundaries_by_level
        .get(&level.component)
        .into_iter()
        .flatten()
        .copied()
        .filter(|boundary| included(&boundary.component))
    {
        write_boundary_line(index, boundary, output, diagnostics);
    }
    output.push_str("    </iidm:voltageLevel>\n");
}

fn is_boundary_calculation_component(
    calculations: &HashSet<ComponentId>,
    component_type: &str,
    local_id: Option<&str>,
) -> bool {
    let Some(local_id) = local_id else {
        return false;
    };
    component_id(component_type, local_id).is_ok_and(|component| calculations.contains(&component))
}

fn assignment_attribute(
    index: &XiidmWriteIndex<'_>,
    component: &ComponentId,
    field: OmittedFieldName,
    name: &str,
    value: f64,
) -> String {
    if index.is_omitted(component, field) {
        String::new()
    } else {
        format!(" {name}=\"{}\"", number(value))
    }
}

fn write_boundary_line(
    index: &XiidmWriteIndex<'_>,
    boundary: &BoundaryLine,
    output: &mut String,
    diagnostics: &mut Diagnostics,
) {
    let metadata = index.metadata(&boundary.component);
    let terminal = terminal_attributes(index, &boundary.component, 1, false, true);
    let selected = selected_boundary_operational_limits_attribute(index, &boundary.component);
    let active_power = assignment_attribute(
        index,
        &boundary.component,
        OmittedFieldName::ActivePower,
        "p0",
        boundary.active_power_setpoint_mw,
    );
    let reactive_power = assignment_attribute(
        index,
        &boundary.component,
        OmittedFieldName::ReactivePower,
        "q0",
        boundary.reactive_power_setpoint_mvar,
    );
    let generation = boundary
        .generation
        .as_ref()
        .map_or_else(String::new, |value| {
            format!(
                " generationVoltageRegulationOn=\"{}\"{}{}{}{}{}",
                value.voltage_regulation_on,
                optional_number_attribute("generationMinP", value.minimum_active_power_mw),
                optional_number_attribute("generationMaxP", value.maximum_active_power_mw),
                optional_number_attribute("generationTargetP", value.target_active_power_mw),
                optional_number_attribute("generationTargetQ", value.target_reactive_power_mvar),
                optional_number_attribute("generationTargetV", value.target_voltage_kv),
            )
        });
    output.push_str(&format!(
        "      <iidm:boundaryLine id=\"{}\"{}{active_power}{reactive_power} r=\"{}\" x=\"{}\" g=\"{}\" b=\"{}\"{}{}{terminal}{selected}>\n",
        xml(boundary.component.local_id()),
        identifiable_attributes(metadata),
        number(boundary.resistance_ohm),
        number(boundary.reactance_ohm),
        number(boundary.conductance_siemens),
        number(boundary.susceptance_siemens),
        optional_text_attribute(
            "pairingKey",
            boundary
                .pairing_key
                .as_deref()
                .filter(|value| !value.is_empty()),
        ),
        generation,
    ));
    write_identifiable_children(metadata, output);
    if let Some(limits) = boundary
        .generation
        .as_ref()
        .and_then(|generation| generation.reactive_limits.as_ref())
    {
        write_reactive_limits(limits, output);
    }
    write_boundary_operational_limit_groups(index, &boundary.component, output, diagnostics);
    output.push_str("      </iidm:boundaryLine>\n");
}

fn write_load(index: &XiidmWriteIndex<'_>, load: &Load, output: &mut String) {
    let id = load.uid.as_deref().unwrap_or("load");
    let component = component_id("load", id).expect("valid stored identity");
    let terminal = terminal_attributes(index, &component, 1, false, load.in_service);
    let metadata = index.metadata(&component);
    let active_power = assignment_attribute(
        index,
        &component,
        OmittedFieldName::ActivePower,
        "p0",
        load.p,
    );
    let reactive_power = assignment_attribute(
        index,
        &component,
        OmittedFieldName::ReactivePower,
        "q0",
        load.q,
    );
    let model = match &load.voltage_model {
        None | Some(LoadVoltageModel::ConstantPower) => None,
        Some(LoadVoltageModel::Zip {
            p_constant_power,
            q_constant_power,
            p_constant_current,
            q_constant_current,
            p_constant_impedance,
            q_constant_impedance,
            ..
        }) => Some(format!(
            "        <iidm:zipModel c0p=\"{}\" c1p=\"{}\" c2p=\"{}\" c0q=\"{}\" c1q=\"{}\" c2q=\"{}\"/>\n",
            number(safe_ratio(*p_constant_power, load.p)),
            number(safe_ratio(*p_constant_current, load.p)),
            number(safe_ratio(*p_constant_impedance, load.p)),
            number(safe_ratio(*q_constant_power, load.q)),
            number(safe_ratio(*q_constant_current, load.q)),
            number(safe_ratio(*q_constant_impedance, load.q)),
        )),
        Some(LoadVoltageModel::Exponential {
            gamma_p, gamma_q, ..
        }) => Some(format!(
            "        <iidm:exponentialModel np=\"{}\" nq=\"{}\"/>\n",
            number(*gamma_p),
            number(*gamma_q),
        )),
    };
    let children = model.is_some() || has_identifiable_children(metadata);
    if children {
        output.push_str(&format!(
            "      <iidm:load id=\"{}\"{} loadType=\"UNDEFINED\"{active_power}{reactive_power}{terminal}>\n",
            xml(id),
            identifiable_attributes(metadata),
        ));
        write_identifiable_children(metadata, output);
        if let Some(model) = model {
            output.push_str(&model);
        }
        output.push_str("      </iidm:load>\n");
    } else {
        output.push_str(&format!(
            "      <iidm:load id=\"{}\"{} loadType=\"UNDEFINED\"{active_power}{reactive_power}{terminal}/>\n",
            xml(id),
            identifiable_attributes(metadata),
        ));
    }
}

fn write_generator(
    index: &XiidmWriteIndex<'_>,
    level: &VoltageLevel,
    generator: &Generator,
    output: &mut String,
    diagnostics: &mut Diagnostics,
) {
    let id = generator.uid.as_deref().unwrap_or("generator");
    let component = component_id("generator", id).expect("valid stored identity");
    let terminal = terminal_attributes(index, &component, 1, false, generator.in_service);
    let metadata = index.metadata(&component);
    let target_base_kv = generator
        .regulated_bus
        .and_then(|bus| index.buses.get(&bus).map(|bus| bus.base_kv))
        .unwrap_or(level.nominal_kv);
    let target_active_power = assignment_attribute(
        index,
        &component,
        OmittedFieldName::ActivePower,
        "targetP",
        generator.pg,
    );
    let target_reactive_power = assignment_attribute(
        index,
        &component,
        OmittedFieldName::ReactivePower,
        "targetQ",
        generator.qg,
    );
    let target_voltage = assignment_attribute(
        index,
        &component,
        OmittedFieldName::VoltageSetpoint,
        "targetV",
        generator.vg * target_base_kv,
    );
    let rated_apparent_power = assignment_attribute(
        index,
        &component,
        OmittedFieldName::RatedApparentPower,
        "ratedS",
        generator.mbase,
    );
    output.push_str(&format!(
        "      <iidm:generator id=\"{}\"{} energySource=\"{}\" minP=\"{}\" maxP=\"{}\" voltageRegulatorOn=\"{}\"{rated_apparent_power}{target_active_power}{target_reactive_power}{target_voltage}{terminal}>\n",
        xml(id), identifiable_attributes(metadata),
        generator_energy_source_text(generator.energy_source), number(generator.pmin),
        number(generator.pmax), generator.voltage_regulation_on,
    ));
    write_identifiable_children(metadata, output);
    if let Some(reference) = &generator.regulating_terminal {
        output.push_str(&write_terminal_reference(
            "regulatingTerminal",
            reference,
            8,
        ));
    } else if generator
        .regulated_bus
        .is_some_and(|bus| bus != generator.bus)
    {
        diagnostics.push(
            &codes::EMIT_XIIDM.field_dropped,
            format!(
                "generator `{id}` names remote regulated bus {} without an exact regulating terminal; XIIDM output uses the generator terminal",
                generator.regulated_bus.expect("checked present")
            ),
        );
    }
    if let Some(record) = index.reactive_limits.get(&component).copied() {
        write_reactive_limits(&record.limits, output);
    } else {
        output.push_str(&format!(
            "        <iidm:minMaxReactiveLimits minQ=\"{}\" maxQ=\"{}\"/>\n",
            number(generator.qmin),
            number(generator.qmax),
        ));
    }
    output.push_str("      </iidm:generator>\n");
    if generator.cost.is_some() {
        diagnostics.push(
            &codes::EMIT_XIIDM.field_dropped,
            format!("generator `{id}` dispatch cost has no XIIDM field"),
        );
    }
}

fn write_storage(
    index: &XiidmWriteIndex<'_>,
    storage: &Storage,
    output: &mut String,
    diagnostics: &mut Diagnostics,
) {
    let id = storage.uid.as_deref().unwrap_or("battery");
    let component = component_id("storage", id).expect("valid stored identity");
    let terminal = terminal_attributes(index, &component, 1, false, storage.in_service);
    let metadata = index.metadata(&component);
    let target_active_power = assignment_attribute(
        index,
        &component,
        OmittedFieldName::ActivePower,
        "targetP",
        storage.ps,
    );
    let target_reactive_power = assignment_attribute(
        index,
        &component,
        OmittedFieldName::ReactivePower,
        "targetQ",
        storage.qs,
    );
    output.push_str(&format!(
        "      <iidm:battery id=\"{}\"{}{target_active_power}{target_reactive_power} minP=\"{}\" maxP=\"{}\"{terminal}>\n",
        xml(id), identifiable_attributes(metadata), number(-storage.charge_rating),
        number(storage.discharge_rating),
    ));
    write_identifiable_children(metadata, output);
    if let Some(record) = index.reactive_limits.get(&component).copied() {
        write_reactive_limits(&record.limits, output);
    } else {
        output.push_str(&format!(
            "        <iidm:minMaxReactiveLimits minQ=\"{}\" maxQ=\"{}\"/>\n",
            number(storage.qmin),
            number(storage.qmax),
        ));
    }
    output.push_str("      </iidm:battery>\n");
    if storage.energy != 0.0 || storage.energy_rating != 0.0 {
        diagnostics.push(
            &codes::EMIT_XIIDM.field_dropped,
            format!("storage `{id}` energy fields have no XIIDM battery field"),
        );
    }
}

fn write_shunt(
    index: &XiidmWriteIndex<'_>,
    level: &VoltageLevel,
    shunt: &Shunt,
    output: &mut String,
    diagnostics: &mut Diagnostics,
) {
    let id = shunt.uid.as_deref().unwrap_or("shunt");
    let component = component_id("shunt", id).expect("valid stored identity");
    let terminal = terminal_attributes(index, &component, 1, false, shunt.in_service);
    let metadata = index.metadata(&component);
    let conductance_omitted =
        index.is_omitted(&component, OmittedFieldName::ShuntConductancePerSection);
    let scale = level.nominal_kv * level.nominal_kv;
    let Some(control) = &shunt.control else {
        let assigned_section_count = shunt
            .section_count
            .or_else(|| metadata.is_none().then_some(1));
        let section_count = assigned_section_count
            .map_or(String::new(), |value| format!(" sectionCount=\"{value}\""));
        output.push_str(&format!(
            "      <iidm:shuntCompensator id=\"{}\"{}{section_count} voltageRegulatorOn=\"false\"{terminal}>\n",
            xml(id), identifiable_attributes(metadata),
        ));
        write_identifiable_children(metadata, output);
        let conductance = if conductance_omitted {
            String::new()
        } else {
            format!(" gPerSection=\"{}\"", number(shunt.g / scale))
        };
        output.push_str(&format!(
            "        <iidm:shuntLinearModel{conductance} bPerSection=\"{}\" maximumSectionCount=\"1\"/>\n      </iidm:shuntCompensator>\n",
            number(shunt.b / scale),
        ));
        return;
    };
    let Some(calculated_section_count) = matching_shunt_section_count(shunt) else {
        output.push_str(&format!(
            "      <iidm:shuntCompensator id=\"{}\"{} sectionCount=\"1\" voltageRegulatorOn=\"false\"{terminal}>\n",
            xml(id), identifiable_attributes(metadata),
        ));
        write_identifiable_children(metadata, output);
        let conductance = if conductance_omitted {
            String::new()
        } else {
            format!(" gPerSection=\"{}\"", number(shunt.g / scale))
        };
        output.push_str(&format!(
            "        <iidm:shuntLinearModel{conductance} bPerSection=\"{}\" maximumSectionCount=\"1\"/>\n      </iidm:shuntCompensator>\n",
            number(shunt.b / scale),
        ));
        diagnostics.push(
            &codes::EMIT_XIIDM.field_dropped,
            format!(
                "shunt `{id}` initial admittance does not match its section blocks; it was emitted as a fixed shunt"
            ),
        );
        return;
    };
    let assigned_section_count = shunt
        .section_count
        .or_else(|| metadata.is_none().then_some(calculated_section_count));
    let section_count =
        assigned_section_count.map_or(String::new(), |value| format!(" sectionCount=\"{value}\""));
    let regulating = control.mode != SwitchedShuntMode::Locked;
    let target = (control.vhigh + control.vlow) * level.nominal_kv / 2.0;
    let deadband = (control.vhigh - control.vlow).abs() * level.nominal_kv;
    let target_attribute = if regulating {
        format!(" targetV=\"{}\"", number(target))
    } else {
        String::new()
    };
    let deadband_attribute = if regulating {
        format!(" targetDeadband=\"{}\"", number(deadband))
    } else {
        String::new()
    };
    output.push_str(&format!(
        "      <iidm:shuntCompensator id=\"{}\"{}{section_count} voltageRegulatorOn=\"{regulating}\"{}{}{terminal}>\n",
        xml(id),
        identifiable_attributes(metadata),
        target_attribute,
        deadband_attribute,
    ));
    write_identifiable_children(metadata, output);
    if shunt
        .section_count
        .is_some_and(|value| value != calculated_section_count)
    {
        diagnostics.push(
            &codes::EMIT_XIIDM.field_dropped,
            format!("shunt `{id}` assigned section count does not match its calculated admittance"),
        );
    }
    if control.blocks.len() == 1 {
        let block = &control.blocks[0];
        let conductance = if conductance_omitted {
            String::new()
        } else {
            format!(" gPerSection=\"{}\"", number(block.g / scale))
        };
        output.push_str(&format!(
            "        <iidm:shuntLinearModel{conductance} bPerSection=\"{}\" maximumSectionCount=\"{}\"/>\n",
            number(block.b / scale),
            block.steps,
        ));
    } else {
        output.push_str("        <iidm:shuntNonLinearModel>\n");
        for block in &control.blocks {
            for _ in 0..block.steps {
                let conductance = if conductance_omitted {
                    String::new()
                } else {
                    format!(" g=\"{}\"", number(block.g / scale))
                };
                output.push_str(&format!(
                    "          <iidm:section{conductance} b=\"{}\"/>\n",
                    number(block.b / scale),
                ));
            }
        }
        output.push_str("        </iidm:shuntNonLinearModel>\n");
    }
    if let Some(reference) = &control.regulating_terminal {
        output.push_str(&write_terminal_reference(
            "regulatingTerminal",
            reference,
            8,
        ));
    } else if control.control_bus.is_some_and(|bus| bus != shunt.bus) {
        diagnostics.push(
            &codes::EMIT_XIIDM.field_dropped,
            format!("shunt `{id}` remote regulated bus has no equipment terminal reference"),
        );
    }
    output.push_str("      </iidm:shuntCompensator>\n");
    if control.mode == SwitchedShuntMode::Continuous {
        diagnostics.push(
            &codes::EMIT_XIIDM.element_relabeled,
            format!("shunt `{id}` continuous control was emitted as XIIDM section control"),
        );
    }
    if (control.rmpct - 100.0).abs() > f64::EPSILON {
        diagnostics.push(
            &codes::EMIT_XIIDM.field_dropped,
            format!("shunt `{id}` reactive range percentage has no XIIDM field"),
        );
    }
}

fn matching_shunt_section_count(shunt: &Shunt) -> Option<u32> {
    let control = shunt.control.as_ref()?;
    let mut g = 0.0;
    let mut b = 0.0;
    let mut count = 0_u32;
    if admittance_matches(g, b, shunt.g, shunt.b) {
        return Some(count);
    }
    for block in &control.blocks {
        let step = if block.g.abs() > block.b.abs() {
            (shunt.g - g) / block.g
        } else if block.b != 0.0 {
            (shunt.b - b) / block.b
        } else {
            0.0
        };
        let steps = step.round();
        if steps >= 0.0 && steps <= f64::from(block.steps) {
            let candidate_g = g + steps * block.g;
            let candidate_b = b + steps * block.b;
            if admittance_matches(candidate_g, candidate_b, shunt.g, shunt.b) {
                return count.checked_add(steps as u32);
            }
        }
        g += f64::from(block.steps) * block.g;
        b += f64::from(block.steps) * block.b;
        count = count.checked_add(block.steps)?;
    }
    admittance_matches(g, b, shunt.g, shunt.b).then_some(count)
}

fn admittance_matches(first_g: f64, first_b: f64, second_g: f64, second_b: f64) -> bool {
    let tolerance_g = 1e-9 * (1.0 + first_g.abs().max(second_g.abs()));
    let tolerance_b = 1e-9 * (1.0 + first_b.abs().max(second_b.abs()));
    (first_g - second_g).abs() <= tolerance_g && (first_b - second_b).abs() <= tolerance_b
}

fn write_static_var_compensator(
    index: &XiidmWriteIndex<'_>,
    svc: &StaticVarCompensator,
    output: &mut String,
) {
    let id = svc.uid.as_deref().unwrap_or("static-var-compensator");
    let component = component_id("static_var_compensator", id).expect("valid stored identity");
    let terminal = terminal_attributes(index, &component, 1, false, svc.in_service);
    let metadata = index.metadata(&component);
    let solution = injection_solution_attributes(index, &component, svc.p, svc.q);
    output.push_str(&format!(
        "      <iidm:staticVarCompensator id=\"{}\"{} bMin=\"{}\" bMax=\"{}\" voltageSetpoint=\"{}\" reactivePowerSetpoint=\"{}\" regulationMode=\"{}\" regulating=\"{}\"{terminal}{solution}>\n",
        xml(id),
        identifiable_attributes(metadata),
        number(svc.b_min_siemens),
        number(svc.b_max_siemens),
        number(svc.voltage_setpoint_kv),
        number(svc.reactive_power_setpoint_mvar),
        match svc.regulation_mode {
            StaticVarCompensatorRegulationMode::Voltage => "VOLTAGE",
            StaticVarCompensatorRegulationMode::ReactivePower => "REACTIVE_POWER",
        },
        svc.regulating,
    ));
    write_identifiable_children(metadata, output);
    if let Some(reference) = &svc.regulating_terminal {
        output.push_str(&write_terminal_reference(
            "regulatingTerminal",
            reference,
            8,
        ));
    }
    output.push_str("      </iidm:staticVarCompensator>\n");
}

fn transformer_tap_changers<'a>(
    index: &'a XiidmWriteIndex<'_>,
    transformer: &ComponentId,
    winding: u8,
) -> Vec<&'a NetworkTapChanger> {
    index
        .tap_changers
        .get(&(transformer.clone(), winding))
        .cloned()
        .unwrap_or_default()
}

fn current_tap_step(tap: &NetworkTapChanger) -> Option<&NetworkTapChangerStep> {
    let position = tap.tap_position?;
    tap.steps.iter().find(|step| step.position == position)
}

fn tap_step_factor(
    taps: &[&NetworkTapChanger],
    value: impl Fn(&NetworkTapChangerStep) -> f64,
) -> f64 {
    taps.iter()
        .filter_map(|tap| current_tap_step(tap))
        .map(value)
        .product()
}

fn write_tap_changer(
    tap: &NetworkTapChanger,
    three_winding: bool,
    output: &mut String,
    diagnostics: &mut Diagnostics,
) -> bool {
    if tap.steps.is_empty() {
        diagnostics.push(
            &codes::EMIT_XIIDM.record_dropped,
            format!(
                "transformer `{}` tap changer on winding {} has no steps",
                tap.transformer.local_id(),
                tap.winding
            ),
        );
        return false;
    }
    let base = match tap.kind {
        TapChangerKind::Ratio => "ratioTapChanger",
        TapChangerKind::Phase => "phaseTapChanger",
    };
    let element = if three_winding {
        format!("{base}{}", tap.winding)
    } else {
        base.to_owned()
    };
    let regulation_mode = match (tap.kind, tap.regulation_mode) {
        (_, None) => None,
        (TapChangerKind::Ratio, Some(TapChangerRegulationMode::Voltage)) => Some("VOLTAGE"),
        (TapChangerKind::Ratio, Some(TapChangerRegulationMode::ReactivePower)) => {
            Some("REACTIVE_POWER")
        }
        (TapChangerKind::Phase, Some(TapChangerRegulationMode::ActivePower)) => {
            Some("ACTIVE_POWER_CONTROL")
        }
        (TapChangerKind::Phase, Some(TapChangerRegulationMode::Current)) => Some("CURRENT_LIMITER"),
        _ => {
            diagnostics.push(
                &codes::EMIT_XIIDM.field_dropped,
                format!(
                    "transformer `{}` tap changer regulation mode is not representable by XIIDM {}",
                    tap.transformer.local_id(),
                    base
                ),
            );
            None
        }
    };
    let phase_mode = (tap.kind == TapChangerKind::Phase)
        .then_some(regulation_mode)
        .flatten();
    let load_tap_changing = format!(
        " loadTapChangingCapabilities=\"{}\"",
        tap.load_tap_changing_capabilities
    );
    output.push_str(&format!(
        "      <iidm:{element}{}{} lowTapPosition=\"{}\"{} regulating=\"{}\"{}{}{}>\n",
        tap.tap_position
            .map(|position| format!(" tapPosition=\"{position}\""))
            .unwrap_or_default(),
        tap.solved_tap_position
            .map(|position| format!(" solvedTapPosition=\"{position}\""))
            .unwrap_or_default(),
        tap.low_tap_position,
        load_tap_changing,
        tap.regulating,
        optional_text_attribute("regulationMode", phase_mode.or(regulation_mode)),
        optional_number_attribute("regulationValue", tap.regulation_value),
        optional_number_attribute("targetDeadband", tap.target_deadband),
    ));
    if let Some(reference) = &tap.regulation_terminal {
        output.push_str(&write_terminal_reference("terminalRef", reference, 8));
    }
    for step in ordered_xiidm_tap_steps(tap).expect("validated XIIDM tap step positions") {
        let alpha = if tap.kind == TapChangerKind::Phase {
            format!(" alpha=\"{}\"", number(step.alpha_degrees))
        } else {
            String::new()
        };
        output.push_str(&format!(
            "        <iidm:step rho=\"{}\"{alpha} r=\"{}\" x=\"{}\" g=\"{}\" b=\"{}\"/>\n",
            number(step.rho),
            number(step.resistance_deviation_percent),
            number(step.reactance_deviation_percent),
            number(step.conductance_deviation_percent),
            number(step.susceptance_deviation_percent),
        ));
    }
    output.push_str(&format!("      </iidm:{element}>\n"));
    true
}

fn write_three_winding_transformer(
    index: &XiidmWriteIndex<'_>,
    transformer: &Transformer3W,
    output: &mut String,
    diagnostics: &mut Diagnostics,
) {
    let id = transformer.uid.as_deref().unwrap_or("transformer");
    let component = component_id("transformer_3w", id).expect("valid stored identity");
    let buses: [&Bus; 3] = std::array::from_fn(|side| {
        let winding = &transformer.windings[side];
        index.bus(winding.bus)
    });
    let rated_u0 = transformer
        .extras
        .get(XIIDM_RATED_U0_EXTRA)
        .and_then(serde_json::Value::as_f64)
        .filter(|base| *base > 0.0)
        .unwrap_or(if transformer.windings[0].nominal_kv > 0.0 {
            transformer.windings[0].nominal_kv
        } else {
            buses[0].base_kv
        });
    let zbase = rated_u0 * rated_u0 / DEFAULT_BASE_MVA;
    let star = transformer.calc_star_impedances();
    let taps: [Vec<&NetworkTapChanger>; 3] =
        std::array::from_fn(|side| transformer_tap_changers(index, &component, side as u8 + 1));
    let unapply = |value: f64, factor: f64| {
        if factor.abs() <= f64::EPSILON {
            value
        } else {
            value / factor
        }
    };
    let resistance_factor: [f64; 3] = std::array::from_fn(|side| {
        tap_step_factor(&taps[side], |step| {
            1.0 + step.resistance_deviation_percent / 100.0
        })
    });
    let reactance_factor: [f64; 3] = std::array::from_fn(|side| {
        tap_step_factor(&taps[side], |step| {
            1.0 + step.reactance_deviation_percent / 100.0
        })
    });
    let conductance_factor = tap_step_factor(&taps[0], |step| {
        1.0 + step.conductance_deviation_percent / 100.0
    });
    let susceptance_factor = tap_step_factor(&taps[0], |step| {
        1.0 + step.susceptance_deviation_percent / 100.0
    });
    let limits_attributes = selected_operational_limits_attributes(index, &component, 3, |side| {
        let winding = &transformer.windings[usize::from(side - 1)];
        winding.rate_a != 0.0 || winding.rate_b != 0.0 || winding.rate_c != 0.0
    });
    output.push_str(&format!(
        "    <iidm:threeWindingsTransformer id=\"{}\"{} ratedU0=\"{}\" r1=\"{}\" x1=\"{}\" g1=\"{}\" b1=\"{}\" ratedU1=\"{}\" r2=\"{}\" x2=\"{}\" g2=\"0\" b2=\"0\" ratedU2=\"{}\" r3=\"{}\" x3=\"{}\" g3=\"0\" b3=\"0\" ratedU3=\"{}\"{}{}{}{}>\n",
        xml(id),
        optional_text_attribute("name", transformer.name.as_deref()),
        number(rated_u0),
        number(unapply(star[0].0 * zbase, resistance_factor[0])),
        number(unapply(star[0].1 * zbase, reactance_factor[0])),
        number(unapply(transformer.mag_g / zbase, conductance_factor)),
        number(unapply(transformer.mag_b / zbase, susceptance_factor)),
        number(winding_nominal_kv(&transformer.windings[0], buses[0])),
        number(unapply(star[1].0 * zbase, resistance_factor[1])),
        number(unapply(star[1].1 * zbase, reactance_factor[1])),
        number(winding_nominal_kv(&transformer.windings[1], buses[1])),
        number(unapply(star[2].0 * zbase, resistance_factor[2])),
        number(unapply(star[2].1 * zbase, reactance_factor[2])),
        number(winding_nominal_kv(&transformer.windings[2], buses[2])),
        terminal_attributes(index, &component, 1, true, transformer.in_service),
        terminal_attributes(index, &component, 2, true, transformer.in_service),
        terminal_attributes(index, &component, 3, true, transformer.in_service),
        limits_attributes,
    ));
    for (side, (winding, bus)) in transformer.windings.iter().zip(buses).enumerate() {
        if !taps[side].is_empty() {
            for tap in &taps[side] {
                write_tap_changer(tap, true, output, diagnostics);
            }
            continue;
        }
        let rated_u = winding_nominal_kv(winding, bus);
        let base_ratio = rated_u / bus.base_kv;
        let rho = if base_ratio == 0.0 {
            1.0
        } else {
            winding.tap / base_ratio
        };
        if (rho - 1.0).abs() > f64::EPSILON {
            output.push_str(&format!(
                "      <iidm:ratioTapChanger{} tapPosition=\"0\" lowTapPosition=\"0\" loadTapChangingCapabilities=\"false\">\n        <iidm:step rho=\"{}\"/>\n      </iidm:ratioTapChanger{}>\n",
                side + 1,
                number(rho),
                side + 1,
            ));
        }
        if winding.shift != 0.0 {
            output.push_str(&format!(
                "      <iidm:phaseTapChanger{} tapPosition=\"0\" lowTapPosition=\"0\" loadTapChangingCapabilities=\"false\">\n        <iidm:step rho=\"1\" alpha=\"{}\"/>\n      </iidm:phaseTapChanger{}>\n",
                side + 1,
                number(-winding.shift),
                side + 1,
            ));
        }
    }
    if has_exact_operational_limit_groups(index, &component) {
        write_exact_operational_limit_groups(index, &component, output, diagnostics);
    } else {
        for (side, winding) in transformer.windings.iter().enumerate() {
            if winding.rate_a == 0.0 && winding.rate_b == 0.0 && winding.rate_c == 0.0 {
                continue;
            }
            output.push_str(&format!(
                "      <iidm:operationalLimitsGroup{} id=\"powerio\">\n",
                side + 1
            ));
            write_loading_limits(
                "apparentPowerLimits",
                winding.rate_a,
                winding.rate_b,
                winding.rate_c,
                output,
            );
            output.push_str(&format!(
                "      </iidm:operationalLimitsGroup{}>\n",
                side + 1
            ));
        }
    }
    output.push_str("    </iidm:threeWindingsTransformer>\n");
    for (side, winding) in transformer.windings.iter().enumerate() {
        if let Some(control) = &winding.control {
            diagnostics.push(
                &codes::EMIT_XIIDM.field_dropped,
                format!(
                    "three winding transformer `{id}` winding {} {} data has no XIIDM transformer field",
                    side + 1,
                    transformer_control_mode_name(control.mode),
                ),
            );
        }
    }
    if (transformer.star_vm - 1.0).abs() > f64::EPSILON || transformer.star_va.abs() > f64::EPSILON
    {
        diagnostics.push(
            &codes::EMIT_XIIDM.field_dropped,
            format!("three winding transformer `{id}` star voltage has no XIIDM equipment field"),
        );
    }
}

fn winding_nominal_kv(winding: &Winding, bus: &Bus) -> f64 {
    if winding.nominal_kv > 0.0 {
        winding.nominal_kv
    } else {
        bus.base_kv
    }
}

fn hvdc_mode(line: &Hvdc) -> &str {
    match line.converters_mode {
        Some(HvdcConvertersMode::Side1RectifierSide2Inverter) => "SIDE_1_RECTIFIER_SIDE_2_INVERTER",
        Some(HvdcConvertersMode::Side1InverterSide2Rectifier) => "SIDE_1_INVERTER_SIDE_2_RECTIFIER",
        None => unreachable!("XIIDM HVDC emission was validated"),
    }
}

fn hvdc_station_id(line: &Hvdc, original_side: u8) -> String {
    let converter = if original_side == 1 {
        line.converter1.as_ref()
    } else {
        line.converter2.as_ref()
    };
    converter
        .expect("XIIDM HVDC emission was validated")
        .component
        .local_id()
        .to_owned()
}

fn hvdc_converter(line: &Hvdc, original_side: u8) -> Option<&HvdcConverter> {
    if original_side == 1 {
        line.converter1.as_ref()
    } else {
        line.converter2.as_ref()
    }
}

fn hvdc_original_side_is_from(line: &Hvdc, original_side: u8) -> bool {
    match hvdc_mode(line) {
        "SIDE_1_INVERTER_SIDE_2_RECTIFIER" => original_side == 2,
        _ => original_side == 1,
    }
}

fn terminal_attributes_for_bus(
    index: &XiidmWriteIndex<'_>,
    level: &VoltageLevel,
    bus: BusId,
    connected: bool,
) -> String {
    if level.topology_kind == TopologyKind::BusBreaker
        && let Some(configured) = index
            .bus_breaker_bus_by_calculated_bus
            .get(&(level.component.clone(), bus))
    {
        let id = xml(configured.component.local_id());
        return if connected {
            format!(" bus=\"{id}\" connectableBus=\"{id}\"")
        } else {
            format!(" connectableBus=\"{id}\"")
        };
    }
    if let Some(node) = index
        .connectivity_node_by_calculated_bus
        .get(&(level.component.clone(), bus))
    {
        if let Some(number) = index.node_number(&node.component) {
            return format!(" node=\"{number}\"");
        }
    }
    String::new()
}

fn write_hvdc_converter_stations(
    index: &XiidmWriteIndex<'_>,
    level: &VoltageLevel,
    included: &dyn Fn(&ComponentId) -> bool,
    output: &mut String,
    diagnostics: &mut Diagnostics,
) {
    for (line, original_side) in index
        .hvdc_converters_by_level
        .get(&level.component)
        .into_iter()
        .flatten()
        .copied()
    {
        let is_from = hvdc_original_side_is_from(line, original_side);
        let bus = if is_from { line.from } else { line.to };
        let id = hvdc_station_id(line, original_side);
        let converter =
            hvdc_converter(line, original_side).expect("XIIDM HVDC emission was validated");
        let component = converter.component.clone();
        if !included(&component) {
            continue;
        }
        let mut terminal = terminal_attributes(index, &component, 1, false, line.in_service);
        if terminal.is_empty() {
            terminal = terminal_attributes_for_bus(index, level, bus, line.in_service);
            if terminal.is_empty() {
                diagnostics.push(
                    &codes::EMIT_XIIDM.record_dropped,
                    format!(
                        "HVDC converter station `{id}` has no terminal in voltage level `{}`",
                        level.component.local_id()
                    ),
                );
                continue;
            }
        }
        let metadata = index.metadata(&component);
        let loss_factor = converter.loss_factor_percent;
        let p = if is_from { line.pf } else { -line.pt };
        let (q, qmin, qmax) = if is_from {
            (line.qf, line.qminf, line.qmaxf)
        } else {
            (line.qt, line.qmint, line.qmaxt)
        };
        if converter.kind == HvdcConverterKind::Lcc {
            let power_factor = converter
                .power_factor
                .expect("XIIDM LCC emission was validated");
            let solution = if index.supplied_detailed_connectivity
                && index.terminal(&component, 1).is_some()
            {
                String::new()
            } else {
                injection_solution_attributes(index, &component, p, q)
            };
            output.push_str(&format!(
                    "      <iidm:lccConverterStation id=\"{}\"{} lossFactor=\"{}\" powerFactor=\"{}\"{terminal}{solution}/>\n",
                    xml(&id),
                    identifiable_attributes(metadata),
                    number(loss_factor),
                    number(power_factor),
                ));
        } else {
            let voltage_regulator_on = converter
                .voltage_regulator_on
                .expect("XIIDM VSC emission was validated");
            let voltage_setpoint =
                optional_number_attribute("voltageSetpoint", converter.voltage_setpoint_kv);
            let reactive_power_setpoint = optional_number_attribute(
                "reactivePowerSetpoint",
                converter.reactive_power_setpoint_mvar,
            );
            let solution = if index.supplied_detailed_connectivity
                && index.terminal(&component, 1).is_some()
            {
                String::new()
            } else {
                injection_solution_attributes(index, &component, p, q)
            };
            output.push_str(&format!(
                    "      <iidm:vscConverterStation id=\"{}\"{} lossFactor=\"{}\" voltageRegulatorOn=\"{}\"{voltage_setpoint}{reactive_power_setpoint}{terminal}{solution}>\n        <iidm:minMaxReactiveLimits minQ=\"{}\" maxQ=\"{}\"/>\n{}      </iidm:vscConverterStation>\n",
                    xml(&id),
                    identifiable_attributes(metadata),
                    number(loss_factor),
                    voltage_regulator_on,
                    number(qmin),
                    number(qmax),
                    converter
                        .regulating_terminal
                        .as_ref()
                        .map_or_else(String::new, |reference| write_terminal_reference(
                            "regulatingTerminal",
                            reference,
                            8,
                        )),
                ));
        }
    }
}

fn write_hvdc_lines(
    network: &BalancedNetwork,
    index: &XiidmWriteIndex<'_>,
    included: &dyn Fn(&ComponentId) -> bool,
    output: &mut String,
    diagnostics: &mut Diagnostics,
) {
    for line in network
        .hvdc()
        .iter()
        .filter(|line| component_is_included(included, "hvdc", line.uid.as_deref()))
    {
        let id = line.uid.as_deref().unwrap_or("hvdc");
        let component = component_id("hvdc", id).expect("valid stored identity");
        let metadata = index.metadata(&component);
        let nominal_v = line
            .nominal_voltage_kv
            .expect("XIIDM HVDC emission was validated");
        let resistance = line
            .resistance_ohm
            .expect("XIIDM HVDC emission was validated");
        output.push_str(&format!(
            "  <iidm:hvdcLine id=\"{}\"{} r=\"{}\" nominalV=\"{}\" activePowerSetpoint=\"{}\" maxP=\"{}\" convertersMode=\"{}\" converterStation1=\"{}\" converterStation2=\"{}\"{}\n",
            xml(id),
            identifiable_attributes(metadata),
            number(resistance),
            number(nominal_v),
            number(line.pf),
            number(line.pmax),
            hvdc_mode(line),
            xml(&hvdc_station_id(line, 1)),
            xml(&hvdc_station_id(line, 2)),
            if has_identifiable_children(metadata) {
                ">"
            } else {
                "/>"
            },
        ));
        if has_identifiable_children(metadata) {
            write_identifiable_children(metadata, output);
            output.push_str("  </iidm:hvdcLine>\n");
        }
        if line.pmin < 0.0 {
            diagnostics.push(
                &codes::EMIT_XIIDM.field_dropped,
                format!("HVDC line `{id}` reverse power bound has no XIIDM hvdcLine attribute"),
            );
        }
        if line.cost.is_some() {
            diagnostics.push(
                &codes::EMIT_XIIDM.field_dropped,
                format!("HVDC line `{id}` dispatch cost has no XIIDM field"),
            );
        }
    }
}

fn write_line(
    index: &XiidmWriteIndex<'_>,
    branch: &Branch,
    output: &mut String,
    diagnostics: &mut Diagnostics,
) {
    let id = branch.uid.as_deref().unwrap_or("line");
    let component = component_id("branch", id).expect("valid stored identity");
    let metadata = index.metadata(&component);
    let from = index.bus(branch.from);
    let to = index.bus(branch.to);
    let r = branch.r * from.base_kv * to.base_kv / DEFAULT_BASE_MVA;
    let x = branch.x * from.base_kv * to.base_kv / DEFAULT_BASE_MVA;
    let denominator = r * r + x * x;
    let y_real = if denominator == 0.0 {
        0.0
    } else {
        r / denominator
    };
    let y_imag = if denominator == 0.0 {
        0.0
    } else {
        -x / denominator
    };
    let charging = branch.calc_terminal_charging();
    let inverse = |shunt: f64, at: f64, other: f64, transmission: f64| {
        shunt * DEFAULT_BASE_MVA / (at * at) - (1.0 - other / at) * transmission
    };
    let g1 = inverse(charging.g_fr, from.base_kv, to.base_kv, y_real);
    let b1 = inverse(charging.b_fr, from.base_kv, to.base_kv, y_imag);
    let g2 = inverse(charging.g_to, to.base_kv, from.base_kv, y_real);
    let b2 = inverse(charging.b_to, to.base_kv, from.base_kv, y_imag);
    let terminal1 = terminal_attributes(index, &component, 1, true, branch.in_service);
    let terminal2 = terminal_attributes(index, &component, 2, true, branch.in_service);
    let limits = has_branch_operational_limits(branch);
    output.push_str(&format!(
        "  <iidm:line id=\"{}\"{} r=\"{}\" x=\"{}\" g1=\"{}\" b1=\"{}\" g2=\"{}\" b2=\"{}\"{terminal1}{terminal2}{}{}>\n",
        xml(id), identifiable_attributes_with_name(metadata, branch.name.as_deref()), number(r), number(x), number(g1), number(b1), number(g2), number(b2),
        branch_solution_attributes(index, &component, branch.solution),
        selected_operational_limits_attributes(index, &component, 2, |_| limits),
    ));
    write_identifiable_children(metadata, output);
    if limits || has_exact_operational_limit_groups(index, &component) {
        write_branch_operational_limits(index, &component, branch, 2, output, diagnostics);
    }
    output.push_str("  </iidm:line>\n");
    warn_branch_fields(id, branch, diagnostics);
}

fn write_transformer(
    index: &XiidmWriteIndex<'_>,
    branch: &Branch,
    output: &mut String,
    diagnostics: &mut Diagnostics,
) {
    let id = branch.uid.as_deref().unwrap_or("transformer");
    let component = component_id("branch", id).expect("valid stored identity");
    let metadata = index.metadata(&component);
    let from = index.bus(branch.from);
    let to = index.bus(branch.to);
    let zbase = to.base_kv * to.base_kv / DEFAULT_BASE_MVA;
    let charging = branch.calc_terminal_charging();
    let taps = transformer_tap_changers(index, &component, 1);
    let rho_factor = tap_step_factor(&taps, |step| step.rho);
    let resistance_factor = tap_step_factor(&taps, |step| {
        1.0 + step.resistance_deviation_percent / 100.0
    });
    let reactance_factor =
        tap_step_factor(&taps, |step| 1.0 + step.reactance_deviation_percent / 100.0);
    let conductance_factor = tap_step_factor(&taps, |step| {
        1.0 + step.conductance_deviation_percent / 100.0
    });
    let susceptance_factor = tap_step_factor(&taps, |step| {
        1.0 + step.susceptance_deviation_percent / 100.0
    });
    let unapply = |value: f64, factor: f64| {
        if factor.abs() <= f64::EPSILON {
            value
        } else {
            value / factor
        }
    };
    let terminal1 = terminal_attributes(index, &component, 1, true, branch.in_service);
    let terminal2 = terminal_attributes(index, &component, 2, true, branch.in_service);
    output.push_str(&format!(
        "    <iidm:twoWindingsTransformer id=\"{}\"{} r=\"{}\" x=\"{}\" g=\"{}\" b=\"{}\" ratedU1=\"{}\" ratedU2=\"{}\"{terminal1}{terminal2}{}{}>\n",
        xml(id),
        identifiable_attributes_with_name(metadata, branch.name.as_deref()),
        number(unapply(branch.r * zbase, resistance_factor)),
        number(unapply(branch.x * zbase, reactance_factor)),
        number(unapply(charging.g_fr / zbase, conductance_factor)),
        number(unapply(charging.b_fr / zbase, susceptance_factor)),
        number(from.base_kv * branch.calc_effective_tap() * rho_factor),
        number(to.base_kv),
        branch_solution_attributes(index, &component, branch.solution),
        selected_operational_limits_attributes(
            index,
            &component,
            2,
            |_| has_branch_operational_limits(branch),
        ),
    ));
    write_identifiable_children(metadata, output);
    for tap in &taps {
        write_tap_changer(tap, false, output, diagnostics);
    }
    if branch.shift != 0.0 && !taps.iter().any(|tap| tap.kind == TapChangerKind::Phase) {
        output.push_str(&format!(
            "      <iidm:phaseTapChanger tapPosition=\"0\" lowTapPosition=\"0\" loadTapChangingCapabilities=\"false\" regulationMode=\"CURRENT_LIMITER\" regulating=\"false\">\n        <iidm:step rho=\"1\" alpha=\"{}\"/>\n      </iidm:phaseTapChanger>\n",
            number(-branch.shift),
        ));
    }
    write_branch_operational_limits(index, &component, branch, 2, output, diagnostics);
    output.push_str("    </iidm:twoWindingsTransformer>\n");
    warn_branch_fields(id, branch, diagnostics);
}

fn has_branch_operational_limits(branch: &Branch) -> bool {
    branch.rate_a != 0.0
        || branch.rate_b != 0.0
        || branch.rate_c != 0.0
        || branch.current_ratings.is_some()
        || !branch.rating_sets.is_empty()
}

fn has_exact_operational_limit_groups(
    index: &XiidmWriteIndex<'_>,
    equipment: &ComponentId,
) -> bool {
    index.operational_limits.contains_key(equipment)
}

fn selected_operational_limits_attributes(
    index: &XiidmWriteIndex<'_>,
    equipment: &ComponentId,
    sides: u8,
    synthesize: impl Fn(u8) -> bool,
) -> String {
    let exact = has_exact_operational_limit_groups(index, equipment);
    let mut attributes = String::new();
    for side in 1..=sides {
        let selected = index
            .operational_limits
            .get(equipment)
            .into_iter()
            .flatten()
            .filter(|group| group.terminal == side && group.selected)
            .map(|group| group.id.as_str())
            .collect::<Vec<_>>();
        if !selected.is_empty() {
            attributes.push_str(&format!(
                " selectedOperationalLimitsGroupIds{side}=\"{}\"",
                xml(&format_string_array(&selected))
            ));
        } else if !exact && synthesize(side) {
            attributes.push_str(&format!(
                " selectedOperationalLimitsGroupIds{side}=\"powerio\""
            ));
        }
    }
    attributes
}

fn selected_boundary_operational_limits_attribute(
    index: &XiidmWriteIndex<'_>,
    equipment: &ComponentId,
) -> String {
    let selected = index
        .operational_limits
        .get(equipment)
        .into_iter()
        .flatten()
        .filter(|group| group.terminal == 1 && group.selected)
        .map(|group| group.id.as_str())
        .collect::<Vec<_>>();
    if selected.is_empty() {
        String::new()
    } else {
        format!(
            " selectedOperationalLimitsGroupIds=\"{}\"",
            xml(&format_string_array(&selected))
        )
    }
}

fn format_string_array(values: &[&str]) -> String {
    values
        .iter()
        .map(|value| {
            if value.contains([',', '"', '\n', '\r']) {
                format!("\"{}\"", value.replace('"', "\"\""))
            } else {
                (*value).to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn write_branch_operational_limits(
    index: &XiidmWriteIndex<'_>,
    equipment: &ComponentId,
    branch: &Branch,
    sides: u8,
    output: &mut String,
    diagnostics: &mut Diagnostics,
) {
    if has_exact_operational_limit_groups(index, equipment) {
        write_exact_operational_limit_groups(index, equipment, output, diagnostics);
        return;
    }
    if !has_branch_operational_limits(branch) {
        return;
    }
    for side in 1..=sides {
        output.push_str(&format!(
            "      <iidm:operationalLimitsGroup{side} id=\"powerio\">\n"
        ));
        write_loading_limits(
            "apparentPowerLimits",
            branch.rate_a,
            branch.rate_b,
            branch.rate_c,
            output,
        );
        if let Some(current) = branch.current_ratings {
            write_loading_limits(
                "currentLimits",
                current.c_rating_a,
                current.c_rating_b,
                current.c_rating_c,
                output,
            );
        }
        output.push_str(&format!("      </iidm:operationalLimitsGroup{side}>\n"));
        if side == 1 {
            for (index, rating) in branch.rating_sets.iter().enumerate() {
                output.push_str(&format!(
                    "      <iidm:operationalLimitsGroup{side} id=\"rating-set-{}\">\n        <iidm:apparentPowerLimits permanentLimit=\"{}\" permanentLimitName=\"{}\"/>\n      </iidm:operationalLimitsGroup{side}>\n",
                    index + 1,
                    number(rating.rate_mva),
                    xml(&rating.name),
                ));
            }
        }
    }
}

fn write_exact_operational_limit_groups(
    index: &XiidmWriteIndex<'_>,
    equipment: &ComponentId,
    output: &mut String,
    diagnostics: &mut Diagnostics,
) {
    for group in index
        .operational_limits
        .get(equipment)
        .into_iter()
        .flatten()
    {
        output.push_str(&format!(
            "      <iidm:operationalLimitsGroup{} id=\"{}\">\n",
            group.terminal,
            xml(&group.id)
        ));
        write_properties(&group.properties, 8, output);
        if let Some(limits) = &group.active_power_limits {
            write_exact_loading_limits("activePowerLimits", limits, output, diagnostics);
        }
        if let Some(limits) = &group.apparent_power_limits {
            write_exact_loading_limits("apparentPowerLimits", limits, output, diagnostics);
        }
        if let Some(limits) = &group.current_limits {
            write_exact_loading_limits("currentLimits", limits, output, diagnostics);
        }
        output.push_str(&format!(
            "      </iidm:operationalLimitsGroup{}>\n",
            group.terminal
        ));
    }
}

fn write_boundary_operational_limit_groups(
    index: &XiidmWriteIndex<'_>,
    equipment: &ComponentId,
    output: &mut String,
    diagnostics: &mut Diagnostics,
) {
    for group in index
        .operational_limits
        .get(equipment)
        .into_iter()
        .flatten()
        .filter(|group| group.terminal == 1)
    {
        output.push_str(&format!(
            "        <iidm:operationalLimitsGroup id=\"{}\">\n",
            xml(&group.id)
        ));
        write_properties(&group.properties, 10, output);
        if let Some(limits) = &group.active_power_limits {
            write_boundary_loading_limits("activePowerLimits", limits, output, diagnostics);
        }
        if let Some(limits) = &group.apparent_power_limits {
            write_boundary_loading_limits("apparentPowerLimits", limits, output, diagnostics);
        }
        if let Some(limits) = &group.current_limits {
            write_boundary_loading_limits("currentLimits", limits, output, diagnostics);
        }
        output.push_str("        </iidm:operationalLimitsGroup>\n");
    }
}

fn write_boundary_loading_limits(
    element: &str,
    limits: &LoadingLimits,
    output: &mut String,
    diagnostics: &mut Diagnostics,
) {
    output.push_str(&format!(
        "          <iidm:{element}{}{}>\n",
        optional_text_attribute("permanentLimitName", limits.permanent_limit_name.as_deref()),
        optional_number_attribute("permanentLimit", limits.permanent_limit),
    ));
    for limit in &limits.temporary_limits {
        let duration = if limit.acceptable_duration_seconds == i32::MAX as u64 {
            String::new()
        } else if i32::try_from(limit.acceptable_duration_seconds).is_ok() {
            format!(
                " acceptableDuration=\"{}\"",
                limit.acceptable_duration_seconds
            )
        } else {
            diagnostics.push(
                &codes::EMIT_XIIDM.field_dropped,
                format!(
                    "temporary limit `{}` duration exceeds the XIIDM xs:int range",
                    limit.name
                ),
            );
            String::new()
        };
        let value = if limit.value.to_bits() == f64::MAX.to_bits() {
            String::new()
        } else {
            format!(" value=\"{}\"", number(limit.value))
        };
        let fictitious = if limit.fictitious {
            " fictitious=\"true\""
        } else {
            ""
        };
        output.push_str(&format!(
            "            <iidm:temporaryLimit name=\"{}\"{duration}{value}{fictitious}/>\n",
            xml(&limit.name)
        ));
    }
    output.push_str(&format!("          </iidm:{element}>\n"));
}

fn write_exact_loading_limits(
    element: &str,
    limits: &LoadingLimits,
    output: &mut String,
    diagnostics: &mut Diagnostics,
) {
    output.push_str(&format!(
        "        <iidm:{element}{}{}>\n",
        optional_text_attribute("permanentLimitName", limits.permanent_limit_name.as_deref()),
        optional_number_attribute("permanentLimit", limits.permanent_limit),
    ));
    for limit in &limits.temporary_limits {
        let duration = if limit.acceptable_duration_seconds == i32::MAX as u64 {
            String::new()
        } else if i32::try_from(limit.acceptable_duration_seconds).is_ok() {
            format!(
                " acceptableDuration=\"{}\"",
                limit.acceptable_duration_seconds
            )
        } else {
            diagnostics.push(
                &codes::EMIT_XIIDM.field_dropped,
                format!(
                    "temporary limit `{}` duration exceeds the XIIDM xs:int range",
                    limit.name
                ),
            );
            String::new()
        };
        let value = if limit.value.to_bits() == f64::MAX.to_bits() {
            String::new()
        } else {
            format!(" value=\"{}\"", number(limit.value))
        };
        let fictitious = if limit.fictitious {
            " fictitious=\"true\""
        } else {
            ""
        };
        output.push_str(&format!(
            "          <iidm:temporaryLimit name=\"{}\"{duration}{value}{fictitious}/>\n",
            xml(&limit.name)
        ));
    }
    output.push_str(&format!("        </iidm:{element}>\n"));
}

fn write_loading_limits(
    element: &str,
    permanent: f64,
    long_term: f64,
    short_term: f64,
    output: &mut String,
) {
    if permanent == 0.0 && long_term == 0.0 && short_term == 0.0 {
        return;
    }
    output.push_str(&format!(
        "        <iidm:{element}{}>\n",
        if permanent == 0.0 {
            String::new()
        } else {
            format!(" permanentLimit=\"{}\"", number(permanent))
        },
    ));
    if long_term != 0.0 {
        output.push_str(&format!(
            "          <iidm:temporaryLimit name=\"rate_b\" acceptableDuration=\"1200\" value=\"{}\"/>\n",
            number(long_term)
        ));
    }
    if short_term != 0.0 {
        output.push_str(&format!(
            "          <iidm:temporaryLimit name=\"rate_c\" acceptableDuration=\"60\" value=\"{}\"/>\n",
            number(short_term)
        ));
    }
    output.push_str(&format!("        </iidm:{element}>\n"));
}

fn warn_branch_fields(id: &str, branch: &Branch, diagnostics: &mut Diagnostics) {
    if branch.has_angle_limits() {
        diagnostics.push(
            &codes::EMIT_XIIDM.field_dropped,
            format!("branch `{id}` angle bounds have no XIIDM branch attribute"),
        );
    }
    if let Some(control) = &branch.control {
        diagnostics.push(
            &codes::EMIT_XIIDM.field_dropped,
            format!(
                "transformer `{id}` {} data has no XIIDM transformer field",
                transformer_control_mode_name(control.mode),
            ),
        );
    }
}

fn transformer_control_mode_name(mode: TransformerControlMode) -> &'static str {
    match mode {
        TransformerControlMode::Fixed => "fixed tap control",
        TransformerControlMode::Voltage => "voltage control",
        TransformerControlMode::ReactiveFlow => "reactive power flow control",
        TransformerControlMode::ActiveFlow => "active power flow control",
        TransformerControlMode::DcLineQuantity => "DC line quantity control",
        TransformerControlMode::AsymmetricActiveFlow => "asymmetric active power flow control",
    }
}

fn terminal_attributes(
    index: &XiidmWriteIndex<'_>,
    equipment: &ComponentId,
    side: u8,
    branch: bool,
    _in_service: bool,
) -> String {
    let Some(terminal) = index.terminal(equipment, side) else {
        return String::new();
    };
    let suffix = if branch {
        side.to_string()
    } else {
        String::new()
    };
    let power = format!(
        "{}{}",
        optional_number_attribute(&format!("p{suffix}"), terminal.active_power_mw),
        optional_number_attribute(&format!("q{suffix}"), terminal.reactive_power_mvar),
    );
    if let Some(node) = terminal
        .node
        .as_ref()
        .and_then(|node| index.node_number(node))
    {
        let voltage_level = if branch {
            format!(
                " voltageLevelId{suffix}=\"{}\"",
                xml(terminal.voltage_level.local_id())
            )
        } else {
            String::new()
        };
        return format!(" node{suffix}=\"{node}\"{voltage_level}{power}");
    }
    let connected = terminal
        .bus
        .as_ref()
        .map(ComponentId::local_id)
        .filter(|_| terminal.connected);
    let connectable = terminal
        .connectable_bus
        .as_ref()
        .map(ComponentId::local_id)
        .or(connected);
    let mut attrs = String::new();
    if let Some(connected) = connected {
        attrs.push_str(&format!(" bus{suffix}=\"{}\"", xml(connected)));
    }
    if let Some(connectable) = connectable {
        attrs.push_str(&format!(" connectableBus{suffix}=\"{}\"", xml(connectable)));
    }
    if branch {
        attrs.push_str(&format!(
            " voltageLevelId{suffix}=\"{}\"",
            xml(terminal.voltage_level.local_id())
        ));
    }
    attrs.push_str(&power);
    attrs
}

#[cfg(test)]
fn component_metadata<'a>(
    detailed: &'a DetailedConnectivity,
    component: &ComponentId,
) -> Option<&'a ComponentMetadata> {
    detailed
        .component_metadata
        .iter()
        .find(|metadata| metadata.component == *component)
}

fn identifiable_attributes(metadata: Option<&ComponentMetadata>) -> String {
    let Some(metadata) = metadata else {
        return String::new();
    };
    let mut attributes = optional_text_attribute("name", metadata.name.as_deref());
    if metadata.fictitious {
        attributes.push_str(" fictitious=\"true\"");
    }
    attributes
}

fn identifiable_attributes_with_name(
    metadata: Option<&ComponentMetadata>,
    typed_name: Option<&str>,
) -> String {
    if metadata
        .and_then(|metadata| metadata.name.as_deref())
        .is_some()
    {
        return identifiable_attributes(metadata);
    }
    let mut attributes = optional_text_attribute("name", typed_name);
    if metadata.is_some_and(|metadata| metadata.fictitious) {
        attributes.push_str(" fictitious=\"true\"");
    }
    attributes
}

fn has_identifiable_children(metadata: Option<&ComponentMetadata>) -> bool {
    metadata.is_some_and(|value| {
        !value.aliases.is_empty()
            || !value.external_identifiers.is_empty()
            || !value.properties.is_empty()
    })
}

fn write_identifiable_children(metadata: Option<&ComponentMetadata>, output: &mut String) {
    write_identifiable_children_at(metadata, 6, output);
}

fn write_identifiable_children_at(
    metadata: Option<&ComponentMetadata>,
    indentation: usize,
    output: &mut String,
) {
    let Some(metadata) = metadata else {
        return;
    };
    let indentation = " ".repeat(indentation);
    for alias in &metadata.aliases {
        output.push_str(&format!(
            "{indentation}<iidm:alias{}>{}</iidm:alias>\n",
            optional_text_attribute("type", alias.alias_type.as_deref()),
            xml(&alias.value)
        ));
    }
    for identifier in &metadata.external_identifiers {
        output.push_str(&format!(
            "{indentation}<iidm:alias{}>{}</iidm:alias>\n",
            optional_text_attribute("type", identifier.authority.as_deref()),
            xml(&identifier.value)
        ));
    }
    for (name, value) in &metadata.properties {
        output.push_str(&format!(
            "{indentation}<iidm:property name=\"{}\" value=\"{}\"/>\n",
            xml(name),
            xml(value)
        ));
    }
}

fn optional_text_attribute(name: &str, value: Option<&str>) -> String {
    value.map_or_else(String::new, |value| format!(" {name}=\"{}\"", xml(value)))
}

fn optional_number_attribute(name: &str, value: Option<f64>) -> String {
    value.map_or_else(String::new, |value| {
        format!(" {name}=\"{}\"", number(value))
    })
}

fn write_terminal_reference(
    element: &str,
    reference: &TerminalReference,
    indentation: usize,
) -> String {
    let side = match reference.terminal {
        1 => String::new(),
        2 => " side=\"TWO\"".to_owned(),
        3 => " side=\"THREE\"".to_owned(),
        other => format!(" side=\"{other}\""),
    };
    format!(
        "{}<iidm:{element} id=\"{}\"{side}/>\n",
        " ".repeat(indentation),
        xml(reference.equipment.local_id()),
    )
}

fn switch_kind(kind: SwitchKind) -> &'static str {
    match kind {
        SwitchKind::Breaker => "BREAKER",
        SwitchKind::Disconnector => "DISCONNECTOR",
        SwitchKind::LoadBreakSwitch => "LOAD_BREAK_SWITCH",
    }
}

fn safe_ratio(value: f64, total: f64) -> f64 {
    if total == 0.0 { 0.0 } else { value / total }
}

fn branch_solution_attributes(
    index: &XiidmWriteIndex<'_>,
    equipment: &ComponentId,
    solution: Option<BranchSolution>,
) -> String {
    if index.terminal_solution.contains(equipment) {
        return String::new();
    }
    solution.map_or_else(String::new, |value| {
        format!(
            " p1=\"{}\" q1=\"{}\" p2=\"{}\" q2=\"{}\"",
            number(value.pf),
            number(value.qf),
            number(value.pt),
            number(value.qt)
        )
    })
}

fn injection_solution_attributes(
    index: &XiidmWriteIndex<'_>,
    equipment: &ComponentId,
    active_power_mw: f64,
    reactive_power_mvar: f64,
) -> String {
    if index.terminal_solution.contains(equipment) {
        String::new()
    } else {
        format!(
            " p=\"{}\" q=\"{}\"",
            number(active_power_mw),
            number(reactive_power_mvar),
        )
    }
}

fn number(value: f64) -> String {
    let value = format!("{value:.17}");
    let value = value.trim_end_matches('0').trim_end_matches('.');
    if value.is_empty() || value == "-0" {
        "0".to_owned()
    } else {
        value.to_owned()
    }
}

fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn assert_f64_close(actual: f64, expected: f64) {
        let tolerance = f64::EPSILON * expected.abs().max(1.0);
        assert!(
            (actual - expected).abs() <= tolerance,
            "expected {expected}, got {actual}"
        );
    }

    const BUS_BREAKER: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<iidm:network xmlns:iidm="http://www.powsybl.org/schema/iidm/1_17" id="case" caseDate="2025-01-01T00:00:00Z" forecastDistance="0" sourceFormat="test" minimumValidationLevel="STEADY_STATE_HYPOTHESIS">
  <iidm:substation id="S" country="US" tso="MISO" geographicalTags="east">
    <iidm:voltageLevel id="VL1" nominalV="230" lowVoltageLimit="207" highVoltageLimit="253" topologyKind="BUS_BREAKER">
      <iidm:busBreakerTopology><iidm:bus id="B1" v="230" angle="0"/></iidm:busBreakerTopology>
      <iidm:generator id="G1" energySource="OTHER" minP="0" maxP="200" voltageRegulatorOn="true" targetP="100" targetQ="10" targetV="230" bus="B1" connectableBus="B1">
        <iidm:minMaxReactiveLimits minQ="-50" maxQ="50"/>
      </iidm:generator>
    </iidm:voltageLevel>
    <iidm:voltageLevel id="VL2" nominalV="230" topologyKind="BUS_BREAKER">
      <iidm:busBreakerTopology><iidm:bus id="B2" v="225" angle="-2"/></iidm:busBreakerTopology>
      <iidm:load id="L1" loadType="UNDEFINED" p0="90" q0="30" bus="B2" connectableBus="B2"/>
    </iidm:voltageLevel>
  </iidm:substation>
  <iidm:line id="LINE" r="2" x="20" g1="0" b1="0.0001" g2="0" b2="0.0001" bus1="B1" connectableBus1="B1" voltageLevelId1="VL1" bus2="B2" connectableBus2="B2" voltageLevelId2="VL2"/>
</iidm:network>"#;

    const REMOTE_GENERATOR_VOLTAGE_CONTROL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<iidm:network xmlns:iidm="http://www.powsybl.org/schema/iidm/1_17" id="remote-generator-control" caseDate="2026-01-01T00:00:00Z" forecastDistance="0" sourceFormat="test" minimumValidationLevel="STEADY_STATE_HYPOTHESIS">
  <iidm:substation id="S">
    <iidm:voltageLevel id="VL1" nominalV="230" topologyKind="BUS_BREAKER">
      <iidm:busBreakerTopology><iidm:bus id="B1"/></iidm:busBreakerTopology>
      <iidm:generator id="G" energySource="OTHER" minP="0" maxP="200" voltageRegulatorOn="false" targetP="100" targetQ="10" targetV="119.6" bus="B1" connectableBus="B1">
        <iidm:regulatingTerminal id="L"/>
        <iidm:minMaxReactiveLimits minQ="-50" maxQ="50"/>
      </iidm:generator>
    </iidm:voltageLevel>
    <iidm:voltageLevel id="VL2" nominalV="115" topologyKind="BUS_BREAKER">
      <iidm:busBreakerTopology><iidm:bus id="B2"/></iidm:busBreakerTopology>
      <iidm:load id="L" loadType="UNDEFINED" p0="90" q0="30" bus="B2" connectableBus="B2"/>
    </iidm:voltageLevel>
  </iidm:substation>
</iidm:network>"#;

    // Reduced from PowSybl's MPL-2.0
    // `V1_17/activePowerControlRoundTripRef.xml` fixture.
    const POWSYBL_ACTIVE_POWER_CONTROL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<iidm:network xmlns:iidm="http://www.powsybl.org/schema/iidm/1_17" xmlns:apc="http://www.powsybl.org/schema/iidm/ext/active_power_control/1_1" id="active-power-control" caseDate="2017-06-25T17:43:00.000+01:00" forecastDistance="0" sourceFormat="test" minimumValidationLevel="STEADY_STATE_HYPOTHESIS">
  <iidm:substation id="P1" country="FR" tso="R" geographicalTags="A">
    <iidm:voltageLevel id="VL" nominalV="400" topologyKind="BUS_BREAKER">
      <iidm:busBreakerTopology><iidm:bus id="B"/></iidm:busBreakerTopology>
      <iidm:generator id="GEN" energySource="OTHER" minP="-200" maxP="900" voltageRegulatorOn="true" targetP="607" targetV="400" targetQ="301" bus="B" connectableBus="B">
        <iidm:minMaxReactiveLimits minQ="-400" maxQ="400"/>
      </iidm:generator>
      <iidm:battery id="BAT" targetP="100" targetQ="20" minP="-200" maxP="200" bus="B" connectableBus="B">
        <iidm:minMaxReactiveLimits minQ="-50" maxQ="50"/>
      </iidm:battery>
    </iidm:voltageLevel>
  </iidm:substation>
  <iidm:extension id="GEN"><apc:activePowerControl participate="false" droop="3" participationFactor="1"/></iidm:extension>
  <iidm:extension id="BAT"><apc:activePowerControl participate="true" droop="4" participationFactor="1.2"/></iidm:extension>
</iidm:network>"#;

    const NODE_BREAKER: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<iidm:network xmlns:iidm="http://www.powsybl.org/schema/iidm/1_17" id="nodes" caseDate="2025-01-01T00:00:00Z" forecastDistance="0" sourceFormat="test" minimumValidationLevel="STEADY_STATE_HYPOTHESIS">
  <iidm:substation id="S">
    <iidm:voltageLevel id="VL" nominalV="110" topologyKind="NODE_BREAKER">
      <iidm:nodeBreakerTopology>
        <iidm:bus v="110" angle="0" nodes="0,1,2"/>
        <iidm:busbarSection id="BBS" node="2"/>
        <iidm:switch id="BR" kind="BREAKER" open="false" node1="1" node2="2"/>
        <iidm:internalConnection node1="0" node2="1"/>
      </iidm:nodeBreakerTopology>
      <iidm:generator id="G" energySource="OTHER" minP="0" maxP="10" voltageRegulatorOn="true" targetP="5" node="0">
        <iidm:minMaxReactiveLimits minQ="-2" maxQ="2"/>
      </iidm:generator>
    </iidm:voltageLevel>
  </iidm:substation>
</iidm:network>"#;

    #[test]
    fn xiidm_projection_refuses_connectivity_loss_and_reports_cgmes_only_records() {
        let empty_terminal = DcTerminal {
            component: None,
            sequence_number: None,
            dc_node: None,
            dc_topological_node: None,
            polarity: None,
            connected: None,
            active_power_mw: None,
            current_a: None,
        };
        let mut connectivity = DetailedConnectivity::default();
        connectivity.dc_busbars.push(crate::network::DcBusbar {
            component: component_id("dc_busbar", "B").unwrap(),
            equipment_container: None,
            dc_terminal: empty_terminal,
            rated_dc_voltage_kv: Some(320.0),
        });
        let error = diagnose_xiidm_projection(&connectivity, &mut Diagnostics::new()).unwrap_err();
        assert!(error.to_string().contains("DC busbar"));
        assert!(error.to_string().contains("without changing connectivity"));

        let mut connectivity = DetailedConnectivity::default();
        connectivity
            .dc_converter_units
            .push(crate::network::DcConverterUnit {
                component: component_id("dc_converter_unit", "U").unwrap(),
                substation: None,
                operation_mode: crate::network::DcConverterOperatingMode::Bipolar,
            });
        connectivity
            .dc_topological_nodes
            .push(crate::network::DcTopologicalNode {
                component: component_id("dc_topological_node", "TN").unwrap(),
                dc_converter_unit: Some(component_id("dc_converter_unit", "U").unwrap()),
            });
        let mut diagnostics = Diagnostics::new();
        diagnose_xiidm_projection(&connectivity, &mut diagnostics).unwrap();
        let messages = diagnostics
            .records()
            .iter()
            .filter(|diagnostic| diagnostic.code() == codes::EMIT_XIIDM.field_dropped.code)
            .map(powerio_core::Diagnostic::message)
            .collect::<Vec<_>>();
        assert_eq!(messages.len(), 2);
        assert!(
            messages
                .iter()
                .any(|message| message.contains("DC converter unit"))
        );
        assert!(
            messages
                .iter()
                .any(|message| message.contains("DC topological node"))
        );
    }

    #[test]
    fn xiidm_projection_reports_source_identities_and_metadata_it_cannot_retain() {
        let voltage_level = component_id("voltage_level", "VL").unwrap();
        let terminal = component_id("terminal", "terminal-mrid").unwrap();
        let connectivity_node = component_id("connectivity_node", "node-mrid").unwrap();
        let transformer = component_id("branch", "T").unwrap();
        let load = component_id("load", "L").unwrap();
        let mut connectivity = DetailedConnectivity::default();
        connectivity.terminals.push(Terminal {
            component: Some(terminal),
            equipment: load.clone(),
            terminal: 1,
            voltage_level: voltage_level.clone(),
            bus: None,
            connectable_bus: None,
            node: Some(connectivity_node.clone()),
            connected: false,
            active_power_mw: None,
            reactive_power_mvar: None,
        });
        connectivity.connectivity_nodes.push(ConnectivityNode {
            component: connectivity_node,
            voltage_level: voltage_level.clone(),
            node_number: Some(7),
            calculated_bus: None,
        });
        connectivity.tap_changers.push(NetworkTapChanger {
            component: Some(component_id("tap_changer", "tap-mrid").unwrap()),
            transformer,
            winding: 1,
            kind: TapChangerKind::Ratio,
            tap_position: Some(1),
            solved_tap_position: None,
            low_tap_position: 0,
            neutral_tap_position: None,
            normal_tap_position: Some(2),
            voltage_step_increment_percent: Some(1.25),
            load_tap_changing_capabilities: true,
            regulating: false,
            regulation_mode: None,
            regulation_value: None,
            target_deadband: None,
            regulation_terminal: None,
            steps: Vec::new(),
        });
        connectivity.component_metadata.push(ComponentMetadata {
            component: load,
            name: None,
            equipment_container: Some(voltage_level),
            aliases: Vec::new(),
            external_identifiers: vec![crate::network::ExternalIdentifier {
                value: "external-1".into(),
                authority: Some("authority".into()),
            }],
            properties: BTreeMap::new(),
            fictitious: false,
        });

        let mut diagnostics = Diagnostics::new();
        diagnose_xiidm_projection(&connectivity, &mut diagnostics).unwrap();
        let messages = diagnostics.lines().join("\n");
        assert!(messages.contains("terminal 1 identity `terminal/terminal-mrid`"));
        assert!(messages.contains("connectivity node `connectivity_node/node-mrid`"));
        assert!(messages.contains("tap changer identity"));
        assert!(messages.contains("neutral tap position distinct from low tap position"));
        assert!(messages.contains("normal tap position distinct from assigned tap position"));
        assert!(messages.contains("voltage step increment"));
        assert!(messages.contains("external identifiers are emitted as XIIDM aliases"));
        assert!(messages.contains("explicit equipment container `voltage_level/VL`"));

        let mut native = DetailedConnectivity::default();
        native.connectivity_nodes.push(ConnectivityNode {
            component: node_component_id("VL", 7).unwrap(),
            voltage_level: component_id("voltage_level", "VL").unwrap(),
            node_number: Some(7),
            calculated_bus: None,
        });
        let mut diagnostics = Diagnostics::new();
        diagnose_xiidm_projection(&native, &mut diagnostics).unwrap();
        assert!(diagnostics.is_empty());
    }

    const EQUIPMENT_COVERAGE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<iidm:network xmlns:iidm="http://www.powsybl.org/schema/iidm/1_17" id="equipment" caseDate="2026-01-01T00:00:00Z" forecastDistance="0" sourceFormat="test" minimumValidationLevel="STEADY_STATE_HYPOTHESIS">
  <iidm:substation id="S">
    <iidm:voltageLevel id="VL1" nominalV="132" topologyKind="BUS_BREAKER">
      <iidm:busBreakerTopology><iidm:bus id="B1" v="132" angle="0"/></iidm:busBreakerTopology>
      <iidm:generator id="G" energySource="OTHER" minP="0" maxP="200" voltageRegulatorOn="true" targetP="50" targetV="132" bus="B1" connectableBus="B1"><iidm:minMaxReactiveLimits minQ="-100" maxQ="100"/></iidm:generator>
      <iidm:vscConverterStation id="C1" lossFactor="1" voltageRegulatorOn="true" voltageSetpoint="132" bus="B1" connectableBus="B1" q="5"><iidm:minMaxReactiveLimits minQ="-20" maxQ="20"/></iidm:vscConverterStation>
    </iidm:voltageLevel>
    <iidm:voltageLevel id="VL2" nominalV="33" topologyKind="BUS_BREAKER">
      <iidm:busBreakerTopology><iidm:bus id="B2" v="33" angle="-1"/></iidm:busBreakerTopology>
      <iidm:vscConverterStation id="C2" lossFactor="1" voltageRegulatorOn="false" reactivePowerSetpoint="3" bus="B2" connectableBus="B2"><iidm:minMaxReactiveLimits minQ="-10" maxQ="10"/></iidm:vscConverterStation>
    </iidm:voltageLevel>
    <iidm:voltageLevel id="VL3" nominalV="11" topologyKind="BUS_BREAKER">
      <iidm:busBreakerTopology><iidm:bus id="B3" v="11" angle="-2"/></iidm:busBreakerTopology>
    </iidm:voltageLevel>
    <iidm:threeWindingsTransformer id="T3" ratedU0="132" ratedU1="132" ratedU2="33" ratedU3="11" r1="17.424" x1="34.848" r2="1.7424" x2="3.4848" r3="0.8712" x3="1.7424" bus1="B1" connectableBus1="B1" voltageLevelId1="VL1" bus2="B2" connectableBus2="B2" voltageLevelId2="VL2" bus3="B3" connectableBus3="B3" voltageLevelId3="VL3" selectedOperationalLimitsGroupIds1="normal">
      <iidm:ratioTapChanger2 tapPosition="0" lowTapPosition="0" loadTapChangingCapabilities="false"><iidm:property name="tap-kind" value="ratio"/><iidm:step rho="1.05"><iidm:property name="step-label" value="nominal"/></iidm:step></iidm:ratioTapChanger2>
      <iidm:operationalLimitsGroup1 id="normal"><iidm:apparentPowerLimits permanentLimit="90"><iidm:temporaryLimit name="emergency" acceptableDuration="600" value="100"/></iidm:apparentPowerLimits></iidm:operationalLimitsGroup1>
    </iidm:threeWindingsTransformer>
  </iidm:substation>
  <iidm:line id="L" r="1" x="10" bus1="B1" connectableBus1="B1" voltageLevelId1="VL1" bus2="B2" connectableBus2="B2" voltageLevelId2="VL2" selectedOperationalLimitsGroupIds1="normal" selectedOperationalLimitsGroupIds2="normal">
    <iidm:operationalLimitsGroup1 id="normal"><iidm:apparentPowerLimits permanentLimit="120"><iidm:property name="limit-set" value="seasonal"/><iidm:temporaryLimit name="long" acceptableDuration="1200" value="140"><iidm:property name="cause" value="contingency"/></iidm:temporaryLimit><iidm:temporaryLimit name="short" acceptableDuration="60" value="160"/></iidm:apparentPowerLimits><iidm:currentLimits permanentLimit="500"/></iidm:operationalLimitsGroup1>
    <iidm:operationalLimitsGroup1 id="seasonal"><iidm:apparentPowerLimits permanentLimit="100" permanentLimitName="summer"/></iidm:operationalLimitsGroup1>
    <iidm:operationalLimitsGroup2 id="normal"><iidm:apparentPowerLimits permanentLimit="110"/><iidm:currentLimits permanentLimit="450"/></iidm:operationalLimitsGroup2>
  </iidm:line>
  <iidm:hvdcLine id="DC" r="1" nominalV="320" activePowerSetpoint="100" maxP="150" convertersMode="SIDE_1_RECTIFIER_SIDE_2_INVERTER" converterStation1="C1" converterStation2="C2"/>
</iidm:network>"#;

    #[test]
    fn reads_bus_breaker_electrical_and_hierarchy_data() {
        let mut diagnostics = Diagnostics::new();
        let network = parse_xiidm_source(BUS_BREAKER, &mut diagnostics).unwrap();
        assert_eq!(network.name(), "case");
        assert!((network.base_mva() - 100.0).abs() < f64::EPSILON);
        assert_eq!(network.buses().len(), 2);
        assert_eq!(network.loads().len(), 1);
        assert_eq!(network.generators().len(), 1);
        assert_eq!(network.branches().len(), 1);
        assert_eq!(
            network.case_metadata().source_model_format.as_deref(),
            Some("test")
        );
        let detailed = network.detailed_connectivity().as_ref().unwrap();
        assert_eq!(detailed.substations.len(), 1);
        assert_eq!(detailed.voltage_levels.len(), 2);
        assert_eq!(detailed.terminals.len(), 4);
    }

    #[test]
    fn generator_voltage_control_round_trips_with_exact_remote_terminal() {
        let network =
            parse_xiidm_source(REMOTE_GENERATOR_VOLTAGE_CONTROL, &mut Diagnostics::new()).unwrap();
        let generator = &network.generators()[0];
        assert!(!generator.voltage_regulation_on);
        assert_eq!(generator.regulated_bus, Some(BusId(2)));
        assert_eq!(
            generator.regulating_terminal,
            Some(TerminalReference {
                equipment: component_id("load", "L").unwrap(),
                terminal: 1,
            })
        );
        assert_f64_close(generator.vg, 1.04);

        let emission = write_xiidm(&network).unwrap();
        assert!(emission.text.contains("voltageRegulatorOn=\"false\""));
        assert!(emission.text.contains("targetV="));
        assert!(
            emission
                .text
                .contains("<iidm:regulatingTerminal id=\"L\"/>")
        );

        let reparsed = parse_xiidm_source(&emission.text, &mut Diagnostics::new()).unwrap();
        assert_eq!(
            reparsed.generators()[0].regulating_terminal,
            generator.regulating_terminal
        );
        assert_eq!(reparsed.generators()[0].regulated_bus, Some(BusId(2)));
        assert!(!reparsed.generators()[0].voltage_regulation_on);
        assert_f64_close(reparsed.generators()[0].vg, generator.vg);
    }

    #[test]
    fn reads_and_emits_powsybl_active_power_control_versions() {
        let mut diagnostics = Diagnostics::new();
        let network = parse_xiidm_source(POWSYBL_ACTIVE_POWER_CONTROL, &mut diagnostics).unwrap();
        assert!(
            !diagnostics
                .records()
                .iter()
                .any(|diagnostic| diagnostic.code() == "READ.XIIDM.ELEMENT_UNMAPPED")
        );
        let generator = network.generators()[0]
            .active_power_control
            .as_ref()
            .unwrap();
        assert!(!generator.participate);
        assert_eq!(generator.droop_percent, Some(3.0));
        assert_eq!(generator.participation_factor, Some(1.0));
        assert_eq!(generator.minimum_target_active_power_mw, None);
        assert_eq!(generator.maximum_target_active_power_mw, None);
        let battery = network.storage()[0].active_power_control.as_ref().unwrap();
        assert!(battery.participate);
        assert_eq!(battery.droop_percent, Some(4.0));
        assert_eq!(battery.participation_factor, Some(1.2));

        let emission = write_xiidm(&network).unwrap();
        assert!(emission.text.contains(ACTIVE_POWER_CONTROL_NAMESPACE_V1_2));
        assert!(emission.text.contains(
            "<apc:activePowerControl participate=\"false\" droop=\"3\" participationFactor=\"1\""
        ));
        let reparsed = parse_xiidm_source(&emission.text, &mut Diagnostics::new()).unwrap();
        assert_eq!(
            reparsed.generators()[0].active_power_control,
            network.generators()[0].active_power_control
        );
        assert_eq!(
            reparsed.storage()[0].active_power_control,
            network.storage()[0].active_power_control
        );

        let v1_2 = POWSYBL_ACTIVE_POWER_CONTROL
            .replace(
                ACTIVE_POWER_CONTROL_NAMESPACE_V1_1,
                ACTIVE_POWER_CONTROL_NAMESPACE_V1_2,
            )
            .replace(
                "participationFactor=\"1\"/>",
                "participationFactor=\"1\" maxTargetP=\"800\"/>",
            )
            .replace(
                "participationFactor=\"1.2\"/>",
                "participationFactor=\"1.2\" minTargetP=\"10\"/>",
            );
        let network = parse_xiidm_source(&v1_2, &mut Diagnostics::new()).unwrap();
        assert_eq!(
            network.generators()[0]
                .active_power_control
                .as_ref()
                .unwrap()
                .maximum_target_active_power_mw,
            Some(800.0)
        );
        assert_eq!(
            network.storage()[0]
                .active_power_control
                .as_ref()
                .unwrap()
                .minimum_target_active_power_mw,
            Some(10.0)
        );

        let v1_0 = POWSYBL_ACTIVE_POWER_CONTROL
            .replace(NAMESPACE, "http://www.powsybl.org/schema/iidm/1_12")
            .replace(
                ACTIVE_POWER_CONTROL_NAMESPACE_V1_1,
                ACTIVE_POWER_CONTROL_NAMESPACE_V1_0,
            )
            .replace(" participationFactor=\"1\"", "")
            .replace(" participationFactor=\"1.2\"", "");
        let network = parse_xiidm_source(&v1_0, &mut Diagnostics::new()).unwrap();
        assert_eq!(
            network.generators()[0]
                .active_power_control
                .as_ref()
                .unwrap()
                .participation_factor,
            None
        );

        let false_only = POWSYBL_ACTIVE_POWER_CONTROL
            .replace(" droop=\"3\" participationFactor=\"1\"", "")
            .replace(" droop=\"4\" participationFactor=\"1.2\"", "");
        let network = parse_xiidm_source(&false_only, &mut Diagnostics::new()).unwrap();
        let control = network.generators()[0]
            .active_power_control
            .as_ref()
            .unwrap();
        assert!(!control.participate);
        assert_eq!(control.droop_percent, None);
        assert_eq!(control.participation_factor, None);
    }

    #[test]
    fn rejects_invalid_active_power_control_values() {
        for invalid in [
            POWSYBL_ACTIVE_POWER_CONTROL
                .replace("participationFactor=\"1\"", "participationFactor=\"-1\""),
            POWSYBL_ACTIVE_POWER_CONTROL.replace("droop=\"3\"", "droop=\"NaN\""),
            POWSYBL_ACTIVE_POWER_CONTROL
                .replace(
                    ACTIVE_POWER_CONTROL_NAMESPACE_V1_1,
                    ACTIVE_POWER_CONTROL_NAMESPACE_V1_2,
                )
                .replace(
                    "participationFactor=\"1\"/>",
                    "participationFactor=\"1\" minTargetP=\"-201\"/>",
                ),
            POWSYBL_ACTIVE_POWER_CONTROL
                .replace(
                    ACTIVE_POWER_CONTROL_NAMESPACE_V1_1,
                    ACTIVE_POWER_CONTROL_NAMESPACE_V1_2,
                )
                .replace(
                    "participationFactor=\"1\"/>",
                    "participationFactor=\"1\" minTargetP=\"100\" maxTargetP=\"50\"/>",
                ),
        ] {
            assert!(parse_xiidm_source(&invalid, &mut Diagnostics::new()).is_err());
        }

        let mut network =
            parse_xiidm_source(POWSYBL_ACTIVE_POWER_CONTROL, &mut Diagnostics::new()).unwrap();
        network.generators_mut()[0]
            .active_power_control
            .as_mut()
            .unwrap()
            .participation_factor = Some(-1.0);
        assert!(write_xiidm(&network).is_err());
    }

    #[test]
    fn reads_node_breaker_connectivity_without_flattening_it_away() {
        let mut diagnostics = Diagnostics::new();
        let network = parse_xiidm_source(NODE_BREAKER, &mut diagnostics).unwrap();
        assert_eq!(network.buses().len(), 1);
        let detailed = network.detailed_connectivity().as_ref().unwrap();
        assert_eq!(detailed.connectivity_nodes.len(), 3);
        assert_eq!(detailed.busbar_sections.len(), 1);
        assert_eq!(detailed.switches.len(), 1);
        assert_eq!(detailed.internal_connections.len(), 1);
        assert!(
            detailed
                .connectivity_nodes
                .iter()
                .all(|node| node.calculated_bus == Some(BusId::new(1)))
        );
        assert!(detailed.terminals.iter().any(|terminal| {
            terminal.equipment == component_id("busbar_section", "BBS").unwrap()
                && terminal.terminal == 1
                && terminal.node == Some(component_id("connectivity_node", "VL/2").unwrap())
        }));
    }

    #[test]
    fn tap_changer_can_regulate_a_busbar_section_terminal() {
        let source = NODE_BREAKER
            .replace(
                "<iidm:bus v=\"110\" angle=\"0\" nodes=\"0,1,2\"/>",
                "<iidm:bus v=\"110\" angle=\"0\" nodes=\"0,1\"/><iidm:bus v=\"110\" angle=\"0\" nodes=\"2\"/>",
            )
            .replace("open=\"false\"", "open=\"true\"")
            .replace(
                "    </iidm:voltageLevel>\n  </iidm:substation>",
                r#"    </iidm:voltageLevel>
    <iidm:twoWindingsTransformer id="T" r="1" x="10" g="0" b="0" ratedU1="110" ratedU2="110" voltageLevelId1="VL" node1="0" voltageLevelId2="VL" node2="2">
      <iidm:ratioTapChanger regulating="true" tapPosition="0" lowTapPosition="0" loadTapChangingCapabilities="true" regulationMode="VOLTAGE" regulationValue="110">
        <iidm:terminalRef id="BBS"/>
        <iidm:step rho="1" r="0" x="0" g="0" b="0"/>
      </iidm:ratioTapChanger>
    </iidm:twoWindingsTransformer>
  </iidm:substation>"#,
            );
        let network = parse_xiidm_source(&source, &mut Diagnostics::new()).unwrap();
        let detailed = network.detailed_connectivity().as_ref().unwrap();
        let tap = detailed
            .tap_changers
            .iter()
            .find(|tap| tap.transformer.local_id() == "T")
            .unwrap();
        assert_eq!(
            tap.regulation_terminal,
            Some(TerminalReference {
                equipment: component_id("busbar_section", "BBS").unwrap(),
                terminal: 1,
            })
        );

        let emission = write_xiidm(&network).unwrap();
        assert!(emission.text.contains("<iidm:terminalRef id=\"BBS\"/>"));
        parse_xiidm_source(&emission.text, &mut Diagnostics::new()).unwrap();
    }

    #[test]
    fn valid_xiidm_preserves_an_omitted_generator_target_q() {
        let source = BUS_BREAKER
            .replace(NAMESPACE, "http://www.powsybl.org/schema/iidm/1_12")
            .replace(" targetQ=\"10\"", "");
        let network = parse_xiidm_source(&source, &mut Diagnostics::new()).unwrap();
        let component = component_id("generator", "G1").unwrap();
        assert_f64_close(network.generators()[0].qg, 0.0);
        assert!(
            network
                .detailed_connectivity()
                .as_deref()
                .unwrap()
                .omitted_fields
                .contains(&OmittedField {
                    component: component.clone(),
                    field: OmittedFieldName::ReactivePower,
                })
        );
        assert!(
            network
                .detailed_connectivity()
                .as_deref()
                .unwrap()
                .omitted_fields
                .contains(&OmittedField {
                    component: component.clone(),
                    field: OmittedFieldName::RatedApparentPower,
                })
        );

        let restored = BalancedNetwork::from_json(&network.to_json().unwrap()).unwrap();
        let emission = write_xiidm(&restored).unwrap();
        let generator = emission
            .text
            .lines()
            .find(|line| line.contains("<iidm:generator id=\"G1\""))
            .unwrap();
        assert!(emission.text.contains(NAMESPACE));
        assert!(!emission.text.contains(EQUIPMENT_NAMESPACE));
        assert!(
            emission
                .text
                .contains("minimumValidationLevel=\"STEADY_STATE_HYPOTHESIS\"")
        );
        assert!(generator.contains("targetP=\"100\""));
        assert!(!generator.contains("targetQ="));
        assert!(generator.contains("targetV=\"230\""));
        assert!(!generator.contains("ratedS="));
        let reparsed = parse_xiidm_source(&emission.text, &mut Diagnostics::new()).unwrap();
        assert!(
            reparsed
                .detailed_connectivity()
                .as_deref()
                .unwrap()
                .omitted_fields
                .contains(&OmittedField {
                    component,
                    field: OmittedFieldName::ReactivePower,
                })
        );

        let mut changed = restored;
        changed.generators_mut()[0].mbase = 125.0;
        let changed = write_xiidm(&changed).unwrap();
        let generator = changed
            .text
            .lines()
            .find(|line| line.contains("<iidm:generator id=\"G1\""))
            .unwrap();
        assert!(generator.contains("ratedS=\"125\""));
    }

    #[test]
    fn reads_equipment_mode_xiidm_1_12_and_emits_xiidm_1_17() {
        let source = r#"<?xml version="1.0" encoding="UTF-8"?>
<iidm:network xmlns:iidm="http://www.powsybl.org/schema/iidm/equipment/1_12" id="old" caseDate="2021-01-03T00:00:00Z" forecastDistance="0" sourceFormat="DIE" minimumValidationLevel="EQUIPMENT">
  <iidm:substation id="S">
    <iidm:voltageLevel id="VL" nominalV="225" topologyKind="NODE_BREAKER">
      <iidm:nodeBreakerTopology><iidm:busbarSection id="BBS" node="0"/></iidm:nodeBreakerTopology>
      <iidm:load id="L" loadType="UNDEFINED" node="0"/>
      <iidm:generator id="G" energySource="SOLAR" minP="0" maxP="100" voltageRegulatorOn="true" node="0"><iidm:minMaxReactiveLimits minQ="-20" maxQ="20"/></iidm:generator>
      <iidm:battery id="BAT" minP="-10" maxP="10" node="0"><iidm:minMaxReactiveLimits minQ="-5" maxQ="5"/></iidm:battery>
      <iidm:shunt id="SH" voltageRegulatorOn="false" node="0"><iidm:shuntLinearModel bPerSection="-0.001" maximumSectionCount="1"/></iidm:shunt>
      <iidm:danglingLine id="BL" r="0" x="0.1" g="0" b="0" node="0"/>
    </iidm:voltageLevel>
  </iidm:substation>
</iidm:network>"#;
        assert!(looks_like_xiidm(source.as_bytes()));
        let mut diagnostics = Diagnostics::new();
        let network = parse_xiidm_source(source, &mut diagnostics).unwrap();
        assert_eq!(network.generators().len(), 1);
        assert_f64_close(network.generators()[0].pg, 0.0);
        assert_f64_close(network.generators()[0].qg, 0.0);
        assert_f64_close(network.generators()[0].vg, 1.0);
        assert_eq!(
            network.generators()[0].energy_source,
            GeneratorEnergySource::Solar
        );
        assert_eq!(network.loads().len(), 2);
        let load = network
            .loads()
            .iter()
            .find(|load| load.uid.as_deref() == Some("L"))
            .unwrap();
        assert_f64_close(load.p, 0.0);
        assert_f64_close(load.q, 0.0);
        assert_eq!(network.storage().len(), 1);
        assert_f64_close(network.storage()[0].ps, 0.0);
        assert_f64_close(network.storage()[0].qs, 0.0);
        assert_eq!(network.shunts().len(), 1);
        assert_eq!(network.shunts()[0].section_count, None);
        assert_f64_close(network.shunts()[0].g, 0.0);
        assert_f64_close(network.shunts()[0].b, 0.0);
        let detailed = network.detailed_connectivity().as_deref().unwrap();
        assert_eq!(detailed.omitted_fields.len(), 11);
        assert!(detailed.omitted_fields.contains(&OmittedField {
            component: component_id("generator", "G").unwrap(),
            field: OmittedFieldName::VoltageSetpoint,
        }));
        assert!(detailed.omitted_fields.contains(&OmittedField {
            component: component_id("generator", "G").unwrap(),
            field: OmittedFieldName::RatedApparentPower,
        }));
        assert!(detailed.omitted_fields.contains(&OmittedField {
            component: component_id("shunt", "SH").unwrap(),
            field: OmittedFieldName::ShuntConductancePerSection,
        }));
        let normalized = network.to_normalized().unwrap();
        assert_f64_close(normalized.generators()[0].pg, 0.0);
        assert_f64_close(normalized.generators()[0].qg, 0.0);
        assert_f64_close(normalized.generators()[0].vg, 1.0);
        assert_eq!(
            normalized
                .detailed_connectivity()
                .as_deref()
                .unwrap()
                .omitted_fields,
            detailed.omitted_fields
        );
        let restored = BalancedNetwork::from_json(&network.to_json().unwrap()).unwrap();
        assert_eq!(
            restored
                .detailed_connectivity()
                .as_deref()
                .unwrap()
                .omitted_fields,
            detailed.omitted_fields
        );
        let mut edited = network.clone();
        edited.generators_mut()[0].pg = 25.0;
        let edited_detailed = edited.detailed_connectivity().as_deref().unwrap();
        assert!(
            !edited_detailed
                .omitted_fields
                .iter()
                .any(|omitted| { omitted.component == component_id("generator", "G").unwrap() })
        );
        assert!(
            edited_detailed
                .omitted_fields
                .iter()
                .any(|omitted| { omitted.component == component_id("load", "L").unwrap() })
        );
        let edited_emission = write_xiidm(&edited).unwrap();
        let generator_line = edited_emission
            .text
            .lines()
            .find(|line| line.contains("<iidm:generator id=\"G\""))
            .unwrap();
        assert!(generator_line.contains("targetP=\"25\""));
        assert!(generator_line.contains("targetQ=\"0\""));
        assert!(generator_line.contains("targetV=\"225\""));
        assert!(
            diagnostics
                .records()
                .iter()
                .any(|diagnostic| diagnostic.code() == "READ.XIIDM.VERSION.COMPATIBILITY")
        );
        assert!(
            diagnostics
                .lines()
                .iter()
                .any(|message| message.contains("has no `targetP`"))
        );
        let emission = write_xiidm(&network).unwrap();
        assert!(emission.text.contains(EQUIPMENT_NAMESPACE));
        assert!(emission.text.contains("energySource=\"SOLAR\""));
        assert!(!emission.text.contains("targetP="));
        assert!(!emission.text.contains("targetQ="));
        assert!(!emission.text.contains("targetV="));
        assert!(!emission.text.contains(" p0="));
        assert!(!emission.text.contains(" q0="));
        assert!(emission.text.contains("<iidm:shuntCompensator"));
        assert!(!emission.text.contains("sectionCount="));
        assert!(emission.text.contains("bPerSection=\"-0.001\""));
        assert!(!emission.text.contains("gPerSection="));
        assert!(!emission.text.contains("<iidm:shunt id="));
        let reparsed = parse_xiidm_source(&emission.text, &mut Diagnostics::new()).unwrap();
        assert_eq!(
            reparsed
                .detailed_connectivity()
                .as_deref()
                .unwrap()
                .omitted_fields,
            detailed.omitted_fields
        );
        assert_eq!(
            reparsed.generators()[0].energy_source,
            GeneratorEnergySource::Solar
        );
    }

    #[test]
    fn summarizes_repeated_equipment_mode_assignment_defaults() {
        let loads = (0..9)
            .map(|index| {
                format!(
                    "      <iidm:load id=\"L{index}\" loadType=\"UNDEFINED\" bus=\"B\" connectableBus=\"B\"/>"
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let source = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<iidm:network xmlns:iidm="http://www.powsybl.org/schema/iidm/equipment/1_17" id="defaults" caseDate="2026-01-01T00:00:00Z" forecastDistance="0" sourceFormat="test" minimumValidationLevel="EQUIPMENT">
  <iidm:substation id="S"><iidm:voltageLevel id="VL" nominalV="100" topologyKind="BUS_BREAKER">
    <iidm:busBreakerTopology><iidm:bus id="B"/></iidm:busBreakerTopology>
{loads}
  </iidm:voltageLevel></iidm:substation>
</iidm:network>"#
        );
        let mut diagnostics = Diagnostics::new();
        let network = parse_xiidm_source(&source, &mut diagnostics).unwrap();
        assert_eq!(network.loads().len(), 9);
        let summaries = diagnostics
            .records()
            .iter()
            .filter(|diagnostic| {
                diagnostic.code() == "READ.XIIDM.VALUE_DEFAULTED"
                    && diagnostic
                        .message()
                        .contains("equipment-mode load records have no")
            })
            .collect::<Vec<_>>();
        assert_eq!(summaries.len(), 2);
        assert!(
            summaries
                .iter()
                .all(|diagnostic| diagnostic.message().starts_with("9 XIIDM"))
        );
        assert!(summaries.iter().all(|diagnostic| {
            diagnostic
                .message()
                .contains("sample IDs: `L0`, `L1`, `L2`, `L3`, `L4`")
        }));
    }

    #[test]
    fn reads_each_supported_xiidm_input_version() {
        for version in ["1_12", "1_13", "1_14", "1_15", "1_16", "1_17"] {
            let source = BUS_BREAKER.replace(
                NAMESPACE,
                &format!("http://www.powsybl.org/schema/iidm/{version}"),
            );
            let mut diagnostics = Diagnostics::new();
            let network = parse_xiidm_source(&source, &mut diagnostics).unwrap();
            assert_eq!(network.buses().len(), 2, "XIIDM {version}");
            if version != "1_17" {
                assert!(
                    diagnostics.records().iter().any(|diagnostic| {
                        diagnostic.code() == "READ.XIIDM.VERSION.COMPATIBILITY"
                    })
                );
            }
            assert!(write_xiidm(&network).unwrap().text.contains(NAMESPACE));
        }
    }

    #[test]
    fn applies_legacy_svc_regulation_modes() {
        let source = r#"<?xml version="1.0" encoding="UTF-8"?>
<iidm:network xmlns:iidm="http://www.powsybl.org/schema/iidm/1_13" id="svc" caseDate="2026-01-01T00:00:00Z" forecastDistance="0" sourceFormat="test" minimumValidationLevel="STEADY_STATE_HYPOTHESIS">
  <iidm:voltageLevel id="VL" nominalV="400" topologyKind="BUS_BREAKER">
    <iidm:busBreakerTopology><iidm:bus id="B"/></iidm:busBreakerTopology>
    <iidm:staticVarCompensator id="SVC" bMin="-0.01" bMax="0.01" voltageSetpoint="400" reactivePowerSetpoint="0" regulationMode="OFF" bus="B" connectableBus="B" p="0" q="-26.6"/>
  </iidm:voltageLevel>
</iidm:network>"#;
        let network = parse_xiidm_source(source, &mut Diagnostics::new()).unwrap();
        assert_eq!(network.static_var_compensators().len(), 1);
        let svc = &network.static_var_compensators()[0];
        assert_eq!(
            svc.regulation_mode,
            StaticVarCompensatorRegulationMode::Voltage
        );
        assert!(!svc.regulating);
        let emission = write_xiidm(&network).unwrap();
        let record = emission
            .text
            .lines()
            .find(|line| line.contains("<iidm:staticVarCompensator"))
            .unwrap();
        assert_eq!(record.matches(" p=\"").count(), 1);
        assert_eq!(record.matches(" q=\"").count(), 1);
        parse_xiidm_source(&emission.text, &mut Diagnostics::new()).unwrap();

        let voltage = source.replace("regulationMode=\"OFF\"", "regulationMode=\"VOLTAGE\"");
        let network = parse_xiidm_source(&voltage, &mut Diagnostics::new()).unwrap();
        assert!(network.static_var_compensators()[0].regulating);
        let emission = write_xiidm(&network).unwrap();
        let record = emission
            .text
            .lines()
            .find(|line| line.contains("<iidm:staticVarCompensator"))
            .unwrap();
        assert!(record.contains("regulating=\"true\""));
    }

    #[test]
    fn applies_pre_1_14_phase_tap_capability_rule() {
        let source = r#"<?xml version="1.0" encoding="UTF-8"?>
<iidm:network xmlns:iidm="http://www.powsybl.org/schema/iidm/equipment/1_12" id="phase" caseDate="2026-01-01T00:00:00Z" forecastDistance="0" sourceFormat="test" minimumValidationLevel="EQUIPMENT">
  <iidm:substation id="S">
    <iidm:voltageLevel id="VL1" nominalV="225" topologyKind="BUS_BREAKER"><iidm:busBreakerTopology><iidm:bus id="B1"/></iidm:busBreakerTopology></iidm:voltageLevel>
    <iidm:voltageLevel id="VL2" nominalV="225" topologyKind="BUS_BREAKER"><iidm:busBreakerTopology><iidm:bus id="B2"/></iidm:busBreakerTopology></iidm:voltageLevel>
    <iidm:twoWindingsTransformer id="T" r="1" x="10" g="0" b="0" ratedU1="225" ratedU2="225" voltageLevelId1="VL1" bus1="B1" connectableBus1="B1" voltageLevelId2="VL2" bus2="B2" connectableBus2="B2">
      <iidm:phaseTapChanger regulating="false" lowTapPosition="0" tapPosition="0" regulationMode="CURRENT_LIMITER" regulationValue="100"><iidm:step rho="1" alpha="0" r="0" x="0" g="0" b="0"/></iidm:phaseTapChanger>
    </iidm:twoWindingsTransformer>
  </iidm:substation>
</iidm:network>"#;
        let network = parse_xiidm_source(source, &mut Diagnostics::new()).unwrap();
        let tap = &network
            .detailed_connectivity()
            .as_ref()
            .unwrap()
            .tap_changers[0];
        assert_eq!(tap.kind, TapChangerKind::Phase);
        assert!(tap.load_tap_changing_capabilities);

        let emission = write_xiidm(&network).unwrap();
        let tap = emission
            .text
            .lines()
            .find(|line| line.contains("<iidm:phaseTapChanger"))
            .unwrap();
        assert!(tap.contains("loadTapChangingCapabilities=\"true\""));
    }

    #[test]
    fn reads_legacy_fixed_phase_tap_as_nonregulating_current_limiter() {
        for version in ["1_12", "1_13"] {
            let source = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<iidm:network xmlns:iidm="http://www.powsybl.org/schema/iidm/{version}" id="phase" caseDate="2026-01-01T00:00:00Z" forecastDistance="0" sourceFormat="test" minimumValidationLevel="STEADY_STATE_HYPOTHESIS">
  <iidm:substation id="S">
    <iidm:voltageLevel id="VL1" nominalV="225" topologyKind="BUS_BREAKER"><iidm:busBreakerTopology><iidm:bus id="B1"/></iidm:busBreakerTopology></iidm:voltageLevel>
    <iidm:voltageLevel id="VL2" nominalV="225" topologyKind="BUS_BREAKER"><iidm:busBreakerTopology><iidm:bus id="B2"/></iidm:busBreakerTopology></iidm:voltageLevel>
    <iidm:twoWindingsTransformer id="T" r="1" x="10" g="0" b="0" ratedU1="225" ratedU2="225" voltageLevelId1="VL1" bus1="B1" connectableBus1="B1" voltageLevelId2="VL2" bus2="B2" connectableBus2="B2">
      <iidm:phaseTapChanger regulating="true" lowTapPosition="0" tapPosition="0" regulationMode="FIXED_TAP"><iidm:step rho="1" alpha="0" r="0" x="0" g="0" b="0"/></iidm:phaseTapChanger>
    </iidm:twoWindingsTransformer>
  </iidm:substation>
</iidm:network>"#
            );
            let mut diagnostics = Diagnostics::new();
            let network = parse_xiidm_source(&source, &mut diagnostics).unwrap();
            let tap = &network
                .detailed_connectivity()
                .as_ref()
                .unwrap()
                .tap_changers[0];
            assert!(!tap.regulating, "XIIDM {version}");
            assert_eq!(
                tap.regulation_mode,
                Some(TapChangerRegulationMode::Current),
                "XIIDM {version}"
            );
            assert!(diagnostics.lines().iter().any(|line| {
                line.contains("legacy FIXED_TAP mode")
                    && line.contains("CURRENT_LIMITER")
                    && line.contains("regulation disabled")
            }));

            let emission = write_xiidm(&network).unwrap();
            let phase = emission
                .text
                .lines()
                .find(|line| line.contains("<iidm:phaseTapChanger"))
                .unwrap();
            assert!(phase.contains("regulationMode=\"CURRENT_LIMITER\""));
            assert!(phase.contains("regulating=\"false\""));
        }
    }

    #[test]
    fn reads_pre_1_17_dc_switch_without_resistance() {
        let source = r#"<?xml version="1.0" encoding="UTF-8"?>
<iidm:network xmlns:iidm="http://www.powsybl.org/schema/iidm/1_16" id="dc-switch" caseDate="2026-01-01T00:00:00Z" forecastDistance="0" sourceFormat="test" minimumValidationLevel="STEADY_STATE_HYPOTHESIS">
  <iidm:dcNode id="N1" nominalV="500"/>
  <iidm:dcNode id="N2" nominalV="500"/>
  <iidm:dcSwitch id="S" dcNode1="N1" dcNode2="N2" kind="BREAKER" open="false"/>
</iidm:network>"#;
        let network = parse_xiidm_source(source, &mut Diagnostics::new()).unwrap();
        let switch = &network
            .detailed_connectivity()
            .as_ref()
            .unwrap()
            .dc_switches[0];
        assert_eq!(switch.resistance_ohm, Some(0.0));
        let emission = write_xiidm(&network).unwrap();
        assert!(emission.text.contains(" r=\"0\""));
    }

    #[test]
    fn refuses_unverified_xiidm_versions_with_a_structured_diagnostic() {
        let source = NODE_BREAKER.replace(NAMESPACE, "http://www.powsybl.org/schema/iidm/1_11");
        assert!(looks_like_xiidm(source.as_bytes()));
        let mut diagnostics = Diagnostics::new();
        let error = parse_xiidm_source(&source, &mut diagnostics).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("supported input versions are 1.12 through 1.17")
        );
        assert!(
            diagnostics
                .records()
                .iter()
                .any(|diagnostic| diagnostic.code() == "PARSE.XIIDM.VERSION_UNSUPPORTED")
        );
    }

    #[test]
    fn phase_tap_changer_emits_the_xiidm_1_17_required_load_tap_attribute() {
        let source = r#"<?xml version="1.0" encoding="UTF-8"?>
<iidm:network xmlns:iidm="http://www.powsybl.org/schema/iidm/equipment/1_12" id="phase" caseDate="2021-01-03T00:00:00Z" forecastDistance="0" sourceFormat="test" minimumValidationLevel="EQUIPMENT">
  <iidm:substation id="S">
    <iidm:voltageLevel id="VL" nominalV="225" topologyKind="BUS_BREAKER">
      <iidm:busBreakerTopology><iidm:bus id="B1"/><iidm:bus id="B2"/></iidm:busBreakerTopology>
    </iidm:voltageLevel>
    <iidm:twoWindingsTransformer id="T" r="1" x="10" g="0" b="0" ratedU1="225" ratedU2="225" voltageLevelId1="VL" bus1="B1" connectableBus1="B1" voltageLevelId2="VL" bus2="B2" connectableBus2="B2">
      <iidm:phaseTapChanger lowTapPosition="0" tapPosition="0" regulating="false" regulationMode="CURRENT_LIMITER" regulationValue="1000"><iidm:step rho="1" alpha="5"/></iidm:phaseTapChanger>
    </iidm:twoWindingsTransformer>
  </iidm:substation>
</iidm:network>"#;
        let network = parse_xiidm_source(source, &mut Diagnostics::new()).unwrap();
        assert_eq!(network.branches().len(), 1);
        let emission = write_xiidm(&network).unwrap();
        let phase = emission
            .text
            .lines()
            .find(|line| line.contains("<iidm:phaseTapChanger"))
            .unwrap();
        assert!(phase.contains("loadTapChangingCapabilities=\"true\""));
        parse_xiidm_source(&emission.text, &mut Diagnostics::new()).unwrap();
    }

    #[test]
    fn node_breaker_connectivity_comes_from_connections_and_closed_switches() {
        let source = NODE_BREAKER
            .replace("open=\"false\"", "open=\"true\"")
            .replace(
                "        <iidm:bus v=\"110\" angle=\"0\" nodes=\"0,1,2\"/>\n",
                "",
            );
        let network = parse_xiidm_source(&source, &mut Diagnostics::new()).unwrap();
        assert_eq!(network.buses().len(), 2);
        let detailed = network.detailed_connectivity().as_ref().unwrap();
        let node2 = detailed
            .connectivity_nodes
            .iter()
            .find(|node| node.node_number == Some(2))
            .unwrap();
        let node0 = detailed
            .connectivity_nodes
            .iter()
            .find(|node| node.node_number == Some(0))
            .unwrap();
        assert_ne!(node0.calculated_bus, node2.calculated_bus);
    }

    #[test]
    fn fresh_emission_allocates_missing_node_numbers_without_collisions() {
        let mut network = parse_xiidm_source(NODE_BREAKER, &mut Diagnostics::new()).unwrap();
        let detailed =
            std::sync::Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        for node in &mut detailed.connectivity_nodes {
            node.node_number = (node.node_number == Some(1)).then_some(4);
        }

        let emission = write_xiidm(&network).unwrap();
        let reparsed = parse_xiidm_source(&emission.text, &mut Diagnostics::new()).unwrap();
        let numbers = reparsed
            .detailed_connectivity()
            .as_ref()
            .unwrap()
            .connectivity_nodes
            .iter()
            .filter_map(|node| node.node_number)
            .collect::<BTreeSet<_>>();
        assert_eq!(numbers, BTreeSet::from([0, 1, 4]));
    }

    #[test]
    fn reads_declared_iso_8859_1_xiidm_without_changing_the_source_bytes() {
        let source = BUS_BREAKER
            .replace("encoding=\"UTF-8\"", "encoding=\"ISO-8859-1\"")
            .replace("sourceFormat=\"test\"", "sourceFormat=\"Réseau PowSybl\"");
        let bytes: Vec<u8> = source
            .chars()
            .map(|value| u8::try_from(u32::from(value)).expect("fixture is ISO-8859-1"))
            .collect();
        assert!(std::str::from_utf8(&bytes).is_err());

        let source = powerio_core::Source::from_memory("case.xiidm", bytes.clone())
            .unwrap()
            .with_format(powerio_core::FormatId::new("xiidm").unwrap());
        let module = crate::format::parse(source).unwrap();
        let network = &module.value;
        assert_eq!(
            network.case_metadata().source_model_format.as_deref(),
            Some("Réseau PowSybl")
        );
        assert_eq!(
            module.source().unwrap().primary_buffer().unwrap().bytes(),
            bytes
        );

        let emitted = crate::format::emit(
            &module,
            crate::format::TargetFormat::Xiidm,
            powerio_core::Destination::memory("copy.xiidm").unwrap(),
        )
        .unwrap();
        assert_eq!(emitted.fidelity(), powerio_core::Fidelity::ExactSameFormat);
        let powerio_core::EmittedOutput::Memory { artifacts } = emitted.into_output() else {
            panic!("memory destination returned a path output");
        };
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].bytes(), bytes);
    }

    #[test]
    fn rejects_unknown_and_multibyte_xml_encodings() {
        for encoding in ["X-UNKNOWN-ENCODING", "Shift_JIS", "UTF-16"] {
            let source = BUS_BREAKER.replace("UTF-8", encoding);
            let error = parse_xiidm_bytes(source.as_bytes(), &mut Diagnostics::new()).unwrap_err();
            assert!(
                error.to_string().contains("unsupported XIIDM XML encoding"),
                "{encoding}: {error}"
            );
            assert!(error.to_string().contains(encoding), "{encoding}: {error}");
        }
    }

    #[test]
    fn non_utf8_xiidm_requires_an_encoding_declaration() {
        let source = BUS_BREAKER
            .replacen("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n", "", 1)
            .replace("sourceFormat=\"test\"", "sourceFormat=\"Réseau\"");
        let bytes: Vec<u8> = source
            .chars()
            .map(|value| u8::try_from(u32::from(value)).expect("fixture is ISO-8859-1"))
            .collect();
        let error = parse_xiidm_bytes(&bytes, &mut Diagnostics::new()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("cannot decode input using UTF-8")
        );
    }

    #[test]
    fn refuses_dtd_entities_and_dangling_bus_references() {
        let mut diagnostics = Diagnostics::new();
        let malicious = BUS_BREAKER.replacen(
            "<iidm:network",
            "<!DOCTYPE network [<!ENTITY xxe SYSTEM \"file:///etc/passwd\">]><iidm:network",
            1,
        );
        assert!(parse_xiidm_source(&malicious, &mut diagnostics).is_err());
        let dangling = BUS_BREAKER.replace("bus2=\"B2\"", "bus2=\"MISSING\"");
        assert!(parse_xiidm_source(&dangling, &mut Diagnostics::new()).is_err());
        let malformed = BUS_BREAKER.replace("</iidm:substation>", "</iidm:voltageLevel>");
        assert!(parse_xiidm_source(&malformed, &mut Diagnostics::new()).is_err());
    }

    #[test]
    fn refuses_oversized_and_deeply_nested_xiidm() {
        let oversized = vec![b' '; MAX_XIIDM_BYTES + 1];
        let error = parse_xiidm_bytes(&oversized, &mut Diagnostics::new()).unwrap_err();
        assert!(error.to_string().contains("byte input limit"));

        let mut nested = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><iidm:network xmlns:iidm=\"{NAMESPACE}\" xmlns:ext=\"urn:test\" id=\"deep\" caseDate=\"2026-01-01T00:00:00Z\" forecastDistance=\"0\" sourceFormat=\"test\" minimumValidationLevel=\"EQUIPMENT\"><iidm:extension id=\"unknown\">"
        );
        for _ in 0..MAX_XIIDM_ELEMENT_DEPTH {
            nested.push_str("<ext:n>");
        }
        let error = parse_xiidm_source(&nested, &mut Diagnostics::new()).unwrap_err();
        assert!(error.to_string().contains("element nesting limit"));
    }

    #[test]
    fn skips_complete_extension_subtrees() {
        let source = BUS_BREAKER
            .replace(
                "xmlns:iidm=\"http://www.powsybl.org/schema/iidm/1_17\"",
                "xmlns:iidm=\"http://www.powsybl.org/schema/iidm/1_17\" xmlns:ext=\"urn:test\"",
            )
            .replace(
                "</iidm:network>",
                "  <iidm:extension id=\"G1\"><ext:bus><ext:property name=\"ignored\"/></ext:bus></iidm:extension>\n</iidm:network>",
            );
        let mut diagnostics = Diagnostics::new();
        let network = parse_xiidm_source(&source, &mut diagnostics).unwrap();
        assert_eq!(network.buses().len(), 2);
        assert!(
            diagnostics
                .lines()
                .iter()
                .any(|message| message.contains("XIIDM extension"))
        );
        assert!(
            !diagnostics
                .lines()
                .iter()
                .any(|message| message.contains("XIIDM element `bus`"))
        );
    }

    #[test]
    fn rejects_model_elements_outside_the_root_xiidm_namespace() {
        let mixed_release = BUS_BREAKER.replacen(
            "<iidm:bus id=\"B1\"",
            "<old:bus xmlns:old=\"http://www.powsybl.org/schema/iidm/1_16\" id=\"B1\"",
            1,
        );
        let error = parse_xiidm_source(&mixed_release, &mut Diagnostics::new()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("element `bus` uses XML namespace")
        );
        assert!(error.to_string().contains("schema/iidm/1_16"));

        let foreign = BUS_BREAKER.replacen(
            "<iidm:bus id=\"B1\"",
            "<foreign:bus xmlns:foreign=\"urn:foreign\" id=\"B1\"",
            1,
        );
        let error = parse_xiidm_source(&foreign, &mut Diagnostics::new()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("element `bus` uses XML namespace")
        );
        assert!(error.to_string().contains("urn:foreign"));

        let foreign_root = BUS_BREAKER
            .replacen(
                "<iidm:network",
                "<foreign:network xmlns:foreign=\"urn:foreign\"",
                1,
            )
            .replace("</iidm:network>", "</foreign:network>");
        let error = parse_xiidm_source(&foreign_root, &mut Diagnostics::new()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("root element uses XML namespace")
        );
        assert!(error.to_string().contains("urn:foreign"));
    }

    #[test]
    fn accepts_extensions_only_at_the_network_extension_point() {
        parse_xiidm_source(POWSYBL_ACTIVE_POWER_CONTROL, &mut Diagnostics::new()).unwrap();

        let misplaced = BUS_BREAKER.replacen(
            "    </iidm:voltageLevel>",
            "      <iidm:extension id=\"G1\"/>\n    </iidm:voltageLevel>",
            1,
        );
        let error = parse_xiidm_source(&misplaced, &mut Diagnostics::new()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("extension must be a direct child of a network")
        );

        let misplaced_model = BUS_BREAKER.replace(
            "</iidm:network>",
            "  <iidm:extension id=\"G1\"><iidm:bus id=\"not-an-extension\"/></iidm:extension>\n</iidm:network>",
        );
        let error = parse_xiidm_source(&misplaced_model, &mut Diagnostics::new()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("model element `bus` appears inside an extension")
        );
    }

    #[test]
    fn parses_powsybl_operational_limit_group_csv() {
        assert_eq!(
            parse_operational_limits_group_ids(
                "DEFAULT,activated_1_1,activated_1_2,\"notANiceName\"\"\",\"anotherName,,,\""
            )
            .unwrap(),
            vec![
                "DEFAULT",
                "activated_1_1",
                "activated_1_2",
                "notANiceName\"",
                "anotherName,,,",
            ]
        );
        assert!(parse_operational_limits_group_ids("\"unclosed").is_err());
    }

    #[test]
    fn reads_pre_1_16_selected_operational_limit_group_ids() {
        let source = EQUIPMENT_COVERAGE
            .replace("/schema/iidm/1_17", "/schema/iidm/1_12")
            .replace(
                "selectedOperationalLimitsGroupIds1",
                "selectedOperationalLimitsGroupId1",
            )
            .replace(
                "selectedOperationalLimitsGroupIds2",
                "selectedOperationalLimitsGroupId2",
            );
        let network = parse_xiidm_source(&source, &mut Diagnostics::new()).unwrap();
        let groups = &network
            .detailed_connectivity()
            .as_ref()
            .unwrap()
            .operational_limit_groups;
        assert_eq!(
            groups
                .iter()
                .filter(|group| group.equipment.local_id() == "L" && group.selected)
                .map(|group| (group.terminal, group.id.as_str()))
                .collect::<Vec<_>>(),
            vec![(1, "normal"), (2, "normal")]
        );

        let emission = write_xiidm(&network).unwrap();
        let line = emission
            .text
            .lines()
            .find(|line| line.contains("<iidm:line id=\"L\""))
            .unwrap();
        assert!(line.contains("selectedOperationalLimitsGroupIds1=\"normal\""));
        assert!(line.contains("selectedOperationalLimitsGroupIds2=\"normal\""));
    }

    #[test]
    fn applies_pre_1_14_ratio_tap_regulation_rule() {
        let source = EQUIPMENT_COVERAGE
            .replace("/schema/iidm/1_17", "/schema/iidm/1_12")
            .replace(
                "selectedOperationalLimitsGroupIds1",
                "selectedOperationalLimitsGroupId1",
            )
            .replace(
                "selectedOperationalLimitsGroupIds2",
                "selectedOperationalLimitsGroupId2",
            )
            .replace(
                "<iidm:ratioTapChanger2 tapPosition=\"0\" lowTapPosition=\"0\" loadTapChangingCapabilities=\"false\">",
                "<iidm:ratioTapChanger2 tapPosition=\"0\" lowTapPosition=\"0\" loadTapChangingCapabilities=\"false\" regulating=\"true\" regulationMode=\"VOLTAGE\" regulationValue=\"33\"><iidm:terminalRef id=\"T3\" side=\"TWO\"/>",
            );
        let mut diagnostics = Diagnostics::new();
        let network = parse_xiidm_source(&source, &mut diagnostics).unwrap();
        let tap = network
            .detailed_connectivity()
            .as_ref()
            .unwrap()
            .tap_changers
            .iter()
            .find(|tap| tap.transformer.local_id() == "T3" && tap.winding == 2)
            .unwrap();
        assert!(!tap.load_tap_changing_capabilities);
        assert!(!tap.regulating);
        assert!(diagnostics.lines().iter().any(|line| {
            line.contains("PowSybl treats regulation as disabled") && line.contains("T3")
        }));

        let emission = write_xiidm(&network).unwrap();
        let tap = emission
            .text
            .lines()
            .find(|line| line.contains("<iidm:ratioTapChanger2"))
            .unwrap();
        assert!(tap.contains("regulating=\"false\""));
    }

    #[test]
    fn fresh_emission_orders_and_validates_tap_step_positions() {
        let mut network = parse_xiidm_source(EQUIPMENT_COVERAGE, &mut Diagnostics::new()).unwrap();
        let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        let tap = detailed
            .tap_changers
            .iter_mut()
            .find(|tap| tap.transformer.local_id() == "T3" && tap.winding == 2)
            .unwrap();
        let mut lower = tap.steps[0].clone();
        lower.position = -1;
        lower.rho = 0.95;
        let mut middle = tap.steps[0].clone();
        middle.position = 0;
        middle.rho = 1.0;
        let mut upper = tap.steps[0].clone();
        upper.position = 1;
        upper.rho = 1.05;
        tap.low_tap_position = -1;
        tap.tap_position = Some(0);
        tap.solved_tap_position = Some(1);
        tap.steps = vec![upper, lower, middle];

        let emission = write_xiidm(&network).unwrap();
        let reparsed = parse_xiidm_source(&emission.text, &mut Diagnostics::new()).unwrap();
        let tap = reparsed
            .detailed_connectivity()
            .as_ref()
            .unwrap()
            .tap_changers
            .iter()
            .find(|tap| tap.transformer.local_id() == "T3" && tap.winding == 2)
            .unwrap();
        assert_eq!(
            tap.steps
                .iter()
                .map(|step| (step.position, step.rho))
                .collect::<Vec<_>>(),
            vec![(-1, 0.95), (0, 1.0), (1, 1.05)]
        );
        assert_eq!(tap.tap_position, Some(0));
        assert_eq!(tap.solved_tap_position, Some(1));

        let mut noncontiguous = network.clone();
        let detailed = Arc::make_mut(noncontiguous.detailed_connectivity_mut().as_mut().unwrap());
        let tap = detailed
            .tap_changers
            .iter_mut()
            .find(|tap| tap.transformer.local_id() == "T3" && tap.winding == 2)
            .unwrap();
        tap.steps[0].position = 2;
        let error = write_xiidm(&noncontiguous).unwrap_err();
        assert!(error.to_string().contains("consecutive step positions"));
        assert!(
            error
                .to_string()
                .contains("position 2 where 1 was required")
        );

        let mut missing_assigned_step = network;
        let detailed = Arc::make_mut(
            missing_assigned_step
                .detailed_connectivity_mut()
                .as_mut()
                .unwrap(),
        );
        let tap = detailed
            .tap_changers
            .iter_mut()
            .find(|tap| tap.transformer.local_id() == "T3" && tap.winding == 2)
            .unwrap();
        tap.tap_position = Some(4);
        let error = write_xiidm(&missing_assigned_step).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("tapPosition 4 has no matching step")
        );
    }

    #[test]
    fn fresh_emission_rejects_constant_y_reactive_limits_on_all_xiidm_equipment() {
        let mut network = parse_xiidm_source(EQUIPMENT_COVERAGE, &mut Diagnostics::new()).unwrap();
        let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        let limits = detailed
            .equipment_reactive_limits
            .iter_mut()
            .find(|limits| limits.equipment.local_id() == "G")
            .unwrap();
        limits.limits = ReactiveLimits::CapabilityCurve(ReactiveCapabilityCurve {
            curve_style: CurveStyle::ConstantYValue,
            properties: BTreeMap::new(),
            points: vec![
                ReactiveCapabilityCurvePoint {
                    active_power_mw: 0.0,
                    minimum_reactive_power_mvar: -100.0,
                    maximum_reactive_power_mvar: 100.0,
                    properties: BTreeMap::new(),
                },
                ReactiveCapabilityCurvePoint {
                    active_power_mw: 200.0,
                    minimum_reactive_power_mvar: -50.0,
                    maximum_reactive_power_mvar: 50.0,
                    properties: BTreeMap::new(),
                },
            ],
        });

        let error = write_xiidm(&network).unwrap_err();
        assert!(error.to_string().contains("generator/G"));
        assert!(error.to_string().contains("CurveStyle.constantYValue"));
        assert!(error.to_string().contains("CurveStyle.straightLineYValues"));
    }

    #[test]
    fn network_properties_survive_fresh_emission() {
        let source = BUS_BREAKER.replacen(
            "  <iidm:substation",
            "  <iidm:property name=\"owner\" value=\"RTE\"/>\n  <iidm:substation",
            1,
        );
        let network = parse_xiidm_source(&source, &mut Diagnostics::new()).unwrap();
        let detailed = network.detailed_connectivity().as_ref().unwrap();
        let component = component_id("balanced_network", "case").unwrap();
        let metadata = component_metadata(detailed, &component).unwrap();
        assert_eq!(metadata.properties["owner"], "RTE");

        let emission = write_xiidm(&network).unwrap();
        assert!(
            emission
                .text
                .contains("  <iidm:property name=\"owner\" value=\"RTE\"/>")
        );
        let reparsed = parse_xiidm_source(&emission.text, &mut Diagnostics::new()).unwrap();
        let metadata = component_metadata(
            reparsed.detailed_connectivity().as_ref().unwrap(),
            &component,
        )
        .unwrap();
        assert_eq!(metadata.properties["owner"], "RTE");
    }

    #[test]
    fn empty_node_breaker_voltage_level_is_retained() {
        let source = r#"<?xml version="1.0" encoding="UTF-8"?>
<iidm:network xmlns:iidm="http://www.powsybl.org/schema/iidm/1_17" id="empty-level" caseDate="2026-01-01T00:00:00Z" forecastDistance="0" sourceFormat="test" minimumValidationLevel="STEADY_STATE_HYPOTHESIS">
  <iidm:voltageLevel id="VL" nominalV="400" topologyKind="NODE_BREAKER">
    <iidm:nodeBreakerTopology/>
  </iidm:voltageLevel>
</iidm:network>"#;
        let source =
            powerio_core::Source::from_memory("empty.xiidm", source.as_bytes().to_vec()).unwrap();
        let module = crate::format::parse(source).unwrap();
        assert!(module.value.buses().is_empty());
        assert_eq!(
            module
                .value
                .detailed_connectivity()
                .as_ref()
                .unwrap()
                .voltage_levels
                .len(),
            1
        );
        assert!(
            module
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message().contains("is empty"))
        );
    }

    #[test]
    fn fresh_emission_is_xiidm_1_17_and_reparses() {
        let network = parse_xiidm_source(BUS_BREAKER, &mut Diagnostics::new()).unwrap();
        let emission = write_xiidm(&network).unwrap();
        assert!(emission.text.contains(NAMESPACE));
        let reparsed = parse_xiidm_source(&emission.text, &mut Diagnostics::new()).unwrap();
        assert_eq!(reparsed.buses().len(), network.buses().len());
        assert_eq!(reparsed.loads().len(), network.loads().len());
        assert_eq!(reparsed.generators().len(), network.generators().len());
    }

    #[test]
    fn fresh_emission_preserves_line_transformer_and_hvdc_names() {
        let transformer = r#"    <iidm:twoWindingsTransformer id="T2" name="Named transformer" r="1" x="10" g="0" b="0" ratedU1="230" ratedU2="230" bus1="B1" connectableBus1="B1" voltageLevelId1="VL1" bus2="B2" connectableBus2="B2" voltageLevelId2="VL2"/>
"#;
        let source = BUS_BREAKER
            .replace(
                "  </iidm:substation>\n",
                &format!("{transformer}  </iidm:substation>\n"),
            )
            .replace(
                "<iidm:line id=\"LINE\"",
                "<iidm:line id=\"LINE\" name=\"Named line\"",
            );
        let network = parse_xiidm_source(&source, &mut Diagnostics::new()).unwrap();
        let emission = write_xiidm(&network).unwrap();
        assert!(
            emission
                .text
                .contains("<iidm:line id=\"LINE\" name=\"Named line\"")
        );
        assert!(
            emission
                .text
                .contains("<iidm:twoWindingsTransformer id=\"T2\" name=\"Named transformer\"")
        );

        let source = EQUIPMENT_COVERAGE.replace(
            "<iidm:hvdcLine id=\"DC\"",
            "<iidm:hvdcLine id=\"DC\" name=\"Named HVDC line\"",
        );
        let network = parse_xiidm_source(&source, &mut Diagnostics::new()).unwrap();
        let emission = write_xiidm(&network).unwrap();
        assert!(
            emission
                .text
                .contains("<iidm:hvdcLine id=\"DC\" name=\"Named HVDC line\"")
        );
    }

    #[test]
    fn fresh_emission_preserves_each_branch_terminal_connection() {
        let source = BUS_BREAKER.replace(
            "bus2=\"B2\" connectableBus2=\"B2\"",
            "connectableBus2=\"B2\"",
        );
        let network = parse_xiidm_source(&source, &mut Diagnostics::new()).unwrap();
        let emission = write_xiidm(&network).unwrap();
        let line = emission
            .text
            .lines()
            .find(|line| line.contains("<iidm:line"))
            .unwrap();
        assert!(line.contains("bus1=\"B1\""));
        assert!(line.contains("connectableBus1=\"B1\""));
        assert!(!line.contains("bus2=\"B2\""));
        assert!(line.contains("connectableBus2=\"B2\""));
    }

    #[test]
    fn partial_terminal_power_values_remain_partial() {
        let source = BUS_BREAKER.replace(
            "voltageLevelId2=\"VL2\"/>",
            "voltageLevelId2=\"VL2\" p1=\"12.5\"/>",
        );
        let network = parse_xiidm_source(&source, &mut Diagnostics::new()).unwrap();
        assert!(network.branches()[0].solution.is_none());
        let detailed = network.detailed_connectivity().as_ref().unwrap();
        let first = detailed
            .terminals
            .iter()
            .find(|terminal| terminal.equipment.local_id() == "LINE" && terminal.terminal == 1)
            .unwrap();
        assert_eq!(first.active_power_mw, Some(12.5));
        assert_eq!(first.reactive_power_mvar, None);
        let emission = write_xiidm(&network).unwrap();
        let line = emission
            .text
            .lines()
            .find(|line| line.contains("<iidm:line"))
            .unwrap();
        assert!(line.contains("p1=\"12.5\""));
        assert!(!line.contains("q1="));
        assert!(!line.contains("p2="));
        assert!(!line.contains("q2="));
    }

    #[test]
    fn absent_hvdc_converter_terminal_power_remains_absent() {
        let network = parse_xiidm_source(EQUIPMENT_COVERAGE, &mut Diagnostics::new()).unwrap();
        let detailed = network.detailed_connectivity().as_ref().unwrap();
        let second = detailed
            .terminals
            .iter()
            .find(|terminal| terminal.equipment.local_id() == "C2" && terminal.terminal == 1)
            .unwrap();
        assert_eq!(second.active_power_mw, None);
        assert_eq!(second.reactive_power_mvar, None);

        let emission = write_xiidm(&network).unwrap();
        let converter = emission
            .text
            .lines()
            .find(|line| line.contains("<iidm:vscConverterStation id=\"C2\""))
            .unwrap();
        assert!(!converter.contains(" p="));
        assert!(!converter.contains(" q="));

        let reparsed = parse_xiidm_source(&emission.text, &mut Diagnostics::new()).unwrap();
        let reparsed = reparsed.detailed_connectivity().as_ref().unwrap();
        let second = reparsed
            .terminals
            .iter()
            .find(|terminal| terminal.equipment.local_id() == "C2" && terminal.terminal == 1)
            .unwrap();
        assert_eq!(second.active_power_mw, None);
        assert_eq!(second.reactive_power_mvar, None);
    }

    #[test]
    fn iidm_is_input_only_and_normalizes_to_xiidm() {
        let source = powerio_core::Source::from_memory("case.xml", BUS_BREAKER.as_bytes().to_vec())
            .unwrap()
            .with_format(powerio_core::FormatId::new("iidm").unwrap());
        let module = crate::format::parse(source).unwrap();
        assert_eq!(module.value.source_format(), SourceFormat::Xiidm);
        assert_eq!(module.source().unwrap().format().unwrap().as_str(), "xiidm");
        assert_eq!(crate::format::parse_target_format("iidm"), None);
        assert_eq!(
            crate::format::parse_target_format("xiidm"),
            Some(crate::format::TargetFormat::Xiidm)
        );
    }

    #[test]
    fn xml_namespace_detects_xiidm_without_a_format_hint() {
        let source =
            powerio_core::Source::from_memory("case.xml", BUS_BREAKER.as_bytes().to_vec()).unwrap();
        let module = crate::format::parse(source).unwrap();
        assert_eq!(module.value.source_format(), SourceFormat::Xiidm);
        assert_eq!(module.source().unwrap().format().unwrap().as_str(), "xiidm");
    }

    #[test]
    fn node_breaker_fresh_emission_retains_detailed_topology() {
        let network = parse_xiidm_source(NODE_BREAKER, &mut Diagnostics::new()).unwrap();
        let emission = write_xiidm(&network).unwrap();
        let reparsed = parse_xiidm_source(&emission.text, &mut Diagnostics::new()).unwrap();
        let detailed = reparsed.detailed_connectivity().as_ref().unwrap();
        assert_eq!(detailed.connectivity_nodes.len(), 3);
        assert_eq!(detailed.busbar_sections.len(), 1);
        assert_eq!(detailed.switches.len(), 1);
        assert_eq!(detailed.internal_connections.len(), 1);
    }

    #[test]
    fn balanced_switches_survive_fresh_xiidm_without_detailed_connectivity() {
        let mut network = parse_xiidm_source(BUS_BREAKER, &mut Diagnostics::new()).unwrap();
        *network.detailed_connectivity_mut() = None;
        let from = network.buses()[0].id;
        let to = network.buses()[1].id;
        let mut switch = Switch::new(from, to, false);
        switch.uid = Some("coupler".into());
        switch.thermal_rating = Some(100.0);
        switch.current_rating = Some(500.0);
        switch.pf = Some(10.0);
        switch.qf = Some(2.0);
        switch.pt = Some(-9.8);
        switch.qt = Some(-1.9);
        network.switches_mut().push(switch);

        let emission = write_xiidm(&network).unwrap();
        assert!(emission.text.contains(
            "<iidm:switch id=\"coupler\" kind=\"BREAKER\" open=\"true\" retained=\"true\""
        ));
        assert!(emission.diagnostics.iter().any(|diagnostic| {
            diagnostic.code() == codes::EMIT_XIIDM.value_defaulted.code
                && diagnostic.message().contains("physical switch kind")
                && diagnostic.message().contains("breakers")
        }));
        let dropped = emission
            .diagnostics
            .iter()
            .find(|diagnostic| {
                diagnostic.code() == codes::EMIT_XIIDM.field_dropped.code
                    && diagnostic.message().contains("switch/coupler")
            })
            .expect("unrepresentable switch fields are diagnosed")
            .message();
        for field in [
            "thermal rating",
            "current rating",
            "from-side active power",
            "from-side reactive power",
            "to-side active power",
            "to-side reactive power",
        ] {
            assert!(dropped.contains(field), "missing `{field}` in `{dropped}`");
        }

        let reparsed = parse_xiidm_source(&emission.text, &mut Diagnostics::new()).unwrap();
        assert_eq!(reparsed.switches().len(), 1);
        assert_eq!(reparsed.switches()[0].uid.as_deref(), Some("coupler"));
        assert!(!reparsed.switches()[0].closed);
        let detailed = reparsed.detailed_connectivity().as_deref().unwrap();
        assert_eq!(detailed.switches.len(), 1);
        assert_eq!(detailed.switches[0].kind, SwitchKind::Breaker);
        assert!(detailed.switches[0].open);
    }

    #[test]
    fn fresh_emission_rejects_dangling_detailed_topology_references() {
        let mut network = parse_xiidm_source(NODE_BREAKER, &mut Diagnostics::new()).unwrap();
        let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        detailed.connectivity_nodes[0].voltage_level =
            component_id("voltage_level", "missing").unwrap();

        let error = write_xiidm(&network).unwrap_err().to_string();
        assert!(error.contains("network validation failed before XIIDM emission"));
        assert!(error.contains("references unknown voltage level"));
    }

    #[test]
    fn node_breaker_emission_keeps_a_disconnected_terminals_physical_node() {
        let mut network = parse_xiidm_source(NODE_BREAKER, &mut Diagnostics::new()).unwrap();
        let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        let generator_terminal = detailed
            .terminals
            .iter_mut()
            .find(|terminal| {
                terminal.equipment.component_type() == "generator"
                    && terminal.equipment.local_id() == "G"
            })
            .unwrap();
        generator_terminal.connected = false;
        let node_number = detailed
            .connectivity_nodes
            .iter()
            .find(|node| node.component == *generator_terminal.node.as_ref().unwrap())
            .and_then(|node| node.node_number)
            .unwrap();

        let emission = write_xiidm(&network).unwrap();
        let generator = emission
            .text
            .lines()
            .find(|line| line.contains("<iidm:generator id=\"G\""))
            .unwrap();
        assert!(generator.contains(&format!(" node=\"{node_number}\"")));
        parse_xiidm_source(&emission.text, &mut Diagnostics::new()).unwrap();
    }

    #[test]
    fn maps_three_winding_hvdc_and_operational_limits() {
        let mut diagnostics = Diagnostics::new();
        let network = parse_xiidm_source(EQUIPMENT_COVERAGE, &mut diagnostics).unwrap();
        let unmapped_limit_properties = diagnostics
            .records()
            .iter()
            .filter(|diagnostic| diagnostic.code() == "READ.XIIDM.FIELD_UNMAPPED")
            .map(powerio_core::Diagnostic::message)
            .filter(|message| message.contains("operational limits group `normal`"))
            .collect::<Vec<_>>();
        assert_eq!(unmapped_limit_properties.len(), 2);
        assert!(
            unmapped_limit_properties
                .iter()
                .any(|message| message.contains("`limit-set=seasonal` on `apparentPowerLimits`"))
        );
        assert!(
            unmapped_limit_properties
                .iter()
                .any(|message| message.contains("`cause=contingency` on `temporaryLimit`"))
        );
        let unmapped_fields = diagnostics
            .records()
            .iter()
            .filter(|diagnostic| diagnostic.code() == "READ.XIIDM.FIELD_UNMAPPED")
            .map(powerio_core::Diagnostic::message)
            .collect::<Vec<_>>();
        assert!(
            unmapped_fields
                .iter()
                .any(|message| message.contains("`tap-kind=ratio`")
                    && message.contains("`ratioTapChanger2`"))
        );
        assert!(
            unmapped_fields
                .iter()
                .any(|message| message.contains("`step-label=nominal`")
                    && message.contains("`step`"))
        );
        assert_eq!(network.branches().len(), 1);
        assert_eq!(network.transformers_3w().len(), 1);
        assert_eq!(network.hvdc().len(), 1);

        let branch = &network.branches()[0];
        assert_f64_close(branch.rate_a, 110.0);
        assert_f64_close(branch.rate_b, 140.0);
        assert_f64_close(branch.rate_c, 160.0);
        assert_f64_close(branch.current_ratings.unwrap().c_rating_a, 450.0);
        assert_eq!(branch.rating_sets.len(), 1);
        assert_eq!(branch.rating_sets[0].name, "summer");
        assert_f64_close(branch.rating_sets[0].rate_mva, 100.0);

        let transformer = &network.transformers_3w()[0];
        assert!((transformer.z[0].r - 0.11).abs() < 1e-12);
        assert!((transformer.windings[1].tap - 1.05).abs() < 1e-12);
        assert_f64_close(transformer.windings[0].rate_a, 90.0);

        let hvdc = &network.hvdc()[0];
        assert_f64_close(hvdc.pf, 100.0);
        assert_f64_close(hvdc.pmax, 150.0);
        assert!((hvdc.loss1 - 0.02).abs() < 1e-12);
        assert!(hvdc.pt < hvdc.pf);

        let emission = write_xiidm(&network).unwrap();
        assert!(emission.text.contains("<iidm:threeWindingsTransformer"));
        assert!(emission.text.contains("<iidm:vscConverterStation"));
        assert!(emission.text.contains("voltageRegulatorOn=\"false\""));
        assert!(emission.text.contains("<iidm:hvdcLine"));
        assert!(emission.text.contains("<iidm:operationalLimitsGroup1"));
        assert!(
            emission
                .text
                .contains("operationalLimitsGroup1 id=\"seasonal\"")
        );
        assert!(emission.text.contains("permanentLimitName=\"summer\""));
        assert!(
            emission
                .text
                .contains("selectedOperationalLimitsGroupIds1=\"normal\"")
        );
        assert!(
            !emission
                .text
                .contains("selectedOperationalLimitsGroupIds1=\"powerio\"")
        );
        assert!(!emission.text.contains("limit-set"));
        assert!(!emission.text.contains("contingency"));
        assert!(!emission.text.contains("tap-kind"));
        assert!(!emission.text.contains("step-label"));
        let reparsed = parse_xiidm_source(&emission.text, &mut Diagnostics::new()).unwrap();
        assert_eq!(reparsed.transformers_3w().len(), 1);
        assert_eq!(reparsed.hvdc().len(), 1);
        assert_f64_close(reparsed.branches()[0].rate_a, 110.0);
        assert_eq!(reparsed.branches()[0].rating_sets, branch.rating_sets);

        let mut without_hierarchy = network.clone();
        *without_hierarchy.detailed_connectivity_mut() = None;
        let derived = write_xiidm(&without_hierarchy).unwrap();
        assert!(derived.text.contains("<iidm:threeWindingsTransformer"));
        assert!(derived.text.contains("<iidm:hvdcLine"));
        let reparsed_derived = parse_xiidm_source(&derived.text, &mut Diagnostics::new()).unwrap();
        assert_eq!(reparsed_derived.transformers_3w().len(), 1);
        assert_eq!(reparsed_derived.hvdc().len(), 1);
    }

    #[test]
    fn xiidm_hvdc_emission_refuses_missing_required_values() {
        let network = parse_xiidm_source(EQUIPMENT_COVERAGE, &mut Diagnostics::new()).unwrap();

        let mut missing_nominal_voltage = network.clone();
        missing_nominal_voltage.hvdc_mut()[0].nominal_voltage_kv = None;
        let error = write_xiidm(&missing_nominal_voltage).unwrap_err();
        assert!(error.to_string().contains("nominalV"));

        let mut missing_resistance = network.clone();
        missing_resistance.hvdc_mut()[0].resistance_ohm = None;
        let error = write_xiidm(&missing_resistance).unwrap_err();
        assert!(error.to_string().contains("`r`"));

        let mut missing_mode = network.clone();
        missing_mode.hvdc_mut()[0].converters_mode = None;
        let error = write_xiidm(&missing_mode).unwrap_err();
        assert!(error.to_string().contains("convertersMode"));

        let mut missing_converter = network.clone();
        missing_converter.hvdc_mut()[0].converter1 = None;
        let error = write_xiidm(&missing_converter).unwrap_err();
        assert!(error.to_string().contains("converterStation1"));

        let mut missing_regulation_flag = network.clone();
        missing_regulation_flag.hvdc_mut()[0]
            .converter1
            .as_mut()
            .unwrap()
            .voltage_regulator_on = None;
        let error = write_xiidm(&missing_regulation_flag).unwrap_err();
        assert!(error.to_string().contains("voltageRegulatorOn"));

        let mut missing_power_factor = network;
        let converter = missing_power_factor.hvdc_mut()[0]
            .converter1
            .as_mut()
            .unwrap();
        converter.kind = HvdcConverterKind::Lcc;
        converter.power_factor = None;
        let error = write_xiidm(&missing_power_factor).unwrap_err();
        assert!(error.to_string().contains("powerFactor"));
    }

    #[test]
    fn xiidm_hvdc_emission_does_not_invent_optional_vsc_setpoints() {
        let mut network = parse_xiidm_source(EQUIPMENT_COVERAGE, &mut Diagnostics::new()).unwrap();
        let converter = network.hvdc_mut()[0].converter1.as_mut().unwrap();
        converter.voltage_setpoint_kv = None;
        converter.reactive_power_setpoint_mvar = None;

        let emission = write_xiidm(&network).unwrap();
        let station = emission
            .text
            .lines()
            .find(|line| line.contains("<iidm:vscConverterStation id=\"C1\""))
            .unwrap();
        assert!(station.contains("voltageRegulatorOn="));
        assert!(!station.contains("voltageSetpoint="));
        assert!(!station.contains("reactivePowerSetpoint="));
    }

    #[test]
    fn rejects_missing_selected_operational_limits_group() {
        let missing = EQUIPMENT_COVERAGE.replacen(
            "selectedOperationalLimitsGroupIds1=\"normal\"",
            "selectedOperationalLimitsGroupIds1=\"missing\"",
            1,
        );
        let error = parse_xiidm_source(&missing, &mut Diagnostics::new()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("missing operational limits group")
        );
    }

    #[test]
    fn absent_operational_limit_selection_does_not_select_a_group() {
        let source = EQUIPMENT_COVERAGE.replace(
            " selectedOperationalLimitsGroupIds1=\"normal\" selectedOperationalLimitsGroupIds2=\"normal\"",
            "",
        );
        let network = parse_xiidm_source(&source, &mut Diagnostics::new()).unwrap();
        assert_f64_close(network.branches()[0].rate_a, 0.0);
        let groups = &network
            .detailed_connectivity()
            .as_ref()
            .unwrap()
            .operational_limit_groups;
        assert!(
            groups
                .iter()
                .filter(|group| group.equipment.local_id() == "L")
                .all(|group| !group.selected)
        );
        let emission = write_xiidm(&network).unwrap();
        let line = emission
            .text
            .lines()
            .find(|line| line.contains("<iidm:line"))
            .unwrap();
        assert!(!line.contains("selectedOperationalLimitsGroupIds"));
    }

    #[test]
    fn maps_and_emits_xiidm_physical_dc_equipment() {
        let source = r#"<?xml version="1.0" encoding="UTF-8"?>
<iidm:network xmlns:iidm="http://www.powsybl.org/schema/iidm/1_17" id="dc" caseDate="2026-01-01T00:00:00Z" forecastDistance="0" sourceFormat="test" minimumValidationLevel="STEADY_STATE_HYPOTHESIS">
  <iidm:dcNode id="N1" name="first" fictitious="true" nominalV="500" v="498"><iidm:alias type="source">n-1</iidm:alias><iidm:property name="owner" value="RTE"/></iidm:dcNode>
  <iidm:dcNode id="N2" nominalV="500"/>
  <iidm:dcSwitch id="S" dcNode1="N1" dcNode2="N2" kind="DISCONNECTOR" open="true" r="0.9"/>
  <iidm:dcGround id="G" dcNode="N1" r="0.1" connected="false"/>
  <iidm:dcLine id="L" dcNode1="N1" dcNode2="N2" r="4" connected1="true" connected2="true" dcP1="100" dcI1="200" dcP2="-98" dcI2="-195"/>
</iidm:network>"#;
        let network = parse_xiidm_source(source, &mut Diagnostics::new()).unwrap();
        assert!(network.buses().is_empty());
        let detailed = network.detailed_connectivity().as_ref().unwrap();
        assert_eq!(detailed.dc_nodes.len(), 2);
        assert_eq!(detailed.dc_grounds.len(), 1);
        assert_eq!(detailed.dc_lines.len(), 1);
        assert_eq!(detailed.dc_switches.len(), 1);
        assert_eq!(detailed.dc_nodes[0].nominal_voltage_kv, Some(500.0));
        assert_eq!(detailed.dc_grounds[0].resistance_ohm, Some(0.1));
        assert_eq!(detailed.dc_lines[0].resistance_ohm, Some(4.0));
        assert_eq!(detailed.dc_switches[0].open, Some(true));
        assert_eq!(detailed.dc_lines[0].dc_terminal1.current_a, Some(200.0));
        let metadata = component_metadata(detailed, &detailed.dc_nodes[0].component).unwrap();
        assert!(metadata.fictitious);
        assert_eq!(metadata.aliases[0].value, "n-1");
        assert_eq!(metadata.properties["owner"], "RTE");

        let model_json = network.to_json().unwrap();
        let restored = BalancedNetwork::from_json(&model_json).unwrap();
        assert!(restored.buses().is_empty());
        assert_eq!(
            restored.detailed_connectivity().as_ref().unwrap().dc_nodes,
            detailed.dc_nodes
        );

        let emission = write_xiidm(&network).unwrap();
        assert!(emission.text.contains("<iidm:dcNode"));
        assert!(emission.text.contains("<iidm:dcSwitch"));
        assert!(emission.text.contains("<iidm:dcGround"));
        assert!(emission.text.contains("<iidm:dcLine"));
        let reparsed = parse_xiidm_source(&emission.text, &mut Diagnostics::new()).unwrap();
        let reparsed = reparsed.detailed_connectivity().as_ref().unwrap();
        assert_eq!(reparsed.dc_nodes, detailed.dc_nodes);
        assert_eq!(reparsed.dc_grounds, detailed.dc_grounds);
        assert_eq!(reparsed.dc_lines, detailed.dc_lines);
        assert_eq!(reparsed.dc_switches, detailed.dc_switches);

        let mut missing_nominal_voltage = network.clone();
        let detailed = std::sync::Arc::make_mut(
            missing_nominal_voltage
                .detailed_connectivity_mut()
                .as_mut()
                .unwrap(),
        );
        detailed.dc_nodes[0].nominal_voltage_kv = None;
        let error = write_xiidm(&missing_nominal_voltage).unwrap_err();
        assert!(matches!(error, Error::Emit { .. }));
        assert!(error.to_string().contains("nominalV"));
    }

    #[test]
    fn maps_and_emits_xiidm_physical_ac_dc_converters() {
        let source = r#"<?xml version="1.0" encoding="UTF-8"?>
<iidm:network xmlns:iidm="http://www.powsybl.org/schema/iidm/1_17" id="converters" caseDate="2026-01-01T00:00:00Z" forecastDistance="0" sourceFormat="test" minimumValidationLevel="STEADY_STATE_HYPOTHESIS">
  <iidm:dcNode id="N1" nominalV="500"/>
  <iidm:dcNode id="N2" nominalV="500"/>
  <iidm:substation id="S">
    <iidm:voltageLevel id="VL" nominalV="400" topologyKind="BUS_BREAKER">
      <iidm:busBreakerTopology><iidm:bus id="B1"/><iidm:bus id="B2"/></iidm:busBreakerTopology>
      <iidm:voltageSourceConverter id="VSC" dcNode1="N1" dcConnected1="true" dcNode2="N2" dcConnected2="false" idleLoss="2" switchingLoss="0.2" resistiveLoss="0.000002" controlMode="P_PCC_DROOP" targetP="301" targetVdc="502" bus1="B1" connectableBus1="B1" bus2="B2" connectableBus2="B2" p1="-100" q2="-200.8" dcP1="-102" dcI2="202" voltageRegulatorOn="true" voltageSetpoint="397">
        <iidm:pccTerminal id="VSC" number="ONE"/>
        <iidm:droopCurve><iidm:segment minV="-100" maxV="100" k="-5"/></iidm:droopCurve>
        <iidm:reactiveCapabilityCurve><iidm:property name="curve" value="retained"/><iidm:point p="-200" minQ="-190" maxQ="192"><iidm:property name="point" value="one"/></iidm:point><iidm:point p="200" minQ="-189" maxQ="191"/></iidm:reactiveCapabilityCurve>
      </iidm:voltageSourceConverter>
      <iidm:lineCommutatedConverter id="LCC" dcNode1="N1" dcConnected1="false" dcNode2="N2" dcConnected2="false" idleLoss="0" switchingLoss="0" resistiveLoss="0" controlMode="V_DC" targetVdc="502" connectableBus1="B1" reactiveModel="FIXED_POWER_FACTOR" powerFactor="0.92">
        <iidm:pccTerminal id="LCC" number="ONE"/>
      </iidm:lineCommutatedConverter>
    </iidm:voltageLevel>
  </iidm:substation>
</iidm:network>"#;
        let network = parse_xiidm_source(source, &mut Diagnostics::new()).unwrap();
        let detailed = network.detailed_connectivity().as_ref().unwrap();
        assert_eq!(detailed.voltage_source_converters.len(), 1);
        assert_eq!(detailed.line_commutated_converters.len(), 1);
        assert_eq!(detailed.terminals.len(), 3);
        let vsc = &detailed.voltage_source_converters[0];
        assert_eq!(
            vsc.control_mode,
            Some(AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroopCurve)
        );
        assert_eq!(vsc.voltage_regulator_on, Some(true));
        assert_eq!(vsc.dc_terminal1.active_power_mw, Some(-102.0));
        assert_eq!(vsc.dc_terminal2.current_a, Some(202.0));
        let Some(ReactiveLimits::CapabilityCurve(curve)) = &vsc.reactive_limits else {
            panic!("expected reactive capability curve");
        };
        assert_eq!(curve.curve_style, CurveStyle::StraightLineYValues);
        assert_eq!(curve.properties["curve"], "retained");
        assert_eq!(curve.points[0].properties["point"], "one");

        let emission = write_xiidm(&network).unwrap();
        assert!(emission.text.contains("<iidm:voltageSourceConverter"));
        assert!(emission.text.contains("<iidm:lineCommutatedConverter"));
        assert!(emission.text.contains("number=\"ONE\""));
        assert!(
            emission
                .text
                .lines()
                .filter(|line| line.contains("<iidm:bus id="))
                .all(|line| !line.contains(" v=") && !line.contains(" angle="))
        );
        let reparsed = parse_xiidm_source(&emission.text, &mut Diagnostics::new()).unwrap();
        let reparsed = reparsed.detailed_connectivity().as_ref().unwrap();
        assert_eq!(
            reparsed.voltage_source_converters,
            detailed.voltage_source_converters
        );
        assert_eq!(
            reparsed.line_commutated_converters,
            detailed.line_commutated_converters
        );

        let mut converter_limits = network.clone();
        let detailed = std::sync::Arc::make_mut(
            converter_limits
                .detailed_connectivity_mut()
                .as_mut()
                .unwrap(),
        );
        detailed.voltage_source_converters[0].minimum_active_power_mw = Some(-250.0);
        detailed.line_commutated_converters[0].maximum_active_power_mw = Some(250.0);
        let emission = write_xiidm(&converter_limits).unwrap();
        let dropped_limits = emission
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code() == codes::EMIT_XIIDM.field_dropped.code)
            .collect::<Vec<_>>();
        assert_eq!(dropped_limits.len(), 2);
        assert!(
            dropped_limits
                .iter()
                .any(|diagnostic| diagnostic.message().contains("VSC")
                    && diagnostic.message().contains("minimum active power"))
        );
        assert!(
            dropped_limits
                .iter()
                .any(|diagnostic| diagnostic.message().contains("LCC")
                    && diagnostic.message().contains("maximum active power"))
        );

        let mut constant_curve = network.clone();
        let detailed =
            std::sync::Arc::make_mut(constant_curve.detailed_connectivity_mut().as_mut().unwrap());
        let Some(ReactiveLimits::CapabilityCurve(curve)) = detailed.voltage_source_converters[0]
            .reactive_limits
            .as_mut()
        else {
            panic!("expected reactive capability curve");
        };
        curve.curve_style = CurveStyle::ConstantYValue;
        let error = write_xiidm(&constant_curve).unwrap_err();
        assert!(matches!(error, Error::Emit { .. }));
        assert!(error.to_string().contains("CurveStyle.constantYValue"));
        assert!(error.to_string().contains("CurveStyle.straightLineYValues"));

        let mut missing_reactive_limits = network.clone();
        let detailed = std::sync::Arc::make_mut(
            missing_reactive_limits
                .detailed_connectivity_mut()
                .as_mut()
                .unwrap(),
        );
        detailed.voltage_source_converters[0].reactive_limits = None;
        let error = write_xiidm(&missing_reactive_limits).unwrap_err();
        assert!(matches!(error, Error::Emit { .. }));
        assert!(error.to_string().contains("reactive limits"));
    }

    #[test]
    fn fresh_emission_does_not_invent_calculated_bus_solution_values() {
        let source = r#"<?xml version="1.0" encoding="UTF-8"?>
<iidm:network xmlns:iidm="http://www.powsybl.org/schema/iidm/1_17" id="calculated" caseDate="2026-01-01T00:00:00Z" forecastDistance="0" sourceFormat="test" minimumValidationLevel="STEADY_STATE_HYPOTHESIS">
  <iidm:substation id="S">
    <iidm:voltageLevel id="VL" nominalV="400" topologyKind="NODE_BREAKER">
      <iidm:nodeBreakerTopology>
        <iidm:busbarSection id="B1" node="0"/>
        <iidm:busbarSection id="B2" node="1"/>
        <iidm:bus nodes="0"><iidm:property name="calculated-source" value="study"/></iidm:bus>
      </iidm:nodeBreakerTopology>
    </iidm:voltageLevel>
  </iidm:substation>
</iidm:network>"#;
        let mut diagnostics = Diagnostics::new();
        let network = parse_xiidm_source(source, &mut diagnostics).unwrap();
        assert!(diagnostics.records().iter().any(|diagnostic| {
            diagnostic.code() == "READ.XIIDM.FIELD_UNMAPPED"
                && diagnostic.message().contains("`calculated-source=study`")
                && diagnostic.message().contains("`bus`")
        }));
        assert_eq!(network.buses().len(), 2);
        let detailed = network.detailed_connectivity().as_ref().unwrap();
        assert_eq!(detailed.calculated_buses.len(), 1);
        assert_eq!(detailed.calculated_buses[0].voltage_kv, None);
        assert_eq!(detailed.calculated_buses[0].angle_degrees, None);

        let emission = write_xiidm(&network).unwrap();
        let calculated = emission
            .text
            .lines()
            .filter(|line| line.contains("<iidm:bus nodes="))
            .collect::<Vec<_>>();
        assert_eq!(calculated.len(), 1);
        assert!(!calculated[0].contains(" v="));
        assert!(!calculated[0].contains(" angle="));
        assert!(!emission.text.contains("calculated-source"));
    }

    #[test]
    fn area_and_nonlinear_shunt_survive_fresh_emission() {
        let source = r#"<?xml version="1.0" encoding="UTF-8"?>
<iidm:network xmlns:iidm="http://www.powsybl.org/schema/iidm/1_17" id="area-shunt" caseDate="2026-01-01T00:00:00Z" forecastDistance="0" sourceFormat="test" minimumValidationLevel="STEADY_STATE_HYPOTHESIS">
  <iidm:substation id="S">
    <iidm:voltageLevel id="VL" nominalV="100" topologyKind="BUS_BREAKER">
      <iidm:busBreakerTopology><iidm:bus id="B"/></iidm:busBreakerTopology>
      <iidm:load id="L" loadType="UNDEFINED" p0="10" q0="2" bus="B" connectableBus="B"><iidm:zipModel c0p="1" c1p="0" c2p="0" c0q="1" c1q="0" c2q="0"><iidm:property name="load-model-source" value="study"/></iidm:zipModel></iidm:load>
      <iidm:shuntCompensator id="SH" sectionCount="2" voltageRegulatorOn="true" targetV="101" targetDeadband="2" bus="B" connectableBus="B">
        <iidm:shuntNonLinearModel>
          <iidm:property name="shunt-model-source" value="study"/>
          <iidm:section g="0.001" b="0.002"><iidm:property name="section-source" value="study"/></iidm:section>
          <iidm:section g="0.003" b="0.004"/>
        </iidm:shuntNonLinearModel>
        <iidm:regulatingTerminal id="L"/>
      </iidm:shuntCompensator>
    </iidm:voltageLevel>
  </iidm:substation>
  <iidm:area id="A7" name="control area" areaType="ControlArea" interchangeTarget="12.5">
    <iidm:voltageLevelRef id="VL"/>
  </iidm:area>
</iidm:network>"#;
        let mut diagnostics = Diagnostics::new();
        let network = parse_xiidm_source(source, &mut diagnostics).unwrap();
        for (parent, property) in [
            ("zipModel", "load-model-source=study"),
            ("shuntNonLinearModel", "shunt-model-source=study"),
            ("section", "section-source=study"),
        ] {
            assert!(diagnostics.records().iter().any(|diagnostic| {
                diagnostic.code() == "READ.XIIDM.FIELD_UNMAPPED"
                    && diagnostic.message().contains(&format!("`{parent}`"))
                    && diagnostic.message().contains(&format!("`{property}`"))
            }));
        }
        assert_eq!(network.areas().len(), 1);
        assert_eq!(network.areas()[0].number, 7);
        assert_eq!(network.areas()[0].uid.as_deref(), Some("A7"));
        assert_eq!(network.areas()[0].area_type.as_deref(), Some("ControlArea"));
        assert_f64_close(network.areas()[0].net_interchange, 12.5);
        assert_eq!(network.buses()[0].area, 7);
        let control = network.shunts()[0].control.as_ref().unwrap();
        assert_eq!(control.blocks.len(), 2);
        assert_eq!(
            control.blocks[0],
            ShuntBlock::with_admittance(1, 10.0, 20.0)
        );
        assert_eq!(
            control.blocks[1],
            ShuntBlock::with_admittance(1, 30.0, 40.0)
        );
        assert_eq!(
            control
                .regulating_terminal
                .as_ref()
                .map(|value| value.equipment.local_id()),
            Some("L")
        );

        let emission = write_xiidm(&network).unwrap();
        assert!(emission.text.contains("<iidm:shuntNonLinearModel>"));
        assert!(emission.text.contains("<iidm:area id=\"A7\""));
        assert!(emission.text.contains("<iidm:voltageLevelRef id=\"VL\"/>"));
        assert!(!emission.text.contains("load-model-source"));
        assert!(!emission.text.contains("shunt-model-source"));
        assert!(!emission.text.contains("section-source"));
        let reparsed = parse_xiidm_source(&emission.text, &mut Diagnostics::new()).unwrap();
        assert_eq!(reparsed.areas(), network.areas());
        assert_eq!(reparsed.shunts(), network.shunts());
    }

    #[test]
    fn boundary_and_tie_lines_survive_fresh_emission_without_duplicate_rows() {
        let source = r#"<?xml version="1.0" encoding="UTF-8"?>
<iidm:network xmlns:iidm="http://www.powsybl.org/schema/iidm/1_17" id="boundaries" caseDate="2026-01-01T00:00:00Z" forecastDistance="0" sourceFormat="test" minimumValidationLevel="STEADY_STATE_HYPOTHESIS">
  <iidm:substation id="S">
    <iidm:voltageLevel id="VL1" nominalV="100" topologyKind="BUS_BREAKER">
      <iidm:busBreakerTopology><iidm:bus id="B1"/></iidm:busBreakerTopology>
      <iidm:boundaryLine id="BL1" p0="1" q0="2" r="3" x="4" g="0.01" b="0.02" pairingKey="pair" bus="B1" connectableBus="B1" selectedOperationalLimitsGroupIds="&quot;not,A&quot;,plain">
        <iidm:operationalLimitsGroup id="not,A"><iidm:property name="owner" value="RTE"/><iidm:currentLimits permanentLimit="350"/></iidm:operationalLimitsGroup>
        <iidm:operationalLimitsGroup id="plain"><iidm:apparentPowerLimits permanentLimit="200"/></iidm:operationalLimitsGroup>
      </iidm:boundaryLine>
      <iidm:boundaryLine id="U" p0="5" q0="6" r="7" x="8" generationVoltageRegulationOn="true" generationMinP="0" generationMaxP="20" generationTargetP="10" generationTargetV="100" bus="B1" connectableBus="B1">
        <iidm:reactiveCapabilityCurve><iidm:property name="curve" value="exact"/><iidm:point p="0" minQ="-10" maxQ="10"/><iidm:point p="10" minQ="20" maxQ="0"/></iidm:reactiveCapabilityCurve>
      </iidm:boundaryLine>
    </iidm:voltageLevel>
    <iidm:voltageLevel id="VL2" nominalV="100" topologyKind="BUS_BREAKER">
      <iidm:busBreakerTopology><iidm:bus id="B2"/></iidm:busBreakerTopology>
      <iidm:boundaryLine id="BL2" p0="-1" q0="-2" r="9" x="10" g="0.03" b="0.04" pairingKey="pair" bus="B2" connectableBus="B2"/>
    </iidm:voltageLevel>
  </iidm:substation>
  <iidm:tieLine id="TL" name="tie" boundaryLineId1="BL1" boundaryLineId2="BL2"><iidm:alias type="source">tie-alias</iidm:alias></iidm:tieLine>
</iidm:network>"#;
        let mut diagnostics = Diagnostics::new();
        let network = parse_xiidm_source(source, &mut diagnostics).unwrap();
        let detailed = network.detailed_connectivity().as_ref().unwrap();
        assert_eq!(detailed.boundary_lines.len(), 3);
        assert_eq!(detailed.tie_lines.len(), 1);
        assert_eq!(network.loads().len(), 1);
        assert_eq!(network.generators().len(), 1);
        assert_eq!(network.branches().len(), 1);
        assert_f64_close(network.generators()[0].qmin, 10.0);
        assert_f64_close(network.generators()[0].qmax, 10.0);
        assert_eq!(
            detailed.operational_limit_groups[0].properties["owner"],
            "RTE"
        );

        let emission = write_xiidm(&network).unwrap();
        assert_eq!(emission.text.matches("<iidm:boundaryLine ").count(), 3);
        assert_eq!(emission.text.matches("<iidm:tieLine ").count(), 1);
        assert!(!emission.text.contains("<iidm:load id=\"U\""));
        assert!(!emission.text.contains("<iidm:generator id=\"U\""));
        assert!(!emission.text.contains("<iidm:line id=\"TL\""));
        assert!(
            emission
                .text
                .contains("selectedOperationalLimitsGroupIds=\"&quot;not,A&quot;,plain\"")
        );
        assert!(emission.text.contains("<iidm:reactiveCapabilityCurve>"));
        assert!(
            emission
                .text
                .contains("<iidm:property name=\"owner\" value=\"RTE\"/>")
        );

        let mut empty_pairing_key = network.clone();
        std::sync::Arc::make_mut(
            empty_pairing_key
                .detailed_connectivity_mut()
                .as_mut()
                .unwrap(),
        )
        .boundary_lines
        .iter_mut()
        .find(|boundary| boundary.component.local_id() == "U")
        .unwrap()
        .pairing_key = Some(String::new());
        let empty_pairing_key = write_xiidm(&empty_pairing_key).unwrap();
        let boundary = empty_pairing_key
            .text
            .lines()
            .find(|line| line.contains("<iidm:boundaryLine id=\"U\""))
            .unwrap();
        assert!(!boundary.contains("pairingKey="));

        let reparsed = parse_xiidm_source(&emission.text, &mut Diagnostics::new()).unwrap();
        let reparsed_detail = reparsed.detailed_connectivity().as_ref().unwrap();
        assert_eq!(reparsed_detail.boundary_lines, detailed.boundary_lines);
        assert_eq!(reparsed_detail.tie_lines, detailed.tie_lines);
        assert_eq!(
            reparsed_detail.operational_limit_groups,
            detailed.operational_limit_groups
        );
    }

    #[test]
    fn a_partial_boundary_generation_assignment_is_not_lost() {
        let source = BUS_BREAKER.replacen(
            "      <iidm:generator id=\"G1\"",
            "      <iidm:boundaryLine id=\"BL\" p0=\"0\" q0=\"0\" r=\"1\" x=\"2\" generationTargetQ=\"7\" bus=\"B1\" connectableBus=\"B1\"/>\n      <iidm:generator id=\"G1\"",
            1,
        );
        let network = parse_xiidm_source(&source, &mut Diagnostics::new()).unwrap();
        let boundary = network
            .detailed_connectivity()
            .as_ref()
            .unwrap()
            .boundary_lines
            .iter()
            .find(|boundary| boundary.component.local_id() == "BL")
            .unwrap();
        let generation = boundary.generation.as_ref().unwrap();
        assert_eq!(generation.target_reactive_power_mvar, Some(7.0));
        assert!(!generation.voltage_regulation_on);
        let generator = network
            .generators()
            .iter()
            .find(|generator| generator.uid.as_deref() == Some("BL"))
            .unwrap();
        assert_f64_close(generator.qg, 7.0);

        let emission = write_xiidm(&network).unwrap();
        let boundary = emission
            .text
            .lines()
            .find(|line| line.contains("<iidm:boundaryLine id=\"BL\""))
            .unwrap();
        assert!(boundary.contains("generationTargetQ=\"7\""));
    }

    #[test]
    fn diagnoses_voltage_ratio_three_winding_shunt_and_zero_zip_projection() {
        let two_winding = BUS_BREAKER.replacen(
            "  </iidm:substation>",
            "    <iidm:twoWindingsTransformer id=\"T\" r=\"1\" x=\"10\" g=\"0\" b=\"0\" ratedU1=\"230\" ratedU2=\"115\" bus1=\"B1\" connectableBus1=\"B1\" voltageLevelId1=\"VL1\" bus2=\"B2\" connectableBus2=\"B2\" voltageLevelId2=\"VL2\"/>\n  </iidm:substation>",
            1,
        );
        let mut diagnostics = Diagnostics::new();
        parse_xiidm_source(&two_winding, &mut diagnostics).unwrap();
        assert!(diagnostics.lines().iter().any(|line| {
            line.contains("two winding transformer `T`")
                && line.contains("ratedU2=115")
                && line.contains("normalizes ratedU2")
        }));

        let three_winding = EQUIPMENT_COVERAGE
            .replace("ratedU0=\"132\"", "ratedU0=\"130\"")
            .replace("r2=\"1.7424\"", "g2=\"0.001\" r2=\"1.7424\"");
        let mut diagnostics = Diagnostics::new();
        let network = parse_xiidm_source(&three_winding, &mut diagnostics).unwrap();
        let transformer = network
            .transformers_3w()
            .iter()
            .find(|transformer| transformer.uid.as_deref() == Some("T3"))
            .unwrap();
        assert_eq!(
            transformer
                .extras
                .get(XIIDM_RATED_U0_EXTRA)
                .and_then(serde_json::Value::as_f64),
            Some(130.0)
        );
        assert!(
            !diagnostics
                .lines()
                .iter()
                .any(|line| line.contains("ratedU0"))
        );
        assert!(diagnostics.lines().iter().any(|line| {
            line.contains("three winding transformer `T3`")
                && line.contains("leg shunt admittances")
        }));

        let zero_zip = BUS_BREAKER.replace(
            "<iidm:load id=\"L1\" loadType=\"UNDEFINED\" p0=\"90\" q0=\"30\" bus=\"B2\" connectableBus=\"B2\"/>",
            "<iidm:load id=\"L1\" loadType=\"UNDEFINED\" p0=\"0\" q0=\"0\" bus=\"B2\" connectableBus=\"B2\"><iidm:zipModel c0p=\"1\" c1p=\"0\" c2p=\"0\" c0q=\"1\" c1q=\"0\" c2q=\"0\"/></iidm:load>",
        );
        let mut diagnostics = Diagnostics::new();
        let network = parse_xiidm_source(&zero_zip, &mut diagnostics).unwrap();
        assert!(diagnostics.lines().iter().any(|line| {
            line.contains("load `L1` has zero p0") && line.contains("ZIP coefficients")
        }));
        assert!(diagnostics.lines().iter().any(|line| {
            line.contains("load `L1` has zero q0") && line.contains("ZIP coefficients")
        }));
        let emission = write_xiidm(&network).unwrap();
        assert!(emission.text.contains(
            "<iidm:zipModel c0p=\"0\" c1p=\"0\" c2p=\"0\" c0q=\"0\" c1q=\"0\" c2q=\"0\"/>"
        ));
    }

    #[test]
    fn one_level_subnetworks_retain_parent_metadata_and_component_containment() {
        let source = r#"<?xml version="1.0" encoding="UTF-8"?>
<iidm:network xmlns:iidm="http://www.powsybl.org/schema/iidm/1_17" id="Merged" caseDate="2026-01-01T00:00:00Z" forecastDistance="0" sourceFormat="root" minimumValidationLevel="STEADY_STATE_HYPOTHESIS">
  <iidm:network id="A" name="first" caseDate="2026-01-01T01:00:00Z" forecastDistance="1" sourceFormat="part-a" minimumValidationLevel="STEADY_STATE_HYPOTHESIS">
    <iidm:property name="owner" value="RTE"/>
    <iidm:substation id="SA"><iidm:voltageLevel id="VA" nominalV="100" topologyKind="BUS_BREAKER"><iidm:busBreakerTopology><iidm:bus id="BA"/></iidm:busBreakerTopology><iidm:boundaryLine id="DLA" p0="0" q0="0" r="1" x="2" bus="BA" connectableBus="BA"/></iidm:voltageLevel></iidm:substation>
  </iidm:network>
  <iidm:network id="B" caseDate="2026-01-01T02:00:00Z" forecastDistance="2" sourceFormat="part-b" minimumValidationLevel="STEADY_STATE_HYPOTHESIS">
    <iidm:substation id="SB"><iidm:voltageLevel id="VB" nominalV="100" topologyKind="BUS_BREAKER"><iidm:busBreakerTopology><iidm:bus id="BB"/></iidm:busBreakerTopology><iidm:boundaryLine id="DLB" p0="0" q0="0" r="3" x="4" bus="BB" connectableBus="BB"/></iidm:voltageLevel></iidm:substation>
  </iidm:network>
  <iidm:tieLine id="TL" boundaryLineId1="DLA" boundaryLineId2="DLB"/>
</iidm:network>"#;
        let network = parse_xiidm_source(source, &mut Diagnostics::new()).unwrap();
        let detailed = network.detailed_connectivity().as_ref().unwrap();
        assert_eq!(detailed.subnetworks.len(), 2);
        let root = component_id("balanced_network", "Merged").unwrap();
        assert!(
            detailed
                .subnetworks
                .iter()
                .all(|value| value.parent == root)
        );
        let first = &detailed.subnetworks[0];
        assert_eq!(first.component, component_id("subnetwork", "A").unwrap());
        assert_eq!(
            first.case_metadata.source_model_format.as_deref(),
            Some("part-a")
        );
        assert!(
            first
                .components
                .contains(&component_id("substation", "SA").unwrap())
        );
        assert!(
            first
                .components
                .contains(&component_id("voltage_level", "VA").unwrap())
        );
        assert!(
            first
                .components
                .contains(&component_id("bus", "BA").unwrap())
        );
        assert!(
            first
                .components
                .contains(&component_id("boundary_line", "DLA").unwrap())
        );
        let metadata = component_metadata(detailed, &first.component).unwrap();
        assert_eq!(metadata.name.as_deref(), Some("first"));
        assert_eq!(metadata.properties["owner"], "RTE");

        let emission = write_xiidm(&network).unwrap();
        assert_eq!(emission.text.matches("<iidm:network id=\"").count(), 2);
        assert_eq!(emission.text.matches("<iidm:substation ").count(), 2);
        assert_eq!(emission.text.matches("<iidm:tieLine ").count(), 1);
        let reparsed = parse_xiidm_source(&emission.text, &mut Diagnostics::new()).unwrap();
        let reparsed = reparsed.detailed_connectivity().as_ref().unwrap();
        assert_eq!(reparsed.subnetworks, detailed.subnetworks);
        assert_eq!(reparsed.boundary_lines, detailed.boundary_lines);
        assert_eq!(reparsed.tie_lines, detailed.tie_lines);

        let nested = source.replacen(
            "<iidm:substation id=\"SA\">",
            "<iidm:network id=\"nested\" caseDate=\"2026-01-01T03:00:00Z\" forecastDistance=\"0\" sourceFormat=\"nested\" minimumValidationLevel=\"EQUIPMENT\"><iidm:substation id=\"SA\">",
            1,
        );
        let error = parse_xiidm_source(&nested, &mut Diagnostics::new()).unwrap_err();
        assert!(error.to_string().contains("only one level"));
    }
}
