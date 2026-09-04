//! Explicit transformations and their preflight checks.
//!
//! A pass that changes one model into another carries the module records
//! forward and appends transformation history, so the result is auditable.
//! Emission borrows the module and returns emission diagnostics separately. The
//! most consequential transformation, multiconductor to balanced, is explicit
//! and diagnosed, never a silent positive sequence projection.

use std::collections::{BTreeMap, BTreeSet};
use std::f64::consts::PI;

use num_complex::Complex64;
use serde::{Deserialize, Serialize};

use crate::{
    BalancedNetwork, Branch, BranchCharging, Bus, BusId, BusType, Extras as BalancedExtras,
    Generator, GeoApplyReport, GeoLayer, Load, Shunt, SourceFormat,
};
use powerio_core::{Diagnostic, DiagnosticSeverity, HistoryEntry, HistoryId, HistoryKind};
use powerio_dist::{
    ConductorMatrix, DistBus, DistLine, DistLineCode, DistLoadVoltageModel, MulticonductorNetwork,
};

use crate::codes;

trait DiagnosticTargetExt {
    fn with_value_target(self, target: String) -> Self;
}

impl DiagnosticTargetExt for Diagnostic {
    fn with_value_target(self, target: String) -> Self {
        self.with_target(target)
            .expect("transform targets are bounded RFC 6901 pointers")
    }
}

/// Records accumulated while the transformation is being built. The public
/// result exposes the current diagnostic and history records instead of this
/// implementation detail.
#[derive(Clone, Debug)]
struct TransformRecords {
    options: serde_json::Map<String, serde_json::Value>,
    assumptions: Vec<String>,
    approximations: Vec<String>,
    dropped_fields: Vec<String>,
    diagnostics: Vec<Diagnostic>,
}

impl TransformRecords {
    fn new(options: MulticonductorToBalancedOptions) -> Self {
        Self {
            options: options_map(options),
            assumptions: Vec::new(),
            approximations: Vec::new(),
            dropped_fields: Vec::new(),
            diagnostics: Vec::new(),
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

/// Report for the multiconductor to balanced transformation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MulticonductorToBalancedReport {
    pub convention: SequenceTransformConvention,
    pub base_mva: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assumptions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approximations: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
}

impl MulticonductorToBalancedReport {
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.diagnostics
            .iter()
            .all(|diagnostic| diagnostic.severity() < DiagnosticSeverity::Error)
    }

    /// The greatest severity among this report's findings, or `None` for a
    /// clean report.
    #[must_use]
    pub fn dominant_severity(&self) -> Option<DiagnosticSeverity> {
        self.diagnostics
            .iter()
            .map(powerio_core::Diagnostic::severity)
            .max()
    }
}

/// A successful raw multiconductor to balanced lowering result.
#[derive(Clone, Debug)]
pub struct MulticonductorToBalancedTransformation {
    pub network: BalancedNetwork,
    /// Findings produced by the transformation itself.
    pub diagnostics: Vec<Diagnostic>,
    /// The current history record for this transformation. A module lowering
    /// remints its ID when the default ID is already present.
    pub history: HistoryEntry,
    /// Buses removed by closed switch merges: removed bus ID to the kept
    /// bus ID, in the source's spelling.
    pub merged_buses: BTreeMap<String, String>,
    /// Closed switches whose merge removed them from the balanced model.
    pub removed_switches: Vec<String>,
}

/// Structured failure from the raw multiconductor to balanced lowering pass.
///
/// `diagnostics` are current module records. Their targets use the
/// multiconductor value's pointer grammar (for example `/sources/0/bus`)
/// because a refusal leaves that value unchanged.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MulticonductorToBalancedError {
    pub options: MulticonductorToBalancedOptions,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
}

impl MulticonductorToBalancedError {
    pub fn new(options: MulticonductorToBalancedOptions, diagnostics: &[Diagnostic]) -> Self {
        Self {
            options,
            diagnostics: diagnostics.to_vec(),
        }
    }
}

impl std::fmt::Display for MulticonductorToBalancedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.diagnostics.first() {
            Some(diagnostic) => write!(f, "{}", diagnostic.message()),
            None => f.write_str("multiconductor to balanced transformation failed"),
        }
    }
}

impl std::error::Error for MulticonductorToBalancedError {}

