//! The consumer contract of the public AC OPF preparation: an external
//! solver formulates the complete pi model AC OPF from
//! `build_ac_opf_preparation` alone. 0.9 exposed this assembly as
//! `powerio_prob::build_ac_opf_instance`; this pins its restored surface.

use powerio_matrix::{
    AcOpfAssemblyOptions, BalancedNetwork, Branch, Bus, BusId, BusType, GenCost, Generator,
    PreparedObjective, build_ac_opf_preparation,
};
use powerio_prob::{AcOpfInstance, ActiveConstraints, ConstraintSelection, Objective};
use powerio_tx::Load;

#[test]
fn public_preparation_formulates_the_complete_ac_opf() {
    let mut transformer = Branch::new(BusId(2), BusId(3), 0.02, 0.2);
    transformer.tap = 1.05;
    transformer.shift = 30.0;
    transformer.rate_a = 60.0;
    transformer.b = 0.04;
    let mut network = BalancedNetwork::in_memory(
        "ac-consumer",
        100.0,
        vec![
            Bus::new(BusId(1), BusType::Ref, 230.0),
            Bus::new(BusId(2), BusType::Pq, 230.0),
            Bus::new(BusId(3), BusType::Pq, 230.0),
        ],
        vec![Branch::new(BusId(1), BusId(2), 0.01, 0.1), transformer],
    );
    network.loads_mut().push(Load::new(BusId(3), 90.0, 30.0));
    let mut generator = Generator::new(BusId(1));
    generator.pmax = 200.0;
    generator.qmax = 100.0;
    generator.qmin = -100.0;
    generator.pg = 90.0;
    generator.vg = 1.02;
    generator.cost = Some(GenCost::new(2, 0.0, 0.0, vec![0.02, 11.0, 3.0]));
    network.generators_mut().push(generator);

    let instance = AcOpfInstance::from_network(network).expect("instance");
    let prep =
        build_ac_opf_preparation(&instance, &AcOpfAssemblyOptions::default()).expect("preparation");

    assert_eq!(prep.n_buses, 3);
    assert_eq!(prep.n_branches(), 2);
    assert_eq!(prep.n_generators(), 1);
    assert!(!prep.synthesize_unrated_limits);

    // Per unit demand and the series admittance of the plain line:
    // y = 1/(0.01 + j0.1) => g = 0.01/0.0101, b = -0.1/0.0101.
    assert_eq!(prep.buses.p_d, vec![0.0, 0.0, 0.9]);
    assert_eq!(prep.buses.q_d, vec![0.0, 0.0, 0.3]);
    let denom = 0.01f64 * 0.01 + 0.1 * 0.1;
    assert!((prep.branches.g[0] - 0.01 / denom).abs() < 1e-12);
    assert!((prep.branches.b[0] + 0.1 / denom).abs() < 1e-12);

    // The transformer row keeps its tap, radian shift, symmetric split
    // charging, and per unit thermal limit.
    assert!((prep.branches.tap[1] - 1.05).abs() < 1e-12);
    assert!((prep.branches.shift[1] - 30.0f64.to_radians()).abs() < 1e-12);
    assert!((prep.branches.b_fr[1] - 0.02).abs() < 1e-12);
    assert!((prep.branches.b_to[1] - 0.02).abs() < 1e-12);
    assert!((prep.branches.s_max[1] - 0.6).abs() < 1e-12);

    // Generator schedule, bounds, cost scaling (per unit: q = 2 c2 base^2,
    // c = c1 base), and the voltage setpoint path.
    assert_eq!(prep.generators.bus_of_gen, vec![0]);
    assert_eq!(prep.generators.source_rows, vec![Some(0)]);
    assert!((prep.generators.q[0] - 2.0 * 0.02 * 100.0 * 100.0).abs() < 1e-9);
    assert!((prep.generators.c[0] - 11.0 * 100.0).abs() < 1e-9);
    assert!((prep.generators.pg[0] - 0.9).abs() < 1e-12);
    assert!((prep.generators.qmin[0] + 1.0).abs() < 1e-12);
    let vm = prep.calc_vm_setpoints();
    assert_eq!(vm, prep.calc_vm_setpoints());
    assert!((vm[0] - 1.02).abs() < 1e-12, "generator vg wins at its bus");
    assert!(
        (vm[1] - 1.0).abs() < 1e-12,
        "no case voltage falls back to 1"
    );

    assert_eq!(
        prep.reference_buses.iter().copied().collect::<Vec<_>>(),
        vec![0]
    );

    // Nodal aggregation exposes the bus level view a relaxation reads.
    let nodal = prep
        .calc_nodal_generator_data()
        .expect("quadratic nodal costs");
    assert_eq!(prep.calc_nodal_generator_data().unwrap(), nodal);
    assert!(nodal.has_gen[0] && !nodal.has_gen[2]);
    assert!((nodal.pmax[0] - 2.0).abs() < 1e-12);
}

