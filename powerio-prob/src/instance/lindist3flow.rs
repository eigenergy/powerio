//! Solver-neutral LinDist3Flow OPF instance front end.
//!
//! This module owns formulation semantics that precede numerical coefficient
//! assembly: applicability, conductor-resolved topology, and the fixed
//! voltage phasor reference. Sparse matrices and affine/SOC preparation remain
//! in `powerio-matrix`.

use std::collections::{BTreeMap, BTreeSet};

use powerio_core::{Diagnostic, DiagnosticInfo, DiagnosticSeverity, Error};
use powerio_dist::{
    Configuration, DistLoadVoltageModel, LinDist3FlowPreparationAction,
    LinDist3FlowPreparationActionKind, LinDist3FlowPreparationReport, MulticonductorNetwork,
    prepare_lindist3flow_network,
};
use serde::{Deserialize, Serialize};

use super::McAcOpfInstance;
use crate::diagnostics::codes;
use crate::{MulticonductorOperatingPointQuantity, ObjectiveTerm};

/// Selection of the fixed phasors used to form LinDist3Flow coefficients.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum LinDist3FlowReferencePolicy {
    /// Use the instance initial point when present, otherwise propagate source
    /// phasors through the no-load line topology.
    #[default]
    Auto,
    /// Require the instance to carry a complete voltage initial point.
    Explicit,
    /// Ignore any initial point and propagate source phasors without line drop.
    SourcePropagated,
}

pub use powerio_dist::LinDist3FlowPreparationPolicy as LinDist3FlowUnsupported;

/// Semantic choices used while creating a LinDist3Flow instance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct LinDist3FlowBuildOptions {
    pub reference_policy: LinDist3FlowReferencePolicy,
    pub unsupported: LinDist3FlowUnsupported,
    /// Require typed neutral-Kron provenance even when no explicit neutral
    /// remains in the network.
    pub require_neutral_provenance: bool,
}

impl LinDist3FlowBuildOptions {
    #[must_use]
    pub const fn with_reference_policy(mut self, policy: LinDist3FlowReferencePolicy) -> Self {
        self.reference_policy = policy;
        self
    }

    #[must_use]
    pub const fn with_unsupported(mut self, policy: LinDist3FlowUnsupported) -> Self {
        self.unsupported = policy;
        self
    }

    #[must_use]
    pub const fn with_required_neutral_provenance(mut self, required: bool) -> Self {
        self.require_neutral_provenance = required;
        self
    }
}

/// One retained voltage node, identified independently of dense row order.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct LinDist3FlowNode {
    pub bus: String,
    pub terminal: String,
}

/// One line conductor in a deterministic source-rooted orientation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct LinDist3FlowOrientedConductor {
    pub line: String,
    pub source_line_row: usize,
    pub conductor_position: usize,
    pub parent: LinDist3FlowNode,
    pub child: LinDist3FlowNode,
    /// Whether this direction is opposite the input line's from/to order.
    pub reversed: bool,
}

/// The source-covered conductor graph used by the formulation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct LinDist3FlowTopology {
    pub nodes: Vec<LinDist3FlowNode>,
    pub conductors: Vec<LinDist3FlowOrientedConductor>,
    pub roots: Vec<LinDist3FlowNode>,
    pub islands: Vec<Vec<LinDist3FlowNode>>,
    /// Whether at least one retained conductor closes a cycle or parallels an
    /// existing conductor connection.
    pub meshed: bool,
}

/// Whether applicability checks permit instance construction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum LinDist3FlowApplicabilityStatus {
    Applicable,
    Inapplicable,
}

/// Structured findings and topology evidence from the formulation front end.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct LinDist3FlowApplicability {
    pub status: LinDist3FlowApplicabilityStatus,
    pub diagnostics: Vec<Diagnostic>,
    pub roots: Vec<LinDist3FlowNode>,
    pub islands: Vec<Vec<LinDist3FlowNode>>,
    pub reference_provenance: Option<LinDist3FlowReferenceProvenance>,
    pub kron_reduced: bool,
    pub lowered: bool,
}

impl LinDist3FlowApplicability {
    #[must_use]
    pub const fn is_applicable(&self) -> bool {
        matches!(self.status, LinDist3FlowApplicabilityStatus::Applicable)
    }
}

/// Origin of the fixed voltage reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum LinDist3FlowReferenceProvenance {
    InitialPoint,
    SourcePropagated,
}

/// One reference voltage phasor in polar SI coordinates.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct LinDist3FlowReferenceVoltage {
    pub node: LinDist3FlowNode,
    /// Volts.
    pub magnitude: f64,
    /// Radians.
    pub angle: f64,
}

/// Complete immutable phasor reference in topology node order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct LinDist3FlowReferenceState {
    pub provenance: LinDist3FlowReferenceProvenance,
    pub voltages: Vec<LinDist3FlowReferenceVoltage>,
}

impl LinDist3FlowReferenceState {
    #[must_use]
    pub fn voltage(&self, bus: &str, terminal: &str) -> Option<&LinDist3FlowReferenceVoltage> {
        self.voltages.iter().find(|voltage| {
            voltage.node.bus.eq_ignore_ascii_case(bus) && voltage.node.terminal == terminal
        })
    }
}

