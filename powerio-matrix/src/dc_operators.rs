//! The public DC matrix operations over a [`DcPfInstance`].
//!
//! The instance stays matrix free; these separate operations project it. The
//! public values carry PowerModels signs — the branch susceptance is
//! `imag(inv(series impedance))` under the selected formula, negative
//! for an inductive branch — and every axis carries stable element mappings:
//! bus axes use [`BusId`] in analysis row order, and branch axes map each
//! column to its source branch or three winding transformer winding. With
//! voltage angles `va` in radians, the branch flow identity is
//!
//! ```text
//! p_branch = -Bf * va + b .* shift
//! ```
//!
//! and the nodal balance is `p_bus = -B * va + p_shift`, all in per unit on
//! the network MVA base. The reference constrained linear system a solver
//! factors is stated over the internal positive factor weights (the positive
//! semidefinite Laplacian, the negation of the public susceptances), with the
//! sign conversion confined to filling public results.
//!
//! Injections update in place from the instance's specifications; the network
//! dependent matrices are built once and an operating point update never
//! reconstructs them.

use crate::{AnalysisBranchSource, SparseMatrix};
use powerio_core::Error;
use powerio_tx::{BranchSusceptanceFormula, BusId, IndexedNetwork};

use powerio_prob::diagnostics::codes;
use powerio_prob::{DcBusSpecification, DcPfInstance};

/// The DC-relevant identity and parameters of an analysis branch. Floating-
/// point values are compared by bits so signed-zero or non-finite changes do
/// not silently reuse operators built from different data.
type DcBranchSignature = (Option<String>, BusId, BusId, u64, u64, u64, u64, bool);

/// The stable row identity a mapping row reports: the element uid when one
/// exists, else `table:row`.
fn row_identity(uid: Option<&str>, table: &str, row: usize) -> String {
    uid.map_or_else(|| format!("{table}:{row}"), str::to_owned)
}

/// The reference constrained linear system: the positive definite matrix a
/// sparse solver factors, its right hand side, and the mapping from reduced
/// rows back to bus rows.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct ReferenceConstrainedSystem {
    /// The reference grounded positive semidefinite matrix, `n - r` square:
    /// the internal positive factor weights, the negation of the public bus
    /// susceptance matrix with the reference rows and columns removed.
    pub matrix: SparseMatrix,
    /// The right hand side: net injection minus phase shift injection at the
    /// retained buses, plus coupling from eliminated reference buses, per unit.
    pub rhs: Vec<f64>,
    /// Reduced row to dense bus row.
    pub retained_rows: Vec<usize>,
}

/// Build options for [`DcOperators`].
///
/// The default refuses a zero impedance branch, because it has no finite DC
/// operator row. `skip_zero_impedance` drops such a branch from the operator
/// axis instead and records its analysis row in
/// [`DcOperators::skipped_branch_rows`], the same choice
/// `BuildOptions::skip_zero_impedance` offers the admittance builders.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct DcOperatorOptions {
    /// Drop a zero impedance branch instead of refusing the build.
    pub skip_zero_impedance: bool,
}

impl DcOperatorOptions {
    /// The default options: refuse a zero impedance branch.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            skip_zero_impedance: false,
        }
    }

    /// Set whether a zero impedance branch is dropped instead of refused.
    #[must_use]
    pub const fn with_skip_zero_impedance(mut self, skip: bool) -> Self {
        self.skip_zero_impedance = skip;
        self
    }
}

