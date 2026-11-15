//! DC OPF assembly: the numerical preparation arrays, sparse matrices, and
//! bundle writer derived from a [`DcOpfInstance`].

mod bundle;
pub(crate) mod limits;
pub(crate) mod nodal;
mod prep;
#[cfg(test)]
mod tests;

use crate::matrix::calc_solver_branch_flow_matrix;
use crate::{
    IndexedNetwork, SparseMatrix, calc_diagonal, calc_reference_indicator, calc_weighted_laplacian,
    ground_at_each,
};

use crate::Result;
use powerio_prob::DcOpfInstance;
use powerio_tx::AngleDifferenceBounds;
use powerio_tx::HvdcTreatment;
use prep::{DcOpfOptions, apply_instance_semantics, preparation_from_view};

pub use bundle::{DcOpfBundleMetadata, DcOpfBundleOptions, DcOpfOutputs, emit_dcopf_bundle};
pub use limits::SynthesizedLimitWindow;
pub use prep::{
    DcBranchParameters, DcGeneratorParameters, DcOpfPreparation, NodalGeneratorParameters, Units,
};

/// Assembly choices that select the numerical content derived from an
/// instance without changing the instance itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct DcOpfAssemblyOptions {
    /// Power and cost scaling of the derived arrays.
    pub units: Units,
    /// Skip non-self-loop branches with an unusable formula denominator.
    /// Series-admittance formulas bound `hypot(r, x)`; reciprocal-reactance
    /// formulas bound `abs(x)`. Purely resistive branches remain as zero-weight
    /// rows for series-admittance formulas. Off by default:
    /// zero impedance branches are preserved in networks and instances, so
    /// assembly refuses them until the caller resolves them explicitly
    /// ([`powerio_prob::merge_zero_impedance_buses`]) or opts into skipping.
    pub skip_zero_impedance: bool,
    /// Give a branch with no thermal rating the bound
    /// [`Branch::synthesize_rate_a`](powerio_tx::Branch::synthesize_rate_a)
    /// states. If false, an absent rating reads as unlimited.
    pub synthesize_unrated_limits: bool,
    /// The angle difference interval a synthesized limit spans: the source's
    /// stated one (the default) or the selected bounds the preparation
    /// carries ([`SynthesizedLimitWindow`]).
    pub synthesized_limit_window: SynthesizedLimitWindow,
    /// Apply PowerModels' ±60 degree correction to unconstrained or unusable
    /// branch angle difference intervals in the prepared arrays.
    pub correct_angle_difference_bounds: bool,
    /// How in service HVDC lines enter the fixed withdrawal. The default
    /// carries each as a fixed injection at its stated terminal powers
    /// ([`DcOpfPreparation::p_hvdc`]).
    pub hvdc_treatment: HvdcTreatment,
    /// The angle difference bounds selected with
    /// `with_angle_difference_bounds`. `None`, the default, leaves the choice
    /// to `correct_angle_difference_bounds`
    /// ([`AngleDifferenceBounds::from_correct_flag`]).
    pub angle_difference_bounds: Option<AngleDifferenceBounds>,
}

impl Default for DcOpfAssemblyOptions {
    fn default() -> Self {
        Self {
            units: Units::default(),
            skip_zero_impedance: false,
            synthesize_unrated_limits: false,
            synthesized_limit_window: SynthesizedLimitWindow::Stated,
            correct_angle_difference_bounds: true,
            hvdc_treatment: HvdcTreatment::default(),
            angle_difference_bounds: None,
        }
    }
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

    /// Select the angle difference interval a synthesized limit spans.
    #[must_use]
    pub const fn with_synthesized_limit_window(mut self, window: SynthesizedLimitWindow) -> Self {
        self.synthesized_limit_window = window;
        self
    }

    /// Turn PowerModels' angle difference pad on or off. This clears a
    /// selection made with `with_angle_difference_bounds`.
    #[must_use]
    pub const fn with_correct_angle_difference_bounds(mut self, correct: bool) -> Self {
        self.correct_angle_difference_bounds = correct;
        self.angle_difference_bounds = None;
        self
    }

    #[must_use]
    pub const fn with_hvdc_treatment(mut self, treatment: HvdcTreatment) -> Self {
        self.hvdc_treatment = treatment;
        self
    }

    /// Select the angle difference bounds the prepared arrays carry. The
    /// selection takes precedence over `correct_angle_difference_bounds`,
    /// which it sets to whether the selection pads.
    #[must_use]
    pub const fn with_angle_difference_bounds(mut self, bounds: AngleDifferenceBounds) -> Self {
        self.correct_angle_difference_bounds = bounds.is_padded();
        self.angle_difference_bounds = Some(bounds);
        self
    }

