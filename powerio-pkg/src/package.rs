//! The `.pio.json` root object.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use powerio::{
    BalancedNetwork, BusId, NORMALIZED_SOLVER_TABLES_PASS, NormalizedSolverTables,
    SolverTableUnits, SourceDocument, SourceFormat,
};
use powerio_dist::{DistSourceFormat, MulticonductorNetwork};

use crate::diagnostics::{DiagnosticSeverity, DiagnosticStage, StructuredDiagnostic};
use crate::lowering::{
    LoweringRecord, MulticonductorToBalancedError, MulticonductorToBalancedOptions,
    MulticonductorToBalancedReadiness, check_multiconductor_to_balanced_lowering,
    lower_multiconductor_to_balanced,
};
use crate::model::{ModelKind, ModelPayload};
use crate::operating::{
    OperatingPointSeries, apply_operating_point_to_model, check_series_identities,
    operating_points_drop_code, operating_points_from_document,
};
use crate::provenance::{
    Confidence, MappingKind, Origin, Producer, SourceDescriptor, SourceMapEntry, SourceRef,
};
use crate::study::{StudyBlock, apply_study_to_model, check_study_identities};
use crate::summary::{ObjectSummary, ObjectTopology, ObjectUnits};
use crate::validation::{ValidationPass, ValidationStatus, ValidationSummary};

/// The canonical schema URL for this package version.
pub const PIO_PACKAGE_SCHEMA_URL: &str = "https://powerio.dev/schema/pio-package/0.1";

/// The package schema version (semver). Keep additive optional fields within
/// the current version when older readers can ignore them; field moves bump the
/// major (or ship a migration pass).
pub const PIO_PACKAGE_SCHEMA_VERSION: &str = "0.1.1";

/// The declared schema URL for the balanced payload (`model.balanced_network`).
///
/// Payload schema URLs are identifiers, not fetch locations (the same
/// convention as JSON Schema `$id`). They name the model JSON shape inside a
/// `.pio.json` document, not a standalone case format.
pub const PIO_PAYLOAD_BALANCED_SCHEMA_URL: &str =
    "https://powerio.dev/schema/pio-payload-balanced/1";

/// The balanced payload schema version (semver). Additive optional fields bump
/// the minor; field moves or removals bump the major. Versioned independently
/// of the envelope: [`PIO_PACKAGE_SCHEMA_VERSION`] covers the package
/// bookkeeping, this covers the network tables a consumer computes on.
pub const PIO_PAYLOAD_BALANCED_SCHEMA_VERSION: &str = "1.1.0";

/// The declared schema URL for the multiconductor payload
/// (`model.multiconductor_network`).
pub const PIO_PAYLOAD_MULTICONDUCTOR_SCHEMA_URL: &str =
    "https://powerio.dev/schema/pio-payload-multiconductor/1";

/// The multiconductor payload schema version (semver); the same policy as
/// [`PIO_PAYLOAD_BALANCED_SCHEMA_VERSION`].
pub const PIO_PAYLOAD_MULTICONDUCTOR_SCHEMA_VERSION: &str = "1.1.0";

pub const READ_TRANSMISSION_PARSE_WARNING: &str = "READ.TRANSMISSION.PARSE_WARNING";
pub const READ_GRIDFM_FIDELITY_WARNING: &str = "READ.GRIDFM.FIDELITY_WARNING";

fn default_schema_url() -> String {
    PIO_PACKAGE_SCHEMA_URL.to_owned()
}

fn default_schema_version() -> String {
    PIO_PACKAGE_SCHEMA_VERSION.to_owned()
}

/// The declared payload schema URL and version for a model kind.
fn payload_schema_for(kind: ModelKind) -> (&'static str, &'static str) {
    match kind {
        ModelKind::Balanced => (
            PIO_PAYLOAD_BALANCED_SCHEMA_URL,
            PIO_PAYLOAD_BALANCED_SCHEMA_VERSION,
        ),
        ModelKind::Multiconductor => (
            PIO_PAYLOAD_MULTICONDUCTOR_SCHEMA_URL,
            PIO_PAYLOAD_MULTICONDUCTOR_SCHEMA_VERSION,
        ),
    }
}

/// Optional derived metadata: matrix statistics, solver table metadata, and
/// cache keys.
/// Empty by default; the scaffold never populates it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DerivedMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matrix_stats: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub normalized_solver_tables: Option<NormalizedSolverTableMetadata>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub cache_keys: BTreeMap<String, String>,
}

impl DerivedMetadata {
    fn is_empty(&self) -> bool {
        self.matrix_stats.is_none()
            && self.normalized_solver_tables.is_none()
            && self.cache_keys.is_empty()
    }
}

/// Compact package metadata for `Network::to_normalized_solver_tables`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct NormalizedSolverTableMetadata {
    pub pass: String,
    pub units: SolverTableUnits,
    pub row_counts: NormalizedSolverTableRowCounts,
    pub bus_ids: Vec<BusId>,
    pub reference_bus_indices: Vec<usize>,
    pub component_labels: Vec<usize>,
    pub branch_from_arc_indices: Vec<usize>,
    pub branch_to_arc_indices: Vec<usize>,
    pub source_rows: NormalizedSolverTableSourceRows,
}

/// Row counts for every normalized solver table.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct NormalizedSolverTableRowCounts {
    pub buses: usize,
    pub loads: usize,
    pub shunts: usize,
    pub branches: usize,
    pub switches: usize,
    pub arcs: usize,
    pub generators: usize,
    pub storage: usize,
    pub hvdc: usize,
}

/// Source row provenance vectors for normalized solver tables.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct NormalizedSolverTableSourceRows {
    pub buses: Vec<Option<usize>>,
    pub loads: Vec<Option<usize>>,
    pub shunts: Vec<Option<usize>>,
    pub branches: Vec<Option<usize>>,
    pub switches: Vec<Option<usize>>,
    pub generators: Vec<Option<usize>>,
    pub storage: Vec<Option<usize>>,
    pub hvdc: Vec<Option<usize>>,
}

impl From<&NormalizedSolverTables> for NormalizedSolverTableMetadata {
    fn from(tables: &NormalizedSolverTables) -> Self {
        Self {
            pass: NORMALIZED_SOLVER_TABLES_PASS.to_owned(),
            units: tables.units.clone(),
            row_counts: NormalizedSolverTableRowCounts {
                buses: tables.buses.len(),
                loads: tables.loads.len(),
                shunts: tables.shunts.len(),
                branches: tables.branches.len(),
                switches: tables.switches.len(),
                arcs: tables.arcs.len(),
                generators: tables.generators.len(),
                storage: tables.storage.len(),
                hvdc: tables.hvdc.len(),
            },
            bus_ids: tables.index.bus_ids.clone(),
            reference_bus_indices: tables.index.reference_bus_indices.clone(),
            component_labels: tables.index.component_labels.clone(),
            branch_from_arc_indices: tables.index.branch_from_arc_indices.clone(),
            branch_to_arc_indices: tables.index.branch_to_arc_indices.clone(),
            source_rows: NormalizedSolverTableSourceRows {
                buses: tables.index.bus_source_rows.clone(),
                loads: tables.index.load_source_rows.clone(),
                shunts: tables.index.shunt_source_rows.clone(),
                branches: tables.index.branch_source_rows.clone(),
                switches: tables.index.switch_source_rows.clone(),
                generators: tables.index.generator_source_rows.clone(),
                storage: tables.index.storage_source_rows.clone(),
                hvdc: tables.index.hvdc_source_rows.clone(),
            },
        }
    }
}

/// A versioned package containing one model payload, provenance, diagnostics,
/// validation results, and lowering history. Serializes to `.pio.json`.
///
/// `model_kind` is stored explicitly and is authoritative; the payload is also
/// self-describing (tagged by `kind`). [`NetworkPackage::kind_is_consistent`]
/// asserts the two agree. The reader ignores unknown top level fields from a
/// newer producer.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct NetworkPackage {
    /// The schema URL identifying this package format.
    #[serde(default = "default_schema_url")]
    pub schema: String,
    /// The package schema version (semver).
    #[serde(default = "default_schema_version")]
    pub schema_version: String,
    pub producer: Producer,
    /// Stable content id, e.g. `"sha256:..."`. The scaffold leaves it `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_id: Option<String>,
    /// RFC 3339 build timestamp. Left `None` by default for deterministic,
    /// round-trip-stable output; set explicitly when a timestamp is wanted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    /// Explicit model kind. Authoritative; never inferred from field presence.
    pub model_kind: ModelKind,
    /// The declared schema URL for the payload family named by `model_kind`.
    /// `None` on packages written before the payload schema was declared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_schema: Option<String>,
    /// The declared payload schema version (semver), independent of the
    /// envelope `schema_version`: the envelope versions the package
    /// bookkeeping, this versions the network tables. A reader rejects a
    /// different major before computing on payload fields; `None` (legacy
    /// packages) is accepted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_schema_version: Option<String>,
    pub model: ModelPayload,
    /// Replayable operating states over the static payload. The package
    /// constructors and setters omit empty series for static single state cases.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operating_points: Option<OperatingPointSeries>,
    /// Cumulative interactive edits over the package payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub study: Option<StudyBlock>,
    pub origin: Origin,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<SourceDescriptor>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_maps: Vec<SourceMapEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<StructuredDiagnostic>,
    pub validation: ValidationSummary,
    #[serde(default)]
    pub summary: ObjectSummary,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lowering_history: Vec<LoweringRecord>,
    #[serde(default, skip_serializing_if = "DerivedMetadata::is_empty")]
    pub derived: DerivedMetadata,
}