/// DC matrix operations built once from an instance.
#[derive(Clone, Debug)]
pub struct DcOperators {
    bus_ids: Vec<BusId>,
    /// Original source bus IDs and their row order, before synthetic expansion.
    source_bus_ids: Vec<BusId>,
    /// DC-relevant branch data, including derived transformer windings, at build time.
    analysis_branch_signature: Vec<DcBranchSignature>,
    /// Network base and normalization state used to build the operators.
    network_base_mva_bits: u64,
    network_normalized: bool,
    branch_identities: Vec<String>,
    /// Operator column to the analysis branch row it was built from.
    branch_rows: Vec<usize>,
    /// Analysis branch rows dropped under `DcOperatorOptions::skip_zero_impedance`.
    skipped_branch_rows: Vec<usize>,
    options: DcOperatorOptions,
    analysis_sources: Vec<AnalysisBranchSource>,
    /// `n × m`, `+1` at the from bus and `-1` at the to bus of each branch.
    incidence: SparseMatrix,
    /// Public per branch susceptance, PowerModels signs.
    branch_susceptance: Vec<f64>,
    /// Per branch phase shift, radians (zero when the formula carries
    /// no shift injections).
    shift_radians: Vec<f64>,
    /// Per column `(from row, to row)`, stored at build so no injection or
    /// system fill rederives it from the incidence pattern.
    endpoints: Vec<(usize, usize)>,
    /// Net per unit injection at each bus from the instance specifications.
    net_injection: Vec<f64>,
    reference_rows: Vec<usize>,
    /// The stated reference angle, radians, dense over every bus row.
    /// Meaningful only at a row listed in `reference_rows`; every other row
    /// holds zero and is never read.
    reference_va_radians: Vec<f64>,
    branch_susceptance_formula: BranchSusceptanceFormula,
}

impl DcOperators {
    /// Build the operators. Zero impedance branches are preserved by the
    /// instance and have no finite DC row, so they refuse the build until
    /// resolved explicitly with
    /// [`merge_zero_impedance_buses`](powerio_prob::merge_zero_impedance_buses); no
    /// branch is ever skipped silently. Out of service branches and self
    /// loops carry no operator column.
    ///
    /// # Errors
    /// A zero impedance branch, a non-finite branch value, or a branch naming
    /// an undeclared bus.
    pub fn build(instance: &DcPfInstance) -> Result<Self, Error> {
        Self::build_with(instance, &DcOperatorOptions::default())
    }

