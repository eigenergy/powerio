//! `powerio`: lossless parsing and a typed data model for power system case
//! files.
//!
//! Readers and writers cover MATPOWER `.m`, PowerModels JSON, PSS/E `.raw`,
//! PowerWorld `.aux`, pandapower JSON, PyPSA CSV, egret JSON, and PSLF `.epc`.
//! PowerWorld `.pwb` case files are read only; `.pwd` display files parse through
//! [`parse_display_file`]. Case formats meet at the typed [`Network`], and
//! [`Network::to_format`] reports whatever a target format cannot represent.
//! See the [`crate::format`] module for the two-tier fidelity contract.
//!
//! Writing back to the source format reproduces the file byte for byte:
//! `parse → write → parse` returns the original text, down to comments and
//! exact numeric tokens. The crate keeps a small dependency set so other
//! tools can embed it as a parser without a matrix or solver stack; the
//! matrices live in the `powerio-matrix` crate.
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

pub mod error;
pub mod format;
pub mod indexed;
pub mod network;
mod normalize;
mod operations;

pub use error::{ElementCounts, Error, ErrorCategory, Result, ScenarioMismatch};
pub use format::{
    Conversion, DisplayData, DisplayFormat, Parsed, PwdDisplay, PwdSubstation, PypsaCsvOutputs,
    TargetFormat, convert_file, convert_str, display_format_from_name, parse_display_bytes,
    parse_display_file, parse_egret_json, parse_file, parse_matpower, parse_matpower_file,
    parse_pandapower_json, parse_powermodels_json, parse_powerworld, parse_pslf, parse_psse,
    parse_str, read_pypsa_csv_folder, target_format_from_name, write_as, write_dir,
    write_egret_json, write_matpower, write_pandapower_json, write_powermodels_json,
    write_powerworld, write_pslf, write_psse, write_psse_rev, write_pypsa_csv_folder,
};
pub use indexed::{ConnectivityReport, IndexCore, IndexedNetwork};
pub use network::{
    Area, BalancedNetwork, Branch, BranchCharging, BranchCurrentRatings, BranchSolution, Bus,
    BusId, BusType, DEFAULT_BASE_FREQUENCY, Diagnostic, Extras, GenCaps, GenCost, Generator, Hvdc,
    Impedance, Load, LoadVoltageModel, Network, Shunt, ShuntBlock, SolverParams, SourceFormat,
    Storage, Switch, SwitchedShuntControl, SwitchedShuntMode, Transformer3W, TransformerControl,
    TransformerControlMode, Winding,
};
pub use operations::Selector;
