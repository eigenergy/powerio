//! DC sensitivity matrices.
//!
//! PTDF maps nodal injections to branch flows (`f = PTDF · p`); LODF maps a
//! branch outage to the flow it redistributes onto the others. Both come from
//! the reference grounded DC bus susceptance matrix
//! `ABA = ground_with(L, refs)`: one row/column removed per reference bus.
//! Every builder routes through the same solver selection: a dense Cholesky
//! (with dense Gaussian elimination as the nonsingular indefinite fallback)
//! below the `Auto` ceilings, and a sparse factorization factored once and
//! reused across every right hand side above them. The sparse factorization is
//! a Cholesky while the matrix is positive definite; a negative branch
//! susceptance (a series capacitor) can make it indefinite, and LDLᵀ and then
//! LU with partial pivoting take over, each accepted only when a probe solve
//! shows a normwise backward error near machine precision and a forward error
//! that leaves the matrix nonsingular to working precision. Disconnected
//! networks with one reference per island are supported. The selected row and
//! column builders ([`calc_ptdf_rows`], [`calc_ptdf_columns`],
//! [`calc_lodf_columns`]) solve only the right hand sides they return.
//! Several references in one island are fixed angle buses; this is not a
//! participation factor based distributed slack model.

// Dense linear algebra: indexed triangular-solve loops and the `.iter()`
// sparse traversal read clearer than the iterator rewrites clippy suggests.
#![allow(clippy::needless_range_loop, clippy::explicit_iter_loop)]

use sprs::CsMat;

use crate::indexed::IndexedNetwork;
use crate::matrix::laplacian::{Grounding, calc_weighted_laplacian, ground_with};
use crate::matrix::triplet::CooBuilder;
use crate::matrix::{
    BranchSusceptanceFormula, BuildOptions, IncidenceParts, build_incidence,
    calc_solver_branch_flow_matrix,
};
use crate::{Error, Result};

/// Entries below this magnitude are dropped from the emitted sparse matrices.
const PRUNE: f64 = 1e-12;
/// Right hand sides per sparse block solve. Bounds the block buffer to
/// `nr × 32` doubles while amortizing each triangular sweep across columns.
const SPARSE_SOLVE_BLOCK: usize = 32;
/// Reduced-dimension ceiling for the `Auto` dense path. Against the retired
/// conjugate gradient solver the dense crossover sat in the thousands; against
/// a sparse direct factorization, whose fill stays near linear on network
/// graphs, the dense path wins only where its constant factors do, on small
/// cases.
const DEFAULT_AUTO_DENSE_THRESHOLD: usize = 512;

/// Memory ceiling for the `Auto` dense path. The dimension alone does not
/// bound the cost: the dense path also materializes an m x n PTDF and an
/// m x m LODF, so a case with few buses and many parallel branches could ask
/// for tens of GB while passing any nr test.
const AUTO_DENSE_MEMORY_BUDGET: usize = 2 << 30;

/// Peak bytes the dense path holds: the reduced Laplacian and its packed
/// factorization (one and a half nr x nr buffers alive together, rounded up),
/// plus the dense PTDF and the LODF built from it.
fn dense_footprint_bytes(reduced_dimension: usize, branches: usize, buses: usize) -> usize {
    let f = size_of::<f64>();
    let sq = |a: usize, b: usize| a.saturating_mul(b).saturating_mul(f);
    sq(reduced_dimension, reduced_dimension)
        .saturating_mul(2)
        .saturating_add(sq(branches, buses))
        .saturating_add(sq(branches, branches))
}
const LODF_ISLAND_TOLERANCE: f64 = 1e-9;

/// Solver selection for option based DC sensitivity builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SensitivitySolver {
    /// Dense below [`SensitivityOptions::auto_dense_threshold`], sparse above it.
    #[default]
    Auto,
    /// Dense grounded factorization. Handles nonsingular indefinite cases.
    Dense,
    /// Sparse factorization, factored once and reused across every right hand
    /// side: Cholesky, then `LDLᵀ`, then LU for an indefinite matrix.
    Sparse,
}

/// Solver path actually used for a sensitivity build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SensitivitySolverPath {
    DenseCholesky,
    DenseInverse,
    SparseCholesky,
    /// Sparse `LDLᵀ` without pivoting, for an indefinite matrix.
    SparseLdlt,
    /// Sparse LU with partial pivoting, when `LDLᵀ` fails or loses accuracy.
    SparseLu,
}

impl SensitivitySolverPath {
    #[inline]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DenseCholesky => "dense_cholesky",
            Self::DenseInverse => "dense_inverse",
            Self::SparseCholesky => "sparse_cholesky",
            Self::SparseLdlt => "sparse_ldlt",
            Self::SparseLu => "sparse_lu",
        }
    }
}

/// Options for PTDF/LODF builders that expose solver choice and output pruning.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct SensitivityOptions {
    /// Formula used to calculate each DC branch susceptance.
    pub formula: BranchSusceptanceFormula,
    /// Solver selection policy.
    pub solver: SensitivitySolver,
    /// Entries with absolute value at or below this value are omitted from the
    /// returned sparse matrices. LODF diagonal entries are structural and kept.
    pub drop_tolerance: f64,
    /// Reduced dimension above which [`SensitivitySolver::Auto`] selects the
    /// sparse path.
    pub auto_dense_threshold: usize,
}

impl Default for SensitivityOptions {
    fn default() -> Self {
        Self {
            formula: BranchSusceptanceFormula::default(),
            solver: SensitivitySolver::Auto,
            drop_tolerance: PRUNE,
            auto_dense_threshold: DEFAULT_AUTO_DENSE_THRESHOLD,
        }
    }
}

impl SensitivityOptions {
    fn validate(&self) -> Result<()> {
        if !self.drop_tolerance.is_finite() || self.drop_tolerance < 0.0 {
            return Err(Error::InvalidSensitivityOptions {
                reason: format!(
                    "drop_tolerance must be finite and nonnegative, got {}",
                    self.drop_tolerance
                ),
            });
        }
        Ok(())
    }

    /// Return the concrete solver selected for a reduced grounded dimension,
    /// assuming a square problem. Prefer
    /// [`Self::select_solver_for_shape`], which also sees the branch count
    /// the dense PTDF and LODF are sized by.
    pub fn select_solver_for_reduced_dimension(
        &self,
        reduced_dimension: usize,
    ) -> SensitivitySolver {
        self.select_solver_for_shape(reduced_dimension, reduced_dimension, reduced_dimension)
    }

    /// Return the concrete solver selected for a problem shape. `Auto` takes
    /// the dense path while both the reduced dimension and the predicted
    /// dense footprint stay within their ceilings, so a wide case (few buses,
    /// many branches) no longer picks a path that would ask for tens of GB.
    pub fn select_solver_for_shape(
        &self,
        reduced_dimension: usize,
        branches: usize,
        buses: usize,
    ) -> SensitivitySolver {
        match self.solver {
            SensitivitySolver::Auto => {
                let fits = reduced_dimension <= self.auto_dense_threshold
                    && dense_footprint_bytes(reduced_dimension, branches, buses)
                        <= AUTO_DENSE_MEMORY_BUDGET;
                if fits {
                    SensitivitySolver::Dense
                } else {
                    SensitivitySolver::Sparse
                }
            }
            other => other,
        }
    }
}

/// PTDF/LODF matrices plus metadata for serialized outputs.
#[derive(Debug, Clone)]
pub struct SensitivityMatrices {
    pub ptdf: CsMat<f64>,
    pub lodf: CsMat<f64>,
    pub metadata: SensitivityMetadata,
}

/// Metadata describing a sensitivity build.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SensitivityMetadata {
    pub requested_solver: SensitivitySolver,
    pub solver_path: SensitivitySolverPath,
    pub drop_tolerance: f64,
    pub auto_dense_threshold: usize,
    pub reduced_dimension: usize,
    pub ptdf: SensitivityMatrixMetadata,
    pub lodf: SensitivityMatrixMetadata,
}

