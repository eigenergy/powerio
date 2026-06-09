//! Interior point operator assembly for the DC-OPF Newton step.
//!
//! The Θ⁻¹ diagonals are central-path state (they change every IPM
//! iteration), so they are passed in, never derived from the case. Given the
//! case factors `A`, `b`, `L` and the bus cost `q`, this builds the operators
//! the reduced Newton system needs:
//!
//! ```text
//! L_eff = A B Θf⁻¹ B Aᵀ + L Dg⁻¹ L = L₁ + L₂ D L₂,   Dg = (Q + Θg⁻¹)⁻¹
//! ```
//!
//! The solver multiplies by `L₁`, solves with `L₂ = L`, and scales by
//! `D^{±1/2}`, all grounded at the slack bus, so the grounded factors are the
//! primary output; the dense `L_eff` is optional.

// Dense linear algebra: single-char indices (i, j, k, m, n) are the math
// notation, and the assembly entry points take the operators as flat inputs.
#![allow(clippy::many_single_char_names, clippy::too_many_arguments)]

use sprs::CsMat;

use crate::matrix::incidence::{build_flow_map, diagonal};
use crate::matrix::laplacian::{GroundedIndexMap, build_weighted_laplacian, ground_at};
use crate::matrix::triplet::CooBuilder;
use crate::{Error, Result};

/// The grounded operators a step of the EKS solver consumes, plus their
/// ungrounded forms and the diagonal `D = Dg⁻¹`.
#[derive(Debug, Clone)]
pub struct KktOperators {
    /// `L₁ = A diag(b²·θf⁻¹) Aᵀ`, the reweighted Laplacian (n×n).
    pub l1: CsMat<f64>,
    /// `L₂ = L`, the DC Laplacian (n×n).
    pub l2: CsMat<f64>,
    /// `D = Dg⁻¹ = (Q + Θg⁻¹)⁻¹`, positive diagonal, length n.
    pub d: Vec<f64>,
    /// `L₁` grounded at the slack bus, SPD.
    pub l1_grounded: CsMat<f64>,
    /// `L₂` grounded at the slack bus, SPD.
    pub l2_grounded: CsMat<f64>,
    /// `L_eff = L₁ + L₂ D L₂` grounded at the slack bus, SPD. Present only
    /// when requested.
    pub l_eff_grounded: Option<CsMat<f64>>,
    pub map: GroundedIndexMap,
}

/// Assemble the reduced KKT operators from the case factors and the
/// caller-supplied positive interior point diagonals.
///
/// `theta_f_inv` (length m) and `theta_g_inv` (length n) are the barrier
/// weights Θf⁻¹, Θg⁻¹; `q_bus` (length n) is the bus cost diagonal; `r` is the
/// slack bus index.
pub fn assemble_kkt(
    a: &CsMat<f64>,
    b: &[f64],
    l: &CsMat<f64>,
    q_bus: &[f64],
    theta_f_inv: &[f64],
    theta_g_inv: &[f64],
    r: usize,
    want_l_eff: bool,
) -> Result<KktOperators> {
    let n = l.rows();
    let m = b.len();
    check_len("theta_f_inv", theta_f_inv, m)?;
    check_len("theta_g_inv", theta_g_inv, n)?;
    check_len("q_bus", q_bus, n)?;

    // L₁ = A diag(b²·θf⁻¹) Aᵀ — fold the diagonal into the edge weights.
    let w: Vec<f64> = (0..m).map(|k| b[k] * b[k] * theta_f_inv[k]).collect();
    let l1 = build_weighted_laplacian(a, &w);
    let l2 = l.clone();

    // D = Dg⁻¹ = 1 / (q + θg⁻¹), positive.
    let d: Vec<f64> = (0..n).map(|i| 1.0 / (q_bus[i] + theta_g_inv[i])).collect();

    let l1_grounded = ground_at(&l1, r);
    let l2_grounded = ground_at(&l2, r);

    let l_eff_grounded = if want_l_eff {
        // L_eff = L₁ + L diag(d) L.
        let dmat = diagonal(&d);
        let ld = l * &dmat; // L diag(d)
        let ldl = &ld * l; // (L diag(d)) L
        let l_eff = add_csr(&l1, &ldl);
        Some(ground_at(&l_eff, r))
    } else {
        None
    };

    Ok(KktOperators {
        l1,
        l2,
        d,
        l1_grounded,
        l2_grounded,
        l_eff_grounded,
        map: GroundedIndexMap::new(n, r),
    })
}

