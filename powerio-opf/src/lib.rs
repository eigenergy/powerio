//! `powerio-opf`: OPF instance generators built on the `powerio` core.
//!
//! An instance is the numeric problem data for one class of optimal power flow,
//! keyed by stable ids and per class position indices, with no model-specific
//! variable stacking and no solver assumptions baked in. A consumer reads an
//! instance and builds its own model (or program) and, after solving, a
//! solution; instance, model, and solution sit at different layers, and this
//! crate stops at the first.
//!
//! The instances here are index based and carry no matrices, so the crate
//! depends only on `powerio`. The generator→bus map is the `bus_of_col` index
//! vector rather than a sparse `C_g`. `powerio-matrix` keeps the graphical
//! DC-OPF builder — its `OpfInstance`, the sparse `C_g`, and the Matrix Market
//! bundle writer — for consumers that want that form. The two crates are
//! independent siblings on `powerio`.
//!
//! Family status:
//!
//! - [`DcOpfInstance`]: shipped. Bus-indexed cost, bounds, thermal limits, the
//!   generator→bus index map, and nodal load, built by [`build_dc_opf_instance`].
//! - [`AcOpfInstance`]: skeleton only; no builder yet.
//! - `ScopfInstance`: not yet started (eigenergy/powerio#235). PowerIO.jl builds
//!   the security-constrained instance as a Julia-side projection over the
//!   parsed GOC3 case until the Rust IR can represent reserves, contingencies,
//!   and cross-period energy budgets.

pub mod ac;
pub mod dc;
mod error;

pub use ac::AcOpfInstance;
pub use dc::{BusCosts, DcOpfInstance, GenCosts, Units, build_dc_opf_instance, project_gen_to_bus};
pub use error::{Error, Result};
