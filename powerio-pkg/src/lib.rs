//! `.pio.json` package metadata and model payloads.
//!
//! PowerIO keeps two static network model families as separate types:
//!
//! - [`powerio::BalancedNetwork`], the scalar positive sequence transmission
//!   model;
//! - [`powerio_dist::MulticonductorNetwork`], the wire coordinate distribution
//!   model.
//!
//! A [`NetworkPackage`] stores one payload with an explicit [`ModelKind`],
//! producer and origin metadata, source maps, diagnostics, validation results,
//! and lowering history. Optional operating points replay independent states;
//! optional study commits apply cumulative edits. GOC3 packages use operating
//! points for the source time series: the payload holds one static interval, and
//! [`NetworkPackage::materialize_operating_point`] derives another static
//! package from a selected period. It serializes to `.pio.json`. See
//! `docs/src/compiler-ir.md` for the architecture and
//! `docs/src/pio-json-schema.md` for the field reference.
//!
//! The package always carries [`NetworkPackage::model_kind`] explicitly; a
//! reader must never infer whether the payload is balanced or multiconductor
//! from which field is present. [`NetworkPackage::kind_is_consistent`] asserts
//! the explicit kind agrees with the payload variant.
//!
//! ```
//! use powerio_pkg::{NetworkPackage, ModelKind};
//!
//! let net = powerio::BalancedNetwork::in_memory("demo", 100.0, vec![], vec![]);
//! let pkg = NetworkPackage::from_balanced(net);
//! assert_eq!(pkg.model_kind(), ModelKind::Balanced);
//! assert!(pkg.kind_is_consistent());
//! let json = pkg.to_json_pretty().unwrap();
//! let back = NetworkPackage::from_json(&json).unwrap();
//! assert_eq!(back.model_kind(), ModelKind::Balanced);
//! ```
//!

pub mod diagnostics;
pub mod geo;
pub mod lowering;
pub mod model;
pub mod operating;
pub mod package;
pub mod provenance;
pub mod study;
pub mod summary;
pub mod validation;

pub use diagnostics::{DiagnosticCode, DiagnosticSeverity, DiagnosticStage, StructuredDiagnostic};
pub use geo::{apply_dist_geo_layer, dist_geo_layer};
pub use lowering::{
    LoweringRecord, MulticonductorToBalancedError, MulticonductorToBalancedLowering,
    MulticonductorToBalancedOptions, MulticonductorToBalancedReadiness,
    SequenceTransformConvention, check_multiconductor_to_balanced_lowering,
    lower_multiconductor_to_balanced,
};
pub use model::{ModelKind, ModelPayload};
pub use operating::{ElementRef, ElementUpdate, OperatingPoint, OperatingPointSeries, TimeAxis};
pub use package::{
    DerivedMetadata, NetworkPackage, NormalizedSolverTableMetadata, NormalizedSolverTableRowCounts,
    NormalizedSolverTableSourceRows, PIO_PACKAGE_SCHEMA_URL, PIO_PACKAGE_SCHEMA_VERSION,
    PIO_PAYLOAD_BALANCED_SCHEMA_URL, PIO_PAYLOAD_BALANCED_SCHEMA_VERSION,
    PIO_PAYLOAD_MULTICONDUCTOR_SCHEMA_URL, PIO_PAYLOAD_MULTICONDUCTOR_SCHEMA_VERSION,
    READ_GRIDFM_FIDELITY_WARNING, READ_TRANSMISSION_PARSE_WARNING, ensure_payload_uids,
};
pub use provenance::{
    Confidence, MappingKind, Origin, Producer, SourceDescriptor, SourceMapEntry, SourceRef,
};
pub use study::{StudyBlock, StudyCommit, StudyEdit};
pub use summary::{ObjectSummary, ObjectTopology, ObjectUnits};
pub use validation::{ValidationCounts, ValidationPass, ValidationStatus, ValidationSummary};
