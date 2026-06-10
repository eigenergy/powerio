//! OpenDSS `.dss` support: tokenizer, RPN, class tables, raw object layer.
//!
//! The semantics mirror the OpenDSS reference implementation
//! (epri-dev/OpenDSS-C): TParser tokenization, executive command dispatch
//! with prefix abbreviation, property resolution in class definition order,
//! and the TRPNCalc expression calculator.

pub mod defaults;
pub mod lex;
pub mod prop;
pub mod raw;
pub mod read;
mod rpn;
mod write;

pub use lex::{BusSpec, Param, Scanner, Value, VarMap};
pub use raw::{BusCoord, RawCommand, RawDss, RawObject, RawProp, parse_raw_file, parse_raw_with};
pub use read::{network_from_raw, parse_dss_file, parse_dss_str};
pub use write::write_dss;