/// Shape and pruning metadata for one sensitivity matrix.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SensitivityMatrixMetadata {
    pub rows: usize,
    pub cols: usize,
    pub nnz: usize,
    pub dropped_entries: usize,
}

/// PTDF (`m × n`): branch flows from nodal injections, `f = PTDF · p`. Every
/// reference bus column is zero. The DC bus susceptance matrix is grounded at
/// the whole reference set (`reference_bus_indices`), one row/column per slack.
/// One reference per island handles disconnected networks; several references
/// within one island fixes all of those bus angles to zero.
pub fn calc_ptdf(case: &IndexedNetwork, formula: BranchSusceptanceFormula) -> Result<CsMat<f64>> {
    let options = SensitivityOptions {
        formula,
        ..SensitivityOptions::default()
    };
    Ok(build_parts(case, &options, Want::Ptdf)?.into_ptdf().0)
}

/// LODF (`m × m`): pre-outage flow on branch `k` redistributes onto branch `l`
/// with factor `LODF[l, k]`. Diagonal is `−1`. A branch whose outage islands
/// the network (denominator `≈ 0`) gets a zero column.
pub fn calc_lodf(case: &IndexedNetwork, formula: BranchSusceptanceFormula) -> Result<CsMat<f64>> {
    let options = SensitivityOptions {
        formula,
        ..SensitivityOptions::default()
    };
    Ok(build_parts(case, &options, Want::Lodf)?.into_lodf().0)
}

/// Both DC sensitivity matrices `(PTDF, LODF)` from one DC bus susceptance
/// matrix factorization. When a caller needs both for the same case (the
/// `sensitivities` bundle), this factors the grounded DC bus susceptance
/// matrix once instead of paying the factorization twice across separate
/// [`calc_ptdf`]/[`calc_lodf`] calls.
pub fn calc_ptdf_lodf(
    case: &IndexedNetwork,
    formula: BranchSusceptanceFormula,
) -> Result<(CsMat<f64>, CsMat<f64>)> {
    let options = SensitivityOptions {
        formula,
        ..SensitivityOptions::default()
    };
    let ((ptdf, _), (lodf, _)) = build_parts(case, &options, Want::Both)?.into_both();
    Ok((ptdf, lodf))
}

/// PTDF and LODF with solver selection, drop tolerance, and output metadata.
pub fn calc_ptdf_lodf_with_options(
    case: &IndexedNetwork,
    options: &SensitivityOptions,
) -> Result<SensitivityMatrices> {
    let parts = build_parts(case, options, Want::Both)?;
    let solver_path = parts.solver_path;
    let reduced_dimension = parts.reduced_dimension;
    let ((ptdf, ptdf_dropped), (lodf, lodf_dropped)) = parts.into_both();

    let metadata = sensitivity_metadata(
        options,
        solver_path,
        reduced_dimension,
        matrix_metadata(&ptdf, ptdf_dropped),
        matrix_metadata(&lodf, lodf_dropped),
    );

    Ok(SensitivityMatrices {
        ptdf,
        lodf,
        metadata,
    })
}

/// The PTDF rows of `branches` (dense branch indices), a `branches.len() × n`
/// matrix whose row `i` is the full PTDF's row `branches[i]`.
///
/// Each row is one solve, `x = ABA⁻¹ (e_from − e_to)` through the symmetric
/// grounded matrix, scaled by the branch susceptance, so neither the `m × n`
/// PTDF nor an inverse is formed. The solver follows
/// [`SensitivityOptions::solver`] on the reduced dimension alone, and entries
/// at or below [`SensitivityOptions::drop_tolerance`] are omitted.
///
/// # Errors
/// An index outside the branch table, the conditions of the full builders,
/// and [`Error::SingularNetwork`] when no factorization is trusted.
pub fn calc_ptdf_rows(
    case: &IndexedNetwork,
    options: &SensitivityOptions,
    branches: &[usize],
) -> Result<CsMat<f64>> {
    let selected = SelectedSolve::new(case, options)?;
    let (inc, g) = (&selected.inc, &selected.g);
    let (n, m) = (inc.n(), inc.m());
    check_selection("branch", branches, m)?;
    let (from, to) = endpoints(&inc.a, m);
    let full_of = g.full_of_reduced(n);
    let mut builder = CooBuilder::new_rect(branches.len(), n);
    for (chunk_index, chunk) in branches.chunks(SPARSE_SOLVE_BLOCK).enumerate() {
        let mut block = faer::Mat::<f64>::zeros(full_of.len(), chunk.len());
        for (col, &branch) in chunk.iter().enumerate() {
            if let Some(rf) = g.reduced(from[branch]) {
                block[(rf, col)] += 1.0;
            }
            if let Some(rt) = g.reduced(to[branch]) {
                block[(rt, col)] -= 1.0;
            }
        }
        selected.solver.solve_block(block.as_mut());
        for (col, &branch) in chunk.iter().enumerate() {
            let row = chunk_index * SPARSE_SOLVE_BLOCK + col;
            for (reduced, &bus) in full_of.iter().enumerate() {
                let v = inc.b[branch] * block[(reduced, col)];
                if v.abs() > options.drop_tolerance {
                    builder.add(row, bus, v);
                }
            }
        }
    }
    Ok(builder.finish_csr())
}

/// The PTDF columns of `buses` (dense bus indices), an `m × buses.len()`
/// matrix whose column `j` is the full PTDF's column `buses[j]`. A reference
/// bus's column is zero.
///
/// Each column is one solve, so neither the `m × n` PTDF nor an inverse is
/// formed. Solver selection and dropping follow [`calc_ptdf_rows`].
///
/// # Errors
/// As [`calc_ptdf_rows`], for an index outside the bus table.
pub fn calc_ptdf_columns(
    case: &IndexedNetwork,
    options: &SensitivityOptions,
    buses: &[usize],
) -> Result<CsMat<f64>> {
    let selected = SelectedSolve::new(case, options)?;
    let inc = &selected.inc;
    check_selection("bus", buses, inc.n())?;
    let mut builder = CooBuilder::new_rect(inc.m(), buses.len());
    ptdf_column_entries(
        inc,
        &selected.g,
        &selected.solver,
        options,
        buses,
        |row, col, value| {
            builder.add(row, col, value);
            Ok(())
        },
    )?;
    Ok(builder.finish_csr())
}

/// The LODF columns of the `outaged` branches (dense branch indices), an
/// `m × outaged.len()` matrix whose column `j` is the full LODF's column
/// `outaged[j]`: the flow each branch picks up per unit of the outaged
/// branch's pre-outage flow, with `−1` at the outaged branch itself and a
/// zero column for a branch whose outage islands the network.
///
/// Each column is one solve, so neither the `m × m` LODF nor the PTDF is
/// formed. Solver selection and dropping follow [`calc_ptdf_rows`].
///
/// # Errors
/// As [`calc_ptdf_rows`].
pub fn calc_lodf_columns(
    case: &IndexedNetwork,
    options: &SensitivityOptions,
    outaged: &[usize],
) -> Result<CsMat<f64>> {
    let selected = SelectedSolve::new(case, options)?;
    let inc = &selected.inc;
    check_selection("branch", outaged, inc.m())?;
    let mut builder = CooBuilder::new_rect(inc.m(), outaged.len());
    lodf_column_entries(
        inc,
        &selected.g,
        &selected.solver,
        options,
        outaged,
        |row, col, value| {
            builder.add(row, col, value);
            Ok(())
        },
    )?;
    Ok(builder.finish_csr())
}

/// The factored grounded matrix the selected row and column builders solve
/// against.
struct SelectedSolve {
    inc: IncidenceParts,
    g: Grounding,
    solver: GroundedSolver,
}

impl SelectedSolve {
    fn new(case: &IndexedNetwork, options: &SensitivityOptions) -> Result<Self> {
        options.validate()?;
        case.check_reference_coverage()?;
        let refs = case.reference_bus_indices();
        let inc = build_incidence(case, options.formula, &BuildOptions::default())?;
        let g = Grounding::new(&refs);
        let lr = ground_with(&calc_weighted_laplacian(&inc.a, &inc.b), &g);
        let solver = GroundedSolver::factor(&inc, &lr, options)?;
        Ok(Self { inc, g, solver })
    }
}

