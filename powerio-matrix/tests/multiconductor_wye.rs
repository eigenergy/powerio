//! Ideal WYE winding constraints with explicit neutral-current balance.

use num_complex::Complex64;
use powerio_dist::{
    DistBus, DistShunt, DistSwitch, DistTransformer, DistWinding, DistWindingConn,
    MulticonductorNetwork, VoltageSource,
};
use powerio_matrix::{MulticonductorNodeIndex, NodeRef, calc_multiconductor_admittance_matrix};

fn names(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).into()).collect()
}

fn node(index: &MulticonductorNodeIndex, bus: &str, terminal: &str) -> usize {
    match index.resolve(bus, terminal).unwrap() {
        NodeRef::Node(n) => n,
        NodeRef::Ground => panic!("unexpected grounding of {bus}.{terminal}"),
    }
}

fn dense(matrix: &powerio_matrix::SparseMatrix) -> Vec<Vec<f64>> {
    let mut result = vec![vec![0.0; matrix.cols()]; matrix.rows()];
    for (row, entries) in matrix.outer_iterator().enumerate() {
        for (column, value) in entries.iter() {
            result[row][column] += value;
        }
    }
    result
}

fn floating_secondary() -> MulticonductorNetwork {
    let mut net = MulticonductorNetwork::new();
    net.buses_mut()
        .push(DistBus::new("p", names(&["1", "2", "3", "0"])));
    // Bus coordinate order must not determine winding phase order.
    net.buses_mut()
        .push(DistBus::new("s", names(&["n", "3", "1", "2"])));
    net.sources_mut().push(VoltageSource::new(
        "slack",
        "p",
        names(&["1", "2", "3"]),
        vec![2300.0; 3],
        vec![
            0.0,
            -std::f64::consts::TAU / 3.0,
            std::f64::consts::TAU / 3.0,
        ],
    ));
    net.transformers_mut().push(DistTransformer::new(
        "t",
        vec![
            DistWinding::new(
                "p",
                names(&["1", "2", "3", "0"]),
                DistWindingConn::Wye,
                4000.0,
                100_000.0,
            ),
            DistWinding::new(
                "s",
                names(&["1", "2", "3", "n"]),
                DistWindingConn::Wye,
                400.0,
                100_000.0,
            ),
        ],
        vec![0.0],
        3,
    ));
    net.shunts_mut().push(DistShunt::new(
        "unequal-ground-loads",
        "s",
        names(&["1", "2", "3"]),
        vec![
            vec![0.1, 0.0, 0.0],
            vec![0.0, 0.05, 0.0],
            vec![0.0, 0.0, 0.025],
        ],
        vec![vec![0.0; 3]; 3],
    ));
    net
}

