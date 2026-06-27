//! Producer, origin, source descriptors, and source maps.
//!
//! These answer the trust questions a compiler artifact must answer: which tool
//! produced it, what the source was, and which canonical field came from which
//! source record by what kind of mapping.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The tool and build that produced the package.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Producer {
    pub tool: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
}

impl Producer {
    /// The producer for packages built by this crate version of PowerIO.
    pub fn powerio() -> Self {
        Self {
            tool: "powerio".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            git_commit: None,
            features: Vec::new(),
        }
    }
}

/// Where the package came from. Internally tagged on `kind` in JSON, so a reader
/// distinguishes an in-memory model, a single text file (with or without
/// retained source), a folder dataset, a partially decoded binary, a derived
/// product of a lowering pass, or a composite of several sources.
///
/// The `hash` fields are unified to `hash` here (the illustrative spec JSON
/// wrote `source_hash` on `File`; one name across variants is cleaner for a real
/// schema).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Origin {
    /// Built in process, no source artifact.
    InMemory,
    File {
        path: String,
        format: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hash: Option<String>,
        /// Whether the original source text was retained for a byte-exact
        /// same-format echo. The retained text itself is not embedded in the
        /// package; this only records that it exists at the frontend.
        #[serde(default)]
        retained_source: bool,
    },
    Folder {
        path: String,
        format: String,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        file_hashes: BTreeMap<String, String>,
    },
    BinaryFile {
        path: String,
        format: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hash: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        decoded_sections: Vec<String>,
    },
    /// A model produced by a lowering/normalization pass from another package.
    Derived {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_package_id: Option<String>,
        pass: String,
        #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
        options: serde_json::Map<String, serde_json::Value>,
    },
    /// Several sources combined, e.g. a static case plus a profile set.
    Composite { sources: Vec<String> },
}

/// A declared source artifact, referenced from source maps and diagnostics by
/// its `id`. (`sources[]` in the package.)
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceDescriptor {
    pub id: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
}

/// A pointer into one source artifact: where a canonical field came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRef {
    pub source_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
    /// Byte offset, for binary sources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_offset: Option<u64>,
    /// Record / section / object type, e.g. `BUS`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<String>,
    /// Field / property name, e.g. `VM`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    /// Raw token / value, when safe to embed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_token: Option<String>,
}

impl SourceRef {
    pub fn new(source_id: impl Into<String>) -> Self {
        Self {
            source_id: source_id.into(),
            line: None,
            column: None,
            byte_offset: None,
            record: None,
            field: None,
            raw_token: None,
        }
    }

    #[must_use]
    pub fn with_field(mut self, field: impl Into<String>) -> Self {
        self.field = Some(field.into());
        self
    }

    #[must_use]
    pub fn with_line(mut self, line: u32) -> Self {
        self.line = Some(line);
        self
    }
}

/// How a canonical field relates to its source value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MappingKind {
    /// Copied verbatim from the source.
    Exact,
    /// Materialized from a format default rather than the source text.
    Defaulted,
    /// Inferred from other source data.
    Inferred,
    /// Converted into canonical units (e.g. ohms to per unit).
    ConvertedUnits,
    /// Produced by a lowering pass (e.g. positive-sequence equivalent).
    Lowered,
    /// One canonical field aggregated from several source records.
    Aggregated,
    /// One source record split into several canonical fields/elements.
    Split,
    /// Synthesized with no direct source (e.g. a generated bus id).
    Synthetic,
    /// A source-specific extra preserved verbatim.
    RetainedExtra,
}

/// How confident the source map entry is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    Exact,
    High,
    Medium,
    Low,
}

/// One `element_path -> source` mapping.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceMapEntry {
    /// JSON pointer (or best-effort locator) into the package payload.
    pub element_path: String,
    pub source_ref: SourceRef,
    pub mapping_kind: MappingKind,
    pub confidence: Confidence,
}