fn check_selection(what: &str, indices: &[usize], len: usize) -> Result<()> {
    match indices.iter().find(|&&index| index >= len) {
        Some(index) => Err(Error::InvalidSensitivityOptions {
            reason: format!("{what} index {index} is outside the {len} {what}es of the network"),
        }),
        None => Ok(()),
    }
}

/// Which matrices a [`build_parts`] call materializes. The dense path always
/// forms the dense PTDF (the LODF is built from it); the sparse path runs only
/// the requested halves.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Want {
    Ptdf,
    Lodf,
    Both,
}

struct BuiltParts {
    /// Matrix plus its dropped-entry count, present when requested.
    ptdf: Option<(CsMat<f64>, usize)>,
    lodf: Option<(CsMat<f64>, usize)>,
    solver_path: SensitivitySolverPath,
    reduced_dimension: usize,
}

impl BuiltParts {
    fn into_ptdf(self) -> (CsMat<f64>, usize) {
        self.ptdf.expect("the requested PTDF was built")
    }

    fn into_lodf(self) -> (CsMat<f64>, usize) {
        self.lodf.expect("the requested LODF was built")
    }

    fn into_both(self) -> ((CsMat<f64>, usize), (CsMat<f64>, usize)) {
        (
            self.ptdf.expect("Want::Both builds the PTDF"),
            self.lodf.expect("Want::Both builds the LODF"),
        )
    }
}

/// Shared body of every in-memory builder: validate, select a solver for the
/// problem shape, and build the requested matrices on that path.
fn build_parts(
    case: &IndexedNetwork,
    options: &SensitivityOptions,
    want: Want,
) -> Result<BuiltParts> {
    options.validate()?;
    case.check_reference_coverage()?;
    let refs = case.reference_bus_indices();
    let inc = build_incidence(case, options.formula, &BuildOptions::default())?;
    let g = Grounding::new(&refs);
    let reduced_dimension = inc.n().saturating_sub(g.len());

    match options.select_solver_for_shape(reduced_dimension, inc.m(), inc.n()) {
        SensitivitySolver::Dense => {
            let (dense, m, n, solver_path) = ptdf_dense_with_path(&inc, &refs)?;
            let ptdf = (want != Want::Lodf)
                .then(|| dense_to_csr_with_drop(&dense, m, n, options.drop_tolerance));
            let lodf = (want != Want::Ptdf)
                .then(|| lodf_from_dense_with_drop(&dense, &inc.a, m, n, options.drop_tolerance));
            Ok(BuiltParts {
                ptdf,
                lodf,
                solver_path,
                reduced_dimension,
            })
        }
        SensitivitySolver::Sparse => {
            ensure_finite_susceptances(&inc)?;
            let lr = ground_with(&calc_weighted_laplacian(&inc.a, &inc.b), &g);
            let factor = SparseFactor::factor(&lr)?;
            let solver = GroundedSolver::Sparse(Box::new(factor));
            let all_buses: Vec<usize> = (0..inc.n()).collect();
            let all_branches: Vec<usize> = (0..inc.m()).collect();
            let ptdf = if want == Want::Lodf {
                None
            } else {
                let mut builder = CooBuilder::new_rect(inc.m(), inc.n());
                let meta = ptdf_column_entries(
                    &inc,
                    &g,
                    &solver,
                    options,
                    &all_buses,
                    |row, col, value| {
                        builder.add(row, col, value);
                        Ok(())
                    },
                )?;
                Some((builder.finish_csr(), meta.dropped_entries))
            };
            let lodf = if want == Want::Ptdf {
                None
            } else {
                let mut builder = CooBuilder::new(inc.m());
                let meta = lodf_column_entries(
                    &inc,
                    &g,
                    &solver,
                    options,
                    &all_branches,
                    |row, col, value| {
                        builder.add(row, col, value);
                        Ok(())
                    },
                )?;
                Some((builder.finish_csr(), meta.dropped_entries))
            };
            Ok(BuiltParts {
                ptdf,
                lodf,
                solver_path: solver.path(),
                reduced_dimension,
            })
        }
        SensitivitySolver::Auto => unreachable!("select_solver_for_shape resolves Auto"),
    }
}

pub(crate) fn for_each_ptdf_lodf_entry(
    case: &IndexedNetwork,
    options: &SensitivityOptions,
    mut ptdf_entry: impl FnMut(usize, usize, f64) -> Result<()>,
    mut lodf_entry: impl FnMut(usize, usize, f64) -> Result<()>,
) -> Result<SensitivityMetadata> {
    options.validate()?;
    case.check_reference_coverage()?;
    let refs = case.reference_bus_indices();
    let inc = build_incidence(case, options.formula, &BuildOptions::default())?;
    let reduced_dimension = inc.n().saturating_sub(Grounding::new(&refs).len());

    let (solver_path, ptdf, lodf) =
        match options.select_solver_for_shape(reduced_dimension, inc.m(), inc.n()) {
            SensitivitySolver::Dense => {
                let (dense, m, n, solver_path) = ptdf_dense_with_path(&inc, &refs)?;
                let (ptdf, ptdf_dropped) =
                    dense_to_csr_with_drop(&dense, m, n, options.drop_tolerance);
                let (lodf, lodf_dropped) =
                    lodf_from_dense_with_drop(&dense, &inc.a, m, n, options.drop_tolerance);
                let ptdf_meta = matrix_metadata(&ptdf, ptdf_dropped);
                let lodf_meta = matrix_metadata(&lodf, lodf_dropped);
                for (&v, (row, col)) in &ptdf {
                    ptdf_entry(row, col, v)?;
                }
                for (&v, (row, col)) in &lodf {
                    lodf_entry(row, col, v)?;
                }
                (solver_path, ptdf_meta, lodf_meta)
            }
            SensitivitySolver::Sparse => {
                ensure_finite_susceptances(&inc)?;
                let g = Grounding::new(&refs);
                let lr = ground_with(&calc_weighted_laplacian(&inc.a, &inc.b), &g);
                let solver = GroundedSolver::Sparse(Box::new(SparseFactor::factor(&lr)?));
                let all_buses: Vec<usize> = (0..inc.n()).collect();
                let all_branches: Vec<usize> = (0..inc.m()).collect();
                let ptdf = ptdf_column_entries(&inc, &g, &solver, options, &all_buses, ptdf_entry)?;
                let lodf =
                    lodf_column_entries(&inc, &g, &solver, options, &all_branches, lodf_entry)?;
                (solver.path(), ptdf, lodf)
            }
            SensitivitySolver::Auto => {
                unreachable!("select_solver_for_shape resolves Auto")
            }
        };

    Ok(sensitivity_metadata(
        options,
        solver_path,
        reduced_dimension,
        ptdf,
        lodf,
    ))
}

fn sensitivity_metadata(
    options: &SensitivityOptions,
    solver_path: SensitivitySolverPath,
    reduced_dimension: usize,
    ptdf: SensitivityMatrixMetadata,
    lodf: SensitivityMatrixMetadata,
) -> SensitivityMetadata {
    SensitivityMetadata {
        requested_solver: options.solver,
        solver_path,
        drop_tolerance: options.drop_tolerance,
        auto_dense_threshold: options.auto_dense_threshold,
        reduced_dimension,
        ptdf,
        lodf,
    }
}

fn matrix_metadata(matrix: &CsMat<f64>, dropped_entries: usize) -> SensitivityMatrixMetadata {
    SensitivityMatrixMetadata {
        rows: matrix.rows(),
        cols: matrix.cols(),
        nnz: matrix.nnz(),
        dropped_entries,
    }
}

