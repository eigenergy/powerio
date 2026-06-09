//! DC OPF matrix forge: incidence, Laplacian, OPF instance, and the export
//! bundle. Run against vendored MATPOWER cases.

use powerio_matrix::IndexedNetwork;
use powerio_matrix::{
    Branch, BuildOptions, Bus, BusId, BusType, DcConvention, Error, Extras, GenCost, Generator,
    Network, Scheme, Units, build_adjacency, build_bprime, build_flow_map, build_incidence,
    build_lodf, build_opf_instance, build_ptdf, build_weighted_laplacian, build_ybus, ground_at,
    parse_matpower_file,
};
use sprs::CsMat;

const CASES: &[&str] = &[
    "../tests/data/case9.m",
    "../tests/data/case14.m",
    "../tests/data/case30.m",
    "../tests/data/case57.m",
    "../tests/data/case118.m",
];

fn load(path: &str) -> Network {
    parse_matpower_file(path).unwrap_or_else(|e| panic!("parse {path}: {e}"))
}

/// In-memory network from hand-built buses/branches (no loads/shunts/source).
fn net(name: &str, buses: Vec<Bus>, branches: Vec<Branch>) -> Network {
    Network::in_memory(name, 100.0, buses, branches)
}

fn net_with_gens(
    name: &str,
    buses: Vec<Bus>,
    branches: Vec<Branch>,
    generators: Vec<Generator>,
) -> Network {
    Network {
        generators,
        ..net(name, buses, branches)
    }
}

fn dense(m: &CsMat<f64>) -> Vec<Vec<f64>> {
    let mut d = vec![vec![0.0; m.cols()]; m.rows()];
    for (&v, (i, j)) in m {
        d[i][j] = v;
    }
    d
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
    assert_eq!(case.generators.len(), 3);
    let quads: Vec<(f64, f64)> = case
        .generators
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
        let view = IndexedNetwork::new(&case);
        let inc = build_incidence(&view, DcConvention::PaperPure).unwrap();
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
        let view = IndexedNetwork::new(&case);
        let inc = build_incidence(&view, DcConvention::PaperPure).unwrap();
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
        let inc = build_incidence(&view, DcConvention::PaperPure).unwrap();
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
        let inc = build_incidence(&view, DcConvention::PaperPure).unwrap();
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
        let inc = build_incidence(&view, DcConvention::PaperPure).unwrap();
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
        let view = IndexedNetwork::new(&case);
        let inc = build_incidence(&view, DcConvention::PaperPure).unwrap();
        let opf = build_opf_instance(&view, &inc, Units::PerUnit).unwrap();
        let (n, m, n_gen) = (view.n(), inc.m(), opf.n_gen());
        assert_eq!(opf.bus.q.len(), n);
        assert_eq!(opf.bus.c.len(), n);
        assert_eq!(opf.bus.pmax.len(), n);
        assert_eq!(opf.bus.p_d.len(), n);
        assert_eq!(opf.f_max.len(), m);
        assert_eq!(opf.gen_costs.q.len(), n_gen);
        assert_eq!(opf.c_g.rows(), n);
        assert_eq!(opf.c_g.cols(), n_gen);
        // C_g: one 1 per generator column.
        assert_eq!(opf.c_g.nnz(), n_gen);
        let mut col_sum = vec![0.0; n_gen];
        for (&v, (_, g)) in &opf.c_g {
            assert!((v - 1.0).abs() < 1e-12);
            col_sum[g] += v;
        }
        assert!(col_sum.iter().all(|&s| (s - 1.0).abs() < 1e-12));
    }
}

