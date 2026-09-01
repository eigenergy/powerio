//! The solver facing requirements of the public AC OPF preparation: an external
//! solver formulates the complete pi model AC OPF from
//! `build_ac_opf_preparation` alone. 0.9 exposed this assembly as
//! `powerio_prob::build_ac_opf_instance`; this pins its restored surface.

use powerio_matrix::{
    AcOpfAssemblyOptions, AcPfAssemblyOptions, BalancedNetwork, Branch, Bus, BusId, BusType,
    GenCost, Generator, PreparedAcBusSpecification, PreparedObjective, build_ac_opf_preparation,
    build_ac_pf_preparation,
};
use powerio_prob::{
    AcBusSpecification, AcOpfInstance, AcPfInstance, ActiveConstraints,
    BalancedOperatingPointBuilder, ConstraintSelection, Objective,
};
use powerio_tx::{Impedance, Load, Shunt, Storage, Transformer3W, Winding};

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
    assert!(prep.correct_angle_difference_bounds);
    assert_eq!(
        prep.branches.angle_min,
        vec![
            -powerio_tx::POWER_MODELS_ANGLE_BOUND_PAD,
            -powerio_tx::POWER_MODELS_ANGLE_BOUND_PAD,
        ]
    );
    assert_eq!(
        prep.branches.angle_max,
        vec![
            powerio_tx::POWER_MODELS_ANGLE_BOUND_PAD,
            powerio_tx::POWER_MODELS_ANGLE_BOUND_PAD,
        ]
    );

    let exact = build_ac_opf_preparation(
        &instance,
        &AcOpfAssemblyOptions::default().with_correct_angle_difference_bounds(false),
    )
    .unwrap();
    assert!(!exact.correct_angle_difference_bounds);
    assert_eq!(
        exact.branches.angle_min,
        vec![-2.0 * std::f64::consts::PI; 2]
    );
    assert_eq!(
        exact.branches.angle_max,
        vec![2.0 * std::f64::consts::PI; 2]
    );

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
fn ac_opf_preparation_uses_the_instance_initial_point() {
    let mut first = Bus::new(BusId(1), BusType::Ref, 230.0);
    first.vm = 0.91;
    first.va = -3.0;
    let mut second = Bus::new(BusId(2), BusType::Pq, 230.0);
    second.vm = 0.92;
    second.va = 4.0;
    let mut network = BalancedNetwork::in_memory(
        "ac-opf-initial",
        100.0,
        vec![first, second],
        vec![Branch::new(BusId(1), BusId(2), 0.01, 0.1)],
    );
    let mut generator = Generator::new(BusId(1));
    generator.uid = Some("generator".into());
    generator.pg = 10.0;
    generator.qg = 2.0;
    generator.vg = 0.97;
    generator.cost = Some(GenCost::new(2, 0.0, 0.0, vec![0.01, 10.0, 0.0]));
    network.generators_mut().push(generator);

    let initial = BalancedOperatingPointBuilder::for_point(network.clone())
        .bus_voltage_magnitudes(vec![1.04, 0.98])
        .bus_voltage_angles(vec![0.11, -0.22])
        .generator_active_powers(vec![75.0])
        .generator_reactive_powers(vec![13.0])
        .generator_voltage_setpoints(vec![1.03])
        .build_point()
        .unwrap();
    let instance = AcOpfInstance::from_network(network)
        .unwrap()
        .with_initial_point(initial);
    let prepared = build_ac_opf_preparation(&instance, &AcOpfAssemblyOptions::default()).unwrap();

    assert_eq!(prepared.buses.initial_vm, vec![1.04, 0.98]);
    assert_eq!(prepared.buses.initial_va, vec![0.11, -0.22]);
    assert_eq!(prepared.generators.pg, vec![0.75]);
    assert_eq!(prepared.generators.qg, vec![0.13]);
    assert_eq!(prepared.generators.vg, vec![1.03]);
}