    /// Build the operators under explicit [`DcOperatorOptions`]. With
    /// `skip_zero_impedance` set, a zero impedance branch is dropped from the
    /// operator axis and listed by [`skipped_branch_rows`](Self::skipped_branch_rows)
    /// instead of refusing the build; every other rule of [`build`](Self::build)
    /// holds.
    ///
    /// # Errors
    /// A zero impedance branch when it is not skipped, a non-finite branch
    /// value, or a branch naming an undeclared bus.
    // One pass over the branch table that fills every axis and operator
    // column; splitting it would scatter the invariants the columns share.
    #[expect(clippy::too_many_lines)]
    pub fn build_with(instance: &DcPfInstance, options: &DcOperatorOptions) -> Result<Self, Error> {
        let source = instance.network();
        let source_bus_ids: Vec<BusId> = source.buses().iter().map(|bus| bus.id).collect();
        let view = IndexedNetwork::new(source);
        let network = view.network();
        let formula = instance.branch_susceptance_formula();
        let base = network.base_mva();
        let analysis_branch_signature: Vec<DcBranchSignature> = network
            .branches()
            .iter()
            .map(|branch| {
                (
                    branch.uid.clone(),
                    branch.from,
                    branch.to,
                    branch.r.to_bits(),
                    branch.x.to_bits(),
                    branch.tap.to_bits(),
                    branch.shift.to_bits(),
                    branch.in_service,
                )
            })
            .collect();
        let network_base_mva_bits = base.to_bits();
        let network_normalized = network.is_normalized();
        let bus_ids: Vec<BusId> = network.buses().iter().map(|bus| bus.id).collect();
        let row_of: std::collections::BTreeMap<BusId, usize> = bus_ids
            .iter()
            .enumerate()
            .map(|(row, &id)| (id, row))
            .collect();
        let position_of = |bus: BusId| row_of.get(&bus).copied();

        let mut branch_identities = Vec::new();
        let mut branch_rows = Vec::new();
        let mut skipped_branch_rows = Vec::new();
        let mut active_analysis_sources = Vec::new();
        let mut branch_susceptance = Vec::new();
        let mut shift_radians = Vec::new();
        let mut endpoints = Vec::new();
        let analysis_sources = crate::opf::analysis_branch_sources(source);
        for (row, branch) in network.branches().iter().enumerate() {
            if !branch.in_service || branch.from == branch.to {
                continue;
            }
            let identity = row_identity(branch.uid.as_deref(), "branches", row);
            let (Some(from), Some(to)) = (position_of(branch.from), position_of(branch.to)) else {
                return Err(Error::new(
                    &codes::BUILD_INSTANCE_SHAPE_MISMATCH,
                    format!(
                        "branch `{identity}` names bus {} or {} the network does not declare",
                        branch.from, branch.to
                    ),
                ));
            };
            // The one DC column rule; only the error wording and the skip
            // policy are this builder's.
            let column = match formula.calc_dc_column(branch, row, network.is_normalized()) {
                Ok(column) => column,
                Err(powerio_tx::Error::DegenerateTap { .. }) => {
                    return Err(Error::new(
                        &codes::BUILD_OPERATOR_NOT_A_NUMBER,
                        format!(
                            "branch `{identity}` states a tap the selected formula cannot divide by"
                        ),
                    ));
                }
                Err(_) => {
                    return Err(Error::new(
                        &codes::BUILD_OPERATOR_ZERO_IMPEDANCE,
                        format!(
                            "branch `{identity}` has no finite DC susceptance under the selected formula; resolve it explicitly with merge_zero_impedance_buses"
                        ),
                    ));
                }
            };
            let Some(column) = column else {
                if options.skip_zero_impedance {
                    skipped_branch_rows.push(row);
                    continue;
                }
                return Err(Error::new(
                    &codes::BUILD_OPERATOR_ZERO_IMPEDANCE,
                    format!(
                        "zero impedance branch `{identity}` has no finite DC operator row; resolve it explicitly with merge_zero_impedance_buses or build with skip_zero_impedance"
                    ),
                ));
            };
            // The public susceptance is the negated factor weight.
            let susceptance = -column.weight;
            let shift = column.shift;
            if !shift.is_finite() {
                return Err(Error::new(
                    &codes::BUILD_OPERATOR_NOT_A_NUMBER,
                    format!("branch `{identity}` states a non-finite phase shift"),
                ));
            }
            branch_identities.push(identity);
            branch_rows.push(row);
            active_analysis_sources.push(analysis_sources[row]);
            branch_susceptance.push(susceptance);
            shift_radians.push(shift);
            endpoints.push((from, to));
        }

        let incidence =
            crate::matrix::assemble_bus_branch_incidence(bus_ids.len(), endpoints.iter().copied());
        let mut operators = Self {
            bus_ids,
            source_bus_ids,
            analysis_branch_signature,
            network_base_mva_bits,
            network_normalized,
            branch_identities,
            branch_rows,
            skipped_branch_rows,
            options: *options,
            analysis_sources: active_analysis_sources,
            incidence,
            branch_susceptance,
            shift_radians,
            endpoints,
            net_injection: Vec::new(),
            reference_rows: Vec::new(),
            reference_va_radians: Vec::new(),
            branch_susceptance_formula: formula,
        };
        operators.refresh_injections(instance, base)?;
        Ok(operators)
    }

