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
//! let net = powerio_dist::parse_file("feeder.dss", None)?;
//! for w in &net.warnings {
//!     eprintln!("parse: {w}");
//! }
//! let conv = net.to_format(powerio_dist::DistTargetFormat::PmdJson);
//! # Ok::<(), powerio_dist::Error>(())
//! ```
//!
//! # Fidelity rules
//!
//! Writing to the retained source format returns the original bytes. Cross
//! format conversion writes from the typed model and reports fields the target
//! cannot represent in [`Conversion::warnings`]. The DSS reader expands OpenDSS
//! class defaults into explicit model values and records them in
//! [`DistNetwork::defaulted`]. BMOPF output includes those values.
//! The per fixture results live in `docs/conversion-matrix.md`.
//!
//! # Float formatting
//!
//! Canonical output formats every number as its shortest round trip
//! representation: Rust's `Display` for `.dss`, serde_json (ryu) for both
//! JSON formats. The readers parse with serde_json's `float_roundtrip`
//! feature, so a parse of canonical output recovers the exact bit pattern
//! and canonical writes are idempotent. JSON cannot carry `Inf`/`NaN`: the
//! PMD writer emits `null` (PMD restores the value from the field name
//! suffix), and the BMOPF writer emits `0` with a warning, since the schema
//! requires numbers. The byte exact echo tier is unaffected; it never
//! reformats.

pub mod bmopf;
pub mod convert;
pub mod diagnostics;
pub mod dss;
pub mod error;
pub mod geo;
pub mod graph;
pub mod model;
pub mod pmd;

pub use bmopf::{
    BmopfWriteOptions, parse_bmopf_file, parse_bmopf_str, write_bmopf_json,
    write_bmopf_json_with_options,
};
pub use convert::{
    Conversion, ConversionSidecar, DistTargetFormat, convert_file, convert_str,
    dist_target_from_name, parse_file, parse_str,
};
pub use diagnostics::{DiagnosticCode, DiagnosticSeverity, DiagnosticStage, StructuredDiagnostic};
pub use dss::{
    DssLoadVoltageBounds, DssWriteOptions, parse_dss_file, parse_dss_str, write_dss,
    write_dss_with_options,
};
pub use error::{Error, Result};
pub use geo::{Canvas, CoordinateSpace, CoordsKind, GeoMeta, Location};
pub use graph::{
    DistGraph, DistGraphAttachment, DistGraphAttachmentKind, DistGraphBus, DistGraphEdge,
    DistGraphEdgeKind,
};
pub use model::{
    ActivePowerReference, ActivePowerUnit, Configuration, ControlVoltageReference, DistBus,
    DistCapacitor, DistControlProfile, DistGenerator, DistIbr, DistLine, DistLineCode, DistLoad,
    DistLoadVoltageModel, DistNetwork, DistShunt, DistSourceFormat, DistSwitch, DistTransformer,
    Extras, IbrPrimeMover, IbrTopology, IbrVoltageAggregation, Mat, MulticonductorNetwork,
    PowerFactorControl, ReactivePowerReference, ReactivePowerUnit, UntypedObject, VoltVarControl,
    VoltWattControl, VoltageSource, Winding, WindingConn,
};
pub use pmd::{parse_pmd_file, parse_pmd_str, write_pmd_json};
