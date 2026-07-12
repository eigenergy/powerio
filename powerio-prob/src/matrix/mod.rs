//! Sparse projections and bundle output for problem instances.

mod bundle;

use powerio_matrix::matrix::incidence::diagonal;
use powerio_matrix::matrix::triplet::CooBuilder;
use powerio_matrix::{
    SparseMatrix, build_flow_map, build_weighted_laplacian, ground_at_each, reference_indicator,
};

use crate::DcOpfInstance;

pub use bundle::{DcOpfBundleMetadata, DcOpfBundleOptions, DcOpfOutputs, write_dcopf_bundle};

/// Sparse matrices for a DC OPF instance.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct DcOpfMatrices {
    pub incidence: SparseMatrix,
    pub laplacian: SparseMatrix,
    pub grounded_laplacian: SparseMatrix,
    pub flow_map: SparseMatrix,
    pub generator_bus: SparseMatrix,
    /// Generator space quadratic cost diagonal.
    pub generator_cost: SparseMatrix,
    pub reference_selector: Vec<f64>,
}

/// Build sparse matrices without reading the source network again.
#[must_use]
pub fn build_dc_opf_matrices(instance: &DcOpfInstance) -> DcOpfMatrices {
    let n = instance.n_buses;
    let m = instance.n_branches();
    let mut incidence = CooBuilder::with_capacity_rect(n, m, 2 * m);
    for column in 0..m {
        incidence.add(instance.branches.from_bus[column], column, 1.0);
        incidence.add(instance.branches.to_bus[column], column, -1.0);
    }
    let incidence = incidence.finish_csr();
    let laplacian = build_weighted_laplacian(&incidence, &instance.branches.b);
    let grounded_laplacian = ground_at_each(&laplacian, &instance.reference_buses);
    let flow_map = build_flow_map(&incidence, &instance.branches.b);

    let n_gen = instance.n_generators();
    let mut generator_bus = CooBuilder::with_capacity_rect(n, n_gen, n_gen);
    for (column, &bus) in instance.generators.bus_of_gen.iter().enumerate() {
        generator_bus.add(bus, column, 1.0);
    }

    DcOpfMatrices {
        incidence,
        laplacian,
        grounded_laplacian,
        flow_map,
        generator_bus: generator_bus.finish_csr(),
        generator_cost: diagonal(&instance.generators.q),
        reference_selector: reference_indicator(n, &instance.reference_buses),
    }
}