/// Check whether a multiconductor network is ready for the lowering pass.
///
/// This is a preflight only: it reports the assumptions and blockers that the
/// lowering would need to account for, but it does not produce a balanced model
/// and does not append to `history`.
#[must_use]
pub fn to_balanced_network_report(
    net: &MulticonductorNetwork,
    options: MulticonductorToBalancedOptions,
) -> MulticonductorToBalancedReport {
    let mut report = MulticonductorToBalancedReport {
        convention: options.convention,
        base_mva: options.base_mva,
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
    report
}

/// Lower a transparent three phase multiconductor network to a balanced model.
///
/// The pass is explicit. It does not run from parsers, emitters, matrix builders,
/// bindings, or PowerIO IR deserialization. Unsupported inputs return structured
/// `TRANSFORM.MULTI_TO_BALANCED.*` diagnostics in [`MulticonductorToBalancedError`].
pub fn to_balanced_network(
    net: &MulticonductorNetwork,
    options: MulticonductorToBalancedOptions,
) -> Result<MulticonductorToBalancedTransformation, MulticonductorToBalancedError> {
    let readiness = to_balanced_network_report(net, options);
    if !readiness.is_ready() {
        return Err(MulticonductorToBalancedError::new(
            options,
            &readiness.diagnostics,
        ));
    }

    let mut state = LoweringState::new(net, options, readiness);
    state.lower()
}

/// Readiness of one module's value for the balanced lowering: the #398
/// inspect operation. The value must be a multiconductor network.
///
/// # Errors
/// A value of any other kind, named.
pub fn to_balanced_report(
    module: &powerio_core::PioModule<crate::PioValue>,
    options: MulticonductorToBalancedOptions,
) -> Result<MulticonductorToBalancedReport, powerio_core::Error> {
    let crate::PioValue::MulticonductorNetwork(net) = &module.value() else {
        return Err(wrong_kind_error(module.value()));
    };
    Ok(to_balanced_network_report(net, options))
}

/// Lower a multiconductor module to a balanced module: the #398 transform
/// operation. The module's common records carry over, the retained source is
/// severed because its bytes describe the input value, and the pass appends
/// its structured findings as module diagnostics and one Transform history
/// entry stating the chosen base power, every assumption and approximation,
/// the dropped fields, and the removed bus and switch identities.
///
/// # Errors
/// A value of any other kind (the module comes back untouched), or the
/// lowering's structured refusal.
///
/// # Panics
/// Only on a broken internal invariant: the pass's diagnostics carry no
/// identity and no span, the note lists are capped, and the history id is
/// minted unused, so every record append succeeds.
#[allow(clippy::result_large_err)]
pub fn to_balanced(
    module: powerio_core::PioModule<crate::PioValue>,
    options: MulticonductorToBalancedOptions,
) -> Result<
    powerio_core::PioModule<crate::PioValue>,
    (
        powerio_core::PioModule<crate::PioValue>,
        Box<MulticonductorToBalancedError>,
    ),
> {
    let crate::PioValue::MulticonductorNetwork(net) = &module.value() else {
        let error = MulticonductorToBalancedError::new(
            options,
            &[Diagnostic::of(
                &codes::TRANSFORM_MULTI_TO_BALANCED_WRONG_MODEL_KIND,
                format!(
                    "the module carries a {} value; the balanced lowering takes a \
                     multiconductor network",
                    module.value().type_name()
                ),
            )],
        );
        return Err((module, Box::new(error)));
    };
    let lowering = match to_balanced_network(net, options) {
        Ok(lowering) => lowering,
        Err(error) => return Err((module, Box::new(error))),
    };
    let MulticonductorToBalancedTransformation {
        network,
        diagnostics,
        history,
        ..
    } = lowering;
    // Room for the pass's own records is checked against the module maxima
    // before the value is consumed, so the additions below hold by
    // construction and a cap-edge input is refused with its module intact.
    let diagnostics_room =
        powerio_core::limits::MAX_MODULE_DIAGNOSTICS.saturating_sub(module.diagnostics.len());
    let history_room =
        powerio_core::limits::MAX_MODULE_HISTORY_ENTRIES.saturating_sub(module.history().len());
    if diagnostics.len() > diagnostics_room || history_room == 0 {
        let error = MulticonductorToBalancedError::new(
            options,
            &[Diagnostic::of(
                &codes::TRANSFORM_MULTI_TO_BALANCED_RECORD_CAP,
                "the module cannot hold the lowering's findings and history entry; export a \
                 fresh module before lowering"
                    .to_string(),
            )],
        );
        return Err((module, Box::new(error)));
    }
    let mut module = module
        .map_value(|_| crate::PioValue::BalancedNetwork(network))
        .sever_source();
    // The value's kind changed, so no RFC 6901 target survives the
    // transform: pre-existing diagnostic targets and the source map pointed
    // into the consumed multiconductor value and are severed here; the
    // pass's own findings are emitted with no target for the same reason.
    module.sever_value_targets();
    for diagnostic in &diagnostics {
        let mut diagnostic = diagnostic.clone();
        diagnostic.clear_target();
        module
            .add_diagnostic(diagnostic)
            .expect("room was checked; pass diagnostics carry no identity and no span");
    }
    let entry = copy_history_with_id(
        &history,
        unused_history_id(&module, "multiconductor-to-balanced"),
    );
    module
        .add_history_entry(entry)
        .expect("room was checked and the history id is unique by construction");
    Ok(module)
}

fn derive_balanced_calculation<I>(
    module: &powerio_core::PioModule<crate::PioValue>,
    operation: &'static str,
    output_type: &'static str,
    build: impl FnOnce(BalancedNetwork) -> Result<I, powerio_core::Error>,
) -> Result<powerio_core::PioModule<I>, powerio_core::Error> {
    if !matches!(module.value(), crate::PioValue::BalancedNetwork(_)) {
        return Err(powerio_core::Error::new(
            &codes::REQUEST_MODULE_WRONG_MODEL_KIND,
            format!(
                "{operation} requires powerio.BalancedNetwork; the module contains {}",
                module.value().type_name()
            ),
        ));
    }
    let history = HistoryEntry::new(
        unused_history_id(module, operation),
        HistoryKind::Transform,
        operation,
    )?
    .with_input_type("powerio.BalancedNetwork")?
    .with_output_type(output_type)?;
    let producer = powerio_core::Producer::new("powerio", crate::VERSION)?;
    module.clone().try_derive_value(producer, history, |value| {
        let crate::PioValue::BalancedNetwork(network) = value else {
            unreachable!("the value type was checked before derivation")
        };
        build(network)
    })
}

fn derive_multiconductor_calculation<I>(
    module: &powerio_core::PioModule<crate::PioValue>,
    operation: &'static str,
    output_type: &'static str,
    build: impl FnOnce(MulticonductorNetwork) -> Result<I, powerio_core::Error>,
) -> Result<powerio_core::PioModule<I>, powerio_core::Error> {
    if !matches!(module.value(), crate::PioValue::MulticonductorNetwork(_)) {
        return Err(powerio_core::Error::new(
            &codes::REQUEST_MODULE_WRONG_MODEL_KIND,
            format!(
                "{operation} requires powerio.MulticonductorNetwork; the module contains {}",
                module.value().type_name()
            ),
        ));
    }
    let history = HistoryEntry::new(
        unused_history_id(module, operation),
        HistoryKind::Transform,
        operation,
    )?
    .with_input_type("powerio.MulticonductorNetwork")?
    .with_output_type(output_type)?;
    let producer = powerio_core::Producer::new("powerio", crate::VERSION)?;
    module.clone().try_derive_value(producer, history, |value| {
        let crate::PioValue::MulticonductorNetwork(network) = value else {
            unreachable!("the value type was checked before derivation")
        };
        build(network)
    })
}

/// Apply one geographic layer to a network module.
///
/// Balanced bus points and branch routes use
/// [`BalancedNetwork::apply_geo_layer`]. Multiconductor coordinates use the
/// same shared matching rules through [`crate::dist_geo::apply_dist_geo_layer`].
/// The source module is unchanged. The returned module clears retained bytes
/// and source mappings, preserves its other records, and appends one
/// `apply_geo_layer` history entry.
///
/// # Errors
/// The module does not contain a balanced or multiconductor network, or its
/// records cannot accept the new history entry.
pub fn apply_geo_layer(
    module: &powerio_core::PioModule<crate::PioValue>,
    layer: &GeoLayer,
) -> Result<(powerio_core::PioModule<crate::PioValue>, GeoApplyReport), powerio_core::Error> {
    let type_name = match &module.value() {
        crate::PioValue::BalancedNetwork(_) => "powerio.BalancedNetwork",
        crate::PioValue::MulticonductorNetwork(_) => "powerio.MulticonductorNetwork",
        value => {
            return Err(powerio_core::Error::new(
                &codes::REQUEST_MODULE_WRONG_MODEL_KIND,
                format!(
                    "apply_geo_layer requires powerio.BalancedNetwork or \
                     powerio.MulticonductorNetwork; the module contains {}",
                    value.type_name()
                ),
            ));
        }
    };
    let history = HistoryEntry::new(
        unused_history_id(module, "apply-geo-layer"),
        HistoryKind::Transform,
        "apply_geo_layer",
    )?
    .with_input_type(type_name)?
    .with_output_type(type_name)?;
    let producer = powerio_core::Producer::new("powerio", crate::VERSION)?;
    let mut report = None;
    let derived = module
        .clone()
        .try_derive_value(producer, history, |mut value| {
            let applied = match &mut value {
                crate::PioValue::BalancedNetwork(network) => network.apply_geo_layer(layer),
                crate::PioValue::MulticonductorNetwork(network) => {
                    crate::dist_geo::apply_dist_geo_layer(network, layer)
                }
                value => {
                    return Err(powerio_core::Error::new(
                        &codes::REQUEST_MODULE_WRONG_MODEL_KIND,
                        format!(
                            "apply_geo_layer requires powerio.BalancedNetwork or \
                             powerio.MulticonductorNetwork; the module contains {}",
                            value.type_name()
                        ),
                    ));
                }
            };
            report = Some(applied);
            Ok(value)
        })?;
    let report = report.ok_or_else(|| {
        powerio_core::Error::new(
            &codes::REQUEST_MODULE_WRONG_MODEL_KIND,
            "apply_geo_layer did not receive a network value",
        )
    })?;
    Ok((derived, report))
}

/// Construct a DC power flow calculation from a balanced network module.
/// Module diagnostics, source descriptions, provenance, and prior history are
/// preserved. Retained source bytes and value locators are cleared because
/// they describe the network rather than the calculation instance.
pub fn to_dc_pf_instance(
    module: &powerio_core::PioModule<crate::PioValue>,
) -> Result<powerio_core::PioModule<powerio_prob::DcPfInstance>, powerio_core::Error> {
    if matches!(module.value(), crate::PioValue::DcPfInstance(_)) {
        return Ok(module.clone().map_value(|value| match value {
            crate::PioValue::DcPfInstance(instance) => instance,
            _ => unreachable!("the value type was checked before extraction"),
        }));
    }
    derive_balanced_calculation(
        module,
        "to_dc_pf_instance",
        "powerio.DcPfInstance",
        powerio_prob::DcPfInstance::from_network,
    )
}

/// Construct an AC power flow calculation from a balanced network module.
pub fn to_ac_pf_instance(
    module: &powerio_core::PioModule<crate::PioValue>,
) -> Result<powerio_core::PioModule<powerio_prob::AcPfInstance>, powerio_core::Error> {
    if matches!(module.value(), crate::PioValue::AcPfInstance(_)) {
        return Ok(module.clone().map_value(|value| match value {
            crate::PioValue::AcPfInstance(instance) => instance,
            _ => unreachable!("the value type was checked before extraction"),
        }));
    }
    derive_balanced_calculation(
        module,
        "to_ac_pf_instance",
        "powerio.AcPfInstance",
        powerio_prob::AcPfInstance::from_network,
    )
}

/// Construct a DC optimal power flow calculation from a balanced network
/// module.
pub fn to_dc_opf_instance(
    module: &powerio_core::PioModule<crate::PioValue>,
) -> Result<powerio_core::PioModule<powerio_prob::DcOpfInstance>, powerio_core::Error> {
    if matches!(module.value(), crate::PioValue::DcOpfInstance(_)) {
        return Ok(module.clone().map_value(|value| match value {
            crate::PioValue::DcOpfInstance(instance) => instance,
            _ => unreachable!("the value type was checked before extraction"),
        }));
    }
    derive_balanced_calculation(
        module,
        "to_dc_opf_instance",
        "powerio.DcOpfInstance",
        powerio_prob::DcOpfInstance::from_network,
    )
}

/// Construct an AC optimal power flow calculation from a balanced network
/// module.
pub fn to_ac_opf_instance(
    module: &powerio_core::PioModule<crate::PioValue>,
) -> Result<powerio_core::PioModule<powerio_prob::AcOpfInstance>, powerio_core::Error> {
    if matches!(module.value(), crate::PioValue::AcOpfInstance(_)) {
        return Ok(module.clone().map_value(|value| match value {
            crate::PioValue::AcOpfInstance(instance) => instance,
            _ => unreachable!("the value type was checked before extraction"),
        }));
    }
    derive_balanced_calculation(
        module,
        "to_ac_opf_instance",
        "powerio.AcOpfInstance",
        powerio_prob::AcOpfInstance::from_network,
    )
}

/// Construct a multiconductor AC power flow calculation from a
/// multiconductor network module.
pub fn to_mc_ac_pf_instance(
    module: &powerio_core::PioModule<crate::PioValue>,
) -> Result<powerio_core::PioModule<powerio_prob::McAcPfInstance>, powerio_core::Error> {
    if matches!(module.value(), crate::PioValue::McAcPfInstance(_)) {
        return Ok(module.clone().map_value(|value| match value {
            crate::PioValue::McAcPfInstance(instance) => instance,
            _ => unreachable!("the value type was checked before extraction"),
        }));
    }
    derive_multiconductor_calculation(
        module,
        "to_mc_ac_pf_instance",
        "powerio.McAcPfInstance",
        powerio_prob::McAcPfInstance::from_network,
    )
}

/// Construct a multiconductor AC optimal power flow calculation from a
/// multiconductor network module.
pub fn to_mc_ac_opf_instance(
    module: &powerio_core::PioModule<crate::PioValue>,
) -> Result<powerio_core::PioModule<powerio_prob::McAcOpfInstance>, powerio_core::Error> {
    if matches!(module.value(), crate::PioValue::McAcOpfInstance(_)) {
        return Ok(module.clone().map_value(|value| match value {
            crate::PioValue::McAcOpfInstance(instance) => instance,
            _ => unreachable!("the value type was checked before extraction"),
        }));
    }
    derive_multiconductor_calculation(
        module,
        "to_mc_ac_opf_instance",
        "powerio.McAcOpfInstance",
        powerio_prob::McAcOpfInstance::from_network,
    )
}

/// Cap a history note list at the record limit, replacing the overflow with
/// one note stating how many entries were elided, and normalize every kept
/// note to the record layer's requirements: NUL replaced, never empty, and
/// truncated at a character boundary within the identifier bound with a
/// visible marker. Truncation and elision are always visible, never silent.
fn capped_history_notes(notes: Vec<String>, what: &str) -> Vec<String> {
    let cap = powerio_core::limits::MAX_HISTORY_NOTES;
    if notes.len() <= cap {
        return notes.into_iter().map(normalized_note).collect();
    }
    let elided = notes.len() - (cap - 1);
    let mut kept: Vec<String> = notes
        .into_iter()
        .take(cap - 1)
        .map(normalized_note)
        .collect();
    kept.push(format!("{elided} more {what} elided"));
    kept
}

fn transform_history(
    id: HistoryId,
    records: &TransformRecords,
    merged_buses: &BTreeMap<String, String>,
    removed_switches: &[String],
) -> HistoryEntry {
    let parameters: BTreeMap<String, serde_json::Value> =
        records.options.clone().into_iter().collect();
    let mut entry = HistoryEntry::new(id, HistoryKind::Transform, "to_balanced")
        .expect("the static history name is valid")
        .with_input_type("powerio.MulticonductorNetwork")
        .expect("the registered input type is valid")
        .with_output_type("powerio.BalancedNetwork")
        .expect("the registered output type is valid")
        .with_parameters(parameters)
        .expect("the transformation has a bounded parameter set");

    let mut assumptions = records.assumptions.clone();
    assumptions.extend(
        records
            .approximations
            .iter()
            .map(|note| format!("approximation: {note}")),
    );
    assumptions.extend(
        merged_buses
            .iter()
            .map(|(removed, kept)| format!("bus {removed} merged into bus {kept}")),
    );
    assumptions.extend(
        removed_switches
            .iter()
            .map(|switch| format!("switch {switch} removed by its bus merge")),
    );
    for assumption in capped_history_notes(assumptions, "assumptions") {
        entry = entry
            .with_assumption(assumption)
            .expect("the note list is under the history cap by construction");
    }
    for loss in capped_history_notes(records.dropped_fields.clone(), "losses") {
        entry = entry
            .with_loss(loss)
            .expect("the loss list is under the history cap by construction");
    }
    entry
}

fn copy_history_with_id(history: &HistoryEntry, id: HistoryId) -> HistoryEntry {
    let mut copied = HistoryEntry::new(id, history.kind(), history.name())
        .expect("the existing history name is valid")
        .with_parameters(history.parameters().clone())
        .expect("the existing parameter set is valid");
    if let Some(type_name) = history.input_type() {
        copied = copied
            .with_input_type(type_name)
            .expect("the existing input type is valid");
    }
    if let Some(type_name) = history.output_type() {
        copied = copied
            .with_output_type(type_name)
            .expect("the existing output type is valid");
    }
    for assumption in history.assumptions() {
        copied = copied
            .with_assumption(assumption.clone())
            .expect("the existing assumption list is valid");
    }
    for loss in history.losses() {
        copied = copied
            .with_loss(loss.clone())
            .expect("the existing loss list is valid");
    }
    copied
}

/// One history note made valid for the record layer, whatever the source
/// element names carried: nonempty, free of NUL, within the identifier
/// bound.
fn normalized_note(note: String) -> String {
    let mut note = if note.contains('\0') {
        note.replace('\0', "\u{fffd}")
    } else {
        note
    };
    let bound = powerio_core::limits::MAX_IDENTIFIER_BYTES;
    if note.len() > bound {
        let marker = " [truncated]";
        let mut end = bound - marker.len();
        while !note.is_char_boundary(end) {
            end -= 1;
        }
        note.truncate(end);
        note.push_str(marker);
    }
    if note.is_empty() {
        note.push_str("(an empty note was elided)");
    }
    note
}

/// A history id unused by the module: the stable name, then a numbered
/// spelling when a prior lowering already recorded one.
fn unused_history_id(
    module: &powerio_core::PioModule<crate::PioValue>,
    base: &str,
) -> powerio_core::HistoryId {
    use powerio_core::HistoryId;
    let taken: std::collections::BTreeSet<&str> = module
        .history()
        .iter()
        .map(|entry| entry.id().as_str())
        .collect();
    if !taken.contains(base) {
        return HistoryId::new(base).expect("static id is valid");
    }
    let mut counter = 2usize;
    loop {
        let candidate = format!("{base}-{counter}");
        if !taken.contains(candidate.as_str()) {
            return HistoryId::new(candidate).expect("numbered id is valid");
        }
        counter += 1;
    }
}

fn wrong_kind_error(value: &crate::PioValue) -> powerio_core::Error {
    powerio_core::Error::new(
        &codes::TRANSFORM_MULTI_TO_BALANCED_WRONG_MODEL_KIND,
        format!(
            "the module carries a {} value; the balanced lowering takes a multiconductor \
             network",
            value.type_name()
        ),
    )
}

struct LoweringState<'a> {
    net: &'a MulticonductorNetwork,
    options: MulticonductorToBalancedOptions,
    neutral_terminals: BTreeSet<String>,
    /// Every multiconductor bus (lowercase) to its balanced bus: merged
    /// members map to their canonical bus's ID.
    bus_ids: BTreeMap<String, BusId>,
    /// Lowercase bus id to its canonical member's row index.
    canonical_rows: BTreeMap<String, usize>,
    /// Removed bus ID to kept bus ID, source spelling.
    merged_buses: BTreeMap<String, String>,
    removed_switches: Vec<String>,
    /// Per bus (lowercase) line to line voltage base in volts.
    bus_base: BTreeMap<String, f64>,
    records: TransformRecords,
}