#[test]
fn floating_star_preserves_voltage_displacement_kcl_and_power() {
    let net = floating_secondary();
    let system = calc_multiconductor_admittance_matrix(&net).unwrap();
    assert_eq!(system.diagnostics(), []);
    let index = system.index();
    let neutral_node = node(index, "s", "n");
    let angles = [
        0.0,
        -std::f64::consts::TAU / 3.0,
        std::f64::consts::TAU / 3.0,
    ];
    let coil = angles.map(|a| Complex64::from_polar(230.0, a));
    let conductance = [0.1, 0.05, 0.025];
    // Independently solve the floating star's KCL for its displacement.
    let neutral = -coil
        .iter()
        .zip(conductance)
        .map(|(v, g)| v * g)
        .sum::<Complex64>()
        / 0.175;
    assert!((neutral.re + 575.0 / 7.0).abs() < 1e-12);
    assert!((neutral.im - 115.0 * 3.0_f64.sqrt() / 7.0).abs() < 1e-12);
    let current = std::array::from_fn::<_, 3, _>(|i| (coil[i] + neutral) * conductance[i]);
    let mut voltage = vec![Complex64::default(); index.len()];
    voltage[neutral_node] = neutral;
    for (i, phase) in ["1", "2", "3"].iter().enumerate() {
        voltage[node(index, "p", phase)] = coil[i] * 10.0;
        voltage[node(index, "s", phase)] = coil[i] + neutral;
    }
    let augmented = system.augmented();
    assert_eq!(augmented.labels.len(), 6, "no neutral-to-ground constraint");
    let a = dense(&augmented.constraint_re);
    let g = dense(system.conductance());
    let mut ideal_currents = Vec::new();
    for (row, label) in augmented.labels.iter().enumerate() {
        let (phase, sign) = if let Some(phase) = label.strip_prefix("source:slack:") {
            (phase.parse::<usize>().unwrap() - 1, -1.0)
        } else {
            (
                label
                    .strip_prefix("transformer:t:")
                    .unwrap()
                    .parse::<usize>()
                    .unwrap(),
                1.0,
            )
        };
        ideal_currents.push(current[phase] * sign / 10.0);
        let lhs: Complex64 = a[row].iter().zip(&voltage).map(|(a, v)| a * v).sum();
        let rhs = Complex64::new(augmented.rhs_re[row], augmented.rhs_im[row]);
        assert!((lhs - rhs).norm() < 1e-10);
    }
    for (row, entries) in g.iter().enumerate() {
        let passive: Complex64 = entries.iter().zip(&voltage).map(|(g, v)| g * v).sum();
        let ideal: Complex64 = a.iter().zip(&ideal_currents).map(|(a, i)| a[row] * i).sum();
        assert!((passive + ideal).norm() < 1e-10, "KCL row {row}");
    }
    let supply: Complex64 = coil.iter().zip(current).map(|(v, i)| v * i.conj()).sum();
    let load: Complex64 = coil
        .iter()
        .zip(current)
        .map(|(v, i)| (v + neutral) * i.conj())
        .sum();
    assert!((supply - load).norm() < 1e-10);
    assert!(load.re > 5000.0 && load.im.abs() < 1e-10);
}

fn single_phase() -> MulticonductorNetwork {
    let mut net = MulticonductorNetwork::new();
    for bus in ["p", "s"] {
        net.buses_mut()
            .push(DistBus::new(bus, names(&["hot", "n", "0"])));
    }
    net.transformers_mut().push(DistTransformer::new(
        "t",
        vec![
            DistWinding::new(
                "p",
                names(&["hot", "n"]),
                DistWindingConn::Wye,
                7200.0,
                100_000.0,
            ),
            DistWinding::new(
                "s",
                names(&["hot", "n"]),
                DistWindingConn::Wye,
                240.0,
                100_000.0,
            ),
        ],
        vec![0.0],
        1,
    ));
    net
}

#[test]
fn both_neutrals_are_independent_and_fixed_taps_preserve_power() {
    let mut net = single_phase();
    net.transformers_mut()[0].windings[0].tap = 1.1;
    let system = calc_multiconductor_admittance_matrix(&net).unwrap();
    let index = system.index();
    let a = dense(&system.augmented().constraint_re);
    assert_eq!(a.len(), 1);
    let coeff = |bus, terminal| a[0][node(index, bus, terminal)];
    assert!((coeff("p", "hot") - 1.0).abs() < 1e-12);
    assert!((coeff("p", "n") + 1.0).abs() < 1e-12);
    assert!((coeff("s", "hot") + 33.0).abs() < 1e-12);
    assert!((coeff("s", "n") - 33.0).abs() < 1e-12);
    // Arbitrary independent common-mode offsets must not affect coil voltage.
    let values = [
        ("p", "hot", 7920.0 + 123.0),
        ("p", "n", 123.0),
        ("s", "hot", 240.0 - 47.0),
        ("s", "n", -47.0),
    ];
    let lhs: f64 = values.iter().map(|(b, t, v)| coeff(b, t) * v).sum();
    assert!(lhs.abs() < 1e-10);
    // The transpose gives [2,-2,-66,66] A: winding powers cancel exactly.
    let watts: f64 = values.iter().map(|(b, t, v)| coeff(b, t) * 2.0 * v).sum();
    assert!(watts.abs() < 1e-10);
}