impl NetworkPackage {
    /// Wrap a balanced network. Origin is inferred from its source format:
    /// `InMemory` / `Derived` (normalized) / `File` (a parsed text format,
    /// recording whether source was retained; the path is not captured here).
    pub fn from_balanced(net: BalancedNetwork) -> Self {
        let mut net = net;
        ensure_payload_uids(&mut net);
        let origin = balanced_origin(&net);
        let summary = balanced_summary(&net);
        let sources = balanced_sources(&net);
        let source_id = sources.first().map(|s| s.id.clone());
        let source_maps = balanced_source_maps(&net, source_id.as_deref());
        let diagnostics = Vec::new();
        let validation = ValidationSummary::from_diagnostics(&diagnostics);
        let (payload_schema, payload_schema_version) = payload_schema_for(ModelKind::Balanced);
        Self {
            schema: default_schema_url(),
            schema_version: default_schema_version(),
            producer: Producer::powerio(),
            package_id: None,
            created_at: None,
            model_kind: ModelKind::Balanced,
            payload_schema: Some(payload_schema.to_owned()),
            payload_schema_version: Some(payload_schema_version.to_owned()),
            model: ModelPayload::balanced(net),
            operating_points: None,
            study: None,
            origin,
            sources,
            source_maps,
            diagnostics,
            validation,
            summary,
            lowering_history: Vec::new(),
            derived: DerivedMetadata::default(),
        }
    }

    /// Wrap the result of a balanced case reader. Reader adapters can attach
    /// source data that is not part of the balanced network model; an
    /// operating point series derives from the reader's own parse, handed
    /// forward as [`powerio::Parsed::document`].
    pub fn from_parsed_balanced(parsed: powerio::Parsed) -> Self {
        let mut package = Self::from_balanced_with_read_warnings(
            parsed.network,
            READ_TRANSMISSION_PARSE_WARNING,
            parsed.warnings,
        );
        if let Some(document) = &parsed.document {
            package.attach_operating_points(document);
        }
        package
    }

    fn attach_operating_points(&mut self, document: &SourceDocument) {
        match operating_points_from_document(document) {
            Ok(series) => self.operating_points = series,
            Err(error) => {
                self.diagnostics.push(StructuredDiagnostic::new(
                    operating_points_drop_code(document),
                    DiagnosticSeverity::Warning,
                    DiagnosticStage::Read,
                    format!(
                        "time series could not be lifted into operating points; \
                         the package is static only: {error}"
                    ),
                ));
                self.validation = ValidationSummary::from_diagnostics(&self.diagnostics);
            }
        }
    }

