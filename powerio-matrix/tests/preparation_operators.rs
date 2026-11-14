//! The operators a DC or AC preparation derives from the arrays it already
//! holds agree with the network level builders: the incidence, Laplacian and
//! branch flow matrix of the DC OPF matrices and the DC operators, the bus
//! admittance matrix and pi model blocks of `calc_admittance_matrix`, and the
//! power flow Jacobian of `calc_power_flow_jacobian`.
// k/m/n, G/B and V are the textbook notation throughout this suite.
#![allow(clippy::many_single_char_names)]
// The dense reference sums index square arrays by both loop variables.
#![allow(clippy::needless_range_loop)]

use num_complex::Complex64;
use powerio_core::Source;
use powerio_matrix::{
    AcOpfAssemblyOptions, AcPfAssemblyOptions, BuildOptions, DcOperators, DcOpfAssemblyOptions,
    Grounding, IndexedNetwork, PowerFlowJacobian, SparseMatrix, VoltageCoordinates,
    build_ac_opf_preparation, build_ac_pf_preparation, build_dc_opf_preparation,
    calc_admittance_matrix, calc_bus_power_injection, calc_dc_opf_matrices,
    calc_power_flow_jacobian, calc_solver_branch_flow_matrix, calc_weighted_laplacian,
    ground_at_each,
};
use powerio_prob::{
    AcOpfInstance, AcPfInstance, BalancedOperatingPointBuilder, DcOpfInstance, DcPfInstance,
};
use powerio_tx::BalancedNetwork;

const CASES: [&str; 6] = [
    "case9.m",
    "case14.m",
    "case30.m",
    "case118.m",
    "t_case9_oos.m",
    "pglib/pglib_opf_case5_pjm.m",
];

fn case(name: &str) -> BalancedNetwork {
    let path = format!("{}/../tests/data/{name}", env!("CARGO_MANIFEST_DIR"));
    powerio_tx::parse(Source::open(path).unwrap())
        .expect("case parses")
        .into_value()
}

fn assert_same_matrix(left: &SparseMatrix, right: &SparseMatrix, what: &str) {
    assert_eq!(left.shape(), right.shape(), "{what}: shape");
    assert_eq!(left.indptr(), right.indptr(), "{what}: row pointers");
    assert_eq!(left.indices(), right.indices(), "{what}: column indices");
    assert_eq!(left.data(), right.data(), "{what}: values");
}

fn off_flat(n: usize) -> (Vec<f64>, Vec<f64>) {
    let vm = (0..n)
        .map(|k| 1.0 + 0.01 * (k as f64) / (n as f64))
        .collect();
    let va = (0..n)
        .map(|k| (2.0 * (k as f64) / (n as f64)).to_radians())
        .collect();
    (vm, va)
}

#[test]
fn preparation_incidence_is_the_dc_opf_matrices_incidence() {
    for name in CASES {
        let instance = DcOpfInstance::from_network(case(name)).unwrap();
        let options = DcOpfAssemblyOptions::default();
        let preparation = build_dc_opf_preparation(&instance, &options).unwrap();
        let matrices = calc_dc_opf_matrices(&instance, &options).unwrap();
        let incidence = preparation.calc_bus_branch_incidence();
        assert_same_matrix(&incidence, &matrices.bus_branch_incidence, name);
        assert_eq!(
            incidence.shape(),
            (preparation.n_buses, preparation.n_branches())
        );

        // Row `i` lists the (branch, sign) pairs at bus `i`, ascending.
        for (bus, row) in incidence.outer_iterator().enumerate() {
            let mut last = None;
            for (branch, &sign) in row.iter() {
                assert!(last < Some(branch), "{name}: ascending branch order");
                last = Some(branch);
                let expected = if preparation.branches.from_bus[branch] == bus {
                    1.0
                } else {
                    assert_eq!(preparation.branches.to_bus[branch], bus);
                    -1.0
                };
                assert_eq!(
                    sign, expected,
                    "{name}: sign of branch {branch} at bus {bus}"
                );
            }
        }

        let again = preparation.calc_matrices();
        assert_same_matrix(&again.laplacian, &matrices.laplacian, name);
        assert_same_matrix(
            &again.grounded_laplacian,
            &matrices.grounded_laplacian,
            name,
        );
        assert_same_matrix(
            &again.branch_flow_matrix,
            &matrices.branch_flow_matrix,
            name,
        );
        assert_same_matrix(&again.generator_bus, &matrices.generator_bus, name);
        assert_eq!(again.reference_selector, matrices.reference_selector);

        // The public pieces rebuild the same Laplacian and flow matrix.
        let weights = &preparation.branches.susceptance_magnitude;
        assert_same_matrix(
            &calc_weighted_laplacian(&incidence, weights),
            &matrices.laplacian,
            name,
        );
        assert_same_matrix(
            &calc_solver_branch_flow_matrix(&incidence, weights),
            &matrices.branch_flow_matrix,
            name,
        );
    }
}

