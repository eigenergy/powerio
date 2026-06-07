//! `caseio`: fast, lossless parsing and a typed data model for power-system
//! case files.
//!
//! Parse a MATPOWER `.m` case, work with the typed [`MpcCase`], and write it
//! back out byte-for-byte — `parse → write → parse` reproduces the source,
//! preserving every `mpc.*` field, in-matrix comments, and exact numeric
//! tokens. The crate is dependency-light on purpose so other tools can take it
//! as a parser without a matrix/solver stack; the matrices live in `casemat`.

pub mod case;
pub mod error;
pub mod format;
pub mod indexed;
pub mod network;
pub mod parser;

pub use case::{Branch, Bus, DcLine, GenCost, Generator, MpcCase, Storage};
pub use error::{Error, Result};
pub use format::{
    parse_powermodels_json, parse_powerworld, parse_psse, write_as, write_egret_json,
    write_powermodels_json, write_powerworld, write_psse, Conversion, TargetFormat,
};
pub use indexed::{ConnectivityReport, IndexedNetwork};
pub use network::{Network, SourceFormat};
pub use parser::{parse_matpower, parse_matpower_file, write_matpower};
