use approx::assert_relative_eq;

use crate::indexed::IndexedNetwork;
use crate::matrix::{
    BuildOptions, DcConvention, MatrixStats, Scheme, build_bdoubleprime, build_bprime,
    build_incidence, build_lacpf, build_ybus,
};
use crate::network::{Branch, BranchCharging, Bus, BusId, BusType, Network, Shunt};
use crate::parse_psse;
use crate::pipeline::{MatrixKind, matrix_stats_for_kind};

fn bus(id: usize, kind: BusType) -> Bus {
    Bus::new(BusId(id), kind, 345.0)
}

fn br(from: usize, to: usize, r: f64, x: f64, b: f64) -> Branch {
    let mut branch = Branch::new(BusId(from), BusId(to), r, x);
    branch.b = b;
    branch
}

fn three_bus() -> Network {
    Network::in_memory(
        "tiny",
        100.0,
        vec![
            bus(1, BusType::Ref),
            bus(2, BusType::Pq),
            bus(3, BusType::Pq),
        ],
        vec![
            br(1, 2, 0.0, 0.1, 0.0),
            br(1, 3, 0.0, 0.2, 0.0),
            br(2, 3, 0.0, 0.25, 0.0),
        ],
    )
}

fn zero_impedance_bus_pair() -> Network {
    Network::in_memory(
        "zero",
        100.0,
        vec![bus(1, BusType::Ref), bus(2, BusType::Pq)],
        vec![br(1, 2, 0.0, 0.0, 0.0)],
    )
}

#[test]
fn three_winding_transformer_enters_the_matrices_and_connects_its_windings() {
    // Three buses (1 reference, 2 and 3 PQ) joined only by a 3-winding
    // transformer, no other branch. The indexed view star-lowers it, so the
    // windings land in one grounded component (plus the synthetic star point)
    // instead of three ungrounded islands, and the star branches scatter into B'.
    let raw = r"0, 100.00, 33, 0, 0, 60.00 / x
CASE
COMMENT
1,'B1          ', 230.0,3,1,1,1,1.00000,0.0,1.1,0.9,1.1,0.9
2,'B2          ', 138.0,1,1,1,1,1.00000,0.0,1.1,0.9,1.1,0.9
3,'B3          ', 13.8,1,1,1,1,1.00000,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
1, 2, 3, '1', 1, 1, 1, 0.0, 0.0, 2, 'T3W         ', 1, 1, 1, 0, 1, 0, 1, 0, 1, '            '
0.01, 0.10, 100.0, 0.02, 0.20, 100.0, 0.03, 0.30, 100.0, 0.98, -1.5
1.0, 230.0, 0.0, 100.0, 90.0, 80.0, 0, 0, 1.1, 0.9, 1.1, 0.9, 33, 0, 0, 0, 0
1.025, 138.0, 0.0, 110.0, 0, 0, 0, 0, 1.1, 0.9, 1.1, 0.9, 33, 0, 0, 0, 0
0.95, 13.8, 30.0, 50.0, 0, 0, 0, 0, 1.1, 0.9, 1.1, 0.9, 33, 0, 0, 0, 0
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
Q
";
    let net = parse_psse(raw).unwrap();
    assert!(net.branches.is_empty(), "a 3W is not folded into branches");
    assert_eq!(net.transformers_3w.len(), 1);

    let view = IndexedNetwork::new(&net);
    assert_eq!(view.n(), 4, "three buses plus the synthetic star point");
    assert_eq!(view.n_connected_components(), 1);
    // Before the star-lowering, buses 2 and 3 were ungrounded islands; now the
    // single component is grounded by the reference bus.
    view.check_reference_coverage().unwrap();

    let b = build_bprime(&view, &BuildOptions::default()).unwrap();
    assert_eq!(b.rows(), 4);
    assert_eq!(b.cols(), 4);

    // The canonical model is untouched: still three buses, no branches, one record.
    assert_eq!(net.buses.len(), 3);
    assert!(net.branches.is_empty());
    assert_eq!(net.transformers_3w.len(), 1);
}