    /// The angle difference bounds these options select.
    #[must_use]
    pub const fn selected_angle_difference_bounds(&self) -> AngleDifferenceBounds {
        match self.angle_difference_bounds {
            Some(bounds) => bounds,
            None => AngleDifferenceBounds::from_correct_flag(self.correct_angle_difference_bounds),
        }
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
    /// Branch by bus flow matrix over the positive solver susceptance
    /// magnitudes.
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
/// shunt, phase shift, and HVDC withdrawals, generator costs and bounds with their
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
            synthesized_limit_window: options.synthesized_limit_window,
            angle_difference_bounds: options.selected_angle_difference_bounds(),
            hvdc_treatment: options.hvdc_treatment,
            objective,
        },
    )?;
    apply_instance_semantics(&mut preparation, instance.network(), instance.constraints())?;
    Ok(preparation)
}

impl DcOpfPreparation {
    /// The signed bus by branch incidence matrix `A` of this preparation,
    /// `n_buses × n_branches` CSR: column `e` has `+1` at the branch's from
    /// bus and `-1` at its to bus, in active branch column order. Self-loops
    /// never reach the preparation, so every column has exactly two entries.
    ///
    /// This is the matrix [`DcOpfMatrices::bus_branch_incidence`] carries,
    /// from the one incidence assembly every builder in the crate uses, read
    /// from the arrays the preparation already holds: one pass over the
    /// branch columns, with no network indexing and no
    /// [`DcOperators`](crate::DcOperators) build. Row `i` of the CSR lists
    /// the `(branch, sign)` pairs at bus `i` in ascending branch order.
    ///
    /// # Panics
    /// Panics if a branch column names a bus `>= n_buses`, which no
    /// preparation builder produces.
    #[must_use]
    pub fn calc_bus_branch_incidence(&self) -> SparseMatrix {
        crate::matrix::assemble_bus_branch_incidence(
            self.n_buses,
            self.branches
                .from_bus
                .iter()
                .copied()
                .zip(self.branches.to_bus.iter().copied()),
        )
    }

    /// The generator to bus map `Cg` of this preparation, `n_buses ×
    /// n_generators` CSR with a `1` at row `bus_of_gen[g]` of column `g`. Row
    /// `i` lists the generators at bus `i` in ascending column order. This is
    /// [`DcOpfMatrices::generator_bus`].
    ///
    /// # Panics
    /// Panics if a generator names a bus `>= n_buses`, which no preparation
    /// builder produces.
    #[must_use]
    pub fn calc_generator_bus_incidence(&self) -> SparseMatrix {
        crate::matrix::assemble_bus_indicator(self.n_buses, &self.generators.bus_of_gen)
    }

    /// Every sparse DC OPF matrix of this preparation, as
    /// [`calc_dc_opf_matrices`] returns them for the instance the
    /// preparation came from, without preparing it again.
    #[must_use]
    pub fn calc_matrices(&self) -> DcOpfMatrices {
        matrices_from_preparation(self)
    }

    /// The branch flows at the bus angles `theta` (radians, dense bus order),
    /// in active branch column order and the preparation's power unit:
    /// `f = diag(b) Aᵀ theta + branch_flow_offset`, with `b` the positive
    /// [`DcBranchParameters::susceptance_magnitude`] and the offset from
    /// [`Self::calc_branch_flow_offset`]. Positive flow runs from the from
    /// bus to the to bus.
    ///
    /// # Panics
    /// Panics if `theta.len()` is not `n_buses`.
    #[must_use]
    pub fn calc_branch_flow(&self, theta: &[f64]) -> Vec<f64> {
        assert_eq!(
            theta.len(),
            self.n_buses,
            "calc_branch_flow: theta has {} entries for {} buses",
            theta.len(),
            self.n_buses
        );
        let branches = &self.branches;
        crate::matrix::calc_dc_branch_flow(
            theta,
            branches
                .from_bus
                .iter()
                .copied()
                .zip(branches.to_bus.iter().copied())
                .zip(branches.susceptance_magnitude.iter().copied())
                .zip(branches.shift.iter().copied())
                .map(|((endpoints, weight), shift)| (endpoints, weight, shift)),
        )
    }
}

pub(crate) fn matrices_from_preparation(instance: &DcOpfPreparation) -> DcOpfMatrices {
    let n = instance.n_buses;
    let incidence = instance.calc_bus_branch_incidence();
    let laplacian = calc_weighted_laplacian(&incidence, &instance.branches.susceptance_magnitude);
    let grounded_laplacian = ground_at_each(&laplacian, instance.reference_buses.as_ref());
    let branch_flow_matrix =
        calc_solver_branch_flow_matrix(&incidence, &instance.branches.susceptance_magnitude);

    DcOpfMatrices {
        bus_branch_incidence: incidence,
        laplacian,
        grounded_laplacian,
        branch_flow_matrix,
        generator_bus: instance.calc_generator_bus_incidence(),
        generator_cost: calc_diagonal(&instance.generators.q),
        reference_selector: calc_reference_indicator(n, instance.reference_buses.as_ref()),
    }
}
