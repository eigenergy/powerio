//! `casemat`: sparse matrices and graph views for power-system case files,
//! built on [`caseio`].
//!
//! Signed incidence `A`, weighted Laplacian `L = A diag(b) Aᵀ` and its
//! slack-grounded form, B'/B''/Y_bus, PTDF/LODF, adjacency, the LACPF block,
//! and the DC-OPF instance bundle, plus a petgraph view and a TUI.

// Re-export the caseio data layer so this crate is a one-stop import, and so
// the matrix modules' `crate::case` / `crate::Error` / `crate::parser` paths
// resolve unchanged after the split.
pub use caseio::{
    case, error, format, network, parser, parse_matpower, parse_matpower_file,
    parse_powermodels_json, parse_powerworld, parse_psse, write_as, write_egret_json, write_matpower,
    write_powermodels_json, write_powerworld, write_psse, Branch, Bus,
    ConnectivityReport, Conversion, DcLine, Error, GenCost, Generator, MpcCase, Network, Result,
    SourceFormat, Storage, TargetFormat,
};

pub mod io;
pub mod matrix;
pub mod opf_pipeline;
pub mod pipeline;
pub mod synth;
#[cfg(feature = "cli")]
pub mod tui;

pub use matrix::{
    build_adjacency, build_bdoubleprime, build_bprime, build_flow_map, build_incidence,
    build_lacpf, build_lodf, build_opf_instance, build_ptdf, build_weighted_laplacian, build_ybus,
    ground_at, sddm_check, susceptance_diag, unit_vector, BuildOptions, DcConvention, GroundMap,
    IncidenceParts, MatrixStats, OpfInstance, Scheme, Units,
};
pub use opf_pipeline::{write_dcopf_bundle, DcOpfOptions, DcOpfOutputs};
pub use pipeline::{MatrixKind, Pipeline, PipelineOutputs, RhsKind};