/// The preparation's incidence is the transpose of the DC operators'
/// PowerModels incidence when both carry the same branch columns.
#[test]
fn preparation_incidence_matches_the_dc_operators() {
    let mut compared = 0;
    for name in CASES {
        let network = case(name);
        let preparation = build_dc_opf_preparation(
            &DcOpfInstance::from_network(network.clone()).unwrap(),
            &DcOpfAssemblyOptions::default(),
        )
        .unwrap();
        let operators = DcOperators::build(&DcPfInstance::from_network(network).unwrap()).unwrap();
        if operators.branch_rows() != preparation.branches.analysis_rows.as_slice()
            || operators.bus_ids() != preparation.bus_ids.as_slice()
        {
            continue;
        }
        let powermodels = operators.calc_incidence_matrix();
        let transposed = powermodels.transpose_view().to_csr();
        assert_same_matrix(&preparation.calc_bus_branch_incidence(), &transposed, name);
        compared += 1;
    }
    assert!(
        compared >= 4,
        "only {compared} cases share the column layout"
    );
}

#[test]
fn preparation_branch_flow_is_the_flow_matrix_plus_offset() {
    for name in CASES {
        let instance = DcOpfInstance::from_network(case(name)).unwrap();
        let preparation =
            build_dc_opf_preparation(&instance, &DcOpfAssemblyOptions::default()).unwrap();
        let theta: Vec<f64> = (0..preparation.n_buses)
            .map(|k| 0.01 * (k as f64).sin())
            .collect();
        let flow = preparation.calc_branch_flow(&theta);
        let matrices = preparation.calc_matrices();
        let offset = preparation.calc_branch_flow_offset();
        for (branch, row) in matrices.branch_flow_matrix.outer_iterator().enumerate() {
            let product: f64 = row.iter().map(|(bus, &value)| value * theta[bus]).sum();
            let expected = product + offset[branch];
            assert!(
                (flow[branch] - expected).abs() <= 1e-12 * expected.abs().max(1.0),
                "{name}: branch {branch} flow {} vs {expected}",
                flow[branch]
            );
        }
    }
}

/// A solve over `ground_at_each` scatters back through the same grounding.
#[test]
fn grounding_maps_a_grounded_laplacian_back_to_full_order() {
    let instance = DcOpfInstance::from_network(case("case14.m")).unwrap();
    let preparation =
        build_dc_opf_preparation(&instance, &DcOpfAssemblyOptions::default()).unwrap();
    let laplacian = preparation.calc_matrices().laplacian;
    let references = preparation.reference_buses.as_ref().to_vec();
    let grounded = ground_at_each(&laplacian, &references);
    let grounding = Grounding::new(&references);
    let full = grounding.full_of_reduced(preparation.n_buses);
    assert_eq!(full.len(), grounded.rows());
    for (reduced_row, row) in grounded.outer_iterator().enumerate() {
        for (reduced_column, &value) in row.iter() {
            assert_eq!(
                laplacian.get(full[reduced_row], full[reduced_column]),
                Some(&value)
            );
            assert_eq!(
                grounding.reduced(full[reduced_column]),
                Some(reduced_column)
            );
        }
    }
    for &reference in &references {
        assert_eq!(grounding.reduced(reference), None);
    }
}

