use num_complex::Complex64;
use powerio_dist::{DistBus, DistShunt, DistSwitch, MulticonductorNetwork, VoltageSource};
use powerio_matrix::{NodeRef, calc_multiconductor_admittance_matrix};

fn names(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| (*s).into()).collect()
}

fn network() -> MulticonductorNetwork {
    let mut net = MulticonductorNetwork::new();
    net.buses_mut()
        .push(DistBus::new("b", names(&["n", "3", "1", "2"])));
    net.sources_mut().push(
        VoltageSource::new(
            "s",
            "b",
            names(&["1", "2", "3"]),
            vec![230.0; 3],
            vec![
                0.0,
                -std::f64::consts::TAU / 3.0,
                std::f64::consts::TAU / 3.0,
            ],
        )
        .with_reference_terminal("n"),
    );
    net.shunts_mut().push(DistShunt::new(
        "loads",
        "b",
        names(&["1", "2", "3"]),
        vec![
            vec![0.01, 0.0, 0.0],
            vec![0.0, 0.02, 0.0],
            vec![0.0, 0.0, 0.04],
        ],
        vec![vec![0.0; 3]; 3],
    ));
    net
}

#[test]
fn referenced_source_matrix_matches_neutral_displacement_and_nodal_kcl() {
    let net = network();
    let system = calc_multiconductor_admittance_matrix(&net).unwrap();
    assert_eq!(system.diagnostics(), []);
    let row = |terminal: &str| match system.index().resolve("b", terminal).unwrap() {
        NodeRef::Node(n) => n,
        NodeRef::Ground => panic!("unexpected earth"),
    };
    let source = &net.sources()[0];
    let emf: Vec<_> = source
        .v_angle
        .iter()
        .map(|a| Complex64::from_polar(230.0, *a))
        .collect();
    let g = [0.01, 0.02, 0.04];
    let star = -emf.iter().zip(g).map(|(e, g)| e * g).sum::<Complex64>() / 0.07;
    assert!(star.norm() > 50.0);
    let mut voltage = vec![Complex64::default(); system.index().len()];
    voltage[row("n")] = star;
    let mut currents = Vec::new();
    for (i, terminal) in ["1", "2", "3"].iter().enumerate() {
        voltage[row(terminal)] = emf[i] + star;
        currents.push(-(emf[i] + star) * g[i]);
    }
    assert!(currents.iter().sum::<Complex64>().norm() < 1e-12);
    let a = &system.augmented().constraint_re;
    assert_eq!(a.rows(), 3);
    assert_eq!(a.nnz(), 6);
    for (constraint, entries) in a.outer_iterator().enumerate() {
        let lhs = entries
            .iter()
            .map(|(j, x)| voltage[j] * x)
            .sum::<Complex64>();
        let rhs = Complex64::new(
            system.augmented().rhs_re[constraint],
            system.augmented().rhs_im[constraint],
        );
        assert!((lhs - rhs).norm() < 1e-10);
    }
    for (node, entries) in system.conductance().outer_iterator().enumerate() {
        let passive = entries
            .iter()
            .map(|(j, x)| voltage[j] * x)
            .sum::<Complex64>();
        let injected: Complex64 = currents
            .iter()
            .enumerate()
            .map(|(i, current)| current * a.get(i, node).copied().unwrap_or(0.0))
            .sum();
        assert!((passive + injected).norm() < 1e-10, "KCL at {node}");
    }
}

#[test]
fn grounded_phase_still_constrains_the_reference_and_grounded_reference_matches_earth() {
    let mut net = network();
    net.buses_mut()[0].grounded.push("1".into());
    let system = calc_multiconductor_admittance_matrix(&net).unwrap();
    let NodeRef::Node(star) = system.index().resolve("b", "n").unwrap() else {
        panic!()
    };
    assert_eq!(system.augmented().constraint_re.get(0, star), Some(&-1.0));
    assert_eq!(
        system
            .augmented()
            .constraint_re
            .outer_view(0)
            .unwrap()
            .nnz(),
        1
    );
    assert_eq!(system.augmented().rhs_re[0].to_bits(), 230.0_f64.to_bits());

    let mut net = network();
    net.buses_mut()[0].grounded.push("n".into());
    let referenced = calc_multiconductor_admittance_matrix(&net).unwrap();
    net.sources_mut()[0].reference_terminal = None;
    let earth = calc_multiconductor_admittance_matrix(&net).unwrap();
    assert_eq!(
        referenced.augmented().constraint_re,
        earth.augmented().constraint_re
    );
    assert_eq!(referenced.augmented().rhs_re, earth.augmented().rhs_re);
}

#[test]
fn a_closed_switch_cannot_short_a_nonzero_source_constraint() {
    let mut net = network();
    net.switches_mut().push(DistSwitch::new(
        "short",
        "b",
        "b",
        names(&["1"]),
        names(&["n"]),
        false,
    ));
    let error = calc_multiconductor_admittance_matrix(&net).unwrap_err();
    assert!(error.to_string().contains("electrically identical"));
}