impl<'a> LoweringState<'a> {
    fn new(
        net: &'a MulticonductorNetwork,
        options: MulticonductorToBalancedOptions,
        readiness: MulticonductorToBalancedReport,
    ) -> Self {
        let mut records = TransformRecords::new(options);
        records.assumptions = readiness.assumptions;
        records.approximations = readiness.approximations;
        records.diagnostics = readiness.diagnostics;
        records
            .assumptions
            .push(format!("balanced power base: {} MVA", options.base_mva));
        records
            .assumptions
            .push("balanced bus ids are synthesized from multiconductor bus order".to_owned());
        records.approximations.push(
            "wire-coordinate branch and shunt matrices are projected to positive sequence"
                .to_owned(),
        );
        records.approximations.push(
            "phase injection records are aggregated into scalar balanced injections".to_owned(),
        );
        records.approximations.push(
            "units are converted from W/var/V/ohm/siemens/radians to MW/MVAr/per-unit/degrees"
                .to_owned(),
        );
        if net.switches().iter().any(|sw| sw.open) {
            records
                .dropped_fields
                .push("open switches dropped from balanced model".to_owned());
        }

        // Union closed switch endpoints: preflight already refused every
        // blocked merge, so a closed switch here merges its buses. The
        // canonical member is the earliest bus row; merged rows disappear
        // from the balanced model and the mapping is recorded.
        let row_of: BTreeMap<String, usize> = net
            .buses()
            .iter()
            .enumerate()
            .map(|(row, bus)| (bus.id.to_ascii_lowercase(), row))
            .collect();
        let mut union = UnionFind::new(net.buses().len());
        let mut removed_switches = Vec::new();
        for sw in net.switches().iter().filter(|sw| !sw.open) {
            let (Some(&from), Some(&to)) = (
                row_of.get(&sw.bus_from.to_ascii_lowercase()),
                row_of.get(&sw.bus_to.to_ascii_lowercase()),
            ) else {
                continue;
            };
            union.join(from, to);
            removed_switches.push(sw.name.clone());
            records.assumptions.push(format!(
                "closed switch {} merged bus {} into bus {} and was removed; no impedance \
                 was invented for it",
                sw.name, sw.bus_to, sw.bus_from
            ));
        }
        let mut canonical_rows = BTreeMap::new();
        let mut merged_buses = BTreeMap::new();
        let mut number = BTreeMap::new();
        for (row, bus) in net.buses().iter().enumerate() {
            let root = union.root(row);
            if root == row {
                let id = BusId(number.len() + 1);
                number.insert(row, id);
            } else {
                merged_buses.insert(bus.id.clone(), net.buses()[root].id.clone());
            }
            canonical_rows.insert(bus.id.to_ascii_lowercase(), root);
        }
        let bus_ids = net
            .buses()
            .iter()
            .map(|bus| {
                let key = bus.id.to_ascii_lowercase();
                let root = union.root(row_of[&key]);
                (key, number[&root])
            })
            .collect();

        Self {
            net,
            options,
            neutral_terminals: global_neutral_terminals(net),
            bus_ids,
            canonical_rows,
            merged_buses,
            removed_switches,
            bus_base: BTreeMap::new(),
            records,
        }
    }