/// Matrix-free LinDist3Flow OPF instance.
#[derive(Clone, Debug)]
pub struct LinDist3FlowOpfInstance {
    source_base: McAcOpfInstance,
    base: McAcOpfInstance,
    topology: LinDist3FlowTopology,
    reference: LinDist3FlowReferenceState,
    applicability: LinDist3FlowApplicability,
    options: LinDist3FlowBuildOptions,
    preparation: LinDist3FlowPreparationReport,
}

impl LinDist3FlowOpfInstance {
    /// Build a LinDist3Flow instance directly from a multiconductor network.
    ///
    /// # Errors
    /// As [`McAcOpfInstance::from_network`] and [`Self::from_mc_ac`].
    pub fn from_network(
        network: MulticonductorNetwork,
        options: LinDist3FlowBuildOptions,
    ) -> Result<Self, Error> {
        Self::from_mc_ac(McAcOpfInstance::from_network(network)?, options)
    }

    /// Compile formulation semantics from a multiconductor AC OPF instance.
    ///
    /// # Errors
    /// The applicability report contains an error, or the selected voltage
    /// reference is absent, incomplete, non-finite, or zero magnitude.
    pub fn from_mc_ac(
        base: McAcOpfInstance,
        options: LinDist3FlowBuildOptions,
    ) -> Result<Self, Error> {
        let source_base = base;
        let (base, preparation) = prepare_base(&source_base, options)?;
        let (applicability, topology, reference) = assess(&base, options, &preparation);
        if let Some(diagnostic) = applicability
            .diagnostics
            .iter()
            .find(|finding| finding.severity() == DiagnosticSeverity::Error)
        {
            return Err(error_from_diagnostic(diagnostic));
        }
        let topology = topology.ok_or_else(|| {
            Error::new(
                &codes::BUILD_LINDIST3FLOW_TOPOLOGY_INVALID,
                "the applicable assessment did not produce a conductor topology",
            )
        })?;
        let reference = reference.ok_or_else(|| {
            Error::new(
                &codes::BUILD_LINDIST3FLOW_REFERENCE_INVALID,
                "the applicable assessment did not produce a coefficient reference",
            )
        })?;
        Ok(Self {
            source_base,
            base,
            topology,
            reference,
            applicability,
            options,
            preparation,
        })
    }

    /// The unmodified multiconductor OPF instance supplied by the caller.
    #[must_use]
    pub fn source_instance(&self) -> &McAcOpfInstance {
        &self.source_base
    }

    /// The unmodified distribution network supplied by the caller.
    #[must_use]
    pub fn source_network(&self) -> &MulticonductorNetwork {
        self.source_base.network()
    }

    #[must_use]
    pub fn base_instance(&self) -> &McAcOpfInstance {
        &self.base
    }

    #[must_use]
    pub fn network(&self) -> &MulticonductorNetwork {
        self.base.network()
    }

    #[must_use]
    pub const fn topology(&self) -> &LinDist3FlowTopology {
        &self.topology
    }

    #[must_use]
    pub const fn reference(&self) -> &LinDist3FlowReferenceState {
        &self.reference
    }

    #[must_use]
    pub const fn applicability(&self) -> &LinDist3FlowApplicability {
        &self.applicability
    }

    #[must_use]
    pub const fn options(&self) -> LinDist3FlowBuildOptions {
        self.options
    }

    /// Typed provenance for every component transformation and omission.
    #[must_use]
    pub const fn preparation(&self) -> &LinDist3FlowPreparationReport {
        &self.preparation
    }
}

fn prepare_base(
    source: &McAcOpfInstance,
    options: LinDist3FlowBuildOptions,
) -> Result<(McAcOpfInstance, LinDist3FlowPreparationReport), Error> {
    let prepared =
        prepare_lindist3flow_network(source.network(), options.unsupported).map_err(|error| {
            Error::new(
                &codes::BUILD_LINDIST3FLOW_PREPARATION_FAILED,
                error.to_string(),
            )
        })?;
    let (network, mut report) = prepared.into_parts();
    if report.actions.is_empty() {
        return Ok((source.clone(), report));
    }
    match source.clone().with_network(network.clone()) {
        Ok(base) => Ok((base, report)),
        Err(error) if source.initial_point().is_some() => {
            let base = McAcOpfInstance::from_network(network)?
                .with_objective(source.objective().clone())
                .with_constraints(source.constraints().clone());
            let mut action = LinDist3FlowPreparationAction::new(
                LinDist3FlowPreparationActionKind::InitialPointOmitted,
                "initial_point",
                None,
            );
            action.details.insert(
                "reason".to_owned(),
                serde_json::Value::String(error.to_string()),
            );
            report.actions.push(action);
            Ok((base, report))
        }
        Err(error) => Err(error),
    }
}

fn finding(
    info: &'static DiagnosticInfo,
    message: impl Into<String>,
    target: Option<String>,
) -> Diagnostic {
    let mut diagnostic = Diagnostic::of(info, message);
    if let Some(target) = target {
        let _ = diagnostic.set_target(target);
    }
    diagnostic
}

