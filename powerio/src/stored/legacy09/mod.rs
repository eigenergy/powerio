//! The frozen 0.9 `.pio.json` package representation, retained only as
//! the private decode behind the one way upgrade in
//! [`crate::stored`]. Nothing here is 1.0 public API: the module is
//! crate private, the live replacement is `PioModule<PioValue>` with
//! the stored schema, and the multiconductor to balanced transformation
//! lives in [`crate::transform`].
#![allow(dead_code)]
#![allow(unused_imports)] // the re-export surface serves the frozen decode and its tests

#[cfg(test)]
mod roundtrip_tests;

pub mod diagnostics;
pub mod document;
pub mod legacy_diag;
pub mod model;
pub mod operating;
pub mod provenance;
pub mod study;
pub mod summary;
pub mod validation;

// The frozen 0.9 severity doc names this path, and the generated 0.9 schema
// records that text, so the link has to resolve here.
pub use powerio_core::ErrorCategory;

pub use diagnostics::{DiagnosticCode, DiagnosticSeverity, DiagnosticStage, StructuredDiagnostic};
pub mod error;
pub use error::{Error, Result};

pub use document::{
    DerivedMetadata, NetworkPackage, NormalizedSolverTableMetadata, NormalizedSolverTableRowCounts,
    NormalizedSolverTableSourceRows, ensure_payload_uids,
};
pub use model::{ModelKind, ModelPayload};
pub use operating::{ElementRef, ElementUpdate, OperatingPoint, OperatingPointSeries, TimeAxis};
pub use provenance::{
    Confidence, MappingKind, Origin, Producer, SourceDescriptor, SourceMapEntry, SourceRef,
};
pub use study::{StudyBlock, StudyCommit, StudyEdit};
pub use summary::{ObjectSummary, ObjectTopology, ObjectUnits};
pub use validation::{ValidationCounts, ValidationPass, ValidationStatus, ValidationSummary};
