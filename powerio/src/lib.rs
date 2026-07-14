//! Typed balanced network models, parsers, and writers.
//!
//! Readers and writers cover MATPOWER `.m`, PowerModels JSON, PSS/E `.raw`,
//! PowerWorld `.aux`, pandapower JSON, PyPSA CSV, egret JSON, PSLF `.epc`, GO
//! Challenge 3 JSON, and Surge JSON. PowerWorld `.pwb` case files are read
//! only, and GO Challenge 3 JSON has no canonical writer beyond same source
//! echo; `.pwd` display files parse through [`parse_display_file`].
//! Each reader produces a [`Network`]. [`Network::to_format`] returns the
//! serialized target and warnings for fields the target cannot represent. See
//! [`crate::format`] for format routing and fidelity rules.
//!
//! A reader that retains source text can return those bytes when writing the
//! same format. Matrix and problem instance builders live in separate crates.
//!
//! ```
//! use powerio::{parse_str, TargetFormat};
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
//! let net = parse_str(src, "matpower")?.network;
//! assert_eq!(net.buses.len(), 2);
//! assert_eq!(net.to_format(TargetFormat::Matpower)?.text, src);
//! # Ok::<(), powerio::Error>(())
//! ```

/// The powerio crate version, for provenance fields written by downstream
/// crates whose own version can drift from the core's.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod dc;
pub mod error;
pub mod format;
pub mod gen_cost;
pub mod geo;
pub mod indexed;
pub mod network;
mod normalize;
mod operations;
pub mod solver_tables;

pub use dc::DcConvention;
pub use error::{ElementCounts, Error, ErrorCategory, Result, ScenarioMismatch};
pub use format::{
    Conversion, DisplayData, DisplayFormat, Parsed, PwdDisplay, PwdSubstation, PypsaCsvOutputs,
    SourceDocument, TargetFormat, WriteOptions, convert_file, convert_file_with_options,
    convert_str, convert_str_with_options, display_format_from_name, parse_display_bytes,
    parse_display_file, parse_egret_json, parse_file, parse_goc3_json, parse_matpower,
    parse_matpower_file, parse_pandapower_json, parse_powermodels_json, parse_powerworld,
    parse_pslf, parse_psse, parse_str, parse_surge_json, read_pypsa_csv_folder,
    target_format_from_name, write_as, write_as_with_options, write_dir, write_egret_json,
    write_matpower, write_pandapower_json, write_powermodels_json, write_powerworld, write_pslf,
    write_psse, write_psse_rev, write_pypsa_csv_folder, write_surge_json,
};
pub use gen_cost::{GenCostPatch, GenCostPolicyReport, MissingGenCostPolicy, parse_gen_cost_csv};
pub use geo::{
    Canvas, CoordinateSpace, CoordsKind, ElementKey, GeoApplyReport, GeoFeature, GeoGeometry,
    GeoLayer, GeoMeta, GeoParsed, GeoTarget, Location, apply_substation_points, geo_layer_from_pwd,
    pwd_mercator_to_lonlat,
};
pub use indexed::{ConnectivityReport, IndexCore, IndexedNetwork};
pub use network::{
    Area, BalancedNetwork, Branch, BranchCharging, BranchCurrentRatings, BranchRatingSet,
    BranchSolution, Bus, BusId, BusType, DEFAULT_BASE_FREQUENCY, Diagnostic, Extras, GenCaps,
    GenCost, Generator, Hvdc, Impedance, Load, LoadVoltageModel, Network, Shunt, ShuntBlock,
    SolverParams, SourceFormat, Storage, Switch, SwitchedShuntControl, SwitchedShuntMode,
    Transformer3W, TransformerControl, TransformerControlMode, Winding,
};
pub use normalize::{NormalizeOptions, NormalizedNetwork, POWER_MODELS_ANGLE_BOUND_PAD};
pub use operations::Selector;
pub use solver_tables::{
    NORMALIZED_SOLVER_TABLES_PASS, NormalizedSolverTables, SolverArcRow, SolverArcTerminal,
    SolverBranchRow, SolverBusRow, SolverCostRow, SolverGeneratorRow, SolverHvdcRow, SolverLoadRow,
    SolverShuntRow, SolverStorageRow, SolverSwitchRow, SolverTableIndex, SolverTableUnits,
};
