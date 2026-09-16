//! Sparse matrix and graph projections from PowerIO networks.
//!
//! Outputs include signed incidence, weighted bus Laplacian, MATPOWER Bp/Bpp,
//! Y bus, PTDF, LODF, adjacency, LACPF, and petgraph views. Calculations take the
//! dense [`IndexedNetwork`] view of a [`BalancedNetwork`]. Parsing and emitting
//! belong to the top level `powerio` facade; this crate owns derived matrix and
//! graph calculations.
//!
//! ```
//! use powerio_core::Source;
//! use powerio_matrix::{BuildOptions, IndexedNetwork, calc_bprime_matrix};
//! use powerio_tx::parse;
//!
//! # let case = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/case14.m");
//! let net = parse(Source::open(case)?)?.into_value();
//! let g = IndexedNetwork::new(&net);           // dense [0, n) analysis view
//! let bprime = calc_bprime_matrix(&g, &BuildOptions::default())?;
//! assert_eq!(bprime.rows(), g.n());            // Bp is n×n
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Conventions
//!
//! Public DC operators follow PowerModels: an inductive branch has negative
//! `b`. Their branch by bus incidence matrix `A_pm` gives
//! `B = A_pmᵀ diag(b) A_pm`, with nonpositive diagonals and nonnegative
//! off-diagonals. Solver preparation retains its bus by branch factor
//! `A_s = A_pmᵀ` and uses `w = -b`, so the sparse factor is the positive
//! M-matrix `L = A_s diag(w) A_sᵀ = -B`. Source bus IDs remain on the model;
//! [`IndexedNetwork`] maps them to dense indices in `[0, n)`.
//! `tap == 0` means `tap = 1`. `calc_bprime_matrix` and
//! `calc_bdoubleprime_matrix` follow MATPOWER `makeB`; Y_bus keeps tap
//! magnitudes and phase shifts. Branch
//! terminal admittance is stored per unit. The default public DC formula is
//! `b = -x/(r² + x²)`. [`BranchSusceptanceFormula::TapAdjustedReactance`] uses
//! `b = -1/(x·τ)`; both carry phase shift injection. The full reference is in
//! [the matrix guide](https://eigenergy.github.io/powerio/guide/matrices.html).

// Re-export the balanced model types used by matrix signatures. Parsing,
// emitting, conversion, and display operations stay on their owning crate and
// the top level facade. `Error` and `Result` are this crate's own: the variants
// below are raised here and nowhere in the hub.
pub use powerio_tx::{
    BalancedNetwork, Branch, Bus, BusId, BusType, ConnectivityReport, Extras, GenCost, Generator,
    Hvdc, IndexCore, IndexedNetwork, Load, POWER_MODELS_ANGLE_BOUND_PAD, Shunt, SourceFormat,
    Storage,
};

// Internal compatibility paths used throughout the matrix implementation.
pub(crate) use powerio_tx::{indexed, network};

pub mod diagnostics;
pub mod error;
pub use error::{ElementCounts, Error, PiecewiseCostInvalidity, Result, ScenarioMismatch};

/// Compressed sparse row matrix used by the projection calculations.
pub type SparseMatrix = sprs::CsMat<f64>;

mod ac_jacobian;
mod acopf;
mod dc_operators;
mod dcopf;
pub mod io;
mod lindist3flow;
mod lindist3flow_conic;
mod lindist3flow_devices;
mod lindist3flow_pf;
mod lindist3flow_standard;
pub mod matrix;
mod opf;
pub mod pipeline;
pub mod synth;

