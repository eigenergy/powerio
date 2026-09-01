//! Multiconductor distribution network models and converters for OpenDSS
//! `.dss`, PowerModelsDistribution ENGINEERING
//! JSON ("PMD JSON"), and the draft JSON schema of the IEEE PES Task Force on
//! Benchmarking Multiconductor OPF ("BMOPF JSON",
//! <https://github.com/frederikgeth/bmopf-report>).
//!
//! The model uses wire coordinates: string bus IDs, ordered terminal names,
//! explicit grounding, terminal maps on every element, SI units, and radians.
//! The transmission model in `powerio` is positive sequence and remains a
//! separate type.
//!
//! ```no_run
//! let source = powerio_core::Source::open("feeder.dss")?;
//! let module = powerio_dist::parse(source)?;
//! for line in powerio_dist::diagnostics::render_diagnostics(&module.diagnostics) {
//!     eprintln!("parse: {line}");
//! }
//! let emitted = powerio_dist::emit(
//!     &module,
//!     powerio_dist::DistTargetFormat::PmdJson,
//!     powerio_core::Destination::memory("feeder.pmd.json")?,
//! )?;
//! for line in powerio_dist::diagnostics::render_diagnostics(emitted.diagnostics()) {
//!     eprintln!("emit: {line}");
//! }
//! # Ok::<(), powerio_core::Error>(())
//! ```
//!
//! # Fidelity rules
//!
//! Emitting to the retained source format returns the original bytes. Cross
//! format emission uses the typed model and reports fields the target cannot
//! represent through [`powerio_core::EmitResult::diagnostics`]. The DSS reader expands OpenDSS
//! class defaults into explicit model values and records them in
//! [`MulticonductorNetwork::defaulted`]. BMOPF output includes those values.
//! The per fixture results live in `docs/conversion-matrix.md`.
//!
//! # Float formatting
//!
//! Canonical output formats every number as its shortest round trip
//! representation: Rust's `Display` for `.dss`, serde_json (ryu) for both
//! JSON formats. The readers parse with serde_json's `float_roundtrip`
//! feature, so a parse of canonical output recovers the exact bit pattern
//! and canonical emissions are idempotent. JSON cannot carry `Inf`/`NaN`: the
//! PMD emitter uses `null` (PMD restores the value from the field name
//! suffix), and the BMOPF emitter uses `0` with a warning, since the schema
//! requires numbers. The byte exact echo tier is unaffected; it never
//! reformats.

pub mod bmopf;
mod collect;
pub mod convert;
pub mod diagnostics;
pub mod dss;
pub mod error;
pub mod geo;
pub mod graph;
pub mod model;
pub mod pmd;
#[cfg(test)]
pub(crate) mod testkit;

pub use bmopf::{BMOPF_SCHEMA_ID, BMOPF_SCHEMA_VERSION, BmopfEmitOptions};
pub use convert::{
    DistTargetFormat, EmitOptions, classify_distribution_json, emit, emit_with_options, parse,
    parse_dist_target_format,
};
pub use diagnostics::{Diagnostic, DiagnosticCode, DiagnosticSeverity, DiagnosticStage};
pub use dss::{DssEmitOptions, DssLoadVoltageBounds};
pub use error::{Error, Result};
pub use geo::{CoordinateSpace, DistCanvas, DistCoordsKind, DistGeoMeta, DistLocation};
pub use graph::{
    DistGraph, DistGraphAttachment, DistGraphAttachmentKind, DistGraphBus, DistGraphEdge,
    DistGraphEdgeKind,
};
pub use model::{
    ActivePowerReference, ActivePowerUnit, ConductorMatrix, Configuration, ControlVoltageReference,
    DistBus, DistCapacitor, DistControlProfile, DistGenerator, DistIbr, DistLine, DistLineCode,
    DistLoad, DistLoadVoltageModel, DistShunt, DistSourceFormat, DistSwitch, DistTransformer,
    DistWinding, DistWindingConn, Extras, IbrPrimeMover, IbrTopology, IbrVoltageAggregation,
    MulticonductorNetwork, PowerFactorControl, ReactivePowerReference, ReactivePowerUnit,
    UntypedObject, VoltVarControl, VoltWattControl, VoltageSource, find_unresolved_references,
};
