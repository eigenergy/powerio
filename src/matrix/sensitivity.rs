//! DC sensitivity matrices.
//!
//! PTDF maps nodal injections to branch flows (`f = PTDF · p`); LODF maps a
//! branch outage to the flow it redistributes onto the others. Both come from
//! the slack-grounded DC Laplacian `ABA = ground_at(L, r)`, factored once with
//! a dense Cholesky (the matrix is SPD for a connected network). PTDF is
//! inherently dense `m × n`; for very large networks an iterative/sparse path
//! is future work.

// Dense linear algebra: indexed triangular-solve loops and the `.iter()`
// sparse traversal read clearer than the iterator rewrites clippy suggests.
#![allow(clippy::needless_range_loop, clippy::explicit_iter_loop)]

use sprs::CsMat;

use crate::case::MpcCase;
use crate::matrix::incidence::{DcConvention, IncidenceParts, build_flow_map, build_incidence};
use crate::matrix::laplacian::{build_weighted_laplacian, ground_at};
use crate::matrix::triplet::CooBuilder;
use crate::{Error, Result};

/// Entries below this magnitude are dropped from the emitted sparse matrices.
const PRUNE: f64 = 1e-12;

/// PTDF (`m × n`): branch flows from nodal injections, `f = PTDF · p`. The
/// reference-bus column is zero.
pub fn build_ptdf(case: &MpcCase, conv: DcConvention) -> Result<CsMat<f64>> {
    check_connected(case)?;
    let inc = build_incidence(case, conv)?;
    let r = case.reference_bus_index()?;
    let (dense, m, n) = ptdf_dense(&inc, r)?;
    Ok(dense_to_csr(&dense, m, n))
}

/// Reject a disconnected network up front so the singular grounded Laplacian
/// reports the real cause ([`Error::DisconnectedNetwork`]) instead of the
/// generic [`Error::SingularNetwork`] the Cholesky would otherwise raise.
fn check_connected(case: &MpcCase) -> Result<()> {
    let components = case.n_connected_components();
    if components > 1 {
        return Err(Error::DisconnectedNetwork { components });
    }
    Ok(())
}

/// LODF (`m × m`): pre-outage flow on branch `k` redistributes onto branch `l`
/// with factor `LODF[l, k]`. Diagonal is `−1`. A branch whose outage islands
/// the network (denominator `≈ 0`) gets a zero column.
pub fn build_lodf(case: &MpcCase, conv: DcConvention) -> Result<CsMat<f64>> {
    check_connected(case)?;
    let inc = build_incidence(case, conv)?;
    let r = case.reference_bus_index()?;
    let (ptdf, m, n) = ptdf_dense(&inc, r)?;

    // Branch endpoints (dense bus indices), recovered from the incidence.
    let (from, to) = endpoints(&inc.a, m);

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
    Ok(lodf.finish_csr())
}

/// Dense PTDF (`m × n`, row-major) plus its shape.
fn ptdf_dense(inc: &IncidenceParts, r: usize) -> Result<(Vec<f64>, usize, usize)> {
    let n = inc.n();
    let m = inc.m();
    let nr = n - 1;

    // Reduced inverse of the grounded Laplacian: Rinv = (ABA_r)^{-1}.
    let lr = ground_at(&build_weighted_laplacian(&inc.a, &inc.b), r);
    let chol = DenseCholesky::factor(&densify(&lr, nr), nr).ok_or(Error::SingularNetwork)?;
    let rinv = chol.inverse(); // nr × nr, row-major

    // Minv (n × n) is Rinv padded with a zero row/col at the slack bus r.
    let reduced = |i: usize| -> Option<usize> {
        match i {
            _ if i == r => None,
            _ if i > r => Some(i - 1),
            _ => Some(i),
        }
    };

    // PTDF = (B Aᵀ) · Minv, computed sparse-times-dense: each nonzero of the
    // flow map scatters a scaled Minv row into a PTDF row.
    let flow = build_flow_map(&inc.a, &inc.b); // m × n
    let mut ptdf = vec![0.0; m * n];
    for (&w, (l, c)) in flow.iter() {
        let Some(rc) = reduced(c) else { continue }; // Minv row at slack is 0
        for k in 0..n {
            if let Some(rk) = reduced(k) {
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
