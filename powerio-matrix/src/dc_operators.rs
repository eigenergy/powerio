//! The public DC matrix operations over a [`DcPfInstance`].
//!
//! The instance stays matrix free; these separate operations project it. The
//! public values carry PowerModels signs — the branch susceptance is
//! `imag(inv(series impedance))` under the selected approximation, negative
//! for an inductive branch — and every axis carries stable element mappings:
//! bus rows by [`BusId`] in bus table order, branch columns by payload
//! identity in branch table order. With voltage angles `va` in radians, the
//! branch flow identity is
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

use crate::SparseMatrix;
use powerio_core::Error;
use powerio_tx::{BusId, DcConvention};

use powerio_prob::diagnostics::codes;
use powerio_prob::{DcBusSpecification, DcPfInstance};

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
    /// The right hand side: net injection plus phase shift injection at the
    /// retained buses, per unit.
    pub rhs: Vec<f64>,
    /// Reduced row to dense bus row.
    pub retained_rows: Vec<usize>,
}

/// DC matrix operations built once from an instance.
#[derive(Clone, Debug)]
pub struct DcOperators {
    bus_ids: Vec<BusId>,
    branch_identities: Vec<String>,
    /// `n × m`, `+1` at the from bus and `-1` at the to bus of each branch.
    incidence: SparseMatrix,
    /// Public per branch susceptance, PowerModels signs.
    branch_susceptance: Vec<f64>,
    /// Per branch phase shift, radians (zero when the approximation carries
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
    approximation: DcConvention,
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
        let network = instance.network();
        let approximation = instance.approximation();
        let base = network.base_mva();
        let bus_ids: Vec<BusId> = network.buses().iter().map(|bus| bus.id).collect();
        let row_of: std::collections::BTreeMap<BusId, usize> = bus_ids
            .iter()
            .enumerate()
            .map(|(row, &id)| (id, row))
            .collect();
        let position_of = |bus: BusId| row_of.get(&bus).copied();

