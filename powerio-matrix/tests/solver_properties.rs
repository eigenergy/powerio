//! Regression fixture for solver facing matrix properties on standard cases.

use std::path::PathBuf;

use powerio_matrix::matrix::{BuildOptions, MatrixStats, sddm_check};
use powerio_matrix::{
    IndexedNetwork, MatrixKind, build_kind, ground_at_each, matrix_stats_for_kind,
    parse_matpower_file,
};
use serde::{Deserialize, Serialize};
use sprs::CsMat;

const CASES: &[(&str, &str)] = &[
    ("case9", "case9.m"),
    ("case14", "case14.m"),
    ("case30", "case30.m"),
    ("case57", "case57.m"),
    ("case118", "case118.m"),
];

const MATRICES: &[(MatrixKind, &str)] = &[
    (MatrixKind::BPrime, "bprime"),
    (MatrixKind::BDoublePrime, "bdoubleprime"),
    (MatrixKind::YbusB, "ybus_imag"),
];

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SolverMatrixRecord {
    case: String,
    matrix: String,
    n: usize,
    nnz: usize,
    min_diag: f64,
    m_matrix_sign: bool,
    min_dd_margin: f64,
    condition_estimate: Option<f64>,
    skipped_zero_impedance: usize,
    skipped_zero_impedance_branches: Vec<usize>,
    sddm: bool,
    symmetric: bool,
    max_abs_row_sum: f64,
    full_spd: bool,
    grounded_spd: Option<bool>,
    solver_input: String,
}

#[test]
fn solver_matrix_property_fixture_matches_standard_cases() {
    let actual = solver_records();
    let fixture = fixture_path();

    if std::env::var_os("POWERIO_UPDATE_SOLVER_FIXTURE").is_some() {
        std::fs::create_dir_all(fixture.parent().unwrap()).unwrap();
        let json = serde_json::to_string_pretty(&actual).unwrap();
        std::fs::write(&fixture, format!("{json}\n")).unwrap();
        return;
    }

    let expected: Vec<SolverMatrixRecord> =
        serde_json::from_str(&std::fs::read_to_string(&fixture).unwrap()).unwrap();
    assert_records_close(&actual, &expected);

    for record in &actual {
        if record.matrix == "bprime" {
            assert!(record.m_matrix_sign, "{} B' sign pattern", record.case);
            assert!(
                record.min_dd_margin.abs() < 1e-8,
                "{} B' diagonal dominance margin {}",
                record.case,
                record.min_dd_margin
            );
            assert!(
                record.max_abs_row_sum < 1e-7,
                "{} B' row sum {}",
                record.case,
                record.max_abs_row_sum
            );
            assert_eq!(
                record.grounded_spd,
                Some(true),
                "{} B' grounded SPD",
                record.case
            );
            assert!(
                record.condition_estimate.is_some(),
                "{} B' condition estimate",
                record.case
            );
        }
        if record.matrix == "bdoubleprime" && record.sddm {
            assert!(record.full_spd, "{} B'' SDDM should be SPD", record.case);
        }
    }
}

fn solver_records() -> Vec<SolverMatrixRecord> {
    let opts = BuildOptions::default();
    let mut records = Vec::new();

    for &(case_name, file) in CASES {
        let net = parse_matpower_file(fixture(file)).unwrap();
        let view = IndexedNetwork::new(&net);
        let refs = view.reference_bus_indices();

        for &(kind, matrix_name) in MATRICES {
            let matrix = build_kind(&view, kind, &opts).unwrap();
            let stats = matrix_stats_for_kind(&matrix, &view, kind, &opts);
            let full_spd = is_spd(&matrix);
            let grounded = (kind == MatrixKind::BPrime).then(|| ground_at_each(&matrix, &refs));
            let grounded_spd = grounded.as_ref().map(is_spd);
            let condition_matrix = grounded.as_ref().or_else(|| full_spd.then_some(&matrix));
            let condition_estimate = condition_matrix.and_then(condition_estimate_spd);

            records.push(record(
                case_name,
                matrix_name,
                &matrix,
                &stats,
                full_spd,
                grounded_spd,
                condition_estimate,
            ));
        }
    }

    records
}