#[test]
fn explicit_and_implicit_grounded_single_phase_maps_agree() {
    let mut net = single_phase();
    net.transformers_mut()[0].windings[0].terminal_map = names(&["hot"]);
    net.transformers_mut()[0].windings[1].terminal_map = names(&["hot", "0"]);
    let implicit = calc_multiconductor_admittance_matrix(&net).unwrap();
    net.transformers_mut()[0].windings[0].terminal_map = names(&["hot", "0"]);
    let explicit = calc_multiconductor_admittance_matrix(&net).unwrap();
    assert_eq!(
        implicit.augmented().constraint_re,
        explicit.augmented().constraint_re
    );
    assert_eq!(explicit.augmented().labels.len(), 1);
}

#[test]
fn a_grounding_switch_removes_the_neutral_unknown_without_extra_constraints() {
    let mut net = floating_secondary();
    net.buses_mut()[1].terminals.push("0".into());
    net.switches_mut().push(DistSwitch::new(
        "earth",
        "s",
        "s",
        names(&["n"]),
        names(&["0"]),
        true,
    ));
    let floating = calc_multiconductor_admittance_matrix(&net).unwrap();
    assert!(matches!(
        floating.index().resolve("s", "n"),
        Some(NodeRef::Node(_))
    ));
    net.switches_mut()[0].open = false;
    let grounded = calc_multiconductor_admittance_matrix(&net).unwrap();
    assert_eq!(grounded.index().resolve("s", "n"), Some(NodeRef::Ground));
    assert_eq!(floating.index().len(), grounded.index().len() + 1);
    assert_eq!(floating.augmented().labels, grounded.augmented().labels);
    let a = dense(&grounded.augmented().constraint_re);
    for (row, label) in grounded.augmented().labels.iter().enumerate() {
        if let Some(phase) = label.strip_prefix("transformer:t:") {
            let terminal = (phase.parse::<usize>().unwrap() + 1).to_string();
            assert!((a[row][node(grounded.index(), "p", &terminal)] - 1.0).abs() < 1e-12);
            assert!((a[row][node(grounded.index(), "s", &terminal)] + 10.0).abs() < 1e-12);
        }
    }
}

#[test]
fn merged_unity_windings_do_not_create_a_redundant_constraint() {
    let mut net = single_phase();
    net.transformers_mut()[0].windings[0].v_ref = 240.0;
    net.switches_mut().push(DistSwitch::new(
        "merge",
        "p",
        "s",
        names(&["hot", "n"]),
        names(&["hot", "n"]),
        false,
    ));
    let system = calc_multiconductor_admittance_matrix(&net).unwrap();
    assert_eq!(system.augmented().labels.as_slice(), []);
    assert_eq!(system.augmented().constraint_re.nnz(), 0);
    net.transformers_mut()[0].windings[0].v_ref = 480.0;
    let system = calc_multiconductor_admittance_matrix(&net).unwrap();
    assert_eq!(system.augmented().labels.len(), 1);
    assert_eq!(system.augmented().constraint_re.nnz(), 2);
}

#[test]
fn incomplete_windings_ratios_and_additional_physics_are_rejected() {
    for case in 0..11 {
        let mut net = single_phase();
        let t = &mut net.transformers_mut()[0];
        match case {
            0 => t.phases = 3,
            1 => t.xsc_pct.clear(),
            2 => t.windings[0].terminal_map.clear(),
            3 => t.windings[0].v_ref = f64::NAN,
            4 => t.windings[1].tap = 0.0,
            5 => t.windings[0].r_neutral = Some(0.0),
            6 => t.windings[0].x_neutral = Some(1.0),
            7 => t.windings[0].conn = DistWindingConn::Delta,
            8 => t.windings[0].v_ref = f64::MAX,
            9 => t.windings[0].r_pct = 1.0,
            _ => {
                t.extras
                    .insert("g_no_load".into(), serde_json::json!("invalid"));
            }
        }
        if case == 8 {
            t.windings[1].v_ref = f64::MIN_POSITIVE;
        }
        let error = calc_multiconductor_admittance_matrix(&net).unwrap_err();
        assert_eq!(
            error.code().code,
            "BUILD.MULTI.PHYSICS_UNSUPPORTED",
            "case {case}"
        );
    }
}