#[test]
fn ac_preparation_honors_feasibility_constraints_and_limit_synthesis() {
    let mut line = Branch::new(BusId(1), BusId(2), 0.01, 0.1);
    line.uid = Some("line".into());
    line.rate_a = 0.0;
    let mut first = Bus::new(BusId(1), BusType::Ref, 230.0);
    first.vmin = 0.95;
    first.vmax = 1.05;
    let mut second = Bus::new(BusId(2), BusType::Pq, 230.0);
    second.vmin = 0.94;
    second.vmax = 1.06;
    let mut network =
        BalancedNetwork::in_memory("ac-semantics", 100.0, vec![first, second], vec![line]);
    let mut generator = Generator::new(BusId(1));
    generator.uid = Some("generator".into());
    generator.cost = None;
    network.generators_mut().push(generator);

    let mut constraints = ActiveConstraints::default();
    constraints.voltage_bounds = ConstraintSelection::Only(vec!["2".into()]);
    constraints.generator_capability = ConstraintSelection::None;
    constraints.thermal_limits = ConstraintSelection::Only(vec!["line".into()]);
    constraints.angle_bounds = ConstraintSelection::None;
    let instance = AcOpfInstance::from_network(network)
        .unwrap()
        .with_objective(Objective::none())
        .with_constraints(constraints);
    let prepared = build_ac_opf_preparation(
        &instance,
        &AcOpfAssemblyOptions::default().with_synthesize_unrated_limits(true),
    )
    .unwrap();

    assert_eq!(prepared.objective, PreparedObjective::Feasibility);
    assert_eq!(prepared.generators.q, vec![0.0]);
    assert_eq!(prepared.buses.voltage_bound_active, vec![false, true]);
    assert_eq!(prepared.generators.capability_active, vec![false]);
    assert_eq!(prepared.branches.thermal_limit_active, vec![true]);
    assert_eq!(prepared.branches.angle_bound_active, vec![false]);
    assert!(prepared.synthesize_unrated_limits);
    assert!(prepared.branches.s_max[0] > 0.0);
}

#[test]
fn ac_preparation_preserves_convex_piecewise_cost_breakpoints() {
    let mut network = BalancedNetwork::in_memory(
        "ac-piecewise",
        100.0,
        vec![
            Bus::new(BusId(1), BusType::Ref, 230.0),
            Bus::new(BusId(2), BusType::Pq, 230.0),
        ],
        vec![Branch::new(BusId(1), BusId(2), 0.01, 0.1)],
    );
    let mut generator = Generator::new(BusId(1));
    generator.pmax = 100.0;
    generator.cost = Some(GenCost::new(
        1,
        0.0,
        0.0,
        vec![0.0, 5.0, 40.0, 85.0, 100.0, 265.0],
    ));
    network.generators_mut().push(generator);

    let instance = AcOpfInstance::from_network(network).unwrap();
    let prepared = build_ac_opf_preparation(&instance, &AcOpfAssemblyOptions::default()).unwrap();
    let cost = prepared.generators.piecewise_linear[0]
        .as_ref()
        .expect("piecewise cost");
    assert_eq!(cost.power, vec![0.0, 0.4, 1.0]);
    assert_eq!(cost.value, vec![5.0, 85.0, 265.0]);
    assert_eq!(prepared.generators.q, vec![0.0]);
    assert!(prepared.calc_nodal_generator_data().is_err());
}

