//! `powerio-pkg`: the `.pio.json` compiler package.
//!
//! PowerIO has no single flattened "universal network" struct. It has two
//! concrete static-grid IR families that stay distinct:
//!
//! - [`powerio::BalancedNetwork`] (the scalar positive-sequence transmission
//!   model, historically `powerio::Network`);
//! - [`powerio_dist::MulticonductorNetwork`] (the wire-coordinate distribution
//!   model, historically `powerio_dist::DistNetwork`).
//!
//! A [`CompilerPackage`] is the readable envelope that wraps exactly one of
//! those payloads at a time, alongside the metadata a compiler artifact needs
//! to be trustworthy: an explicit [`ModelKind`], producer and origin metadata,
//! source maps, structured diagnostics, a validation summary, and lowering
//! history. It serializes to `.pio.json`. See `docs/src/compiler-ir.md` for the
//! architecture and `docs/src/pio-json-schema.md` for the field reference.
//!
//! The package always carries [`CompilerPackage::model_kind`] explicitly; a
//! reader must never infer whether the payload is balanced or multiconductor
//! from which field is present. [`CompilerPackage::kind_is_consistent`] asserts
//! the explicit kind agrees with the payload variant.
//!
//! ```
//! use powerio_pkg::{CompilerPackage, ModelKind};
//!
//! let net = powerio::BalancedNetwork::in_memory("demo", 100.0, vec![], vec![]);
//! let pkg = CompilerPackage::from_balanced(net);
//! assert_eq!(pkg.model_kind(), ModelKind::Balanced);
//! assert!(pkg.kind_is_consistent());
//! let json = pkg.to_json_pretty().unwrap();
//! let back = CompilerPackage::from_json(&json).unwrap();
//! assert_eq!(back.model_kind(), ModelKind::Balanced);
//! ```
//!
//! Binary `.pio` is out of scope until the JSON package stabilizes; this crate
//! is JSON only.

pub mod diagnostics;
pub mod lowering;
pub mod model;
pub mod package;
pub mod provenance;
pub mod summary;
pub mod validation;

pub use diagnostics::{DiagnosticCode, DiagnosticSeverity, DiagnosticStage, StructuredDiagnostic};
pub use lowering::{
    LoweringRecord, MulticonductorToBalancedError, MulticonductorToBalancedLowering,
    MulticonductorToBalancedOptions, MulticonductorToBalancedReadiness,
    SequenceTransformConvention, check_multiconductor_to_balanced_lowering,
    lower_multiconductor_to_balanced,
};
pub use model::{ModelKind, ModelPayload};
pub use package::{
    CompilerPackage, DerivedMetadata, NormalizedSolverTableMetadata,
    NormalizedSolverTableRowCounts, NormalizedSolverTableSourceRows, PIO_PACKAGE_SCHEMA_URL,
    PIO_PACKAGE_SCHEMA_VERSION,
};
pub use provenance::{
    Confidence, MappingKind, Origin, Producer, SourceDescriptor, SourceMapEntry, SourceRef,
};
pub use summary::{ObjectSummary, ObjectTopology, ObjectUnits};
pub use validation::{ValidationCounts, ValidationPass, ValidationStatus, ValidationSummary};
