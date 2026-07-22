//! Lowering records and preflight checks.
//!
//! Lowering is where PowerIO is a compiler rather than a parser: every pass that
//! transforms one model into another (normalization, multiconductor to balanced,
//! emission to a target format) appends a [`LoweringRecord`] to the package's
//! `lowering_history`, so the transformation is auditable. The most consequential
//! case, multiconductor to balanced, must be an explicit pass with diagnostics,
//! never a silent positive sequence projection.

use std::collections::{BTreeMap, BTreeSet};
use std::f64::consts::PI;

use num_complex::Complex64;
use serde::{Deserialize, Serialize};

use powerio::{
    BalancedNetwork, Branch, BranchCharging, Bus, BusId, BusType, Extras as BalancedExtras,
    Generator, Load, Network, Shunt, SourceFormat,
};
use powerio_dist::{DistBus, DistLineCode, DistLoadVoltageModel, Mat, MulticonductorNetwork};

use crate::diagnostics::{DiagnosticSeverity, DiagnosticStage, StructuredDiagnostic};
use crate::model::ModelKind;
use crate::validation::ValidationStatus;

/// One lowering/normalization/emission pass and what it changed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LoweringRecord {
    /// A stable pass name, e.g. `normalize-balanced` or `multiconductor-to-balanced`.
    pub pass: String,
    pub input_kind: ModelKind,
    pub output_kind: ModelKind,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub options: serde_json::Map<String, serde_json::Value>,
    /// Modeling assumptions the pass relied on (e.g. "balanced four-wire feeder").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assumptions: Vec<String>,
    /// Approximations the pass introduced (e.g. "Kron reduction of neutral").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approximations: Vec<String>,
    /// Fields/constraints dropped because the output family cannot carry them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dropped_fields: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<StructuredDiagnostic>,
    pub validation_status: ValidationStatus,
}

impl LoweringRecord {
    pub fn new(pass: impl Into<String>, input_kind: ModelKind, output_kind: ModelKind) -> Self {
        Self {
            pass: pass.into(),
            input_kind,
            output_kind,
            options: serde_json::Map::new(),
            assumptions: Vec::new(),
            approximations: Vec::new(),
            dropped_fields: Vec::new(),
            diagnostics: Vec::new(),
            validation_status: ValidationStatus::Ok,
        }
    }
}

/// Sequence transform used by the multiconductor to balanced lowering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SequenceTransformConvention {
    FortescuePowerInvariant,
}

impl std::fmt::Display for SequenceTransformConvention {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FortescuePowerInvariant => f.write_str("FortescuePowerInvariant"),
        }
    }
}

const DEFAULT_LOWERING_BASE_MVA: f64 = 100.0;
const SQRT_3: f64 = 1.732_050_807_568_877_2;
const COUPLING_TOLERANCE: f64 = 1.0e-9;

fn default_lowering_base_mva() -> f64 {
    DEFAULT_LOWERING_BASE_MVA
}

/// Options for the multiconductor to balanced lowering preflight and pass.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MulticonductorToBalancedOptions {
    pub convention: SequenceTransformConvention,
    /// Three phase system power base used for the balanced per-unit projection.
    #[serde(default = "default_lowering_base_mva")]
    pub base_mva: f64,
}

impl Default for MulticonductorToBalancedOptions {
    fn default() -> Self {
        Self {
            convention: SequenceTransformConvention::FortescuePowerInvariant,
            base_mva: DEFAULT_LOWERING_BASE_MVA,
        }
    }
}

/// Readiness report for the multiconductor to balanced lowering pass.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MulticonductorToBalancedReadiness {
    pub convention: SequenceTransformConvention,
    pub base_mva: f64,
    pub status: ValidationStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assumptions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approximations: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<StructuredDiagnostic>,
}

impl MulticonductorToBalancedReadiness {
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.status <= ValidationStatus::Info
    }
}

/// A successful raw multiconductor to balanced lowering result.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MulticonductorToBalancedLowering {
    pub network: BalancedNetwork,
    pub record: LoweringRecord,
}

/// Structured failure from the raw multiconductor to balanced lowering pass.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MulticonductorToBalancedError {
    pub options: MulticonductorToBalancedOptions,
    pub status: ValidationStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<StructuredDiagnostic>,
}

impl MulticonductorToBalancedError {
    pub fn new(
        options: MulticonductorToBalancedOptions,
        diagnostics: Vec<StructuredDiagnostic>,
    ) -> Self {
        Self {
            options,
            status: status_from_diagnostics(&diagnostics),
            diagnostics,
        }
    }
}

impl std::fmt::Display for MulticonductorToBalancedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.diagnostics.first() {
            Some(diagnostic) => write!(f, "{}", diagnostic.message),
            None => f.write_str("multiconductor to balanced lowering failed"),
        }
    }
}

impl std::error::Error for MulticonductorToBalancedError {}

/// Check whether a multiconductor package is ready for the lowering pass.
///
/// This is a preflight only: it reports the assumptions and blockers that the
/// lowering would need to account for, but it does not produce a balanced model
/// and does not append to `lowering_history`.
#[must_use]
pub fn check_multiconductor_to_balanced_lowering(
    net: &MulticonductorNetwork,
    options: MulticonductorToBalancedOptions,
) -> MulticonductorToBalancedReadiness {
    let mut report = MulticonductorToBalancedReadiness {
        convention: options.convention,
        base_mva: options.base_mva,
        status: ValidationStatus::Ok,
        assumptions: vec![format!(
            "sequence transform convention: {}",
            options.convention
        )],
        approximations: Vec::new(),
        diagnostics: Vec::new(),
    };

    check_options(options, &mut report);
    check_bus_conductor_sets(net, &mut report);
    check_phase_reference(net, &mut report);
    check_line_terminal_maps(net, &mut report);
    check_linecodes(net, &mut report);
    check_switches(net, &mut report);
    check_transformers(net, &mut report);
    check_untyped_objects(net, &mut report);

    report.status = status_from_diagnostics(&report.diagnostics);
    report
}

/// Lower a transparent three phase multiconductor network to a balanced model.
///
/// The pass is explicit. It does not run from readers, writers, matrix builders,
/// bindings, or package deserialization. Unsupported inputs return structured
/// `LOWER.MULTI_TO_BALANCED.*` diagnostics in [`MulticonductorToBalancedError`].
pub fn lower_multiconductor_to_balanced(
    net: &MulticonductorNetwork,
    options: MulticonductorToBalancedOptions,
) -> Result<MulticonductorToBalancedLowering, MulticonductorToBalancedError> {
    let readiness = check_multiconductor_to_balanced_lowering(net, options);
    if !readiness.is_ready() {
        return Err(MulticonductorToBalancedError::new(
            options,
            readiness.diagnostics,
        ));
    }

    let mut state = LoweringState::new(net, options, readiness);
    state.lower()
}

