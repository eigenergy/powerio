//! Typed balanced network models and format implementations.
//!
//! Readers and writers cover MATPOWER `.m`, PowerModels JSON, PSS/E `.raw`,
//! PowerWorld `.aux`, pandapower JSON, PyPSA CSV, egret JSON, PSLF `.epc`, GO
//! Challenge 3 JSON, Surge JSON, and DeepMind OPFData JSON. PowerWorld `.pwb`
//! case files are read only, and GO Challenge 3 and OPFData JSON have no
//! canonical writer beyond same source echo; `.pwd` display files parse through
//! [`parse_display_file`].
//! Each reader produces a [`BalancedNetwork`]. The top level `powerio` facade
//! owns universal parsing and `emit`; this component crate supplies the typed
//! transmission implementation.
//!
//! A reader that retains source text can return those bytes when writing the
//! same format. Matrix and problem instance builders live in separate crates.
//!
//! ```
//! use powerio_core::Source;
//! use powerio_tx::{parse, parse_format_id};
//!
//! let src = "\
//! function mpc = example
//! mpc.version = '2';
//! mpc.baseMVA = 100;
//! mpc.bus = [
//! \t1\t3\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
//! \t2\t1\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
//! ];
//! mpc.branch = [
//! \t1\t2\t0.01\t0.1\t0\t0\t0\t0\t0\t0\t1\t-360\t360;
//! ];
//! ";
//! let source = Source::from_bytes("example.m", src.as_bytes().to_vec())?
//!     .with_format(parse_format_id("matpower")?);
//! let module = parse(source)?;
//! assert_eq!(module.value().buses().len(), 2);
//! # Ok::<(), powerio_core::Error>(())
//! ```

/// The powerio crate version, for provenance fields written by downstream
/// crates whose own version can drift from the core's.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

mod collect;
pub mod dc;
pub mod diagnostics;
pub mod error;
pub mod format;
pub mod gen_cost;
pub mod geo;
pub mod indexed;
pub mod network;
mod normalize;
mod operations;
#[doc(hidden)]
pub mod solver_tables;
pub mod version;

pub use dc::BranchSusceptanceFormula;
pub use diagnostics::{Diagnostic, DiagnosticCode, DiagnosticSeverity, EmitFamily};
pub use error::{Error, ErrorCategory, Result};
pub use format::routing::{
    Detection, JSON_CLASSES, JsonClass, classify_json_bytes, classify_json_text,
    parse_distribution_format, parse_transmission_format,
};
#[cfg(test)]
pub(crate) use format::test_parse::{parse_file, parse_str};
pub use format::{
    DisplayData, DisplayFormat, EmitOptions, OpfDataSolution, PwdDisplay, PwdSubstation,
    PypsaCsvSequence, SOURCE_FORMAT_NAMES, TargetFormat, emit, emit_with_options, parse,
    parse_display, parse_display_file, parse_display_format, parse_egret_time_series,
    parse_format_id, parse_goc3_json, parse_opfdata_json, parse_pypsa_csv_time_series,
    parse_target_format,
};

#[doc(hidden)]
pub use format::{__emit_pypsa_csv, __emit_pypsa_csv_with_options};
pub use gen_cost::{GenCostPatch, GenCostPolicyReport, MissingGenCostPolicy, parse_gen_cost_csv};
pub use geo::{
    Canvas, CoordinateSpace, CoordsKind, ElementKey, GeoApplyReport, GeoFeature, GeoGeometry,
    GeoLayer, GeoMeta, GeoParsed, GeoTarget, Location, apply_substation_points,
    to_geo_layer_from_aux_substations, to_geo_layer_from_pwd, to_lonlat_from_pwd_mercator,
};
pub use indexed::{ConnectivityReport, IndexCore, IndexedNetwork};
pub use network::{
    Area, BalancedNetwork, Branch, BranchCharging, BranchCurrentRatings, BranchRatingSet,
    BranchSolution, Bus, BusId, BusType, DEFAULT_BASE_FREQUENCY, Extras, GenCaps, GenCost,
    Generator, Hvdc, Impedance, Load, LoadVoltageModel, Shunt, ShuntBlock, SolverParams,
    SourceFormat, Storage, Switch, SwitchedShuntControl, SwitchedShuntMode, Transformer3W,
    TransformerControl, TransformerControlMode, Winding, calc_series_admittance_of, repair_values,
};
pub use normalize::{
    NormalizeOptions, NormalizeSourceRows, NormalizedNetwork, POWER_MODELS_ANGLE_BOUND_PAD,
};
pub use operations::Selector;

#[doc(hidden)]
pub use solver_tables::{
    NORMALIZED_SOLVER_TABLES_PASS, NormalizedSolverTables, SolverArcRow, SolverArcTerminal,
    SolverBranchRow, SolverBusRow, SolverCostRow, SolverGeneratorRow, SolverHvdcRow, SolverLoadRow,
    SolverShuntRow, SolverStorageRow, SolverSwitchRow, SolverTableIndex, SolverTableUnits,
};