    #[allow(clippy::too_many_lines)]
    fn lower(
        &mut self,
    ) -> Result<MulticonductorToBalancedTransformation, MulticonductorToBalancedError> {
        let Some(base) = self.voltage_base()? else {
            return Err(MulticonductorToBalancedError::new(
                self.options,
                &self.records.diagnostics,
            ));
        };

        self.assign_bus_bases(base);
        let buses = self.lower_buses(base);
        let mut branches = self.lower_lines()?;
        branches.extend(self.lower_transformers());
        let loads = self.lower_loads();
        let shunts = self.lower_shunts()?;
        let generators = self.lower_generators(&buses);
        self.record_capacitor_drops();
        self.err_if_errors()?;

        let mut network = BalancedNetwork::new(
            self.net
                .name()
                .clone()
                .unwrap_or_else(|| "lowered-multiconductor".to_owned()),
            self.options.base_mva,
        );
        *network.base_frequency_mut() = self.net.base_frequency();
        *network.buses_mut() = buses;
        *network.loads_mut() = loads;
        *network.shunts_mut() = shunts;
        *network.branches_mut() = branches;
        *network.generators_mut() = generators;
        *network.source_format_mut() = SourceFormat::InMemory;
        if let Err(err) = network.validate() {
            self.records.diagnostics.push(Diagnostic::of(
                &codes::TRANSFORM_MULTI_TO_BALANCED_INVALID_BALANCED_OUTPUT,
                format!("lowered balanced network failed structural validation: {err}"),
            ));
            return Err(MulticonductorToBalancedError::new(
                self.options,
                &self.records.diagnostics,
            ));
        }
        for finding in network.validate_values() {
            let details = finding.details();
            self.records.diagnostics.push(
                Diagnostic::of(
                    &codes::TRANSFORM_MULTI_TO_BALANCED_BALANCED_VALUE_DOMAIN,
                    format!(
                        "{} field `{}` is outside its value domain after lowering",
                        details["element"].as_str().unwrap_or_default(),
                        details["field"].as_str().unwrap_or_default()
                    ),
                )
                .with_suggested_action(
                    "Inspect the multiconductor source values before using the lowered model.",
                ),
            );
        }

        let history = transform_history(
            HistoryId::new("multiconductor-to-balanced").expect("static history id is valid"),
            &self.records,
            &self.merged_buses,
            &self.removed_switches,
        );
        Ok(MulticonductorToBalancedTransformation {
            network,
            diagnostics: self.records.diagnostics.clone(),
            history,
            merged_buses: self.merged_buses.clone(),
            removed_switches: self.removed_switches.clone(),
        })
    }

    /// Voltage bases by zone: buses joined by lines or merged switches share
    /// one base; a zone with a source takes the source's positive sequence
    /// magnitude; a supported transformer bases the zone across it with the
    /// far winding's rated voltage; anything still unbased defaults to the
    /// reference base with a note.
    fn assign_bus_bases(&mut self, reference: VoltageBase) {
        let row_of: BTreeMap<String, usize> = self
            .net
            .buses()
            .iter()
            .enumerate()
            .map(|(row, bus)| (bus.id.to_ascii_lowercase(), row))
            .collect();
        let mut zones = UnionFind::new(self.net.buses().len());
        for line in self.net.lines() {
            if let (Some(&from), Some(&to)) = (
                row_of.get(&line.bus_from.to_ascii_lowercase()),
                row_of.get(&line.bus_to.to_ascii_lowercase()),
            ) {
                zones.join(from, to);
            }
        }
        for sw in self.net.switches().iter().filter(|sw| !sw.open) {
            if let (Some(&from), Some(&to)) = (
                row_of.get(&sw.bus_from.to_ascii_lowercase()),
                row_of.get(&sw.bus_to.to_ascii_lowercase()),
            ) {
                zones.join(from, to);
            }
        }

        let mut zone_base: BTreeMap<usize, f64> = BTreeMap::new();
        for source in self.net.sources() {
            let Some(&row) = row_of.get(&source.bus.to_ascii_lowercase()) else {
                continue;
            };
            let bus = self.net.bus(&source.bus);
            let positions = active_positions(&source.terminal_map, bus, &self.neutral_terminals);
            if positions.len() != 3 {
                continue;
            }
            let Some(v1) = positive_sequence_voltage(source, &positions) else {
                continue;
            };
            if v1.norm().is_finite() && v1.norm() > 0.0 {
                zone_base.entry(zones.root(row)).or_insert(v1.norm());
            }
        }

        let supported: Vec<(usize, usize, f64, f64)> = self
            .net
            .transformers()
            .iter()
            .filter_map(|transformer| {
                let [high, low] =
                    classify_transformer(self.net, transformer, &self.neutral_terminals).ok()?;
                let high_row = *row_of.get(&high.bus.to_ascii_lowercase())?;
                let low_row = *row_of.get(&low.bus.to_ascii_lowercase())?;
                Some((high_row, low_row, high.v_ref, low.v_ref))
            })
            .collect();
        loop {
            let mut changed = false;
            for &(high_row, low_row, high_v, low_v) in &supported {
                let (high_zone, low_zone) = (zones.root(high_row), zones.root(low_row));
                match (
                    zone_base.contains_key(&high_zone),
                    zone_base.contains_key(&low_zone),
                ) {
                    (true, false) => {
                        zone_base.insert(low_zone, low_v);
                        changed = true;
                    }
                    (false, true) => {
                        zone_base.insert(high_zone, high_v);
                        changed = true;
                    }
                    _ => {}
                }
            }
            if !changed {
                break;
            }
        }

        for (row, bus) in self.net.buses().iter().enumerate() {
            let zone = zones.root(row);
            let base = zone_base.get(&zone).copied().unwrap_or_else(|| {
                self.records.dropped_fields.push(format!(
                    "bus {} voltage base defaulted to the reference base",
                    bus.id
                ));
                reference.line_to_line_volts
            });
            self.bus_base.insert(bus.id.to_ascii_lowercase(), base);
        }
    }

    /// The line to line voltage base of one bus, in volts.
    fn base_volts(&self, bus: &str) -> f64 {
        self.bus_base
            .get(&bus.to_ascii_lowercase())
            .copied()
            .expect("every declared bus was based")
    }

