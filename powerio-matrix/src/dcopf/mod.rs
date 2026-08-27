//! DC OPF assembly: the private preparation arrays, sparse matrices, and
//! bundle writer derived from a [`DcOpfInstance`].

mod bundle;
mod limits;
mod nodal;
mod prep;
#[cfg(test)]
mod tests;

use crate::matrix::incidence::diagonal;
use crate::matrix::triplet::CooBuilder;
use crate::{
    IndexedNetwork, SparseMatrix, build_flow_map, build_weighted_laplacian, ground_at_each,
    reference_indicator,
};

use crate::Result;
use powerio_prob::DcOpfInstance;
use prep::{DcOpfOptions, DcOpfPreparation, build_dc_opf_preparation};

pub use bundle::{DcOpfBundleMetadata, DcOpfBundleOptions, DcOpfOutputs, write_dcopf_bundle};
pub use prep::Units;

/// Assembly choices that select the numerical content derived from an
/// instance without changing the instance itself.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct DcOpfAssemblyOptions {
    /// Power and cost scaling of the derived arrays.
    pub units: Units,
    /// Skip non-self-loop branches with zero reactance. Off by default:
    /// zero impedance branches are preserved in networks and instances, so
    /// assembly refuses them until the caller resolves them explicitly
    /// ([`crate::merge_zero_impedance_buses`]) or opts into skipping.
    pub skip_zero_impedance: bool,
    /// Give a branch with no thermal rating the bound
    /// [`Branch::synthesize_rate_a`](powerio_tx::Branch::synthesize_rate_a)
    /// states. If false, an absent rating reads as unlimited.
    pub synthesize_unrated_limits: bool,
}

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

/// Derive the sparse DC OPF matrices from the instance. The instance keeps
/// the typed network; the contiguous preparation arrays are private.
///
/// # Errors
/// A network the selected approximation cannot assemble: missing reference
/// coverage, an unresolved zero impedance branch, or an unusable cost curve.
pub fn build_dc_opf_matrices(
    instance: &DcOpfInstance,
    options: &DcOpfAssemblyOptions,
) -> Result<DcOpfMatrices> {
    Ok(matrices_from_preparation(&prepare(instance, *options)?))
}

/// The private preparation arrays behind the matrix and bundle builders.
pub(crate) fn prepare(
    instance: &DcOpfInstance,
    options: DcOpfAssemblyOptions,
) -> Result<DcOpfPreparation> {
    let view = IndexedNetwork::new(instance.network());
    build_dc_opf_preparation(
        &view,
        DcOpfOptions {
            convention: instance.approximation(),
            units: options.units,
            skip_zero_impedance: options.skip_zero_impedance,
            synthesize_unrated_limits: options.synthesize_unrated_limits,
        },
    )
}

pub(crate) fn matrices_from_preparation(instance: &DcOpfPreparation) -> DcOpfMatrices {
    let n = instance.n_buses;
    let m = instance.n_branches();
    let mut incidence = CooBuilder::with_capacity_rect(n, m, 2 * m);
    for column in 0..m {
        incidence.add(instance.branches.from_bus[column], column, 1.0);
        incidence.add(instance.branches.to_bus[column], column, -1.0);
    }
    let incidence = incidence.finish_csr();
    let laplacian = build_weighted_laplacian(&incidence, &instance.branches.b);
    let grounded_laplacian = ground_at_each(&laplacian, instance.reference_buses.as_ref());
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
        reference_selector: reference_indicator(n, instance.reference_buses.as_ref()),
    }
}