fn error_from_diagnostic(diagnostic: &Diagnostic) -> Error {
    Error::new(
        diagnostic
            .registered_info()
            .expect("LinDist3Flow findings use registered codes"),
        diagnostic.message(),
    )
}

fn matrix_has_nonzero(matrix: &[Vec<f64>]) -> bool {
    matrix.iter().flatten().any(|value| *value != 0.0)
}

fn check_objective(instance: &McAcOpfInstance, diagnostics: &mut Vec<Diagnostic>) {
    let network = instance.network();
    let dispatch_cost = match instance.objective().terms() {
        [] => false,
        [ObjectiveTerm::ActivePowerDispatchCost] => true,
        terms => {
            diagnostics.push(finding(
                &codes::BUILD_LINDIST3FLOW_OBJECTIVE_UNSUPPORTED,
                format!(
                    "the strict LinDist3Flow slice accepts feasibility or exactly one active-power dispatch-cost term, but received {} term(s)",
                    terms.len()
                ),
                None,
            ));
            false
        }
    };
    if !dispatch_cost {
        return;
    }
    for (row, generator) in network.generators().iter().enumerate() {
        if generator.cost.is_none() {
            diagnostics.push(finding(
                &codes::BUILD_LINDIST3FLOW_COST_MISSING,
                format!(
                    "generator `{}` has no active-power dispatch cost; its coefficient is zero",
                    generator.name
                ),
                Some(format!("/generators/{row}/cost")),
            ));
        }
    }
    for (row, source) in network.sources().iter().enumerate() {
        if source.energy_cost_rate.is_none() {
            diagnostics.push(finding(
                &codes::BUILD_LINDIST3FLOW_COST_MISSING,
                format!(
                    "voltage source `{}` has no energy cost rate; its coefficient is zero",
                    source.name
                ),
                Some(format!("/sources/{row}/energy_cost_rate")),
            ));
        }
    }
}

fn supported_connection(configuration: Configuration, terminals: usize, channels: usize) -> bool {
    match configuration {
        Configuration::Wye => terminals == channels,
        Configuration::SinglePhase => terminals == channels || (terminals == 2 && channels == 1),
        Configuration::Delta => {
            (terminals == 2 && channels == 1) || (terminals == 3 && channels == 3)
        }
        _ => false,
    }
}

fn scalar_or_channels(values: &[f64], channels: usize) -> bool {
    matches!(values.len(), 1) || values.len() == channels
}

fn finite_scalar_or_channels(values: &[f64], channels: usize) -> bool {
    scalar_or_channels(values, channels) && values.iter().all(|value| value.is_finite())
}

fn positive_scalar_or_channels(values: &[f64], channels: usize) -> bool {
    scalar_or_channels(values, channels)
        && values.iter().all(|value| value.is_finite() && *value > 0.0)
}

fn valid_bounds(lower: Option<&[f64]>, upper: Option<&[f64]>, channels: usize) -> bool {
    match (lower, upper) {
        (None, None) => true,
        (Some(lower), Some(upper)) => {
            lower.len() == channels
                && upper.len() == channels
                && lower
                    .iter()
                    .zip(upper)
                    .all(|(lower, upper)| lower.is_finite() && upper.is_finite() && lower <= upper)
        }
        _ => false,
    }
}

fn square_finite(matrix: &[Vec<f64>], dimension: usize) -> bool {
    matrix.len() == dimension
        && matrix
            .iter()
            .all(|row| row.len() == dimension && row.iter().all(|value| value.is_finite()))
}

fn device_invalid(diagnostics: &mut Vec<Diagnostic>, message: impl Into<String>, target: String) {
    diagnostics.push(finding(
        &codes::BUILD_LINDIST3FLOW_DEVICE_INVALID,
        message,
        Some(target),
    ));
}

