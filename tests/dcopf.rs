//! DC-OPF matrix forge: incidence, Laplacian, OPF instance, and the export
//! bundle. Run against vendored MATPOWER cases.

use netmat::case::BusType;
use netmat::{
    Branch, Bus, DcConvention, Error, GenCost, Generator, MpcCase, Scheme, Units, build_adjacency,
    build_bprime, build_flow_map, build_incidence, build_lodf, build_opf_instance, build_ptdf,
    build_weighted_laplacian, ground_at, parse_matpower_file,
};
use sprs::CsMat;

const CASES: &[&str] = &[
    "tests/data/case9.m",
    "tests/data/case14.m",
    "tests/data/case30.m",
    "tests/data/case57.m",
    "tests/data/case118.m",
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
        name: None,
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
        let dptdf = dense(&ptdf);
        for l in 0..inc.m() {
            assert!(dptdf[l][r].abs() < 1e-12, "{path}: PTDF slack col nonzero");
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

// ---------------------------------------------------------------------------
// Helpers for hand-built synthetic cases (no fixture has a phase shifter, a
// disconnected topology, or two generators on one bus).
// ---------------------------------------------------------------------------

fn bus(id: usize, kind: BusType) -> Bus {
    Bus {
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
        name: None,
    }
}

fn branch(from: usize, to: usize, x: f64) -> Branch {
    branch_xts(from, to, x, 0.0, 0.0)
}

fn branch_xts(from: usize, to: usize, x: f64, tap: f64, shift: f64) -> Branch {
    Branch {
        from_id: from,
        to_id: to,
        r: 0.0,
        x,
        b: 0.0,
        rate_a: 0.0,
        rate_b: 0.0,
        rate_c: 0.0,
        tap,
        shift,
        status: 1.0,
        angmin: -360.0,
        angmax: 360.0,
    }
}

/// Generator on `bus_id` with the given cost curve (pmax = 100 MW).
fn gen_with_cost(bus_id: usize, cost: Option<GenCost>) -> Generator {
    Generator {
        bus_id,
        pg: 0.0,
        qg: 0.0,
        qmax: 0.0,
        qmin: 0.0,
        vg: 1.0,
        mbase: 100.0,
        status: 1.0,
        pmax: 100.0,
        pmin: 0.0,
        cost,
        reactive_cost: None,
        extra: Vec::new(),
    }
}

/// Polynomial (model 2, quadratic) generator: cost `c2 p² + c1 p`.
fn poly_gen(bus_id: usize, pmax: f64, c2: f64, c1: f64) -> Generator {
    let cost = GenCost {
        model: 2,
        startup: 0.0,
        shutdown: 0.0,
        ncost: 3,
        coeffs: vec![c2, c1, 0.0],
    };
    Generator {
        pmax,
        ..gen_with_cost(bus_id, Some(cost))
    }
}

/// DC-OPF instance for `case` under the default PaperPure convention. Returns
/// the `Result` so error-path tests can assert on the failure.
fn opf_of(case: &MpcCase, units: Units) -> netmat::Result<netmat::OpfInstance> {
    let inc = build_incidence(case, DcConvention::PaperPure)?;
    build_opf_instance(case, &inc, units)
}

/// Symmetric 3-bus triangle, slack at bus 1, unit susceptance on every branch.
/// Branch order fixes the incidence columns: e0=1→2, e1=1→3, e2=2→3.
fn triangle() -> MpcCase {
    MpcCase::new(
        "triangle",
        100.0,
        vec![bus(1, BusType::Ref), bus(2, BusType::Pq), bus(3, BusType::Pq)],
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
    let ptdf = dense(&build_ptdf(&triangle(), DcConvention::PaperPure).unwrap());
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
    let lodf = dense(&build_lodf(&triangle(), DcConvention::PaperPure).unwrap());
    let expected = [
        [-1.0, 1.0, -1.0],
        [1.0, -1.0, 1.0],
        [-1.0, 1.0, -1.0],
    ];
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
    let case = MpcCase::new(
        "shifter",
        100.0,
        vec![bus(1, BusType::Ref), bus(2, BusType::Pq)],
        vec![branch_xts(1, 2, x, tap, shift_deg)],
    );

    // PaperPure ignores tap and shift: b = 1/x, no phase injection.
    let pp = build_incidence(&case, DcConvention::PaperPure).unwrap();
    assert!((pp.b[0] - 1.0 / x).abs() < 1e-12);
    assert!(pp.p_shift.iter().all(|&v| v == 0.0));

    // Matpower: b = 1/(x·τ); makeBdc injection ±b·shift at from/to.
    let mp = build_incidence(&case, DcConvention::Matpower).unwrap();
    let b_e = 1.0 / (x * tap);
    let shift_rad = shift_deg.to_radians();
    assert!((mp.b[0] - b_e).abs() < 1e-12, "b_e {} != {b_e}", mp.b[0]);
    assert!((mp.p_shift[0] - (-b_e * shift_rad)).abs() < 1e-12);
    assert!((mp.p_shift[1] - (b_e * shift_rad)).abs() < 1e-12);
}

#[test]
fn bundle_vectors_round_trip() {
    let case = load("tests/data/case14.m");
    let dir = std::env::temp_dir().join("netmat_dcopf_vectors_test");
    let _ = std::fs::remove_dir_all(&dir);
    let out = netmat::write_dcopf_bundle(&case, &dir, &netmat::DcOpfOptions::default()).unwrap();

    // Default options are PaperPure + PerUnit; rebuild the instance to compare.
    let inc = build_incidence(&case, DcConvention::PaperPure).unwrap();
    let opf = build_opf_instance(&case, &inc, Units::PerUnit).unwrap();

    let check = |name: &str, want: &[f64]| {
        let got = netmat::io::read_vector_mtx(out.dir.join(name)).unwrap();
        assert_eq!(got.len(), want.len(), "{name}: length");
        for (i, (&g, &w)) in got.iter().zip(want).enumerate() {
            assert!((g - w).abs() < 1e-9, "{name}[{i}]={g} != {w}");
        }
    };
    check("q.mtx", &opf.q_bus);
    check("c.mtx", &opf.c_bus);
    check("fmax.mtx", &opf.f_max);
    check("pd.mtx", &opf.p_d);
    check("b.mtx", &inc.b);

    // Manifest agrees with the case.
    let meta: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out.dir.join("dcopf_meta.json")).unwrap())
            .unwrap();
    assert_eq!(meta["n_gen"].as_u64().unwrap() as usize, opf.n_gen);
    assert_eq!(
        meta["reference_bus"].as_u64().unwrap() as usize,
        case.reference_bus_index().unwrap()
    );
    assert_eq!(meta["units"], "PerUnit");
    assert_eq!(meta["convention"], "PaperPure");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn radial_lodf_is_negative_identity() {
    // Path 1-2-3: every branch is a bridge, so each outage islands the network
    // and the LODF column zeroes out except the −1 diagonal.
    let case = MpcCase::new(
        "path",
        100.0,
        vec![bus(1, BusType::Ref), bus(2, BusType::Pq), bus(3, BusType::Pq)],
        vec![branch(1, 2, 0.1), branch(2, 3, 0.1)],
    );
    let lodf = dense(&build_lodf(&case, DcConvention::PaperPure).unwrap());
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
fn disconnected_network_errors() {
    // Two islands (1-2 and 3-4) with a single reference bus.
    let case = MpcCase::new(
        "islands",
        100.0,
        vec![
            bus(1, BusType::Ref),
            bus(2, BusType::Pq),
            bus(3, BusType::Pq),
            bus(4, BusType::Pq),
        ],
        vec![branch(1, 2, 0.1), branch(3, 4, 0.1)],
    );
    assert_eq!(case.n_connected_components(), 2);
    let p = build_ptdf(&case, DcConvention::PaperPure).unwrap_err();
    assert!(matches!(p, Error::DisconnectedNetwork { components: 2 }), "ptdf: {p:?}");
    let l = build_lodf(&case, DcConvention::PaperPure).unwrap_err();
    assert!(matches!(l, Error::DisconnectedNetwork { components: 2 }), "lodf: {l:?}");
}

#[test]
fn perunit_scales_native_by_base() {
    let case = load("tests/data/case9.m");
    let base = case.base_mva;
    let native = opf_of(&case, Units::Native).unwrap();
    let pu = opf_of(&case, Units::PerUnit).unwrap();
    for i in 0..case.n() {
        assert!((pu.q_bus[i] - native.q_bus[i] * base * base).abs() < 1e-6, "q[{i}]");
        assert!((pu.c_bus[i] - native.c_bus[i] * base).abs() < 1e-6, "c[{i}]");
        assert!((pu.pmax_bus[i] - native.pmax_bus[i] / base).abs() < 1e-9, "pmax[{i}]");
        assert!((pu.p_d[i] - native.p_d[i] / base).abs() < 1e-9, "pd[{i}]");
    }
}

#[test]
fn multi_generator_bus_sums_cost() {
    // Two in-service generators on bus 1; the bus-indexed vectors sum them.
    let case = MpcCase::new(
        "twogen",
        100.0,
        vec![bus(1, BusType::Ref), bus(2, BusType::Pq)],
        vec![branch(1, 2, 0.1)],
    )
    .with_gens(vec![poly_gen(1, 100.0, 1.0, 2.0), poly_gen(1, 50.0, 3.0, 4.0)]);
    let opf = opf_of(&case, Units::Native).unwrap();
    assert_eq!(opf.n_gen, 2);
    let b0 = case.bus_index(1).unwrap();
    assert!((opf.q_bus[b0] - (opf.q_gen[0] + opf.q_gen[1])).abs() < 1e-12);
    assert!((opf.c_bus[b0] - (opf.c_gen[0] + opf.c_gen[1])).abs() < 1e-12);
    assert!((opf.pmax_bus[b0] - (opf.pmax_gen[0] + opf.pmax_gen[1])).abs() < 1e-12);
}

#[test]
fn gencost_quadratic_branches() {
    let mk = |model: u8, ncost: usize, coeffs: Vec<f64>| GenCost {
        model,
        startup: 0.0,
        shutdown: 0.0,
        ncost,
        coeffs,
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

#[test]
fn opf_distinguishes_missing_from_unsupported_cost() {
    let case = |name: &str, cost: Option<GenCost>| {
        MpcCase::new(
            name,
            100.0,
            vec![bus(1, BusType::Ref), bus(2, BusType::Pq)],
            vec![branch(1, 2, 0.1)],
        )
        .with_gens(vec![gen_with_cost(1, cost)])
    };

    // No cost row → MissingGenCost.
    assert!(matches!(
        opf_of(&case("nocost", None), Units::Native).unwrap_err(),
        Error::MissingGenCost { gen: 0 }
    ));

    // Present but piecewise-linear → UnsupportedCostModel.
    let pwl = GenCost {
        model: 1,
        startup: 0.0,
        shutdown: 0.0,
        ncost: 2,
        coeffs: vec![0.0, 0.0, 1.0, 1.0],
    };
    assert!(matches!(
        opf_of(&case("pwl", Some(pwl)), Units::Native).unwrap_err(),
        Error::UnsupportedCostModel { gen: 0, model: 1, .. }
    ));
}