fn record(
    case_name: &str,
    matrix_name: &str,
    matrix: &CsMat<f64>,
    stats: &MatrixStats,
    full_spd: bool,
    grounded_spd: Option<bool>,
    condition_estimate: Option<f64>,
) -> SolverMatrixRecord {
    SolverMatrixRecord {
        case: case_name.to_string(),
        matrix: matrix_name.to_string(),
        n: stats.n,
        nnz: stats.nnz,
        min_diag: stats.min_diag,
        m_matrix_sign: stats.m_matrix_sign,
        min_dd_margin: stats.min_dd_margin,
        condition_estimate,
        skipped_zero_impedance: stats.skipped_zero_impedance,
        skipped_zero_impedance_branches: stats.skipped_zero_impedance_branches.clone(),
        sddm: sddm_check(matrix),
        symmetric: is_symmetric(matrix),
        max_abs_row_sum: max_abs_row_sum(matrix),
        full_spd,
        grounded_spd,
        solver_input: solver_input_note(matrix_name, sddm_check(matrix), full_spd, grounded_spd),
    }
}

fn solver_input_note(
    matrix_name: &str,
    sddm: bool,
    full_spd: bool,
    grounded_spd: Option<bool>,
) -> String {
    match matrix_name {
        "bprime" => {
            if grounded_spd == Some(true) {
                "singular Laplacian; reference grounded form is SPD".to_string()
            } else {
                "unexpected: reference grounded form is not SPD".to_string()
            }
        }
        "bdoubleprime" | "ybus_imag" if sddm && full_spd => "full matrix is SPD SDDM".to_string(),
        "bdoubleprime" | "ybus_imag" if sddm => {
            "SDDM sign and dominance hold; full SPD check failed".to_string()
        }
        "bdoubleprime" | "ybus_imag" => "not SDDM under this fixture data".to_string(),
        _ => "not classified".to_string(),
    }
}

fn fixture(name: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("../tests/data");
    p.push(name);
    p
}

fn fixture_path() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/solver_matrix_stats.json");
    p
}

fn assert_records_close(actual: &[SolverMatrixRecord], expected: &[SolverMatrixRecord]) {
    assert_eq!(actual.len(), expected.len());
    for (a, e) in actual.iter().zip(expected) {
        assert_eq!(a.case, e.case);
        assert_eq!(a.matrix, e.matrix);
        assert_eq!(a.n, e.n, "{} {}", a.case, a.matrix);
        assert_eq!(a.nnz, e.nnz, "{} {}", a.case, a.matrix);
        assert_close(
            a.min_diag,
            e.min_diag,
            format!("{} {} min_diag", a.case, a.matrix),
        );
        assert_eq!(a.m_matrix_sign, e.m_matrix_sign, "{} {}", a.case, a.matrix);
        assert_close(
            a.min_dd_margin,
            e.min_dd_margin,
            format!("{} {} min_dd_margin", a.case, a.matrix),
        );
        assert_option_close(
            a.condition_estimate,
            e.condition_estimate,
            format!("{} {} condition_estimate", a.case, a.matrix),
        );
        assert_eq!(
            a.skipped_zero_impedance, e.skipped_zero_impedance,
            "{} {}",
            a.case, a.matrix
        );
        assert_eq!(
            a.skipped_zero_impedance_branches, e.skipped_zero_impedance_branches,
            "{} {}",
            a.case, a.matrix
        );
        assert_eq!(a.sddm, e.sddm, "{} {}", a.case, a.matrix);
        assert_eq!(a.symmetric, e.symmetric, "{} {}", a.case, a.matrix);
        assert_close(
            a.max_abs_row_sum,
            e.max_abs_row_sum,
            format!("{} {} max_abs_row_sum", a.case, a.matrix),
        );
        assert_eq!(a.full_spd, e.full_spd, "{} {}", a.case, a.matrix);
        assert_eq!(a.grounded_spd, e.grounded_spd, "{} {}", a.case, a.matrix);
        assert_eq!(a.solver_input, e.solver_input, "{} {}", a.case, a.matrix);
    }
}

fn assert_close(actual: f64, expected: f64, label: impl std::fmt::Display) {
    let scale = expected.abs().max(1.0);
    let tol = 1e-8 * scale;
    assert!(
        (actual - expected).abs() <= tol,
        "{label}: actual {actual}, expected {expected}, tol {tol}"
    );
}

