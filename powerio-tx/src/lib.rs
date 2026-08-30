//! Typed balanced network models, parsers, and writers.
//!
//! Readers and writers cover MATPOWER `.m`, PowerModels JSON, PSS/E `.raw`,
//! PowerWorld `.aux`, pandapower JSON, PyPSA CSV, egret JSON, PSLF `.epc`, GO
//! Challenge 3 JSON, Surge JSON, and DeepMind OPFData JSON. PowerWorld `.pwb`
//! case files are read only, and GO Challenge 3 and OPFData JSON have no
//! canonical writer beyond same source echo; `.pwd` display files parse through
//! [`parse_display_file`].
//! Each reader produces a [`BalancedNetwork`]. [`BalancedNetwork::to_format`] returns the
//! serialized target and warnings for fields the target cannot represent. See
//! [`crate::format`] for format routing and fidelity rules.
//!
//! A reader that retains source text can return those bytes when writing the
//! same format. Matrix and problem instance builders live in separate crates.
//!
//! ```
//! use powerio_core::Source;
//! use powerio_tx::{TargetFormat, format_id_for, parse, write_as};
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
//!     .with_format(format_id_for("matpower")?);
//! let module = parse(source)?;
//! assert_eq!(module.value().buses().len(), 2);
//! // An unchanged parsed module echoes its source bytes exactly.
//! assert_eq!(write_as(&module, TargetFormat::Matpower)?.text, src);
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
pub mod solver_tables;
pub mod version;

pub use dc::{BranchSusceptanceFormula, DcConvention, DcNetworkData, dc_network_data};
pub use diagnostics::{Diagnostic, DiagnosticCode, DiagnosticSeverity, EmitFamily};
pub use error::{Error, ErrorCategory, Result};
pub use format::routing::{
    Detection, JSON_CLASSES, JsonClass, classify_json_bytes, classify_json_text,
};
#[cfg(test)]
pub(crate) use format::test_parse::{parse_file, parse_str};
pub use format::{
    Conversion, DisplayData, DisplayFormat, OpfDataSolution, PwdDisplay, PwdSubstation,
    PypsaCsvOutputs, PypsaCsvSequence, SOURCE_FORMAT_NAMES, TargetFormat, WriteOptions,
    convert_file, convert_file_with_options, convert_str, convert_str_with_options,
    display_format_from_name, format_id_for, parse, parse_display_bytes, parse_display_file,
    parse_egret_time_series, parse_goc3_json, parse_opfdata_json, parse_pypsa_csv_time_series,
    target_format_from_name, write, write_as, write_as_with_options, write_dir,
    write_dir_with_options, write_egret_json, write_matpower, write_network, write_pandapower_json,
    write_powermodels_json, write_powerworld, write_pslf, write_psse, write_psse_rev,
    write_pypsa_csv, write_pypsa_csv_folder, write_surge_json, write_with_options,
};
pub use gen_cost::{GenCostPatch, GenCostPolicyReport, MissingGenCostPolicy, parse_gen_cost_csv};
pub use geo::{
    Canvas, CoordinateSpace, CoordsKind, ElementKey, GeoApplyReport, GeoFeature, GeoGeometry,
    GeoLayer, GeoMeta, GeoParsed, GeoTarget, Location, apply_substation_points,
    geo_layer_from_aux_substations, geo_layer_from_pwd, pwd_mercator_to_lonlat,
};
pub use indexed::{ConnectivityReport, IndexCore, IndexedNetwork};
pub use network::{
    Area, BalancedNetwork, Branch, BranchCharging, BranchCurrentRatings, BranchRatingSet,
    BranchSolution, Bus, BusId, BusType, DEFAULT_BASE_FREQUENCY, Extras, GenCaps, GenCost,
    Generator, Hvdc, Impedance, Load, LoadVoltageModel, Shunt, ShuntBlock, SolverParams,
    SourceFormat, Storage, Switch, SwitchedShuntControl, SwitchedShuntMode, Transformer3W,
    TransformerControl, TransformerControlMode, Winding, repair_values, series_admittance_of,
};
pub use normalize::{
    NormalizeOptions, NormalizeSourceRows, NormalizedNetwork, POWER_MODELS_ANGLE_BOUND_PAD,
};
pub use operations::Selector;

pub use solver_tables::{
    NORMALIZED_SOLVER_TABLES_PASS, NormalizedSolverTables, SolverArcRow, SolverArcTerminal,
    SolverBranchRow, SolverBusRow, SolverCostRow, SolverGeneratorRow, SolverHvdcRow, SolverLoadRow,
    SolverShuntRow, SolverStorageRow, SolverSwitchRow, SolverTableIndex, SolverTableUnits,
};