#[test]
fn bundle_round_trips() {
    let case = load("../tests/data/case14.m");
    let dir = std::env::temp_dir().join("powerio_dcopf_test");
    let _ = std::fs::remove_dir_all(&dir);
    let out =
        powerio_matrix::write_dcopf_bundle(&case, &dir, &powerio_matrix::DcOpfOptions::default())
            .unwrap();
    assert!(out.dir.join("A.mtx").exists());
    assert!(out.dir.join("L.mtx").exists());
    assert!(out.dir.join("dcopf_meta.json").exists());

    let a = powerio_matrix::io::read_mtx(out.dir.join("A.mtx")).unwrap();
    assert_eq!(a.rows(), case.buses.len());
    let l = powerio_matrix::io::read_mtx(out.dir.join("L.mtx")).unwrap();
    assert_eq!(l.rows(), case.buses.len());
    assert_eq!(l.cols(), case.buses.len());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn no_generators_errors() {
    // A synthetic case has branches but no generators.
    let spec = powerio_matrix::synth::SynthSpec {
        topology: powerio_matrix::synth::Topology::Tree,
        n: 16,
        r_over_x: 0.1,
        mean_x: 0.05,
        seed: 1,
    };
    let case = powerio_matrix::synth::generate(&spec);
    let view = IndexedNetwork::new(&case);
    let inc = build_incidence(&view, DcConvention::PaperPure).unwrap();
    let err = build_opf_instance(&view, &inc, Units::PerUnit).unwrap_err();
    assert!(matches!(err, Error::NoGenerators), "got {err:?}");
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
        Err(Error::ReferenceBusCount { found: 2 })
    ));
    // Zero reference buses.
    let zero = net("no_ref", vec![bus(1, BusType::Pq)], vec![]);
    assert!(matches!(
        IndexedNetwork::new(&zero).reference_bus_index(),
        Err(Error::ReferenceBusCount { found: 0 })
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
        let inc = build_incidence(&view, DcConvention::PaperPure).unwrap();
        let ptdf = build_ptdf(&view, DcConvention::PaperPure).unwrap();
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
        let lodf = build_lodf(&view, DcConvention::PaperPure).unwrap();
        let inc = build_incidence(&view, DcConvention::PaperPure).unwrap();
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
        id: BusId(id),
        kind,
        vm: 1.0,
        va: 0.0,
        base_kv: 345.0,
        vmax: 1.1,
        vmin: 0.9,
        area: 1,
        zone: 1,
        name: None,
        extras: Extras::new(),
    }
}

fn branch(from: usize, to: usize, x: f64) -> Branch {
    branch_xts(from, to, x, 0.0, 0.0)
}

fn branch_xts(from: usize, to: usize, x: f64, tap: f64, shift: f64) -> Branch {
    Branch {
        from: BusId(from),
        to: BusId(to),
        r: 0.0,
        x,
        b: 0.0,
        rate_a: 0.0,
        rate_b: 0.0,
        rate_c: 0.0,
        tap,
        shift,
        in_service: true,
        angmin: -360.0,
        angmax: 360.0,
        extras: Extras::new(),
    }
}

