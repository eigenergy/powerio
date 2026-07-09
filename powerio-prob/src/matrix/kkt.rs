//! Experimental DC OPF interior point operators.

#![allow(clippy::many_single_char_names, clippy::too_many_arguments)]

use powerio::{Error, Result};
use powerio_matrix::matrix::incidence::diagonal;
use powerio_matrix::matrix::triplet::CooBuilder;
use powerio_matrix::{GroundedIndexMap, SparseMatrix, build_weighted_laplacian, ground_at};

use crate::DcOpfInstance;

use super::build_dc_opf_matrices;

/// Grounded operators for one reduced Newton step.
#[derive(Debug, Clone)]
pub struct KktOperators {
    pub l1: SparseMatrix,
    pub l2: SparseMatrix,
    pub d: Vec<f64>,
    pub l1_grounded: SparseMatrix,
    pub l2_grounded: SparseMatrix,
    pub l_eff_grounded: Option<SparseMatrix>,
    pub map: GroundedIndexMap,
}

/// Assemble reduced Newton operators from a problem instance and barrier data.
///
/// This bus space formulation requires an exact nodal generator reduction and
/// rejects an instance with several generators at one bus.
pub fn assemble_kkt(
    instance: &DcOpfInstance,
    theta_f_inv: &[f64],
    theta_g_inv: &[f64],
    reference_bus: usize,
    want_l_eff: bool,
) -> Result<KktOperators> {
    let matrices = build_dc_opf_matrices(instance);
    let nodal = instance.nodal_generator_data()?;
    let n = instance.n_buses;
    let m = instance.n_branches();
    check_len("theta_f_inv", theta_f_inv, m)?;
    check_len("theta_g_inv", theta_g_inv, n)?;

    let weights: Vec<f64> = (0..m)
        .map(|branch| {
            instance.branches.b[branch] * instance.branches.b[branch] * theta_f_inv[branch]
        })
        .collect();
    let l1 = build_weighted_laplacian(&matrices.incidence, &weights);
    let l2 = matrices.laplacian;
    let d: Vec<f64> = (0..n)
        .map(|bus| 1.0 / (nodal.q[bus] + theta_g_inv[bus]))
        .collect();
    let l1_grounded = ground_at(&l1, reference_bus);
    let l2_grounded = ground_at(&l2, reference_bus);
    let l_eff_grounded = if want_l_eff {
        let ld = &l2 * &diagonal(&d);
        let ldl = &ld * &l2;
        Some(ground_at(&add_csr(&l1, &ldl), reference_bus))
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
        map: GroundedIndexMap::new(n, reference_bus),
    })
}

/// Assemble the reduced augmented KKT block.
///
/// This bus space formulation requires an exact nodal generator reduction and
/// rejects an instance with several generators at one bus.
pub fn assemble_reduced_kkt(
    instance: &DcOpfInstance,
    theta_f_inv: &[f64],
    theta_g_inv: &[f64],
    reference_bus: usize,
) -> Result<SparseMatrix> {
    let matrices = build_dc_opf_matrices(instance);
    let nodal = instance.nodal_generator_data()?;
    let n = instance.n_buses;
    let m = instance.n_branches();
    check_len("theta_f_inv", theta_f_inv, m)?;
    check_len("theta_g_inv", theta_g_inv, n)?;

    let (o_pg, o_th, o_f, o_nu, o_eta, o_rho) = (0, n, 2 * n, 2 * n + m, 3 * n + m, 3 * n + 2 * m);
    let dim = 3 * n + 2 * m + 1;
    let ab = &matrices.incidence * &diagonal(&instance.branches.b);
    let mut k = CooBuilder::with_capacity_rect(
        dim,
        dim,
        4 * matrices.laplacian.nnz() + 4 * ab.nnz() + 4 * n + 4 * m,
    );

    for (bus, &theta_g) in theta_g_inv.iter().enumerate() {
        k.add(o_pg + bus, o_pg + bus, nodal.q[bus] + theta_g);
        k.add(o_pg + bus, o_nu + bus, -1.0);
        k.add(o_nu + bus, o_pg + bus, -1.0);
    }
    for (&value, (row, column)) in &matrices.laplacian {
        k.add(o_th + row, o_nu + column, value);
        k.add(o_nu + row, o_th + column, value);
    }
    for (&value, (row, column)) in &ab {
        k.add(o_th + row, o_eta + column, -value);
    }
    for (&value, (row, column)) in &matrices.flow_map {
        k.add(o_eta + row, o_th + column, -value);
    }
    k.add(o_th + reference_bus, o_rho, 1.0);
    k.add(o_rho, o_th + reference_bus, 1.0);
    for (branch, &weight) in theta_f_inv.iter().enumerate() {
        k.add(o_f + branch, o_f + branch, weight);
        k.add(o_f + branch, o_eta + branch, 1.0);
        k.add(o_eta + branch, o_f + branch, 1.0);
    }

    Ok(k.finish_csr())
}

fn check_len(what: &'static str, values: &[f64], expected: usize) -> Result<()> {
    if values.len() == expected {
        Ok(())
    } else {
        Err(Error::ShapeMismatch {
            what,
            expected,
            got: values.len(),
        })
    }
}

fn add_csr(left: &SparseMatrix, right: &SparseMatrix) -> SparseMatrix {
    let mut out =
        CooBuilder::with_capacity_rect(left.rows(), left.cols(), left.nnz() + right.nnz());
    for (&value, (row, column)) in left {
        out.add(row, column, value);
    }
    for (&value, (row, column)) in right {
        out.add(row, column, value);
    }
    out.finish_csr()
}