#[allow(clippy::too_many_lines)]
fn check_device_shapes(instance: &McAcOpfInstance, diagnostics: &mut Vec<Diagnostic>) {
    let network = instance.network();
    for (row, load) in network.loads().iter().enumerate() {
        let channels = load.p_nom.len();
        let mut valid = channels != 0
            && load.q_nom.len() == channels
            && load.p_nom.iter().all(|value| value.is_finite())
            && load.q_nom.iter().all(|value| value.is_finite())
            && supported_connection(load.configuration, load.terminal_map.len(), channels);
        valid &= match &load.voltage_model {
            DistLoadVoltageModel::ConstantPower { .. }
            | DistLoadVoltageModel::ConstantCurrent { .. }
            | DistLoadVoltageModel::Exponential { .. } => true,
            DistLoadVoltageModel::ConstantImpedance { v_nom } => {
                positive_scalar_or_channels(v_nom, channels)
            }
            DistLoadVoltageModel::Zip {
                v_nom,
                alpha_z,
                alpha_i,
                alpha_p,
                beta_z,
                beta_i,
                beta_p,
            } => {
                positive_scalar_or_channels(v_nom, channels)
                    && [alpha_z, alpha_i, alpha_p, beta_z, beta_i, beta_p]
                        .into_iter()
                        .all(|values| finite_scalar_or_channels(values, channels))
            }
            _ => false,
        };
        if !valid {
            device_invalid(
                diagnostics,
                format!(
                    "load `{}` has invalid channel, connection, nominal-power, or voltage-model dimensions",
                    load.name
                ),
                format!("/loads/{row}"),
            );
        }
    }

    for (row, generator) in network.generators().iter().enumerate() {
        let channels = generator.p_nom.len();
        let limits_valid = [generator.s_max.as_deref(), generator.i_max.as_deref()]
            .into_iter()
            .flatten()
            .all(|values| {
                values.len() == channels
                    && values.iter().all(|value| value.is_finite() && *value > 0.0)
            });
        let cost_valid = generator
            .cost
            .as_deref()
            .is_none_or(|values| finite_scalar_or_channels(values, channels));
        let valid = channels != 0
            && generator.q_nom.len() == channels
            && generator.p_nom.iter().all(|value| value.is_finite())
            && generator.q_nom.iter().all(|value| value.is_finite())
            && supported_connection(
                generator.configuration,
                generator.terminal_map.len(),
                channels,
            )
            && valid_bounds(
                generator.p_min.as_deref(),
                generator.p_max.as_deref(),
                channels,
            )
            && valid_bounds(
                generator.q_min.as_deref(),
                generator.q_max.as_deref(),
                channels,
            )
            && limits_valid
            && cost_valid;
        if !valid {
            device_invalid(
                diagnostics,
                format!(
                    "generator `{}` has invalid channel, connection, paired-bound, rating, or cost dimensions",
                    generator.name
                ),
                format!("/generators/{row}"),
            );
        }
    }

    for (row, shunt) in network.shunts().iter().enumerate() {
        let terminals = shunt.terminal_map.len();
        if terminals == 0
            || !square_finite(&shunt.g, terminals)
            || !square_finite(&shunt.b, terminals)
        {
            device_invalid(
                diagnostics,
                format!(
                    "shunt `{}` admittance matrices are not finite {terminals}x{terminals} arrays",
                    shunt.name
                ),
                format!("/shunts/{row}"),
            );
        }
    }

    for (row, source) in network.sources().iter().enumerate() {
        let channels = source.terminal_map.len();
        let valid = channels != 0
            && source.v_magnitude.len() == channels
            && source.v_angle.len() == channels
            && source
                .v_magnitude
                .iter()
                .all(|value| value.is_finite() && *value > 0.0)
            && source.v_angle.iter().all(|value| value.is_finite())
            && source
                .energy_cost_rate
                .as_deref()
                .is_none_or(|values| finite_scalar_or_channels(values, channels));
        if !valid {
            device_invalid(
                diagnostics,
                format!(
                    "voltage source `{}` has invalid terminal, phasor, or cost dimensions",
                    source.name
                ),
                format!("/sources/{row}"),
            );
        }
    }
}

fn check_supported_slice(
    instance: &McAcOpfInstance,
    options: LinDist3FlowBuildOptions,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let network = instance.network();
    check_objective(instance, diagnostics);
    check_device_shapes(instance, diagnostics);
    let conventions = network.extras().get("bmopf_terminal_conventions");
    for (row, bus) in network.buses().iter().enumerate() {
        if bus.phase_indices(conventions).len() != bus.terminals.len() {
            diagnostics.push(finding(
                &codes::BUILD_LINDIST3FLOW_EXPLICIT_NEUTRAL,
                format!(
                    "bus `{}` still declares an explicit neutral conductor; apply neutral_kron_reduce first",
                    bus.id
                ),
                Some(format!("/buses/{row}/terminals")),
            ));
        }
    }
    if options.require_neutral_provenance && !network.extras().contains_key("powerio_neutral_kron")
    {
        diagnostics.push(finding(
            &codes::BUILD_LINDIST3FLOW_EXPLICIT_NEUTRAL,
            "the instance requires neutral-Kron provenance but the network carries none",
            None,
        ));
    }

    for (row, line) in network.lines().iter().enumerate() {
        let Some(code) = network.linecode(&line.linecode) else {
            continue;
        };
        if matrix_has_nonzero(&code.g_from)
            || matrix_has_nonzero(&code.b_from)
            || matrix_has_nonzero(&code.g_to)
            || matrix_has_nonzero(&code.b_to)
        {
            diagnostics.push(finding(
                &codes::BUILD_LINDIST3FLOW_UNSUPPORTED_COMPONENT,
                format!(
                    "line `{}` has pi shunt admittance; endpoint-shunt lowering is not implemented yet",
                    line.name
                ),
                Some(format!("/lines/{row}/linecode")),
            ));
        }
    }
    for (row, load) in network.loads().iter().enumerate() {
        let supported = match &load.voltage_model {
            DistLoadVoltageModel::ConstantPower { .. }
            | DistLoadVoltageModel::ConstantImpedance { .. } => true,
            DistLoadVoltageModel::Zip {
                alpha_i, beta_i, ..
            } => {
                alpha_i.iter().all(|value| value.abs() <= f64::EPSILON)
                    && beta_i.iter().all(|value| value.abs() <= f64::EPSILON)
            }
            DistLoadVoltageModel::ConstantCurrent { .. }
            | DistLoadVoltageModel::Exponential { .. }
            | _ => false,
        };
        if !supported {
            diagnostics.push(finding(
                &codes::BUILD_LINDIST3FLOW_UNSUPPORTED_COMPONENT,
                format!(
                    "load `{}` needs a current/exponential approximation outside the strict model",
                    load.name
                ),
                Some(format!("/loads/{row}/voltage_model")),
            ));
        }
    }

    for (family, count) in [
        ("switch", network.switches().len()),
        ("transformer", network.transformers().len()),
        ("capacitor", network.capacitors().len()),
        ("IBR", network.ibrs().len()),
        ("untyped object", network.untyped_objects().len()),
    ] {
        if count != 0 {
            diagnostics.push(finding(
                &codes::BUILD_LINDIST3FLOW_UNSUPPORTED_COMPONENT,
                format!(
                    "the initial strict LinDist3Flow slice does not yet compile {count} {family} record(s)"
                ),
                None,
            ));
        }
    }
}