#[test]
fn preparation_admittance_matches_the_network_admittance() {
    let mut compared = 0;
    for name in CASES {
        let network = case(name);
        let view = IndexedNetwork::new(&network);
        let preparation = build_ac_opf_preparation(
            &AcOpfInstance::from_network(network.clone()).unwrap(),
            &AcOpfAssemblyOptions::default(),
        )
        .unwrap();
        let ours = preparation.calc_admittance_matrix();
        assert_eq!(ours.g.shape(), (preparation.n_buses, preparation.n_buses));
        for k in 0..preparation.n_buses {
            assert!(ours.g.get(k, k).is_some(), "{name}: G diagonal {k} stored");
            assert!(ours.b.get(k, k).is_some(), "{name}: B diagonal {k} stored");
        }
        if preparation.n_buses != view.n() {
            continue;
        }
        let theirs = calc_admittance_matrix(&view, &BuildOptions::default()).unwrap();
        for (left, right, part) in [(&ours.g, &theirs.g, "G"), (&ours.b, &theirs.b, "B")] {
            for (row, row_vec) in left.outer_iterator().enumerate() {
                for (column, &value) in row_vec.iter() {
                    let expected = right.get(row, column).copied().unwrap_or(0.0);
                    assert!(
                        (value - expected).abs() <= 1e-12 * expected.abs().max(1.0),
                        "{name}: {part}[{row},{column}] {value} vs {expected}"
                    );
                }
            }
            for (&value, (row, column)) in right {
                assert!(
                    left.get(row, column).is_some() || value == 0.0,
                    "{name}: {part}[{row},{column}] missing"
                );
            }
        }
        compared += 1;
    }
    assert!(compared >= 4, "only {compared} cases share the bus axis");
}

#[test]
fn preparation_branch_flows_match_the_pi_model() {
    for name in CASES {
        let network = case(name);
        let preparation = build_ac_opf_preparation(
            &AcOpfInstance::from_network(network).unwrap(),
            &AcOpfAssemblyOptions::default(),
        )
        .unwrap();
        let branches = &preparation.branches;
        let (vm, va) = off_flat(preparation.n_buses);
        let flows = branches.calc_branch_flows(&vm, &va);
        assert_eq!(flows.pf.len(), branches.len());
        for branch in 0..branches.len() {
            let (f, t) = (branches.from_bus[branch], branches.to_bus[branch]);
            let y = Complex64::new(branches.g[branch], branches.b[branch]);
            let a = Complex64::from_polar(branches.tap[branch], branches.shift[branch]);
            let y_fr = Complex64::new(branches.g_fr[branch], branches.b_fr[branch]);
            let y_to = Complex64::new(branches.g_to[branch], branches.b_to[branch]);
            let vf = Complex64::from_polar(vm[f], va[f]);
            let vt = Complex64::from_polar(vm[t], va[t]);
            let i_f = (y + y_fr) / (a * a.conj()) * vf - y / a.conj() * vt;
            let i_t = -y / a * vf + (y + y_to) * vt;
            let s_f = vf * i_f.conj();
            let s_t = vt * i_t.conj();
            for (value, expected) in [
                (flows.pf[branch], s_f.re),
                (flows.qf[branch], s_f.im),
                (flows.pt[branch], s_t.re),
                (flows.qt[branch], s_t.im),
            ] {
                assert!(
                    (value - expected).abs() <= 1e-10 * expected.abs().max(1.0),
                    "{name}: branch {branch} flow {value} vs {expected}"
                );
            }
        }
    }
}

#[test]
fn clearing_jumper_charging_keeps_the_series_admittance() {
    let mut network = case("case9.m");
    network.branches_mut()[0].r = 0.0;
    network.branches_mut()[0].x = 1e-6;
    network.branches_mut()[0].b = 0.2;
    let mut preparation = build_ac_pf_preparation(
        &AcPfInstance::from_network(network).unwrap(),
        &AcPfAssemblyOptions::default(),
    )
    .unwrap();
    let column = preparation
        .branches
        .analysis_rows
        .iter()
        .position(|&row| row == 0)
        .unwrap();
    let series = (
        preparation.branches.g[column],
        preparation.branches.b[column],
    );
    assert!(preparation.branches.b_fr[column] != 0.0);
    let cleared = preparation.branches.clear_jumper_charging(1e-10);
    assert_eq!(cleared, vec![column]);
    assert_eq!(preparation.branches.b_fr[column], 0.0);
    assert_eq!(preparation.branches.b_to[column], 0.0);
    assert_eq!(
        (
            preparation.branches.g[column],
            preparation.branches.b[column]
        ),
        series
    );
    // Every other branch keeps its charging.
    assert!(
        preparation
            .branches
            .b_fr
            .iter()
            .enumerate()
            .any(|(other, &b)| other != column && b != 0.0)
    );
}

fn operating_point(
    network: &BalancedNetwork,
    vm: &[f64],
    va: &[f64],
) -> powerio_prob::OperatingPoint<BalancedNetwork> {
    BalancedOperatingPointBuilder::new(
        network.clone(),
        vec![powerio_core::TimePoint::new("0", None).unwrap()],
    )
    .bus_voltage_magnitudes(vm.to_vec())
    .bus_voltage_angles(va.to_vec())
    .build()
    .unwrap()
    .get(0)
    .unwrap()
    .clone()
}

