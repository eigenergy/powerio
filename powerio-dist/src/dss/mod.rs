//! OpenDSS `.dss` support: tokenizer, RPN, class tables, raw object layer.
//!
//! The semantics mirror the OpenDSS reference implementation
//! (epri-dev/OpenDSS-C): TParser tokenization, executive command dispatch
//! with prefix abbreviation, property resolution in class definition order,
//! and the TRPNCalc expression calculator.

pub mod defaults;
mod emit_project;
mod frequency;
mod geometry;
pub mod lex;
mod line;
mod line_history;
mod linecode;
mod load_power;
mod loadshape;
mod primitive;
pub mod prop;
pub mod raw;
pub mod read;
mod rpn;
mod solution_mode;
mod source;
mod write;
mod xfmrcode;

pub use lex::{BusSpec, Param, Scanner, Value, VarMap};
pub use raw::{BusCoord, RawCommand, RawDss, RawObject, RawProp};

pub(crate) use emit_project::retained_project;
#[doc(hidden)]
pub use raw::{__parse_raw_file, __parse_raw_with};
pub use read::to_network_from_raw;
pub use write::{DssEmitOptions, DssLoadVoltageBounds};
pub(crate) use write::{emit_dss_text_with_options, validate_emission};
