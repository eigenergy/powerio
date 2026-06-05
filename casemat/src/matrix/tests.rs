use approx::assert_relative_eq;

use crate::case::{Branch, Bus, BusType, MpcCase};
use crate::matrix::{
    build_bdoubleprime, build_bprime, build_lacpf, build_ybus, BuildOptions, MatrixStats, Scheme,
};

fn bus(id: usize, kind: BusType, gs: f64, bs: f64) -> Bus {
    Bus {
        id,
        kind,
        pd: 0.0,
        qd: 0.0,
        gs,
        bs,
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

fn br(from: usize, to: usize, r: f64, x: f64, b: f64) -> Branch {
    Branch {
        from_id: from,
        to_id: to,
        r,
        x,
        b,
        rate_a: 0.0,
        rate_b: 0.0,
        rate_c: 0.0,
        tap: 0.0,
        shift: 0.0,
        status: 1.0,
        angmin: -360.0,
        angmax: 360.0,
    }
}

fn three_bus() -> MpcCase {
    MpcCase::new(
        "tiny",
        100.0,
        vec![
            bus(1, BusType::Ref, 0.0, 0.0),
            bus(2, BusType::Pq, 0.0, 0.0),
            bus(3, BusType::Pq, 0.0, 0.0),
        ],
        vec![
            br(1, 2, 0.0, 0.1, 0.0),
            br(1, 3, 0.0, 0.2, 0.0),
            br(2, 3, 0.0, 0.25, 0.0),
        ],
    )
}

#[test]
fn bprime_three_bus_has_correct_structure() {
    let case = three_bus();
    let b = build_bprime(&case, &BuildOptions::default()).unwrap();
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
    let case = three_bus();
    let b = build_bprime(&case, &BuildOptions::default()).unwrap();
    let stats = MatrixStats::from_csr(&b);
    // M-matrix sign pattern, exactly singular Laplacian (diag = sum).
    assert!(stats.m_matrix_sign);
    assert_relative_eq!(stats.min_dd_margin, 0.0, epsilon = 1e-12);
    assert!(stats.min_diag > 0.0);
}

#[test]
fn bprime_ignores_out_of_service() {
    let mut case = three_bus();
    case.branches[0].status = 0.0;
    let b = build_bprime(&case, &BuildOptions::default()).unwrap();
    let dense = b.to_dense();
    // Bus 1 only connects via branch 1-3 (x=0.2 → 1/x=5)
    assert_relative_eq!(dense[[0, 0]], 5.0, max_relative = 1e-12);
    assert_relative_eq!(dense[[0, 1]], 0.0, max_relative = 1e-12);
}

#[test]
fn xb_and_bx_disagree_when_resistance_present() {
    let mut case = three_bus();
    for b in &mut case.branches {
        b.r = 0.05;
    }
    let xb = build_bprime(
        &case,
        &BuildOptions {
            scheme: Scheme::Xb,
            ..Default::default()
        },
    )
    .unwrap();
    let bx = build_bprime(
        &case,
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
    let mut case = three_bus();
    // Add capacitive shunts to break the singularity.
    case.buses[0].bs = -10.0; // negative bs → -bs/baseMVA > 0 contribution
    case.buses[1].bs = -10.0;
    case.buses[2].bs = -10.0;
    let bpp = build_bdoubleprime(&case, &BuildOptions::default()).unwrap();
    let stats = MatrixStats::from_csr(&bpp);
    assert!(stats.min_dd_margin > 0.0, "expected strict dominance");
}

#[test]
fn ybus_reciprocity_and_symmetry() {
    // Without taps and shifts, Y_ij == Y_ji.
    let case = three_bus();
    let parts = build_ybus(&case, &BuildOptions::default()).unwrap();
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
fn lacpf_block_is_2n_by_2n() {
    let case = three_bus();
    let j = build_lacpf(&case, &BuildOptions::default()).unwrap();
    assert_eq!(j.rows(), 6);
    assert_eq!(j.cols(), 6);
}