    /// Refresh the injection vectors and reference rows from the instance's
    /// current specifications. The network dependent matrices are untouched:
    /// an operating point update goes through here and reconstructs nothing.
    ///
    /// # Errors
    /// The source bus specification axis differs from the built axis, or the
    /// network base, normalization state, DC formula, topology, or branch
    /// parameters changed after the operators were built.
    pub fn update(&mut self, instance: &DcPfInstance) -> Result<(), Error> {
        let view = IndexedNetwork::new(instance.network());
        let network = view.network();
        let branch_signature: Vec<DcBranchSignature> = network
            .branches()
            .iter()
            .map(|branch| {
                (
                    branch.uid.clone(),
                    branch.from,
                    branch.to,
                    branch.r.to_bits(),
                    branch.x.to_bits(),
                    branch.tap.to_bits(),
                    branch.shift.to_bits(),
                    branch.in_service,
                )
            })
            .collect();
        // Expanded electrical branches alone do not identify their source
        // rows: inserting an out-of-service three-winding transformer can
        // shift transformer_row while leaving every expanded branch unchanged.
        // Compare the source mapping for every operator column before touching
        // any cached injections, so a rejected update leaves operators intact.
        let current_sources = crate::opf::analysis_branch_sources(instance.network());
        let source_mappings_match = self
            .branch_rows
            .iter()
            .zip(&self.analysis_sources)
            .all(|(&row, saved)| current_sources.get(row) == Some(saved));
        if network.base_mva().to_bits() != self.network_base_mva_bits
            || network.is_normalized() != self.network_normalized
            || instance.branch_susceptance_formula() != self.branch_susceptance_formula
            || branch_signature != self.analysis_branch_signature
            || !source_mappings_match
        {
            return Err(Error::new(
                &codes::BUILD_INSTANCE_SHAPE_MISMATCH,
                "the network base, normalization, branch parameters, topology, source mappings, or DC formula differ from those used to build the DC operators; rebuild the operators before updating",
            ));
        }
        self.refresh_injections(instance, network.base_mva())
    }

    fn refresh_injections(&mut self, instance: &DcPfInstance, base: f64) -> Result<(), Error> {
        let source_buses = instance.network().buses();
        let source_bus_count = source_buses.len();
        if instance.specifications().len() != source_bus_count
            || source_bus_count > self.bus_ids.len()
        {
            return Err(Error::new(
                &codes::BUILD_INSTANCE_SHAPE_MISMATCH,
                format!(
                    "the instance states {} bus specifications for {} source buses; the operators were built over {} analysis buses",
                    instance.specifications().len(),
                    source_bus_count,
                    self.bus_ids.len()
                ),
            ));
        }
        if source_buses
            .iter()
            .map(|bus| bus.id)
            .ne(self.source_bus_ids.iter().copied())
        {
            return Err(Error::new(
                &codes::BUILD_INSTANCE_SHAPE_MISMATCH,
                "the instance bus IDs or bus order differ from the bus axis used to build the DC operators",
            ));
        }
        let mut net_injection = vec![0.0; self.bus_ids.len()];
        let mut reference_va_radians = vec![0.0; self.bus_ids.len()];
        let mut reference_rows = Vec::new();
        for (row, specification) in instance.specifications().iter().enumerate() {
            match *specification {
                DcBusSpecification::NetActivePower { p_mw } => {
                    net_injection[row] = p_mw / base;
                }
                DcBusSpecification::Reference { va_degrees } => {
                    reference_rows.push(row);
                    reference_va_radians[row] = va_degrees.to_radians();
                }
                _ => {}
            }
        }
        self.net_injection = net_injection;
        self.reference_rows = reference_rows;
        self.reference_va_radians = reference_va_radians;
        Ok(())
    }

    /// The selected branch susceptance formula.
    #[must_use]
    pub const fn branch_susceptance_formula(&self) -> BranchSusceptanceFormula {
        self.branch_susceptance_formula
    }

    /// Dense bus row to bus id, the row mapping of every bus axis.
    #[must_use]
    pub fn bus_ids(&self) -> &[BusId] {
        &self.bus_ids
    }

    /// Operator column to stable analysis branch identity.
    #[must_use]
    pub fn branch_identities(&self) -> &[String] {
        &self.branch_identities
    }

    /// Operator column to the analysis branch row it represents: the
    /// position in the network's branch table (three winding transformer
    /// windings follow the branches). Out of service branches, self loops,
    /// and skipped zero impedance branches have no column, so this is the
    /// row selection every branch axis shares.
    #[must_use]
    pub fn branch_rows(&self) -> &[usize] {
        &self.branch_rows
    }

    /// Analysis branch rows dropped under
    /// [`DcOperatorOptions::skip_zero_impedance`], in table order. Empty
    /// unless the option was set and a zero impedance branch existed.
    #[must_use]
    pub fn skipped_branch_rows(&self) -> &[usize] {
        &self.skipped_branch_rows
    }

    /// The options the operators were built with.
    #[must_use]
    pub const fn options(&self) -> DcOperatorOptions {
        self.options
    }