struct LoweringState<'a> {
    net: &'a MulticonductorNetwork,
    options: MulticonductorToBalancedOptions,
    neutral_terminals: BTreeSet<String>,
    bus_ids: BTreeMap<String, BusId>,
    record: LoweringRecord,
}

impl<'a> LoweringState<'a> {
    fn new(
        net: &'a MulticonductorNetwork,
        options: MulticonductorToBalancedOptions,
        readiness: MulticonductorToBalancedReadiness,
    ) -> Self {
        let mut record = LoweringRecord::new(
            "multiconductor-to-balanced",
            ModelKind::Multiconductor,
            ModelKind::Balanced,
        );
        record.options = options_map(options);
        record.assumptions = readiness.assumptions;
        record.approximations = readiness.approximations;
        record.diagnostics = readiness.diagnostics;
        record
            .assumptions
            .push(format!("balanced power base: {} MVA", options.base_mva));
        record
            .assumptions
            .push("balanced bus ids are synthesized from multiconductor bus order".to_owned());
        record.approximations.push(
            "wire-coordinate branch and shunt matrices are projected to positive sequence"
                .to_owned(),
        );
        record.approximations.push(
            "phase injection records are aggregated into scalar balanced injections".to_owned(),
        );
        record.approximations.push(
            "units are converted from W/var/V/ohm/siemens/radians to MW/MVAr/per-unit/degrees"
                .to_owned(),
        );
        if net.switches.iter().any(|sw| sw.open) {
            record
                .dropped_fields
                .push("open switches dropped from balanced model".to_owned());
        }

        let bus_ids = net
            .buses
            .iter()
            .enumerate()
            .map(|(idx, bus)| (bus.id.to_ascii_lowercase(), BusId(idx + 1)))
            .collect();

        Self {
            net,
            options,
            neutral_terminals: global_neutral_terminals(net),
            bus_ids,
            record,
        }
    }

    #[allow(clippy::too_many_lines)]
    fn lower(&mut self) -> Result<MulticonductorToBalancedLowering, MulticonductorToBalancedError> {
        let Some(base) = self.voltage_base()? else {
            return Err(MulticonductorToBalancedError::new(
                self.options,
                self.record.diagnostics.clone(),
            ));
        };

        let buses = self.lower_buses(base);
        let branches = self.lower_lines(base)?;
        let loads = self.lower_loads();
        let shunts = self.lower_shunts(base)?;
        let generators = self.lower_generators(&buses);
        self.err_if_errors()?;

        let mut network = Network::new(
            self.net
                .name
                .clone()
                .unwrap_or_else(|| "lowered-multiconductor".to_owned()),
            self.options.base_mva,
        );
        network.base_frequency = self.net.base_frequency;
        network.buses = buses;
        network.loads = loads;
        network.shunts = shunts;
        network.branches = branches;
        network.generators = generators;
        network.source_format = SourceFormat::InMemory;

        if let Err(err) = network.validate() {
            self.record.diagnostics.push(StructuredDiagnostic::new(
                "LOWER.MULTI_TO_BALANCED.INVALID_BALANCED_OUTPUT",
                DiagnosticSeverity::Error,
                DiagnosticStage::Lower,
                format!("lowered balanced network failed structural validation: {err}"),
            ));
            return Err(MulticonductorToBalancedError::new(
                self.options,
                self.record.diagnostics.clone(),
            ));
        }
        for finding in network.validate_values() {
            self.record.diagnostics.push(
                StructuredDiagnostic::new(
                    "LOWER.MULTI_TO_BALANCED.BALANCED_VALUE_DOMAIN",
                    DiagnosticSeverity::Warning,
                    DiagnosticStage::Lower,
                    format!(
                        "{} field `{}` is outside its value domain after lowering",
                        finding.element, finding.field
                    ),
                )
                .with_suggested_action(
                    "Inspect the multiconductor source values before using the lowered model.",
                ),
            );
        }

        self.record.validation_status = status_from_diagnostics(&self.record.diagnostics);
        Ok(MulticonductorToBalancedLowering {
            network,
            record: self.record.clone(),
        })
    }

    fn voltage_base(&mut self) -> Result<Option<VoltageBase>, MulticonductorToBalancedError> {
        for (idx, source) in self.net.sources.iter().enumerate() {
            let Some(bus) = self.net.bus(&source.bus) else {
                self.record.diagnostics.push(
                    StructuredDiagnostic::new(
                        "LOWER.MULTI_TO_BALANCED.UNKNOWN_SOURCE_BUS",
                        DiagnosticSeverity::Error,
                        DiagnosticStage::Lower,
                        format!(
                            "voltage source {} references unknown bus {}",
                            source.name, source.bus
                        ),
                    )
                    .with_element_path(format!("/model/multiconductor_network/sources/{idx}/bus")),
                );
                continue;
            };
            let positions =
                active_positions(&source.terminal_map, Some(bus), &self.neutral_terminals);
            if positions.len() != 3 {
                continue;
            }
            let Some(v1) = positive_sequence_voltage(source, &positions) else {
                self.record.diagnostics.push(
                    StructuredDiagnostic::new(
                        "LOWER.MULTI_TO_BALANCED.INVALID_PHASE_REFERENCE",
                        DiagnosticSeverity::Error,
                        DiagnosticStage::Lower,
                        format!(
                            "voltage source {} does not carry finite three phase voltage magnitudes and angles",
                            source.name
                        ),
                    )
                    .with_element_path(format!("/model/multiconductor_network/sources/{idx}")),
                );
                continue;
            };
            let line_to_line_volts = v1.norm();
            if !line_to_line_volts.is_finite() || line_to_line_volts <= 0.0 {
                self.record.diagnostics.push(
                    StructuredDiagnostic::new(
                        "LOWER.MULTI_TO_BALANCED.INVALID_PHASE_REFERENCE",
                        DiagnosticSeverity::Error,
                        DiagnosticStage::Lower,
                        format!(
                            "voltage source {} produced a non-positive positive-sequence voltage base",
                            source.name
                        ),
                    )
                    .with_element_path(format!("/model/multiconductor_network/sources/{idx}")),
                );
                continue;
            }
            self.record.assumptions.push(format!(
                "voltage base synthesized from source {} positive-sequence voltage: {} kV line-to-line",
                source.name,
                line_to_line_volts / 1000.0
            ));
            return Ok(Some(VoltageBase { line_to_line_volts }));
        }

        if self
            .record
            .diagnostics
            .iter()
            .any(|d| d.severity >= DiagnosticSeverity::Error)
        {
            return Err(MulticonductorToBalancedError::new(
                self.options,
                self.record.diagnostics.clone(),
            ));
        }
        self.record.diagnostics.push(StructuredDiagnostic::new(
            "LOWER.MULTI_TO_BALANCED.MISSING_PHASE_REFERENCE",
            DiagnosticSeverity::Error,
            DiagnosticStage::Lower,
            "multiconductor to balanced lowering requires a finite three phase voltage source reference",
        ));
        Ok(None)
    }