#[test]
fn bprime_three_bus_has_correct_structure() {
    let net = three_bus();
    let view = IndexedNetwork::new(&net);
    let b = build_bprime(&view, &BuildOptions::default()).unwrap();
    assert_eq!(b.rows(), 3);
    assert_eq!(b.cols(), 3);

    // Branch 1-2: x=0.1 → 1/x = 10.0 → off-diag entry = -10.0 (BX with r=0
    // gives -x/(0+x²) = -1/x).
    // Diag of bus 1 = sum of incident edge-stiffnesses = 10 + 5 = 15.
    // Diag of bus 2 = 10 + 4 = 14.
    // Diag of bus 3 = 5 + 4 = 9.
    let dense = b.to_dense();
    assert_relative_eq!(dense[[0, 0]], 15.0, max_relative = 1e-12);
    assert_relative_eq!(dense[[1, 1]], 14.0, max_relative = 1e-12);
    assert_relative_eq!(dense[[2, 2]], 9.0, max_relative = 1e-12);
    assert_relative_eq!(dense[[0, 1]], -10.0, max_relative = 1e-12);
    assert_relative_eq!(dense[[1, 0]], -10.0, max_relative = 1e-12);
    assert_relative_eq!(dense[[0, 2]], -5.0, max_relative = 1e-12);
    assert_relative_eq!(dense[[1, 2]], -4.0, max_relative = 1e-12);
}

#[test]
fn bprime_is_symmetric_and_laplacian() {
    let net = three_bus();
    let view = IndexedNetwork::new(&net);
    let b = build_bprime(&view, &BuildOptions::default()).unwrap();
    let stats = MatrixStats::from_csr(&b);
    // M-matrix sign pattern, exactly singular Laplacian (diag = sum).
    assert!(stats.m_matrix_sign);
    assert_relative_eq!(stats.min_dd_margin, 0.0, epsilon = 1e-12);
    assert!(stats.min_diag > 0.0);
}

#[test]
fn bprime_ignores_out_of_service() {
    let mut net = three_bus();
    net.branches[0].in_service = false;
    let view = IndexedNetwork::new(&net);
    let b = build_bprime(&view, &BuildOptions::default()).unwrap();
    let dense = b.to_dense();
    // Bus 1 only connects via branch 1-3 (x=0.2 → 1/x=5)
    assert_relative_eq!(dense[[0, 0]], 5.0, max_relative = 1e-12);
    assert_relative_eq!(dense[[0, 1]], 0.0, max_relative = 1e-12);
}

#[test]
fn xb_and_bx_disagree_when_resistance_present() {
    let mut net = three_bus();
    for b in &mut net.branches {
        b.r = 0.05;
    }
    let view = IndexedNetwork::new(&net);
    let xb = build_bprime(
        &view,
        &BuildOptions {
            scheme: Scheme::Xb,
            ..Default::default()
        },
    )
    .unwrap();
    let bx = build_bprime(
        &view,
        &BuildOptions {
            scheme: Scheme::Bx,
            ..Default::default()
        },
    )
    .unwrap();
    let xb_dense = xb.to_dense();
    let bx_dense = bx.to_dense();
    // XB: -1/x = -10, BX: -x/(r²+x²) = -0.1/(0.0025+0.01) = -8.0
    assert_relative_eq!(xb_dense[[0, 1]], -10.0, max_relative = 1e-12);
    assert_relative_eq!(bx_dense[[0, 1]], -8.0, max_relative = 1e-12);
}

#[test]
fn bdoubleprime_with_shunts_is_strictly_dominant() {
    let mut net = three_bus();
    // Add capacitive shunts to break the singularity (negative bs → positive
    // contribution to −Im(Y_bus)).
    net.shunts = vec![
        Shunt::new(BusId(1), 0.0, -10.0),
        Shunt::new(BusId(2), 0.0, -10.0),
        Shunt::new(BusId(3), 0.0, -10.0),
    ];
    let view = IndexedNetwork::new(&net);
    let bpp = build_bdoubleprime(&view, &BuildOptions::default()).unwrap();
    let stats = MatrixStats::from_csr(&bpp);
    assert!(stats.min_dd_margin > 0.0, "expected strict dominance");
}