    /// Wrap a balanced network and lift reader warnings into structured
    /// diagnostics under `code`.
    pub fn from_balanced_with_read_warnings<I, S>(
        net: BalancedNetwork,
        code: &str,
        warnings: I,
    ) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut package = Self::from_balanced(net);
        package.record_read_warnings(code, warnings);
        package
    }

    /// Append reader warnings to package diagnostics.
    pub fn record_read_warnings<I, S>(&mut self, code: &str, warnings: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let diagnostics: Vec<StructuredDiagnostic> = warnings
            .into_iter()
            .map(|w| {
                StructuredDiagnostic::new(
                    code,
                    DiagnosticSeverity::Warning,
                    DiagnosticStage::Read,
                    w.into(),
                )
            })
            .collect();
        if diagnostics.is_empty() {
            return;
        }
        self.diagnostics.extend(diagnostics);
        self.validation = ValidationSummary::from_diagnostics(&self.diagnostics);
    }

    /// Wrap a multiconductor network. Parse `warnings` are lifted into structured
    /// diagnostics, and `defaulted` fields are lifted into source maps with
    /// `mapping_kind = defaulted`, so the package surfaces that provenance even
    /// though those parser-side fields are not part of the IR payload.
    pub fn from_multiconductor(net: MulticonductorNetwork) -> Self {
        let summary = multiconductor_summary(&net);
        let sources = multiconductor_sources(&net);
        let source_id = sources.first().map(|s| s.id.clone());
        let source_maps = multiconductor_source_maps(&net, source_id.as_deref());
        let origin = multiconductor_origin(&net);

        let diagnostics: Vec<StructuredDiagnostic> = net
            .warnings
            .iter()
            .map(|w| {
                StructuredDiagnostic::new(
                    "READ.DIST.PARSE_WARNING",
                    DiagnosticSeverity::Warning,
                    DiagnosticStage::Read,
                    w.clone(),
                )
            })
            .collect();
        let validation = ValidationSummary::from_diagnostics(&diagnostics);

        let (payload_schema, payload_schema_version) =
            payload_schema_for(ModelKind::Multiconductor);
        Self {
            schema: default_schema_url(),
            schema_version: default_schema_version(),
            producer: Producer::powerio(),
            package_id: None,
            created_at: None,
            model_kind: ModelKind::Multiconductor,
            payload_schema: Some(payload_schema.to_owned()),
            payload_schema_version: Some(payload_schema_version.to_owned()),
            model: ModelPayload::multiconductor(net),
            operating_points: None,
            study: None,
            origin,
            sources,
            source_maps,
            diagnostics,
            validation,
            summary,
            lowering_history: Vec::new(),
            derived: DerivedMetadata::default(),
        }
    }

    /// The explicit model kind.
    pub fn model_kind(&self) -> ModelKind {
        self.model_kind
    }

    /// Whether the explicit `model_kind` agrees with the payload variant. A
    /// reader should reject a package where this is false.
    pub fn kind_is_consistent(&self) -> bool {
        self.model_kind == self.model.kind()
    }

    /// The balanced payload, if this package carries one.
    pub fn as_balanced(&self) -> Option<&BalancedNetwork> {
        self.model.as_balanced()
    }

    /// The multiconductor payload, if this package carries one.
    pub fn as_multiconductor(&self) -> Option<&MulticonductorNetwork> {
        self.model.as_multiconductor()
    }

    /// Replayable operating states over the static payload, when present.
    #[must_use]
    pub fn operating_points(&self) -> Option<&OperatingPointSeries> {
        self.operating_points.as_ref()
    }

    /// Attach a format neutral operating point series to this package.
    #[must_use]
    pub fn with_operating_points(mut self, operating_points: OperatingPointSeries) -> Self {
        self.set_operating_points(operating_points);
        self
    }

    /// Attach or replace operating points in place. Empty series are omitted.
    pub fn set_operating_points(&mut self, operating_points: OperatingPointSeries) {
        self.operating_points = (!operating_points.is_empty()).then_some(operating_points);
    }

    /// Remove operating points from this package.
    pub fn clear_operating_points(&mut self) {
        self.operating_points = None;
    }

    /// Cumulative study edits over the static payload, when present.
    #[must_use]
    pub fn study(&self) -> Option<&StudyBlock> {
        self.study.as_ref()
    }

    /// Attach a study block to this package. Empty blocks are omitted.
    #[must_use]
    pub fn with_study(mut self, study: StudyBlock) -> Self {
        self.set_study(study);
        self
    }

    /// Attach or replace the study block in place. Empty blocks are omitted.
    pub fn set_study(&mut self, study: StudyBlock) {
        self.study = (!study.is_empty()).then_some(study);
    }

    /// Remove the study block from this package.
    pub fn clear_study(&mut self) {
        self.study = None;
    }

    /// Materialize one operating point into a static package.
    ///
    /// The returned package has the same metadata and model kind, with its
    /// payload updated for `index`, `operating_points` cleared, and sane
    /// validation recomputed for the updated payload.
    pub fn materialize_operating_point(&self, index: usize) -> serde_json::Result<Self> {
        let series = self.operating_points.as_ref().ok_or_else(|| {
            <serde_json::Error as serde::de::Error>::custom("package has no operating points")
        })?;
        let point = series.unique_point(index)?.ok_or_else(|| {
            <serde_json::Error as serde::de::Error>::custom(format!(
                "package has no operating point {index}"
            ))
        })?;
        // Applying resolves each update's row (identity first, wire row as
        // fallback), so the stale provenance paths come from the same
        // resolution rather than the wire row values.
        let (updated_model, updated_paths) = apply_operating_point_to_model(&self.model, point)?;
        let had_normalized_solver_tables = self.derived.normalized_solver_tables.is_some();
        let options = materialize_operating_point_options(index);
        // Built field by field rather than cloned: cloning would deep copy the
        // whole payload only to overwrite it, and a future envelope field must
        // make an explicit carry-or-clear decision here instead of silently
        // riding along stale.
        let mut package = Self {
            schema: self.schema.clone(),
            schema_version: self.schema_version.clone(),
            producer: self.producer.clone(),
            // A derived package is new content: it records the parent's id in
            // its origin and never inherits it as its own (as in
            // `lower_multiconductor_to_balanced`).
            package_id: None,
            created_at: self.created_at.clone(),
            model_kind: self.model_kind,
            // The payload content derives from the parent, so the derived
            // package restates the parent's declared payload schema.
            payload_schema: self.payload_schema.clone(),
            payload_schema_version: self.payload_schema_version.clone(),
            model: updated_model,
            operating_points: None,
            study: None,
            origin: Origin::Derived {
                parent_package_id: self.package_id.clone(),
                pass: "materialize-operating-point".to_owned(),
                options: options.clone(),
            },
            sources: self.sources.clone(),
            source_maps: self
                .source_maps
                .iter()
                .filter(|entry| !updated_paths.contains(entry.element_path.as_str()))
                .cloned()
                .collect(),
            diagnostics: self
                .diagnostics
                .iter()
                .filter(|diagnostic| {
                    diagnostic
                        .element_path
                        .as_deref()
                        .is_none_or(|path| !updated_paths.contains(path))
                })
                .cloned()
                .collect(),
            // Replaced by run_sane_validation below.
            validation: self.validation.clone(),
            summary: self.summary.clone(),
            lowering_history: self.lowering_history.clone(),
            // Derived products are stale against the updated payload; solver
            // table metadata is rebuilt below when the parent carried it.
            derived: DerivedMetadata::default(),
        };
        let mut record = LoweringRecord::new(
            "materialize-operating-point",
            self.model_kind,
            self.model_kind,
        );
        record.options = options;
        package.run_sane_validation();
        record.validation_status = package.validation.status;
        package.push_lowering(record);
        if had_normalized_solver_tables {
            package
                .attach_normalized_solver_table_metadata()
                .map_err(|err| {
                    <serde_json::Error as serde::de::Error>::custom(format!(
                        "failed to recompute normalized solver table metadata: {err}"
                    ))
                })?;
        }
        Ok(package)
    }

    /// Materialize one operating point and return the balanced payload if this
    /// is a balanced package.
    pub fn materialize_balanced_operating_point(
        &self,
        index: usize,
    ) -> serde_json::Result<Option<BalancedNetwork>> {
        Ok(self
            .materialize_operating_point(index)?
            .model
            .as_balanced()
            .cloned())
    }

    /// Materialize one operating point and return the multiconductor payload if
    /// this is a multiconductor package.
    pub fn materialize_multiconductor_operating_point(
        &self,
        index: usize,
    ) -> serde_json::Result<Option<MulticonductorNetwork>> {
        Ok(self
            .materialize_operating_point(index)?
            .model
            .as_multiconductor()
            .cloned())
    }

    /// Materialize a study commit into a static package.
    ///
    /// The returned package folds commits `0..=commit_index`, clears
    /// `operating_points` and `study`, and records the replay pass in
    /// `lowering_history`.
    pub fn materialize_study_commit(&self, commit_index: usize) -> serde_json::Result<Self> {
        let study = self.study.as_ref().ok_or_else(|| {
            <serde_json::Error as serde::de::Error>::custom("package has no study block")
        })?;
        let base = if let Some(index) = study.base_operating_point {
            self.materialize_operating_point(index)?
        } else {
            self.clone()
        };
        let (updated_model, updated_paths) =
            apply_study_to_model(&base.model, study, commit_index)?;
        let had_normalized_solver_tables = base.derived.normalized_solver_tables.is_some();
        let options = materialize_study_commit_options(study, commit_index);

        let mut package = Self {
            schema: base.schema.clone(),
            schema_version: base.schema_version.clone(),
            producer: base.producer.clone(),
            package_id: None,
            created_at: base.created_at.clone(),
            model_kind: base.model_kind,
            payload_schema: base.payload_schema.clone(),
            payload_schema_version: base.payload_schema_version.clone(),
            model: updated_model,
            operating_points: None,
            study: None,
            origin: Origin::Derived {
                parent_package_id: self.package_id.clone(),
                pass: "materialize-study-commit".to_owned(),
                options: options.clone(),
            },
            sources: base.sources.clone(),
            source_maps: base
                .source_maps
                .iter()
                .filter(|entry| !updated_paths.contains(entry.element_path.as_str()))
                .cloned()
                .collect(),
            diagnostics: base
                .diagnostics
                .iter()
                .filter(|diagnostic| {
                    diagnostic
                        .element_path
                        .as_deref()
                        .is_none_or(|path| !updated_paths.contains(path))
                })
                .cloned()
                .collect(),
            validation: base.validation.clone(),
            summary: base.summary.clone(),
            lowering_history: base.lowering_history.clone(),
            derived: DerivedMetadata::default(),
        };
        let mut record =
            LoweringRecord::new("materialize-study-commit", base.model_kind, base.model_kind);
        record.options = options;
        record
            .assumptions
            .push(format!("applied study commits 0..={commit_index}"));
        package.run_sane_validation();
        record.validation_status = package.validation.status;
        package.push_lowering(record);
        if had_normalized_solver_tables {
            package
                .attach_normalized_solver_table_metadata()
                .map_err(|err| {
                    <serde_json::Error as serde::de::Error>::custom(format!(
                        "failed to recompute normalized solver table metadata: {err}"
                    ))
                })?;
        }
        Ok(package)
    }

    /// Materialize a study commit and return the balanced payload.
    pub fn materialize_balanced_study_commit(
        &self,
        commit_index: usize,
    ) -> serde_json::Result<Option<BalancedNetwork>> {
        Ok(self
            .materialize_study_commit(commit_index)?
            .model
            .as_balanced()
            .cloned())
    }

    /// Serialize to compact `.pio.json`.
    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string(self)
    }

    /// Serialize to pretty `.pio.json`.
    pub fn to_json_pretty(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }

    /// Deserialize from `.pio.json`.
    pub fn from_json(text: &str) -> serde_json::Result<Self> {
        let pkg: Self = serde_json::from_str(text)?;
        if !Self::supports_schema_version(&pkg.schema_version) {
            return Err(<serde_json::Error as serde::de::Error>::custom(format!(
                "unsupported .pio.json schema_version {}; this reader supports major version {}",
                pkg.schema_version,
                supported_schema_major()
            )));
        }
        if !pkg.kind_is_consistent() {
            return Err(<serde_json::Error as serde::de::Error>::custom(
                "model_kind does not match model.kind",
            ));
        }
        if let Some(version) = pkg.payload_schema_version.as_deref() {
            let supported = supported_payload_schema_major(pkg.model_kind);
            if schema_major(version) != Some(supported) {
                return Err(<serde_json::Error as serde::de::Error>::custom(format!(
                    "unsupported payload_schema_version {version}; this reader supports \
                     major version {supported} for {:?} payloads",
                    pkg.model_kind
                )));
            }
        }
        Ok(pkg)
    }

    /// Whether this reader accepts the envelope schema version.
    ///
    /// The `.pio.json` compatibility rule is envelope scoped: unknown
    /// future top-level fields are ignored, additive same major versions load,
    /// and a different major version is rejected before payload use.
    pub fn supports_schema_version(version: &str) -> bool {
        schema_major(version).is_some_and(|major| major == supported_schema_major())
    }

    #[must_use]
    pub fn with_origin(mut self, origin: Origin) -> Self {
        self.origin = origin;
        self
    }

    #[must_use]
    pub fn with_package_id(mut self, id: impl Into<String>) -> Self {
        self.package_id = Some(id.into());
        self
    }

    #[must_use]
    pub fn with_created_at(mut self, created_at: impl Into<String>) -> Self {
        self.created_at = Some(created_at.into());
        self
    }

    #[must_use]
    pub fn with_sources(mut self, sources: Vec<SourceDescriptor>) -> Self {
        self.sources = sources;
        self
    }

    #[must_use]
    pub fn with_source_maps(mut self, source_maps: Vec<SourceMapEntry>) -> Self {
        self.source_maps = source_maps;
        self
    }

    /// Append a lowering record to the history.
    pub fn push_lowering(&mut self, record: LoweringRecord) {
        self.lowering_history.push(record);
    }

    /// Attach compact metadata for the normalized dense solver table lowering.
    ///
    /// Returns `Ok(false)` for non-balanced packages. The full table rows stay
    /// outside the package payload; this records the pass name, row counts,
    /// units, dense identities, and source row provenance a compiler cache needs
    /// to validate external table artifacts.
    pub fn attach_normalized_solver_table_metadata(
        &mut self,
    ) -> std::result::Result<bool, powerio::Error> {
        let Some(net) = self.as_balanced() else {
            return Ok(false);
        };
        let tables = net.to_normalized_solver_tables()?;
        self.derived.normalized_solver_tables = Some(NormalizedSolverTableMetadata::from(&tables));
        Ok(true)
    }

    /// Return a package with normalized solver table metadata attached.
    pub fn with_normalized_solver_table_metadata(
        mut self,
    ) -> std::result::Result<Self, powerio::Error> {
        self.attach_normalized_solver_table_metadata()?;
        Ok(self)
    }

    /// Check whether this package's multiconductor payload is ready for the
    /// explicit multiconductor to balanced lowering pass.
    #[must_use]
    pub fn check_multiconductor_to_balanced_lowering(
        &self,
    ) -> Option<MulticonductorToBalancedReadiness> {
        self.as_multiconductor().map(|net| {
            check_multiconductor_to_balanced_lowering(
                net,
                MulticonductorToBalancedOptions::default(),
            )
        })
    }

    /// Explicitly lower a multiconductor package to a derived balanced package.
    ///
    /// This method only accepts packages whose payload is
    /// [`ModelKind::Multiconductor`]. It does not mutate the input package.
    pub fn lower_multiconductor_to_balanced(
        &self,
        options: MulticonductorToBalancedOptions,
    ) -> Result<Self, MulticonductorToBalancedError> {
        let Some(net) = self.as_multiconductor() else {
            let diagnostic = StructuredDiagnostic::new(
                "LOWER.MULTI_TO_BALANCED.WRONG_MODEL_KIND",
                DiagnosticSeverity::Error,
                DiagnosticStage::Lower,
                format!(
                    "multiconductor to balanced lowering requires a multiconductor package, got {:?}",
                    self.model_kind
                ),
            );
            return Err(MulticonductorToBalancedError::new(
                options,
                vec![diagnostic],
            ));
        };

        let lowered = lower_multiconductor_to_balanced(net, options)?;
        let mut record = lowered.record;
        let mut output = NetworkPackage::from_balanced(lowered.network);
        output.origin = Origin::Derived {
            parent_package_id: self.package_id.clone(),
            pass: "multiconductor-to-balanced".to_owned(),
            options: record.options.clone(),
        };
        output.sources = derived_sources(self);
        let source_id = output.sources.first().map(|source| source.id.as_str());
        output.source_maps = match output.as_balanced() {
            Some(balanced) => lowered_balanced_source_maps(net, balanced, source_id),
            None => Vec::new(),
        };
        output.diagnostics.clone_from(&record.diagnostics);
        output.lowering_history.clone_from(&self.lowering_history);
        output.run_sane_validation();
        record.validation_status = output.validation.status;
        output.push_lowering(record);
        Ok(output)
    }

    /// Run the package semantic validation profile and record its findings.
    ///
    /// This pass leaves the payload untouched: it reports structural and
    /// semantic issues, but never repairs or rewrites the model. It does rewrite
    /// the package's own `diagnostics` and `validation`, so it needs `&mut self`.
    pub fn run_sane_validation(&mut self) {
        self.diagnostics
            .retain(|d| !is_sane_validation_code(d.code.as_str()));

        let (mut diagnostics, mut passes) = match &self.model {
            ModelPayload::Balanced { balanced_network } => sane_validate_balanced(balanced_network),
            ModelPayload::Multiconductor {
                multiconductor_network,
            } => sane_validate_multiconductor(multiconductor_network),
        };

        if let Some(series) = &self.operating_points {
            let (identity_diagnostics, identity_pass) =
                validate_operating_identity(&self.model, series);
            diagnostics.extend(identity_diagnostics);
            passes.push(identity_pass);
        }
        if let Some(study) = &self.study {
            let (study_diagnostics, study_pass) = validate_study(&self.model, study);
            diagnostics.extend(study_diagnostics);
            passes.push(study_pass);
        }

        attach_source_refs(&mut diagnostics, &self.source_maps);
        self.diagnostics.extend(diagnostics);
        self.validation =
            ValidationSummary::from_diagnostics(&self.diagnostics).with_passes(passes);
    }
}

