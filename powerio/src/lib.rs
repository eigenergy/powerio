//! PowerIO: compiler infrastructure for power system data.
//!
//! The short `powerio` name is the entry facade over the component crates:
//! `powerio-core` (sources, diagnostics, errors, modules), `powerio-tx`
//! (the balanced transmission model and its format parsers and writers),
//! `powerio-dist` (the multiconductor distribution model), and `powerio-prob`
//! (operating points, problem instances, and solutions). The facade owns the
//! dynamic value boundary: `PioValue`, `PioValueKind`, universal format
//! dispatch, `try_into_typed`, and the `.pio.json` stored document.
//!
//! This revision re-exports the balanced implementation surface while the
//! dynamic value, dispatch, and stored document land in the following
//! commits on this branch.

pub use powerio_tx::*;
