//! The parts of the CLI that are worth running from somewhere other than the
//! command line.
//!
//! [`invariants`] holds the conversion properties: what must survive a format
//! conversion, stated once. `powerio-cli/tests/conversion_matrix_report.rs`
//! holds the vendored cases to them, grades each conversion by its worst
//! finding, and compares the grades with `tests/conversion_matrix.baseline`.
//! [`corpus`] holds an arbitrary corpus to the same properties, so the gate in
//! CI and the harness pointed at private data cannot drift apart.
//!
//! The `powerio` binary is the CLI proper; everything else about it lives in
//! `src/main.rs`.

pub mod corpus;
pub mod invariants;
pub(crate) mod module_io;