fn materialize_operating_point_options(index: usize) -> serde_json::Map<String, serde_json::Value> {
    let mut options = serde_json::Map::new();
    options.insert("index".to_owned(), serde_json::json!(index));
    options
}

fn materialize_study_commit_options(
    study: &StudyBlock,
    commit_index: usize,
) -> serde_json::Map<String, serde_json::Value> {
    let mut options = serde_json::Map::new();
    options.insert("commit_index".to_owned(), serde_json::json!(commit_index));
    if let Some(index) = study.base_operating_point {
        options.insert("base_operating_point".to_owned(), serde_json::json!(index));
    }
    options
}

fn schema_major(version: &str) -> Option<u64> {
    // Accept a semver core `MAJOR.MINOR.PATCH` with an optional prerelease
    // (`-...`) or build (`+...`) tag: same-major additive versions load, so a
    // forward-compatible writer that stamps e.g. `0.2.0-rc.1` is not rejected.
    let (core, suffix) = match version.split_once('-') {
        Some((core, rest)) => match rest.split_once('+') {
            Some((pre, build)) => (core, Some((Some(pre), Some(build)))),
            None => (core, Some((Some(rest), None))),
        },
        None => match version.split_once('+') {
            Some((core, build)) => (core, Some((None, Some(build)))),
            None => (version, None),
        },
    };
    if let Some((pre, build)) = suffix {
        if pre.is_some_and(|s| !valid_semver_suffix(s))
            || build.is_some_and(|s| !valid_semver_suffix(s))
        {
            return None;
        }
    }
    let mut parts = core.split('.');
    let major = parts.next()?;
    let minor = parts.next()?;
    let patch = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let major = parse_semver_number(major)?;
    parse_semver_number(minor)?;
    parse_semver_number(patch)?;
    Some(major)
}

fn parse_semver_number(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) || (s.len() > 1 && s.starts_with('0'))
    {
        return None;
    }
    s.parse().ok()
}