    /// Operator column to the source branch or three winding transformer
    /// winding represented by that column.
    #[must_use]
    pub fn analysis_sources(&self) -> &[AnalysisBranchSource] {
        &self.analysis_sources
    }

    /// The PowerModels incidence matrix `A`, `m × n`: each branch row has
    /// `+1` at its from bus and `-1` at its to bus.
    #[must_use]
    pub fn calc_incidence_matrix(&self) -> SparseMatrix {
        self.incidence.transpose_view().to_csr()
    }

    /// Calculate the per branch susceptances `b`, with PowerModels signs.
    #[must_use]
    pub fn calc_branch_susceptances(&self) -> &[f64] {
        &self.branch_susceptance
    }

    /// Calculate the branch flow matrix `Bf = Diagonal(b) * A`, `m × n`.
    #[must_use]
    pub fn calc_branch_flow_matrix(&self) -> SparseMatrix {
        crate::matrix::calc_solver_branch_flow_matrix(&self.incidence, &self.branch_susceptance)
    }

    /// Calculate the bus susceptance matrix `B = Aᵀ * Diagonal(b) * A`, `n × n`.
    #[must_use]
    pub fn calc_bus_susceptance_matrix(&self) -> SparseMatrix {
        crate::matrix::calc_weighted_laplacian(&self.incidence, &self.branch_susceptance)
    }

    /// The per bus net power injection the instance states, per unit on the
    /// network MVA base, in bus row order.
    #[must_use]
    pub fn bus_power_injection(&self) -> &[f64] {
        &self.net_injection
    }

    /// Calculate the branch phase shift injection `b .* shift` in branch
    /// order.
    #[must_use]
    pub fn calc_branch_phase_shift_injection(&self) -> Vec<f64> {
        self.branch_susceptance
            .iter()
            .zip(self.shift_radians.iter())
            .map(|(&susceptance, &shift)| crate::matrix::calc_dc_flow_offset(-susceptance, shift))
            .collect()
    }

    /// Calculate the bus phase shift injection
    /// `p_shift = Aᵀ * (b .* shift)` in bus order.
    #[must_use]
    pub fn calc_bus_phase_shift_injection(&self) -> Vec<f64> {
        // The positive factor weight is the negated public susceptance; the
        // scatter is the crate's one phase shift withdrawal over it.
        crate::matrix::calc_dc_phase_shift_withdrawal(
            self.bus_ids.len(),
            self.endpoints
                .iter()
                .copied()
                .zip(self.branch_susceptance.iter())
                .zip(self.shift_radians.iter().copied())
                .map(|((endpoints, &susceptance), shift)| (endpoints, -susceptance, shift)),
        )
    }

    /// Calculate DC active power flow in branch order from bus voltage angles
    /// in radians: `p_branch = -Bf va + b .* shift`.
    ///
    /// # Errors
    /// The voltage angle length differs from the bus axis length.
    pub fn calc_branch_flow_dc(&self, voltage_angles: &[f64]) -> Result<Vec<f64>, Error> {
        if voltage_angles.len() != self.bus_ids.len() {
            return Err(Error::new(
                &codes::BUILD_INSTANCE_SHAPE_MISMATCH,
                format!(
                    "voltage_angles has length {}; expected {} for the bus axis",
                    voltage_angles.len(),
                    self.bus_ids.len()
                ),
            ));
        }
        // The positive factor weight is the negated public susceptance; the
        // flow is the crate's one DC flow over it.
        Ok(crate::matrix::calc_dc_branch_flow(
            voltage_angles,
            self.endpoints
                .iter()
                .copied()
                .zip(self.branch_susceptance.iter())
                .zip(self.shift_radians.iter().copied())
                .map(|((endpoints, &susceptance), shift)| (endpoints, -susceptance, shift)),
        ))
    }

    /// Calculate DC active power injection in bus order from bus voltage
    /// angles in radians: `p_bus = -B va + p_shift`.
    ///
    /// # Errors
    /// The voltage angle length differs from the bus axis length.
    pub fn calc_bus_injection_dc(&self, voltage_angles: &[f64]) -> Result<Vec<f64>, Error> {
        let branch_flows = self.calc_branch_flow_dc(voltage_angles)?;
        Ok(crate::matrix::calc_bus_scatter(
            self.bus_ids.len(),
            self.endpoints.iter().copied().zip(branch_flows),
        ))
    }