        let mut branch_identities = Vec::new();
        let mut branch_susceptance = Vec::new();
        let mut shift_radians = Vec::new();
        let mut endpoints = Vec::new();
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
            // Only the tap-reading formula can be bounded by a tap (#324).
            let tap = if approximation.reads_tap() {
                branch.divisible_tap(row).map_err(|_| {
                    Error::new(
                        &codes::BUILD_OPERATOR_NOT_A_NUMBER,
                        format!(
                            "branch `{identity}` states a tap the selected approximation cannot divide by"
                        ),
                    )
                })?
            } else {
                1.0
            };
            // The same divisibility floor the other DC builders apply: a
            // formally nonzero impedance below it yields a finite weight big
            // enough to annihilate every real branch sharing a bus.
            let degenerate = match approximation {
                DcConvention::SeriesSusceptance => {
                    branch.r.hypot(branch.x) < powerio_tx::dc::MIN_DIVISIBLE_MAGNITUDE
                }
                // Any formula that reads a reactance is bounded by it.
                _ => branch.x.abs() < powerio_tx::dc::MIN_DIVISIBLE_MAGNITUDE,
            };
            if degenerate {
                return Err(Error::new(
                    &codes::BUILD_OPERATOR_ZERO_IMPEDANCE,
                    format!(
                        "zero impedance branch `{identity}` has no finite DC operator row; resolve it explicitly with merge_zero_impedance_buses"
                    ),
                ));
            }
            let susceptance = approximation.branch_susceptance(branch.r, branch.x, tap);
            if !susceptance.is_finite() {
                return Err(Error::new(
                    &codes::BUILD_OPERATOR_ZERO_IMPEDANCE,
                    format!(
                        "branch `{identity}` has no finite DC susceptance under the selected approximation; resolve it explicitly with merge_zero_impedance_buses"
                    ),
                ));
            }
            let shift = if approximation.includes_phase_shifts() {
                branch.shift.to_radians()
            } else {
                0.0
            };
            if !shift.is_finite() {
                return Err(Error::new(
                    &codes::BUILD_OPERATOR_NOT_A_NUMBER,
                    format!("branch `{identity}` states a non-finite phase shift"),
                ));
            }
            branch_identities.push(identity);
            branch_susceptance.push(susceptance);
            shift_radians.push(shift);
            endpoints.push((from, to));
        }

        let mut incidence =
            crate::matrix::triplet::CooBuilder::new_rect(bus_ids.len(), endpoints.len());
        for (column, &(from, to)) in endpoints.iter().enumerate() {
            incidence.add(from, column, 1.0);
            incidence.add(to, column, -1.0);
        }

        let mut operators = Self {
            bus_ids,
            branch_identities,
            incidence: incidence.finish_csr(),
            branch_susceptance,
            shift_radians,
            endpoints,
            net_injection: Vec::new(),
            reference_rows: Vec::new(),
            reference_va_radians: Vec::new(),
            approximation,
        };
        operators.refresh_injections(instance, base)?;
        Ok(operators)
    }

    /// Refresh the injection vectors and reference rows from the instance's
    /// current specifications. The network dependent matrices are untouched:
    /// an operating point update goes through here and reconstructs nothing.
    ///
    /// # Errors
    /// A specification list whose length disagrees with the built bus axis.
    pub fn update(&mut self, instance: &DcPfInstance) -> Result<(), Error> {
        let base = instance.network().base_mva();
        self.refresh_injections(instance, base)
    }

    fn refresh_injections(&mut self, instance: &DcPfInstance, base: f64) -> Result<(), Error> {
        if instance.specifications().len() != self.bus_ids.len() {
            return Err(Error::new(
                &codes::BUILD_INSTANCE_SHAPE_MISMATCH,
                format!(
                    "the instance states {} bus specifications; the operators were built over {} buses",
                    instance.specifications().len(),
                    self.bus_ids.len()
                ),
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

    /// The selected DC branch approximation.
    #[must_use]
    pub const fn approximation(&self) -> DcConvention {
        self.approximation
    }

    /// Dense bus row to bus id, the row mapping of every bus axis.
    #[must_use]
    pub fn bus_ids(&self) -> &[BusId] {
        &self.bus_ids
    }

    /// Operator column to stable branch identity, the column mapping of every
    /// branch axis.
    #[must_use]
    pub fn branch_identities(&self) -> &[String] {
        &self.branch_identities
    }

    /// The incidence matrix `A`, `n × m`: `+1` at each branch's from bus and
    /// `-1` at its to bus.
    #[must_use]
    pub const fn incidence(&self) -> &SparseMatrix {
        &self.incidence
    }

    /// The public per branch susceptances `b`, PowerModels signs.
    #[must_use]
    pub fn branch_susceptances(&self) -> &[f64] {
        &self.branch_susceptance
    }

    /// The branch susceptance matrix `Bf = diag(b) Aᵀ`, `m × n`, PowerModels
    /// signs: `p_branch = -Bf va + b .* shift`.
    #[must_use]
    pub fn branch_susceptance_matrix(&self) -> SparseMatrix {
        let transpose = self.incidence.transpose_view().to_csr();
        scale_rows(&transpose, &self.branch_susceptance)
    }

    /// The bus susceptance matrix `B = A diag(b) Aᵀ`, `n × n`, PowerModels
    /// signs: `p_bus = -B va + p_shift`.
    #[must_use]
    pub fn bus_susceptance_matrix(&self) -> SparseMatrix {
        let bf = self.branch_susceptance_matrix();
        &self.incidence * &bf
    }

    /// The per bus net power injection the instance states, per unit on the
    /// network MVA base, in bus row order.
    #[must_use]
    pub fn bus_power_injection(&self) -> &[f64] {
        &self.net_injection
    }

    /// The phase shift injection `p_shift = A (b .* shift)`, per unit, in bus
    /// row order: the fixed nodal term of `p_bus = -B va + p_shift`.
    #[must_use]
    pub fn phase_shift_injection(&self) -> Vec<f64> {
        let mut injection = vec![0.0; self.bus_ids.len()];
        for (column, (&susceptance, &shift)) in self
            .branch_susceptance
            .iter()
            .zip(self.shift_radians.iter())
            .enumerate()
        {
            if shift == 0.0 {
                continue;
            }
            let value = susceptance * shift;
            // Column `column` of A is `e_from - e_to`.
            let (from, to) = self.endpoints(column);
            injection[from] += value;
            injection[to] -= value;
        }
        injection
    }

    /// The reference constrained linear system over the internal positive
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
    pub fn reference_constrained_system(&self) -> Result<ReferenceConstrainedSystem, Error> {
        if self.reference_rows.is_empty() {
            return Err(Error::new(
                &codes::BUILD_INSTANCE_NO_REFERENCE_BUS,
                "the instance states no reference bus to ground the system",
            ));
        }
        let n = self.bus_ids.len();
        let mut is_reference = vec![false; n];
        for &row in &self.reference_rows {
            is_reference[row] = true;
        }
        let mut reduced_of_full = vec![usize::MAX; n];
        let mut retained_rows = Vec::with_capacity(n - self.reference_rows.len());
        for (row, reduced) in reduced_of_full.iter_mut().enumerate() {
            if !is_reference[row] {
                *reduced = retained_rows.len();
                retained_rows.push(row);
            }
        }

        let mut matrix = crate::matrix::triplet::CooBuilder::new(retained_rows.len());
        // A branch to an eliminated reference bus still carries an
        // off-diagonal Laplacian entry; since its column is gone, that
        // entry's contribution moves to the retained row's right hand side
        // instead, carrying the reference bus's stated angle in.
        let mut reference_coupling = vec![0.0; retained_rows.len()];
        for (column, &(from, to)) in self.endpoint_table().iter().enumerate() {
            // The positive factor weight is the negated public susceptance.
            let weight = -self.branch_susceptance[column];
            let (rf, rt) = (reduced_of_full[from], reduced_of_full[to]);
            if rf != usize::MAX {
                matrix.add(rf, rf, weight);
            }
            if rt != usize::MAX {
                matrix.add(rt, rt, weight);
            }
            match (rf != usize::MAX, rt != usize::MAX) {
                (true, true) => {
                    matrix.add(rf, rt, -weight);
                    matrix.add(rt, rf, -weight);
                }
                (true, false) => reference_coupling[rf] += weight * self.reference_va_radians[to],
                (false, true) => {
                    reference_coupling[rt] += weight * self.reference_va_radians[from];
                }
                (false, false) => {}
            }
        }
        let shift_injection = self.phase_shift_injection();
        let rhs = retained_rows
            .iter()
            .zip(reference_coupling.iter())
            .map(|(&row, &coupling)| self.net_injection[row] - shift_injection[row] + coupling)
            .collect();
        Ok(ReferenceConstrainedSystem {
            matrix: matrix.finish_csr(),
            rhs,
            retained_rows,
        })
    }

    fn endpoints(&self, column: usize) -> (usize, usize) {
        self.endpoints[column]
    }

    /// Endpoint rows per column, recovered from the incidence structure.
    fn endpoint_table(&self) -> &[(usize, usize)] {
        &self.endpoints
    }
}

/// `diag(values) * matrix`, scaling each row of a CSR matrix.
fn scale_rows(matrix: &SparseMatrix, values: &[f64]) -> SparseMatrix {
    let mut scaled = matrix.clone();
    for (row, mut row_vec) in scaled.outer_iterator_mut().enumerate() {
        for (_, entry) in row_vec.iter_mut() {
            *entry *= values[row];
        }
    }
    scaled
}
