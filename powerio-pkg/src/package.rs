//! The `.pio.json` root object.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use powerio::{BalancedNetwork, SourceFormat};
use powerio_dist::{DistSourceFormat, MulticonductorNetwork};

use crate::diagnostics::{DiagnosticSeverity, DiagnosticStage, StructuredDiagnostic};
use crate::lowering::LoweringRecord;
use crate::model::{ModelKind, ModelPayload};
use crate::provenance::{
    Confidence, MappingKind, Origin, Producer, SourceDescriptor, SourceMapEntry, SourceRef,
};
use crate::summary::{ObjectSummary, ObjectTopology, ObjectUnits};
use crate::validation::ValidationSummary;

/// The canonical schema URL for this package version.
pub const PIO_PACKAGE_SCHEMA_URL: &str = "https://powerio.dev/schema/pio-package/0.1";

/// The package schema version (semver). Additive fields bump the minor; field
/// moves bump the major (or ship a migration pass).
pub const PIO_PACKAGE_SCHEMA_VERSION: &str = "0.1.0";

fn default_schema_url() -> String {
    PIO_PACKAGE_SCHEMA_URL.to_owned()
}

fn default_schema_version() -> String {
    PIO_PACKAGE_SCHEMA_VERSION.to_owned()
}

/// Optional derived metadata: matrix statistics and cache keys.
/// Empty by default; the scaffold never populates it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DerivedMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matrix_stats: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub cache_keys: BTreeMap<String, String>,
}

impl DerivedMetadata {
    fn is_empty(&self) -> bool {
        self.matrix_stats.is_none() && self.cache_keys.is_empty()
    }
}

/// The compiler package: a versioned envelope around one IR payload plus the
/// provenance, diagnostics, validation, and lowering history that make the
/// artifact trustworthy. Serializes to `.pio.json`.
///
/// `model_kind` is stored explicitly and is authoritative; the payload is also
/// self-describing (tagged by `kind`). [`CompilerPackage::kind_is_consistent`]
/// asserts the two agree. Unknown future top-level fields are tolerated on read
/// (ignored) so a newer producer's package still deserializes here.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompilerPackage {
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
    pub model: ModelPayload,
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

impl CompilerPackage {
    /// Wrap a balanced network. Origin is inferred from its source format:
    /// `InMemory` / `Derived` (normalized) / `File` (a parsed text format,
    /// recording whether source was retained; the path is not captured here).
    pub fn from_balanced(net: BalancedNetwork) -> Self {
        let origin = balanced_origin(&net);
        let summary = balanced_summary(&net);
        Self {
            schema: default_schema_url(),
            schema_version: default_schema_version(),
            producer: Producer::powerio(),
            package_id: None,
            created_at: None,
            model_kind: ModelKind::Balanced,
            model: ModelPayload::balanced(net),
            origin,
            sources: Vec::new(),
            source_maps: Vec::new(),
            diagnostics: Vec::new(),
            validation: ValidationSummary::ok(),
            summary,
            lowering_history: Vec::new(),
            derived: DerivedMetadata::default(),
        }
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

        Self {
            schema: default_schema_url(),
            schema_version: default_schema_version(),
            producer: Producer::powerio(),
            package_id: None,
            created_at: None,
            model_kind: ModelKind::Multiconductor,
            model: ModelPayload::multiconductor(net),
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
        if !pkg.kind_is_consistent() {
            return Err(<serde_json::Error as serde::de::Error>::custom(
                "model_kind does not match model.kind",
            ));
        }
        Ok(pkg)
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
}

/// Canonical format name for a balanced source format.
fn balanced_format_name(f: SourceFormat) -> &'static str {
    match f {
        SourceFormat::Matpower => "matpower",
        SourceFormat::PowerModelsJson => "powermodels-json",
        SourceFormat::EgretJson => "egret-json",
        SourceFormat::Psse => "psse",
        SourceFormat::PowerWorld => "powerworld",
        SourceFormat::PandapowerJson => "pandapower-json",
        SourceFormat::Pslf => "pslf",
        SourceFormat::PowerWorldBinary => "powerworld-pwb",
        SourceFormat::InMemory => "in-memory",
        SourceFormat::Normalized => "normalized",
        SourceFormat::Gridfm => "gridfm",
        SourceFormat::PypsaCsv => "pypsa-csv",
        _ => "unknown",
    }
}

fn balanced_origin(net: &BalancedNetwork) -> Origin {
    match net.source_format {
        SourceFormat::InMemory => Origin::InMemory,
        SourceFormat::Normalized => Origin::Derived {
            parent_package_id: None,
            pass: "normalize-balanced".to_owned(),
            options: serde_json::Map::new(),
        },
        other => Origin::File {
            path: String::new(),
            format: balanced_format_name(other).to_owned(),
            hash: None,
            retained_source: net.source.is_some(),
        },
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