fn assert_option_close(actual: Option<f64>, expected: Option<f64>, label: impl std::fmt::Display) {
    match (actual, expected) {
        (Some(a), Some(e)) => assert_close(a, e, label),
        (None, None) => {}
        _ => panic!("{label}: actual {actual:?}, expected {expected:?}"),
    }
}

fn dense(matrix: &CsMat<f64>) -> Vec<f64> {
    let n = matrix.rows();
    let mut out = vec![0.0; n * matrix.cols()];
    for (&v, (i, j)) in matrix {
        out[i * matrix.cols() + j] = v;
    }
    out
}

fn is_symmetric(matrix: &CsMat<f64>) -> bool {
    if matrix.rows() != matrix.cols() {
        return false;
    }
    let n = matrix.rows();
    let a = dense(matrix);
    for i in 0..n {
        for j in (i + 1)..n {
            if (a[i * n + j] - a[j * n + i]).abs() > 1e-9 {
                return false;
            }
        }
    }
    true
}

fn max_abs_row_sum(matrix: &CsMat<f64>) -> f64 {
    matrix
        .outer_iterator()
        .map(|row| row.iter().map(|(_, &v)| v).sum::<f64>().abs())
        .fold(0.0, f64::max)
}

fn is_spd(matrix: &CsMat<f64>) -> bool {
    if matrix.rows() != matrix.cols() {
        return false;
    }
    cholesky(&dense(matrix), matrix.rows()).is_some()
}

fn condition_estimate_spd(matrix: &CsMat<f64>) -> Option<f64> {
    if matrix.rows() != matrix.cols() || matrix.rows() == 0 {
        return None;
    }
    let n = matrix.rows();
    let a = dense(matrix);
    let l = cholesky(&a, n)?;
    let lambda_max = power_iteration(&a, n)?;
    let lambda_min = inverse_power_iteration(&a, &l, n)?;
    (lambda_min > 0.0).then_some(lambda_max / lambda_min)
}

fn cholesky(a: &[f64], n: usize) -> Option<Vec<f64>> {
    let mut l = vec![0.0; n * n];
    for i in 0..n {
        for j in 0..=i {
            let mut sum = a[i * n + j];
            for k in 0..j {
                sum -= l[i * n + k] * l[j * n + k];
            }
            if i == j {
                if sum <= 1e-10 {
                    return None;
                }
                l[i * n + j] = sum.sqrt();
            } else {
                l[i * n + j] = sum / l[j * n + j];
            }
        }
    }
    Some(l)
}

fn power_iteration(a: &[f64], n: usize) -> Option<f64> {
    let mut x = vec![1.0 / (n as f64).sqrt(); n];
    for _ in 0..80 {
        let y = mat_vec(a, n, &x);
        let norm = norm(&y);
        if norm == 0.0 {
            return None;
        }
        for (xi, yi) in x.iter_mut().zip(y) {
            *xi = yi / norm;
        }
    }
    Some(rayleigh(a, n, &x))
}

fn inverse_power_iteration(a: &[f64], l: &[f64], n: usize) -> Option<f64> {
    let mut x = vec![1.0 / (n as f64).sqrt(); n];
    for _ in 0..80 {
        let y = chol_solve(l, n, &x);
        let norm = norm(&y);
        if norm == 0.0 {
            return None;
        }
        for (xi, yi) in x.iter_mut().zip(y) {
            *xi = yi / norm;
        }
    }
    Some(rayleigh(a, n, &x))
}

fn chol_solve(l: &[f64], n: usize, b: &[f64]) -> Vec<f64> {
    let mut y = vec![0.0; n];
    for i in 0..n {
        let mut sum = b[i];
        for k in 0..i {
            sum -= l[i * n + k] * y[k];
        }
        y[i] = sum / l[i * n + i];
    }

    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        let mut sum = y[i];
        for k in (i + 1)..n {
            sum -= l[k * n + i] * x[k];
        }
        x[i] = sum / l[i * n + i];
    }
    x
}

fn mat_vec(a: &[f64], n: usize, x: &[f64]) -> Vec<f64> {
    let mut y = vec![0.0; n];
    for i in 0..n {
        for j in 0..n {
            y[i] += a[i * n + j] * x[j];
        }
    }
    y
}

fn rayleigh(a: &[f64], n: usize, x: &[f64]) -> f64 {
    let y = mat_vec(a, n, x);
    dot(x, &y) / dot(x, x)
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn norm(x: &[f64]) -> f64 {
    dot(x, x).sqrt()
}
