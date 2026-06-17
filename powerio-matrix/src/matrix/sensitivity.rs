//! DC sensitivity matrices.
//!
//! PTDF maps nodal injections to branch flows (`f = PTDF · p`); LODF maps a
//! branch outage to the flow it redistributes onto the others. Both come from
//! the reference grounded DC Laplacian `ABA = ground_with(L, refs)`: one
//! row/column removed per reference bus. Positive branch weights use a dense
//! Cholesky factorization; nonsingular indefinite cases fall back to dense
//! Gaussian elimination. Disconnected networks with one reference per island are
//! supported. Several references in one island are fixed angle buses; this is
//! not a participation factor based distributed slack model. PTDF is dense
//! `m × n`; a future sparse path would compute selected columns or use sparse
//! factors rather than make PTDF itself sparse.

// Dense linear algebra: indexed triangular-solve loops and the `.iter()`
// sparse traversal read clearer than the iterator rewrites clippy suggests.
#![allow(clippy::needless_range_loop, clippy::explicit_iter_loop)]

use sprs::CsMat;

use crate::indexed::IndexedNetwork;
use crate::matrix::incidence::{DcConvention, IncidenceParts, build_flow_map, build_incidence};
use crate::matrix::laplacian::{Grounding, build_weighted_laplacian, ground_with};
use crate::matrix::triplet::CooBuilder;
use crate::{Error, Result};

/// Entries below this magnitude are dropped from the emitted sparse matrices.
const PRUNE: f64 = 1e-12;

/// PTDF (`m × n`): branch flows from nodal injections, `f = PTDF · p`. Every
/// reference-bus column is zero. The Laplacian is grounded at the whole
/// reference set (`reference_bus_indices`), one row/column per slack. One
/// reference per island handles disconnected networks; several references within
/// one island fixes all of those bus angles to zero.
pub fn build_ptdf(case: &IndexedNetwork, conv: DcConvention) -> Result<CsMat<f64>> {
    case.check_reference_coverage()?;
    let refs = case.reference_bus_indices();
    let inc = build_incidence(case, conv)?;
    let (dense, m, n) = ptdf_dense(&inc, &refs)?;
    Ok(dense_to_csr(&dense, m, n))
}

/// LODF (`m × m`): pre-outage flow on branch `k` redistributes onto branch `l`
/// with factor `LODF[l, k]`. Diagonal is `−1`. A branch whose outage islands
/// the network (denominator `≈ 0`) gets a zero column.
pub fn build_lodf(case: &IndexedNetwork, conv: DcConvention) -> Result<CsMat<f64>> {
    case.check_reference_coverage()?;
    let refs = case.reference_bus_indices();
    let inc = build_incidence(case, conv)?;
    let (ptdf, m, n) = ptdf_dense(&inc, &refs)?;
    Ok(lodf_from_dense(&ptdf, &inc.a, m, n))
}

/// Both DC sensitivity matrices `(PTDF, LODF)` from a single Laplacian
/// factorization. When a caller needs both for the same case (the
/// `sensitivities` bundle), this factors and inverts the grounded Laplacian
/// once instead of paying the O(n³) twice across separate
/// [`build_ptdf`]/[`build_lodf`] calls.
pub fn build_ptdf_lodf(
    case: &IndexedNetwork,
    conv: DcConvention,
) -> Result<(CsMat<f64>, CsMat<f64>)> {
    case.check_reference_coverage()?;
    let refs = case.reference_bus_indices();
    let inc = build_incidence(case, conv)?;
    let (dense, m, n) = ptdf_dense(&inc, &refs)?;
    let ptdf = dense_to_csr(&dense, m, n);
    let lodf = lodf_from_dense(&dense, &inc.a, m, n);
    Ok((ptdf, lodf))
}

