//! Lowering records and preflight checks.
//!
//! Lowering is where PowerIO is a compiler rather than a parser: every pass that
//! transforms one model into another (normalization, multiconductor to balanced,
//! emission to a target format) appends a [`LoweringRecord`] to the package's
//! `lowering_history`, so the transformation is auditable. The most consequential
//! case, multiconductor to balanced, must be an explicit pass with diagnostics,
//! never a silent positive sequence projection.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use powerio_dist::{DistBus, MulticonductorNetwork};

use crate::diagnostics::{DiagnosticSeverity, DiagnosticStage, StructuredDiagnostic};
use crate::model::ModelKind;
use crate::validation::ValidationStatus;

/// One lowering/normalization/emission pass and what it changed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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

/// Options for the multiconductor to balanced lowering preflight.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MulticonductorToBalancedOptions {
    pub convention: SequenceTransformConvention,
}

impl Default for MulticonductorToBalancedOptions {
    fn default() -> Self {
        Self {
            convention: SequenceTransformConvention::FortescuePowerInvariant,
        }
    }
}

/// Readiness report for the future multiconductor to balanced lowering pass.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MulticonductorToBalancedReadiness {
    pub convention: SequenceTransformConvention,
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

/// Check whether a multiconductor package is ready for the future lowering pass.
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
        status: ValidationStatus::Ok,
        assumptions: vec![format!(
            "sequence transform convention: {}",
            options.convention
        )],
        approximations: Vec::new(),
        diagnostics: Vec::new(),
    };

    check_bus_conductor_sets(net, &mut report);
    check_phase_reference(net, &mut report);
    check_transformers(net, &mut report);
    check_untyped_objects(net, &mut report);

    report.status = status_from_diagnostics(&report.diagnostics);
    report
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
        .filter(|terminal| {
            !bus.is_some_and(|b| b.grounded.contains(*terminal))
                && !neutral_terminals.contains(*terminal)
        })
        .count()
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