    fn lower_buses(&mut self, base: VoltageBase) -> Vec<Bus> {
        self.net
            .buses
            .iter()
            .enumerate()
            .map(|(idx, bus)| {
                let source = self
                    .net
                    .sources
                    .iter()
                    .find(|source| source.bus.eq_ignore_ascii_case(&bus.id));
                let (vm, va) = source
                    .and_then(|source| {
                        let positions = active_positions(
                            &source.terminal_map,
                            Some(bus),
                            &self.neutral_terminals,
                        );
                        positive_sequence_voltage(source, &positions)
                    })
                    .map_or((1.0, 0.0), |v| {
                        (
                            v.norm() / base.line_to_line_volts,
                            radians_to_degrees(v.arg()),
                        )
                    });
                if source.is_none() {
                    self.record.dropped_fields.push(format!(
                        "bus {} voltage magnitude and angle defaulted to 1.0 p.u. and 0 degrees",
                        bus.id
                    ));
                }
                let (vmin, vmax) = match (bus.v_min, bus.v_max) {
                    (Some(vmin), Some(vmax)) if vmin.is_finite() && vmax.is_finite() => (
                        vmin / base.line_to_line_volts,
                        vmax / base.line_to_line_volts,
                    ),
                    _ => {
                        self.record.dropped_fields.push(format!(
                            "bus {} voltage bounds defaulted to 0.9/1.1 p.u.",
                            bus.id
                        ));
                        (0.9, 1.1)
                    }
                };
                self.record_bus_bound_drops(bus);
                let mut balanced = Bus::new(
                    BusId(idx + 1),
                    self.bus_kind(&bus.id),
                    base.line_to_line_volts / 1000.0,
                );
                balanced.vm = vm;
                balanced.va = va;
                balanced.vmax = vmax;
                balanced.vmin = vmin;
                balanced.name = Some(bus.id.clone());
                balanced.extras = source_extra("multiconductor_bus_id", &bus.id);
                balanced
            })
            .collect()
    }

    fn record_bus_bound_drops(&mut self, bus: &DistBus) {
        if bus.vpn_min.is_some()
            || bus.vpn_max.is_some()
            || bus.vpp_min.is_some()
            || bus.vpp_max.is_some()
            || bus.vpos_min.is_some()
            || bus.vpos_max.is_some()
            || bus.vneg_max.is_some()
            || bus.vzero_max.is_some()
            || bus.vn_max.is_some()
        {
            self.record.dropped_fields.push(format!(
                "bus {} conductor voltage bound families dropped",
                bus.id
            ));
        }
    }

    fn bus_kind(&self, bus_id: &str) -> BusType {
        if self
            .net
            .sources
            .iter()
            .any(|source| source.bus.eq_ignore_ascii_case(bus_id))
        {
            BusType::Ref
        } else if self
            .net
            .generators
            .iter()
            .any(|generator| generator.bus.eq_ignore_ascii_case(bus_id))
        {
            BusType::Pv
        } else {
            BusType::Pq
        }
    }

    #[allow(clippy::too_many_lines)]
    fn lower_lines(
        &mut self,
        base: VoltageBase,
    ) -> Result<Vec<Branch>, MulticonductorToBalancedError> {
        let mut branches = Vec::with_capacity(self.net.lines.len());
        for (idx, line) in self.net.lines.iter().enumerate() {
            let Some(code) = self.net.linecode(&line.linecode) else {
                self.record.diagnostics.push(
                    StructuredDiagnostic::new(
                        "LOWER.MULTI_TO_BALANCED.UNKNOWN_LINECODE",
                        DiagnosticSeverity::Error,
                        DiagnosticStage::Lower,
                        format!(
                            "line {} references unknown linecode `{}`",
                            line.name, line.linecode
                        ),
                    )
                    .with_element_path(format!(
                        "/model/multiconductor_network/lines/{idx}/linecode"
                    )),
                );
                continue;
            };
            if !same_active_phase_order(
                self.net.bus(&line.bus_from),
                &line.terminal_map_from,
                self.net.bus(&line.bus_to),
                &line.terminal_map_to,
                &self.neutral_terminals,
            ) {
                self.record.diagnostics.push(
                    StructuredDiagnostic::new(
                        "LOWER.MULTI_TO_BALANCED.PHASE_MAP_MISMATCH",
                        DiagnosticSeverity::Error,
                        DiagnosticStage::Lower,
                        format!(
                            "line {} connects different active terminal orders and cannot be lowered transparently",
                            line.name
                        ),
                    )
                    .with_element_path(format!("/model/multiconductor_network/lines/{idx}")),
                );
                continue;
            }
            let Some(from) = self.bus_id(&line.bus_from) else {
                self.unknown_bus_diag("line", &line.name, &line.bus_from, idx, "bus_from");
                continue;
            };
            let Some(to) = self.bus_id(&line.bus_to) else {
                self.unknown_bus_diag("line", &line.name, &line.bus_to, idx, "bus_to");
                continue;
            };
            let from_bus = self.net.bus(&line.bus_from);
            let active =
                active_positions(&line.terminal_map_from, from_bus, &self.neutral_terminals);
            let neutral =
                neutral_positions(&line.terminal_map_from, from_bus, &self.neutral_terminals);
            let z_ohm =
                self.line_positive_sequence_impedance(idx, code, &active, &neutral, line.length)?;
            let y_from = self.line_positive_sequence_admittance(
                idx,
                code,
                &active,
                &neutral,
                line.length,
                ShuntSide::From,
            )?;
            let y_to = self.line_positive_sequence_admittance(
                idx,
                code,
                &active,
                &neutral,
                line.length,
                ShuntSide::To,
            )?;
            let z_base = base.z_base_ohm(self.options.base_mva);
            let y_scale = z_base;
            let charging = BranchCharging::new(
                y_from.re * y_scale,
                y_from.im * y_scale,
                y_to.re * y_scale,
                y_to.im * y_scale,
            );
            let rate = line_rate_mva(code, &active, base.line_to_line_volts).unwrap_or_else(|| {
                self.record.dropped_fields.push(format!(
                    "line {} thermal rating defaulted to 0 MVA",
                    line.name
                ));
                0.0
            });
            let mut branch = Branch::new(from, to, z_ohm.re / z_base, z_ohm.im / z_base);
            branch.b = charging.total_b();
            branch.charging = Some(charging);
            branch.rate_a = rate;
            branch.rate_b = rate;
            branch.rate_c = rate;
            branch.extras = source_extra("multiconductor_line", &line.name);
            branches.push(branch);
        }
        self.err_if_errors()?;
        Ok(branches)
    }

