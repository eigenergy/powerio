//! Explicit transformations and their preflight checks.
//!
//! A pass that changes one model into another carries the module records
//! forward and appends transformation history, so the result is auditable.
//! Emission borrows the module and returns writer diagnostics separately. The
//! most consequential transformation, multiconductor to balanced, is explicit
//! and diagnosed, never a silent positive sequence projection.

use std::collections::{BTreeMap, BTreeSet};
use std::f64::consts::PI;

use num_complex::Complex64;
use serde::{Deserialize, Serialize};

use crate::{
    BalancedNetwork, Branch, BranchCharging, Bus, BusId, BusType, Extras as BalancedExtras,
    Generator, Load, Shunt, SourceFormat,
};
use powerio_dist::{
    DistBus, DistLine, DistLineCode, DistLoadVoltageModel, Mat, MulticonductorNetwork,
};

use crate::stored::legacy09::diagnostics::{DiagnosticSeverity, StructuredDiagnostic, codes};
use crate::stored::legacy09::model::ModelKind;
use crate::stored::legacy09::validation::ValidationStatus;

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

    /// This report's findings as 1.0 module records, `target` rebased onto
    /// the multiconductor value's own pointer grammar instead of the retired
    /// `/model/multiconductor_network/...` element path. `diagnostics` itself
    /// stays the legacy shape for the internal preflight checks that build
    /// it; a caller that publishes this report (a binding, an MCP tool)
    /// should publish this instead.
    #[must_use]
    pub fn diagnostics_as_module_records(&self) -> Vec<powerio_core::Diagnostic> {
        multiconductor_diagnostics_to_module_records(&self.diagnostics)
    }
}

/// The preflight report for a multiconductor to balanced transformation.
///
/// [`MulticonductorToBalancedReadiness`] remains available for compatibility.
pub type MulticonductorToBalancedReport = MulticonductorToBalancedReadiness;

/// A successful raw multiconductor to balanced lowering result.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MulticonductorToBalancedLowering {
    pub network: BalancedNetwork,
    pub record: LoweringRecord,
    /// Buses removed by closed switch merges: removed bus ID to the kept
    /// bus ID, in the source's spelling.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub merged_buses: BTreeMap<String, String>,
    /// Closed switches whose merge removed them from the balanced model.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed_switches: Vec<String>,
}

/// The result of a successful multiconductor to balanced transformation.
///
/// [`MulticonductorToBalancedLowering`] remains available for 0.10
/// compatibility.
pub type MulticonductorToBalancedTransformation = MulticonductorToBalancedLowering;

/// Structured failure from the raw multiconductor to balanced lowering pass.
///
/// `diagnostics` are 1.0 module records: the same codes and severities the
/// preflight computed, with `target` rebased from the retired
/// `/model/multiconductor_network/...` element path onto the multiconductor
/// value's own pointer grammar (e.g. `/sources/0/bus`), since a refusal never
/// changes the module's value kind.
///
/// No `JsonSchema` derive: `powerio_core::Diagnostic` is the runtime record,
/// not a schema DTO (the stored document schema mirrors it separately as
/// `DiagnosticV1`, per `powerio::stored::dto`), and this type is not part of
/// any generated schema family.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MulticonductorToBalancedError {
    pub options: MulticonductorToBalancedOptions,
    pub status: ValidationStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<powerio_core::Diagnostic>,
}