#[test]
fn ac_opf_preparation_keeps_storage_fields_and_units() {
    let mut network = BalancedNetwork::in_memory(
        "ac-opf-storage",
        100.0,
        vec![
            Bus::new(BusId(1), BusType::Ref, 230.0),
            Bus::new(BusId(2), BusType::Pq, 230.0),
        ],
        vec![Branch::new(BusId(1), BusId(2), 0.01, 0.1)],
    );
    let mut generator = Generator::new(BusId(1));
    generator.cost = Some(GenCost::new(2, 0.0, 0.0, vec![0.01, 10.0, 0.0]));
    network.generators_mut().push(generator);

    let mut storage = Storage::new(BusId(2));
    storage.uid = Some("battery".into());
    storage.ps = 12.0;
    storage.qs = -3.0;
    storage.energy = 80.0;
    storage.energy_rating = 120.0;
    storage.charge_rating = 25.0;
    storage.discharge_rating = 30.0;
    storage.charge_efficiency = 0.91;
    storage.discharge_efficiency = 0.88;
    storage.thermal_rating = 35.0;
    storage.qmin = -14.0;
    storage.qmax = 16.0;
    storage.r = 0.001;
    storage.x = 0.002;
    storage.p_loss = 0.4;
    storage.q_loss = 0.5;
    network.storage_mut().push(storage);
    let mut inactive = Storage::new(BusId(2));
    inactive.uid = Some("inactive".into());
    inactive.in_service = false;
    network.storage_mut().push(inactive);

    let instance = AcOpfInstance::from_network(network).unwrap();
    let per_unit = build_ac_opf_preparation(&instance, &AcOpfAssemblyOptions::default()).unwrap();
    assert_eq!(per_unit.n_storage(), 1);
    assert_eq!(per_unit.storage.identities, vec!["battery"]);
    assert_eq!(per_unit.storage.bus_of_storage, vec![1]);
    assert_eq!(per_unit.storage.source_rows, vec![0]);
    let close = |actual: f64, expected: f64| assert!((actual - expected).abs() < 1e-12);
    close(per_unit.storage.p[0], 0.12);
    close(per_unit.storage.q[0], -0.03);
    close(per_unit.storage.energy[0], 0.8);
    close(per_unit.storage.energy_rating[0], 1.2);
    close(per_unit.storage.charge_rating[0], 0.25);
    close(per_unit.storage.discharge_rating[0], 0.3);
    assert_eq!(per_unit.storage.charge_efficiency, vec![0.91]);
    assert_eq!(per_unit.storage.discharge_efficiency, vec![0.88]);
    close(per_unit.storage.s_max[0], 0.35);
    close(per_unit.storage.qmin[0], -0.14);
    close(per_unit.storage.qmax[0], 0.16);
    assert_eq!(per_unit.storage.r, vec![0.001]);
    assert_eq!(per_unit.storage.x, vec![0.002]);
    close(per_unit.storage.p_loss[0], 0.004);
    close(per_unit.storage.q_loss[0], 0.005);
    assert_eq!(per_unit.storage.in_service, vec![true]);

    let native = build_ac_opf_preparation(
        &instance,
        &AcOpfAssemblyOptions::default().with_units(powerio_matrix::Units::Native),
    )
    .unwrap();
    assert_eq!(native.storage.p, vec![12.0]);
    assert_eq!(native.storage.q, vec![-3.0]);
    assert_eq!(native.storage.energy, vec![80.0]);
    assert_eq!(native.storage.energy_rating, vec![120.0]);
    assert_eq!(native.storage.charge_rating, vec![25.0]);
    assert_eq!(native.storage.discharge_rating, vec![30.0]);
    assert_eq!(native.storage.s_max, vec![35.0]);
    assert_eq!(native.storage.qmin, vec![-14.0]);
    assert_eq!(native.storage.qmax, vec![16.0]);
    assert_eq!(native.storage.p_loss, vec![0.4]);
    assert_eq!(native.storage.q_loss, vec![0.5]);
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
    assert_eq!(
        prepared.branches.analysis_sources,
        vec![powerio_matrix::AnalysisBranchSource::Branch { row: 0 }]
    );
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

#[test]
fn ac_pf_preparation_preserves_explicit_specifications_and_initial_point() {
    let mut first = Bus::new(BusId(1), BusType::Pq, 230.0);
    first.vm = 0.91;
    first.va = -3.0;
    let mut second = Bus::new(BusId(2), BusType::Pq, 230.0);
    second.vm = 0.92;
    second.va = 4.0;
    let isolated = Bus::new(BusId(3), BusType::Ref, 230.0);
    let mut network = BalancedNetwork::in_memory(
        "custom-ac-pf",
        100.0,
        vec![first, second, isolated],
        vec![Branch::new(BusId(1), BusId(2), 0.01, 0.1)],
    );
    network.shunts_mut().push(Shunt::new(BusId(2), 2.0, -3.0));
    network.loads_mut().push(Load::new(BusId(2), 4.0, 5.0));
    let mut generator = Generator::new(BusId(2));
    generator.qg = 12.0;
    generator.qmin = -20.0;
    generator.qmax = 30.0;
    network.generators_mut().push(generator);

    let initial = BalancedOperatingPointBuilder::for_point(network.clone())
        .bus_voltage_magnitudes(vec![1.04, 0.98, 0.5])
        .bus_voltage_angles(vec![0.11, -0.22, 0.33])
        .build_point()
        .unwrap();
    let specifications = vec![
        AcBusSpecification::Reference { vm: 1.03, va: 7.5 },
        AcBusSpecification::Pv { p: 12.5, vm: 1.01 },
        AcBusSpecification::Isolated,
    ];
    let instance = AcPfInstance::new(network, specifications.clone())
        .unwrap()
        .with_initial_point(initial);
    assert_eq!(instance.specifications(), specifications);

    let prepared = build_ac_pf_preparation(&instance, &AcPfAssemblyOptions::default()).unwrap();
    assert_eq!(prepared.bus_ids, vec![BusId(1), BusId(2)]);
    assert_eq!(prepared.bus_source_rows, vec![Some(0), Some(1)]);
    assert_eq!(
        prepared.specifications,
        vec![
            PreparedAcBusSpecification::Reference {
                vm: 1.03,
                va: 7.5_f64.to_radians(),
            },
            PreparedAcBusSpecification::Pv { p: 0.125, vm: 1.01 },
        ]
    );
    assert_eq!(
        prepared.reference_buses.iter().copied().collect::<Vec<_>>(),
        vec![0]
    );
    assert_eq!(prepared.buses.g_s, vec![0.0, 0.02]);
    assert_eq!(prepared.buses.b_s, vec![0.0, -0.03]);
    assert_eq!(prepared.buses.q_d, vec![0.0, 0.05]);
    assert_eq!(prepared.buses.initial_vm, vec![1.04, 0.98]);
    assert_eq!(prepared.buses.initial_va, vec![0.11, -0.22]);
    assert_eq!(prepared.generators.bus_of_gen, vec![1]);
    assert_eq!(prepared.generators.source_rows, vec![Some(0)]);
    assert_eq!(prepared.generators.qg, vec![0.12]);
    assert_eq!(prepared.generators.qmin, vec![-0.2]);
    assert_eq!(prepared.generators.qmax, vec![0.3]);
}

#[test]
fn ac_pf_preparation_does_not_require_a_generator() {
    let network = BalancedNetwork::in_memory(
        "fixed-ac-pf",
        100.0,
        vec![
            Bus::new(BusId(1), BusType::Pq, 230.0),
            Bus::new(BusId(2), BusType::Pq, 230.0),
        ],
        vec![Branch::new(BusId(1), BusId(2), 0.01, 0.1)],
    );
    let instance = AcPfInstance::new(
        network,
        vec![
            AcBusSpecification::Reference { vm: 1.0, va: 0.0 },
            AcBusSpecification::Pq { p: -10.0, q: -2.0 },
        ],
    )
    .unwrap();
    let prepared = build_ac_pf_preparation(&instance, &AcPfAssemblyOptions::default()).unwrap();
    assert_eq!(prepared.n_generators(), 0);
    assert_eq!(prepared.n_branches(), 1);
    assert!(prepared.correct_angle_difference_bounds);
    assert_eq!(
        prepared.branches.angle_min,
        vec![-powerio_tx::POWER_MODELS_ANGLE_BOUND_PAD]
    );

    let exact = build_ac_pf_preparation(
        &instance,
        &AcPfAssemblyOptions::default().with_correct_angle_difference_bounds(false),
    )
    .unwrap();
    assert!(!exact.correct_angle_difference_bounds);
    assert_eq!(exact.branches.angle_min, vec![-2.0 * std::f64::consts::PI]);
}

#[test]
fn ac_pf_preparation_keeps_transformer_windings_out_of_source_branch_rows() {
    let mut network = BalancedNetwork::in_memory(
        "three-winding-ac-pf",
        100.0,
        vec![
            Bus::new(BusId(1), BusType::Ref, 230.0),
            Bus::new(BusId(2), BusType::Pq, 115.0),
            Bus::new(BusId(3), BusType::Pq, 13.8),
        ],
        Vec::new(),
    );
    let mut transformer = Transformer3W::new(
        [
            Winding::new(BusId(1)),
            Winding::new(BusId(2)),
            Winding::new(BusId(3)),
        ],
        [
            Impedance::new(0.0, 0.10, 100.0),
            Impedance::new(0.0, 0.12, 100.0),
            Impedance::new(0.0, 0.14, 100.0),
        ],
    );
    transformer.uid = Some("tx".into());
    network.transformers_3w_mut().push(transformer);
    let instance = AcPfInstance::from_network(network).unwrap();
    let prepared = build_ac_pf_preparation(&instance, &AcPfAssemblyOptions::default()).unwrap();

    assert_eq!(prepared.n_source_branches, 0);
    assert_eq!(prepared.n_buses, 4);
    assert_eq!(
        prepared.bus_source_rows,
        vec![Some(0), Some(1), Some(2), None]
    );
    assert_eq!(
        prepared.specifications[3],
        PreparedAcBusSpecification::Pq { p: 0.0, q: 0.0 }
    );
    assert_eq!(prepared.buses.q_d, vec![0.0, 0.0, 0.0, 0.0]);
    assert_eq!(
        prepared.branches.analysis_sources,
        vec![
            powerio_matrix::AnalysisBranchSource::ThreeWindingTransformerWinding {
                transformer_row: 0,
                winding: 0,
            },
            powerio_matrix::AnalysisBranchSource::ThreeWindingTransformerWinding {
                transformer_row: 0,
                winding: 1,
            },
            powerio_matrix::AnalysisBranchSource::ThreeWindingTransformerWinding {
                transformer_row: 0,
                winding: 2,
            },
        ]
    );
}
