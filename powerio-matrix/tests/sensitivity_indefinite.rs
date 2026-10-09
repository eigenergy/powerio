//! DC sensitivities of networks with negative branch reactance (series
//! capacitors), and the selected row and column builders.
//!
//! A negative reactance gives a negative branch susceptance, which can make
//! the reference grounded DC bus susceptance matrix indefinite. The sparse
//! path then factors it with `LDLᵀ` or LU instead of Cholesky, and its
//! matrices must agree with the dense path's. The selected builders must
//! return exactly the slices of the full matrices.
mod helpers;
use helpers::*;

use powerio_matrix::{
    BalancedNetwork, Branch, Bus, BusId, BusType, Error, IndexedNetwork, SensitivityOptions,
    SensitivitySolver, SensitivitySolverPath, calc_lodf_columns, calc_ptdf_columns,
    calc_ptdf_lodf_with_options, calc_ptdf_rows,
};
use sprs::CsMat;

fn net(buses: &[(usize, BusType)], branches: &[(usize, usize, f64)]) -> BalancedNetwork {
    BalancedNetwork::in_memory(
        "series_capacitors",
        100.0,
        buses
            .iter()
            .map(|&(id, kind)| Bus::new(BusId(id), kind, 345.0))
            .collect(),
        branches
            .iter()
            .map(|&(from, to, x)| Branch::new(BusId(from), BusId(to), 0.0, x))
            .collect(),
    )
}

fn options(solver: SensitivitySolver) -> SensitivityOptions {
    SensitivityOptions {
        solver,
        ..SensitivityOptions::default()
    }
}

fn dense(m: &CsMat<f64>) -> Vec<Vec<f64>> {
    let mut d = vec![vec![0.0; m.cols()]; m.rows()];
    for (&v, (i, j)) in m {
        d[i][j] = v;
    }
    d
}

fn assert_close(left: &CsMat<f64>, right: &CsMat<f64>, tol: f64, label: &str) {
    assert_eq!(left.shape(), right.shape(), "{label}: shape");
    for (i, (l, r)) in dense(left).iter().zip(dense(right)).enumerate() {
        for (j, (a, b)) in l.iter().zip(r).enumerate() {
            assert!((a - b).abs() <= tol, "{label}[{i}][{j}]: {a} vs {b}");
        }
    }
}

/// Rows `rows` of `full`, in order.
fn rows_of(full: &CsMat<f64>, rows: &[usize]) -> CsMat<f64> {
    let d = dense(full);
    let mut out = sprs::TriMat::new((rows.len(), full.cols()));
    for (i, &row) in rows.iter().enumerate() {
        for (j, &v) in d[row].iter().enumerate() {
            if v != 0.0 {
                out.add_triplet(i, j, v);
            }
        }
    }
    out.to_csr()
}

/// Columns `cols` of `full`, in order.
fn cols_of(full: &CsMat<f64>, cols: &[usize]) -> CsMat<f64> {
    let d = dense(full);
    let mut out = sprs::TriMat::new((full.rows(), cols.len()));
    for (j, &col) in cols.iter().enumerate() {
        for (i, row) in d.iter().enumerate() {
            if row[col] != 0.0 {
                out.add_triplet(i, j, row[col]);
            }
        }
    }
    out.to_csr()
}

/// A meshed network with two series capacitors: one feeds bus 6 radially, so
/// that bus's diagonal is negative and the grounded matrix is indefinite, and
/// one compensates the 3-5 chord, which a series line keeps net inductive.
fn series_capacitor_case() -> BalancedNetwork {
    net(
        &[
            (1, BusType::Ref),
            (2, BusType::Pq),
            (3, BusType::Pq),
            (4, BusType::Pq),
            (5, BusType::Pq),
            (6, BusType::Pq),
            (7, BusType::Pq),
        ],
        &[
            (1, 2, 0.1),
            (2, 3, 0.12),
            (3, 4, 0.08),
            (4, 5, 0.1),
            (5, 1, 0.15),
            (2, 6, -0.05),
            (3, 7, -0.04),
            (7, 5, 0.2),
            (6, 4, 0.3),
        ],
    )
}

#[test]
fn an_indefinite_matrix_factors_sparsely_and_matches_the_dense_path() {
    let case = series_capacitor_case();
    let view = IndexedNetwork::new(&case);
    let sparse = calc_ptdf_lodf_with_options(&view, &options(SensitivitySolver::Sparse)).unwrap();
    let dense_path =
        calc_ptdf_lodf_with_options(&view, &options(SensitivitySolver::Dense)).unwrap();
    assert!(
        matches!(
            sparse.metadata.solver_path,
            SensitivitySolverPath::SparseLdlt | SensitivitySolverPath::SparseLu
        ),
        "{:?}",
        sparse.metadata.solver_path
    );
    assert_eq!(
        dense_path.metadata.solver_path,
        SensitivitySolverPath::DenseInverse
    );
    assert_close(&sparse.ptdf, &dense_path.ptdf, 1e-10, "PTDF");
    assert_close(&sparse.lodf, &dense_path.lodf, 1e-10, "LODF");
}