#[test]
fn ybus_reciprocity_and_symmetry() {
    // Without taps and shifts, Y_ij == Y_ji.
    let net = three_bus();
    let view = IndexedNetwork::new(&net);
    let parts = build_ybus(&view, &BuildOptions::default()).unwrap();
    let g = parts.g.to_dense();
    let b = parts.b.to_dense();
    for i in 0..3 {
        for j in 0..3 {
            assert_relative_eq!(g[[i, j]], g[[j, i]], epsilon = 1e-12);
            assert_relative_eq!(b[[i, j]], b[[j, i]], epsilon = 1e-12);
        }
    }
}

#[test]
fn ybus_uses_asymmetric_terminal_admittance() {
    let mut branch = br(1, 2, 0.0, 0.1, 0.0);
    branch.charging = Some(BranchCharging::new(0.01, 0.02, 0.03, 0.04));
    let net = Network::in_memory(
        "terminal-charging",
        100.0,
        vec![bus(1, BusType::Ref), bus(2, BusType::Pq)],
        vec![branch],
    );
    let view = IndexedNetwork::new(&net);
    let parts = build_ybus(&view, &BuildOptions::default()).unwrap();
    let g = parts.g.to_dense();
    let b = parts.b.to_dense();

    assert_relative_eq!(g[[0, 0]], 0.01, epsilon = 1e-12);
    assert_relative_eq!(g[[1, 1]], 0.03, epsilon = 1e-12);
    assert_relative_eq!(g[[0, 1]], 0.0, epsilon = 1e-12);
    assert_relative_eq!(g[[1, 0]], 0.0, epsilon = 1e-12);
    assert_relative_eq!(b[[0, 0]], -9.98, epsilon = 1e-12);
    assert_relative_eq!(b[[1, 1]], -9.96, epsilon = 1e-12);
    assert_relative_eq!(b[[0, 1]], 10.0, epsilon = 1e-12);
    assert_relative_eq!(b[[1, 0]], 10.0, epsilon = 1e-12);
}

#[test]
fn lacpf_block_is_2n_by_2n() {
    let net = three_bus();
    let view = IndexedNetwork::new(&net);
    let j = build_lacpf(&view, &BuildOptions::default()).unwrap();
    assert_eq!(j.rows(), 6);
    assert_eq!(j.cols(), 6);
}

#[test]
fn lacpf_blocks_equal_g_and_minus_b() {
    // LACPF is the 2n×2n block `[[G, -B], [-B, -G]]` from Y_bus = G + jB. Tie the
    // four n×n quadrants to build_ybus entrywise: a sign flip or a swapped block
    // (the one failure the 2n×2n shape check above cannot see) trips here.
    let net = three_bus();
    let view = IndexedNetwork::new(&net);
    let opts = BuildOptions::default();
    let ybus = build_ybus(&view, &opts).unwrap();
    let g = ybus.g.to_dense();
    let b = ybus.b.to_dense();
    let j = build_lacpf(&view, &opts).unwrap().to_dense();
    let n = 3;
    for r in 0..n {
        for c in 0..n {
            assert_relative_eq!(j[[r, c]], g[[r, c]], epsilon = 1e-12); // top-left = +G
            assert_relative_eq!(j[[r, c + n]], -b[[r, c]], epsilon = 1e-12); // top-right = -B
            assert_relative_eq!(j[[r + n, c]], -b[[r, c]], epsilon = 1e-12); // bottom-left = -B
            assert_relative_eq!(j[[r + n, c + n]], -g[[r, c]], epsilon = 1e-12); // bottom-right = -G
        }
    }
}