/// Check the current strict LinDist3Flow boundary without constructing an
/// instance. Findings are stable diagnostics; callers can display all of them
/// before deciding whether to transform the network.
#[must_use]
pub fn check_lindist3flow_applicability(
    instance: &McAcOpfInstance,
    options: LinDist3FlowBuildOptions,
) -> LinDist3FlowApplicability {
    match prepare_base(instance, options) {
        Ok((prepared, report)) => assess(&prepared, options, &report).0,
        Err(error) => LinDist3FlowApplicability {
            status: LinDist3FlowApplicabilityStatus::Inapplicable,
            diagnostics: error.into_diagnostics(),
            roots: Vec::new(),
            islands: Vec::new(),
            reference_provenance: None,
            kron_reduced: false,
            lowered: false,
        },
    }
}

fn preparation_findings(report: &LinDist3FlowPreparationReport) -> Vec<Diagnostic> {
    report
        .actions
        .iter()
        .map(|action| {
            let info = match action.kind {
                LinDist3FlowPreparationActionKind::LoadApproximated
                | LinDist3FlowPreparationActionKind::IbrApproximated
                | LinDist3FlowPreparationActionKind::StaticControlFrozen => {
                    &codes::BUILD_LINDIST3FLOW_COMPONENT_APPROXIMATED
                }
                LinDist3FlowPreparationActionKind::UntypedObjectOmitted
                | LinDist3FlowPreparationActionKind::InitialPointOmitted => {
                    &codes::BUILD_LINDIST3FLOW_COMPONENT_OMITTED
                }
                _ => &codes::BUILD_LINDIST3FLOW_COMPONENT_LOWERED,
            };
            finding(
                info,
                format!(
                    "`{}` was prepared by {:?}{}",
                    action.source,
                    action.kind,
                    action
                        .target
                        .as_ref()
                        .map_or_else(String::new, |target| format!(" as `{target}`"))
                ),
                Some(action.source.clone()),
            )
        })
        .collect()
}

fn assess(
    instance: &McAcOpfInstance,
    options: LinDist3FlowBuildOptions,
    preparation: &LinDist3FlowPreparationReport,
) -> (
    LinDist3FlowApplicability,
    Option<LinDist3FlowTopology>,
    Option<LinDist3FlowReferenceState>,
) {
    let mut diagnostics = preparation_findings(preparation);
    check_supported_slice(instance, options, &mut diagnostics);
    let topology = match build_topology(instance.network()) {
        Ok(topology) => {
            if topology.meshed {
                diagnostics.push(finding(
                    &codes::BUILD_LINDIST3FLOW_MESH_APPROXIMATION,
                    "the retained conductor graph contains a cycle or parallel connection; the model keeps every line but adds no angle, loop-consistency, circulating-flow, or radialisation constraint",
                    None,
                ));
            }
            Some(topology)
        }
        Err(message) => {
            diagnostics.push(finding(
                &codes::BUILD_LINDIST3FLOW_TOPOLOGY_INVALID,
                message,
                None,
            ));
            None
        }
    };
    let reference = topology.as_ref().and_then(|topology| {
        match build_reference(instance, topology, options.reference_policy) {
            Ok(reference) => Some(reference),
            Err(error) => {
                diagnostics.extend(error.into_diagnostics());
                None
            }
        }
    });
    let status = if diagnostics
        .iter()
        .any(|finding| finding.severity() == DiagnosticSeverity::Error)
    {
        LinDist3FlowApplicabilityStatus::Inapplicable
    } else {
        LinDist3FlowApplicabilityStatus::Applicable
    };
    let roots = topology
        .as_ref()
        .map_or_else(Vec::new, |topology| topology.roots.clone());
    let islands = topology
        .as_ref()
        .map_or_else(Vec::new, |topology| topology.islands.clone());
    (
        LinDist3FlowApplicability {
            status,
            diagnostics,
            roots,
            islands,
            reference_provenance: reference.as_ref().map(|reference| reference.provenance),
            kron_reduced: instance
                .network()
                .extras()
                .contains_key("powerio_neutral_kron"),
            lowered: !preparation.actions.is_empty(),
        },
        topology,
        reference,
    )
}

