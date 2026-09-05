//! PowerIO C ABI, version 7.
//!
//! The ABI exposes the same four representation operations as Rust:
//! [`pio_parse`], [`pio_emit`], [`pio_module_serialize`], and
//! [`pio_module_deserialize`]. Files and memory enter through [`PioSource`];
//! files, directories, and memory output use [`PioDestination`]. Values are
//! identified by canonical structural type names rather than an ordinal enum.

#![allow(clippy::missing_safety_doc)]

use std::ffi::c_char;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::Arc;

use powerio::{
    BalancedNetwork, Destination, Diagnostic, EmitResult, EmittedOutput, PioScenarioSet,
    PioTimeSeries, PioValue, Source,
};
use powerio_core::{ComponentId, HistoryEntry, HistoryId, HistoryKind, Producer};
use powerio_matrix::{
    AcOpfAssemblyOptions, AcOpfPreparation, AnalysisBranchSource, DcOperators,
    DcOpfAssemblyOptions, DcOpfPreparation, PreparedObjective, SparseMatrix, Units,
    build_ac_opf_preparation, build_dc_opf_preparation,
};
use powerio_prob::{
    ActivePower, ActivePowerUnit, ApparentPower, ApparentPowerUnit, CalculationUpdate,
    DcPfInstance, LoadAllocation, NetworkUpdate, OperatingPointUpdate, ReactivePower,
    ReactivePowerUnit, UpdateChange, UpdatedField, apply_bus_load_active_power, apply_updates,
};
use powerio_tx::BranchSusceptanceFormula;

use crate::diagnostics::codes;

pub mod diagnostics;

/// C ABI version.
pub const PIO_ABI_VERSION: u32 = 7;

// ---- views -----------------------------------------------------------------

/// Borrowed UTF-8 bytes. The bytes need not end in NUL.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioStringView {
    pub data: *const c_char,
    pub len: usize,
}

impl PioStringView {
    const EMPTY: Self = Self {
        data: std::ptr::null(),
        len: 0,
    };

    fn new(text: &str) -> Self {
        Self {
            data: text.as_ptr().cast(),
            len: text.len(),
        }
    }
}

/// One borrowed source byte range from a diagnostic.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioDiagnosticSpanView {
    pub source: PioStringView,
    pub byte_start: u64,
    pub byte_end: u64,
}

/// Program identity recorded with one module.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioModuleProducerView {
    pub name: PioStringView,
    pub version: PioStringView,
}

/// One durable source descriptor recorded with a module.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioModuleSourceView {
    pub id: PioStringView,
    pub name: PioStringView,
    pub byte_length: u64,
    pub format: PioStringView,
    pub has_format: bool,
    pub digest_algorithm: PioStringView,
    pub digest: PioStringView,
    pub has_digest: bool,
}

/// One borrowed source byte range from a source map entry.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioSourceSpanView {
    pub source: PioStringView,
    pub byte_start: u64,
    pub byte_end: u64,
}

/// One typed value target and its relation to source bytes.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioModuleSourceMapEntryView {
    pub target: PioStringView,
    pub relation: PioStringView,
    pub span_count: usize,
}

/// One operation recorded in module history.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioModuleHistoryEntryView {
    pub id: PioStringView,
    pub kind: PioStringView,
    pub name: PioStringView,
    pub input_type: PioStringView,
    pub has_input_type: bool,
    pub output_type: PioStringView,
    pub has_output_type: bool,
    pub parameter_count: usize,
    pub assumption_count: usize,
    pub loss_count: usize,
}

/// One named structured parameter in a module history entry.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioModuleHistoryParameterView {
    pub name: PioStringView,
    pub value_kind: PioStringView,
}

/// One namespaced structured module extension.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioModuleExtensionView {
    pub namespace: PioStringView,
    pub value_kind: PioStringView,
}

/// One structured JSON value stored in module history or extensions.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioJsonValueView {
    pub kind: PioStringView,
    pub boolean_value: bool,
    pub number_kind: PioStringView,
    pub signed_integer_value: i64,
    pub unsigned_integer_value: u64,
    pub floating_point_value: f64,
    pub string_value: PioStringView,
    pub element_count: usize,
}

/// One key and value type in a structured JSON object.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioJsonObjectEntryView {
    pub key: PioStringView,
    pub value_kind: PioStringView,
}

/// Borrowed binary bytes.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioByteView {
    pub data: *const u8,
    pub len: usize,
}

impl PioByteView {
    const EMPTY: Self = Self {
        data: std::ptr::null(),
        len: 0,
    };

    fn new(bytes: &[u8]) -> Self {
        Self {
            data: bytes.as_ptr(),
            len: bytes.len(),
        }
    }
}

/// Borrowed `double` values.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioF64View {
    pub data: *const f64,
    pub len: usize,
}

impl PioF64View {
    const EMPTY: Self = Self {
        data: std::ptr::null(),
        len: 0,
    };

    fn new(values: &[f64]) -> Self {
        Self {
            data: values.as_ptr(),
            len: values.len(),
        }
    }
}

/// Borrowed `size_t` values.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioSizeView {
    pub data: *const usize,
    pub len: usize,
}

impl PioSizeView {
    const EMPTY: Self = Self {
        data: std::ptr::null(),
        len: 0,
    };

    fn new(values: &[usize]) -> Self {
        Self {
            data: values.as_ptr(),
            len: values.len(),
        }
    }
}

/// One point in a balanced network coordinate space.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioBalancedLocationView {
    pub x: f64,
    pub y: f64,
    pub kind: PioStringView,
    pub has_kind: bool,
}

/// Coordinate metadata for a balanced network.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioBalancedGeoView {
    pub has_geo: bool,
    pub space: PioStringView,
    pub crs: PioStringView,
    pub has_crs: bool,
    pub kind: PioStringView,
    pub has_kind: bool,
    pub has_canvas: bool,
    pub canvas_width: f64,
    pub has_canvas_width: bool,
    pub canvas_height: f64,
    pub has_canvas_height: bool,
    pub canvas_units: PioStringView,
    pub has_canvas_units: bool,
}

/// One balanced bus. String and coefficient spans borrow from the network.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioBalancedBusView {
    pub component_id: PioStringView,
    pub has_component_id: bool,
    pub id: usize,
    pub bus_type: PioStringView,
    pub vm_pu: f64,
    pub va_degrees: f64,
    pub base_kv: f64,
    pub vmax_pu: f64,
    pub vmin_pu: f64,
    pub has_emergency_voltage_limits: bool,
    pub emergency_vmax_pu: f64,
    pub emergency_vmin_pu: f64,
    pub area: usize,
    pub zone: usize,
    pub name: PioStringView,
    pub has_name: bool,
    pub location: PioBalancedLocationView,
    pub has_location: bool,
}

/// Voltage dependence attached to one balanced load.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioBalancedLoadVoltageModelView {
    pub kind: PioStringView,
    pub p_constant_power_mw: f64,
    pub q_constant_power_mvar: f64,
    pub p_constant_current_mw: f64,
    pub q_constant_current_mvar: f64,
    pub p_constant_impedance_mw: f64,
    pub q_constant_impedance_mvar: f64,
    pub exponential_p_mw: f64,
    pub exponential_q_mvar: f64,
    pub gamma_p: f64,
    pub gamma_q: f64,
    pub nominal_voltage_pu: f64,
    pub has_nominal_voltage: bool,
    pub load_type: i32,
    pub has_load_type: bool,
    pub scaling: f64,
    pub has_scaling: bool,
}

/// One balanced load.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioBalancedLoadView {
    pub component_id: PioStringView,
    pub has_component_id: bool,
    pub bus_id: usize,
    pub p_mw: f64,
    pub q_mvar: f64,
    pub in_service: bool,
    pub voltage_model: PioBalancedLoadVoltageModelView,
}

/// One switched shunt block.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioShuntBlockView {
    pub steps: u32,
    pub conductance_mw: f64,
    pub susceptance_mvar: f64,
}

/// One balanced shunt.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioBalancedShuntView {
    pub component_id: PioStringView,
    pub has_component_id: bool,
    pub bus_id: usize,
    pub conductance_mw: f64,
    pub susceptance_mvar: f64,
    pub in_service: bool,
    pub section_count: u32,
    pub has_section_count: bool,
    pub has_control: bool,
    pub control_mode: PioStringView,
    pub control_vmax_pu: f64,
    pub control_vmin_pu: f64,
    pub control_bus_id: usize,
    pub has_control_bus: bool,
    pub control_reactive_range_percent: f64,
    pub control_block_count: usize,
}

/// One named branch MVA rating beyond rating A, B, and C.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioBranchRatingView {
    pub name: PioStringView,
    pub rate_mva: f64,
}

/// One balanced branch or two winding transformer.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioBalancedBranchView {
    pub component_id: PioStringView,
    pub has_component_id: bool,
    pub name: PioStringView,
    pub has_name: bool,
    pub from_bus_id: usize,
    pub to_bus_id: usize,
    pub resistance_pu: f64,
    pub reactance_pu: f64,
    pub total_charging_susceptance_pu: f64,
    pub terminal_charging_is_explicit: bool,
    pub from_conductance_pu: f64,
    pub from_susceptance_pu: f64,
    pub to_conductance_pu: f64,
    pub to_susceptance_pu: f64,
    pub rate_a_mva: f64,
    pub rate_b_mva: f64,
    pub rate_c_mva: f64,
    pub additional_rating_count: usize,
    pub has_current_ratings: bool,
    pub current_rating_a: f64,
    pub current_rating_b: f64,
    pub current_rating_c: f64,
    pub tap_ratio: f64,
    pub effective_tap_ratio: f64,
    pub phase_shift_degrees: f64,
    pub in_service: bool,
    pub angle_min_degrees: f64,
    pub angle_max_degrees: f64,
    pub control: PioTransformerControlView,
    pub has_control: bool,
    pub route_point_count: usize,
    pub has_route: bool,
}

/// One generator cost curve.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioGeneratorCostView {
    pub model: u8,
    pub startup: f64,
    pub shutdown: f64,
    pub ncost: usize,
    pub coefficients: PioF64View,
}

/// One optional generator capability or ramp field.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioGeneratorCapabilityView {
    pub name: PioStringView,
    pub value: f64,
    pub has_value: bool,
}

/// Governor and distributed slack settings for a generator or storage element.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioActivePowerControlView {
    pub participate: bool,
    pub droop_percent: f64,
    pub has_droop_percent: bool,
    pub participation_factor: f64,
    pub has_participation_factor: bool,
    pub minimum_target_active_power_mw: f64,
    pub has_minimum_target_active_power: bool,
    pub maximum_target_active_power_mw: f64,
    pub has_maximum_target_active_power: bool,
}

/// One balanced generator.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioBalancedGeneratorView {
    pub component_id: PioStringView,
    pub has_component_id: bool,
    pub bus_id: usize,
    pub energy_source: PioStringView,
    pub active_power_mw: f64,
    pub reactive_power_mvar: f64,
    pub active_power_max_mw: f64,
    pub active_power_min_mw: f64,
    pub reactive_power_max_mvar: f64,
    pub reactive_power_min_mvar: f64,
    pub voltage_setpoint_pu: f64,
    pub machine_base_mva: f64,
    pub in_service: bool,
    pub has_cost: bool,
    pub cost: PioGeneratorCostView,
    pub regulated_bus_id: usize,
    pub has_regulated_bus: bool,
    pub capability_count: usize,
    pub active_power_control: PioActivePowerControlView,
    pub has_active_power_control: bool,
    pub voltage_regulation_on: bool,
    pub regulating_terminal: PioTerminalReferenceView,
    pub has_regulating_terminal: bool,
}

/// One balanced storage element.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioBalancedStorageView {
    pub component_id: PioStringView,
    pub has_component_id: bool,
    pub bus_id: usize,
    pub active_power_mw: f64,
    pub reactive_power_mvar: f64,
    pub energy_mwh: f64,
    pub energy_rating_mwh: f64,
    pub charge_rating_mw: f64,
    pub discharge_rating_mw: f64,
    pub charge_efficiency: f64,
    pub discharge_efficiency: f64,
    pub thermal_rating_mva: f64,
    pub current_rating: f64,
    pub has_current_rating: bool,
    pub reactive_power_min_mvar: f64,
    pub reactive_power_max_mvar: f64,
    pub resistance_pu: f64,
    pub reactance_pu: f64,
    pub active_power_loss_mw: f64,
    pub reactive_power_loss_mvar: f64,
    pub in_service: bool,
    pub active_power_control: PioActivePowerControlView,
    pub has_active_power_control: bool,
}

/// Borrowed reference to one numbered equipment terminal.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioTerminalReferenceView {
    pub equipment: PioComponentIdView,
    pub terminal: u8,
}

/// Automatic transformer tap or phase control.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioTransformerControlView {
    pub mode: PioStringView,
    pub enabled: bool,
    pub controlled_bus_id: usize,
    pub has_controlled_bus: bool,
    pub controlled_bus_on_winding_side: bool,
    pub regulating_terminal: PioTerminalReferenceView,
    pub has_regulating_terminal: bool,
    pub tap_min: f64,
    pub tap_max: f64,
    pub band_min: f64,
    pub band_max: f64,
    pub tap_position_count: u32,
    pub mva_base: f64,
    pub winding_connection_angle: f64,
    pub has_winding_connection_angle: bool,
}

/// One balanced static VAR compensator.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioBalancedStaticVarCompensatorView {
    pub component_id: PioStringView,
    pub has_component_id: bool,
    pub bus_id: usize,
    pub minimum_susceptance_siemens: f64,
    pub maximum_susceptance_siemens: f64,
    pub voltage_setpoint_kv: f64,
    pub reactive_power_setpoint_mvar: f64,
    pub regulation_mode: PioStringView,
    pub regulating: bool,
    pub regulating_terminal: PioTerminalReferenceView,
    pub has_regulating_terminal: bool,
    pub active_power_mw: f64,
    pub reactive_power_mvar: f64,
    pub in_service: bool,
}

/// One balanced transmission switch.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioBalancedSwitchView {
    pub component_id: PioStringView,
    pub has_component_id: bool,
    pub from_bus_id: usize,
    pub to_bus_id: usize,
    pub closed: bool,
    pub thermal_rating_mva: f64,
    pub has_thermal_rating: bool,
    pub current_rating_a: f64,
    pub has_current_rating: bool,
    pub from_active_power_mw: f64,
    pub has_from_active_power: bool,
    pub from_reactive_power_mvar: f64,
    pub has_from_reactive_power: bool,
    pub to_active_power_mw: f64,
    pub has_to_active_power: bool,
    pub to_reactive_power_mvar: f64,
    pub has_to_reactive_power: bool,
}

/// One AC terminal converter station of a balanced HVDC line.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioBalancedHvdcConverterView {
    pub component: PioComponentIdView,
    pub kind: PioStringView,
    pub loss_factor_percent: f64,
    pub voltage_regulator_on: bool,
    pub has_voltage_regulator_on: bool,
    pub voltage_setpoint_kv: f64,
    pub has_voltage_setpoint: bool,
    pub reactive_power_setpoint_mvar: f64,
    pub has_reactive_power_setpoint: bool,
    pub power_factor: f64,
    pub has_power_factor: bool,
    pub regulating_terminal: PioTerminalReferenceView,
    pub has_regulating_terminal: bool,
}

/// One balanced two terminal HVDC line.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioBalancedHvdcView {
    pub component_id: PioStringView,
    pub has_component_id: bool,
    pub from_bus_id: usize,
    pub to_bus_id: usize,
    pub in_service: bool,
    pub from_active_power_mw: f64,
    pub to_active_power_mw: f64,
    pub from_reactive_power_mvar: f64,
    pub to_reactive_power_mvar: f64,
    pub from_voltage_pu: f64,
    pub to_voltage_pu: f64,
    pub minimum_active_power_mw: f64,
    pub maximum_active_power_mw: f64,
    pub minimum_from_reactive_power_mvar: f64,
    pub maximum_from_reactive_power_mvar: f64,
    pub minimum_to_reactive_power_mvar: f64,
    pub maximum_to_reactive_power_mvar: f64,
    pub constant_loss_mw: f64,
    pub proportional_loss: f64,
    pub resistance_ohm: f64,
    pub has_resistance: bool,
    pub nominal_voltage_kv: f64,
    pub has_nominal_voltage: bool,
    pub converters_mode: PioStringView,
    pub has_converters_mode: bool,
    pub converter1: PioBalancedHvdcConverterView,
    pub has_converter1: bool,
    pub converter2: PioBalancedHvdcConverterView,
    pub has_converter2: bool,
    pub cost: PioGeneratorCostView,
    pub has_cost: bool,
}

/// One winding of a balanced three winding transformer.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioThreeWindingTransformerWindingView {
    pub bus_id: usize,
    pub tap_ratio: f64,
    pub phase_shift_degrees: f64,
    pub nominal_voltage_kv: f64,
    pub rating_a_mva: f64,
    pub rating_b_mva: f64,
    pub rating_c_mva: f64,
    pub control: PioTransformerControlView,
    pub has_control: bool,
}

/// One pairwise impedance of a balanced three winding transformer.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioThreeWindingTransformerImpedanceView {
    pub resistance_pu: f64,
    pub reactance_pu: f64,
    pub base_mva: f64,
}

/// One balanced three winding transformer.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioBalancedThreeWindingTransformerView {
    pub component_id: PioStringView,
    pub has_component_id: bool,
    pub name: PioStringView,
    pub has_name: bool,
    pub winding_count: usize,
    pub impedance_count: usize,
    pub star_voltage_magnitude_pu: f64,
    pub star_voltage_angle_degrees: f64,
    pub magnetizing_conductance_pu: f64,
    pub magnetizing_susceptance_pu: f64,
    pub in_service: bool,
}

/// One balanced control area.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioBalancedAreaView {
    pub number: usize,
    pub slack_bus_id: usize,
    pub has_slack_bus: bool,
    pub net_interchange_mw: f64,
    pub tolerance_mw: f64,
    pub name: PioStringView,
    pub has_name: bool,
    pub component_id: PioStringView,
    pub has_component_id: bool,
    pub area_type: PioStringView,
    pub has_area_type: bool,
}

/// Exact table lengths in source neutral detailed connectivity.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioDetailedConnectivityCountsView {
    pub omitted_fields: usize,
    pub component_metadata: usize,
    pub subnetworks: usize,
    pub substations: usize,
    pub voltage_levels: usize,
    pub bus_breaker_buses: usize,
    pub calculated_buses: usize,
    pub connectivity_nodes: usize,
    pub busbar_sections: usize,
    pub junctions: usize,
    pub terminals: usize,
    pub switches: usize,
    pub internal_connections: usize,
    pub operational_limit_groups: usize,
    pub tap_changers: usize,
    pub equipment_reactive_limits: usize,
    pub boundary_lines: usize,
    pub tie_lines: usize,
    pub dc_converter_units: usize,
    pub dc_topological_nodes: usize,
    pub dc_nodes: usize,
    pub dc_grounds: usize,
    pub dc_busbars: usize,
    pub dc_lines: usize,
    pub dc_series_devices: usize,
    pub dc_switches: usize,
    pub voltage_source_converters: usize,
    pub line_commutated_converters: usize,
}

/// One source field that was absent rather than explicitly assigned a value.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioOmittedFieldView {
    pub component: PioComponentIdView,
    pub field: PioStringView,
}

/// Reactive limits retained for one equipment record.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioEquipmentReactiveLimitsView {
    pub equipment: PioComponentIdView,
    pub limits: PioReactiveLimitsView,
}

/// Source neutral case metadata attached to one subnetwork.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioCaseMetadataView {
    pub case_date: PioStringView,
    pub has_case_date: bool,
    pub forecast_distance: i32,
    pub has_forecast_distance: bool,
    pub source_model_format: PioStringView,
    pub has_source_model_format: bool,
    pub minimum_validation_level: PioStringView,
    pub has_minimum_validation_level: bool,
}

/// One PowSybl subnetwork contained directly by the balanced network.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioSubnetworkView {
    pub component: PioComponentIdView,
    pub parent: PioComponentIdView,
    pub case_metadata: PioCaseMetadataView,
    pub component_count: usize,
}

/// One point of a reactive capability curve.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioReactiveCapabilityCurvePointView {
    pub active_power_mw: f64,
    pub minimum_reactive_power_mvar: f64,
    pub maximum_reactive_power_mvar: f64,
    pub property_count: usize,
}

/// Min/max or active power dependent reactive limits.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioReactiveLimitsView {
    pub kind: PioStringView,
    pub minimum_reactive_power_mvar: f64,
    pub maximum_reactive_power_mvar: f64,
    pub has_minimum_and_maximum: bool,
    pub curve_style: PioStringView,
    pub has_curve_style: bool,
    pub property_count: usize,
    pub point_count: usize,
}

/// Optional generation attached to one PowSybl boundary line.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioBoundaryLineGenerationView {
    pub voltage_regulation_on: bool,
    pub minimum_active_power_mw: f64,
    pub has_minimum_active_power: bool,
    pub maximum_active_power_mw: f64,
    pub has_maximum_active_power: bool,
    pub target_active_power_mw: f64,
    pub has_target_active_power: bool,
    pub target_reactive_power_mvar: f64,
    pub has_target_reactive_power: bool,
    pub target_voltage_kv: f64,
    pub has_target_voltage: bool,
    pub reactive_limits: PioReactiveLimitsView,
    pub has_reactive_limits: bool,
}

/// One PowSybl boundary line retained beside the balanced calculation view.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioBoundaryLineView {
    pub component: PioComponentIdView,
    pub voltage_level: PioComponentIdView,
    pub active_power_setpoint_mw: f64,
    pub reactive_power_setpoint_mvar: f64,
    pub resistance_ohm: f64,
    pub reactance_ohm: f64,
    pub conductance_siemens: f64,
    pub susceptance_siemens: f64,
    pub pairing_key: PioStringView,
    pub has_pairing_key: bool,
    pub generation: PioBoundaryLineGenerationView,
    pub has_generation: bool,
    pub calculation_load: PioComponentIdView,
    pub has_calculation_load: bool,
    pub calculation_generator: PioComponentIdView,
    pub has_calculation_generator: bool,
}

/// One PowSybl tie line and the two boundary lines that define it.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioTieLineView {
    pub component: PioComponentIdView,
    pub boundary_line1: PioComponentIdView,
    pub boundary_line2: PioComponentIdView,
    pub calculation_branch: PioComponentIdView,
    pub has_calculation_branch: bool,
}

/// Source neutral metadata attached to one stable component identity.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioComponentMetadataView {
    pub component: PioComponentIdView,
    pub name: PioStringView,
    pub has_name: bool,
    pub equipment_container: PioComponentIdView,
    pub has_equipment_container: bool,
    pub fictitious: bool,
    pub alias_count: usize,
    pub external_identifier_count: usize,
    pub property_count: usize,
}

/// One source neutral component alias.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioComponentAliasView {
    pub value: PioStringView,
    pub alias_type: PioStringView,
    pub has_alias_type: bool,
}

/// One source neutral external component identifier.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioExternalIdentifierView {
    pub value: PioStringView,
    pub authority: PioStringView,
    pub has_authority: bool,
}

/// One source neutral string property.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioStringPropertyView {
    pub name: PioStringView,
    pub value: PioStringView,
}

/// One source neutral substation.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioSubstationView {
    pub component: PioComponentIdView,
    pub country: PioStringView,
    pub has_country: bool,
    pub operator_name: PioStringView,
    pub has_operator_name: bool,
    pub geographical_tag_count: usize,
}

/// One source neutral voltage level.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioVoltageLevelView {
    pub component: PioComponentIdView,
    pub substation: PioComponentIdView,
    pub has_substation: bool,
    pub nominal_voltage_kv: f64,
    pub low_voltage_limit_kv: f64,
    pub has_low_voltage_limit: bool,
    pub high_voltage_limit_kv: f64,
    pub has_high_voltage_limit: bool,
    pub topology_kind: PioStringView,
    pub bus_count: usize,
}

/// One source neutral connectivity node.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioConnectivityNodeView {
    pub component: PioComponentIdView,
    pub voltage_level: PioComponentIdView,
    pub node_number: i32,
    pub has_node_number: bool,
    pub calculated_bus_id: usize,
    pub has_calculated_bus: bool,
}

/// One configured bus in bus breaker topology.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioBusBreakerBusView {
    pub component: PioComponentIdView,
    pub voltage_level: PioComponentIdView,
    pub calculated_bus_id: usize,
    pub has_calculated_bus: bool,
    pub voltage_kv: f64,
    pub has_voltage: bool,
    pub angle_degrees: f64,
    pub has_angle: bool,
}

/// One calculated bus explicitly recorded in node breaker topology.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioCalculatedBusView {
    pub voltage_level: PioComponentIdView,
    pub calculated_bus_id: usize,
    pub node_count: usize,
    pub voltage_kv: f64,
    pub has_voltage: bool,
    pub angle_degrees: f64,
    pub has_angle: bool,
}

/// One source neutral busbar section.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioBusbarSectionView {
    pub component: PioComponentIdView,
    pub voltage_level: PioComponentIdView,
    pub node: PioComponentIdView,
}

/// One source neutral CIM junction.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioJunctionView {
    pub component: PioComponentIdView,
}

/// One source neutral AC terminal.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioDetailedTerminalView {
    pub component: PioComponentIdView,
    pub has_component: bool,
    pub equipment: PioComponentIdView,
    pub terminal: u8,
    pub voltage_level: PioComponentIdView,
    pub bus: PioComponentIdView,
    pub has_bus: bool,
    pub connectable_bus: PioComponentIdView,
    pub has_connectable_bus: bool,
    pub node: PioComponentIdView,
    pub has_node: bool,
    pub connected: bool,
    pub active_power_mw: f64,
    pub has_active_power: bool,
    pub reactive_power_mvar: f64,
    pub has_reactive_power: bool,
}

/// One source neutral bus breaker or node breaker switch.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioTopologySwitchView {
    pub component: PioComponentIdView,
    pub voltage_level: PioComponentIdView,
    pub kind: PioStringView,
    pub endpoint1_kind: PioStringView,
    pub endpoint1: PioComponentIdView,
    pub endpoint2_kind: PioStringView,
    pub endpoint2: PioComponentIdView,
    pub open: bool,
    pub retained: bool,
}

/// One permanent connection between two node breaker connectivity nodes.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioInternalConnectionView {
    pub voltage_level: PioComponentIdView,
    pub node1: PioComponentIdView,
    pub node2: PioComponentIdView,
}

/// One named source neutral loading limit set at an equipment terminal.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioOperationalLimitGroupView {
    pub equipment: PioComponentIdView,
    pub terminal: u8,
    pub id: PioStringView,
    pub selected: bool,
    pub property_count: usize,
    pub has_current_limits: bool,
    pub current_permanent_limit_a: f64,
    pub current_permanent_limit_name: PioStringView,
    pub has_current_permanent_limit: bool,
    pub has_current_permanent_limit_name: bool,
    pub current_temporary_limit_count: usize,
    pub has_active_power_limits: bool,
    pub active_power_permanent_limit_mw: f64,
    pub active_power_permanent_limit_name: PioStringView,
    pub has_active_power_permanent_limit: bool,
    pub has_active_power_permanent_limit_name: bool,
    pub active_power_temporary_limit_count: usize,
    pub has_apparent_power_limits: bool,
    pub apparent_power_permanent_limit_mva: f64,
    pub apparent_power_permanent_limit_name: PioStringView,
    pub has_apparent_power_permanent_limit: bool,
    pub has_apparent_power_permanent_limit_name: bool,
    pub apparent_power_temporary_limit_count: usize,
}

/// One segment of an AC/DC converter DC voltage droop curve.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioDroopCurveSegmentView {
    pub minimum_voltage_kv: f64,
    pub maximum_voltage_kv: f64,
    pub k: f64,
}

/// One temporary source neutral loading limit.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioTemporaryLimitView {
    pub name: PioStringView,
    pub value: f64,
    pub acceptable_duration_seconds: u64,
    pub fictitious: bool,
}

/// One source neutral transformer tap changer.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioTapChangerView {
    pub component: PioComponentIdView,
    pub has_component: bool,
    pub transformer: PioComponentIdView,
    pub winding: u8,
    pub kind: PioStringView,
    pub tap_position: i32,
    pub has_tap_position: bool,
    pub solved_tap_position: i32,
    pub has_solved_tap_position: bool,
    pub low_tap_position: i32,
    pub neutral_tap_position: i32,
    pub has_neutral_tap_position: bool,
    pub normal_tap_position: i32,
    pub has_normal_tap_position: bool,
    pub voltage_step_increment_percent: f64,
    pub has_voltage_step_increment_percent: bool,
    pub load_tap_changing_capabilities: bool,
    pub regulating: bool,
    pub regulation_mode: PioStringView,
    pub has_regulation_mode: bool,
    pub regulation_value: f64,
    pub has_regulation_value: bool,
    pub target_deadband: f64,
    pub has_target_deadband: bool,
    pub regulation_terminal: PioTerminalReferenceView,
    pub has_regulation_terminal: bool,
    pub step_count: usize,
}

/// One source neutral transformer tap changer step.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioTapChangerStepView {
    pub position: i32,
    pub ratio_pu: f64,
    pub phase_shift_degrees: f64,
    pub resistance_deviation_percent: f64,
    pub reactance_deviation_percent: f64,
    pub conductance_deviation_percent: f64,
    pub susceptance_deviation_percent: f64,
}

/// One terminal of source neutral DC conducting equipment.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioDcTerminalView {
    pub component: PioComponentIdView,
    pub has_component: bool,
    pub sequence_number: u32,
    pub has_sequence_number: bool,
    pub dc_node: PioComponentIdView,
    pub has_dc_node: bool,
    pub dc_topological_node: PioComponentIdView,
    pub has_dc_topological_node: bool,
    pub polarity: PioStringView,
    pub has_polarity: bool,
    pub connected: bool,
    pub has_connected: bool,
    pub active_power_mw: f64,
    pub has_active_power: bool,
    pub current_a: f64,
    pub has_current: bool,
}

/// One source neutral DC conducting equipment record.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioDcEquipmentView {
    pub component: PioComponentIdView,
    pub equipment_container: PioComponentIdView,
    pub has_equipment_container: bool,
    pub kind: PioStringView,
    pub terminal_count: usize,
    pub terminal1: PioDcTerminalView,
    pub terminal2: PioDcTerminalView,
    pub rated_dc_voltage_kv: f64,
    pub has_rated_dc_voltage: bool,
    pub resistance_ohm: f64,
    pub has_resistance: bool,
    pub inductance_h: f64,
    pub has_inductance: bool,
    pub capacitance_f: f64,
    pub has_capacitance: bool,
    pub length_km: f64,
    pub has_length: bool,
    pub switch_kind: PioStringView,
    pub has_switch_kind: bool,
    pub open: bool,
    pub has_open: bool,
}

/// One physical or energized node in source neutral DC connectivity.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioDcNodeView {
    pub component: PioComponentIdView,
    pub kind: PioStringView,
    pub nominal_voltage_kv: f64,
    pub has_nominal_voltage: bool,
    pub voltage_kv: f64,
    pub has_voltage: bool,
    pub dc_converter_unit: PioComponentIdView,
    pub has_dc_converter_unit: bool,
    pub dc_topological_node: PioComponentIdView,
    pub has_dc_topological_node: bool,
}

/// One source neutral DC converter unit.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioDcConverterUnitView {
    pub component: PioComponentIdView,
    pub substation: PioComponentIdView,
    pub has_substation: bool,
    pub operation_mode: PioStringView,
}

/// One source neutral AC/DC converter.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioAcDcConverterView {
    pub component: PioComponentIdView,
    pub kind: PioStringView,
    pub dc_converter_unit: PioComponentIdView,
    pub has_dc_converter_unit: bool,
    pub dc_terminal1: PioDcTerminalView,
    pub dc_terminal2: PioDcTerminalView,
    pub base_apparent_power_mva: f64,
    pub has_base_apparent_power: bool,
    pub minimum_active_power_mw: f64,
    pub has_minimum_active_power: bool,
    pub maximum_active_power_mw: f64,
    pub has_maximum_active_power: bool,
    pub minimum_dc_voltage_kv: f64,
    pub has_minimum_dc_voltage: bool,
    pub maximum_dc_voltage_kv: f64,
    pub has_maximum_dc_voltage: bool,
    pub rated_dc_voltage_kv: f64,
    pub has_rated_dc_voltage: bool,
    pub valve_u0_kv: f64,
    pub has_valve_u0: bool,
    pub number_of_valves: u32,
    pub has_number_of_valves: bool,
    pub idle_loss_mw: f64,
    pub has_idle_loss: bool,
    pub switching_loss_mw_per_ampere: f64,
    pub has_switching_loss: bool,
    pub resistive_loss_ohm: f64,
    pub has_resistive_loss: bool,
    pub control_mode: PioStringView,
    pub has_control_mode: bool,
    pub active_power_at_pcc_mw: f64,
    pub has_active_power_at_pcc: bool,
    pub reactive_power_at_pcc_mvar: f64,
    pub has_reactive_power_at_pcc: bool,
    pub target_active_power_mw: f64,
    pub has_target_active_power: bool,
    pub target_dc_voltage_kv: f64,
    pub has_target_dc_voltage: bool,
    pub pcc_terminal: PioTerminalReferenceView,
    pub has_pcc_terminal: bool,
    pub droop_curve_segment_count: usize,
    pub has_droop_curve: bool,
    pub droop: f64,
    pub has_droop: bool,
    pub droop_compensation: f64,
    pub has_droop_compensation: bool,
    pub q_share: f64,
    pub has_q_share: bool,
    pub maximum_modulation_index: f64,
    pub has_maximum_modulation_index: bool,
    pub maximum_valve_current_a: f64,
    pub has_maximum_valve_current: bool,
    pub dc_current_a: f64,
    pub has_dc_current: bool,
    pub ac_voltage_kv: f64,
    pub has_ac_voltage: bool,
    pub dc_voltage_kv: f64,
    pub has_dc_voltage: bool,
    pub voltage_regulator_on: bool,
    pub has_voltage_regulator_on: bool,
    pub voltage_setpoint_kv: f64,
    pub has_voltage_setpoint: bool,
    pub reactive_power_setpoint_mvar: f64,
    pub has_reactive_power_setpoint: bool,
    pub reactive_limits: PioReactiveLimitsView,
    pub has_reactive_limits: bool,
    pub pole_loss_active_power_mw: f64,
    pub has_pole_loss_active_power: bool,
    pub reactive_model: PioStringView,
    pub has_reactive_model: bool,
    pub power_factor: f64,
    pub has_power_factor: bool,
    pub operating_mode: PioStringView,
    pub has_operating_mode: bool,
    pub rated_dc_current_a: f64,
    pub has_rated_dc_current: bool,
    pub minimum_alpha_degrees: f64,
    pub has_minimum_alpha: bool,
    pub maximum_alpha_degrees: f64,
    pub has_maximum_alpha: bool,
    pub minimum_gamma_degrees: f64,
    pub has_minimum_gamma: bool,
    pub maximum_gamma_degrees: f64,
    pub has_maximum_gamma: bool,
    pub target_alpha_degrees: f64,
    pub has_target_alpha: bool,
    pub target_gamma_degrees: f64,
    pub has_target_gamma: bool,
    pub target_dc_current_a: f64,
    pub has_target_dc_current: bool,
    pub alpha_degrees: f64,
    pub has_alpha: bool,
    pub gamma_degrees: f64,
    pub has_gamma: bool,
    pub delta_degrees: f64,
    pub has_delta: bool,
    pub uf_kv: f64,
    pub has_uf: bool,
    pub uv_kv: f64,
    pub has_uv: bool,
}

/// One point in a multiconductor network coordinate space.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioMulticonductorLocationView {
    pub x: f64,
    pub y: f64,
    pub kind: PioStringView,
    pub has_kind: bool,
}

/// Coordinate metadata for a multiconductor network.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioMulticonductorGeoView {
    pub has_geo: bool,
    pub space: PioStringView,
    pub crs: PioStringView,
    pub has_crs: bool,
    pub kind: PioStringView,
    pub has_kind: bool,
    pub has_canvas: bool,
    pub canvas_width: f64,
    pub has_canvas_width: bool,
    pub canvas_height: f64,
    pub has_canvas_height: bool,
    pub canvas_units: PioStringView,
    pub has_canvas_units: bool,
}

/// Exact table lengths in a multiconductor network.
///
/// Source extension `extras` maps are not exposed through ABI 7. They are
/// retained by PowerIO for same format emission but are not PowerIO domain data.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioMulticonductorNetworkCountsView {
    pub buses: usize,
    pub line_codes: usize,
    pub lines: usize,
    pub switches: usize,
    pub transformers: usize,
    pub loads: usize,
    pub generators: usize,
    pub inverter_based_resources: usize,
    pub control_profiles: usize,
    pub shunts: usize,
    pub capacitors: usize,
    pub voltage_sources: usize,
    pub untyped_objects: usize,
    pub commands: usize,
    pub options: usize,
}

/// One multiconductor bus.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioMulticonductorBusView {
    pub id: PioStringView,
    pub terminal_count: usize,
    pub grounded_terminal_count: usize,
    pub voltage_min_v: f64,
    pub has_voltage_min: bool,
    pub voltage_max_v: f64,
    pub has_voltage_max: bool,
    pub phase_to_neutral_voltage_min_v: PioF64View,
    pub has_phase_to_neutral_voltage_min: bool,
    pub phase_to_neutral_voltage_max_v: PioF64View,
    pub has_phase_to_neutral_voltage_max: bool,
    pub phase_to_phase_voltage_min_v: PioF64View,
    pub has_phase_to_phase_voltage_min: bool,
    pub phase_to_phase_voltage_max_v: PioF64View,
    pub has_phase_to_phase_voltage_max: bool,
    pub positive_sequence_voltage_min_v: f64,
    pub has_positive_sequence_voltage_min: bool,
    pub positive_sequence_voltage_max_v: f64,
    pub has_positive_sequence_voltage_max: bool,
    pub negative_sequence_voltage_max_v: f64,
    pub has_negative_sequence_voltage_max: bool,
    pub zero_sequence_voltage_max_v: f64,
    pub has_zero_sequence_voltage_max: bool,
    pub neutral_to_ground_voltage_max_v: f64,
    pub has_neutral_to_ground_voltage_max: bool,
    pub location: PioMulticonductorLocationView,
    pub has_location: bool,
}

/// One multiconductor line code.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioMulticonductorLineCodeView {
    pub name: PioStringView,
    pub conductor_count: usize,
    pub resistance_matrix_row_count: usize,
    pub reactance_matrix_row_count: usize,
    pub conductance_from_matrix_row_count: usize,
    pub susceptance_from_matrix_row_count: usize,
    pub conductance_to_matrix_row_count: usize,
    pub susceptance_to_matrix_row_count: usize,
    pub current_limit_a: PioF64View,
    pub has_current_limit: bool,
    pub apparent_power_limit_va: PioF64View,
    pub has_apparent_power_limit: bool,
    pub source: PioStringView,
    pub has_source: bool,
}

/// One multiconductor line.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioMulticonductorLineView {
    pub name: PioStringView,
    pub bus_from: PioStringView,
    pub bus_to: PioStringView,
    pub terminal_map_from_count: usize,
    pub terminal_map_to_count: usize,
    pub line_code: PioStringView,
    pub length_m: f64,
    pub route_point_count: usize,
    pub has_route: bool,
    pub current_limit_a: PioF64View,
    pub has_current_limit: bool,
    pub apparent_power_limit_va: PioF64View,
    pub has_apparent_power_limit: bool,
}

/// One multiconductor switch.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioMulticonductorSwitchView {
    pub name: PioStringView,
    pub bus_from: PioStringView,
    pub bus_to: PioStringView,
    pub terminal_map_from_count: usize,
    pub terminal_map_to_count: usize,
    pub open: bool,
    pub current_limit_a: PioF64View,
    pub has_current_limit: bool,
}

/// One multiconductor transformer.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioMulticonductorTransformerView {
    pub name: PioStringView,
    pub winding_count: usize,
    pub short_circuit_reactance_percent: PioF64View,
    pub phase_count: usize,
}

/// One winding of a multiconductor transformer.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioMulticonductorTransformerWindingView {
    pub bus: PioStringView,
    pub terminal_map_count: usize,
    pub connection: PioStringView,
    pub rated_voltage_v: f64,
    pub apparent_power_rating_va: f64,
    pub resistance_percent: f64,
    pub tap: f64,
    pub neutral_resistance_ohm: f64,
    pub has_neutral_resistance: bool,
    pub neutral_reactance_ohm: f64,
    pub has_neutral_reactance: bool,
}

/// One multiconductor load.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioMulticonductorLoadView {
    pub name: PioStringView,
    pub bus: PioStringView,
    pub terminal_map_count: usize,
    pub configuration: PioStringView,
    pub active_power_nominal_w: PioF64View,
    pub reactive_power_nominal_var: PioF64View,
    pub voltage_model: PioStringView,
    pub nominal_voltage_v: PioF64View,
    pub active_power_constant_impedance: PioF64View,
    pub active_power_constant_current: PioF64View,
    pub active_power_constant_power: PioF64View,
    pub reactive_power_constant_impedance: PioF64View,
    pub reactive_power_constant_current: PioF64View,
    pub reactive_power_constant_power: PioF64View,
    pub active_power_exponent: PioF64View,
    pub reactive_power_exponent: PioF64View,
}

/// One multiconductor generator.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioMulticonductorGeneratorView {
    pub name: PioStringView,
    pub bus: PioStringView,
    pub terminal_map_count: usize,
    pub configuration: PioStringView,
    pub active_power_nominal_w: PioF64View,
    pub reactive_power_nominal_var: PioF64View,
    pub active_power_min_w: PioF64View,
    pub has_active_power_min: bool,
    pub active_power_max_w: PioF64View,
    pub has_active_power_max: bool,
    pub reactive_power_min_var: PioF64View,
    pub has_reactive_power_min: bool,
    pub reactive_power_max_var: PioF64View,
    pub has_reactive_power_max: bool,
    pub active_power_dispatch_cost_per_kwh: PioF64View,
    pub has_active_power_dispatch_cost: bool,
    pub apparent_power_limit_va: PioF64View,
    pub has_apparent_power_limit: bool,
    pub current_limit_a: PioF64View,
    pub has_current_limit: bool,
}

/// One inverter based resource.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioInverterBasedResourceView {
    pub name: PioStringView,
    pub bus: PioStringView,
    pub terminal_map_count: usize,
    pub topology: PioStringView,
    pub prime_mover: PioStringView,
    pub apparent_power_limit_va: PioF64View,
    pub current_limit_a: PioF64View,
    pub has_current_limit: bool,
    pub active_power_available_w: f64,
    pub has_active_power_available: bool,
    pub active_power_min_w: PioF64View,
    pub has_active_power_min: bool,
    pub active_power_max_w: PioF64View,
    pub has_active_power_max: bool,
    pub reactive_power_min_var: PioF64View,
    pub has_reactive_power_min: bool,
    pub reactive_power_max_var: PioF64View,
    pub has_reactive_power_max: bool,
    pub control_profile: PioStringView,
    pub has_control_profile: bool,
    pub voltage_aggregation: PioStringView,
    pub has_voltage_aggregation: bool,
}

/// One inverter control profile.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioControlProfileView {
    pub name: PioStringView,
    pub has_power_factor: bool,
    pub power_factor: f64,
    pub has_volt_var: bool,
    pub volt_var_voltage_reference: PioStringView,
    pub has_volt_var_voltage_reference: bool,
    pub volt_var_breakpoints: PioF64View,
    pub volt_var_reactive_power_limits: PioF64View,
    pub volt_var_reactive_power_unit: PioStringView,
    pub has_volt_var_reactive_power_unit: bool,
    pub volt_var_reactive_power_reference: PioStringView,
    pub has_volt_var_reactive_power_reference: bool,
    pub volt_var_active_power_min_for_reactive_power_w: f64,
    pub has_volt_var_active_power_min_for_reactive_power: bool,
    pub volt_var_active_power_min_for_max_reactive_power_w: f64,
    pub has_volt_var_active_power_min_for_max_reactive_power: bool,
    pub has_volt_watt: bool,
    pub volt_watt_voltage_reference: PioStringView,
    pub has_volt_watt_voltage_reference: bool,
    pub volt_watt_breakpoints: PioF64View,
    pub volt_watt_active_power_limits: PioF64View,
    pub volt_watt_active_power_unit: PioStringView,
    pub has_volt_watt_active_power_unit: bool,
    pub volt_watt_active_power_reference: PioStringView,
    pub has_volt_watt_active_power_reference: bool,
}

/// One multiconductor shunt.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioMulticonductorShuntView {
    pub name: PioStringView,
    pub bus: PioStringView,
    pub terminal_map_count: usize,
    pub conductance_matrix_row_count: usize,
    pub susceptance_matrix_row_count: usize,
}

/// One multiconductor capacitor.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioMulticonductorCapacitorView {
    pub name: PioStringView,
    pub bus: PioStringView,
    pub terminal_map_count: usize,
    pub configuration: PioStringView,
    pub rated_reactive_power_var: f64,
    pub nominal_voltage_v: f64,
}

/// One multiconductor voltage source.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioVoltageSourceView {
    pub name: PioStringView,
    pub bus: PioStringView,
    pub terminal_map_count: usize,
    pub voltage_magnitude_v: PioF64View,
    pub voltage_angle_rad: PioF64View,
}

/// One source object retained without a typed PowerIO representation.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioMulticonductorUntypedObjectView {
    pub class_name: PioStringView,
    pub name: PioStringView,
    pub property_count: usize,
}

/// One property of an untyped source object.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioMulticonductorUntypedPropertyView {
    pub name: PioStringView,
    pub has_name: bool,
    pub value: PioStringView,
}

/// One retained source command.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioMulticonductorCommandView {
    pub verb: PioStringView,
    pub args: PioStringView,
}

/// One bus boundary specification in a DC power flow instance.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioDcBusSpecificationView {
    pub bus_id: usize,
    pub kind: PioStringView,
    pub net_active_power_mw: f64,
    pub voltage_angle_degrees: f64,
}

/// One bus boundary specification in an AC power flow instance.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioAcBusSpecificationView {
    pub bus_id: usize,
    pub kind: PioStringView,
    pub net_active_power_mw: f64,
    pub net_reactive_power_mvar: f64,
    pub voltage_magnitude_pu: f64,
    pub voltage_angle_degrees: f64,
}

/// Shape and conventions of one prepared DC OPF calculation.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioDcOpfPreparationView {
    pub name: PioStringView,
    pub bus_count: usize,
    pub generator_count: usize,
    pub branch_count: usize,
    pub source_generator_count: usize,
    pub source_branch_count: usize,
    pub base_mva: f64,
    pub units: PioStringView,
    pub branch_susceptance_formula: PioStringView,
    pub objective: PioStringView,
    pub skip_zero_impedance: bool,
    pub synthesize_unrated_limits: bool,
    pub correct_angle_difference_bounds: bool,
    pub reference_bus_count: usize,
    pub skipped_zero_impedance_count: usize,
}

/// One dense bus row in a DC OPF preparation.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioDcOpfBusView {
    pub bus_id: usize,
    pub analysis_row: usize,
    pub source_row: usize,
    pub has_source_row: bool,
    pub active_power_demand: f64,
    pub shunt_conductance: f64,
    pub phase_shift_injection: f64,
}

/// One generator row in a DC OPF preparation.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioDcOpfGeneratorView {
    pub component_id: PioStringView,
    pub bus_index: usize,
    pub analysis_row: usize,
    pub source_row: usize,
    pub has_source_row: bool,
    pub quadratic_cost: f64,
    pub linear_cost: f64,
    pub constant_cost: f64,
    pub has_piecewise_linear_cost: bool,
    pub piecewise_linear_power: PioF64View,
    pub piecewise_linear_value: PioF64View,
    pub active_power_max: f64,
    pub active_power_min: f64,
    pub capability_active: bool,
}

/// One active branch row in a DC OPF preparation.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioDcOpfBranchView {
    pub component_id: PioStringView,
    pub from_bus_index: usize,
    pub to_bus_index: usize,
    pub susceptance_magnitude: f64,
    pub phase_shift_radians: f64,
    pub active_power_max: f64,
    pub angle_difference_min_radians: f64,
    pub angle_difference_max_radians: f64,
    pub analysis_row: usize,
    pub source_kind: PioStringView,
    pub source_row: usize,
    pub winding: usize,
    pub has_winding: bool,
    pub thermal_limit_active: bool,
    pub angle_bound_active: bool,
}

/// Shape and conventions of one prepared AC OPF calculation.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioAcOpfPreparationView {
    pub name: PioStringView,
    pub bus_count: usize,
    pub generator_count: usize,
    pub storage_count: usize,
    pub branch_count: usize,
    pub source_generator_count: usize,
    pub source_branch_count: usize,
    pub base_mva: f64,
    pub units: PioStringView,
    pub objective: PioStringView,
    pub skip_zero_impedance: bool,
    pub synthesize_unrated_limits: bool,
    pub correct_angle_difference_bounds: bool,
    pub reference_bus_count: usize,
    pub skipped_zero_impedance_count: usize,
}

/// One dense bus row in an AC OPF preparation.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioAcOpfBusView {
    pub bus_id: usize,
    pub analysis_row: usize,
    pub source_row: usize,
    pub has_source_row: bool,
    pub active_power_demand: f64,
    pub reactive_power_demand: f64,
    pub shunt_conductance: f64,
    pub shunt_susceptance: f64,
    pub voltage_magnitude_min_pu: f64,
    pub voltage_magnitude_max_pu: f64,
    pub initial_voltage_magnitude_pu: f64,
    pub initial_voltage_angle_radians: f64,
    pub voltage_bound_active: bool,
}

/// One generator row in an AC OPF preparation.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioAcOpfGeneratorView {
    pub component_id: PioStringView,
    pub bus_index: usize,
    pub analysis_row: usize,
    pub source_row: usize,
    pub has_source_row: bool,
    pub quadratic_cost: f64,
    pub linear_cost: f64,
    pub constant_cost: f64,
    pub has_piecewise_linear_cost: bool,
    pub piecewise_linear_power: PioF64View,
    pub piecewise_linear_value: PioF64View,
    pub active_power_max: f64,
    pub active_power_min: f64,
    pub reactive_power_max: f64,
    pub reactive_power_min: f64,
    pub initial_active_power: f64,
    pub initial_reactive_power: f64,
    pub voltage_magnitude_setpoint_pu: f64,
    pub capability_active: bool,
}

/// One storage row in an AC OPF preparation.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioAcOpfStorageView {
    pub component_id: PioStringView,
    pub bus_index: usize,
    pub source_row: usize,
    pub initial_active_power: f64,
    pub initial_reactive_power: f64,
    pub energy: f64,
    pub energy_rating: f64,
    pub charge_rating: f64,
    pub discharge_rating: f64,
    pub charge_efficiency: f64,
    pub discharge_efficiency: f64,
    pub apparent_power_max: f64,
    pub reactive_power_min: f64,
    pub reactive_power_max: f64,
    pub resistance_pu: f64,
    pub reactance_pu: f64,
    pub active_power_loss: f64,
    pub reactive_power_loss: f64,
    pub in_service: bool,
}

/// One active branch row in an AC OPF preparation.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioAcOpfBranchView {
    pub component_id: PioStringView,
    pub from_bus_index: usize,
    pub to_bus_index: usize,
    pub series_conductance: f64,
    pub series_susceptance: f64,
    pub from_conductance: f64,
    pub from_susceptance: f64,
    pub to_conductance: f64,
    pub to_susceptance: f64,
    pub tap_ratio: f64,
    pub phase_shift_radians: f64,
    pub apparent_power_max: f64,
    pub angle_difference_min_radians: f64,
    pub angle_difference_max_radians: f64,
    pub analysis_row: usize,
    pub source_kind: PioStringView,
    pub source_row: usize,
    pub winding: usize,
    pub has_winding: bool,
    pub thermal_limit_active: bool,
    pub angle_bound_active: bool,
}

/// One typed objective term.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioObjectiveTermView {
    pub kind: PioStringView,
}

/// One active constraint family and its element selection.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioActiveConstraintView {
    pub family: PioStringView,
    pub selection: PioStringView,
    pub identity_count: usize,
}

/// One prescribed multiconductor load terminal power.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioPrescribedTerminalPowerView {
    pub load: PioStringView,
    pub terminal_count: usize,
    pub voltage_model: PioStringView,
}

/// One terminal of a prescribed multiconductor load.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioTerminalPowerView {
    pub terminal: PioStringView,
    pub active_power_w: f64,
    pub reactive_power_var: f64,
    pub nominal_voltage_v: f64,
    pub has_nominal_voltage: bool,
    pub active_impedance_fraction: f64,
    pub active_current_fraction: f64,
    pub active_power_fraction: f64,
    pub reactive_impedance_fraction: f64,
    pub reactive_current_fraction: f64,
    pub reactive_power_fraction: f64,
    pub active_power_exponent: f64,
    pub reactive_power_exponent: f64,
}

/// One prescribed multiconductor source terminal voltage.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioPrescribedSourceVoltageView {
    pub source: PioStringView,
    pub terminal_count: usize,
}

/// One terminal of a prescribed multiconductor source.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioTerminalVoltageView {
    pub terminal: PioStringView,
    pub magnitude_v: f64,
    pub angle_radians: f64,
}

/// One isolated multiconductor terminal.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioIsolatedTerminalView {
    pub bus: PioStringView,
    pub terminal: PioStringView,
}

/// One active multiconductor equipment control.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioActiveControlView {
    pub kind: PioStringView,
    pub component_id: PioStringView,
}

/// Set sizes and time horizon of one AC SCUC instance.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioScucDimensionsView {
    pub period_count: usize,
    pub device_count: usize,
    pub producer_count: usize,
    pub consumer_count: usize,
    pub shunt_count: usize,
    pub branch_switching_cost_count: usize,
    pub transformer_control_count: usize,
    pub active_reserve_zone_count: usize,
    pub reactive_reserve_zone_count: usize,
    pub contingency_count: usize,
}

/// Required SCUC violation costs.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioScucViolationCostView {
    pub active_power_balance: f64,
    pub reactive_power_balance: f64,
    pub branch_thermal_limit: f64,
    pub energy_requirement: f64,
}

/// Borrowed stable component identity.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioComponentIdView {
    pub component_type: PioStringView,
    pub local_id: PioStringView,
}

/// Active power ramp limits for one SCUC device, in per unit per hour.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioScucRampLimitsView {
    pub up_pu_per_hour: f64,
    pub down_pu_per_hour: f64,
    pub startup_pu_per_hour: f64,
    pub shutdown_pu_per_hour: f64,
}

/// Reserve quantity limits for one SCUC device.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioScucReserveLimitsView {
    pub regulation_up_pu: f64,
    pub regulation_down_pu: f64,
    pub synchronized_pu: f64,
    pub nonsynchronized_pu: f64,
    pub ramping_up_online_pu: f64,
    pub ramping_down_online_pu: f64,
    pub ramping_up_offline_pu: f64,
    pub ramping_down_offline_pu: f64,
}

/// Initial commitment durations for one SCUC device.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioScucInitialCommitmentView {
    pub accumulated_up_time_hours: f64,
    pub accumulated_down_time_hours: f64,
}

/// Additional active and reactive power capability relation for one device.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioScucReactiveCapabilityView {
    pub kind: PioStringView,
    pub reactive_power_at_zero_active_power_pu: f64,
    pub reactive_power_at_zero_active_power_min_pu: f64,
    pub reactive_power_at_zero_active_power_max_pu: f64,
    pub slope: f64,
    pub slope_min: f64,
    pub slope_max: f64,
}

/// One SCUC producer or consumer.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioScucDeviceView {
    pub id: PioComponentIdView,
    pub kind: PioStringView,
    pub initial_on_status: bool,
    pub on_cost: f64,
    pub startup_cost: f64,
    pub shutdown_cost: f64,
    pub minimum_up_time_hours: f64,
    pub minimum_down_time_hours: f64,
    pub ramp_limits: PioScucRampLimitsView,
    pub reserve_limits: PioScucReserveLimitsView,
    pub initial_commitment: PioScucInitialCommitmentView,
    pub reactive_capability: PioScucReactiveCapabilityView,
    pub period_count: usize,
    pub startup_cost_adjustment_count: usize,
    pub startup_limit_count: usize,
    pub energy_upper_bound_count: usize,
    pub energy_lower_bound_count: usize,
}

/// One downtime dependent startup cost adjustment.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioScucStartupCostAdjustmentView {
    pub cost: f64,
    pub maximum_down_time_hours: f64,
}

/// One SCUC device period.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioScucDevicePeriodView {
    pub on_status_min: bool,
    pub on_status_max: bool,
    pub active_power_min_pu: f64,
    pub active_power_max_pu: f64,
    pub reactive_power_min_pu: f64,
    pub reactive_power_max_pu: f64,
    pub energy_cost_block_count: usize,
    pub reserve_costs: PioScucReserveCostsView,
}

/// One limit on the number of device startups during a time window.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioScucStartupLimitView {
    pub start_time_hours: f64,
    pub end_time_hours: f64,
    pub maximum_startups: u64,
}

/// One energy requirement over a time window, in per unit as defined by GOC3.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioScucEnergyRequirementView {
    pub start_time_hours: f64,
    pub end_time_hours: f64,
    pub energy_pu: f64,
}

/// One piecewise linear active energy cost block.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioScucEnergyCostBlockView {
    pub marginal_cost: f64,
    pub block_size_pu: f64,
}

/// Reserve costs for one device and one interval, in $/(p.u. h).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioScucReserveCostsView {
    pub regulation_up: f64,
    pub regulation_down: f64,
    pub synchronized: f64,
    pub nonsynchronized: f64,
    pub ramping_up_online: f64,
    pub ramping_down_online: f64,
    pub ramping_up_offline: f64,
    pub ramping_down_offline: f64,
    pub reactive_up: f64,
    pub reactive_down: f64,
}

/// Discrete step limits for one SCUC shunt.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioScucShuntView {
    pub id: PioComponentIdView,
    pub conductance_per_step_pu: f64,
    pub susceptance_per_step_pu: f64,
    pub step_min: i64,
    pub step_max: i64,
    pub initial_step: i64,
}

/// Connection and disconnection costs for one switchable AC branch.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioScucBranchSwitchingCostView {
    pub id: PioComponentIdView,
    pub connection_cost: f64,
    pub disconnection_cost: f64,
}

/// Tap ratio and phase shift bounds for one transformer.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioScucTransformerControlView {
    pub id: PioComponentIdView,
    pub tap_ratio_min: f64,
    pub tap_ratio_max: f64,
    pub phase_shift_min_radians: f64,
    pub phase_shift_max_radians: f64,
}

/// One active power reserve zone.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioScucActiveReserveZoneView {
    pub id: PioComponentIdView,
    pub regulation_up_requirement_fraction: f64,
    pub regulation_down_requirement_fraction: f64,
    pub synchronized_requirement_fraction: f64,
    pub nonsynchronized_requirement_fraction: f64,
    pub regulation_up_violation_cost: f64,
    pub regulation_down_violation_cost: f64,
    pub synchronized_violation_cost: f64,
    pub nonsynchronized_violation_cost: f64,
    pub ramping_up_violation_cost: f64,
    pub ramping_down_violation_cost: f64,
    pub period_count: usize,
    pub bus_count: usize,
}

/// One period of an active power reserve zone.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioScucActiveReservePeriodView {
    pub ramping_up_requirement_pu: f64,
    pub ramping_down_requirement_pu: f64,
}

/// One reactive power reserve zone.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioScucReactiveReserveZoneView {
    pub id: PioComponentIdView,
    pub reactive_up_violation_cost: f64,
    pub reactive_down_violation_cost: f64,
    pub period_count: usize,
    pub bus_count: usize,
}

/// One period of a reactive power reserve zone.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioScucReactiveReservePeriodView {
    pub reactive_up_requirement_pu: f64,
    pub reactive_down_requirement_pu: f64,
}

/// One named SCUC contingency.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioScucContingencyView {
    pub id: PioComponentIdView,
    pub component_count: usize,
}

/// One component removed by a SCUC contingency.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PioScucContingencyComponentView {
    pub id: PioComponentIdView,
}

// ---- shared handle machinery ----------------------------------------------

/// Every handle payload must be shareable across threads: the header lets
/// callers move and retain handles from any thread.
const fn assert_send_sync<T: Send + Sync>() {}

#[repr(transparent)]
struct HandleBox<T> {
    inner: Arc<T>,
}

fn handle_new<T>(value: T) -> *mut HandleBox<T> {
    Box::into_raw(Box::new(HandleBox {
        inner: Arc::new(value),
    }))
}

fn handle_from_arc<T>(inner: Arc<T>) -> *mut HandleBox<T> {
    Box::into_raw(Box::new(HandleBox { inner }))
}

unsafe fn handle_get<'a, T>(raw: *const HandleBox<T>) -> Option<&'a T> {
    unsafe { raw.as_ref() }.map(|handle| handle.inner.as_ref())
}

unsafe fn handle_arc<T>(raw: *const HandleBox<T>) -> Option<Arc<T>> {
    unsafe { raw.as_ref() }.map(|handle| Arc::clone(&handle.inner))
}

unsafe fn handle_retain<T>(raw: *const HandleBox<T>) -> *mut HandleBox<T> {
    unsafe { handle_arc(raw) }.map_or(std::ptr::null_mut(), handle_from_arc)
}

unsafe fn handle_release<T>(raw: *mut HandleBox<T>) {
    if !raw.is_null() {
        drop(unsafe { Box::from_raw(raw) });
    }
}

macro_rules! opaque_handle {
    ($(#[$doc:meta])* $name:ident, $inner:ty) => {
        $(#[$doc])*
        #[repr(transparent)]
        pub struct $name(HandleBox<$inner>);
        const _: () = assert_send_sync::<$inner>();

        #[allow(dead_code)]
        impl $name {
            fn new_raw(value: $inner) -> *mut Self {
                handle_new(value).cast()
            }

            fn from_arc(inner: Arc<$inner>) -> *mut Self {
                handle_from_arc(inner).cast()
            }

            unsafe fn get<'a>(raw: *const Self) -> Option<&'a $inner> {
                unsafe { handle_get(raw.cast()) }
            }

            unsafe fn arc(raw: *const Self) -> Option<Arc<$inner>> {
                unsafe { handle_arc(raw.cast()) }
            }

            unsafe fn retain_raw(raw: *const Self) -> *mut Self {
                unsafe { handle_retain(raw.cast::<HandleBox<$inner>>()) }.cast::<Self>()
            }

            unsafe fn release_raw(raw: *mut Self) {
                unsafe { handle_release(raw.cast::<HandleBox<$inner>>()) }
            }
        }
    };
}

// ---- errors and diagnostics ------------------------------------------------

struct ErrorInner {
    code: String,
    message: String,
    diagnostics: Arc<DiagnosticsInner>,
}

struct DiagnosticsInner {
    owner: DiagnosticsOwner,
}

enum DiagnosticsOwner {
    Owned(Vec<Diagnostic>),
    Module(Arc<ModuleInner>),
}

impl DiagnosticsInner {
    fn records(&self) -> &[Diagnostic] {
        match &self.owner {
            DiagnosticsOwner::Owned(records) => records,
            DiagnosticsOwner::Module(module) => module.module.diagnostics(),
        }
    }
}

opaque_handle!(
    /// Structured operation failure.
    PioError,
    ErrorInner
);
opaque_handle!(
    /// Immutable diagnostic list.
    PioDiagnostics,
    DiagnosticsInner
);

fn boundary_diagnostic(
    info: &'static powerio_core::DiagnosticInfo,
    message: impl Into<String>,
) -> Diagnostic {
    Diagnostic::of(info, message)
}

fn error_from_diagnostics(message: String, mut records: Vec<Diagnostic>) -> *mut PioError {
    if records.is_empty() {
        records.push(boundary_diagnostic(
            &codes::BIND_CAPI_UNCODED_FAILURE,
            message.clone(),
        ));
    }
    let code = records[0].code().to_owned();
    PioError::new_raw(ErrorInner {
        code,
        message,
        diagnostics: Arc::new(DiagnosticsInner {
            owner: DiagnosticsOwner::Owned(records),
        }),
    })
}

fn error_from_core(error: &powerio_core::Error) -> *mut PioError {
    error_from_diagnostics(error.to_string(), error.diagnostics().to_vec())
}

fn error_from_tx(error: &powerio_tx::Error) -> *mut PioError {
    let diagnostic = Diagnostic::of(error.code(), error.to_string());
    error_from_diagnostics(error.to_string(), vec![diagnostic])
}

fn error_from_matrix(error: &powerio_matrix::Error) -> *mut PioError {
    let diagnostic = Diagnostic::of(error.code(), error.to_string());
    error_from_diagnostics(error.to_string(), vec![diagnostic])
}

fn boundary_error(
    info: &'static powerio_core::DiagnosticInfo,
    message: impl Into<String>,
) -> *mut PioError {
    let diagnostic = boundary_diagnostic(info, message);
    error_from_diagnostics(
        powerio_core::render_diagnostic(&diagnostic),
        vec![diagnostic],
    )
}

unsafe fn store_error(slot: *mut *mut PioError, error: *mut PioError) {
    if slot.is_null() {
        unsafe { PioError::release_raw(error) };
    } else {
        unsafe { *slot = error };
    }
}

unsafe fn entry<R>(
    error: *mut *mut PioError,
    fallback: R,
    operation: impl FnOnce() -> Result<R, *mut PioError>,
) -> R {
    if !error.is_null() {
        unsafe { *error = std::ptr::null_mut() };
    }
    match catch_unwind(AssertUnwindSafe(operation)) {
        Ok(Ok(value)) => value,
        Ok(Err(failure)) => {
            unsafe { store_error(error, failure) };
            fallback
        }
        Err(_) => {
            let failure = boundary_error(
                &codes::BIND_CAPI_PANIC,
                "the operation panicked and made no C-visible change",
            );
            unsafe { store_error(error, failure) };
            fallback
        }
    }
}

unsafe fn input_bytes<'a>(
    data: *const u8,
    len: usize,
    argument: &str,
) -> Result<&'a [u8], *mut PioError> {
    if data.is_null() {
        if len == 0 {
            return Ok(&[]);
        }
        return Err(boundary_error(
            &codes::BIND_CAPI_NULL_ARGUMENT,
            format!("{argument} is NULL with a nonzero length"),
        ));
    }
    Ok(unsafe { std::slice::from_raw_parts(data, len) })
}

unsafe fn input_str<'a>(
    data: *const c_char,
    len: usize,
    argument: &str,
) -> Result<&'a str, *mut PioError> {
    let bytes = unsafe { input_bytes(data.cast(), len, argument) }?;
    std::str::from_utf8(bytes).map_err(|_| {
        boundary_error(
            &codes::BIND_CAPI_INVALID_UTF8,
            format!("{argument} is not valid UTF-8"),
        )
    })
}

unsafe fn required_str<'a>(
    data: *const c_char,
    len: usize,
    argument: &str,
) -> Result<&'a str, *mut PioError> {
    if data.is_null() || len == 0 {
        return Err(boundary_error(
            &codes::BIND_CAPI_NULL_ARGUMENT,
            format!("{argument} is required"),
        ));
    }
    unsafe { input_str(data, len, argument) }
}

unsafe fn optional_str<'a>(
    data: *const c_char,
    len: usize,
    argument: &str,
) -> Result<Option<&'a str>, *mut PioError> {
    if data.is_null() {
        if len == 0 {
            return Ok(None);
        }
        return Err(boundary_error(
            &codes::BIND_CAPI_NULL_ARGUMENT,
            format!("{argument} is NULL with a nonzero length"),
        ));
    }
    unsafe { input_str(data, len, argument) }.map(Some)
}

/// Return the ABI number compiled into this library.
#[unsafe(no_mangle)]
pub extern "C" fn pio_abi_version() -> u32 {
    PIO_ABI_VERSION
}

/// Return the PowerIO crate version.
#[unsafe(no_mangle)]
pub extern "C" fn pio_version() -> PioStringView {
    PioStringView::new(powerio::VERSION)
}

/// The failure's stable diagnostic code.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_error_code(error: *const PioError) -> PioStringView {
    unsafe { PioError::get(error) }.map_or(PioStringView::EMPTY, |error| {
        PioStringView::new(&error.code)
    })
}

/// The rendered failure message.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_error_message(error: *const PioError) -> PioStringView {
    unsafe { PioError::get(error) }.map_or(PioStringView::EMPTY, |error| {
        PioStringView::new(&error.message)
    })
}

/// The structured diagnostics that caused the failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_error_diagnostics(error: *const PioError) -> *mut PioDiagnostics {
    unsafe { PioError::get(error) }.map_or(std::ptr::null_mut(), |error| {
        PioDiagnostics::from_arc(Arc::clone(&error.diagnostics))
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_error_retain(error: *const PioError) -> *mut PioError {
    unsafe { PioError::retain_raw(error) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_error_release(error: *mut PioError) {
    unsafe { PioError::release_raw(error) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_diagnostics_len(diagnostics: *const PioDiagnostics) -> usize {
    unsafe { PioDiagnostics::get(diagnostics) }.map_or(0, |values| values.records().len())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_diagnostic_code(
    diagnostics: *const PioDiagnostics,
    index: usize,
) -> PioStringView {
    unsafe { PioDiagnostics::get(diagnostics) }
        .and_then(|values| values.records().get(index))
        .map_or(PioStringView::EMPTY, |record| {
            PioStringView::new(record.code())
        })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_diagnostic_severity(
    diagnostics: *const PioDiagnostics,
    index: usize,
) -> PioStringView {
    unsafe { PioDiagnostics::get(diagnostics) }
        .and_then(|values| values.records().get(index))
        .map_or(PioStringView::EMPTY, |record| {
            PioStringView::new(record.severity().as_str())
        })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_diagnostic_message(
    diagnostics: *const PioDiagnostics,
    index: usize,
) -> PioStringView {
    unsafe { PioDiagnostics::get(diagnostics) }
        .and_then(|values| values.records().get(index))
        .map_or(PioStringView::EMPTY, |record| {
            PioStringView::new(record.message())
        })
}

/// Whether this diagnostic has a durable identity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_diagnostic_has_id(
    diagnostics: *const PioDiagnostics,
    index: usize,
) -> bool {
    unsafe { PioDiagnostics::get(diagnostics) }
        .and_then(|values| values.records().get(index))
        .is_some_and(|record| record.id().is_some())
}

/// Borrow this diagnostic's durable identity, or an empty view when absent.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_diagnostic_id(
    diagnostics: *const PioDiagnostics,
    index: usize,
) -> PioStringView {
    unsafe { PioDiagnostics::get(diagnostics) }
        .and_then(|values| values.records().get(index))
        .and_then(Diagnostic::id)
        .map_or(PioStringView::EMPTY, |id| PioStringView::new(id.as_str()))
}

/// Whether this diagnostic names a value element.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_diagnostic_has_target(
    diagnostics: *const PioDiagnostics,
    index: usize,
) -> bool {
    unsafe { PioDiagnostics::get(diagnostics) }
        .and_then(|values| values.records().get(index))
        .is_some_and(|record| record.target().is_some())
}

/// Borrow this diagnostic's value element locator, or an empty view when absent.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_diagnostic_target(
    diagnostics: *const PioDiagnostics,
    index: usize,
) -> PioStringView {
    unsafe { PioDiagnostics::get(diagnostics) }
        .and_then(|values| values.records().get(index))
        .and_then(Diagnostic::target)
        .map_or(PioStringView::EMPTY, PioStringView::new)
}

/// Whether this diagnostic carries a suggested action.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_diagnostic_has_suggested_action(
    diagnostics: *const PioDiagnostics,
    index: usize,
) -> bool {
    unsafe { PioDiagnostics::get(diagnostics) }
        .and_then(|values| values.records().get(index))
        .is_some_and(|record| record.suggested_action().is_some())
}

/// Borrow this diagnostic's suggested action, or an empty view when absent.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_diagnostic_suggested_action(
    diagnostics: *const PioDiagnostics,
    index: usize,
) -> PioStringView {
    unsafe { PioDiagnostics::get(diagnostics) }
        .and_then(|values| values.records().get(index))
        .and_then(Diagnostic::suggested_action)
        .map_or(PioStringView::EMPTY, PioStringView::new)
}

/// Number of source byte ranges attached to this diagnostic.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_diagnostic_n_spans(
    diagnostics: *const PioDiagnostics,
    index: usize,
) -> usize {
    unsafe { PioDiagnostics::get(diagnostics) }
        .and_then(|values| values.records().get(index))
        .map_or(0, |record| record.spans().len())
}

/// Read one source byte range. The source string borrows from `diagnostics`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_diagnostic_span(
    diagnostics: *const PioDiagnostics,
    index: usize,
    span_index: usize,
    output: *mut PioDiagnosticSpanView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let values = PioDiagnostics::get(diagnostics).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_NULL_HANDLE,
                    "PioDiagnostics must not be NULL",
                )
            })?;
            let record = values.records().get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("diagnostic index {index} is out of range"),
                )
            })?;
            let span = record.spans().get(span_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("diagnostic span index {span_index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = PioDiagnosticSpanView {
                source: PioStringView::new(span.source().as_str()),
                byte_start: span.byte_start(),
                byte_end: span.byte_end(),
            };
            Ok(true)
        })
    }
}

/// Number of other diagnostic identities referenced by this diagnostic.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_diagnostic_n_related(
    diagnostics: *const PioDiagnostics,
    index: usize,
) -> usize {
    unsafe { PioDiagnostics::get(diagnostics) }
        .and_then(|values| values.records().get(index))
        .map_or(0, |record| record.related().len())
}

/// Borrow one related diagnostic identity, or an empty view when out of range.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_diagnostic_related(
    diagnostics: *const PioDiagnostics,
    index: usize,
    related_index: usize,
) -> PioStringView {
    unsafe { PioDiagnostics::get(diagnostics) }
        .and_then(|values| values.records().get(index))
        .and_then(|record| record.related().get(related_index))
        .map_or(PioStringView::EMPTY, |id| PioStringView::new(id.as_str()))
}

/// Serialize this diagnostic's structured details as an owned JSON object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_diagnostic_details_json(
    diagnostics: *const PioDiagnostics,
    index: usize,
    error: *mut *mut PioError,
) -> *mut PioString {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let values = PioDiagnostics::get(diagnostics).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_NULL_HANDLE,
                    "PioDiagnostics must not be NULL",
                )
            })?;
            let record = values.records().get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("diagnostic index {index} is out of range"),
                )
            })?;
            serde_json::to_string(record.details())
                .map(|text| PioString::new_raw(StringInner { text }))
                .map_err(|failure| {
                    boundary_error(
                        &codes::EMIT_CAPI_SERIALIZE_FAILED,
                        format!("cannot serialize diagnostic details: {failure}"),
                    )
                })
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_diagnostics_retain(
    diagnostics: *const PioDiagnostics,
) -> *mut PioDiagnostics {
    unsafe { PioDiagnostics::retain_raw(diagnostics) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_diagnostics_release(diagnostics: *mut PioDiagnostics) {
    unsafe { PioDiagnostics::release_raw(diagnostics) };
}

// ---- source and destination ------------------------------------------------

opaque_handle!(
    /// Acquired file, directory, or named memory bytes.
    PioSource,
    Source
);

enum DestinationSpec {
    Path(PathBuf),
    Memory(String),
}

impl DestinationSpec {
    fn build(&self) -> Result<Destination, powerio_core::Error> {
        match self {
            Self::Path(path) => Ok(Destination::path(path)),
            Self::Memory(root) => Destination::memory(root.clone()),
        }
    }
}

opaque_handle!(
    /// File, directory, or memory output destination.
    PioDestination,
    DestinationSpec
);

struct GeoLayerInner {
    layer: powerio::GeoLayer,
    diagnostics: Vec<Diagnostic>,
}

opaque_handle!(
    /// Parsed geographic sidecar.
    PioGeoLayer,
    GeoLayerInner
);
opaque_handle!(
    /// Counts and notes from applying one geographic layer.
    PioGeoApplyReport,
    powerio::GeoApplyReport
);

/// Acquire a file or directory path.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_source_open(
    path: *const c_char,
    path_len: usize,
    error: *mut *mut PioError,
) -> *mut PioSource {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let path = required_str(path, path_len, "path")?;
            Source::open(path)
                .map(PioSource::new_raw)
                .map_err(|failure| error_from_core(&failure))
        })
    }
}

/// Retain named bytes as an in-memory source. Binary content is supported.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_source_from_memory(
    name: *const c_char,
    name_len: usize,
    data: *const u8,
    data_len: usize,
    error: *mut *mut PioError,
) -> *mut PioSource {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let name = required_str(name, name_len, "name")?;
            let bytes = input_bytes(data, data_len, "data")?;
            Source::from_memory(name, bytes.to_vec())
                .map(PioSource::new_raw)
                .map_err(|failure| error_from_core(&failure))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_source_retain(source: *const PioSource) -> *mut PioSource {
    unsafe { PioSource::retain_raw(source) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_source_release(source: *mut PioSource) {
    unsafe { PioSource::release_raw(source) };
}

/// Select a filesystem output path.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_destination_path(
    path: *const c_char,
    path_len: usize,
    error: *mut *mut PioError,
) -> *mut PioDestination {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let path = required_str(path, path_len, "path")?;
            Ok(PioDestination::new_raw(DestinationSpec::Path(
                PathBuf::from(path),
            )))
        })
    }
}

/// Select memory output and prefix returned artifact names with `root`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_destination_memory(
    root: *const c_char,
    root_len: usize,
    error: *mut *mut PioError,
) -> *mut PioDestination {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let root = required_str(root, root_len, "root")?;
            Destination::memory(root).map_err(|failure| error_from_core(&failure))?;
            Ok(PioDestination::new_raw(DestinationSpec::Memory(
                root.to_owned(),
            )))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_destination_retain(
    destination: *const PioDestination,
) -> *mut PioDestination {
    unsafe { PioDestination::retain_raw(destination) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_destination_release(destination: *mut PioDestination) {
    unsafe { PioDestination::release_raw(destination) };
}

/// Parse one geographic sidecar from an acquired source.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_geo_layer_parse(
    source: *const PioSource,
    error: *mut *mut PioError,
) -> *mut PioGeoLayer {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let source = PioSource::get(source).ok_or_else(|| {
                boundary_error(&codes::BIND_CAPI_NULL_HANDLE, "PioSource must not be NULL")
            })?;
            let buffer = source
                .primary_buffer()
                .map_err(|failure| error_from_core(&failure))?;
            let text = std::str::from_utf8(buffer.content_bytes()).map_err(|_| {
                boundary_error(
                    &codes::BIND_CAPI_INVALID_UTF8,
                    "a geographic sidecar must be valid UTF-8",
                )
            })?;
            powerio::GeoLayer::parse(text, Some(source.name()))
                .map(|parsed| {
                    PioGeoLayer::new_raw(GeoLayerInner {
                        layer: parsed.layer,
                        diagnostics: parsed.diagnostics,
                    })
                })
                .map_err(|failure| error_from_tx(&failure))
        })
    }
}

/// Return diagnostics produced while parsing a geographic sidecar.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_geo_layer_diagnostics(
    layer: *const PioGeoLayer,
) -> *mut PioDiagnostics {
    unsafe { PioGeoLayer::get(layer) }.map_or(std::ptr::null_mut(), |layer| {
        PioDiagnostics::new_raw(DiagnosticsInner {
            owner: DiagnosticsOwner::Owned(layer.diagnostics.clone()),
        })
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_geo_layer_retain(layer: *const PioGeoLayer) -> *mut PioGeoLayer {
    unsafe { PioGeoLayer::retain_raw(layer) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_geo_layer_release(layer: *mut PioGeoLayer) {
    unsafe { PioGeoLayer::release_raw(layer) };
}

// ---- modules and values ----------------------------------------------------

#[derive(Clone)]
struct ModuleInner {
    module: powerio::PioModule<PioValue>,
}

opaque_handle!(
    /// PowerIO value with diagnostics, source mappings, and history.
    PioModule,
    ModuleInner
);

#[derive(Clone, Copy)]
enum ModuleJsonRoot {
    Extension(usize),
    HistoryParameter {
        history_index: usize,
        parameter_index: usize,
    },
}

#[derive(Clone, Copy)]
enum JsonValueStep {
    Array(usize),
    Object(usize),
}

struct JsonValueInner {
    owner: Arc<ModuleInner>,
    root: ModuleJsonRoot,
    steps: Vec<JsonValueStep>,
}

impl JsonValueInner {
    fn child(&self, step: JsonValueStep) -> Self {
        let mut steps = self.steps.clone();
        steps.push(step);
        Self {
            owner: Arc::clone(&self.owner),
            root: self.root,
            steps,
        }
    }

    fn value(&self) -> Option<&serde_json::Value> {
        let mut value = match self.root {
            ModuleJsonRoot::Extension(index) => {
                self.owner.module.extensions().values().nth(index)?
            }
            ModuleJsonRoot::HistoryParameter {
                history_index,
                parameter_index,
            } => self
                .owner
                .module
                .history()
                .get(history_index)?
                .parameters()
                .values()
                .nth(parameter_index)?,
        };
        for step in &self.steps {
            value = match (step, value) {
                (JsonValueStep::Array(index), serde_json::Value::Array(values)) => {
                    values.get(*index)?
                }
                (JsonValueStep::Object(index), serde_json::Value::Object(values)) => {
                    values.values().nth(*index)?
                }
                _ => return None,
            };
        }
        Some(value)
    }
}

opaque_handle!(
    /// Owner-rooted structured value from module history or extensions.
    PioJsonValue,
    JsonValueInner
);

#[derive(Clone, Copy)]
enum ValueStep {
    TimeSeries(usize),
    Scenario(usize),
}

struct ValueInner {
    owner: Arc<ModuleInner>,
    steps: Vec<ValueStep>,
}

impl ValueInner {
    fn root(owner: Arc<ModuleInner>) -> Self {
        Self {
            owner,
            steps: Vec::new(),
        }
    }

    fn child(&self, step: ValueStep) -> Self {
        let mut steps = self.steps.clone();
        steps.push(step);
        Self {
            owner: Arc::clone(&self.owner),
            steps,
        }
    }

    fn value(&self) -> Option<&PioValue> {
        let mut value = self.owner.module.value();
        for step in &self.steps {
            value = match (step, value) {
                (ValueStep::TimeSeries(index), PioValue::TimeSeries(series)) => {
                    series.get(*index)?
                }
                (ValueStep::Scenario(index), PioValue::ScenarioSet(scenarios)) => {
                    scenarios.get_at(*index)?
                }
                _ => return None,
            };
        }
        Some(value)
    }
}

opaque_handle!(
    /// Owner-rooted view of one module or collection value.
    PioValueHandle,
    ValueInner
);

#[derive(Clone, Copy)]
enum BalancedNetworkProjection {
    Direct,
    OperatingPoint,
    DcPfInstance,
    AcPfInstance,
    DcOpfInstance,
    AcOpfInstance,
    AcScucInstance,
    DcPfSolution,
    AcPfSolution,
    DcOpfSolution,
    AcOpfSolution,
    SocwrOpfSolution,
    AcScucSolution,
}

struct BalancedNetworkInner {
    value: ValueInner,
    projection: BalancedNetworkProjection,
}

impl BalancedNetworkInner {
    fn network(&self) -> Option<&BalancedNetwork> {
        match (self.projection, self.value.value()?) {
            (BalancedNetworkProjection::Direct, PioValue::BalancedNetwork(network)) => {
                Some(network)
            }
            (
                BalancedNetworkProjection::OperatingPoint,
                PioValue::BalancedOperatingPoint(point),
            ) => Some(point.network()),
            (BalancedNetworkProjection::DcPfInstance, PioValue::DcPfInstance(instance)) => {
                Some(instance.network())
            }
            (BalancedNetworkProjection::AcPfInstance, PioValue::AcPfInstance(instance)) => {
                Some(instance.network())
            }
            (BalancedNetworkProjection::DcOpfInstance, PioValue::DcOpfInstance(instance)) => {
                Some(instance.network())
            }
            (BalancedNetworkProjection::AcOpfInstance, PioValue::AcOpfInstance(instance)) => {
                Some(instance.network())
            }
            (BalancedNetworkProjection::AcScucInstance, PioValue::AcScucInstance(instance)) => {
                Some(instance.network())
            }
            (BalancedNetworkProjection::DcPfSolution, PioValue::DcPfSolution(solution)) => {
                Some(solution.network())
            }
            (BalancedNetworkProjection::AcPfSolution, PioValue::AcPfSolution(solution)) => {
                Some(solution.network())
            }
            (BalancedNetworkProjection::DcOpfSolution, PioValue::DcOpfSolution(solution)) => {
                Some(solution.network())
            }
            (BalancedNetworkProjection::AcOpfSolution, PioValue::AcOpfSolution(solution)) => {
                Some(solution.network())
            }
            (BalancedNetworkProjection::SocwrOpfSolution, PioValue::SocwrOpfSolution(solution)) => {
                Some(solution.network())
            }
            (BalancedNetworkProjection::AcScucSolution, PioValue::AcScucSolution(solution)) => {
                Some(solution.instance().network())
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
enum MulticonductorNetworkProjection {
    Direct,
    OperatingPoint,
    McAcPfInstance,
    McAcOpfInstance,
    McAcPfSolution,
    McAcOpfSolution,
}

struct MulticonductorNetworkInner {
    value: ValueInner,
    projection: MulticonductorNetworkProjection,
}

impl MulticonductorNetworkInner {
    fn network(&self) -> Option<&powerio::MulticonductorNetwork> {
        match (self.projection, self.value.value()?) {
            (MulticonductorNetworkProjection::Direct, PioValue::MulticonductorNetwork(network)) => {
                Some(network)
            }
            (
                MulticonductorNetworkProjection::OperatingPoint,
                PioValue::MulticonductorOperatingPoint(point),
            ) => Some(point.network()),
            (
                MulticonductorNetworkProjection::McAcPfInstance,
                PioValue::McAcPfInstance(instance),
            ) => Some(instance.network()),
            (
                MulticonductorNetworkProjection::McAcOpfInstance,
                PioValue::McAcOpfInstance(instance),
            ) => Some(instance.network()),
            (
                MulticonductorNetworkProjection::McAcPfSolution,
                PioValue::McAcPfSolution(solution),
            ) => Some(solution.network()),
            (
                MulticonductorNetworkProjection::McAcOpfSolution,
                PioValue::McAcOpfSolution(solution),
            ) => Some(solution.network()),
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
enum OperatingPointProjection {
    DirectBalanced,
    DirectMulticonductor,
    DcPfInitial,
    AcPfInitial,
    DcOpfInitial,
    AcOpfInitial,
    McAcPfInitial,
    McAcOpfInitial,
    DcPfSolutionInitial,
    AcPfSolutionInitial,
    DcOpfSolutionInitial,
    AcOpfSolutionInitial,
    SocwrOpfSolutionInitial,
    McAcPfSolutionInitial,
    McAcOpfSolutionInitial,
}

struct OperatingPointInner {
    value: ValueInner,
    projection: OperatingPointProjection,
}

impl OperatingPointInner {
    fn balanced(&self) -> Option<&powerio_prob::OperatingPoint<BalancedNetwork>> {
        match (self.projection, self.value.value()?) {
            (OperatingPointProjection::DirectBalanced, PioValue::BalancedOperatingPoint(point)) => {
                Some(point)
            }
            (OperatingPointProjection::DcPfInitial, PioValue::DcPfInstance(instance)) => {
                instance.initial_point()
            }
            (OperatingPointProjection::AcPfInitial, PioValue::AcPfInstance(instance)) => {
                instance.initial_point()
            }
            (OperatingPointProjection::DcOpfInitial, PioValue::DcOpfInstance(instance)) => {
                instance.initial_point()
            }
            (OperatingPointProjection::AcOpfInitial, PioValue::AcOpfInstance(instance)) => {
                instance.initial_point()
            }
            (OperatingPointProjection::DcPfSolutionInitial, PioValue::DcPfSolution(solution)) => {
                solution.instance().initial_point()
            }
            (OperatingPointProjection::AcPfSolutionInitial, PioValue::AcPfSolution(solution)) => {
                solution.instance().initial_point()
            }
            (OperatingPointProjection::DcOpfSolutionInitial, PioValue::DcOpfSolution(solution)) => {
                solution.instance().initial_point()
            }
            (OperatingPointProjection::AcOpfSolutionInitial, PioValue::AcOpfSolution(solution)) => {
                solution.instance().initial_point()
            }
            (
                OperatingPointProjection::SocwrOpfSolutionInitial,
                PioValue::SocwrOpfSolution(solution),
            ) => solution.instance().initial_point(),
            _ => None,
        }
    }

    fn multiconductor(
        &self,
    ) -> Option<&powerio_prob::OperatingPoint<powerio::MulticonductorNetwork>> {
        match (self.projection, self.value.value()?) {
            (
                OperatingPointProjection::DirectMulticonductor,
                PioValue::MulticonductorOperatingPoint(point),
            ) => Some(point),
            (OperatingPointProjection::McAcPfInitial, PioValue::McAcPfInstance(instance)) => {
                instance.initial_point()
            }
            (OperatingPointProjection::McAcOpfInitial, PioValue::McAcOpfInstance(instance)) => {
                instance.initial_point()
            }
            (
                OperatingPointProjection::McAcPfSolutionInitial,
                PioValue::McAcPfSolution(solution),
            ) => solution.instance().initial_point(),
            (
                OperatingPointProjection::McAcOpfSolutionInitial,
                PioValue::McAcOpfSolution(solution),
            ) => solution.instance().initial_point(),
            _ => None,
        }
    }

    fn type_name(&self) -> Option<&'static str> {
        if self.balanced().is_some() {
            Some("powerio.OperatingPoint<powerio.BalancedNetwork>")
        } else if self.multiconductor().is_some() {
            Some("powerio.OperatingPoint<powerio.MulticonductorNetwork>")
        } else {
            None
        }
    }

    fn balanced_network_projection(&self) -> Option<BalancedNetworkProjection> {
        self.balanced()?;
        Some(match self.projection {
            OperatingPointProjection::DirectBalanced => BalancedNetworkProjection::OperatingPoint,
            OperatingPointProjection::DcPfInitial => BalancedNetworkProjection::DcPfInstance,
            OperatingPointProjection::AcPfInitial => BalancedNetworkProjection::AcPfInstance,
            OperatingPointProjection::DcOpfInitial => BalancedNetworkProjection::DcOpfInstance,
            OperatingPointProjection::AcOpfInitial => BalancedNetworkProjection::AcOpfInstance,
            OperatingPointProjection::DcPfSolutionInitial => {
                BalancedNetworkProjection::DcPfSolution
            }
            OperatingPointProjection::AcPfSolutionInitial => {
                BalancedNetworkProjection::AcPfSolution
            }
            OperatingPointProjection::DcOpfSolutionInitial => {
                BalancedNetworkProjection::DcOpfSolution
            }
            OperatingPointProjection::AcOpfSolutionInitial => {
                BalancedNetworkProjection::AcOpfSolution
            }
            OperatingPointProjection::SocwrOpfSolutionInitial => {
                BalancedNetworkProjection::SocwrOpfSolution
            }
            _ => return None,
        })
    }

    fn multiconductor_network_projection(&self) -> Option<MulticonductorNetworkProjection> {
        self.multiconductor()?;
        Some(match self.projection {
            OperatingPointProjection::DirectMulticonductor => {
                MulticonductorNetworkProjection::OperatingPoint
            }
            OperatingPointProjection::McAcPfInitial => {
                MulticonductorNetworkProjection::McAcPfInstance
            }
            OperatingPointProjection::McAcOpfInitial => {
                MulticonductorNetworkProjection::McAcOpfInstance
            }
            OperatingPointProjection::McAcPfSolutionInitial => {
                MulticonductorNetworkProjection::McAcPfSolution
            }
            OperatingPointProjection::McAcOpfSolutionInitial => {
                MulticonductorNetworkProjection::McAcOpfSolution
            }
            _ => return None,
        })
    }
}

#[derive(Clone, Copy)]
enum CalculationInstanceProjection {
    Direct,
    DcPfSolution,
    AcPfSolution,
    DcOpfSolution,
    AcOpfSolution,
    SocwrOpfSolution,
    McAcPfSolution,
    McAcOpfSolution,
    AcScucSolution,
}

#[derive(Clone, Copy)]
enum CalculationInstanceRef<'a> {
    DcPf(&'a powerio_prob::DcPfInstance),
    AcPf(&'a powerio_prob::AcPfInstance),
    DcOpf(&'a powerio_prob::DcOpfInstance),
    AcOpf(&'a powerio_prob::AcOpfInstance),
    McAcPf(&'a powerio_prob::McAcPfInstance),
    McAcOpf(&'a powerio_prob::McAcOpfInstance),
    AcScuc(&'a powerio_prob::AcScucInstance),
}

impl CalculationInstanceRef<'_> {
    fn type_name(self) -> &'static str {
        match self {
            Self::DcPf(_) => "powerio.DcPfInstance",
            Self::AcPf(_) => "powerio.AcPfInstance",
            Self::DcOpf(_) => "powerio.DcOpfInstance",
            Self::AcOpf(_) => "powerio.AcOpfInstance",
            Self::McAcPf(_) => "powerio.McAcPfInstance",
            Self::McAcOpf(_) => "powerio.McAcOpfInstance",
            Self::AcScuc(_) => "powerio.AcScucInstance",
        }
    }
}

struct CalculationInstanceInner {
    value: ValueInner,
    projection: CalculationInstanceProjection,
}

impl CalculationInstanceInner {
    fn instance(&self) -> Option<CalculationInstanceRef<'_>> {
        match (self.projection, self.value.value()?) {
            (CalculationInstanceProjection::Direct, PioValue::DcPfInstance(instance)) => {
                Some(CalculationInstanceRef::DcPf(instance))
            }
            (CalculationInstanceProjection::Direct, PioValue::AcPfInstance(instance)) => {
                Some(CalculationInstanceRef::AcPf(instance))
            }
            (CalculationInstanceProjection::Direct, PioValue::DcOpfInstance(instance)) => {
                Some(CalculationInstanceRef::DcOpf(instance))
            }
            (CalculationInstanceProjection::Direct, PioValue::AcOpfInstance(instance)) => {
                Some(CalculationInstanceRef::AcOpf(instance))
            }
            (CalculationInstanceProjection::Direct, PioValue::McAcPfInstance(instance)) => {
                Some(CalculationInstanceRef::McAcPf(instance))
            }
            (CalculationInstanceProjection::Direct, PioValue::McAcOpfInstance(instance)) => {
                Some(CalculationInstanceRef::McAcOpf(instance))
            }
            (CalculationInstanceProjection::Direct, PioValue::AcScucInstance(instance)) => {
                Some(CalculationInstanceRef::AcScuc(instance))
            }
            (CalculationInstanceProjection::DcPfSolution, PioValue::DcPfSolution(solution)) => {
                Some(CalculationInstanceRef::DcPf(solution.instance()))
            }
            (CalculationInstanceProjection::AcPfSolution, PioValue::AcPfSolution(solution)) => {
                Some(CalculationInstanceRef::AcPf(solution.instance()))
            }
            (CalculationInstanceProjection::DcOpfSolution, PioValue::DcOpfSolution(solution)) => {
                Some(CalculationInstanceRef::DcOpf(solution.instance()))
            }
            (CalculationInstanceProjection::AcOpfSolution, PioValue::AcOpfSolution(solution)) => {
                Some(CalculationInstanceRef::AcOpf(solution.instance()))
            }
            (
                CalculationInstanceProjection::SocwrOpfSolution,
                PioValue::SocwrOpfSolution(solution),
            ) => Some(CalculationInstanceRef::AcOpf(solution.instance())),
            (CalculationInstanceProjection::McAcPfSolution, PioValue::McAcPfSolution(solution)) => {
                Some(CalculationInstanceRef::McAcPf(solution.instance()))
            }
            (
                CalculationInstanceProjection::McAcOpfSolution,
                PioValue::McAcOpfSolution(solution),
            ) => Some(CalculationInstanceRef::McAcOpf(solution.instance())),
            (CalculationInstanceProjection::AcScucSolution, PioValue::AcScucSolution(solution)) => {
                Some(CalculationInstanceRef::AcScuc(solution.instance()))
            }
            _ => None,
        }
    }

    fn type_name(&self) -> Option<&'static str> {
        self.instance().map(CalculationInstanceRef::type_name)
    }

    fn dc_pf(&self) -> Option<&powerio_prob::DcPfInstance> {
        match self.instance()? {
            CalculationInstanceRef::DcPf(instance) => Some(instance),
            _ => None,
        }
    }

    fn ac_pf(&self) -> Option<&powerio_prob::AcPfInstance> {
        match self.instance()? {
            CalculationInstanceRef::AcPf(instance) => Some(instance),
            _ => None,
        }
    }

    fn dc_opf(&self) -> Option<&powerio_prob::DcOpfInstance> {
        match self.instance()? {
            CalculationInstanceRef::DcOpf(instance) => Some(instance),
            _ => None,
        }
    }

    fn ac_opf(&self) -> Option<&powerio_prob::AcOpfInstance> {
        match self.instance()? {
            CalculationInstanceRef::AcOpf(instance) => Some(instance),
            _ => None,
        }
    }

    fn mc_ac_pf(&self) -> Option<&powerio_prob::McAcPfInstance> {
        match self.instance()? {
            CalculationInstanceRef::McAcPf(instance) => Some(instance),
            _ => None,
        }
    }

    fn ac_scuc(&self) -> Option<&powerio_prob::AcScucInstance> {
        match self.instance()? {
            CalculationInstanceRef::AcScuc(instance) => Some(instance),
            _ => None,
        }
    }

    fn has_initial_point(&self) -> bool {
        match self.instance() {
            Some(CalculationInstanceRef::DcPf(instance)) => instance.initial_point().is_some(),
            Some(CalculationInstanceRef::AcPf(instance)) => instance.initial_point().is_some(),
            Some(CalculationInstanceRef::DcOpf(instance)) => instance.initial_point().is_some(),
            Some(CalculationInstanceRef::AcOpf(instance)) => instance.initial_point().is_some(),
            Some(CalculationInstanceRef::McAcPf(instance)) => instance.initial_point().is_some(),
            Some(CalculationInstanceRef::McAcOpf(instance)) => instance.initial_point().is_some(),
            _ => false,
        }
    }
}

opaque_handle!(
    /// Owner-rooted balanced network view.
    PioBalancedNetwork,
    BalancedNetworkInner
);

struct DetailedConnectivityInner {
    owner: Arc<BalancedNetworkInner>,
}

impl DetailedConnectivityInner {
    fn details(&self) -> Option<&powerio_tx::DetailedConnectivity> {
        self.owner.network()?.detailed_connectivity().as_deref()
    }
}

opaque_handle!(
    /// Owner-rooted source neutral hierarchy and detailed connectivity view.
    PioDetailedConnectivity,
    DetailedConnectivityInner
);
opaque_handle!(
    /// Owner-rooted multiconductor network view.
    PioMulticonductorNetwork,
    MulticonductorNetworkInner
);
opaque_handle!(
    /// Owner-rooted time series view.
    PioTimeSeriesHandle,
    ValueInner
);
opaque_handle!(
    /// Owner-rooted scenario set view.
    PioScenarioSetHandle,
    ValueInner
);
opaque_handle!(
    /// Owner-rooted operating point view.
    PioOperatingPoint,
    OperatingPointInner
);
opaque_handle!(
    /// Owner-rooted calculation instance view.
    PioCalculationInstance,
    CalculationInstanceInner
);
opaque_handle!(
    /// Owned DC OPF preparation whose borrowed row views remain valid until release.
    PioDcOpfPreparation,
    DcOpfPreparation
);
opaque_handle!(
    /// Owned AC OPF preparation whose borrowed row views remain valid until release.
    PioAcOpfPreparation,
    AcOpfPreparation
);
opaque_handle!(
    /// Owner-rooted calculation solution view.
    PioCalculationSolution,
    ValueInner
);

fn module_handle(module: powerio::PioModule<PioValue>) -> *mut PioModule {
    PioModule::new_raw(ModuleInner { module })
}

fn value_handle(value: ValueInner) -> *mut PioValueHandle {
    PioValueHandle::new_raw(value)
}

unsafe fn require_value<'a>(value: *const PioValueHandle) -> Result<&'a ValueInner, *mut PioError> {
    unsafe { PioValueHandle::get(value) }.ok_or_else(|| {
        boundary_error(
            &codes::BIND_CAPI_NULL_HANDLE,
            "PioValueHandle must not be NULL",
        )
    })
}

/// Parse one acquired grid exchange source.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_parse(
    source: *const PioSource,
    format: *const c_char,
    format_len: usize,
    error: *mut *mut PioError,
) -> *mut PioModule {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let source = PioSource::get(source).ok_or_else(|| {
                boundary_error(&codes::BIND_CAPI_NULL_HANDLE, "PioSource must not be NULL")
            })?;
            let format = optional_str(format, format_len, "format")?;
            let mut options = powerio::ParseOptions::default();
            if let Some(format) = format {
                options = options
                    .format(format)
                    .map_err(|failure| error_from_core(&failure))?;
            }
            powerio::parse_with_options(source.clone(), &options)
                .map(module_handle)
                .map_err(|failure| error_from_core(&failure))
        })
    }
}

/// Deserialize one PowerIO IR source.
///
/// A document carries the independent PowerIO IR generation reported by
/// `pio_schema_report`. This library refuses any unsupported identity or
/// generation through `error`, naming what it found.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_deserialize(
    source: *const PioSource,
    error: *mut *mut PioError,
) -> *mut PioModule {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let source = PioSource::get(source).ok_or_else(|| {
                boundary_error(&codes::BIND_CAPI_NULL_HANDLE, "PioSource must not be NULL")
            })?;
            powerio::deserialize(source.clone())
                .map(module_handle)
                .map_err(|failure| error_from_core(&failure))
        })
    }
}

unsafe fn transform_module<T>(
    module: *const PioModule,
    error: *mut *mut PioError,
    transform: impl FnOnce(
        &powerio::PioModule<PioValue>,
    ) -> Result<powerio::PioModule<T>, powerio_core::Error>,
    wrap: impl FnOnce(T) -> PioValue,
) -> *mut PioModule {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let module = PioModule::get(module).ok_or_else(|| {
                boundary_error(&codes::BIND_CAPI_NULL_HANDLE, "PioModule must not be NULL")
            })?;
            transform(&module.module)
                .map(|derived| module_handle(derived.map_value(wrap)))
                .map_err(|failure| error_from_core(&failure))
        })
    }
}

/// Construct a DC power flow calculation module from a balanced network module.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_to_dc_pf_instance(
    module: *const PioModule,
    error: *mut *mut PioError,
) -> *mut PioModule {
    unsafe {
        transform_module(
            module,
            error,
            powerio::transform::to_dc_pf_instance,
            PioValue::DcPfInstance,
        )
    }
}

/// Construct an AC power flow calculation module from a balanced network module.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_to_ac_pf_instance(
    module: *const PioModule,
    error: *mut *mut PioError,
) -> *mut PioModule {
    unsafe {
        transform_module(
            module,
            error,
            powerio::transform::to_ac_pf_instance,
            PioValue::AcPfInstance,
        )
    }
}

/// Construct a DC optimal power flow calculation module from a balanced network module.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_to_dc_opf_instance(
    module: *const PioModule,
    error: *mut *mut PioError,
) -> *mut PioModule {
    unsafe {
        transform_module(
            module,
            error,
            powerio::transform::to_dc_opf_instance,
            PioValue::DcOpfInstance,
        )
    }
}

/// Construct an AC optimal power flow calculation module from a balanced network module.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_to_ac_opf_instance(
    module: *const PioModule,
    error: *mut *mut PioError,
) -> *mut PioModule {
    unsafe {
        transform_module(
            module,
            error,
            powerio::transform::to_ac_opf_instance,
            PioValue::AcOpfInstance,
        )
    }
}

/// Construct a multiconductor AC power flow calculation module from a
/// multiconductor network module.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_to_mc_ac_pf_instance(
    module: *const PioModule,
    error: *mut *mut PioError,
) -> *mut PioModule {
    unsafe {
        transform_module(
            module,
            error,
            powerio::transform::to_mc_ac_pf_instance,
            PioValue::McAcPfInstance,
        )
    }
}

/// Construct a multiconductor AC optimal power flow calculation module from a
/// multiconductor network module.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_to_mc_ac_opf_instance(
    module: *const PioModule,
    error: *mut *mut PioError,
) -> *mut PioModule {
    unsafe {
        transform_module(
            module,
            error,
            powerio::transform::to_mc_ac_opf_instance,
            PioValue::McAcOpfInstance,
        )
    }
}

/// Apply one geographic layer to a balanced or multiconductor network module.
/// The input module is unchanged. When `out_report` is not NULL, it receives
/// an independently owned report handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_apply_geo_layer(
    module: *const PioModule,
    layer: *const PioGeoLayer,
    out_report: *mut *mut PioGeoApplyReport,
    error: *mut *mut PioError,
) -> *mut PioModule {
    unsafe {
        if !out_report.is_null() {
            *out_report = std::ptr::null_mut();
        }
        entry(error, std::ptr::null_mut(), || {
            let module = PioModule::get(module).ok_or_else(|| {
                boundary_error(&codes::BIND_CAPI_NULL_HANDLE, "PioModule must not be NULL")
            })?;
            let layer = PioGeoLayer::get(layer).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_NULL_HANDLE,
                    "PioGeoLayer must not be NULL",
                )
            })?;
            let (mut derived, report) = powerio::apply_geo_layer(&module.module, &layer.layer)
                .map_err(|failure| error_from_core(&failure))?;
            for diagnostic in &layer.diagnostics {
                derived
                    .add_diagnostic(diagnostic.clone())
                    .map_err(|failure| error_from_core(&failure))?;
            }
            if !out_report.is_null() {
                *out_report = PioGeoApplyReport::new_raw(report);
            }
            Ok(module_handle(derived))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_geo_apply_report_matched_buses(
    report: *const PioGeoApplyReport,
) -> usize {
    unsafe { PioGeoApplyReport::get(report) }.map_or(0, |report| report.matched_buses)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_geo_apply_report_matched_branches(
    report: *const PioGeoApplyReport,
) -> usize {
    unsafe { PioGeoApplyReport::get(report) }.map_or(0, |report| report.matched_branches)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_geo_apply_report_unmatched_features(
    report: *const PioGeoApplyReport,
) -> usize {
    unsafe { PioGeoApplyReport::get(report) }.map_or(0, |report| report.unmatched_features)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_geo_apply_report_unlocated_buses(
    report: *const PioGeoApplyReport,
) -> usize {
    unsafe { PioGeoApplyReport::get(report) }.map_or(0, |report| report.unlocated_buses)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_geo_apply_report_unlocated_branches(
    report: *const PioGeoApplyReport,
) -> usize {
    unsafe { PioGeoApplyReport::get(report) }.map_or(0, |report| report.unlocated_branches)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_geo_apply_report_note_count(
    report: *const PioGeoApplyReport,
) -> usize {
    unsafe { PioGeoApplyReport::get(report) }.map_or(0, |report| report.notes.len())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_geo_apply_report_note_at(
    report: *const PioGeoApplyReport,
    index: usize,
    error: *mut *mut PioError,
) -> PioStringView {
    unsafe {
        entry(error, PioStringView::EMPTY, || {
            let report = PioGeoApplyReport::get(report).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_NULL_HANDLE,
                    "PioGeoApplyReport must not be NULL",
                )
            })?;
            let note = report.notes.get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("geo apply note index {index} is out of range"),
                )
            })?;
            Ok(PioStringView::new(note))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_geo_apply_report_retain(
    report: *const PioGeoApplyReport,
) -> *mut PioGeoApplyReport {
    unsafe { PioGeoApplyReport::retain_raw(report) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_geo_apply_report_release(report: *mut PioGeoApplyReport) {
    unsafe { PioGeoApplyReport::release_raw(report) };
}

/// Return an owner-rooted view of the module's value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_value(module: *const PioModule) -> *mut PioValueHandle {
    unsafe { PioModule::arc(module) }.map_or(std::ptr::null_mut(), |owner| {
        value_handle(ValueInner::root(owner))
    })
}

/// Return the module's stored diagnostics.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_diagnostics(module: *const PioModule) -> *mut PioDiagnostics {
    unsafe { PioModule::arc(module) }.map_or(std::ptr::null_mut(), |module| {
        PioDiagnostics::new_raw(DiagnosticsInner {
            owner: DiagnosticsOwner::Module(module),
        })
    })
}

fn source_relation_name(relation: powerio_core::SourceRelation) -> &'static str {
    match relation {
        powerio_core::SourceRelation::Exact => "exact",
        powerio_core::SourceRelation::Defaulted => "defaulted",
        powerio_core::SourceRelation::Inferred => "inferred",
        powerio_core::SourceRelation::ConvertedUnits => "converted_units",
        powerio_core::SourceRelation::Aggregated => "aggregated",
        powerio_core::SourceRelation::Split => "split",
        powerio_core::SourceRelation::Synthetic => "synthetic",
        powerio_core::SourceRelation::Transformed => "transformed",
        powerio_core::SourceRelation::RetainedExtra => "retained_extra",
        _ => "unknown",
    }
}

fn history_kind_name(kind: HistoryKind) -> &'static str {
    match kind {
        HistoryKind::Parse => "parse",
        HistoryKind::Transform => "transform",
        HistoryKind::Edit => "edit",
        HistoryKind::Repair => "repair",
        HistoryKind::Solve => "solve",
        _ => "unknown",
    }
}

fn json_value_kind(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

/// Read the program identity recorded with a module.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_producer(
    module: *const PioModule,
    output: *mut PioModuleProducerView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let module = PioModule::get(module).ok_or_else(|| {
                boundary_error(&codes::BIND_CAPI_NULL_HANDLE, "PioModule must not be NULL")
            })?;
            let producer = module.module.producer();
            *require_output(output, "output")? = PioModuleProducerView {
                name: PioStringView::new(producer.name()),
                version: PioStringView::new(producer.version()),
            };
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_source_count(module: *const PioModule) -> usize {
    unsafe { PioModule::get(module) }.map_or(0, |module| module.module.sources().len())
}

/// Read one durable source descriptor by zero based position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_source_at(
    module: *const PioModule,
    index: usize,
    output: *mut PioModuleSourceView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let module = PioModule::get(module).ok_or_else(|| {
                boundary_error(&codes::BIND_CAPI_NULL_HANDLE, "PioModule must not be NULL")
            })?;
            let source = module.module.sources().get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("module source index {index} is out of range"),
                )
            })?;
            let (format, has_format) = source
                .format()
                .map_or((PioStringView::EMPTY, false), |format| {
                    (PioStringView::new(format.as_str()), true)
                });
            let digest = source.digest();
            *require_output(output, "output")? = PioModuleSourceView {
                id: PioStringView::new(source.id().as_str()),
                name: PioStringView::new(source.name()),
                byte_length: source.byte_length(),
                format,
                has_format,
                digest_algorithm: digest.map_or(PioStringView::EMPTY, |digest| {
                    PioStringView::new(digest.algorithm().as_str())
                }),
                digest: digest.map_or(PioStringView::EMPTY, |digest| {
                    PioStringView::new(digest.value())
                }),
                has_digest: digest.is_some(),
            };
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_source_map_count(module: *const PioModule) -> usize {
    unsafe { PioModule::get(module) }.map_or(0, |module| module.module.source_map().len())
}

/// Read one source map entry by zero based position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_source_map_at(
    module: *const PioModule,
    index: usize,
    output: *mut PioModuleSourceMapEntryView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let module = PioModule::get(module).ok_or_else(|| {
                boundary_error(&codes::BIND_CAPI_NULL_HANDLE, "PioModule must not be NULL")
            })?;
            let source_map = module.module.source_map().get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("module source map index {index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = PioModuleSourceMapEntryView {
                target: PioStringView::new(source_map.target()),
                relation: PioStringView::new(source_relation_name(source_map.relation())),
                span_count: source_map.spans().len(),
            };
            Ok(true)
        })
    }
}

/// Read one byte range from a source map entry.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_source_map_span_at(
    module: *const PioModule,
    entry_index: usize,
    span_index: usize,
    output: *mut PioSourceSpanView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let module = PioModule::get(module).ok_or_else(|| {
                boundary_error(&codes::BIND_CAPI_NULL_HANDLE, "PioModule must not be NULL")
            })?;
            let source_map = module.module.source_map().get(entry_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("module source map index {entry_index} is out of range"),
                )
            })?;
            let span = source_map.spans().get(span_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!(
                        "source map span index {span_index} is out of range for entry {entry_index}"
                    ),
                )
            })?;
            *require_output(output, "output")? = PioSourceSpanView {
                source: PioStringView::new(span.source().as_str()),
                byte_start: span.byte_start(),
                byte_end: span.byte_end(),
            };
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_history_count(module: *const PioModule) -> usize {
    unsafe { PioModule::get(module) }.map_or(0, |module| module.module.history().len())
}

/// Read one operation from module history by zero based position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_history_at(
    module: *const PioModule,
    index: usize,
    output: *mut PioModuleHistoryEntryView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let module = PioModule::get(module).ok_or_else(|| {
                boundary_error(&codes::BIND_CAPI_NULL_HANDLE, "PioModule must not be NULL")
            })?;
            let history = module.module.history().get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("module history index {index} is out of range"),
                )
            })?;
            let (input_type, has_input_type) = optional_string_view(history.input_type());
            let (output_type, has_output_type) = optional_string_view(history.output_type());
            *require_output(output, "output")? = PioModuleHistoryEntryView {
                id: PioStringView::new(history.id().as_str()),
                kind: PioStringView::new(history_kind_name(history.kind())),
                name: PioStringView::new(history.name()),
                input_type,
                has_input_type,
                output_type,
                has_output_type,
                parameter_count: history.parameters().len(),
                assumption_count: history.assumptions().len(),
                loss_count: history.losses().len(),
            };
            Ok(true)
        })
    }
}

/// Read one named structured history parameter by zero based position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_history_parameter_at(
    module: *const PioModule,
    history_index: usize,
    parameter_index: usize,
    output: *mut PioModuleHistoryParameterView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let module = PioModule::get(module).ok_or_else(|| {
                boundary_error(&codes::BIND_CAPI_NULL_HANDLE, "PioModule must not be NULL")
            })?;
            let history = module.module.history().get(history_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("module history index {history_index} is out of range"),
                )
            })?;
            let (name, value) = history
                .parameters()
                .iter()
                .nth(parameter_index)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!(
                            "history parameter index {parameter_index} is out of range for entry {history_index}"
                        ),
                    )
                })?;
            *require_output(output, "output")? = PioModuleHistoryParameterView {
                name: PioStringView::new(name),
                value_kind: PioStringView::new(json_value_kind(value)),
            };
            Ok(true)
        })
    }
}

/// Return an owner-rooted structured history parameter value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_history_parameter_value_at(
    module: *const PioModule,
    history_index: usize,
    parameter_index: usize,
    error: *mut *mut PioError,
) -> *mut PioJsonValue {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let owner = PioModule::arc(module).ok_or_else(|| {
                boundary_error(&codes::BIND_CAPI_NULL_HANDLE, "PioModule must not be NULL")
            })?;
            let history = owner.module.history().get(history_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("module history index {history_index} is out of range"),
                )
            })?;
            if history.parameters().values().nth(parameter_index).is_none() {
                return Err(boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!(
                        "history parameter index {parameter_index} is out of range for entry {history_index}"
                    ),
                ));
            }
            Ok(PioJsonValue::new_raw(JsonValueInner {
                owner,
                root: ModuleJsonRoot::HistoryParameter {
                    history_index,
                    parameter_index,
                },
                steps: Vec::new(),
            }))
        })
    }
}

/// Read one assumption attached to a history entry.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_history_assumption_at(
    module: *const PioModule,
    history_index: usize,
    assumption_index: usize,
    error: *mut *mut PioError,
) -> PioStringView {
    unsafe {
        entry(error, PioStringView::EMPTY, || {
            let module = PioModule::get(module).ok_or_else(|| {
                boundary_error(&codes::BIND_CAPI_NULL_HANDLE, "PioModule must not be NULL")
            })?;
            let history = module.module.history().get(history_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("module history index {history_index} is out of range"),
                )
            })?;
            history
                .assumptions()
                .get(assumption_index)
                .map(|value| PioStringView::new(value))
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!(
                            "history assumption index {assumption_index} is out of range for entry {history_index}"
                        ),
                    )
                })
        })
    }
}

/// Read one declared loss attached to a history entry.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_history_loss_at(
    module: *const PioModule,
    history_index: usize,
    loss_index: usize,
    error: *mut *mut PioError,
) -> PioStringView {
    unsafe {
        entry(error, PioStringView::EMPTY, || {
            let module = PioModule::get(module).ok_or_else(|| {
                boundary_error(&codes::BIND_CAPI_NULL_HANDLE, "PioModule must not be NULL")
            })?;
            let history = module.module.history().get(history_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("module history index {history_index} is out of range"),
                )
            })?;
            history
                .losses()
                .get(loss_index)
                .map(|value| PioStringView::new(value))
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!(
                            "history loss index {loss_index} is out of range for entry {history_index}"
                        ),
                    )
                })
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_extension_count(module: *const PioModule) -> usize {
    unsafe { PioModule::get(module) }.map_or(0, |module| module.module.extensions().len())
}

/// Read one namespaced structured module extension by zero based position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_extension_at(
    module: *const PioModule,
    index: usize,
    output: *mut PioModuleExtensionView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let module = PioModule::get(module).ok_or_else(|| {
                boundary_error(&codes::BIND_CAPI_NULL_HANDLE, "PioModule must not be NULL")
            })?;
            let (namespace, value) =
                module
                    .module
                    .extensions()
                    .iter()
                    .nth(index)
                    .ok_or_else(|| {
                        boundary_error(
                            &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                            format!("module extension index {index} is out of range"),
                        )
                    })?;
            *require_output(output, "output")? = PioModuleExtensionView {
                namespace: PioStringView::new(namespace),
                value_kind: PioStringView::new(json_value_kind(value)),
            };
            Ok(true)
        })
    }
}

/// Return an owner-rooted structured module extension value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_extension_value_at(
    module: *const PioModule,
    index: usize,
    error: *mut *mut PioError,
) -> *mut PioJsonValue {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let owner = PioModule::arc(module).ok_or_else(|| {
                boundary_error(&codes::BIND_CAPI_NULL_HANDLE, "PioModule must not be NULL")
            })?;
            if owner.module.extensions().values().nth(index).is_none() {
                return Err(boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("module extension index {index} is out of range"),
                ));
            }
            Ok(PioJsonValue::new_raw(JsonValueInner {
                owner,
                root: ModuleJsonRoot::Extension(index),
                steps: Vec::new(),
            }))
        })
    }
}

fn json_value_view(value: &serde_json::Value) -> PioJsonValueView {
    let mut view = PioJsonValueView {
        kind: PioStringView::new(json_value_kind(value)),
        boolean_value: false,
        number_kind: PioStringView::EMPTY,
        signed_integer_value: 0,
        unsigned_integer_value: 0,
        floating_point_value: 0.0,
        string_value: PioStringView::EMPTY,
        element_count: 0,
    };
    match value {
        serde_json::Value::Bool(value) => view.boolean_value = *value,
        serde_json::Value::Number(value) if value.is_i64() => {
            view.number_kind = PioStringView::new("signed_integer");
            view.signed_integer_value = value.as_i64().unwrap_or_default();
        }
        serde_json::Value::Number(value) if value.is_u64() => {
            view.number_kind = PioStringView::new("unsigned_integer");
            view.unsigned_integer_value = value.as_u64().unwrap_or_default();
        }
        serde_json::Value::Number(value) => {
            view.number_kind = PioStringView::new("floating_point");
            view.floating_point_value = value.as_f64().unwrap_or(f64::NAN);
        }
        serde_json::Value::String(value) => view.string_value = PioStringView::new(value),
        serde_json::Value::Array(values) => view.element_count = values.len(),
        serde_json::Value::Object(values) => view.element_count = values.len(),
        serde_json::Value::Null => {}
    }
    view
}

/// Read the type and scalar or collection data for a structured value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_json_value_get(
    value: *const PioJsonValue,
    output: *mut PioJsonValueView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let value = PioJsonValue::get(value).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_NULL_HANDLE,
                    "PioJsonValue must not be NULL",
                )
            })?;
            let value = value.value().ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    "the structured value path is out of range",
                )
            })?;
            *require_output(output, "output")? = json_value_view(value);
            Ok(true)
        })
    }
}

/// Return one owner-rooted element from a structured JSON array.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_json_value_array_at(
    value: *const PioJsonValue,
    index: usize,
    error: *mut *mut PioError,
) -> *mut PioJsonValue {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let value = PioJsonValue::get(value).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_NULL_HANDLE,
                    "PioJsonValue must not be NULL",
                )
            })?;
            let values = value
                .value()
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::REQUEST_CAPI_TYPE_MISMATCH,
                        "the structured value is not an array",
                    )
                })?;
            if values.get(index).is_none() {
                return Err(boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("structured array index {index} is out of range"),
                ));
            }
            Ok(PioJsonValue::new_raw(
                value.child(JsonValueStep::Array(index)),
            ))
        })
    }
}

/// Read one key and value type from a structured JSON object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_json_value_object_entry_at(
    value: *const PioJsonValue,
    index: usize,
    output: *mut PioJsonObjectEntryView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let value = PioJsonValue::get(value).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_NULL_HANDLE,
                    "PioJsonValue must not be NULL",
                )
            })?;
            let values = value
                .value()
                .and_then(serde_json::Value::as_object)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::REQUEST_CAPI_TYPE_MISMATCH,
                        "the structured value is not an object",
                    )
                })?;
            let (key, value) = values.iter().nth(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("structured object index {index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = PioJsonObjectEntryView {
                key: PioStringView::new(key),
                value_kind: PioStringView::new(json_value_kind(value)),
            };
            Ok(true)
        })
    }
}

/// Return one owner-rooted value from a structured JSON object by position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_json_value_object_value_at(
    value: *const PioJsonValue,
    index: usize,
    error: *mut *mut PioError,
) -> *mut PioJsonValue {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let value = PioJsonValue::get(value).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_NULL_HANDLE,
                    "PioJsonValue must not be NULL",
                )
            })?;
            let values = value
                .value()
                .and_then(serde_json::Value::as_object)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::REQUEST_CAPI_TYPE_MISMATCH,
                        "the structured value is not an object",
                    )
                })?;
            if values.values().nth(index).is_none() {
                return Err(boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("structured object index {index} is out of range"),
                ));
            }
            Ok(PioJsonValue::new_raw(
                value.child(JsonValueStep::Object(index)),
            ))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_json_value_retain(value: *const PioJsonValue) -> *mut PioJsonValue {
    unsafe { PioJsonValue::retain_raw(value) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_json_value_release(value: *mut PioJsonValue) {
    unsafe { PioJsonValue::release_raw(value) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_retain(module: *const PioModule) -> *mut PioModule {
    unsafe { PioModule::retain_raw(module) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_release(module: *mut PioModule) {
    unsafe { PioModule::release_raw(module) };
}

/// Canonical structural type name, such as `powerio.BalancedNetwork`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_value_type_name(value: *const PioValueHandle) -> PioStringView {
    unsafe { PioValueHandle::get(value) }
        .and_then(ValueInner::value)
        .map_or(PioStringView::EMPTY, |value| {
            PioStringView::new(value.type_name())
        })
}

/// Exact structural type predicate.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_value_is_type(
    value: *const PioValueHandle,
    type_name: *const c_char,
    type_name_len: usize,
) -> bool {
    let Some(value) = unsafe { PioValueHandle::get(value) }.and_then(ValueInner::value) else {
        return false;
    };
    let Ok(type_name) = (unsafe { input_str(type_name, type_name_len, "type_name") }) else {
        return false;
    };
    value.type_name() == type_name
}

/// Borrow the value as a balanced network without serialization or copying.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_value_balanced_network(
    value: *const PioValueHandle,
    error: *mut *mut PioError,
) -> *mut PioBalancedNetwork {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let value = require_value(value)?;
            if !matches!(value.value(), Some(PioValue::BalancedNetwork(_))) {
                return Err(boundary_error(
                    &codes::REQUEST_CAPI_TYPE_MISMATCH,
                    "the value is not powerio.BalancedNetwork",
                ));
            }
            Ok(PioBalancedNetwork::new_raw(BalancedNetworkInner {
                value: ValueInner {
                    owner: Arc::clone(&value.owner),
                    steps: value.steps.clone(),
                },
                projection: BalancedNetworkProjection::Direct,
            }))
        })
    }
}

/// Take the value as a geographic layer handle. The layer is copied out of
/// the value, so the handle outlives the module the way
/// `pio_geo_layer_parse` produces one.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_value_geo_layer(
    value: *const PioValueHandle,
    error: *mut *mut PioError,
) -> *mut PioGeoLayer {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let value = require_value(value)?;
            let Some(PioValue::GeoLayer(layer)) = value.value() else {
                return Err(boundary_error(
                    &codes::REQUEST_CAPI_TYPE_MISMATCH,
                    "the value is not powerio.GeoLayer",
                ));
            };
            Ok(PioGeoLayer::new_raw(GeoLayerInner {
                layer: layer.clone(),
                diagnostics: Vec::new(),
            }))
        })
    }
}

/// Borrow the value as a multiconductor network without serialization or copying.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_value_multiconductor_network(
    value: *const PioValueHandle,
    error: *mut *mut PioError,
) -> *mut PioMulticonductorNetwork {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let value = require_value(value)?;
            if !matches!(value.value(), Some(PioValue::MulticonductorNetwork(_))) {
                return Err(boundary_error(
                    &codes::REQUEST_CAPI_TYPE_MISMATCH,
                    "the value is not powerio.MulticonductorNetwork",
                ));
            }
            Ok(PioMulticonductorNetwork::new_raw(
                MulticonductorNetworkInner {
                    value: ValueInner {
                        owner: Arc::clone(&value.owner),
                        steps: value.steps.clone(),
                    },
                    projection: MulticonductorNetworkProjection::Direct,
                },
            ))
        })
    }
}

/// Borrow the value as a time series.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_value_time_series(
    value: *const PioValueHandle,
    error: *mut *mut PioError,
) -> *mut PioTimeSeriesHandle {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let value = require_value(value)?;
            if !matches!(value.value(), Some(PioValue::TimeSeries(_))) {
                return Err(boundary_error(
                    &codes::REQUEST_CAPI_TYPE_MISMATCH,
                    "the value is not powerio.TimeSeries<T>",
                ));
            }
            Ok(PioTimeSeriesHandle::new_raw(ValueInner {
                owner: Arc::clone(&value.owner),
                steps: value.steps.clone(),
            }))
        })
    }
}

/// Borrow the value as a scenario set.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_value_scenario_set(
    value: *const PioValueHandle,
    error: *mut *mut PioError,
) -> *mut PioScenarioSetHandle {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let value = require_value(value)?;
            if !matches!(value.value(), Some(PioValue::ScenarioSet(_))) {
                return Err(boundary_error(
                    &codes::REQUEST_CAPI_TYPE_MISMATCH,
                    "the value is not powerio.ScenarioSet<T>",
                ));
            }
            Ok(PioScenarioSetHandle::new_raw(ValueInner {
                owner: Arc::clone(&value.owner),
                steps: value.steps.clone(),
            }))
        })
    }
}

#[derive(Clone, Copy)]
enum ExpectedValue {
    BalancedOperatingPoint,
    MulticonductorOperatingPoint,
    DcPfInstance,
    AcPfInstance,
    DcOpfInstance,
    AcOpfInstance,
    McAcPfInstance,
    McAcOpfInstance,
    AcScucInstance,
    DcPfSolution,
    AcPfSolution,
    DcOpfSolution,
    AcOpfSolution,
    SocwrOpfSolution,
    McAcPfSolution,
    McAcOpfSolution,
    AcScucSolution,
}

impl ExpectedValue {
    fn matches(self, value: &PioValue) -> bool {
        matches!(
            (self, value),
            (
                Self::BalancedOperatingPoint,
                PioValue::BalancedOperatingPoint(_)
            ) | (
                Self::MulticonductorOperatingPoint,
                PioValue::MulticonductorOperatingPoint(_)
            ) | (Self::DcPfInstance, PioValue::DcPfInstance(_))
                | (Self::AcPfInstance, PioValue::AcPfInstance(_))
                | (Self::DcOpfInstance, PioValue::DcOpfInstance(_))
                | (Self::AcOpfInstance, PioValue::AcOpfInstance(_))
                | (Self::McAcPfInstance, PioValue::McAcPfInstance(_))
                | (Self::McAcOpfInstance, PioValue::McAcOpfInstance(_))
                | (Self::AcScucInstance, PioValue::AcScucInstance(_))
                | (Self::DcPfSolution, PioValue::DcPfSolution(_))
                | (Self::AcPfSolution, PioValue::AcPfSolution(_))
                | (Self::DcOpfSolution, PioValue::DcOpfSolution(_))
                | (Self::AcOpfSolution, PioValue::AcOpfSolution(_))
                | (Self::SocwrOpfSolution, PioValue::SocwrOpfSolution(_))
                | (Self::McAcPfSolution, PioValue::McAcPfSolution(_))
                | (Self::McAcOpfSolution, PioValue::McAcOpfSolution(_))
                | (Self::AcScucSolution, PioValue::AcScucSolution(_))
        )
    }

    fn type_name(self) -> &'static str {
        match self {
            Self::BalancedOperatingPoint => "powerio.OperatingPoint<powerio.BalancedNetwork>",
            Self::MulticonductorOperatingPoint => {
                "powerio.OperatingPoint<powerio.MulticonductorNetwork>"
            }
            Self::DcPfInstance => "powerio.DcPfInstance",
            Self::AcPfInstance => "powerio.AcPfInstance",
            Self::DcOpfInstance => "powerio.DcOpfInstance",
            Self::AcOpfInstance => "powerio.AcOpfInstance",
            Self::McAcPfInstance => "powerio.McAcPfInstance",
            Self::McAcOpfInstance => "powerio.McAcOpfInstance",
            Self::AcScucInstance => "powerio.AcScucInstance",
            Self::DcPfSolution => "powerio.DcPfSolution",
            Self::AcPfSolution => "powerio.AcPfSolution",
            Self::DcOpfSolution => "powerio.DcOpfSolution",
            Self::AcOpfSolution => "powerio.AcOpfSolution",
            Self::SocwrOpfSolution => "powerio.SocwrOpfSolution",
            Self::McAcPfSolution => "powerio.McAcPfSolution",
            Self::McAcOpfSolution => "powerio.McAcOpfSolution",
            Self::AcScucSolution => "powerio.AcScucSolution",
        }
    }
}

unsafe fn checked_typed_value(
    value: *const PioValueHandle,
    expected: ExpectedValue,
) -> Result<ValueInner, *mut PioError> {
    let value = unsafe { require_value(value) }?;
    if !value.value().is_some_and(|value| expected.matches(value)) {
        return Err(boundary_error(
            &codes::REQUEST_CAPI_TYPE_MISMATCH,
            format!("the value is not {}", expected.type_name()),
        ));
    }
    Ok(ValueInner {
        owner: Arc::clone(&value.owner),
        steps: value.steps.clone(),
    })
}

unsafe fn operating_point_accessor(
    value: *const PioValueHandle,
    expected: ExpectedValue,
    error: *mut *mut PioError,
) -> *mut PioOperatingPoint {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let value = checked_typed_value(value, expected)?;
            let projection = match expected {
                ExpectedValue::BalancedOperatingPoint => OperatingPointProjection::DirectBalanced,
                ExpectedValue::MulticonductorOperatingPoint => {
                    OperatingPointProjection::DirectMulticonductor
                }
                _ => {
                    return Err(boundary_error(
                        &codes::REQUEST_CAPI_TYPE_MISMATCH,
                        "the requested value is not an operating point",
                    ));
                }
            };
            Ok(PioOperatingPoint::new_raw(OperatingPointInner {
                value,
                projection,
            }))
        })
    }
}

unsafe fn instance_accessor(
    value: *const PioValueHandle,
    expected: ExpectedValue,
    error: *mut *mut PioError,
) -> *mut PioCalculationInstance {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            checked_typed_value(value, expected).map(|value| {
                PioCalculationInstance::new_raw(CalculationInstanceInner {
                    value,
                    projection: CalculationInstanceProjection::Direct,
                })
            })
        })
    }
}

unsafe fn solution_accessor(
    value: *const PioValueHandle,
    expected: ExpectedValue,
    error: *mut *mut PioError,
) -> *mut PioCalculationSolution {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            checked_typed_value(value, expected).map(PioCalculationSolution::new_raw)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_value_balanced_operating_point(
    value: *const PioValueHandle,
    error: *mut *mut PioError,
) -> *mut PioOperatingPoint {
    unsafe { operating_point_accessor(value, ExpectedValue::BalancedOperatingPoint, error) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_value_multiconductor_operating_point(
    value: *const PioValueHandle,
    error: *mut *mut PioError,
) -> *mut PioOperatingPoint {
    unsafe { operating_point_accessor(value, ExpectedValue::MulticonductorOperatingPoint, error) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_value_dc_pf_instance(
    value: *const PioValueHandle,
    error: *mut *mut PioError,
) -> *mut PioCalculationInstance {
    unsafe { instance_accessor(value, ExpectedValue::DcPfInstance, error) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_value_ac_pf_instance(
    value: *const PioValueHandle,
    error: *mut *mut PioError,
) -> *mut PioCalculationInstance {
    unsafe { instance_accessor(value, ExpectedValue::AcPfInstance, error) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_value_dc_opf_instance(
    value: *const PioValueHandle,
    error: *mut *mut PioError,
) -> *mut PioCalculationInstance {
    unsafe { instance_accessor(value, ExpectedValue::DcOpfInstance, error) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_value_ac_opf_instance(
    value: *const PioValueHandle,
    error: *mut *mut PioError,
) -> *mut PioCalculationInstance {
    unsafe { instance_accessor(value, ExpectedValue::AcOpfInstance, error) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_value_mc_ac_pf_instance(
    value: *const PioValueHandle,
    error: *mut *mut PioError,
) -> *mut PioCalculationInstance {
    unsafe { instance_accessor(value, ExpectedValue::McAcPfInstance, error) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_value_mc_ac_opf_instance(
    value: *const PioValueHandle,
    error: *mut *mut PioError,
) -> *mut PioCalculationInstance {
    unsafe { instance_accessor(value, ExpectedValue::McAcOpfInstance, error) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_value_ac_scuc_instance(
    value: *const PioValueHandle,
    error: *mut *mut PioError,
) -> *mut PioCalculationInstance {
    unsafe { instance_accessor(value, ExpectedValue::AcScucInstance, error) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_value_dc_pf_solution(
    value: *const PioValueHandle,
    error: *mut *mut PioError,
) -> *mut PioCalculationSolution {
    unsafe { solution_accessor(value, ExpectedValue::DcPfSolution, error) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_value_ac_pf_solution(
    value: *const PioValueHandle,
    error: *mut *mut PioError,
) -> *mut PioCalculationSolution {
    unsafe { solution_accessor(value, ExpectedValue::AcPfSolution, error) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_value_dc_opf_solution(
    value: *const PioValueHandle,
    error: *mut *mut PioError,
) -> *mut PioCalculationSolution {
    unsafe { solution_accessor(value, ExpectedValue::DcOpfSolution, error) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_value_ac_opf_solution(
    value: *const PioValueHandle,
    error: *mut *mut PioError,
) -> *mut PioCalculationSolution {
    unsafe { solution_accessor(value, ExpectedValue::AcOpfSolution, error) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_value_socwr_opf_solution(
    value: *const PioValueHandle,
    error: *mut *mut PioError,
) -> *mut PioCalculationSolution {
    unsafe { solution_accessor(value, ExpectedValue::SocwrOpfSolution, error) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_value_mc_ac_pf_solution(
    value: *const PioValueHandle,
    error: *mut *mut PioError,
) -> *mut PioCalculationSolution {
    unsafe { solution_accessor(value, ExpectedValue::McAcPfSolution, error) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_value_mc_ac_opf_solution(
    value: *const PioValueHandle,
    error: *mut *mut PioError,
) -> *mut PioCalculationSolution {
    unsafe { solution_accessor(value, ExpectedValue::McAcOpfSolution, error) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_value_ac_scuc_solution(
    value: *const PioValueHandle,
    error: *mut *mut PioError,
) -> *mut PioCalculationSolution {
    unsafe { solution_accessor(value, ExpectedValue::AcScucSolution, error) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_operating_point_type_name(
    point: *const PioOperatingPoint,
) -> PioStringView {
    unsafe { PioOperatingPoint::get(point) }
        .and_then(OperatingPointInner::type_name)
        .map_or(PioStringView::EMPTY, PioStringView::new)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_calculation_instance_type_name(
    instance: *const PioCalculationInstance,
) -> PioStringView {
    unsafe { PioCalculationInstance::get(instance) }
        .and_then(CalculationInstanceInner::type_name)
        .map_or(PioStringView::EMPTY, PioStringView::new)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_calculation_solution_type_name(
    solution: *const PioCalculationSolution,
) -> PioStringView {
    unsafe { PioCalculationSolution::get(solution) }
        .and_then(ValueInner::value)
        .map_or(PioStringView::EMPTY, |value| {
            PioStringView::new(value.type_name())
        })
}

/// Return an owner-rooted view of the exact calculation instance retained by
/// a calculation solution.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_calculation_solution_instance(
    solution: *const PioCalculationSolution,
    error: *mut *mut PioError,
) -> *mut PioCalculationInstance {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let solution = PioCalculationSolution::get(solution).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_NULL_HANDLE,
                    "PioCalculationSolution must not be NULL",
                )
            })?;
            let projection = match solution.value() {
                Some(PioValue::DcPfSolution(_)) => CalculationInstanceProjection::DcPfSolution,
                Some(PioValue::AcPfSolution(_)) => CalculationInstanceProjection::AcPfSolution,
                Some(PioValue::DcOpfSolution(_)) => CalculationInstanceProjection::DcOpfSolution,
                Some(PioValue::AcOpfSolution(_)) => CalculationInstanceProjection::AcOpfSolution,
                Some(PioValue::SocwrOpfSolution(_)) => {
                    CalculationInstanceProjection::SocwrOpfSolution
                }
                Some(PioValue::McAcPfSolution(_)) => CalculationInstanceProjection::McAcPfSolution,
                Some(PioValue::McAcOpfSolution(_)) => {
                    CalculationInstanceProjection::McAcOpfSolution
                }
                Some(PioValue::AcScucSolution(_)) => CalculationInstanceProjection::AcScucSolution,
                _ => {
                    return Err(boundary_error(
                        &codes::REQUEST_CAPI_TYPE_MISMATCH,
                        "the handle does not refer to a calculation solution",
                    ));
                }
            };
            Ok(PioCalculationInstance::new_raw(CalculationInstanceInner {
                value: ValueInner {
                    owner: Arc::clone(&solution.owner),
                    steps: solution.steps.clone(),
                },
                projection,
            }))
        })
    }
}

fn make_balanced_network_view(
    value: &ValueInner,
    projection: BalancedNetworkProjection,
) -> *mut PioBalancedNetwork {
    PioBalancedNetwork::new_raw(BalancedNetworkInner {
        value: ValueInner {
            owner: Arc::clone(&value.owner),
            steps: value.steps.clone(),
        },
        projection,
    })
}

fn make_multiconductor_network_view(
    value: &ValueInner,
    projection: MulticonductorNetworkProjection,
) -> *mut PioMulticonductorNetwork {
    PioMulticonductorNetwork::new_raw(MulticonductorNetworkInner {
        value: ValueInner {
            owner: Arc::clone(&value.owner),
            steps: value.steps.clone(),
        },
        projection,
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_operating_point_balanced_network(
    point: *const PioOperatingPoint,
    error: *mut *mut PioError,
) -> *mut PioBalancedNetwork {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let point = PioOperatingPoint::get(point).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_NULL_HANDLE,
                    "PioOperatingPoint must not be NULL",
                )
            })?;
            let projection = point.balanced_network_projection().ok_or_else(|| {
                boundary_error(
                    &codes::REQUEST_CAPI_TYPE_MISMATCH,
                    "the operating point does not use powerio.BalancedNetwork",
                )
            })?;
            Ok(make_balanced_network_view(&point.value, projection))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_operating_point_multiconductor_network(
    point: *const PioOperatingPoint,
    error: *mut *mut PioError,
) -> *mut PioMulticonductorNetwork {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let point = PioOperatingPoint::get(point).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_NULL_HANDLE,
                    "PioOperatingPoint must not be NULL",
                )
            })?;
            let projection = point.multiconductor_network_projection().ok_or_else(|| {
                boundary_error(
                    &codes::REQUEST_CAPI_TYPE_MISMATCH,
                    "the operating point does not use powerio.MulticonductorNetwork",
                )
            })?;
            Ok(make_multiconductor_network_view(&point.value, projection))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_calculation_instance_balanced_network(
    instance: *const PioCalculationInstance,
    error: *mut *mut PioError,
) -> *mut PioBalancedNetwork {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let instance = PioCalculationInstance::get(instance).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_NULL_HANDLE,
                    "PioCalculationInstance must not be NULL",
                )
            })?;
            let projection = match instance.projection {
                CalculationInstanceProjection::Direct => match instance.instance() {
                    Some(CalculationInstanceRef::DcPf(_)) => {
                        BalancedNetworkProjection::DcPfInstance
                    }
                    Some(CalculationInstanceRef::AcPf(_)) => {
                        BalancedNetworkProjection::AcPfInstance
                    }
                    Some(CalculationInstanceRef::DcOpf(_)) => {
                        BalancedNetworkProjection::DcOpfInstance
                    }
                    Some(CalculationInstanceRef::AcOpf(_)) => {
                        BalancedNetworkProjection::AcOpfInstance
                    }
                    Some(CalculationInstanceRef::AcScuc(_)) => {
                        BalancedNetworkProjection::AcScucInstance
                    }
                    _ => {
                        return Err(boundary_error(
                            &codes::REQUEST_CAPI_TYPE_MISMATCH,
                            "the calculation instance does not use powerio.BalancedNetwork",
                        ));
                    }
                },
                CalculationInstanceProjection::DcPfSolution => {
                    BalancedNetworkProjection::DcPfSolution
                }
                CalculationInstanceProjection::AcPfSolution => {
                    BalancedNetworkProjection::AcPfSolution
                }
                CalculationInstanceProjection::DcOpfSolution => {
                    BalancedNetworkProjection::DcOpfSolution
                }
                CalculationInstanceProjection::AcOpfSolution => {
                    BalancedNetworkProjection::AcOpfSolution
                }
                CalculationInstanceProjection::SocwrOpfSolution => {
                    BalancedNetworkProjection::SocwrOpfSolution
                }
                CalculationInstanceProjection::AcScucSolution => {
                    BalancedNetworkProjection::AcScucSolution
                }
                _ => {
                    return Err(boundary_error(
                        &codes::REQUEST_CAPI_TYPE_MISMATCH,
                        "the calculation instance does not use powerio.BalancedNetwork",
                    ));
                }
            };
            Ok(make_balanced_network_view(&instance.value, projection))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_calculation_instance_multiconductor_network(
    instance: *const PioCalculationInstance,
    error: *mut *mut PioError,
) -> *mut PioMulticonductorNetwork {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let instance = PioCalculationInstance::get(instance).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_NULL_HANDLE,
                    "PioCalculationInstance must not be NULL",
                )
            })?;
            let projection = match instance.projection {
                CalculationInstanceProjection::Direct => match instance.instance() {
                    Some(CalculationInstanceRef::McAcPf(_)) => {
                        MulticonductorNetworkProjection::McAcPfInstance
                    }
                    Some(CalculationInstanceRef::McAcOpf(_)) => {
                        MulticonductorNetworkProjection::McAcOpfInstance
                    }
                    _ => {
                        return Err(boundary_error(
                            &codes::REQUEST_CAPI_TYPE_MISMATCH,
                            "the calculation instance does not use powerio.MulticonductorNetwork",
                        ));
                    }
                },
                CalculationInstanceProjection::McAcPfSolution => {
                    MulticonductorNetworkProjection::McAcPfSolution
                }
                CalculationInstanceProjection::McAcOpfSolution => {
                    MulticonductorNetworkProjection::McAcOpfSolution
                }
                _ => {
                    return Err(boundary_error(
                        &codes::REQUEST_CAPI_TYPE_MISMATCH,
                        "the calculation instance does not use powerio.MulticonductorNetwork",
                    ));
                }
            };
            Ok(make_multiconductor_network_view(
                &instance.value,
                projection,
            ))
        })
    }
}

fn branch_susceptance_formula_name(formula: BranchSusceptanceFormula) -> &'static str {
    match formula {
        BranchSusceptanceFormula::ReactanceOnly => "reactance_only",
        BranchSusceptanceFormula::TapAdjustedReactance => "tap_adjusted_reactance",
        BranchSusceptanceFormula::SeriesSusceptance => "series_susceptance",
        _ => "unknown",
    }
}

unsafe fn require_calculation_instance<'a>(
    instance: *const PioCalculationInstance,
) -> Result<&'a CalculationInstanceInner, *mut PioError> {
    unsafe { PioCalculationInstance::get(instance) }.ok_or_else(|| {
        boundary_error(
            &codes::BIND_CAPI_NULL_HANDLE,
            "PioCalculationInstance must not be NULL",
        )
    })
}

fn opf_preparation_units(name: &str) -> Result<Units, *mut PioError> {
    match name {
        "per_unit" => Ok(Units::PerUnit),
        "native" => Ok(Units::Native),
        _ => Err(boundary_error(
            &codes::BIND_CAPI_INVALID_OPTIONS,
            format!("unknown OPF preparation units '{name}'; expected 'per_unit' or 'native'"),
        )),
    }
}

fn opf_preparation_units_name(units: Units) -> Option<&'static str> {
    match units {
        Units::PerUnit => Some("per_unit"),
        Units::Native => Some("native"),
        _ => None,
    }
}

fn prepared_objective_name(objective: PreparedObjective) -> Option<&'static str> {
    match objective {
        PreparedObjective::Feasibility => Some("feasibility"),
        PreparedObjective::NetworkGeneratorCost => Some("network_generator_cost"),
        _ => None,
    }
}

fn opf_analysis_branch_source(
    source: AnalysisBranchSource,
    preparation: &str,
) -> Result<(&'static str, usize, Option<usize>), *mut PioError> {
    match source {
        AnalysisBranchSource::Branch { row } => Ok(("branch", row, None)),
        AnalysisBranchSource::ThreeWindingTransformerWinding {
            transformer_row,
            winding,
        } => Ok((
            "three_winding_transformer_winding",
            transformer_row,
            Some(winding),
        )),
        _ => Err(boundary_error(
            &codes::REQUEST_CAPI_TYPE_MISMATCH,
            format!(
                "the {preparation} OPF preparation has an analysis branch source unsupported by ABI 7"
            ),
        )),
    }
}

/// Build the matrix free DC OPF inputs from one typed instance.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_build_dc_opf_preparation(
    instance: *const PioCalculationInstance,
    units: *const c_char,
    units_len: usize,
    skip_zero_impedance: bool,
    synthesize_unrated_limits: bool,
    correct_angle_difference_bounds: bool,
    error: *mut *mut PioError,
) -> *mut PioDcOpfPreparation {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let instance = require_calculation_instance(instance)?
                .dc_opf()
                .ok_or_else(|| {
                    boundary_error(
                        &codes::REQUEST_CAPI_TYPE_MISMATCH,
                        "DC OPF preparation requires powerio.DcOpfInstance",
                    )
                })?;
            let units = opf_preparation_units(required_str(units, units_len, "units")?)?;
            let options = DcOpfAssemblyOptions::default()
                .with_units(units)
                .with_skip_zero_impedance(skip_zero_impedance)
                .with_synthesize_unrated_limits(synthesize_unrated_limits)
                .with_correct_angle_difference_bounds(correct_angle_difference_bounds);
            build_dc_opf_preparation(instance, &options)
                .map(PioDcOpfPreparation::new_raw)
                .map_err(|failure| error_from_matrix(&failure))
        })
    }
}

/// Read the dimensions and conventions of a DC OPF preparation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_dc_opf_preparation_summary(
    preparation: *const PioDcOpfPreparation,
    output: *mut PioDcOpfPreparationView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let preparation = PioDcOpfPreparation::get(preparation).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_NULL_HANDLE,
                    "PioDcOpfPreparation must not be NULL",
                )
            })?;
            let units = opf_preparation_units_name(preparation.units).ok_or_else(|| {
                boundary_error(
                    &codes::REQUEST_CAPI_TYPE_MISMATCH,
                    "the DC OPF preparation uses units unsupported by ABI 7",
                )
            })?;
            let objective = prepared_objective_name(preparation.objective).ok_or_else(|| {
                boundary_error(
                    &codes::REQUEST_CAPI_TYPE_MISMATCH,
                    "the DC OPF preparation uses an objective unsupported by ABI 7",
                )
            })?;
            *require_output(output, "output")? = PioDcOpfPreparationView {
                name: PioStringView::new(&preparation.name),
                bus_count: preparation.n_buses,
                generator_count: preparation.n_generators(),
                branch_count: preparation.n_branches(),
                source_generator_count: preparation.n_source_generators,
                source_branch_count: preparation.n_source_branches,
                base_mva: preparation.base_mva,
                units: PioStringView::new(units),
                branch_susceptance_formula: PioStringView::new(branch_susceptance_formula_name(
                    preparation.formula,
                )),
                objective: PioStringView::new(objective),
                skip_zero_impedance: preparation.skip_zero_impedance,
                synthesize_unrated_limits: preparation.synthesize_unrated_limits,
                correct_angle_difference_bounds: preparation.correct_angle_difference_bounds,
                reference_bus_count: preparation.reference_buses.len(),
                skipped_zero_impedance_count: preparation.branches.skipped_zero_impedance.len(),
            };
            Ok(true)
        })
    }
}

/// Borrow the dense reference bus indices of a DC OPF preparation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_dc_opf_preparation_reference_buses(
    preparation: *const PioDcOpfPreparation,
) -> PioSizeView {
    unsafe { PioDcOpfPreparation::get(preparation) }.map_or(PioSizeView::EMPTY, |value| {
        PioSizeView::new(value.reference_buses.as_ref())
    })
}

/// Borrow the analysis rows skipped for zero impedance.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_dc_opf_preparation_skipped_zero_impedance(
    preparation: *const PioDcOpfPreparation,
) -> PioSizeView {
    unsafe { PioDcOpfPreparation::get(preparation) }.map_or(PioSizeView::EMPTY, |value| {
        PioSizeView::new(&value.branches.skipped_zero_impedance)
    })
}

/// Read one dense bus row of a DC OPF preparation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_dc_opf_preparation_bus_at(
    preparation: *const PioDcOpfPreparation,
    index: usize,
    output: *mut PioDcOpfBusView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let value = PioDcOpfPreparation::get(preparation).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_NULL_HANDLE,
                    "PioDcOpfPreparation must not be NULL",
                )
            })?;
            if index >= value.n_buses {
                return Err(boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("DC OPF preparation bus index {index} is out of range"),
                ));
            }
            let source_row = value.bus_source_rows[index];
            *require_output(output, "output")? = PioDcOpfBusView {
                bus_id: value.bus_ids[index].0,
                analysis_row: value.bus_analysis_rows[index],
                source_row: source_row.unwrap_or(0),
                has_source_row: source_row.is_some(),
                active_power_demand: value.p_d[index],
                shunt_conductance: value.g_s[index],
                phase_shift_injection: value.p_shift[index],
            };
            Ok(true)
        })
    }
}

/// Read one generator row of a DC OPF preparation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_dc_opf_preparation_generator_at(
    preparation: *const PioDcOpfPreparation,
    index: usize,
    output: *mut PioDcOpfGeneratorView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let value = PioDcOpfPreparation::get(preparation).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_NULL_HANDLE,
                    "PioDcOpfPreparation must not be NULL",
                )
            })?;
            if index >= value.n_generators() {
                return Err(boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("DC OPF preparation generator index {index} is out of range"),
                ));
            }
            let source_row = value.generators.source_rows[index];
            let piecewise = value.generators.piecewise_linear[index].as_ref();
            *require_output(output, "output")? = PioDcOpfGeneratorView {
                component_id: PioStringView::new(&value.generators.identities[index]),
                bus_index: value.generators.bus_of_gen[index],
                analysis_row: value.generators.analysis_rows[index],
                source_row: source_row.unwrap_or(0),
                has_source_row: source_row.is_some(),
                quadratic_cost: value.generators.q[index],
                linear_cost: value.generators.c[index],
                constant_cost: value.generators.c0[index],
                has_piecewise_linear_cost: piecewise.is_some(),
                piecewise_linear_power: piecewise
                    .map_or(PioF64View::EMPTY, |cost| PioF64View::new(&cost.power)),
                piecewise_linear_value: piecewise
                    .map_or(PioF64View::EMPTY, |cost| PioF64View::new(&cost.value)),
                active_power_max: value.generators.pmax[index],
                active_power_min: value.generators.pmin[index],
                capability_active: value.generators.capability_active[index],
            };
            Ok(true)
        })
    }
}

/// Read one active branch row of a DC OPF preparation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_dc_opf_preparation_branch_at(
    preparation: *const PioDcOpfPreparation,
    index: usize,
    output: *mut PioDcOpfBranchView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let value = PioDcOpfPreparation::get(preparation).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_NULL_HANDLE,
                    "PioDcOpfPreparation must not be NULL",
                )
            })?;
            if index >= value.n_branches() {
                return Err(boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("DC OPF preparation branch index {index} is out of range"),
                ));
            }
            let (source_kind, source_row, winding) =
                opf_analysis_branch_source(value.branches.analysis_sources[index], "DC")?;
            *require_output(output, "output")? = PioDcOpfBranchView {
                component_id: PioStringView::new(&value.branches.identities[index]),
                from_bus_index: value.branches.from_bus[index],
                to_bus_index: value.branches.to_bus[index],
                susceptance_magnitude: value.branches.susceptance_magnitude[index],
                phase_shift_radians: value.branches.shift[index],
                active_power_max: value.branches.f_max[index],
                angle_difference_min_radians: value.branches.angle_min[index],
                angle_difference_max_radians: value.branches.angle_max[index],
                analysis_row: value.branches.analysis_rows[index],
                source_kind: PioStringView::new(source_kind),
                source_row,
                winding: winding.unwrap_or(0),
                has_winding: winding.is_some(),
                thermal_limit_active: value.branches.thermal_limit_active[index],
                angle_bound_active: value.branches.angle_bound_active[index],
            };
            Ok(true)
        })
    }
}

/// Build the matrix free AC OPF inputs from one typed instance.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_build_ac_opf_preparation(
    instance: *const PioCalculationInstance,
    units: *const c_char,
    units_len: usize,
    skip_zero_impedance: bool,
    synthesize_unrated_limits: bool,
    correct_angle_difference_bounds: bool,
    error: *mut *mut PioError,
) -> *mut PioAcOpfPreparation {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let instance = require_calculation_instance(instance)?
                .ac_opf()
                .ok_or_else(|| {
                    boundary_error(
                        &codes::REQUEST_CAPI_TYPE_MISMATCH,
                        "AC OPF preparation requires powerio.AcOpfInstance",
                    )
                })?;
            let units = opf_preparation_units(required_str(units, units_len, "units")?)?;
            let options = AcOpfAssemblyOptions::default()
                .with_units(units)
                .with_skip_zero_impedance(skip_zero_impedance)
                .with_synthesize_unrated_limits(synthesize_unrated_limits)
                .with_correct_angle_difference_bounds(correct_angle_difference_bounds);
            build_ac_opf_preparation(instance, &options)
                .map(PioAcOpfPreparation::new_raw)
                .map_err(|failure| error_from_matrix(&failure))
        })
    }
}

/// Read the dimensions and conventions of an AC OPF preparation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_opf_preparation_summary(
    preparation: *const PioAcOpfPreparation,
    output: *mut PioAcOpfPreparationView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let preparation = PioAcOpfPreparation::get(preparation).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_NULL_HANDLE,
                    "PioAcOpfPreparation must not be NULL",
                )
            })?;
            let units = opf_preparation_units_name(preparation.units).ok_or_else(|| {
                boundary_error(
                    &codes::REQUEST_CAPI_TYPE_MISMATCH,
                    "the AC OPF preparation uses units unsupported by ABI 7",
                )
            })?;
            let objective = prepared_objective_name(preparation.objective).ok_or_else(|| {
                boundary_error(
                    &codes::REQUEST_CAPI_TYPE_MISMATCH,
                    "the AC OPF preparation uses an objective unsupported by ABI 7",
                )
            })?;
            *require_output(output, "output")? = PioAcOpfPreparationView {
                name: PioStringView::new(&preparation.name),
                bus_count: preparation.n_buses,
                generator_count: preparation.n_generators(),
                storage_count: preparation.n_storage(),
                branch_count: preparation.n_branches(),
                source_generator_count: preparation.n_source_generators,
                source_branch_count: preparation.n_source_branches,
                base_mva: preparation.base_mva,
                units: PioStringView::new(units),
                objective: PioStringView::new(objective),
                skip_zero_impedance: preparation.skip_zero_impedance,
                synthesize_unrated_limits: preparation.synthesize_unrated_limits,
                correct_angle_difference_bounds: preparation.correct_angle_difference_bounds,
                reference_bus_count: preparation.reference_buses.len(),
                skipped_zero_impedance_count: preparation.branches.skipped_zero_impedance.len(),
            };
            Ok(true)
        })
    }
}

/// Borrow the dense reference bus indices of an AC OPF preparation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_opf_preparation_reference_buses(
    preparation: *const PioAcOpfPreparation,
) -> PioSizeView {
    unsafe { PioAcOpfPreparation::get(preparation) }.map_or(PioSizeView::EMPTY, |value| {
        PioSizeView::new(value.reference_buses.as_ref())
    })
}

/// Borrow the analysis rows skipped for zero impedance.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_opf_preparation_skipped_zero_impedance(
    preparation: *const PioAcOpfPreparation,
) -> PioSizeView {
    unsafe { PioAcOpfPreparation::get(preparation) }.map_or(PioSizeView::EMPTY, |value| {
        PioSizeView::new(&value.branches.skipped_zero_impedance)
    })
}

/// Read one dense bus row of an AC OPF preparation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_opf_preparation_bus_at(
    preparation: *const PioAcOpfPreparation,
    index: usize,
    output: *mut PioAcOpfBusView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let value = PioAcOpfPreparation::get(preparation).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_NULL_HANDLE,
                    "PioAcOpfPreparation must not be NULL",
                )
            })?;
            if index >= value.n_buses {
                return Err(boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("AC OPF preparation bus index {index} is out of range"),
                ));
            }
            let source_row = value.bus_source_rows[index];
            *require_output(output, "output")? = PioAcOpfBusView {
                bus_id: value.bus_ids[index].0,
                analysis_row: value.bus_analysis_rows[index],
                source_row: source_row.unwrap_or(0),
                has_source_row: source_row.is_some(),
                active_power_demand: value.buses.p_d[index],
                reactive_power_demand: value.buses.q_d[index],
                shunt_conductance: value.buses.g_s[index],
                shunt_susceptance: value.buses.b_s[index],
                voltage_magnitude_min_pu: value.buses.vm_min[index],
                voltage_magnitude_max_pu: value.buses.vm_max[index],
                initial_voltage_magnitude_pu: value.buses.initial_vm[index],
                initial_voltage_angle_radians: value.buses.initial_va[index],
                voltage_bound_active: value.buses.voltage_bound_active[index],
            };
            Ok(true)
        })
    }
}

/// Read one generator row of an AC OPF preparation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_opf_preparation_generator_at(
    preparation: *const PioAcOpfPreparation,
    index: usize,
    output: *mut PioAcOpfGeneratorView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let value = PioAcOpfPreparation::get(preparation).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_NULL_HANDLE,
                    "PioAcOpfPreparation must not be NULL",
                )
            })?;
            if index >= value.n_generators() {
                return Err(boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("AC OPF preparation generator index {index} is out of range"),
                ));
            }
            let source_row = value.generators.source_rows[index];
            let piecewise = value.generators.piecewise_linear[index].as_ref();
            *require_output(output, "output")? = PioAcOpfGeneratorView {
                component_id: PioStringView::new(&value.generators.identities[index]),
                bus_index: value.generators.bus_of_gen[index],
                analysis_row: value.generators.analysis_rows[index],
                source_row: source_row.unwrap_or(0),
                has_source_row: source_row.is_some(),
                quadratic_cost: value.generators.q[index],
                linear_cost: value.generators.c[index],
                constant_cost: value.generators.c0[index],
                has_piecewise_linear_cost: piecewise.is_some(),
                piecewise_linear_power: piecewise
                    .map_or(PioF64View::EMPTY, |cost| PioF64View::new(&cost.power)),
                piecewise_linear_value: piecewise
                    .map_or(PioF64View::EMPTY, |cost| PioF64View::new(&cost.value)),
                active_power_max: value.generators.pmax[index],
                active_power_min: value.generators.pmin[index],
                reactive_power_max: value.generators.qmax[index],
                reactive_power_min: value.generators.qmin[index],
                initial_active_power: value.generators.pg[index],
                initial_reactive_power: value.generators.qg[index],
                voltage_magnitude_setpoint_pu: value.generators.vg[index],
                capability_active: value.generators.capability_active[index],
            };
            Ok(true)
        })
    }
}

/// Read one storage row of an AC OPF preparation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_opf_preparation_storage_at(
    preparation: *const PioAcOpfPreparation,
    index: usize,
    output: *mut PioAcOpfStorageView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let value = PioAcOpfPreparation::get(preparation).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_NULL_HANDLE,
                    "PioAcOpfPreparation must not be NULL",
                )
            })?;
            if index >= value.n_storage() {
                return Err(boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("AC OPF preparation storage index {index} is out of range"),
                ));
            }
            let storage = &value.storage;
            *require_output(output, "output")? = PioAcOpfStorageView {
                component_id: PioStringView::new(&storage.identities[index]),
                bus_index: storage.bus_of_storage[index],
                source_row: storage.source_rows[index],
                initial_active_power: storage.p[index],
                initial_reactive_power: storage.q[index],
                energy: storage.energy[index],
                energy_rating: storage.energy_rating[index],
                charge_rating: storage.charge_rating[index],
                discharge_rating: storage.discharge_rating[index],
                charge_efficiency: storage.charge_efficiency[index],
                discharge_efficiency: storage.discharge_efficiency[index],
                apparent_power_max: storage.s_max[index],
                reactive_power_min: storage.qmin[index],
                reactive_power_max: storage.qmax[index],
                resistance_pu: storage.r[index],
                reactance_pu: storage.x[index],
                active_power_loss: storage.p_loss[index],
                reactive_power_loss: storage.q_loss[index],
                in_service: storage.in_service[index],
            };
            Ok(true)
        })
    }
}

/// Read one active branch row of an AC OPF preparation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_opf_preparation_branch_at(
    preparation: *const PioAcOpfPreparation,
    index: usize,
    output: *mut PioAcOpfBranchView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let value = PioAcOpfPreparation::get(preparation).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_NULL_HANDLE,
                    "PioAcOpfPreparation must not be NULL",
                )
            })?;
            if index >= value.n_branches() {
                return Err(boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("AC OPF preparation branch index {index} is out of range"),
                ));
            }
            let (source_kind, source_row, winding) =
                opf_analysis_branch_source(value.branches.analysis_sources[index], "AC")?;
            *require_output(output, "output")? = PioAcOpfBranchView {
                component_id: PioStringView::new(&value.branches.identities[index]),
                from_bus_index: value.branches.from_bus[index],
                to_bus_index: value.branches.to_bus[index],
                series_conductance: value.branches.g[index],
                series_susceptance: value.branches.b[index],
                from_conductance: value.branches.g_fr[index],
                from_susceptance: value.branches.b_fr[index],
                to_conductance: value.branches.g_to[index],
                to_susceptance: value.branches.b_to[index],
                tap_ratio: value.branches.tap[index],
                phase_shift_radians: value.branches.shift[index],
                apparent_power_max: value.branches.s_max[index],
                angle_difference_min_radians: value.branches.angle_min[index],
                angle_difference_max_radians: value.branches.angle_max[index],
                analysis_row: value.branches.analysis_rows[index],
                source_kind: PioStringView::new(source_kind),
                source_row,
                winding: winding.unwrap_or(0),
                has_winding: winding.is_some(),
                thermal_limit_active: value.branches.thermal_limit_active[index],
                angle_bound_active: value.branches.angle_bound_active[index],
            };
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_dc_pf_instance_bus_specification_count(
    instance: *const PioCalculationInstance,
) -> usize {
    unsafe { PioCalculationInstance::get(instance) }
        .and_then(CalculationInstanceInner::dc_pf)
        .map_or(0, |instance| instance.specifications().len())
}

/// Read one DC power flow bus specification by zero based bus table position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_dc_pf_instance_bus_specification_at(
    instance: *const PioCalculationInstance,
    index: usize,
    output: *mut PioDcBusSpecificationView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let instance = require_calculation_instance(instance)?
                .dc_pf()
                .ok_or_else(|| {
                    boundary_error(
                        &codes::REQUEST_CAPI_TYPE_MISMATCH,
                        "the calculation instance is not powerio.DcPfInstance",
                    )
                })?;
            let specification = instance.specifications().get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("DC bus specification index {index} is out of range"),
                )
            })?;
            let bus_id = instance
                .network()
                .buses()
                .get(index)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!("bus specification index {index} has no bus"),
                    )
                })?
                .id
                .0;
            let (kind, net_active_power_mw, voltage_angle_degrees) = match *specification {
                powerio_prob::DcBusSpecification::NetActivePower { p_mw } => {
                    ("net_active_power", p_mw, 0.0)
                }
                powerio_prob::DcBusSpecification::Reference { va_degrees } => {
                    ("reference", 0.0, va_degrees)
                }
                powerio_prob::DcBusSpecification::Isolated => ("isolated", 0.0, 0.0),
                _ => ("unknown", 0.0, 0.0),
            };
            *require_output(output, "output")? = PioDcBusSpecificationView {
                bus_id,
                kind: PioStringView::new(kind),
                net_active_power_mw,
                voltage_angle_degrees,
            };
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_dc_pf_instance_branch_susceptance_formula(
    instance: *const PioCalculationInstance,
) -> PioStringView {
    unsafe { PioCalculationInstance::get(instance) }
        .and_then(CalculationInstanceInner::dc_pf)
        .map_or(PioStringView::EMPTY, |instance| {
            PioStringView::new(branch_susceptance_formula_name(
                instance.branch_susceptance_formula(),
            ))
        })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_pf_instance_bus_specification_count(
    instance: *const PioCalculationInstance,
) -> usize {
    unsafe { PioCalculationInstance::get(instance) }
        .and_then(CalculationInstanceInner::ac_pf)
        .map_or(0, |instance| instance.specifications().len())
}

/// Read one AC power flow bus specification by zero based bus table position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_pf_instance_bus_specification_at(
    instance: *const PioCalculationInstance,
    index: usize,
    output: *mut PioAcBusSpecificationView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let instance = require_calculation_instance(instance)?
                .ac_pf()
                .ok_or_else(|| {
                    boundary_error(
                        &codes::REQUEST_CAPI_TYPE_MISMATCH,
                        "the calculation instance is not powerio.AcPfInstance",
                    )
                })?;
            let specification = instance.specifications().get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("AC bus specification index {index} is out of range"),
                )
            })?;
            let bus_id = instance
                .network()
                .buses()
                .get(index)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!("bus specification index {index} has no bus"),
                    )
                })?
                .id
                .0;
            let (kind, p, q, vm, va) = match *specification {
                powerio_prob::AcBusSpecification::Pq { p, q } => ("pq", p, q, 0.0, 0.0),
                powerio_prob::AcBusSpecification::Pv { p, vm } => ("pv", p, 0.0, vm, 0.0),
                powerio_prob::AcBusSpecification::Reference { vm, va } => {
                    ("reference", 0.0, 0.0, vm, va)
                }
                powerio_prob::AcBusSpecification::Isolated => ("isolated", 0.0, 0.0, 0.0, 0.0),
                _ => ("unknown", 0.0, 0.0, 0.0, 0.0),
            };
            *require_output(output, "output")? = PioAcBusSpecificationView {
                bus_id,
                kind: PioStringView::new(kind),
                net_active_power_mw: p,
                net_reactive_power_mvar: q,
                voltage_magnitude_pu: vm,
                voltage_angle_degrees: va,
            };
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_dc_opf_instance_branch_susceptance_formula(
    instance: *const PioCalculationInstance,
) -> PioStringView {
    unsafe { PioCalculationInstance::get(instance) }
        .and_then(CalculationInstanceInner::dc_opf)
        .map_or(PioStringView::EMPTY, |instance| {
            PioStringView::new(branch_susceptance_formula_name(
                instance.branch_susceptance_formula(),
            ))
        })
}

fn objective(instance: &CalculationInstanceInner) -> Option<&powerio_prob::Objective> {
    match instance.instance()? {
        CalculationInstanceRef::DcOpf(instance) => Some(instance.objective()),
        CalculationInstanceRef::AcOpf(instance) => Some(instance.objective()),
        CalculationInstanceRef::McAcOpf(instance) => Some(instance.objective()),
        _ => None,
    }
}

fn objective_term_name(term: &powerio_prob::ObjectiveTerm) -> &'static str {
    match term {
        powerio_prob::ObjectiveTerm::NetworkGeneratorCost => "network_generator_cost",
        powerio_prob::ObjectiveTerm::ActivePowerDispatchCost => "active_power_dispatch_cost",
        _ => "unknown",
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_calculation_instance_objective_term_count(
    instance: *const PioCalculationInstance,
) -> usize {
    unsafe { PioCalculationInstance::get(instance) }
        .and_then(objective)
        .map_or(0, |objective| objective.terms().len())
}

/// Read one typed objective term by zero based position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_calculation_instance_objective_term_at(
    instance: *const PioCalculationInstance,
    index: usize,
    output: *mut PioObjectiveTermView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let value = require_calculation_instance(instance)?;
            let objective = objective(value).ok_or_else(|| {
                boundary_error(
                    &codes::REQUEST_CAPI_TYPE_MISMATCH,
                    "the calculation instance has no optimization objective",
                )
            })?;
            let term = objective.terms().get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("objective term index {index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = PioObjectiveTermView {
                kind: PioStringView::new(objective_term_name(term)),
            };
            Ok(true)
        })
    }
}

fn active_constraint(
    instance: &CalculationInstanceInner,
    index: usize,
) -> Option<(&'static str, &powerio_prob::ConstraintSelection)> {
    match instance.instance()? {
        CalculationInstanceRef::DcOpf(instance) => match index {
            0 => Some((
                "generator_capability",
                &instance.constraints().generator_capability,
            )),
            1 => Some(("voltage_bounds", &instance.constraints().voltage_bounds)),
            2 => Some(("thermal_limits", &instance.constraints().thermal_limits)),
            3 => Some(("angle_bounds", &instance.constraints().angle_bounds)),
            _ => None,
        },
        CalculationInstanceRef::AcOpf(instance) => match index {
            0 => Some((
                "generator_capability",
                &instance.constraints().generator_capability,
            )),
            1 => Some(("voltage_bounds", &instance.constraints().voltage_bounds)),
            2 => Some(("thermal_limits", &instance.constraints().thermal_limits)),
            3 => Some(("angle_bounds", &instance.constraints().angle_bounds)),
            _ => None,
        },
        CalculationInstanceRef::McAcOpf(instance) => match index {
            0 => Some((
                "terminal_voltage_bounds",
                &instance.constraints().terminal_voltage_bounds,
            )),
            1 => Some(("conductor_limits", &instance.constraints().conductor_limits)),
            2 => Some((
                "generator_capability",
                &instance.constraints().generator_capability,
            )),
            _ => None,
        },
        _ => None,
    }
}

fn constraint_selection_parts(
    selection: &powerio_prob::ConstraintSelection,
) -> (&'static str, &[String]) {
    match selection {
        powerio_prob::ConstraintSelection::All => ("all", &[]),
        powerio_prob::ConstraintSelection::None => ("none", &[]),
        powerio_prob::ConstraintSelection::Only(identities) => ("only", identities),
        _ => ("unknown", &[]),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_calculation_instance_active_constraint_count(
    instance: *const PioCalculationInstance,
) -> usize {
    match unsafe { PioCalculationInstance::get(instance) }
        .and_then(CalculationInstanceInner::instance)
    {
        Some(CalculationInstanceRef::DcOpf(_) | CalculationInstanceRef::AcOpf(_)) => 4,
        Some(CalculationInstanceRef::McAcOpf(_)) => 3,
        _ => 0,
    }
}

/// Read one active constraint family by zero based position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_calculation_instance_active_constraint_at(
    instance: *const PioCalculationInstance,
    index: usize,
    output: *mut PioActiveConstraintView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let instance = require_calculation_instance(instance)?;
            let (family, selection) = active_constraint(instance, index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("active constraint index {index} is out of range"),
                )
            })?;
            let (selection, identities) = constraint_selection_parts(selection);
            *require_output(output, "output")? = PioActiveConstraintView {
                family: PioStringView::new(family),
                selection: PioStringView::new(selection),
                identity_count: identities.len(),
            };
            Ok(true)
        })
    }
}

/// Read one selected component identity from an `only` constraint selection.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_calculation_instance_active_constraint_identity_at(
    instance: *const PioCalculationInstance,
    constraint_index: usize,
    identity_index: usize,
    error: *mut *mut PioError,
) -> PioStringView {
    unsafe {
        entry(error, PioStringView::EMPTY, || {
            let instance = require_calculation_instance(instance)?;
            let (_, selection) =
                active_constraint(instance, constraint_index).ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!("active constraint index {constraint_index} is out of range"),
                    )
                })?;
            let (_, identities) = constraint_selection_parts(selection);
            let identity = identities.get(identity_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("constraint identity index {identity_index} is out of range"),
                )
            })?;
            Ok(PioStringView::new(identity))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_calculation_instance_has_initial_point(
    instance: *const PioCalculationInstance,
) -> bool {
    unsafe { PioCalculationInstance::get(instance) }
        .is_some_and(CalculationInstanceInner::has_initial_point)
}

/// Return the optional owner-rooted initial operating point. A calculation
/// instance with no initial point returns NULL without setting an error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_calculation_instance_initial_point(
    instance: *const PioCalculationInstance,
    error: *mut *mut PioError,
) -> *mut PioOperatingPoint {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let instance = PioCalculationInstance::get(instance).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_NULL_HANDLE,
                    "PioCalculationInstance must not be NULL",
                )
            })?;
            let has_initial_point = instance.has_initial_point();
            let projection = if has_initial_point {
                Some(match (instance.projection, instance.instance()) {
                    (
                        CalculationInstanceProjection::Direct,
                        Some(CalculationInstanceRef::DcPf(_)),
                    ) => OperatingPointProjection::DcPfInitial,
                    (
                        CalculationInstanceProjection::Direct,
                        Some(CalculationInstanceRef::AcPf(_)),
                    ) => OperatingPointProjection::AcPfInitial,
                    (
                        CalculationInstanceProjection::Direct,
                        Some(CalculationInstanceRef::DcOpf(_)),
                    ) => OperatingPointProjection::DcOpfInitial,
                    (
                        CalculationInstanceProjection::Direct,
                        Some(CalculationInstanceRef::AcOpf(_)),
                    ) => OperatingPointProjection::AcOpfInitial,
                    (
                        CalculationInstanceProjection::Direct,
                        Some(CalculationInstanceRef::McAcPf(_)),
                    ) => OperatingPointProjection::McAcPfInitial,
                    (
                        CalculationInstanceProjection::Direct,
                        Some(CalculationInstanceRef::McAcOpf(_)),
                    ) => OperatingPointProjection::McAcOpfInitial,
                    (CalculationInstanceProjection::DcPfSolution, _) => {
                        OperatingPointProjection::DcPfSolutionInitial
                    }
                    (CalculationInstanceProjection::AcPfSolution, _) => {
                        OperatingPointProjection::AcPfSolutionInitial
                    }
                    (CalculationInstanceProjection::DcOpfSolution, _) => {
                        OperatingPointProjection::DcOpfSolutionInitial
                    }
                    (CalculationInstanceProjection::AcOpfSolution, _) => {
                        OperatingPointProjection::AcOpfSolutionInitial
                    }
                    (CalculationInstanceProjection::SocwrOpfSolution, _) => {
                        OperatingPointProjection::SocwrOpfSolutionInitial
                    }
                    (CalculationInstanceProjection::McAcPfSolution, _) => {
                        OperatingPointProjection::McAcPfSolutionInitial
                    }
                    (CalculationInstanceProjection::McAcOpfSolution, _) => {
                        OperatingPointProjection::McAcOpfSolutionInitial
                    }
                    _ => {
                        return Err(boundary_error(
                            &codes::REQUEST_CAPI_TYPE_MISMATCH,
                            "the handle does not refer to a calculation instance with an operating point",
                        ));
                    }
                })
            } else {
                None
            };
            Ok(projection.map_or(std::ptr::null_mut(), |projection| {
                PioOperatingPoint::new_raw(OperatingPointInner {
                    value: ValueInner {
                        owner: Arc::clone(&instance.value.owner),
                        steps: instance.value.steps.clone(),
                    },
                    projection,
                })
            }))
        })
    }
}

fn dist_load_voltage_model_name(model: &powerio_dist::DistLoadVoltageModel) -> &'static str {
    match model {
        powerio_dist::DistLoadVoltageModel::ConstantPower { .. } => "constant_power",
        powerio_dist::DistLoadVoltageModel::ConstantCurrent { .. } => "constant_current",
        powerio_dist::DistLoadVoltageModel::ConstantImpedance { .. } => "constant_impedance",
        powerio_dist::DistLoadVoltageModel::Zip { .. } => "zip",
        powerio_dist::DistLoadVoltageModel::Exponential { .. } => "exponential",
        _ => "unknown",
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_mc_ac_pf_instance_load_count(
    instance: *const PioCalculationInstance,
) -> usize {
    unsafe { PioCalculationInstance::get(instance) }
        .and_then(CalculationInstanceInner::mc_ac_pf)
        .map_or(0, |instance| instance.loads().len())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_mc_ac_pf_instance_load_at(
    instance: *const PioCalculationInstance,
    index: usize,
    output: *mut PioPrescribedTerminalPowerView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let instance = require_calculation_instance(instance)?
                .mc_ac_pf()
                .ok_or_else(|| {
                    boundary_error(
                        &codes::REQUEST_CAPI_TYPE_MISMATCH,
                        "the calculation instance is not powerio.McAcPfInstance",
                    )
                })?;
            let load = instance.loads().get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("prescribed load index {index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = PioPrescribedTerminalPowerView {
                load: PioStringView::new(&load.load),
                terminal_count: load.terminals.len(),
                voltage_model: PioStringView::new(dist_load_voltage_model_name(
                    &load.voltage_model,
                )),
            };
            Ok(true)
        })
    }
}

fn model_value(values: &[f64], index: usize) -> (f64, bool) {
    values
        .get(index)
        .copied()
        .map_or((0.0, false), |value| (value, true))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_mc_ac_pf_instance_load_terminal_at(
    instance: *const PioCalculationInstance,
    load_index: usize,
    terminal_index: usize,
    output: *mut PioTerminalPowerView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let instance = require_calculation_instance(instance)?
                .mc_ac_pf()
                .ok_or_else(|| {
                    boundary_error(
                        &codes::REQUEST_CAPI_TYPE_MISMATCH,
                        "the calculation instance is not powerio.McAcPfInstance",
                    )
                })?;
            let load = instance.loads().get(load_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("prescribed load index {load_index} is out of range"),
                )
            })?;
            let terminal = load.terminals.get(terminal_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("load terminal index {terminal_index} is out of range"),
                )
            })?;
            let p = *load.p_w.get(terminal_index).ok_or_else(|| {
                boundary_error(
                    &codes::REQUEST_CAPI_TYPE_MISMATCH,
                    "the prescribed load active power does not align with its terminals",
                )
            })?;
            let q = *load.q_var.get(terminal_index).ok_or_else(|| {
                boundary_error(
                    &codes::REQUEST_CAPI_TYPE_MISMATCH,
                    "the prescribed load reactive power does not align with its terminals",
                )
            })?;
            let mut model_values = (0.0, false, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
            match &load.voltage_model {
                powerio_dist::DistLoadVoltageModel::ConstantPower { v_nom } => {
                    let (v, has_v) = model_value(v_nom, terminal_index);
                    model_values.0 = v;
                    model_values.1 = has_v;
                }
                powerio_dist::DistLoadVoltageModel::ConstantCurrent { v_nom } => {
                    let (v, has_v) = model_value(v_nom, terminal_index);
                    model_values = (v, has_v, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0);
                }
                powerio_dist::DistLoadVoltageModel::ConstantImpedance { v_nom } => {
                    let (v, has_v) = model_value(v_nom, terminal_index);
                    model_values = (v, has_v, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0);
                }
                powerio_dist::DistLoadVoltageModel::Zip {
                    v_nom,
                    alpha_z,
                    alpha_i,
                    alpha_p,
                    beta_z,
                    beta_i,
                    beta_p,
                } => {
                    let (v, has_v) = model_value(v_nom, terminal_index);
                    model_values = (
                        v,
                        has_v,
                        alpha_z.get(terminal_index).copied().unwrap_or(0.0),
                        alpha_i.get(terminal_index).copied().unwrap_or(0.0),
                        alpha_p.get(terminal_index).copied().unwrap_or(0.0),
                        beta_z.get(terminal_index).copied().unwrap_or(0.0),
                        beta_i.get(terminal_index).copied().unwrap_or(0.0),
                        beta_p.get(terminal_index).copied().unwrap_or(0.0),
                        0.0,
                        0.0,
                    );
                }
                powerio_dist::DistLoadVoltageModel::Exponential {
                    v_nom,
                    gamma_p,
                    gamma_q,
                } => {
                    let (v, has_v) = model_value(v_nom, terminal_index);
                    model_values = (
                        v,
                        has_v,
                        0.0,
                        0.0,
                        0.0,
                        0.0,
                        0.0,
                        0.0,
                        gamma_p.get(terminal_index).copied().unwrap_or(0.0),
                        gamma_q.get(terminal_index).copied().unwrap_or(0.0),
                    );
                }
                _ => {}
            }
            *require_output(output, "output")? = PioTerminalPowerView {
                terminal: PioStringView::new(terminal),
                active_power_w: p,
                reactive_power_var: q,
                nominal_voltage_v: model_values.0,
                has_nominal_voltage: model_values.1,
                active_impedance_fraction: model_values.2,
                active_current_fraction: model_values.3,
                active_power_fraction: model_values.4,
                reactive_impedance_fraction: model_values.5,
                reactive_current_fraction: model_values.6,
                reactive_power_fraction: model_values.7,
                active_power_exponent: model_values.8,
                reactive_power_exponent: model_values.9,
            };
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_mc_ac_pf_instance_source_count(
    instance: *const PioCalculationInstance,
) -> usize {
    unsafe { PioCalculationInstance::get(instance) }
        .and_then(CalculationInstanceInner::mc_ac_pf)
        .map_or(0, |instance| instance.sources().len())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_mc_ac_pf_instance_source_at(
    instance: *const PioCalculationInstance,
    index: usize,
    output: *mut PioPrescribedSourceVoltageView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let instance = require_calculation_instance(instance)?
                .mc_ac_pf()
                .ok_or_else(|| {
                    boundary_error(
                        &codes::REQUEST_CAPI_TYPE_MISMATCH,
                        "the calculation instance is not powerio.McAcPfInstance",
                    )
                })?;
            let source = instance.sources().get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("prescribed source index {index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = PioPrescribedSourceVoltageView {
                source: PioStringView::new(&source.source),
                terminal_count: source.terminals.len(),
            };
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_mc_ac_pf_instance_source_terminal_at(
    instance: *const PioCalculationInstance,
    source_index: usize,
    terminal_index: usize,
    output: *mut PioTerminalVoltageView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let instance = require_calculation_instance(instance)?
                .mc_ac_pf()
                .ok_or_else(|| {
                    boundary_error(
                        &codes::REQUEST_CAPI_TYPE_MISMATCH,
                        "the calculation instance is not powerio.McAcPfInstance",
                    )
                })?;
            let source = instance.sources().get(source_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("prescribed source index {source_index} is out of range"),
                )
            })?;
            let terminal = source.terminals.get(terminal_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("source terminal index {terminal_index} is out of range"),
                )
            })?;
            let magnitude = source.v_magnitude.get(terminal_index).ok_or_else(|| {
                boundary_error(
                    &codes::REQUEST_CAPI_TYPE_MISMATCH,
                    "the prescribed source magnitudes do not align with its terminals",
                )
            })?;
            let angle = source.v_angle.get(terminal_index).ok_or_else(|| {
                boundary_error(
                    &codes::REQUEST_CAPI_TYPE_MISMATCH,
                    "the prescribed source angles do not align with its terminals",
                )
            })?;
            *require_output(output, "output")? = PioTerminalVoltageView {
                terminal: PioStringView::new(terminal),
                magnitude_v: *magnitude,
                angle_radians: *angle,
            };
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_mc_ac_pf_instance_isolated_terminal_count(
    instance: *const PioCalculationInstance,
) -> usize {
    unsafe { PioCalculationInstance::get(instance) }
        .and_then(CalculationInstanceInner::mc_ac_pf)
        .map_or(0, |instance| instance.isolated_terminals().len())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_mc_ac_pf_instance_isolated_terminal_at(
    instance: *const PioCalculationInstance,
    index: usize,
    output: *mut PioIsolatedTerminalView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let instance = require_calculation_instance(instance)?
                .mc_ac_pf()
                .ok_or_else(|| {
                    boundary_error(
                        &codes::REQUEST_CAPI_TYPE_MISMATCH,
                        "the calculation instance is not powerio.McAcPfInstance",
                    )
                })?;
            let (bus, terminal) = instance.isolated_terminals().get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("isolated terminal index {index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = PioIsolatedTerminalView {
                bus: PioStringView::new(bus),
                terminal: PioStringView::new(terminal),
            };
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_mc_ac_pf_instance_active_control_count(
    instance: *const PioCalculationInstance,
) -> usize {
    unsafe { PioCalculationInstance::get(instance) }
        .and_then(CalculationInstanceInner::mc_ac_pf)
        .map_or(0, |instance| instance.control_modes().len())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_mc_ac_pf_instance_active_control_at(
    instance: *const PioCalculationInstance,
    index: usize,
    output: *mut PioActiveControlView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let instance = require_calculation_instance(instance)?
                .mc_ac_pf()
                .ok_or_else(|| {
                    boundary_error(
                        &codes::REQUEST_CAPI_TYPE_MISMATCH,
                        "the calculation instance is not powerio.McAcPfInstance",
                    )
                })?;
            let control = instance.control_modes().get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("active control index {index} is out of range"),
                )
            })?;
            let (kind, component_id) = match control {
                powerio_prob::ActiveControlMode::RegulatorTap { transformer } => {
                    ("regulator_tap", transformer.as_str())
                }
                powerio_prob::ActiveControlMode::CapacitorSteps { capacitor } => {
                    ("capacitor_steps", capacitor.as_str())
                }
                _ => ("unknown", ""),
            };
            *require_output(output, "output")? = PioActiveControlView {
                kind: PioStringView::new(kind),
                component_id: PioStringView::new(component_id),
            };
            Ok(true)
        })
    }
}

fn scuc_inputs(instance: &CalculationInstanceInner) -> Option<&powerio_prob::ScucInputs> {
    instance.ac_scuc().map(powerio_prob::AcScucInstance::inputs)
}

fn scuc_inputs_or_error(
    instance: &CalculationInstanceInner,
) -> Result<&powerio_prob::ScucInputs, *mut PioError> {
    scuc_inputs(instance).ok_or_else(|| {
        boundary_error(
            &codes::REQUEST_CAPI_TYPE_MISMATCH,
            "the calculation instance is not powerio.AcScucInstance",
        )
    })
}

fn component_id_view(id: &ComponentId) -> PioComponentIdView {
    PioComponentIdView {
        component_type: PioStringView::new(id.component_type()),
        local_id: PioStringView::new(id.local_id()),
    }
}

fn empty_component_id_view() -> PioComponentIdView {
    PioComponentIdView {
        component_type: PioStringView::EMPTY,
        local_id: PioStringView::EMPTY,
    }
}

fn optional_component_id_view(value: Option<&ComponentId>) -> (PioComponentIdView, bool) {
    value.map_or((empty_component_id_view(), false), |value| {
        (component_id_view(value), true)
    })
}

fn terminal_reference_view(
    value: Option<&powerio_tx::TerminalReference>,
) -> (PioTerminalReferenceView, bool) {
    value.map_or(
        (
            PioTerminalReferenceView {
                equipment: empty_component_id_view(),
                terminal: 0,
            },
            false,
        ),
        |value| {
            (
                PioTerminalReferenceView {
                    equipment: component_id_view(&value.equipment),
                    terminal: value.terminal,
                },
                true,
            )
        },
    )
}

fn transformer_control_mode_name(mode: powerio_tx::TransformerControlMode) -> &'static str {
    match mode {
        powerio_tx::TransformerControlMode::Fixed => "fixed",
        powerio_tx::TransformerControlMode::Voltage => "voltage",
        powerio_tx::TransformerControlMode::ReactiveFlow => "reactive_flow",
        powerio_tx::TransformerControlMode::ActiveFlow => "active_flow",
        powerio_tx::TransformerControlMode::DcLineQuantity => "dc_line_quantity",
        powerio_tx::TransformerControlMode::AsymmetricActiveFlow => "asymmetric_active_flow",
        _ => "unknown",
    }
}

fn transformer_control_view(
    value: Option<&powerio_tx::TransformerControl>,
) -> (PioTransformerControlView, bool) {
    let empty_terminal = terminal_reference_view(None).0;
    value.map_or(
        (
            PioTransformerControlView {
                mode: PioStringView::EMPTY,
                enabled: false,
                controlled_bus_id: 0,
                has_controlled_bus: false,
                controlled_bus_on_winding_side: false,
                regulating_terminal: empty_terminal,
                has_regulating_terminal: false,
                tap_min: 0.0,
                tap_max: 0.0,
                band_min: 0.0,
                band_max: 0.0,
                tap_position_count: 0,
                mva_base: 0.0,
                winding_connection_angle: 0.0,
                has_winding_connection_angle: false,
            },
            false,
        ),
        |control| {
            let (regulating_terminal, has_regulating_terminal) =
                terminal_reference_view(control.regulating_terminal.as_ref());
            (
                PioTransformerControlView {
                    mode: PioStringView::new(transformer_control_mode_name(control.mode)),
                    enabled: control.enabled,
                    controlled_bus_id: control.controlled_bus.map_or(0, |bus| bus.0),
                    has_controlled_bus: control.controlled_bus.is_some(),
                    controlled_bus_on_winding_side: control.controlled_bus_on_winding_side,
                    regulating_terminal,
                    has_regulating_terminal,
                    tap_min: control.tap_min,
                    tap_max: control.tap_max,
                    band_min: control.band_min,
                    band_max: control.band_max,
                    tap_position_count: control.ntp,
                    mva_base: control.mva_base,
                    winding_connection_angle: control.winding_connection_angle.unwrap_or(0.0),
                    has_winding_connection_angle: control.winding_connection_angle.is_some(),
                },
                true,
            )
        },
    )
}

fn scuc_device_kind_name(kind: powerio_prob::ScucDeviceKind) -> &'static str {
    match kind {
        powerio_prob::ScucDeviceKind::Producer => "producer",
        powerio_prob::ScucDeviceKind::Consumer => "consumer",
        _ => "unknown",
    }
}

fn scuc_reactive_capability_view(
    capability: &powerio_prob::ScucReactiveCapability,
) -> PioScucReactiveCapabilityView {
    match capability {
        powerio_prob::ScucReactiveCapability::None => PioScucReactiveCapabilityView {
            kind: PioStringView::new("none"),
            reactive_power_at_zero_active_power_pu: 0.0,
            reactive_power_at_zero_active_power_min_pu: 0.0,
            reactive_power_at_zero_active_power_max_pu: 0.0,
            slope: 0.0,
            slope_min: 0.0,
            slope_max: 0.0,
        },
        powerio_prob::ScucReactiveCapability::Linear {
            reactive_power_at_zero_active_power,
            slope,
        } => PioScucReactiveCapabilityView {
            kind: PioStringView::new("linear"),
            reactive_power_at_zero_active_power_pu: *reactive_power_at_zero_active_power,
            reactive_power_at_zero_active_power_min_pu: 0.0,
            reactive_power_at_zero_active_power_max_pu: 0.0,
            slope: *slope,
            slope_min: 0.0,
            slope_max: 0.0,
        },
        powerio_prob::ScucReactiveCapability::Bounded {
            reactive_power_at_zero_active_power_min,
            reactive_power_at_zero_active_power_max,
            slope_min,
            slope_max,
        } => PioScucReactiveCapabilityView {
            kind: PioStringView::new("bounded"),
            reactive_power_at_zero_active_power_pu: 0.0,
            reactive_power_at_zero_active_power_min_pu: *reactive_power_at_zero_active_power_min,
            reactive_power_at_zero_active_power_max_pu: *reactive_power_at_zero_active_power_max,
            slope: 0.0,
            slope_min: *slope_min,
            slope_max: *slope_max,
        },
        _ => PioScucReactiveCapabilityView {
            kind: PioStringView::new("unknown"),
            reactive_power_at_zero_active_power_pu: 0.0,
            reactive_power_at_zero_active_power_min_pu: 0.0,
            reactive_power_at_zero_active_power_max_pu: 0.0,
            slope: 0.0,
            slope_min: 0.0,
            slope_max: 0.0,
        },
    }
}

fn scuc_reserve_costs_view(costs: &powerio_prob::ScucReserveCosts) -> PioScucReserveCostsView {
    PioScucReserveCostsView {
        regulation_up: costs.regulation_up,
        regulation_down: costs.regulation_down,
        synchronized: costs.synchronized,
        nonsynchronized: costs.nonsynchronized,
        ramping_up_online: costs.ramping_up_online,
        ramping_down_online: costs.ramping_down_online,
        ramping_up_offline: costs.ramping_up_offline,
        ramping_down_offline: costs.ramping_down_offline,
        reactive_up: costs.reactive_up,
        reactive_down: costs.reactive_down,
    }
}

fn scuc_device_view(device: &powerio_prob::ScucDevice) -> PioScucDeviceView {
    PioScucDeviceView {
        id: component_id_view(&device.id),
        kind: PioStringView::new(scuc_device_kind_name(device.kind)),
        initial_on_status: device.initial_on_status,
        on_cost: device.on_cost,
        startup_cost: device.startup_cost,
        shutdown_cost: device.shutdown_cost,
        minimum_up_time_hours: device.minimum_up_time,
        minimum_down_time_hours: device.minimum_down_time,
        ramp_limits: PioScucRampLimitsView {
            up_pu_per_hour: device.ramp_limits.up,
            down_pu_per_hour: device.ramp_limits.down,
            startup_pu_per_hour: device.ramp_limits.startup,
            shutdown_pu_per_hour: device.ramp_limits.shutdown,
        },
        reserve_limits: PioScucReserveLimitsView {
            regulation_up_pu: device.reserve_limits.regulation_up,
            regulation_down_pu: device.reserve_limits.regulation_down,
            synchronized_pu: device.reserve_limits.synchronized,
            nonsynchronized_pu: device.reserve_limits.nonsynchronized,
            ramping_up_online_pu: device.reserve_limits.ramping_up_online,
            ramping_down_online_pu: device.reserve_limits.ramping_down_online,
            ramping_up_offline_pu: device.reserve_limits.ramping_up_offline,
            ramping_down_offline_pu: device.reserve_limits.ramping_down_offline,
        },
        initial_commitment: PioScucInitialCommitmentView {
            accumulated_up_time_hours: device.initial_commitment.accumulated_up_time,
            accumulated_down_time_hours: device.initial_commitment.accumulated_down_time,
        },
        reactive_capability: scuc_reactive_capability_view(&device.reactive_capability),
        period_count: device.periods.len(),
        startup_cost_adjustment_count: device.startup_cost_adjustments.len(),
        startup_limit_count: device.startup_limits.len(),
        energy_upper_bound_count: device.energy_upper_bounds.len(),
        energy_lower_bound_count: device.energy_lower_bounds.len(),
    }
}

fn scuc_device_or_error(
    inputs: &powerio_prob::ScucInputs,
    device_index: usize,
) -> Result<&powerio_prob::ScucDevice, *mut PioError> {
    inputs.devices.get(device_index).ok_or_else(|| {
        boundary_error(
            &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
            format!("SCUC device index {device_index} is out of range"),
        )
    })
}

/// Read semantic collection sizes for one AC SCUC instance.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_scuc_instance_dimensions(
    instance: *const PioCalculationInstance,
    output: *mut PioScucDimensionsView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let inputs = scuc_inputs_or_error(require_calculation_instance(instance)?)?;
            *require_output(output, "output")? = PioScucDimensionsView {
                period_count: inputs.interval_durations.len(),
                device_count: inputs.devices.len(),
                producer_count: inputs.producers().count(),
                consumer_count: inputs.consumers().count(),
                shunt_count: inputs.shunts.len(),
                branch_switching_cost_count: inputs.branch_switching_costs.len(),
                transformer_control_count: inputs.transformer_controls.len(),
                active_reserve_zone_count: inputs.active_reserve_zones.len(),
                reactive_reserve_zone_count: inputs.reactive_reserve_zones.len(),
                contingency_count: inputs.contingencies.len(),
            };
            Ok(true)
        })
    }
}

/// Borrow interval durations in hours, in chronological order.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_scuc_instance_interval_durations(
    instance: *const PioCalculationInstance,
) -> PioF64View {
    unsafe { PioCalculationInstance::get(instance) }
        .and_then(scuc_inputs)
        .map_or(PioF64View::EMPTY, |inputs| {
            PioF64View::new(&inputs.interval_durations)
        })
}

/// Read the four required violation costs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_scuc_instance_violation_costs(
    instance: *const PioCalculationInstance,
    output: *mut PioScucViolationCostView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let inputs = scuc_inputs_or_error(require_calculation_instance(instance)?)?;
            let costs = inputs.violation_costs;
            *require_output(output, "output")? = PioScucViolationCostView {
                active_power_balance: costs.active_power_balance,
                reactive_power_balance: costs.reactive_power_balance,
                branch_thermal_limit: costs.branch_thermal_limit,
                energy_requirement: costs.energy_requirement,
            };
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_scuc_instance_device_count(
    instance: *const PioCalculationInstance,
) -> usize {
    unsafe { PioCalculationInstance::get(instance) }
        .and_then(scuc_inputs)
        .map_or(0, |inputs| inputs.devices.len())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_scuc_instance_device_at(
    instance: *const PioCalculationInstance,
    index: usize,
    output: *mut PioScucDeviceView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let inputs = scuc_inputs_or_error(require_calculation_instance(instance)?)?;
            let device = scuc_device_or_error(inputs, index)?;
            *require_output(output, "output")? = scuc_device_view(device);
            Ok(true)
        })
    }
}

/// Read one device by its exact source UID.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_scuc_instance_device_get(
    instance: *const PioCalculationInstance,
    uid: *const c_char,
    uid_len: usize,
    output: *mut PioScucDeviceView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let uid = required_str(uid, uid_len, "device_uid")?;
            let inputs = scuc_inputs_or_error(require_calculation_instance(instance)?)?;
            let device = inputs.device(uid).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("SCUC device UID '{uid}' does not exist"),
                )
            })?;
            *require_output(output, "output")? = scuc_device_view(device);
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_scuc_instance_device_startup_cost_adjustment_count(
    instance: *const PioCalculationInstance,
    device_index: usize,
) -> usize {
    unsafe { PioCalculationInstance::get(instance) }
        .and_then(scuc_inputs)
        .and_then(|inputs| inputs.devices.get(device_index))
        .map_or(0, |device| device.startup_cost_adjustments.len())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_scuc_instance_device_startup_cost_adjustment_at(
    instance: *const PioCalculationInstance,
    device_index: usize,
    adjustment_index: usize,
    output: *mut PioScucStartupCostAdjustmentView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let inputs = scuc_inputs_or_error(require_calculation_instance(instance)?)?;
            let device = scuc_device_or_error(inputs, device_index)?;
            let adjustment = device
                .startup_cost_adjustments
                .get(adjustment_index)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!(
                            "SCUC device {device_index} startup cost adjustment index {adjustment_index} is out of range"
                        ),
                    )
                })?;
            *require_output(output, "output")? = PioScucStartupCostAdjustmentView {
                cost: adjustment.cost,
                maximum_down_time_hours: adjustment.maximum_down_time,
            };
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_scuc_instance_device_startup_limit_count(
    instance: *const PioCalculationInstance,
    device_index: usize,
) -> usize {
    unsafe { PioCalculationInstance::get(instance) }
        .and_then(scuc_inputs)
        .and_then(|inputs| inputs.devices.get(device_index))
        .map_or(0, |device| device.startup_limits.len())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_scuc_instance_device_startup_limit_at(
    instance: *const PioCalculationInstance,
    device_index: usize,
    limit_index: usize,
    output: *mut PioScucStartupLimitView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let inputs = scuc_inputs_or_error(require_calculation_instance(instance)?)?;
            let device = scuc_device_or_error(inputs, device_index)?;
            let limit = device.startup_limits.get(limit_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!(
                        "SCUC device {device_index} startup limit index {limit_index} is out of range"
                    ),
                )
            })?;
            *require_output(output, "output")? = PioScucStartupLimitView {
                start_time_hours: limit.start_time,
                end_time_hours: limit.end_time,
                maximum_startups: limit.maximum_startups,
            };
            Ok(true)
        })
    }
}

unsafe fn scuc_energy_requirement_count(
    instance: *const PioCalculationInstance,
    device_index: usize,
    upper: bool,
) -> usize {
    unsafe { PioCalculationInstance::get(instance) }
        .and_then(scuc_inputs)
        .and_then(|inputs| inputs.devices.get(device_index))
        .map_or(0, |device| {
            if upper {
                device.energy_upper_bounds.len()
            } else {
                device.energy_lower_bounds.len()
            }
        })
}

unsafe fn scuc_energy_requirement_at(
    instance: *const PioCalculationInstance,
    device_index: usize,
    requirement_index: usize,
    upper: bool,
    output: *mut PioScucEnergyRequirementView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let inputs = scuc_inputs_or_error(require_calculation_instance(instance)?)?;
            let device = scuc_device_or_error(inputs, device_index)?;
            let requirements = if upper {
                &device.energy_upper_bounds
            } else {
                &device.energy_lower_bounds
            };
            let requirement = requirements.get(requirement_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!(
                        "SCUC device {device_index} {} energy requirement index {requirement_index} is out of range",
                        if upper { "upper" } else { "lower" }
                    ),
                )
            })?;
            *require_output(output, "output")? = PioScucEnergyRequirementView {
                start_time_hours: requirement.start_time,
                end_time_hours: requirement.end_time,
                energy_pu: requirement.energy,
            };
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_scuc_instance_device_energy_upper_bound_count(
    instance: *const PioCalculationInstance,
    device_index: usize,
) -> usize {
    unsafe { scuc_energy_requirement_count(instance, device_index, true) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_scuc_instance_device_energy_upper_bound_at(
    instance: *const PioCalculationInstance,
    device_index: usize,
    requirement_index: usize,
    output: *mut PioScucEnergyRequirementView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        scuc_energy_requirement_at(
            instance,
            device_index,
            requirement_index,
            true,
            output,
            error,
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_scuc_instance_device_energy_lower_bound_count(
    instance: *const PioCalculationInstance,
    device_index: usize,
) -> usize {
    unsafe { scuc_energy_requirement_count(instance, device_index, false) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_scuc_instance_device_energy_lower_bound_at(
    instance: *const PioCalculationInstance,
    device_index: usize,
    requirement_index: usize,
    output: *mut PioScucEnergyRequirementView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        scuc_energy_requirement_at(
            instance,
            device_index,
            requirement_index,
            false,
            output,
            error,
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_scuc_instance_device_period_at(
    instance: *const PioCalculationInstance,
    device_index: usize,
    period_index: usize,
    output: *mut PioScucDevicePeriodView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let inputs = scuc_inputs_or_error(require_calculation_instance(instance)?)?;
            let device = scuc_device_or_error(inputs, device_index)?;
            let period = device.periods.get(period_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!(
                        "SCUC device {device_index} period index {period_index} is out of range"
                    ),
                )
            })?;
            *require_output(output, "output")? = PioScucDevicePeriodView {
                on_status_min: period.on_status_min,
                on_status_max: period.on_status_max,
                active_power_min_pu: period.active_power_min,
                active_power_max_pu: period.active_power_max,
                reactive_power_min_pu: period.reactive_power_min,
                reactive_power_max_pu: period.reactive_power_max,
                energy_cost_block_count: period.energy_cost_blocks.len(),
                reserve_costs: scuc_reserve_costs_view(&period.reserve_costs),
            };
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_scuc_instance_device_energy_cost_block_count(
    instance: *const PioCalculationInstance,
    device_index: usize,
    period_index: usize,
) -> usize {
    unsafe { PioCalculationInstance::get(instance) }
        .and_then(scuc_inputs)
        .and_then(|inputs| inputs.devices.get(device_index))
        .and_then(|device| device.periods.get(period_index))
        .map_or(0, |period| period.energy_cost_blocks.len())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_scuc_instance_device_energy_cost_block_at(
    instance: *const PioCalculationInstance,
    device_index: usize,
    period_index: usize,
    block_index: usize,
    output: *mut PioScucEnergyCostBlockView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let inputs = scuc_inputs_or_error(require_calculation_instance(instance)?)?;
            let device = scuc_device_or_error(inputs, device_index)?;
            let period = device.periods.get(period_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!(
                        "SCUC device {device_index} period index {period_index} is out of range"
                    ),
                )
            })?;
            let block = period.energy_cost_blocks.get(block_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!(
                        "SCUC device {device_index} period {period_index} energy cost block index {block_index} is out of range"
                    ),
                )
            })?;
            *require_output(output, "output")? = PioScucEnergyCostBlockView {
                marginal_cost: block.marginal_cost,
                block_size_pu: block.block_size,
            };
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_scuc_instance_shunt_count(
    instance: *const PioCalculationInstance,
) -> usize {
    unsafe { PioCalculationInstance::get(instance) }
        .and_then(scuc_inputs)
        .map_or(0, |inputs| inputs.shunts.len())
}

fn scuc_shunt_view(shunt: &powerio_prob::ScucShunt) -> PioScucShuntView {
    PioScucShuntView {
        id: component_id_view(&shunt.id),
        conductance_per_step_pu: shunt.conductance_per_step,
        susceptance_per_step_pu: shunt.susceptance_per_step,
        step_min: shunt.step_min,
        step_max: shunt.step_max,
        initial_step: shunt.initial_step,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_scuc_instance_shunt_at(
    instance: *const PioCalculationInstance,
    index: usize,
    output: *mut PioScucShuntView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let inputs = scuc_inputs_or_error(require_calculation_instance(instance)?)?;
            let shunt = inputs.shunts.get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("SCUC shunt index {index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = scuc_shunt_view(shunt);
            Ok(true)
        })
    }
}

/// Read one shunt by its exact source UID.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_scuc_instance_shunt_get(
    instance: *const PioCalculationInstance,
    uid: *const c_char,
    uid_len: usize,
    output: *mut PioScucShuntView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let uid = required_str(uid, uid_len, "shunt_uid")?;
            let inputs = scuc_inputs_or_error(require_calculation_instance(instance)?)?;
            let shunt = inputs.shunt(uid).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("SCUC shunt UID '{uid}' does not exist"),
                )
            })?;
            *require_output(output, "output")? = scuc_shunt_view(shunt);
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_scuc_instance_branch_switching_cost_count(
    instance: *const PioCalculationInstance,
) -> usize {
    unsafe { PioCalculationInstance::get(instance) }
        .and_then(scuc_inputs)
        .map_or(0, |inputs| inputs.branch_switching_costs.len())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_scuc_instance_branch_switching_cost_at(
    instance: *const PioCalculationInstance,
    index: usize,
    output: *mut PioScucBranchSwitchingCostView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let inputs = scuc_inputs_or_error(require_calculation_instance(instance)?)?;
            let cost = inputs.branch_switching_costs.get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("SCUC branch switching cost index {index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = PioScucBranchSwitchingCostView {
                id: component_id_view(&cost.id),
                connection_cost: cost.connection_cost,
                disconnection_cost: cost.disconnection_cost,
            };
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_scuc_instance_transformer_control_count(
    instance: *const PioCalculationInstance,
) -> usize {
    unsafe { PioCalculationInstance::get(instance) }
        .and_then(scuc_inputs)
        .map_or(0, |inputs| inputs.transformer_controls.len())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_scuc_instance_transformer_control_at(
    instance: *const PioCalculationInstance,
    index: usize,
    output: *mut PioScucTransformerControlView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let inputs = scuc_inputs_or_error(require_calculation_instance(instance)?)?;
            let control = inputs.transformer_controls.get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("SCUC transformer control index {index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = PioScucTransformerControlView {
                id: component_id_view(&control.id),
                tap_ratio_min: control.tap_ratio_min,
                tap_ratio_max: control.tap_ratio_max,
                phase_shift_min_radians: control.phase_shift_min,
                phase_shift_max_radians: control.phase_shift_max,
            };
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_scuc_instance_active_reserve_zone_count(
    instance: *const PioCalculationInstance,
) -> usize {
    unsafe { PioCalculationInstance::get(instance) }
        .and_then(scuc_inputs)
        .map_or(0, |inputs| inputs.active_reserve_zones.len())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_scuc_instance_active_reserve_zone_at(
    instance: *const PioCalculationInstance,
    index: usize,
    output: *mut PioScucActiveReserveZoneView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let inputs = scuc_inputs_or_error(require_calculation_instance(instance)?)?;
            let zone = inputs.active_reserve_zones.get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("SCUC active reserve zone index {index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = PioScucActiveReserveZoneView {
                id: component_id_view(&zone.id),
                regulation_up_requirement_fraction: zone.regulation_up_requirement_fraction,
                regulation_down_requirement_fraction: zone.regulation_down_requirement_fraction,
                synchronized_requirement_fraction: zone.synchronized_requirement_fraction,
                nonsynchronized_requirement_fraction: zone.nonsynchronized_requirement_fraction,
                regulation_up_violation_cost: zone.regulation_up_violation_cost,
                regulation_down_violation_cost: zone.regulation_down_violation_cost,
                synchronized_violation_cost: zone.synchronized_violation_cost,
                nonsynchronized_violation_cost: zone.nonsynchronized_violation_cost,
                ramping_up_violation_cost: zone.ramping_up_violation_cost,
                ramping_down_violation_cost: zone.ramping_down_violation_cost,
                period_count: zone.ramping_up_requirement.len(),
                bus_count: zone.buses.len(),
            };
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_scuc_instance_active_reserve_zone_period_at(
    instance: *const PioCalculationInstance,
    zone_index: usize,
    period_index: usize,
    output: *mut PioScucActiveReservePeriodView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let inputs = scuc_inputs_or_error(require_calculation_instance(instance)?)?;
            let zone = inputs.active_reserve_zones.get(zone_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("SCUC active reserve zone index {zone_index} is out of range"),
                )
            })?;
            let up = zone
                .ramping_up_requirement
                .get(period_index)
                .copied()
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!("SCUC reserve period index {period_index} is out of range"),
                    )
                })?;
            let down = zone
                .ramping_down_requirement
                .get(period_index)
                .copied()
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!("SCUC reserve period index {period_index} is out of range"),
                    )
                })?;
            *require_output(output, "output")? = PioScucActiveReservePeriodView {
                ramping_up_requirement_pu: up,
                ramping_down_requirement_pu: down,
            };
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_scuc_instance_active_reserve_zone_bus_at(
    instance: *const PioCalculationInstance,
    zone_index: usize,
    bus_index: usize,
    output: *mut PioComponentIdView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let inputs = scuc_inputs_or_error(require_calculation_instance(instance)?)?;
            let zone = inputs.active_reserve_zones.get(zone_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("SCUC active reserve zone index {zone_index} is out of range"),
                )
            })?;
            let bus = zone.buses.get(bus_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("SCUC reserve zone bus index {bus_index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = component_id_view(bus);
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_scuc_instance_reactive_reserve_zone_count(
    instance: *const PioCalculationInstance,
) -> usize {
    unsafe { PioCalculationInstance::get(instance) }
        .and_then(scuc_inputs)
        .map_or(0, |inputs| inputs.reactive_reserve_zones.len())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_scuc_instance_reactive_reserve_zone_at(
    instance: *const PioCalculationInstance,
    index: usize,
    output: *mut PioScucReactiveReserveZoneView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let inputs = scuc_inputs_or_error(require_calculation_instance(instance)?)?;
            let zone = inputs.reactive_reserve_zones.get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("SCUC reactive reserve zone index {index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = PioScucReactiveReserveZoneView {
                id: component_id_view(&zone.id),
                reactive_up_violation_cost: zone.reactive_up_violation_cost,
                reactive_down_violation_cost: zone.reactive_down_violation_cost,
                period_count: zone.reactive_up_requirement.len(),
                bus_count: zone.buses.len(),
            };
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_scuc_instance_reactive_reserve_zone_period_at(
    instance: *const PioCalculationInstance,
    zone_index: usize,
    period_index: usize,
    output: *mut PioScucReactiveReservePeriodView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let inputs = scuc_inputs_or_error(require_calculation_instance(instance)?)?;
            let zone = inputs
                .reactive_reserve_zones
                .get(zone_index)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!("SCUC reactive reserve zone index {zone_index} is out of range"),
                    )
                })?;
            let up = zone
                .reactive_up_requirement
                .get(period_index)
                .copied()
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!("SCUC reserve period index {period_index} is out of range"),
                    )
                })?;
            let down = zone
                .reactive_down_requirement
                .get(period_index)
                .copied()
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!("SCUC reserve period index {period_index} is out of range"),
                    )
                })?;
            *require_output(output, "output")? = PioScucReactiveReservePeriodView {
                reactive_up_requirement_pu: up,
                reactive_down_requirement_pu: down,
            };
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_scuc_instance_reactive_reserve_zone_bus_at(
    instance: *const PioCalculationInstance,
    zone_index: usize,
    bus_index: usize,
    output: *mut PioComponentIdView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let inputs = scuc_inputs_or_error(require_calculation_instance(instance)?)?;
            let zone = inputs
                .reactive_reserve_zones
                .get(zone_index)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!("SCUC reactive reserve zone index {zone_index} is out of range"),
                    )
                })?;
            let bus = zone.buses.get(bus_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("SCUC reserve zone bus index {bus_index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = component_id_view(bus);
            Ok(true)
        })
    }
}

/// Return the number of named contingencies.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_scuc_instance_contingency_count(
    instance: *const PioCalculationInstance,
) -> usize {
    unsafe { PioCalculationInstance::get(instance) }
        .and_then(scuc_inputs)
        .map_or(0, |inputs| inputs.contingencies.len())
}

fn scuc_contingency_view(contingency: &powerio_prob::ScucContingency) -> PioScucContingencyView {
    PioScucContingencyView {
        id: component_id_view(&contingency.id),
        component_count: contingency.components.len(),
    }
}

/// Read one named contingency in source order.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_scuc_instance_contingency_at(
    instance: *const PioCalculationInstance,
    contingency_index: usize,
    output: *mut PioScucContingencyView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let inputs = scuc_inputs_or_error(require_calculation_instance(instance)?)?;
            let contingency = inputs.contingencies.get(contingency_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("SCUC contingency index {contingency_index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = scuc_contingency_view(contingency);
            Ok(true)
        })
    }
}

/// Read one named contingency by its exact source UID.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_scuc_instance_contingency_get(
    instance: *const PioCalculationInstance,
    uid: *const c_char,
    uid_len: usize,
    output: *mut PioScucContingencyView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let uid = required_str(uid, uid_len, "contingency_uid")?;
            let inputs = scuc_inputs_or_error(require_calculation_instance(instance)?)?;
            let contingency = inputs.contingency(uid).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("SCUC contingency UID '{uid}' does not exist"),
                )
            })?;
            *require_output(output, "output")? = scuc_contingency_view(contingency);
            Ok(true)
        })
    }
}

/// Read one stable component identity from a named contingency.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_scuc_instance_contingency_component_at(
    instance: *const PioCalculationInstance,
    contingency_index: usize,
    component_index: usize,
    output: *mut PioScucContingencyComponentView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let inputs = scuc_inputs_or_error(require_calculation_instance(instance)?)?;
            let contingency = inputs.contingencies.get(contingency_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("SCUC contingency index {contingency_index} is out of range"),
                )
            })?;
            let component = contingency.components.get(component_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!(
                        "SCUC contingency {contingency_index} component index {component_index} is out of range"
                    ),
                )
            })?;
            *require_output(output, "output")? = PioScucContingencyComponentView {
                id: component_id_view(component),
            };
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_calculation_solution_balanced_network(
    solution: *const PioCalculationSolution,
    error: *mut *mut PioError,
) -> *mut PioBalancedNetwork {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let solution = PioCalculationSolution::get(solution).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_NULL_HANDLE,
                    "PioCalculationSolution must not be NULL",
                )
            })?;
            let projection = match solution.value() {
                Some(PioValue::DcPfSolution(_)) => BalancedNetworkProjection::DcPfSolution,
                Some(PioValue::AcPfSolution(_)) => BalancedNetworkProjection::AcPfSolution,
                Some(PioValue::DcOpfSolution(_)) => BalancedNetworkProjection::DcOpfSolution,
                Some(PioValue::AcOpfSolution(_)) => BalancedNetworkProjection::AcOpfSolution,
                Some(PioValue::SocwrOpfSolution(_)) => BalancedNetworkProjection::SocwrOpfSolution,
                Some(PioValue::AcScucSolution(_)) => BalancedNetworkProjection::AcScucSolution,
                _ => {
                    return Err(boundary_error(
                        &codes::REQUEST_CAPI_TYPE_MISMATCH,
                        "the calculation solution does not use powerio.BalancedNetwork",
                    ));
                }
            };
            Ok(make_balanced_network_view(solution, projection))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_calculation_solution_multiconductor_network(
    solution: *const PioCalculationSolution,
    error: *mut *mut PioError,
) -> *mut PioMulticonductorNetwork {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let solution = PioCalculationSolution::get(solution).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_NULL_HANDLE,
                    "PioCalculationSolution must not be NULL",
                )
            })?;
            let projection = match solution.value() {
                Some(PioValue::McAcPfSolution(_)) => {
                    MulticonductorNetworkProjection::McAcPfSolution
                }
                Some(PioValue::McAcOpfSolution(_)) => {
                    MulticonductorNetworkProjection::McAcOpfSolution
                }
                _ => {
                    return Err(boundary_error(
                        &codes::REQUEST_CAPI_TYPE_MISMATCH,
                        "the calculation solution does not use powerio.MulticonductorNetwork",
                    ));
                }
            };
            Ok(make_multiconductor_network_view(solution, projection))
        })
    }
}

/// Read one operating point quantity by its PowerIO quantity name and stable
/// component identity. Multiconductor terminal identities use
/// component/terminal. Returns false when the point does not contain the
/// quantity or identity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_operating_point_get_value(
    point: *const PioOperatingPoint,
    quantity: *const c_char,
    quantity_len: usize,
    identity: *const c_char,
    identity_len: usize,
    out_value: *mut f64,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            if out_value.is_null() {
                return Err(boundary_error(
                    &codes::BIND_CAPI_NULL_ARGUMENT,
                    "out_value must not be NULL",
                ));
            }
            let quantity = required_str(quantity, quantity_len, "quantity")?;
            let identity = required_str(identity, identity_len, "identity")?;
            let point = PioOperatingPoint::get(point).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_NULL_HANDLE,
                    "PioOperatingPoint must not be NULL",
                )
            })?;
            let value = if let Some(point) = point.balanced() {
                use powerio_prob::{
                    BalancedOperatingPointFlag as F, BalancedOperatingPointQuantity as Q,
                };
                match quantity {
                    "bus_voltage_magnitude" => point.values(Q::BusVoltageMagnitude),
                    "bus_voltage_angle" => point.values(Q::BusVoltageAngle),
                    "bus_active_injection" => point.values(Q::BusActiveInjection),
                    "bus_reactive_injection" => point.values(Q::BusReactiveInjection),
                    "generator_active_power" => point.values(Q::GeneratorActivePower),
                    "generator_reactive_power" => point.values(Q::GeneratorReactivePower),
                    "generator_voltage_setpoint" => point.values(Q::GeneratorVoltageSetpoint),
                    "load_active_power" => point.values(Q::LoadActivePower),
                    "load_reactive_power" => point.values(Q::LoadReactivePower),
                    "branch_tap_ratio" => point.values(Q::BranchTapRatio),
                    "branch_phase_shift" => point.values(Q::BranchPhaseShift),
                    "generator_in_service" => {
                        let found = point
                            .flags(F::GeneratorInService)
                            .and_then(|mut values| values.find(|(key, _)| *key == identity))
                            .map(|(_, value)| if value { 1.0 } else { 0.0 });
                        if let Some(found) = found {
                            *out_value = found;
                            return Ok(true);
                        }
                        return Ok(false);
                    }
                    "branch_in_service" => {
                        let found = point
                            .flags(F::BranchInService)
                            .and_then(|mut values| values.find(|(key, _)| *key == identity))
                            .map(|(_, value)| if value { 1.0 } else { 0.0 });
                        if let Some(found) = found {
                            *out_value = found;
                            return Ok(true);
                        }
                        return Ok(false);
                    }
                    "switch_closed" => {
                        let found = point
                            .flags(F::SwitchClosed)
                            .and_then(|mut values| values.find(|(key, _)| *key == identity))
                            .map(|(_, value)| if value { 1.0 } else { 0.0 });
                        if let Some(found) = found {
                            *out_value = found;
                            return Ok(true);
                        }
                        return Ok(false);
                    }
                    _ => {
                        return Err(boundary_error(
                            &codes::REQUEST_CAPI_QUANTITY_UNKNOWN,
                            format!("unknown balanced operating point quantity '{quantity}'"),
                        ));
                    }
                }
                .and_then(|mut values| values.find(|(key, _)| *key == identity))
                .map(|(_, value)| value)
            } else if let Some(point) = point.multiconductor() {
                use powerio_prob::{
                    MulticonductorOperatingPointFlag as F,
                    MulticonductorOperatingPointQuantity as Q,
                };
                match quantity {
                    "terminal_voltage_magnitude" => point.values(Q::TerminalVoltageMagnitude),
                    "terminal_voltage_angle" => point.values(Q::TerminalVoltageAngle),
                    "load_active_power" => point.values(Q::LoadActivePower),
                    "load_reactive_power" => point.values(Q::LoadReactivePower),
                    "generator_active_power" => point.values(Q::GeneratorActivePower),
                    "generator_reactive_power" => point.values(Q::GeneratorReactivePower),
                    "transformer_tap" => point.values(Q::TransformerTap),
                    "capacitor_steps" => point.values(Q::CapacitorSteps),
                    "switch_closed" => {
                        let found = point
                            .flags(F::SwitchClosed)
                            .and_then(|mut values| values.find(|(key, _)| *key == identity))
                            .map(|(_, value)| if value { 1.0 } else { 0.0 });
                        if let Some(found) = found {
                            *out_value = found;
                            return Ok(true);
                        }
                        return Ok(false);
                    }
                    _ => {
                        return Err(boundary_error(
                            &codes::REQUEST_CAPI_QUANTITY_UNKNOWN,
                            format!("unknown multiconductor operating point quantity '{quantity}'"),
                        ));
                    }
                }
                .and_then(|mut values| values.find(|(key, _)| *key == identity))
                .map(|(_, value)| value)
            } else {
                return Err(boundary_error(
                    &codes::REQUEST_CAPI_TYPE_MISMATCH,
                    "the handle does not refer to an operating point",
                ));
            };
            if let Some(value) = value {
                *out_value = value;
                Ok(true)
            } else {
                Ok(false)
            }
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_operating_point_retain(
    point: *const PioOperatingPoint,
) -> *mut PioOperatingPoint {
    unsafe { PioOperatingPoint::retain_raw(point) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_operating_point_release(point: *mut PioOperatingPoint) {
    unsafe { PioOperatingPoint::release_raw(point) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_calculation_instance_retain(
    instance: *const PioCalculationInstance,
) -> *mut PioCalculationInstance {
    unsafe { PioCalculationInstance::retain_raw(instance) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_calculation_instance_release(instance: *mut PioCalculationInstance) {
    unsafe { PioCalculationInstance::release_raw(instance) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_dc_opf_preparation_retain(
    preparation: *const PioDcOpfPreparation,
) -> *mut PioDcOpfPreparation {
    unsafe { PioDcOpfPreparation::retain_raw(preparation) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_dc_opf_preparation_release(preparation: *mut PioDcOpfPreparation) {
    unsafe { PioDcOpfPreparation::release_raw(preparation) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_opf_preparation_retain(
    preparation: *const PioAcOpfPreparation,
) -> *mut PioAcOpfPreparation {
    unsafe { PioAcOpfPreparation::retain_raw(preparation) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_opf_preparation_release(preparation: *mut PioAcOpfPreparation) {
    unsafe { PioAcOpfPreparation::release_raw(preparation) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_calculation_solution_retain(
    solution: *const PioCalculationSolution,
) -> *mut PioCalculationSolution {
    unsafe { PioCalculationSolution::retain_raw(solution) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_calculation_solution_release(solution: *mut PioCalculationSolution) {
    unsafe { PioCalculationSolution::release_raw(solution) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_value_retain(value: *const PioValueHandle) -> *mut PioValueHandle {
    unsafe { PioValueHandle::retain_raw(value) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_value_release(value: *mut PioValueHandle) {
    unsafe { PioValueHandle::release_raw(value) };
}

// ---- collections -----------------------------------------------------------

fn time_series(value: &ValueInner) -> Option<&PioTimeSeries> {
    match value.value()? {
        PioValue::TimeSeries(series) => Some(series),
        _ => None,
    }
}

fn scenario_set(value: &ValueInner) -> Option<&PioScenarioSet> {
    match value.value()? {
        PioValue::ScenarioSet(scenarios) => Some(scenarios),
        _ => None,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_time_series_len(series: *const PioTimeSeriesHandle) -> usize {
    unsafe { PioTimeSeriesHandle::get(series) }
        .and_then(time_series)
        .map_or(0, PioTimeSeries::len)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_time_series_element_type(
    series: *const PioTimeSeriesHandle,
) -> PioStringView {
    unsafe { PioTimeSeriesHandle::get(series) }
        .and_then(time_series)
        .map_or(PioStringView::EMPTY, |series| {
            PioStringView::new(series.element_type())
        })
}

/// Return an owner-rooted entry by zero-based position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_time_series_get(
    series: *const PioTimeSeriesHandle,
    index: usize,
    error: *mut *mut PioError,
) -> *mut PioValueHandle {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let series = PioTimeSeriesHandle::get(series).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_NULL_HANDLE,
                    "PioTimeSeriesHandle must not be NULL",
                )
            })?;
            let values = time_series(series).ok_or_else(|| {
                boundary_error(
                    &codes::REQUEST_CAPI_TYPE_MISMATCH,
                    "the handle does not refer to a time series",
                )
            })?;
            if values.get(index).is_none() {
                return Err(boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("time series index {index} is out of range"),
                ));
            }
            Ok(value_handle(series.child(ValueStep::TimeSeries(index))))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_time_series_retain(
    series: *const PioTimeSeriesHandle,
) -> *mut PioTimeSeriesHandle {
    unsafe { PioTimeSeriesHandle::retain_raw(series) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_time_series_release(series: *mut PioTimeSeriesHandle) {
    unsafe { PioTimeSeriesHandle::release_raw(series) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_scenario_set_len(set: *const PioScenarioSetHandle) -> usize {
    unsafe { PioScenarioSetHandle::get(set) }
        .and_then(scenario_set)
        .map_or(0, PioScenarioSet::len)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_scenario_set_element_type(
    set: *const PioScenarioSetHandle,
) -> PioStringView {
    unsafe { PioScenarioSetHandle::get(set) }
        .and_then(scenario_set)
        .map_or(PioStringView::EMPTY, |set| {
            PioStringView::new(set.element_type())
        })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_scenario_set_id_at(
    set: *const PioScenarioSetHandle,
    index: usize,
) -> PioStringView {
    unsafe { PioScenarioSetHandle::get(set) }
        .and_then(scenario_set)
        .and_then(|set| set.iter().nth(index))
        .map_or(PioStringView::EMPTY, |scenario| {
            PioStringView::new(scenario.id().as_str())
        })
}

/// Return an owner-rooted scenario value by zero-based position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_scenario_set_get_at(
    set: *const PioScenarioSetHandle,
    index: usize,
    error: *mut *mut PioError,
) -> *mut PioValueHandle {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let set = PioScenarioSetHandle::get(set).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_NULL_HANDLE,
                    "PioScenarioSetHandle must not be NULL",
                )
            })?;
            let values = scenario_set(set).ok_or_else(|| {
                boundary_error(
                    &codes::REQUEST_CAPI_TYPE_MISMATCH,
                    "the handle does not refer to a scenario set",
                )
            })?;
            if values.get_at(index).is_none() {
                return Err(boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("scenario position {index} is out of range"),
                ));
            }
            Ok(value_handle(set.child(ValueStep::Scenario(index))))
        })
    }
}

/// Return an owner-rooted scenario value by exact scenario ID.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_scenario_set_get(
    set: *const PioScenarioSetHandle,
    id: *const c_char,
    id_len: usize,
    error: *mut *mut PioError,
) -> *mut PioValueHandle {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let id = required_str(id, id_len, "scenario_id")?;
            let set = PioScenarioSetHandle::get(set).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_NULL_HANDLE,
                    "PioScenarioSetHandle must not be NULL",
                )
            })?;
            let values = scenario_set(set).ok_or_else(|| {
                boundary_error(
                    &codes::REQUEST_CAPI_TYPE_MISMATCH,
                    "the handle does not refer to a scenario set",
                )
            })?;
            let Some(position) = values
                .iter()
                .position(|scenario| scenario.id().as_str() == id)
            else {
                return Err(boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("scenario ID '{id}' does not exist"),
                ));
            };
            Ok(value_handle(set.child(ValueStep::Scenario(position))))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_scenario_set_retain(
    set: *const PioScenarioSetHandle,
) -> *mut PioScenarioSetHandle {
    unsafe { PioScenarioSetHandle::retain_raw(set) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_scenario_set_release(set: *mut PioScenarioSetHandle) {
    unsafe { PioScenarioSetHandle::release_raw(set) };
}

// ---- network access --------------------------------------------------------

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_balanced_network_name(
    network: *const PioBalancedNetwork,
) -> PioStringView {
    unsafe { PioBalancedNetwork::get(network) }
        .and_then(BalancedNetworkInner::network)
        .map_or(PioStringView::EMPTY, |network| {
            PioStringView::new(network.name())
        })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_balanced_network_base_mva(network: *const PioBalancedNetwork) -> f64 {
    unsafe { PioBalancedNetwork::get(network) }
        .and_then(BalancedNetworkInner::network)
        .map_or(f64::NAN, BalancedNetwork::base_mva)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_balanced_network_base_frequency_hz(
    network: *const PioBalancedNetwork,
) -> f64 {
    unsafe { PioBalancedNetwork::get(network) }
        .and_then(BalancedNetworkInner::network)
        .map_or(f64::NAN, BalancedNetwork::base_frequency)
}

/// Read the optional coordinate space metadata for a balanced network.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_balanced_network_geo(
    network: *const PioBalancedNetwork,
    output: *mut PioBalancedGeoView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_balanced_network(network)?;
            *require_output(output, "output")? = balanced_geo_view(network.geo().as_ref());
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_balanced_network_has_detailed_connectivity(
    network: *const PioBalancedNetwork,
) -> bool {
    unsafe { PioBalancedNetwork::get(network) }
        .and_then(BalancedNetworkInner::network)
        .is_some_and(|network| network.detailed_connectivity().is_some())
}

/// Return the optional owner-rooted detailed connectivity view.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_balanced_network_detailed_connectivity(
    network: *const PioBalancedNetwork,
) -> *mut PioDetailedConnectivity {
    let Some(owner) = (unsafe { PioBalancedNetwork::arc(network) }) else {
        return std::ptr::null_mut();
    };
    if owner
        .network()
        .is_none_or(|network| network.detailed_connectivity().is_none())
    {
        return std::ptr::null_mut();
    }
    PioDetailedConnectivity::new_raw(DetailedConnectivityInner { owner })
}

/// Read every detailed connectivity table length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_counts(
    details: *const PioDetailedConnectivity,
    output: *mut PioDetailedConnectivityCountsView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            *require_output(output, "output")? = PioDetailedConnectivityCountsView {
                omitted_fields: details.omitted_fields.len(),
                component_metadata: details.component_metadata.len(),
                subnetworks: details.subnetworks.len(),
                substations: details.substations.len(),
                voltage_levels: details.voltage_levels.len(),
                bus_breaker_buses: details.bus_breaker_buses.len(),
                calculated_buses: details.calculated_buses.len(),
                connectivity_nodes: details.connectivity_nodes.len(),
                busbar_sections: details.busbar_sections.len(),
                junctions: details.junctions.len(),
                terminals: details.terminals.len(),
                switches: details.switches.len(),
                internal_connections: details.internal_connections.len(),
                operational_limit_groups: details.operational_limit_groups.len(),
                tap_changers: details.tap_changers.len(),
                equipment_reactive_limits: details.equipment_reactive_limits.len(),
                boundary_lines: details.boundary_lines.len(),
                tie_lines: details.tie_lines.len(),
                dc_converter_units: details.dc_converter_units.len(),
                dc_topological_nodes: details.dc_topological_nodes.len(),
                dc_nodes: details.dc_nodes.len(),
                dc_grounds: details.dc_grounds.len(),
                dc_busbars: details.dc_busbars.len(),
                dc_lines: details.dc_lines.len(),
                dc_series_devices: details.dc_series_devices.len(),
                dc_switches: details.dc_switches.len(),
                voltage_source_converters: details.voltage_source_converters.len(),
                line_commutated_converters: details.line_commutated_converters.len(),
            };
            Ok(true)
        })
    }
}

fn omitted_field_name(value: powerio_tx::OmittedFieldName) -> &'static str {
    match value {
        powerio_tx::OmittedFieldName::ActivePower => "active_power",
        powerio_tx::OmittedFieldName::ReactivePower => "reactive_power",
        powerio_tx::OmittedFieldName::VoltageSetpoint => "voltage_setpoint",
        powerio_tx::OmittedFieldName::RatedApparentPower => "rated_apparent_power",
        powerio_tx::OmittedFieldName::ShuntConductancePerSection => "shunt_conductance_per_section",
        _ => "unknown",
    }
}

fn topology_kind_name(kind: powerio_tx::TopologyKind) -> &'static str {
    match kind {
        powerio_tx::TopologyKind::BusBreaker => "bus_breaker",
        powerio_tx::TopologyKind::NodeBreaker => "node_breaker",
        _ => "unknown",
    }
}

fn topology_switch_kind_name(kind: powerio_tx::SwitchKind) -> &'static str {
    match kind {
        powerio_tx::SwitchKind::Breaker => "breaker",
        powerio_tx::SwitchKind::Disconnector => "disconnector",
        powerio_tx::SwitchKind::LoadBreakSwitch => "load_break_switch",
        _ => "unknown",
    }
}

fn curve_style_name(style: powerio_tx::CurveStyle) -> &'static str {
    match style {
        powerio_tx::CurveStyle::ConstantYValue => "constant_y_value",
        powerio_tx::CurveStyle::StraightLineYValues => "straight_line_y_values",
        _ => "unknown",
    }
}

fn empty_reactive_limits_view() -> PioReactiveLimitsView {
    PioReactiveLimitsView {
        kind: PioStringView::EMPTY,
        minimum_reactive_power_mvar: 0.0,
        maximum_reactive_power_mvar: 0.0,
        has_minimum_and_maximum: false,
        curve_style: PioStringView::EMPTY,
        has_curve_style: false,
        property_count: 0,
        point_count: 0,
    }
}

fn reactive_limits_view(
    value: Option<&powerio_tx::ReactiveLimits>,
) -> (PioReactiveLimitsView, bool) {
    match value {
        Some(powerio_tx::ReactiveLimits::MinMax(limits)) => (
            PioReactiveLimitsView {
                kind: PioStringView::new("min_max"),
                minimum_reactive_power_mvar: limits.minimum_reactive_power_mvar,
                maximum_reactive_power_mvar: limits.maximum_reactive_power_mvar,
                has_minimum_and_maximum: true,
                curve_style: PioStringView::EMPTY,
                has_curve_style: false,
                property_count: limits.properties.len(),
                point_count: 0,
            },
            true,
        ),
        Some(powerio_tx::ReactiveLimits::CapabilityCurve(curve)) => (
            PioReactiveLimitsView {
                kind: PioStringView::new("capability_curve"),
                minimum_reactive_power_mvar: 0.0,
                maximum_reactive_power_mvar: 0.0,
                has_minimum_and_maximum: false,
                curve_style: PioStringView::new(curve_style_name(curve.curve_style)),
                has_curve_style: true,
                property_count: curve.properties.len(),
                point_count: curve.points.len(),
            },
            true,
        ),
        Some(_) => (empty_reactive_limits_view(), true),
        None => (empty_reactive_limits_view(), false),
    }
}

fn reactive_limit_properties(
    limits: &powerio_tx::ReactiveLimits,
) -> &std::collections::BTreeMap<String, String> {
    match limits {
        powerio_tx::ReactiveLimits::MinMax(limits) => &limits.properties,
        powerio_tx::ReactiveLimits::CapabilityCurve(curve) => &curve.properties,
        _ => unreachable!("all reactive limit forms are handled"),
    }
}

fn reactive_capability_curve(
    limits: &powerio_tx::ReactiveLimits,
) -> Option<&powerio_tx::ReactiveCapabilityCurve> {
    match limits {
        powerio_tx::ReactiveLimits::CapabilityCurve(curve) => Some(curve),
        _ => None,
    }
}

fn string_property_view(
    properties: &std::collections::BTreeMap<String, String>,
    index: usize,
) -> Result<PioStringPropertyView, *mut PioError> {
    let (name, value) = properties.iter().nth(index).ok_or_else(|| {
        boundary_error(
            &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
            format!("property index {index} is out of range"),
        )
    })?;
    Ok(PioStringPropertyView {
        name: PioStringView::new(name),
        value: PioStringView::new(value),
    })
}

fn reactive_capability_point_view(
    curve: &powerio_tx::ReactiveCapabilityCurve,
    index: usize,
) -> Result<PioReactiveCapabilityCurvePointView, *mut PioError> {
    let point = curve.points.get(index).ok_or_else(|| {
        boundary_error(
            &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
            format!("reactive capability point index {index} is out of range"),
        )
    })?;
    Ok(PioReactiveCapabilityCurvePointView {
        active_power_mw: point.active_power_mw,
        minimum_reactive_power_mvar: point.minimum_reactive_power_mvar,
        maximum_reactive_power_mvar: point.maximum_reactive_power_mvar,
        property_count: point.properties.len(),
    })
}

fn empty_boundary_line_generation_view() -> PioBoundaryLineGenerationView {
    PioBoundaryLineGenerationView {
        voltage_regulation_on: false,
        minimum_active_power_mw: 0.0,
        has_minimum_active_power: false,
        maximum_active_power_mw: 0.0,
        has_maximum_active_power: false,
        target_active_power_mw: 0.0,
        has_target_active_power: false,
        target_reactive_power_mvar: 0.0,
        has_target_reactive_power: false,
        target_voltage_kv: 0.0,
        has_target_voltage: false,
        reactive_limits: empty_reactive_limits_view(),
        has_reactive_limits: false,
    }
}

fn boundary_line_generation_view(
    value: Option<&powerio_tx::BoundaryLineGeneration>,
) -> (PioBoundaryLineGenerationView, bool) {
    let Some(value) = value else {
        return (empty_boundary_line_generation_view(), false);
    };
    let (reactive_limits, has_reactive_limits) =
        reactive_limits_view(value.reactive_limits.as_ref());
    (
        PioBoundaryLineGenerationView {
            voltage_regulation_on: value.voltage_regulation_on,
            minimum_active_power_mw: value.minimum_active_power_mw.unwrap_or(0.0),
            has_minimum_active_power: value.minimum_active_power_mw.is_some(),
            maximum_active_power_mw: value.maximum_active_power_mw.unwrap_or(0.0),
            has_maximum_active_power: value.maximum_active_power_mw.is_some(),
            target_active_power_mw: value.target_active_power_mw.unwrap_or(0.0),
            has_target_active_power: value.target_active_power_mw.is_some(),
            target_reactive_power_mvar: value.target_reactive_power_mvar.unwrap_or(0.0),
            has_target_reactive_power: value.target_reactive_power_mvar.is_some(),
            target_voltage_kv: value.target_voltage_kv.unwrap_or(0.0),
            has_target_voltage: value.target_voltage_kv.is_some(),
            reactive_limits,
            has_reactive_limits,
        },
        true,
    )
}

fn topology_endpoint_view(
    endpoint: &powerio_tx::TopologyEndpoint,
) -> (PioStringView, PioComponentIdView) {
    match endpoint {
        powerio_tx::TopologyEndpoint::Bus(component) => {
            (PioStringView::new("bus"), component_id_view(component))
        }
        powerio_tx::TopologyEndpoint::Node(component) => {
            (PioStringView::new("node"), component_id_view(component))
        }
        _ => (PioStringView::new("unknown"), empty_component_id_view()),
    }
}

fn tap_changer_kind_name(kind: powerio_tx::TapChangerKind) -> &'static str {
    match kind {
        powerio_tx::TapChangerKind::Ratio => "ratio",
        powerio_tx::TapChangerKind::Phase => "phase",
        _ => "unknown",
    }
}

fn tap_changer_regulation_mode_name(mode: powerio_tx::TapChangerRegulationMode) -> &'static str {
    match mode {
        powerio_tx::TapChangerRegulationMode::Voltage => "voltage",
        powerio_tx::TapChangerRegulationMode::ReactivePower => "reactive_power",
        powerio_tx::TapChangerRegulationMode::ActivePower => "active_power",
        powerio_tx::TapChangerRegulationMode::Current => "current",
        _ => "unknown",
    }
}

fn dc_polarity_name(value: powerio_tx::DcPolarity) -> &'static str {
    match value {
        powerio_tx::DcPolarity::Positive => "positive",
        powerio_tx::DcPolarity::Middle => "middle",
        powerio_tx::DcPolarity::Negative => "negative",
        _ => "unknown",
    }
}

fn dc_switch_kind_name(value: powerio_tx::DcSwitchKind) -> &'static str {
    match value {
        powerio_tx::DcSwitchKind::Switch => "switch",
        powerio_tx::DcSwitchKind::Breaker => "breaker",
        powerio_tx::DcSwitchKind::Disconnector => "disconnector",
        _ => "unknown",
    }
}

fn dc_converter_operation_mode_name(value: powerio_tx::DcConverterOperatingMode) -> &'static str {
    match value {
        powerio_tx::DcConverterOperatingMode::Bipolar => "bipolar",
        powerio_tx::DcConverterOperatingMode::MonopolarGroundReturn => "monopolar_ground_return",
        powerio_tx::DcConverterOperatingMode::MonopolarMetallicReturn => {
            "monopolar_metallic_return"
        }
        _ => "unknown",
    }
}

fn ac_dc_converter_control_mode_name(value: powerio_tx::AcDcConverterControlMode) -> &'static str {
    match value {
        powerio_tx::AcDcConverterControlMode::ActivePowerAtPcc => "active_power_at_pcc",
        powerio_tx::AcDcConverterControlMode::DcVoltage => "dc_voltage",
        powerio_tx::AcDcConverterControlMode::DcCurrent => "dc_current",
        powerio_tx::AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroopCurve => {
            "active_power_at_pcc_and_dc_voltage_droop_curve"
        }
        powerio_tx::AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroop => {
            "active_power_at_pcc_and_dc_voltage_droop"
        }
        powerio_tx::AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroopWithCompensation => {
            "active_power_at_pcc_and_dc_voltage_droop_with_compensation"
        }
        powerio_tx::AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroopPilot => {
            "active_power_at_pcc_and_dc_voltage_droop_pilot"
        }
        _ => "unknown",
    }
}

fn line_commutated_converter_reactive_model_name(
    value: powerio_tx::LineCommutatedConverterReactiveModel,
) -> &'static str {
    match value {
        powerio_tx::LineCommutatedConverterReactiveModel::FixedPowerFactor => "fixed_power_factor",
        powerio_tx::LineCommutatedConverterReactiveModel::CalculatedPowerFactor => {
            "calculated_power_factor"
        }
        _ => "unknown",
    }
}

fn line_commutated_converter_operating_mode_name(
    value: powerio_tx::LineCommutatedConverterOperatingMode,
) -> &'static str {
    match value {
        powerio_tx::LineCommutatedConverterOperatingMode::Rectifier => "rectifier",
        powerio_tx::LineCommutatedConverterOperatingMode::Inverter => "inverter",
        _ => "unknown",
    }
}

fn empty_dc_terminal_view() -> PioDcTerminalView {
    PioDcTerminalView {
        component: empty_component_id_view(),
        has_component: false,
        sequence_number: 0,
        has_sequence_number: false,
        dc_node: empty_component_id_view(),
        has_dc_node: false,
        dc_topological_node: empty_component_id_view(),
        has_dc_topological_node: false,
        polarity: PioStringView::EMPTY,
        has_polarity: false,
        connected: false,
        has_connected: false,
        active_power_mw: 0.0,
        has_active_power: false,
        current_a: 0.0,
        has_current: false,
    }
}

fn dc_terminal_view(value: &powerio_tx::DcTerminal) -> PioDcTerminalView {
    let (component, has_component) = optional_component_id_view(value.component.as_ref());
    let (dc_node, has_dc_node) = optional_component_id_view(value.dc_node.as_ref());
    let (dc_topological_node, has_dc_topological_node) =
        optional_component_id_view(value.dc_topological_node.as_ref());
    let (polarity, has_polarity) = value
        .polarity
        .map_or((PioStringView::EMPTY, false), |polarity| {
            (PioStringView::new(dc_polarity_name(polarity)), true)
        });
    PioDcTerminalView {
        component,
        has_component,
        sequence_number: value.sequence_number.unwrap_or(0),
        has_sequence_number: value.sequence_number.is_some(),
        dc_node,
        has_dc_node,
        dc_topological_node,
        has_dc_topological_node,
        polarity,
        has_polarity,
        connected: value.connected.unwrap_or(false),
        has_connected: value.connected.is_some(),
        active_power_mw: value.active_power_mw.unwrap_or(0.0),
        has_active_power: value.active_power_mw.is_some(),
        current_a: value.current_a.unwrap_or(0.0),
        has_current: value.current_a.is_some(),
    }
}

#[allow(clippy::too_many_arguments)]
fn dc_equipment_view(
    component: &ComponentId,
    equipment_container: Option<&ComponentId>,
    kind: &'static str,
    terminal1: &powerio_tx::DcTerminal,
    terminal2: Option<&powerio_tx::DcTerminal>,
    rated_dc_voltage_kv: Option<f64>,
    resistance_ohm: Option<f64>,
    inductance_h: Option<f64>,
    capacitance_f: Option<f64>,
    length_km: Option<f64>,
    switch_kind: Option<powerio_tx::DcSwitchKind>,
    open: Option<bool>,
) -> PioDcEquipmentView {
    let (equipment_container, has_equipment_container) =
        optional_component_id_view(equipment_container);
    let (switch_kind, has_switch_kind) = switch_kind
        .map_or((PioStringView::EMPTY, false), |kind| {
            (PioStringView::new(dc_switch_kind_name(kind)), true)
        });
    PioDcEquipmentView {
        component: component_id_view(component),
        equipment_container,
        has_equipment_container,
        kind: PioStringView::new(kind),
        terminal_count: usize::from(terminal2.is_some()) + 1,
        terminal1: dc_terminal_view(terminal1),
        terminal2: terminal2.map_or_else(empty_dc_terminal_view, dc_terminal_view),
        rated_dc_voltage_kv: rated_dc_voltage_kv.unwrap_or(0.0),
        has_rated_dc_voltage: rated_dc_voltage_kv.is_some(),
        resistance_ohm: resistance_ohm.unwrap_or(0.0),
        has_resistance: resistance_ohm.is_some(),
        inductance_h: inductance_h.unwrap_or(0.0),
        has_inductance: inductance_h.is_some(),
        capacitance_f: capacitance_f.unwrap_or(0.0),
        has_capacitance: capacitance_f.is_some(),
        length_km: length_km.unwrap_or(0.0),
        has_length: length_km.is_some(),
        switch_kind,
        has_switch_kind,
        open: open.unwrap_or(false),
        has_open: open.is_some(),
    }
}

/// Read one field that was absent from the source representation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_omitted_field_at(
    details: *const PioDetailedConnectivity,
    index: usize,
    output: *mut PioOmittedFieldView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let field = details.omitted_fields.get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("omitted field index {index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = PioOmittedFieldView {
                component: component_id_view(&field.component),
                field: PioStringView::new(omitted_field_name(field.field)),
            };
            Ok(true)
        })
    }
}

/// Read one component metadata record by zero based table position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_component_metadata_at(
    details: *const PioDetailedConnectivity,
    index: usize,
    output: *mut PioComponentMetadataView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let metadata = details.component_metadata.get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("component metadata index {index} is out of range"),
                )
            })?;
            let (name, has_name) = optional_string_view(metadata.name.as_deref());
            let (equipment_container, has_equipment_container) =
                optional_component_id_view(metadata.equipment_container.as_ref());
            *require_output(output, "output")? = PioComponentMetadataView {
                component: component_id_view(&metadata.component),
                name,
                has_name,
                equipment_container,
                has_equipment_container,
                fictitious: metadata.fictitious,
                alias_count: metadata.aliases.len(),
                external_identifier_count: metadata.external_identifiers.len(),
                property_count: metadata.properties.len(),
            };
            Ok(true)
        })
    }
}

/// Read one alias from a component metadata record.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_component_alias_at(
    details: *const PioDetailedConnectivity,
    metadata_index: usize,
    alias_index: usize,
    output: *mut PioComponentAliasView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let metadata = details
                .component_metadata
                .get(metadata_index)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!("component metadata index {metadata_index} is out of range"),
                    )
                })?;
            let alias = metadata.aliases.get(alias_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("component alias index {alias_index} is out of range"),
                )
            })?;
            let (alias_type, has_alias_type) = optional_string_view(alias.alias_type.as_deref());
            *require_output(output, "output")? = PioComponentAliasView {
                value: PioStringView::new(&alias.value),
                alias_type,
                has_alias_type,
            };
            Ok(true)
        })
    }
}

/// Read one external identifier from a component metadata record.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_external_identifier_at(
    details: *const PioDetailedConnectivity,
    metadata_index: usize,
    identifier_index: usize,
    output: *mut PioExternalIdentifierView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let metadata = details
                .component_metadata
                .get(metadata_index)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!("component metadata index {metadata_index} is out of range"),
                    )
                })?;
            let identifier = metadata
                .external_identifiers
                .get(identifier_index)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!("external identifier index {identifier_index} is out of range"),
                    )
                })?;
            let (authority, has_authority) = optional_string_view(identifier.authority.as_deref());
            *require_output(output, "output")? = PioExternalIdentifierView {
                value: PioStringView::new(&identifier.value),
                authority,
                has_authority,
            };
            Ok(true)
        })
    }
}

/// Read one string property from a component metadata record.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_component_property_at(
    details: *const PioDetailedConnectivity,
    metadata_index: usize,
    property_index: usize,
    output: *mut PioStringPropertyView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let metadata = details
                .component_metadata
                .get(metadata_index)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!("component metadata index {metadata_index} is out of range"),
                    )
                })?;
            let (name, value) =
                metadata
                    .properties
                    .iter()
                    .nth(property_index)
                    .ok_or_else(|| {
                        boundary_error(
                            &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                            format!("component property index {property_index} is out of range"),
                        )
                    })?;
            *require_output(output, "output")? = PioStringPropertyView {
                name: PioStringView::new(name),
                value: PioStringView::new(value),
            };
            Ok(true)
        })
    }
}

fn case_metadata_view(metadata: &powerio_tx::CaseMetadata) -> PioCaseMetadataView {
    let (case_date, has_case_date) = optional_string_view(metadata.case_date.as_deref());
    let (source_model_format, has_source_model_format) =
        optional_string_view(metadata.source_model_format.as_deref());
    let (minimum_validation_level, has_minimum_validation_level) =
        optional_string_view(metadata.minimum_validation_level.as_deref());
    PioCaseMetadataView {
        case_date,
        has_case_date,
        forecast_distance: metadata.forecast_distance.unwrap_or(0),
        has_forecast_distance: metadata.forecast_distance.is_some(),
        source_model_format,
        has_source_model_format,
        minimum_validation_level,
        has_minimum_validation_level,
    }
}

/// Read one PowSybl subnetwork by zero based table position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_subnetwork_at(
    details: *const PioDetailedConnectivity,
    index: usize,
    output: *mut PioSubnetworkView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let subnetwork = details.subnetworks.get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("subnetwork index {index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = PioSubnetworkView {
                component: component_id_view(&subnetwork.component),
                parent: component_id_view(&subnetwork.parent),
                case_metadata: case_metadata_view(&subnetwork.case_metadata),
                component_count: subnetwork.components.len(),
            };
            Ok(true)
        })
    }
}

/// Read one component identity contained by a PowSybl subnetwork.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_subnetwork_component_at(
    details: *const PioDetailedConnectivity,
    subnetwork_index: usize,
    component_index: usize,
    output: *mut PioComponentIdView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let subnetwork = details.subnetworks.get(subnetwork_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("subnetwork index {subnetwork_index} is out of range"),
                )
            })?;
            let component = subnetwork.components.get(component_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("subnetwork component index {component_index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = component_id_view(component);
            Ok(true)
        })
    }
}

/// Read one substation by zero based table position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_substation_at(
    details: *const PioDetailedConnectivity,
    index: usize,
    output: *mut PioSubstationView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let substation = details.substations.get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("substation index {index} is out of range"),
                )
            })?;
            let (country, has_country) = optional_string_view(substation.country.as_deref());
            let (operator_name, has_operator_name) =
                optional_string_view(substation.operator.as_deref());
            *require_output(output, "output")? = PioSubstationView {
                component: component_id_view(&substation.component),
                country,
                has_country,
                operator_name,
                has_operator_name,
                geographical_tag_count: substation.geographical_tags.len(),
            };
            Ok(true)
        })
    }
}

/// Read one geographical tag of a substation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_substation_geographical_tag_at(
    details: *const PioDetailedConnectivity,
    substation_index: usize,
    tag_index: usize,
    output: *mut PioStringView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let substation = details.substations.get(substation_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("substation index {substation_index} is out of range"),
                )
            })?;
            let tag = substation.geographical_tags.get(tag_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("geographical tag index {tag_index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = PioStringView::new(tag);
            Ok(true)
        })
    }
}

/// Read one voltage level by zero based table position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_voltage_level_at(
    details: *const PioDetailedConnectivity,
    index: usize,
    output: *mut PioVoltageLevelView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let level = details.voltage_levels.get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("voltage level index {index} is out of range"),
                )
            })?;
            let (substation, has_substation) =
                optional_component_id_view(level.substation.as_ref());
            *require_output(output, "output")? = PioVoltageLevelView {
                component: component_id_view(&level.component),
                substation,
                has_substation,
                nominal_voltage_kv: level.nominal_kv,
                low_voltage_limit_kv: level.low_voltage_limit_kv.unwrap_or(0.0),
                has_low_voltage_limit: level.low_voltage_limit_kv.is_some(),
                high_voltage_limit_kv: level.high_voltage_limit_kv.unwrap_or(0.0),
                has_high_voltage_limit: level.high_voltage_limit_kv.is_some(),
                topology_kind: PioStringView::new(topology_kind_name(level.topology_kind)),
                bus_count: level.buses.len(),
            };
            Ok(true)
        })
    }
}

/// Read one balanced bus ID assigned to a voltage level.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_voltage_level_bus_at(
    details: *const PioDetailedConnectivity,
    voltage_level_index: usize,
    bus_index: usize,
    output: *mut usize,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let level = details
                .voltage_levels
                .get(voltage_level_index)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!("voltage level index {voltage_level_index} is out of range"),
                    )
                })?;
            let bus = level.buses.get(bus_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("voltage level bus index {bus_index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = bus.0;
            Ok(true)
        })
    }
}

/// Read one configured bus breaker bus by zero based table position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_bus_breaker_bus_at(
    details: *const PioDetailedConnectivity,
    index: usize,
    output: *mut PioBusBreakerBusView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let bus = details.bus_breaker_buses.get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("bus breaker bus index {index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = PioBusBreakerBusView {
                component: component_id_view(&bus.component),
                voltage_level: component_id_view(&bus.voltage_level),
                calculated_bus_id: bus.calculated_bus.map_or(0, |value| value.0),
                has_calculated_bus: bus.calculated_bus.is_some(),
                voltage_kv: bus.voltage_kv.unwrap_or(0.0),
                has_voltage: bus.voltage_kv.is_some(),
                angle_degrees: bus.angle_degrees.unwrap_or(0.0),
                has_angle: bus.angle_degrees.is_some(),
            };
            Ok(true)
        })
    }
}

/// Read one calculated bus by zero based table position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_calculated_bus_at(
    details: *const PioDetailedConnectivity,
    index: usize,
    output: *mut PioCalculatedBusView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let bus = details.calculated_buses.get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("calculated bus index {index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = PioCalculatedBusView {
                voltage_level: component_id_view(&bus.voltage_level),
                calculated_bus_id: bus.calculated_bus.0,
                node_count: bus.nodes.len(),
                voltage_kv: bus.voltage_kv.unwrap_or(0.0),
                has_voltage: bus.voltage_kv.is_some(),
                angle_degrees: bus.angle_degrees.unwrap_or(0.0),
                has_angle: bus.angle_degrees.is_some(),
            };
            Ok(true)
        })
    }
}

/// Read one node identity from a calculated bus.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_calculated_bus_node_at(
    details: *const PioDetailedConnectivity,
    calculated_bus_index: usize,
    node_index: usize,
    output: *mut PioComponentIdView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let bus = details
                .calculated_buses
                .get(calculated_bus_index)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!("calculated bus index {calculated_bus_index} is out of range"),
                    )
                })?;
            let node = bus.nodes.get(node_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("calculated bus node index {node_index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = component_id_view(node);
            Ok(true)
        })
    }
}

/// Read one connectivity node by zero based table position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_node_at(
    details: *const PioDetailedConnectivity,
    index: usize,
    output: *mut PioConnectivityNodeView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let node = details.connectivity_nodes.get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("connectivity node index {index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = PioConnectivityNodeView {
                component: component_id_view(&node.component),
                voltage_level: component_id_view(&node.voltage_level),
                node_number: node.node_number.unwrap_or(0),
                has_node_number: node.node_number.is_some(),
                calculated_bus_id: node.calculated_bus.map_or(0, |bus| bus.0),
                has_calculated_bus: node.calculated_bus.is_some(),
            };
            Ok(true)
        })
    }
}

/// Read one busbar section by zero based table position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_busbar_section_at(
    details: *const PioDetailedConnectivity,
    index: usize,
    output: *mut PioBusbarSectionView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let section = details.busbar_sections.get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("busbar section index {index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = PioBusbarSectionView {
                component: component_id_view(&section.component),
                voltage_level: component_id_view(&section.voltage_level),
                node: component_id_view(&section.node),
            };
            Ok(true)
        })
    }
}

/// Read one CIM junction by zero based table position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_junction_at(
    details: *const PioDetailedConnectivity,
    index: usize,
    output: *mut PioJunctionView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let junction = details.junctions.get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("junction index {index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = PioJunctionView {
                component: component_id_view(&junction.component),
            };
            Ok(true)
        })
    }
}

/// Read one AC terminal by zero based table position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_terminal_at(
    details: *const PioDetailedConnectivity,
    index: usize,
    output: *mut PioDetailedTerminalView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let terminal = details.terminals.get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("terminal index {index} is out of range"),
                )
            })?;
            let (bus, has_bus) = optional_component_id_view(terminal.bus.as_ref());
            let (connectable_bus, has_connectable_bus) =
                optional_component_id_view(terminal.connectable_bus.as_ref());
            let (node, has_node) = optional_component_id_view(terminal.node.as_ref());
            let (component, has_component) =
                optional_component_id_view(terminal.component.as_ref());
            *require_output(output, "output")? = PioDetailedTerminalView {
                component,
                has_component,
                equipment: component_id_view(&terminal.equipment),
                terminal: terminal.terminal,
                voltage_level: component_id_view(&terminal.voltage_level),
                bus,
                has_bus,
                connectable_bus,
                has_connectable_bus,
                node,
                has_node,
                connected: terminal.connected,
                active_power_mw: terminal.active_power_mw.unwrap_or(0.0),
                has_active_power: terminal.active_power_mw.is_some(),
                reactive_power_mvar: terminal.reactive_power_mvar.unwrap_or(0.0),
                has_reactive_power: terminal.reactive_power_mvar.is_some(),
            };
            Ok(true)
        })
    }
}

/// Read one detailed topology switch by zero based table position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_switch_at(
    details: *const PioDetailedConnectivity,
    index: usize,
    output: *mut PioTopologySwitchView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let switch = details.switches.get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("topology switch index {index} is out of range"),
                )
            })?;
            let (endpoint1_kind, endpoint1) = topology_endpoint_view(&switch.endpoint1);
            let (endpoint2_kind, endpoint2) = topology_endpoint_view(&switch.endpoint2);
            *require_output(output, "output")? = PioTopologySwitchView {
                component: component_id_view(&switch.component),
                voltage_level: component_id_view(&switch.voltage_level),
                kind: PioStringView::new(topology_switch_kind_name(switch.kind)),
                endpoint1_kind,
                endpoint1,
                endpoint2_kind,
                endpoint2,
                open: switch.open,
                retained: switch.retained,
            };
            Ok(true)
        })
    }
}

/// Read one node breaker internal connection by zero based table position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_internal_connection_at(
    details: *const PioDetailedConnectivity,
    index: usize,
    output: *mut PioInternalConnectionView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let connection = details.internal_connections.get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("internal connection index {index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = PioInternalConnectionView {
                voltage_level: component_id_view(&connection.voltage_level),
                node1: component_id_view(&connection.node1),
                node2: component_id_view(&connection.node2),
            };
            Ok(true)
        })
    }
}

/// Read one operational limit group by zero based table position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_operational_limit_group_at(
    details: *const PioDetailedConnectivity,
    index: usize,
    output: *mut PioOperationalLimitGroupView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let group = details.operational_limit_groups.get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("operational limit group index {index} is out of range"),
                )
            })?;
            let current = group.current_limits.as_ref();
            let active = group.active_power_limits.as_ref();
            let apparent = group.apparent_power_limits.as_ref();
            let (current_name, has_current_name) = optional_string_view(
                current.and_then(|limits| limits.permanent_limit_name.as_deref()),
            );
            let (active_name, has_active_name) = optional_string_view(
                active.and_then(|limits| limits.permanent_limit_name.as_deref()),
            );
            let (apparent_name, has_apparent_name) = optional_string_view(
                apparent.and_then(|limits| limits.permanent_limit_name.as_deref()),
            );
            *require_output(output, "output")? = PioOperationalLimitGroupView {
                equipment: component_id_view(&group.equipment),
                terminal: group.terminal,
                id: PioStringView::new(&group.id),
                selected: group.selected,
                property_count: group.properties.len(),
                has_current_limits: current.is_some(),
                current_permanent_limit_a: current
                    .and_then(|limits| limits.permanent_limit)
                    .unwrap_or(0.0),
                current_permanent_limit_name: current_name,
                has_current_permanent_limit: current
                    .is_some_and(|limits| limits.permanent_limit.is_some()),
                has_current_permanent_limit_name: has_current_name,
                current_temporary_limit_count: current
                    .map_or(0, |limits| limits.temporary_limits.len()),
                has_active_power_limits: active.is_some(),
                active_power_permanent_limit_mw: active
                    .and_then(|limits| limits.permanent_limit)
                    .unwrap_or(0.0),
                active_power_permanent_limit_name: active_name,
                has_active_power_permanent_limit: active
                    .is_some_and(|limits| limits.permanent_limit.is_some()),
                has_active_power_permanent_limit_name: has_active_name,
                active_power_temporary_limit_count: active
                    .map_or(0, |limits| limits.temporary_limits.len()),
                has_apparent_power_limits: apparent.is_some(),
                apparent_power_permanent_limit_mva: apparent
                    .and_then(|limits| limits.permanent_limit)
                    .unwrap_or(0.0),
                apparent_power_permanent_limit_name: apparent_name,
                has_apparent_power_permanent_limit: apparent
                    .is_some_and(|limits| limits.permanent_limit.is_some()),
                has_apparent_power_permanent_limit_name: has_apparent_name,
                apparent_power_temporary_limit_count: apparent
                    .map_or(0, |limits| limits.temporary_limits.len()),
            };
            Ok(true)
        })
    }
}

/// Read one string property from an operational limit group.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_operational_limit_group_property_at(
    details: *const PioDetailedConnectivity,
    group_index: usize,
    property_index: usize,
    output: *mut PioStringPropertyView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let group = details
                .operational_limit_groups
                .get(group_index)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!("operational limit group index {group_index} is out of range"),
                    )
                })?;
            *require_output(output, "output")? =
                string_property_view(&group.properties, property_index)?;
            Ok(true)
        })
    }
}

/// Read one temporary current, active power, or apparent power limit.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_temporary_limit_at(
    details: *const PioDetailedConnectivity,
    group_index: usize,
    quantity: *const c_char,
    quantity_len: usize,
    limit_index: usize,
    output: *mut PioTemporaryLimitView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let group = details
                .operational_limit_groups
                .get(group_index)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!("operational limit group index {group_index} is out of range"),
                    )
                })?;
            let quantity = required_str(quantity, quantity_len, "quantity")?;
            let limits = match quantity {
                "current" => group.current_limits.as_ref(),
                "active_power" => group.active_power_limits.as_ref(),
                "apparent_power" => group.apparent_power_limits.as_ref(),
                _ => {
                    return Err(boundary_error(
                        &codes::REQUEST_CAPI_QUANTITY_UNKNOWN,
                        "quantity must be current, active_power, or apparent_power",
                    ));
                }
            }
            .ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("operational limit group {group_index} has no {quantity} limits"),
                )
            })?;
            let limit = limits.temporary_limits.get(limit_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("temporary limit index {limit_index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = PioTemporaryLimitView {
                name: PioStringView::new(&limit.name),
                value: limit.value,
                acceptable_duration_seconds: limit.acceptable_duration_seconds,
                fictitious: limit.fictitious,
            };
            Ok(true)
        })
    }
}

/// Read one PowSybl boundary line by zero based table position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_boundary_line_at(
    details: *const PioDetailedConnectivity,
    index: usize,
    output: *mut PioBoundaryLineView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let line = details.boundary_lines.get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("boundary line index {index} is out of range"),
                )
            })?;
            let (pairing_key, has_pairing_key) = optional_string_view(line.pairing_key.as_deref());
            let (generation, has_generation) =
                boundary_line_generation_view(line.generation.as_ref());
            let (calculation_load, has_calculation_load) =
                optional_component_id_view(line.calculation_load.as_ref());
            let (calculation_generator, has_calculation_generator) =
                optional_component_id_view(line.calculation_generator.as_ref());
            *require_output(output, "output")? = PioBoundaryLineView {
                component: component_id_view(&line.component),
                voltage_level: component_id_view(&line.voltage_level),
                active_power_setpoint_mw: line.active_power_setpoint_mw,
                reactive_power_setpoint_mvar: line.reactive_power_setpoint_mvar,
                resistance_ohm: line.resistance_ohm,
                reactance_ohm: line.reactance_ohm,
                conductance_siemens: line.conductance_siemens,
                susceptance_siemens: line.susceptance_siemens,
                pairing_key,
                has_pairing_key,
                generation,
                has_generation,
                calculation_load,
                has_calculation_load,
                calculation_generator,
                has_calculation_generator,
            };
            Ok(true)
        })
    }
}

/// Read one property on a boundary line generation reactive limit record.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_boundary_line_reactive_limit_property_at(
    details: *const PioDetailedConnectivity,
    boundary_line_index: usize,
    property_index: usize,
    output: *mut PioStringPropertyView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let line = details
                .boundary_lines
                .get(boundary_line_index)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!("boundary line index {boundary_line_index} is out of range"),
                    )
                })?;
            let limits = line
                .generation
                .as_ref()
                .and_then(|generation| generation.reactive_limits.as_ref())
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!(
                            "boundary line {boundary_line_index} has no generation reactive limits"
                        ),
                    )
                })?;
            let properties = match limits {
                powerio_tx::ReactiveLimits::MinMax(limits) => &limits.properties,
                powerio_tx::ReactiveLimits::CapabilityCurve(curve) => &curve.properties,
                _ => {
                    return Err(boundary_error(
                        &codes::REQUEST_CAPI_TYPE_MISMATCH,
                        "unsupported reactive limit record",
                    ));
                }
            };
            let (name, value) = properties.iter().nth(property_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("reactive limit property index {property_index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = PioStringPropertyView {
                name: PioStringView::new(name),
                value: PioStringView::new(value),
            };
            Ok(true)
        })
    }
}

/// Read one point from a boundary line generation reactive capability curve.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_boundary_line_reactive_capability_point_at(
    details: *const PioDetailedConnectivity,
    boundary_line_index: usize,
    point_index: usize,
    output: *mut PioReactiveCapabilityCurvePointView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let line = details
                .boundary_lines
                .get(boundary_line_index)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!("boundary line index {boundary_line_index} is out of range"),
                    )
                })?;
            let curve = line
                .generation
                .as_ref()
                .and_then(|generation| generation.reactive_limits.as_ref())
                .and_then(|limits| match limits {
                    powerio_tx::ReactiveLimits::CapabilityCurve(curve) => Some(curve),
                    _ => None,
                })
                .ok_or_else(|| {
                    boundary_error(
                        &codes::REQUEST_CAPI_TYPE_MISMATCH,
                        format!(
                            "boundary line {boundary_line_index} has no generation reactive capability curve"
                        ),
                    )
                })?;
            let point = curve.points.get(point_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("reactive capability point index {point_index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = PioReactiveCapabilityCurvePointView {
                active_power_mw: point.active_power_mw,
                minimum_reactive_power_mvar: point.minimum_reactive_power_mvar,
                maximum_reactive_power_mvar: point.maximum_reactive_power_mvar,
                property_count: point.properties.len(),
            };
            Ok(true)
        })
    }
}

/// Read one property from one boundary line reactive capability curve point.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_boundary_line_reactive_capability_point_property_at(
    details: *const PioDetailedConnectivity,
    boundary_line_index: usize,
    point_index: usize,
    property_index: usize,
    output: *mut PioStringPropertyView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let line = details
                .boundary_lines
                .get(boundary_line_index)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!("boundary line index {boundary_line_index} is out of range"),
                    )
                })?;
            let curve = line
                .generation
                .as_ref()
                .and_then(|generation| generation.reactive_limits.as_ref())
                .and_then(|limits| match limits {
                    powerio_tx::ReactiveLimits::CapabilityCurve(curve) => Some(curve),
                    _ => None,
                })
                .ok_or_else(|| {
                    boundary_error(
                        &codes::REQUEST_CAPI_TYPE_MISMATCH,
                        format!(
                            "boundary line {boundary_line_index} has no generation reactive capability curve"
                        ),
                    )
                })?;
            let point = curve.points.get(point_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("reactive capability point index {point_index} is out of range"),
                )
            })?;
            let (name, value) = point.properties.iter().nth(property_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!(
                        "reactive capability point property index {property_index} is out of range"
                    ),
                )
            })?;
            *require_output(output, "output")? = PioStringPropertyView {
                name: PioStringView::new(name),
                value: PioStringView::new(value),
            };
            Ok(true)
        })
    }
}

/// Read one PowSybl tie line by zero based table position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_tie_line_at(
    details: *const PioDetailedConnectivity,
    index: usize,
    output: *mut PioTieLineView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let tie = details.tie_lines.get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("tie line index {index} is out of range"),
                )
            })?;
            let (calculation_branch, has_calculation_branch) =
                optional_component_id_view(tie.calculation_branch.as_ref());
            *require_output(output, "output")? = PioTieLineView {
                component: component_id_view(&tie.component),
                boundary_line1: component_id_view(&tie.boundary_line1),
                boundary_line2: component_id_view(&tie.boundary_line2),
                calculation_branch,
                has_calculation_branch,
            };
            Ok(true)
        })
    }
}

/// Read one transformer tap changer by zero based table position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_tap_changer_at(
    details: *const PioDetailedConnectivity,
    index: usize,
    output: *mut PioTapChangerView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let changer = details.tap_changers.get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("tap changer index {index} is out of range"),
                )
            })?;
            let (regulation_mode, has_regulation_mode) =
                changer
                    .regulation_mode
                    .map_or((PioStringView::EMPTY, false), |mode| {
                        (
                            PioStringView::new(tap_changer_regulation_mode_name(mode)),
                            true,
                        )
                    });
            let (regulation_terminal, has_regulation_terminal) =
                terminal_reference_view(changer.regulation_terminal.as_ref());
            let (component, has_component) = optional_component_id_view(changer.component.as_ref());
            *require_output(output, "output")? = PioTapChangerView {
                component,
                has_component,
                transformer: component_id_view(&changer.transformer),
                winding: changer.winding,
                kind: PioStringView::new(tap_changer_kind_name(changer.kind)),
                tap_position: changer.tap_position.unwrap_or(0),
                has_tap_position: changer.tap_position.is_some(),
                solved_tap_position: changer.solved_tap_position.unwrap_or(0),
                has_solved_tap_position: changer.solved_tap_position.is_some(),
                low_tap_position: changer.low_tap_position,
                neutral_tap_position: changer.neutral_tap_position.unwrap_or(0),
                has_neutral_tap_position: changer.neutral_tap_position.is_some(),
                normal_tap_position: changer.normal_tap_position.unwrap_or(0),
                has_normal_tap_position: changer.normal_tap_position.is_some(),
                voltage_step_increment_percent: changer
                    .voltage_step_increment_percent
                    .unwrap_or(0.0),
                has_voltage_step_increment_percent: changer
                    .voltage_step_increment_percent
                    .is_some(),
                load_tap_changing_capabilities: changer.load_tap_changing_capabilities,
                regulating: changer.regulating,
                regulation_mode,
                has_regulation_mode,
                regulation_value: changer.regulation_value.unwrap_or(0.0),
                has_regulation_value: changer.regulation_value.is_some(),
                target_deadband: changer.target_deadband.unwrap_or(0.0),
                has_target_deadband: changer.target_deadband.is_some(),
                regulation_terminal,
                has_regulation_terminal,
                step_count: changer.steps.len(),
            };
            Ok(true)
        })
    }
}

/// Read one transformer tap changer step by zero based position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_tap_changer_step_at(
    details: *const PioDetailedConnectivity,
    tap_changer_index: usize,
    step_index: usize,
    output: *mut PioTapChangerStepView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let changer = details.tap_changers.get(tap_changer_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("tap changer index {tap_changer_index} is out of range"),
                )
            })?;
            let step = changer.steps.get(step_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("tap changer step index {step_index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = PioTapChangerStepView {
                position: step.position,
                ratio_pu: step.rho,
                phase_shift_degrees: step.alpha_degrees,
                resistance_deviation_percent: step.resistance_deviation_percent,
                reactance_deviation_percent: step.reactance_deviation_percent,
                conductance_deviation_percent: step.conductance_deviation_percent,
                susceptance_deviation_percent: step.susceptance_deviation_percent,
            };
            Ok(true)
        })
    }
}

/// Read reactive limits retained for one equipment record.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_equipment_reactive_limits_at(
    details: *const PioDetailedConnectivity,
    index: usize,
    output: *mut PioEquipmentReactiveLimitsView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let record = details
                .equipment_reactive_limits
                .get(index)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!("equipment reactive limits index {index} is out of range"),
                    )
                })?;
            *require_output(output, "output")? = PioEquipmentReactiveLimitsView {
                equipment: component_id_view(&record.equipment),
                limits: reactive_limits_view(Some(&record.limits)).0,
            };
            Ok(true)
        })
    }
}

/// Read one property from an equipment reactive limit record.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_equipment_reactive_limit_property_at(
    details: *const PioDetailedConnectivity,
    equipment_index: usize,
    property_index: usize,
    output: *mut PioStringPropertyView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let record = details
                .equipment_reactive_limits
                .get(equipment_index)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!(
                            "equipment reactive limits index {equipment_index} is out of range"
                        ),
                    )
                })?;
            *require_output(output, "output")? =
                string_property_view(reactive_limit_properties(&record.limits), property_index)?;
            Ok(true)
        })
    }
}

/// Read one point from an equipment reactive capability curve.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_equipment_reactive_capability_point_at(
    details: *const PioDetailedConnectivity,
    equipment_index: usize,
    point_index: usize,
    output: *mut PioReactiveCapabilityCurvePointView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let record = details
                .equipment_reactive_limits
                .get(equipment_index)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!(
                            "equipment reactive limits index {equipment_index} is out of range"
                        ),
                    )
                })?;
            let curve = reactive_capability_curve(&record.limits).ok_or_else(|| {
                boundary_error(
                    &codes::REQUEST_CAPI_TYPE_MISMATCH,
                    format!(
                        "equipment reactive limits {equipment_index} is not a capability curve"
                    ),
                )
            })?;
            *require_output(output, "output")? =
                reactive_capability_point_view(curve, point_index)?;
            Ok(true)
        })
    }
}

/// Read one property from an equipment reactive capability curve point.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_equipment_reactive_capability_point_property_at(
    details: *const PioDetailedConnectivity,
    equipment_index: usize,
    point_index: usize,
    property_index: usize,
    output: *mut PioStringPropertyView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let record = details
                .equipment_reactive_limits
                .get(equipment_index)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!(
                            "equipment reactive limits index {equipment_index} is out of range"
                        ),
                    )
                })?;
            let curve = reactive_capability_curve(&record.limits).ok_or_else(|| {
                boundary_error(
                    &codes::REQUEST_CAPI_TYPE_MISMATCH,
                    format!(
                        "equipment reactive limits {equipment_index} is not a capability curve"
                    ),
                )
            })?;
            let point = curve.points.get(point_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("reactive capability point index {point_index} is out of range"),
                )
            })?;
            *require_output(output, "output")? =
                string_property_view(&point.properties, property_index)?;
            Ok(true)
        })
    }
}

/// Read one DC converter unit by zero based table position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_dc_converter_unit_at(
    details: *const PioDetailedConnectivity,
    index: usize,
    output: *mut PioDcConverterUnitView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let unit = details.dc_converter_units.get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("DC converter unit index {index} is out of range"),
                )
            })?;
            let (substation, has_substation) = optional_component_id_view(unit.substation.as_ref());
            *require_output(output, "output")? = PioDcConverterUnitView {
                component: component_id_view(&unit.component),
                substation,
                has_substation,
                operation_mode: PioStringView::new(dc_converter_operation_mode_name(
                    unit.operation_mode,
                )),
            };
            Ok(true)
        })
    }
}

/// Read one DC topological node by zero based table position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_dc_topological_node_at(
    details: *const PioDetailedConnectivity,
    index: usize,
    output: *mut PioDcNodeView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let node = details.dc_topological_nodes.get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("DC topological node index {index} is out of range"),
                )
            })?;
            let (dc_converter_unit, has_dc_converter_unit) =
                optional_component_id_view(node.dc_converter_unit.as_ref());
            *require_output(output, "output")? = PioDcNodeView {
                component: component_id_view(&node.component),
                kind: PioStringView::new("topological_node"),
                nominal_voltage_kv: 0.0,
                has_nominal_voltage: false,
                voltage_kv: 0.0,
                has_voltage: false,
                dc_converter_unit,
                has_dc_converter_unit,
                dc_topological_node: empty_component_id_view(),
                has_dc_topological_node: false,
            };
            Ok(true)
        })
    }
}

/// Read one physical DC node by zero based table position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_dc_node_at(
    details: *const PioDetailedConnectivity,
    index: usize,
    output: *mut PioDcNodeView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let node = details.dc_nodes.get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("DC node index {index} is out of range"),
                )
            })?;
            let (dc_converter_unit, has_dc_converter_unit) =
                optional_component_id_view(node.dc_converter_unit.as_ref());
            let (dc_topological_node, has_dc_topological_node) =
                optional_component_id_view(node.dc_topological_node.as_ref());
            *require_output(output, "output")? = PioDcNodeView {
                component: component_id_view(&node.component),
                kind: PioStringView::new("node"),
                nominal_voltage_kv: node.nominal_voltage_kv.unwrap_or(0.0),
                has_nominal_voltage: node.nominal_voltage_kv.is_some(),
                voltage_kv: node.voltage_kv.unwrap_or(0.0),
                has_voltage: node.voltage_kv.is_some(),
                dc_converter_unit,
                has_dc_converter_unit,
                dc_topological_node,
                has_dc_topological_node,
            };
            Ok(true)
        })
    }
}

/// Read one DC ground by zero based table position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_dc_ground_at(
    details: *const PioDetailedConnectivity,
    index: usize,
    output: *mut PioDcEquipmentView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let record = details.dc_grounds.get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("DC ground index {index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = dc_equipment_view(
                &record.component,
                record.equipment_container.as_ref(),
                "ground",
                &record.dc_terminal,
                None,
                record.rated_dc_voltage_kv,
                record.resistance_ohm,
                record.inductance_h,
                None,
                None,
                None,
                None,
            );
            Ok(true)
        })
    }
}

/// Read one DC busbar by zero based table position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_dc_busbar_at(
    details: *const PioDetailedConnectivity,
    index: usize,
    output: *mut PioDcEquipmentView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let record = details.dc_busbars.get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("DC busbar index {index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = dc_equipment_view(
                &record.component,
                record.equipment_container.as_ref(),
                "busbar",
                &record.dc_terminal,
                None,
                record.rated_dc_voltage_kv,
                None,
                None,
                None,
                None,
                None,
                None,
            );
            Ok(true)
        })
    }
}

/// Read one DC line by zero based table position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_dc_line_at(
    details: *const PioDetailedConnectivity,
    index: usize,
    output: *mut PioDcEquipmentView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let record = details.dc_lines.get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("DC line index {index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = dc_equipment_view(
                &record.component,
                record.equipment_container.as_ref(),
                "line",
                &record.dc_terminal1,
                Some(&record.dc_terminal2),
                record.rated_dc_voltage_kv,
                record.resistance_ohm,
                record.inductance_h,
                record.capacitance_f,
                record.length_km,
                None,
                None,
            );
            Ok(true)
        })
    }
}

/// Read one DC series device by zero based table position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_dc_series_device_at(
    details: *const PioDetailedConnectivity,
    index: usize,
    output: *mut PioDcEquipmentView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let record = details.dc_series_devices.get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("DC series device index {index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = dc_equipment_view(
                &record.component,
                record.equipment_container.as_ref(),
                "series_device",
                &record.dc_terminal1,
                Some(&record.dc_terminal2),
                record.rated_dc_voltage_kv,
                record.resistance_ohm,
                record.inductance_h,
                None,
                None,
                None,
                None,
            );
            Ok(true)
        })
    }
}

/// Read one DC switch by zero based table position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_dc_switch_at(
    details: *const PioDetailedConnectivity,
    index: usize,
    output: *mut PioDcEquipmentView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let record = details.dc_switches.get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("DC switch index {index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = dc_equipment_view(
                &record.component,
                record.equipment_container.as_ref(),
                "switch",
                &record.dc_terminal1,
                Some(&record.dc_terminal2),
                record.rated_dc_voltage_kv,
                record.resistance_ohm,
                None,
                None,
                None,
                Some(record.kind),
                record.open,
            );
            Ok(true)
        })
    }
}

/// Read one voltage source converter by zero based table position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_voltage_source_converter_at(
    details: *const PioDetailedConnectivity,
    index: usize,
    output: *mut PioAcDcConverterView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let converter = details
                .voltage_source_converters
                .get(index)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!("voltage source converter index {index} is out of range"),
                    )
                })?;
            let (dc_converter_unit, has_dc_converter_unit) =
                optional_component_id_view(converter.dc_converter_unit.as_ref());
            let (control_mode, has_control_mode) =
                converter
                    .control_mode
                    .map_or((PioStringView::EMPTY, false), |mode| {
                        (
                            PioStringView::new(ac_dc_converter_control_mode_name(mode)),
                            true,
                        )
                    });
            let (pcc_terminal, has_pcc_terminal) =
                terminal_reference_view(converter.pcc_terminal.as_ref());
            let (reactive_limits, has_reactive_limits) =
                reactive_limits_view(converter.reactive_limits.as_ref());
            *require_output(output, "output")? = PioAcDcConverterView {
                component: component_id_view(&converter.component),
                kind: PioStringView::new("voltage_source"),
                dc_converter_unit,
                has_dc_converter_unit,
                dc_terminal1: dc_terminal_view(&converter.dc_terminal1),
                dc_terminal2: dc_terminal_view(&converter.dc_terminal2),
                base_apparent_power_mva: converter.base_apparent_power_mva.unwrap_or(0.0),
                has_base_apparent_power: converter.base_apparent_power_mva.is_some(),
                minimum_active_power_mw: converter.minimum_active_power_mw.unwrap_or(0.0),
                has_minimum_active_power: converter.minimum_active_power_mw.is_some(),
                maximum_active_power_mw: converter.maximum_active_power_mw.unwrap_or(0.0),
                has_maximum_active_power: converter.maximum_active_power_mw.is_some(),
                minimum_dc_voltage_kv: converter.minimum_dc_voltage_kv.unwrap_or(0.0),
                has_minimum_dc_voltage: converter.minimum_dc_voltage_kv.is_some(),
                maximum_dc_voltage_kv: converter.maximum_dc_voltage_kv.unwrap_or(0.0),
                has_maximum_dc_voltage: converter.maximum_dc_voltage_kv.is_some(),
                rated_dc_voltage_kv: converter.rated_dc_voltage_kv.unwrap_or(0.0),
                has_rated_dc_voltage: converter.rated_dc_voltage_kv.is_some(),
                valve_u0_kv: converter.valve_u0_kv.unwrap_or(0.0),
                has_valve_u0: converter.valve_u0_kv.is_some(),
                number_of_valves: converter.number_of_valves.unwrap_or(0),
                has_number_of_valves: converter.number_of_valves.is_some(),
                idle_loss_mw: converter.idle_loss_mw.unwrap_or(0.0),
                has_idle_loss: converter.idle_loss_mw.is_some(),
                switching_loss_mw_per_ampere: converter.switching_loss_mw_per_ampere.unwrap_or(0.0),
                has_switching_loss: converter.switching_loss_mw_per_ampere.is_some(),
                resistive_loss_ohm: converter.resistive_loss_ohm.unwrap_or(0.0),
                has_resistive_loss: converter.resistive_loss_ohm.is_some(),
                control_mode,
                has_control_mode,
                active_power_at_pcc_mw: converter.active_power_at_pcc_mw.unwrap_or(0.0),
                has_active_power_at_pcc: converter.active_power_at_pcc_mw.is_some(),
                reactive_power_at_pcc_mvar: converter.reactive_power_at_pcc_mvar.unwrap_or(0.0),
                has_reactive_power_at_pcc: converter.reactive_power_at_pcc_mvar.is_some(),
                target_active_power_mw: converter.target_active_power_mw.unwrap_or(0.0),
                has_target_active_power: converter.target_active_power_mw.is_some(),
                target_dc_voltage_kv: converter.target_dc_voltage_kv.unwrap_or(0.0),
                has_target_dc_voltage: converter.target_dc_voltage_kv.is_some(),
                pcc_terminal,
                has_pcc_terminal,
                droop_curve_segment_count: converter
                    .droop_curve
                    .as_ref()
                    .map_or(0, |curve| curve.segments.len()),
                has_droop_curve: converter.droop_curve.is_some(),
                droop: converter.droop.unwrap_or(0.0),
                has_droop: converter.droop.is_some(),
                droop_compensation: converter.droop_compensation.unwrap_or(0.0),
                has_droop_compensation: converter.droop_compensation.is_some(),
                q_share: converter.q_share.unwrap_or(0.0),
                has_q_share: converter.q_share.is_some(),
                maximum_modulation_index: converter.maximum_modulation_index.unwrap_or(0.0),
                has_maximum_modulation_index: converter.maximum_modulation_index.is_some(),
                maximum_valve_current_a: converter.maximum_valve_current_a.unwrap_or(0.0),
                has_maximum_valve_current: converter.maximum_valve_current_a.is_some(),
                dc_current_a: converter.dc_current_a.unwrap_or(0.0),
                has_dc_current: converter.dc_current_a.is_some(),
                ac_voltage_kv: converter.ac_voltage_kv.unwrap_or(0.0),
                has_ac_voltage: converter.ac_voltage_kv.is_some(),
                dc_voltage_kv: converter.dc_voltage_kv.unwrap_or(0.0),
                has_dc_voltage: converter.dc_voltage_kv.is_some(),
                voltage_regulator_on: converter.voltage_regulator_on.unwrap_or(false),
                has_voltage_regulator_on: converter.voltage_regulator_on.is_some(),
                voltage_setpoint_kv: converter.voltage_setpoint_kv.unwrap_or(0.0),
                has_voltage_setpoint: converter.voltage_setpoint_kv.is_some(),
                reactive_power_setpoint_mvar: converter.reactive_power_setpoint_mvar.unwrap_or(0.0),
                has_reactive_power_setpoint: converter.reactive_power_setpoint_mvar.is_some(),
                reactive_limits,
                has_reactive_limits,
                pole_loss_active_power_mw: converter.pole_loss_active_power_mw.unwrap_or(0.0),
                has_pole_loss_active_power: converter.pole_loss_active_power_mw.is_some(),
                reactive_model: PioStringView::EMPTY,
                has_reactive_model: false,
                power_factor: 0.0,
                has_power_factor: false,
                operating_mode: PioStringView::EMPTY,
                has_operating_mode: false,
                rated_dc_current_a: 0.0,
                has_rated_dc_current: false,
                minimum_alpha_degrees: 0.0,
                has_minimum_alpha: false,
                maximum_alpha_degrees: 0.0,
                has_maximum_alpha: false,
                minimum_gamma_degrees: 0.0,
                has_minimum_gamma: false,
                maximum_gamma_degrees: 0.0,
                has_maximum_gamma: false,
                target_alpha_degrees: 0.0,
                has_target_alpha: false,
                target_gamma_degrees: 0.0,
                has_target_gamma: false,
                target_dc_current_a: 0.0,
                has_target_dc_current: false,
                alpha_degrees: 0.0,
                has_alpha: false,
                gamma_degrees: 0.0,
                has_gamma: false,
                delta_degrees: converter.delta_degrees.unwrap_or(0.0),
                has_delta: converter.delta_degrees.is_some(),
                uf_kv: converter.uf_kv.unwrap_or(0.0),
                has_uf: converter.uf_kv.is_some(),
                uv_kv: converter.uv_kv.unwrap_or(0.0),
                has_uv: converter.uv_kv.is_some(),
            };
            Ok(true)
        })
    }
}

/// Read one line commutated converter by zero based table position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_line_commutated_converter_at(
    details: *const PioDetailedConnectivity,
    index: usize,
    output: *mut PioAcDcConverterView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let converter = details
                .line_commutated_converters
                .get(index)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!("line commutated converter index {index} is out of range"),
                    )
                })?;
            let (dc_converter_unit, has_dc_converter_unit) =
                optional_component_id_view(converter.dc_converter_unit.as_ref());
            let (control_mode, has_control_mode) =
                converter
                    .control_mode
                    .map_or((PioStringView::EMPTY, false), |mode| {
                        (
                            PioStringView::new(ac_dc_converter_control_mode_name(mode)),
                            true,
                        )
                    });
            let (reactive_model, has_reactive_model) =
                converter
                    .reactive_model
                    .map_or((PioStringView::EMPTY, false), |model| {
                        (
                            PioStringView::new(line_commutated_converter_reactive_model_name(
                                model,
                            )),
                            true,
                        )
                    });
            let (operating_mode, has_operating_mode) =
                converter
                    .operating_mode
                    .map_or((PioStringView::EMPTY, false), |mode| {
                        (
                            PioStringView::new(line_commutated_converter_operating_mode_name(mode)),
                            true,
                        )
                    });
            let (pcc_terminal, has_pcc_terminal) =
                terminal_reference_view(converter.pcc_terminal.as_ref());
            *require_output(output, "output")? = PioAcDcConverterView {
                component: component_id_view(&converter.component),
                kind: PioStringView::new("line_commutated"),
                dc_converter_unit,
                has_dc_converter_unit,
                dc_terminal1: dc_terminal_view(&converter.dc_terminal1),
                dc_terminal2: dc_terminal_view(&converter.dc_terminal2),
                base_apparent_power_mva: converter.base_apparent_power_mva.unwrap_or(0.0),
                has_base_apparent_power: converter.base_apparent_power_mva.is_some(),
                minimum_active_power_mw: converter.minimum_active_power_mw.unwrap_or(0.0),
                has_minimum_active_power: converter.minimum_active_power_mw.is_some(),
                maximum_active_power_mw: converter.maximum_active_power_mw.unwrap_or(0.0),
                has_maximum_active_power: converter.maximum_active_power_mw.is_some(),
                minimum_dc_voltage_kv: converter.minimum_dc_voltage_kv.unwrap_or(0.0),
                has_minimum_dc_voltage: converter.minimum_dc_voltage_kv.is_some(),
                maximum_dc_voltage_kv: converter.maximum_dc_voltage_kv.unwrap_or(0.0),
                has_maximum_dc_voltage: converter.maximum_dc_voltage_kv.is_some(),
                rated_dc_voltage_kv: converter.rated_dc_voltage_kv.unwrap_or(0.0),
                has_rated_dc_voltage: converter.rated_dc_voltage_kv.is_some(),
                valve_u0_kv: converter.valve_u0_kv.unwrap_or(0.0),
                has_valve_u0: converter.valve_u0_kv.is_some(),
                number_of_valves: converter.number_of_valves.unwrap_or(0),
                has_number_of_valves: converter.number_of_valves.is_some(),
                idle_loss_mw: converter.idle_loss_mw.unwrap_or(0.0),
                has_idle_loss: converter.idle_loss_mw.is_some(),
                switching_loss_mw_per_ampere: converter.switching_loss_mw_per_ampere.unwrap_or(0.0),
                has_switching_loss: converter.switching_loss_mw_per_ampere.is_some(),
                resistive_loss_ohm: converter.resistive_loss_ohm.unwrap_or(0.0),
                has_resistive_loss: converter.resistive_loss_ohm.is_some(),
                control_mode,
                has_control_mode,
                active_power_at_pcc_mw: converter.active_power_at_pcc_mw.unwrap_or(0.0),
                has_active_power_at_pcc: converter.active_power_at_pcc_mw.is_some(),
                reactive_power_at_pcc_mvar: converter.reactive_power_at_pcc_mvar.unwrap_or(0.0),
                has_reactive_power_at_pcc: converter.reactive_power_at_pcc_mvar.is_some(),
                target_active_power_mw: converter.target_active_power_mw.unwrap_or(0.0),
                has_target_active_power: converter.target_active_power_mw.is_some(),
                target_dc_voltage_kv: converter.target_dc_voltage_kv.unwrap_or(0.0),
                has_target_dc_voltage: converter.target_dc_voltage_kv.is_some(),
                pcc_terminal,
                has_pcc_terminal,
                droop_curve_segment_count: converter
                    .droop_curve
                    .as_ref()
                    .map_or(0, |curve| curve.segments.len()),
                has_droop_curve: converter.droop_curve.is_some(),
                droop: 0.0,
                has_droop: false,
                droop_compensation: 0.0,
                has_droop_compensation: false,
                q_share: 0.0,
                has_q_share: false,
                maximum_modulation_index: 0.0,
                has_maximum_modulation_index: false,
                maximum_valve_current_a: 0.0,
                has_maximum_valve_current: false,
                dc_current_a: converter.dc_current_a.unwrap_or(0.0),
                has_dc_current: converter.dc_current_a.is_some(),
                ac_voltage_kv: converter.ac_voltage_kv.unwrap_or(0.0),
                has_ac_voltage: converter.ac_voltage_kv.is_some(),
                dc_voltage_kv: converter.dc_voltage_kv.unwrap_or(0.0),
                has_dc_voltage: converter.dc_voltage_kv.is_some(),
                voltage_regulator_on: false,
                has_voltage_regulator_on: false,
                voltage_setpoint_kv: 0.0,
                has_voltage_setpoint: false,
                reactive_power_setpoint_mvar: 0.0,
                has_reactive_power_setpoint: false,
                reactive_limits: empty_reactive_limits_view(),
                has_reactive_limits: false,
                pole_loss_active_power_mw: converter.pole_loss_active_power_mw.unwrap_or(0.0),
                has_pole_loss_active_power: converter.pole_loss_active_power_mw.is_some(),
                reactive_model,
                has_reactive_model,
                power_factor: converter.power_factor.unwrap_or(0.0),
                has_power_factor: converter.power_factor.is_some(),
                operating_mode,
                has_operating_mode,
                rated_dc_current_a: converter.rated_dc_current_a.unwrap_or(0.0),
                has_rated_dc_current: converter.rated_dc_current_a.is_some(),
                minimum_alpha_degrees: converter.minimum_alpha_degrees.unwrap_or(0.0),
                has_minimum_alpha: converter.minimum_alpha_degrees.is_some(),
                maximum_alpha_degrees: converter.maximum_alpha_degrees.unwrap_or(0.0),
                has_maximum_alpha: converter.maximum_alpha_degrees.is_some(),
                minimum_gamma_degrees: converter.minimum_gamma_degrees.unwrap_or(0.0),
                has_minimum_gamma: converter.minimum_gamma_degrees.is_some(),
                maximum_gamma_degrees: converter.maximum_gamma_degrees.unwrap_or(0.0),
                has_maximum_gamma: converter.maximum_gamma_degrees.is_some(),
                target_alpha_degrees: converter.target_alpha_degrees.unwrap_or(0.0),
                has_target_alpha: converter.target_alpha_degrees.is_some(),
                target_gamma_degrees: converter.target_gamma_degrees.unwrap_or(0.0),
                has_target_gamma: converter.target_gamma_degrees.is_some(),
                target_dc_current_a: converter.target_dc_current_a.unwrap_or(0.0),
                has_target_dc_current: converter.target_dc_current_a.is_some(),
                alpha_degrees: converter.alpha_degrees.unwrap_or(0.0),
                has_alpha: converter.alpha_degrees.is_some(),
                gamma_degrees: converter.gamma_degrees.unwrap_or(0.0),
                has_gamma: converter.gamma_degrees.is_some(),
                delta_degrees: 0.0,
                has_delta: false,
                uf_kv: 0.0,
                has_uf: false,
                uv_kv: 0.0,
                has_uv: false,
            };
            Ok(true)
        })
    }
}

fn droop_curve_segment_view(segment: &powerio_tx::DroopCurveSegment) -> PioDroopCurveSegmentView {
    PioDroopCurveSegmentView {
        minimum_voltage_kv: segment.minimum_voltage_kv,
        maximum_voltage_kv: segment.maximum_voltage_kv,
        k: segment.k,
    }
}

/// Read one DC voltage droop curve segment from a voltage source converter.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_voltage_source_converter_droop_curve_segment_at(
    details: *const PioDetailedConnectivity,
    converter_index: usize,
    segment_index: usize,
    output: *mut PioDroopCurveSegmentView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let converter = details
                .voltage_source_converters
                .get(converter_index)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!("voltage source converter index {converter_index} is out of range"),
                    )
                })?;
            let curve = converter.droop_curve.as_ref().ok_or_else(|| {
                boundary_error(
                    &codes::REQUEST_CAPI_TYPE_MISMATCH,
                    format!("voltage source converter {converter_index} has no droop curve"),
                )
            })?;
            let segment = curve.segments.get(segment_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("droop curve segment index {segment_index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = droop_curve_segment_view(segment);
            Ok(true)
        })
    }
}

/// Read one DC voltage droop curve segment from a line commutated converter.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_line_commutated_converter_droop_curve_segment_at(
    details: *const PioDetailedConnectivity,
    converter_index: usize,
    segment_index: usize,
    output: *mut PioDroopCurveSegmentView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let converter = details
                .line_commutated_converters
                .get(converter_index)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!(
                            "line commutated converter index {converter_index} is out of range"
                        ),
                    )
                })?;
            let curve = converter.droop_curve.as_ref().ok_or_else(|| {
                boundary_error(
                    &codes::REQUEST_CAPI_TYPE_MISMATCH,
                    format!("line commutated converter {converter_index} has no droop curve"),
                )
            })?;
            let segment = curve.segments.get(segment_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("droop curve segment index {segment_index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = droop_curve_segment_view(segment);
            Ok(true)
        })
    }
}

/// Read one property from a voltage source converter reactive limit record.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_voltage_source_converter_reactive_limit_property_at(
    details: *const PioDetailedConnectivity,
    converter_index: usize,
    property_index: usize,
    output: *mut PioStringPropertyView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let converter = details
                .voltage_source_converters
                .get(converter_index)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!("voltage source converter index {converter_index} is out of range"),
                    )
                })?;
            let limits = converter.reactive_limits.as_ref().ok_or_else(|| {
                boundary_error(
                    &codes::REQUEST_CAPI_TYPE_MISMATCH,
                    format!("voltage source converter {converter_index} has no reactive limits"),
                )
            })?;
            *require_output(output, "output")? =
                string_property_view(reactive_limit_properties(limits), property_index)?;
            Ok(true)
        })
    }
}

/// Read one point from a voltage source converter reactive capability curve.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_voltage_source_converter_reactive_capability_point_at(
    details: *const PioDetailedConnectivity,
    converter_index: usize,
    point_index: usize,
    output: *mut PioReactiveCapabilityCurvePointView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let converter = details
                .voltage_source_converters
                .get(converter_index)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!("voltage source converter index {converter_index} is out of range"),
                    )
                })?;
            let curve = converter
                .reactive_limits
                .as_ref()
                .and_then(reactive_capability_curve)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::REQUEST_CAPI_TYPE_MISMATCH,
                        format!(
                            "voltage source converter {converter_index} has no reactive capability curve"
                        ),
                    )
                })?;
            *require_output(output, "output")? =
                reactive_capability_point_view(curve, point_index)?;
            Ok(true)
        })
    }
}

/// Read one property from a voltage source converter reactive capability point.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_voltage_source_converter_reactive_capability_point_property_at(
    details: *const PioDetailedConnectivity,
    converter_index: usize,
    point_index: usize,
    property_index: usize,
    output: *mut PioStringPropertyView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let details = require_detailed_connectivity(details)?;
            let converter = details
                .voltage_source_converters
                .get(converter_index)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!("voltage source converter index {converter_index} is out of range"),
                    )
                })?;
            let curve = converter
                .reactive_limits
                .as_ref()
                .and_then(reactive_capability_curve)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::REQUEST_CAPI_TYPE_MISMATCH,
                        format!(
                            "voltage source converter {converter_index} has no reactive capability curve"
                        ),
                    )
                })?;
            let point = curve.points.get(point_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("reactive capability point index {point_index} is out of range"),
                )
            })?;
            *require_output(output, "output")? =
                string_property_view(&point.properties, property_index)?;
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_retain(
    details: *const PioDetailedConnectivity,
) -> *mut PioDetailedConnectivity {
    unsafe { PioDetailedConnectivity::retain_raw(details) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_detailed_connectivity_release(details: *mut PioDetailedConnectivity) {
    unsafe { PioDetailedConnectivity::release_raw(details) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_balanced_network_bus_count(
    network: *const PioBalancedNetwork,
) -> usize {
    unsafe { PioBalancedNetwork::get(network) }
        .and_then(BalancedNetworkInner::network)
        .map_or(0, |network| network.buses().len())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_balanced_network_branch_count(
    network: *const PioBalancedNetwork,
) -> usize {
    unsafe { PioBalancedNetwork::get(network) }
        .and_then(BalancedNetworkInner::network)
        .map_or(0, |network| network.branches().len())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_balanced_network_load_count(
    network: *const PioBalancedNetwork,
) -> usize {
    unsafe { PioBalancedNetwork::get(network) }
        .and_then(BalancedNetworkInner::network)
        .map_or(0, |network| network.loads().len())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_balanced_network_shunt_count(
    network: *const PioBalancedNetwork,
) -> usize {
    unsafe { PioBalancedNetwork::get(network) }
        .and_then(BalancedNetworkInner::network)
        .map_or(0, |network| network.shunts().len())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_balanced_network_static_var_compensator_count(
    network: *const PioBalancedNetwork,
) -> usize {
    unsafe { PioBalancedNetwork::get(network) }
        .and_then(BalancedNetworkInner::network)
        .map_or(0, |network| network.static_var_compensators().len())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_balanced_network_generator_count(
    network: *const PioBalancedNetwork,
) -> usize {
    unsafe { PioBalancedNetwork::get(network) }
        .and_then(BalancedNetworkInner::network)
        .map_or(0, |network| network.generators().len())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_balanced_network_storage_count(
    network: *const PioBalancedNetwork,
) -> usize {
    unsafe { PioBalancedNetwork::get(network) }
        .and_then(BalancedNetworkInner::network)
        .map_or(0, |network| network.storage().len())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_balanced_network_switch_count(
    network: *const PioBalancedNetwork,
) -> usize {
    unsafe { PioBalancedNetwork::get(network) }
        .and_then(BalancedNetworkInner::network)
        .map_or(0, |network| network.switches().len())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_balanced_network_hvdc_count(
    network: *const PioBalancedNetwork,
) -> usize {
    unsafe { PioBalancedNetwork::get(network) }
        .and_then(BalancedNetworkInner::network)
        .map_or(0, |network| network.hvdc().len())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_balanced_network_three_winding_transformer_count(
    network: *const PioBalancedNetwork,
) -> usize {
    unsafe { PioBalancedNetwork::get(network) }
        .and_then(BalancedNetworkInner::network)
        .map_or(0, |network| network.transformers_3w().len())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_balanced_network_area_count(
    network: *const PioBalancedNetwork,
) -> usize {
    unsafe { PioBalancedNetwork::get(network) }
        .and_then(BalancedNetworkInner::network)
        .map_or(0, |network| network.areas().len())
}

unsafe fn require_balanced_network<'a>(
    network: *const PioBalancedNetwork,
) -> Result<&'a BalancedNetwork, *mut PioError> {
    unsafe { PioBalancedNetwork::get(network) }
        .and_then(BalancedNetworkInner::network)
        .ok_or_else(|| {
            boundary_error(
                &codes::BIND_CAPI_NULL_HANDLE,
                "PioBalancedNetwork must not be NULL",
            )
        })
}

unsafe fn require_output<'a, T>(
    output: *mut T,
    argument: &str,
) -> Result<&'a mut T, *mut PioError> {
    unsafe { output.as_mut() }.ok_or_else(|| {
        boundary_error(
            &codes::BIND_CAPI_NULL_ARGUMENT,
            format!("{argument} must not be NULL"),
        )
    })
}

unsafe fn require_detailed_connectivity<'a>(
    details: *const PioDetailedConnectivity,
) -> Result<&'a powerio_tx::DetailedConnectivity, *mut PioError> {
    unsafe { PioDetailedConnectivity::get(details) }
        .and_then(DetailedConnectivityInner::details)
        .ok_or_else(|| {
            boundary_error(
                &codes::BIND_CAPI_NULL_HANDLE,
                "PioDetailedConnectivity must not be NULL",
            )
        })
}

fn optional_string_view(value: Option<&str>) -> (PioStringView, bool) {
    value.map_or((PioStringView::EMPTY, false), |value| {
        (PioStringView::new(value), true)
    })
}

fn balanced_coords_kind_name(kind: powerio_tx::CoordsKind) -> &'static str {
    match kind {
        powerio_tx::CoordsKind::Source => "source",
        powerio_tx::CoordsKind::Synthetic => "synthetic",
        powerio_tx::CoordsKind::Manual => "manual",
        powerio_tx::CoordsKind::Derived => "derived",
        _ => "unknown",
    }
}

fn balanced_location_view(location: &powerio_tx::Location) -> PioBalancedLocationView {
    let (kind, has_kind) = location.kind.map_or((PioStringView::EMPTY, false), |kind| {
        (PioStringView::new(balanced_coords_kind_name(kind)), true)
    });
    PioBalancedLocationView {
        x: location.x,
        y: location.y,
        kind,
        has_kind,
    }
}

fn empty_balanced_location_view() -> PioBalancedLocationView {
    PioBalancedLocationView {
        x: 0.0,
        y: 0.0,
        kind: PioStringView::EMPTY,
        has_kind: false,
    }
}

fn balanced_geo_view(geo: Option<&powerio_tx::GeoMeta>) -> PioBalancedGeoView {
    let Some(geo) = geo else {
        return PioBalancedGeoView {
            has_geo: false,
            space: PioStringView::EMPTY,
            crs: PioStringView::EMPTY,
            has_crs: false,
            kind: PioStringView::EMPTY,
            has_kind: false,
            has_canvas: false,
            canvas_width: 0.0,
            has_canvas_width: false,
            canvas_height: 0.0,
            has_canvas_height: false,
            canvas_units: PioStringView::EMPTY,
            has_canvas_units: false,
        };
    };
    let (space, crs, has_crs, canvas) = match &geo.space {
        powerio_tx::CoordinateSpace::Geographic { crs } => (
            "geographic",
            crs.as_deref()
                .map_or(PioStringView::EMPTY, PioStringView::new),
            crs.is_some(),
            None,
        ),
        powerio_tx::CoordinateSpace::Projected { crs } => (
            "projected",
            crs.as_deref()
                .map_or(PioStringView::EMPTY, PioStringView::new),
            crs.is_some(),
            None,
        ),
        powerio_tx::CoordinateSpace::Diagram { canvas } => {
            ("diagram", PioStringView::EMPTY, false, canvas.as_ref())
        }
        powerio_tx::CoordinateSpace::Unknown => ("unknown", PioStringView::EMPTY, false, None),
        _ => ("unknown", PioStringView::EMPTY, false, None),
    };
    let (kind, has_kind) = geo.kind.map_or((PioStringView::EMPTY, false), |kind| {
        (PioStringView::new(balanced_coords_kind_name(kind)), true)
    });
    PioBalancedGeoView {
        has_geo: true,
        space: PioStringView::new(space),
        crs,
        has_crs,
        kind,
        has_kind,
        has_canvas: canvas.is_some(),
        canvas_width: canvas.and_then(|canvas| canvas.width).unwrap_or(0.0),
        has_canvas_width: canvas.is_some_and(|canvas| canvas.width.is_some()),
        canvas_height: canvas.and_then(|canvas| canvas.height).unwrap_or(0.0),
        has_canvas_height: canvas.is_some_and(|canvas| canvas.height.is_some()),
        canvas_units: canvas
            .and_then(|canvas| canvas.units.as_deref())
            .map_or(PioStringView::EMPTY, PioStringView::new),
        has_canvas_units: canvas.is_some_and(|canvas| canvas.units.is_some()),
    }
}

fn load_voltage_model_view(load: &powerio_tx::Load) -> PioBalancedLoadVoltageModelView {
    let mut view = PioBalancedLoadVoltageModelView {
        kind: PioStringView::new("constant_power"),
        p_constant_power_mw: load.p,
        q_constant_power_mvar: load.q,
        p_constant_current_mw: 0.0,
        q_constant_current_mvar: 0.0,
        p_constant_impedance_mw: 0.0,
        q_constant_impedance_mvar: 0.0,
        exponential_p_mw: 0.0,
        exponential_q_mvar: 0.0,
        gamma_p: 0.0,
        gamma_q: 0.0,
        nominal_voltage_pu: 0.0,
        has_nominal_voltage: false,
        load_type: 0,
        has_load_type: false,
        scaling: 0.0,
        has_scaling: false,
    };
    match &load.voltage_model {
        None | Some(powerio_tx::LoadVoltageModel::ConstantPower) => {}
        Some(powerio_tx::LoadVoltageModel::Zip {
            p_constant_power,
            q_constant_power,
            p_constant_current,
            q_constant_current,
            p_constant_impedance,
            q_constant_impedance,
            v_nom,
            load_type,
            scaling,
        }) => {
            view.kind = PioStringView::new("zip");
            view.p_constant_power_mw = *p_constant_power;
            view.q_constant_power_mvar = *q_constant_power;
            view.p_constant_current_mw = *p_constant_current;
            view.q_constant_current_mvar = *q_constant_current;
            view.p_constant_impedance_mw = *p_constant_impedance;
            view.q_constant_impedance_mvar = *q_constant_impedance;
            if let Some(value) = v_nom {
                view.nominal_voltage_pu = *value;
                view.has_nominal_voltage = true;
            }
            if let Some(value) = load_type {
                view.load_type = *value;
                view.has_load_type = true;
            }
            if let Some(value) = scaling {
                view.scaling = *value;
                view.has_scaling = true;
            }
        }
        Some(powerio_tx::LoadVoltageModel::Exponential {
            p,
            q,
            v_nom,
            gamma_p,
            gamma_q,
        }) => {
            view.kind = PioStringView::new("exponential");
            view.p_constant_power_mw = 0.0;
            view.q_constant_power_mvar = 0.0;
            view.exponential_p_mw = *p;
            view.exponential_q_mvar = *q;
            view.gamma_p = *gamma_p;
            view.gamma_q = *gamma_q;
            if let Some(value) = v_nom {
                view.nominal_voltage_pu = *value;
                view.has_nominal_voltage = true;
            }
        }
        Some(_) => view.kind = PioStringView::new("unknown"),
    }
    view
}

fn switched_shunt_mode_name(mode: powerio_tx::SwitchedShuntMode) -> &'static str {
    match mode {
        powerio_tx::SwitchedShuntMode::Locked => "locked",
        powerio_tx::SwitchedShuntMode::Continuous => "continuous",
        powerio_tx::SwitchedShuntMode::Discrete => "discrete",
        _ => "unknown",
    }
}

const GENERATOR_CAPABILITY_NAMES: [&str; 11] = [
    "pc1", "pc2", "qc1min", "qc1max", "qc2min", "qc2max", "ramp_agc", "ramp_10", "ramp_30",
    "ramp_q", "apf",
];

fn static_var_compensator_regulation_mode_name(
    mode: powerio_tx::StaticVarCompensatorRegulationMode,
) -> &'static str {
    match mode {
        powerio_tx::StaticVarCompensatorRegulationMode::Voltage => "voltage",
        powerio_tx::StaticVarCompensatorRegulationMode::ReactivePower => "reactive_power",
        _ => "unknown",
    }
}

fn hvdc_converter_kind_name(kind: powerio_tx::HvdcConverterKind) -> &'static str {
    match kind {
        powerio_tx::HvdcConverterKind::Vsc => "vsc",
        powerio_tx::HvdcConverterKind::Lcc => "lcc",
        _ => "unknown",
    }
}

fn hvdc_converters_mode_name(mode: powerio_tx::HvdcConvertersMode) -> &'static str {
    match mode {
        powerio_tx::HvdcConvertersMode::Side1RectifierSide2Inverter => {
            "side1_rectifier_side2_inverter"
        }
        powerio_tx::HvdcConvertersMode::Side1InverterSide2Rectifier => {
            "side1_inverter_side2_rectifier"
        }
        _ => "unknown",
    }
}

fn hvdc_converter_view(
    converter: Option<&powerio_tx::HvdcConverter>,
) -> (PioBalancedHvdcConverterView, bool) {
    let empty_terminal = terminal_reference_view(None).0;
    converter.map_or(
        (
            PioBalancedHvdcConverterView {
                component: empty_component_id_view(),
                kind: PioStringView::EMPTY,
                loss_factor_percent: 0.0,
                voltage_regulator_on: false,
                has_voltage_regulator_on: false,
                voltage_setpoint_kv: 0.0,
                has_voltage_setpoint: false,
                reactive_power_setpoint_mvar: 0.0,
                has_reactive_power_setpoint: false,
                power_factor: 0.0,
                has_power_factor: false,
                regulating_terminal: empty_terminal,
                has_regulating_terminal: false,
            },
            false,
        ),
        |converter| {
            let (regulating_terminal, has_regulating_terminal) =
                terminal_reference_view(converter.regulating_terminal.as_ref());
            (
                PioBalancedHvdcConverterView {
                    component: component_id_view(&converter.component),
                    kind: PioStringView::new(hvdc_converter_kind_name(converter.kind)),
                    loss_factor_percent: converter.loss_factor_percent,
                    voltage_regulator_on: converter.voltage_regulator_on.unwrap_or(false),
                    has_voltage_regulator_on: converter.voltage_regulator_on.is_some(),
                    voltage_setpoint_kv: converter.voltage_setpoint_kv.unwrap_or(0.0),
                    has_voltage_setpoint: converter.voltage_setpoint_kv.is_some(),
                    reactive_power_setpoint_mvar: converter
                        .reactive_power_setpoint_mvar
                        .unwrap_or(0.0),
                    has_reactive_power_setpoint: converter.reactive_power_setpoint_mvar.is_some(),
                    power_factor: converter.power_factor.unwrap_or(0.0),
                    has_power_factor: converter.power_factor.is_some(),
                    regulating_terminal,
                    has_regulating_terminal,
                },
                true,
            )
        },
    )
}

/// Read one bus by zero based table position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_balanced_network_bus_at(
    network: *const PioBalancedNetwork,
    index: usize,
    output: *mut PioBalancedBusView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_balanced_network(network)?;
            let bus = network.buses().get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("bus index {index} is out of range"),
                )
            })?;
            let output = require_output(output, "output")?;
            let (component_id, has_component_id) = optional_string_view(bus.uid.as_deref());
            let (name, has_name) = optional_string_view(bus.name.as_deref());
            let (location, has_location) = bus
                .location
                .as_ref()
                .map_or((empty_balanced_location_view(), false), |location| {
                    (balanced_location_view(location), true)
                });
            *output = PioBalancedBusView {
                component_id,
                has_component_id,
                id: bus.id.0,
                bus_type: PioStringView::new(bus.kind.as_str()),
                vm_pu: bus.vm,
                va_degrees: bus.va,
                base_kv: bus.base_kv,
                vmax_pu: bus.vmax,
                vmin_pu: bus.vmin,
                has_emergency_voltage_limits: bus.evhi.is_some() || bus.evlo.is_some(),
                emergency_vmax_pu: bus.evhi.unwrap_or(bus.vmax),
                emergency_vmin_pu: bus.evlo.unwrap_or(bus.vmin),
                area: bus.area,
                zone: bus.zone,
                name,
                has_name,
                location,
                has_location,
            };
            Ok(true)
        })
    }
}

/// Read one load by zero based table position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_balanced_network_load_at(
    network: *const PioBalancedNetwork,
    index: usize,
    output: *mut PioBalancedLoadView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_balanced_network(network)?;
            let load = network.loads().get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("load index {index} is out of range"),
                )
            })?;
            let output = require_output(output, "output")?;
            let (component_id, has_component_id) = optional_string_view(load.uid.as_deref());
            *output = PioBalancedLoadView {
                component_id,
                has_component_id,
                bus_id: load.bus.0,
                p_mw: load.p,
                q_mvar: load.q,
                in_service: load.in_service,
                voltage_model: load_voltage_model_view(load),
            };
            Ok(true)
        })
    }
}

/// Read one shunt by zero based table position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_balanced_network_shunt_at(
    network: *const PioBalancedNetwork,
    index: usize,
    output: *mut PioBalancedShuntView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_balanced_network(network)?;
            let shunt = network.shunts().get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("shunt index {index} is out of range"),
                )
            })?;
            let output = require_output(output, "output")?;
            let (component_id, has_component_id) = optional_string_view(shunt.uid.as_deref());
            let control = shunt.control.as_ref();
            *output = PioBalancedShuntView {
                component_id,
                has_component_id,
                bus_id: shunt.bus.0,
                conductance_mw: shunt.g,
                susceptance_mvar: shunt.b,
                in_service: shunt.in_service,
                section_count: shunt.section_count.unwrap_or(0),
                has_section_count: shunt.section_count.is_some(),
                has_control: control.is_some(),
                control_mode: control.map_or(PioStringView::EMPTY, |control| {
                    PioStringView::new(switched_shunt_mode_name(control.mode))
                }),
                control_vmax_pu: control.map_or(0.0, |control| control.vhigh),
                control_vmin_pu: control.map_or(0.0, |control| control.vlow),
                control_bus_id: control
                    .and_then(|control| control.control_bus)
                    .map_or(0, |bus| bus.0),
                has_control_bus: control.is_some_and(|control| control.control_bus.is_some()),
                control_reactive_range_percent: control.map_or(0.0, |control| control.rmpct),
                control_block_count: control.map_or(0, |control| control.blocks.len()),
            };
            Ok(true)
        })
    }
}

/// Read one switched shunt block by zero based position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_balanced_network_shunt_block_at(
    network: *const PioBalancedNetwork,
    shunt_index: usize,
    block_index: usize,
    output: *mut PioShuntBlockView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_balanced_network(network)?;
            let shunt = network.shunts().get(shunt_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("shunt index {shunt_index} is out of range"),
                )
            })?;
            let block = shunt
                .control
                .as_ref()
                .and_then(|control| control.blocks.get(block_index))
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!(
                            "shunt block index {block_index} is out of range for shunt {shunt_index}"
                        ),
                    )
                })?;
            *require_output(output, "output")? = PioShuntBlockView {
                steps: block.steps,
                conductance_mw: block.g,
                susceptance_mvar: block.b,
            };
            Ok(true)
        })
    }
}

/// Read one static VAR compensator by zero based table position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_balanced_network_static_var_compensator_at(
    network: *const PioBalancedNetwork,
    index: usize,
    output: *mut PioBalancedStaticVarCompensatorView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_balanced_network(network)?;
            let svc = network
                .static_var_compensators()
                .get(index)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!("static VAR compensator index {index} is out of range"),
                    )
                })?;
            let (component_id, has_component_id) = optional_string_view(svc.uid.as_deref());
            let (regulating_terminal, has_regulating_terminal) =
                terminal_reference_view(svc.regulating_terminal.as_ref());
            *require_output(output, "output")? = PioBalancedStaticVarCompensatorView {
                component_id,
                has_component_id,
                bus_id: svc.bus.0,
                minimum_susceptance_siemens: svc.b_min_siemens,
                maximum_susceptance_siemens: svc.b_max_siemens,
                voltage_setpoint_kv: svc.voltage_setpoint_kv,
                reactive_power_setpoint_mvar: svc.reactive_power_setpoint_mvar,
                regulation_mode: PioStringView::new(static_var_compensator_regulation_mode_name(
                    svc.regulation_mode,
                )),
                regulating: svc.regulating,
                regulating_terminal,
                has_regulating_terminal,
                active_power_mw: svc.p,
                reactive_power_mvar: svc.q,
                in_service: svc.in_service,
            };
            Ok(true)
        })
    }
}

/// Read one branch by zero based table position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_balanced_network_branch_at(
    network: *const PioBalancedNetwork,
    index: usize,
    output: *mut PioBalancedBranchView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_balanced_network(network)?;
            let branch = network.branches().get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("branch index {index} is out of range"),
                )
            })?;
            let output = require_output(output, "output")?;
            let (component_id, has_component_id) = optional_string_view(branch.uid.as_deref());
            let (name, has_name) = optional_string_view(branch.name.as_deref());
            let charging = branch.calc_terminal_charging();
            let current = branch.current_ratings;
            let (control, has_control) = transformer_control_view(branch.control.as_ref());
            *output = PioBalancedBranchView {
                component_id,
                has_component_id,
                name,
                has_name,
                from_bus_id: branch.from.0,
                to_bus_id: branch.to.0,
                resistance_pu: branch.r,
                reactance_pu: branch.x,
                total_charging_susceptance_pu: branch.b,
                terminal_charging_is_explicit: branch.charging.is_some(),
                from_conductance_pu: charging.g_fr,
                from_susceptance_pu: charging.b_fr,
                to_conductance_pu: charging.g_to,
                to_susceptance_pu: charging.b_to,
                rate_a_mva: branch.rate_a,
                rate_b_mva: branch.rate_b,
                rate_c_mva: branch.rate_c,
                additional_rating_count: branch.rating_sets.len(),
                has_current_ratings: current.is_some(),
                current_rating_a: current.map_or(0.0, |rating| rating.c_rating_a),
                current_rating_b: current.map_or(0.0, |rating| rating.c_rating_b),
                current_rating_c: current.map_or(0.0, |rating| rating.c_rating_c),
                tap_ratio: branch.tap,
                effective_tap_ratio: branch.calc_effective_tap(),
                phase_shift_degrees: branch.shift,
                in_service: branch.in_service,
                angle_min_degrees: branch.angmin,
                angle_max_degrees: branch.angmax,
                control,
                has_control,
                route_point_count: branch.route.as_ref().map_or(0, Vec::len),
                has_route: branch.route.is_some(),
            };
            Ok(true)
        })
    }
}

/// Read one point from an explicitly stored balanced branch route.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_balanced_network_branch_route_point_at(
    network: *const PioBalancedNetwork,
    branch_index: usize,
    point_index: usize,
    output: *mut PioBalancedLocationView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_balanced_network(network)?;
            let branch = network.branches().get(branch_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("branch index {branch_index} is out of range"),
                )
            })?;
            let route = branch.route.as_ref().ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("balanced branch {branch_index} has no route"),
                )
            })?;
            let point = route.get(point_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("branch route point index {point_index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = balanced_location_view(point);
            Ok(true)
        })
    }
}

/// Read one additional named branch MVA rating.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_balanced_network_branch_rating_at(
    network: *const PioBalancedNetwork,
    branch_index: usize,
    rating_index: usize,
    output: *mut PioBranchRatingView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_balanced_network(network)?;
            let branch = network.branches().get(branch_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("branch index {branch_index} is out of range"),
                )
            })?;
            let rating = branch.rating_sets.get(rating_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!(
                        "rating index {rating_index} is out of range for branch {branch_index}"
                    ),
                )
            })?;
            *require_output(output, "output")? = PioBranchRatingView {
                name: PioStringView::new(&rating.name),
                rate_mva: rating.rate_mva,
            };
            Ok(true)
        })
    }
}

fn active_power_control_view(
    value: Option<&powerio_tx::ActivePowerControl>,
) -> (PioActivePowerControlView, bool) {
    let Some(value) = value else {
        return (
            PioActivePowerControlView {
                participate: false,
                droop_percent: 0.0,
                has_droop_percent: false,
                participation_factor: 0.0,
                has_participation_factor: false,
                minimum_target_active_power_mw: 0.0,
                has_minimum_target_active_power: false,
                maximum_target_active_power_mw: 0.0,
                has_maximum_target_active_power: false,
            },
            false,
        );
    };
    (
        PioActivePowerControlView {
            participate: value.participate,
            droop_percent: value.droop_percent.unwrap_or(0.0),
            has_droop_percent: value.droop_percent.is_some(),
            participation_factor: value.participation_factor.unwrap_or(0.0),
            has_participation_factor: value.participation_factor.is_some(),
            minimum_target_active_power_mw: value.minimum_target_active_power_mw.unwrap_or(0.0),
            has_minimum_target_active_power: value.minimum_target_active_power_mw.is_some(),
            maximum_target_active_power_mw: value.maximum_target_active_power_mw.unwrap_or(0.0),
            has_maximum_target_active_power: value.maximum_target_active_power_mw.is_some(),
        },
        true,
    )
}

fn generator_energy_source_name(value: powerio_tx::GeneratorEnergySource) -> &'static str {
    match value {
        powerio_tx::GeneratorEnergySource::Hydro => "hydro",
        powerio_tx::GeneratorEnergySource::Nuclear => "nuclear",
        powerio_tx::GeneratorEnergySource::Wind => "wind",
        powerio_tx::GeneratorEnergySource::Thermal => "thermal",
        powerio_tx::GeneratorEnergySource::Solar => "solar",
        powerio_tx::GeneratorEnergySource::Other => "other",
        _ => "unknown",
    }
}

/// Read one generator by zero based table position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_balanced_network_generator_at(
    network: *const PioBalancedNetwork,
    index: usize,
    output: *mut PioBalancedGeneratorView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_balanced_network(network)?;
            let generator = network.generators().get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("generator index {index} is out of range"),
                )
            })?;
            let output = require_output(output, "output")?;
            let (component_id, has_component_id) = optional_string_view(generator.uid.as_deref());
            let cost = generator.cost.as_ref();
            let (active_power_control, has_active_power_control) =
                active_power_control_view(generator.active_power_control.as_ref());
            let (regulating_terminal, has_regulating_terminal) =
                terminal_reference_view(generator.regulating_terminal.as_ref());
            *output = PioBalancedGeneratorView {
                component_id,
                has_component_id,
                bus_id: generator.bus.0,
                energy_source: PioStringView::new(generator_energy_source_name(
                    generator.energy_source,
                )),
                active_power_mw: generator.pg,
                reactive_power_mvar: generator.qg,
                active_power_max_mw: generator.pmax,
                active_power_min_mw: generator.pmin,
                reactive_power_max_mvar: generator.qmax,
                reactive_power_min_mvar: generator.qmin,
                voltage_setpoint_pu: generator.vg,
                machine_base_mva: generator.mbase,
                in_service: generator.in_service,
                has_cost: cost.is_some(),
                cost: cost.map_or(
                    PioGeneratorCostView {
                        model: 0,
                        startup: 0.0,
                        shutdown: 0.0,
                        ncost: 0,
                        coefficients: PioF64View::EMPTY,
                    },
                    |cost| PioGeneratorCostView {
                        model: cost.model,
                        startup: cost.startup,
                        shutdown: cost.shutdown,
                        ncost: cost.ncost,
                        coefficients: PioF64View::new(&cost.coeffs),
                    },
                ),
                regulated_bus_id: generator.regulated_bus.map_or(0, |bus| bus.0),
                has_regulated_bus: generator.regulated_bus.is_some(),
                capability_count: GENERATOR_CAPABILITY_NAMES.len(),
                active_power_control,
                has_active_power_control,
                voltage_regulation_on: generator.voltage_regulation_on,
                regulating_terminal,
                has_regulating_terminal,
            };
            Ok(true)
        })
    }
}

/// Read one named generator capability or ramp field.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_balanced_network_generator_capability_at(
    network: *const PioBalancedNetwork,
    generator_index: usize,
    capability_index: usize,
    output: *mut PioGeneratorCapabilityView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_balanced_network(network)?;
            let generator = network.generators().get(generator_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("generator index {generator_index} is out of range"),
                )
            })?;
            let name = GENERATOR_CAPABILITY_NAMES
                .get(capability_index)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!("generator capability index {capability_index} is out of range"),
                    )
                })?;
            let value = generator.caps[capability_index];
            *require_output(output, "output")? = PioGeneratorCapabilityView {
                name: PioStringView::new(name),
                value: value.unwrap_or(0.0),
                has_value: value.is_some(),
            };
            Ok(true)
        })
    }
}

/// Read one storage element by zero based table position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_balanced_network_storage_at(
    network: *const PioBalancedNetwork,
    index: usize,
    output: *mut PioBalancedStorageView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_balanced_network(network)?;
            let storage = network.storage().get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("storage index {index} is out of range"),
                )
            })?;
            let output = require_output(output, "output")?;
            let (component_id, has_component_id) = optional_string_view(storage.uid.as_deref());
            let (active_power_control, has_active_power_control) =
                active_power_control_view(storage.active_power_control.as_ref());
            *output = PioBalancedStorageView {
                component_id,
                has_component_id,
                bus_id: storage.bus.0,
                active_power_mw: storage.ps,
                reactive_power_mvar: storage.qs,
                energy_mwh: storage.energy,
                energy_rating_mwh: storage.energy_rating,
                charge_rating_mw: storage.charge_rating,
                discharge_rating_mw: storage.discharge_rating,
                charge_efficiency: storage.charge_efficiency,
                discharge_efficiency: storage.discharge_efficiency,
                thermal_rating_mva: storage.thermal_rating,
                current_rating: storage.current_rating.unwrap_or(0.0),
                has_current_rating: storage.current_rating.is_some(),
                reactive_power_min_mvar: storage.qmin,
                reactive_power_max_mvar: storage.qmax,
                resistance_pu: storage.r,
                reactance_pu: storage.x,
                active_power_loss_mw: storage.p_loss,
                reactive_power_loss_mvar: storage.q_loss,
                in_service: storage.in_service,
                active_power_control,
                has_active_power_control,
            };
            Ok(true)
        })
    }
}

/// Read one transmission switch by zero based table position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_balanced_network_switch_at(
    network: *const PioBalancedNetwork,
    index: usize,
    output: *mut PioBalancedSwitchView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_balanced_network(network)?;
            let switch = network.switches().get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("switch index {index} is out of range"),
                )
            })?;
            let (component_id, has_component_id) = optional_string_view(switch.uid.as_deref());
            *require_output(output, "output")? = PioBalancedSwitchView {
                component_id,
                has_component_id,
                from_bus_id: switch.from.0,
                to_bus_id: switch.to.0,
                closed: switch.closed,
                thermal_rating_mva: switch.thermal_rating.unwrap_or(0.0),
                has_thermal_rating: switch.thermal_rating.is_some(),
                current_rating_a: switch.current_rating.unwrap_or(0.0),
                has_current_rating: switch.current_rating.is_some(),
                from_active_power_mw: switch.pf.unwrap_or(0.0),
                has_from_active_power: switch.pf.is_some(),
                from_reactive_power_mvar: switch.qf.unwrap_or(0.0),
                has_from_reactive_power: switch.qf.is_some(),
                to_active_power_mw: switch.pt.unwrap_or(0.0),
                has_to_active_power: switch.pt.is_some(),
                to_reactive_power_mvar: switch.qt.unwrap_or(0.0),
                has_to_reactive_power: switch.qt.is_some(),
            };
            Ok(true)
        })
    }
}

/// Read one HVDC line by zero based table position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_balanced_network_hvdc_at(
    network: *const PioBalancedNetwork,
    index: usize,
    output: *mut PioBalancedHvdcView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_balanced_network(network)?;
            let hvdc = network.hvdc().get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("HVDC index {index} is out of range"),
                )
            })?;
            let (component_id, has_component_id) = optional_string_view(hvdc.uid.as_deref());
            let (converter1, has_converter1) = hvdc_converter_view(hvdc.converter1.as_ref());
            let (converter2, has_converter2) = hvdc_converter_view(hvdc.converter2.as_ref());
            let (converters_mode, has_converters_mode) = hvdc
                .converters_mode
                .map_or((PioStringView::EMPTY, false), |mode| {
                    (PioStringView::new(hvdc_converters_mode_name(mode)), true)
                });
            let cost = hvdc.cost.as_ref();
            *require_output(output, "output")? = PioBalancedHvdcView {
                component_id,
                has_component_id,
                from_bus_id: hvdc.from.0,
                to_bus_id: hvdc.to.0,
                in_service: hvdc.in_service,
                from_active_power_mw: hvdc.pf,
                to_active_power_mw: hvdc.pt,
                from_reactive_power_mvar: hvdc.qf,
                to_reactive_power_mvar: hvdc.qt,
                from_voltage_pu: hvdc.vf,
                to_voltage_pu: hvdc.vt,
                minimum_active_power_mw: hvdc.pmin,
                maximum_active_power_mw: hvdc.pmax,
                minimum_from_reactive_power_mvar: hvdc.qminf,
                maximum_from_reactive_power_mvar: hvdc.qmaxf,
                minimum_to_reactive_power_mvar: hvdc.qmint,
                maximum_to_reactive_power_mvar: hvdc.qmaxt,
                constant_loss_mw: hvdc.loss0,
                proportional_loss: hvdc.loss1,
                resistance_ohm: hvdc.resistance_ohm.unwrap_or(0.0),
                has_resistance: hvdc.resistance_ohm.is_some(),
                nominal_voltage_kv: hvdc.nominal_voltage_kv.unwrap_or(0.0),
                has_nominal_voltage: hvdc.nominal_voltage_kv.is_some(),
                converters_mode,
                has_converters_mode,
                converter1,
                has_converter1,
                converter2,
                has_converter2,
                cost: cost.map_or(
                    PioGeneratorCostView {
                        model: 0,
                        startup: 0.0,
                        shutdown: 0.0,
                        ncost: 0,
                        coefficients: PioF64View::EMPTY,
                    },
                    |cost| PioGeneratorCostView {
                        model: cost.model,
                        startup: cost.startup,
                        shutdown: cost.shutdown,
                        ncost: cost.ncost,
                        coefficients: PioF64View::new(&cost.coeffs),
                    },
                ),
                has_cost: cost.is_some(),
            };
            Ok(true)
        })
    }
}

/// Read one three winding transformer by zero based table position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_balanced_network_three_winding_transformer_at(
    network: *const PioBalancedNetwork,
    index: usize,
    output: *mut PioBalancedThreeWindingTransformerView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_balanced_network(network)?;
            let transformer = network.transformers_3w().get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("three winding transformer index {index} is out of range"),
                )
            })?;
            let (component_id, has_component_id) = optional_string_view(transformer.uid.as_deref());
            let (name, has_name) = optional_string_view(transformer.name.as_deref());
            *require_output(output, "output")? = PioBalancedThreeWindingTransformerView {
                component_id,
                has_component_id,
                name,
                has_name,
                winding_count: transformer.windings.len(),
                impedance_count: transformer.z.len(),
                star_voltage_magnitude_pu: transformer.star_vm,
                star_voltage_angle_degrees: transformer.star_va,
                magnetizing_conductance_pu: transformer.mag_g,
                magnetizing_susceptance_pu: transformer.mag_b,
                in_service: transformer.in_service,
            };
            Ok(true)
        })
    }
}

/// Read one winding of a three winding transformer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_balanced_network_three_winding_transformer_winding_at(
    network: *const PioBalancedNetwork,
    transformer_index: usize,
    winding_index: usize,
    output: *mut PioThreeWindingTransformerWindingView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_balanced_network(network)?;
            let transformer = network
                .transformers_3w()
                .get(transformer_index)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!(
                            "three winding transformer index {transformer_index} is out of range"
                        ),
                    )
                })?;
            let winding = transformer.windings.get(winding_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("winding index {winding_index} is out of range"),
                )
            })?;
            let (control, has_control) = transformer_control_view(winding.control.as_ref());
            *require_output(output, "output")? = PioThreeWindingTransformerWindingView {
                bus_id: winding.bus.0,
                tap_ratio: winding.tap,
                phase_shift_degrees: winding.shift,
                nominal_voltage_kv: winding.nominal_kv,
                rating_a_mva: winding.rate_a,
                rating_b_mva: winding.rate_b,
                rating_c_mva: winding.rate_c,
                control,
                has_control,
            };
            Ok(true)
        })
    }
}

/// Read one pairwise impedance of a three winding transformer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_balanced_network_three_winding_transformer_impedance_at(
    network: *const PioBalancedNetwork,
    transformer_index: usize,
    impedance_index: usize,
    output: *mut PioThreeWindingTransformerImpedanceView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_balanced_network(network)?;
            let transformer = network
                .transformers_3w()
                .get(transformer_index)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!(
                            "three winding transformer index {transformer_index} is out of range"
                        ),
                    )
                })?;
            let impedance = transformer.z.get(impedance_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("impedance index {impedance_index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = PioThreeWindingTransformerImpedanceView {
                resistance_pu: impedance.r,
                reactance_pu: impedance.x,
                base_mva: impedance.base_mva,
            };
            Ok(true)
        })
    }
}

/// Read one control area by zero based table position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_balanced_network_area_at(
    network: *const PioBalancedNetwork,
    index: usize,
    output: *mut PioBalancedAreaView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_balanced_network(network)?;
            let area = network.areas().get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("area index {index} is out of range"),
                )
            })?;
            let (name, has_name) = optional_string_view(area.name.as_deref());
            let (component_id, has_component_id) = optional_string_view(area.uid.as_deref());
            let (area_type, has_area_type) = optional_string_view(area.area_type.as_deref());
            *require_output(output, "output")? = PioBalancedAreaView {
                number: area.number,
                slack_bus_id: area.slack_bus.map_or(0, |bus| bus.0),
                has_slack_bus: area.slack_bus.is_some(),
                net_interchange_mw: area.net_interchange,
                tolerance_mw: area.tolerance,
                name,
                has_name,
                component_id,
                has_component_id,
                area_type,
                has_area_type,
            };
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_balanced_network_retain(
    network: *const PioBalancedNetwork,
) -> *mut PioBalancedNetwork {
    unsafe { PioBalancedNetwork::retain_raw(network) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_balanced_network_release(network: *mut PioBalancedNetwork) {
    unsafe { PioBalancedNetwork::release_raw(network) };
}

unsafe fn require_multiconductor_network<'a>(
    network: *const PioMulticonductorNetwork,
) -> Result<&'a powerio_dist::MulticonductorNetwork, *mut PioError> {
    unsafe { PioMulticonductorNetwork::get(network) }
        .and_then(MulticonductorNetworkInner::network)
        .ok_or_else(|| {
            boundary_error(
                &codes::BIND_CAPI_NULL_HANDLE,
                "PioMulticonductorNetwork must not be NULL",
            )
        })
}

fn optional_f64_view(value: Option<&[f64]>) -> (PioF64View, bool) {
    value.map_or((PioF64View::EMPTY, false), |value| {
        (PioF64View::new(value), true)
    })
}

fn dist_coords_kind_name(kind: powerio_dist::DistCoordsKind) -> &'static str {
    match kind {
        powerio_dist::DistCoordsKind::Source => "source",
        powerio_dist::DistCoordsKind::Synthetic => "synthetic",
        powerio_dist::DistCoordsKind::Manual => "manual",
        powerio_dist::DistCoordsKind::Derived => "derived",
        _ => "unknown",
    }
}

fn multiconductor_location_view(
    location: &powerio_dist::DistLocation,
) -> PioMulticonductorLocationView {
    let (kind, has_kind) = location.kind.map_or((PioStringView::EMPTY, false), |kind| {
        (PioStringView::new(dist_coords_kind_name(kind)), true)
    });
    PioMulticonductorLocationView {
        x: location.x,
        y: location.y,
        kind,
        has_kind,
    }
}

fn empty_multiconductor_location_view() -> PioMulticonductorLocationView {
    PioMulticonductorLocationView {
        x: 0.0,
        y: 0.0,
        kind: PioStringView::EMPTY,
        has_kind: false,
    }
}

fn multiconductor_geo_view(geo: Option<&powerio_dist::DistGeoMeta>) -> PioMulticonductorGeoView {
    let Some(geo) = geo else {
        return PioMulticonductorGeoView {
            has_geo: false,
            space: PioStringView::EMPTY,
            crs: PioStringView::EMPTY,
            has_crs: false,
            kind: PioStringView::EMPTY,
            has_kind: false,
            has_canvas: false,
            canvas_width: 0.0,
            has_canvas_width: false,
            canvas_height: 0.0,
            has_canvas_height: false,
            canvas_units: PioStringView::EMPTY,
            has_canvas_units: false,
        };
    };
    let (space, crs, has_crs, canvas) = match &geo.space {
        powerio_dist::CoordinateSpace::Geographic { crs } => (
            "geographic",
            crs.as_deref()
                .map_or(PioStringView::EMPTY, PioStringView::new),
            crs.is_some(),
            None,
        ),
        powerio_dist::CoordinateSpace::Projected { crs } => (
            "projected",
            crs.as_deref()
                .map_or(PioStringView::EMPTY, PioStringView::new),
            crs.is_some(),
            None,
        ),
        powerio_dist::CoordinateSpace::Diagram { canvas } => {
            ("diagram", PioStringView::EMPTY, false, canvas.as_ref())
        }
        powerio_dist::CoordinateSpace::Unknown => ("unknown", PioStringView::EMPTY, false, None),
        _ => ("unknown", PioStringView::EMPTY, false, None),
    };
    let (kind, has_kind) = geo.kind.map_or((PioStringView::EMPTY, false), |kind| {
        (PioStringView::new(dist_coords_kind_name(kind)), true)
    });
    PioMulticonductorGeoView {
        has_geo: true,
        space: PioStringView::new(space),
        crs,
        has_crs,
        kind,
        has_kind,
        has_canvas: canvas.is_some(),
        canvas_width: canvas.and_then(|canvas| canvas.width).unwrap_or(0.0),
        has_canvas_width: canvas.is_some_and(|canvas| canvas.width.is_some()),
        canvas_height: canvas.and_then(|canvas| canvas.height).unwrap_or(0.0),
        has_canvas_height: canvas.is_some_and(|canvas| canvas.height.is_some()),
        canvas_units: canvas
            .and_then(|canvas| canvas.units.as_deref())
            .map_or(PioStringView::EMPTY, PioStringView::new),
        has_canvas_units: canvas.is_some_and(|canvas| canvas.units.is_some()),
    }
}

fn multiconductor_configuration_name(configuration: powerio_dist::Configuration) -> &'static str {
    match configuration {
        powerio_dist::Configuration::Wye => "wye",
        powerio_dist::Configuration::Delta => "delta",
        powerio_dist::Configuration::SinglePhase => "single_phase",
        _ => "unknown",
    }
}

fn multiconductor_winding_connection_name(
    connection: powerio_dist::DistWindingConn,
) -> &'static str {
    match connection {
        powerio_dist::DistWindingConn::Wye => "wye",
        powerio_dist::DistWindingConn::Delta => "delta",
        _ => "unknown",
    }
}

fn inverter_topology_name(topology: powerio_dist::IbrTopology) -> &'static str {
    match topology {
        powerio_dist::IbrTopology::SinglePhase => "SINGLE_PHASE",
        powerio_dist::IbrTopology::ThreeLeg => "THREE_LEG",
        powerio_dist::IbrTopology::FourLeg => "FOUR_LEG",
        _ => "UNKNOWN",
    }
}

fn inverter_prime_mover_name(prime_mover: powerio_dist::IbrPrimeMover) -> &'static str {
    match prime_mover {
        powerio_dist::IbrPrimeMover::Pv => "PV",
        powerio_dist::IbrPrimeMover::Battery => "BATTERY",
        powerio_dist::IbrPrimeMover::Generic => "GENERIC",
        powerio_dist::IbrPrimeMover::Statcom => "STATCOM",
        powerio_dist::IbrPrimeMover::Dstatcom => "DSTATCOM",
        _ => "UNKNOWN",
    }
}

fn inverter_voltage_aggregation_name(
    aggregation: powerio_dist::IbrVoltageAggregation,
) -> &'static str {
    match aggregation {
        powerio_dist::IbrVoltageAggregation::PerPhase => "PER_PHASE",
        powerio_dist::IbrVoltageAggregation::Average => "AVERAGE",
        _ => "UNKNOWN",
    }
}

fn control_voltage_reference_name(
    reference: powerio_dist::ControlVoltageReference,
) -> &'static str {
    match reference {
        powerio_dist::ControlVoltageReference::PnPerPhase => "PN_PER_PHASE",
        powerio_dist::ControlVoltageReference::PpPerPhase => "PP_PER_PHASE",
        powerio_dist::ControlVoltageReference::PpAveraged => "PP_AVERAGED",
        powerio_dist::ControlVoltageReference::PgAveraged => "PG_AVERAGED",
        powerio_dist::ControlVoltageReference::PnAveraged => "PN_AVERAGED",
        powerio_dist::ControlVoltageReference::PgPerPhase => "PG_PER_PHASE",
        _ => "UNKNOWN",
    }
}

fn reactive_power_unit_name(unit: powerio_dist::ReactivePowerUnit) -> &'static str {
    match unit {
        powerio_dist::ReactivePowerUnit::VaFraction => "VA_FRACTION",
        powerio_dist::ReactivePowerUnit::Var => "VAR",
        _ => "UNKNOWN",
    }
}

fn active_power_unit_name(unit: powerio_dist::ActivePowerUnit) -> &'static str {
    match unit {
        powerio_dist::ActivePowerUnit::VaFraction => "VA_FRACTION",
        powerio_dist::ActivePowerUnit::W => "W",
        _ => "UNKNOWN",
    }
}

fn reactive_power_reference_name(reference: powerio_dist::ReactivePowerReference) -> &'static str {
    match reference {
        powerio_dist::ReactivePowerReference::VarMax => "VAR_MAX",
        powerio_dist::ReactivePowerReference::VarAvailable => "VAR_AVAILABLE",
        _ => "UNKNOWN",
    }
}

fn active_power_reference_name(reference: powerio_dist::ActivePowerReference) -> &'static str {
    match reference {
        powerio_dist::ActivePowerReference::PAvailable => "P_AVAILABLE",
        powerio_dist::ActivePowerReference::PMax => "P_MAX",
        powerio_dist::ActivePowerReference::SMax => "S_MAX",
        _ => "UNKNOWN",
    }
}

fn string_slice_at(
    values: &[String],
    index: usize,
    description: &str,
) -> Result<PioStringView, *mut PioError> {
    values
        .get(index)
        .map(|value| PioStringView::new(value))
        .ok_or_else(|| {
            boundary_error(
                &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                format!("{description} index {index} is out of range"),
            )
        })
}

fn conductor_matrix_row(
    matrix: &powerio_dist::ConductorMatrix,
    row_index: usize,
    description: &str,
) -> Result<PioF64View, *mut PioError> {
    matrix
        .get(row_index)
        .map(|row| PioF64View::new(row))
        .ok_or_else(|| {
            boundary_error(
                &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                format!("{description} row index {row_index} is out of range"),
            )
        })
}

fn multiconductor_bus_view(bus: &powerio_dist::DistBus) -> PioMulticonductorBusView {
    let (vpn_min, has_vpn_min) = optional_f64_view(bus.vpn_min.as_deref());
    let (vpn_max, has_vpn_max) = optional_f64_view(bus.vpn_max.as_deref());
    let (vpp_min, has_vpp_min) = optional_f64_view(bus.vpp_min.as_deref());
    let (vpp_max, has_vpp_max) = optional_f64_view(bus.vpp_max.as_deref());
    let (location, has_location) = bus
        .location
        .as_ref()
        .map_or((empty_multiconductor_location_view(), false), |location| {
            (multiconductor_location_view(location), true)
        });
    PioMulticonductorBusView {
        id: PioStringView::new(&bus.id),
        terminal_count: bus.terminals.len(),
        grounded_terminal_count: bus.grounded.len(),
        voltage_min_v: bus.v_min.unwrap_or(0.0),
        has_voltage_min: bus.v_min.is_some(),
        voltage_max_v: bus.v_max.unwrap_or(0.0),
        has_voltage_max: bus.v_max.is_some(),
        phase_to_neutral_voltage_min_v: vpn_min,
        has_phase_to_neutral_voltage_min: has_vpn_min,
        phase_to_neutral_voltage_max_v: vpn_max,
        has_phase_to_neutral_voltage_max: has_vpn_max,
        phase_to_phase_voltage_min_v: vpp_min,
        has_phase_to_phase_voltage_min: has_vpp_min,
        phase_to_phase_voltage_max_v: vpp_max,
        has_phase_to_phase_voltage_max: has_vpp_max,
        positive_sequence_voltage_min_v: bus.vpos_min.unwrap_or(0.0),
        has_positive_sequence_voltage_min: bus.vpos_min.is_some(),
        positive_sequence_voltage_max_v: bus.vpos_max.unwrap_or(0.0),
        has_positive_sequence_voltage_max: bus.vpos_max.is_some(),
        negative_sequence_voltage_max_v: bus.vneg_max.unwrap_or(0.0),
        has_negative_sequence_voltage_max: bus.vneg_max.is_some(),
        zero_sequence_voltage_max_v: bus.vzero_max.unwrap_or(0.0),
        has_zero_sequence_voltage_max: bus.vzero_max.is_some(),
        neutral_to_ground_voltage_max_v: bus.vn_max.unwrap_or(0.0),
        has_neutral_to_ground_voltage_max: bus.vn_max.is_some(),
        location,
        has_location,
    }
}

fn multiconductor_line_code_view(
    line_code: &powerio_dist::DistLineCode,
) -> PioMulticonductorLineCodeView {
    let (current_limit, has_current_limit) = optional_f64_view(line_code.i_max.as_deref());
    let (apparent_power_limit, has_apparent_power_limit) =
        optional_f64_view(line_code.s_max.as_deref());
    let (source, has_source) = optional_string_view(line_code.source.as_deref());
    PioMulticonductorLineCodeView {
        name: PioStringView::new(&line_code.name),
        conductor_count: line_code.n_conductors,
        resistance_matrix_row_count: line_code.r_series.len(),
        reactance_matrix_row_count: line_code.x_series.len(),
        conductance_from_matrix_row_count: line_code.g_from.len(),
        susceptance_from_matrix_row_count: line_code.b_from.len(),
        conductance_to_matrix_row_count: line_code.g_to.len(),
        susceptance_to_matrix_row_count: line_code.b_to.len(),
        current_limit_a: current_limit,
        has_current_limit,
        apparent_power_limit_va: apparent_power_limit,
        has_apparent_power_limit,
        source,
        has_source,
    }
}

fn multiconductor_line_view(line: &powerio_dist::DistLine) -> PioMulticonductorLineView {
    let (current_limit, has_current_limit) = optional_f64_view(line.i_max.as_deref());
    let (apparent_power_limit, has_apparent_power_limit) = optional_f64_view(line.s_max.as_deref());
    PioMulticonductorLineView {
        name: PioStringView::new(&line.name),
        bus_from: PioStringView::new(&line.bus_from),
        bus_to: PioStringView::new(&line.bus_to),
        terminal_map_from_count: line.terminal_map_from.len(),
        terminal_map_to_count: line.terminal_map_to.len(),
        line_code: PioStringView::new(&line.linecode),
        length_m: line.length,
        route_point_count: line.route.as_ref().map_or(0, Vec::len),
        has_route: line.route.is_some(),
        current_limit_a: current_limit,
        has_current_limit,
        apparent_power_limit_va: apparent_power_limit,
        has_apparent_power_limit,
    }
}

fn multiconductor_switch_view(switch: &powerio_dist::DistSwitch) -> PioMulticonductorSwitchView {
    let (current_limit, has_current_limit) = optional_f64_view(switch.i_max.as_deref());
    PioMulticonductorSwitchView {
        name: PioStringView::new(&switch.name),
        bus_from: PioStringView::new(&switch.bus_from),
        bus_to: PioStringView::new(&switch.bus_to),
        terminal_map_from_count: switch.terminal_map_from.len(),
        terminal_map_to_count: switch.terminal_map_to.len(),
        open: switch.open,
        current_limit_a: current_limit,
        has_current_limit,
    }
}

fn multiconductor_load_view(load: &powerio_dist::DistLoad) -> PioMulticonductorLoadView {
    let empty = PioF64View::EMPTY;
    let (
        voltage_model,
        nominal_voltage,
        alpha_z,
        alpha_i,
        alpha_p,
        beta_z,
        beta_i,
        beta_p,
        gamma_p,
        gamma_q,
    ) = match &load.voltage_model {
        powerio_dist::DistLoadVoltageModel::ConstantPower { v_nom } => (
            "constant_power",
            PioF64View::new(v_nom),
            empty,
            empty,
            empty,
            empty,
            empty,
            empty,
            empty,
            empty,
        ),
        powerio_dist::DistLoadVoltageModel::ConstantCurrent { v_nom } => (
            "constant_current",
            PioF64View::new(v_nom),
            empty,
            empty,
            empty,
            empty,
            empty,
            empty,
            empty,
            empty,
        ),
        powerio_dist::DistLoadVoltageModel::ConstantImpedance { v_nom } => (
            "constant_impedance",
            PioF64View::new(v_nom),
            empty,
            empty,
            empty,
            empty,
            empty,
            empty,
            empty,
            empty,
        ),
        powerio_dist::DistLoadVoltageModel::Zip {
            v_nom,
            alpha_z,
            alpha_i,
            alpha_p,
            beta_z,
            beta_i,
            beta_p,
        } => (
            "zip",
            PioF64View::new(v_nom),
            PioF64View::new(alpha_z),
            PioF64View::new(alpha_i),
            PioF64View::new(alpha_p),
            PioF64View::new(beta_z),
            PioF64View::new(beta_i),
            PioF64View::new(beta_p),
            empty,
            empty,
        ),
        powerio_dist::DistLoadVoltageModel::Exponential {
            v_nom,
            gamma_p,
            gamma_q,
        } => (
            "exponential",
            PioF64View::new(v_nom),
            empty,
            empty,
            empty,
            empty,
            empty,
            empty,
            PioF64View::new(gamma_p),
            PioF64View::new(gamma_q),
        ),
        _ => (
            "unknown", empty, empty, empty, empty, empty, empty, empty, empty, empty,
        ),
    };
    PioMulticonductorLoadView {
        name: PioStringView::new(&load.name),
        bus: PioStringView::new(&load.bus),
        terminal_map_count: load.terminal_map.len(),
        configuration: PioStringView::new(multiconductor_configuration_name(load.configuration)),
        active_power_nominal_w: PioF64View::new(&load.p_nom),
        reactive_power_nominal_var: PioF64View::new(&load.q_nom),
        voltage_model: PioStringView::new(voltage_model),
        nominal_voltage_v: nominal_voltage,
        active_power_constant_impedance: alpha_z,
        active_power_constant_current: alpha_i,
        active_power_constant_power: alpha_p,
        reactive_power_constant_impedance: beta_z,
        reactive_power_constant_current: beta_i,
        reactive_power_constant_power: beta_p,
        active_power_exponent: gamma_p,
        reactive_power_exponent: gamma_q,
    }
}

fn multiconductor_generator_view(
    generator: &powerio_dist::DistGenerator,
) -> PioMulticonductorGeneratorView {
    let (p_min, has_p_min) = optional_f64_view(generator.p_min.as_deref());
    let (p_max, has_p_max) = optional_f64_view(generator.p_max.as_deref());
    let (q_min, has_q_min) = optional_f64_view(generator.q_min.as_deref());
    let (q_max, has_q_max) = optional_f64_view(generator.q_max.as_deref());
    let (cost, has_cost) = optional_f64_view(generator.cost.as_deref());
    let (s_max, has_s_max) = optional_f64_view(generator.s_max.as_deref());
    let (i_max, has_i_max) = optional_f64_view(generator.i_max.as_deref());
    PioMulticonductorGeneratorView {
        name: PioStringView::new(&generator.name),
        bus: PioStringView::new(&generator.bus),
        terminal_map_count: generator.terminal_map.len(),
        configuration: PioStringView::new(multiconductor_configuration_name(
            generator.configuration,
        )),
        active_power_nominal_w: PioF64View::new(&generator.p_nom),
        reactive_power_nominal_var: PioF64View::new(&generator.q_nom),
        active_power_min_w: p_min,
        has_active_power_min: has_p_min,
        active_power_max_w: p_max,
        has_active_power_max: has_p_max,
        reactive_power_min_var: q_min,
        has_reactive_power_min: has_q_min,
        reactive_power_max_var: q_max,
        has_reactive_power_max: has_q_max,
        active_power_dispatch_cost_per_kwh: cost,
        has_active_power_dispatch_cost: has_cost,
        apparent_power_limit_va: s_max,
        has_apparent_power_limit: has_s_max,
        current_limit_a: i_max,
        has_current_limit: has_i_max,
    }
}

fn inverter_based_resource_view(ibr: &powerio_dist::DistIbr) -> PioInverterBasedResourceView {
    let (i_max, has_i_max) = optional_f64_view(ibr.i_max.as_deref());
    let (p_min, has_p_min) = optional_f64_view(ibr.p_min.as_deref());
    let (p_max, has_p_max) = optional_f64_view(ibr.p_max.as_deref());
    let (q_min, has_q_min) = optional_f64_view(ibr.q_min.as_deref());
    let (q_max, has_q_max) = optional_f64_view(ibr.q_max.as_deref());
    let (control_profile, has_control_profile) =
        optional_string_view(ibr.control_profile.as_deref());
    let (voltage_aggregation, has_voltage_aggregation) =
        ibr.voltage_aggregation
            .map_or((PioStringView::EMPTY, false), |aggregation| {
                (
                    PioStringView::new(inverter_voltage_aggregation_name(aggregation)),
                    true,
                )
            });
    PioInverterBasedResourceView {
        name: PioStringView::new(&ibr.name),
        bus: PioStringView::new(&ibr.bus),
        terminal_map_count: ibr.terminal_map.len(),
        topology: PioStringView::new(inverter_topology_name(ibr.topology)),
        prime_mover: PioStringView::new(inverter_prime_mover_name(ibr.prime_mover)),
        apparent_power_limit_va: PioF64View::new(&ibr.s_max),
        current_limit_a: i_max,
        has_current_limit: has_i_max,
        active_power_available_w: ibr.p_avail.unwrap_or(0.0),
        has_active_power_available: ibr.p_avail.is_some(),
        active_power_min_w: p_min,
        has_active_power_min: has_p_min,
        active_power_max_w: p_max,
        has_active_power_max: has_p_max,
        reactive_power_min_var: q_min,
        has_reactive_power_min: has_q_min,
        reactive_power_max_var: q_max,
        has_reactive_power_max: has_q_max,
        control_profile,
        has_control_profile,
        voltage_aggregation,
        has_voltage_aggregation,
    }
}

fn control_profile_view(profile: &powerio_dist::DistControlProfile) -> PioControlProfileView {
    let power_factor = profile.power_factor.as_ref();
    let volt_var = profile.volt_var.as_ref();
    let volt_watt = profile.volt_watt.as_ref();
    let (vv_voltage_reference, has_vv_voltage_reference) = volt_var
        .and_then(|control| control.voltage_reference)
        .map_or((PioStringView::EMPTY, false), |reference| {
            (
                PioStringView::new(control_voltage_reference_name(reference)),
                true,
            )
        });
    let (vv_q_unit, has_vv_q_unit) = volt_var
        .and_then(|control| control.q_unit)
        .map_or((PioStringView::EMPTY, false), |unit| {
            (PioStringView::new(reactive_power_unit_name(unit)), true)
        });
    let (vv_q_ref, has_vv_q_ref) = volt_var.and_then(|control| control.q_ref).map_or(
        (PioStringView::EMPTY, false),
        |reference| {
            (
                PioStringView::new(reactive_power_reference_name(reference)),
                true,
            )
        },
    );
    let (vw_voltage_reference, has_vw_voltage_reference) = volt_watt
        .and_then(|control| control.voltage_reference)
        .map_or((PioStringView::EMPTY, false), |reference| {
            (
                PioStringView::new(control_voltage_reference_name(reference)),
                true,
            )
        });
    let (vw_p_unit, has_vw_p_unit) = volt_watt
        .and_then(|control| control.p_unit)
        .map_or((PioStringView::EMPTY, false), |unit| {
            (PioStringView::new(active_power_unit_name(unit)), true)
        });
    let (vw_p_ref, has_vw_p_ref) = volt_watt.and_then(|control| control.p_ref).map_or(
        (PioStringView::EMPTY, false),
        |reference| {
            (
                PioStringView::new(active_power_reference_name(reference)),
                true,
            )
        },
    );
    PioControlProfileView {
        name: PioStringView::new(&profile.name),
        has_power_factor: power_factor.is_some(),
        power_factor: power_factor.map_or(0.0, |control| control.pf),
        has_volt_var: volt_var.is_some(),
        volt_var_voltage_reference: vv_voltage_reference,
        has_volt_var_voltage_reference: has_vv_voltage_reference,
        volt_var_breakpoints: volt_var.map_or(PioF64View::EMPTY, |control| {
            PioF64View::new(&control.breakpoints)
        }),
        volt_var_reactive_power_limits: volt_var.map_or(PioF64View::EMPTY, |control| {
            PioF64View::new(&control.q_limits)
        }),
        volt_var_reactive_power_unit: vv_q_unit,
        has_volt_var_reactive_power_unit: has_vv_q_unit,
        volt_var_reactive_power_reference: vv_q_ref,
        has_volt_var_reactive_power_reference: has_vv_q_ref,
        volt_var_active_power_min_for_reactive_power_w: volt_var
            .and_then(|control| control.p_min_for_q)
            .unwrap_or(0.0),
        has_volt_var_active_power_min_for_reactive_power: volt_var
            .is_some_and(|control| control.p_min_for_q.is_some()),
        volt_var_active_power_min_for_max_reactive_power_w: volt_var
            .and_then(|control| control.p_min_for_q_max)
            .unwrap_or(0.0),
        has_volt_var_active_power_min_for_max_reactive_power: volt_var
            .is_some_and(|control| control.p_min_for_q_max.is_some()),
        has_volt_watt: volt_watt.is_some(),
        volt_watt_voltage_reference: vw_voltage_reference,
        has_volt_watt_voltage_reference: has_vw_voltage_reference,
        volt_watt_breakpoints: volt_watt.map_or(PioF64View::EMPTY, |control| {
            PioF64View::new(&control.breakpoints)
        }),
        volt_watt_active_power_limits: volt_watt.map_or(PioF64View::EMPTY, |control| {
            PioF64View::new(&control.p_limits)
        }),
        volt_watt_active_power_unit: vw_p_unit,
        has_volt_watt_active_power_unit: has_vw_p_unit,
        volt_watt_active_power_reference: vw_p_ref,
        has_volt_watt_active_power_reference: has_vw_p_ref,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_name(
    network: *const PioMulticonductorNetwork,
) -> PioStringView {
    unsafe { PioMulticonductorNetwork::get(network) }
        .and_then(MulticonductorNetworkInner::network)
        .and_then(|network| network.name().as_deref())
        .map_or(PioStringView::EMPTY, PioStringView::new)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_has_name(
    network: *const PioMulticonductorNetwork,
) -> bool {
    unsafe { PioMulticonductorNetwork::get(network) }
        .and_then(MulticonductorNetworkInner::network)
        .is_some_and(|network| network.name().is_some())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_source_format(
    network: *const PioMulticonductorNetwork,
) -> PioStringView {
    unsafe { PioMulticonductorNetwork::get(network) }
        .and_then(MulticonductorNetworkInner::network)
        .and_then(|network| *network.source_format())
        .map_or(PioStringView::EMPTY, |format| {
            PioStringView::new(format.name())
        })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_has_source_format(
    network: *const PioMulticonductorNetwork,
) -> bool {
    unsafe { PioMulticonductorNetwork::get(network) }
        .and_then(MulticonductorNetworkInner::network)
        .is_some_and(|network| network.source_format().is_some())
}

/// Read the network coordinate metadata, including absence through `has_geo`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_geo(
    network: *const PioMulticonductorNetwork,
    output: *mut PioMulticonductorGeoView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_multiconductor_network(network)?;
            *require_output(output, "output")? = multiconductor_geo_view(network.geo().as_ref());
            Ok(true)
        })
    }
}

/// Read exact table lengths. Defaulted source fields and arbitrary extension
/// maps are retained internally and are not separate domain tables.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_counts(
    network: *const PioMulticonductorNetwork,
    output: *mut PioMulticonductorNetworkCountsView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_multiconductor_network(network)?;
            *require_output(output, "output")? = PioMulticonductorNetworkCountsView {
                buses: network.buses().len(),
                line_codes: network.line_codes().len(),
                lines: network.lines().len(),
                switches: network.switches().len(),
                transformers: network.transformers().len(),
                loads: network.loads().len(),
                generators: network.generators().len(),
                inverter_based_resources: network.ibrs().len(),
                control_profiles: network.control_profiles().len(),
                shunts: network.shunts().len(),
                capacitors: network.capacitors().len(),
                voltage_sources: network.sources().len(),
                untyped_objects: network.untyped_objects().len(),
                commands: network.commands().len(),
                options: network.options().len(),
            };
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_base_frequency_hz(
    network: *const PioMulticonductorNetwork,
) -> f64 {
    unsafe { PioMulticonductorNetwork::get(network) }
        .and_then(MulticonductorNetworkInner::network)
        .map_or(f64::NAN, |network| network.base_frequency())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_bus_count(
    network: *const PioMulticonductorNetwork,
) -> usize {
    unsafe { PioMulticonductorNetwork::get(network) }
        .and_then(MulticonductorNetworkInner::network)
        .map_or(0, |network| network.buses().len())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_line_count(
    network: *const PioMulticonductorNetwork,
) -> usize {
    unsafe { PioMulticonductorNetwork::get(network) }
        .and_then(MulticonductorNetworkInner::network)
        .map_or(0, |network| network.lines().len())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_load_count(
    network: *const PioMulticonductorNetwork,
) -> usize {
    unsafe { PioMulticonductorNetwork::get(network) }
        .and_then(MulticonductorNetworkInner::network)
        .map_or(0, |network| network.loads().len())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_generator_count(
    network: *const PioMulticonductorNetwork,
) -> usize {
    unsafe { PioMulticonductorNetwork::get(network) }
        .and_then(MulticonductorNetworkInner::network)
        .map_or(0, |network| network.generators().len())
}

/// Read one multiconductor bus by zero based table position. Borrowed strings
/// and numeric spans remain valid while the network handle is alive.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_bus_at(
    network: *const PioMulticonductorNetwork,
    index: usize,
    output: *mut PioMulticonductorBusView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_multiconductor_network(network)?;
            let bus = network.buses().get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("multiconductor bus index {index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = multiconductor_bus_view(bus);
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_bus_terminal_at(
    network: *const PioMulticonductorNetwork,
    bus_index: usize,
    terminal_index: usize,
    output: *mut PioStringView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_multiconductor_network(network)?;
            let bus = network.buses().get(bus_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("multiconductor bus index {bus_index} is out of range"),
                )
            })?;
            *require_output(output, "output")? =
                string_slice_at(&bus.terminals, terminal_index, "bus terminal")?;
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_bus_grounded_terminal_at(
    network: *const PioMulticonductorNetwork,
    bus_index: usize,
    terminal_index: usize,
    output: *mut PioStringView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_multiconductor_network(network)?;
            let bus = network.buses().get(bus_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("multiconductor bus index {bus_index} is out of range"),
                )
            })?;
            *require_output(output, "output")? =
                string_slice_at(&bus.grounded, terminal_index, "grounded bus terminal")?;
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_line_code_at(
    network: *const PioMulticonductorNetwork,
    index: usize,
    output: *mut PioMulticonductorLineCodeView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_multiconductor_network(network)?;
            let line_code = network.line_codes().get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("multiconductor line code index {index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = multiconductor_line_code_view(line_code);
            Ok(true)
        })
    }
}

fn multiconductor_line_code_matrix<'a>(
    network: &'a powerio_dist::MulticonductorNetwork,
    line_code_index: usize,
    matrix: &str,
) -> Result<&'a powerio_dist::ConductorMatrix, *mut PioError> {
    let line_code = network.line_codes().get(line_code_index).ok_or_else(|| {
        boundary_error(
            &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
            format!("multiconductor line code index {line_code_index} is out of range"),
        )
    })?;
    Ok(match matrix {
        "resistance" => &line_code.r_series,
        "reactance" => &line_code.x_series,
        "conductance_from" => &line_code.g_from,
        "susceptance_from" => &line_code.b_from,
        "conductance_to" => &line_code.g_to,
        "susceptance_to" => &line_code.b_to,
        _ => unreachable!("all line code matrices are handled"),
    })
}

unsafe fn write_multiconductor_line_code_matrix_row(
    network: *const PioMulticonductorNetwork,
    line_code_index: usize,
    row_index: usize,
    matrix: &str,
    output: *mut PioF64View,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_multiconductor_network(network)?;
            let values = multiconductor_line_code_matrix(network, line_code_index, matrix)?;
            *require_output(output, "output")? =
                conductor_matrix_row(values, row_index, &format!("line code {matrix} matrix"))?;
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_line_code_resistance_matrix_row_at(
    network: *const PioMulticonductorNetwork,
    line_code_index: usize,
    row_index: usize,
    output: *mut PioF64View,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        write_multiconductor_line_code_matrix_row(
            network,
            line_code_index,
            row_index,
            "resistance",
            output,
            error,
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_line_code_reactance_matrix_row_at(
    network: *const PioMulticonductorNetwork,
    line_code_index: usize,
    row_index: usize,
    output: *mut PioF64View,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        write_multiconductor_line_code_matrix_row(
            network,
            line_code_index,
            row_index,
            "reactance",
            output,
            error,
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_line_code_conductance_from_matrix_row_at(
    network: *const PioMulticonductorNetwork,
    line_code_index: usize,
    row_index: usize,
    output: *mut PioF64View,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        write_multiconductor_line_code_matrix_row(
            network,
            line_code_index,
            row_index,
            "conductance_from",
            output,
            error,
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_line_code_susceptance_from_matrix_row_at(
    network: *const PioMulticonductorNetwork,
    line_code_index: usize,
    row_index: usize,
    output: *mut PioF64View,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        write_multiconductor_line_code_matrix_row(
            network,
            line_code_index,
            row_index,
            "susceptance_from",
            output,
            error,
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_line_code_conductance_to_matrix_row_at(
    network: *const PioMulticonductorNetwork,
    line_code_index: usize,
    row_index: usize,
    output: *mut PioF64View,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        write_multiconductor_line_code_matrix_row(
            network,
            line_code_index,
            row_index,
            "conductance_to",
            output,
            error,
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_line_code_susceptance_to_matrix_row_at(
    network: *const PioMulticonductorNetwork,
    line_code_index: usize,
    row_index: usize,
    output: *mut PioF64View,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        write_multiconductor_line_code_matrix_row(
            network,
            line_code_index,
            row_index,
            "susceptance_to",
            output,
            error,
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_line_at(
    network: *const PioMulticonductorNetwork,
    index: usize,
    output: *mut PioMulticonductorLineView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_multiconductor_network(network)?;
            let line = network.lines().get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("multiconductor line index {index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = multiconductor_line_view(line);
            Ok(true)
        })
    }
}

unsafe fn write_multiconductor_line_terminal(
    network: *const PioMulticonductorNetwork,
    line_index: usize,
    terminal_index: usize,
    from: bool,
    output: *mut PioStringView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_multiconductor_network(network)?;
            let line = network.lines().get(line_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("multiconductor line index {line_index} is out of range"),
                )
            })?;
            let (terminals, description) = if from {
                (&line.terminal_map_from, "line from terminal")
            } else {
                (&line.terminal_map_to, "line to terminal")
            };
            *require_output(output, "output")? =
                string_slice_at(terminals, terminal_index, description)?;
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_line_terminal_from_at(
    network: *const PioMulticonductorNetwork,
    line_index: usize,
    terminal_index: usize,
    output: *mut PioStringView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        write_multiconductor_line_terminal(network, line_index, terminal_index, true, output, error)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_line_terminal_to_at(
    network: *const PioMulticonductorNetwork,
    line_index: usize,
    terminal_index: usize,
    output: *mut PioStringView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        write_multiconductor_line_terminal(
            network,
            line_index,
            terminal_index,
            false,
            output,
            error,
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_line_route_point_at(
    network: *const PioMulticonductorNetwork,
    line_index: usize,
    point_index: usize,
    output: *mut PioMulticonductorLocationView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_multiconductor_network(network)?;
            let line = network.lines().get(line_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("multiconductor line index {line_index} is out of range"),
                )
            })?;
            let route = line.route.as_ref().ok_or_else(|| {
                boundary_error(
                    &codes::REQUEST_CAPI_TYPE_MISMATCH,
                    format!("multiconductor line {line_index} has no route"),
                )
            })?;
            let point = route.get(point_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("line route point index {point_index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = multiconductor_location_view(point);
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_switch_at(
    network: *const PioMulticonductorNetwork,
    index: usize,
    output: *mut PioMulticonductorSwitchView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_multiconductor_network(network)?;
            let switch = network.switches().get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("multiconductor switch index {index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = multiconductor_switch_view(switch);
            Ok(true)
        })
    }
}

unsafe fn write_multiconductor_switch_terminal(
    network: *const PioMulticonductorNetwork,
    switch_index: usize,
    terminal_index: usize,
    from: bool,
    output: *mut PioStringView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_multiconductor_network(network)?;
            let switch = network.switches().get(switch_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("multiconductor switch index {switch_index} is out of range"),
                )
            })?;
            let (terminals, description) = if from {
                (&switch.terminal_map_from, "switch from terminal")
            } else {
                (&switch.terminal_map_to, "switch to terminal")
            };
            *require_output(output, "output")? =
                string_slice_at(terminals, terminal_index, description)?;
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_switch_terminal_from_at(
    network: *const PioMulticonductorNetwork,
    switch_index: usize,
    terminal_index: usize,
    output: *mut PioStringView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        write_multiconductor_switch_terminal(
            network,
            switch_index,
            terminal_index,
            true,
            output,
            error,
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_switch_terminal_to_at(
    network: *const PioMulticonductorNetwork,
    switch_index: usize,
    terminal_index: usize,
    output: *mut PioStringView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        write_multiconductor_switch_terminal(
            network,
            switch_index,
            terminal_index,
            false,
            output,
            error,
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_transformer_at(
    network: *const PioMulticonductorNetwork,
    index: usize,
    output: *mut PioMulticonductorTransformerView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_multiconductor_network(network)?;
            let transformer = network.transformers().get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("multiconductor transformer index {index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = PioMulticonductorTransformerView {
                name: PioStringView::new(&transformer.name),
                winding_count: transformer.windings.len(),
                short_circuit_reactance_percent: PioF64View::new(&transformer.xsc_pct),
                phase_count: transformer.phases,
            };
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_transformer_winding_at(
    network: *const PioMulticonductorNetwork,
    transformer_index: usize,
    winding_index: usize,
    output: *mut PioMulticonductorTransformerWindingView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_multiconductor_network(network)?;
            let transformer = network
                .transformers()
                .get(transformer_index)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!(
                            "multiconductor transformer index {transformer_index} is out of range"
                        ),
                    )
                })?;
            let winding = transformer.windings.get(winding_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("transformer winding index {winding_index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = PioMulticonductorTransformerWindingView {
                bus: PioStringView::new(&winding.bus),
                terminal_map_count: winding.terminal_map.len(),
                connection: PioStringView::new(multiconductor_winding_connection_name(
                    winding.conn,
                )),
                rated_voltage_v: winding.v_ref,
                apparent_power_rating_va: winding.s_rating,
                resistance_percent: winding.r_pct,
                tap: winding.tap,
                neutral_resistance_ohm: winding.r_neutral.unwrap_or(0.0),
                has_neutral_resistance: winding.r_neutral.is_some(),
                neutral_reactance_ohm: winding.x_neutral.unwrap_or(0.0),
                has_neutral_reactance: winding.x_neutral.is_some(),
            };
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_transformer_winding_terminal_at(
    network: *const PioMulticonductorNetwork,
    transformer_index: usize,
    winding_index: usize,
    terminal_index: usize,
    output: *mut PioStringView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_multiconductor_network(network)?;
            let transformer = network
                .transformers()
                .get(transformer_index)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                        format!(
                            "multiconductor transformer index {transformer_index} is out of range"
                        ),
                    )
                })?;
            let winding = transformer.windings.get(winding_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("transformer winding index {winding_index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = string_slice_at(
                &winding.terminal_map,
                terminal_index,
                "transformer winding terminal",
            )?;
            Ok(true)
        })
    }
}

#[derive(Clone, Copy)]
enum MulticonductorTerminalTable {
    Load,
    Generator,
    InverterBasedResource,
    Shunt,
    Capacitor,
    VoltageSource,
}

unsafe fn write_multiconductor_terminal(
    network: *const PioMulticonductorNetwork,
    table: MulticonductorTerminalTable,
    element_index: usize,
    terminal_index: usize,
    output: *mut PioStringView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_multiconductor_network(network)?;
            let (terminals, description): (&[String], &str) = match table {
                MulticonductorTerminalTable::Load => (
                    &network
                        .loads()
                        .get(element_index)
                        .ok_or_else(|| {
                            boundary_error(
                                &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                                format!(
                                    "multiconductor load index {element_index} is out of range"
                                ),
                            )
                        })?
                        .terminal_map,
                    "load terminal",
                ),
                MulticonductorTerminalTable::Generator => (
                    &network
                        .generators()
                        .get(element_index)
                        .ok_or_else(|| {
                            boundary_error(
                                &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                                format!(
                                    "multiconductor generator index {element_index} is out of range"
                                ),
                            )
                        })?
                        .terminal_map,
                    "generator terminal",
                ),
                MulticonductorTerminalTable::InverterBasedResource => (
                    &network
                        .ibrs()
                        .get(element_index)
                        .ok_or_else(|| {
                            boundary_error(
                                &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                                format!(
                                    "inverter based resource index {element_index} is out of range"
                                ),
                            )
                        })?
                        .terminal_map,
                    "inverter based resource terminal",
                ),
                MulticonductorTerminalTable::Shunt => (
                    &network
                        .shunts()
                        .get(element_index)
                        .ok_or_else(|| {
                            boundary_error(
                                &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                                format!(
                                    "multiconductor shunt index {element_index} is out of range"
                                ),
                            )
                        })?
                        .terminal_map,
                    "shunt terminal",
                ),
                MulticonductorTerminalTable::Capacitor => (
                    &network
                        .capacitors()
                        .get(element_index)
                        .ok_or_else(|| {
                            boundary_error(
                                &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                                format!(
                                    "multiconductor capacitor index {element_index} is out of range"
                                ),
                            )
                        })?
                        .terminal_map,
                    "capacitor terminal",
                ),
                MulticonductorTerminalTable::VoltageSource => (
                    &network
                        .sources()
                        .get(element_index)
                        .ok_or_else(|| {
                            boundary_error(
                                &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                                format!("voltage source index {element_index} is out of range"),
                            )
                        })?
                        .terminal_map,
                    "voltage source terminal",
                ),
            };
            *require_output(output, "output")? =
                string_slice_at(terminals, terminal_index, description)?;
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_load_at(
    network: *const PioMulticonductorNetwork,
    index: usize,
    output: *mut PioMulticonductorLoadView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_multiconductor_network(network)?;
            let load = network.loads().get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("multiconductor load index {index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = multiconductor_load_view(load);
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_load_terminal_at(
    network: *const PioMulticonductorNetwork,
    load_index: usize,
    terminal_index: usize,
    output: *mut PioStringView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        write_multiconductor_terminal(
            network,
            MulticonductorTerminalTable::Load,
            load_index,
            terminal_index,
            output,
            error,
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_generator_at(
    network: *const PioMulticonductorNetwork,
    index: usize,
    output: *mut PioMulticonductorGeneratorView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_multiconductor_network(network)?;
            let generator = network.generators().get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("multiconductor generator index {index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = multiconductor_generator_view(generator);
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_generator_terminal_at(
    network: *const PioMulticonductorNetwork,
    generator_index: usize,
    terminal_index: usize,
    output: *mut PioStringView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        write_multiconductor_terminal(
            network,
            MulticonductorTerminalTable::Generator,
            generator_index,
            terminal_index,
            output,
            error,
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_inverter_based_resource_at(
    network: *const PioMulticonductorNetwork,
    index: usize,
    output: *mut PioInverterBasedResourceView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_multiconductor_network(network)?;
            let resource = network.ibrs().get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("inverter based resource index {index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = inverter_based_resource_view(resource);
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_inverter_based_resource_terminal_at(
    network: *const PioMulticonductorNetwork,
    resource_index: usize,
    terminal_index: usize,
    output: *mut PioStringView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        write_multiconductor_terminal(
            network,
            MulticonductorTerminalTable::InverterBasedResource,
            resource_index,
            terminal_index,
            output,
            error,
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_control_profile_at(
    network: *const PioMulticonductorNetwork,
    index: usize,
    output: *mut PioControlProfileView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_multiconductor_network(network)?;
            let profile = network.control_profiles().get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("control profile index {index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = control_profile_view(profile);
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_shunt_at(
    network: *const PioMulticonductorNetwork,
    index: usize,
    output: *mut PioMulticonductorShuntView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_multiconductor_network(network)?;
            let shunt = network.shunts().get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("multiconductor shunt index {index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = PioMulticonductorShuntView {
                name: PioStringView::new(&shunt.name),
                bus: PioStringView::new(&shunt.bus),
                terminal_map_count: shunt.terminal_map.len(),
                conductance_matrix_row_count: shunt.g.len(),
                susceptance_matrix_row_count: shunt.b.len(),
            };
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_shunt_terminal_at(
    network: *const PioMulticonductorNetwork,
    shunt_index: usize,
    terminal_index: usize,
    output: *mut PioStringView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        write_multiconductor_terminal(
            network,
            MulticonductorTerminalTable::Shunt,
            shunt_index,
            terminal_index,
            output,
            error,
        )
    }
}

unsafe fn write_multiconductor_shunt_matrix_row(
    network: *const PioMulticonductorNetwork,
    shunt_index: usize,
    row_index: usize,
    conductance: bool,
    output: *mut PioF64View,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_multiconductor_network(network)?;
            let shunt = network.shunts().get(shunt_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("multiconductor shunt index {shunt_index} is out of range"),
                )
            })?;
            let (matrix, description) = if conductance {
                (&shunt.g, "shunt conductance matrix")
            } else {
                (&shunt.b, "shunt susceptance matrix")
            };
            *require_output(output, "output")? =
                conductor_matrix_row(matrix, row_index, description)?;
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_shunt_conductance_matrix_row_at(
    network: *const PioMulticonductorNetwork,
    shunt_index: usize,
    row_index: usize,
    output: *mut PioF64View,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        write_multiconductor_shunt_matrix_row(network, shunt_index, row_index, true, output, error)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_shunt_susceptance_matrix_row_at(
    network: *const PioMulticonductorNetwork,
    shunt_index: usize,
    row_index: usize,
    output: *mut PioF64View,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        write_multiconductor_shunt_matrix_row(network, shunt_index, row_index, false, output, error)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_capacitor_at(
    network: *const PioMulticonductorNetwork,
    index: usize,
    output: *mut PioMulticonductorCapacitorView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_multiconductor_network(network)?;
            let capacitor = network.capacitors().get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("multiconductor capacitor index {index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = PioMulticonductorCapacitorView {
                name: PioStringView::new(&capacitor.name),
                bus: PioStringView::new(&capacitor.bus),
                terminal_map_count: capacitor.terminal_map.len(),
                configuration: PioStringView::new(multiconductor_configuration_name(
                    capacitor.configuration,
                )),
                rated_reactive_power_var: capacitor.q_rated,
                nominal_voltage_v: capacitor.v_nom,
            };
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_capacitor_terminal_at(
    network: *const PioMulticonductorNetwork,
    capacitor_index: usize,
    terminal_index: usize,
    output: *mut PioStringView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        write_multiconductor_terminal(
            network,
            MulticonductorTerminalTable::Capacitor,
            capacitor_index,
            terminal_index,
            output,
            error,
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_voltage_source_at(
    network: *const PioMulticonductorNetwork,
    index: usize,
    output: *mut PioVoltageSourceView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_multiconductor_network(network)?;
            let source = network.sources().get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("voltage source index {index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = PioVoltageSourceView {
                name: PioStringView::new(&source.name),
                bus: PioStringView::new(&source.bus),
                terminal_map_count: source.terminal_map.len(),
                voltage_magnitude_v: PioF64View::new(&source.v_magnitude),
                voltage_angle_rad: PioF64View::new(&source.v_angle),
            };
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_voltage_source_terminal_at(
    network: *const PioMulticonductorNetwork,
    source_index: usize,
    terminal_index: usize,
    output: *mut PioStringView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        write_multiconductor_terminal(
            network,
            MulticonductorTerminalTable::VoltageSource,
            source_index,
            terminal_index,
            output,
            error,
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_untyped_object_at(
    network: *const PioMulticonductorNetwork,
    index: usize,
    output: *mut PioMulticonductorUntypedObjectView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_multiconductor_network(network)?;
            let object = network.untyped_objects().get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("untyped object index {index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = PioMulticonductorUntypedObjectView {
                class_name: PioStringView::new(&object.class),
                name: PioStringView::new(&object.name),
                property_count: object.props.len(),
            };
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_untyped_object_property_at(
    network: *const PioMulticonductorNetwork,
    object_index: usize,
    property_index: usize,
    output: *mut PioMulticonductorUntypedPropertyView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_multiconductor_network(network)?;
            let object = network.untyped_objects().get(object_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("untyped object index {object_index} is out of range"),
                )
            })?;
            let (name, value) = object.props.get(property_index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("untyped object property index {property_index} is out of range"),
                )
            })?;
            let (name, has_name) = optional_string_view(name.as_deref());
            *require_output(output, "output")? = PioMulticonductorUntypedPropertyView {
                name,
                has_name,
                value: PioStringView::new(value),
            };
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_command_at(
    network: *const PioMulticonductorNetwork,
    index: usize,
    output: *mut PioMulticonductorCommandView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_multiconductor_network(network)?;
            let (verb, args) = network.commands().get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("source command index {index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = PioMulticonductorCommandView {
                verb: PioStringView::new(verb),
                args: PioStringView::new(args),
            };
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_option_at(
    network: *const PioMulticonductorNetwork,
    index: usize,
    output: *mut PioStringPropertyView,
    error: *mut *mut PioError,
) -> bool {
    unsafe {
        entry(error, false, || {
            let network = require_multiconductor_network(network)?;
            let (name, value) = network.options().get(index).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("source option index {index} is out of range"),
                )
            })?;
            *require_output(output, "output")? = PioStringPropertyView {
                name: PioStringView::new(name),
                value: PioStringView::new(value),
            };
            Ok(true)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_retain(
    network: *const PioMulticonductorNetwork,
) -> *mut PioMulticonductorNetwork {
    unsafe { PioMulticonductorNetwork::retain_raw(network) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_multiconductor_network_release(
    network: *mut PioMulticonductorNetwork,
) {
    unsafe { PioMulticonductorNetwork::release_raw(network) };
}

// ---- typed updates ---------------------------------------------------------

opaque_handle!(
    /// Stable, type-qualified component identity.
    PioComponentId,
    ComponentId
);
opaque_handle!(
    /// Active power replacement with an explicit unit.
    PioActivePower,
    ActivePower
);
opaque_handle!(
    /// Reactive power replacement with an explicit unit.
    PioReactivePower,
    ReactivePower
);
opaque_handle!(
    /// Apparent power replacement with an explicit unit.
    PioApparentPower,
    ApparentPower
);
opaque_handle!(
    /// Typed update to an operating point.
    PioOperatingPointUpdate,
    OperatingPointUpdate
);
opaque_handle!(
    /// Typed update to physical network data.
    PioNetworkUpdate,
    NetworkUpdate
);
opaque_handle!(
    /// Typed update to a calculation instance.
    PioCalculationUpdate,
    CalculationUpdate
);

struct UpdateReportInner {
    changes: Vec<UpdateChange>,
    connectivity_changed: bool,
}

struct UpdateChangeInner {
    owner: Arc<UpdateReportInner>,
    index: usize,
}

impl UpdateChangeInner {
    fn change(&self) -> Option<&UpdateChange> {
        self.owner.changes.get(self.index)
    }
}

opaque_handle!(
    /// Exact changes made by one atomic update batch.
    PioUpdateReport,
    UpdateReportInner
);
opaque_handle!(
    /// Owner-rooted view of one changed component field.
    PioUpdateChange,
    UpdateChangeInner
);

unsafe fn require_handle<'a, T>(
    handle: *const HandleBox<T>,
    name: &str,
) -> Result<&'a T, *mut PioError> {
    unsafe { handle_get(handle) }.ok_or_else(|| {
        boundary_error(
            &codes::BIND_CAPI_NULL_HANDLE,
            format!("{name} must not be NULL"),
        )
    })
}

unsafe fn optional_owned_string(
    value: *const c_char,
    value_len: usize,
    name: &str,
) -> Result<Option<String>, *mut PioError> {
    unsafe { optional_str(value, value_len, name) }.map(|value| value.map(str::to_owned))
}

/// Construct a stable component identity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_component_id_new(
    component_type: *const c_char,
    component_type_len: usize,
    local_id: *const c_char,
    local_id_len: usize,
    error: *mut *mut PioError,
) -> *mut PioComponentId {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let component_type =
                required_str(component_type, component_type_len, "component_type")?;
            let local_id = required_str(local_id, local_id_len, "local_id")?;
            ComponentId::new(component_type, local_id)
                .map(PioComponentId::new_raw)
                .map_err(|failure| error_from_core(&failure))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_component_id_type(component: *const PioComponentId) -> PioStringView {
    unsafe { PioComponentId::get(component) }.map_or(PioStringView::EMPTY, |component| {
        PioStringView::new(component.component_type())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_component_id_local_id(
    component: *const PioComponentId,
) -> PioStringView {
    unsafe { PioComponentId::get(component) }.map_or(PioStringView::EMPTY, |component| {
        PioStringView::new(component.local_id())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_component_id_retain(
    component: *const PioComponentId,
) -> *mut PioComponentId {
    unsafe { PioComponentId::retain_raw(component) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_component_id_release(component: *mut PioComponentId) {
    unsafe { PioComponentId::release_raw(component) };
}

#[unsafe(no_mangle)]
pub extern "C" fn pio_active_power_from_watts(value: f64) -> *mut PioActivePower {
    PioActivePower::new_raw(ActivePower::from_watts(value))
}

#[unsafe(no_mangle)]
pub extern "C" fn pio_active_power_from_megawatts(value: f64) -> *mut PioActivePower {
    PioActivePower::new_raw(ActivePower::from_megawatts(value))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_active_power_value(power: *const PioActivePower) -> f64 {
    unsafe { PioActivePower::get(power) }.map_or(f64::NAN, |power| power.value())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_active_power_unit(power: *const PioActivePower) -> PioStringView {
    unsafe { PioActivePower::get(power) }.map_or(PioStringView::EMPTY, |power| {
        PioStringView::new(match power.unit() {
            ActivePowerUnit::Watts => "watts",
            ActivePowerUnit::Megawatts => "megawatts",
            _ => "unknown",
        })
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_active_power_retain(
    power: *const PioActivePower,
) -> *mut PioActivePower {
    unsafe { PioActivePower::retain_raw(power) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_active_power_release(power: *mut PioActivePower) {
    unsafe { PioActivePower::release_raw(power) };
}

#[unsafe(no_mangle)]
pub extern "C" fn pio_reactive_power_from_vars(value: f64) -> *mut PioReactivePower {
    PioReactivePower::new_raw(ReactivePower::from_vars(value))
}

#[unsafe(no_mangle)]
pub extern "C" fn pio_reactive_power_from_megavars(value: f64) -> *mut PioReactivePower {
    PioReactivePower::new_raw(ReactivePower::from_megavars(value))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_reactive_power_value(power: *const PioReactivePower) -> f64 {
    unsafe { PioReactivePower::get(power) }.map_or(f64::NAN, |power| power.value())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_reactive_power_unit(power: *const PioReactivePower) -> PioStringView {
    unsafe { PioReactivePower::get(power) }.map_or(PioStringView::EMPTY, |power| {
        PioStringView::new(match power.unit() {
            ReactivePowerUnit::Vars => "vars",
            ReactivePowerUnit::Megavars => "megavars",
            _ => "unknown",
        })
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_reactive_power_retain(
    power: *const PioReactivePower,
) -> *mut PioReactivePower {
    unsafe { PioReactivePower::retain_raw(power) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_reactive_power_release(power: *mut PioReactivePower) {
    unsafe { PioReactivePower::release_raw(power) };
}

#[unsafe(no_mangle)]
pub extern "C" fn pio_apparent_power_from_volt_amperes(value: f64) -> *mut PioApparentPower {
    PioApparentPower::new_raw(ApparentPower::from_volt_amperes(value))
}

#[unsafe(no_mangle)]
pub extern "C" fn pio_apparent_power_from_megavolt_amperes(value: f64) -> *mut PioApparentPower {
    PioApparentPower::new_raw(ApparentPower::from_megavolt_amperes(value))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_apparent_power_value(power: *const PioApparentPower) -> f64 {
    unsafe { PioApparentPower::get(power) }.map_or(f64::NAN, |power| power.value())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_apparent_power_unit(power: *const PioApparentPower) -> PioStringView {
    unsafe { PioApparentPower::get(power) }.map_or(PioStringView::EMPTY, |power| {
        PioStringView::new(match power.unit() {
            ApparentPowerUnit::VoltAmperes => "volt_amperes",
            ApparentPowerUnit::MegavoltAmperes => "megavolt_amperes",
            _ => "unknown",
        })
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_apparent_power_retain(
    power: *const PioApparentPower,
) -> *mut PioApparentPower {
    unsafe { PioApparentPower::retain_raw(power) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_apparent_power_release(power: *mut PioApparentPower) {
    unsafe { PioApparentPower::release_raw(power) };
}

unsafe fn component_clone(component: *const PioComponentId) -> Result<ComponentId, *mut PioError> {
    unsafe { require_handle(component.cast(), "PioComponentId") }.cloned()
}

unsafe fn active_power_copy(power: *const PioActivePower) -> Result<ActivePower, *mut PioError> {
    unsafe { require_handle(power.cast(), "PioActivePower") }.copied()
}

unsafe fn reactive_power_copy(
    power: *const PioReactivePower,
) -> Result<ReactivePower, *mut PioError> {
    unsafe { require_handle(power.cast(), "PioReactivePower") }.copied()
}

unsafe fn apparent_power_copy(
    power: *const PioApparentPower,
) -> Result<ApparentPower, *mut PioError> {
    unsafe { require_handle(power.cast(), "PioApparentPower") }.copied()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_operating_point_update_set_load_active_power(
    load: *const PioComponentId,
    terminal: *const c_char,
    terminal_len: usize,
    power: *const PioActivePower,
    error: *mut *mut PioError,
) -> *mut PioOperatingPointUpdate {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            Ok(PioOperatingPointUpdate::new_raw(
                OperatingPointUpdate::LoadActivePower {
                    load: component_clone(load)?,
                    terminal: optional_owned_string(terminal, terminal_len, "terminal")?,
                    p: active_power_copy(power)?,
                },
            ))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_operating_point_update_set_load_reactive_power(
    load: *const PioComponentId,
    terminal: *const c_char,
    terminal_len: usize,
    power: *const PioReactivePower,
    error: *mut *mut PioError,
) -> *mut PioOperatingPointUpdate {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            Ok(PioOperatingPointUpdate::new_raw(
                OperatingPointUpdate::LoadReactivePower {
                    load: component_clone(load)?,
                    terminal: optional_owned_string(terminal, terminal_len, "terminal")?,
                    q: reactive_power_copy(power)?,
                },
            ))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_operating_point_update_set_generator_active_power(
    generator: *const PioComponentId,
    terminal: *const c_char,
    terminal_len: usize,
    power: *const PioActivePower,
    error: *mut *mut PioError,
) -> *mut PioOperatingPointUpdate {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            Ok(PioOperatingPointUpdate::new_raw(
                OperatingPointUpdate::GeneratorActivePower {
                    generator: component_clone(generator)?,
                    terminal: optional_owned_string(terminal, terminal_len, "terminal")?,
                    p: active_power_copy(power)?,
                },
            ))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_operating_point_update_set_generator_reactive_power(
    generator: *const PioComponentId,
    terminal: *const c_char,
    terminal_len: usize,
    power: *const PioReactivePower,
    error: *mut *mut PioError,
) -> *mut PioOperatingPointUpdate {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            Ok(PioOperatingPointUpdate::new_raw(
                OperatingPointUpdate::GeneratorReactivePower {
                    generator: component_clone(generator)?,
                    terminal: optional_owned_string(terminal, terminal_len, "terminal")?,
                    q: reactive_power_copy(power)?,
                },
            ))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_operating_point_update_set_generator_voltage_magnitude(
    generator: *const PioComponentId,
    voltage_magnitude_per_unit: f64,
    error: *mut *mut PioError,
) -> *mut PioOperatingPointUpdate {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            Ok(PioOperatingPointUpdate::new_raw(
                OperatingPointUpdate::GeneratorVoltageMagnitude {
                    generator: component_clone(generator)?,
                    vm_pu: voltage_magnitude_per_unit,
                },
            ))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_operating_point_update_set_generator_in_service(
    generator: *const PioComponentId,
    in_service: bool,
    error: *mut *mut PioError,
) -> *mut PioOperatingPointUpdate {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            Ok(PioOperatingPointUpdate::new_raw(
                OperatingPointUpdate::GeneratorInService {
                    generator: component_clone(generator)?,
                    in_service,
                },
            ))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_operating_point_update_set_branch_in_service(
    branch: *const PioComponentId,
    in_service: bool,
    error: *mut *mut PioError,
) -> *mut PioOperatingPointUpdate {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            Ok(PioOperatingPointUpdate::new_raw(
                OperatingPointUpdate::BranchInService {
                    branch: component_clone(branch)?,
                    in_service,
                },
            ))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_operating_point_update_set_transformer_tap_ratio(
    transformer: *const PioComponentId,
    tap_ratio: f64,
    error: *mut *mut PioError,
) -> *mut PioOperatingPointUpdate {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            Ok(PioOperatingPointUpdate::new_raw(
                OperatingPointUpdate::TransformerTapRatio {
                    transformer: component_clone(transformer)?,
                    tap_ratio,
                },
            ))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_operating_point_update_set_transformer_phase_shift_degrees(
    transformer: *const PioComponentId,
    phase_shift_degrees: f64,
    error: *mut *mut PioError,
) -> *mut PioOperatingPointUpdate {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            Ok(PioOperatingPointUpdate::new_raw(
                OperatingPointUpdate::TransformerPhaseShift {
                    transformer: component_clone(transformer)?,
                    shift_degrees: phase_shift_degrees,
                },
            ))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_operating_point_update_set_switch_closed(
    switch_id: *const PioComponentId,
    closed: bool,
    error: *mut *mut PioError,
) -> *mut PioOperatingPointUpdate {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            Ok(PioOperatingPointUpdate::new_raw(
                OperatingPointUpdate::SwitchClosed {
                    switch: component_clone(switch_id)?,
                    closed,
                },
            ))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_network_update_set_branch_thermal_rating(
    branch: *const PioComponentId,
    terminal: *const c_char,
    terminal_len: usize,
    rating: *const PioApparentPower,
    error: *mut *mut PioError,
) -> *mut PioNetworkUpdate {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            Ok(PioNetworkUpdate::new_raw(
                NetworkUpdate::BranchThermalRating {
                    branch: component_clone(branch)?,
                    terminal: optional_owned_string(terminal, terminal_len, "terminal")?,
                    rating: apparent_power_copy(rating)?,
                },
            ))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_calculation_update_from_operating_point(
    update: *const PioOperatingPointUpdate,
    error: *mut *mut PioError,
) -> *mut PioCalculationUpdate {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let update = PioOperatingPointUpdate::get(update)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_NULL_HANDLE,
                        "PioOperatingPointUpdate must not be NULL",
                    )
                })?
                .clone();
            Ok(PioCalculationUpdate::new_raw(
                CalculationUpdate::OperatingPoint(update),
            ))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_calculation_update_from_network(
    update: *const PioNetworkUpdate,
    error: *mut *mut PioError,
) -> *mut PioCalculationUpdate {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let update = PioNetworkUpdate::get(update)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_NULL_HANDLE,
                        "PioNetworkUpdate must not be NULL",
                    )
                })?
                .clone();
            Ok(PioCalculationUpdate::new_raw(CalculationUpdate::Network(
                update,
            )))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_operating_point_update_retain(
    update: *const PioOperatingPointUpdate,
) -> *mut PioOperatingPointUpdate {
    unsafe { PioOperatingPointUpdate::retain_raw(update) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_operating_point_update_release(update: *mut PioOperatingPointUpdate) {
    unsafe { PioOperatingPointUpdate::release_raw(update) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_network_update_retain(
    update: *const PioNetworkUpdate,
) -> *mut PioNetworkUpdate {
    unsafe { PioNetworkUpdate::retain_raw(update) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_network_update_release(update: *mut PioNetworkUpdate) {
    unsafe { PioNetworkUpdate::release_raw(update) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_calculation_update_retain(
    update: *const PioCalculationUpdate,
) -> *mut PioCalculationUpdate {
    unsafe { PioCalculationUpdate::retain_raw(update) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_calculation_update_release(update: *mut PioCalculationUpdate) {
    unsafe { PioCalculationUpdate::release_raw(update) };
}

fn append_update_report(output: &mut UpdateReportInner, report: powerio_prob::UpdateReport) {
    output.connectivity_changed |= report.connectivity_changed();
    output.changes.extend_from_slice(report.changes());
}

fn apply_bus_load_to_typed_module<T>(
    module: &mut powerio::PioModule<PioValue>,
    bus: powerio_tx::BusId,
    total: ActivePower,
    allocation: LoadAllocation,
    narrow: impl FnOnce(PioValue) -> Result<T, PioValue>,
    wrap: impl FnOnce(T) -> PioValue,
) -> Result<UpdateReportInner, powerio_core::Error>
where
    T: powerio_prob::BalancedCalculationInstance,
{
    let mut typed = module.clone().__try_map_value(narrow).map_err(|value| {
        powerio_core::Error::new(
            &codes::REQUEST_CAPI_TYPE_MISMATCH,
            format!(
                "{} does not accept aggregate bus active demand",
                value.value().type_name()
            ),
        )
    })?;
    let report = apply_bus_load_active_power(&mut typed, bus, total, allocation)?;
    *module = typed.map_value(wrap);
    let mut output = UpdateReportInner {
        changes: Vec::new(),
        connectivity_changed: false,
    };
    append_update_report(&mut output, report);
    Ok(output)
}

// Recoverable narrowing returns the original dynamic value on a type mismatch,
// preserving the module records without serializing or cloning the value again.
#[allow(clippy::result_large_err)]
fn apply_dynamic_bus_load_active_power(
    module: &mut powerio::PioModule<PioValue>,
    bus: powerio_tx::BusId,
    total: ActivePower,
    allocation: LoadAllocation,
) -> Result<UpdateReportInner, powerio_core::Error> {
    match &module.value() {
        PioValue::DcPfInstance(_) => apply_bus_load_to_typed_module(
            module,
            bus,
            total,
            allocation,
            |value| match value {
                PioValue::DcPfInstance(instance) => Ok(instance),
                other => Err(other),
            },
            PioValue::DcPfInstance,
        ),
        PioValue::AcPfInstance(_) => apply_bus_load_to_typed_module(
            module,
            bus,
            total,
            allocation,
            |value| match value {
                PioValue::AcPfInstance(instance) => Ok(instance),
                other => Err(other),
            },
            PioValue::AcPfInstance,
        ),
        PioValue::DcOpfInstance(_) => apply_bus_load_to_typed_module(
            module,
            bus,
            total,
            allocation,
            |value| match value {
                PioValue::DcOpfInstance(instance) => Ok(instance),
                other => Err(other),
            },
            PioValue::DcOpfInstance,
        ),
        PioValue::AcOpfInstance(_) => apply_bus_load_to_typed_module(
            module,
            bus,
            total,
            allocation,
            |value| match value {
                PioValue::AcOpfInstance(instance) => Ok(instance),
                other => Err(other),
            },
            PioValue::AcOpfInstance,
        ),
        value => Err(powerio_core::Error::new(
            &codes::REQUEST_CAPI_TYPE_MISMATCH,
            format!(
                "{} does not accept aggregate bus active demand",
                value.type_name()
            ),
        )),
    }
}

fn parse_load_allocation(value: &str) -> Result<LoadAllocation, *mut PioError> {
    match value {
        "equal" => Ok(LoadAllocation::Equal),
        "proportional_to_current_active_power" => {
            Ok(LoadAllocation::ProportionalToCurrentActivePower)
        }
        other => Err(boundary_error(
            &codes::REQUEST_CAPI_ALLOCATION_UNKNOWN,
            format!("unknown load allocation rule '{other}'"),
        )),
    }
}

fn separated_updates(
    updates: &[CalculationUpdate],
) -> (Vec<OperatingPointUpdate>, Vec<NetworkUpdate>) {
    let mut operating = Vec::new();
    let mut network = Vec::new();
    for update in updates {
        match update {
            CalculationUpdate::OperatingPoint(update) => operating.push(update.clone()),
            CalculationUpdate::Network(update) => network.push(update.clone()),
            _ => unreachable!("unsupported calculation update from this PowerIO build"),
        }
    }
    (operating, network)
}

fn reject_network_updates(
    updates: &[CalculationUpdate],
) -> Result<Vec<OperatingPointUpdate>, powerio_core::Error> {
    updates
        .iter()
        .map(|update| match update {
            CalculationUpdate::OperatingPoint(update) => Ok(update.clone()),
            CalculationUpdate::Network(_) => Err(powerio_core::Error::new(
                &codes::REQUEST_CAPI_TYPE_MISMATCH,
                "a network update cannot be applied to an operating point",
            )),
            _ => unreachable!("unsupported calculation update from this PowerIO build"),
        })
        .collect()
}

fn apply_dynamic_updates(
    value: &mut PioValue,
    updates: &[CalculationUpdate],
) -> Result<UpdateReportInner, powerio_core::Error> {
    let mut output = UpdateReportInner {
        changes: Vec::new(),
        connectivity_changed: false,
    };
    match value {
        PioValue::BalancedNetwork(network) => {
            let (operating, physical) = separated_updates(updates);
            append_update_report(&mut output, apply_updates(network, &operating)?);
            append_update_report(&mut output, apply_updates(network, &physical)?);
        }
        PioValue::MulticonductorNetwork(network) => {
            let (operating, physical) = separated_updates(updates);
            append_update_report(&mut output, apply_updates(network, &operating)?);
            append_update_report(&mut output, apply_updates(network, &physical)?);
        }
        PioValue::BalancedOperatingPoint(point) => {
            let operating = reject_network_updates(updates)?;
            append_update_report(&mut output, apply_updates(point, &operating)?);
        }
        PioValue::MulticonductorOperatingPoint(point) => {
            let operating = reject_network_updates(updates)?;
            append_update_report(&mut output, apply_updates(point, &operating)?);
        }
        PioValue::DcPfInstance(instance) => {
            append_update_report(&mut output, apply_updates(instance, updates)?);
        }
        PioValue::AcPfInstance(instance) => {
            append_update_report(&mut output, apply_updates(instance, updates)?);
        }
        PioValue::DcOpfInstance(instance) => {
            append_update_report(&mut output, apply_updates(instance, updates)?);
        }
        PioValue::AcOpfInstance(instance) => {
            append_update_report(&mut output, apply_updates(instance, updates)?);
        }
        PioValue::McAcPfInstance(instance) => {
            append_update_report(&mut output, apply_updates(instance, updates)?);
        }
        PioValue::McAcOpfInstance(instance) => {
            append_update_report(&mut output, apply_updates(instance, updates)?);
        }
        _ => {
            return Err(powerio_core::Error::new(
                &codes::REQUEST_CAPI_TYPE_MISMATCH,
                format!("{} does not accept calculation updates", value.type_name()),
            ));
        }
    }
    Ok(output)
}

fn update_history_id(
    module: &powerio::PioModule<PioValue>,
) -> Result<HistoryId, powerio_core::Error> {
    let mut suffix = 1usize;
    loop {
        let value = if suffix == 1 {
            "apply-updates".to_owned()
        } else {
            format!("apply-updates-{suffix}")
        };
        if module
            .history()
            .iter()
            .all(|entry| entry.id().as_str() != value)
        {
            return HistoryId::new(value);
        }
        suffix += 1;
    }
}

fn apply_module_updates(
    module: &mut powerio::PioModule<PioValue>,
    updates: &[CalculationUpdate],
) -> Result<UpdateReportInner, powerio_core::Error> {
    let history_id = update_history_id(module)?;
    let mut edit = module.stage_edit();
    let report = apply_dynamic_updates(edit.value_mut(), updates)?;
    if report.changes.is_empty() {
        return Ok(report);
    }

    let mut parameters = std::collections::BTreeMap::new();
    parameters.insert(
        "updates".to_owned(),
        serde_json::to_value(updates).map_err(|failure| {
            powerio_core::Error::new(
                &codes::EMIT_CAPI_SERIALIZE_FAILED,
                format!("cannot record applied updates: {failure}"),
            )
        })?,
    );
    parameters.insert(
        "changes".to_owned(),
        serde_json::to_value(&report.changes).map_err(|failure| {
            powerio_core::Error::new(
                &codes::EMIT_CAPI_SERIALIZE_FAILED,
                format!("cannot record update changes: {failure}"),
            )
        })?,
    );
    parameters.insert(
        "connectivity_changed".to_owned(),
        serde_json::Value::Bool(report.connectivity_changed),
    );
    let history = HistoryEntry::new(history_id, HistoryKind::Edit, "apply_updates")?
        .with_parameters(parameters)?;
    edit.commit(Producer::new("powerio-capi", powerio::VERSION)?, history)?;
    Ok(report)
}

unsafe fn module_make_mut<'a>(
    module: *mut PioModule,
) -> Result<&'a mut ModuleInner, *mut PioError> {
    let handle = unsafe { module.cast::<HandleBox<ModuleInner>>().as_mut() }.ok_or_else(|| {
        boundary_error(&codes::BIND_CAPI_NULL_HANDLE, "PioModule must not be NULL")
    })?;
    Ok(Arc::make_mut(&mut handle.inner))
}

/// Apply a complete typed update batch atomically.
///
/// Owner rooted handles obtained before the call (values, networks, collection
/// entries, artifacts) keep the pre-update module alive. Plain view structs read
/// from this module handle (`PioStringView`, `PioModuleSourceView`, history and
/// source map views) are invalidated by a successful call and must be read
/// again. The caller must hold exclusive access to `module` for the duration of
/// the call: no concurrent call of any kind on this handle, including retain.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_apply_updates(
    module: *mut PioModule,
    updates: *const *const PioCalculationUpdate,
    updates_len: usize,
    error: *mut *mut PioError,
) -> *mut PioUpdateReport {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            if updates.is_null() && updates_len != 0 {
                return Err(boundary_error(
                    &codes::BIND_CAPI_NULL_ARGUMENT,
                    "updates is NULL with a nonzero length",
                ));
            }
            let handles = if updates_len == 0 {
                &[]
            } else {
                std::slice::from_raw_parts(updates, updates_len)
            };
            let mut values = Vec::with_capacity(handles.len());
            for (index, handle) in handles.iter().copied().enumerate() {
                let update = PioCalculationUpdate::get(handle).ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_NULL_HANDLE,
                        format!("updates[{index}] must not be NULL"),
                    )
                })?;
                values.push(update.clone());
            }
            let module = module_make_mut(module)?;
            apply_module_updates(&mut module.module, &values)
                .map(PioUpdateReport::new_raw)
                .map_err(|failure| error_from_core(&failure))
        })
    }
}

/// Replace aggregate active demand at one bus using the named allocation rule.
///
/// The view invalidation and exclusivity contract of `pio_apply_updates`
/// applies.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_apply_bus_load_active_power(
    module: *mut PioModule,
    bus_id: usize,
    power: *const PioActivePower,
    allocation: *const c_char,
    allocation_len: usize,
    error: *mut *mut PioError,
) -> *mut PioUpdateReport {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let total = active_power_copy(power)?;
            let allocation = required_str(allocation, allocation_len, "allocation")?;
            let allocation = parse_load_allocation(allocation)?;
            let module = module_make_mut(module)?;
            apply_dynamic_bus_load_active_power(
                &mut module.module,
                powerio_tx::BusId::new(bus_id),
                total,
                allocation,
            )
            .map(PioUpdateReport::new_raw)
            .map_err(|failure| error_from_core(&failure))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_update_report_len(report: *const PioUpdateReport) -> usize {
    unsafe { PioUpdateReport::get(report) }.map_or(0, |report| report.changes.len())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_update_report_connectivity_changed(
    report: *const PioUpdateReport,
) -> bool {
    unsafe { PioUpdateReport::get(report) }.is_some_and(|report| report.connectivity_changed)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_update_report_change(
    report: *const PioUpdateReport,
    index: usize,
    error: *mut *mut PioError,
) -> *mut PioUpdateChange {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let owner = PioUpdateReport::arc(report).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_NULL_HANDLE,
                    "PioUpdateReport must not be NULL",
                )
            })?;
            if index >= owner.changes.len() {
                return Err(boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("update change index {index} is out of range"),
                ));
            }
            Ok(PioUpdateChange::new_raw(UpdateChangeInner { owner, index }))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_update_change_component_id(
    change: *const PioUpdateChange,
) -> *mut PioComponentId {
    unsafe { PioUpdateChange::get(change) }
        .and_then(UpdateChangeInner::change)
        .map_or(std::ptr::null_mut(), |change| {
            PioComponentId::new_raw(change.component_id().clone())
        })
}

fn updated_field_name(field: UpdatedField) -> &'static str {
    match field {
        UpdatedField::LoadActivePower => "load_active_power",
        UpdatedField::LoadReactivePower => "load_reactive_power",
        UpdatedField::GeneratorActivePower => "generator_active_power",
        UpdatedField::GeneratorReactivePower => "generator_reactive_power",
        UpdatedField::GeneratorVoltageMagnitude => "generator_voltage_magnitude",
        UpdatedField::GeneratorInService => "generator_in_service",
        UpdatedField::BranchThermalRating => "branch_thermal_rating",
        UpdatedField::BranchInService => "branch_in_service",
        UpdatedField::TransformerTapRatio => "transformer_tap_ratio",
        UpdatedField::TransformerPhaseShift => "transformer_phase_shift",
        UpdatedField::SwitchClosed => "switch_closed",
        _ => "unknown",
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_update_change_field(change: *const PioUpdateChange) -> PioStringView {
    unsafe { PioUpdateChange::get(change) }
        .and_then(UpdateChangeInner::change)
        .map_or(PioStringView::EMPTY, |change| {
            PioStringView::new(updated_field_name(change.field()))
        })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_update_change_terminal(
    change: *const PioUpdateChange,
) -> PioStringView {
    unsafe { PioUpdateChange::get(change) }
        .and_then(UpdateChangeInner::change)
        .and_then(UpdateChange::terminal)
        .map_or(PioStringView::EMPTY, PioStringView::new)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_update_report_retain(
    report: *const PioUpdateReport,
) -> *mut PioUpdateReport {
    unsafe { PioUpdateReport::retain_raw(report) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_update_report_release(report: *mut PioUpdateReport) {
    unsafe { PioUpdateReport::release_raw(report) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_update_change_retain(
    change: *const PioUpdateChange,
) -> *mut PioUpdateChange {
    unsafe { PioUpdateChange::retain_raw(change) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_update_change_release(change: *mut PioUpdateChange) {
    unsafe { PioUpdateChange::release_raw(change) };
}

// ---- emission --------------------------------------------------------------

struct ArtifactRecord {
    name: String,
    bytes: Option<Vec<u8>>,
}

struct EmitResultInner {
    layout: &'static str,
    fidelity: &'static str,
    artifacts: Vec<ArtifactRecord>,
    diagnostics: Arc<DiagnosticsInner>,
}

struct ArtifactInner {
    owner: Arc<EmitResultInner>,
    index: usize,
}

impl ArtifactInner {
    fn artifact(&self) -> Option<&ArtifactRecord> {
        self.owner.artifacts.get(self.index)
    }
}

opaque_handle!(
    /// Completed artifact inventory and emission diagnostics.
    PioEmitResult,
    EmitResultInner
);
opaque_handle!(
    /// Owner-rooted emitted artifact.
    PioArtifact,
    ArtifactInner
);

fn emit_result_handle(result: EmitResult) -> *mut PioEmitResult {
    let layout = match result.layout() {
        powerio::OutputLayout::File => "file",
        powerio::OutputLayout::Directory => "directory",
    };
    let fidelity = match result.fidelity() {
        powerio::Fidelity::ExactSameFormat => "exact_same_format",
        powerio::Fidelity::Canonical => "canonical",
    };
    let diagnostics = Arc::new(DiagnosticsInner {
        owner: DiagnosticsOwner::Owned(result.diagnostics().to_vec()),
    });
    let artifacts = match result.into_output() {
        EmittedOutput::Memory { artifacts } => artifacts
            .into_iter()
            .map(|artifact| {
                let name = artifact.name().as_str().to_owned();
                ArtifactRecord {
                    name,
                    bytes: Some(artifact.into_bytes()),
                }
            })
            .collect(),
        EmittedOutput::Path { artifacts, .. } => artifacts
            .into_iter()
            .map(|path| ArtifactRecord {
                name: path.to_string_lossy().into_owned(),
                bytes: None,
            })
            .collect(),
        _ => unreachable!("unsupported emitted output from this PowerIO build"),
    };
    PioEmitResult::new_raw(EmitResultInner {
        layout,
        fidelity,
        artifacts,
        diagnostics,
    })
}

unsafe fn run_output_operation(
    module: *const PioModule,
    destination: *const PioDestination,
    error: *mut *mut PioError,
    operation: impl FnOnce(
        &powerio::PioModule<PioValue>,
        Destination,
    ) -> Result<EmitResult, *mut PioError>,
) -> *mut PioEmitResult {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let module = PioModule::get(module).ok_or_else(|| {
                boundary_error(&codes::BIND_CAPI_NULL_HANDLE, "PioModule must not be NULL")
            })?;
            let destination = PioDestination::get(destination).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_NULL_HANDLE,
                    "PioDestination must not be NULL",
                )
            })?;
            let destination = destination
                .build()
                .map_err(|failure| error_from_core(&failure))?;
            operation(&module.module, destination).map(emit_result_handle)
        })
    }
}

/// Emit one module as a grid exchange format.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_emit(
    module: *const PioModule,
    format: *const c_char,
    format_len: usize,
    destination: *const PioDestination,
    error: *mut *mut PioError,
) -> *mut PioEmitResult {
    unsafe {
        run_output_operation(module, destination, error, |module, destination| {
            let format = required_str(format, format_len, "format")?;
            powerio::emit(module, format, destination).map_err(|failure| error_from_core(&failure))
        })
    }
}

/// Serialize one module as PowerIO IR.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_serialize(
    module: *const PioModule,
    destination: *const PioDestination,
    error: *mut *mut PioError,
) -> *mut PioEmitResult {
    unsafe {
        run_output_operation(module, destination, error, |module, destination| {
            powerio::serialize(module, destination).map_err(|failure| error_from_core(&failure))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_emit_result_layout(result: *const PioEmitResult) -> PioStringView {
    unsafe { PioEmitResult::get(result) }.map_or(PioStringView::EMPTY, |result| {
        PioStringView::new(result.layout)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_emit_result_fidelity(result: *const PioEmitResult) -> PioStringView {
    unsafe { PioEmitResult::get(result) }.map_or(PioStringView::EMPTY, |result| {
        PioStringView::new(result.fidelity)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_emit_result_artifact_count(result: *const PioEmitResult) -> usize {
    unsafe { PioEmitResult::get(result) }.map_or(0, |result| result.artifacts.len())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_emit_result_artifact(
    result: *const PioEmitResult,
    index: usize,
    error: *mut *mut PioError,
) -> *mut PioArtifact {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let owner = PioEmitResult::arc(result).ok_or_else(|| {
                boundary_error(
                    &codes::BIND_CAPI_NULL_HANDLE,
                    "PioEmitResult must not be NULL",
                )
            })?;
            if index >= owner.artifacts.len() {
                return Err(boundary_error(
                    &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                    format!("artifact index {index} is out of range"),
                ));
            }
            Ok(PioArtifact::new_raw(ArtifactInner { owner, index }))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_emit_result_diagnostics(
    result: *const PioEmitResult,
) -> *mut PioDiagnostics {
    unsafe { PioEmitResult::get(result) }.map_or(std::ptr::null_mut(), |result| {
        PioDiagnostics::from_arc(Arc::clone(&result.diagnostics))
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_emit_result_retain(
    result: *const PioEmitResult,
) -> *mut PioEmitResult {
    unsafe { PioEmitResult::retain_raw(result) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_emit_result_release(result: *mut PioEmitResult) {
    unsafe { PioEmitResult::release_raw(result) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_artifact_name(artifact: *const PioArtifact) -> PioStringView {
    unsafe { PioArtifact::get(artifact) }
        .and_then(ArtifactInner::artifact)
        .map_or(PioStringView::EMPTY, |artifact| {
            PioStringView::new(&artifact.name)
        })
}

/// Return emitted memory bytes. A path destination has no memory bytes and
/// returns an empty view.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_artifact_bytes(artifact: *const PioArtifact) -> PioByteView {
    unsafe { PioArtifact::get(artifact) }
        .and_then(ArtifactInner::artifact)
        .and_then(|artifact| artifact.bytes.as_deref())
        .map_or(PioByteView::EMPTY, PioByteView::new)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_artifact_retain(artifact: *const PioArtifact) -> *mut PioArtifact {
    unsafe { PioArtifact::retain_raw(artifact) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_artifact_release(artifact: *mut PioArtifact) {
    unsafe { PioArtifact::release_raw(artifact) };
}

// ---- sparse matrices and vectors ------------------------------------------

struct SparseMatrixInner {
    rows: usize,
    columns: usize,
    row_offsets: Vec<usize>,
    column_indices: Vec<usize>,
    values: Vec<f64>,
}

impl From<SparseMatrix> for SparseMatrixInner {
    fn from(matrix: SparseMatrix) -> Self {
        let matrix = matrix.to_csr();
        Self {
            rows: matrix.rows(),
            columns: matrix.cols(),
            row_offsets: matrix.indptr().raw_storage().to_vec(),
            column_indices: matrix.indices().to_vec(),
            values: matrix.data().to_vec(),
        }
    }
}

struct VectorInner {
    values: Vec<f64>,
}

opaque_handle!(
    /// Immutable CSR sparse matrix.
    PioSparseMatrix,
    SparseMatrixInner
);
opaque_handle!(
    /// Immutable dense vector.
    PioVector,
    VectorInner
);

fn termination_name(termination: &powerio_prob::Termination) -> &'static str {
    use powerio_prob::Termination;
    match termination {
        Termination::Converged => "converged",
        Termination::IterationLimit => "iteration_limit",
        Termination::Infeasible => "infeasible",
        Termination::Unbounded => "unbounded",
        Termination::Failed => "failed",
        Termination::NotReported => "not_reported",
        _ => "not_reported",
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_calculation_solution_termination(
    solution: *const PioCalculationSolution,
) -> PioStringView {
    let Some(solution) =
        (unsafe { PioCalculationSolution::get(solution) }).and_then(ValueInner::value)
    else {
        return PioStringView::EMPTY;
    };
    let termination = match solution {
        PioValue::DcPfSolution(solution) => solution.termination(),
        PioValue::AcPfSolution(solution) => solution.termination(),
        PioValue::DcOpfSolution(solution) => solution.termination(),
        PioValue::AcOpfSolution(solution) => solution.termination(),
        PioValue::SocwrOpfSolution(solution) => solution.termination(),
        PioValue::McAcPfSolution(solution) => solution.termination(),
        PioValue::McAcOpfSolution(solution) => solution.termination(),
        PioValue::AcScucSolution(solution) => solution.termination(),
        _ => return PioStringView::EMPTY,
    };
    PioStringView::new(termination_name(termination))
}

/// Return an OPF or SCUC objective. SOCWR reports a lower bound through
/// pio_socwr_opf_solution_get_objective_lower_bound instead.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_calculation_solution_get_objective(
    solution: *const PioCalculationSolution,
    out_objective: *mut f64,
) -> bool {
    if out_objective.is_null() {
        return false;
    }
    // No error slot: a missing objective is an ordinary false. `entry` still
    // turns a panic into false instead of aborting the caller.
    unsafe {
        entry(std::ptr::null_mut(), false, || {
            let Some(solution) = PioCalculationSolution::get(solution).and_then(ValueInner::value)
            else {
                return Ok(false);
            };
            let objective = match solution {
                PioValue::DcOpfSolution(solution) => Some(solution.objective()),
                PioValue::AcOpfSolution(solution) => Some(solution.objective()),
                PioValue::McAcOpfSolution(solution) => Some(solution.objective()),
                PioValue::AcScucSolution(solution) => solution.objective(),
                _ => None,
            };
            Ok(match objective {
                Some(objective) => {
                    *out_objective = objective;
                    true
                }
                None => false,
            })
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_socwr_opf_solution_get_objective_lower_bound(
    solution: *const PioCalculationSolution,
    out_lower_bound: *mut f64,
) -> bool {
    if out_lower_bound.is_null() {
        return false;
    }
    // No error slot, as for pio_calculation_solution_get_objective.
    unsafe {
        entry(std::ptr::null_mut(), false, || {
            let Some(PioValue::SocwrOpfSolution(solution)) =
                PioCalculationSolution::get(solution).and_then(ValueInner::value)
            else {
                return Ok(false);
            };
            *out_lower_bound = solution.objective_lower_bound();
            Ok(true)
        })
    }
}

fn balanced_branch_identities(network: &BalancedNetwork) -> impl Iterator<Item = &str> {
    network
        .branches()
        .iter()
        .map(|branch| branch.uid.as_deref().expect("facade assigns component IDs"))
}

fn balanced_generator_identities(network: &BalancedNetwork) -> impl Iterator<Item = &str> {
    network.generators().iter().map(|generator| {
        generator
            .uid
            .as_deref()
            .expect("facade assigns component IDs")
    })
}

fn collect_solution_values(solution: &PioValue, quantity: &str) -> Result<Vec<f64>, *mut PioError> {
    let unknown = || {
        boundary_error(
            &codes::REQUEST_CAPI_QUANTITY_UNKNOWN,
            format!(
                "{} does not define solution quantity '{quantity}'",
                solution.type_name()
            ),
        )
    };
    let values = match solution {
        PioValue::DcPfSolution(solution) => match quantity {
            "bus_voltage_angle" => solution.bus_voltage_angles().to_vec(),
            "bus_active_injection" => solution.bus_active_injections().to_vec(),
            "branch_from_active_flow" => solution.branch_from_active_flows().to_vec(),
            "branch_to_active_flow" => solution.branch_to_active_flows().to_vec(),
            "generator_active_power" => solution
                .generator_dispatch()
                .map(|dispatch| dispatch.p_mw.clone())
                .ok_or_else(unknown)?,
            "generator_reactive_power" => solution
                .generator_dispatch()
                .map(|dispatch| dispatch.q_mvar.clone())
                .ok_or_else(unknown)?,
            _ => return Err(unknown()),
        },
        PioValue::AcPfSolution(solution) => match quantity {
            "bus_voltage_magnitude" => solution
                .network()
                .buses()
                .iter()
                .map(|bus| {
                    solution
                        .bus_voltage_magnitude(bus.id)
                        .expect("complete solution")
                })
                .collect(),
            "bus_voltage_angle" => solution
                .network()
                .buses()
                .iter()
                .map(|bus| {
                    solution
                        .bus_voltage_angle(bus.id)
                        .expect("complete solution")
                })
                .collect(),
            "bus_active_injection" => solution
                .network()
                .buses()
                .iter()
                .map(|bus| {
                    solution
                        .bus_active_injection(bus.id)
                        .expect("complete solution")
                })
                .collect(),
            "bus_reactive_injection" => solution
                .network()
                .buses()
                .iter()
                .map(|bus| {
                    solution
                        .bus_reactive_injection(bus.id)
                        .expect("complete solution")
                })
                .collect(),
            "branch_from_active_flow" => balanced_branch_identities(solution.network())
                .map(|id| {
                    solution
                        .branch_from_active_flow(id)
                        .expect("complete solution")
                })
                .collect(),
            "branch_from_reactive_flow" => balanced_branch_identities(solution.network())
                .map(|id| {
                    solution
                        .branch_from_reactive_flow(id)
                        .expect("complete solution")
                })
                .collect(),
            "branch_to_active_flow" => balanced_branch_identities(solution.network())
                .map(|id| {
                    solution
                        .branch_to_active_flow(id)
                        .expect("complete solution")
                })
                .collect(),
            "branch_to_reactive_flow" => balanced_branch_identities(solution.network())
                .map(|id| {
                    solution
                        .branch_to_reactive_flow(id)
                        .expect("complete solution")
                })
                .collect(),
            "generator_active_power" => solution
                .generator_dispatch()
                .map(|dispatch| dispatch.p_mw.clone())
                .ok_or_else(unknown)?,
            "generator_reactive_power" => solution
                .generator_dispatch()
                .map(|dispatch| dispatch.q_mvar.clone())
                .ok_or_else(unknown)?,
            _ => return Err(unknown()),
        },
        PioValue::DcOpfSolution(solution) => match quantity {
            "bus_voltage_angle" => solution
                .network()
                .buses()
                .iter()
                .map(|bus| {
                    solution
                        .bus_voltage_angle(bus.id)
                        .expect("complete solution")
                })
                .collect(),
            "bus_active_injection" => solution
                .network()
                .buses()
                .iter()
                .map(|bus| {
                    solution
                        .bus_active_injection(bus.id)
                        .expect("complete solution")
                })
                .collect(),
            "branch_from_active_flow" => balanced_branch_identities(solution.network())
                .map(|id| {
                    solution
                        .branch_from_active_flow(id)
                        .expect("complete solution")
                })
                .collect(),
            "branch_to_active_flow" => balanced_branch_identities(solution.network())
                .map(|id| {
                    solution
                        .branch_to_active_flow(id)
                        .expect("complete solution")
                })
                .collect(),
            "generator_active_power" => balanced_generator_identities(solution.network())
                .map(|id| {
                    solution
                        .generator_active_power(id)
                        .expect("complete solution")
                })
                .collect(),
            "bus_active_power_marginal" => solution
                .bus_active_power_marginals()
                .map(<[f64]>::to_vec)
                .ok_or_else(unknown)?,
            "branch_from_limit_multiplier" => solution
                .branch_from_limit_multipliers()
                .map(<[f64]>::to_vec)
                .ok_or_else(unknown)?,
            "branch_to_limit_multiplier" => solution
                .branch_to_limit_multipliers()
                .map(<[f64]>::to_vec)
                .ok_or_else(unknown)?,
            _ => return Err(unknown()),
        },
        PioValue::AcOpfSolution(solution) => match quantity {
            "bus_voltage_magnitude" => solution.bus_voltage_magnitudes().to_vec(),
            "bus_voltage_angle" => solution.bus_voltage_angles().to_vec(),
            "bus_active_injection" => solution.bus_active_injections().to_vec(),
            "bus_reactive_injection" => solution.bus_reactive_injections().to_vec(),
            "branch_from_active_flow" => solution.branch_from_active_flows().to_vec(),
            "branch_from_reactive_flow" => solution.branch_from_reactive_flows().to_vec(),
            "branch_to_active_flow" => solution.branch_to_active_flows().to_vec(),
            "branch_to_reactive_flow" => solution.branch_to_reactive_flows().to_vec(),
            "generator_active_power" => solution.generator_active_powers().to_vec(),
            "generator_reactive_power" => solution.generator_reactive_powers().to_vec(),
            "bus_active_power_marginal" => solution
                .bus_active_power_marginals()
                .map(<[f64]>::to_vec)
                .ok_or_else(unknown)?,
            "bus_reactive_power_marginal" => solution
                .bus_reactive_power_marginals()
                .map(<[f64]>::to_vec)
                .ok_or_else(unknown)?,
            "branch_from_limit_multiplier" => solution
                .branch_from_limit_multipliers()
                .map(<[f64]>::to_vec)
                .ok_or_else(unknown)?,
            "branch_to_limit_multiplier" => solution
                .branch_to_limit_multipliers()
                .map(<[f64]>::to_vec)
                .ok_or_else(unknown)?,
            _ => return Err(unknown()),
        },
        PioValue::SocwrOpfSolution(solution) => {
            let values = solution.values();
            match quantity {
                "bus_voltage_magnitude_squared" => values.bus_voltage_magnitude_squared.clone(),
                "branch_voltage_product_real" => values.branch_voltage_product_real.clone(),
                "branch_voltage_product_imaginary" => {
                    values.branch_voltage_product_imaginary.clone()
                }
                "generator_active_power" => values.generator_active_power.clone(),
                "generator_reactive_power" => values.generator_reactive_power.clone(),
                "branch_from_active_power" => values.branch_from_active_power.clone(),
                "branch_from_reactive_power" => values.branch_from_reactive_power.clone(),
                "branch_to_active_power" => values.branch_to_active_power.clone(),
                "branch_to_reactive_power" => values.branch_to_reactive_power.clone(),
                _ => return Err(unknown()),
            }
        }
        PioValue::McAcPfSolution(solution) => match quantity {
            "terminal_voltage_magnitude" => solution
                .network()
                .buses()
                .iter()
                .flat_map(|bus| {
                    bus.terminals.iter().map(move |terminal| {
                        solution
                            .terminal_voltage_magnitude(&bus.id, terminal)
                            .expect("complete solution")
                    })
                })
                .collect(),
            "terminal_voltage_angle" => solution
                .network()
                .buses()
                .iter()
                .flat_map(|bus| {
                    bus.terminals.iter().map(move |terminal| {
                        solution
                            .terminal_voltage_angle(&bus.id, terminal)
                            .expect("complete solution")
                    })
                })
                .collect(),
            "source_active_injection" => solution.source_active_injections().to_vec(),
            _ => return Err(unknown()),
        },
        PioValue::McAcOpfSolution(solution) => match quantity {
            "terminal_voltage_magnitude" => solution
                .network()
                .buses()
                .iter()
                .flat_map(|bus| {
                    bus.terminals.iter().map(move |terminal| {
                        solution
                            .terminal_voltage_magnitude(&bus.id, terminal)
                            .expect("complete solution")
                    })
                })
                .collect(),
            "terminal_voltage_angle" => solution
                .network()
                .buses()
                .iter()
                .flat_map(|bus| {
                    bus.terminals.iter().map(move |terminal| {
                        solution
                            .terminal_voltage_angle(&bus.id, terminal)
                            .expect("complete solution")
                    })
                })
                .collect(),
            "source_active_injection" => solution.source_active_injections().to_vec(),
            "generator_active_power" => solution.generator_active_powers().to_vec(),
            _ => return Err(unknown()),
        },
        _ => {
            return Err(boundary_error(
                &codes::REQUEST_CAPI_TYPE_MISMATCH,
                "the handle does not refer to a supported calculation solution",
            ));
        }
    };
    Ok(values)
}

/// Copy one named solution quantity into an independently owned vector.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_calculation_solution_get_values(
    solution: *const PioCalculationSolution,
    quantity: *const c_char,
    quantity_len: usize,
    error: *mut *mut PioError,
) -> *mut PioVector {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let quantity = required_str(quantity, quantity_len, "quantity")?;
            let solution = PioCalculationSolution::get(solution)
                .and_then(ValueInner::value)
                .ok_or_else(|| {
                    boundary_error(
                        &codes::BIND_CAPI_NULL_HANDLE,
                        "PioCalculationSolution must not be NULL",
                    )
                })?;
            collect_solution_values(solution, quantity)
                .map(|values| PioVector::new_raw(VectorInner { values }))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_scuc_solution_time_count(
    solution: *const PioCalculationSolution,
) -> usize {
    let Some(PioValue::AcScucSolution(solution)) =
        (unsafe { PioCalculationSolution::get(solution) }).and_then(ValueInner::value)
    else {
        return 0;
    };
    solution.instance().inputs().interval_durations.len()
}

/// Copy one AC SCUC output row for one time position into an owned vector.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_ac_scuc_solution_get_values_at(
    solution: *const PioCalculationSolution,
    quantity: *const c_char,
    quantity_len: usize,
    time_index: usize,
    error: *mut *mut PioError,
) -> *mut PioVector {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let quantity = required_str(quantity, quantity_len, "quantity")?;
            let Some(PioValue::AcScucSolution(solution)) =
                PioCalculationSolution::get(solution).and_then(ValueInner::value)
            else {
                return Err(boundary_error(
                    &codes::REQUEST_CAPI_TYPE_MISMATCH,
                    "the handle does not refer to powerio.AcScucSolution",
                ));
            };
            let network = solution.network_outputs();
            let device = solution.device_outputs();
            macro_rules! row {
                ($rows:expr, $convert:expr) => {{
                    $rows
                        .get(time_index)
                        .ok_or_else(|| {
                            boundary_error(
                                &codes::BIND_CAPI_INDEX_OUT_OF_RANGE,
                                format!("AC SCUC time index {time_index} is out of range"),
                            )
                        })?
                        .iter()
                        .copied()
                        .map($convert)
                        .collect::<Vec<f64>>()
                }};
            }
            let values = match quantity {
                "bus_voltage_magnitude" => row!(&network.bus_vm, |value| value),
                "bus_voltage_angle" => row!(&network.bus_va, |value| value),
                "shunt_step" => row!(&network.shunt_step, |value| value as f64),
                "ac_line_on_status" => row!(&network.ac_line_on_status, f64::from),
                "transformer_tap_ratio" => row!(&network.transformer_tm, |value| value),
                "transformer_phase_shift" => row!(&network.transformer_ta, |value| value),
                "transformer_on_status" => row!(&network.transformer_on_status, f64::from),
                "dc_line_from_active_power" => row!(&network.dc_line_pdc_fr, |value| value),
                "dc_line_from_reactive_power" => row!(&network.dc_line_qdc_fr, |value| value),
                "dc_line_to_reactive_power" => row!(&network.dc_line_qdc_to, |value| value),
                "device_on_status" => row!(&device.on_status, f64::from),
                "device_startup_status" => row!(&device.startup_status, f64::from),
                "device_shutdown_status" => row!(&device.shutdown_status, f64::from),
                "device_active_power" => row!(&device.p_on, |value| value),
                "device_reactive_power" => row!(&device.q, |value| value),
                "regulation_reserve_up" => row!(&device.p_reg_res_up, |value| value),
                "regulation_reserve_down" => row!(&device.p_reg_res_down, |value| value),
                "synchronized_reserve" => row!(&device.p_syn_res, |value| value),
                "nonsynchronized_reserve" => row!(&device.p_nsyn_res, |value| value),
                "ramping_reserve_up_online" => row!(&device.p_ramp_res_up_online, |value| value),
                "ramping_reserve_up_offline" => {
                    row!(&device.p_ramp_res_up_offline, |value| value)
                }
                "ramping_reserve_down_online" => {
                    row!(&device.p_ramp_res_down_online, |value| value)
                }
                "ramping_reserve_down_offline" => {
                    row!(&device.p_ramp_res_down_offline, |value| value)
                }
                "reactive_reserve_up" => row!(&device.q_res_up, |value| value),
                "reactive_reserve_down" => row!(&device.q_res_down, |value| value),
                _ => {
                    return Err(boundary_error(
                        &codes::REQUEST_CAPI_QUANTITY_UNKNOWN,
                        format!("unknown AC SCUC solution quantity '{quantity}'"),
                    ));
                }
            };
            Ok(PioVector::new_raw(VectorInner { values }))
        })
    }
}

fn formula(name: Option<&str>) -> Result<BranchSusceptanceFormula, *mut PioError> {
    match name.unwrap_or("series_susceptance") {
        "series_susceptance" => Ok(BranchSusceptanceFormula::SeriesSusceptance),
        "tap_adjusted_reactance" => Ok(BranchSusceptanceFormula::TapAdjustedReactance),
        "reactance_only" => Ok(BranchSusceptanceFormula::ReactanceOnly),
        other => Err(boundary_error(
            &codes::REQUEST_CAPI_UNKNOWN_FORMULA,
            format!("unknown branch susceptance formula '{other}'"),
        )),
    }
}

unsafe fn dc_operators(
    network: *const PioBalancedNetwork,
    formula_name: *const c_char,
    formula_name_len: usize,
) -> Result<DcOperators, *mut PioError> {
    let network = unsafe { PioBalancedNetwork::get(network) }
        .and_then(BalancedNetworkInner::network)
        .ok_or_else(|| {
            boundary_error(
                &codes::BIND_CAPI_NULL_HANDLE,
                "PioBalancedNetwork must not be NULL",
            )
        })?;
    let formula_name = unsafe { optional_str(formula_name, formula_name_len, "formula") }?;
    let formula = formula(formula_name)?;
    let instance = DcPfInstance::from_network(network.clone())
        .map_err(|failure| error_from_core(&failure))?
        .with_branch_susceptance_formula(formula);
    DcOperators::build(&instance).map_err(|failure| error_from_core(&failure))
}

unsafe fn dc_matrix(
    network: *const PioBalancedNetwork,
    formula: *const c_char,
    formula_len: usize,
    error: *mut *mut PioError,
    calculation: impl FnOnce(&DcOperators) -> SparseMatrix,
) -> *mut PioSparseMatrix {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let operators = dc_operators(network, formula, formula_len)?;
            Ok(PioSparseMatrix::new_raw(SparseMatrixInner::from(
                calculation(&operators),
            )))
        })
    }
}

unsafe fn dc_vector(
    network: *const PioBalancedNetwork,
    formula: *const c_char,
    formula_len: usize,
    error: *mut *mut PioError,
    calculation: impl FnOnce(&DcOperators) -> Vec<f64>,
) -> *mut PioVector {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let operators = dc_operators(network, formula, formula_len)?;
            Ok(PioVector::new_raw(VectorInner {
                values: calculation(&operators),
            }))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_calc_incidence_matrix(
    network: *const PioBalancedNetwork,
    formula: *const c_char,
    formula_len: usize,
    error: *mut *mut PioError,
) -> *mut PioSparseMatrix {
    unsafe {
        dc_matrix(network, formula, formula_len, error, |operators| {
            operators.calc_incidence_matrix()
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_calc_bus_susceptance_matrix(
    network: *const PioBalancedNetwork,
    formula: *const c_char,
    formula_len: usize,
    error: *mut *mut PioError,
) -> *mut PioSparseMatrix {
    unsafe {
        dc_matrix(network, formula, formula_len, error, |operators| {
            operators.calc_bus_susceptance_matrix()
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_calc_branch_flow_matrix(
    network: *const PioBalancedNetwork,
    formula: *const c_char,
    formula_len: usize,
    error: *mut *mut PioError,
) -> *mut PioSparseMatrix {
    unsafe {
        dc_matrix(network, formula, formula_len, error, |operators| {
            operators.calc_branch_flow_matrix()
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_calc_branch_susceptances(
    network: *const PioBalancedNetwork,
    formula: *const c_char,
    formula_len: usize,
    error: *mut *mut PioError,
) -> *mut PioVector {
    unsafe {
        dc_vector(network, formula, formula_len, error, |operators| {
            operators.calc_branch_susceptances().to_vec()
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_calc_branch_phase_shift_injection(
    network: *const PioBalancedNetwork,
    formula: *const c_char,
    formula_len: usize,
    error: *mut *mut PioError,
) -> *mut PioVector {
    unsafe {
        dc_vector(network, formula, formula_len, error, |operators| {
            operators.calc_branch_phase_shift_injection()
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_calc_bus_phase_shift_injection(
    network: *const PioBalancedNetwork,
    formula: *const c_char,
    formula_len: usize,
    error: *mut *mut PioError,
) -> *mut PioVector {
    unsafe {
        dc_vector(network, formula, formula_len, error, |operators| {
            operators.calc_bus_phase_shift_injection()
        })
    }
}

unsafe fn dc_vector_from_angles(
    network: *const PioBalancedNetwork,
    formula_name: *const c_char,
    formula_name_len: usize,
    voltage_angles: *const f64,
    voltage_angles_len: usize,
    error: *mut *mut PioError,
    calculation: impl FnOnce(&DcOperators, &[f64]) -> Result<Vec<f64>, powerio_core::Error>,
) -> *mut PioVector {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            if voltage_angles.is_null() && voltage_angles_len != 0 {
                return Err(boundary_error(
                    &codes::BIND_CAPI_NULL_ARGUMENT,
                    "voltage_angles is NULL with a nonzero length",
                ));
            }
            let angles = if voltage_angles_len == 0 {
                &[]
            } else {
                std::slice::from_raw_parts(voltage_angles, voltage_angles_len)
            };
            let operators = dc_operators(network, formula_name, formula_name_len)?;
            calculation(&operators, angles)
                .map(|values| PioVector::new_raw(VectorInner { values }))
                .map_err(|failure| error_from_core(&failure))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_calc_branch_flow_dc(
    network: *const PioBalancedNetwork,
    formula: *const c_char,
    formula_len: usize,
    voltage_angles: *const f64,
    voltage_angles_len: usize,
    error: *mut *mut PioError,
) -> *mut PioVector {
    unsafe {
        dc_vector_from_angles(
            network,
            formula,
            formula_len,
            voltage_angles,
            voltage_angles_len,
            error,
            DcOperators::calc_branch_flow_dc,
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_calc_bus_injection_dc(
    network: *const PioBalancedNetwork,
    formula: *const c_char,
    formula_len: usize,
    voltage_angles: *const f64,
    voltage_angles_len: usize,
    error: *mut *mut PioError,
) -> *mut PioVector {
    unsafe {
        dc_vector_from_angles(
            network,
            formula,
            formula_len,
            voltage_angles,
            voltage_angles_len,
            error,
            DcOperators::calc_bus_injection_dc,
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_sparse_matrix_rows(matrix: *const PioSparseMatrix) -> usize {
    unsafe { PioSparseMatrix::get(matrix) }.map_or(0, |matrix| matrix.rows)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_sparse_matrix_columns(matrix: *const PioSparseMatrix) -> usize {
    unsafe { PioSparseMatrix::get(matrix) }.map_or(0, |matrix| matrix.columns)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_sparse_matrix_row_offsets(
    matrix: *const PioSparseMatrix,
) -> PioSizeView {
    unsafe { PioSparseMatrix::get(matrix) }.map_or(PioSizeView::EMPTY, |matrix| {
        PioSizeView::new(&matrix.row_offsets)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_sparse_matrix_column_indices(
    matrix: *const PioSparseMatrix,
) -> PioSizeView {
    unsafe { PioSparseMatrix::get(matrix) }.map_or(PioSizeView::EMPTY, |matrix| {
        PioSizeView::new(&matrix.column_indices)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_sparse_matrix_values(matrix: *const PioSparseMatrix) -> PioF64View {
    unsafe { PioSparseMatrix::get(matrix) }
        .map_or(PioF64View::EMPTY, |matrix| PioF64View::new(&matrix.values))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_sparse_matrix_retain(
    matrix: *const PioSparseMatrix,
) -> *mut PioSparseMatrix {
    unsafe { PioSparseMatrix::retain_raw(matrix) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_sparse_matrix_release(matrix: *mut PioSparseMatrix) {
    unsafe { PioSparseMatrix::release_raw(matrix) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_vector_values(vector: *const PioVector) -> PioF64View {
    unsafe { PioVector::get(vector) }
        .map_or(PioF64View::EMPTY, |vector| PioF64View::new(&vector.values))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_vector_retain(vector: *const PioVector) -> *mut PioVector {
    unsafe { PioVector::retain_raw(vector) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_vector_release(vector: *mut PioVector) {
    unsafe { PioVector::release_raw(vector) };
}

// ---- schema report ---------------------------------------------------------

struct StringInner {
    text: String,
}

opaque_handle!(
    /// Owned UTF-8 text returned by report functions.
    PioString,
    StringInner
);

/// Return version information for this ABI and the PowerIO IR serializer/deserializer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_schema_report(error: *mut *mut PioError) -> *mut PioString {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            serde_json::to_string(&serde_json::json!({
                "powerio_version": powerio::VERSION,
                "abi": PIO_ABI_VERSION,
                "powerio_ir": {
                    "schema": powerio::IR_SCHEMA_NAME,
                    "version": powerio::IR_VERSION
                },
                "bmopf_schema": powerio_dist::BMOPF_SCHEMA_VERSION,
                // ABI 7 exports one fixed symbol set. Matrices, the
                // multiconductor model, and calculation types are always
                // present; only GridFM Parquet support is a build option.
                "features": {
                    "matrix": true,
                    "gridfm": cfg!(feature = "gridfm"),
                    "dist": true,
                    "prob": true
                },
                "foreign_schemas": {
                    "bmopf": powerio_dist::BMOPF_SCHEMA_VERSION
                },
                "error_categories": powerio_core::ErrorCategory::TOKENS,
                "diagnostic_namespaces": powerio_core::DiagnosticStage::NAMESPACES,
                "json_classes": powerio::JSON_CLASSES
            }))
            .map(|text| PioString::new_raw(StringInner { text }))
            .map_err(|failure| {
                boundary_error(
                    &codes::EMIT_CAPI_SERIALIZE_FAILED,
                    format!("cannot serialize the schema report: {failure}"),
                )
            })
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_string_view(string: *const PioString) -> PioStringView {
    unsafe { PioString::get(string) }.map_or(PioStringView::EMPTY, |string| {
        PioStringView::new(&string.text)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_string_retain(string: *const PioString) -> *mut PioString {
    unsafe { PioString::retain_raw(string) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_string_release(string: *mut PioString) {
    unsafe { PioString::release_raw(string) };
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe fn view_text(view: PioStringView) -> String {
        if view.data.is_null() {
            return String::new();
        }
        String::from_utf8_lossy(unsafe {
            std::slice::from_raw_parts(view.data.cast::<u8>(), view.len)
        })
        .into_owned()
    }

    unsafe fn parse_case9() -> *mut PioModule {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data/case9.m");
        let path = path.to_string_lossy();
        let mut error = std::ptr::null_mut();
        let source = unsafe { pio_source_open(path.as_ptr().cast(), path.len(), &mut error) };
        assert!(!source.is_null(), "{}", unsafe { error_text(error) });
        let module = unsafe { pio_parse(source, std::ptr::null(), 0, &mut error) };
        unsafe { pio_source_release(source) };
        assert!(!module.is_null(), "{}", unsafe { error_text(error) });
        module
    }

    fn parse_xiidm_text(text: &str) -> *mut PioModule {
        unsafe {
            let name = b"case.xiidm";
            let format = b"xiidm";
            let mut error = std::ptr::null_mut();
            let source = pio_source_from_memory(
                name.as_ptr().cast(),
                name.len(),
                text.as_ptr(),
                text.len(),
                &mut error,
            );
            assert!(!source.is_null(), "{}", error_text(error));
            let module = pio_parse(source, format.as_ptr().cast(), format.len(), &mut error);
            pio_source_release(source);
            assert!(!module.is_null(), "{}", error_text(error));
            module
        }
    }

    unsafe fn parse_bmopf() -> *mut PioModule {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/data/dist/bmopf/example_ieee13.json");
        let path = path.to_string_lossy();
        let mut error = std::ptr::null_mut();
        let source = unsafe { pio_source_open(path.as_ptr().cast(), path.len(), &mut error) };
        assert!(!source.is_null(), "{}", unsafe { error_text(error) });
        let module = unsafe { pio_parse(source, std::ptr::null(), 0, &mut error) };
        unsafe { pio_source_release(source) };
        assert!(!module.is_null(), "{}", unsafe { error_text(error) });
        module
    }

    unsafe fn error_text(error: *const PioError) -> String {
        let view = unsafe { pio_error_message(error) };
        if view.data.is_null() {
            return "missing PioError".to_owned();
        }
        String::from_utf8_lossy(unsafe {
            std::slice::from_raw_parts(view.data.cast::<u8>(), view.len)
        })
        .into_owned()
    }

    unsafe fn case9_network() -> BalancedNetwork {
        let module = unsafe { parse_case9() };
        let network = match &unsafe { PioModule::get(module) }.unwrap().module.value() {
            PioValue::BalancedNetwork(network) => network.clone(),
            value => panic!("expected balanced network, got {}", value.type_name()),
        };
        unsafe { pio_module_release(module) };
        network
    }

    fn module_with_complete_records() -> *mut PioModule {
        use std::collections::BTreeMap;

        let mut module = powerio::PioModule::new(PioValue::BalancedNetwork(BalancedNetwork::new(
            "metadata", 100.0,
        )))
        .with_producer(Producer::new("record-test", "1.2.3").unwrap());
        let source_id = powerio_core::SourceId::new("source-1").unwrap();
        let source =
            powerio_core::SourceDescriptor::new(source_id.clone(), "record-test.xiidm", 128)
                .unwrap()
                .with_format(powerio_core::FormatId::new("xiidm").unwrap())
                .with_digest(
                    powerio_core::Digest::sha256(
                        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                    )
                    .unwrap(),
                );
        module.add_source_descriptor(source).unwrap();
        module
            .add_source_map_entry(
                powerio_core::SourceMapEntry::new(
                    "/value/buses/0",
                    powerio_core::SourceRelation::Exact,
                    vec![powerio_core::SourceSpan::new(source_id, 4, 12).unwrap()],
                )
                .unwrap(),
            )
            .unwrap();
        let parameters = BTreeMap::from([(
            "settings".to_owned(),
            serde_json::json!({
                "items": [null, true, "named", 18446744073709551615u64, 1.25]
            }),
        )]);
        let history = HistoryEntry::new(
            HistoryId::new("history-1").unwrap(),
            HistoryKind::Transform,
            "normalize",
        )
        .unwrap()
        .with_input_type("powerio.BalancedNetwork")
        .unwrap()
        .with_output_type("powerio.BalancedNetwork")
        .unwrap()
        .with_parameters(parameters)
        .unwrap()
        .with_assumption("base power is known")
        .unwrap()
        .with_loss("source token spelling")
        .unwrap();
        module.add_history_entry(history).unwrap();
        module
            .insert_extension("org.example.record", serde_json::json!({"enabled": true}))
            .unwrap();
        module_handle(module)
    }

    fn multiconductor_network() -> powerio_dist::MulticonductorNetwork {
        let mut network = powerio_dist::MulticonductorNetwork::named("capi-solution-instance");
        network.buses_mut().push(powerio_dist::DistBus::new(
            "source",
            vec!["1".into(), "2".into(), "3".into()],
        ));
        network.sources_mut().push(powerio_dist::VoltageSource::new(
            "source",
            "source",
            vec!["1".into(), "2".into(), "3".into()],
            vec![240.0, 240.0, 240.0],
            vec![0.0, -2.094, 2.094],
        ));
        network
    }

    fn complete_multiconductor_network() -> powerio_dist::MulticonductorNetwork {
        let mut network = powerio_dist::MulticonductorNetwork::named("");
        *network.source_format_mut() = Some(powerio_dist::DistSourceFormat::Dss);
        *network.geo_mut() = Some(powerio_dist::DistGeoMeta {
            space: powerio_dist::CoordinateSpace::Diagram {
                canvas: Some(powerio_dist::DistCanvas {
                    width: Some(640.0),
                    height: None,
                    units: Some(String::new()),
                }),
            },
            kind: Some(powerio_dist::DistCoordsKind::Manual),
        });

        let mut bus = powerio_dist::DistBus::new("bus", vec!["1".into(), "2".into()]);
        bus.grounded.push("2".into());
        bus.v_min = Some(0.0);
        bus.vpn_min = Some(Vec::new());
        bus.vpp_max = Some(vec![240.0, 240.0]);
        bus.location = Some(powerio_dist::DistLocation {
            x: 10.0,
            y: 20.0,
            kind: None,
        });
        network.buses_mut().push(bus);

        let mut line_code = powerio_dist::DistLineCode::new(
            "code",
            vec![vec![1.0, 2.0], vec![3.0]],
            vec![vec![4.0]],
        );
        line_code.g_from = vec![vec![5.0]];
        line_code.b_from = vec![vec![6.0]];
        line_code.g_to = vec![vec![7.0]];
        line_code.b_to = vec![vec![8.0]];
        line_code.i_max = Some(Vec::new());
        line_code.s_max = Some(vec![900.0]);
        line_code.source = Some(String::new());
        network.line_codes_mut().push(line_code);

        let mut line = powerio_dist::DistLine::new(
            "line",
            "bus",
            "bus",
            vec!["1".into()],
            vec!["2".into()],
            "code",
            12.0,
        );
        line.route = Some(Vec::new());
        line.i_max = Some(vec![10.0]);
        network.lines_mut().push(line);

        let mut switch = powerio_dist::DistSwitch::new(
            "switch",
            "bus",
            "bus",
            vec!["1".into()],
            vec!["2".into()],
            true,
        );
        switch.i_max = Some(Vec::new());
        network.switches_mut().push(switch);

        let mut winding = powerio_dist::DistWinding::new(
            "bus",
            vec!["1".into(), "2".into()],
            powerio_dist::DistWindingConn::Wye,
            240.0,
            1_000.0,
        );
        winding.r_pct = 0.5;
        winding.r_neutral = Some(0.0);
        network
            .transformers_mut()
            .push(powerio_dist::DistTransformer::new(
                "transformer",
                vec![winding],
                vec![2.5],
                1,
            ));

        let mut load = powerio_dist::DistLoad::new(
            "load",
            "bus",
            vec!["1".into()],
            powerio_dist::Configuration::Wye,
            vec![100.0],
            vec![20.0],
        );
        load.voltage_model = powerio_dist::DistLoadVoltageModel::Zip {
            v_nom: vec![240.0],
            alpha_z: vec![0.1],
            alpha_i: vec![0.2],
            alpha_p: vec![0.7],
            beta_z: vec![0.2],
            beta_i: vec![0.3],
            beta_p: vec![0.5],
        };
        network.loads_mut().push(load);

        let mut generator = powerio_dist::DistGenerator::new(
            "generator",
            "bus",
            vec!["1".into()],
            powerio_dist::Configuration::SinglePhase,
            vec![50.0],
            vec![5.0],
        );
        generator.p_min = Some(Vec::new());
        generator.cost = Some(vec![0.25]);
        network.generators_mut().push(generator);

        let mut resource = powerio_dist::DistIbr::new(
            "ibr",
            "bus",
            vec!["1".into()],
            powerio_dist::IbrTopology::SinglePhase,
            powerio_dist::IbrPrimeMover::Pv,
            vec![100.0],
        );
        resource.i_max = Some(Vec::new());
        resource.p_avail = Some(75.0);
        resource.control_profile = Some(String::new());
        resource.voltage_aggregation = Some(powerio_dist::IbrVoltageAggregation::PerPhase);
        network.ibrs_mut().push(resource);

        let profile: powerio_dist::DistControlProfile = serde_json::from_value(serde_json::json!({
            "name": "profile",
            "power_factor": { "pf": 0.97 },
            "volt_var": {
                "voltage_reference": "PN_PER_PHASE",
                "breakpoints": [0.95, 1.05],
                "q_limits": [-0.4, 0.4],
                "q_unit": "VA_FRACTION",
                "q_ref": "VAR_MAX",
                "p_min_for_q": 10.0,
                "p_min_for_q_max": null
            },
            "volt_watt": {
                "voltage_reference": null,
                "breakpoints": [1.0],
                "p_limits": [0.8],
                "p_unit": "W",
                "p_ref": "P_AVAILABLE"
            },
            "extras": {}
        }))
        .unwrap();
        network.control_profiles_mut().push(profile);

        network.shunts_mut().push(powerio_dist::DistShunt::new(
            "shunt",
            "bus",
            vec!["2".into()],
            vec![vec![0.01, 0.02], vec![0.03]],
            vec![vec![0.04]],
        ));
        network
            .capacitors_mut()
            .push(powerio_dist::DistCapacitor::new(
                "capacitor",
                "bus",
                vec!["1".into()],
                powerio_dist::Configuration::SinglePhase,
                100.0,
                240.0,
            ));
        network.sources_mut().push(powerio_dist::VoltageSource::new(
            "source",
            "bus",
            vec!["1".into()],
            vec![240.0],
            vec![0.0],
        ));
        network
            .untyped_objects_mut()
            .push(powerio_dist::UntypedObject::new(
                "curve",
                "raw",
                vec![
                    (None, "first".into()),
                    (Some(String::new()), "second".into()),
                ],
            ));
        network
            .commands_mut()
            .push(("solve".into(), "mode=daily".into()));
        network
            .options_mut()
            .push(("frequency".into(), "60".into()));
        network
    }

    fn goc3_instance(source: Source) -> powerio_prob::AcScucInstance {
        let module = powerio::parse_with_options(
            source,
            &powerio::ParseOptions::default()
                .format("goc3-json")
                .unwrap(),
        )
        .unwrap();
        let PioValue::AcScucInstance(instance) = module.into_value() else {
            panic!("GO Challenge 3 problem did not produce powerio.AcScucInstance");
        };
        instance
    }

    #[test]
    fn reports_abi_seven() {
        assert_eq!(pio_abi_version(), 7);
        assert_eq!(PIO_ABI_VERSION, 7);
        unsafe {
            let mut error = std::ptr::null_mut();
            let report = pio_schema_report(&mut error);
            assert!(!report.is_null(), "{}", error_text(error));
            let parsed: serde_json::Value =
                serde_json::from_str(&view_text(pio_string_view(report))).unwrap();
            assert_eq!(parsed["abi"], 7);
            assert_eq!(parsed["powerio_ir"]["schema"], powerio::IR_SCHEMA_NAME);
            assert_eq!(parsed["powerio_ir"]["version"], powerio::IR_VERSION);
            assert_eq!(
                parsed["bmopf_schema"],
                serde_json::json!(powerio_dist::BMOPF_SCHEMA_VERSION)
            );
            assert_eq!(parsed["foreign_schemas"]["bmopf"], parsed["bmopf_schema"]);
            assert_eq!(
                parsed["error_categories"],
                serde_json::json!(powerio_core::ErrorCategory::TOKENS)
            );
            assert_eq!(
                parsed["diagnostic_namespaces"],
                serde_json::json!(powerio_core::DiagnosticStage::NAMESPACES)
            );
            assert_eq!(
                parsed["json_classes"],
                serde_json::json!(powerio::JSON_CLASSES)
            );
            for feature in ["matrix", "gridfm", "dist", "prob"] {
                assert!(parsed["features"][feature].is_boolean());
            }
            pio_string_release(report);
        }
    }

    #[test]
    fn structured_diagnostic_fields_keep_their_owner_alive() {
        unsafe {
            let source = powerio_core::SourceId::new("input").unwrap();
            let span = powerio_core::SourceSpan::new(source, 2, 7).unwrap();
            let mut details = serde_json::Map::new();
            details.insert("count".to_owned(), serde_json::json!(3));
            details.insert("nested".to_owned(), serde_json::json!({ "ok": true }));
            let diagnostic = Diagnostic::new(
                powerio_core::DiagnosticCode::new("PARTNER.TEST.STRUCTURED").unwrap(),
                powerio_core::DiagnosticSeverity::Warning,
                "structured finding",
            )
            .with_id(powerio_core::DiagnosticId::new("d1").unwrap())
            .with_target("/value/data")
            .unwrap()
            .with_span(span)
            .unwrap()
            .with_related(powerio_core::DiagnosticId::new("d0").unwrap())
            .unwrap()
            .with_details(details)
            .unwrap()
            .with_suggested_action("fix it");
            let diagnostics = PioDiagnostics::new_raw(DiagnosticsInner {
                owner: DiagnosticsOwner::Owned(vec![diagnostic]),
            });
            let retained = pio_diagnostics_retain(diagnostics);
            pio_diagnostics_release(diagnostics);

            assert_eq!(pio_diagnostics_len(retained), 1);
            assert_eq!(
                view_text(pio_diagnostic_code(retained, 0)),
                "PARTNER.TEST.STRUCTURED"
            );
            assert_eq!(view_text(pio_diagnostic_severity(retained, 0)), "warning");
            assert_eq!(
                view_text(pio_diagnostic_message(retained, 0)),
                "structured finding"
            );
            assert!(pio_diagnostic_has_id(retained, 0));
            assert_eq!(view_text(pio_diagnostic_id(retained, 0)), "d1");
            assert!(pio_diagnostic_has_target(retained, 0));
            assert_eq!(view_text(pio_diagnostic_target(retained, 0)), "/value/data");
            assert!(pio_diagnostic_has_suggested_action(retained, 0));
            assert_eq!(
                view_text(pio_diagnostic_suggested_action(retained, 0)),
                "fix it"
            );

            assert_eq!(pio_diagnostic_n_spans(retained, 0), 1);
            let mut output = PioDiagnosticSpanView {
                source: PioStringView::EMPTY,
                byte_start: 0,
                byte_end: 0,
            };
            let mut error = std::ptr::null_mut();
            assert!(pio_diagnostic_span(retained, 0, 0, &mut output, &mut error));
            assert!(error.is_null());
            assert_eq!(view_text(output.source), "input");
            assert_eq!(output.byte_start, 2);
            assert_eq!(output.byte_end, 7);

            assert_eq!(pio_diagnostic_n_related(retained, 0), 1);
            assert_eq!(view_text(pio_diagnostic_related(retained, 0, 0)), "d0");

            let details_text = pio_diagnostic_details_json(retained, 0, &mut error);
            assert!(!details_text.is_null(), "{}", error_text(error));
            let details: serde_json::Value =
                serde_json::from_str(&view_text(pio_string_view(details_text))).unwrap();
            assert_eq!(details["count"], 3);
            assert_eq!(details["nested"]["ok"], true);
            pio_string_release(details_text);

            pio_diagnostics_release(retained);
        }
    }

    #[test]
    fn module_records_are_typed_and_structured_values_keep_the_owner_alive() {
        unsafe {
            let module = module_with_complete_records();
            let mut error = std::ptr::null_mut();

            let mut producer = std::mem::MaybeUninit::<PioModuleProducerView>::uninit();
            assert!(pio_module_producer(
                module,
                producer.as_mut_ptr(),
                &mut error
            ));
            let producer = producer.assume_init();
            assert_eq!(view_text(producer.name), "record-test");
            assert_eq!(view_text(producer.version), "1.2.3");

            assert_eq!(pio_module_source_count(module), 1);
            let mut source = std::mem::MaybeUninit::<PioModuleSourceView>::uninit();
            assert!(pio_module_source_at(
                module,
                0,
                source.as_mut_ptr(),
                &mut error
            ));
            let source = source.assume_init();
            assert_eq!(view_text(source.id), "source-1");
            assert_eq!(view_text(source.name), "record-test.xiidm");
            assert_eq!(source.byte_length, 128);
            assert!(source.has_format);
            assert_eq!(view_text(source.format), "xiidm");
            assert!(source.has_digest);
            assert_eq!(view_text(source.digest_algorithm), "sha256");
            assert_eq!(source.digest.len, 64);

            assert_eq!(pio_module_source_map_count(module), 1);
            let mut mapping = std::mem::MaybeUninit::<PioModuleSourceMapEntryView>::uninit();
            assert!(pio_module_source_map_at(
                module,
                0,
                mapping.as_mut_ptr(),
                &mut error
            ));
            let mapping = mapping.assume_init();
            assert_eq!(view_text(mapping.target), "/value/buses/0");
            assert_eq!(view_text(mapping.relation), "exact");
            assert_eq!(mapping.span_count, 1);
            let mut span = std::mem::MaybeUninit::<PioSourceSpanView>::uninit();
            assert!(pio_module_source_map_span_at(
                module,
                0,
                0,
                span.as_mut_ptr(),
                &mut error
            ));
            let span = span.assume_init();
            assert_eq!(view_text(span.source), "source-1");
            assert_eq!((span.byte_start, span.byte_end), (4, 12));

            assert_eq!(pio_module_history_count(module), 1);
            let mut history = std::mem::MaybeUninit::<PioModuleHistoryEntryView>::uninit();
            assert!(pio_module_history_at(
                module,
                0,
                history.as_mut_ptr(),
                &mut error
            ));
            let history = history.assume_init();
            assert_eq!(view_text(history.id), "history-1");
            assert_eq!(view_text(history.kind), "transform");
            assert_eq!(view_text(history.name), "normalize");
            assert!(history.has_input_type);
            assert_eq!(view_text(history.input_type), "powerio.BalancedNetwork");
            assert!(history.has_output_type);
            assert_eq!(view_text(history.output_type), "powerio.BalancedNetwork");
            assert_eq!(history.parameter_count, 1);
            assert_eq!(history.assumption_count, 1);
            assert_eq!(history.loss_count, 1);
            assert_eq!(
                view_text(pio_module_history_assumption_at(module, 0, 0, &mut error)),
                "base power is known"
            );
            assert_eq!(
                view_text(pio_module_history_loss_at(module, 0, 0, &mut error)),
                "source token spelling"
            );
            let mut parameter = std::mem::MaybeUninit::<PioModuleHistoryParameterView>::uninit();
            assert!(pio_module_history_parameter_at(
                module,
                0,
                0,
                parameter.as_mut_ptr(),
                &mut error
            ));
            let parameter = parameter.assume_init();
            assert_eq!(view_text(parameter.name), "settings");
            assert_eq!(view_text(parameter.value_kind), "object");

            assert_eq!(pio_module_extension_count(module), 1);
            let mut extension = std::mem::MaybeUninit::<PioModuleExtensionView>::uninit();
            assert!(pio_module_extension_at(
                module,
                0,
                extension.as_mut_ptr(),
                &mut error
            ));
            let extension = extension.assume_init();
            assert_eq!(view_text(extension.namespace), "org.example.record");
            assert_eq!(view_text(extension.value_kind), "object");

            let parameter_value = pio_module_history_parameter_value_at(module, 0, 0, &mut error);
            assert!(!parameter_value.is_null(), "{}", error_text(error));
            let items = pio_json_value_object_value_at(parameter_value, 0, &mut error);
            assert!(!items.is_null(), "{}", error_text(error));
            let unsigned = pio_json_value_array_at(items, 3, &mut error);
            assert!(!unsigned.is_null(), "{}", error_text(error));
            let retained_unsigned = pio_json_value_retain(unsigned);
            let extension_value = pio_module_extension_value_at(module, 0, &mut error);
            assert!(!extension_value.is_null(), "{}", error_text(error));

            let mut ignored = std::mem::MaybeUninit::<PioModuleSourceView>::uninit();
            assert!(!pio_module_source_at(
                module,
                1,
                ignored.as_mut_ptr(),
                &mut error
            ));
            assert!(!error.is_null());
            pio_error_release(error);
            error = std::ptr::null_mut();

            pio_module_release(module);
            pio_json_value_release(parameter_value);
            pio_json_value_release(items);
            pio_json_value_release(unsigned);

            let mut number = std::mem::MaybeUninit::<PioJsonValueView>::uninit();
            assert!(pio_json_value_get(
                retained_unsigned,
                number.as_mut_ptr(),
                &mut error
            ));
            let number = number.assume_init();
            assert_eq!(view_text(number.kind), "number");
            assert_eq!(view_text(number.number_kind), "unsigned_integer");
            assert_eq!(number.unsigned_integer_value, u64::MAX);

            let enabled = pio_json_value_object_value_at(extension_value, 0, &mut error);
            assert!(!enabled.is_null(), "{}", error_text(error));
            let mut boolean = std::mem::MaybeUninit::<PioJsonValueView>::uninit();
            assert!(pio_json_value_get(
                enabled,
                boolean.as_mut_ptr(),
                &mut error
            ));
            let boolean = boolean.assume_init();
            assert_eq!(view_text(boolean.kind), "boolean");
            assert!(boolean.boolean_value);

            let mut null_output = std::mem::MaybeUninit::<PioModuleProducerView>::uninit();
            assert!(!pio_module_producer(
                std::ptr::null(),
                null_output.as_mut_ptr(),
                &mut error
            ));
            assert!(!error.is_null());
            pio_error_release(error);
            pio_json_value_release(enabled);
            pio_json_value_release(extension_value);
            pio_json_value_release(retained_unsigned);
        }
    }

    #[test]
    fn structural_access_keeps_the_module_owner_alive() {
        unsafe {
            let module = parse_case9();
            let diagnostics = pio_module_diagnostics(module);
            let value = pio_module_value(module);
            assert!(pio_value_is_type(
                value,
                c"powerio.BalancedNetwork".as_ptr(),
                "powerio.BalancedNetwork".len(),
            ));
            let mut error = std::ptr::null_mut();
            let network = pio_value_balanced_network(value, &mut error);
            assert!(!network.is_null(), "{}", error_text(error));

            pio_module_release(module);
            pio_value_release(value);
            assert_eq!(pio_balanced_network_bus_count(network), 9);
            assert_eq!(pio_balanced_network_branch_count(network), 9);
            assert_eq!(pio_diagnostics_len(diagnostics), 0);

            pio_balanced_network_release(network);
            pio_diagnostics_release(diagnostics);
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn multiconductor_typed_views_preserve_ownership_and_optional_values() {
        unsafe {
            let module = module_handle(powerio::PioModule::new(PioValue::from(
                complete_multiconductor_network(),
            )));
            let value = pio_module_value(module);
            let mut error = std::ptr::null_mut();
            let network = pio_value_multiconductor_network(value, &mut error);
            assert!(!network.is_null(), "{}", error_text(error));
            let retained = pio_multiconductor_network_retain(network);
            pio_module_release(module);
            pio_value_release(value);
            pio_multiconductor_network_release(network);

            assert!(pio_multiconductor_network_has_name(retained));
            assert_eq!(pio_multiconductor_network_name(retained).len, 0);
            assert!(pio_multiconductor_network_has_source_format(retained));
            assert_eq!(
                view_text(pio_multiconductor_network_source_format(retained)),
                "dss"
            );

            let mut geo = std::mem::MaybeUninit::<PioMulticonductorGeoView>::uninit();
            assert!(pio_multiconductor_network_geo(
                retained,
                geo.as_mut_ptr(),
                &mut error,
            ));
            let geo = geo.assume_init();
            assert!(geo.has_geo);
            assert_eq!(view_text(geo.space), "diagram");
            assert!(geo.has_canvas);
            assert!(geo.has_canvas_width);
            assert!(!geo.has_canvas_height);
            assert!(geo.has_canvas_units);
            assert_eq!(geo.canvas_units.len, 0);

            let mut counts = std::mem::MaybeUninit::<PioMulticonductorNetworkCountsView>::uninit();
            assert!(pio_multiconductor_network_counts(
                retained,
                counts.as_mut_ptr(),
                &mut error,
            ));
            let counts = counts.assume_init();
            for count in [
                counts.buses,
                counts.line_codes,
                counts.lines,
                counts.switches,
                counts.transformers,
                counts.loads,
                counts.generators,
                counts.inverter_based_resources,
                counts.control_profiles,
                counts.shunts,
                counts.capacitors,
                counts.voltage_sources,
                counts.untyped_objects,
                counts.commands,
                counts.options,
            ] {
                assert_eq!(count, 1);
            }

            let mut bus = std::mem::MaybeUninit::<PioMulticonductorBusView>::uninit();
            assert!(pio_multiconductor_network_bus_at(
                retained,
                0,
                bus.as_mut_ptr(),
                &mut error,
            ));
            let bus = bus.assume_init();
            assert_eq!(view_text(bus.id), "bus");
            assert!(bus.has_phase_to_neutral_voltage_min);
            assert_eq!(bus.phase_to_neutral_voltage_min_v.len, 0);
            assert!(bus.has_location);
            assert!(!bus.location.has_kind);
            let mut text = std::mem::MaybeUninit::<PioStringView>::uninit();
            assert!(pio_multiconductor_network_bus_terminal_at(
                retained,
                0,
                1,
                text.as_mut_ptr(),
                &mut error,
            ));
            assert_eq!(view_text(text.assume_init()), "2");

            let mut line_code = std::mem::MaybeUninit::<PioMulticonductorLineCodeView>::uninit();
            assert!(pio_multiconductor_network_line_code_at(
                retained,
                0,
                line_code.as_mut_ptr(),
                &mut error,
            ));
            let line_code = line_code.assume_init();
            assert_eq!(line_code.resistance_matrix_row_count, 2);
            assert!(line_code.has_current_limit);
            assert_eq!(line_code.current_limit_a.len, 0);
            assert!(line_code.has_source);
            assert_eq!(line_code.source.len, 0);
            let mut matrix_row = std::mem::MaybeUninit::<PioF64View>::uninit();
            assert!(
                pio_multiconductor_network_line_code_resistance_matrix_row_at(
                    retained,
                    0,
                    1,
                    matrix_row.as_mut_ptr(),
                    &mut error,
                )
            );
            let matrix_row = matrix_row.assume_init();
            assert_eq!(matrix_row.len, 1);
            assert_eq!(*matrix_row.data, 3.0);

            let mut line = std::mem::MaybeUninit::<PioMulticonductorLineView>::uninit();
            assert!(pio_multiconductor_network_line_at(
                retained,
                0,
                line.as_mut_ptr(),
                &mut error,
            ));
            let line = line.assume_init();
            assert!(line.has_route);
            assert_eq!(line.route_point_count, 0);

            let mut switch = std::mem::MaybeUninit::<PioMulticonductorSwitchView>::uninit();
            assert!(pio_multiconductor_network_switch_at(
                retained,
                0,
                switch.as_mut_ptr(),
                &mut error,
            ));
            let switch = switch.assume_init();
            assert!(switch.open);
            assert!(switch.has_current_limit);
            assert_eq!(switch.current_limit_a.len, 0);

            let mut transformer =
                std::mem::MaybeUninit::<PioMulticonductorTransformerView>::uninit();
            assert!(pio_multiconductor_network_transformer_at(
                retained,
                0,
                transformer.as_mut_ptr(),
                &mut error,
            ));
            assert_eq!(transformer.assume_init().winding_count, 1);
            let mut winding =
                std::mem::MaybeUninit::<PioMulticonductorTransformerWindingView>::uninit();
            assert!(pio_multiconductor_network_transformer_winding_at(
                retained,
                0,
                0,
                winding.as_mut_ptr(),
                &mut error,
            ));
            let winding = winding.assume_init();
            assert!(winding.has_neutral_resistance);
            assert!(!winding.has_neutral_reactance);

            let mut load = std::mem::MaybeUninit::<PioMulticonductorLoadView>::uninit();
            assert!(pio_multiconductor_network_load_at(
                retained,
                0,
                load.as_mut_ptr(),
                &mut error,
            ));
            let load = load.assume_init();
            assert_eq!(view_text(load.voltage_model), "zip");
            assert_eq!(*load.active_power_constant_power.data, 0.7);

            let mut generator = std::mem::MaybeUninit::<PioMulticonductorGeneratorView>::uninit();
            assert!(pio_multiconductor_network_generator_at(
                retained,
                0,
                generator.as_mut_ptr(),
                &mut error,
            ));
            let generator = generator.assume_init();
            assert!(generator.has_active_power_min);
            assert_eq!(generator.active_power_min_w.len, 0);
            assert!(generator.has_active_power_dispatch_cost);

            let mut resource = std::mem::MaybeUninit::<PioInverterBasedResourceView>::uninit();
            assert!(pio_multiconductor_network_inverter_based_resource_at(
                retained,
                0,
                resource.as_mut_ptr(),
                &mut error,
            ));
            let resource = resource.assume_init();
            assert_eq!(view_text(resource.topology), "SINGLE_PHASE");
            assert!(resource.has_control_profile);
            assert_eq!(resource.control_profile.len, 0);

            let mut profile = std::mem::MaybeUninit::<PioControlProfileView>::uninit();
            assert!(pio_multiconductor_network_control_profile_at(
                retained,
                0,
                profile.as_mut_ptr(),
                &mut error,
            ));
            let profile = profile.assume_init();
            assert!(profile.has_power_factor);
            assert!(profile.has_volt_var);
            assert!(profile.has_volt_watt);
            assert_eq!(
                view_text(profile.volt_var_voltage_reference),
                "PN_PER_PHASE"
            );

            let mut shunt = std::mem::MaybeUninit::<PioMulticonductorShuntView>::uninit();
            assert!(pio_multiconductor_network_shunt_at(
                retained,
                0,
                shunt.as_mut_ptr(),
                &mut error,
            ));
            assert_eq!(shunt.assume_init().conductance_matrix_row_count, 2);
            let mut shunt_matrix_row = std::mem::MaybeUninit::<PioF64View>::uninit();
            assert!(pio_multiconductor_network_shunt_conductance_matrix_row_at(
                retained,
                0,
                1,
                shunt_matrix_row.as_mut_ptr(),
                &mut error,
            ));
            assert_eq!(shunt_matrix_row.assume_init().len, 1);

            let mut capacitor = std::mem::MaybeUninit::<PioMulticonductorCapacitorView>::uninit();
            assert!(pio_multiconductor_network_capacitor_at(
                retained,
                0,
                capacitor.as_mut_ptr(),
                &mut error,
            ));
            assert_eq!(capacitor.assume_init().rated_reactive_power_var, 100.0);

            let mut source = std::mem::MaybeUninit::<PioVoltageSourceView>::uninit();
            assert!(pio_multiconductor_network_voltage_source_at(
                retained,
                0,
                source.as_mut_ptr(),
                &mut error,
            ));
            assert_eq!(*source.assume_init().voltage_magnitude_v.data, 240.0);

            let mut object = std::mem::MaybeUninit::<PioMulticonductorUntypedObjectView>::uninit();
            assert!(pio_multiconductor_network_untyped_object_at(
                retained,
                0,
                object.as_mut_ptr(),
                &mut error,
            ));
            assert_eq!(object.assume_init().property_count, 2);
            let mut property =
                std::mem::MaybeUninit::<PioMulticonductorUntypedPropertyView>::uninit();
            assert!(pio_multiconductor_network_untyped_object_property_at(
                retained,
                0,
                0,
                property.as_mut_ptr(),
                &mut error,
            ));
            assert!(!property.assume_init().has_name);
            assert!(pio_multiconductor_network_untyped_object_property_at(
                retained,
                0,
                1,
                property.as_mut_ptr(),
                &mut error,
            ));
            let property = property.assume_init();
            assert!(property.has_name);
            assert_eq!(property.name.len, 0);

            let mut command = std::mem::MaybeUninit::<PioMulticonductorCommandView>::uninit();
            assert!(pio_multiconductor_network_command_at(
                retained,
                0,
                command.as_mut_ptr(),
                &mut error,
            ));
            assert_eq!(view_text(command.assume_init().verb), "solve");
            let mut option = std::mem::MaybeUninit::<PioStringPropertyView>::uninit();
            assert!(pio_multiconductor_network_option_at(
                retained,
                0,
                option.as_mut_ptr(),
                &mut error,
            ));
            assert_eq!(view_text(option.assume_init().name), "frequency");

            assert!(
                std::mem::offset_of!(PioMulticonductorLineView, has_route)
                    > std::mem::offset_of!(PioMulticonductorLineView, route_point_count)
            );
            assert!(
                std::mem::offset_of!(PioMulticonductorBusView, has_location)
                    > std::mem::offset_of!(PioMulticonductorBusView, location)
            );

            let mut null_error = std::ptr::null_mut();
            let mut null_output = std::mem::MaybeUninit::<PioMulticonductorBusView>::uninit();
            assert!(!pio_multiconductor_network_bus_at(
                std::ptr::null(),
                0,
                null_output.as_mut_ptr(),
                &mut null_error,
            ));
            assert!(!null_error.is_null());
            assert_eq!(
                view_text(pio_error_code(null_error)),
                codes::BIND_CAPI_NULL_HANDLE.code
            );
            pio_error_release(null_error);
            pio_multiconductor_network_release(retained);
        }
    }

    #[test]
    fn balanced_geography_is_typed_and_preserves_optional_routes() {
        unsafe {
            let mut network = case9_network();
            *network.geo_mut() = Some(powerio_tx::GeoMeta {
                space: powerio_tx::CoordinateSpace::Diagram {
                    canvas: Some(powerio_tx::Canvas {
                        width: Some(800.0),
                        height: None,
                        units: Some(String::new()),
                    }),
                },
                kind: Some(powerio_tx::CoordsKind::Manual),
            });
            network.buses_mut()[0].location = Some(powerio_tx::Location {
                x: 12.0,
                y: 34.0,
                kind: None,
            });
            network.branches_mut()[0].route = Some(Vec::new());
            network.branches_mut()[1].route = Some(vec![powerio_tx::Location {
                x: 56.0,
                y: 78.0,
                kind: Some(powerio_tx::CoordsKind::Derived),
            }]);

            let module = module_handle(powerio::PioModule::new(PioValue::BalancedNetwork(network)));
            let value = pio_module_value(module);
            let mut error = std::ptr::null_mut();
            let balanced = pio_value_balanced_network(value, &mut error);
            assert!(!balanced.is_null(), "{}", error_text(error));
            let retained = pio_balanced_network_retain(balanced);
            pio_balanced_network_release(balanced);
            pio_value_release(value);
            pio_module_release(module);

            let mut geo = std::mem::MaybeUninit::<PioBalancedGeoView>::uninit();
            assert!(pio_balanced_network_geo(
                retained,
                geo.as_mut_ptr(),
                &mut error
            ));
            let geo = geo.assume_init();
            assert!(geo.has_geo);
            assert_eq!(view_text(geo.space), "diagram");
            assert!(geo.has_kind);
            assert_eq!(view_text(geo.kind), "manual");
            assert!(geo.has_canvas);
            assert!(geo.has_canvas_width);
            assert_eq!(geo.canvas_width, 800.0);
            assert!(!geo.has_canvas_height);
            assert!(geo.has_canvas_units);
            assert_eq!(geo.canvas_units.len, 0);

            let mut bus = std::mem::MaybeUninit::<PioBalancedBusView>::uninit();
            assert!(pio_balanced_network_bus_at(
                retained,
                0,
                bus.as_mut_ptr(),
                &mut error
            ));
            let bus = bus.assume_init();
            assert!(bus.has_location);
            assert_eq!((bus.location.x, bus.location.y), (12.0, 34.0));
            assert!(!bus.location.has_kind);

            let mut branch = std::mem::MaybeUninit::<PioBalancedBranchView>::uninit();
            assert!(pio_balanced_network_branch_at(
                retained,
                0,
                branch.as_mut_ptr(),
                &mut error
            ));
            let branch = branch.assume_init();
            assert!(branch.has_route);
            assert_eq!(branch.route_point_count, 0);

            let mut routed_branch = std::mem::MaybeUninit::<PioBalancedBranchView>::uninit();
            assert!(pio_balanced_network_branch_at(
                retained,
                1,
                routed_branch.as_mut_ptr(),
                &mut error
            ));
            let routed_branch = routed_branch.assume_init();
            assert!(routed_branch.has_route);
            assert_eq!(routed_branch.route_point_count, 1);
            let mut point = std::mem::MaybeUninit::<PioBalancedLocationView>::uninit();
            assert!(pio_balanced_network_branch_route_point_at(
                retained,
                1,
                0,
                point.as_mut_ptr(),
                &mut error
            ));
            let point = point.assume_init();
            assert_eq!((point.x, point.y), (56.0, 78.0));
            assert!(point.has_kind);
            assert_eq!(view_text(point.kind), "derived");

            assert!(
                std::mem::offset_of!(PioBalancedBusView, has_location)
                    > std::mem::offset_of!(PioBalancedBusView, location)
            );
            assert!(
                std::mem::offset_of!(PioBalancedBranchView, has_route)
                    > std::mem::offset_of!(PioBalancedBranchView, route_point_count)
            );

            let mut null_geo = std::mem::MaybeUninit::<PioBalancedGeoView>::uninit();
            assert!(!pio_balanced_network_geo(
                std::ptr::null(),
                null_geo.as_mut_ptr(),
                &mut error
            ));
            assert!(!error.is_null());
            pio_error_release(error);
            pio_balanced_network_release(retained);
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn every_solution_exposes_its_owner_rooted_instance() {
        unsafe {
            let network = case9_network();
            let buses = network.buses().len();
            let branches = network.branches().len();
            let generators = network.generators().len();
            let three_winding = network.transformers_3w().len();

            let dc_pf_instance =
                Arc::new(powerio_prob::DcPfInstance::from_network(network.clone()).unwrap());
            let dc_pf = powerio_prob::DcPfSolution::new(
                dc_pf_instance,
                powerio_prob::Termination::Converged,
                vec![0.0; buses],
                vec![0.0; buses],
                vec![0.0; branches],
                vec![0.0; branches],
                vec![Default::default(); three_winding],
            )
            .unwrap();

            let ac_pf_instance =
                Arc::new(powerio_prob::AcPfInstance::from_network(network.clone()).unwrap());
            let ac_pf = powerio_prob::AcPfSolution::new(
                ac_pf_instance,
                powerio_prob::Termination::Converged,
                vec![1.0; buses],
                vec![0.0; buses],
                vec![0.0; buses],
                vec![0.0; buses],
                vec![0.0; branches],
                vec![0.0; branches],
                vec![0.0; branches],
                vec![0.0; branches],
                vec![Default::default(); three_winding],
            )
            .unwrap();

            let dc_opf_instance =
                Arc::new(powerio_prob::DcOpfInstance::from_network(network.clone()).unwrap());
            let dc_opf = powerio_prob::DcOpfSolution::new(
                dc_opf_instance,
                powerio_prob::Termination::Converged,
                vec![0.0; buses],
                vec![0.0; buses],
                vec![0.0; branches],
                vec![0.0; branches],
                vec![0.0; generators],
                0.0,
                vec![Default::default(); three_winding],
            )
            .unwrap();

            let ac_opf_instance =
                Arc::new(powerio_prob::AcOpfInstance::from_network(network.clone()).unwrap());
            let ac_opf = powerio_prob::AcOpfSolution::new(
                Arc::clone(&ac_opf_instance),
                powerio_prob::Termination::Converged,
                vec![1.0; buses],
                vec![0.0; buses],
                vec![0.0; buses],
                vec![0.0; buses],
                vec![0.0; branches],
                vec![0.0; branches],
                vec![0.0; branches],
                vec![0.0; branches],
                vec![0.0; generators],
                vec![0.0; generators],
                0.0,
                vec![Default::default(); three_winding],
            )
            .unwrap();

            let mut socwr_values = powerio_prob::solution::SocwrOpfValues::default();
            socwr_values.bus_voltage_magnitude_squared = vec![1.0; buses];
            socwr_values.branch_voltage_product_real = vec![0.0; branches];
            socwr_values.branch_voltage_product_imaginary = vec![0.0; branches];
            socwr_values.generator_active_power = vec![0.0; generators];
            socwr_values.generator_reactive_power = vec![0.0; generators];
            socwr_values.branch_from_active_power = vec![0.0; branches];
            socwr_values.branch_from_reactive_power = vec![0.0; branches];
            socwr_values.branch_to_active_power = vec![0.0; branches];
            socwr_values.branch_to_reactive_power = vec![0.0; branches];
            socwr_values.three_winding_transformer_terminal_powers =
                vec![Default::default(); three_winding];
            let socwr = powerio_prob::solution::SocwrOpfSolution::new(
                ac_opf_instance,
                powerio_prob::Termination::Converged,
                socwr_values,
                0.0,
            )
            .unwrap();

            let multiconductor = multiconductor_network();
            let terminals: usize = multiconductor
                .buses()
                .iter()
                .map(|bus| bus.terminals.len())
                .sum();
            let source_terminals: usize = multiconductor
                .sources()
                .iter()
                .map(|source| source.terminal_map.len())
                .sum();
            let generator_terminals: usize = multiconductor
                .generators()
                .iter()
                .map(|generator| generator.terminal_map.len())
                .sum();
            let mc_pf_instance = Arc::new(
                powerio_prob::McAcPfInstance::from_network(multiconductor.clone()).unwrap(),
            );
            let mc_pf = powerio_prob::McAcPfSolution::new(
                mc_pf_instance,
                powerio_prob::Termination::Converged,
                vec![1.0; terminals],
                vec![0.0; terminals],
                vec![0.0; source_terminals],
            )
            .unwrap();
            let mc_opf_instance =
                Arc::new(powerio_prob::McAcOpfInstance::from_network(multiconductor).unwrap());
            let mc_opf = powerio_prob::McAcOpfSolution::new(
                mc_opf_instance,
                powerio_prob::Termination::Converged,
                vec![1.0; terminals],
                vec![0.0; terminals],
                vec![0.0; source_terminals],
                vec![0.0; generator_terminals],
                0.0,
            )
            .unwrap();

            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../tests/data/goc3/goc3_small.json");
            let scuc_instance = goc3_instance(Source::open(path).unwrap());
            let scuc = powerio_prob::AcScucSolution::new(
                Arc::new(scuc_instance),
                powerio_prob::Termination::Converged,
                powerio_prob::ScucNetworkOutputs::default(),
                powerio_prob::ScucDeviceOutputs::default(),
                None,
            )
            .unwrap();

            type SolutionAccessor = unsafe extern "C" fn(
                *const PioValueHandle,
                *mut *mut PioError,
            )
                -> *mut PioCalculationSolution;
            let solutions: [(PioValue, &str, bool, SolutionAccessor); 8] = [
                (
                    PioValue::DcPfSolution(dc_pf),
                    "powerio.DcPfInstance",
                    false,
                    pio_value_dc_pf_solution,
                ),
                (
                    PioValue::AcPfSolution(ac_pf),
                    "powerio.AcPfInstance",
                    false,
                    pio_value_ac_pf_solution,
                ),
                (
                    PioValue::DcOpfSolution(dc_opf),
                    "powerio.DcOpfInstance",
                    false,
                    pio_value_dc_opf_solution,
                ),
                (
                    PioValue::AcOpfSolution(ac_opf),
                    "powerio.AcOpfInstance",
                    false,
                    pio_value_ac_opf_solution,
                ),
                (
                    PioValue::SocwrOpfSolution(socwr),
                    "powerio.AcOpfInstance",
                    false,
                    pio_value_socwr_opf_solution,
                ),
                (
                    PioValue::McAcPfSolution(mc_pf),
                    "powerio.McAcPfInstance",
                    true,
                    pio_value_mc_ac_pf_solution,
                ),
                (
                    PioValue::McAcOpfSolution(mc_opf),
                    "powerio.McAcOpfInstance",
                    true,
                    pio_value_mc_ac_opf_solution,
                ),
                (
                    PioValue::AcScucSolution(scuc),
                    "powerio.AcScucInstance",
                    false,
                    pio_value_ac_scuc_solution,
                ),
            ];

            for (value, expected_instance_type, is_multiconductor, accessor) in solutions {
                let module = module_handle(powerio::PioModule::new(value));
                let value = pio_module_value(module);
                let mut error = std::ptr::null_mut();
                let solution = accessor(value, &mut error);
                assert!(!solution.is_null(), "{}", error_text(error));
                let instance = pio_calculation_solution_instance(solution, &mut error);
                assert!(!instance.is_null(), "{}", error_text(error));

                pio_calculation_solution_release(solution);
                pio_value_release(value);
                pio_module_release(module);

                assert_eq!(
                    view_text(pio_calculation_instance_type_name(instance)),
                    expected_instance_type
                );
                if is_multiconductor {
                    let network =
                        pio_calculation_instance_multiconductor_network(instance, &mut error);
                    assert!(!network.is_null(), "{}", error_text(error));
                    assert!(pio_multiconductor_network_bus_count(network) > 0);
                    pio_multiconductor_network_release(network);
                } else {
                    let network = pio_calculation_instance_balanced_network(instance, &mut error);
                    assert!(!network.is_null(), "{}", error_text(error));
                    assert!(pio_balanced_network_bus_count(network) > 0);
                    pio_balanced_network_release(network);
                }
                pio_calculation_instance_release(instance);
            }
        }
    }

    #[test]
    fn balanced_rows_are_checked_borrowed_views() {
        unsafe {
            let module = parse_case9();
            let value = pio_module_value(module);
            let mut error = std::ptr::null_mut();
            let network = pio_value_balanced_network(value, &mut error);
            assert!(!network.is_null(), "{}", error_text(error));

            pio_module_release(module);
            pio_value_release(value);

            assert_eq!(pio_balanced_network_base_mva(network), 100.0);
            assert_eq!(pio_balanced_network_base_frequency_hz(network), 60.0);
            assert_eq!(pio_balanced_network_bus_count(network), 9);
            assert_eq!(pio_balanced_network_load_count(network), 3);
            assert_eq!(pio_balanced_network_shunt_count(network), 0);
            assert_eq!(pio_balanced_network_branch_count(network), 9);
            assert_eq!(pio_balanced_network_generator_count(network), 3);
            assert_eq!(pio_balanced_network_storage_count(network), 0);

            let mut bus = std::mem::MaybeUninit::<PioBalancedBusView>::uninit();
            assert!(pio_balanced_network_bus_at(
                network,
                0,
                bus.as_mut_ptr(),
                &mut error,
            ));
            let bus = bus.assume_init();
            assert_eq!(bus.id, 1);
            assert_eq!(view_text(bus.bus_type), "REF");
            assert_eq!(bus.base_kv, 345.0);
            assert!(bus.has_component_id);
            assert_eq!(view_text(bus.component_id), "1");

            let mut load = std::mem::MaybeUninit::<PioBalancedLoadView>::uninit();
            assert!(pio_balanced_network_load_at(
                network,
                0,
                load.as_mut_ptr(),
                &mut error,
            ));
            let load = load.assume_init();
            assert_eq!(load.bus_id, 5);
            assert_eq!(load.p_mw, 90.0);
            assert_eq!(load.q_mvar, 30.0);
            assert_eq!(view_text(load.voltage_model.kind), "constant_power");

            let mut branch = std::mem::MaybeUninit::<PioBalancedBranchView>::uninit();
            assert!(pio_balanced_network_branch_at(
                network,
                0,
                branch.as_mut_ptr(),
                &mut error,
            ));
            let branch = branch.assume_init();
            assert_eq!(branch.from_bus_id, 1);
            assert_eq!(branch.to_bus_id, 4);
            assert_eq!(branch.reactance_pu, 0.0576);
            assert_eq!(branch.rate_a_mva, 250.0);
            assert_eq!(branch.tap_ratio, 0.0);
            assert_eq!(branch.effective_tap_ratio, 1.0);
            assert!(!branch.terminal_charging_is_explicit);
            assert_eq!(branch.additional_rating_count, 0);

            let mut generator = std::mem::MaybeUninit::<PioBalancedGeneratorView>::uninit();
            assert!(pio_balanced_network_generator_at(
                network,
                0,
                generator.as_mut_ptr(),
                &mut error,
            ));
            let generator = generator.assume_init();
            assert_eq!(generator.bus_id, 1);
            assert_eq!(view_text(generator.energy_source), "other");
            assert_eq!(generator.active_power_mw, 72.3);
            assert!(generator.has_cost);
            assert!(generator.voltage_regulation_on);
            assert!(!generator.has_regulating_terminal);
            assert!(!generator.has_active_power_control);
            assert!(!generator.active_power_control.has_droop_percent);
            assert_eq!(generator.cost.model, 2);
            assert_eq!(generator.cost.ncost, 3);
            assert_eq!(generator.cost.coefficients.len, 3);
            let coefficients = std::slice::from_raw_parts(
                generator.cost.coefficients.data,
                generator.cost.coefficients.len,
            );
            assert_eq!(coefficients, &[0.11, 5.0, 150.0]);

            let mut capability = std::mem::MaybeUninit::<PioGeneratorCapabilityView>::uninit();
            assert!(pio_balanced_network_generator_capability_at(
                network,
                0,
                0,
                capability.as_mut_ptr(),
                &mut error,
            ));
            let capability = capability.assume_init();
            assert_eq!(view_text(capability.name), "pc1");
            assert!(capability.has_value);
            assert_eq!(capability.value, 0.0);

            let mut missing = std::mem::MaybeUninit::<PioBalancedBusView>::uninit();
            assert!(!pio_balanced_network_bus_at(
                network,
                9,
                missing.as_mut_ptr(),
                &mut error,
            ));
            assert_eq!(
                view_text(pio_error_code(error)),
                "BIND.CAPI.INDEX_OUT_OF_RANGE"
            );
            pio_error_release(error);

            pio_balanced_network_release(network);
        }
    }

    #[test]
    fn balanced_optional_row_fields_are_explicit() {
        unsafe {
            let mut network = BalancedNetwork::new("optional-fields", 100.0);
            network.buses_mut().push(powerio_tx::Bus::new(
                powerio_tx::BusId(1),
                powerio_tx::BusType::Ref,
                115.0,
            ));
            network.buses_mut().push(powerio_tx::Bus::new(
                powerio_tx::BusId(2),
                powerio_tx::BusType::Pq,
                115.0,
            ));

            let mut load = powerio_tx::Load::new(powerio_tx::BusId(2), 6.0, 3.0);
            load.voltage_model = Some(powerio_tx::LoadVoltageModel::Zip {
                p_constant_power: 3.0,
                q_constant_power: 1.5,
                p_constant_current: 2.0,
                q_constant_current: 1.0,
                p_constant_impedance: 1.0,
                q_constant_impedance: 0.5,
                v_nom: Some(1.0),
                load_type: Some(7),
                scaling: Some(0.9),
            });
            network.loads_mut().push(load);

            let mut shunt = powerio_tx::Shunt::new(powerio_tx::BusId(2), 0.0, 2.0);
            let mut control = powerio_tx::SwitchedShuntControl::new(
                powerio_tx::SwitchedShuntMode::Discrete,
                1.05,
                0.95,
                vec![powerio_tx::ShuntBlock::with_admittance(4, 0.25, 0.5)],
            );
            control.control_bus = Some(powerio_tx::BusId(1));
            shunt.control = Some(control);
            network.shunts_mut().push(shunt);

            let mut branch =
                powerio_tx::Branch::new(powerio_tx::BusId(1), powerio_tx::BusId(2), 0.01, 0.1);
            branch.name = Some("controlled transformer".to_owned());
            branch.charging = Some(powerio_tx::BranchCharging::new(0.01, 0.02, 0.03, 0.04));
            branch
                .rating_sets
                .push(powerio_tx::BranchRatingSet::new("LTE", 175.0));
            branch.current_ratings = Some(powerio_tx::BranchCurrentRatings::new(1.0, 2.0, 3.0));
            branch.tap = 1.05;
            branch.shift = 4.0;
            branch.angmin = -30.0;
            branch.angmax = 30.0;
            let mut transformer_control = powerio_tx::TransformerControl::new(
                powerio_tx::TransformerControlMode::DcLineQuantity,
            );
            transformer_control.enabled = false;
            transformer_control.controlled_bus = Some(powerio_tx::BusId(2));
            transformer_control.controlled_bus_on_winding_side = true;
            transformer_control.regulating_terminal = Some(
                serde_json::from_value(serde_json::json!({
                    "equipment": {
                        "component_type": "transformer",
                        "local_id": "controlled-transformer"
                    },
                    "terminal": 2
                }))
                .unwrap(),
            );
            transformer_control.ntp = 17;
            transformer_control.winding_connection_angle = Some(12.5);
            branch.control = Some(transformer_control);
            network.branches_mut().push(branch);

            let mut generator = powerio_tx::Generator::new(powerio_tx::BusId(1));
            generator.energy_source = powerio_tx::GeneratorEnergySource::Nuclear;
            generator.cost = Some(powerio_tx::GenCost::new(2, 10.0, 20.0, vec![1.0, 2.0]));
            generator.caps[8] = Some(25.0);
            generator.voltage_regulation_on = false;
            generator.regulating_terminal = Some(
                serde_json::from_value(serde_json::json!({
                    "equipment": {
                        "component_type": "load",
                        "local_id": "regulated-load"
                    },
                    "terminal": 1
                }))
                .unwrap(),
            );
            generator.regulated_bus = Some(powerio_tx::BusId(2));
            let mut generator_control = powerio_tx::ActivePowerControl::new(true);
            generator_control.droop_percent = Some(4.0);
            generator_control.participation_factor = Some(0.6);
            generator_control.minimum_target_active_power_mw = Some(10.0);
            generator_control.maximum_target_active_power_mw = Some(100.0);
            generator.active_power_control = Some(generator_control);
            network.generators_mut().push(generator);

            let mut storage = powerio_tx::Storage::new(powerio_tx::BusId(2));
            storage.energy = 2.0;
            storage.energy_rating = 10.0;
            storage.current_rating = Some(100.0);
            let mut storage_control = powerio_tx::ActivePowerControl::new(false);
            storage_control.participation_factor = Some(0.4);
            storage.active_power_control = Some(storage_control);
            network.storage_mut().push(storage);

            let module = module_handle(powerio::PioModule::new(PioValue::from(network)));
            let value = pio_module_value(module);
            let mut error = std::ptr::null_mut();
            let network = pio_value_balanced_network(value, &mut error);

            let mut load = std::mem::MaybeUninit::<PioBalancedLoadView>::uninit();
            assert!(pio_balanced_network_load_at(
                network,
                0,
                load.as_mut_ptr(),
                &mut error
            ));
            let load = load.assume_init();
            assert_eq!(view_text(load.voltage_model.kind), "zip");
            assert_eq!(load.voltage_model.p_constant_current_mw, 2.0);
            assert!(load.voltage_model.has_nominal_voltage);
            assert!(load.voltage_model.has_load_type);
            assert!(load.voltage_model.has_scaling);

            let mut shunt = std::mem::MaybeUninit::<PioBalancedShuntView>::uninit();
            assert!(pio_balanced_network_shunt_at(
                network,
                0,
                shunt.as_mut_ptr(),
                &mut error
            ));
            let shunt = shunt.assume_init();
            assert!(!shunt.has_section_count);
            assert_eq!(shunt.section_count, 0);
            assert!(shunt.has_control);
            assert_eq!(view_text(shunt.control_mode), "discrete");
            assert_eq!(shunt.control_block_count, 1);
            assert!(shunt.has_control_bus);
            let mut block = std::mem::MaybeUninit::<PioShuntBlockView>::uninit();
            assert!(pio_balanced_network_shunt_block_at(
                network,
                0,
                0,
                block.as_mut_ptr(),
                &mut error,
            ));
            let block = block.assume_init();
            assert_eq!(block.steps, 4);
            assert_eq!(block.conductance_mw, 0.25);
            assert_eq!(block.susceptance_mvar, 0.5);

            let mut branch = std::mem::MaybeUninit::<PioBalancedBranchView>::uninit();
            assert!(pio_balanced_network_branch_at(
                network,
                0,
                branch.as_mut_ptr(),
                &mut error,
            ));
            let branch = branch.assume_init();
            assert!(branch.has_name);
            assert_eq!(view_text(branch.name), "controlled transformer");
            assert!(branch.has_control);
            assert_eq!(view_text(branch.control.mode), "dc_line_quantity");
            assert!(!branch.control.enabled);
            assert!(branch.control.has_controlled_bus);
            assert_eq!(branch.control.controlled_bus_id, 2);
            assert!(branch.control.controlled_bus_on_winding_side);
            assert!(branch.control.has_regulating_terminal);
            assert_eq!(branch.control.regulating_terminal.terminal, 2);
            assert_eq!(branch.control.tap_position_count, 17);
            assert!(branch.control.has_winding_connection_angle);
            assert_eq!(branch.control.winding_connection_angle, 12.5);
            assert!(branch.terminal_charging_is_explicit);
            assert_eq!(branch.from_conductance_pu, 0.01);
            assert_eq!(branch.to_susceptance_pu, 0.04);
            assert!(branch.has_current_ratings);
            assert_eq!(branch.current_rating_c, 3.0);
            assert_eq!(branch.additional_rating_count, 1);
            let mut rating = std::mem::MaybeUninit::<PioBranchRatingView>::uninit();
            assert!(pio_balanced_network_branch_rating_at(
                network,
                0,
                0,
                rating.as_mut_ptr(),
                &mut error,
            ));
            let rating = rating.assume_init();
            assert_eq!(view_text(rating.name), "LTE");
            assert_eq!(rating.rate_mva, 175.0);

            let mut generator = std::mem::MaybeUninit::<PioBalancedGeneratorView>::uninit();
            assert!(pio_balanced_network_generator_at(
                network,
                0,
                generator.as_mut_ptr(),
                &mut error,
            ));
            let generator = generator.assume_init();
            assert!(generator.has_regulated_bus);
            assert_eq!(generator.regulated_bus_id, 2);
            assert!(!generator.voltage_regulation_on);
            assert!(generator.has_regulating_terminal);
            assert_eq!(view_text(generator.energy_source), "nuclear");
            assert_eq!(
                view_text(generator.regulating_terminal.equipment.component_type),
                "load"
            );
            assert_eq!(
                view_text(generator.regulating_terminal.equipment.local_id),
                "regulated-load"
            );
            assert_eq!(generator.regulating_terminal.terminal, 1);
            assert!(generator.has_active_power_control);
            assert!(generator.active_power_control.participate);
            assert!(generator.active_power_control.has_droop_percent);
            assert_eq!(generator.active_power_control.droop_percent, 4.0);
            assert!(generator.active_power_control.has_participation_factor);
            assert_eq!(generator.active_power_control.participation_factor, 0.6);
            assert!(
                generator
                    .active_power_control
                    .has_minimum_target_active_power
            );
            assert_eq!(
                generator
                    .active_power_control
                    .minimum_target_active_power_mw,
                10.0
            );
            assert!(
                generator
                    .active_power_control
                    .has_maximum_target_active_power
            );
            assert_eq!(
                generator
                    .active_power_control
                    .maximum_target_active_power_mw,
                100.0
            );
            let mut capability = std::mem::MaybeUninit::<PioGeneratorCapabilityView>::uninit();
            assert!(pio_balanced_network_generator_capability_at(
                network,
                0,
                8,
                capability.as_mut_ptr(),
                &mut error,
            ));
            let capability = capability.assume_init();
            assert_eq!(view_text(capability.name), "ramp_30");
            assert!(capability.has_value);
            assert_eq!(capability.value, 25.0);

            let mut storage = std::mem::MaybeUninit::<PioBalancedStorageView>::uninit();
            assert!(pio_balanced_network_storage_at(
                network,
                0,
                storage.as_mut_ptr(),
                &mut error,
            ));
            let storage = storage.assume_init();
            assert_eq!(storage.energy_mwh, 2.0);
            assert_eq!(storage.energy_rating_mwh, 10.0);
            assert!(storage.has_current_rating);
            assert_eq!(storage.current_rating, 100.0);
            assert!(storage.has_active_power_control);
            assert!(!storage.active_power_control.participate);
            assert!(!storage.active_power_control.has_droop_percent);
            assert!(storage.active_power_control.has_participation_factor);
            assert_eq!(storage.active_power_control.participation_factor, 0.4);

            pio_balanced_network_release(network);
            pio_value_release(value);
            pio_module_release(module);
        }
    }

    #[test]
    fn powsybl_balanced_tables_are_borrowed_without_serialization() {
        unsafe {
            let mut network = BalancedNetwork::new("powsybl-tables", 100.0);
            for (id, kind, kv) in [
                (1, powerio_tx::BusType::Ref, 400.0),
                (2, powerio_tx::BusType::Pq, 225.0),
                (3, powerio_tx::BusType::Pq, 63.0),
            ] {
                network
                    .buses_mut()
                    .push(powerio_tx::Bus::new(powerio_tx::BusId(id), kind, kv));
            }

            let mut shunt = powerio_tx::Shunt::new(powerio_tx::BusId(2), 0.0, 12.0);
            shunt.uid = Some("SH".to_owned());
            shunt.section_count = Some(2);
            network.shunts_mut().push(shunt);

            let mut svc = powerio_tx::StaticVarCompensator::new(powerio_tx::BusId(2), -0.02, 0.03);
            svc.uid = Some("SVC".to_owned());
            svc.regulating = true;
            svc.regulation_mode = powerio_tx::StaticVarCompensatorRegulationMode::ReactivePower;
            svc.reactive_power_setpoint_mvar = 12.0;
            svc.regulating_terminal = Some(
                serde_json::from_value(serde_json::json!({
                    "equipment": {
                        "component_type": "static_var_compensator",
                        "local_id": "SVC"
                    },
                    "terminal": 1
                }))
                .unwrap(),
            );
            network.static_var_compensators_mut().push(svc);

            let mut switch =
                powerio_tx::Switch::new(powerio_tx::BusId(1), powerio_tx::BusId(2), true);
            switch.uid = Some("SW".to_owned());
            switch.thermal_rating = Some(500.0);
            switch.pf = Some(75.0);
            network.switches_mut().push(switch);

            let mut hvdc = powerio_tx::Hvdc::new(powerio_tx::BusId(1), powerio_tx::BusId(2));
            hvdc.uid = Some("HVDC".to_owned());
            hvdc.resistance_ohm = Some(1.5);
            hvdc.nominal_voltage_kv = Some(320.0);
            hvdc.converters_mode =
                Some(powerio_tx::HvdcConvertersMode::Side1RectifierSide2Inverter);
            hvdc.converter1 = Some(
                serde_json::from_value(serde_json::json!({
                    "component": {
                        "component_type": "hvdc_converter",
                        "local_id": "C1"
                    },
                    "kind": "vsc",
                    "loss_factor_percent": 1.0,
                    "voltage_regulator_on": true,
                    "voltage_setpoint_kv": 400.0
                }))
                .unwrap(),
            );
            network.hvdc_mut().push(hvdc);

            let mut windings = [
                powerio_tx::Winding::new(powerio_tx::BusId(1)),
                powerio_tx::Winding::new(powerio_tx::BusId(2)),
                powerio_tx::Winding::new(powerio_tx::BusId(3)),
            ];
            let mut winding_control =
                powerio_tx::TransformerControl::new(powerio_tx::TransformerControlMode::ActiveFlow);
            winding_control.controlled_bus = Some(powerio_tx::BusId(3));
            winding_control.regulating_terminal = Some(
                serde_json::from_value(serde_json::json!({
                    "equipment": {
                        "component_type": "transformer",
                        "local_id": "T3"
                    },
                    "terminal": 3
                }))
                .unwrap(),
            );
            winding_control.ntp = 21;
            windings[2].control = Some(winding_control);
            let mut transformer = powerio_tx::Transformer3W::new(
                windings,
                [
                    powerio_tx::Impedance::new(0.01, 0.1, 100.0),
                    powerio_tx::Impedance::new(0.02, 0.2, 100.0),
                    powerio_tx::Impedance::new(0.03, 0.3, 100.0),
                ],
            );
            transformer.uid = Some("T3".to_owned());
            transformer.name = Some("three winding".to_owned());
            network.transformers_3w_mut().push(transformer);

            let mut area = powerio_tx::Area::new(7);
            area.uid = Some("A7".to_owned());
            area.name = Some("control area".to_owned());
            area.area_type = Some("control_area".to_owned());
            area.slack_bus = Some(powerio_tx::BusId(1));
            area.net_interchange = 12.5;
            network.areas_mut().push(area);

            let module = module_handle(powerio::PioModule::new(PioValue::from(network)));
            let value = pio_module_value(module);
            let mut error = std::ptr::null_mut();
            let network = pio_value_balanced_network(value, &mut error);
            pio_value_release(value);
            pio_module_release(module);

            assert_eq!(
                pio_balanced_network_static_var_compensator_count(network),
                1
            );
            assert_eq!(pio_balanced_network_shunt_count(network), 1);
            assert_eq!(pio_balanced_network_switch_count(network), 1);
            assert_eq!(pio_balanced_network_hvdc_count(network), 1);
            assert_eq!(
                pio_balanced_network_three_winding_transformer_count(network),
                1
            );
            assert_eq!(pio_balanced_network_area_count(network), 1);

            let mut svc = std::mem::MaybeUninit::<PioBalancedStaticVarCompensatorView>::uninit();
            assert!(pio_balanced_network_static_var_compensator_at(
                network,
                0,
                svc.as_mut_ptr(),
                &mut error,
            ));
            let svc = svc.assume_init();
            assert_eq!(view_text(svc.component_id), "SVC");
            assert_eq!(view_text(svc.regulation_mode), "reactive_power");
            assert!(svc.has_regulating_terminal);

            let mut shunt = std::mem::MaybeUninit::<PioBalancedShuntView>::uninit();
            assert!(pio_balanced_network_shunt_at(
                network,
                0,
                shunt.as_mut_ptr(),
                &mut error,
            ));
            let shunt = shunt.assume_init();
            assert!(shunt.has_section_count);
            assert_eq!(shunt.section_count, 2);

            let mut switch = std::mem::MaybeUninit::<PioBalancedSwitchView>::uninit();
            assert!(pio_balanced_network_switch_at(
                network,
                0,
                switch.as_mut_ptr(),
                &mut error,
            ));
            let switch = switch.assume_init();
            assert!(switch.closed);
            assert_eq!(switch.thermal_rating_mva, 500.0);
            assert!(switch.has_from_active_power);

            let mut hvdc = std::mem::MaybeUninit::<PioBalancedHvdcView>::uninit();
            assert!(pio_balanced_network_hvdc_at(
                network,
                0,
                hvdc.as_mut_ptr(),
                &mut error,
            ));
            let hvdc = hvdc.assume_init();
            assert!(hvdc.has_converter1);
            assert_eq!(view_text(hvdc.converter1.kind), "vsc");
            assert_eq!(hvdc.nominal_voltage_kv, 320.0);

            let mut transformer =
                std::mem::MaybeUninit::<PioBalancedThreeWindingTransformerView>::uninit();
            assert!(pio_balanced_network_three_winding_transformer_at(
                network,
                0,
                transformer.as_mut_ptr(),
                &mut error,
            ));
            assert_eq!(transformer.assume_init().winding_count, 3);
            let mut winding =
                std::mem::MaybeUninit::<PioThreeWindingTransformerWindingView>::uninit();
            assert!(pio_balanced_network_three_winding_transformer_winding_at(
                network,
                0,
                2,
                winding.as_mut_ptr(),
                &mut error,
            ));
            let winding = winding.assume_init();
            assert_eq!(winding.bus_id, 3);
            assert!(winding.has_control);
            assert_eq!(view_text(winding.control.mode), "active_flow");
            assert!(winding.control.enabled);
            assert_eq!(winding.control.controlled_bus_id, 3);
            assert!(winding.control.has_regulating_terminal);
            assert_eq!(winding.control.regulating_terminal.terminal, 3);
            assert_eq!(winding.control.tap_position_count, 21);

            let mut area = std::mem::MaybeUninit::<PioBalancedAreaView>::uninit();
            assert!(pio_balanced_network_area_at(
                network,
                0,
                area.as_mut_ptr(),
                &mut error,
            ));
            let area = area.assume_init();
            assert_eq!(view_text(area.component_id), "A7");
            assert_eq!(area.slack_bus_id, 1);

            pio_balanced_network_release(network);
        }
    }

    #[test]
    fn detailed_connectivity_tables_are_owner_rooted_views() {
        const HIERARCHY: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<iidm:network xmlns:iidm="http://www.powsybl.org/schema/iidm/1_17" id="details" caseDate="2026-01-01T00:00:00Z" forecastDistance="0" sourceFormat="test" minimumValidationLevel="STEADY_STATE_HYPOTHESIS">
  <iidm:substation id="S" country="US" tso="MISO" geographicalTags="east">
    <iidm:voltageLevel id="VL1" nominalV="132" topologyKind="BUS_BREAKER"><iidm:busBreakerTopology><iidm:bus id="B1"/></iidm:busBreakerTopology></iidm:voltageLevel>
    <iidm:voltageLevel id="VL2" nominalV="33" topologyKind="BUS_BREAKER"><iidm:busBreakerTopology><iidm:bus id="B2"/></iidm:busBreakerTopology></iidm:voltageLevel>
    <iidm:voltageLevel id="VL3" nominalV="11" topologyKind="BUS_BREAKER"><iidm:busBreakerTopology><iidm:bus id="B3"/></iidm:busBreakerTopology></iidm:voltageLevel>
    <iidm:threeWindingsTransformer id="T3" ratedU0="132" ratedU1="132" ratedU2="33" ratedU3="11" r1="17.424" x1="34.848" r2="1.7424" x2="3.4848" r3="0.8712" x3="1.7424" bus1="B1" connectableBus1="B1" voltageLevelId1="VL1" bus2="B2" connectableBus2="B2" voltageLevelId2="VL2" bus3="B3" connectableBus3="B3" voltageLevelId3="VL3" selectedOperationalLimitsGroupIds1="normal">
      <iidm:ratioTapChanger2 tapPosition="0" lowTapPosition="0" loadTapChangingCapabilities="false"><iidm:step rho="1.05"/></iidm:ratioTapChanger2>
      <iidm:operationalLimitsGroup1 id="normal"><iidm:apparentPowerLimits permanentLimit="90"><iidm:temporaryLimit name="emergency" acceptableDuration="600" value="100"/></iidm:apparentPowerLimits></iidm:operationalLimitsGroup1>
    </iidm:threeWindingsTransformer>
  </iidm:substation>
</iidm:network>"#;

        unsafe {
            let module = parse_xiidm_text(HIERARCHY);
            let mut identified_network = match &PioModule::get(module).unwrap().module.value() {
                PioValue::BalancedNetwork(network) => network.clone(),
                value => panic!("expected balanced network, got {}", value.type_name()),
            };
            let identified_details = Arc::make_mut(
                identified_network
                    .detailed_connectivity_mut()
                    .as_mut()
                    .unwrap(),
            );
            identified_details.terminals[0].component =
                Some(ComponentId::new("terminal", "terminal-T3-1").unwrap());
            identified_details.tap_changers[0].component =
                Some(ComponentId::new("tap_changer", "tap-T3-2").unwrap());
            let value = pio_module_value(module);
            let mut error = std::ptr::null_mut();
            let network = pio_value_balanced_network(value, &mut error);
            assert!(pio_balanced_network_has_detailed_connectivity(network));
            let details = pio_balanced_network_detailed_connectivity(network);
            assert!(!details.is_null());
            pio_value_release(value);
            pio_module_release(module);
            pio_balanced_network_release(network);

            let mut counts = std::mem::MaybeUninit::<PioDetailedConnectivityCountsView>::uninit();
            assert!(pio_detailed_connectivity_counts(
                details,
                counts.as_mut_ptr(),
                &mut error,
            ));
            let counts = counts.assume_init();
            assert_eq!(counts.substations, 1);
            assert_eq!(counts.voltage_levels, 3);
            assert_eq!(counts.terminals, 3);
            assert_eq!(counts.operational_limit_groups, 1);
            assert_eq!(counts.tap_changers, 1);

            let mut substation = std::mem::MaybeUninit::<PioSubstationView>::uninit();
            assert!(pio_detailed_connectivity_substation_at(
                details,
                0,
                substation.as_mut_ptr(),
                &mut error,
            ));
            let substation = substation.assume_init();
            assert_eq!(view_text(substation.component.local_id), "S");
            assert_eq!(substation.geographical_tag_count, 1);

            let mut level = std::mem::MaybeUninit::<PioVoltageLevelView>::uninit();
            assert!(pio_detailed_connectivity_voltage_level_at(
                details,
                0,
                level.as_mut_ptr(),
                &mut error,
            ));
            let level = level.assume_init();
            assert_eq!(view_text(level.topology_kind), "bus_breaker");
            assert_eq!(level.nominal_voltage_kv, 132.0);

            let mut terminal = std::mem::MaybeUninit::<PioDetailedTerminalView>::uninit();
            assert!(pio_detailed_connectivity_terminal_at(
                details,
                0,
                terminal.as_mut_ptr(),
                &mut error,
            ));
            let terminal = terminal.assume_init();
            assert!(terminal.connected);
            assert!(!terminal.has_component);

            let mut limit = std::mem::MaybeUninit::<PioOperationalLimitGroupView>::uninit();
            assert!(pio_detailed_connectivity_operational_limit_group_at(
                details,
                0,
                limit.as_mut_ptr(),
                &mut error,
            ));
            let limit = limit.assume_init();
            assert!(limit.has_apparent_power_limits);
            assert_eq!(limit.apparent_power_permanent_limit_mva, 90.0);
            assert_eq!(limit.apparent_power_temporary_limit_count, 1);

            let mut changer = std::mem::MaybeUninit::<PioTapChangerView>::uninit();
            assert!(pio_detailed_connectivity_tap_changer_at(
                details,
                0,
                changer.as_mut_ptr(),
                &mut error,
            ));
            let changer = changer.assume_init();
            assert!(!changer.has_component);
            assert_eq!(view_text(changer.kind), "ratio");
            assert!(changer.has_tap_position);
            assert_eq!(changer.tap_position, 0);
            assert_eq!(changer.step_count, 1);

            pio_detailed_connectivity_release(details);

            let module = module_handle(powerio::PioModule::new(PioValue::BalancedNetwork(
                identified_network,
            )));
            let value = pio_module_value(module);
            let network = pio_value_balanced_network(value, &mut error);
            let details = pio_balanced_network_detailed_connectivity(network);
            pio_value_release(value);
            pio_module_release(module);
            pio_balanced_network_release(network);

            let mut terminal = std::mem::MaybeUninit::<PioDetailedTerminalView>::uninit();
            assert!(pio_detailed_connectivity_terminal_at(
                details,
                0,
                terminal.as_mut_ptr(),
                &mut error,
            ));
            let terminal = terminal.assume_init();
            assert!(terminal.has_component);
            assert_eq!(view_text(terminal.component.component_type), "terminal");
            assert_eq!(view_text(terminal.component.local_id), "terminal-T3-1");

            let mut changer = std::mem::MaybeUninit::<PioTapChangerView>::uninit();
            assert!(pio_detailed_connectivity_tap_changer_at(
                details,
                0,
                changer.as_mut_ptr(),
                &mut error,
            ));
            let changer = changer.assume_init();
            assert!(changer.has_component);
            assert_eq!(view_text(changer.component.component_type), "tap_changer");
            assert_eq!(view_text(changer.component.local_id), "tap-T3-2");
            pio_detailed_connectivity_release(details);

            let without_assigned_tap = HIERARCHY.replace(r#" tapPosition="0""#, "");
            let module = parse_xiidm_text(&without_assigned_tap);
            let value = pio_module_value(module);
            let network = pio_value_balanced_network(value, &mut error);
            let details = pio_balanced_network_detailed_connectivity(network);
            let mut changer = std::mem::MaybeUninit::<PioTapChangerView>::uninit();
            assert!(pio_detailed_connectivity_tap_changer_at(
                details,
                0,
                changer.as_mut_ptr(),
                &mut error,
            ));
            let changer = changer.assume_init();
            assert!(!changer.has_tap_position);
            assert_eq!(changer.tap_position, 0);
            pio_detailed_connectivity_release(details);
            pio_balanced_network_release(network);
            pio_value_release(value);
            pio_module_release(module);
        }
    }

    #[test]
    fn subnetworks_boundary_lines_and_tie_lines_are_owner_rooted_views() {
        const XIIDM: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<iidm:network xmlns:iidm="http://www.powsybl.org/schema/iidm/1_17" id="Merged" caseDate="2026-01-01T00:00:00Z" forecastDistance="0" sourceFormat="root" minimumValidationLevel="STEADY_STATE_HYPOTHESIS">
  <iidm:network id="A" caseDate="2026-01-01T01:00:00Z" forecastDistance="1" sourceFormat="part-a" minimumValidationLevel="STEADY_STATE_HYPOTHESIS">
    <iidm:substation id="SA"><iidm:voltageLevel id="VA" nominalV="100" topologyKind="BUS_BREAKER"><iidm:busBreakerTopology><iidm:bus id="BA"/></iidm:busBreakerTopology><iidm:boundaryLine id="DLA" p0="5" q0="6" r="1" x="2" generationVoltageRegulationOn="true" generationMinP="0" generationMaxP="20" generationTargetP="10" generationTargetV="100" bus="BA" connectableBus="BA"><iidm:reactiveCapabilityCurve><iidm:property name="owner" value="RTE"/><iidm:point p="0" minQ="-10" maxQ="10"/><iidm:point p="10" minQ="0" maxQ="20"/></iidm:reactiveCapabilityCurve></iidm:boundaryLine></iidm:voltageLevel></iidm:substation>
  </iidm:network>
  <iidm:network id="B" caseDate="2026-01-01T02:00:00Z" forecastDistance="2" sourceFormat="part-b" minimumValidationLevel="STEADY_STATE_HYPOTHESIS">
    <iidm:substation id="SB"><iidm:voltageLevel id="VB" nominalV="100" topologyKind="BUS_BREAKER"><iidm:busBreakerTopology><iidm:bus id="BB"/></iidm:busBreakerTopology><iidm:boundaryLine id="DLB" p0="-5" q0="-6" r="3" x="4" bus="BB" connectableBus="BB"/></iidm:voltageLevel></iidm:substation>
  </iidm:network>
  <iidm:tieLine id="TL" boundaryLineId1="DLA" boundaryLineId2="DLB"/>
</iidm:network>"#;

        unsafe {
            let module = parse_xiidm_text(XIIDM);
            let value = pio_module_value(module);
            let mut error = std::ptr::null_mut();
            let network = pio_value_balanced_network(value, &mut error);
            let details = pio_balanced_network_detailed_connectivity(network);
            pio_value_release(value);
            pio_module_release(module);
            pio_balanced_network_release(network);

            let mut counts = std::mem::MaybeUninit::<PioDetailedConnectivityCountsView>::uninit();
            assert!(pio_detailed_connectivity_counts(
                details,
                counts.as_mut_ptr(),
                &mut error,
            ));
            let counts = counts.assume_init();
            assert_eq!(counts.subnetworks, 2);
            assert_eq!(counts.boundary_lines, 2);
            assert_eq!(counts.tie_lines, 1);

            let mut subnetwork = std::mem::MaybeUninit::<PioSubnetworkView>::uninit();
            assert!(pio_detailed_connectivity_subnetwork_at(
                details,
                0,
                subnetwork.as_mut_ptr(),
                &mut error,
            ));
            let subnetwork = subnetwork.assume_init();
            assert_eq!(view_text(subnetwork.component.local_id), "A");
            assert_eq!(view_text(subnetwork.parent.local_id), "Merged");
            assert!(subnetwork.case_metadata.has_forecast_distance);
            assert_eq!(subnetwork.case_metadata.forecast_distance, 1);
            assert_eq!(
                view_text(subnetwork.case_metadata.source_model_format),
                "part-a"
            );
            assert!(subnetwork.component_count >= 4);

            let mut member = std::mem::MaybeUninit::<PioComponentIdView>::uninit();
            assert!(pio_detailed_connectivity_subnetwork_component_at(
                details,
                0,
                0,
                member.as_mut_ptr(),
                &mut error,
            ));
            assert!(!view_text(member.assume_init().local_id).is_empty());

            let mut boundary = std::mem::MaybeUninit::<PioBoundaryLineView>::uninit();
            assert!(pio_detailed_connectivity_boundary_line_at(
                details,
                0,
                boundary.as_mut_ptr(),
                &mut error,
            ));
            let boundary = boundary.assume_init();
            assert_eq!(view_text(boundary.component.local_id), "DLA");
            assert_eq!(boundary.active_power_setpoint_mw, 5.0);
            assert!(boundary.has_generation);
            assert!(boundary.generation.voltage_regulation_on);
            assert_eq!(boundary.generation.target_active_power_mw, 10.0);
            assert!(boundary.generation.has_reactive_limits);
            assert_eq!(
                view_text(boundary.generation.reactive_limits.kind),
                "capability_curve"
            );
            assert_eq!(boundary.generation.reactive_limits.point_count, 2);

            let mut property = std::mem::MaybeUninit::<PioStringPropertyView>::uninit();
            assert!(
                pio_detailed_connectivity_boundary_line_reactive_limit_property_at(
                    details,
                    0,
                    0,
                    property.as_mut_ptr(),
                    &mut error,
                )
            );
            let property = property.assume_init();
            assert_eq!(view_text(property.name), "owner");
            assert_eq!(view_text(property.value), "RTE");

            let mut point = std::mem::MaybeUninit::<PioReactiveCapabilityCurvePointView>::uninit();
            assert!(
                pio_detailed_connectivity_boundary_line_reactive_capability_point_at(
                    details,
                    0,
                    1,
                    point.as_mut_ptr(),
                    &mut error,
                )
            );
            let point = point.assume_init();
            assert_eq!(point.active_power_mw, 10.0);
            assert_eq!(point.maximum_reactive_power_mvar, 20.0);

            let mut tie = std::mem::MaybeUninit::<PioTieLineView>::uninit();
            assert!(pio_detailed_connectivity_tie_line_at(
                details,
                0,
                tie.as_mut_ptr(),
                &mut error,
            ));
            let tie = tie.assume_init();
            assert_eq!(view_text(tie.component.local_id), "TL");
            assert_eq!(view_text(tie.boundary_line1.local_id), "DLA");
            assert_eq!(view_text(tie.boundary_line2.local_id), "DLB");
            assert!(tie.has_calculation_branch);

            pio_detailed_connectivity_release(details);
        }
    }

    #[test]
    fn connectivity_node_numbers_are_optional_borrowed_fields() {
        const XIIDM: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<iidm:network xmlns:iidm="http://www.powsybl.org/schema/iidm/1_17" id="nodes" caseDate="2025-01-01T00:00:00Z" forecastDistance="0" sourceFormat="test" minimumValidationLevel="STEADY_STATE_HYPOTHESIS">
  <iidm:substation id="S">
    <iidm:voltageLevel id="VL" nominalV="110" topologyKind="NODE_BREAKER">
      <iidm:nodeBreakerTopology>
        <iidm:bus v="110" angle="0" nodes="0,1,2"/>
        <iidm:busbarSection id="BBS" node="2"/>
        <iidm:switch id="BR" kind="BREAKER" open="false" node1="1" node2="2"/>
        <iidm:internalConnection node1="0" node2="1"/>
      </iidm:nodeBreakerTopology>
      <iidm:generator id="G" energySource="OTHER" minP="0" maxP="10" voltageRegulatorOn="true" targetP="5" node="0"><iidm:minMaxReactiveLimits minQ="-2" maxQ="2"/></iidm:generator>
    </iidm:voltageLevel>
  </iidm:substation>
</iidm:network>"#;

        unsafe {
            let module = parse_xiidm_text(XIIDM);
            let value = pio_module_value(module);
            let mut error = std::ptr::null_mut();
            let network = pio_value_balanced_network(value, &mut error);
            let details = pio_balanced_network_detailed_connectivity(network);

            let mut numbers = Vec::new();
            for index in 0..3 {
                let mut node = std::mem::MaybeUninit::<PioConnectivityNodeView>::uninit();
                assert!(pio_detailed_connectivity_node_at(
                    details,
                    index,
                    node.as_mut_ptr(),
                    &mut error,
                ));
                let node = node.assume_init();
                assert!(node.has_node_number);
                assert!(node.has_calculated_bus);
                numbers.push(node.node_number);
            }
            numbers.sort_unstable();
            assert_eq!(numbers, [0, 1, 2]);

            pio_detailed_connectivity_release(details);
            pio_balanced_network_release(network);
            pio_value_release(value);
            pio_module_release(module);
        }
    }

    #[test]
    fn omitted_fields_and_equipment_reactive_limits_are_typed_views() {
        const XIIDM: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<iidm:network xmlns:iidm="http://www.powsybl.org/schema/iidm/equipment/1_12" id="equipment" caseDate="2021-01-03T00:00:00Z" forecastDistance="0" sourceFormat="test" minimumValidationLevel="EQUIPMENT">
  <iidm:substation id="S"><iidm:voltageLevel id="VL" nominalV="225" topologyKind="NODE_BREAKER">
    <iidm:nodeBreakerTopology><iidm:busbarSection id="BBS" node="0"/></iidm:nodeBreakerTopology>
    <iidm:generator id="G" energySource="SOLAR" minP="0" maxP="100" voltageRegulatorOn="true" node="0">
      <iidm:reactiveCapabilityCurve><iidm:property name="curve" value="retained"/><iidm:point p="0" minQ="-20" maxQ="20"><iidm:property name="point" value="first"/></iidm:point><iidm:point p="100" minQ="-10" maxQ="10"/></iidm:reactiveCapabilityCurve>
    </iidm:generator>
  </iidm:voltageLevel></iidm:substation>
</iidm:network>"#;

        unsafe {
            let module = parse_xiidm_text(XIIDM);
            let value = pio_module_value(module);
            let mut error = std::ptr::null_mut();
            let network = pio_value_balanced_network(value, &mut error);
            let details = pio_balanced_network_detailed_connectivity(network);

            let mut counts = std::mem::MaybeUninit::<PioDetailedConnectivityCountsView>::uninit();
            assert!(pio_detailed_connectivity_counts(
                details,
                counts.as_mut_ptr(),
                &mut error,
            ));
            let counts = counts.assume_init();
            assert_eq!(counts.omitted_fields, 4);
            assert_eq!(counts.equipment_reactive_limits, 1);

            let mut omitted = std::mem::MaybeUninit::<PioOmittedFieldView>::uninit();
            assert!(pio_detailed_connectivity_omitted_field_at(
                details,
                2,
                omitted.as_mut_ptr(),
                &mut error,
            ));
            let omitted = omitted.assume_init();
            assert_eq!(view_text(omitted.component.local_id), "G");
            assert_eq!(view_text(omitted.field), "voltage_setpoint");

            let mut limits = std::mem::MaybeUninit::<PioEquipmentReactiveLimitsView>::uninit();
            assert!(pio_detailed_connectivity_equipment_reactive_limits_at(
                details,
                0,
                limits.as_mut_ptr(),
                &mut error,
            ));
            let limits = limits.assume_init();
            assert_eq!(view_text(limits.equipment.local_id), "G");
            assert_eq!(view_text(limits.limits.kind), "capability_curve");
            assert_eq!(limits.limits.property_count, 1);
            assert_eq!(limits.limits.point_count, 2);

            let mut point = std::mem::MaybeUninit::<PioReactiveCapabilityCurvePointView>::uninit();
            assert!(
                pio_detailed_connectivity_equipment_reactive_capability_point_at(
                    details,
                    0,
                    0,
                    point.as_mut_ptr(),
                    &mut error,
                )
            );
            let point = point.assume_init();
            assert_eq!(point.minimum_reactive_power_mvar, -20.0);
            assert_eq!(point.property_count, 1);

            let mut property = std::mem::MaybeUninit::<PioStringPropertyView>::uninit();
            assert!(
                pio_detailed_connectivity_equipment_reactive_capability_point_property_at(
                    details,
                    0,
                    0,
                    0,
                    property.as_mut_ptr(),
                    &mut error,
                )
            );
            let property = property.assume_init();
            assert_eq!(view_text(property.name), "point");
            assert_eq!(view_text(property.value), "first");

            let mut generator = std::mem::MaybeUninit::<PioBalancedGeneratorView>::uninit();
            assert!(pio_balanced_network_generator_at(
                network,
                0,
                generator.as_mut_ptr(),
                &mut error,
            ));
            assert_eq!(view_text(generator.assume_init().energy_source), "solar");

            pio_detailed_connectivity_release(details);
            pio_balanced_network_release(network);
            pio_value_release(value);
            pio_module_release(module);
        }
    }

    #[test]
    fn detailed_dc_and_converter_records_are_typed_views() {
        const DC: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<iidm:network xmlns:iidm="http://www.powsybl.org/schema/iidm/1_17" id="dc" caseDate="2026-01-01T00:00:00Z" forecastDistance="0" sourceFormat="test" minimumValidationLevel="STEADY_STATE_HYPOTHESIS">
  <iidm:dcNode id="N1" nominalV="500" v="498"/><iidm:dcNode id="N2" nominalV="500"/>
  <iidm:dcSwitch id="S" dcNode1="N1" dcNode2="N2" kind="DISCONNECTOR" open="true" r="0.9"/>
  <iidm:dcGround id="G" dcNode="N1" r="0.1" connected="false"/>
  <iidm:dcLine id="L" dcNode1="N1" dcNode2="N2" r="4" connected1="true" connected2="true" dcP1="100" dcI1="200" dcP2="-98" dcI2="-195"/>
  <iidm:substation id="SUB"><iidm:voltageLevel id="VL" nominalV="400" topologyKind="BUS_BREAKER">
    <iidm:busBreakerTopology><iidm:bus id="B1"/><iidm:bus id="B2"/></iidm:busBreakerTopology>
    <iidm:voltageSourceConverter id="VSC" dcNode1="N1" dcConnected1="true" dcNode2="N2" dcConnected2="false" idleLoss="2" switchingLoss="0.2" resistiveLoss="0.000002" controlMode="P_PCC_DROOP" targetP="301" targetVdc="502" bus1="B1" connectableBus1="B1" bus2="B2" connectableBus2="B2" voltageRegulatorOn="true" voltageSetpoint="397"><iidm:pccTerminal id="VSC" number="ONE"/><iidm:droopCurve><iidm:segment minV="-100" maxV="100" k="-5"/></iidm:droopCurve><iidm:reactiveCapabilityCurve><iidm:property name="curve" value="retained"/><iidm:point p="-200" minQ="-190" maxQ="192"><iidm:property name="point" value="one"/></iidm:point><iidm:point p="200" minQ="-189" maxQ="191"/></iidm:reactiveCapabilityCurve></iidm:voltageSourceConverter>
  </iidm:voltageLevel></iidm:substation>
</iidm:network>"#;

        unsafe {
            let module = parse_xiidm_text(DC);
            let value = pio_module_value(module);
            let mut error = std::ptr::null_mut();
            let network = pio_value_balanced_network(value, &mut error);
            let details = pio_balanced_network_detailed_connectivity(network);

            let mut counts = std::mem::MaybeUninit::<PioDetailedConnectivityCountsView>::uninit();
            assert!(pio_detailed_connectivity_counts(
                details,
                counts.as_mut_ptr(),
                &mut error,
            ));
            let counts = counts.assume_init();
            assert_eq!(counts.dc_nodes, 2);
            assert_eq!(counts.dc_grounds, 1);
            assert_eq!(counts.dc_lines, 1);
            assert_eq!(counts.dc_switches, 1);
            assert_eq!(counts.voltage_source_converters, 1);

            let mut line = std::mem::MaybeUninit::<PioDcEquipmentView>::uninit();
            assert!(pio_detailed_connectivity_dc_line_at(
                details,
                0,
                line.as_mut_ptr(),
                &mut error,
            ));
            let line = line.assume_init();
            assert_eq!(view_text(line.kind), "line");
            assert_eq!(line.terminal_count, 2);
            assert_eq!(line.resistance_ohm, 4.0);
            assert_eq!(line.terminal1.current_a, 200.0);

            let mut converter = std::mem::MaybeUninit::<PioAcDcConverterView>::uninit();
            assert!(pio_detailed_connectivity_voltage_source_converter_at(
                details,
                0,
                converter.as_mut_ptr(),
                &mut error,
            ));
            let converter = converter.assume_init();
            assert_eq!(view_text(converter.kind), "voltage_source");
            assert_eq!(
                view_text(converter.control_mode),
                "active_power_at_pcc_and_dc_voltage_droop_curve"
            );
            assert!(converter.voltage_regulator_on);
            assert_eq!(converter.idle_loss_mw, 2.0);
            assert!(converter.has_switching_loss);
            assert_eq!(converter.switching_loss_mw_per_ampere, 0.2);
            assert!(converter.has_resistive_loss);
            assert!(converter.has_pcc_terminal);
            assert_eq!(converter.pcc_terminal.terminal, 1);
            assert!(converter.has_reactive_limits);
            assert_eq!(
                view_text(converter.reactive_limits.kind),
                "capability_curve"
            );
            assert_eq!(converter.droop_curve_segment_count, 1);
            assert!(converter.has_droop_curve);

            let mut segment = std::mem::MaybeUninit::<PioDroopCurveSegmentView>::uninit();
            assert!(
                pio_detailed_connectivity_voltage_source_converter_droop_curve_segment_at(
                    details,
                    0,
                    0,
                    segment.as_mut_ptr(),
                    &mut error,
                )
            );
            let segment = segment.assume_init();
            assert_eq!(segment.minimum_voltage_kv, -100.0);
            assert_eq!(segment.maximum_voltage_kv, 100.0);
            assert_eq!(segment.k, -5.0);

            let mut point = std::mem::MaybeUninit::<PioReactiveCapabilityCurvePointView>::uninit();
            assert!(
                pio_detailed_connectivity_voltage_source_converter_reactive_capability_point_at(
                    details,
                    0,
                    0,
                    point.as_mut_ptr(),
                    &mut error,
                )
            );
            let point = point.assume_init();
            assert_eq!(point.active_power_mw, -200.0);
            assert_eq!(point.property_count, 1);

            let base_network = match &PioModule::get(module).unwrap().module.value() {
                PioValue::BalancedNetwork(network) => network.clone(),
                value => panic!("expected balanced network, got {}", value.type_name()),
            };

            pio_detailed_connectivity_release(details);
            pio_balanced_network_release(network);
            pio_value_release(value);
            pio_module_release(module);

            let mut empty_curve_network = base_network.clone();
            std::sync::Arc::make_mut(
                empty_curve_network
                    .detailed_connectivity_mut()
                    .as_mut()
                    .unwrap(),
            )
            .voltage_source_converters[0]
                .droop_curve = Some(serde_json::from_str(r#"{"segments":[]}"#).unwrap());
            let mut absent_curve_network = base_network;
            std::sync::Arc::make_mut(
                absent_curve_network
                    .detailed_connectivity_mut()
                    .as_mut()
                    .unwrap(),
            )
            .voltage_source_converters[0]
                .droop_curve = None;

            for (network, has_droop_curve) in
                [(empty_curve_network, true), (absent_curve_network, false)]
            {
                let module = module_handle(powerio::PioModule::new(PioValue::from(network)));
                let value = pio_module_value(module);
                let network = pio_value_balanced_network(value, &mut error);
                let details = pio_balanced_network_detailed_connectivity(network);
                let mut converter = std::mem::MaybeUninit::<PioAcDcConverterView>::uninit();
                assert!(pio_detailed_connectivity_voltage_source_converter_at(
                    details,
                    0,
                    converter.as_mut_ptr(),
                    &mut error,
                ));
                let converter = converter.assume_init();
                assert_eq!(converter.droop_curve_segment_count, 0);
                assert_eq!(converter.has_droop_curve, has_droop_curve);
                pio_detailed_connectivity_release(details);
                pio_balanced_network_release(network);
                pio_value_release(value);
                pio_module_release(module);
            }
        }
    }

    #[test]
    fn balanced_calculation_instances_expose_typed_inputs() {
        unsafe {
            let network = case9_network();
            let dc_pf = powerio_prob::DcPfInstance::from_network(network.clone()).unwrap();
            let dc_pf_module =
                module_handle(powerio::PioModule::new(PioValue::DcPfInstance(dc_pf)));
            let dc_pf_value = pio_module_value(dc_pf_module);
            let mut error = std::ptr::null_mut();
            let dc_pf = pio_value_dc_pf_instance(dc_pf_value, &mut error);
            assert!(!dc_pf.is_null(), "{}", error_text(error));
            assert_eq!(pio_dc_pf_instance_bus_specification_count(dc_pf), 9);
            assert_eq!(
                view_text(pio_dc_pf_instance_branch_susceptance_formula(dc_pf)),
                "series_susceptance"
            );
            assert!(!pio_calculation_instance_has_initial_point(dc_pf));
            assert!(pio_calculation_instance_initial_point(dc_pf, &mut error).is_null());
            assert!(error.is_null());

            let mut reference = std::mem::MaybeUninit::<PioDcBusSpecificationView>::uninit();
            assert!(pio_dc_pf_instance_bus_specification_at(
                dc_pf,
                0,
                reference.as_mut_ptr(),
                &mut error,
            ));
            let reference = reference.assume_init();
            assert_eq!(reference.bus_id, 1);
            assert_eq!(view_text(reference.kind), "reference");
            assert_eq!(reference.voltage_angle_degrees, 0.0);

            let mut load_bus = std::mem::MaybeUninit::<PioDcBusSpecificationView>::uninit();
            assert!(pio_dc_pf_instance_bus_specification_at(
                dc_pf,
                4,
                load_bus.as_mut_ptr(),
                &mut error,
            ));
            let load_bus = load_bus.assume_init();
            assert_eq!(load_bus.bus_id, 5);
            assert_eq!(view_text(load_bus.kind), "net_active_power");
            assert_eq!(load_bus.net_active_power_mw, -90.0);

            pio_calculation_instance_release(dc_pf);
            pio_value_release(dc_pf_value);
            pio_module_release(dc_pf_module);

            let initial = powerio_prob::BalancedOperatingPointBuilder::for_point(network.clone())
                .bus_voltage_magnitudes(vec![1.01; network.buses().len()])
                .build_point()
                .unwrap();
            let dc_pf = powerio_prob::DcPfInstance::from_network(network.clone())
                .unwrap()
                .with_initial_point(initial);
            let dc_pf_module =
                module_handle(powerio::PioModule::new(PioValue::DcPfInstance(dc_pf)));
            let dc_pf_value = pio_module_value(dc_pf_module);
            let dc_pf = pio_value_dc_pf_instance(dc_pf_value, &mut error);
            let initial = pio_calculation_instance_initial_point(dc_pf, &mut error);
            assert!(!initial.is_null(), "{}", error_text(error));
            assert_eq!(
                view_text(pio_operating_point_type_name(initial)),
                "powerio.OperatingPoint<powerio.BalancedNetwork>"
            );
            pio_calculation_instance_release(dc_pf);
            pio_value_release(dc_pf_value);
            pio_module_release(dc_pf_module);
            let mut initial_vm = 0.0;
            assert!(pio_operating_point_get_value(
                initial,
                c"bus_voltage_magnitude".as_ptr(),
                "bus_voltage_magnitude".len(),
                c"1".as_ptr(),
                1,
                &mut initial_vm,
                &mut error,
            ));
            assert_eq!(initial_vm, 1.01);
            let initial_network = pio_operating_point_balanced_network(initial, &mut error);
            assert_eq!(pio_balanced_network_bus_count(initial_network), 9);
            pio_balanced_network_release(initial_network);
            pio_operating_point_release(initial);

            let ac_pf = powerio_prob::AcPfInstance::from_network(network.clone()).unwrap();
            let ac_pf_module =
                module_handle(powerio::PioModule::new(PioValue::AcPfInstance(ac_pf)));
            let ac_pf_value = pio_module_value(ac_pf_module);
            let ac_pf = pio_value_ac_pf_instance(ac_pf_value, &mut error);
            let mut pv = std::mem::MaybeUninit::<PioAcBusSpecificationView>::uninit();
            assert!(pio_ac_pf_instance_bus_specification_at(
                ac_pf,
                1,
                pv.as_mut_ptr(),
                &mut error,
            ));
            let pv = pv.assume_init();
            assert_eq!(pv.bus_id, 2);
            assert_eq!(view_text(pv.kind), "pv");
            assert_eq!(pv.net_active_power_mw, 163.0);
            assert_eq!(pv.voltage_magnitude_pu, 1.025);
            pio_calculation_instance_release(ac_pf);
            pio_value_release(ac_pf_value);
            pio_module_release(ac_pf_module);

            let dc_opf = powerio_prob::DcOpfInstance::from_network(network).unwrap();
            let dc_opf_module =
                module_handle(powerio::PioModule::new(PioValue::DcOpfInstance(dc_opf)));
            let dc_opf_value = pio_module_value(dc_opf_module);
            let dc_opf = pio_value_dc_opf_instance(dc_opf_value, &mut error);
            assert_eq!(pio_calculation_instance_objective_term_count(dc_opf), 1);
            let mut term = std::mem::MaybeUninit::<PioObjectiveTermView>::uninit();
            assert!(pio_calculation_instance_objective_term_at(
                dc_opf,
                0,
                term.as_mut_ptr(),
                &mut error,
            ));
            assert_eq!(view_text(term.assume_init().kind), "network_generator_cost");
            assert_eq!(pio_calculation_instance_active_constraint_count(dc_opf), 4);
            let mut constraint = std::mem::MaybeUninit::<PioActiveConstraintView>::uninit();
            assert!(pio_calculation_instance_active_constraint_at(
                dc_opf,
                2,
                constraint.as_mut_ptr(),
                &mut error,
            ));
            let constraint = constraint.assume_init();
            assert_eq!(view_text(constraint.family), "thermal_limits");
            assert_eq!(view_text(constraint.selection), "all");
            assert_eq!(constraint.identity_count, 0);

            pio_calculation_instance_release(dc_opf);
            pio_value_release(dc_opf_value);
            pio_module_release(dc_opf_module);
        }
    }

    #[test]
    fn dc_opf_preparation_rows_keep_their_owner_alive() {
        unsafe {
            let source_module = parse_case9();
            let mut error = std::ptr::null_mut();
            let instance_module = pio_module_to_dc_opf_instance(source_module, &mut error);
            assert!(!instance_module.is_null(), "{}", error_text(error));
            let value = pio_module_value(instance_module);
            let instance = pio_value_dc_opf_instance(value, &mut error);
            assert!(!instance.is_null(), "{}", error_text(error));
            let units = "per_unit";
            let preparation = pio_build_dc_opf_preparation(
                instance,
                units.as_ptr().cast(),
                units.len(),
                false,
                false,
                true,
                &mut error,
            );
            assert!(!preparation.is_null(), "{}", error_text(error));

            pio_calculation_instance_release(instance);
            pio_value_release(value);
            pio_module_release(instance_module);
            pio_module_release(source_module);

            let mut summary = std::mem::MaybeUninit::<PioDcOpfPreparationView>::uninit();
            assert!(pio_dc_opf_preparation_summary(
                preparation,
                summary.as_mut_ptr(),
                &mut error,
            ));
            let summary = summary.assume_init();
            assert_eq!(summary.bus_count, 9);
            assert_eq!(summary.generator_count, 3);
            assert_eq!(summary.branch_count, 9);
            assert_eq!(summary.base_mva, 100.0);
            assert_eq!(view_text(summary.units), "per_unit");
            assert!(summary.correct_angle_difference_bounds);
            assert_eq!(
                view_text(summary.branch_susceptance_formula),
                "series_susceptance"
            );
            assert_eq!(view_text(summary.objective), "network_generator_cost");

            let reference = pio_dc_opf_preparation_reference_buses(preparation);
            assert_eq!(
                std::slice::from_raw_parts(reference.data, reference.len),
                &[0]
            );

            let mut bus = std::mem::MaybeUninit::<PioDcOpfBusView>::uninit();
            assert!(pio_dc_opf_preparation_bus_at(
                preparation,
                4,
                bus.as_mut_ptr(),
                &mut error,
            ));
            let bus = bus.assume_init();
            assert_eq!(bus.bus_id, 5);
            assert_eq!(bus.active_power_demand, 0.9);
            assert_eq!(bus.shunt_conductance, 0.0);
            assert_eq!(bus.phase_shift_injection, 0.0);

            let mut generator = std::mem::MaybeUninit::<PioDcOpfGeneratorView>::uninit();
            assert!(pio_dc_opf_preparation_generator_at(
                preparation,
                0,
                generator.as_mut_ptr(),
                &mut error,
            ));
            let generator = generator.assume_init();
            assert_eq!(generator.bus_index, 0);
            assert_eq!(generator.quadratic_cost, 2200.0);
            assert!(!generator.has_piecewise_linear_cost);

            let mut branch = std::mem::MaybeUninit::<PioDcOpfBranchView>::uninit();
            assert!(pio_dc_opf_preparation_branch_at(
                preparation,
                0,
                branch.as_mut_ptr(),
                &mut error,
            ));
            let branch = branch.assume_init();
            assert_eq!(branch.from_bus_index, 0);
            assert_eq!(branch.to_bus_index, 3);
            assert!(branch.susceptance_magnitude > 0.0);
            assert_eq!(view_text(branch.source_kind), "branch");
            assert_eq!(branch.source_row, 0);
            assert!(!branch.has_winding);

            let mut missing = std::mem::MaybeUninit::<PioDcOpfBusView>::uninit();
            assert!(!pio_dc_opf_preparation_bus_at(
                preparation,
                summary.bus_count,
                missing.as_mut_ptr(),
                &mut error,
            ));
            assert_eq!(
                view_text(pio_error_code(error)),
                "BIND.CAPI.INDEX_OUT_OF_RANGE"
            );
            pio_error_release(error);
            pio_dc_opf_preparation_release(preparation);
        }
    }

    #[test]
    fn ac_opf_preparation_rows_keep_their_owner_alive() {
        unsafe {
            let source_module = parse_case9();
            let mut error = std::ptr::null_mut();
            let instance_module = pio_module_to_ac_opf_instance(source_module, &mut error);
            assert!(!instance_module.is_null(), "{}", error_text(error));
            let value = pio_module_value(instance_module);
            let instance = pio_value_ac_opf_instance(value, &mut error);
            assert!(!instance.is_null(), "{}", error_text(error));
            let units = "per_unit";
            let preparation = pio_build_ac_opf_preparation(
                instance,
                units.as_ptr().cast(),
                units.len(),
                false,
                false,
                true,
                &mut error,
            );
            assert!(!preparation.is_null(), "{}", error_text(error));

            pio_calculation_instance_release(instance);
            pio_value_release(value);
            pio_module_release(instance_module);
            pio_module_release(source_module);

            let mut summary = std::mem::MaybeUninit::<PioAcOpfPreparationView>::uninit();
            assert!(pio_ac_opf_preparation_summary(
                preparation,
                summary.as_mut_ptr(),
                &mut error,
            ));
            let summary = summary.assume_init();
            assert_eq!(summary.bus_count, 9);
            assert_eq!(summary.generator_count, 3);
            assert_eq!(summary.branch_count, 9);
            assert_eq!(summary.base_mva, 100.0);
            assert_eq!(view_text(summary.units), "per_unit");
            assert!(summary.correct_angle_difference_bounds);
            assert_eq!(view_text(summary.objective), "network_generator_cost");

            let reference = pio_ac_opf_preparation_reference_buses(preparation);
            assert_eq!(
                std::slice::from_raw_parts(reference.data, reference.len),
                &[0]
            );

            let mut bus = std::mem::MaybeUninit::<PioAcOpfBusView>::uninit();
            assert!(pio_ac_opf_preparation_bus_at(
                preparation,
                4,
                bus.as_mut_ptr(),
                &mut error,
            ));
            let bus = bus.assume_init();
            assert_eq!(bus.bus_id, 5);
            assert_eq!(bus.active_power_demand, 0.9);
            assert_eq!(bus.reactive_power_demand, 0.3);
            assert_eq!(bus.initial_voltage_angle_radians, 0.0);

            let mut generator = std::mem::MaybeUninit::<PioAcOpfGeneratorView>::uninit();
            assert!(pio_ac_opf_preparation_generator_at(
                preparation,
                0,
                generator.as_mut_ptr(),
                &mut error,
            ));
            let generator = generator.assume_init();
            assert_eq!(generator.bus_index, 0);
            assert_eq!(generator.initial_active_power, 0.723);
            assert_eq!(generator.quadratic_cost, 2200.0);
            assert!(!generator.has_piecewise_linear_cost);

            let mut branch = std::mem::MaybeUninit::<PioAcOpfBranchView>::uninit();
            assert!(pio_ac_opf_preparation_branch_at(
                preparation,
                0,
                branch.as_mut_ptr(),
                &mut error,
            ));
            let branch = branch.assume_init();
            assert_eq!(branch.from_bus_index, 0);
            assert_eq!(branch.to_bus_index, 3);
            assert_eq!(view_text(branch.source_kind), "branch");
            assert_eq!(branch.source_row, 0);
            assert!(!branch.has_winding);

            let mut missing = std::mem::MaybeUninit::<PioAcOpfBusView>::uninit();
            assert!(!pio_ac_opf_preparation_bus_at(
                preparation,
                summary.bus_count,
                missing.as_mut_ptr(),
                &mut error,
            ));
            assert_eq!(
                view_text(pio_error_code(error)),
                "BIND.CAPI.INDEX_OUT_OF_RANGE"
            );
            pio_error_release(error);
            pio_ac_opf_preparation_release(preparation);
        }
    }

    #[test]
    fn ac_opf_storage_view_preserves_fields_and_units() {
        unsafe {
            let mut network = case9_network();
            let mut storage = powerio_tx::Storage::new(powerio_tx::BusId(5));
            storage.uid = Some("battery".into());
            storage.ps = 12.0;
            storage.qs = -3.0;
            storage.energy = 80.0;
            storage.energy_rating = 120.0;
            storage.charge_rating = 25.0;
            storage.discharge_rating = 30.0;
            storage.charge_efficiency = 0.91;
            storage.discharge_efficiency = 0.88;
            storage.thermal_rating = 35.0;
            storage.qmin = -14.0;
            storage.qmax = 16.0;
            storage.r = 0.001;
            storage.x = 0.002;
            storage.p_loss = 0.4;
            storage.q_loss = 0.5;
            network.storage_mut().push(storage);
            let instance = powerio_prob::AcOpfInstance::from_network(network).unwrap();
            let close = |actual: f64, expected: f64| assert!((actual - expected).abs() < 1e-12);

            for (units, power_scale) in [(Units::PerUnit, 0.01), (Units::Native, 1.0)] {
                let prepared = build_ac_opf_preparation(
                    &instance,
                    &AcOpfAssemblyOptions::default().with_units(units),
                )
                .unwrap();
                let preparation = PioAcOpfPreparation::new_raw(prepared);
                let mut error = std::ptr::null_mut();
                let mut summary = std::mem::MaybeUninit::<PioAcOpfPreparationView>::uninit();
                assert!(pio_ac_opf_preparation_summary(
                    preparation,
                    summary.as_mut_ptr(),
                    &mut error,
                ));
                assert_eq!(summary.assume_init().storage_count, 1);

                let mut row = std::mem::MaybeUninit::<PioAcOpfStorageView>::uninit();
                assert!(pio_ac_opf_preparation_storage_at(
                    preparation,
                    0,
                    row.as_mut_ptr(),
                    &mut error,
                ));
                let row = row.assume_init();
                assert_eq!(view_text(row.component_id), "battery");
                assert_eq!(row.bus_index, 4);
                assert_eq!(row.source_row, 0);
                close(row.initial_active_power, 12.0 * power_scale);
                close(row.initial_reactive_power, -3.0 * power_scale);
                close(row.energy, 80.0 * power_scale);
                close(row.energy_rating, 120.0 * power_scale);
                close(row.charge_rating, 25.0 * power_scale);
                close(row.discharge_rating, 30.0 * power_scale);
                assert_eq!(row.charge_efficiency, 0.91);
                assert_eq!(row.discharge_efficiency, 0.88);
                close(row.apparent_power_max, 35.0 * power_scale);
                close(row.reactive_power_min, -14.0 * power_scale);
                close(row.reactive_power_max, 16.0 * power_scale);
                assert_eq!(row.resistance_pu, 0.001);
                assert_eq!(row.reactance_pu, 0.002);
                close(row.active_power_loss, 0.4 * power_scale);
                close(row.reactive_power_loss, 0.5 * power_scale);
                assert!(row.in_service);

                pio_ac_opf_preparation_release(preparation);
            }
        }
    }

    #[test]
    fn module_transforms_build_typed_calculation_modules() {
        unsafe {
            type Transform =
                unsafe extern "C" fn(*const PioModule, *mut *mut PioError) -> *mut PioModule;
            let source = parse_case9();
            let operations: [(Transform, &str, &str); 4] = [
                (
                    pio_module_to_dc_pf_instance,
                    "powerio.DcPfInstance",
                    "to_dc_pf_instance",
                ),
                (
                    pio_module_to_ac_pf_instance,
                    "powerio.AcPfInstance",
                    "to_ac_pf_instance",
                ),
                (
                    pio_module_to_dc_opf_instance,
                    "powerio.DcOpfInstance",
                    "to_dc_opf_instance",
                ),
                (
                    pio_module_to_ac_opf_instance,
                    "powerio.AcOpfInstance",
                    "to_ac_opf_instance",
                ),
            ];
            for (operation, expected_type, expected_history) in operations {
                let mut error = std::ptr::null_mut();
                let derived = operation(source, &mut error);
                assert!(!derived.is_null(), "{}", error_text(error));
                let value = pio_module_value(derived);
                assert!(pio_value_is_type(
                    value,
                    expected_type.as_ptr().cast(),
                    expected_type.len(),
                ));
                let derived_module = &PioModule::get(derived).unwrap().module;
                assert_eq!(
                    derived_module.history().last().unwrap().name(),
                    expected_history
                );
                assert!(derived_module.source().is_none());
                pio_value_release(value);
                pio_module_release(derived);
            }

            let mut error = std::ptr::null_mut();
            let instance = pio_module_to_dc_pf_instance(source, &mut error);
            let invalid = pio_module_to_ac_pf_instance(instance, &mut error);
            assert!(invalid.is_null());
            assert_eq!(
                view_text(pio_error_code(error)),
                "REQUEST.MODULE.WRONG_MODEL_KIND"
            );
            pio_error_release(error);
            pio_module_release(instance);
            pio_module_release(source);
        }
    }

    #[test]
    fn multiconductor_module_transforms_build_typed_calculation_modules() {
        unsafe {
            let source = parse_bmopf();
            let value = pio_module_value(source);
            let network_type = "powerio.MulticonductorNetwork";
            assert!(pio_value_is_type(
                value,
                network_type.as_ptr().cast(),
                network_type.len(),
            ));
            pio_value_release(value);

            type Transform =
                unsafe extern "C" fn(*const PioModule, *mut *mut PioError) -> *mut PioModule;
            let operations: [(Transform, &str, &str); 2] = [
                (
                    pio_module_to_mc_ac_pf_instance,
                    "powerio.McAcPfInstance",
                    "to_mc_ac_pf_instance",
                ),
                (
                    pio_module_to_mc_ac_opf_instance,
                    "powerio.McAcOpfInstance",
                    "to_mc_ac_opf_instance",
                ),
            ];
            for (operation, expected_type, expected_history) in operations {
                let mut error = std::ptr::null_mut();
                let derived = operation(source, &mut error);
                assert!(!derived.is_null(), "{}", error_text(error));
                let value = pio_module_value(derived);
                assert!(pio_value_is_type(
                    value,
                    expected_type.as_ptr().cast(),
                    expected_type.len(),
                ));
                let derived_module = &PioModule::get(derived).unwrap().module;
                assert_eq!(
                    derived_module.history().last().unwrap().name(),
                    expected_history
                );
                assert!(derived_module.source().is_none());
                pio_value_release(value);
                pio_module_release(derived);
            }
            pio_module_release(source);
        }
    }

    #[test]
    fn geo_layer_derives_a_new_network_module() {
        unsafe {
            let source_module = parse_case9();
            let geojson = br#"{
                "type": "FeatureCollection",
                "features": [{
                    "type": "Feature",
                    "geometry": {
                        "type": "Point",
                        "coordinates": [-83.743, 42.281]
                    },
                    "properties": {"bus": "1"}
                }]
            }"#;
            let mut error = std::ptr::null_mut();
            let source = pio_source_from_memory(
                c"case9.geojson".as_ptr(),
                "case9.geojson".len(),
                geojson.as_ptr(),
                geojson.len(),
                &mut error,
            );
            assert!(!source.is_null(), "{}", error_text(error));
            let layer = pio_geo_layer_parse(source, &mut error);
            pio_source_release(source);
            assert!(!layer.is_null(), "{}", error_text(error));

            let diagnostics = pio_geo_layer_diagnostics(layer);
            assert_eq!(pio_diagnostics_len(diagnostics), 0);
            pio_diagnostics_release(diagnostics);

            let mut report = std::ptr::null_mut();
            let derived = pio_module_apply_geo_layer(source_module, layer, &mut report, &mut error);
            assert!(!derived.is_null(), "{}", error_text(error));
            assert!(!report.is_null());
            assert_eq!(pio_geo_apply_report_matched_buses(report), 1);
            assert_eq!(pio_geo_apply_report_matched_branches(report), 0);
            assert_eq!(pio_geo_apply_report_unmatched_features(report), 0);
            assert_eq!(pio_geo_apply_report_unlocated_buses(report), 8);
            assert_eq!(pio_geo_apply_report_unlocated_branches(report), 9);

            let note_count = pio_geo_apply_report_note_count(report);
            let note = pio_geo_apply_report_note_at(report, note_count, &mut error);
            assert_eq!(note.len, 0);
            assert_eq!(
                view_text(pio_error_code(error)),
                "BIND.CAPI.INDEX_OUT_OF_RANGE"
            );
            pio_error_release(error);
            error = std::ptr::null_mut();

            let derived_module = &PioModule::get(derived).unwrap().module;
            assert!(derived_module.source().is_none());
            assert_eq!(
                derived_module.history().last().unwrap().name(),
                "apply_geo_layer"
            );
            let PioValue::BalancedNetwork(derived_network) = &derived_module.value() else {
                panic!("geo application changed the value type")
            };
            let derived_location = derived_network
                .buses()
                .iter()
                .find(|bus| bus.id == powerio_tx::BusId(1))
                .and_then(|bus| bus.location)
                .unwrap();
            assert_eq!((derived_location.x, derived_location.y), (-83.743, 42.281));

            let original_module = &PioModule::get(source_module).unwrap().module;
            let PioValue::BalancedNetwork(original_network) = &original_module.value() else {
                unreachable!()
            };
            assert!(
                original_network
                    .buses()
                    .iter()
                    .find(|bus| bus.id == powerio_tx::BusId(1))
                    .unwrap()
                    .location
                    .is_none()
            );

            let calculation = pio_module_to_dc_pf_instance(source_module, &mut error);
            assert!(!calculation.is_null(), "{}", error_text(error));
            let rejected =
                pio_module_apply_geo_layer(calculation, layer, std::ptr::null_mut(), &mut error);
            assert!(rejected.is_null());
            assert_eq!(
                view_text(pio_error_code(error)),
                "REQUEST.MODULE.WRONG_MODEL_KIND"
            );
            pio_error_release(error);

            pio_module_release(calculation);
            pio_geo_apply_report_release(report);
            pio_geo_layer_release(layer);
            pio_module_release(derived);
            pio_module_release(source_module);
        }
    }

    #[test]
    fn aggregate_bus_active_demand_uses_explicit_proportional_allocation() {
        unsafe {
            let mut network = case9_network();
            let mut second = network
                .loads()
                .iter()
                .find(|load| load.bus == powerio_tx::BusId::new(5))
                .unwrap()
                .clone();
            second.p = 30.0;
            second.q = 10.0;
            second.uid = Some("extra-load-at-bus-5".to_owned());
            network.loads_mut().push(second);
            let source = module_handle(powerio::PioModule::new(PioValue::BalancedNetwork(network)));
            let mut error = std::ptr::null_mut();
            let module = pio_module_to_dc_pf_instance(source, &mut error);
            assert!(!module.is_null(), "{}", error_text(error));
            pio_module_release(source);

            let value_before = pio_module_value(module);
            let instance_before = pio_value_dc_pf_instance(value_before, &mut error);
            let network_before =
                pio_calculation_instance_balanced_network(instance_before, &mut error);

            let power = pio_active_power_from_watts(240_000_000.0);
            let bad_rule = "first_load";
            let bad_report = pio_apply_bus_load_active_power(
                module,
                5,
                power,
                bad_rule.as_ptr().cast(),
                bad_rule.len(),
                &mut error,
            );
            assert!(bad_report.is_null());
            assert_eq!(
                view_text(pio_error_code(error)),
                "REQUEST.CAPI.ALLOCATION_UNKNOWN"
            );
            pio_error_release(error);
            error = std::ptr::null_mut();

            let rule = "proportional_to_current_active_power";
            let report = pio_apply_bus_load_active_power(
                module,
                5,
                power,
                rule.as_ptr().cast(),
                rule.len(),
                &mut error,
            );
            assert!(!report.is_null(), "{}", error_text(error));
            assert_eq!(pio_update_report_len(report), 2);
            assert!(!pio_update_report_connectivity_changed(report));

            let value_after = pio_module_value(module);
            let instance_after = pio_value_dc_pf_instance(value_after, &mut error);
            let network_after =
                pio_calculation_instance_balanced_network(instance_after, &mut error);
            let mut first_before = std::mem::MaybeUninit::<PioBalancedLoadView>::uninit();
            let mut second_before = std::mem::MaybeUninit::<PioBalancedLoadView>::uninit();
            let mut first_after = std::mem::MaybeUninit::<PioBalancedLoadView>::uninit();
            let mut second_after = std::mem::MaybeUninit::<PioBalancedLoadView>::uninit();
            assert!(pio_balanced_network_load_at(
                network_before,
                0,
                first_before.as_mut_ptr(),
                &mut error,
            ));
            assert!(pio_balanced_network_load_at(
                network_before,
                3,
                second_before.as_mut_ptr(),
                &mut error,
            ));
            assert!(pio_balanced_network_load_at(
                network_after,
                0,
                first_after.as_mut_ptr(),
                &mut error,
            ));
            assert!(pio_balanced_network_load_at(
                network_after,
                3,
                second_after.as_mut_ptr(),
                &mut error,
            ));
            assert_eq!(first_before.assume_init().p_mw, 90.0);
            assert_eq!(second_before.assume_init().p_mw, 30.0);
            assert_eq!(first_after.assume_init().p_mw, 180.0);
            assert_eq!(second_after.assume_init().p_mw, 60.0);
            assert_eq!(
                PioModule::get(module)
                    .unwrap()
                    .module
                    .history()
                    .last()
                    .unwrap()
                    .name(),
                "apply_updates"
            );

            pio_balanced_network_release(network_before);
            pio_calculation_instance_release(instance_before);
            pio_value_release(value_before);
            pio_balanced_network_release(network_after);
            pio_calculation_instance_release(instance_after);
            pio_value_release(value_after);
            pio_update_report_release(report);
            pio_active_power_release(power);
            pio_module_release(module);
        }
    }

    #[test]
    fn scuc_instance_exposes_typed_scheduling_inputs() {
        unsafe {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../tests/data/goc3/goc3_small.json");
            let source = std::fs::read_to_string(path).unwrap().replacen(
                "\"startup_states\": []",
                "\"startup_states\": [[2.0, 3.0]]",
                1,
            );
            let parsed =
                goc3_instance(Source::from_memory("goc3_small.json", source.into_bytes()).unwrap());
            let module = module_handle(powerio::PioModule::new(PioValue::AcScucInstance(parsed)));
            let value = pio_module_value(module);
            let mut error = std::ptr::null_mut();
            let instance = pio_value_ac_scuc_instance(value, &mut error);
            assert!(!instance.is_null(), "{}", error_text(error));
            pio_value_release(value);
            pio_module_release(module);

            let mut dimensions = std::mem::MaybeUninit::<PioScucDimensionsView>::uninit();
            assert!(pio_ac_scuc_instance_dimensions(
                instance,
                dimensions.as_mut_ptr(),
                &mut error,
            ));
            let dimensions = dimensions.assume_init();
            assert_eq!(dimensions.period_count, 2);
            assert_eq!(dimensions.device_count, 2);
            assert_eq!(dimensions.producer_count, 1);
            assert_eq!(dimensions.consumer_count, 1);
            assert_eq!(dimensions.shunt_count, 1);
            assert_eq!(dimensions.branch_switching_cost_count, 3);
            assert_eq!(dimensions.transformer_control_count, 1);
            assert_eq!(dimensions.active_reserve_zone_count, 1);
            assert_eq!(dimensions.reactive_reserve_zone_count, 1);
            assert_eq!(dimensions.contingency_count, 3);

            let durations = pio_ac_scuc_instance_interval_durations(instance);
            assert_eq!(
                std::slice::from_raw_parts(durations.data, durations.len),
                &[1.0, 1.0]
            );

            let mut violation = std::mem::MaybeUninit::<PioScucViolationCostView>::uninit();
            assert!(pio_ac_scuc_instance_violation_costs(
                instance,
                violation.as_mut_ptr(),
                &mut error,
            ));
            let violation = violation.assume_init();
            assert_eq!(violation.active_power_balance, 1.0);
            assert_eq!(violation.reactive_power_balance, 1.0);
            assert_eq!(violation.branch_thermal_limit, 1.0);
            assert_eq!(violation.energy_requirement, 1.0);

            let mut producer = std::mem::MaybeUninit::<PioScucDeviceView>::uninit();
            assert!(pio_ac_scuc_instance_device_at(
                instance,
                0,
                producer.as_mut_ptr(),
                &mut error,
            ));
            let producer = producer.assume_init();
            assert_eq!(view_text(producer.id.component_type), "generator");
            assert_eq!(view_text(producer.id.local_id), "sd_00");
            assert_eq!(view_text(producer.kind), "producer");
            assert!(producer.initial_on_status);
            assert_eq!(producer.minimum_up_time_hours, 1.0);
            assert_eq!(producer.minimum_down_time_hours, 1.0);
            assert_eq!(producer.initial_commitment.accumulated_up_time_hours, 4.0);
            assert_eq!(producer.initial_commitment.accumulated_down_time_hours, 0.0);
            assert_eq!(producer.startup_cost_adjustment_count, 1);
            assert_eq!(producer.startup_limit_count, 1);
            assert_eq!(producer.energy_upper_bound_count, 1);
            assert_eq!(producer.energy_lower_bound_count, 1);
            assert_eq!(producer.period_count, 2);

            let uid = "sd_00";
            let mut found_device = std::mem::MaybeUninit::<PioScucDeviceView>::uninit();
            assert!(pio_ac_scuc_instance_device_get(
                instance,
                uid.as_ptr().cast(),
                uid.len(),
                found_device.as_mut_ptr(),
                &mut error,
            ));
            assert_eq!(view_text(found_device.assume_init().id.local_id), "sd_00");

            let mut adjustment =
                std::mem::MaybeUninit::<PioScucStartupCostAdjustmentView>::uninit();
            assert!(pio_ac_scuc_instance_device_startup_cost_adjustment_at(
                instance,
                0,
                0,
                adjustment.as_mut_ptr(),
                &mut error,
            ));
            assert_eq!(adjustment.assume_init().maximum_down_time_hours, 3.0);

            let mut limit = std::mem::MaybeUninit::<PioScucStartupLimitView>::uninit();
            assert!(pio_ac_scuc_instance_device_startup_limit_at(
                instance,
                0,
                0,
                limit.as_mut_ptr(),
                &mut error,
            ));
            assert_eq!(limit.assume_init().maximum_startups, 1);

            let mut energy = std::mem::MaybeUninit::<PioScucEnergyRequirementView>::uninit();
            assert!(pio_ac_scuc_instance_device_energy_upper_bound_at(
                instance,
                0,
                0,
                energy.as_mut_ptr(),
                &mut error,
            ));
            assert_eq!(energy.assume_init().energy_pu, 9.0);
            assert!(pio_ac_scuc_instance_device_energy_lower_bound_at(
                instance,
                0,
                0,
                energy.as_mut_ptr(),
                &mut error,
            ));
            assert_eq!(energy.assume_init().energy_pu, 1.0);

            let mut period = std::mem::MaybeUninit::<PioScucDevicePeriodView>::uninit();
            assert!(pio_ac_scuc_instance_device_period_at(
                instance,
                0,
                0,
                period.as_mut_ptr(),
                &mut error,
            ));
            let period = period.assume_init();
            assert!(period.on_status_min);
            assert!(period.on_status_max);
            assert_eq!(period.active_power_min_pu, 2.0);
            assert_eq!(period.active_power_max_pu, 5.0);
            assert_eq!(period.energy_cost_block_count, 1);

            let mut cost_block = std::mem::MaybeUninit::<PioScucEnergyCostBlockView>::uninit();
            assert!(pio_ac_scuc_instance_device_energy_cost_block_at(
                instance,
                0,
                0,
                0,
                cost_block.as_mut_ptr(),
                &mut error,
            ));
            let cost_block = cost_block.assume_init();
            assert_eq!(cost_block.marginal_cost, 10.0);
            assert_eq!(cost_block.block_size_pu, 5.0);

            let mut shunt = std::mem::MaybeUninit::<PioScucShuntView>::uninit();
            assert!(pio_ac_scuc_instance_shunt_at(
                instance,
                0,
                shunt.as_mut_ptr(),
                &mut error,
            ));
            let shunt = shunt.assume_init();
            assert_eq!(view_text(shunt.id.local_id), "sh_00");
            assert_eq!(shunt.conductance_per_step_pu, 0.0);
            assert_eq!(shunt.susceptance_per_step_pu, 3.0);
            assert_eq!(
                (shunt.step_min, shunt.initial_step, shunt.step_max),
                (0, 1, 4)
            );
            let shunt_uid = "sh_00";
            let mut found_shunt = std::mem::MaybeUninit::<PioScucShuntView>::uninit();
            assert!(pio_ac_scuc_instance_shunt_get(
                instance,
                shunt_uid.as_ptr().cast(),
                shunt_uid.len(),
                found_shunt.as_mut_ptr(),
                &mut error,
            ));
            assert_eq!(view_text(found_shunt.assume_init().id.local_id), "sh_00");

            let mut switching = std::mem::MaybeUninit::<PioScucBranchSwitchingCostView>::uninit();
            assert!(pio_ac_scuc_instance_branch_switching_cost_at(
                instance,
                0,
                switching.as_mut_ptr(),
                &mut error,
            ));
            let switching = switching.assume_init();
            assert_eq!(view_text(switching.id.component_type), "branch");
            assert_eq!(view_text(switching.id.local_id), "acl_00");

            let mut control = std::mem::MaybeUninit::<PioScucTransformerControlView>::uninit();
            assert!(pio_ac_scuc_instance_transformer_control_at(
                instance,
                0,
                control.as_mut_ptr(),
                &mut error,
            ));
            let control = control.assume_init();
            assert_eq!(view_text(control.id.component_type), "transformer");
            assert_eq!(view_text(control.id.local_id), "xf_00");
            assert_eq!((control.tap_ratio_min, control.tap_ratio_max), (1.0, 1.0));

            let mut active_zone = std::mem::MaybeUninit::<PioScucActiveReserveZoneView>::uninit();
            assert!(pio_ac_scuc_instance_active_reserve_zone_at(
                instance,
                0,
                active_zone.as_mut_ptr(),
                &mut error,
            ));
            assert_eq!(active_zone.assume_init().bus_count, 2);
            let mut active_period =
                std::mem::MaybeUninit::<PioScucActiveReservePeriodView>::uninit();
            assert!(pio_ac_scuc_instance_active_reserve_zone_period_at(
                instance,
                0,
                0,
                active_period.as_mut_ptr(),
                &mut error,
            ));
            assert_eq!(active_period.assume_init().ramping_up_requirement_pu, 0.0);
            let mut reserve_bus = std::mem::MaybeUninit::<PioComponentIdView>::uninit();
            assert!(pio_ac_scuc_instance_active_reserve_zone_bus_at(
                instance,
                0,
                0,
                reserve_bus.as_mut_ptr(),
                &mut error,
            ));
            assert_eq!(view_text(reserve_bus.assume_init().local_id), "bus_00");

            let mut reactive_zone =
                std::mem::MaybeUninit::<PioScucReactiveReserveZoneView>::uninit();
            assert!(pio_ac_scuc_instance_reactive_reserve_zone_at(
                instance,
                0,
                reactive_zone.as_mut_ptr(),
                &mut error,
            ));
            assert_eq!(reactive_zone.assume_init().bus_count, 2);
            let mut reactive_period =
                std::mem::MaybeUninit::<PioScucReactiveReservePeriodView>::uninit();
            assert!(pio_ac_scuc_instance_reactive_reserve_zone_period_at(
                instance,
                0,
                0,
                reactive_period.as_mut_ptr(),
                &mut error,
            ));
            assert_eq!(
                reactive_period.assume_init().reactive_up_requirement_pu,
                0.0
            );
            assert!(pio_ac_scuc_instance_reactive_reserve_zone_bus_at(
                instance,
                0,
                1,
                reserve_bus.as_mut_ptr(),
                &mut error,
            ));
            assert_eq!(view_text(reserve_bus.assume_init().local_id), "bus_01");

            let mut contingency = std::mem::MaybeUninit::<PioScucContingencyView>::uninit();
            assert!(pio_ac_scuc_instance_contingency_at(
                instance,
                0,
                contingency.as_mut_ptr(),
                &mut error,
            ));
            let first_contingency = contingency.assume_init();
            assert_eq!(
                view_text(first_contingency.id.component_type),
                "contingency"
            );
            assert_eq!(view_text(first_contingency.id.local_id), "ctg_00");
            let contingency_uid = "ctg_02";
            assert!(pio_ac_scuc_instance_contingency_get(
                instance,
                contingency_uid.as_ptr().cast(),
                contingency_uid.len(),
                contingency.as_mut_ptr(),
                &mut error,
            ));
            assert_eq!(view_text(contingency.assume_init().id.local_id), "ctg_02");
            let mut component = std::mem::MaybeUninit::<PioScucContingencyComponentView>::uninit();
            assert!(pio_ac_scuc_instance_contingency_component_at(
                instance,
                0,
                0,
                component.as_mut_ptr(),
                &mut error,
            ));
            let component = component.assume_init();
            assert_eq!(view_text(component.id.component_type), "branch");
            assert_eq!(view_text(component.id.local_id), "acl_00");

            pio_calculation_instance_release(instance);
        }
    }

    #[test]
    fn one_parse_reads_a_goc3_problem_and_solution_directory() {
        unsafe {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data/goc3");
            let path = path.to_string_lossy();
            let mut error = std::ptr::null_mut();
            let source = pio_source_open(path.as_ptr().cast(), path.len(), &mut error);
            assert!(!source.is_null(), "{}", error_text(error));
            let module = pio_parse(source, std::ptr::null(), 0, &mut error);
            pio_source_release(source);
            assert!(!module.is_null(), "{}", error_text(error));

            let inner = PioModule::get(module).unwrap();
            assert_eq!(inner.module.value().type_name(), "powerio.AcScucSolution");
            assert_eq!(inner.module.sources().len(), 2);

            let value = pio_module_value(module);
            let solution = pio_value_ac_scuc_solution(value, &mut error);
            pio_value_release(value);
            assert!(!solution.is_null(), "{}", error_text(error));
            assert_eq!(pio_ac_scuc_solution_time_count(solution), 2);
            let quantity = "bus_voltage_magnitude";
            let row = pio_ac_scuc_solution_get_values_at(
                solution,
                quantity.as_ptr().cast(),
                quantity.len(),
                0,
                &mut error,
            );
            assert!(!row.is_null(), "{}", error_text(error));
            let values = pio_vector_values(row);
            assert_eq!(
                std::slice::from_raw_parts(values.data, values.len),
                &[1.0, 0.99]
            );
            pio_vector_release(row);

            for quantity in [
                "bus_voltage_magnitude",
                "bus_voltage_angle",
                "shunt_step",
                "ac_line_on_status",
                "transformer_tap_ratio",
                "transformer_phase_shift",
                "transformer_on_status",
                "dc_line_from_active_power",
                "dc_line_from_reactive_power",
                "dc_line_to_reactive_power",
                "device_on_status",
                "device_startup_status",
                "device_shutdown_status",
                "device_active_power",
                "device_reactive_power",
                "regulation_reserve_up",
                "regulation_reserve_down",
                "synchronized_reserve",
                "nonsynchronized_reserve",
                "ramping_reserve_up_online",
                "ramping_reserve_up_offline",
                "ramping_reserve_down_online",
                "ramping_reserve_down_offline",
                "reactive_reserve_up",
                "reactive_reserve_down",
            ] {
                let vector = pio_ac_scuc_solution_get_values_at(
                    solution,
                    quantity.as_ptr().cast(),
                    quantity.len(),
                    0,
                    &mut error,
                );
                assert!(!vector.is_null(), "{quantity}: {}", error_text(error));
                pio_vector_release(vector);
            }

            pio_calculation_solution_release(solution);
            pio_module_release(module);
        }
    }

    #[test]
    fn one_parse_and_one_emit_cover_powsybl_formats() {
        unsafe {
            let module = parse_case9();
            let directory = tempfile::tempdir().unwrap();

            for (format, output_name) in [
                ("xiidm", "case9.xiidm"),
                ("cgmes", "case9-cgmes"),
                ("psse-rawx", "case9.rawx"),
                ("ucte", "case9.uct"),
            ] {
                let output = directory.path().join(output_name);
                let output_text = output.to_string_lossy();
                let mut error = std::ptr::null_mut();
                let destination = pio_destination_path(
                    output_text.as_ptr().cast(),
                    output_text.len(),
                    &mut error,
                );
                assert!(!destination.is_null(), "{}", error_text(error));
                let result = pio_emit(
                    module,
                    format.as_ptr().cast(),
                    format.len(),
                    destination,
                    &mut error,
                );
                pio_destination_release(destination);
                assert!(!result.is_null(), "{format}: {}", error_text(error));
                pio_emit_result_release(result);

                let source =
                    pio_source_open(output_text.as_ptr().cast(), output_text.len(), &mut error);
                assert!(!source.is_null(), "{format}: {}", error_text(error));
                let reparsed = pio_parse(source, std::ptr::null(), 0, &mut error);
                pio_source_release(source);
                assert!(!reparsed.is_null(), "{format}: {}", error_text(error));
                assert_eq!(
                    PioModule::get(reparsed).unwrap().module.value().type_name(),
                    "powerio.BalancedNetwork"
                );
                pio_module_release(reparsed);
            }

            pio_module_release(module);
        }
    }

    #[test]
    fn scuc_solution_exposes_every_device_status_and_offline_reserve() {
        unsafe {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../tests/data/goc3/goc3_small.json");
            let parsed = goc3_instance(Source::open(path).unwrap());
            let mut outputs = powerio_prob::ScucDeviceOutputs::default();
            outputs.startup_status = vec![vec![false, true], vec![true, false]];
            outputs.shutdown_status = vec![vec![true, false], vec![false, true]];
            outputs.p_ramp_res_up_offline = vec![vec![1.25, 1.75], vec![1.5, 2.0]];
            outputs.p_ramp_res_down_offline = vec![vec![2.25, 2.75], vec![2.5, 3.0]];
            let solution = powerio_prob::AcScucSolution::new(
                Arc::new(parsed),
                powerio_prob::Termination::Converged,
                powerio_prob::ScucNetworkOutputs::default(),
                outputs,
                None,
            )
            .unwrap();
            let module = module_handle(powerio::PioModule::new(PioValue::AcScucSolution(solution)));
            let value = pio_module_value(module);
            let mut error = std::ptr::null_mut();
            let solution = pio_value_ac_scuc_solution(value, &mut error);
            assert!(!solution.is_null(), "{}", error_text(error));
            pio_value_release(value);
            pio_module_release(module);

            for (quantity, expected) in [
                ("device_startup_status", [0.0, 1.0]),
                ("device_shutdown_status", [1.0, 0.0]),
                ("ramping_reserve_up_offline", [1.25, 1.75]),
                ("ramping_reserve_down_offline", [2.25, 2.75]),
            ] {
                let vector = pio_ac_scuc_solution_get_values_at(
                    solution,
                    quantity.as_ptr().cast(),
                    quantity.len(),
                    0,
                    &mut error,
                );
                assert!(!vector.is_null(), "{}", error_text(error));
                let view = pio_vector_values(vector);
                assert_eq!(view.len, 2);
                assert_eq!(std::slice::from_raw_parts(view.data, view.len), &expected);
                pio_vector_release(vector);
            }

            pio_calculation_solution_release(solution);
        }
    }

    #[test]
    fn update_detaches_from_retained_value_views() {
        unsafe {
            let module = parse_case9();
            let value = pio_module_value(module);
            let mut error = std::ptr::null_mut();
            let old_network = pio_value_balanced_network(value, &mut error);
            pio_value_release(value);
            assert!(!old_network.is_null(), "{}", error_text(error));

            let old = PioBalancedNetwork::get(old_network)
                .and_then(BalancedNetworkInner::network)
                .unwrap();
            let load = &old.loads()[0];
            let old_active_power = load.p;
            let local_id = load.uid.as_deref().unwrap().to_owned();

            let component = pio_component_id_new(
                c"load".as_ptr(),
                4,
                local_id.as_ptr().cast(),
                local_id.len(),
                &mut error,
            );
            let replacement = pio_active_power_from_megawatts(old_active_power + 1.0);
            let operating = pio_operating_point_update_set_load_active_power(
                component,
                std::ptr::null(),
                0,
                replacement,
                &mut error,
            );
            let update = pio_calculation_update_from_operating_point(operating, &mut error);
            let updates = [update.cast_const()];
            let report = pio_apply_updates(module, updates.as_ptr(), updates.len(), &mut error);
            assert!(!report.is_null(), "{}", error_text(error));
            assert_eq!(pio_update_report_len(report), 1);
            assert!(!pio_update_report_connectivity_changed(report));

            let new_value = pio_module_value(module);
            let new_network = pio_value_balanced_network(new_value, &mut error);
            let new = PioBalancedNetwork::get(new_network)
                .and_then(BalancedNetworkInner::network)
                .unwrap();
            assert_eq!(new.loads()[0].p, old_active_power + 1.0);
            assert_eq!(
                PioBalancedNetwork::get(old_network)
                    .and_then(BalancedNetworkInner::network)
                    .unwrap()
                    .loads()[0]
                    .p,
                old_active_power,
            );

            pio_balanced_network_release(new_network);
            pio_value_release(new_value);
            pio_update_report_release(report);
            pio_calculation_update_release(update);
            pio_operating_point_update_release(operating);
            pio_active_power_release(replacement);
            pio_component_id_release(component);
            pio_balanced_network_release(old_network);
            pio_module_release(module);
        }
    }

    #[test]
    fn dc_calculations_return_named_generic_handles() {
        unsafe {
            let module = parse_case9();
            let value = pio_module_value(module);
            let mut error = std::ptr::null_mut();
            let network = pio_value_balanced_network(value, &mut error);
            let incidence = pio_calc_incidence_matrix(network, std::ptr::null(), 0, &mut error);
            assert!(!incidence.is_null(), "{}", error_text(error));
            assert_eq!(pio_sparse_matrix_rows(incidence), 9);
            assert_eq!(pio_sparse_matrix_columns(incidence), 9);
            assert_eq!(pio_sparse_matrix_values(incidence).len, 18);

            let branch = pio_calc_branch_susceptances(
                network,
                c"reactance_only".as_ptr(),
                "reactance_only".len(),
                &mut error,
            );
            assert!(!branch.is_null(), "{}", error_text(error));
            assert_eq!(pio_vector_values(branch).len, 9);

            pio_vector_release(branch);
            pio_sparse_matrix_release(incidence);
            pio_balanced_network_release(network);
            pio_value_release(value);
            pio_module_release(module);
        }
    }

    #[test]
    fn powerio_ir_uses_serialize_and_deserialize() {
        unsafe {
            let module = parse_case9();
            let mut error = std::ptr::null_mut();
            let destination = pio_destination_memory(
                c"case.pio.json".as_ptr(),
                "case.pio.json".len(),
                &mut error,
            );
            let result = pio_module_serialize(module, destination, &mut error);
            assert!(!result.is_null(), "{}", error_text(error));
            let artifact = pio_emit_result_artifact(result, 0, &mut error);
            let bytes = pio_artifact_bytes(artifact);
            let source = pio_source_from_memory(
                c"case.pio.json".as_ptr(),
                "case.pio.json".len(),
                bytes.data,
                bytes.len,
                &mut error,
            );
            let decoded = pio_module_deserialize(source, &mut error);
            assert!(!decoded.is_null(), "{}", error_text(error));

            pio_module_release(decoded);
            pio_source_release(source);
            pio_artifact_release(artifact);
            pio_emit_result_release(result);
            pio_destination_release(destination);
            pio_module_release(module);
        }
    }
}
