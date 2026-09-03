//! DC OPF matrix forge: incidence, Laplacian, OPF instance, and the export
//! bundle. Run against vendored MATPOWER cases.
//!
//! These cases pin `b = 1/x`, so they name `DcConvention::ReactanceOnly`.
//! `SeriesImpedance` gives a different weight for any branch that carries
//! resistance.
mod helpers;
#[allow(unused_imports)]
use helpers::*;

use powerio_matrix::IndexedNetwork;
use powerio_matrix::io::{read_mtx, write_sensitivity_mtx_with_options};
use powerio_matrix::{
    BalancedNetwork, Branch, BuildOptions, Bus, BusId, BusType, DcConvention, Error, GenCost,
    Generator, Scheme, build_adjacency, build_bprime, build_flow_map, build_incidence, build_lodf,
    build_ptdf, build_weighted_laplacian, build_ybus, ground_at,
};
use powerio_matrix::{
    SensitivityOptions, SensitivitySolver, SensitivitySolverPath, build_ptdf_lodf,
    build_ptdf_lodf_with_options,
};
use sprs::CsMat;

const CASES: &[&str] = &[
    "../tests/data/case9.m",
    "../tests/data/case14.m",
    "../tests/data/case30.m",
    "../tests/data/case57.m",
    "../tests/data/case118.m",
];

fn load(path: &str) -> BalancedNetwork {
    parse_matpower_file(path).unwrap_or_else(|e| panic!("parse {path}: {e}"))
}

/// In-memory network from hand-built buses/branches (no loads/shunts/source).
fn net(name: &str, buses: Vec<Bus>, branches: Vec<Branch>) -> BalancedNetwork {
    BalancedNetwork::in_memory(name, 100.0, buses, branches)
}

fn net_with_gens(
    name: &str,
    buses: Vec<Bus>,
    branches: Vec<Branch>,
    generators: Vec<Generator>,
) -> BalancedNetwork {
    let mut network = net(name, buses, branches);
    *network.generators_mut() = generators;
    network
}

fn dense(m: &CsMat<f64>) -> Vec<Vec<f64>> {
    let mut d = vec![vec![0.0; m.cols()]; m.rows()];
    for (&v, (i, j)) in m {
        d[i][j] = v;
    }
    d
}

fn assert_matrix_close(left: &CsMat<f64>, right: &CsMat<f64>, tol: f64, label: &str) {
    assert_eq!(left.rows(), right.rows(), "{label}: row count");
    assert_eq!(left.cols(), right.cols(), "{label}: col count");
    let dl = dense(left);
    let dr = dense(right);
    for i in 0..left.rows() {
        for j in 0..left.cols() {
            let err = (dl[i][j] - dr[i][j]).abs();
            assert!(
                err <= tol,
                "{label}[{i},{j}] differs by {err}: {} vs {}",
                dl[i][j],
                dr[i][j]
            );
        }
    }
}

/// Positive definiteness via dense Cholesky; small matrices only.
// k indexes l[i][k] and l[j][k] in the same inner product; an iterator rewrite
// would only obscure the Cholesky recurrence.
#[allow(clippy::needless_range_loop)]
fn is_spd(a: &[Vec<f64>]) -> bool {
    let n = a.len();
    let mut l = vec![vec![0.0_f64; n]; n];
    for i in 0..n {
        for j in 0..=i {
            let mut s = a[i][j];
            for k in 0..j {
                s -= l[i][k] * l[j][k];
            }
            if i == j {
                if s <= 1e-10 {
                    return false;
                }
                l[i][j] = s.sqrt();
            } else {
                l[i][j] = s / l[j][j];
            }
        }
    }
    true
}

#[test]
fn parses_generators_and_costs() {
    let case = load("../tests/data/case9.m");
    assert_eq!(case.generators().len(), 3);
    let quads: Vec<(f64, f64)> = case
        .generators()
        .iter()
        .map(|g| g.cost.as_ref().unwrap().quadratic().unwrap())
        .collect();
    // MATPOWER c2 p² + c1 p + c0 → (q = 2 c2, c = c1), native units.
    let expected = [(0.22, 5.0), (0.17, 1.2), (0.245, 1.0)];
    for ((q, c), (eq, ec)) in quads.iter().zip(expected) {
        assert!((q - eq).abs() < 1e-9, "q {q} != {eq}");
        assert!((c - ec).abs() < 1e-9, "c {c} != {ec}");
    }
}

#[test]
fn laplacian_equals_bprime_xb() {
    // With zero phase shifts, L = A diag(1/x) Aᵀ matches Bp in the XB scheme.
    for path in CASES {
        let case = load(path);
        let view = IndexedNetwork::new(&case);
        let inc =
            build_incidence(&view, DcConvention::ReactanceOnly, &BuildOptions::default()).unwrap();
        let l = build_weighted_laplacian(&inc.a, &inc.b);
        let bp = build_bprime(
            &view,
            &powerio_matrix::BuildOptions {
                scheme: Scheme::Xb,
                ..Default::default()
            },
        )
        .unwrap();
        let (dl, db) = (dense(&l), dense(&bp));
        assert_eq!(dl.len(), db.len(), "{path}: size");
        for i in 0..dl.len() {
            for j in 0..dl.len() {
                assert!(
                    (dl[i][j] - db[i][j]).abs() < 1e-9,
                    "{path}: L[{i}][{j}]={} != Bp[{i}][{j}]={}",
                    dl[i][j],
                    db[i][j]
                );
            }
        }
    }
}

#[test]
fn incidence_structure() {
    for path in CASES {
        let case = load(path);
        let view = IndexedNetwork::new(&case);
        let inc =
            build_incidence(&view, DcConvention::ReactanceOnly, &BuildOptions::default()).unwrap();
        let (n, m) = (inc.n(), inc.m());
        assert_eq!(inc.a.rows(), n);
        assert_eq!(inc.a.cols(), m);
        assert_eq!(inc.a.nnz(), 2 * m, "{path}: two nonzeros per column");
        assert_eq!(inc.b.len(), m);
        assert_eq!(inc.branch_of_col.len(), m);

        // Each column sums to 0 with one +1 and one −1.
        let mut col_sum = vec![0.0; m];
        let mut col_cnt = vec![0usize; m];
        for (&v, (_, j)) in &inc.a {
            col_sum[j] += v;
            col_cnt[j] += 1;
            assert!((v.abs() - 1.0).abs() < 1e-12, "{path}: |A entry| != 1");
        }
        for j in 0..m {
            assert_eq!(col_cnt[j], 2, "{path}: column {j} degree");
            assert!(col_sum[j].abs() < 1e-12, "{path}: column {j} sum");
        }
    }
}

#[test]
// i/j index d[i][j] and d[j][i] for the symmetry check; the index pair is the point.
#[allow(clippy::needless_range_loop)]
fn laplacian_is_psd_with_constant_kernel() {
    for path in CASES {
        let case = load(path);
        let view = IndexedNetwork::new(&case);
        let inc =
            build_incidence(&view, DcConvention::ReactanceOnly, &BuildOptions::default()).unwrap();
        let l = build_weighted_laplacian(&inc.a, &inc.b);
        let d = dense(&l);
        let n = d.len();
        // Symmetric, and every row sums to ~0 (L·1 = 0).
        for i in 0..n {
            let row_sum: f64 = d[i].iter().sum();
            assert!(row_sum.abs() < 1e-7, "{path}: row {i} sum {row_sum}");
            for j in 0..n {
                assert!((d[i][j] - d[j][i]).abs() < 1e-12, "{path}: asymmetry");
            }
        }
    }
}

