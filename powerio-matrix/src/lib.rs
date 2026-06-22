//! `powerio-matrix`: sparse matrices and graph views for power system case files,
//! built on [`powerio`] (re-exported, so one `use powerio_matrix::...` pulls in
//! both layers).
//!
//! Signed incidence `A`, weighted Laplacian `L = A diag(b) Aᵀ` and its
//! reference-grounded form, B'/B''/Y_bus, PTDF/LODF, adjacency, the LACPF block,
//! and the DC OPF instance bundle, plus a petgraph view. The builders take the
//! dense-indexed [`IndexedNetwork`] view of a [`Network`].
//!
//! ```
//! use powerio_matrix::{parse_file, IndexedNetwork, build_bprime, BuildOptions};
//!
//! # let case = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/case14.m");
//! let net = parse_file(case, None)?.network;   // re-exported from powerio
//! let g = IndexedNetwork::new(&net);           // dense [0, n) analysis view
//! let bprime = build_bprime(&g, &BuildOptions::default())?;
//! assert_eq!(bprime.rows(), g.n());            // B' is n×n
//! # Ok::<(), powerio_matrix::Error>(())
//! ```
//!
//! # Conventions
//!
//! B' and the Laplacians use the positive (M-matrix) form: off-diagonal `< 0`,
//! diagonal `> 0`, `diag = Σ|off-diag|`. Bus ids are 1-based on the
//! model; [`IndexedNetwork`] maps them to a dense `[0, n)`. `tap == 0` means
//! `tap = 1`; B' ignores taps and shifts, B'' keeps taps and zeros shifts,
//! Y_bus keeps both. Branch charging susceptance is stored per unit. DC OPF is
//! bus indexed
//! (`p_g ∈ ℝⁿ`), default susceptance `b = 1/x`, with [`DcConvention::Matpower`]
//! the `1/(x·τ)` plus phase-shift variant. The full reference across every
//! matrix is in
//! [docs/matrices.md](https://github.com/eigenergy/powerio/blob/main/docs/matrices.md).

// Re-export the powerio data layer so this crate is a one-stop import, and so
// the matrix modules' `crate::Error` / `crate::network` / `crate::format` paths
// resolve unchanged after the split.
pub use powerio::{
    Branch, Bus, BusId, BusType, ConnectivityReport, Conversion, DisplayData, DisplayFormat,
    ElementCounts, Error, ErrorCategory, Extras, GenCost, Generator, Hvdc, IndexCore,
    IndexedNetwork, Load, Network, Parsed, PwdDisplay, PwdSubstation, PypsaCsvOutputs, Result,
    ScenarioMismatch, Shunt, SourceFormat, Storage, TargetFormat, convert_file, convert_str,
    display_format_from_name, error, format, indexed, network, parse_display_bytes,
    parse_display_file, parse_file, parse_matpower, parse_matpower_file, parse_pandapower_json,
    parse_powermodels_json, parse_powerworld, parse_pslf, parse_psse, parse_str,
    read_pypsa_csv_folder, target_format_from_name, write_as, write_egret_json, write_matpower,
    write_pandapower_json, write_powermodels_json, write_powerworld, write_psse,
    write_pypsa_csv_folder,
};

pub mod io;
pub mod matrix;
pub mod opf_pipeline;
pub mod pipeline;
pub mod synth;

pub use matrix::{
    BuildOptions, BusCosts, DcConvention, GenCosts, GroundedIndexMap, IncidenceParts,
    LinDist3FlowMatrices, MatrixStats, OpfInstance, Scheme, Units, build_adjacency,
    build_bdoubleprime, build_bprime, build_flow_map, build_incidence, build_lacpf,
    build_lindist3flow, build_lodf, build_opf_instance, build_ptdf, build_ptdf_lodf,
    build_weighted_laplacian, build_ybus, ground_at, ground_at_each, reference_indicator,
    sddm_check, susceptance_diag, unit_vector,
};
pub use opf_pipeline::{DcOpfOptions, DcOpfOutputs, write_dcopf_bundle};
pub use pipeline::{MatrixKind, Pipeline, PipelineOutputs, RhsKind, build_kind};

#[cfg(feature = "gridfm")]
pub use io::gridfm::{
    GridfmOptions, GridfmOutputs, GridfmRead, GridfmSnapshot, GridfmTables, gridfm_base_case,
    gridfm_record_batches, gridfm_record_batches_batch, gridfm_scenario_ids, numbered_snapshots,
    read_gridfm_dataset, read_gridfm_network, read_gridfm_scenarios, write_gridfm_batch,
    write_gridfm_dataset,
};
#[cfg(feature = "gridfm")]
pub use io::{dataset_scenario_ids, read_dataset_dir};
