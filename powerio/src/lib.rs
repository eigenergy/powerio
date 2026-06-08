//! `powerio`: fast, lossless parsing and a typed data model for power system
//! case files.
//!
//! Parse a MATPOWER `.m` case, work with the typed [`Network`], and write it
//! back out byte-for-byte: `parse → write → parse` reproduces the source,
//! preserving every `mpc.*` field, in-matrix comments, and exact numeric
//! tokens. The crate keeps a small dependency set so other tools can embed it
//! as a parser without a matrix or solver stack; the matrices live in the
//! `powerio-matrix` crate.
//!
//! Readers and writers cover MATPOWER `.m`, PowerModels JSON, PSS/E `.raw`,
//! PowerWorld `.aux`, and EGRET JSON. Every format meets at [`Network`], and
//! [`write_as`] reports whatever a target format cannot represent — see the
//! [`crate::format`] module for the two-tier fidelity contract.
//!
//! ```
//! use powerio::{parse_matpower, write_matpower};
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
//! let net = parse_matpower(src)?;
//! assert_eq!(net.buses.len(), 2);
//! assert_eq!(write_matpower(&net), src); // byte-exact echo of the retained source
//! # Ok::<(), powerio::Error>(())
//! ```

pub mod error;
pub mod format;
pub mod indexed;
pub mod network;

pub use error::{Error, Result};
pub use format::{
    Conversion, TargetFormat, parse, parse_egret_json, parse_matpower, parse_matpower_file,
    parse_powermodels_json, parse_powerworld, parse_psse, parse_str, read_path,
    target_format_from_name, write_as, write_egret_json, write_matpower, write_powermodels_json,
    write_powerworld, write_psse,
};
pub use indexed::{ConnectivityReport, IndexCore, IndexedNetwork};
pub use network::{
    Branch, Bus, BusId, BusType, Extras, GenCaps, GenCost, Generator, Hvdc, Load, Network, Shunt,
    SourceFormat, Storage,
};