    fn voltage_base(&mut self) -> Result<Option<VoltageBase>, MulticonductorToBalancedError> {
        for (idx, source) in self.net.sources().iter().enumerate() {
            let Some(bus) = self.net.bus(&source.bus) else {
                self.records.diagnostics.push(
                    Diagnostic::of(
                        &codes::TRANSFORM_MULTI_TO_BALANCED_UNKNOWN_SOURCE_BUS,
                        format!(
                            "voltage source {} references unknown bus {}",
                            source.name, source.bus
                        ),
                    )
                    .with_value_target(format!("/sources/{idx}/bus")),
                );
                continue;
            };
            let positions =
                active_positions(&source.terminal_map, Some(bus), &self.neutral_terminals);
            if positions.len() != 3 {
                continue;
            }
            let Some(v1) = positive_sequence_voltage(source, &positions) else {
                self.records.diagnostics.push(
                    Diagnostic::of(
    &codes::TRANSFORM_MULTI_TO_BALANCED_INVALID_PHASE_REFERENCE,
format!(
                            "voltage source {} does not carry finite three phase voltage magnitudes and angles",
                            source.name
                        ),
                    )
                    .with_value_target(format!("/sources/{idx}")),
                );
                continue;
            };
            let line_to_line_volts = v1.norm();
            if !line_to_line_volts.is_finite() || line_to_line_volts <= 0.0 {
                self.records.diagnostics.push(
                    Diagnostic::of(
    &codes::TRANSFORM_MULTI_TO_BALANCED_INVALID_PHASE_REFERENCE,
format!(
                            "voltage source {} produced a non-positive positive-sequence voltage base",
                            source.name
                        ),
                    )
                    .with_value_target(format!("/sources/{idx}")),
                );
                continue;
            }
            self.records.assumptions.push(format!(
                "voltage base synthesized from source {} positive-sequence voltage: {} kV line-to-line",
                source.name,
                line_to_line_volts / 1000.0
            ));
            return Ok(Some(VoltageBase { line_to_line_volts }));
        }

        if self
            .records
            .diagnostics
            .iter()
            .any(|d| d.severity() >= DiagnosticSeverity::Error)
        {
            return Err(MulticonductorToBalancedError::new(
                self.options,
                &self.records.diagnostics,
            ));
        }
        self.records.diagnostics.push(Diagnostic::of(
    &codes::TRANSFORM_MULTI_TO_BALANCED_MISSING_PHASE_REFERENCE,
"multiconductor to balanced lowering requires a finite three phase voltage source reference",
        ));
        Ok(None)
    }

    fn lower_buses(&mut self, _reference: VoltageBase) -> Vec<Bus> {
        // Canonical buses only: a merged member's data folds into its
        // canonical bus, and its identity is recorded in `merged_buses`.
        let mut members: BTreeMap<usize, Vec<&DistBus>> = BTreeMap::new();
        for bus in self.net.buses() {
            let root = self.canonical_rows[&bus.id.to_ascii_lowercase()];
            members.entry(root).or_default().push(bus);
        }
        let mut balanced_buses = Vec::with_capacity(members.len());
        for (&root, group) in &members {
            let canonical = &self.net.buses()[root];
            let base_volts = self.base_volts(&canonical.id);
            let sourced = group.iter().find_map(|bus| {
                self.net
                    .sources()
                    .iter()
                    .find(|source| source.bus.eq_ignore_ascii_case(&bus.id))
                    .map(|source| (*bus, source))
            });
            let (vm, va) = sourced
                .and_then(|(bus, source)| {
                    let positions =
                        active_positions(&source.terminal_map, Some(bus), &self.neutral_terminals);
                    positive_sequence_voltage(source, &positions)
                })
                .map_or((1.0, 0.0), |v| {
                    (v.norm() / base_volts, radians_to_degrees(v.arg()))
                });
            if sourced.is_none() {
                self.records.dropped_fields.push(format!(
                    "bus {} voltage magnitude and angle defaulted to 1.0 p.u. and 0 degrees",
                    canonical.id
                ));
            }
            // Preflight refused conflicting stated bounds across a merge, so
            // the first member stating both carries the group's bounds.
            let stated = group.iter().find_map(|bus| match (bus.v_min, bus.v_max) {
                (Some(vmin), Some(vmax)) if vmin.is_finite() && vmax.is_finite() => {
                    Some((vmin / base_volts, vmax / base_volts))
                }
                _ => None,
            });
            let (vmin, vmax) = stated.unwrap_or_else(|| {
                self.records.dropped_fields.push(format!(
                    "bus {} voltage bounds defaulted to 0.9/1.1 p.u.",
                    canonical.id
                ));
                (0.9, 1.1)
            });
            for bus in group {
                self.record_bus_bound_drops(bus);
            }
            let kind = group
                .iter()
                .map(|bus| self.bus_kind(&bus.id))
                .min_by_key(|kind| match kind {
                    BusType::Ref => 0,
                    BusType::Pv => 1,
                    _ => 2,
                })
                .unwrap_or(BusType::Pq);
            let mut balanced = Bus::new(
                self.bus_ids[&canonical.id.to_ascii_lowercase()],
                kind,
                base_volts / 1000.0,
            );
            balanced.vm = vm;
            balanced.va = va;
            balanced.vmax = vmax;
            balanced.vmin = vmin;
            balanced.name = Some(canonical.id.clone());
            balanced.extras = source_extra("multiconductor_bus_id", &canonical.id);
            balanced_buses.push(balanced);
        }
        balanced_buses
    }

