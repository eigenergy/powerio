//! DC sensitivity matrices.
//!
//! PTDF maps nodal injections to branch flows (`f = PTDF · p`); LODF maps a
//! branch outage to the flow it redistributes onto the others. Both come from
//! the reference grounded DC bus susceptance matrix
//! `ABA = ground_with(L, refs)`: one row/column removed per reference bus. The
//! default public builders keep the dense Cholesky path, with dense Gaussian
//! elimination as the nonsingular indefinite fallback. Option based builders can
//! choose an iterative path that solves one grounded right hand side at a time
//! and writes directly into sparse output. Disconnected networks with one
//! reference per island are supported.
//! Several references in one island are fixed angle buses; this is not a
//! participation factor based distributed slack model.

// Dense linear algebra: indexed triangular-solve loops and the `.iter()`
// sparse traversal read clearer than the iterator rewrites clippy suggests.
#![allow(clippy::needless_range_loop, clippy::explicit_iter_loop)]

use sprs::CsMat;

use crate::indexed::IndexedNetwork;
use crate::matrix::BuildOptions;
use crate::matrix::incidence::{DcConvention, IncidenceParts, build_flow_map, build_incidence};
use crate::matrix::laplacian::{Grounding, build_weighted_laplacian, ground_with};
use crate::matrix::triplet::CooBuilder;
use crate::{Error, Result};

/// Entries below this magnitude are dropped from the emitted sparse matrices.
const PRUNE: f64 = 1e-12;
const DEFAULT_CG_TOLERANCE: f64 = 1e-10;
const DEFAULT_CG_MAX_ITERATIONS: usize = 20_000;
/// Reduced-dimension ceiling for the `Auto` dense path. The old value of 512
/// was far below the real crossover: at nr = 600 the dense path is a ~7e7
/// flop factorization while the iterative path runs ~1200 conjugate-gradient
/// solves, so `Auto` picked the slower solver by one to three orders of
/// magnitude across the whole range that holds the common published cases
/// (1354, 2869, 3120, 6470 buses).
const DEFAULT_AUTO_DENSE_THRESHOLD: usize = 8192;

/// Memory ceiling for the `Auto` dense path. The dimension alone does not
/// bound the cost: the dense path also materializes an m x n PTDF and an
/// m x m LODF, so a case with few buses and many parallel branches could ask
/// for tens of GB while passing any nr test.
const AUTO_DENSE_MEMORY_BUDGET: usize = 2 << 30;

/// Peak bytes the dense path holds: the reduced Laplacian and its
/// factorization and inverse (three nr x nr buffers alive together), plus the
/// dense PTDF and the LODF built from it.
fn dense_footprint_bytes(reduced_dimension: usize, branches: usize, buses: usize) -> usize {
    let f = size_of::<f64>();
    let sq = |a: usize, b: usize| a.saturating_mul(b).saturating_mul(f);
    sq(reduced_dimension, reduced_dimension)
        .saturating_mul(3)
        .saturating_add(sq(branches, buses))
        .saturating_add(sq(branches, branches))
}
const LODF_ISLAND_TOLERANCE: f64 = 1e-9;

/// Solver selection for option based DC sensitivity builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SensitivitySolver {
    /// Dense below [`SensitivityOptions::auto_dense_threshold`], iterative above it.
    #[default]
    Auto,
    /// Dense grounded inverse. This is the historical builder path.
    Dense,
    /// Preconditioned conjugate gradient, one right hand side at a time.
    Iterative,
}

/// Solver path actually used for a sensitivity build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SensitivitySolverPath {
    DenseCholesky,
    DenseInverse,
    IterativeCg,
}

impl SensitivitySolverPath {
    #[inline]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DenseCholesky => "dense_cholesky",
            Self::DenseInverse => "dense_inverse",
            Self::IterativeCg => "iterative_cg",
        }
    }
}

/// Options for PTDF/LODF builders that expose solver choice and output pruning.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct SensitivityOptions {
    /// DC branch susceptance convention.
    pub convention: DcConvention,
    /// Solver selection policy.
    pub solver: SensitivitySolver,
    /// Entries with absolute value at or below this value are omitted from the
    /// returned sparse matrices. LODF diagonal entries are structural and kept.
    pub drop_tolerance: f64,
    /// Relative residual tolerance for the iterative solver.
    pub cg_tolerance: f64,
    /// Maximum conjugate gradient iterations per right hand side.
    pub cg_max_iterations: usize,
    /// Reduced dimension above which [`SensitivitySolver::Auto`] selects the
    /// iterative path.
    pub auto_dense_threshold: usize,
}