impl MulticonductorToBalancedError {
    pub fn new(
        options: MulticonductorToBalancedOptions,
        diagnostics: &[StructuredDiagnostic],
    ) -> Self {
        Self {
            options,
            status: status_from_diagnostics(diagnostics),
            diagnostics: multiconductor_diagnostics_to_module_records(diagnostics),
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

/// Convert legacy preflight findings, whose `element_path` (when present)
/// reads `/model/multiconductor_network/...`, into 1.0 module records whose
/// `target` is that same locator rebased onto the multiconductor value's own
/// pointer grammar. Used by both [`MulticonductorToBalancedError`] and
/// [`MulticonductorToBalancedReadiness::diagnostics_as_module_records`].
fn multiconductor_diagnostics_to_module_records(
    diagnostics: &[StructuredDiagnostic],
) -> Vec<powerio_core::Diagnostic> {
    diagnostics
        .iter()
        .map(|diagnostic| {
            let target = crate::stored::legacy09::diagnostics::translate_legacy_target(
                diagnostic.element_path.as_deref(),
                "multiconductor_network",
            );
            crate::stored::legacy09::diagnostics::to_module_diagnostic(diagnostic, target)
        })
        .collect()
}

/// Check whether a multiconductor package is ready for the lowering pass.
///
/// This is a preflight only: it reports the assumptions and blockers that the
/// lowering would need to account for, but it does not produce a balanced model
/// and does not append to `history`.
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

/// Report whether `net` can be transformed to a balanced network under
/// `options`, including every assumption, approximation, and refusal.
#[must_use]
pub fn multiconductor_to_balanced_report(
    net: &MulticonductorNetwork,
    options: MulticonductorToBalancedOptions,
) -> MulticonductorToBalancedReport {
    check_multiconductor_to_balanced_lowering(net, options)
}

/// Lower a transparent three phase multiconductor network to a balanced model.
///
/// The pass is explicit. It does not run from readers, writers, matrix builders,
/// bindings, or package deserialization. Unsupported inputs return structured
/// `TRANSFORM.MULTI_TO_BALANCED.*` diagnostics in [`MulticonductorToBalancedError`].
pub fn lower_multiconductor_to_balanced(
    net: &MulticonductorNetwork,
    options: MulticonductorToBalancedOptions,
) -> Result<MulticonductorToBalancedLowering, MulticonductorToBalancedError> {
    let readiness = check_multiconductor_to_balanced_lowering(net, options);
    if !readiness.is_ready() {
        return Err(MulticonductorToBalancedError::new(
            options,
            &readiness.diagnostics,
        ));
    }

    let mut state = LoweringState::new(net, options, readiness);
    state.lower()
}

/// Transform a supported three phase multiconductor network to a balanced
/// network.
///
/// The returned record states the assumptions, approximations, merged buses,
/// and removed switches. Unsupported input returns structured diagnostics.
pub fn multiconductor_to_balanced(
    net: &MulticonductorNetwork,
    options: MulticonductorToBalancedOptions,
) -> Result<MulticonductorToBalancedTransformation, MulticonductorToBalancedError> {
    lower_multiconductor_to_balanced(net, options)
}

/// Readiness of one module's value for the balanced lowering: the #398
/// inspect operation. The value must be a multiconductor network.
///
/// # Errors
/// A value of any other kind, named.
pub fn check_module_lowering(
    module: &powerio_core::PioModule<crate::PioValue>,
    options: MulticonductorToBalancedOptions,
) -> Result<MulticonductorToBalancedReadiness, powerio_core::Error> {
    let crate::PioValue::MulticonductorNetwork(net) = module.value() else {
        return Err(wrong_kind_error(module.value()));
    };
    Ok(check_multiconductor_to_balanced_lowering(net, options))
}

/// Report whether a module's multiconductor value can be transformed to a
/// balanced network.
///
/// # Errors
/// The module carries any other value kind.
pub fn module_to_balanced_report(
    module: &powerio_core::PioModule<crate::PioValue>,
    options: MulticonductorToBalancedOptions,
) -> Result<MulticonductorToBalancedReport, powerio_core::Error> {
    check_module_lowering(module, options)
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
pub fn lower_module_to_balanced(
    module: powerio_core::PioModule<crate::PioValue>,
    options: MulticonductorToBalancedOptions,
) -> Result<
    powerio_core::PioModule<crate::PioValue>,
    (
        powerio_core::PioModule<crate::PioValue>,
        Box<MulticonductorToBalancedError>,
    ),
> {
    use powerio_core::{HistoryEntry, HistoryKind};

    let crate::PioValue::MulticonductorNetwork(net) = module.value() else {
        let error = MulticonductorToBalancedError::new(
            options,
            &[StructuredDiagnostic::of(
                &codes::TRANSFORM_MULTI_TO_BALANCED_WRONG_MODEL_KIND,
                format!(
                    "the module carries a {} value; the balanced lowering takes a \
                     multiconductor network",
                    module.value().kind().as_str()
                ),
            )],
        );
        return Err((module, Box::new(error)));
    };
    let lowering = match lower_multiconductor_to_balanced(net, options) {
        Ok(lowering) => lowering,
        Err(error) => return Err((module, Box::new(error))),
    };
    let MulticonductorToBalancedLowering {
        network,
        record,
        merged_buses,
        removed_switches,
    } = lowering;
    // Room for the pass's own records is checked against the module maxima
    // before the value is consumed, so the additions below hold by
    // construction and a cap-edge input is refused with its module intact.
    let diagnostics_room =
        powerio_core::limits::MAX_MODULE_DIAGNOSTICS.saturating_sub(module.diagnostics().len());
    let history_room =
        powerio_core::limits::MAX_MODULE_HISTORY_ENTRIES.saturating_sub(module.history().len());
    if record.diagnostics.len() > diagnostics_room || history_room == 0 {
        let error = MulticonductorToBalancedError::new(
            options,
            &[StructuredDiagnostic::of(
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
    for diagnostic in &record.diagnostics {
        module
            .add_diagnostic(crate::stored::legacy09::diagnostics::to_module_diagnostic(
                diagnostic, None,
            ))
            .expect("room was checked; pass diagnostics carry no identity and no span");
    }
    let mut entry = HistoryEntry::new(
        unused_history_id(&module, "multiconductor-to-balanced"),
        HistoryKind::Transform,
        "lower_multiconductor_to_balanced",
    )
    .expect("static name is valid");
    let mut notes = vec![format!("balanced power base: {} MVA", options.base_mva)];
    notes.extend(record.assumptions.iter().cloned());
    notes.extend(record.approximations.iter().cloned());
    notes.extend(
        merged_buses
            .iter()
            .map(|(removed, kept)| format!("bus {removed} merged into bus {kept}")),
    );
    notes.extend(
        removed_switches
            .iter()
            .map(|switch| format!("switch {switch} removed by its bus merge")),
    );
    for note in capped_history_notes(notes, "assumptions") {
        entry = entry
            .with_assumption(note)
            .expect("the note list is under the history cap by construction");
    }
    for loss in capped_history_notes(record.dropped_fields.clone(), "dropped fields") {
        entry = entry
            .with_loss(loss)
            .expect("the loss list is under the history cap by construction");
    }
    module
        .add_history_entry(entry)
        .expect("room was checked and the history id is unique by construction");
    Ok(module)
}

/// Transform a module's multiconductor value to a balanced network while
/// carrying its common records forward. The retained source is severed because
/// its bytes no longer describe the transformed value.
///
/// # Errors
/// Returns the original module with the transformation refusal when the value
/// kind is wrong or the network cannot be transformed under `options`.
#[allow(clippy::result_large_err)]
pub fn module_to_balanced(
    module: powerio_core::PioModule<crate::PioValue>,
    options: MulticonductorToBalancedOptions,
) -> Result<
    powerio_core::PioModule<crate::PioValue>,
    (
        powerio_core::PioModule<crate::PioValue>,
        Box<MulticonductorToBalancedError>,
    ),
> {
    lower_module_to_balanced(module, options)
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
            value.kind().as_str()
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
        if net.switches().iter().any(|sw| sw.open) {
            record
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
            record.assumptions.push(format!(
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
            record,
        }
    }

    #[allow(clippy::too_many_lines)]
    fn lower(&mut self) -> Result<MulticonductorToBalancedLowering, MulticonductorToBalancedError> {
        let Some(base) = self.voltage_base()? else {
            return Err(MulticonductorToBalancedError::new(
                self.options,
                &self.record.diagnostics,
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
            self.record.diagnostics.push(StructuredDiagnostic::of(
                &codes::TRANSFORM_MULTI_TO_BALANCED_INVALID_BALANCED_OUTPUT,
                format!("lowered balanced network failed structural validation: {err}"),
            ));
            return Err(MulticonductorToBalancedError::new(
                self.options,
                &self.record.diagnostics,
            ));
        }
        for finding in network.validate_values() {
            let details = finding.details();
            self.record.diagnostics.push(
                StructuredDiagnostic::of(
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

        self.record.validation_status = status_from_diagnostics(&self.record.diagnostics);
        Ok(MulticonductorToBalancedLowering {
            network,
            record: self.record.clone(),
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
                self.record.dropped_fields.push(format!(
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
                self.record.diagnostics.push(
                    StructuredDiagnostic::of(
                        &codes::TRANSFORM_MULTI_TO_BALANCED_UNKNOWN_SOURCE_BUS,
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
                    StructuredDiagnostic::of(
    &codes::TRANSFORM_MULTI_TO_BALANCED_INVALID_PHASE_REFERENCE,
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
                    StructuredDiagnostic::of(
    &codes::TRANSFORM_MULTI_TO_BALANCED_INVALID_PHASE_REFERENCE,
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
                &self.record.diagnostics,
            ));
        }
        self.record.diagnostics.push(StructuredDiagnostic::of(
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
                self.record.dropped_fields.push(format!(
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
                self.record.dropped_fields.push(format!(
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
            self.record.dropped_fields.push(format!(
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
            self.record.dropped_fields.push(format!(
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
                self.record.diagnostics.push(
                    StructuredDiagnostic::of(
                        &codes::TRANSFORM_MULTI_TO_BALANCED_UNKNOWN_LINECODE,
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
                    StructuredDiagnostic::of(
    &codes::TRANSFORM_MULTI_TO_BALANCED_PHASE_MAP_MISMATCH,
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
            self.record.approximations.push(format!(
                "linecode {code_name} {label} has sequence coupling norm {coupling}; positive-sequence diagonal retained"
            ));
            let mut diagnostic = StructuredDiagnostic::of(
    &codes::TRANSFORM_MULTI_TO_BALANCED_SEQUENCE_COUPLING_DROPPED,
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
        let mut diagnostics = self.record.diagnostics.clone();
        diagnostics.push(
            StructuredDiagnostic::of(
    &codes::TRANSFORM_MULTI_TO_BALANCED_NONFINITE_LINE_LENGTH,
format!("line {line_idx} has no finite length ({length}), so its impedance cannot be scaled"),
            )
            .with_element_path(format!("/model/multiconductor_network/lines/{line_idx}/length"))
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
        let mut diagnostics = self.record.diagnostics.clone();
        diagnostics.push(
            StructuredDiagnostic::of(
                &codes::TRANSFORM_MULTI_TO_BALANCED_INVALID_LINECODE_MATRIX,
                format!("linecode {code_name} {label} cannot be lowered: {message}"),
            )
            .with_element_path(format!(
                "/model/multiconductor_network/lines/{line_idx}/linecode"
            )),
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
            self.record.assumptions.push(format!(
                "transformer {} lowered as a balanced branch with tap {tap:.6} and the ANSI \
                 {shift} degree connection shift (high voltage side leads)",
                transformer.name
            ));
            if (low_rating_scale - 1.0).abs() > 1e-9 {
                self.record.assumptions.push(format!(
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
                self.record.dropped_fields.push(format!(
                    "transformer {} neutral grounding impedance dropped",
                    transformer.name
                ));
            }
            if transformer.xsc_pct.len() > 1 {
                self.record.dropped_fields.push(format!(
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
                    self.record.dropped_fields.push(format!(
                        "load {} voltage model dropped; balanced load is constant power",
                        load.name
                    ));
                    self.record.diagnostics.push(
                        StructuredDiagnostic::of(
    &codes::TRANSFORM_MULTI_TO_BALANCED_DROPPED_LOAD_VOLTAGE_MODEL,
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
                self.record.approximations.push(format!(
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
        let mut diagnostics = self.record.diagnostics.clone();
        diagnostics.push(
            StructuredDiagnostic::of(
                &codes::TRANSFORM_MULTI_TO_BALANCED_INVALID_SHUNT_MATRIX,
                format!("shunt {name} cannot be lowered: {message}"),
            )
            .with_element_path(format!("/model/multiconductor_network/shunts/{shunt_idx}")),
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
                if generator.s_max.is_some() || generator.i_max.is_some() {
                    self.record.dropped_fields.push(format!(
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
        self.record.diagnostics.push(
            StructuredDiagnostic::of(
                &codes::TRANSFORM_MULTI_TO_BALANCED_UNKNOWN_BUS,
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
                &self.record.diagnostics,
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
        report.diagnostics.push(StructuredDiagnostic::of(
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
    report: &mut MulticonductorToBalancedReadiness,
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
                StructuredDiagnostic::of(
    &codes::TRANSFORM_MULTI_TO_BALANCED_AMBIGUOUS_TERMINAL_MAP,
format!(
                        "bus {} has two active terminals; no unique positive sequence projection is defined",
                        bus.id
                    ),
                )
                .with_element_path(format!("/model/multiconductor_network/buses/{i}/terminals")),
            ),
            0 | 1 => report.diagnostics.push(
                StructuredDiagnostic::of(
    &codes::TRANSFORM_MULTI_TO_BALANCED_UNSUPPORTED_CONDUCTOR_SET,
format!(
                        "bus {} has {active_count} active terminal; multiconductor to balanced lowering starts with three phase input",
                        bus.id
                    ),
                )
                .with_element_path(format!("/model/multiconductor_network/buses/{i}/terminals")),
            ),
            _ => report.diagnostics.push(
                StructuredDiagnostic::of(
    &codes::TRANSFORM_MULTI_TO_BALANCED_UNSUPPORTED_CONDUCTOR_SET,
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
        report.diagnostics.push(StructuredDiagnostic::of(
            &codes::TRANSFORM_MULTI_TO_BALANCED_KRON_REDUCTION_REQUIRED,
            "neutral conductors require Kron reduction before the sequence transform",
        ));
    }
}

fn check_line_terminal_maps(
    net: &MulticonductorNetwork,
    report: &mut MulticonductorToBalancedReadiness,
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
                    StructuredDiagnostic::of(
    &codes::TRANSFORM_MULTI_TO_BALANCED_UNSUPPORTED_CONDUCTOR_SET,
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
    for (i, line) in net.lines().iter().enumerate() {
        let Some(code) = net.linecode(&line.linecode) else {
            report.diagnostics.push(
                StructuredDiagnostic::of(
                    &codes::TRANSFORM_MULTI_TO_BALANCED_UNKNOWN_LINECODE,
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
                StructuredDiagnostic::of(
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
                StructuredDiagnostic::of(
                    &codes::TRANSFORM_MULTI_TO_BALANCED_INVALID_LINECODE_MATRIX,
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
    let neutral_terminals = global_neutral_terminals(net);
    for (i, sw) in net.switches().iter().enumerate() {
        if sw.open {
            report.diagnostics.push(
                StructuredDiagnostic::of(
                    &codes::TRANSFORM_MULTI_TO_BALANCED_DROPPED_OPEN_SWITCH,
                    format!(
                        "open switch {} is dropped by multiconductor to balanced lowering",
                        sw.name
                    ),
                )
                .with_element_path(format!("/model/multiconductor_network/switches/{i}")),
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
) -> Vec<StructuredDiagnostic> {
    let path = format!("/model/multiconductor_network/switches/{index}");
    let mut blockers = Vec::new();
    if sw
        .i_max
        .as_ref()
        .is_some_and(|limits| limits.iter().any(|limit| limit.is_finite()))
    {
        blockers.push(
            StructuredDiagnostic::of(
                &codes::TRANSFORM_MULTI_TO_BALANCED_RATED_CLOSED_SWITCH,
                format!(
                    "closed switch {} carries a finite ampacity; merging its buses would \
                     remove the branch flow the limit constrains",
                    sw.name
                ),
            )
            .with_element_path(path.clone()),
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
            StructuredDiagnostic::of(
                &codes::TRANSFORM_MULTI_TO_BALANCED_UNKNOWN_BUS,
                format!("switch {} references unknown bus {missing}", sw.name),
            )
            .with_element_path(path.clone()),
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
            StructuredDiagnostic::of(
                &codes::TRANSFORM_MULTI_TO_BALANCED_SWITCH_TERMINAL_MISMATCH,
                format!(
                    "closed switch {} does not map identical conductors on both ends, so its \
                     buses are not electrically identical",
                    sw.name
                ),
            )
            .with_element_path(path.clone()),
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
                StructuredDiagnostic::of(
                    &codes::TRANSFORM_MULTI_TO_BALANCED_SWITCH_MERGE_CONFLICT,
                    format!(
                        "closed switch {} joins two buses that both carry voltage source \
                         references",
                        sw.name
                    ),
                )
                .with_element_path(path.clone()),
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
                    StructuredDiagnostic::of(
                        &codes::TRANSFORM_MULTI_TO_BALANCED_SWITCH_MERGE_CONFLICT,
                        format!(
                            "closed switch {} joins buses stating different {label} bounds \
                             ({a} and {b})",
                            sw.name
                        ),
                    )
                    .with_element_path(path.clone()),
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

fn check_phase_reference(
    net: &MulticonductorNetwork,
    report: &mut MulticonductorToBalancedReadiness,
) {
    let neutral_terminals = global_neutral_terminals(net);
    let has_three_phase_source = net.sources().iter().any(|source| {
        let bus = net.bus(&source.bus);
        active_terminal_count(&source.terminal_map, bus, &neutral_terminals) == 3
    });

    if !has_three_phase_source {
        report.diagnostics.push(StructuredDiagnostic::of(
            &codes::TRANSFORM_MULTI_TO_BALANCED_MISSING_PHASE_REFERENCE,
            "multiconductor to balanced lowering requires a three phase voltage source reference",
        ));
    }
}

fn check_transformers(net: &MulticonductorNetwork, report: &mut MulticonductorToBalancedReadiness) {
    let neutral_terminals = global_neutral_terminals(net);
    for (i, transformer) in net.transformers().iter().enumerate() {
        if let Err(reason) = classify_transformer(net, transformer, &neutral_terminals) {
            report.diagnostics.push(
                StructuredDiagnostic::of(
                    &codes::TRANSFORM_MULTI_TO_BALANCED_UNSUPPORTED_TRANSFORMER,
                    format!("transformer {} {reason}", transformer.name),
                )
                .with_element_path(format!("/model/multiconductor_network/transformers/{i}")),
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

fn check_untyped_objects(
    net: &MulticonductorNetwork,
    report: &mut MulticonductorToBalancedReadiness,
) {
    for (i, obj) in net.untyped().iter().enumerate() {
        report.diagnostics.push(
            StructuredDiagnostic::of(
                &codes::TRANSFORM_MULTI_TO_BALANCED_UNSUPPORTED_OBJECT,
                format!(
                    "{} {} is preserved as an untyped object and cannot be lowered",
                    obj.class, obj.name
                ),
            )
            .with_element_path(format!("/model/multiconductor_network/untyped/{i}")),
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
                        "lower_multiconductor_to_balanced",
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