fn lodf_from_dense_with_drop(
    ptdf: &[f64],
    a: &CsMat<f64>,
    m: usize,
    n: usize,
    drop_tolerance: f64,
) -> (CsMat<f64>, usize) {
    // Branch endpoints (dense bus indices), recovered from the incidence.
    let (from, to) = endpoints(a, m);

    // Outaging a bridge redistributes nothing, so its column is structurally
    // zero. The magnitude test this replaces let a near bridge at
    // `delta(k,k) = 1 - 1.1e-9` through, amplifying its column to ~1e9 with
    // about seven digits gone.
    let is_bridge = bridges(&from, &to, n);

    // Denominator `1 − (PTDF[k, from_k] − PTDF[k, to_k])` and the islanding
    // decision, once per outage column instead of once per entry.
    let mut denoms = vec![0.0; m];
    let mut islands = vec![false; m];
    for k in 0..m {
        denoms[k] = 1.0 - (ptdf[k * n + from[k]] - ptdf[k * n + to[k]]);
        islands[k] = is_bridge[k] || denoms[k].abs() < LODF_ISLAND_TOLERANCE;
    }

    // Row l of the LODF reads only PTDF row l, so walking l in the outer loop
    // visits both matrices in row order and emits finished CSR rows directly.
    // The old k-outer form re-read a full strided PTDF column per outage and
    // scattered into a triplet map.
    let mut indptr = Vec::with_capacity(m + 1);
    indptr.push(0usize);
    let mut indices = Vec::new();
    let mut data = Vec::new();
    let mut dropped = 0usize;
    for l in 0..m {
        let row = &ptdf[l * n..l * n + n];
        for k in 0..m {
            let v = if l == k {
                -1.0
            } else if islands[k] {
                0.0
            } else {
                (row[from[k]] - row[to[k]]) / denoms[k]
            };
            if l == k || v.abs() > drop_tolerance {
                indices.push(k);
                data.push(v);
            } else if v != 0.0 {
                dropped += 1;
            }
        }
        indptr.push(indices.len());
    }
    (CsMat::new((m, m), indptr, indices, data), dropped)
}

fn ptdf_dense_with_path(
    inc: &IncidenceParts,
    refs: &[usize],
) -> Result<(Vec<f64>, usize, usize, SensitivitySolverPath)> {
    let n = inc.n();
    let m = inc.m();
    let g = Grounding::new(refs);
    let nr = n - g.len();

    // Reduced grounded DC bus susceptance matrix: ABA_refs.
    let lr = ground_with(&calc_weighted_laplacian(&inc.a, &inc.b), &g);
    let dense_lr = densify(&lr, nr);

    // Minv (n × n) is the reduced inverse padded with a zero row/col at every
    // grounded bus, so each reference's PTDF column comes out zero.
    // PTDF = (B Aᵀ) · Minv: each nonzero of the branch flow matrix scatters a scaled
    // Minv row into a PTDF row. Grouping the flow nonzeros by reduced column
    // up front names exactly which Minv rows the scatter reads, so the
    // factored path below produces each of those rows once by a back-solve
    // instead of materializing the whole inverse.
    let flow = calc_solver_branch_flow_matrix(&inc.a, &inc.b); // m × n
    let mut rows_used: Vec<Vec<(usize, f64)>> = vec![Vec::new(); nr];
    for (&w, (l, c)) in flow.iter() {
        // Minv row at a slack is 0.
        if let Some(rc) = g.reduced(c) {
            rows_used[rc].push((l, w));
        }
    }
    // Reduced → full column map, built once. The scatter walks each Minv row
    // contiguously and skips grounded columns instead of testing every one of
    // them; reduced order is ascending full order, so the accumulation order
    // is unchanged.
    let full_of = g.full_of_reduced(n);
    let mut ptdf = vec![0.0; m * n];

    if let Some(chol) = DenseCholesky::factor(&dense_lr, nr) {
        // The reduced matrix is symmetric, so solving `ABA_refs · x = e_rc`
        // yields Minv row rc directly. Peak memory is the packed triangle
        // plus one row buffer; the retired form held the dense matrix, a full
        // triangular factor, and the explicit inverse at once.
        drop(dense_lr);
        let mut row = vec![0.0; nr];
        for (rc, uses) in rows_used.iter().enumerate() {
            if uses.is_empty() {
                continue;
            }
            row.fill(0.0);
            row[rc] = 1.0;
            chol.solve(&mut row);
            scatter_minv_row(&row, uses, &full_of, n, &mut ptdf);
        }
        return Ok((ptdf, m, n, SensitivitySolverPath::DenseCholesky));
    }

    // Nonsingular indefinite fallback: explicit inverse by Gaussian
    // elimination with partial pivoting.
    let rinv = dense_inverse(dense_lr, nr).ok_or(Error::SingularNetwork)?;
    for (rc, uses) in rows_used.iter().enumerate() {
        scatter_minv_row(&rinv[rc * nr..rc * nr + nr], uses, &full_of, n, &mut ptdf);
    }
    Ok((ptdf, m, n, SensitivitySolverPath::DenseInverse))
}

/// Scatter one Minv row into every PTDF row that reads it: `uses` holds the
/// `(branch row, weight)` pairs grouped by [`ptdf_dense_with_path`].
fn scatter_minv_row(
    row: &[f64],
    uses: &[(usize, f64)],
    full_of: &[usize],
    n: usize,
    ptdf: &mut [f64],
) {
    for &(l, w) in uses {
        let out = &mut ptdf[l * n..l * n + n];
        for (rk, &k) in full_of.iter().enumerate() {
            out[k] += w * row[rk];
        }
    }
}

/// Normwise backward error a factorization must reach on its probe solve to
/// be trusted. A backward stable solve lands near machine precision (about
/// 1e-16 on transmission cases of any size); a factorization that lost
/// accuracy, such as `LDLᵀ` without pivoting through a small pivot, lands far
/// above it.
const BACKWARD_ERROR_TRUST: f64 = 1e-10;

/// Relative forward error a factorization's probe solve may reach before the
/// matrix counts as singular to working precision. A backward stable solve
/// errs forward by about the condition number times machine precision, so a
/// grounded matrix stays usable up to a condition number near 1e12; a matrix
/// that is singular but for rounding, where any factorization still reports
/// a small backward error, lands near or above 1.
const FORWARD_ERROR_TRUST: f64 = 1e-4;

/// One way to factor the grounded matrix, `None` when it breaks down.
type FactorAttempt = fn(&faer::sparse::SparseColMat<usize, f64>) -> Option<SparseFactor>;

/// A sparse factorization of the grounded DC bus susceptance matrix, factored
/// once and reused across every right hand side.
///
/// Positive branch susceptances make the matrix positive definite, and the
/// Cholesky factorization serves it. A negative susceptance, a series
/// capacitor's, can make it indefinite: `LDLᵀ` without pivoting takes over,
/// and LU with partial pivoting when that fails or loses accuracy. A
/// factorization is accepted only when it solves a probe right hand side
/// with a normwise backward error below [`BACKWARD_ERROR_TRUST`], a finite
/// result, and a forward error below [`FORWARD_ERROR_TRUST`]. The matrix is
/// refused as singular only when every one fails.
enum SparseFactor {
    Llt(faer::sparse::linalg::solvers::Llt<usize, f64>),
    Ldlt {
        symbolic: faer::sparse::linalg::cholesky::SymbolicCholesky<usize>,
        values: Vec<f64>,
    },
    Lu(faer::sparse::linalg::solvers::Lu<usize, f64>),
}

impl SparseFactor {
    fn factor(lr: &CsMat<f64>) -> Result<Self> {
        let nr = lr.rows();
        if lr.cols() != nr {
            return Err(Error::ShapeMismatch {
                what: "grounded DC bus susceptance matrix columns",
                expected: nr,
                got: lr.cols(),
            });
        }
        // A nonfinite entry is NaN or infinity poisoning, not a property of
        // the network; refuse it before any factorization runs.
        if lr.data().iter().any(|v| !v.is_finite()) {
            return Err(Error::SingularNetwork);
        }
        let mut triplets = Vec::with_capacity(lr.nnz());
        for (i, row) in lr.outer_iterator().enumerate() {
            for (j, &v) in row.iter() {
                triplets.push(faer::sparse::Triplet::new(i, j, v));
            }
        }
        let mat = faer::sparse::SparseColMat::try_new_from_triplets(nr, nr, &triplets)
            .map_err(|_| Error::SingularNetwork)?;
        let probe = Probe::new(lr);
        let candidates: [FactorAttempt; 3] = [
            |mat| {
                mat.as_ref()
                    .sp_cholesky(faer::Side::Lower)
                    .ok()
                    .map(Self::Llt)
            },
            Self::factor_ldlt,
            |mat| mat.as_ref().sp_lu().ok().map(Self::Lu),
        ];
        for candidate in candidates {
            if let Some(factor) = candidate(&mat)
                && probe.trusts(&factor)
            {
                return Ok(factor);
            }
        }
        Err(Error::SingularNetwork)
    }