#[derive(Clone, Copy)]
struct Edge {
    from: usize,
    to: usize,
    line: usize,
    conductor: usize,
}

struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
        }
    }

    fn find(&mut self, node: usize) -> usize {
        let mut root = node;
        while self.parent[root] != root {
            root = self.parent[root];
        }
        let mut cursor = node;
        while self.parent[cursor] != root {
            let next = self.parent[cursor];
            self.parent[cursor] = root;
            cursor = next;
        }
        root
    }

    fn join(&mut self, left: usize, right: usize) -> bool {
        let left = self.find(left);
        let right = self.find(right);
        if left == right {
            return false;
        }
        self.parent[left.max(right)] = left.min(right);
        true
    }
}

fn node_key(bus: &str, terminal: &str) -> (String, String) {
    (bus.to_ascii_lowercase(), terminal.to_owned())
}

#[allow(clippy::too_many_lines)]
fn build_topology(network: &MulticonductorNetwork) -> Result<LinDist3FlowTopology, String> {
    let mut nodes = Vec::new();
    let mut positions = BTreeMap::new();
    let mut bus_ids = BTreeSet::new();
    let mut bus_positions = BTreeMap::new();
    for bus in network.buses() {
        if !bus_ids.insert(bus.id.to_ascii_lowercase()) {
            return Err(format!(
                "bus identity `{}` is duplicated case-insensitively",
                bus.id
            ));
        }
        bus_positions.insert(bus.id.to_ascii_lowercase(), bus_positions.len());
        let mut terminals = BTreeSet::new();
        for terminal in &bus.terminals {
            if !terminals.insert(terminal.clone()) {
                return Err(format!("bus `{}` repeats terminal `{terminal}`", bus.id));
            }
            let node = LinDist3FlowNode {
                bus: bus.id.clone(),
                terminal: terminal.clone(),
            };
            positions.insert(node_key(&node.bus, &node.terminal), nodes.len());
            nodes.push(node);
        }
    }
    if nodes.is_empty() {
        return Err("the network has no retained bus terminals".to_owned());
    }

    let mut components = UnionFind::new(nodes.len());
    let mut bus_components = UnionFind::new(bus_positions.len());
    let mut edges = Vec::new();
    let mut meshed = false;
    for (line_row, line) in network.lines().iter().enumerate() {
        if line.terminal_map_from.len() != line.terminal_map_to.len()
            || line.terminal_map_from.is_empty()
        {
            return Err(format!(
                "line `{}` has empty or unequal terminal maps",
                line.name
            ));
        }
        let from_bus = *bus_positions
            .get(&line.bus_from.to_ascii_lowercase())
            .ok_or_else(|| {
                format!(
                    "line `{}` names unknown from bus `{}`",
                    line.name, line.bus_from
                )
            })?;
        let to_bus = *bus_positions
            .get(&line.bus_to.to_ascii_lowercase())
            .ok_or_else(|| {
                format!(
                    "line `{}` names unknown to bus `{}`",
                    line.name, line.bus_to
                )
            })?;
        meshed |= !bus_components.join(from_bus, to_bus);
        for (conductor, (from_terminal, to_terminal)) in line
            .terminal_map_from
            .iter()
            .zip(&line.terminal_map_to)
            .enumerate()
        {
            let from = *positions
                .get(&node_key(&line.bus_from, from_terminal))
                .ok_or_else(|| {
                    format!(
                        "line `{}` names undeclared from terminal `{}/{from_terminal}`",
                        line.name, line.bus_from
                    )
                })?;
            let to = *positions
                .get(&node_key(&line.bus_to, to_terminal))
                .ok_or_else(|| {
                    format!(
                        "line `{}` names undeclared to terminal `{}/{to_terminal}`",
                        line.name, line.bus_to
                    )
                })?;
            meshed |= !components.join(from, to);
            edges.push(Edge {
                from,
                to,
                line: line_row,
                conductor,
            });
        }
    }

    let mut island_indices: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for node in 0..nodes.len() {
        island_indices
            .entry(components.find(node))
            .or_default()
            .push(node);
    }
    let mut source_nodes: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let mut island_sources: BTreeMap<usize, Vec<&str>> = BTreeMap::new();
    let mut source_bus_roots = Vec::new();
    for source in network.sources() {
        let bus = *bus_positions
            .get(&source.bus.to_ascii_lowercase())
            .ok_or_else(|| {
                format!(
                    "voltage source `{}` names unknown bus `{}`",
                    source.name, source.bus
                )
            })?;
        island_sources
            .entry(bus_components.find(bus))
            .or_default()
            .push(&source.name);
        source_bus_roots.push(bus);
        for terminal in &source.terminal_map {
            let node = *positions
                .get(&node_key(&source.bus, terminal))
                .ok_or_else(|| {
                    format!(
                        "voltage source `{}` names undeclared terminal `{}/{terminal}`",
                        source.name, source.bus
                    )
                })?;
            source_nodes
                .entry(components.find(node))
                .or_default()
                .push(node);
        }
    }
    let mut physical_islands: BTreeMap<usize, Vec<&str>> = BTreeMap::new();
    for bus in network.buses() {
        let position = bus_positions[&bus.id.to_ascii_lowercase()];
        physical_islands
            .entry(bus_components.find(position))
            .or_default()
            .push(&bus.id);
    }
    for (component, buses) in physical_islands {
        let sources: &[&str] = island_sources.get(&component).map_or(&[], Vec::as_slice);
        if sources.len() != 1 {
            return Err(format!(
                "physical island containing bus `{}` has {} voltage source records; exactly one is required",
                buses[0],
                sources.len()
            ));
        }
    }

    let mut ordered_islands = island_indices.into_values().collect::<Vec<_>>();
    ordered_islands.sort_by_key(|island| island[0]);
    let mut root_indices = Vec::with_capacity(ordered_islands.len());
    for island in &ordered_islands {
        let component = components.find(island[0]);
        let roots: &[usize] = source_nodes.get(&component).map_or(&[], Vec::as_slice);
        if roots.len() != 1 {
            return Err(format!(
                "conductor island rooted at `{}/{}` contains {} fixed-voltage source terminals; exactly one is required",
                nodes[island[0]].bus,
                nodes[island[0]].terminal,
                roots.len()
            ));
        }
        root_indices.push(roots[0]);
    }

    let mut bus_adjacency = vec![Vec::new(); bus_positions.len()];
    for line in network.lines() {
        let from = bus_positions[&line.bus_from.to_ascii_lowercase()];
        let to = bus_positions[&line.bus_to.to_ascii_lowercase()];
        bus_adjacency[from].push(to);
        bus_adjacency[to].push(from);
    }
    let mut bus_distances = vec![usize::MAX; bus_positions.len()];
    let mut frontier = std::collections::VecDeque::new();
    for root in source_bus_roots {
        bus_distances[root] = 0;
        frontier.push_back(root);
    }
    while let Some(parent) = frontier.pop_front() {
        for &child in &bus_adjacency[parent] {
            if bus_distances[child] == usize::MAX {
                bus_distances[child] = bus_distances[parent] + 1;
                frontier.push_back(child);
            }
        }
    }
    if let Some(bus) = bus_distances
        .iter()
        .position(|distance| *distance == usize::MAX)
    {
        let id = network
            .buses()
            .iter()
            .find(|candidate| bus_positions[&candidate.id.to_ascii_lowercase()] == bus)
            .map_or("<unknown>", |candidate| candidate.id.as_str());
        return Err(format!(
            "bus `{id}` was not reached from its physical-island source"
        ));
    }
    let directions = edges
        .iter()
        .map(|edge| {
            let line = &network.lines()[edge.line];
            let from_bus = bus_positions[&line.bus_from.to_ascii_lowercase()];
            let to_bus = bus_positions[&line.bus_to.to_ascii_lowercase()];
            match bus_distances[from_bus].cmp(&bus_distances[to_bus]) {
                std::cmp::Ordering::Less => (edge.from, edge.to),
                std::cmp::Ordering::Greater => (edge.to, edge.from),
                std::cmp::Ordering::Equal => {
                    let from = line.bus_from.to_ascii_lowercase();
                    let to = line.bus_to.to_ascii_lowercase();
                    if from <= to {
                        (edge.from, edge.to)
                    } else {
                        (edge.to, edge.from)
                    }
                }
            }
        })
        .collect::<Vec<_>>();
    let conductors = edges
        .iter()
        .zip(directions)
        .map(|(edge, (parent, child))| LinDist3FlowOrientedConductor {
            line: network.lines()[edge.line].name.clone(),
            source_line_row: edge.line,
            conductor_position: edge.conductor,
            parent: nodes[parent].clone(),
            child: nodes[child].clone(),
            reversed: parent != edge.from,
        })
        .collect();
    Ok(LinDist3FlowTopology {
        roots: root_indices
            .iter()
            .map(|&root| nodes[root].clone())
            .collect(),
        islands: ordered_islands
            .iter()
            .map(|island| island.iter().map(|&node| nodes[node].clone()).collect())
            .collect(),
        nodes,
        conductors,
        meshed,
    })
}