    /// Calculate the reference constrained linear system over the internal positive
    /// factor weights: the reference grounded positive semidefinite matrix
    /// `L = -B` with reference rows and columns removed, and the right hand
    /// side at the retained buses is `p - p_shift` (from `p = -B va +
    /// p_shift`) plus the coupling carried in from every eliminated
    /// reference bus's stated angle, radians, so `L_grounded va = rhs`
    /// solves the stated problem with each reference bus fixed at the angle
    /// it states rather than at zero. Sign conversion from the public
    /// susceptances is confined to this fill.
    ///
    /// # Errors
    /// An instance with no reference row.
    pub fn calc_reference_constrained_system(&self) -> Result<ReferenceConstrainedSystem, Error> {
        if self.reference_rows.is_empty() {
            return Err(Error::new(
                &codes::BUILD_INSTANCE_NO_REFERENCE_BUS,
                "the instance states no reference bus to ground the system",
            ));
        }
        // The internal positive factor weights are the negated public
        // susceptances; the grounded matrix is the crate's one weighted
        // Laplacian with the reference rows and columns removed.
        let weights: Vec<f64> = self.branch_susceptance.iter().map(|b| -b).collect();
        let laplacian = crate::matrix::calc_weighted_laplacian(&self.incidence, &weights);
        let shift_injection = self.calc_bus_phase_shift_injection();
        let injection: Vec<f64> = self
            .net_injection
            .iter()
            .zip(&shift_injection)
            .map(|(p, shift)| p - shift)
            .collect();
        let reference_va: Vec<f64> = self
            .reference_rows
            .iter()
            .map(|&row| self.reference_va_radians[row])
            .collect();
        Ok(calc_reference_constrained_system(
            &laplacian,
            &self.reference_rows,
            &reference_va,
            &injection,
        ))
    }
}

/// The reference constrained linear system of a DC power flow over a bus
/// Laplacian `L` with positive branch weights: `L` with the reference rows and
/// columns removed, and at each retained row the right hand side
/// `injection - (L va_ref)`, where `va_ref` holds each reference row's angle,
/// radians, and zero elsewhere. `injection` is the full length nodal power
/// the angles must carry (net injection less every fixed withdrawal), per
/// unit; `reference_va` is aligned with `reference_rows`. Solving
/// `matrix · va = rhs` gives the retained angles with each reference fixed at
/// its stated angle. [`DcOperators::calc_reference_constrained_system`] is this
/// over the instance's operators.
///
/// # Panics
/// `injection` shorter than the Laplacian's row count, or a reference row
/// outside it.
#[must_use]
pub fn calc_reference_constrained_system(
    laplacian: &SparseMatrix,
    reference_rows: &[usize],
    reference_va: &[f64],
    injection: &[f64],
) -> ReferenceConstrainedSystem {
    let n = laplacian.rows();
    let grounding = crate::matrix::Grounding::new(reference_rows);
    let retained_rows = grounding.full_of_reduced(n);
    // A branch to an eliminated reference bus still carries an off-diagonal
    // Laplacian entry; since its column is gone, its contribution moves to the
    // retained row's right hand side, carrying the reference bus's stated
    // angle in. With every reference at zero there is nothing to carry.
    let mut reference_angles = vec![0.0; n];
    for (&row, &va) in reference_rows.iter().zip(reference_va) {
        reference_angles[row] = va;
    }
    let reference_load = if reference_angles.iter().any(|&va| va != 0.0) {
        crate::matrix::calc_csr_product(laplacian, &reference_angles)
    } else {
        vec![0.0; n]
    };
    let rhs = retained_rows
        .iter()
        .map(|&row| injection[row] - reference_load[row])
        .collect();
    ReferenceConstrainedSystem {
        matrix: crate::matrix::ground_with(laplacian, &grounding),
        rhs,
        retained_rows,
    }
}