impl Default for SensitivityOptions {
    fn default() -> Self {
        Self {
            convention: DcConvention::PaperPure,
            solver: SensitivitySolver::Auto,
            drop_tolerance: PRUNE,
            cg_tolerance: DEFAULT_CG_TOLERANCE,
            cg_max_iterations: DEFAULT_CG_MAX_ITERATIONS,
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
        if !self.cg_tolerance.is_finite() || self.cg_tolerance <= 0.0 {
            return Err(Error::InvalidSensitivityOptions {
                reason: format!(
                    "cg_tolerance must be finite and positive, got {}",
                    self.cg_tolerance
                ),
            });
        }
        if self.cg_max_iterations == 0 {
            return Err(Error::InvalidSensitivityOptions {
                reason: "cg_max_iterations must be positive".into(),
            });
        }
        Ok(())
    }

    /// Return the concrete solver selected for a reduced grounded dimension,
    /// assuming a square problem. Prefer
    /// [`Self::selected_solver_for_shape`], which also sees the branch count
    /// the dense PTDF and LODF are sized by.
    pub fn selected_solver_for_reduced_dimension(
        &self,
        reduced_dimension: usize,
    ) -> SensitivitySolver {
        self.selected_solver_for_shape(reduced_dimension, reduced_dimension, reduced_dimension)
    }

    /// Return the concrete solver selected for a problem shape. `Auto` takes
    /// the dense path while both the reduced dimension and the predicted
    /// dense footprint stay within their ceilings, so a wide case (few buses,
    /// many branches) no longer picks a path that would ask for tens of GB.
    pub fn selected_solver_for_shape(
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
                    SensitivitySolver::Iterative
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
    pub cg_tolerance: Option<f64>,
    pub cg_max_iterations: Option<usize>,
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
pub fn build_ptdf(case: &IndexedNetwork, conv: DcConvention) -> Result<CsMat<f64>> {
    case.check_reference_coverage()?;
    let refs = case.reference_bus_indices();
    let inc = build_incidence(case, conv, &BuildOptions::default())?;
    let (dense, m, n) = ptdf_dense(&inc, &refs)?;
    Ok(dense_to_csr(&dense, m, n))
}

/// LODF (`m × m`): pre-outage flow on branch `k` redistributes onto branch `l`
/// with factor `LODF[l, k]`. Diagonal is `−1`. A branch whose outage islands
/// the network (denominator `≈ 0`) gets a zero column.
pub fn build_lodf(case: &IndexedNetwork, conv: DcConvention) -> Result<CsMat<f64>> {
    case.check_reference_coverage()?;
    let refs = case.reference_bus_indices();
    let inc = build_incidence(case, conv, &BuildOptions::default())?;
    let (ptdf, m, n) = ptdf_dense(&inc, &refs)?;
    Ok(lodf_from_dense(&ptdf, &inc.a, m, n))
}

/// Both DC sensitivity matrices `(PTDF, LODF)` from one DC bus susceptance matrix
/// factorization. When a caller needs both for the same case (the
/// `sensitivities` bundle), this factors and inverts the grounded DC bus
/// susceptance matrix once instead of paying the O(n³) twice across separate
/// [`build_ptdf`]/[`build_lodf`] calls.
pub fn build_ptdf_lodf(
    case: &IndexedNetwork,
    conv: DcConvention,
) -> Result<(CsMat<f64>, CsMat<f64>)> {
    case.check_reference_coverage()?;
    let refs = case.reference_bus_indices();
    let inc = build_incidence(case, conv, &BuildOptions::default())?;
    let (dense, m, n) = ptdf_dense(&inc, &refs)?;
    let ptdf = dense_to_csr(&dense, m, n);
    let lodf = lodf_from_dense(&dense, &inc.a, m, n);
    Ok((ptdf, lodf))
}

/// PTDF and LODF with solver selection, drop tolerance, and output metadata.
pub fn build_ptdf_lodf_with_options(
    case: &IndexedNetwork,
    options: &SensitivityOptions,
) -> Result<SensitivityMatrices> {
    options.validate()?;
    case.check_reference_coverage()?;
    let refs = case.reference_bus_indices();
    let inc = build_incidence(case, options.convention, &BuildOptions::default())?;
    let reduced_dimension = inc.n().saturating_sub(Grounding::new(&refs).len());

    let (ptdf, lodf, solver_path, ptdf_dropped, lodf_dropped) = match options
        .selected_solver_for_shape(reduced_dimension, inc.m(), inc.n())
    {
        SensitivitySolver::Dense => {
            let (dense, m, n, solver_path) = ptdf_dense_with_path(&inc, &refs)?;
            let (ptdf, ptdf_dropped) = dense_to_csr_with_drop(&dense, m, n, options.drop_tolerance);
            let (lodf, lodf_dropped) =
                lodf_from_dense_with_drop(&dense, &inc.a, m, n, options.drop_tolerance);
            (ptdf, lodf, solver_path, ptdf_dropped, lodf_dropped)
        }
        SensitivitySolver::Iterative => {
            ensure_iterative_solver_eligible(&inc)?;
            let (ptdf, ptdf_dropped, lodf, lodf_dropped) =
                iterative_ptdf_lodf(&inc, &refs, options)?;
            (
                ptdf,
                lodf,
                SensitivitySolverPath::IterativeCg,
                ptdf_dropped,
                lodf_dropped,
            )
        }
        SensitivitySolver::Auto => unreachable!("selected_solver resolves Auto"),
    };

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

pub(crate) fn for_each_ptdf_lodf_entry(
    case: &IndexedNetwork,
    options: &SensitivityOptions,
    mut ptdf_entry: impl FnMut(usize, usize, f64) -> Result<()>,
    mut lodf_entry: impl FnMut(usize, usize, f64) -> Result<()>,
) -> Result<SensitivityMetadata> {
    options.validate()?;
    case.check_reference_coverage()?;
    let refs = case.reference_bus_indices();
    let inc = build_incidence(case, options.convention, &BuildOptions::default())?;
    let reduced_dimension = inc.n().saturating_sub(Grounding::new(&refs).len());

    let (solver_path, ptdf, lodf) =
        match options.selected_solver_for_shape(reduced_dimension, inc.m(), inc.n()) {
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
            SensitivitySolver::Iterative => {
                ensure_iterative_solver_eligible(&inc)?;
                let (ptdf, lodf) =
                    iterative_ptdf_lodf_entries(&inc, &refs, options, ptdf_entry, lodf_entry)?;
                (SensitivitySolverPath::IterativeCg, ptdf, lodf)
            }
            SensitivitySolver::Auto => {
                unreachable!("selected_solver_for_reduced_dimension resolves Auto")
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
        cg_tolerance: matches!(solver_path, SensitivitySolverPath::IterativeCg)
            .then_some(options.cg_tolerance),
        cg_max_iterations: matches!(solver_path, SensitivitySolverPath::IterativeCg)
            .then_some(options.cg_max_iterations),
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

/// LODF from a dense PTDF and the signed incidence (the shared tail of
/// [`build_lodf`] and [`build_ptdf_lodf`]).
fn lodf_from_dense(ptdf: &[f64], a: &CsMat<f64>, m: usize, n: usize) -> CsMat<f64> {
    lodf_from_dense_with_drop(ptdf, a, m, n, PRUNE).0
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

    // δ[l,k] = PTDF[l, from_k] − PTDF[l, to_k]: flow on l from a unit transfer
    // along branch k.
    let delta = |l: usize, k: usize| ptdf[l * n + from[k]] - ptdf[l * n + to[k]];

    // Outaging a bridge redistributes nothing, so its column is structurally
    // zero. The magnitude test this replaces let a near bridge at
    // `delta(k,k) = 1 - 1.1e-9` through, amplifying its column to ~1e9 with
    // about seven digits gone.
    let is_bridge = bridges(&from, &to, n);

    let mut lodf = CooBuilder::new(m); // m × m
    let mut dropped = 0usize;
    for k in 0..m {
        let denom = 1.0 - delta(k, k);
        let islands = is_bridge[k] || denom.abs() < LODF_ISLAND_TOLERANCE;
        for l in 0..m {
            let v = if l == k {
                -1.0
            } else if islands {
                0.0
            } else {
                delta(l, k) / denom
            };
            if l == k || v.abs() > drop_tolerance {
                lodf.add(l, k, v);
            } else if v != 0.0 {
                dropped += 1;
            }
        }
    }
    (lodf.finish_csr(), dropped)
}

/// Dense PTDF (`m × n`, row-major) plus its shape. `refs` is the reference set;
/// the DC bus susceptance matrix is grounded at every entry (one row/column each).
fn ptdf_dense(inc: &IncidenceParts, refs: &[usize]) -> Result<(Vec<f64>, usize, usize)> {
    let (ptdf, m, n, _) = ptdf_dense_with_path(inc, refs)?;
    Ok((ptdf, m, n))
}

fn ptdf_dense_with_path(
    inc: &IncidenceParts,
    refs: &[usize],
) -> Result<(Vec<f64>, usize, usize, SensitivitySolverPath)> {
    let n = inc.n();
    let m = inc.m();
    let g = Grounding::new(refs);
    let nr = n - g.len();

    // Reduced inverse of the grounded DC bus susceptance matrix: Rinv = (ABA_refs)^{-1}.
    let lr = ground_with(&build_weighted_laplacian(&inc.a, &inc.b), &g);
    let dense_lr = densify(&lr, nr);
    let (rinv, solver_path) = DenseCholesky::factor(&dense_lr, nr).map_or_else(
        || {
            dense_inverse(&dense_lr, nr)
                .map(|rinv| (rinv, SensitivitySolverPath::DenseInverse))
                .ok_or(Error::SingularNetwork)
        },
        |chol| Ok((chol.inverse(), SensitivitySolverPath::DenseCholesky)),
    )?; // nr × nr, row-major

    // Minv (n × n) is Rinv padded with a zero row/col at every grounded bus, so
    // each reference's PTDF column comes out zero. PTDF = (B Aᵀ) · Minv, computed
    // sparse-times-dense: each nonzero of the flow map scatters a scaled Minv row
    // into a PTDF row.
    let flow = build_flow_map(&inc.a, &inc.b); // m × n
    let mut ptdf = vec![0.0; m * n];
    // Reduced → full column map, built once. The inner loop then walks the
    // Rinv row contiguously and skips grounded columns instead of testing
    // every one of them; reduced order is ascending full order, so the
    // accumulation order is unchanged.
    let full_of = g.full_of_reduced(n);
    for (&w, (l, c)) in flow.iter() {
        let Some(rc) = g.reduced(c) else { continue }; // Minv row at a slack is 0
        let row = &rinv[rc * nr..rc * nr + nr];
        let out = &mut ptdf[l * n..l * n + n];
        for (rk, &k) in full_of.iter().enumerate() {
            out[k] += w * row[rk];
        }
    }
    Ok((ptdf, m, n, solver_path))
}

fn iterative_ptdf_lodf(
    inc: &IncidenceParts,
    refs: &[usize],
    options: &SensitivityOptions,
) -> Result<(CsMat<f64>, usize, CsMat<f64>, usize)> {
    ensure_iterative_solver_eligible(inc)?;
    let mut ptdf = CooBuilder::new_rect(inc.m(), inc.n());
    let mut lodf = CooBuilder::new(inc.m());
    let (ptdf_meta, lodf_meta) = iterative_ptdf_lodf_entries(
        inc,
        refs,
        options,
        |row, col, value| {
            ptdf.add(row, col, value);
            Ok(())
        },
        |row, col, value| {
            lodf.add(row, col, value);
            Ok(())
        },
    )?;
    Ok((
        ptdf.finish_csr(),
        ptdf_meta.dropped_entries,
        lodf.finish_csr(),
        lodf_meta.dropped_entries,
    ))
}

fn iterative_ptdf_lodf_entries(
    inc: &IncidenceParts,
    refs: &[usize],
    options: &SensitivityOptions,
    mut ptdf_entry: impl FnMut(usize, usize, f64) -> Result<()>,
    mut lodf_entry: impl FnMut(usize, usize, f64) -> Result<()>,
) -> Result<(SensitivityMatrixMetadata, SensitivityMatrixMetadata)> {
    let n = inc.n();
    let m = inc.m();
    let g = Grounding::new(refs);
    let nr = n - g.len();
    let lr = ground_with(&build_weighted_laplacian(&inc.a, &inc.b), &g);
    let solver = CgSolver::new(&lr, options.cg_tolerance, options.cg_max_iterations)?;
    let (from, to) = endpoints(&inc.a, m);

    let mut rhs = vec![0.0; nr];
    let mut ptdf_nnz = 0usize;
    let mut ptdf_dropped = 0usize;
    for bus in 0..n {
        let Some(rb) = g.reduced(bus) else {
            continue;
        };
        rhs.fill(0.0);
        rhs[rb] = 1.0;
        let theta = solver.solve(&rhs)?;
        for branch in 0..m {
            let v = branch_flow(branch, &from, &to, &inc.b, &g, &theta);
            if v.abs() > options.drop_tolerance {
                ptdf_entry(branch, bus, v)?;
                ptdf_nnz += 1;
            } else if v != 0.0 {
                ptdf_dropped += 1;
            }
        }
    }

    // Same rule as the dense path: a bridge redistributes nothing, decided on
    // the topology rather than on how close the denominator came to zero.
    let is_bridge = bridges(&from, &to, n);

    let mut lodf_nnz = 0usize;
    let mut lodf_dropped = 0usize;
    for outage in 0..m {
        rhs.fill(0.0);
        if let Some(rf) = g.reduced(from[outage]) {
            rhs[rf] += 1.0;
        }
        if let Some(rt) = g.reduced(to[outage]) {
            rhs[rt] -= 1.0;
        }
        let theta = solver.solve(&rhs)?;
        let outage_delta = branch_flow(outage, &from, &to, &inc.b, &g, &theta);
        let denom = 1.0 - outage_delta;
        let islands = is_bridge[outage] || denom.abs() < LODF_ISLAND_TOLERANCE;
        for branch in 0..m {
            let v = if branch == outage {
                -1.0
            } else if islands {
                0.0
            } else {
                branch_flow(branch, &from, &to, &inc.b, &g, &theta) / denom
            };
            if branch == outage || v.abs() > options.drop_tolerance {
                lodf_entry(branch, outage, v)?;
                lodf_nnz += 1;
            } else if v != 0.0 {
                lodf_dropped += 1;
            }
        }
    }

    Ok((
        SensitivityMatrixMetadata {
            rows: m,
            cols: n,
            nnz: ptdf_nnz,
            dropped_entries: ptdf_dropped,
        },
        SensitivityMatrixMetadata {
            rows: m,
            cols: m,
            nnz: lodf_nnz,
            dropped_entries: lodf_dropped,
        },
    ))
}

fn ensure_iterative_solver_eligible(inc: &IncidenceParts) -> Result<()> {
    for (branch, &b) in inc.b.iter().enumerate() {
        if !b.is_finite() || b <= 0.0 {
            return Err(Error::InvalidSensitivityOptions {
                reason: format!(
                    "iterative sensitivity solver requires positive finite branch susceptances; \
                     branch {branch} has {b}; use solver=dense for nonsingular indefinite cases"
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

fn dense_to_csr(dense: &[f64], rows: usize, cols: usize) -> CsMat<f64> {
    dense_to_csr_with_drop(dense, rows, cols, PRUNE).0
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

fn dense_inverse(a: &[f64], n: usize) -> Option<Vec<f64>> {
    // The pivot floor tracks the matrix's own scale. A fixed 1e-12 is at once
    // too strict for a legitimately small scaled matrix and far too loose for
    // one whose entries run to 1e12.
    let scale = a.iter().fold(0.0_f64, |m, v| m.max(v.abs()));
    #[allow(clippy::cast_precision_loss)]
    let floor = n as f64 * f64::EPSILON * scale;
    let mut a = a.to_vec();
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
        if !pivot_abs.is_finite() || pivot_abs <= floor {
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

struct CgSolver<'a> {
    a: &'a CsMat<f64>,
    diag: Vec<f64>,
    tolerance: f64,
    max_iterations: usize,
}

impl<'a> CgSolver<'a> {
    fn new(a: &'a CsMat<f64>, tolerance: f64, max_iterations: usize) -> Result<Self> {
        let n = a.rows();
        if a.cols() != n {
            return Err(Error::ShapeMismatch {
                what: "grounded DC bus susceptance matrix columns",
                expected: n,
                got: a.cols(),
            });
        }
        let mut diag = vec![0.0; n];
        for (i, slot) in diag.iter_mut().enumerate() {
            *slot = a.get(i, i).copied().unwrap_or(0.0);
            if !slot.is_finite() || *slot <= 0.0 {
                return Err(Error::SingularNetwork);
            }
        }
        Ok(Self {
            a,
            diag,
            tolerance,
            max_iterations,
        })
    }

    fn solve(&self, rhs: &[f64]) -> Result<Vec<f64>> {
        let n = self.a.rows();
        if rhs.len() != n {
            return Err(Error::DimensionMismatch {
                n,
                b_len: rhs.len(),
            });
        }
        if n == 0 {
            return Ok(Vec::new());
        }

        let rhs_norm = norm2(rhs);
        if rhs_norm == 0.0 {
            return Ok(vec![0.0; n]);
        }
        let target = self.tolerance * rhs_norm;
        let mut solution = vec![0.0; n];
        let mut residual_vec = rhs.to_vec();
        let mut preconditioned = self.precondition(&residual_vec);
        let mut direction = preconditioned.clone();
        let mut residual_dot = dot(&residual_vec, &preconditioned);
        if !residual_dot.is_finite() || residual_dot <= 0.0 {
            return Err(Error::SingularNetwork);
        }
        let mut matvec_out = vec![0.0; n];

        for iter in 1..=self.max_iterations {
            matvec(self.a, &direction, &mut matvec_out);
            let denom = dot(&direction, &matvec_out);
            if !denom.is_finite() || denom <= 0.0 {
                return Err(Error::SingularNetwork);
            }
            let alpha = residual_dot / denom;
            for i in 0..n {
                solution[i] += alpha * direction[i];
                residual_vec[i] -= alpha * matvec_out[i];
            }
            let residual = norm2(&residual_vec);
            if residual <= target {
                return Ok(solution);
            }
            preconditioned = self.precondition(&residual_vec);
            let next_residual_dot = dot(&residual_vec, &preconditioned);
            if !next_residual_dot.is_finite() || next_residual_dot <= 0.0 {
                return Err(Error::SingularNetwork);
            }
            let beta = next_residual_dot / residual_dot;
            for i in 0..n {
                direction[i] = preconditioned[i] + beta * direction[i];
            }
            residual_dot = next_residual_dot;

            if iter == self.max_iterations {
                return Err(Error::SensitivitySolveDidNotConverge {
                    iterations: iter,
                    relative_residual: residual / rhs_norm,
                });
            }
        }
        unreachable!("positive max_iterations loop returns")
    }

    fn precondition(&self, r: &[f64]) -> Vec<f64> {
        r.iter().zip(&self.diag).map(|(&ri, &di)| ri / di).collect()
    }
}

fn matvec(a: &CsMat<f64>, x: &[f64], out: &mut [f64]) {
    out.fill(0.0);
    for (i, row) in a.outer_iterator().enumerate() {
        let mut sum = 0.0;
        for (j, &v) in row.iter() {
            sum += v * x[j];
        }
        out[i] = sum;
    }
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(&x, &y)| x * y).sum()
}

fn norm2(a: &[f64]) -> f64 {
    dot(a, a).sqrt()
}

/// Dense lower-triangular Cholesky `A = L Lᵀ` for a small SPD matrix.
struct DenseCholesky {
    n: usize,
    l: Vec<f64>, // row-major lower triangle
}

impl DenseCholesky {
    fn factor(a: &[f64], n: usize) -> Option<Self> {
        // A pivot is accepted against the matrix's own scale. `s > 0.0` alone
        // accepts 1e-300, whose square root divides the column by 1e-150 twice
        // and returns entries near 1e300 with no error — the shape a near
        // disconnected island joined by one very high impedance branch takes,
        // which `check_reference_coverage` passes.
        let scale = a.iter().fold(0.0_f64, |m, v| m.max(v.abs()));
        #[allow(clippy::cast_precision_loss)]
        let floor = n as f64 * f64::EPSILON * scale;
        let mut l = vec![0.0; n * n];
        for i in 0..n {
            for j in 0..=i {
                let mut s = a[i * n + j];
                for k in 0..j {
                    s -= l[i * n + k] * l[j * n + k];
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
                    l[i * n + i] = s.sqrt();
                } else {
                    l[i * n + j] = s / l[j * n + j];
                }
            }
        }
        Some(Self { n, l })
    }

    /// Solve `A x = b` in place.
    fn solve(&self, b: &mut [f64]) {
        let n = self.n;
        for i in 0..n {
            let mut s = b[i];
            for k in 0..i {
                s -= self.l[i * n + k] * b[k];
            }
            b[i] = s / self.l[i * n + i];
        }
        for i in (0..n).rev() {
            let mut s = b[i];
            for k in (i + 1)..n {
                s -= self.l[k * n + i] * b[k];
            }
            b[i] = s / self.l[i * n + i];
        }
    }

    /// Full inverse, row-major. The matrix is symmetric, so rows = columns.
    fn inverse(&self) -> Vec<f64> {
        let n = self.n;
        let mut inv = vec![0.0; n * n];
        let mut e = vec![0.0; n];
        for j in 0..n {
            e.fill(0.0);
            e[j] = 1.0;
            self.solve(&mut e);
            for (i, &x) in e.iter().enumerate() {
                inv[i * n + j] = x;
            }
        }
        inv
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

    /// #292. The pivot floor tracks the matrix's own scale, so it rejects the
    /// same relative degeneracy at any magnitude and accepts a matrix that is
    /// merely small.
    #[test]
    fn a_pivot_is_judged_against_the_matrix_scale() {
        // Scaled up: against entries of 1e12 a pivot of 1e-6 carries no
        // significant digits, and the old absolute 1e-12 accepted it.
        let big = [1e12, 0.0, 0.0, 1e-6];
        assert!(dense_inverse(&big, 2).is_none(), "1e-18 relative accepted");
        assert!(DenseCholesky::factor(&big, 2).is_none());

        // Scaled down: every entry is tiny but the matrix is perfectly
        // conditioned, and the old absolute floor refused it outright.
        let small = [1e-14, 0.0, 0.0, 1e-14];
        let inv = dense_inverse(&small, 2).expect("a well conditioned small matrix inverts");
        assert!((inv[0] - 1e14).abs() < 1.0, "{inv:?}");
        assert!(DenseCholesky::factor(&small, 2).is_some());

        // A genuinely singular matrix is still refused at any scale.
        assert!(dense_inverse(&[1.0, 1.0, 1.0, 1.0], 2).is_none());
        assert!(DenseCholesky::factor(&[1.0, 1.0, 1.0, 1.0], 2).is_none());
    }

    /// The `!(s > floor)` idiom must still reject a NaN pivot; `NaN > x` is
    /// false, which is the whole reason the comparison is negated.
    #[test]
    fn a_nan_pivot_does_not_factor() {
        assert!(DenseCholesky::factor(&[f64::NAN, 0.0, 0.0, 1.0], 2).is_none());
        assert!(dense_inverse(&[f64::NAN, 0.0, 0.0, 1.0], 2).is_none());
    }
}

#[cfg(test)]
mod auto_policy_tests {
    use super::{
        AUTO_DENSE_MEMORY_BUDGET, SensitivityOptions, SensitivitySolver, dense_footprint_bytes,
    };

    #[test]
    fn auto_takes_the_dense_path_for_a_mid_size_case() {
        // 2869 buses is a common published case, far inside the dense path's
        // real crossover; the old 512 ceiling sent it to the iterative
        // solver, one to three orders of magnitude slower in this range.
        let o = SensitivityOptions::default();
        assert_eq!(
            o.selected_solver_for_shape(2868, 4582, 2869),
            SensitivitySolver::Dense
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
            o.selected_solver_for_shape(nr, m, n),
            SensitivitySolver::Iterative
        );
    }

    #[test]
    fn an_explicit_solver_choice_ignores_both_ceilings() {
        for solver in [SensitivitySolver::Dense, SensitivitySolver::Iterative] {
            let o = SensitivityOptions {
                solver,
                ..SensitivityOptions::default()
            };
            assert_eq!(o.selected_solver_for_shape(1, 1, 1), solver);
            assert_eq!(o.selected_solver_for_shape(99_999, 99_999, 99_999), solver);
        }
    }

    #[test]
    fn the_footprint_saturates_instead_of_overflowing() {
        assert_eq!(
            SensitivityOptions::default().selected_solver_for_shape(
                usize::MAX,
                usize::MAX,
                usize::MAX
            ),
            SensitivitySolver::Iterative
        );
    }
}