/// Generator on `bus_id` with the given cost curve (pmax = 100 MW).
fn gen_with_cost(bus: usize, cost: Option<GenCost>) -> Generator {
    Generator {
        bus: BusId(bus),
        pg: 0.0,
        qg: 0.0,
        qmax: 0.0,
        qmin: 0.0,
        vg: 1.0,
        mbase: 100.0,
        pmax: 100.0,
        pmin: 0.0,
        in_service: true,
        cost,
        caps: Default::default(),
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

/// DC OPF instance for `case` under the default PaperPure convention. Returns
/// the `Result` so error-path tests can assert on the failure.
fn opf_of(case: &Network, units: Units) -> powerio_matrix::Result<powerio_matrix::OpfInstance> {
    let view = IndexedNetwork::new(case);
    let inc = build_incidence(&view, DcConvention::PaperPure)?;
    build_opf_instance(&view, &inc, units)
}

/// Symmetric 3-bus triangle, slack at bus 1, unit susceptance on every branch.
/// Branch order fixes the incidence columns: e0=1→2, e1=1→3, e2=2→3.
fn triangle() -> Network {
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
    let ptdf = dense(&build_ptdf(&view, DcConvention::PaperPure).unwrap());
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
    let lodf = dense(&build_lodf(&view, DcConvention::PaperPure).unwrap());
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

    // PaperPure ignores tap and shift: b = 1/x, no phase injection.
    let pp = build_incidence(&view, DcConvention::PaperPure).unwrap();
    assert!((pp.b[0] - 1.0 / x).abs() < 1e-12);
    assert!(pp.p_shift.iter().all(|&v| v == 0.0));

    // Matpower: b = 1/(x·τ); makeBdc injection ±b·shift at from/to.
    let mp = build_incidence(&view, DcConvention::Matpower).unwrap();
    let b_e = 1.0 / (x * tap);
    let shift_rad = shift_deg.to_radians();
    assert!((mp.b[0] - b_e).abs() < 1e-12, "b_e {} != {b_e}", mp.b[0]);
    assert!((mp.p_shift[0] - (-b_e * shift_rad)).abs() < 1e-12);
    assert!((mp.p_shift[1] - (b_e * shift_rad)).abs() < 1e-12);
}

#[test]
fn bundle_vectors_round_trip() {
    let case = load("../tests/data/case14.m");
    let dir = std::env::temp_dir().join("powerio_dcopf_vectors_test");
    let _ = std::fs::remove_dir_all(&dir);
    let out =
        powerio_matrix::write_dcopf_bundle(&case, &dir, &powerio_matrix::DcOpfOptions::default())
            .unwrap();

    // Default options are PaperPure + PerUnit; rebuild the instance to compare.
    let view = IndexedNetwork::new(&case);
    let inc = build_incidence(&view, DcConvention::PaperPure).unwrap();
    let opf = build_opf_instance(&view, &inc, Units::PerUnit).unwrap();

    let check = |name: &str, want: &[f64]| {
        let got = powerio_matrix::io::read_vector_mtx(out.dir.join(name)).unwrap();
        assert_eq!(got.len(), want.len(), "{name}: length");
        for (i, (&g, &w)) in got.iter().zip(want).enumerate() {
            assert!((g - w).abs() < 1e-9, "{name}[{i}]={g} != {w}");
        }
    };
    check("q.mtx", &opf.bus.q);
    check("c.mtx", &opf.bus.c);
    check("fmax.mtx", &opf.f_max);
    check("pd.mtx", &opf.bus.p_d);
    check("b.mtx", &inc.b);

    // Manifest agrees with the case.
    let meta: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out.dir.join("dcopf_meta.json")).unwrap())
            .unwrap();
    assert_eq!(meta["n_gen"].as_u64().unwrap() as usize, opf.n_gen());
    let ref_buses: Vec<usize> = meta["reference_buses"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap() as usize)
        .collect();
    assert_eq!(
        ref_buses,
        IndexedNetwork::new(&case).reference_bus_indices()
    );
    assert_eq!(meta["units"], "PerUnit");
    assert_eq!(meta["convention"], "PaperPure");

    let _ = std::fs::remove_dir_all(&dir);
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
    let lodf = dense(&build_lodf(&view, DcConvention::PaperPure).unwrap());
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
    let p = build_ptdf(&view, DcConvention::PaperPure).unwrap_err();
    assert!(
        matches!(p, Error::UngroundedComponent { components: 1 }),
        "ptdf: {p:?}"
    );
    let l = build_lodf(&view, DcConvention::PaperPure).unwrap_err();
    assert!(
        matches!(l, Error::UngroundedComponent { components: 1 }),
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
    let ptdf = dense(&build_ptdf(&view, DcConvention::PaperPure).unwrap());
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
    let ptdf = dense(&build_ptdf(&view, DcConvention::PaperPure).unwrap());
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
    let lodf = dense(&build_lodf(&view, DcConvention::PaperPure).unwrap());
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
    let ir = build_incidence(&IndexedNetwork::new(&raw), DcConvention::Matpower).unwrap();
    let in_ = build_incidence(&IndexedNetwork::new(&norm), DcConvention::Matpower).unwrap();
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
fn perunit_scales_native_by_base() {
    let case = load("../tests/data/case9.m");
    let base = case.base_mva;
    let native = opf_of(&case, Units::Native).unwrap();
    let pu = opf_of(&case, Units::PerUnit).unwrap();
    for i in 0..case.buses.len() {
        assert!(
            (pu.bus.q[i] - native.bus.q[i] * base * base).abs() < 1e-6,
            "q[{i}]"
        );
        assert!(
            (pu.bus.c[i] - native.bus.c[i] * base).abs() < 1e-6,
            "c[{i}]"
        );
        assert!(
            (pu.bus.pmax[i] - native.bus.pmax[i] / base).abs() < 1e-9,
            "pmax[{i}]"
        );
        assert!(
            (pu.bus.p_d[i] - native.bus.p_d[i] / base).abs() < 1e-9,
            "pd[{i}]"
        );
    }
}

#[test]
fn multi_generator_bus_sums_cost() {
    // Two in-service generators on bus 1; the bus-indexed vectors sum them.
    let case = net_with_gens(
        "twogen",
        vec![bus(1, BusType::Ref), bus(2, BusType::Pq)],
        vec![branch(1, 2, 0.1)],
        vec![poly_gen(1, 100.0, 1.0, 2.0), poly_gen(1, 50.0, 3.0, 4.0)],
    );
    let opf = opf_of(&case, Units::Native).unwrap();
    assert_eq!(opf.n_gen(), 2);
    let b0 = IndexedNetwork::new(&case).bus_index(BusId(1)).unwrap();
    assert!((opf.bus.q[b0] - (opf.gen_costs.q[0] + opf.gen_costs.q[1])).abs() < 1e-12);
    assert!((opf.bus.c[b0] - (opf.gen_costs.c[0] + opf.gen_costs.c[1])).abs() < 1e-12);
    assert!((opf.bus.pmax[b0] - (opf.gen_costs.pmax[0] + opf.gen_costs.pmax[1])).abs() < 1e-12);
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
        net_with_gens(
            name,
            vec![bus(1, BusType::Ref), bus(2, BusType::Pq)],
            vec![branch(1, 2, 0.1)],
            vec![gen_with_cost(1, cost)],
        )
    };

    // No cost row → MissingGenCost.
    assert!(matches!(
        opf_of(&case("nocost", None), Units::Native).unwrap_err(),
        Error::MissingGenCost { gen_index: 0 }
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
        Error::UnsupportedCostModel {
            gen_index: 0,
            model: 1,
            ..
        }
    ));
}