    fn factor_ldlt(mat: &faer::sparse::SparseColMat<usize, f64>) -> Option<Self> {
        use faer::dyn_stack::{MemBuffer, MemStack};
        use faer::linalg::cholesky::ldlt::factor::LdltRegularization;
        use faer::sparse::linalg::cholesky::{
            CholeskySymbolicParams, SymmetricOrdering, factorize_symbolic_cholesky,
        };
        let symbolic = factorize_symbolic_cholesky(
            mat.symbolic(),
            faer::Side::Lower,
            SymmetricOrdering::default(),
            CholeskySymbolicParams::default(),
        )
        .ok()?;
        let mut values = vec![0.0; symbolic.len_val()];
        let par = faer::get_global_parallelism();
        let mut buffer = MemBuffer::try_new(
            symbolic.factorize_numeric_ldlt_scratch::<f64>(par, faer::Spec::default()),
        )
        .ok()?;
        symbolic
            .factorize_numeric_ldlt(
                &mut values,
                mat.as_ref(),
                faer::Side::Lower,
                LdltRegularization::default(),
                par,
                MemStack::new(&mut buffer),
                faer::Spec::default(),
            )
            .ok()?;
        Some(Self::Ldlt { symbolic, values })
    }

    fn path(&self) -> SensitivitySolverPath {
        match self {
            Self::Llt(_) => SensitivitySolverPath::SparseCholesky,
            Self::Ldlt { .. } => SensitivitySolverPath::SparseLdlt,
            Self::Lu(_) => SensitivitySolverPath::SparseLu,
        }
    }

    /// Solve in place, one right hand side per column.
    fn solve_block(&self, mut rhs: faer::MatMut<'_, f64>) {
        use faer::linalg::solvers::Solve;
        match self {
            Self::Llt(llt) => llt.solve_in_place(rhs),
            Self::Lu(lu) => lu.solve_in_place(rhs),
            Self::Ldlt { symbolic, values } => {
                use faer::dyn_stack::{MemBuffer, MemStack};
                let par = faer::get_global_parallelism();
                let mut buffer =
                    MemBuffer::new(symbolic.solve_in_place_scratch::<f64>(rhs.ncols(), par));
                faer::sparse::linalg::cholesky::LdltRef::new(symbolic, values)
                    .solve_in_place_with_conj(
                        faer::Conj::No,
                        rhs.as_mut(),
                        par,
                        MemStack::new(&mut buffer),
                    );
            }
        }
    }
}

/// The probe a sparse factorization must solve before it is trusted: a fixed
/// right hand side `b = A x` from a deterministic `x` with entries spread in
/// `[-1, 1]`.
struct Probe<'a> {
    a: &'a CsMat<f64>,
    x: Vec<f64>,
    b: Vec<f64>,
    /// `‖A‖∞`, the largest absolute row sum.
    a_norm: f64,
}

impl<'a> Probe<'a> {
    #[allow(clippy::cast_precision_loss)]
    fn new(a: &'a CsMat<f64>) -> Self {
        // Fractional parts of multiples of the golden ratio: deterministic,
        // nonzero, and never aligned with a structured null vector.
        let x: Vec<f64> = (0..a.rows())
            .map(|i| ((i as f64 + 1.0) * 0.618_033_988_749_895).fract() * 2.0 - 1.0)
            .collect();
        let b = mat_vec(a, &x);
        let a_norm = a
            .outer_iterator()
            .map(|row| row.data().iter().map(|v| v.abs()).sum::<f64>())
            .fold(0.0, f64::max);
        Self { a, x, b, a_norm }
    }

    /// Whether `factor` solves the probe with a finite result, a normwise
    /// backward error `‖b − A x̂‖∞ / (‖A‖∞ ‖x̂‖∞ + ‖b‖∞)` within
    /// [`BACKWARD_ERROR_TRUST`], and a relative forward error
    /// `‖x̂ − x‖∞ / ‖x‖∞` within [`FORWARD_ERROR_TRUST`].
    fn trusts(&self, factor: &SparseFactor) -> bool {
        let n = self.b.len();
        let mut rhs = faer::Mat::<f64>::from_fn(n, 1, |i, _| self.b[i]);
        factor.solve_block(rhs.as_mut());
        let x: Vec<f64> = (0..n).map(|i| rhs[(i, 0)]).collect();
        if x.iter().any(|v| !v.is_finite()) {
            return false;
        }
        let ax = mat_vec(self.a, &x);
        let residual = self
            .b
            .iter()
            .zip(&ax)
            .map(|(b, ax)| (b - ax).abs())
            .fold(0.0, f64::max);
        let x_norm = x.iter().map(|v| v.abs()).fold(0.0, f64::max);
        let b_norm = self.b.iter().map(|v| v.abs()).fold(0.0, f64::max);
        let scale = self.a_norm * x_norm + b_norm;
        let forward = x
            .iter()
            .zip(&self.x)
            .map(|(x, exact)| (x - exact).abs())
            .fold(0.0, f64::max);
        let exact_norm = self.x.iter().map(|v| v.abs()).fold(0.0, f64::max);
        scale > 0.0
            && residual <= BACKWARD_ERROR_TRUST * scale
            && forward <= FORWARD_ERROR_TRUST * exact_norm
    }
}

fn mat_vec(a: &CsMat<f64>, x: &[f64]) -> Vec<f64> {
    a.outer_iterator()
        .map(|row| row.iter().map(|(j, &v)| v * x[j]).sum())
        .collect()
}

/// A factorization of the grounded DC bus susceptance matrix that solves
/// blocks of right hand sides, for the builders that solve column by column.
enum GroundedSolver {
    DenseCholesky(DenseCholesky),
    /// The explicit inverse, row major, of a nonsingular indefinite matrix.
    DenseInverse {
        inverse: Vec<f64>,
        n: usize,
    },
    Sparse(Box<SparseFactor>),
}

impl GroundedSolver {
    /// Factor `lr` on the path `options` selects for a problem that solves
    /// right hand sides without forming the full matrices.
    fn factor(inc: &IncidenceParts, lr: &CsMat<f64>, options: &SensitivityOptions) -> Result<Self> {
        let nr = lr.rows();
        match options.select_solver_for_reduced_dimension(nr) {
            SensitivitySolver::Dense => {
                let dense = densify(lr, nr);
                if let Some(chol) = DenseCholesky::factor(&dense, nr) {
                    return Ok(Self::DenseCholesky(chol));
                }
                let inverse = dense_inverse(dense, nr).ok_or(Error::SingularNetwork)?;
                Ok(Self::DenseInverse { inverse, n: nr })
            }
            SensitivitySolver::Sparse => {
                ensure_finite_susceptances(inc)?;
                Ok(Self::Sparse(Box::new(SparseFactor::factor(lr)?)))
            }
            SensitivitySolver::Auto => unreachable!("select_solver_for_shape resolves Auto"),
        }
    }

    fn path(&self) -> SensitivitySolverPath {
        match self {
            Self::DenseCholesky(_) => SensitivitySolverPath::DenseCholesky,
            Self::DenseInverse { .. } => SensitivitySolverPath::DenseInverse,
            Self::Sparse(factor) => factor.path(),
        }
    }