#[test]
fn grounded_laplacian_is_spd() {
    for path in CASES {
        let case = load(path);
        let view = IndexedNetwork::new(&case);
        let r = view.reference_bus_index().unwrap();
        let inc =
            build_incidence(&view, DcConvention::ReactanceOnly, &BuildOptions::default()).unwrap();
        let l = build_weighted_laplacian(&inc.a, &inc.b);
        let lg = ground_at(&l, r);
        assert_eq!(lg.rows(), view.n() - 1);
        assert_eq!(lg.cols(), view.n() - 1);
        assert!(is_spd(&dense(&lg)), "{path}: grounded L not SPD");
    }
}

#[test]
fn flow_map_reconstructs_laplacian() {
    for path in CASES {
        let case = load(path);
        let view = IndexedNetwork::new(&case);
        let inc =
            build_incidence(&view, DcConvention::ReactanceOnly, &BuildOptions::default()).unwrap();
        let flow = build_flow_map(&inc.a, &inc.b); // B Aᵀ, m×n
        assert_eq!(flow.rows(), inc.m());
        assert_eq!(flow.cols(), inc.n());
        // A · (B Aᵀ) == L.
        let l_from_flow = &inc.a * &flow;
        let l = build_weighted_laplacian(&inc.a, &inc.b);
        let (df, dl) = (dense(&l_from_flow), dense(&l));
        for i in 0..dl.len() {
            for j in 0..dl.len() {
                assert!((df[i][j] - dl[i][j]).abs() < 1e-9, "{path}: flow≠L");
            }
        }
        // Each row of B Aᵀ sums to 0.
        let dflow = dense(&flow);
        for (k, row) in dflow.iter().enumerate() {
            let s: f64 = row.iter().sum();
            assert!(s.abs() < 1e-9, "{path}: BAᵀ row {k} sum {s}");
        }
    }
}

#[test]
fn reference_bus_count_errors() {
    // Two reference buses.
    let two = net(
        "two_ref",
        vec![bus(1, BusType::Ref), bus(2, BusType::Ref)],
        vec![],
    );
    assert!(matches!(
        IndexedNetwork::new(&two).reference_bus_index(),
        Err(powerio_tx::Error::ReferenceBusCount { found: 2, .. })
    ));
    // Zero reference buses.
    let zero = net("no_ref", vec![bus(1, BusType::Pq)], vec![]);
    assert!(matches!(
        IndexedNetwork::new(&zero).reference_bus_index(),
        Err(powerio_tx::Error::ReferenceBusCount { found: 0, .. })
    ));
}

#[test]
// i/j index d[i][j] and d[j][i] for symmetry; adjacency entries are exact 0/1.
#[allow(clippy::needless_range_loop, clippy::float_cmp)]
fn adjacency_is_symmetric_01() {
    for path in CASES {
        let case = load(path);
        let view = IndexedNetwork::new(&case);
        let a = build_adjacency(&view).unwrap();
        assert_eq!(a.rows(), view.n());
        assert_eq!(a.cols(), view.n());
        let d = dense(&a);
        for i in 0..d.len() {
            assert!((d[i][i]).abs() < 1e-12, "{path}: nonzero diagonal");
            for j in 0..d.len() {
                assert!(d[i][j] == 0.0 || d[i][j] == 1.0, "{path}: entry not 0/1");
                assert!((d[i][j] - d[j][i]).abs() < 1e-12, "{path}: not symmetric");
            }
        }
    }
}

#[test]
// i/k/l index entries of (A·PTDF) and PTDF; the indices are the assertion.
#[allow(clippy::needless_range_loop)]
fn ptdf_satisfies_kcl() {
    // A · PTDF = I − e_r·1ᵀ: nodal balance for every injection.
    for path in CASES {
        let case = load(path);
        let view = IndexedNetwork::new(&case);
        let r = view.reference_bus_index().unwrap();
        let inc =
            build_incidence(&view, DcConvention::ReactanceOnly, &BuildOptions::default()).unwrap();
        let ptdf = build_ptdf(&view, DcConvention::ReactanceOnly).unwrap();
        assert_eq!(ptdf.rows(), inc.m());
        assert_eq!(ptdf.cols(), view.n());
        let m = dense(&(&inc.a * &ptdf)); // n × n
        let n = view.n();
        for i in 0..n {
            for k in 0..n {
                let expected = f64::from(i == k) - f64::from(i == r);
                assert!(
                    (m[i][k] - expected).abs() < 1e-6,
                    "{path}: (A·PTDF)[{i}][{k}]={} != {expected}",
                    m[i][k]
                );
            }
        }
        // Reference column is zero.
        let dptdf = dense(&ptdf);
        for l in 0..inc.m() {
            assert!(dptdf[l][r].abs() < 1e-12, "{path}: PTDF slack col nonzero");
        }
    }
}

#[test]
// k/l index LODF entries d[k][k] and d[l][k]; the indices are the assertion.
#[allow(clippy::needless_range_loop)]
fn lodf_diagonal_is_minus_one() {
    for path in CASES {
        let case = load(path);
        let view = IndexedNetwork::new(&case);
        let lodf = build_lodf(&view, DcConvention::ReactanceOnly).unwrap();
        let inc =
            build_incidence(&view, DcConvention::ReactanceOnly, &BuildOptions::default()).unwrap();
        assert_eq!(lodf.rows(), inc.m());
        assert_eq!(lodf.cols(), inc.m());
        let d = dense(&lodf);
        for k in 0..inc.m() {
            assert!((d[k][k] + 1.0).abs() < 1e-9, "{path}: LODF[{k}][{k}] != -1");
            for l in 0..inc.m() {
                assert!(d[l][k].is_finite(), "{path}: LODF not finite");
            }
        }
    }
}

