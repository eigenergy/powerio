//! `powerio-matrix`: sparse matrices and graph views for power system case files,
//! built on [`powerio`] (re-exported, so one `use powerio_matrix::...` pulls in
//! both layers).
//!
//! Signed incidence `A`, weighted Laplacian `L = A diag(b) Aᵀ` and its
//! slack-grounded form, B'/B''/Y_bus, PTDF/LODF, adjacency, the LACPF block,
//! and the DC-OPF instance bundle, plus a petgraph view. The builders take the
//! dense-indexed [`IndexedNetwork`] view of a [`Network`].
//!
//! ```
//! use powerio_matrix::{parse_matpower_file, IndexedNetwork, build_bprime, BuildOptions};
//!
//! # let case = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/case14.m");
//! let net = parse_matpower_file(case)?;        // re-exported from powerio
//! let g = IndexedNetwork::new(&net);           // dense [0, n) analysis view
//! let bprime = build_bprime(&g, &BuildOptions::default())?;
//! assert_eq!(bprime.rows(), g.n());            // B' is n×n
//! # Ok::<(), powerio_matrix::Error>(())
//! ```
//!
//! # Conventions
//!
//! B' and the Laplacians use the positive (M-matrix) form: off-diagonal `< 0`,
//! diagonal `> 0`, `diag = Σ|off-diag|`. Bus ids are MATPOWER 1-based on the
//! model; [`IndexedNetwork`] maps them to a dense `[0, n)`. `tap == 0` means
//! `tap = 1`; B' ignores taps and shifts, B'' keeps taps and zeros shifts,
//! Y_bus keeps both. `BR_B` is already per unit. DC-OPF is bus-indexed
//! (`p_g ∈ ℝⁿ`), default susceptance `b = 1/x`, with [`DcConvention::Matpower`]
//! the `1/(x·τ)` plus phase-shift variant. The full reference across every
//! matrix is in
//! [docs/matrices.md](https://github.com/eigenergy/powerio/blob/main/docs/matrices.md).

// Re-export the powerio data layer so this crate is a one-stop import, and so
// the matrix modules' `crate::Error` / `crate::network` / `crate::format` paths
// resolve unchanged after the split.
pub use powerio::{
    Branch, Bus, BusId, BusType, ConnectivityReport, Conversion, ElementCounts, Error,
    ErrorCategory, Extras, GenCost, Generator, Hvdc, IndexCore, IndexedNetwork, Load, Network,
    Result, ScenarioMismatch, Shunt, SourceFormat, Storage, TargetFormat, error, format, indexed,
    network, parse, parse_matpower, parse_matpower_file, parse_powermodels_json, parse_powerworld,
    parse_psse, parse_str, read_path, target_format_from_name, write_as, write_egret_json,
    write_matpower, write_powermodels_json, write_powerworld, write_psse,
};

pub mod io;
pub mod matrix;
pub mod opf_pipeline;
pub mod pipeline;
pub mod synth;

pub use matrix::{
    BuildOptions, BusCosts, DcConvention, GenCosts, GroundMap, IncidenceParts, MatrixStats,
    OpfInstance, Scheme, Units, build_adjacency, build_bdoubleprime, build_bprime, build_flow_map,
    build_incidence, build_lacpf, build_lodf, build_opf_instance, build_ptdf, build_ptdf_lodf,
    build_weighted_laplacian, build_ybus, ground_at, sddm_check, susceptance_diag, unit_vector,
};
pub use opf_pipeline::{DcOpfOptions, DcOpfOutputs, write_dcopf_bundle};
pub use pipeline::{MatrixKind, Pipeline, PipelineOutputs, RhsKind, build_kind};

#[cfg(feature = "gridfm")]
pub use io::gridfm::{
    GridfmOptions, GridfmOutputs, GridfmSnapshot, GridfmTables, gridfm_record_batches,
    gridfm_record_batches_batch, numbered_snapshots, write_gridfm_batch, write_gridfm_dataset,
};