    /// Solve in place, one right hand side per column.
    fn solve_block(&self, mut rhs: faer::MatMut<'_, f64>) {
        match self {
            Self::Sparse(factor) => factor.solve_block(rhs),
            Self::DenseCholesky(chol) => {
                let mut column = vec![0.0; rhs.nrows()];
                for j in 0..rhs.ncols() {
                    for (i, slot) in column.iter_mut().enumerate() {
                        *slot = rhs[(i, j)];
                    }
                    chol.solve(&mut column);
                    for (i, value) in column.iter().enumerate() {
                        rhs[(i, j)] = *value;
                    }
                }
            }
            Self::DenseInverse { inverse, n } => {
                let mut column = vec![0.0; *n];
                for j in 0..rhs.ncols() {
                    for (i, slot) in column.iter_mut().enumerate() {
                        *slot = rhs[(i, j)];
                    }
                    for i in 0..*n {
                        rhs[(i, j)] = inverse[i * n..i * n + n]
                            .iter()
                            .zip(&column)
                            .map(|(a, b)| a * b)
                            .sum();
                    }
                }
            }
        }
    }
}

/// PTDF columns for `buses`, column `position` holding bus `buses[position]`.
/// A grounded bus's column is zero and solves nothing.
fn ptdf_column_entries(
    inc: &IncidenceParts,
    g: &Grounding,
    solver: &GroundedSolver,
    options: &SensitivityOptions,
    buses: &[usize],
    mut ptdf_entry: impl FnMut(usize, usize, f64) -> Result<()>,
) -> Result<SensitivityMatrixMetadata> {
    let n = inc.n();
    let m = inc.m();
    let nr = n - g.len();
    let (from, to) = endpoints(&inc.a, m);

    let mut nnz = 0usize;
    let mut dropped = 0usize;
    let solved: Vec<(usize, usize)> = buses
        .iter()
        .enumerate()
        .filter_map(|(position, &bus)| g.reduced(bus).map(|reduced| (position, reduced)))
        .collect();
    let mut theta = vec![0.0; nr];
    for chunk in solved.chunks(SPARSE_SOLVE_BLOCK) {
        let mut block = faer::Mat::<f64>::zeros(nr, chunk.len());
        for (col, &(_, reduced)) in chunk.iter().enumerate() {
            block[(reduced, col)] = 1.0;
        }
        solver.solve_block(block.as_mut());
        for (col, &(position, _)) in chunk.iter().enumerate() {
            for (r, slot) in theta.iter_mut().enumerate() {
                *slot = block[(r, col)];
            }
            for branch in 0..m {
                let v = branch_flow(branch, &from, &to, &inc.b, g, &theta);
                if v.abs() > options.drop_tolerance {
                    ptdf_entry(branch, position, v)?;
                    nnz += 1;
                } else if v != 0.0 {
                    dropped += 1;
                }
            }
        }
    }

    Ok(SensitivityMatrixMetadata {
        rows: m,
        cols: buses.len(),
        nnz,
        dropped_entries: dropped,
    })
}

/// LODF columns for `outages`, column `position` holding the outage of branch
/// `outages[position]`.
fn lodf_column_entries(
    inc: &IncidenceParts,
    g: &Grounding,
    solver: &GroundedSolver,
    options: &SensitivityOptions,
    outages: &[usize],
    mut lodf_entry: impl FnMut(usize, usize, f64) -> Result<()>,
) -> Result<SensitivityMatrixMetadata> {
    let n = inc.n();
    let m = inc.m();
    let nr = n - g.len();
    let (from, to) = endpoints(&inc.a, m);

    // Same rule as the dense path: a bridge redistributes nothing, decided on
    // the topology rather than on how close the denominator came to zero.
    let is_bridge = bridges(&from, &to, n);

    let mut nnz = 0usize;
    let mut dropped = 0usize;
    let mut theta = vec![0.0; nr];
    for (chunk_index, chunk) in outages.chunks(SPARSE_SOLVE_BLOCK).enumerate() {
        let start = chunk_index * SPARSE_SOLVE_BLOCK;
        // A bridge's column is its diagonal alone, so only the other outages
        // of this block get a right hand side and a solve. Every branch of a
        // radial feeder is a bridge, which is every solve skipped here.
        let solved: Vec<usize> = chunk.iter().copied().filter(|&k| !is_bridge[k]).collect();
        let mut block = faer::Mat::<f64>::zeros(nr, solved.len());
        for (col, &outage) in solved.iter().enumerate() {
            if let Some(rf) = g.reduced(from[outage]) {
                block[(rf, col)] += 1.0;
            }
            if let Some(rt) = g.reduced(to[outage]) {
                block[(rt, col)] -= 1.0;
            }
        }
        if !solved.is_empty() {
            solver.solve_block(block.as_mut());
        }
        let mut next = 0usize;
        for (offset, &outage) in chunk.iter().enumerate() {
            let position = start + offset;
            // Neither the solve that would have produced the rest of a
            // bridge's column nor the scan that would emit it runs: every
            // other entry is an exact zero, which is neither above the drop
            // tolerance nor counted as dropped.
            if is_bridge[outage] {
                lodf_entry(outage, position, -1.0)?;
                nnz += 1;
                continue;
            }
            for (r, slot) in theta.iter_mut().enumerate() {
                *slot = block[(r, next)];
            }
            next += 1;
            let denom = 1.0 - branch_flow(outage, &from, &to, &inc.b, g, &theta);
            let islands = denom.abs() < LODF_ISLAND_TOLERANCE;
            for branch in 0..m {
                let v = if branch == outage {
                    -1.0
                } else if islands {
                    0.0
                } else {
                    branch_flow(branch, &from, &to, &inc.b, g, &theta) / denom
                };
                if branch == outage || v.abs() > options.drop_tolerance {
                    lodf_entry(branch, position, v)?;
                    nnz += 1;
                } else if v != 0.0 {
                    dropped += 1;
                }
            }
        }
    }

    Ok(SensitivityMatrixMetadata {
        rows: m,
        cols: outages.len(),
        nnz,
        dropped_entries: dropped,
    })
}

/// Refuse a nonfinite branch susceptance before a sparse factorization: it
/// poisons every entry it touches. A zero or negative susceptance is a valid
/// network (a series capacitor has a negative one), which the factorization
/// fallback handles.
fn ensure_finite_susceptances(inc: &IncidenceParts) -> Result<()> {
    for (branch, &b) in inc.b.iter().enumerate() {
        if !b.is_finite() {
            return Err(Error::InvalidSensitivityOptions {
                reason: format!(
                    "the sparse sensitivity solver requires finite branch susceptances; \\
                     branch {branch} has {b}"
                ),
            });
        }
    }
    Ok(())
}

fn branch_flow(
    branch: usize,
    from: &[usize],
    to: &[usize],
    b: &[f64],
    g: &Grounding,
    theta: &[f64],
) -> f64 {
    let theta_from = g.reduced(from[branch]).map_or(0.0, |i| theta[i]);
    let theta_to = g.reduced(to[branch]).map_or(0.0, |i| theta[i]);
    b[branch] * (theta_from - theta_to)
}