    fn line_positive_sequence_impedance(
        &mut self,
        line_idx: usize,
        code: &DistLineCode,
        active: &[usize],
        neutral: &[usize],
        length: f64,
    ) -> Result<Complex64, MulticonductorToBalancedError> {
        let matrix = complex_matrix(&code.r_series, &code.x_series, length);
        let reduced = kron_or_select(&matrix, active, neutral).map_err(|message| {
            self.matrix_error(line_idx, &code.name, "series impedance", &message)
        })?;
        Ok(self.positive_sequence_from_matrix(line_idx, &code.name, "series impedance", &reduced))
    }

    fn line_positive_sequence_admittance(
        &mut self,
        line_idx: usize,
        code: &DistLineCode,
        active: &[usize],
        neutral: &[usize],
        length: f64,
        side: ShuntSide,
    ) -> Result<Complex64, MulticonductorToBalancedError> {
        let (g, b, label) = match side {
            ShuntSide::From => (&code.g_from, &code.b_from, "from shunt admittance"),
            ShuntSide::To => (&code.g_to, &code.b_to, "to shunt admittance"),
        };
        let matrix = complex_matrix(g, b, length);
        let reduced = kron_or_select(&matrix, active, neutral)
            .map_err(|message| self.matrix_error(line_idx, &code.name, label, &message))?;
        Ok(self.positive_sequence_from_matrix(line_idx, &code.name, label, &reduced))
    }

    fn positive_sequence_from_matrix(
        &mut self,
        line_idx: usize,
        code_name: &str,
        label: &str,
        matrix: &[Vec<Complex64>],
    ) -> Complex64 {
        let seq = sequence_matrix(matrix);
        let coupling = sequence_coupling_norm(&seq);
        if coupling > COUPLING_TOLERANCE {
            self.record.approximations.push(format!(
                "linecode {code_name} {label} has sequence coupling norm {coupling}; positive-sequence diagonal retained"
            ));
            let mut diagnostic = StructuredDiagnostic::new(
                "LOWER.MULTI_TO_BALANCED.SEQUENCE_COUPLING_DROPPED",
                DiagnosticSeverity::Info,
                DiagnosticStage::Lower,
                format!(
                    "linecode {code_name} {label} has nonzero sequence coupling; the balanced model keeps the positive-sequence diagonal"
                ),
            )
            .with_element_path(format!("/model/multiconductor_network/lines/{line_idx}/linecode"));
            diagnostic.details.insert(
                "sequence_coupling_norm".to_owned(),
                serde_json::json!(coupling),
            );
            self.record.diagnostics.push(diagnostic);
        }
        seq[1][1]
    }

    fn matrix_error(
        &self,
        line_idx: usize,
        code_name: &str,
        label: &str,
        message: &str,
    ) -> MulticonductorToBalancedError {
        let mut diagnostics = self.record.diagnostics.clone();
        diagnostics.push(
            StructuredDiagnostic::new(
                "LOWER.MULTI_TO_BALANCED.INVALID_LINECODE_MATRIX",
                DiagnosticSeverity::Error,
                DiagnosticStage::Lower,
                format!("linecode {code_name} {label} cannot be lowered: {message}"),
            )
            .with_element_path(format!(
                "/model/multiconductor_network/lines/{line_idx}/linecode"
            )),
        );
        MulticonductorToBalancedError::new(self.options, diagnostics)
    }

    fn lower_loads(&mut self) -> Vec<Load> {
        self.net
            .loads
            .iter()
            .enumerate()
            .filter_map(|(idx, load)| {
                let Some(bus) = self.bus_id(&load.bus) else {
                    self.unknown_bus_diag("load", &load.name, &load.bus, idx, "bus");
                    return None;
                };
                if !matches!(
                    load.voltage_model,
                    DistLoadVoltageModel::ConstantPower { .. }
                ) {
                    self.record.dropped_fields.push(format!(
                        "load {} voltage model dropped; balanced load is constant power",
                        load.name
                    ));
                    self.record.diagnostics.push(
                        StructuredDiagnostic::new(
                            "LOWER.MULTI_TO_BALANCED.DROPPED_LOAD_VOLTAGE_MODEL",
                            DiagnosticSeverity::Warning,
                            DiagnosticStage::Lower,
                            format!(
                                "load {} voltage model cannot be represented by the conservative balanced lowering",
                                load.name
                            ),
                        )
                        .with_element_path(format!("/model/multiconductor_network/loads/{idx}/voltage_model")),
                    );
                }
                let mut balanced = Load::new(
                    bus,
                    si_power_to_mega(load.p_nom.iter().sum()),
                    si_power_to_mega(load.q_nom.iter().sum()),
                );
                balanced.extras = source_extra("multiconductor_load", &load.name);
                Some(balanced)
            })
            .collect()
    }

    fn lower_shunts(
        &mut self,
        base: VoltageBase,
    ) -> Result<Vec<Shunt>, MulticonductorToBalancedError> {
        let mut shunts = Vec::with_capacity(self.net.shunts.len());
        for (idx, shunt) in self.net.shunts.iter().enumerate() {
            let Some(bus) = self.bus_id(&shunt.bus) else {
                self.unknown_bus_diag("shunt", &shunt.name, &shunt.bus, idx, "bus");
                continue;
            };
            let dist_bus = self.net.bus(&shunt.bus);
            let active = active_positions(&shunt.terminal_map, dist_bus, &self.neutral_terminals);
            let neutral = neutral_positions(&shunt.terminal_map, dist_bus, &self.neutral_terminals);
            let y = if active.len() == 3 {
                let matrix = complex_matrix(&shunt.g, &shunt.b, 1.0);
                let reduced = kron_or_select(&matrix, &active, &neutral)
                    .map_err(|message| self.shunt_matrix_error(idx, &shunt.name, &message))?;
                let seq = sequence_matrix(&reduced);
                seq[1][1]
            } else {
                self.record.approximations.push(format!(
                    "shunt {} has {} active terminal(s); diagonal admittance projected with missing phases as zero",
                    shunt.name,
                    active.len()
                ));
                partial_phase_admittance(&shunt.g, &shunt.b, &active)
            };
            let scale = base.line_to_line_volts * base.line_to_line_volts / 1_000_000.0;
            let mut balanced = Shunt::new(bus, y.re * scale, y.im * scale);
            balanced.extras = source_extra("multiconductor_shunt", &shunt.name);
            shunts.push(balanced);
        }
        self.err_if_errors()?;
        Ok(shunts)
    }