fn valid_phasor(magnitude: f64, angle: f64) -> bool {
    magnitude.is_finite() && magnitude > 0.0 && angle.is_finite()
}

fn invalid_reference(message: impl Into<String>) -> Error {
    Error::new(&codes::BUILD_LINDIST3FLOW_REFERENCE_INVALID, message)
}

fn explicit_reference(
    instance: &McAcOpfInstance,
    topology: &LinDist3FlowTopology,
) -> Result<LinDist3FlowReferenceState, Error> {
    let point = instance.initial_point().ok_or_else(|| {
        invalid_reference("the explicit reference policy requires a voltage initial point")
    })?;
    if point
        .values(MulticonductorOperatingPointQuantity::TerminalVoltageMagnitude)
        .is_none()
        || point
            .values(MulticonductorOperatingPointQuantity::TerminalVoltageAngle)
            .is_none()
    {
        return Err(invalid_reference(
            "the initial point must contain both terminal voltage magnitude and angle columns",
        ));
    }
    let mut voltages = Vec::with_capacity(topology.nodes.len());
    for node in &topology.nodes {
        let magnitude = point
            .terminal_voltage_magnitude(&node.bus, &node.terminal)
            .ok_or_else(|| {
                invalid_reference(format!(
                    "the initial point has no voltage magnitude for `{}/{}`",
                    node.bus, node.terminal
                ))
            })?;
        let angle = point
            .terminal_voltage_angle(&node.bus, &node.terminal)
            .ok_or_else(|| {
                invalid_reference(format!(
                    "the initial point has no voltage angle for `{}/{}`",
                    node.bus, node.terminal
                ))
            })?;
        if !valid_phasor(magnitude, angle) {
            return Err(invalid_reference(format!(
                "the initial point voltage for `{}/{}` is zero or non-finite",
                node.bus, node.terminal
            )));
        }
        voltages.push(LinDist3FlowReferenceVoltage {
            node: node.clone(),
            magnitude,
            angle,
        });
    }
    Ok(LinDist3FlowReferenceState {
        provenance: LinDist3FlowReferenceProvenance::InitialPoint,
        voltages,
    })
}

