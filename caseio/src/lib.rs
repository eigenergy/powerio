//! `caseio`: fast, lossless parsing and a typed data model for power-system
//! case files.
//!
//! Parse a MATPOWER `.m` case, work with the typed [`Network`], and write it
//! back out byte-for-byte — `parse → write → parse` reproduces the source,
//! preserving every `mpc.*` field, in-matrix comments, and exact numeric
//! tokens. The crate is dependency-light on purpose so other tools can take it
//! as a parser without a matrix/solver stack; the matrices live in `casemat`.

pub mod error;
pub mod format;
pub mod indexed;
pub mod network;

pub use error::{Error, Result};
pub use format::{
    Conversion, TargetFormat, parse, parse_matpower, parse_matpower_file, parse_powermodels_json,
    parse_powerworld, parse_psse, parse_str, read_path, target_format_from_name, write_as,
    write_egret_json, write_matpower, write_powermodels_json, write_powerworld, write_psse,
};
pub use indexed::{ConnectivityReport, IndexCore, IndexedNetwork};
pub use network::{
    Branch, Bus, BusType, Extras, GenCost, Generator, Hvdc, Load, Network, Shunt, SourceFormat,
    Storage,
};
