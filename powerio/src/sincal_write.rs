//! Explicit, source-free experimental SINCAL emission. Electrical validation
//! stays with the owning network family; only packaging is shared here.
use std::collections::BTreeMap;

use powerio_core::{
    ArtifactPath, Destination, Diagnostic, EmitResult, Error, Fidelity, MemoryArtifact, PioModule,
};

use crate::PioValue;

/// Container for fresh experimental SINCAL input data. The destination filename
/// does not select a container or change the electrical family.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub enum SincalContainer {
    /// One schema-14.8 SQLite input database.
    #[default]
    Sqlite,
    /// Deterministic candidate `.sinx` packaging. This does not include an
    /// undocumented desktop `.sin` file, diagrams, results or an Access database.
    Archive {
        /// Portable project name: 1–128 ASCII letters, digits, `_` or `-`.
        project_name: String,
    },
}

/// Explicit opt-in to the experimental static SINCAL writer. Native SINCAL
/// open/save/calculation acceptance has not been established.
///
/// The module's typed network selects its owning writer. There is no implicit
/// balancing, conductor expansion, or fallback to the other electrical family.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct SincalExperimentalOptions {
    pub container: SincalContainer,
    /// Required for a multiconductor network: finite positive line-line volts
    /// for every typed bus. Eliminated transformer auxiliary buses span two
    /// levels; their entries may be omitted and are ignored if supplied.
    /// Must be empty for a balanced network, which already has nominal kV.
    pub nominal_ll_volts: BTreeMap<String, f64>,
}

pub(crate) fn emit(
    module: &PioModule<PioValue>,
    format: &str,
    options: &SincalExperimentalOptions,
    destination: Destination,
) -> Result<EmitResult, Error> {
    let selected = crate::resolve_format(format).map(|f| f.token);
    if !matches!(
        selected,
        Some("sincal" | "sincal-balanced" | "sincal-multiconductor")
    ) {
        return Err(invalid(
            "experimental SINCAL options require a SINCAL target",
        ));
    }
    let (database, mut diagnostics) = match module.value() {
        PioValue::BalancedNetwork(net) => {
            if selected == Some("sincal-multiconductor") {
                return Err(invalid(
                    "sincal-multiconductor cannot write a BalancedNetwork; use sincal-balanced with experimental options",
                ));
            }
            if !options.nominal_ll_volts.is_empty() {
                return Err(invalid(
                    "balanced SINCAL output uses typed bus nominal kV; nominal_ll_volts is only a multiconductor option",
                ));
            }
            let output = powerio_tx::format::__write_sincal_balanced_experimental(net)?;
            (output.database, output.diagnostics)
        }
        PioValue::MulticonductorNetwork(net) => {
            if selected == Some("sincal-balanced") {
                return Err(invalid(
                    "sincal-balanced cannot write a MulticonductorNetwork; use sincal-multiconductor with experimental options to retain its conductor model",
                ));
            }
            let output = powerio_dist::__write_sincal_multiconductor_experimental(
                net,
                &powerio_dist::ExperimentalMulticonductorOptions {
                    nominal_ll_volts: options.nominal_ll_volts.clone(),
                },
            )?;
            (output.database, output.diagnostics)
        }
        value => {
            return Err(Error::new(
                &crate::codes::REQUEST_MODULE_WRONG_MODEL_KIND,
                format!(
                    "experimental SINCAL output requires a BalancedNetwork or MulticonductorNetwork, received {}",
                    value.type_name()
                ),
            ));
        }
    };
    if module.sources().iter().any(|s| {
        s.format().is_some_and(|f| {
            matches!(
                f.as_str(),
                "sincal" | "sincal-balanced" | "sincal-multiconductor"
            )
        })
    }) {
        diagnostics.push(Diagnostic::of(
            &powerio_tx::diagnostics::codes::EMIT_SINCAL_RETAINED_SOURCE_OMITTED,
            "Experimental fresh output uses the typed electrical value only. Retained native fault, dynamic, protection, diagram, result and other source-only records are omitted.",
        ));
    }
    let (name, bytes) = match &options.container {
        SincalContainer::Sqlite => ("case.db", database),
        SincalContainer::Archive { project_name } => (
            "case.sinx",
            powerio_sincal::authoring::candidate_archive(&database, project_name).map_err(|e| {
                Error::new(&crate::codes::EMIT_SINCAL_PACKAGING_FAILED, e.to_string()).with_cause(e)
            })?,
        ),
    };
    // All electrical checks and complete packaging finish in memory before
    // committing. Unsupported input cannot create or replace an output file.
    destination.__commit_artifacts(
        false,
        Fidelity::Canonical,
        vec![MemoryArtifact::new(ArtifactPath::new(name)?, bytes)],
        diagnostics,
    )
}

fn invalid(message: &str) -> Error {
    Error::new(&crate::codes::REQUEST_EMIT_INVALID_OPTIONS, message)
}
