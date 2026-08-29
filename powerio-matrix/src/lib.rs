//! Sparse matrix and graph projections from PowerIO networks.
//!
//! Outputs include signed incidence, weighted bus Laplacian, MATPOWER Bp/Bpp,
//! Y bus, PTDF, LODF, adjacency, LACPF, and petgraph views. Builders take the
//! dense [`IndexedNetwork`] view of a [`BalancedNetwork`]. The crate reexports
//! [`powerio_tx`] types and functions.
//!
//! ```
//! use powerio_matrix::{BuildOptions, IndexedNetwork, build_bprime, parse};
//!
//! # let case = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/case14.m");
//! let net = parse(powerio_core::Source::open(case)?)?.into_value();
//! let g = IndexedNetwork::new(&net);           // dense [0, n) analysis view
//! let bprime = build_bprime(&g, &BuildOptions::default())?;
//! assert_eq!(bprime.rows(), g.n());            // Bp is n×n
//! # Ok::<(), Box<dyn std::error::Error>>(())
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
//! Branch terminal admittance is stored per unit. DC incidence uses
//! `b = x/(r² + x²)` by default. [`DcConvention::TapAdjustedReactance`] uses `1/(x·τ)`, and
//! both carry phase shift injection. The full reference across every matrix is in
//! [the matrix guide](https://eigenergy.github.io/powerio/guide/matrices.html).

// Re-export the powerio data layer so one import covers model and matrix types,
// and so the matrix modules' `crate::network` / `crate::format` paths resolve
// unchanged after the split. `Error` and `Result` are this crate's own: the
// variants below are raised here and nowhere in the hub.
pub use powerio_tx::{
    BalancedNetwork, Branch, Bus, BusId, BusType, ConnectivityReport, Conversion, DisplayData,
    DisplayFormat, ErrorCategory, Extras, GenCost, GenCostPatch, GenCostPolicyReport, Generator,
    Hvdc, IndexCore, IndexedNetwork, Load, MissingGenCostPolicy, NormalizeOptions,
    NormalizedNetwork, POWER_MODELS_ANGLE_BOUND_PAD, PwdDisplay, PwdSubstation, PypsaCsvOutputs,
    Shunt, SourceFormat, Storage, TargetFormat, WriteOptions, convert_file,
    convert_file_with_options, convert_str, convert_str_with_options, display_format_from_name,
    format, gen_cost, geo, indexed, network, parse, parse_display_bytes, parse_display_file,
    parse_gen_cost_csv, target_format_from_name, write_as, write_as_with_options, write_dir,
    write_dir_with_options, write_egret_json, write_matpower, write_network, write_pandapower_json,
    write_powermodels_json, write_powerworld, write_psse, write_pypsa_csv_folder,
};

mod collect;
pub mod diagnostics;
pub mod error;
pub use error::{ElementCounts, Error, Result, ScenarioMismatch};

/// The hub's error, so a binding can map both through one taxonomy.
pub use powerio_tx::Error as CoreError;

/// Compressed sparse row matrix used by the projection builders.
pub type SparseMatrix = sprs::CsMat<f64>;

mod ac_jacobian;
mod dc_operators;
mod dcopf;
pub mod io;
pub mod matrix;
pub mod pipeline;
pub mod synth;

pub use ac_jacobian::{PowerFlowJacobian, VoltageCoordinates, calc_power_flow_jacobian};
pub use dc_operators::{DcOperators, ReferenceConstrainedSystem};
pub use dcopf::{
    DcOpfAssemblyOptions, DcOpfBundleMetadata, DcOpfBundleOptions, DcOpfMatrices, DcOpfOutputs,
    Units, build_dc_opf_matrices, write_dcopf_bundle,
};

pub use matrix::multiconductor::{
    AugmentedSystem, DistNode, MulticonductorAdmittance, MulticonductorNodeIndex, NodeRef,
    build_multiconductor_admittance,
};
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
    sanitize_stem, zero_impedance_rule_for_kind, zero_impedance_skips_for_kind,
};

#[cfg(feature = "gridfm")]
pub use io::gridfm::{
    GridfmOptions, GridfmOutputs, GridfmRead, GridfmSnapshot, GridfmTables, gridfm_base_case,
    gridfm_record_batches, gridfm_record_batches_single, gridfm_scenario_ids, numbered_snapshots,
    read_gridfm_dataset, read_gridfm_network, read_gridfm_scenarios, write_gridfm_batch,
    write_gridfm_dataset,
};
#[cfg(feature = "gridfm")]
pub use io::{dataset_scenario_ids, read_dataset_dir};
