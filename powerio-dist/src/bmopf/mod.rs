//! The draft BMOPF task force JSON schema (frederikgeth/bmopf-report).
//!
//! Everything is explicit SI: volts, watts, vars, ohms, siemens, meters,
//! radians, string bus ids and terminal names. The schema sets
//! `additionalProperties: false` on every element, so the strict writer
//! drops what the schema cannot carry and says so per field; the dropped
//! data stays in the model's `extras`, never in the emitted JSON.

mod read;
mod write;

pub use read::{parse_bmopf_file, parse_bmopf_str};
pub use write::{BmopfWriteOptions, write_bmopf_json, write_bmopf_json_with_options};
