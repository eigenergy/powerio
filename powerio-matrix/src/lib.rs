//! `powerio-matrix`: sparse matrices and graph views for power system case files,
//! built on [`powerio`].
//!
//! Signed incidence `A`, weighted Laplacian `L = A diag(b) Aᵀ` and its
//! slack-grounded form, B'/B''/Y_bus, PTDF/LODF, adjacency, the LACPF block,
//! and the DC-OPF instance bundle, plus a petgraph view and a TUI.

// Re-export the powerio data layer so this crate is a one-stop import, and so
// the matrix modules' `crate::Error` / `crate::network` / `crate::format` paths
// resolve unchanged after the split.
pub use powerio::{
    Branch, Bus, BusType, ConnectivityReport, Conversion, Error, Extras, GenCost, Generator, Hvdc,
    IndexCore, IndexedNetwork, Load, Network, Result, Shunt, SourceFormat, Storage, TargetFormat,
    error, format, indexed, network, parse, parse_matpower, parse_matpower_file,
    parse_powermodels_json, parse_powerworld, parse_psse, parse_str, read_path,
    target_format_from_name, write_as, write_egret_json, write_matpower, write_powermodels_json,
    write_powerworld, write_psse,
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