/// LODF from a dense PTDF and the signed incidence (the shared tail of
/// [`build_lodf`] and [`build_ptdf_lodf`]).
fn lodf_from_dense(ptdf: &[f64], a: &CsMat<f64>, m: usize, n: usize) -> CsMat<f64> {
    // Branch endpoints (dense bus indices), recovered from the incidence.
    let (from, to) = endpoints(a, m);

    // δ[l,k] = PTDF[l, from_k] − PTDF[l, to_k]: flow on l from a unit transfer
    // along branch k.
    let delta = |l: usize, k: usize| ptdf[l * n + from[k]] - ptdf[l * n + to[k]];

    let mut lodf = CooBuilder::new(m); // m × m
    for k in 0..m {
        let denom = 1.0 - delta(k, k);
        let islands = denom.abs() < 1e-9;
        for l in 0..m {
            let v = if l == k {
                -1.0
            } else if islands {
                0.0
            } else {
                delta(l, k) / denom
            };
            if v.abs() > PRUNE {
                lodf.add(l, k, v);
            }
        }
    }
    lodf.finish_csr()
}

/// Dense PTDF (`m × n`, row-major) plus its shape. `refs` is the reference set;
/// the Laplacian is grounded at every entry (one row/column each).
fn ptdf_dense(inc: &IncidenceParts, refs: &[usize]) -> Result<(Vec<f64>, usize, usize)> {
    let n = inc.n();
    let m = inc.m();
    let g = Grounding::new(refs);
    let nr = n - g.len();

    // Reduced inverse of the grounded Laplacian: Rinv = (ABA_refs)^{-1}.
    let lr = ground_with(&build_weighted_laplacian(&inc.a, &inc.b), &g);
    let dense_lr = densify(&lr, nr);
    let rinv = DenseCholesky::factor(&dense_lr, nr).map_or_else(
        || dense_inverse(&dense_lr, nr).ok_or(Error::SingularNetwork),
        |chol| Ok(chol.inverse()),
    )?; // nr × nr, row-major

    // Minv (n × n) is Rinv padded with a zero row/col at every grounded bus, so
    // each reference's PTDF column comes out zero. PTDF = (B Aᵀ) · Minv, computed
    // sparse-times-dense: each nonzero of the flow map scatters a scaled Minv row
    // into a PTDF row.
    let flow = build_flow_map(&inc.a, &inc.b); // m × n
    let mut ptdf = vec![0.0; m * n];
    for (&w, (l, c)) in flow.iter() {
        let Some(rc) = g.reduced(c) else { continue }; // Minv row at a slack is 0
        for k in 0..n {
            if let Some(rk) = g.reduced(k) {
                ptdf[l * n + k] += w * rinv[rc * nr + rk];
            }
        }
    }
    Ok((ptdf, m, n))
}

/// Branch endpoints from the signed incidence: `+1` row is from, `−1` is to.
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
    let mut coo = CooBuilder::with_capacity_rect(rows, cols, dense.len() / 2);
    for i in 0..rows {
        for j in 0..cols {
            let v = dense[i * cols + j];
            if v.abs() > PRUNE {
                coo.add(i, j, v);
            }
        }
    }
    coo.finish_csr()
}

fn dense_inverse(a: &[f64], n: usize) -> Option<Vec<f64>> {
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
        if !pivot_abs.is_finite() || pivot_abs <= 1e-12 {
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

/// Dense lower-triangular Cholesky `A = L Lᵀ` for a small SPD matrix.
struct DenseCholesky {
    n: usize,
    l: Vec<f64>, // row-major lower triangle
}

impl DenseCholesky {
    fn factor(a: &[f64], n: usize) -> Option<Self> {
        let mut l = vec![0.0; n * n];
        for i in 0..n {
            for j in 0..=i {
                let mut s = a[i * n + j];
                for k in 0..j {
                    s -= l[i * n + k] * l[j * n + k];
                }
                if i == j {
                    // `!(s > 0.0)` rejects negative, zero, AND NaN pivots:
                    // `NaN <= 0.0` is false, so `s <= 0.0` would let a
                    // NaN-poisoned matrix factor "successfully" into all-NaN.
                    // The negated comparison is the point (NaN incomparability),
                    // so the partial_cmp rewrite clippy suggests would obscure it.
                    #[allow(clippy::neg_cmp_op_on_partial_ord)]
                    if !(s > 0.0) {
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