#[test]
fn ac_preparation_excludes_explicitly_isolated_rows() {
    let mut network = BalancedNetwork::in_memory(
        "ac-isolated-source-row",
        100.0,
        vec![
            Bus::new(BusId(1), BusType::Ref, 230.0),
            Bus::new(BusId(2), BusType::Pq, 230.0),
            Bus::new(BusId(3), BusType::Isolated, 230.0),
        ],
        vec![
            Branch::new(BusId(1), BusId(2), 0.01, 0.1),
            Branch::new(BusId(2), BusId(3), 0.01, 0.1),
        ],
    );
    network.loads_mut().push(Load::new(BusId(2), 40.0, 10.0));
    network.loads_mut().push(Load::new(BusId(3), 99.0, 99.0));
    let mut active_generator = Generator::new(BusId(1));
    active_generator.cost = Some(GenCost::new(2, 0.0, 0.0, vec![0.01, 10.0, 0.0]));
    network.generators_mut().push(active_generator);
    network.generators_mut().push(Generator::new(BusId(3)));

    let instance = AcOpfInstance::from_network(network).unwrap();
    let prepared = build_ac_opf_preparation(&instance, &AcOpfAssemblyOptions::default()).unwrap();

    assert_eq!(prepared.bus_ids, vec![BusId(1), BusId(2)]);
    assert_eq!(prepared.bus_analysis_rows, vec![0, 1]);
    assert_eq!(prepared.bus_source_rows, vec![Some(0), Some(1)]);
    assert_eq!(prepared.buses.p_d, vec![0.0, 0.4]);
    assert_eq!(prepared.buses.q_d, vec![0.0, 0.1]);
    assert_eq!(prepared.branches.analysis_rows, vec![0]);
    assert_eq!(prepared.branches.source_rows, vec![Some(0)]);
    assert_eq!(prepared.generators.analysis_rows, vec![0]);
    assert_eq!(prepared.generators.source_rows, vec![Some(0)]);
    assert_eq!(prepared.n_source_branches, 2);
    assert_eq!(prepared.n_source_generators, 2);
}

#[test]
fn ac_preparation_refuses_unknown_identities_in_every_constraint_family() {
    use powerio_matrix::Error;

    let mut line = Branch::new(BusId(1), BusId(2), 0.01, 0.1);
    line.uid = Some("line".into());
    line.rate_a = 100.0;
    let mut network = BalancedNetwork::in_memory(
        "ac-unknown-selection",
        100.0,
        vec![
            Bus::new(BusId(1), BusType::Ref, 230.0),
            Bus::new(BusId(2), BusType::Pq, 230.0),
        ],
        vec![line],
    );
    let mut generator = Generator::new(BusId(1));
    generator.uid = Some("generator".into());
    generator.cost = Some(GenCost::new(2, 0.0, 0.0, vec![0.01, 10.0, 0.0]));
    network.generators_mut().push(generator);

    for family in [
        "generator capability",
        "bus voltage bounds",
        "branch thermal limits",
        "branch angle bounds",
    ] {
        let mut constraints = ActiveConstraints::default();
        let selection = ConstraintSelection::Only(vec!["missing-identity".into()]);
        match family {
            "generator capability" => constraints.generator_capability = selection,
            "bus voltage bounds" => constraints.voltage_bounds = selection,
            "branch thermal limits" => constraints.thermal_limits = selection,
            "branch angle bounds" => constraints.angle_bounds = selection,
            _ => unreachable!(),
        }
        let instance = AcOpfInstance::from_network(network.clone())
            .unwrap()
            .with_constraints(constraints);
        assert!(matches!(
            build_ac_opf_preparation(&instance, &AcOpfAssemblyOptions::default()),
            Err(Error::UnknownConstraintIdentity {
                family: actual,
                ..
            }) if actual == family
        ));
    }
}
