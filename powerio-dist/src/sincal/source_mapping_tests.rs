use num_complex::Complex64;
use powerio_core::Source;

use super::{
    infeeder_tests::infeeder_database,
    read,
    schema::NativeDatabase,
    source_mapping::{IdealSourceCircuit, VoltageReference},
};
use crate::DistBus;

fn circuit(edit: &str) -> IdealSourceCircuit {
    let db = NativeDatabase::decode(&infeeder_database(edit), None).unwrap();
    db.infeeder_input(30)
        .unwrap()
        .ideal_boundary_circuit(
            &DistBus::new("10", ["3", "n", "1", "2"].map(str::to_owned).to_vec()),
            400.0,
        )
        .unwrap()
}

fn near(actual: Complex64, expected: Complex64) {
    assert!((actual - expected).norm() < 1e-9, "{actual} != {expected}");
}

/// Test-only dense MNA solve. Stamp the actual generated incidence rows and
/// their transpose, then compare with an independently derived neutral-shift
/// formula. No native stored result or guessed SINCAL impedance is used.
fn solve(
    source: &IdealSourceCircuit,
    phase_admittance: [Complex64; 3],
    star_admittance: Complex64,
) -> Option<Vec<Complex64>> {
    let terminals = &source.bus.terminals;
    let nodes = terminals.len();
    let size = nodes + 3;
    let mut matrix = vec![vec![Complex64::default(); size + 1]; size];
    for (phase, admittance) in phase_admittance.into_iter().enumerate() {
        matrix[phase][phase] = admittance;
    }
    if let VoltageReference::Terminal(reference) = &source.boundary.reference {
        let star = terminals.iter().position(|t| t == reference).unwrap();
        matrix[star][star] = star_admittance;
    } else {
        assert_eq!(star_admittance, Complex64::default());
    }
    for (row, equation) in source.boundary.equations().iter().enumerate() {
        let constraint = nodes + row;
        for (terminal, coefficient) in [
            (Some(&equation.positive), 1.0),
            (equation.negative.as_ref(), -1.0),
        ] {
            if let Some(terminal) = terminal {
                let column = terminals.iter().position(|t| t == terminal).unwrap();
                matrix[constraint][column] += coefficient;
                matrix[column][constraint] += coefficient;
            }
        }
        matrix[constraint][size] = equation.voltage;
    }
    // Pivoted elimination is only a test oracle; production PowerIO emits
    // sparse operators and does not acquire a solver dependency here.
    for pivot in 0..size {
        let best = (pivot..size)
            .max_by(|&a, &b| matrix[a][pivot].norm().total_cmp(&matrix[b][pivot].norm()))?;
        if matrix[best][pivot].norm() < 1e-13 {
            return None;
        }
        matrix.swap(pivot, best);
        let diagonal = matrix[pivot][pivot];
        for value in &mut matrix[pivot][pivot..=size] {
            *value /= diagonal;
        }
        let normalized = matrix[pivot].clone();
        for (row, values) in matrix.iter_mut().enumerate() {
            if row != pivot {
                let factor = values[pivot];
                for (column, coefficient) in normalized.iter().enumerate().skip(pivot) {
                    values[column] -= factor * coefficient;
                }
            }
        }
    }
    Some(matrix.into_iter().map(|row| row[size]).collect())
}

#[test]
fn floating_star_displaces_without_changing_line_voltage_or_leaking_current() {
    let source = circuit("");
    let admittance = [
        Complex64::new(0.01, -0.003),
        Complex64::new(0.02, -0.004),
        Complex64::new(0.04, 0.002),
    ];
    let emf = source.boundary.equations().map(|row| row.voltage);
    for star_admittance in [Complex64::default(), Complex64::new(0.05, -0.01)] {
        let solved = solve(&source, admittance, star_admittance).unwrap();
        let expected_star = -admittance
            .iter()
            .zip(emf)
            .map(|(y, e)| y * e)
            .sum::<Complex64>()
            / (admittance.iter().sum::<Complex64>() + star_admittance);
        assert!(expected_star.norm() > 10.0);
        near(solved[3], expected_star);
        let mut sum_current = Complex64::default();
        for phase in 0..3 {
            near(solved[phase] - solved[3], emf[phase]);
            near(
                solved[phase] - solved[(phase + 1) % 3],
                emf[phase] - emf[(phase + 1) % 3],
            );
            let load_current = admittance[phase] * solved[phase];
            near(solved[4 + phase], -load_current);
            sum_current += load_current;
        }
        near(
            sum_current + star_admittance * solved[3],
            Complex64::default(),
        );
    }
    assert!(source.into_grounded().is_err());
}

