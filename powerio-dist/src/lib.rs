//! Multiconductor distribution network models and converters for OpenDSS
//! `.dss`, PowerModelsDistribution ENGINEERING
//! JSON ("PMD JSON"), and the draft JSON schema of the IEEE PES Task Force on
//! Benchmarking Multiconductor OPF ("BMOPF JSON",
//! <https://github.com/frederikgeth/bmopf-report>).
//!
//! The model uses wire coordinates: string bus IDs, ordered terminal names,
//! explicit grounding, terminal maps, SI units, and radians. The transmission
//! model in `powerio` is positive sequence and remains a separate type.

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
pub mod readiness;
pub(crate) mod nonfinite;
#[cfg(test)]
pub(crate) mod testkit;

pub use bmopf::{
    BMOPF_SCHEMA_ID, BMOPF_SCHEMA_VERSION, BmopfWriteOptions, write_bmopf_json,
    write_bmopf_json_with_options,
};
pub use convert::{
    Conversion, ConversionSidecar, DistTargetFormat, classify_distribution_json, convert_source,
    dist_target_from_name, parse, write, write_as, write_network,
};
pub use diagnostics::{Diagnostic, DiagnosticCode, DiagnosticSeverity, DiagnosticStage};
pub use dss::{DssLoadVoltageBounds, DssWriteOptions, write_dss, write_dss_with_options};
pub use error::{Error, Result};
pub use geo::{CoordinateSpace, DistCanvas, DistCoordsKind, DistGeoMeta, DistLocation};
pub use graph::{
    DistGraph, DistGraphAttachment, DistGraphAttachmentKind, DistGraphBus, DistGraphEdge,
    DistGraphEdgeKind,
};
pub use model::{
    ActivePowerReference, ActivePowerUnit, Configuration, ControlVoltageReference, DistBus,
    DistCapacitor, DistControlProfile, DistGenerator, DistIbr, DistLine, DistLineCode, DistLoad,
    DistLoadVoltageModel, DistShunt, DistSourceFormat, DistSwitch, DistTransformer, DistWinding,
    DistWindingConn, Extras, IbrPrimeMover, IbrTopology, IbrVoltageAggregation, Mat,
    MulticonductorNetwork, PowerFactorControl, ReactivePowerReference, ReactivePowerUnit,
    UntypedObject, VoltVarControl, VoltWattControl, VoltageSource, unresolved_references,
};
pub use readiness::{
    audit_electrical_readiness, ElectricalReadiness, ReadinessFinding, ReadinessSeverity,
};
pub use pmd::write_pmd_json;