    fn shunt_matrix_error(
        &self,
        shunt_idx: usize,
        name: &str,
        message: &str,
    ) -> MulticonductorToBalancedError {
        let mut diagnostics = self.record.diagnostics.clone();
        diagnostics.push(
            StructuredDiagnostic::new(
                "LOWER.MULTI_TO_BALANCED.INVALID_SHUNT_MATRIX",
                DiagnosticSeverity::Error,
                DiagnosticStage::Lower,
                format!("shunt {name} cannot be lowered: {message}"),
            )
            .with_element_path(format!("/model/multiconductor_network/shunts/{shunt_idx}")),
        );
        MulticonductorToBalancedError::new(self.options, diagnostics)
    }

    fn lower_generators(&mut self, buses: &[Bus]) -> Vec<Generator> {
        self.net
            .generators
            .iter()
            .enumerate()
            .filter_map(|(idx, generator)| {
                let Some(bus) = self.bus_id(&generator.bus) else {
                    self.unknown_bus_diag("generator", &generator.name, &generator.bus, idx, "bus");
                    return None;
                };
                let pg = si_power_to_mega(generator.p_nom.iter().sum());
                let qg = si_power_to_mega(generator.q_nom.iter().sum());
                let pmin = option_vec_sum_mw(generator.p_min.as_deref()).unwrap_or_else(|| {
                    self.record.dropped_fields.push(format!(
                        "generator {} p_min defaulted to pg",
                        generator.name
                    ));
                    pg
                });
                let pmax = option_vec_sum_mw(generator.p_max.as_deref()).unwrap_or_else(|| {
                    self.record.dropped_fields.push(format!(
                        "generator {} p_max defaulted to pg",
                        generator.name
                    ));
                    pg
                });
                let qmin = option_vec_sum_mw(generator.q_min.as_deref()).unwrap_or_else(|| {
                    self.record.dropped_fields.push(format!(
                        "generator {} q_min defaulted to qg",
                        generator.name
                    ));
                    qg
                });
                let qmax = option_vec_sum_mw(generator.q_max.as_deref()).unwrap_or_else(|| {
                    self.record.dropped_fields.push(format!(
                        "generator {} q_max defaulted to qg",
                        generator.name
                    ));
                    qg
                });
                if generator.cost.is_some() {
                    self.record.dropped_fields.push(format!(
                        "generator {} scalar distribution cost dropped",
                        generator.name
                    ));
                }
                let vg = buses
                    .iter()
                    .find(|balanced_bus| balanced_bus.id == bus)
                    .map_or(1.0, |balanced_bus| balanced_bus.vm);
                let mut balanced = Generator::new(bus);
                balanced.pg = pg;
                balanced.qg = qg;
                balanced.pmax = pmax;
                balanced.pmin = pmin;
                balanced.qmax = qmax;
                balanced.qmin = qmin;
                balanced.vg = vg;
                balanced.mbase = self.options.base_mva;
                Some(balanced)
            })
            .collect()
    }

    fn bus_id(&self, bus: &str) -> Option<BusId> {
        self.bus_ids.get(&bus.to_ascii_lowercase()).copied()
    }

    fn unknown_bus_diag(&mut self, element: &str, name: &str, bus: &str, idx: usize, field: &str) {
        self.record.diagnostics.push(
            StructuredDiagnostic::new(
                "LOWER.MULTI_TO_BALANCED.UNKNOWN_BUS",
                DiagnosticSeverity::Error,
                DiagnosticStage::Lower,
                format!("{element} {name} references unknown bus {bus}"),
            )
            .with_element_path(format!(
                "/model/multiconductor_network/{element}s/{idx}/{field}"
            )),
        );
    }

