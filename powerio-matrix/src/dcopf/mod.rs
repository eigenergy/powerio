//! DC OPF assembly: the numerical preparation arrays, sparse matrices, and
//! bundle writer derived from a [`DcOpfInstance`].

mod bundle;
pub(crate) mod limits;
pub(crate) mod nodal;
mod prep;
#[cfg(test)]
mod tests;

use crate::matrix::triplet::CooBuilder;
use crate::{
    IndexedNetwork, SparseMatrix, calc_branch_flow_matrix, calc_diagonal, calc_reference_indicator,
    calc_weighted_laplacian, ground_at_each,
};

use crate::Result;
use powerio_prob::DcOpfInstance;
use prep::{DcOpfOptions, apply_instance_semantics, preparation_from_view};

pub use bundle::{DcOpfBundleMetadata, DcOpfBundleOptions, DcOpfOutputs, emit_dcopf_bundle};
pub use prep::{
    DcBranchParameters, DcGeneratorParameters, DcOpfPreparation, NodalGeneratorParameters, Units,
};

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
    /// ([`powerio_prob::merge_zero_impedance_buses`]) or opts into skipping.
    pub skip_zero_impedance: bool,
    /// Give a branch with no thermal rating the bound
    /// [`Branch::synthesize_rate_a`](powerio_tx::Branch::synthesize_rate_a)
    /// states. If false, an absent rating reads as unlimited.
    pub synthesize_unrated_limits: bool,
}

impl DcOpfAssemblyOptions {
    #[must_use]
    pub const fn with_units(mut self, units: Units) -> Self {
        self.units = units;
        self
    }

    #[must_use]
    pub const fn with_skip_zero_impedance(mut self, skip: bool) -> Self {
        self.skip_zero_impedance = skip;
        self
    }

    #[must_use]
    pub const fn with_synthesize_unrated_limits(mut self, synthesize: bool) -> Self {
        self.synthesize_unrated_limits = synthesize;
        self
    }
}

/// Sparse matrices for a DC OPF instance.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct DcOpfMatrices {
    /// Bus by branch incidence matrix, with `+1` at each from bus and `-1` at
    /// each to bus.
    pub bus_branch_incidence: SparseMatrix,
    pub laplacian: SparseMatrix,
    pub grounded_laplacian: SparseMatrix,
    /// Branch by bus angle coefficient matrix over the positive solver
    /// susceptance magnitudes.
    pub branch_flow_matrix: SparseMatrix,
    pub generator_bus: SparseMatrix,
    /// Generator space quadratic cost diagonal.
    pub generator_cost: SparseMatrix,
    pub reference_selector: Vec<f64>,
}

/// Derive the sparse DC OPF matrices from the instance. The instance keeps
/// the typed network; an external solver that needs the contiguous arrays
/// behind these matrices calls [`build_dc_opf_preparation`] instead.
///
/// # Errors
/// A network the selected branch susceptance formula cannot assemble: missing reference
/// coverage, an unresolved zero impedance branch, or an unusable cost curve.
pub fn calc_dc_opf_matrices(
    instance: &DcOpfInstance,
    options: &DcOpfAssemblyOptions,
) -> Result<DcOpfMatrices> {
    Ok(matrices_from_preparation(&build_dc_opf_preparation(
        instance, options,
    )?))
}

/// Derive the complete matrix free DC OPF arrays from the instance: demand,
/// shunt, and phase shift withdrawals, generator costs and bounds with their
/// source row mapping, branch susceptances as positive solver edge weights,
/// thermal limits, angle bounds, and the reference bus set. This is the one
/// numerical assembly the matrix builders, the bundle writer, and external
/// solvers read, published so each consumer formulates over the same arrays
/// instead of re-deriving them from the network. [`DcOpfPreparation`]
/// documents each field's unit and sign.
///
/// # Errors
/// As [`calc_dc_opf_matrices`].
pub fn build_dc_opf_preparation(
    instance: &DcOpfInstance,
    options: &DcOpfAssemblyOptions,
) -> Result<DcOpfPreparation> {
    let view = IndexedNetwork::new(instance.network());
    let objective = crate::opf::compile_objective(instance.objective())?;
    let mut preparation = preparation_from_view(
        &view,
        DcOpfOptions {
            formula: instance.branch_susceptance_formula(),
            units: options.units,
            skip_zero_impedance: options.skip_zero_impedance,
            synthesize_unrated_limits: options.synthesize_unrated_limits,
            objective,
        },
    )?;
    apply_instance_semantics(&mut preparation, instance.network(), instance.constraints())?;
    Ok(preparation)
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
    let laplacian = calc_weighted_laplacian(&incidence, &instance.branches.susceptance_magnitude);
    let grounded_laplacian = ground_at_each(&laplacian, instance.reference_buses.as_ref());
    let branch_flow_matrix =
        calc_branch_flow_matrix(&incidence, &instance.branches.susceptance_magnitude);

    let n_gen = instance.n_generators();
    let mut generator_bus = CooBuilder::with_capacity_rect(n, n_gen, n_gen);
    for (column, &bus) in instance.generators.bus_of_gen.iter().enumerate() {
        generator_bus.add(bus, column, 1.0);
    }

    DcOpfMatrices {
        bus_branch_incidence: incidence,
        laplacian,
        grounded_laplacian,
        branch_flow_matrix,
        generator_bus: generator_bus.finish_csr(),
        generator_cost: calc_diagonal(&instance.generators.q),
        reference_selector: calc_reference_indicator(n, instance.reference_buses.as_ref()),
    }
}
