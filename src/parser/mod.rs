//! Format dispatcher. One submodule per supported file format.
//!
//! - [`matpower`] — MATPOWER 7.x `.m` (transmission, balanced).
//! - `opendss` — OpenDSS `.dss` feeders. *(planned)*
//! - `psse` — PSS/E `.raw`. *(planned)*
//! - `pglib_json` — PowerModels.jl JSON. *(planned)*

pub mod matpower;

pub use matpower::{parse_matpower, parse_matpower_file};