/// Assemble the full reduced augmented KKT block (eq. "reduced"): a symmetric
/// indefinite saddle matrix in `(Δp_g, Δθ, Δf, Δν, Δη, Δρ)` of size
/// `3n + 2m + 1`. Useful as a single operator for a direct factorization.
pub fn assemble_reduced_kkt(
    a: &CsMat<f64>,
    b: &[f64],
    l: &CsMat<f64>,
    q_bus: &[f64],
    theta_f_inv: &[f64],
    theta_g_inv: &[f64],
    r: usize,
) -> Result<CsMat<f64>> {
    let n = l.rows();
    let m = b.len();
    check_len("theta_f_inv", theta_f_inv, m)?;
    check_len("theta_g_inv", theta_g_inv, n)?;
    check_len("q_bus", q_bus, n)?;

    // Block column / row offsets for (p_g, θ, f, ν, η, ρ).
    let (o_pg, o_th, o_f, o_nu, o_eta, o_rho) = (0, n, 2 * n, 2 * n + m, 3 * n + m, 3 * n + 2 * m);
    let dim = 3 * n + 2 * m + 1;

    let ab = a * &diagonal(b); // AB = A diag(b), n×m
    let flow = build_flow_map(a, b); // BAᵀ, m×n

    let mut k =
        CooBuilder::with_capacity_rect(dim, dim, 4 * l.nnz() + 4 * ab.nnz() + 4 * n + 4 * m);

    // (1,1) Q + Θg⁻¹ and (1,4) −I.
    for i in 0..n {
        k.add(o_pg + i, o_pg + i, q_bus[i] + theta_g_inv[i]);
        k.add(o_pg + i, o_nu + i, -1.0);
        k.add(o_nu + i, o_pg + i, -1.0); // (4,1) −I
    }
    // (2,4) L and its transpose (4,2) L.
    for (&v, (i, j)) in l {
        k.add(o_th + i, o_nu + j, v);
        k.add(o_nu + i, o_th + j, v);
    }
    // (2,5) −AB and transpose (5,2) −BAᵀ.
    for (&v, (i, j)) in &ab {
        k.add(o_th + i, o_eta + j, -v);
    }
    for (&v, (i, j)) in &flow {
        k.add(o_eta + i, o_th + j, -v);
    }
    // (2,6) e_r and (6,2) e_rᵀ.
    k.add(o_th + r, o_rho, 1.0);
    k.add(o_rho, o_th + r, 1.0);
    // (3,3) Θf⁻¹ and (3,5)/(5,3) I.
    for (kk, &tf) in theta_f_inv.iter().enumerate() {
        k.add(o_f + kk, o_f + kk, tf);
        k.add(o_f + kk, o_eta + kk, 1.0);
        k.add(o_eta + kk, o_f + kk, 1.0);
    }

    Ok(k.finish_csr())
}

fn check_len(what: &'static str, v: &[f64], expected: usize) -> Result<()> {
    if v.len() == expected {
        Ok(())
    } else {
        Err(Error::ShapeMismatch {
            what,
            expected,
            got: v.len(),
        })
    }
}

/// Sparse `A + B` via the COO accumulator (independent of sprs `Add`).
fn add_csr(a: &CsMat<f64>, b: &CsMat<f64>) -> CsMat<f64> {
    let mut out = CooBuilder::with_capacity_rect(a.rows(), a.cols(), a.nnz() + b.nnz());
    for (&v, (i, j)) in a {
        out.add(i, j, v);
    }
    for (&v, (i, j)) in b {
        out.add(i, j, v);
    }
    out.finish_csr()
}