    fn err_if_errors(&self) -> Result<(), MulticonductorToBalancedError> {
        if self
            .record
            .diagnostics
            .iter()
            .any(|d| d.severity >= DiagnosticSeverity::Error)
        {
            Err(MulticonductorToBalancedError::new(
                self.options,
                self.record.diagnostics.clone(),
            ))
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Copy)]
struct VoltageBase {
    line_to_line_volts: f64,
}

impl VoltageBase {
    fn z_base_ohm(self, base_mva: f64) -> f64 {
        self.line_to_line_volts * self.line_to_line_volts / (base_mva * 1_000_000.0)
    }
}

#[derive(Clone, Copy)]
enum ShuntSide {
    From,
    To,
}

fn options_map(
    options: MulticonductorToBalancedOptions,
) -> serde_json::Map<String, serde_json::Value> {
    serde_json::to_value(options)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

fn source_extra(key: &str, value: &str) -> BalancedExtras {
    let mut extras = BalancedExtras::new();
    extras.insert(key.to_owned(), serde_json::Value::String(value.to_owned()));
    extras
}

fn active_positions(
    terminals: &[String],
    bus: Option<&DistBus>,
    neutral_terminals: &BTreeSet<String>,
) -> Vec<usize> {
    terminals
        .iter()
        .enumerate()
        .filter_map(|(idx, terminal)| {
            (!is_neutral_terminal(terminal, bus, neutral_terminals)).then_some(idx)
        })
        .collect()
}

fn neutral_positions(
    terminals: &[String],
    bus: Option<&DistBus>,
    neutral_terminals: &BTreeSet<String>,
) -> Vec<usize> {
    terminals
        .iter()
        .enumerate()
        .filter_map(|(idx, terminal)| {
            is_neutral_terminal(terminal, bus, neutral_terminals).then_some(idx)
        })
        .collect()
}

fn same_active_phase_order(
    from_bus: Option<&DistBus>,
    from_terminals: &[String],
    to_bus: Option<&DistBus>,
    to_terminals: &[String],
    neutral_terminals: &BTreeSet<String>,
) -> bool {
    let from: Vec<_> = from_terminals
        .iter()
        .filter(|terminal| !is_neutral_terminal(terminal, from_bus, neutral_terminals))
        .map(|terminal| terminal.to_ascii_lowercase())
        .collect();
    let to: Vec<_> = to_terminals
        .iter()
        .filter(|terminal| !is_neutral_terminal(terminal, to_bus, neutral_terminals))
        .map(|terminal| terminal.to_ascii_lowercase())
        .collect();
    from == to
}

fn positive_sequence_voltage(
    source: &powerio_dist::VoltageSource,
    positions: &[usize],
) -> Option<Complex64> {
    if positions.len() != 3 {
        return None;
    }
    let mut phase = [Complex64::new(0.0, 0.0); 3];
    for (out, &idx) in phase.iter_mut().zip(positions.iter()) {
        let magnitude = *source.v_magnitude.get(idx)?;
        let angle = *source.v_angle.get(idx)?;
        if !magnitude.is_finite() || !angle.is_finite() {
            return None;
        }
        *out = Complex64::from_polar(magnitude, angle);
    }
    let basis = sequence_basis();
    let mut seq = [Complex64::new(0.0, 0.0); 3];
    for (sequence_idx, out) in seq.iter_mut().enumerate() {
        for phase_idx in 0..3 {
            *out += basis[phase_idx][sequence_idx].conj() * phase[phase_idx];
        }
    }
    Some(seq[1])
}

fn complex_matrix(g_or_r: &Mat, b_or_x: &Mat, scale: f64) -> Vec<Vec<Complex64>> {
    g_or_r
        .iter()
        .zip(b_or_x.iter())
        .map(|(g_row, b_row)| {
            g_row
                .iter()
                .zip(b_row.iter())
                .map(|(&g, &b)| Complex64::new(g * scale, b * scale))
                .collect()
        })
        .collect()
}

fn kron_or_select(
    matrix: &[Vec<Complex64>],
    active: &[usize],
    neutral: &[usize],
) -> Result<Vec<Vec<Complex64>>, String> {
    if active.len() != 3 {
        return Err(format!(
            "expected three active conductors, got {}",
            active.len()
        ));
    }
    validate_indices(matrix, active)?;
    validate_indices(matrix, neutral)?;
    if neutral.is_empty() {
        return Ok(submatrix(matrix, active, active));
    }

    let m_pp = submatrix(matrix, active, active);
    let m_pn = submatrix(matrix, active, neutral);
    let m_np = submatrix(matrix, neutral, active);
    let m_nn = submatrix(matrix, neutral, neutral);
    if matrix_is_near_zero(&m_pn) && matrix_is_near_zero(&m_np) && matrix_is_near_zero(&m_nn) {
        return Ok(m_pp);
    }
    let inv_nn = invert_complex_matrix(&m_nn)?;
    let correction = matmul(&matmul(&m_pn, &inv_nn), &m_np);
    Ok(matrix_sub(&m_pp, &correction))
}

fn matrix_is_near_zero(matrix: &[Vec<Complex64>]) -> bool {
    matrix
        .iter()
        .flatten()
        .all(|value| value.norm() <= f64::EPSILON)
}

fn validate_indices(matrix: &[Vec<Complex64>], indices: &[usize]) -> Result<(), String> {
    let n = matrix.len();
    if matrix.iter().any(|row| row.len() != n) {
        return Err("matrix is not square".to_owned());
    }
    if indices.iter().any(|&idx| idx >= n) {
        return Err("terminal map references a conductor outside the matrix".to_owned());
    }
    Ok(())
}

fn submatrix(matrix: &[Vec<Complex64>], rows: &[usize], cols: &[usize]) -> Vec<Vec<Complex64>> {
    rows.iter()
        .map(|&row| cols.iter().map(|&col| matrix[row][col]).collect())
        .collect()
}

#[allow(clippy::needless_range_loop)]
fn invert_complex_matrix(matrix: &[Vec<Complex64>]) -> Result<Vec<Vec<Complex64>>, String> {
    let n = matrix.len();
    if n == 0 || matrix.iter().any(|row| row.len() != n) {
        return Err("neutral block is not square".to_owned());
    }
    let mut aug = vec![vec![Complex64::new(0.0, 0.0); 2 * n]; n];
    for i in 0..n {
        for j in 0..n {
            aug[i][j] = matrix[i][j];
        }
        aug[i][n + i] = Complex64::new(1.0, 0.0);
    }

    for col in 0..n {
        let pivot = (col..n)
            .max_by(|&a, &b| aug[a][col].norm_sqr().total_cmp(&aug[b][col].norm_sqr()))
            .ok_or_else(|| "neutral block is singular".to_owned())?;
        if aug[pivot][col].norm() <= f64::EPSILON {
            return Err("neutral block is singular".to_owned());
        }
        if pivot != col {
            aug.swap(pivot, col);
        }
        let pivot_value = aug[col][col];
        for j in 0..(2 * n) {
            aug[col][j] /= pivot_value;
        }
        for row in 0..n {
            if row == col {
                continue;
            }
            let factor = aug[row][col];
            if factor.norm() <= f64::EPSILON {
                continue;
            }
            for j in 0..(2 * n) {
                let pivot_entry = aug[col][j];
                aug[row][j] -= factor * pivot_entry;
            }
        }
    }

    Ok(aug
        .into_iter()
        .map(|row| row.into_iter().skip(n).collect())
        .collect())
}

fn matmul(a: &[Vec<Complex64>], b: &[Vec<Complex64>]) -> Vec<Vec<Complex64>> {
    if a.is_empty() || b.is_empty() {
        return Vec::new();
    }
    let rows = a.len();
    let cols = b[0].len();
    let inner = b.len();
    let mut out = vec![vec![Complex64::new(0.0, 0.0); cols]; rows];
    for i in 0..rows {
        for k in 0..inner {
            for j in 0..cols {
                out[i][j] += a[i][k] * b[k][j];
            }
        }
    }
    out
}

fn matrix_sub(a: &[Vec<Complex64>], b: &[Vec<Complex64>]) -> Vec<Vec<Complex64>> {
    a.iter()
        .zip(b.iter())
        .map(|(a_row, b_row)| {
            a_row
                .iter()
                .zip(b_row.iter())
                .map(|(&a_value, &b_value)| a_value - b_value)
                .collect()
        })
        .collect()
}

#[allow(clippy::many_single_char_names)]
fn sequence_basis() -> [[Complex64; 3]; 3] {
    let scale = 1.0 / SQRT_3;
    let a = Complex64::from_polar(1.0, 2.0 * PI / 3.0);
    let a2 = a * a;
    [
        [
            Complex64::new(scale, 0.0),
            Complex64::new(scale, 0.0),
            Complex64::new(scale, 0.0),
        ],
        [Complex64::new(scale, 0.0), a2 * scale, a * scale],
        [Complex64::new(scale, 0.0), a * scale, a2 * scale],
    ]
}

fn sequence_matrix(matrix: &[Vec<Complex64>]) -> [[Complex64; 3]; 3] {
    let basis = sequence_basis();
    let mut seq = [[Complex64::new(0.0, 0.0); 3]; 3];
    for p in 0..3 {
        for q in 0..3 {
            for i in 0..3 {
                for j in 0..3 {
                    seq[p][q] += basis[i][p].conj() * matrix[i][j] * basis[j][q];
                }
            }
        }
    }
    seq
}

fn sequence_coupling_norm(seq: &[[Complex64; 3]; 3]) -> f64 {
    let mut sum = 0.0;
    for (i, row) in seq.iter().enumerate() {
        for (j, value) in row.iter().enumerate() {
            if i != j {
                sum += value.norm_sqr();
            }
        }
    }
    sum.sqrt()
}

fn line_rate_mva(code: &DistLineCode, active: &[usize], line_to_line_volts: f64) -> Option<f64> {
    if let Some(s_max) = &code.s_max {
        let values: Vec<_> = active
            .iter()
            .filter_map(|&idx| s_max.get(idx).copied())
            .collect();
        if !values.is_empty() && values.iter().all(|value| value.is_finite()) {
            return Some(values.iter().sum::<f64>() / 1_000_000.0);
        }
    }
    let i_max = code.i_max.as_ref()?;
    let amps: Vec<_> = active
        .iter()
        .filter_map(|&idx| i_max.get(idx).copied())
        .filter(|value| value.is_finite() && *value >= 0.0)
        .collect();
    let amps = amps.into_iter().reduce(f64::min)?;
    Some(SQRT_3 * line_to_line_volts * amps / 1_000_000.0)
}

fn partial_phase_admittance(g: &Mat, b: &Mat, active: &[usize]) -> Complex64 {
    let mut total = Complex64::new(0.0, 0.0);
    for &idx in active {
        let Some(g_row) = g.get(idx) else {
            continue;
        };
        let Some(b_row) = b.get(idx) else {
            continue;
        };
        let Some(&g_value) = g_row.get(idx) else {
            continue;
        };
        let Some(&b_value) = b_row.get(idx) else {
            continue;
        };
        total += Complex64::new(g_value, b_value);
    }
    total / 3.0
}

fn si_power_to_mega(value: f64) -> f64 {
    value / 1_000_000.0
}

fn option_vec_sum_mw(values: Option<&[f64]>) -> Option<f64> {
    values.map(|v| si_power_to_mega(v.iter().sum()))
}

fn radians_to_degrees(value: f64) -> f64 {
    value * 180.0 / PI
}

fn status_from_diagnostics(diagnostics: &[StructuredDiagnostic]) -> ValidationStatus {
    diagnostics
        .iter()
        .map(|d| match d.severity {
            DiagnosticSeverity::Debug => ValidationStatus::Ok,
            DiagnosticSeverity::Info => ValidationStatus::Info,
            DiagnosticSeverity::Warning => ValidationStatus::Warning,
            DiagnosticSeverity::Error => ValidationStatus::Error,
            DiagnosticSeverity::Fatal => ValidationStatus::Fatal,
        })
        .max()
        .unwrap_or(ValidationStatus::Ok)
}

fn check_options(
    options: MulticonductorToBalancedOptions,
    report: &mut MulticonductorToBalancedReadiness,
) {
    if !options.base_mva.is_finite() || options.base_mva <= 0.0 {
        report.diagnostics.push(StructuredDiagnostic::new(
            "LOWER.MULTI_TO_BALANCED.INVALID_BASE_MVA",
            DiagnosticSeverity::Error,
            DiagnosticStage::Lower,
            format!(
                "base_mva must be positive and finite for multiconductor to balanced lowering; got {}",
                options.base_mva
            ),
        ));
    }
}

fn check_bus_conductor_sets(
    net: &MulticonductorNetwork,
    report: &mut MulticonductorToBalancedReadiness,
) {
    let neutral_terminals = global_neutral_terminals(net);
    let mut saw_neutral = false;
    for (i, bus) in net.buses.iter().enumerate() {
        let active_count = active_terminal_count(&bus.terminals, Some(bus), &neutral_terminals);
        if active_count < bus.terminals.len() {
            saw_neutral = true;
        }

        match active_count {
            3 => {}
            2 => report.diagnostics.push(
                StructuredDiagnostic::new(
                    "LOWER.MULTI_TO_BALANCED.AMBIGUOUS_TERMINAL_MAP",
                    DiagnosticSeverity::Error,
                    DiagnosticStage::Lower,
                    format!(
                        "bus {} has two active terminals; no unique positive sequence projection is defined",
                        bus.id
                    ),
                )
                .with_element_path(format!("/model/multiconductor_network/buses/{i}/terminals")),
            ),
            0 | 1 => report.diagnostics.push(
                StructuredDiagnostic::new(
                    "LOWER.MULTI_TO_BALANCED.UNSUPPORTED_CONDUCTOR_SET",
                    DiagnosticSeverity::Error,
                    DiagnosticStage::Lower,
                    format!(
                        "bus {} has {active_count} active terminal; multiconductor to balanced lowering starts with three phase input",
                        bus.id
                    ),
                )
                .with_element_path(format!("/model/multiconductor_network/buses/{i}/terminals")),
            ),
            _ => report.diagnostics.push(
                StructuredDiagnostic::new(
                    "LOWER.MULTI_TO_BALANCED.UNSUPPORTED_CONDUCTOR_SET",
                    DiagnosticSeverity::Error,
                    DiagnosticStage::Lower,
                    format!(
                        "bus {} has {active_count} active terminals; multiconductor to balanced lowering starts with three phase input",
                        bus.id
                    ),
                )
                .with_element_path(format!("/model/multiconductor_network/buses/{i}/terminals")),
            ),
        }
    }

    if saw_neutral {
        report
            .approximations
            .push("Kron reduction of neutral conductor before sequence transform".to_owned());
        report.diagnostics.push(StructuredDiagnostic::new(
            "LOWER.MULTI_TO_BALANCED.KRON_REDUCTION_REQUIRED",
            DiagnosticSeverity::Info,
            DiagnosticStage::Lower,
            "neutral conductors require Kron reduction before the sequence transform",
        ));
    }
}

fn check_line_terminal_maps(
    net: &MulticonductorNetwork,
    report: &mut MulticonductorToBalancedReadiness,
) {
    let neutral_terminals = global_neutral_terminals(net);
    for (i, line) in net.lines.iter().enumerate() {
        for (field, bus_id, terminal_map) in [
            (
                "terminal_map_from",
                line.bus_from.as_str(),
                line.terminal_map_from.as_slice(),
            ),
            (
                "terminal_map_to",
                line.bus_to.as_str(),
                line.terminal_map_to.as_slice(),
            ),
        ] {
            let bus = net.bus(bus_id);
            let active_count = active_terminal_count(terminal_map, bus, &neutral_terminals);
            if active_count != 3 {
                report.diagnostics.push(
                    StructuredDiagnostic::new(
                        "LOWER.MULTI_TO_BALANCED.UNSUPPORTED_CONDUCTOR_SET",
                        DiagnosticSeverity::Error,
                        DiagnosticStage::Lower,
                        format!(
                            "line {} {field} has {active_count} active terminal(s); balanced branch lowering requires three active phase conductors",
                            line.name
                        ),
                    )
                    .with_element_path(format!("/model/multiconductor_network/lines/{i}/{field}")),
                );
            }
        }
    }
}

fn check_linecodes(net: &MulticonductorNetwork, report: &mut MulticonductorToBalancedReadiness) {
    for (i, line) in net.lines.iter().enumerate() {
        let Some(code) = net.linecode(&line.linecode) else {
            report.diagnostics.push(
                StructuredDiagnostic::new(
                    "LOWER.MULTI_TO_BALANCED.UNKNOWN_LINECODE",
                    DiagnosticSeverity::Error,
                    DiagnosticStage::Lower,
                    format!(
                        "line {} references unknown linecode `{}`",
                        line.name, line.linecode
                    ),
                )
                .with_element_path(format!("/model/multiconductor_network/lines/{i}/linecode")),
            );
            continue;
        };
        if code.n_conductors != line.terminal_map_from.len()
            || code.n_conductors != line.terminal_map_to.len()
        {
            report.diagnostics.push(
                StructuredDiagnostic::new(
                    "LOWER.MULTI_TO_BALANCED.LINECODE_TERMINAL_MISMATCH",
                    DiagnosticSeverity::Error,
                    DiagnosticStage::Lower,
                    format!(
                        "line {} uses linecode {} with {} conductor(s), but its terminal maps have {} and {} terminal(s)",
                        line.name,
                        code.name,
                        code.n_conductors,
                        line.terminal_map_from.len(),
                        line.terminal_map_to.len()
                    ),
                )
                .with_element_path(format!("/model/multiconductor_network/lines/{i}/linecode")),
            );
        }
        if !square_matrix_shape(&code.r_series, code.n_conductors)
            || !square_matrix_shape(&code.x_series, code.n_conductors)
            || !square_matrix_shape(&code.g_from, code.n_conductors)
            || !square_matrix_shape(&code.b_from, code.n_conductors)
            || !square_matrix_shape(&code.g_to, code.n_conductors)
            || !square_matrix_shape(&code.b_to, code.n_conductors)
        {
            report.diagnostics.push(
                StructuredDiagnostic::new(
                    "LOWER.MULTI_TO_BALANCED.INVALID_LINECODE_MATRIX",
                    DiagnosticSeverity::Error,
                    DiagnosticStage::Lower,
                    format!(
                        "linecode {} does not carry square {} conductor matrices",
                        code.name, code.n_conductors
                    ),
                )
                .with_element_path(format!(
                    "/model/multiconductor_network/linecodes/{}",
                    code.name
                )),
            );
        }
    }
}

fn square_matrix_shape(matrix: &Mat, n: usize) -> bool {
    matrix.len() == n && matrix.iter().all(|row| row.len() == n)
}

fn check_switches(net: &MulticonductorNetwork, report: &mut MulticonductorToBalancedReadiness) {
    for (i, sw) in net.switches.iter().enumerate() {
        if sw.open {
            report.diagnostics.push(
                StructuredDiagnostic::new(
                    "LOWER.MULTI_TO_BALANCED.DROPPED_OPEN_SWITCH",
                    DiagnosticSeverity::Info,
                    DiagnosticStage::Lower,
                    format!(
                        "open switch {} is dropped by multiconductor to balanced lowering",
                        sw.name
                    ),
                )
                .with_element_path(format!("/model/multiconductor_network/switches/{i}")),
            );
        } else {
            report.diagnostics.push(
                StructuredDiagnostic::new(
                    "LOWER.MULTI_TO_BALANCED.UNSUPPORTED_CLOSED_SWITCH",
                    DiagnosticSeverity::Error,
                    DiagnosticStage::Lower,
                    format!(
                        "closed switch {} is not lowered into a zero impedance balanced branch",
                        sw.name
                    ),
                )
                .with_element_path(format!("/model/multiconductor_network/switches/{i}")),
            );
        }
    }
}

fn global_neutral_terminals(net: &MulticonductorNetwork) -> BTreeSet<String> {
    net.buses
        .iter()
        .flat_map(|bus| bus.grounded.iter().cloned())
        .collect()
}

fn active_terminal_count(
    terminals: &[String],
    bus: Option<&DistBus>,
    neutral_terminals: &BTreeSet<String>,
) -> usize {
    terminals
        .iter()
        .filter(|terminal| !is_neutral_terminal(terminal, bus, neutral_terminals))
        .count()
}

fn is_neutral_terminal(
    terminal: &str,
    bus: Option<&DistBus>,
    neutral_terminals: &BTreeSet<String>,
) -> bool {
    terminal == "0"
        || terminal.eq_ignore_ascii_case("n")
        || bus.is_some_and(|b| b.grounded.iter().any(|g| g == terminal))
        || neutral_terminals.contains(terminal)
}

fn check_phase_reference(
    net: &MulticonductorNetwork,
    report: &mut MulticonductorToBalancedReadiness,
) {
    let neutral_terminals = global_neutral_terminals(net);
    let has_three_phase_source = net.sources.iter().any(|source| {
        let bus = net.bus(&source.bus);
        active_terminal_count(&source.terminal_map, bus, &neutral_terminals) == 3
    });

    if !has_three_phase_source {
        report.diagnostics.push(StructuredDiagnostic::new(
            "LOWER.MULTI_TO_BALANCED.MISSING_PHASE_REFERENCE",
            DiagnosticSeverity::Error,
            DiagnosticStage::Lower,
            "multiconductor to balanced lowering requires a three phase voltage source reference",
        ));
    }
}

fn check_transformers(net: &MulticonductorNetwork, report: &mut MulticonductorToBalancedReadiness) {
    for (i, transformer) in net.transformers.iter().enumerate() {
        report.diagnostics.push(
            StructuredDiagnostic::new(
                "LOWER.MULTI_TO_BALANCED.UNSUPPORTED_TRANSFORMER",
                DiagnosticSeverity::Error,
                DiagnosticStage::Lower,
                format!(
                    "transformer {} is not supported by the multiconductor to balanced preflight",
                    transformer.name
                ),
            )
            .with_element_path(format!("/model/multiconductor_network/transformers/{i}")),
        );
    }
}

fn check_untyped_objects(
    net: &MulticonductorNetwork,
    report: &mut MulticonductorToBalancedReadiness,
) {
    for (i, obj) in net.untyped.iter().enumerate() {
        report.diagnostics.push(
            StructuredDiagnostic::new(
                "LOWER.MULTI_TO_BALANCED.UNSUPPORTED_OBJECT",
                DiagnosticSeverity::Error,
                DiagnosticStage::Lower,
                format!(
                    "{} {} is preserved as an untyped object and cannot be lowered",
                    obj.class, obj.name
                ),
            )
            .with_element_path(format!("/model/multiconductor_network/untyped/{i}")),
        );
    }
}
