//! DC-OPF matrix forge: incidence, Laplacian, OPF instance, and the export
//! bundle. Run against vendored MATPOWER cases.

use netmat::case::BusType;
use netmat::{
    Bus, DcConvention, Error, MpcCase, Scheme, Units, build_adjacency, build_bprime,
    build_flow_map, build_incidence, build_lodf, build_opf_instance, build_ptdf,
    build_weighted_laplacian, ground_at, parse_matpower_file,
};
use sprs::CsMat;

const CASES: &[&str] = &[
    "tests/data/case9.m",
    "tests/data/case14.m",
    "tests/data/case30.m",
];

fn load(path: &str) -> MpcCase {
    parse_matpower_file(path).unwrap_or_else(|e| panic!("parse {path}: {e}"))
}

fn dense(m: &CsMat<f64>) -> Vec<Vec<f64>> {
    let mut d = vec![vec![0.0; m.cols()]; m.rows()];
    for (&v, (i, j)) in m.iter() {
        d[i][j] = v;
    }
    d
}

/// Positive definiteness via dense Cholesky; small matrices only.
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
    let case = load("tests/data/case9.m");
    assert_eq!(case.gens.len(), 3);
    let quads: Vec<(f64, f64)> = case
        .gens
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
    // L = A diag(1/x) Aᵀ is exactly B' in the XB scheme.
    for path in CASES {
        let case = load(path);
        let inc = build_incidence(&case, DcConvention::PaperPure).unwrap();
        let l = build_weighted_laplacian(&inc.a, &inc.b);
        let bp = build_bprime(
            &case,
            &netmat::BuildOptions {
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
                    "{path}: L[{i}][{j}]={} != B'[{i}][{j}]={}",
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
        let inc = build_incidence(&case, DcConvention::PaperPure).unwrap();
        let (n, m) = (inc.n(), inc.m());
        assert_eq!(inc.a.rows(), n);
        assert_eq!(inc.a.cols(), m);
        assert_eq!(inc.a.nnz(), 2 * m, "{path}: two nonzeros per column");
        assert_eq!(inc.b.len(), m);
        assert_eq!(inc.branch_of_col.len(), m);

        // Each column sums to 0 with one +1 and one −1.
        let mut col_sum = vec![0.0; m];
        let mut col_cnt = vec![0usize; m];
        for (&v, (_, j)) in inc.a.iter() {
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
fn laplacian_is_psd_with_constant_kernel() {
    for path in CASES {
        let case = load(path);
        let inc = build_incidence(&case, DcConvention::PaperPure).unwrap();
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
        let r = case.reference_bus_index().unwrap();
        let inc = build_incidence(&case, DcConvention::PaperPure).unwrap();
        let l = build_weighted_laplacian(&inc.a, &inc.b);
        let lg = ground_at(&l, r);
        assert_eq!(lg.rows(), case.n() - 1);
        assert_eq!(lg.cols(), case.n() - 1);
        assert!(is_spd(&dense(&lg)), "{path}: grounded L not SPD");
    }
}

#[test]
fn flow_map_reconstructs_laplacian() {
    for path in CASES {
        let case = load(path);
        let inc = build_incidence(&case, DcConvention::PaperPure).unwrap();
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
fn opf_instance_shapes_and_cg() {
    for path in CASES {
        let case = load(path);
        let inc = build_incidence(&case, DcConvention::PaperPure).unwrap();
        let opf = build_opf_instance(&case, &inc, Units::PerUnit).unwrap();
        let (n, m, n_gen) = (case.n(), inc.m(), opf.n_gen);
        assert_eq!(opf.q_bus.len(), n);
        assert_eq!(opf.c_bus.len(), n);
        assert_eq!(opf.pmax_bus.len(), n);
        assert_eq!(opf.p_d.len(), n);
        assert_eq!(opf.f_max.len(), m);
        assert_eq!(opf.q_gen.len(), n_gen);
        assert_eq!(opf.c_g.rows(), n);
        assert_eq!(opf.c_g.cols(), n_gen);
        // C_g: one 1 per generator column.
        assert_eq!(opf.c_g.nnz(), n_gen);
        let mut col_sum = vec![0.0; n_gen];
        for (&v, (_, g)) in opf.c_g.iter() {
            assert!((v - 1.0).abs() < 1e-12);
            col_sum[g] += v;
        }
        assert!(col_sum.iter().all(|&s| (s - 1.0).abs() < 1e-12));
    }
}

#[test]
fn bundle_round_trips() {
    let case = load("tests/data/case14.m");
    let dir = std::env::temp_dir().join("netmat_dcopf_test");
    let _ = std::fs::remove_dir_all(&dir);
    let out = netmat::write_dcopf_bundle(&case, &dir, &netmat::DcOpfOptions::default()).unwrap();
    assert!(out.dir.join("A.mtx").exists());
    assert!(out.dir.join("L.mtx").exists());
    assert!(out.dir.join("dcopf_meta.json").exists());

    let a = netmat::io::read_mtx(out.dir.join("A.mtx")).unwrap();
    assert_eq!(a.rows(), case.n());
    let l = netmat::io::read_mtx(out.dir.join("L.mtx")).unwrap();
    assert_eq!(l.rows(), case.n());
    assert_eq!(l.cols(), case.n());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn no_generators_errors() {
    // A synthetic case has branches but no generators.
    let spec = netmat::synth::SynthSpec {
        topology: netmat::synth::Topology::Tree,
        n: 16,
        r_over_x: 0.1,
        mean_x: 0.05,
        seed: 1,
    };
    let case = netmat::synth::generate(&spec);
    let inc = build_incidence(&case, DcConvention::PaperPure).unwrap();
    let err = build_opf_instance(&case, &inc, Units::PerUnit).unwrap_err();
    assert!(matches!(err, Error::NoGenerators), "got {err:?}");
}

#[test]
fn reference_bus_count_errors() {
    let mk = |id: usize, kind: BusType| Bus {
        id,
        kind,
        pd: 0.0,
        qd: 0.0,
        gs: 0.0,
        bs: 0.0,
        area: 1,
        vm: 1.0,
        va: 0.0,
        base_kv: 345.0,
        zone: 1,
        vmax: 1.1,
        vmin: 0.9,
    };
    // Two reference buses.
    let two = MpcCase::new(
        "two_ref",
        100.0,
        vec![mk(1, BusType::Ref), mk(2, BusType::Ref)],
        vec![],
    );
    assert!(matches!(
        two.reference_bus_index(),
        Err(Error::ReferenceBusCount { found: 2 })
    ));
    // Zero reference buses.
    let zero = MpcCase::new("no_ref", 100.0, vec![mk(1, BusType::Pq)], vec![]);
    assert!(matches!(
        zero.reference_bus_index(),
        Err(Error::ReferenceBusCount { found: 0 })
    ));
}

#[test]
fn adjacency_is_symmetric_01() {
    for path in CASES {
        let case = load(path);
        let a = build_adjacency(&case).unwrap();
        assert_eq!(a.rows(), case.n());
        assert_eq!(a.cols(), case.n());
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
fn ptdf_satisfies_kcl() {
    // A · PTDF = I − e_r·1ᵀ: nodal balance for every injection.
    for path in CASES {
        let case = load(path);
        let r = case.reference_bus_index().unwrap();
        let inc = build_incidence(&case, DcConvention::PaperPure).unwrap();
        let ptdf = build_ptdf(&case, DcConvention::PaperPure).unwrap();
        assert_eq!(ptdf.rows(), inc.m());
        assert_eq!(ptdf.cols(), case.n());
        let m = dense(&(&inc.a * &ptdf)); // n × n
        let n = case.n();
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
        for l in 0..inc.m() {
            assert!(dense(&ptdf)[l][r].abs() < 1e-12, "{path}: PTDF slack col nonzero");
        }
    }
}

#[test]
fn lodf_diagonal_is_minus_one() {
    for path in CASES {
        let case = load(path);
        let lodf = build_lodf(&case, DcConvention::PaperPure).unwrap();
        let inc = build_incidence(&case, DcConvention::PaperPure).unwrap();
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
