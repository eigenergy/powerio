//! `mpower-bmat`: parse MATPOWER `.m` cases and emit Laplacian/SDDM-shaped
//! matrices (FDPF B', B'', Y-bus G/B, LACPF block) for the Scalable
//! Approximate Cholesky solver.

pub mod case;
pub mod error;
pub mod io;
pub mod matrix;
pub mod parser;
pub mod pipeline;
pub mod synth;
pub mod tui;

pub use case::{Branch, Bus, ConnectivityReport, MpcCase};
pub use error::{Error, Result};
pub use matrix::{
    BuildOptions, MatrixStats, Scheme, build_bdoubleprime, build_bprime, build_lacpf,
    build_ybus, sddm_check,
};
pub use parser::{parse_matpower, parse_matpower_file};
pub use pipeline::{MatrixKind, Pipeline, PipelineOutputs, RhsKind};
