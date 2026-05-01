//! Format-agnostic parsing layer.
//!
//! Each supported file format gets its own submodule. The top-level
//! `parse_file` will eventually sniff by extension and dispatch; for now
//! callers should use the format-specific entry points.
//!
//! - [`matpower`] — MATPOWER 7.x `.m` files (transmission, balanced).
//! - `opendss` — OpenDSS `.dss` feeders (distribution, unbalanced 3-phase). *(planned)*
//! - `psse` — PSS/E `.raw` files (transmission planning). *(planned)*
//! - `pglib_json` — PowerModels.jl JSON. *(planned)*

pub mod matpower;

pub use matpower::{parse_matpower, parse_matpower_file};