/// Branch endpoints from the signed incidence: `+1` row is from, `−1` is to.
/// Which branches are bridges of the graph the columns describe: an edge whose
/// removal disconnects its endpoints.
///
/// Outaging a bridge moves no flow anywhere, which is the condition the LODF
/// denominator `1 - delta(k,k)` approaches. Deciding it topologically is exact,
/// where a magnitude test on the denominator cannot separate a true bridge from
/// a branch that merely carries almost everything.
///
/// Iterative Tarjan, O(n + m); the textbook recursion overflows the stack on a
/// real feeder. Entry is tracked by arc rather than by parent node, so parallel
/// branches leave neither of them a bridge.
fn bridges(from: &[usize], to: &[usize], n: usize) -> Vec<bool> {
    let m = from.len();
    // Forward star: arc `2k` runs from[k] -> to[k], arc `2k+1` its reverse, so
    // `arc ^ 1` is the other direction of the same branch and `arc / 2` is the
    // branch itself.
    let mut head = vec![usize::MAX; n];
    let mut next = vec![usize::MAX; 2 * m];
    let mut dest = vec![0usize; 2 * m];
    for k in 0..m {
        for (arc, tail, other) in [(2 * k, from[k], to[k]), (2 * k + 1, to[k], from[k])] {
            dest[arc] = other;
            next[arc] = head[tail];
            head[tail] = arc;
        }
    }

    let mut disc = vec![usize::MAX; n];
    let mut low = vec![0usize; n];
    let mut is_bridge = vec![false; m];
    let mut timer = 0usize;
    // (node, the arc it was entered by, the next arc to examine)
    let mut stack: Vec<(usize, usize, usize)> = Vec::new();

    for root in 0..n {
        if disc[root] != usize::MAX {
            continue;
        }
        disc[root] = timer;
        low[root] = timer;
        timer += 1;
        stack.push((root, usize::MAX, head[root]));
        while let Some(top) = stack.last_mut() {
            let (v, in_arc) = (top.0, top.1);
            if top.2 == usize::MAX {
                stack.pop();
                if let Some(parent) = stack.last_mut() {
                    let p = parent.0;
                    low[p] = low[p].min(low[v]);
                    // The subtree under v reaches nothing at or above p, so
                    // the edge into v is the only way back.
                    if low[v] > disc[p] {
                        is_bridge[in_arc / 2] = true;
                    }
                }
                continue;
            }
            let arc = top.2;
            top.2 = next[arc];
            // Skip the branch we arrived on, but not a parallel one beside it.
            if arc == in_arc ^ 1 {
                continue;
            }
            let w = dest[arc];
            if disc[w] == usize::MAX {
                disc[w] = timer;
                low[w] = timer;
                timer += 1;
                stack.push((w, arc, head[w]));
            } else {
                low[v] = low[v].min(disc[w]);
            }
        }
    }
    is_bridge
}

fn endpoints(a: &CsMat<f64>, m: usize) -> (Vec<usize>, Vec<usize>) {
    let mut from = vec![0usize; m];
    let mut to = vec![0usize; m];
    for (&v, (bus, branch)) in a.iter() {
        if v > 0.0 {
            from[branch] = bus;
        } else {
            to[branch] = bus;
        }
    }
    (from, to)
}

fn densify(a: &CsMat<f64>, n: usize) -> Vec<f64> {
    let mut d = vec![0.0; n * n];
    for (&v, (i, j)) in a.iter() {
        d[i * n + j] = v;
    }
    d
}

fn dense_to_csr_with_drop(
    dense: &[f64],
    rows: usize,
    cols: usize,
    drop_tolerance: f64,
) -> (CsMat<f64>, usize) {
    // The scan is row major and every coordinate is unique, so the CSR
    // arrays fill directly. Routing it through a hash map bought a dedup
    // that cannot fire, then copied the entries into a triplet matrix and
    // sorted them: on a 10k-bus PTDF that was several GB of intermediates
    // and an O(nnz log nnz) sort to emit an already-ordered matrix. One
    // counting pass sizes the buffers exactly instead.
    let mut dropped = 0usize;
    let mut nnz = 0usize;
    for &v in dense {
        if v.abs() > drop_tolerance {
            nnz += 1;
        } else if v != 0.0 {
            dropped += 1;
        }
    }
    let mut indptr = Vec::with_capacity(rows + 1);
    let mut indices = Vec::with_capacity(nnz);
    let mut data = Vec::with_capacity(nnz);
    indptr.push(0usize);
    for i in 0..rows {
        for j in 0..cols {
            let v = dense[i * cols + j];
            if v.abs() > drop_tolerance {
                indices.push(j);
                data.push(v);
            }
        }
        indptr.push(data.len());
    }
    (CsMat::new((rows, cols), indptr, indices, data), dropped)
}

/// The smallest pivot a dense factorization of `a` accepts.
///
/// It tracks the matrix's own scale. A fixed 1e-12 is at once too strict for a
/// legitimately small scaled matrix and far too loose for one whose entries run
/// to 1e12. Accepting 1e-300 instead lets a square root divide a column by
/// 1e-150 twice and return entries near 1e300 with no error, which is the shape
/// a near disconnected island joined by one very high impedance branch takes,
/// and which `check_reference_coverage` passes.
#[allow(clippy::cast_precision_loss)]
fn dense_inverse(mut a: Vec<f64>, n: usize) -> Option<Vec<f64>> {
    // Each elimination pivot is judged against its own column's original
    // magnitude, so the floor measures cancellation within that column. A
    // single floor scaled from the largest entry anywhere in the matrix
    // refused wide but valid magnitude spreads as singular (#324).
    let mut floors = vec![0.0; n];
    for (c, floor) in floors.iter_mut().enumerate() {
        let scale = (0..n).fold(0.0_f64, |mx, r| mx.max(a[r * n + c].abs()));
        *floor = n as f64 * f64::EPSILON * scale;
    }
    let mut inv = vec![0.0; n * n];
    for i in 0..n {
        inv[i * n + i] = 1.0;
    }

    for col in 0..n {
        let mut pivot_row = col;
        let mut pivot_abs = a[col * n + col].abs();
        for r in (col + 1)..n {
            let v = a[r * n + col].abs();
            if v > pivot_abs {
                pivot_abs = v;
                pivot_row = r;
            }
        }
        if !pivot_abs.is_finite() || pivot_abs <= floors[col] {
            return None;
        }
        if pivot_row != col {
            swap_dense_rows(&mut a, n, pivot_row, col);
            swap_dense_rows(&mut inv, n, pivot_row, col);
        }

        let pivot = a[col * n + col];
        for c in 0..n {
            a[col * n + c] /= pivot;
            inv[col * n + c] /= pivot;
        }
        for r in 0..n {
            if r == col {
                continue;
            }
            let factor = a[r * n + col];
            if factor == 0.0 {
                continue;
            }
            for c in 0..n {
                a[r * n + c] -= factor * a[col * n + c];
                inv[r * n + c] -= factor * inv[col * n + c];
            }
        }
    }
    Some(inv)
}

fn swap_dense_rows(a: &mut [f64], n: usize, r1: usize, r2: usize) {
    for c in 0..n {
        a.swap(r1 * n + c, r2 * n + c);
    }
}

/// Dense lower-triangular Cholesky `A = L Lᵀ` for a small SPD matrix, stored
/// packed: row `i` of the lower triangle starts at `i·(i+1)/2`.
struct DenseCholesky {
    n: usize,
    l: Vec<f64>,
}

impl DenseCholesky {
    fn factor(a: &[f64], n: usize) -> Option<Self> {
        let mut l = vec![0.0; n * (n + 1) / 2];
        for i in 0..n {
            // Each pivot is judged against its own original diagonal entry:
            // the floor measures how much elimination has cancelled it, so a
            // wide but valid spread of magnitudes factors while `s > 0.0`
            // alone would still accept a pivot ground down to noise (#324).
            let floor = n as f64 * f64::EPSILON * a[i * n + i].abs();
            let row = i * (i + 1) / 2;
            for j in 0..=i {
                let mut s = a[i * n + j];
                let jrow = j * (j + 1) / 2;
                for k in 0..j {
                    s -= l[row + k] * l[jrow + k];
                }
                if i == j {
                    // `!(s > floor)` rejects negative, too small, AND NaN
                    // pivots: `NaN <= x` is false, so `s <= floor` would let a
                    // NaN-poisoned matrix factor "successfully" into all-NaN.
                    // The negated comparison is the point (NaN incomparability),
                    // so the partial_cmp rewrite clippy suggests would obscure it.
                    #[allow(clippy::neg_cmp_op_on_partial_ord)]
                    if !(s > floor) {
                        return None;
                    }
                    l[row + i] = s.sqrt();
                } else {
                    l[row + j] = s / l[jrow + j];
                }
            }
        }
        Some(Self { n, l })
    }

    /// Solve `A x = b` in place.
    fn solve(&self, b: &mut [f64]) {
        let n = self.n;
        for i in 0..n {
            let row = i * (i + 1) / 2;
            let mut s = b[i];
            for k in 0..i {
                s -= self.l[row + k] * b[k];
            }
            b[i] = s / self.l[row + i];
        }
        for i in (0..n).rev() {
            let mut s = b[i];
            for k in (i + 1)..n {
                s -= self.l[k * (k + 1) / 2 + i] * b[k];
            }
            b[i] = s / self.l[i * (i + 1) / 2 + i];
        }
    }
}

