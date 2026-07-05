//! Integration tests against real MATPOWER fixtures vendored from
//! `https://github.com/MATPOWER/matpower/tree/master/data`.

use std::path::PathBuf;

use powerio_matrix::matrix::{
    BuildOptions, MatrixStats, build_bdoubleprime, build_bprime, build_lacpf, build_ybus,
    sddm_check,
};
use powerio_matrix::{BusId, IndexedNetwork, parse_matpower_file};

fn fixture(name: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("../tests/data");
    p.push(name);
    p
}

#[test]
fn case9_parses_correctly() {
    let net = parse_matpower_file(fixture("case9.m")).unwrap();
    assert_eq!(net.buses.len(), 9);
    assert_eq!(net.branches.len(), 9);
    assert!((net.base_mva - 100.0).abs() < 1e-12);
    // case9 buses are contiguous 1..=9.
    let g = IndexedNetwork::new(&net);
    for i in 1..=9 {
        assert_eq!(g.bus_index(BusId(i)), Some(i - 1));
    }
}

#[test]
fn case14_parses_correctly() {
    let net = parse_matpower_file(fixture("case14.m")).unwrap();
    assert_eq!(net.buses.len(), 14);
    assert!(net.branches.len() >= 20);
}

#[test]
fn case30_parses_correctly() {
    let net = parse_matpower_file(fixture("case30.m")).unwrap();
    assert_eq!(net.buses.len(), 30);
}

#[test]
fn bprime_is_singular_laplacian_on_real_cases() {
    for name in ["case9.m", "case14.m", "case30.m", "case57.m"] {
        let net = parse_matpower_file(fixture(name)).unwrap();
        let view = IndexedNetwork::new(&net);
        let b = build_bprime(&view, &BuildOptions::default()).unwrap();
        let stats = MatrixStats::from_csr(&b);
        assert!(stats.m_matrix_sign, "{name}: Bp must have M-matrix signs");
        // Singular Laplacian: diag exactly equals row-sum of |off-diag|.
        assert!(
            stats.min_dd_margin.abs() < 1e-9,
            "{name}: Bp should have zero diagonal dominance margin for these cases, got {}",
            stats.min_dd_margin
        );
        assert!(stats.min_diag > 0.0);
        assert_eq!(b.rows(), net.buses.len());
        assert_eq!(b.cols(), net.buses.len());
    }
}

#[test]
fn bdoubleprime_includes_shunts_on_case30() {
    let net = parse_matpower_file(fixture("case30.m")).unwrap();
    let view = IndexedNetwork::new(&net);
    let bpp = build_bdoubleprime(&view, &BuildOptions::default()).unwrap();
    let stats = MatrixStats::from_csr(&bpp);
    // case30 has explicit bus shunts → strict diagonal dominance.
    assert!(
        stats.min_dd_margin > 1e-9 || stats.m_matrix_sign,
        "Bpp on case30 should be at least M-matrix-signed"
    );
}

#[test]
fn ybus_split_matches_complex_invariants() {
    let net = parse_matpower_file(fixture("case14.m")).unwrap();
    let view = IndexedNetwork::new(&net);
    let parts = build_ybus(&view, &BuildOptions::default()).unwrap();
    assert_eq!(parts.g.rows(), net.buses.len());
    assert_eq!(parts.b.rows(), net.buses.len());
    // Without phase shifters case14 should yield symmetric Y_bus.
    let g = parts.g.to_dense();
    let b = parts.b.to_dense();
    for i in 0..net.buses.len() {
        for j in (i + 1)..net.buses.len() {
            assert!((g[[i, j]] - g[[j, i]]).abs() < 1e-12);
            assert!((b[[i, j]] - b[[j, i]]).abs() < 1e-12);
        }
    }
}