#[test]
fn floating_source_needs_a_reference_path_and_balanced_loads_do_not_shift_it() {
    let source = circuit("");
    let zero = Complex64::default();
    assert!(solve(&source, [zero; 3], zero).is_none());
    let solved = solve(&source, [Complex64::new(0.02, -0.01); 3], zero).unwrap();
    near(solved[3], zero);
    // Only L1 has a path to earth: it sits at earth potential without
    // source zero-sequence current, while the other line voltages survive.
    let solved = solve(&source, [Complex64::new(0.02, 0.0), zero, zero], zero).unwrap();
    near(solved[0], zero);
    near(solved[3], -source.boundary.equations()[0].voltage);
    for current in &solved[4..] {
        near(*current, zero);
    }
}

#[test]
fn grounded_source_has_no_floating_star_and_can_supply_zero_sequence_current() {
    let source = circuit("UPDATE Infeeder SET Flag_Z0=1,R0=0,X0=0");
    assert_eq!(source.boundary.reference, VoltageReference::Earth);
    let admittance = [0.01, 0.02, 0.04].map(|g| Complex64::new(g, 0.0));
    let solved = solve(&source, admittance, Complex64::default()).unwrap();
    for (phase, equation) in source.boundary.equations().iter().enumerate() {
        assert!(equation.negative.is_none());
        near(solved[phase], equation.voltage);
    }
    assert!(solved[3..].iter().sum::<Complex64>().norm() > 1.0);
    let grounded = source.into_grounded().unwrap();
    assert_eq!(grounded.source.terminal_map, ["1", "2", "3"]);
}

#[test]
fn floating_star_is_local_and_never_connected_by_the_external_phase_switch() {
    for terminal_state in [0, 1] {
        let source = circuit(&format!("UPDATE Terminal SET Flag_State={terminal_state}"));
        assert_eq!(source.bus.id, source.boundary.bus);
        assert_eq!(source.bus.terminals, ["1", "2", "3", "star"]);
        assert_eq!(
            source.boundary.reference,
            VoltageReference::Terminal("star".into())
        );
        assert!(source.bus.grounded.is_empty());
        assert_eq!(source.switch.bus_from, "10");
        assert_eq!(source.switch.bus_to, source.bus.id);
        assert_eq!(source.switch.terminal_map_from, ["1", "2", "3"]);
        assert_eq!(source.switch.terminal_map_to, ["1", "2", "3"]);
        assert_eq!(source.switch.open, terminal_state == 0);
    }
}

#[test]
fn native_floating_infeeder_produces_constraints_without_claiming_network_support() {
    let archive = include_bytes!("../../../tests/data/sincal/1-LV-rural1--0-sw.sinx");
    let db = read(
        &Source::from_memory("case.sinx", &archive[..]).unwrap(),
        None,
    )
    .unwrap();
    let topology = db.topology_draft().unwrap();
    let input = db.infeeder_input(18).unwrap();
    let bus = topology
        .buses()
        .into_iter()
        .find(|bus| bus.id == input.terminal.node.to_string())
        .unwrap();
    let source = input
        .ideal_boundary_circuit(&bus, topology.nodes[&input.terminal.node].nominal_ll_volts)
        .unwrap();
    assert_eq!(
        source.boundary.reference,
        VoltageReference::Terminal("star".into())
    );
    let equations = source.boundary.equations();
    for equation in &equations {
        assert_eq!(equation.negative.as_deref(), Some("star"));
    }
    assert!(((equations[0].voltage - equations[1].voltage).norm() - 20_500.0).abs() < 1e-9);
    assert!(source.into_grounded().is_err());
    assert!(db.network().is_err());
}

#[test]
fn floating_boundary_does_not_admit_finite_impedance_or_unresolved_controls() {
    for edit in [
        "UPDATE Infeeder SET xi=10",
        "UPDATE Infeeder SET Flag_Z0=1,R0=1,X0=0",
        "UPDATE Infeeder SET Flag_Z0=2,Stp_ID=5",
        "UPDATE Infeeder SET Flag_Lf=2",
        "UPDATE Element SET Flag_State=0",
        "UPDATE Infeeder SET DayOpSer_ID=1",
        "UPDATE Terminal SET Flag_Terminal=1",
    ] {
        let db = NativeDatabase::decode(&infeeder_database(edit), None).unwrap();
        let input = db.infeeder_input(30).unwrap();
        let bus = DistBus::new("10", ["1", "2", "3"].map(str::to_owned).to_vec());
        assert!(
            input.ideal_boundary_circuit(&bus, 400.0).is_err(),
            "accepted {edit}"
        );
    }
}
