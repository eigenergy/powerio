//! `netmat`: turns power network case files into structured sparse matrices
//! and graph views for any downstream solver — the signed incidence `A`, the
//! weighted Laplacian `L = A diag(b) Aᵀ` and its slack-grounded form,
//! B'/B''/Y_bus, PTDF/LODF, adjacency, the LACPF block, and DC-OPF instance
//! data — plus a petgraph view.

pub mod case;
pub mod error;
pub mod io;
pub mod matrix;
pub mod opf_pipeline;
pub mod parser;
pub mod pipeline;
pub mod synth;
pub mod tui;

pub use case::{Branch, Bus, ConnectivityReport, GenCost, Generator, MpcCase};
pub use error::{Error, Result};
pub use matrix::{
    BuildOptions, DcConvention, GroundMap, IncidenceParts, MatrixStats, OpfInstance, Scheme,
    Units, build_adjacency, build_bdoubleprime, build_bprime, build_flow_map, build_incidence,
    build_lacpf, build_lodf, build_opf_instance, build_ptdf, build_weighted_laplacian,
    build_ybus, ground_at, sddm_check, susceptance_diag, unit_vector,
};
pub use opf_pipeline::{DcOpfOptions, DcOpfOutputs, write_dcopf_bundle};
pub use parser::{parse_matpower, parse_matpower_file};
pub use pipeline::{MatrixKind, Pipeline, PipelineOutputs, RhsKind};