/// Over the network's own admittance and voltages, the slice entry point and
/// the instance entry point fill the same matrix, and a voltage refresh
/// equals a rebuild.
#[test]
fn jacobian_from_admittance_matches_the_instance_jacobian() {
    for name in ["case9.m", "case14.m", "case118.m"] {
        let network = case(name);
        let view = IndexedNetwork::new(&network);
        let n = view.n();
        let bus_ids: Vec<_> = (0..n).map(|k| view.bus_id(k)).collect();
        let (vm, va) = off_flat(n);
        let instance = AcPfInstance::from_network(network.clone()).unwrap();
        for coordinates in [VoltageCoordinates::Polar, VoltageCoordinates::Cartesian] {
            let reference = calc_power_flow_jacobian(
                &instance,
                &operating_point(&network, &vm, &va),
                coordinates,
            )
            .unwrap();
            let admittance = calc_admittance_matrix(&view, &BuildOptions::default()).unwrap();
            let mut ours = PowerFlowJacobian::from_admittance(
                &admittance,
                bus_ids.clone(),
                coordinates,
                &vm,
                &va,
            )
            .unwrap();
            assert_same_matrix(ours.matrix(), reference.matrix(), name);
            assert_eq!(ours.bus_ids(), reference.bus_ids());

            let vm2: Vec<f64> = vm.iter().map(|v| v * 0.98).collect();
            let va2: Vec<f64> = va.iter().map(|a| a * 1.5).collect();
            ours.update_from_voltages(&vm2, &va2).unwrap();
            let rebuilt = PowerFlowJacobian::from_admittance(
                &admittance,
                bus_ids.clone(),
                coordinates,
                &vm2,
                &va2,
            )
            .unwrap();
            assert_same_matrix(ours.matrix(), rebuilt.matrix(), name);
            assert!(ours.update_from_voltages(&vm2[1..], &va2).is_err());
            // An admittance-built Jacobian has no instance to pair a point with.
            assert!(
                ours.update(&instance, &operating_point(&network, &vm, &va))
                    .is_err()
            );
        }
    }
}

#[test]
fn bus_power_injection_is_v_conj_y_v() {
    let network = case("case14.m");
    let preparation = build_ac_pf_preparation(
        &AcPfInstance::from_network(network).unwrap(),
        &AcPfAssemblyOptions::default(),
    )
    .unwrap();
    let admittance = preparation.calc_admittance_matrix();
    let n = preparation.n_buses;
    let (vm, va) = off_flat(n);
    let (p, q) = calc_bus_power_injection(&admittance, &vm, &va).unwrap();
    let v: Vec<Complex64> = (0..n)
        .map(|k| Complex64::from_polar(vm[k], va[k]))
        .collect();
    for k in 0..n {
        let mut current = Complex64::new(0.0, 0.0);
        for m in 0..n {
            let g = admittance.g.get(k, m).copied().unwrap_or(0.0);
            let b = admittance.b.get(k, m).copied().unwrap_or(0.0);
            current += Complex64::new(g, b) * v[m];
        }
        let s = v[k] * current.conj();
        assert!((p[k] - s.re).abs() < 1e-10, "p[{k}]");
        assert!((q[k] - s.im).abs() < 1e-10, "q[{k}]");
    }
    // The branch flows plus the shunt draw balance the injection at each bus.
    let flows = preparation.branches.calc_branch_flows(&vm, &va);
    let mut balance: Vec<Complex64> = (0..n)
        .map(|k| {
            Complex64::new(preparation.buses.g_s[k], -preparation.buses.b_s[k]) * vm[k] * vm[k]
        })
        .collect();
    for branch in 0..preparation.branches.len() {
        balance[preparation.branches.from_bus[branch]] +=
            Complex64::new(flows.pf[branch], flows.qf[branch]);
        balance[preparation.branches.to_bus[branch]] +=
            Complex64::new(flows.pt[branch], flows.qt[branch]);
    }
    for k in 0..n {
        assert!((balance[k].re - p[k]).abs() < 1e-10, "P balance at {k}");
        assert!((balance[k].im - q[k]).abs() < 1e-10, "Q balance at {k}");
    }
    let labels = preparation.calc_island_labels();
    assert_eq!(labels.len(), n);
    assert!(labels.iter().all(|&label| label == 0));
}