#[test]
fn ybus_tap_scales_from_diagonal_only() {
    // makeYbus puts |a|² (the tap magnitude squared) on the FROM-bus diagonal
    // only: Y[from,from] = (y + jb/2)/|a|², Y[to,to] = y + jb/2, off-diag = -y/a.
    // Reciprocity/symmetry tests cannot see this asymmetric scaling.
    let mut branch = br(1, 2, 0.0, 0.2, 0.0); // x = 0.2, r = 0, no line charging
    branch.tap = 1.25;
    let net = Network::in_memory(
        "tap2",
        100.0,
        vec![bus(1, BusType::Ref), bus(2, BusType::Pq)],
        vec![branch],
    );
    let view = IndexedNetwork::new(&net);
    let b = build_ybus(&view, &BuildOptions::default())
        .unwrap()
        .b
        .to_dense();
    // y = 1/(j·0.2) = -j5, so Im(y) = -5; |a|² = 1.5625; real tap ⇒ -y/a is j4.
    assert_relative_eq!(b[[0, 0]], -5.0 / 1.5625, max_relative = 1e-12); // -3.2
    assert_relative_eq!(b[[1, 1]], -5.0, max_relative = 1e-12);
    assert_relative_eq!(b[[0, 1]], 4.0, max_relative = 1e-12);
    assert_relative_eq!(b[[1, 0]], 4.0, max_relative = 1e-12);
}

#[test]
fn bprime_rejects_nan_reactance() {
    // A NaN reactance (the MATPOWER tokenizer accepts `NaN`) must error, not
    // write a non-finite entry that silently breaks the M-matrix/SDDM checks.
    let mut net = three_bus();
    net.branches[0].x = f64::NAN;
    let view = IndexedNetwork::new(&net);
    let err = build_bprime(&view, &BuildOptions::default()).unwrap_err();
    assert!(matches!(err, crate::Error::NonFiniteSusceptance { .. }));
}

#[test]
fn ybus_rejects_nan_reactance() {
    let mut net = three_bus();
    net.branches[0].x = f64::NAN;
    let view = IndexedNetwork::new(&net);
    let err = build_ybus(&view, &BuildOptions::default()).unwrap_err();
    assert!(matches!(err, crate::Error::NonFiniteSusceptance { .. }));
}

#[test]
fn zero_impedance_policy_is_shared_across_matrix_builders() {
    let net = zero_impedance_bus_pair();
    let view = IndexedNetwork::new(&net);
    let opts = BuildOptions::default();

    let bprime = build_bprime(&view, &opts).unwrap();
    let bprime_stats = matrix_stats_for_kind(&bprime, &view, MatrixKind::BPrime, &opts);
    assert_eq!(bprime_stats.skipped_zero_impedance, 1);
    assert_eq!(bprime_stats.skipped_zero_impedance_branches, vec![0]);

    let ybus = build_ybus(&view, &opts).unwrap();
    let ybus_stats = matrix_stats_for_kind(&ybus.b, &view, MatrixKind::YbusB, &opts);
    assert_eq!(ybus_stats.skipped_zero_impedance, 1);
    assert_eq!(ybus_stats.skipped_zero_impedance_branches, vec![0]);

    let inc = build_incidence(&view, DcConvention::PaperPure, &opts).unwrap();
    assert_eq!(inc.skipped_zero_impedance.count, 1);
    assert_eq!(inc.skipped_zero_impedance.branch_indices, vec![0]);
}

#[test]
fn zero_impedance_policy_can_error_instead_of_skipping() {
    let net = zero_impedance_bus_pair();
    let view = IndexedNetwork::new(&net);
    let opts = BuildOptions {
        skip_zero_impedance: false,
        ..Default::default()
    };

    let bprime = build_bprime(&view, &opts).unwrap_err();
    assert!(matches!(bprime, crate::Error::ZeroImpedance { row: 0 }));
    let ybus = build_ybus(&view, &opts).unwrap_err();
    assert!(matches!(ybus, crate::Error::ZeroImpedance { row: 0 }));
    let inc = build_incidence(&view, DcConvention::PaperPure, &opts).unwrap_err();
    assert!(matches!(inc, crate::Error::ZeroImpedance { row: 0 }));
}