pub use ac_jacobian::{PowerFlowJacobian, VoltageCoordinates, calc_power_flow_jacobian};
pub use acopf::{
    AcBranchData, AcBusData, AcGeneratorData, AcOpfAssemblyOptions, AcOpfPreparation,
    AcPfAssemblyOptions, AcPfBusData, AcPfGeneratorData, AcPfPreparation, AcStorageData,
    NodalAcGeneratorData, PreparedAcBusSpecification, build_ac_opf_preparation,
    build_ac_pf_preparation,
};
pub use dc_operators::{DcOperatorOptions, DcOperators, ReferenceConstrainedSystem};
pub use dcopf::{
    DcBranchParameters, DcGeneratorParameters, DcOpfAssemblyOptions, DcOpfBundleMetadata,
    DcOpfBundleOptions, DcOpfMatrices, DcOpfOutputs, DcOpfPreparation, NodalGeneratorParameters,
    Units, build_dc_opf_preparation, calc_dc_opf_matrices, emit_dcopf_bundle,
};
pub use lindist3flow::{
    AffineScalarCoefficients, ConnectionPowerMap, CrossVoltageCoefficients, LinDist3FlowLineData,
    LinDist3FlowNetworkData, LinDist3FlowNodeData, LineDropCoefficients,
    build_lindist3flow_network_data, connection_power_map, cross_voltage_coefficients,
    evaluate_affine, evaluate_cross_voltage, line_drop_coefficients, winding_voltage_coefficients,
};
pub use lindist3flow_conic::{
    LinDist3FlowCone, LinDist3FlowConeOrigin, LinDist3FlowConicProblem,
    LinDist3FlowDecisionVariable, LinDist3FlowEquality, LinDist3FlowEqualityOrigin,
    LinDist3FlowLinearExpression, LinDist3FlowLinearTerm, LinDist3FlowVariableData,
    build_lindist3flow_conic_problem, lindist3flow_values_from_primal,
};
pub use lindist3flow_devices::{
    LinDist3FlowAffineExpression, LinDist3FlowBalanceEquation, LinDist3FlowBalanceRow,
    LinDist3FlowBalanceTerm, LinDist3FlowComplexAffinePower, LinDist3FlowDeviceData,
    LinDist3FlowDispatchChannel, LinDist3FlowGeneratorData, LinDist3FlowLoadData,
    LinDist3FlowPreparation, LinDist3FlowShuntData, LinDist3FlowSourceData, LinDist3FlowVariable,
    LinDist3FlowVoltageTerm, build_lindist3flow_device_data, build_lindist3flow_preparation,
    lindist3flow_connection_incidence,
};
pub use lindist3flow_pf::{
    build_lindist3flow_pf_conic_problem, build_lindist3flow_pf_standard_form,
    build_lindist3flow_pf_standard_form_with_options, evaluate_lindist3flow_pf_limits,
    lindist3flow_pf_values_from_primal, lindist3flow_pf_values_from_standard_primal,
};
pub use lindist3flow_standard::{
    LinDist3FlowScaling, LinDist3FlowStandardCone, LinDist3FlowStandardForm,
    LinDist3FlowStandardFormOptions, LinDist3FlowStandardRowOrigin,
    build_lindist3flow_standard_form, build_lindist3flow_standard_form_with_options,
    lindist3flow_values_from_standard_primal,
};
pub use opf::{AnalysisBranchSource, PiecewiseLinearCost, PreparedObjective};

pub use matrix::multiconductor::{
    AugmentedSystem, DistNode, MulticonductorAdmittance, MulticonductorNodeIndex, NodeRef,
    calc_multiconductor_admittance_matrix,
};
pub use matrix::{
    BranchSusceptanceFormula, BuildOptions, GroundedIndexMap, MatrixStats, Scheme,
    SensitivityMatrices, SensitivityMatrixMetadata, SensitivityMetadata, SensitivityOptions,
    SensitivitySolver, SensitivitySolverPath, ZeroImpedanceRule, ZeroImpedanceSkips,
    calc_adjacency_matrix, calc_admittance_matrix, calc_bdoubleprime_matrix, calc_bprime_matrix,
    calc_diagonal, calc_lacpf_matrix, calc_lodf, calc_ptdf, calc_ptdf_lodf,
    calc_ptdf_lodf_with_options, calc_reference_indicator, calc_susceptance_diagonal,
    calc_unit_vector, calc_weighted_laplacian, calc_zero_impedance_skips, check_sddm, ground_at,
    ground_at_each,
};
pub use pipeline::{
    MatrixKind, Pipeline, PipelineOutputs, RhsKind, calc_matrix, calc_matrix_stats_for_kind,
    calc_zero_impedance_skips_for_kind, sanitize_stem, select_zero_impedance_rule_for_kind,
};

#[cfg(feature = "gridfm")]
pub use io::gridfm::{
    GridfmDataset, GridfmOptions, GridfmOutputs, GridfmSnapshot, GridfmTables, build_gridfm_batch,
    build_gridfm_dataset, emit_gridfm_batch, emit_gridfm_dataset, number_snapshots,
    to_gridfm_record_batches, to_gridfm_record_batches_single,
};