fn valid_semver_suffix(s: &str) -> bool {
    !s.is_empty()
        && s.split('.').all(|part| {
            !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

fn supported_schema_major() -> u64 {
    schema_major(PIO_PACKAGE_SCHEMA_VERSION).expect("package schema version has a major number")
}

fn supported_payload_schema_major(kind: ModelKind) -> u64 {
    schema_major(payload_schema_for(kind).1).expect("payload schema version has a major number")
}

/// Add a stable UID to each payload row that does not have one.
///
/// Source UIDs remain unchanged. Generated values use `{table}:{row}` and stay
/// attached to the element if rows are reordered. Operating point and study
/// references resolve against these values.
pub fn ensure_payload_uids(net: &mut BalancedNetwork) {
    macro_rules! fill {
        ($table:ident) => {
            for (row, element) in net.$table.iter_mut().enumerate() {
                if element.uid.is_none() {
                    element.uid = Some(format!(concat!(stringify!($table), ":{}"), row));
                }
            }
        };
    }
    fill!(buses);
    fill!(loads);
    fill!(shunts);
    fill!(branches);
    fill!(switches);
    fill!(generators);
    fill!(storage);
    fill!(hvdc);
    fill!(transformers_3w);
}

const SANE_VALIDATION_CODES: [&str; 9] = [
    "VALIDATE.BALANCED.STRUCTURE",
    "VALIDATE.BALANCED.VALUE_DOMAIN",
    "VALIDATE.MULTI.STRUCTURE",
    "VALIDATE.MULTI.TERMINAL_MAP",
    "VALIDATE.MULTI.UNTYPED_OBJECT",
    "VALIDATE.MULTI.NO_VOLTAGE_SOURCE",
    "VALIDATE.PACKAGE.OPERATING_IDENTITY",
    "VALIDATE.PACKAGE.STUDY_MODEL_KIND",
    "VALIDATE.PACKAGE.STUDY_IDENTITY",
];

/// Check every operating point update against the payload's identity index:
/// unknown `source_uid`, a wire `row` that contradicts the resolved row,
/// ambiguous (duplicate) payload uids, and rows out of range all become Error
/// diagnostics, so `pio_package_validate` rejects a package whose updates
/// reference unknown identities without materializing it.
fn validate_operating_identity(
    model: &ModelPayload,
    series: &OperatingPointSeries,
) -> (Vec<StructuredDiagnostic>, ValidationPass) {
    let diagnostics: Vec<StructuredDiagnostic> = check_series_identities(model, series)
        .into_iter()
        .map(|(point_pos, update_pos, message)| {
            StructuredDiagnostic::new(
                "VALIDATE.PACKAGE.OPERATING_IDENTITY",
                DiagnosticSeverity::Error,
                DiagnosticStage::Validate,
                message,
            )
            .with_element_path(format!(
                "/operating_points/points/{point_pos}/updates/{update_pos}"
            ))
        })
        .collect();
    let status = validation_status(&diagnostics);
    (
        diagnostics,
        ValidationPass::new("package.operating_identity", status),
    )
}

fn validate_study(
    model: &ModelPayload,
    study: &StudyBlock,
) -> (Vec<StructuredDiagnostic>, ValidationPass) {
    if !matches!(model, ModelPayload::Balanced { .. }) {
        let diagnostics = vec![
            StructuredDiagnostic::new(
                "VALIDATE.PACKAGE.STUDY_MODEL_KIND",
                DiagnosticSeverity::Error,
                DiagnosticStage::Validate,
                "study blocks are only defined for balanced packages",
            )
            .with_element_path("/study"),
        ];
        return (
            diagnostics,
            ValidationPass::new("package.study", ValidationStatus::Error),
        );
    }

    let diagnostics: Vec<StructuredDiagnostic> = check_study_identities(model, study)
        .into_iter()
        .map(|(commit_pos, edit_pos, message)| {
            StructuredDiagnostic::new(
                "VALIDATE.PACKAGE.STUDY_IDENTITY",
                DiagnosticSeverity::Error,
                DiagnosticStage::Validate,
                message,
            )
            .with_element_path(format!("/study/commits/{commit_pos}/edits/{edit_pos}"))
        })
        .collect();
    let status = validation_status(&diagnostics);
    (
        diagnostics,
        ValidationPass::new("package.study_identity", status),
    )
}

fn is_sane_validation_code(code: &str) -> bool {
    SANE_VALIDATION_CODES.contains(&code)
}

fn validation_status(diagnostics: &[StructuredDiagnostic]) -> ValidationStatus {
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

fn sane_validate_balanced(
    net: &BalancedNetwork,
) -> (Vec<StructuredDiagnostic>, Vec<ValidationPass>) {
    let mut structure = Vec::new();
    if let Err(err) = net.validate() {
        structure.push(StructuredDiagnostic::new(
            "VALIDATE.BALANCED.STRUCTURE",
            DiagnosticSeverity::Error,
            DiagnosticStage::Validate,
            err.to_string(),
        ));
    }

    let bus_index: HashMap<usize, usize> = net
        .buses
        .iter()
        .enumerate()
        .map(|(idx, b)| (b.id.0, idx))
        .collect();
    let mut value_domain = Vec::new();
    for finding in net.validate_values() {
        let element_path =
            balanced_value_finding_path(net, &bus_index, &finding).unwrap_or_else(|| {
                format!(
                    "/model/balanced_network/{}#{}",
                    finding.element.replace(' ', "_"),
                    finding.field
                )
            });
        let mut d = StructuredDiagnostic::new(
            "VALIDATE.BALANCED.VALUE_DOMAIN",
            DiagnosticSeverity::Warning,
            DiagnosticStage::Validate,
            format!(
                "{} field `{}` is outside its value domain; suggested value is {}",
                finding.element, finding.field, finding.new
            ),
        )
        .with_element_path(element_path)
        .with_suggested_action("Run the explicit repair pass if these defaults are desired.");
        d.details
            .insert("element".to_owned(), serde_json::json!(finding.element));
        d.details
            .insert("field".to_owned(), serde_json::json!(finding.field));
        d.details
            .insert("old".to_owned(), serde_json::json!(finding.old));
        d.details
            .insert("new".to_owned(), serde_json::json!(finding.new));
        d.details
            .insert("reason".to_owned(), serde_json::json!(finding.reason));
        value_domain.push(d);
    }

    let passes = vec![
        ValidationPass::new("balanced.structure", validation_status(&structure)),
        ValidationPass::new("balanced.value_domain", validation_status(&value_domain)),
    ];
    structure.extend(value_domain);
    (structure, passes)
}

fn attach_source_refs(diagnostics: &mut [StructuredDiagnostic], source_maps: &[SourceMapEntry]) {
    // Index by element path once: `source_maps` holds a row per field per
    // element, so a per-diagnostic linear scan is quadratic. First entry wins,
    // matching the previous `iter().find` order.
    let mut by_path: HashMap<&str, &SourceRef> = HashMap::with_capacity(source_maps.len());
    for map in source_maps {
        by_path
            .entry(map.element_path.as_str())
            .or_insert(&map.source_ref);
    }
    for diagnostic in diagnostics {
        if diagnostic.source_ref.is_some() {
            continue;
        }
        let Some(path) = diagnostic.element_path.as_deref() else {
            continue;
        };
        if let Some(source_ref) = by_path.get(path) {
            diagnostic.source_ref = Some((*source_ref).clone());
        }
    }
}

fn balanced_value_finding_path(
    net: &BalancedNetwork,
    bus_index: &HashMap<usize, usize>,
    finding: &powerio::Diagnostic,
) -> Option<String> {
    if let Some(id) = finding
        .element
        .strip_prefix("bus ")
        .and_then(|s| s.parse::<usize>().ok())
    {
        let idx = *bus_index.get(&id)?;
        return Some(format!(
            "/model/balanced_network/buses/{idx}/{}",
            finding.field
        ));
    }

    if let Some(id) = finding
        .element
        .strip_prefix("generator at bus ")
        .and_then(|s| s.parse::<usize>().ok())
    {
        // When several units at a bus share the same out-of-domain value the
        // finding cannot be pinned to one array index, so skip the precise path
        // rather than misattribute it (see the ambiguity test).
        let mut matches = net
            .generators
            .iter()
            .enumerate()
            .filter(|(_, g)| {
                g.bus.0 == id
                    && generator_field(g, finding.field)
                        .is_some_and(|v| v.to_bits() == finding.old.to_bits())
            })
            .map(|(idx, _)| idx);
        let idx = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        return Some(format!(
            "/model/balanced_network/generators/{idx}/{}",
            finding.field
        ));
    }

    None
}

fn generator_field(generator: &powerio::Generator, field: &str) -> Option<f64> {
    Some(match field {
        "mbase" => generator.mbase,
        "vg" => generator.vg,
        _ => return None,
    })
}

fn sane_validate_multiconductor(
    net: &MulticonductorNetwork,
) -> (Vec<StructuredDiagnostic>, Vec<ValidationPass>) {
    let mut structure = Vec::new();
    let mut terminal_maps = Vec::new();
    let mut untyped = Vec::new();
    let mut sources = Vec::new();

    let (bus_ids, bus_terminals) = multiconductor_bus_index(net, &mut structure);

    validate_multiconductor_lines(
        net,
        &bus_ids,
        &bus_terminals,
        &mut structure,
        &mut terminal_maps,
    );
    validate_multiconductor_switches(
        net,
        &bus_ids,
        &bus_terminals,
        &mut structure,
        &mut terminal_maps,
    );
    validate_multiconductor_transformers(
        net,
        &bus_ids,
        &bus_terminals,
        &mut structure,
        &mut terminal_maps,
    );
    validate_multiconductor_injections(
        net,
        &bus_ids,
        &bus_terminals,
        &mut structure,
        &mut terminal_maps,
    );

    for (i, obj) in net.untyped.iter().enumerate() {
        untyped.push(
            StructuredDiagnostic::new(
                "VALIDATE.MULTI.UNTYPED_OBJECT",
                DiagnosticSeverity::Warning,
                DiagnosticStage::Validate,
                format!(
                    "{} {} is preserved as an untyped object",
                    obj.class, obj.name
                ),
            )
            .with_element_path(format!("/model/multiconductor_network/untyped/{i}")),
        );
    }

    if net.sources.is_empty() {
        sources.push(StructuredDiagnostic::new(
            "VALIDATE.MULTI.NO_VOLTAGE_SOURCE",
            DiagnosticSeverity::Warning,
            DiagnosticStage::Validate,
            "multiconductor package has no voltage source",
        ));
    }

    let passes = vec![
        ValidationPass::new("multiconductor.structure", validation_status(&structure)),
        ValidationPass::new(
            "multiconductor.terminal_map",
            validation_status(&terminal_maps),
        ),
        ValidationPass::new("multiconductor.untyped_object", validation_status(&untyped)),
        ValidationPass::new("multiconductor.voltage_source", validation_status(&sources)),
    ];

    let mut diagnostics = structure;
    diagnostics.extend(terminal_maps);
    diagnostics.extend(untyped);
    diagnostics.extend(sources);
    (diagnostics, passes)
}

fn validate_multiconductor_lines(
    net: &MulticonductorNetwork,
    bus_ids: &BTreeSet<String>,
    bus_terminals: &BTreeMap<String, BTreeSet<String>>,
    structure: &mut Vec<StructuredDiagnostic>,
    terminal_maps: &mut Vec<StructuredDiagnostic>,
) {
    for (i, line) in net.lines.iter().enumerate() {
        check_bus_ref(
            &line.bus_from,
            &format!("line {} from bus", line.name),
            &format!("/model/multiconductor_network/lines/{i}/bus_from"),
            bus_ids,
            structure,
        );
        check_bus_ref(
            &line.bus_to,
            &format!("line {} to bus", line.name),
            &format!("/model/multiconductor_network/lines/{i}/bus_to"),
            bus_ids,
            structure,
        );
        if !net
            .linecodes
            .iter()
            .any(|c| c.name.eq_ignore_ascii_case(&line.linecode))
        {
            structure.push(
                StructuredDiagnostic::new(
                    "VALIDATE.MULTI.STRUCTURE",
                    DiagnosticSeverity::Error,
                    DiagnosticStage::Validate,
                    format!(
                        "line {} references unknown linecode `{}`",
                        line.name, line.linecode
                    ),
                )
                .with_element_path(format!("/model/multiconductor_network/lines/{i}/linecode")),
            );
        }
        check_terminal_map(
            &line.bus_from,
            &line.terminal_map_from,
            &format!("line {} from terminals", line.name),
            &format!("/model/multiconductor_network/lines/{i}/terminal_map_from"),
            bus_terminals,
            terminal_maps,
        );
        check_terminal_map(
            &line.bus_to,
            &line.terminal_map_to,
            &format!("line {} to terminals", line.name),
            &format!("/model/multiconductor_network/lines/{i}/terminal_map_to"),
            bus_terminals,
            terminal_maps,
        );
    }
}

fn validate_multiconductor_switches(
    net: &MulticonductorNetwork,
    bus_ids: &BTreeSet<String>,
    bus_terminals: &BTreeMap<String, BTreeSet<String>>,
    structure: &mut Vec<StructuredDiagnostic>,
    terminal_maps: &mut Vec<StructuredDiagnostic>,
) {
    for (i, sw) in net.switches.iter().enumerate() {
        check_bus_ref(
            &sw.bus_from,
            &format!("switch {} from bus", sw.name),
            &format!("/model/multiconductor_network/switches/{i}/bus_from"),
            bus_ids,
            structure,
        );
        check_bus_ref(
            &sw.bus_to,
            &format!("switch {} to bus", sw.name),
            &format!("/model/multiconductor_network/switches/{i}/bus_to"),
            bus_ids,
            structure,
        );
        check_terminal_map(
            &sw.bus_from,
            &sw.terminal_map_from,
            &format!("switch {} from terminals", sw.name),
            &format!("/model/multiconductor_network/switches/{i}/terminal_map_from"),
            bus_terminals,
            terminal_maps,
        );
        check_terminal_map(
            &sw.bus_to,
            &sw.terminal_map_to,
            &format!("switch {} to terminals", sw.name),
            &format!("/model/multiconductor_network/switches/{i}/terminal_map_to"),
            bus_terminals,
            terminal_maps,
        );
    }
}

fn validate_multiconductor_transformers(
    net: &MulticonductorNetwork,
    bus_ids: &BTreeSet<String>,
    bus_terminals: &BTreeMap<String, BTreeSet<String>>,
    structure: &mut Vec<StructuredDiagnostic>,
    terminal_maps: &mut Vec<StructuredDiagnostic>,
) {
    for (i, tx) in net.transformers.iter().enumerate() {
        for (j, winding) in tx.windings.iter().enumerate() {
            check_bus_ref(
                &winding.bus,
                &format!("transformer {} winding {j} bus", tx.name),
                &format!("/model/multiconductor_network/transformers/{i}/windings/{j}/bus"),
                bus_ids,
                structure,
            );
            check_terminal_map(
                &winding.bus,
                &winding.terminal_map,
                &format!("transformer {} winding {j} terminals", tx.name),
                &format!(
                    "/model/multiconductor_network/transformers/{i}/windings/{j}/terminal_map"
                ),
                bus_terminals,
                terminal_maps,
            );
        }
    }
}

fn validate_multiconductor_injections(
    net: &MulticonductorNetwork,
    bus_ids: &BTreeSet<String>,
    bus_terminals: &BTreeMap<String, BTreeSet<String>>,
    structure: &mut Vec<StructuredDiagnostic>,
    terminal_maps: &mut Vec<StructuredDiagnostic>,
) {
    let mut ctx = MultiValidationContext {
        bus_ids,
        bus_terminals,
        structure,
        terminal_maps,
    };
    for (i, load) in net.loads.iter().enumerate() {
        check_one_bus_element(
            &load.bus,
            &load.terminal_map,
            &format!("load {}", load.name),
            &format!("/model/multiconductor_network/loads/{i}"),
            &mut ctx,
        );
    }
    for (i, generator) in net.generators.iter().enumerate() {
        check_one_bus_element(
            &generator.bus,
            &generator.terminal_map,
            &format!("generator {}", generator.name),
            &format!("/model/multiconductor_network/generators/{i}"),
            &mut ctx,
        );
    }
    for (i, shunt) in net.shunts.iter().enumerate() {
        check_one_bus_element(
            &shunt.bus,
            &shunt.terminal_map,
            &format!("shunt {}", shunt.name),
            &format!("/model/multiconductor_network/shunts/{i}"),
            &mut ctx,
        );
    }
    for (i, source) in net.sources.iter().enumerate() {
        check_one_bus_element(
            &source.bus,
            &source.terminal_map,
            &format!("voltage source {}", source.name),
            &format!("/model/multiconductor_network/sources/{i}"),
            &mut ctx,
        );
    }
}

struct MultiValidationContext<'a> {
    bus_ids: &'a BTreeSet<String>,
    bus_terminals: &'a BTreeMap<String, BTreeSet<String>>,
    structure: &'a mut Vec<StructuredDiagnostic>,
    terminal_maps: &'a mut Vec<StructuredDiagnostic>,
}

fn check_one_bus_element(
    bus: &str,
    terminal_map: &[String],
    label: &str,
    path: &str,
    ctx: &mut MultiValidationContext<'_>,
) {
    check_bus_ref(
        bus,
        &format!("{label} bus"),
        &format!("{path}/bus"),
        ctx.bus_ids,
        ctx.structure,
    );
    check_terminal_map(
        bus,
        terminal_map,
        &format!("{label} terminals"),
        &format!("{path}/terminal_map"),
        ctx.bus_terminals,
        ctx.terminal_maps,
    );
}

fn multiconductor_bus_index(
    net: &MulticonductorNetwork,
    diagnostics: &mut Vec<StructuredDiagnostic>,
) -> (BTreeSet<String>, BTreeMap<String, BTreeSet<String>>) {
    let mut ids = BTreeSet::new();
    let mut terminals = BTreeMap::new();
    let mut first_seen = BTreeMap::<String, String>::new();
    for (i, bus) in net.buses.iter().enumerate() {
        let key = bus.id.to_ascii_lowercase();
        if let Some(first) = first_seen.insert(key.clone(), bus.id.clone()) {
            diagnostics.push(
                StructuredDiagnostic::new(
                    "VALIDATE.MULTI.STRUCTURE",
                    DiagnosticSeverity::Error,
                    DiagnosticStage::Validate,
                    format!("duplicate bus id `{}` conflicts with `{first}`", bus.id),
                )
                .with_element_path(format!("/model/multiconductor_network/buses/{i}/id")),
            );
        }
        ids.insert(key.clone());
        terminals.insert(key, bus.terminals.iter().cloned().collect());
    }
    (ids, terminals)
}

fn check_bus_ref(
    bus: &str,
    what: &str,
    path: &str,
    bus_ids: &BTreeSet<String>,
    diagnostics: &mut Vec<StructuredDiagnostic>,
) {
    if !bus_ids.contains(&bus.to_ascii_lowercase()) {
        diagnostics.push(
            StructuredDiagnostic::new(
                "VALIDATE.MULTI.STRUCTURE",
                DiagnosticSeverity::Error,
                DiagnosticStage::Validate,
                format!("{what} references unknown bus `{bus}`"),
            )
            .with_element_path(path),
        );
    }
}

fn check_terminal_map(
    bus: &str,
    terminal_map: &[String],
    what: &str,
    path: &str,
    bus_terminals: &BTreeMap<String, BTreeSet<String>>,
    diagnostics: &mut Vec<StructuredDiagnostic>,
) {
    if terminal_map.is_empty() {
        diagnostics.push(
            StructuredDiagnostic::new(
                "VALIDATE.MULTI.TERMINAL_MAP",
                DiagnosticSeverity::Error,
                DiagnosticStage::Validate,
                format!("{what} has an empty terminal map"),
            )
            .with_element_path(path),
        );
        return;
    }

    let Some(known) = bus_terminals.get(&bus.to_ascii_lowercase()) else {
        return;
    };
    for terminal in terminal_map {
        if !known.contains(terminal) {
            diagnostics.push(
                StructuredDiagnostic::new(
                    "VALIDATE.MULTI.TERMINAL_MAP",
                    DiagnosticSeverity::Error,
                    DiagnosticStage::Validate,
                    format!("{what} references unknown terminal `{terminal}` on bus `{bus}`"),
                )
                .with_element_path(path),
            );
        }
    }
}

/// Canonical format name for a balanced source format.
fn balanced_origin(net: &BalancedNetwork) -> Origin {
    match net.source_format {
        SourceFormat::InMemory => Origin::InMemory,
        SourceFormat::Normalized => Origin::Derived {
            parent_package_id: None,
            pass: "normalize-balanced".to_owned(),
            options: serde_json::Map::new(),
        },
        SourceFormat::Gridfm | SourceFormat::PypsaCsv => Origin::Folder {
            path: String::new(),
            format: net.source_format.name().to_owned(),
            file_hashes: BTreeMap::new(),
        },
        SourceFormat::PowerWorldBinary => Origin::BinaryFile {
            path: String::new(),
            format: net.source_format.name().to_owned(),
            hash: None,
            decoded_sections: Vec::new(),
        },
        other => Origin::File {
            path: String::new(),
            format: other.name().to_owned(),
            hash: None,
            retained_source: net.source.is_some(),
        },
    }
}

fn balanced_sources(net: &BalancedNetwork) -> Vec<SourceDescriptor> {
    let Some(kind) = balanced_source_kind(net.source_format) else {
        return Vec::new();
    };
    vec![SourceDescriptor {
        id: "src0".to_owned(),
        kind: kind.to_owned(),
        path: None,
        format: Some(net.source_format.name().to_owned()),
        hash: None,
    }]
}

fn balanced_source_kind(f: SourceFormat) -> Option<&'static str> {
    match f {
        SourceFormat::InMemory | SourceFormat::Normalized => None,
        SourceFormat::Gridfm | SourceFormat::PypsaCsv => Some("folder"),
        SourceFormat::PowerWorldBinary => Some("binary_file"),
        _ => Some("file"),
    }
}

fn balanced_summary(net: &BalancedNetwork) -> ObjectSummary {
    let mut elements = BTreeMap::new();
    elements.insert("buses".to_owned(), net.buses.len() as u64);
    elements.insert("loads".to_owned(), net.loads.len() as u64);
    elements.insert("shunts".to_owned(), net.shunts.len() as u64);
    elements.insert("branches".to_owned(), net.branches.len() as u64);
    elements.insert("generators".to_owned(), net.generators.len() as u64);
    elements.insert("storage".to_owned(), net.storage.len() as u64);
    elements.insert("hvdc".to_owned(), net.hvdc.len() as u64);
    elements.insert(
        "transformers_3w".to_owned(),
        net.transformers_3w.len() as u64,
    );

    let reference_buses: Vec<String> = net
        .buses
        .iter()
        .filter(|b| b.kind == powerio::BusType::Ref)
        .map(|b| b.id.0.to_string())
        .collect();

    ObjectSummary {
        elements,
        topology: Some(ObjectTopology {
            connected_components: None,
            reference_buses,
        }),
        units: Some(ObjectUnits {
            power: Some("MW/MVAr".to_owned()),
            angle: Some("degrees".to_owned()),
            base_mva: Some(net.base_mva),
        }),
    }
}

fn balanced_source_maps(net: &BalancedNetwork, source_id: Option<&str>) -> Vec<SourceMapEntry> {
    let Some(source_id) = source_id else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    push_balanced_network_maps(&mut entries, source_id, net.source_format);
    push_balanced_bus_maps(&mut entries, source_id, net.buses.len());
    push_balanced_injection_maps(&mut entries, source_id, net);
    push_balanced_branch_maps(&mut entries, source_id, net);
    push_balanced_generator_maps(&mut entries, source_id, net.generators.len());
    entries
}

fn push_balanced_network_maps(
    entries: &mut Vec<SourceMapEntry>,
    source_id: &str,
    source_format: SourceFormat,
) {
    push_balanced_map(
        entries,
        source_id,
        "/model/balanced_network/base_mva",
        "case",
        "base_mva",
        MappingKind::Exact,
    );
    if balanced_has_frequency_source(source_format) {
        push_balanced_map(
            entries,
            source_id,
            "/model/balanced_network/base_frequency",
            "case",
            "base_frequency",
            MappingKind::Exact,
        );
    }
}

fn push_balanced_bus_maps(entries: &mut Vec<SourceMapEntry>, source_id: &str, len: usize) {
    push_balanced_record_maps(
        entries,
        source_id,
        "buses",
        len,
        "bus",
        &[
            "id", "kind", "vm", "va", "base_kv", "vmax", "vmin", "area", "zone",
        ],
        MappingKind::Exact,
    );
}

fn push_balanced_injection_maps(
    entries: &mut Vec<SourceMapEntry>,
    source_id: &str,
    net: &BalancedNetwork,
) {
    if net.source_format == SourceFormat::Matpower {
        push_matpower_injection_maps(entries, source_id, net);
    } else {
        push_balanced_record_maps(
            entries,
            source_id,
            "loads",
            net.loads.len(),
            "load",
            &["bus", "p", "q", "in_service"],
            MappingKind::Exact,
        );
        push_balanced_record_maps(
            entries,
            source_id,
            "shunts",
            net.shunts.len(),
            "shunt",
            &["bus", "g", "b", "in_service"],
            MappingKind::Exact,
        );
    }
}

fn push_balanced_branch_maps(
    entries: &mut Vec<SourceMapEntry>,
    source_id: &str,
    net: &BalancedNetwork,
) {
    for (i, branch) in net.branches.iter().enumerate() {
        push_balanced_record_map(
            entries,
            source_id,
            "branches",
            i,
            "branch",
            &[
                "from",
                "to",
                "r",
                "x",
                "b",
                "rate_a",
                "rate_b",
                "rate_c",
                "tap",
                "shift",
                "in_service",
                "angmin",
                "angmax",
            ],
            MappingKind::Exact,
        );
        if branch.charging.is_some() {
            for field in ["g_fr", "b_fr", "g_to", "b_to"] {
                push_balanced_map(
                    entries,
                    source_id,
                    &format!("/model/balanced_network/branches/{i}/charging/{field}"),
                    "branch",
                    field,
                    MappingKind::Exact,
                );
            }
        }
    }
}

fn push_balanced_generator_maps(entries: &mut Vec<SourceMapEntry>, source_id: &str, len: usize) {
    push_balanced_record_maps(
        entries,
        source_id,
        "generators",
        len,
        "generator",
        &[
            "bus",
            "pg",
            "qg",
            "pmax",
            "pmin",
            "qmax",
            "qmin",
            "vg",
            "mbase",
            "in_service",
        ],
        MappingKind::Exact,
    );
}

fn balanced_has_frequency_source(source_format: SourceFormat) -> bool {
    matches!(
        source_format,
        SourceFormat::Psse | SourceFormat::PandapowerJson
    )
}

fn push_matpower_injection_maps(
    entries: &mut Vec<SourceMapEntry>,
    source_id: &str,
    net: &BalancedNetwork,
) {
    // MATPOWER folds loads and shunts into the bus record. Keep the source
    // field token canonical like the rest of the balanced source maps; the
    // record and mapping kind carry the folded-row relationship.
    push_balanced_record_maps(
        entries,
        source_id,
        "loads",
        net.loads.len(),
        "bus",
        &["bus", "p", "q", "in_service"],
        MappingKind::Split,
    );
    push_balanced_record_maps(
        entries,
        source_id,
        "shunts",
        net.shunts.len(),
        "bus",
        &["bus", "g", "b", "in_service"],
        MappingKind::Split,
    );
}

fn push_balanced_record_maps(
    entries: &mut Vec<SourceMapEntry>,
    source_id: &str,
    collection: &str,
    len: usize,
    record: &str,
    fields: &[&str],
    mapping_kind: MappingKind,
) {
    for i in 0..len {
        push_balanced_record_map(
            entries,
            source_id,
            collection,
            i,
            record,
            fields,
            mapping_kind,
        );
    }
}

fn push_balanced_record_map(
    entries: &mut Vec<SourceMapEntry>,
    source_id: &str,
    collection: &str,
    i: usize,
    record: &str,
    fields: &[&str],
    mapping_kind: MappingKind,
) {
    for &field in fields {
        push_balanced_map(
            entries,
            source_id,
            &format!("/model/balanced_network/{collection}/{i}/{field}"),
            record,
            field,
            mapping_kind,
        );
    }
}

fn push_balanced_map(
    entries: &mut Vec<SourceMapEntry>,
    source_id: &str,
    element_path: &str,
    record: &str,
    field: &str,
    mapping_kind: MappingKind,
) {
    entries.push(SourceMapEntry {
        element_path: element_path.to_owned(),
        source_ref: SourceRef::new(source_id)
            .with_record(record)
            .with_field(field),
        mapping_kind,
        confidence: Confidence::High,
    });
}

fn multiconductor_summary(net: &MulticonductorNetwork) -> ObjectSummary {
    let mut elements = BTreeMap::new();
    elements.insert("buses".to_owned(), net.buses.len() as u64);
    elements.insert("linecodes".to_owned(), net.linecodes.len() as u64);
    elements.insert("lines".to_owned(), net.lines.len() as u64);
    elements.insert("switches".to_owned(), net.switches.len() as u64);
    elements.insert("transformers".to_owned(), net.transformers.len() as u64);
    elements.insert("loads".to_owned(), net.loads.len() as u64);
    elements.insert("generators".to_owned(), net.generators.len() as u64);
    elements.insert("shunts".to_owned(), net.shunts.len() as u64);
    elements.insert("voltage_sources".to_owned(), net.sources.len() as u64);

    ObjectSummary {
        elements,
        topology: None,
        units: Some(ObjectUnits {
            power: Some("W/var".to_owned()),
            angle: Some("radians".to_owned()),
            base_mva: None,
        }),
    }
}

fn multiconductor_sources(net: &MulticonductorNetwork) -> Vec<SourceDescriptor> {
    match net.source_format {
        Some(sf) => vec![SourceDescriptor {
            id: "src0".to_owned(),
            kind: "file".to_owned(),
            path: None,
            format: Some(dist_format_name(sf).to_owned()),
            hash: None,
        }],
        None => Vec::new(),
    }
}

fn dist_format_name(f: DistSourceFormat) -> &'static str {
    f.name()
}

fn multiconductor_origin(net: &MulticonductorNetwork) -> Origin {
    match net.source_format {
        Some(sf) => Origin::File {
            path: String::new(),
            format: dist_format_name(sf).to_owned(),
            hash: None,
            retained_source: net.source.is_some(),
        },
        None => Origin::InMemory,
    }
}

fn derived_sources(parent: &NetworkPackage) -> Vec<SourceDescriptor> {
    if !parent.sources.is_empty() {
        return parent.sources.clone();
    }
    vec![SourceDescriptor {
        id: "parent".to_owned(),
        kind: "package".to_owned(),
        path: None,
        format: Some("pio-json".to_owned()),
        hash: parent.package_id.clone(),
    }]
}

fn lowered_balanced_source_maps(
    input: &MulticonductorNetwork,
    balanced: &BalancedNetwork,
    source_id: Option<&str>,
) -> Vec<SourceMapEntry> {
    let Some(source_id) = source_id else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    push_lowered_bus_maps(&mut entries, source_id, input);
    push_lowered_branch_maps(&mut entries, source_id, input, balanced);
    push_lowered_load_maps(&mut entries, source_id, input, balanced);
    push_lowered_shunt_maps(&mut entries, source_id, input, balanced);
    push_lowered_generator_maps(&mut entries, source_id, input, balanced);
    entries
}

fn push_lowered_bus_maps(
    entries: &mut Vec<SourceMapEntry>,
    source_id: &str,
    input: &MulticonductorNetwork,
) {
    for (idx, bus) in input.buses.iter().enumerate() {
        for (field, mapping_kind) in [
            ("id", MappingKind::Synthetic),
            ("kind", MappingKind::Lowered),
            ("vm", MappingKind::ConvertedUnits),
            ("va", MappingKind::ConvertedUnits),
            ("base_kv", MappingKind::ConvertedUnits),
            ("area", MappingKind::Defaulted),
            ("zone", MappingKind::Defaulted),
            ("name", MappingKind::Lowered),
        ] {
            push_lowered_map(
                entries,
                source_id,
                &format!("/model/balanced_network/buses/{idx}/{field}"),
                "multiconductor_bus",
                field,
                mapping_kind,
            );
        }
        for field in ["vmin", "vmax"] {
            let mapping_kind = if bus.v_min.is_some() && bus.v_max.is_some() {
                MappingKind::ConvertedUnits
            } else {
                MappingKind::Defaulted
            };
            push_lowered_map(
                entries,
                source_id,
                &format!("/model/balanced_network/buses/{idx}/{field}"),
                "multiconductor_bus",
                field,
                mapping_kind,
            );
        }
    }
}

fn push_lowered_branch_maps(
    entries: &mut Vec<SourceMapEntry>,
    source_id: &str,
    input: &MulticonductorNetwork,
    balanced: &BalancedNetwork,
) {
    for (idx, branch) in balanced.branches.iter().enumerate() {
        let record = "multiconductor_line";
        for (field, mapping_kind) in [
            ("from", MappingKind::Lowered),
            ("to", MappingKind::Lowered),
            ("r", MappingKind::ConvertedUnits),
            ("x", MappingKind::ConvertedUnits),
            ("b", MappingKind::ConvertedUnits),
            ("in_service", MappingKind::Lowered),
            ("tap", MappingKind::Defaulted),
            ("shift", MappingKind::Defaulted),
            ("angmin", MappingKind::Defaulted),
            ("angmax", MappingKind::Defaulted),
        ] {
            push_lowered_map(
                entries,
                source_id,
                &format!("/model/balanced_network/branches/{idx}/{field}"),
                record,
                field,
                mapping_kind,
            );
        }
        let has_rating = input
            .lines
            .get(idx)
            .and_then(|line| input.linecode(&line.linecode))
            .is_some_and(|code| code.i_max.is_some() || code.s_max.is_some());
        let rate_kind = if has_rating {
            MappingKind::ConvertedUnits
        } else {
            MappingKind::Defaulted
        };
        for field in ["rate_a", "rate_b", "rate_c"] {
            push_lowered_map(
                entries,
                source_id,
                &format!("/model/balanced_network/branches/{idx}/{field}"),
                record,
                field,
                rate_kind,
            );
        }
        if branch.charging.is_some() {
            for field in ["g_fr", "b_fr", "g_to", "b_to"] {
                push_lowered_map(
                    entries,
                    source_id,
                    &format!("/model/balanced_network/branches/{idx}/charging/{field}"),
                    record,
                    field,
                    MappingKind::ConvertedUnits,
                );
            }
        }
    }
}

fn push_lowered_load_maps(
    entries: &mut Vec<SourceMapEntry>,
    source_id: &str,
    input: &MulticonductorNetwork,
    balanced: &BalancedNetwork,
) {
    for idx in 0..balanced.loads.len().min(input.loads.len()) {
        for (field, mapping_kind) in [
            ("bus", MappingKind::Lowered),
            ("p", MappingKind::Aggregated),
            ("q", MappingKind::Aggregated),
            ("in_service", MappingKind::Lowered),
        ] {
            push_lowered_map(
                entries,
                source_id,
                &format!("/model/balanced_network/loads/{idx}/{field}"),
                "multiconductor_load",
                field,
                mapping_kind,
            );
        }
    }
}

fn push_lowered_shunt_maps(
    entries: &mut Vec<SourceMapEntry>,
    source_id: &str,
    input: &MulticonductorNetwork,
    balanced: &BalancedNetwork,
) {
    for idx in 0..balanced.shunts.len().min(input.shunts.len()) {
        for (field, mapping_kind) in [
            ("bus", MappingKind::Lowered),
            ("g", MappingKind::Aggregated),
            ("b", MappingKind::Aggregated),
            ("in_service", MappingKind::Lowered),
        ] {
            push_lowered_map(
                entries,
                source_id,
                &format!("/model/balanced_network/shunts/{idx}/{field}"),
                "multiconductor_shunt",
                field,
                mapping_kind,
            );
        }
    }
}

fn push_lowered_generator_maps(
    entries: &mut Vec<SourceMapEntry>,
    source_id: &str,
    input: &MulticonductorNetwork,
    balanced: &BalancedNetwork,
) {
    for idx in 0..balanced.generators.len().min(input.generators.len()) {
        let generator = &input.generators[idx];
        for (field, mapping_kind) in [
            ("bus", MappingKind::Lowered),
            ("pg", MappingKind::Aggregated),
            ("qg", MappingKind::Aggregated),
            ("vg", MappingKind::Defaulted),
            ("mbase", MappingKind::Synthetic),
            ("in_service", MappingKind::Lowered),
        ] {
            push_lowered_map(
                entries,
                source_id,
                &format!("/model/balanced_network/generators/{idx}/{field}"),
                "multiconductor_generator",
                field,
                mapping_kind,
            );
        }
        for (field, present) in [
            ("pmin", generator.p_min.is_some()),
            ("pmax", generator.p_max.is_some()),
            ("qmin", generator.q_min.is_some()),
            ("qmax", generator.q_max.is_some()),
        ] {
            push_lowered_map(
                entries,
                source_id,
                &format!("/model/balanced_network/generators/{idx}/{field}"),
                "multiconductor_generator",
                field,
                if present {
                    MappingKind::Aggregated
                } else {
                    MappingKind::Defaulted
                },
            );
        }
    }
}

fn push_lowered_map(
    entries: &mut Vec<SourceMapEntry>,
    source_id: &str,
    element_path: &str,
    record: &str,
    field: &str,
    mapping_kind: MappingKind,
) {
    entries.push(SourceMapEntry {
        element_path: element_path.to_owned(),
        source_ref: SourceRef::new(source_id)
            .with_record(record)
            .with_field(field),
        mapping_kind,
        confidence: Confidence::High,
    });
}

/// Lift the `defaulted` map into source-map entries with `mapping_kind =
/// defaulted`. Each key is `"class.name"`; each value is the list of fields the
/// reader materialized from a format default. The element path is a best-effort
/// locator (a precise JSON pointer into the payload arrays is future work).
fn multiconductor_source_maps(
    net: &MulticonductorNetwork,
    source_id: Option<&str>,
) -> Vec<SourceMapEntry> {
    let Some(source_id) = source_id else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    for (element, fields) in &net.defaulted {
        for field in fields {
            entries.push(SourceMapEntry {
                element_path: format!("/model/multiconductor_network/{element}#{field}"),
                source_ref: SourceRef::new(source_id).with_field((*field).to_owned()),
                mapping_kind: MappingKind::Defaulted,
                confidence: Confidence::High,
            });
        }
    }
    entries
}