#[cfg(test)]
mod bridge_tests {
    use super::bridges;

    #[test]
    fn every_edge_of_a_path_is_a_bridge() {
        // 0 - 1 - 2 - 3
        let b = bridges(&[0, 1, 2], &[1, 2, 3], 4);
        assert_eq!(b, vec![true, true, true]);
    }

    #[test]
    fn no_edge_of_a_cycle_is_a_bridge() {
        // 0 - 1 - 2 - 0
        let b = bridges(&[0, 1, 2], &[1, 2, 0], 3);
        assert_eq!(b, vec![false, false, false]);
    }

    #[test]
    fn parallel_branches_leave_neither_a_bridge() {
        // Two circuits on the same corridor: outaging one still leaves a path,
        // so neither is a bridge. Tracking entry by node rather than by arc
        // would call both of them bridges.
        let b = bridges(&[0, 0], &[1, 1], 2);
        assert_eq!(b, vec![false, false]);
    }

    #[test]
    fn only_the_tie_between_two_loops_is_a_bridge() {
        // Two triangles joined by one tie line: 0-1-2-0, tie 2-3, 3-4-5-3.
        let from = [0, 1, 2, 2, 3, 4, 5];
        let to = [1, 2, 0, 3, 4, 5, 3];
        let b = bridges(&from, &to, 6);
        assert_eq!(b, vec![false, false, false, true, false, false, false]);
    }

    #[test]
    fn a_self_loop_is_not_a_bridge() {
        let b = bridges(&[0, 1], &[1, 1], 2);
        assert_eq!(b, vec![true, false]);
    }

    #[test]
    fn separate_components_are_each_walked() {
        // 0-1 and 2-3, no tie. Both edges are bridges of their own component,
        // and the root loop must reach the second one.
        let b = bridges(&[0, 2], &[1, 3], 4);
        assert_eq!(b, vec![true, true]);
    }

    #[test]
    fn a_long_path_does_not_overflow_the_stack() {
        // The recursion a textbook writes dies here.
        let n = 200_000;
        let from: Vec<usize> = (0..n - 1).collect();
        let to: Vec<usize> = (1..n).collect();
        let b = bridges(&from, &to, n);
        assert_eq!(b.len(), n - 1);
        assert!(b.iter().all(|&x| x));
    }
}

#[cfg(test)]
mod pivot_tests {
    use super::{DenseCholesky, dense_inverse};

    /// #324. A pivot is judged against its own column (inverse) or its own
    /// diagonal entry (Cholesky), so a wide but valid spread of magnitudes
    /// factors, while cancellation — a pivot that elimination has erased
    /// relative to where its column started — is refused at any scale.
    #[test]
    fn a_wide_but_valid_magnitude_spread_factors() {
        // diag(1e12, 1e-6): condition 1e18, yet every pivot keeps its full
        // original magnitude. The old floor scaled from the matrix-wide
        // maximum refused this as singular.
        let wide = [1e12, 0.0, 0.0, 1e-6];
        let inv = dense_inverse(wide.to_vec(), 2).expect("a diagonal matrix inverts");
        assert!((inv[3] - 1e6).abs() < 1e-4, "{inv:?}");
        assert!(DenseCholesky::factor(&wide, 2).is_some());
    }

    #[test]
    fn a_cancelled_pivot_is_refused_at_any_scale() {
        // Elimination reduces the second pivot from order 1 to machine
        // epsilon: no significant digits survive.
        let close = [1.0, 1.0, 1.0, 1.0 + f64::EPSILON];
        assert!(dense_inverse(close.to_vec(), 2).is_none());
        assert!(DenseCholesky::factor(&close, 2).is_none());

        // The same cancellation scaled down by 1e14 is caught by the same
        // relative floor.
        let small = [1e-14, 1e-14, 1e-14, 1e-14 * (1.0 + f64::EPSILON)];
        assert!(dense_inverse(small.to_vec(), 2).is_none());
        assert!(DenseCholesky::factor(&small, 2).is_none());

        // A genuinely singular matrix is refused outright.
        assert!(dense_inverse(vec![1.0, 1.0, 1.0, 1.0], 2).is_none());
        assert!(DenseCholesky::factor(&[1.0, 1.0, 1.0, 1.0], 2).is_none());
    }

    #[test]
    fn a_small_well_conditioned_matrix_factors() {
        // Every entry is tiny but the matrix is perfectly conditioned; an
        // absolute floor refused it outright.
        let small = [1e-14, 0.0, 0.0, 1e-14];
        let inv =
            dense_inverse(small.to_vec(), 2).expect("a well conditioned small matrix inverts");
        assert!((inv[0] - 1e14).abs() < 1.0, "{inv:?}");
        assert!(DenseCholesky::factor(&small, 2).is_some());
    }

    /// The `!(s > floor)` idiom must still reject a NaN pivot; `NaN > x` is
    /// false, which is the whole reason the comparison is negated.
    #[test]
    fn a_nan_pivot_does_not_factor() {
        assert!(DenseCholesky::factor(&[f64::NAN, 0.0, 0.0, 1.0], 2).is_none());
        assert!(dense_inverse(vec![f64::NAN, 0.0, 0.0, 1.0], 2).is_none());
    }
}

#[cfg(test)]
mod auto_policy_tests {
    use super::{
        AUTO_DENSE_MEMORY_BUDGET, SensitivityOptions, SensitivitySolver, dense_footprint_bytes,
    };

    #[test]
    fn auto_takes_the_dense_path_for_a_small_case() {
        // Below the crossover the dense factorization's constant factors win.
        let o = SensitivityOptions::default();
        assert_eq!(
            o.select_solver_for_shape(118, 186, 118),
            SensitivitySolver::Dense
        );
        assert_eq!(
            o.select_solver_for_shape(118, 186, 118),
            o.select_solver_for_shape(118, 186, 118)
        );
    }

    #[test]
    fn auto_takes_the_sparse_path_for_a_mid_size_case() {
        // 2869 buses: the sparse factorization's fill stays near linear on a
        // network graph, while the dense path would factor a 2868² matrix.
        let o = SensitivityOptions::default();
        assert_eq!(
            o.select_solver_for_shape(2868, 4582, 2869),
            SensitivitySolver::Sparse
        );
    }

    #[test]
    fn auto_refuses_the_dense_path_for_a_wide_case() {
        // Few buses, very many parallel branches: the reduced dimension is
        // small but the dense LODF alone is m x m, so the footprint veto has
        // to fire even though the dimension test passes.
        let o = SensitivityOptions::default();
        let (nr, m, n) = (400usize, 40_000usize, 401usize);
        assert!(nr <= o.auto_dense_threshold);
        assert!(dense_footprint_bytes(nr, m, n) > AUTO_DENSE_MEMORY_BUDGET);
        assert_eq!(
            o.select_solver_for_shape(nr, m, n),
            SensitivitySolver::Sparse
        );
    }

    #[test]
    fn an_explicit_solver_choice_ignores_both_ceilings() {
        for solver in [SensitivitySolver::Dense, SensitivitySolver::Sparse] {
            let o = SensitivityOptions {
                solver,
                ..SensitivityOptions::default()
            };
            assert_eq!(o.select_solver_for_shape(1, 1, 1), solver);
            assert_eq!(o.select_solver_for_shape(99_999, 99_999, 99_999), solver);
        }
    }

    #[test]
    fn serialized_options_use_formula_and_no_retired_solver_aliases() {
        let value = serde_json::to_value(SensitivityOptions::default()).unwrap();
        assert!(value.get("formula").is_some());
        assert!(value.get("convention").is_none());
        assert!(serde_json::from_str::<SensitivitySolver>(r#""iterative""#).is_err());
        assert!(serde_json::from_str::<super::SensitivitySolverPath>(r#""iterative_cg""#).is_err());
    }

    #[test]
    fn the_footprint_saturates_instead_of_overflowing() {
        assert_eq!(
            SensitivityOptions::default().select_solver_for_shape(
                usize::MAX,
                usize::MAX,
                usize::MAX
            ),
            SensitivitySolver::Sparse
        );
    }
}