#[allow(clippy::too_many_lines)]
fn propagated_reference(
    instance: &McAcOpfInstance,
    topology: &LinDist3FlowTopology,
) -> Result<LinDist3FlowReferenceState, Error> {
    let mut positions = BTreeMap::new();
    for (position, node) in topology.nodes.iter().enumerate() {
        positions.insert(node_key(&node.bus, &node.terminal), position);
    }
    let mut values = vec![None; topology.nodes.len()];
    for source in instance.network().sources() {
        if source.v_magnitude.len() != source.terminal_map.len()
            || source.v_angle.len() != source.terminal_map.len()
        {
            return Err(invalid_reference(format!(
                "voltage source `{}` values do not align with its terminal map",
                source.name
            )));
        }
        for (terminal_position, terminal) in source.terminal_map.iter().enumerate() {
            let node = *positions
                .get(&node_key(&source.bus, terminal))
                .ok_or_else(|| {
                    invalid_reference(format!(
                        "voltage source `{}` names unknown terminal `{}/{terminal}`",
                        source.name, source.bus
                    ))
                })?;
            let magnitude = source.v_magnitude[terminal_position];
            let angle = source.v_angle[terminal_position];
            if !valid_phasor(magnitude, angle) {
                return Err(invalid_reference(format!(
                    "voltage source `{}` terminal `{terminal}` is zero or non-finite",
                    source.name
                )));
            }
            values[node] = Some((magnitude, angle));
        }
    }

    let mut adjacency = vec![Vec::new(); topology.nodes.len()];
    for conductor in &topology.conductors {
        let parent = positions[&node_key(&conductor.parent.bus, &conductor.parent.terminal)];
        let child = positions[&node_key(&conductor.child.bus, &conductor.child.terminal)];
        adjacency[parent].push(child);
        adjacency[child].push(parent);
    }
    for root in &topology.roots {
        let root = positions[&node_key(&root.bus, &root.terminal)];
        let mut stack = vec![root];
        let mut visited = vec![false; topology.nodes.len()];
        visited[root] = true;
        while let Some(node) = stack.pop() {
            let value = values[node].ok_or_else(|| {
                invalid_reference(format!(
                    "source root `{}/{}` has no phasor",
                    topology.nodes[root].bus, topology.nodes[root].terminal
                ))
            })?;
            for &neighbor in &adjacency[node] {
                if !visited[neighbor] {
                    visited[neighbor] = true;
                    values[neighbor] = Some(value);
                    stack.push(neighbor);
                }
            }
        }
    }
    let voltages = topology
        .nodes
        .iter()
        .cloned()
        .zip(values)
        .map(|(node, value)| {
            let (magnitude, angle) = value.ok_or_else(|| {
                invalid_reference(format!(
                    "node `{}/{}` was not reached from a source",
                    node.bus, node.terminal
                ))
            })?;
            Ok(LinDist3FlowReferenceVoltage {
                node,
                magnitude,
                angle,
            })
        })
        .collect::<Result<Vec<_>, Error>>()?;
    Ok(LinDist3FlowReferenceState {
        provenance: LinDist3FlowReferenceProvenance::SourcePropagated,
        voltages,
    })
}

fn build_reference(
    instance: &McAcOpfInstance,
    topology: &LinDist3FlowTopology,
    policy: LinDist3FlowReferencePolicy,
) -> Result<LinDist3FlowReferenceState, Error> {
    match policy {
        LinDist3FlowReferencePolicy::Auto if instance.initial_point().is_some() => {
            explicit_reference(instance, topology)
        }
        LinDist3FlowReferencePolicy::Explicit => explicit_reference(instance, topology),
        LinDist3FlowReferencePolicy::Auto | LinDist3FlowReferencePolicy::SourcePropagated => {
            propagated_reference(instance, topology)
        }
    }
}