/// Both diagonals of the grounded matrix are zero (`[[0, -1], [-1, 0]]`), so
/// `LDLᵀ` without pivoting meets a zero first pivot whatever the ordering, and
/// LU with partial pivoting is the factorization that solves it.
#[test]
fn a_zero_pivot_falls_through_to_lu() {
    let case = net(
        &[(1, BusType::Ref), (2, BusType::Pq), (3, BusType::Pq)],
        &[(1, 2, -1.0), (1, 3, -1.0), (2, 3, 1.0)],
    );
    let view = IndexedNetwork::new(&case);
    let sparse = calc_ptdf_lodf_with_options(&view, &options(SensitivitySolver::Sparse)).unwrap();
    let dense_path =
        calc_ptdf_lodf_with_options(&view, &options(SensitivitySolver::Dense)).unwrap();
    assert_eq!(sparse.metadata.solver_path, SensitivitySolverPath::SparseLu);
    assert_close(&sparse.ptdf, &dense_path.ptdf, 1e-12, "PTDF");
    assert_close(&sparse.lodf, &dense_path.lodf, 1e-12, "LODF");
}

/// A capacitor that cancels the parallel path exactly leaves a truly singular
/// grounded matrix (`[[0.5, 0.5], [0.5, 0.5]]`): every factorization fails
/// and both paths refuse it. A positive definite network still takes the
/// sparse Cholesky.
#[test]
fn only_a_truly_singular_matrix_is_refused() {
    let singular = net(
        &[(1, BusType::Ref), (2, BusType::Pq), (3, BusType::Pq)],
        &[(1, 2, 1.0), (2, 3, -2.0), (1, 3, 1.0)],
    );
    let view = IndexedNetwork::new(&singular);
    for solver in [SensitivitySolver::Sparse, SensitivitySolver::Dense] {
        let error = calc_ptdf_lodf_with_options(&view, &options(solver)).unwrap_err();
        assert!(
            matches!(error, Error::SingularNetwork),
            "{solver:?}: {error}"
        );
        let error = calc_ptdf_rows(&view, &options(solver), &[0]).unwrap_err();
        assert!(
            matches!(error, Error::SingularNetwork),
            "{solver:?}: {error}"
        );
    }

    let definite = net(
        &[(1, BusType::Ref), (2, BusType::Pq), (3, BusType::Pq)],
        &[(1, 2, 0.1), (2, 3, 0.2), (1, 3, 0.3)],
    );
    let view = IndexedNetwork::new(&definite);
    let sparse = calc_ptdf_lodf_with_options(&view, &options(SensitivitySolver::Sparse)).unwrap();
    assert_eq!(
        sparse.metadata.solver_path,
        SensitivitySolverPath::SparseCholesky
    );
}

/// The selected builders return exactly the slices of the full matrices, on
/// both solver paths, in the order requested, repeats included.
#[test]
fn selected_rows_and_columns_are_slices_of_the_full_matrices() {
    let case118 = parse_file("../tests/data/case118.m", None).unwrap().network;
    for (label, case) in [
        ("case118", case118),
        ("series capacitors", series_capacitor_case()),
    ] {
        let view = IndexedNetwork::new(&case);
        let (n, m) = (view.n(), case.branches().len());
        let branches = [m - 1, 0, m / 2, 3, 0];
        let buses = [n - 1, 0, n / 3, 2];
        for solver in [SensitivitySolver::Sparse, SensitivitySolver::Dense] {
            let full = calc_ptdf_lodf_with_options(&view, &options(solver)).unwrap();
            let label = format!("{label} {solver:?}");
            let rows = calc_ptdf_rows(&view, &options(solver), &branches).unwrap();
            assert_close(&rows, &rows_of(&full.ptdf, &branches), 1e-12, &label);
            let columns = calc_ptdf_columns(&view, &options(solver), &buses).unwrap();
            assert_close(&columns, &cols_of(&full.ptdf, &buses), 1e-12, &label);
            let lodf = calc_lodf_columns(&view, &options(solver), &branches).unwrap();
            assert_close(&lodf, &cols_of(&full.lodf, &branches), 1e-12, &label);
        }
    }
}

#[test]
fn a_selection_outside_the_network_is_refused() {
    let case = series_capacitor_case();
    let view = IndexedNetwork::new(&case);
    let options = options(SensitivitySolver::Sparse);
    let m = case.branches().len();
    for error in [
        calc_ptdf_rows(&view, &options, &[m]).unwrap_err(),
        calc_lodf_columns(&view, &options, &[0, m + 3]).unwrap_err(),
        calc_ptdf_columns(&view, &options, &[view.n()]).unwrap_err(),
    ] {
        assert!(
            matches!(error, Error::InvalidSensitivityOptions { .. }),
            "{error}"
        );
    }
    assert_eq!(
        calc_ptdf_rows(&view, &options, &[]).unwrap().shape(),
        (0, view.n())
    );
}