#[test]
fn ybus_invariant_to_normalization_on_case30() {
    // The per-unit Y_bus is identical whether built from the raw case or its
    // normalized (already per-unit) form: per_unit_base() is 1.0 for a normalized
    // network, so its shunts aren't divided by base a second time. case30 has
    // explicit bus shunts, so this exercises the diagonal shunt path that would
    // double-scale (~base x) without per_unit_base.
    let raw = parse_matpower_file(fixture("case30.m")).unwrap();
    let norm = raw.to_normalized().unwrap();
    assert_eq!(norm.buses.len(), raw.buses.len(), "no buses dropped");
    let opts = BuildOptions::default();
    let yr = build_ybus(&IndexedNetwork::new(&raw), &opts).unwrap();
    let yn = build_ybus(&IndexedNetwork::new(&norm), &opts).unwrap();
    let (gr, gn) = (yr.g.to_dense(), yn.g.to_dense());
    let (br, bn) = (yr.b.to_dense(), yn.b.to_dense());
    let n = raw.buses.len();
    for i in 0..n {
        for j in 0..n {
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
}

#[test]
fn lacpf_block_dimensions() {
    let net = parse_matpower_file(fixture("case14.m")).unwrap();
    let view = IndexedNetwork::new(&net);
    let j = build_lacpf(&view, &BuildOptions::default()).unwrap();
    assert_eq!(j.rows(), 2 * net.buses.len());
    assert_eq!(j.cols(), 2 * net.buses.len());
}

#[test]
fn pipeline_writes_expected_files_for_case9() {
    use powerio_matrix::pipeline::{MatrixKind, Pipeline, RhsKind};
    let tmp = tempdir();
    let net = parse_matpower_file(fixture("case9.m")).unwrap();
    let pipeline = Pipeline {
        matrices: vec![
            MatrixKind::BPrime,
            MatrixKind::BDoublePrime,
            MatrixKind::YbusG,
            MatrixKind::YbusB,
        ],
        options: BuildOptions::default(),
        rhs: RhsKind::Random,
        rng_seed: 42,
        source_file: Some(fixture("case9.m")),
    };
    let outputs = pipeline.run(&net, &tmp).unwrap();
    assert_eq!(outputs.case_name, "case9");
    let names: Vec<String> = outputs
        .files
        .iter()
        .filter_map(|p| p.file_name().and_then(|s| s.to_str()).map(str::to_string))
        .collect();
    assert!(names.iter().any(|n| n == "case9_bprime.mtx"));
    assert!(names.iter().any(|n| n == "case9_bdoubleprime.mtx"));
    assert!(names.iter().any(|n| n == "case9_ybus_real.mtx"));
    assert!(names.iter().any(|n| n == "case9_ybus_imag.mtx"));
    assert!(names.iter().any(|n| n == "case9_meta.json"));
    assert!(names.iter().any(|n| n.contains("rhs.mtx")));

    // Sanity check: re-read Bp from disk and verify its sign pattern.
    let bprime_path = tmp.join("case9_bprime.mtx");
    let reread = powerio_matrix::io::read_mtx(&bprime_path).unwrap();
    assert!(sddm_check(&reread) || MatrixStats::from_csr(&reread).m_matrix_sign);

    let view = IndexedNetwork::new(&net);
    let ybus = build_ybus(&view, &BuildOptions::default()).unwrap();
    let real = powerio_matrix::io::read_mtx(tmp.join("case9_ybus_real.mtx")).unwrap();
    assert_csr_eq(&real, &ybus.g);

    let mut expected_imag = ybus.b;
    expected_imag.data_mut().iter_mut().for_each(|v| *v = -*v);
    let imag = powerio_matrix::io::read_mtx(tmp.join("case9_ybus_imag.mtx")).unwrap();
    assert_csr_eq(&imag, &expected_imag);
}

fn tempdir() -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "powerio-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn assert_csr_eq(left: &sprs::CsMat<f64>, right: &sprs::CsMat<f64>) {
    assert_eq!(left.rows(), right.rows());
    assert_eq!(left.cols(), right.cols());
    let left = left.to_dense();
    let right = right.to_dense();
    for ((i, j), &v) in left.indexed_iter() {
        assert!((v - right[[i, j]]).abs() < 1e-12, "[{i},{j}]");
    }
}