    /// A rated capacitor bank (BMOPF schema 0.1.0 `capacitor`) has no
    /// balanced equivalent yet: `q_rated` at `v_nom` is a nameplate rating,
    /// not the admittance a balanced `Shunt` carries. The bank therefore
    /// drops, and the record names it, because a silent drop removes
    /// reactive support the case depends on.
    fn record_capacitor_drops(&mut self) {
        for capacitor in self.net.capacitors() {
            self.records.dropped_fields.push(format!(
                "capacitor {} dropped: a rated bank has no balanced shunt equivalent",
                capacitor.name
            ));
        }
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
            self.records.dropped_fields.push(format!(
                "bus {} conductor voltage bound families dropped",
                bus.id
            ));
        }
    }

    fn bus_kind(&self, bus_id: &str) -> BusType {
        if self
            .net
            .sources()
            .iter()
            .any(|source| source.bus.eq_ignore_ascii_case(bus_id))
        {
            BusType::Ref
        } else if self
            .net
            .generators()
            .iter()
            .any(|generator| generator.bus.eq_ignore_ascii_case(bus_id))
        {
            BusType::Pv
        } else {
            BusType::Pq
        }
    }

    #[allow(clippy::too_many_lines)]
    fn lower_lines(&mut self) -> Result<Vec<Branch>, MulticonductorToBalancedError> {
        let mut branches = Vec::with_capacity(self.net.lines().len());
        for (idx, line) in self.net.lines().iter().enumerate() {
            let Some(code) = self.net.linecode(&line.linecode) else {
                self.records.diagnostics.push(
                    Diagnostic::of(
                        &codes::TRANSFORM_MULTI_TO_BALANCED_UNKNOWN_LINECODE,
                        format!(
                            "line {} references unknown linecode `{}`",
                            line.name, line.linecode
                        ),
                    )
                    .with_value_target(format!("/lines/{idx}/linecode")),
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
                self.records.diagnostics.push(
                    Diagnostic::of(
    &codes::TRANSFORM_MULTI_TO_BALANCED_PHASE_MAP_MISMATCH,
format!(
                            "line {} connects different active terminal orders and cannot be lowered transparently",
                            line.name
                        ),
                    )
                    .with_value_target(format!("/lines/{idx}")),
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
            let base_volts = self.base_volts(&line.bus_from);
            let z_base = z_base_ohm_of(base_volts, self.options.base_mva);
            let y_scale = z_base;
            let charging = BranchCharging::new(
                y_from.re * y_scale,
                y_from.im * y_scale,
                y_to.re * y_scale,
                y_to.im * y_scale,
            );
            let rate = line_rate_mva(line, code, &active, base_volts).unwrap_or_else(|| {
                self.records.dropped_fields.push(format!(
                    "line {} thermal rating defaulted to 0 MVA",
                    line.name
                ));
                0.0
            });
            let mut branch = Branch::new(from, to, z_ohm.re / z_base, z_ohm.im / z_base);
            branch.b = charging.calc_total_b();
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
        self.check_finite_length(line_idx, length)?;
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
            self.records.approximations.push(format!(
                "linecode {code_name} {label} has sequence coupling norm {coupling}; positive-sequence diagonal retained"
            ));
            let mut diagnostic = Diagnostic::of(
    &codes::TRANSFORM_MULTI_TO_BALANCED_SEQUENCE_COUPLING_DROPPED,
format!(
                    "linecode {code_name} {label} has nonzero sequence coupling; the balanced model keeps the positive-sequence diagonal"
                ),
            )
            .with_value_target(format!("/lines/{line_idx}/linecode"));
            diagnostic
                .insert_detail("sequence_coupling_norm", serde_json::json!(coupling))
                .expect("the static detail key is valid");
            self.records.diagnostics.push(diagnostic);
        }
        seq[1][1]
    }

    /// Refuse a line whose length is not a finite number. A BMOPF line without
    /// a length reads back as `NaN` (the `null` spelling), and every impedance
    /// and admittance below scales by it, so an unchecked value would reach the
    /// solver as a `NaN` branch with nothing said about it.
    fn check_finite_length(
        &self,
        line_idx: usize,
        length: f64,
    ) -> Result<(), MulticonductorToBalancedError> {
        if length.is_finite() {
            return Ok(());
        }
        let mut diagnostics = self.records.diagnostics.clone();
        diagnostics.push(
            Diagnostic::of(
    &codes::TRANSFORM_MULTI_TO_BALANCED_NONFINITE_LINE_LENGTH,
format!("line {line_idx} has no finite length ({length}), so its impedance cannot be scaled"),
            )
            .with_value_target(format!("/lines/{line_idx}/length"))
            .with_suggested_action("give the line a length in meters, or drop it from the network"),
        );
        Err(MulticonductorToBalancedError::new(
            self.options,
            &diagnostics,
        ))
    }

    fn matrix_error(
        &self,
        line_idx: usize,
        code_name: &str,
        label: &str,
        message: &str,
    ) -> MulticonductorToBalancedError {
        let mut diagnostics = self.records.diagnostics.clone();
        diagnostics.push(
            Diagnostic::of(
                &codes::TRANSFORM_MULTI_TO_BALANCED_INVALID_LINECODE_MATRIX,
                format!("linecode {code_name} {label} cannot be lowered: {message}"),
            )
            .with_value_target(format!("/lines/{line_idx}/linecode")),
        );
        MulticonductorToBalancedError::new(self.options, &diagnostics)
    }

    /// Supported transformers lower into balanced branches: series impedance
    /// from the winding resistances and the first short circuit reactance on
    /// the transformer's own base converted to the system base, tap from the
    /// rated winding voltages against the zone voltage bases, and the
    /// representable ANSI thirty degree connection shift with the high
    /// voltage side leading.
    fn lower_transformers(&mut self) -> Vec<Branch> {
        let mut branches = Vec::new();
        for transformer in self.net.transformers() {
            let Ok([high, low]) =
                classify_transformer(self.net, transformer, &self.neutral_terminals)
            else {
                // Preflight refused the pass for unsupported transformers.
                continue;
            };
            let (Some(from), Some(to)) = (self.bus_id(&high.bus), self.bus_id(&low.bus)) else {
                continue;
            };
            let base_from = self.base_volts(&high.bus);
            let base_to = self.base_volts(&low.bus);
            let tap = (high.v_ref * high.tap / base_from) / (low.v_ref * low.tap / base_to);
            let z_scale = (self.options.base_mva * 1_000_000.0 / high.s_rating)
                * (high.v_ref / base_from).powi(2);
            // Each winding states %R on its own power rating; the low winding
            // figure converts onto the high winding base before the sum.
            let low_rating_scale = high.s_rating / low.s_rating;
            let r = ((high.r_pct + low.r_pct * low_rating_scale) / 100.0) * z_scale;
            let x = (transformer.xsc_pct[0] / 100.0) * z_scale;
            let shift = if high.v_ref >= low.v_ref { 30.0 } else { -30.0 };
            let rate = high.s_rating / 1_000_000.0;
            self.records.assumptions.push(format!(
                "transformer {} lowered as a balanced branch with tap {tap:.6} and the ANSI \
                 {shift} degree connection shift (high voltage side leads)",
                transformer.name
            ));
            if (low_rating_scale - 1.0).abs() > 1e-9 {
                self.records.assumptions.push(format!(
                    "transformer {}: the low winding resistance was converted from its own \
                     {:.3} kVA base onto the high winding {:.3} kVA base",
                    transformer.name,
                    low.s_rating / 1_000.0,
                    high.s_rating / 1_000.0
                ));
            }
            if high.r_neutral.is_some()
                || high.x_neutral.is_some()
                || low.r_neutral.is_some()
                || low.x_neutral.is_some()
            {
                self.records.dropped_fields.push(format!(
                    "transformer {} neutral grounding impedance dropped",
                    transformer.name
                ));
            }
            if transformer.xsc_pct.len() > 1 {
                self.records.dropped_fields.push(format!(
                    "transformer {} extra short circuit reactances dropped",
                    transformer.name
                ));
            }
            let mut branch = Branch::new(from, to, r, x);
            branch.tap = tap;
            branch.shift = shift;
            branch.rate_a = rate;
            branch.rate_b = rate;
            branch.rate_c = rate;
            branch.extras = source_extra("multiconductor_transformer", &transformer.name);
            branches.push(branch);
        }
        branches
    }

    fn lower_loads(&mut self) -> Vec<Load> {
        self.net
            .loads()
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
                    self.records.dropped_fields.push(format!(
                        "load {} voltage model dropped; balanced load is constant power",
                        load.name
                    ));
                    self.records.diagnostics.push(
                        Diagnostic::of(
    &codes::TRANSFORM_MULTI_TO_BALANCED_DROPPED_LOAD_VOLTAGE_MODEL,
format!(
                                "load {} voltage model cannot be represented by the conservative balanced lowering",
                                load.name
                            ),
                        )
                        .with_value_target(format!("/loads/{idx}/voltage_model")),
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

    fn lower_shunts(&mut self) -> Result<Vec<Shunt>, MulticonductorToBalancedError> {
        let mut shunts = Vec::with_capacity(self.net.shunts().len());
        for (idx, shunt) in self.net.shunts().iter().enumerate() {
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
                self.records.approximations.push(format!(
                    "shunt {} has {} active terminal(s); diagonal admittance projected with missing phases as zero",
                    shunt.name,
                    active.len()
                ));
                partial_phase_admittance(&shunt.g, &shunt.b, &active)
            };
            let base_volts = self.base_volts(&shunt.bus);
            let scale = base_volts * base_volts / 1_000_000.0;
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
        let mut diagnostics = self.records.diagnostics.clone();
        diagnostics.push(
            Diagnostic::of(
                &codes::TRANSFORM_MULTI_TO_BALANCED_INVALID_SHUNT_MATRIX,
                format!("shunt {name} cannot be lowered: {message}"),
            )
            .with_value_target(format!("/shunts/{shunt_idx}")),
        );
        MulticonductorToBalancedError::new(self.options, &diagnostics)
    }

    fn lower_generators(&mut self, buses: &[Bus]) -> Vec<Generator> {
        self.net
            .generators()
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
                    self.records.dropped_fields.push(format!(
                        "generator {} p_min defaulted to pg",
                        generator.name
                    ));
                    pg
                });
                let pmax = option_vec_sum_mw(generator.p_max.as_deref()).unwrap_or_else(|| {
                    self.records.dropped_fields.push(format!(
                        "generator {} p_max defaulted to pg",
                        generator.name
                    ));
                    pg
                });
                let qmin = option_vec_sum_mw(generator.q_min.as_deref()).unwrap_or_else(|| {
                    self.records.dropped_fields.push(format!(
                        "generator {} q_min defaulted to qg",
                        generator.name
                    ));
                    qg
                });
                let qmax = option_vec_sum_mw(generator.q_max.as_deref()).unwrap_or_else(|| {
                    self.records.dropped_fields.push(format!(
                        "generator {} q_max defaulted to qg",
                        generator.name
                    ));
                    qg
                });
                if generator.cost.is_some() {
                    self.records.dropped_fields.push(format!(
                        "generator {} scalar distribution cost dropped",
                        generator.name
                    ));
                }
                if generator.s_max.is_some() || generator.i_max.is_some() {
                    self.records.dropped_fields.push(format!(
                        "generator {} per-conductor rating fields dropped",
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
        self.records.diagnostics.push(
            Diagnostic::of(
                &codes::TRANSFORM_MULTI_TO_BALANCED_UNKNOWN_BUS,
                format!("{element} {name} references unknown bus {bus}"),
            )
            .with_value_target(format!("/{element}s/{idx}/{field}")),
        );
    }

    fn err_if_errors(&self) -> Result<(), MulticonductorToBalancedError> {
        if self
            .records
            .diagnostics
            .iter()
            .any(|d| d.severity() >= DiagnosticSeverity::Error)
        {
            Err(MulticonductorToBalancedError::new(
                self.options,
                &self.records.diagnostics,
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

fn z_base_ohm_of(line_to_line_volts: f64, base_mva: f64) -> f64 {
    line_to_line_volts * line_to_line_volts / (base_mva * 1_000_000.0)
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

    fn root(&mut self, mut node: usize) -> usize {
        while self.parent[node] != node {
            self.parent[node] = self.parent[self.parent[node]];
            node = self.parent[node];
        }
        node
    }

    fn join(&mut self, a: usize, b: usize) {
        let (a, b) = (self.root(a), self.root(b));
        // The smaller row stays the root, so the canonical member is stable.
        let (keep, fold) = if a <= b { (a, b) } else { (b, a) };
        self.parent[fold] = keep;
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

fn complex_matrix(
    g_or_r: &ConductorMatrix,
    b_or_x: &ConductorMatrix,
    scale: f64,
) -> Vec<Vec<Complex64>> {
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

/// The branch rating, in MVA. BMOPF schema 0.1.0 gives a line its own
/// `i_max`/`s_max`, which "overrides the linecode's i_max for this line", so
/// both line fields are tried before either linecode field. Within one owner
/// `s_max` comes first, because an apparent power limit needs no voltage.
///
/// A field the active conductors leave unusable falls through to the next
/// candidate rather than ending the search: a line whose `s_max` is all
/// infinities must not hide a linecode that carries a real rating.
fn line_rate_mva(
    line: &DistLine,
    code: &DistLineCode,
    active: &[usize],
    line_to_line_volts: f64,
) -> Option<f64> {
    for (s_max, i_max) in [
        (line.s_max.as_ref(), line.i_max.as_ref()),
        (code.s_max.as_ref(), code.i_max.as_ref()),
    ] {
        if let Some(mva) = s_max.and_then(|values| apparent_power_mva(values, active)) {
            return Some(mva);
        }
        if let Some(amps) = i_max.and_then(|values| limiting_amps(values, active)) {
            return Some(SQRT_3 * line_to_line_volts * amps / 1_000_000.0);
        }
    }
    None
}

/// The summed apparent power limit of the active conductors, in MVA, or None
/// when any of them has no finite limit.
fn apparent_power_mva(s_max: &[f64], active: &[usize]) -> Option<f64> {
    let values: Vec<_> = active
        .iter()
        .filter_map(|&idx| s_max.get(idx).copied())
        .collect();
    (!values.is_empty() && values.iter().all(|value| value.is_finite()))
        .then(|| values.iter().sum::<f64>() / 1_000_000.0)
}

/// The smallest usable current limit over the active conductors, in amps.
fn limiting_amps(i_max: &[f64], active: &[usize]) -> Option<f64> {
    active
        .iter()
        .filter_map(|&idx| i_max.get(idx).copied())
        .filter(|value| value.is_finite() && *value >= 0.0)
        .reduce(f64::min)
}

fn partial_phase_admittance(
    g: &ConductorMatrix,
    b: &ConductorMatrix,
    active: &[usize],
) -> Complex64 {
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

fn check_options(
    options: MulticonductorToBalancedOptions,
    report: &mut MulticonductorToBalancedReport,
) {
    if !options.base_mva.is_finite() || options.base_mva <= 0.0 {
        report.diagnostics.push(Diagnostic::of(
    &codes::TRANSFORM_MULTI_TO_BALANCED_INVALID_BASE_MVA,
format!(
                "base_mva must be positive and finite for multiconductor to balanced lowering; got {}",
                options.base_mva
            ),
        ));
    }
}

fn check_bus_conductor_sets(
    net: &MulticonductorNetwork,
    report: &mut MulticonductorToBalancedReport,
) {
    let neutral_terminals = global_neutral_terminals(net);
    let mut saw_neutral = false;
    for (i, bus) in net.buses().iter().enumerate() {
        let active_count = active_terminal_count(&bus.terminals, Some(bus), &neutral_terminals);
        if active_count < bus.terminals.len() {
            saw_neutral = true;
        }

        match active_count {
            3 => {}
            2 => report.diagnostics.push(
                Diagnostic::of(
    &codes::TRANSFORM_MULTI_TO_BALANCED_AMBIGUOUS_TERMINAL_MAP,
format!(
                        "bus {} has two active terminals; no unique positive sequence projection is defined",
                        bus.id
                    ),
                )
                .with_value_target(format!("/buses/{i}/terminals")),
            ),
            0 | 1 => report.diagnostics.push(
                Diagnostic::of(
    &codes::TRANSFORM_MULTI_TO_BALANCED_UNSUPPORTED_CONDUCTOR_SET,
format!(
                        "bus {} has {active_count} active terminal; multiconductor to balanced lowering starts with three phase input",
                        bus.id
                    ),
                )
                .with_value_target(format!("/buses/{i}/terminals")),
            ),
            _ => report.diagnostics.push(
                Diagnostic::of(
    &codes::TRANSFORM_MULTI_TO_BALANCED_UNSUPPORTED_CONDUCTOR_SET,
format!(
                        "bus {} has {active_count} active terminals; multiconductor to balanced lowering starts with three phase input",
                        bus.id
                    ),
                )
                .with_value_target(format!("/buses/{i}/terminals")),
            ),
        }
    }

    if saw_neutral {
        report
            .approximations
            .push("Kron reduction of neutral conductor before sequence transform".to_owned());
        report.diagnostics.push(Diagnostic::of(
            &codes::TRANSFORM_MULTI_TO_BALANCED_KRON_REDUCTION_REQUIRED,
            "neutral conductors require Kron reduction before the sequence transform",
        ));
    }
}

fn check_line_terminal_maps(
    net: &MulticonductorNetwork,
    report: &mut MulticonductorToBalancedReport,
) {
    let neutral_terminals = global_neutral_terminals(net);
    for (i, line) in net.lines().iter().enumerate() {
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
                    Diagnostic::of(
    &codes::TRANSFORM_MULTI_TO_BALANCED_UNSUPPORTED_CONDUCTOR_SET,
format!(
                            "line {} {field} has {active_count} active terminal(s); balanced branch lowering requires three active phase conductors",
                            line.name
                        ),
                    )
                    .with_value_target(format!("/lines/{i}/{field}")),
                );
            }
        }
    }
}

fn check_linecodes(net: &MulticonductorNetwork, report: &mut MulticonductorToBalancedReport) {
    for (i, line) in net.lines().iter().enumerate() {
        let Some(code) = net.linecode(&line.linecode) else {
            report.diagnostics.push(
                Diagnostic::of(
                    &codes::TRANSFORM_MULTI_TO_BALANCED_UNKNOWN_LINECODE,
                    format!(
                        "line {} references unknown linecode `{}`",
                        line.name, line.linecode
                    ),
                )
                .with_value_target(format!("/lines/{i}/linecode")),
            );
            continue;
        };
        if code.n_conductors != line.terminal_map_from.len()
            || code.n_conductors != line.terminal_map_to.len()
        {
            report.diagnostics.push(
                Diagnostic::of(
    &codes::TRANSFORM_MULTI_TO_BALANCED_LINECODE_TERMINAL_MISMATCH,
format!(
                        "line {} uses linecode {} with {} conductor(s), but its terminal maps have {} and {} terminal(s)",
                        line.name,
                        code.name,
                        code.n_conductors,
                        line.terminal_map_from.len(),
                        line.terminal_map_to.len()
                    ),
                )
                .with_value_target(format!("/lines/{i}/linecode")),
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
                Diagnostic::of(
                    &codes::TRANSFORM_MULTI_TO_BALANCED_INVALID_LINECODE_MATRIX,
                    format!(
                        "linecode {} does not carry square {} conductor matrices",
                        code.name, code.n_conductors
                    ),
                )
                .with_value_target(format!("/lines/{i}/linecode")),
            );
        }
    }
}

fn square_matrix_shape(matrix: &ConductorMatrix, n: usize) -> bool {
    matrix.len() == n && matrix.iter().all(|row| row.len() == n)
}

fn check_switches(net: &MulticonductorNetwork, report: &mut MulticonductorToBalancedReport) {
    let neutral_terminals = global_neutral_terminals(net);
    for (i, sw) in net.switches().iter().enumerate() {
        if sw.open {
            report.diagnostics.push(
                Diagnostic::of(
                    &codes::TRANSFORM_MULTI_TO_BALANCED_DROPPED_OPEN_SWITCH,
                    format!(
                        "open switch {} is dropped by multiconductor to balanced lowering",
                        sw.name
                    ),
                )
                .with_value_target(format!("/switches/{i}")),
            );
        } else {
            report
                .diagnostics
                .extend(switch_merge_blockers(net, i, sw, &neutral_terminals));
        }
    }
}

/// Everything that stops a closed switch from merging its buses. An empty
/// list means the switch merges: its endpoints collapse to one balanced bus
/// and the switch identity is removed with the mapping recorded. Merging
/// never invents an impedance.
fn switch_merge_blockers(
    net: &MulticonductorNetwork,
    index: usize,
    sw: &powerio_dist::DistSwitch,
    neutral_terminals: &BTreeSet<String>,
) -> Vec<Diagnostic> {
    let path = format!("/switches/{index}");
    let mut blockers = Vec::new();
    if sw
        .i_max
        .as_ref()
        .is_some_and(|limits| limits.iter().any(|limit| limit.is_finite()))
    {
        blockers.push(
            Diagnostic::of(
                &codes::TRANSFORM_MULTI_TO_BALANCED_RATED_CLOSED_SWITCH,
                format!(
                    "closed switch {} carries a finite ampacity; merging its buses would \
                     remove the branch flow the limit constrains",
                    sw.name
                ),
            )
            .with_value_target(path.clone()),
        );
    }
    let from_bus = net.bus(&sw.bus_from);
    let to_bus = net.bus(&sw.bus_to);
    if from_bus.is_none() || to_bus.is_none() {
        let missing = if from_bus.is_none() {
            &sw.bus_from
        } else {
            &sw.bus_to
        };
        blockers.push(
            Diagnostic::of(
                &codes::TRANSFORM_MULTI_TO_BALANCED_UNKNOWN_BUS,
                format!("switch {} references unknown bus {missing}", sw.name),
            )
            .with_value_target(path.clone()),
        );
        return blockers;
    }
    if !same_active_phase_order(
        from_bus,
        &sw.terminal_map_from,
        to_bus,
        &sw.terminal_map_to,
        neutral_terminals,
    ) {
        blockers.push(
            Diagnostic::of(
                &codes::TRANSFORM_MULTI_TO_BALANCED_SWITCH_TERMINAL_MISMATCH,
                format!(
                    "closed switch {} does not map identical conductors on both ends, so its \
                     buses are not electrically identical",
                    sw.name
                ),
            )
            .with_value_target(path.clone()),
        );
    }
    if !sw.bus_from.eq_ignore_ascii_case(&sw.bus_to) {
        let sourced = |bus: &str| {
            net.sources()
                .iter()
                .any(|source| source.bus.eq_ignore_ascii_case(bus))
        };
        if sourced(&sw.bus_from) && sourced(&sw.bus_to) {
            blockers.push(
                Diagnostic::of(
                    &codes::TRANSFORM_MULTI_TO_BALANCED_SWITCH_MERGE_CONFLICT,
                    format!(
                        "closed switch {} joins two buses that both carry voltage source \
                         references",
                        sw.name
                    ),
                )
                .with_value_target(path.clone()),
            );
        }
        let (from_bus, to_bus) = (from_bus.expect("checked"), to_bus.expect("checked"));
        for (label, a, b) in [
            ("v_min", from_bus.v_min, to_bus.v_min),
            ("v_max", from_bus.v_max, to_bus.v_max),
        ] {
            if let (Some(a), Some(b)) = (a, b)
                && (a - b).abs() > f64::EPSILON * a.abs().max(b.abs()).max(1.0)
            {
                blockers.push(
                    Diagnostic::of(
                        &codes::TRANSFORM_MULTI_TO_BALANCED_SWITCH_MERGE_CONFLICT,
                        format!(
                            "closed switch {} joins buses stating different {label} bounds \
                             ({a} and {b})",
                            sw.name
                        ),
                    )
                    .with_value_target(path.clone()),
                );
            }
        }
    }
    blockers
}

fn global_neutral_terminals(net: &MulticonductorNetwork) -> BTreeSet<String> {
    net.buses()
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

fn check_phase_reference(net: &MulticonductorNetwork, report: &mut MulticonductorToBalancedReport) {
    let neutral_terminals = global_neutral_terminals(net);
    let has_three_phase_source = net.sources().iter().any(|source| {
        let bus = net.bus(&source.bus);
        active_terminal_count(&source.terminal_map, bus, &neutral_terminals) == 3
    });

    if !has_three_phase_source {
        report.diagnostics.push(Diagnostic::of(
            &codes::TRANSFORM_MULTI_TO_BALANCED_MISSING_PHASE_REFERENCE,
            "multiconductor to balanced lowering requires a three phase voltage source reference",
        ));
    }
}

fn check_transformers(net: &MulticonductorNetwork, report: &mut MulticonductorToBalancedReport) {
    let neutral_terminals = global_neutral_terminals(net);
    for (i, transformer) in net.transformers().iter().enumerate() {
        if let Err(reason) = classify_transformer(net, transformer, &neutral_terminals) {
            report.diagnostics.push(
                Diagnostic::of(
                    &codes::TRANSFORM_MULTI_TO_BALANCED_UNSUPPORTED_TRANSFORMER,
                    format!("transformer {} {reason}", transformer.name),
                )
                .with_value_target(format!("/transformers/{i}")),
            );
        }
    }
}

/// Why one transformer lowers or refuses. The supported shape is a three
/// phase two winding `wye_delta` or `delta_wye` transformer with finite
/// positive ratings and full three phase terminal maps.
fn classify_transformer<'net>(
    net: &'net MulticonductorNetwork,
    transformer: &'net powerio_dist::DistTransformer,
    neutral_terminals: &BTreeSet<String>,
) -> Result<[&'net powerio_dist::DistWinding; 2], String> {
    use powerio_dist::DistWindingConn;
    if transformer.phases != 3 {
        return Err(format!(
            "has {} phases; only a three phase transformer lowers",
            transformer.phases
        ));
    }
    let [high, low] = transformer.windings.as_slice() else {
        return Err(format!(
            "has {} windings; only a two winding transformer lowers",
            transformer.windings.len()
        ));
    };
    if high.conn == low.conn {
        return Err(format!(
            "states a {:?}-{:?} connection; only wye_delta and delta_wye lower, with their \
             representable thirty degree shift",
            high.conn, low.conn
        ));
    }
    debug_assert!(matches!(
        (high.conn, low.conn),
        (DistWindingConn::Wye, DistWindingConn::Delta)
            | (DistWindingConn::Delta, DistWindingConn::Wye)
    ));
    for winding in [high, low] {
        let Some(bus) = net.bus(&winding.bus) else {
            return Err(format!("references unknown bus {}", winding.bus));
        };
        let active = active_terminal_count(&winding.terminal_map, Some(bus), neutral_terminals);
        if active != 3 {
            return Err(format!(
                "winding on bus {} maps {active} active conductors; a full three phase map \
                 is required",
                winding.bus
            ));
        }
        if !(winding.v_ref.is_finite() && winding.v_ref > 0.0) {
            return Err(format!(
                "winding on bus {} has no finite positive voltage rating",
                winding.bus
            ));
        }
        if !winding.r_pct.is_finite() || winding.r_pct < 0.0 {
            return Err(format!(
                "winding on bus {} has no finite nonnegative resistance",
                winding.bus
            ));
        }
        if !winding.tap.is_finite() || winding.tap <= 0.0 {
            return Err(format!(
                "winding on bus {} has no finite positive tap",
                winding.bus
            ));
        }
    }
    if !(high.s_rating.is_finite() && high.s_rating > 0.0) {
        return Err("has no finite positive power rating".to_owned());
    }
    if !(low.s_rating.is_finite() && low.s_rating > 0.0) {
        return Err("has no finite positive low winding power rating".to_owned());
    }
    match transformer.xsc_pct.first() {
        Some(x) if x.is_finite() && *x >= 0.0 => {}
        _ => return Err("states no finite short circuit reactance".to_owned()),
    }
    Ok([high, low])
}

fn check_untyped_objects(net: &MulticonductorNetwork, report: &mut MulticonductorToBalancedReport) {
    for (i, obj) in net.untyped().iter().enumerate() {
        report.diagnostics.push(
            Diagnostic::of(
                &codes::TRANSFORM_MULTI_TO_BALANCED_UNSUPPORTED_OBJECT,
                format!(
                    "{} {} is preserved as an untyped object and cannot be lowered",
                    obj.class, obj.name
                ),
            )
            .with_value_target(format!("/untyped/{i}")),
        );
    }
}

#[cfg(test)]
mod history_record_tests {
    use super::{capped_history_notes, unused_history_id};

    #[test]
    fn note_overflow_is_stated_within_the_cap() {
        let cap = powerio_core::limits::MAX_HISTORY_NOTES;
        let notes: Vec<String> = (0..cap + 40).map(|index| format!("note {index}")).collect();
        let kept = capped_history_notes(notes, "assumptions");
        assert_eq!(kept.len(), cap);
        assert_eq!(kept.last().unwrap(), "41 more assumptions elided");

        let short: Vec<String> = (0..3).map(|index| format!("note {index}")).collect();
        assert_eq!(capped_history_notes(short.clone(), "assumptions"), short);
    }

    #[test]
    fn the_history_id_is_minted_unused() {
        use powerio_core::{HistoryEntry, HistoryId, HistoryKind, PioModule};
        let mut module = PioModule::new(crate::PioValue::BalancedNetwork(
            crate::BalancedNetwork::in_memory("t", 100.0, Vec::new(), Vec::new()),
        ));
        assert_eq!(
            unused_history_id(&module, "multiconductor-to-balanced").as_str(),
            "multiconductor-to-balanced"
        );
        for id in ["multiconductor-to-balanced", "multiconductor-to-balanced-2"] {
            module
                .add_history_entry(
                    HistoryEntry::new(
                        HistoryId::new(id).unwrap(),
                        HistoryKind::Transform,
                        "to_balanced",
                    )
                    .unwrap(),
                )
                .unwrap();
        }
        assert_eq!(
            unused_history_id(&module, "multiconductor-to-balanced").as_str(),
            "multiconductor-to-balanced-3"
        );
    }
}
