//! Sparse matrix and graph projections from PowerIO networks.
//!
//! Outputs include signed incidence, weighted bus Laplacian, MATPOWER Bp/Bpp,
//! Y bus, PTDF, LODF, adjacency, LACPF, and petgraph views. Builders take the
//! dense [`IndexedNetwork`] view of a [`Network`]. The crate reexports
//! [`powerio`] types and functions.
//!
//! ```
//! use powerio_matrix::{parse_file, IndexedNetwork, build_bprime, BuildOptions};
//!
//! # let case = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/case14.m");
//! let net = parse_file(case, None)?.network;   // re-exported from powerio
//! let g = IndexedNetwork::new(&net);           // dense [0, n) analysis view
//! let bprime = build_bprime(&g, &BuildOptions::default())?;
//! assert_eq!(bprime.rows(), g.n());            // Bp is n×n
//! # Ok::<(), powerio_matrix::Error>(())
//! ```
//!
//! # Conventions
//!
//! The DC bus susceptance matrix and other weighted bus Laplacians use the
//! positive M-matrix form: stored nonzero off-diagonal entries are negative,
//! diagonals are nonnegative, and `diag = Σ|off-diag|`. Source bus IDs remain on
//! the model; [`IndexedNetwork`] maps them to dense indices in `[0, n)`. `tap == 0` means
//! `tap = 1`. `build_bprime` and `build_bdoubleprime` follow MATPOWER `makeB`;
//! Y_bus keeps tap magnitudes and phase shifts.
//! Branch terminal admittance is stored per unit. DC incidence uses `b = 1/x`
//! by default. [`DcConvention::Matpower`] uses `1/(x·τ)` and phase shift
//! injection. The full reference across every matrix is in
//! [the matrix guide](https://eigenergy.github.io/powerio/guide/matrices.html).

// Re-export the powerio data layer so one import covers model and matrix types, and so
// the matrix modules' `crate::Error` / `crate::network` / `crate::format` paths
// resolve unchanged after the split.
pub use powerio::{
    Branch, Bus, BusId, BusType, ConnectivityReport, Conversion, DisplayData, DisplayFormat,
    ElementCounts, Error, ErrorCategory, Extras, GenCost, GenCostPatch, GenCostPolicyReport,
    Generator, Hvdc, IndexCore, IndexedNetwork, Load, MissingGenCostPolicy, Network,
    NormalizeOptions, NormalizedNetwork, POWER_MODELS_ANGLE_BOUND_PAD, Parsed, PwdDisplay,
    PwdSubstation, PypsaCsvOutputs, Result, ScenarioMismatch, Shunt, SourceFormat, Storage,
    TargetFormat, WriteOptions, convert_file, convert_file_with_options, convert_str,
    convert_str_with_options, display_format_from_name, error, format, gen_cost, geo, indexed,
    network, parse_display_bytes, parse_display_file, parse_file, parse_gen_cost_csv,
    parse_matpower, parse_matpower_file, parse_pandapower_json, parse_powermodels_json,
    parse_powerworld, parse_pslf, parse_psse, parse_str, read_pypsa_csv_folder,
    target_format_from_name, write_as, write_as_with_options, write_egret_json, write_matpower,
    write_pandapower_json, write_powermodels_json, write_powerworld, write_psse,
    write_pypsa_csv_folder,
};

/// Compressed sparse row matrix used by the projection builders.
pub type SparseMatrix = sprs::CsMat<f64>;

pub mod io;
pub mod matrix;
pub mod pipeline;
pub mod synth;

pub use matrix::{
    BuildOptions, DcConvention, GroundedIndexMap, IncidenceParts, MatrixStats, Scheme,
    SensitivityMatrices, SensitivityMatrixMetadata, SensitivityMetadata, SensitivityOptions,
    SensitivitySolver, SensitivitySolverPath, ZeroImpedanceRule, ZeroImpedanceSkips,
    build_adjacency, build_bdoubleprime, build_bprime, build_flow_map, build_incidence,
    build_lacpf, build_lodf, build_ptdf, build_ptdf_lodf, build_ptdf_lodf_with_options,
    build_weighted_laplacian, build_ybus, ground_at, ground_at_each, reference_indicator,
    sddm_check, skipped_zero_impedance, susceptance_diag, unit_vector,
};
pub use pipeline::{
    MatrixKind, Pipeline, PipelineOutputs, RhsKind, build_kind, matrix_stats_for_kind,
    zero_impedance_rule_for_kind, zero_impedance_skips_for_kind,
};

#[cfg(feature = "gridfm")]
pub use io::gridfm::{
    GridfmOptions, GridfmOutputs, GridfmRead, GridfmSnapshot, GridfmTables, gridfm_base_case,
    gridfm_record_batches, gridfm_record_batches_batch, gridfm_scenario_ids, numbered_snapshots,
    read_gridfm_dataset, read_gridfm_network, read_gridfm_scenarios, write_gridfm_batch,
    write_gridfm_dataset,
};
#[cfg(feature = "gridfm")]
pub use io::{dataset_scenario_ids, read_dataset_dir};