#[test]
fn sparse_sensitivities_match_dense_oracle() {
    for path in [
        "../tests/data/case9.m",
        "../tests/data/case14.m",
        "../tests/data/case30.m",
    ] {
        let case = load(path);
        let view = IndexedNetwork::new(&case);
        // Both paths take the same convention: this compares the two solvers,
        // not two weightings.
        let (dense_ptdf, dense_lodf) = build_ptdf_lodf(&view, DcConvention::default()).unwrap();
        let sparse = build_ptdf_lodf_with_options(
            &view,
            &SensitivityOptions {
                solver: SensitivitySolver::Sparse,
                drop_tolerance: 1e-12,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            sparse.metadata.solver_path,
            SensitivitySolverPath::SparseCholesky,
            "{path}: solver path"
        );
        assert_matrix_close(&sparse.ptdf, &dense_ptdf, 1e-7, path);
        assert_matrix_close(&sparse.lodf, &dense_lodf, 1e-7, path);
    }
}

#[test]
fn sensitivity_drop_tolerance_records_dropped_entries() {
    let case = load("../tests/data/case30.m");
    let view = IndexedNetwork::new(&case);
    let full = build_ptdf_lodf_with_options(
        &view,
        &SensitivityOptions {
            solver: SensitivitySolver::Dense,
            drop_tolerance: 0.0,
            ..Default::default()
        },
    )
    .unwrap();
    let pruned = build_ptdf_lodf_with_options(
        &view,
        &SensitivityOptions {
            solver: SensitivitySolver::Dense,
            drop_tolerance: 0.2,
            ..Default::default()
        },
    )
    .unwrap();

    assert!(pruned.metadata.ptdf.dropped_entries > 0);
    assert!(pruned.metadata.lodf.dropped_entries > 0);
    assert!(pruned.ptdf.nnz() < full.ptdf.nnz());
    assert!(pruned.lodf.nnz() < full.lodf.nnz());
    assert_eq!(pruned.metadata.ptdf.rows, pruned.ptdf.rows());
    assert_eq!(pruned.metadata.ptdf.cols, pruned.ptdf.cols());
    assert_eq!(pruned.metadata.lodf.rows, pruned.lodf.rows());
    assert_eq!(pruned.metadata.lodf.cols, pruned.lodf.cols());
}

#[test]
fn a_sensitivity_write_never_replaces_an_existing_entry() {
    let case = load("../tests/data/case30.m");
    let view = IndexedNetwork::new(&case);
    let options = SensitivityOptions::default();

    let residue_free = |dir: &std::path::Path| {
        let tmp: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains("tmp") || name.contains("body") || name.contains("final"))
            .collect();
        assert!(tmp.is_empty(), "staging residue: {tmp:?}");
    };

    // Regular files at both targets: the write refuses and both keep their
    // bytes, with no staging residue beside them.
    let temp = tempfile::tempdir().unwrap();
    let ptdf_path = temp.path().join("ptdf.mtx");
    let lodf_path = temp.path().join("lodf.mtx");
    std::fs::write(&ptdf_path, b"precious ptdf").unwrap();
    std::fs::write(&lodf_path, b"precious lodf").unwrap();
    let error =
        write_sensitivity_mtx_with_options(&view, &options, &ptdf_path, &lodf_path).unwrap_err();
    assert!(error.to_string().contains("already exists"), "{error}");
    assert_eq!(std::fs::read(&ptdf_path).unwrap(), b"precious ptdf");
    assert_eq!(std::fs::read(&lodf_path).unwrap(), b"precious lodf");
    residue_free(temp.path());

    // A symbolic link at one target: the link survives and the designated
    // file keeps its bytes and its length.
    #[cfg(unix)]
    {
        let temp = tempfile::tempdir().unwrap();
        let designated = temp.path().join("designated.mtx");
        std::fs::write(&designated, b"designated").unwrap();
        let ptdf_path = temp.path().join("ptdf.mtx");
        let lodf_path = temp.path().join("lodf.mtx");
        std::os::unix::fs::symlink(&designated, &lodf_path).unwrap();
        let error = write_sensitivity_mtx_with_options(&view, &options, &ptdf_path, &lodf_path)
            .unwrap_err();
        assert!(error.to_string().contains("already exists"), "{error}");
        assert!(
            std::fs::symlink_metadata(&lodf_path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read(&designated).unwrap(), b"designated");
        assert_eq!(std::fs::metadata(&designated).unwrap().len(), 10);
        residue_free(temp.path());
    }

    // Fresh paths still produce both matrices, and both read back; a second
    // write to the same targets refuses, with no staging residue either way.
    let temp = tempfile::tempdir().unwrap();
    let ptdf_path = temp.path().join("ptdf.mtx");
    let lodf_path = temp.path().join("lodf.mtx");
    write_sensitivity_mtx_with_options(&view, &options, &ptdf_path, &lodf_path).unwrap();
    let ptdf_first = std::fs::read(&ptdf_path).unwrap();
    assert!(read_mtx(&ptdf_path).unwrap().nnz() > 0);
    assert!(read_mtx(&lodf_path).unwrap().nnz() > 0);
    residue_free(temp.path());
    let error =
        write_sensitivity_mtx_with_options(&view, &options, &ptdf_path, &lodf_path).unwrap_err();
    assert!(error.to_string().contains("already exists"), "{error}");
    assert_eq!(std::fs::read(&ptdf_path).unwrap(), ptdf_first);
    residue_free(temp.path());
}

#[test]
fn streamed_sparse_sensitivities_match_in_memory() {
    let case = load("../tests/data/case30.m");
    let view = IndexedNetwork::new(&case);
    let options = SensitivityOptions {
        solver: SensitivitySolver::Sparse,
        drop_tolerance: 1e-9,
        ..Default::default()
    };
    let expected = build_ptdf_lodf_with_options(&view, &options).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let ptdf_path = temp.path().join("ptdf.mtx");
    let lodf_path = temp.path().join("lodf.mtx");

    let meta = write_sensitivity_mtx_with_options(&view, &options, &ptdf_path, &lodf_path).unwrap();
    let ptdf = read_mtx(&ptdf_path).unwrap();
    let lodf = read_mtx(&lodf_path).unwrap();

    assert_eq!(meta.solver_path, SensitivitySolverPath::SparseCholesky);
    assert_eq!(meta.ptdf.nnz, ptdf.nnz());
    assert_eq!(meta.lodf.nnz, lodf.nnz());
    assert_matrix_close(&ptdf, &expected.ptdf, 0.0, "streamed PTDF");
    assert_matrix_close(&lodf, &expected.lodf, 0.0, "streamed LODF");
}

#[test]
fn auto_sensitivity_solver_switches_to_sparse_above_threshold() {
    let case = load("../tests/data/case118.m");
    let view = IndexedNetwork::new(&case);
    let out = build_ptdf_lodf_with_options(
        &view,
        &SensitivityOptions {
            solver: SensitivitySolver::Auto,
            auto_dense_threshold: 16,
            drop_tolerance: 1e-9,
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(out.metadata.requested_solver, SensitivitySolver::Auto);
    assert_eq!(
        out.metadata.solver_path,
        SensitivitySolverPath::SparseCholesky
    );
    assert_eq!(out.metadata.ptdf.rows, out.ptdf.rows());
    assert_eq!(out.metadata.ptdf.cols, out.ptdf.cols());
    assert_eq!(out.metadata.lodf.rows, out.lodf.rows());
    assert_eq!(out.metadata.lodf.cols, out.lodf.cols());
    assert!(out.metadata.reduced_dimension > 16);
}

#[test]
fn auto_writes_sparse_outputs_above_the_dense_threshold() {
    let n = 600;
    let mut buses = Vec::with_capacity(n);
    buses.push(bus(1, BusType::Ref));
    for id in 2..=n {
        buses.push(bus(id, BusType::Pq));
    }
    let branches = (2..=n).map(|id| branch(1, id, 0.1)).collect::<Vec<_>>();
    let branch_count = branches.len();
    let case = net("star600", buses, branches);
    let view = IndexedNetwork::new(&case);
    let reduced_dimension = view.n() - view.reference_bus_indices().len();
    assert!(reduced_dimension > 512);

    // The knob is spelled out so the routing this asserts does not depend
    // on the default ceiling's value.
    let options = SensitivityOptions {
        auto_dense_threshold: 512,
        ..SensitivityOptions::default()
    };
    let temp = tempfile::tempdir().unwrap();
    let ptdf_path = temp.path().join("ptdf.mtx");
    let lodf_path = temp.path().join("lodf.mtx");
    let meta = write_sensitivity_mtx_with_options(&view, &options, &ptdf_path, &lodf_path).unwrap();
    let ptdf = read_mtx(&ptdf_path).unwrap();
    let lodf = read_mtx(&lodf_path).unwrap();

    assert_eq!(meta.solver_path, SensitivitySolverPath::SparseCholesky);
    assert_eq!(meta.reduced_dimension, reduced_dimension);
    assert_eq!(ptdf.rows(), branch_count);
    assert_eq!(ptdf.cols(), n);
    assert_eq!(lodf.rows(), branch_count);
    assert_eq!(lodf.cols(), branch_count);
    assert_eq!(meta.ptdf.nnz, ptdf.nnz());
    assert_eq!(meta.lodf.nnz, lodf.nnz());
    assert!(meta.ptdf.nnz <= branch_count);
    assert_eq!(meta.lodf.nnz, branch_count);
}

#[test]
fn auto_sparse_rejects_non_positive_susceptance() {
    let case = net(
        "negative_x",
        vec![bus(1, BusType::Ref), bus(2, BusType::Pq)],
        vec![branch(1, 2, -0.1)],
    );
    let view = IndexedNetwork::new(&case);
    let err = build_ptdf_lodf_with_options(
        &view,
        &SensitivityOptions {
            solver: SensitivitySolver::Auto,
            auto_dense_threshold: 0,
            ..Default::default()
        },
    )
    .unwrap_err();

    match err {
        Error::InvalidSensitivityOptions { reason } => {
            assert!(reason.contains("positive finite branch susceptances"));
        }
        other => panic!("unexpected error: {other}"),
    }

    let dense = build_ptdf_lodf_with_options(
        &view,
        &SensitivityOptions {
            solver: SensitivitySolver::Dense,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        dense.metadata.solver_path,
        SensitivitySolverPath::DenseInverse
    );
}

// ---------------------------------------------------------------------------
// Helpers for hand-built synthetic cases (no fixture has a phase shifter, a
// disconnected topology, or two generators on one bus).
// ---------------------------------------------------------------------------

fn bus(id: usize, kind: BusType) -> Bus {
    Bus::new(BusId(id), kind, 345.0)
}

fn branch(from: usize, to: usize, x: f64) -> Branch {
    branch_xts(from, to, x, 0.0, 0.0)
}

fn branch_xts(from: usize, to: usize, x: f64, tap: f64, shift: f64) -> Branch {
    let mut branch = Branch::new(BusId(from), BusId(to), 0.0, x);
    branch.tap = tap;
    branch.shift = shift;
    branch
}

/// Generator on `bus_id` with the given cost curve (pmax = 100 MW).
fn gen_with_cost(bus: usize, cost: Option<GenCost>) -> Generator {
    let mut generator = Generator::new(BusId(bus));
    generator.mbase = 100.0;
    generator.pmax = 100.0;
    generator.cost = cost;
    generator
}

/// Polynomial (model 2, quadratic) generator: cost `c2 p² + c1 p`.
fn poly_gen(bus_id: usize, pmax: f64, c2: f64, c1: f64) -> Generator {
    let cost = GenCost::new(2, 0.0, 0.0, vec![c2, c1, 0.0]);
    let mut generator = gen_with_cost(bus_id, Some(cost));
    generator.pmax = pmax;
    generator
}

/// Symmetric 3-bus triangle, slack at bus 1, unit susceptance on every branch.
/// Branch order fixes the incidence columns: e0=1→2, e1=1→3, e2=2→3.
fn triangle() -> BalancedNetwork {
    net(
        "triangle",
        vec![
            bus(1, BusType::Ref),
            bus(2, BusType::Pq),
            bus(3, BusType::Pq),
        ],
        vec![branch(1, 2, 1.0), branch(1, 3, 1.0), branch(2, 3, 1.0)],
    )
}

// ---------------------------------------------------------------------------
// Reference-pinned numerical checks. The invariant tests above are satisfied
// by a whole family of wrong matrices; these pin actual values so a sign,
// scale, or index regression in the DC core is caught.
// ---------------------------------------------------------------------------

#[test]
fn ptdf_matches_analytic_triangle() {
    // Hand-derived for the unit triangle, slack = bus 1 (column 0).
    // Inject at bus j, withdraw at slack; read the flow on each branch.
    let case = triangle();
    let view = IndexedNetwork::new(&case);
    let ptdf = dense(&build_ptdf(&view, DcConvention::ReactanceOnly).unwrap());
    let expected = [
        [0.0, -2.0 / 3.0, -1.0 / 3.0], // e0: 1→2
        [0.0, -1.0 / 3.0, -2.0 / 3.0], // e1: 1→3
        [0.0, 1.0 / 3.0, -1.0 / 3.0],  // e2: 2→3
    ];
    for (e, row) in expected.iter().enumerate() {
        for (b, &want) in row.iter().enumerate() {
            assert!(
                (ptdf[e][b] - want).abs() < 1e-9,
                "PTDF[{e}][{b}]={} != {want}",
                ptdf[e][b]
            );
        }
    }
}

#[test]
fn lodf_matches_analytic_triangle() {
    // Column k = outage of branch k; row l = the flow it pushes onto branch l.
    // Tripping any one edge of the triangle reroutes its flow around the other
    // two, giving ±1 entries.
    let case = triangle();
    let view = IndexedNetwork::new(&case);
    let lodf = dense(&build_lodf(&view, DcConvention::ReactanceOnly).unwrap());
    let expected = [[-1.0, 1.0, -1.0], [1.0, -1.0, 1.0], [-1.0, 1.0, -1.0]];
    for (l, row) in expected.iter().enumerate() {
        for (k, &want) in row.iter().enumerate() {
            assert!(
                (lodf[l][k] - want).abs() < 1e-9,
                "LODF[{l}][{k}]={} != {want}",
                lodf[l][k]
            );
        }
    }
}

#[test]
fn matpower_convention_tap_and_shift() {
    let (x, tap, shift_deg) = (0.2, 1.25, 10.0);
    let case = net(
        "shifter",
        vec![bus(1, BusType::Ref), bus(2, BusType::Pq)],
        vec![branch_xts(1, 2, x, tap, shift_deg)],
    );

    let view = IndexedNetwork::new(&case);

    // ReactanceOnly ignores tap and shift: b = 1/x, no phase injection.
    let pp = build_incidence(&view, DcConvention::ReactanceOnly, &BuildOptions::default()).unwrap();
    assert!((pp.b[0] - 1.0 / x).abs() < 1e-12);
    assert!(pp.p_shift.iter().all(|&v| v == 0.0));

    // Matpower: b = 1/(x·τ); makeBdc injection ±b·shift at from/to.
    let mp = build_incidence(
        &view,
        DcConvention::TapAdjustedReactance,
        &BuildOptions::default(),
    )
    .unwrap();
    let b_e = 1.0 / (x * tap);
    let shift_rad = shift_deg.to_radians();
    assert!((mp.b[0] - b_e).abs() < 1e-12, "b_e {} != {b_e}", mp.b[0]);
    assert!((mp.p_shift[0] - (-b_e * shift_rad)).abs() < 1e-12);
    assert!((mp.p_shift[1] - (b_e * shift_rad)).abs() < 1e-12);
}

#[test]
// l/k index lodf[l][k] against the expected −1 diagonal; the indices are the assertion.
#[allow(clippy::needless_range_loop)]
fn radial_lodf_is_negative_identity() {
    // Path 1-2-3: every branch is a bridge, so each outage islands the network
    // and the LODF column zeroes out except the −1 diagonal.
    let case = net(
        "path",
        vec![
            bus(1, BusType::Ref),
            bus(2, BusType::Pq),
            bus(3, BusType::Pq),
        ],
        vec![branch(1, 2, 0.1), branch(2, 3, 0.1)],
    );
    let view = IndexedNetwork::new(&case);
    let lodf = dense(&build_lodf(&view, DcConvention::ReactanceOnly).unwrap());
    for l in 0..2 {
        for k in 0..2 {
            let want = if l == k { -1.0 } else { 0.0 };
            assert!(
                (lodf[l][k] - want).abs() < 1e-9,
                "LODF[{l}][{k}]={} != {want}",
                lodf[l][k]
            );
        }
    }
}

#[test]
fn ptdf_handles_indefinite_but_invertible_laplacian() {
    let case = net(
        "negative-x",
        vec![bus(1, BusType::Ref), bus(2, BusType::Pq)],
        vec![branch(1, 2, -1.0)],
    );
    let view = IndexedNetwork::new(&case);

    let ptdf = dense(&build_ptdf(&view, DcConvention::ReactanceOnly).unwrap());
    let lodf = dense(&build_lodf(&view, DcConvention::ReactanceOnly).unwrap());

    assert_eq!(ptdf.len(), 1);
    assert!(ptdf[0][0].abs() < 1e-12);
    assert!((ptdf[0][1] + 1.0).abs() < 1e-12);
    assert_eq!(lodf, vec![vec![-1.0]]);
}

#[test]
fn ungrounded_island_errors() {
    // Two islands (1-2 and 3-4), but only island 1-2 carries a reference: the
    // 3-4 island has no slack to ground, so its all-ones null vector survives.
    let case = net(
        "ungrounded",
        vec![
            bus(1, BusType::Ref),
            bus(2, BusType::Pq),
            bus(3, BusType::Pq),
            bus(4, BusType::Pq),
        ],
        vec![branch(1, 2, 0.1), branch(3, 4, 0.1)],
    );
    let view = IndexedNetwork::new(&case);
    assert_eq!(view.n_connected_components(), 2);
    let p = build_ptdf(&view, DcConvention::ReactanceOnly).unwrap_err();
    assert!(
        matches!(
            p,
            Error::Core(powerio_tx::Error::UngroundedComponent { components: 1 })
        ),
        "ptdf: {p:?}"
    );
    let l = build_lodf(&view, DcConvention::ReactanceOnly).unwrap_err();
    assert!(
        matches!(
            l,
            Error::Core(powerio_tx::Error::UngroundedComponent { components: 1 })
        ),
        "lodf: {l:?}"
    );
}

#[test]
fn two_grounded_islands_solve_block_diagonal() {
    // Two islands (1-2 and 3-4), each with its own reference bus. Grounding one
    // slack per island makes the Laplacian invertible, and the PTDF is block
    // diagonal: an injection in one island moves no flow in the other.
    let case = net(
        "grounded-islands",
        vec![
            bus(1, BusType::Ref),
            bus(2, BusType::Pq),
            bus(3, BusType::Ref),
            bus(4, BusType::Pq),
        ],
        vec![branch(1, 2, 0.1), branch(3, 4, 0.1)],
    );
    let view = IndexedNetwork::new(&case);
    assert_eq!(view.reference_bus_indices(), vec![0, 2]);
    let ptdf = dense(&build_ptdf(&view, DcConvention::ReactanceOnly).unwrap());
    // Branch 0 is in island {0,1}; its only nonzero sensitivity is to that
    // island's non-slack bus (col 1). Branch 1 is in island {2,3} → col 3.
    // Both reference columns (0 and 2) are zero. The sign is −1: a unit
    // injection at the branch's "to"-side bus returns against its 1→2
    // orientation toward the slack (matches the analytic-triangle convention).
    for (l, row) in ptdf.iter().enumerate() {
        assert!(row[0].abs() < 1e-12, "ref col 0 nonzero on branch {l}");
        assert!(row[2].abs() < 1e-12, "ref col 2 nonzero on branch {l}");
    }
    assert!(
        (ptdf[0][1] + 1.0).abs() < 1e-9,
        "branch0 vs bus1: {}",
        ptdf[0][1]
    );
    assert!(ptdf[0][3].abs() < 1e-12, "branch0 leaked into island 2");
    assert!(
        (ptdf[1][3] + 1.0).abs() < 1e-9,
        "branch1 vs bus3: {}",
        ptdf[1][3]
    );
    assert!(ptdf[1][1].abs() < 1e-12, "branch1 leaked into island 1");
}

#[test]
fn multi_reference_two_refs_one_island() {
    // One connected island, two reference buses: grounding both fixes both
    // reference angles to zero. Both reference columns are zero, and a unit
    // injection at the middle bus splits its return between the two references by
    // electrical distance: symmetric here (equal reactances), so each branch
    // carries half.
    let case = net(
        "multi-reference",
        vec![
            bus(1, BusType::Ref),
            bus(2, BusType::Pq),
            bus(3, BusType::Ref),
        ],
        vec![branch(1, 2, 0.1), branch(2, 3, 0.1)],
    );
    let view = IndexedNetwork::new(&case);
    assert_eq!(view.reference_bus_indices(), vec![0, 2]);
    let ptdf = dense(&build_ptdf(&view, DcConvention::ReactanceOnly).unwrap());
    // Both reference columns (0 and 2) are zero; the middle bus (col 1) splits.
    for (l, row) in ptdf.iter().enumerate() {
        assert!(row[0].abs() < 1e-12, "ref col 0 nonzero on branch {l}");
        assert!(row[2].abs() < 1e-12, "ref col 2 nonzero on branch {l}");
    }
    // An injection at bus 2 returns half to each reference: branch 0 (1→2) carries
    // −1/2 (back toward slack 1, against its orientation); branch 1 (2→3)
    // carries +1/2 (out toward slack 3, with its orientation).
    assert!(
        (ptdf[0][1] + 0.5).abs() < 1e-9,
        "branch0 split: {}",
        ptdf[0][1]
    );
    assert!(
        (ptdf[1][1] - 0.5).abs() < 1e-9,
        "branch1 split: {}",
        ptdf[1][1]
    );
}

#[test]
fn lodf_two_refs_multi_reference_triangle() {
    // The unit triangle with buses 1 and 3 as references.
    // LODF differs from the single reference triangle because two voltage angles
    // are fixed: tripping branch 1-3 (between the two references) redistributes
    // nothing, while tripping 1-2 or 2-3 reroutes bus 2's flow fully onto the
    // other reference-connected branch. Hand-derived against the reduced 1x1
    // system (only bus 2 survives grounding, diag = 2, so PTDF col for bus 2 is
    // [-1/2, 0, +1/2]). This pins the multi-grounded ptdf_dense -> build_lodf path.
    let case = net(
        "triangle-2ref",
        vec![
            bus(1, BusType::Ref),
            bus(2, BusType::Pq),
            bus(3, BusType::Ref),
        ],
        vec![branch(1, 2, 1.0), branch(1, 3, 1.0), branch(2, 3, 1.0)],
    );
    let view = IndexedNetwork::new(&case);
    assert_eq!(view.reference_bus_indices(), vec![0, 2]);
    let lodf = dense(&build_lodf(&view, DcConvention::ReactanceOnly).unwrap());
    let expected = [[-1.0, 0.0, -1.0], [0.0, -1.0, 0.0], [-1.0, 0.0, -1.0]];
    for (l, row) in expected.iter().enumerate() {
        for (k, &want) in row.iter().enumerate() {
            assert!(
                (lodf[l][k] - want).abs() < 1e-9,
                "LODF[{l}][{k}]={} != {want}",
                lodf[l][k]
            );
        }
    }
}

#[test]
fn ybus_shift_invariant_to_normalization() {
    // A 30-degree phase shifter: shift is in degrees on the raw network and in
    // radians on its normalized form. Y_bus must be identical: branch_admittance
    // takes the shift via angle_radians, converting degrees->rad for the raw case
    // and leaving the already-radian normalized case alone (no double conversion).
    let raw = net_with_gens(
        "shifter",
        vec![bus(1, BusType::Ref), bus(2, BusType::Pq)],
        vec![branch_xts(1, 2, 0.1, 1.0, 30.0)],
        vec![poly_gen(1, 100.0, 0.0, 1.0)],
    );
    let norm = raw.to_normalized().unwrap();
    let opts = BuildOptions::default();
    let yr = build_ybus(&IndexedNetwork::new(&raw), &opts).unwrap();
    let yn = build_ybus(&IndexedNetwork::new(&norm), &opts).unwrap();
    let (gr, gn) = (yr.g.to_dense(), yn.g.to_dense());
    let (br, bn) = (yr.b.to_dense(), yn.b.to_dense());
    for i in 0..2 {
        for j in 0..2 {
            assert!(
                (gr[[i, j]] - gn[[i, j]]).abs() < 1e-12,
                "G[{i},{j}] differs"
            );
            assert!(
                (br[[i, j]] - bn[[i, j]]).abs() < 1e-12,
                "B[{i},{j}] differs"
            );
        }
    }
    // The shift makes Y_bus non-symmetric, so a dropped or doubled conversion
    // would change these off-diagonals and the test would catch it.
    assert!(
        (gr[[0, 1]] - gr[[1, 0]]).abs() > 1e-6,
        "a real phase shift should break Y_bus symmetry"
    );
}

#[test]
fn incidence_matpower_pshift_invariant_to_normalization() {
    // The MATPOWER DC convention injects a phase-shift term `p_shift` that scales
    // with the shift angle. Built from the raw (degrees) or normalized (radians)
    // network it must match, since incidence reads the shift via angle_radians.
    let raw = net_with_gens(
        "shifter",
        vec![bus(1, BusType::Ref), bus(2, BusType::Pq)],
        vec![branch_xts(1, 2, 0.1, 1.0, 30.0)],
        vec![poly_gen(1, 100.0, 0.0, 1.0)],
    );
    let norm = raw.to_normalized().unwrap();
    let ir = build_incidence(
        &IndexedNetwork::new(&raw),
        DcConvention::TapAdjustedReactance,
        &BuildOptions::default(),
    )
    .unwrap();
    let in_ = build_incidence(
        &IndexedNetwork::new(&norm),
        DcConvention::TapAdjustedReactance,
        &BuildOptions::default(),
    )
    .unwrap();
    assert_eq!(ir.p_shift.len(), in_.p_shift.len());
    for (a, b) in ir.p_shift.iter().zip(&in_.p_shift) {
        assert!((a - b).abs() < 1e-12, "p_shift differs: {a} vs {b}");
    }
    // A nonzero shift produces a nonzero injection, so the test isn't vacuous.
    assert!(
        ir.p_shift.iter().any(|&v| v.abs() > 1e-6),
        "30-degree shift should produce a nonzero p_shift"
    );
}

#[test]
fn gencost_quadratic_branches() {
    let mk = |model: u8, ncost: usize, coeffs: Vec<f64>| {
        GenCost::with_ncost(model, 0.0, 0.0, ncost, coeffs)
    };
    // Quadratic: q = 2 c2, c = c1.
    assert_eq!(mk(2, 3, vec![1.5, 2.0, 9.0]).quadratic(), Some((3.0, 2.0)));
    // Linear: q = 0, c = c1.
    assert_eq!(mk(2, 2, vec![4.0, 0.0]).quadratic(), Some((0.0, 4.0)));
    // Constant: treated as free.
    assert_eq!(mk(2, 1, vec![7.0]).quadratic(), Some((0.0, 0.0)));
    // Piecewise linear (model 1): unsupported.
    assert_eq!(mk(1, 2, vec![0.0, 0.0, 1.0, 1.0]).quadratic(), None);
    // Cubic and higher: unsupported.
    assert_eq!(mk(2, 4, vec![1.0, 2.0, 3.0, 4.0]).quadratic(), None);
    // Coefficient slice shorter than ncost: rejected, not misread by position.
    assert_eq!(mk(2, 3, vec![1.0]).quadratic(), None);
}

/// The consumer contract of the public preparation: an external solver
/// formulates the complete DC OPF from `build_dc_opf_preparation` alone —
/// demand, generator costs and bounds with source rows, thermal limits, and
/// the reference set — and its positive solver edge weights are exactly the
/// negation of the PowerModels signed susceptances `dc_network_data`
/// reports, term for term, with an identical phase shift injection. That is
/// the 0.10 sign relation between the two public assemblies: 0.9's
/// `branch_susceptance` returned the positive weight, 0.10's returns the
/// PowerModels value.
#[test]
fn public_preparation_formulates_the_complete_dc_opf() {
    use powerio_matrix::{DcOpfAssemblyOptions, build_dc_opf_preparation as prepare_instance};
    use powerio_prob::DcOpfInstance;
    use powerio_tx::{Load, dc_network_data};

    let mut shifted = Branch::new(BusId(2), BusId(3), 0.0, 0.2);
    shifted.shift = 30.0;
    shifted.rate_a = 60.0;
    let mut network = net(
        "consumer",
        vec![
            Bus::new(BusId(1), BusType::Ref, 230.0),
            Bus::new(BusId(2), BusType::Pq, 230.0),
            Bus::new(BusId(3), BusType::Pq, 230.0),
        ],
        vec![Branch::new(BusId(1), BusId(2), 0.01, 0.1), shifted],
    );
    network.loads_mut().push(Load::new(BusId(3), 90.0, 0.0));
    let mut generator = Generator::new(BusId(1));
    generator.pmax = 200.0;
    generator.cost = Some(GenCost::new(2, 0.0, 0.0, vec![0.02, 11.0, 3.0]));
    network.generators_mut().push(generator);

    let view = IndexedNetwork::new(&network);
    let data = dc_network_data(&view, DcConvention::SeriesSusceptance);
    assert!(data.omitted.is_empty(), "{:?}", data.omitted);

    let instance = DcOpfInstance::from_network(network.clone())
        .expect("instance")
        .with_approximation(DcConvention::SeriesSusceptance);
    let prep = prepare_instance(&instance, &DcOpfAssemblyOptions::default()).expect("prepare");

    // The two public assemblies describe the same rows in the same order.
    assert_eq!(prep.branches.from_bus, data.from_indices);
    assert_eq!(prep.branches.to_bus, data.to_indices);
    for (row, (&weight, &susceptance)) in prep.branches.b.iter().zip(&data.susceptance).enumerate()
    {
        assert!(weight > 0.0, "row {row}: solver weight must be positive");
        assert!(
            susceptance < 0.0,
            "row {row}: public susceptance is PowerModels signed"
        );
        assert!(
            (weight + susceptance).abs() < 1e-12,
            "row {row}: weight {weight} is not the negation of {susceptance}"
        );
    }
    // One shared phase shift injection, entry for entry.
    assert_eq!(prep.p_shift.len(), data.shift_injection.len());
    for (bus, (&prepared, &public)) in prep.p_shift.iter().zip(&data.shift_injection).enumerate() {
        assert!(
            (prepared - public).abs() < 1e-12,
            "bus {bus}: p_shift {prepared} vs shift_injection {public}"
        );
    }

    // The preparation alone carries the complete numerical problem: per unit
    // demand, thermal limits, and generator cost and bounds with their
    // source rows, so a solver never re-derives them from the network.
    assert_eq!(prep.p_d, vec![0.0, 0.0, 0.9]);
    assert!((prep.branches.f_max[1] - 0.6).abs() < 1e-12);
    assert_eq!(prep.generators.bus_of_gen, vec![0]);
    assert_eq!(prep.generators.source_rows, vec![Some(0)]);
    // MATPOWER c2 p^2 + c1 p + c0 in per unit: q = 2 c2 base^2, c = c1 base.
    assert!((prep.generators.q[0] - 2.0 * 0.02 * 100.0 * 100.0).abs() < 1e-9);
    assert!((prep.generators.c[0] - 11.0 * 100.0).abs() < 1e-9);
    assert!((prep.generators.c0[0] - 3.0).abs() < 1e-12);
    assert!((prep.generators.pmax[0] - 2.0).abs() < 1e-12);
    assert_eq!(
        prep.reference_buses.iter().copied().collect::<Vec<_>>(),
        vec![0]
    );
    // The withdrawal helper agrees with the raw columns.
    let withdrawal = prep.fixed_nodal_withdrawal();
    for (bus, &total) in withdrawal.iter().enumerate() {
        let expected = prep.p_d[bus] + prep.g_s[bus] + prep.p_shift[bus];
        assert!((total - expected).abs() < 1e-12);
    }
}

#[test]
fn public_preparation_compiles_objective_and_constraint_selections() {
    use powerio_matrix::{DcOpfAssemblyOptions, PreparedObjective, build_dc_opf_preparation};
    use powerio_prob::{ActiveConstraints, ConstraintSelection, DcOpfInstance, Objective};

    let mut first = Branch::new(BusId(1), BusId(2), 0.0, 0.1);
    first.uid = Some("line-a".into());
    first.rate_a = 80.0;
    let mut second = Branch::new(BusId(2), BusId(3), 0.0, 0.1);
    second.uid = Some("line-b".into());
    second.rate_a = 70.0;
    let mut network = net(
        "semantics",
        vec![
            Bus::new(BusId(1), BusType::Ref, 230.0),
            Bus::new(BusId(2), BusType::Pq, 230.0),
            Bus::new(BusId(3), BusType::Pq, 230.0),
        ],
        vec![first, second],
    );
    let mut generator = Generator::new(BusId(1));
    generator.uid = Some("generator-a".into());
    // A feasibility objective must not require or silently apply a cost.
    generator.cost = None;
    network.generators_mut().push(generator);

    let mut constraints = ActiveConstraints::default();
    constraints.generator_capability = ConstraintSelection::None;
    constraints.thermal_limits = ConstraintSelection::Only(vec!["line-b".into()]);
    constraints.angle_bounds = ConstraintSelection::Only(vec!["line-a".into()]);
    let instance = DcOpfInstance::from_network(network)
        .unwrap()
        .with_objective(Objective::none())
        .with_constraints(constraints);
    let prepared = build_dc_opf_preparation(&instance, &DcOpfAssemblyOptions::default()).unwrap();

    assert_eq!(prepared.objective, PreparedObjective::Feasibility);
    assert_eq!(prepared.generators.q, vec![0.0]);
    assert_eq!(prepared.generators.identities, vec!["generator-a"]);
    assert_eq!(prepared.generators.capability_active, vec![false]);
    assert_eq!(prepared.branches.identities, vec!["line-a", "line-b"]);
    assert_eq!(prepared.branches.thermal_limit_active, vec![false, true]);
    assert_eq!(prepared.branches.angle_bound_active, vec![true, false]);
}

#[test]
fn public_preparation_excludes_explicitly_isolated_rows() {
    use powerio_matrix::{DcOpfAssemblyOptions, build_dc_opf_preparation};
    use powerio_prob::DcOpfInstance;
    use powerio_tx::Load;

    let mut network = net(
        "isolated-source-row",
        vec![
            Bus::new(BusId(1), BusType::Ref, 230.0),
            Bus::new(BusId(2), BusType::Pq, 230.0),
            Bus::new(BusId(3), BusType::Isolated, 230.0),
        ],
        vec![
            Branch::new(BusId(1), BusId(2), 0.0, 0.1),
            Branch::new(BusId(2), BusId(3), 0.0, 0.1),
        ],
    );
    network.loads_mut().push(Load::new(BusId(2), 40.0, 0.0));
    network.loads_mut().push(Load::new(BusId(3), 99.0, 0.0));
    network.generators_mut().push(poly_gen(1, 100.0, 0.0, 1.0));
    network.generators_mut().push(Generator::new(BusId(3)));

    let instance = DcOpfInstance::from_network(network).unwrap();
    let prepared = build_dc_opf_preparation(&instance, &DcOpfAssemblyOptions::default()).unwrap();

    assert_eq!(prepared.bus_ids, vec![BusId(1), BusId(2)]);
    assert_eq!(prepared.bus_analysis_rows, vec![0, 1]);
    assert_eq!(prepared.bus_source_rows, vec![Some(0), Some(1)]);
    assert_eq!(prepared.p_d, vec![0.0, 0.4]);
    assert_eq!(prepared.branches.analysis_rows, vec![0]);
    assert_eq!(prepared.branches.source_rows, vec![Some(0)]);
    assert_eq!(prepared.generators.analysis_rows, vec![0]);
    assert_eq!(prepared.generators.source_rows, vec![Some(0)]);
    assert_eq!(prepared.n_source_branches, 2);
    assert_eq!(prepared.n_source_generators, 2);
}

#[test]
fn public_preparation_refuses_unsupported_objectives_and_unknown_constraints() {
    use powerio_matrix::{DcOpfAssemblyOptions, build_dc_opf_preparation};
    use powerio_prob::{ActiveConstraints, ConstraintSelection, DcOpfInstance, Objective};

    let network = net_with_gens(
        "errors",
        vec![
            Bus::new(BusId(1), BusType::Ref, 230.0),
            Bus::new(BusId(2), BusType::Pq, 230.0),
        ],
        vec![Branch::new(BusId(1), BusId(2), 0.0, 0.1)],
        vec![poly_gen(1, 100.0, 0.0, 1.0)],
    );
    let unsupported = DcOpfInstance::from_network(network.clone())
        .unwrap()
        .with_objective(Objective::network_per_phase_cost());
    assert!(matches!(
        build_dc_opf_preparation(&unsupported, &DcOpfAssemblyOptions::default()),
        Err(Error::UnsupportedOpfObjective { .. })
    ));

    for family in [
        "generator capability",
        "bus voltage bounds",
        "branch thermal limits",
        "branch angle bounds",
    ] {
        let mut constraints = ActiveConstraints::default();
        let selection = ConstraintSelection::Only(vec!["missing-identity".into()]);
        match family {
            "generator capability" => constraints.generator_capability = selection,
            "bus voltage bounds" => constraints.voltage_bounds = selection,
            "branch thermal limits" => constraints.thermal_limits = selection,
            "branch angle bounds" => constraints.angle_bounds = selection,
            _ => unreachable!(),
        }
        let unknown = DcOpfInstance::from_network(network.clone())
            .unwrap()
            .with_constraints(constraints);
        assert!(matches!(
            build_dc_opf_preparation(&unknown, &DcOpfAssemblyOptions::default()),
            Err(Error::UnknownConstraintIdentity {
                family: actual,
                ..
            }) if actual == family
        ));
    }
}

#[test]
fn three_winding_lowering_has_explicit_analysis_identities_and_no_source_rows() {
    use powerio_matrix::{DcOpfAssemblyOptions, build_dc_opf_preparation};
    use powerio_prob::{ActiveConstraints, ConstraintSelection, DcOpfInstance};
    use powerio_tx::{Impedance, Transformer3W, Winding};

    let mut network = net(
        "three-winding",
        vec![
            Bus::new(BusId(1), BusType::Ref, 230.0),
            Bus::new(BusId(2), BusType::Pq, 230.0),
            Bus::new(BusId(3), BusType::Pq, 230.0),
        ],
        Vec::new(),
    );
    let mut transformer = Transformer3W::new(
        [
            Winding::new(BusId(1)),
            Winding::new(BusId(2)),
            Winding::new(BusId(3)),
        ],
        [
            Impedance::new(0.0, 0.2, 100.0),
            Impedance::new(0.0, 0.2, 100.0),
            Impedance::new(0.0, 0.2, 100.0),
        ],
    );
    transformer.uid = Some("tx-main".into());
    for winding in &mut transformer.windings {
        winding.rate_a = 100.0;
    }
    network.transformers_3w_mut().push(transformer);
    network.generators_mut().push(poly_gen(1, 100.0, 0.0, 1.0));

    let mut constraints = ActiveConstraints::default();
    constraints.thermal_limits = ConstraintSelection::Only(vec!["tx-main/winding:2".into()]);
    let instance = DcOpfInstance::from_network(network)
        .unwrap()
        .with_constraints(constraints);
    let prepared = build_dc_opf_preparation(&instance, &DcOpfAssemblyOptions::default()).unwrap();

    assert_eq!(prepared.n_source_branches, 0);
    assert_eq!(
        prepared.branches.identities,
        vec![
            "tx-main/winding:1",
            "tx-main/winding:2",
            "tx-main/winding:3"
        ]
    );
    assert_eq!(prepared.branches.analysis_rows, vec![0, 1, 2]);
    assert_eq!(prepared.branches.source_rows, vec![None, None, None]);
    assert_eq!(
        prepared.branches.thermal_limit_active,
        vec![false, true, false]
    );
}

/// Exact value of a two bus linear dispatch. The branch flow is
/// `p_from - demand_from`, so its feasible interval is the symmetric rating.
fn two_bus_dispatch_value(costs: [f64; 2], demand: [f64; 2], rating: f64) -> f64 {
    let total = demand[0] + demand[1];
    let lower = 0.0_f64.max(demand[0] - rating);
    let upper = total.min(demand[0] + rating);
    assert!(lower <= upper);
    let p_from = if costs[0] <= costs[1] { upper } else { lower };
    costs[0] * p_from + costs[1] * (total - p_from)
}

fn central_difference(mut f: impl FnMut(f64) -> f64, x: f64) -> f64 {
    let step = 1e-4;
    (f(x + step) - f(x - step)) / (2.0 * step)
}

#[test]
fn economic_output_signs_match_optimal_value_derivatives() {
    use std::sync::Arc;

    use powerio_prob::{DcOpfInstance, DcOpfSolution, Termination};

    let mut branch = Branch::new(BusId(1), BusId(2), 0.0, 0.1);
    branch.rate_a = 40.0;
    let network = net_with_gens(
        "economic-signs",
        vec![bus(1, BusType::Ref), bus(2, BusType::Pq)],
        vec![branch],
        vec![poly_gen(1, 200.0, 0.0, 10.0), poly_gen(2, 200.0, 0.0, 30.0)],
    );
    let instance = Arc::new(DcOpfInstance::from_network(network).unwrap());

    let costs = [10.0, 30.0];
    let demand = [0.0, 100.0];
    let rating = 40.0;
    let demand_from_derivative = central_difference(
        |value| two_bus_dispatch_value(costs, [value, demand[1]], rating),
        demand[0],
    );
    let demand_to_derivative = central_difference(
        |value| two_bus_dispatch_value(costs, [demand[0], value], rating),
        demand[1],
    );
    let rating_derivative =
        central_difference(|value| two_bus_dispatch_value(costs, demand, value), rating);

    let solution = DcOpfSolution::new(
        instance,
        Termination::Converged,
        vec![0.0, -0.04],
        vec![40.0, -40.0],
        vec![40.0],
        vec![-40.0],
        vec![40.0, 60.0],
        two_bus_dispatch_value(costs, demand, rating),
    )
    .unwrap()
    .with_bus_active_power_marginals(vec![10.0, 30.0])
    .unwrap()
    .with_branch_thermal_limit_multipliers(vec![20.0], vec![0.0])
    .unwrap();

    assert!(
        (solution.bus_active_power_marginal(BusId(1)).unwrap() - demand_from_derivative).abs()
            < 1e-8
    );
    assert!(
        (solution.bus_active_power_marginal(BusId(2)).unwrap() - demand_to_derivative).abs() < 1e-8
    );
    let multiplier_sum = solution.branch_from_limit_multiplier("branches:0").unwrap()
        + solution.branch_to_limit_multiplier("branches:0").unwrap();
    assert!((rating_derivative + multiplier_sum).abs() < 1e-8);

    // Reverse the merit order and demand direction. The negative flow bound
    // binds, so the same shadow value belongs to the separate `to` column.
    let reverse_costs = [30.0, 10.0];
    let reverse_demand = [100.0, 0.0];
    let reverse_rating_derivative = central_difference(
        |value| two_bus_dispatch_value(reverse_costs, reverse_demand, value),
        rating,
    );
    assert!((reverse_rating_derivative + 20.0).abs() < 1e-8);
}
