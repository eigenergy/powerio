//! The per coil generator dispatch column of `McAcOpfSolution`: its length
//! follows the generators' coils, it is absent unless recorded, and for a
//! DELTA generator it reproduces a cost times dispatch objective that the
//! per terminal column does not.

use std::sync::Arc;

use powerio_dist::{Configuration, DistBus, DistGenerator, MulticonductorNetwork, VoltageSource};
use powerio_prob::{McAcOpfInstance, McAcOpfSolution, Termination};

const V_LN: f64 = 240.0;
const ANGLES: [f64; 3] = [
    0.0,
    -2.0 * std::f64::consts::FRAC_PI_3,
    2.0 * std::f64::consts::FRAC_PI_3,
];

fn names(terminals: &[&str]) -> Vec<String> {
    terminals.iter().map(|&name| name.to_owned()).collect()
}

/// One bus with terminals a, b, c and a grounded neutral n, a voltage
/// source on the phases, and the stated generators.
fn network(generators: Vec<DistGenerator>) -> MulticonductorNetwork {
    let mut net = MulticonductorNetwork::named("coil-dispatch");
    let mut bus = DistBus::new("src", names(&["a", "b", "c", "n"]));
    bus.grounded = names(&["n"]);
    net.buses_mut().push(bus);
    net.sources_mut().push(VoltageSource::new(
        "vs",
        "src",
        names(&["a", "b", "c"]),
        vec![V_LN; 3],
        ANGLES.to_vec(),
    ));
    net.generators_mut().extend(generators);
    net
}

fn generator(name: &str, terminals: &[&str], configuration: Configuration) -> DistGenerator {
    DistGenerator::new(
        name,
        "src",
        names(terminals),
        configuration,
        Vec::new(),
        Vec::new(),
    )
}

/// A solution over `net` with zero source injections and the stated per
/// terminal generator column and objective.
fn solution(
    net: MulticonductorNetwork,
    generator_terminal_power: Vec<f64>,
    objective: f64,
) -> McAcOpfSolution {
    let instance = Arc::new(McAcOpfInstance::from_network(net).unwrap());
    McAcOpfSolution::new(
        instance,
        Termination::Converged,
        vec![V_LN, V_LN, V_LN, 0.0],
        vec![ANGLES[0], ANGLES[1], ANGLES[2], 0.0],
        vec![0.0; 3],
        generator_terminal_power,
        objective,
    )
    .unwrap()
}

#[test]
fn the_coil_column_is_absent_until_recorded() {
    let net = network(vec![generator("g", &["a", "b", "c"], Configuration::Delta)]);
    let solution = solution(net, vec![0.0; 3], 0.0);
    assert_eq!(solution.generator_coil_active_powers(), None);
}

#[test]
fn the_coil_column_length_follows_the_generators_coils() {
    // A WYE generator on a, b, c, n maps four terminals and has three coils;
    // a three terminal DELTA has three; a two terminal DELTA has one.
    let net = network(vec![
        generator("wye", &["a", "b", "c", "n"], Configuration::Wye),
        generator("delta", &["a", "b", "c"], Configuration::Delta),
        generator("pair", &["b", "c"], Configuration::Delta),
    ]);
    let solution = solution(net, vec![0.0; 9], 0.0);
    assert_eq!(solution.generator_active_powers().len(), 9);

    for wrong in [0, 6, 8, 9] {
        let error = solution
            .clone()
            .with_generator_coil_active_powers(vec![0.0; wrong])
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(&format!("carries {wrong} values")) && error.contains("7 coils"),
            "{error}"
        );
    }

    let coils = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0];
    let solution = solution
        .with_generator_coil_active_powers(coils.clone())
        .unwrap();
    assert_eq!(solution.generator_coil_active_powers(), Some(&coils[..]));
    // The per terminal column is unchanged.
    assert_eq!(solution.generator_active_powers(), &[0.0; 9][..]);
}

type Complex = (f64, f64);

fn polar(magnitude: f64, angle: f64) -> Complex {
    (magnitude * angle.cos(), magnitude * angle.sin())
}

fn sub(a: Complex, b: Complex) -> Complex {
    (a.0 - b.0, a.1 - b.1)
}

/// Re(v · conj(i)).
fn active_power(v: Complex, i: Complex) -> f64 {
    v.0 * i.0 + v.1 * i.1
}

#[test]
fn a_delta_generator_cost_is_reproduced_per_coil_and_not_per_terminal() {
    // One DELTA generator on a, b, c whose coil ab alone generates, at
    // unity power factor: 10 A in phase with the coil voltage V_a - V_b.
    let cost_per_kwh = [0.10, 0.20, 0.30];
    let mut delta = generator("g", &["a", "b", "c"], Configuration::Delta);
    delta.cost = Some(cost_per_kwh.to_vec());
    let net = network(vec![delta]);

    let v: Vec<Complex> = ANGLES.iter().map(|&angle| polar(V_LN, angle)).collect();
    // Coil k spans terminal k to terminal k + 1 of the map: ab, bc, ca.
    let coil_voltage: Vec<Complex> = (0..3).map(|k| sub(v[k], v[(k + 1) % 3])).collect();
    let ab_angle = coil_voltage[0].1.atan2(coil_voltage[0].0);
    let coil_current = [polar(10.0, ab_angle), (0.0, 0.0), (0.0, 0.0)];

    let coil_power: Vec<f64> = (0..3)
        .map(|k| active_power(coil_voltage[k], coil_current[k]))
        .collect();
    // Terminal k receives coil k's current and returns coil k - 1's.
    let terminal_power: Vec<f64> = (0..3)
        .map(|k| active_power(v[k], sub(coil_current[k], coil_current[(k + 2) % 3])))
        .collect();

    // Both columns carry the same total power, split differently.
    let total = V_LN * 3.0_f64.sqrt() * 10.0;
    assert!((coil_power.iter().sum::<f64>() - total).abs() < 1e-9);
    assert!((terminal_power.iter().sum::<f64>() - total).abs() < 1e-9);
    assert!((coil_power[0] - total).abs() < 1e-9);
    assert!((terminal_power[0] - total / 2.0).abs() < 1e-9);
    assert!((terminal_power[1] - total / 2.0).abs() < 1e-9);

    // The objective is the cost rate per coil against that coil's dispatch.
    let cost = |dispatch: &[f64]| -> f64 {
        cost_per_kwh
            .iter()
            .zip(dispatch)
            .map(|(rate, watts)| rate * watts / 1000.0)
            .sum()
    };
    let objective = cost(&coil_power);
    let solution = solution(net, terminal_power, objective)
        .with_generator_coil_active_powers(coil_power)
        .unwrap();

    let per_coil = cost(solution.generator_coil_active_powers().unwrap());
    let per_terminal = cost(solution.generator_active_powers());
    assert!((per_coil - solution.objective()).abs() < 1e-12);
    // Half of the power sits on terminal b, priced at coil bc's rate.
    assert!((per_terminal - solution.objective()).abs() > 0.2);
}
