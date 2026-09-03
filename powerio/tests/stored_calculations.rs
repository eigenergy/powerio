//! Stored round trips for the calculation kinds: the seven instances, the
//! seven solutions, and the multiconductor operating point series. Every
//! kind writes byte stably, reads back, and has a committed fixture.

use std::sync::Arc;

use powerio::BranchSusceptanceFormula;
use powerio::{BalancedNetwork, PioValue};
use powerio_core::{PioModule, TimePoint};
use powerio_prob::{
    AcOpfInstance, AcOpfSolution, AcPfInstance, AcPfSolution, AcScucSolution, DcOpfInstance,
    DcOpfSolution, DcPfInstance, DcPfSolution, McAcOpfInstance, McAcOpfSolution, McAcPfInstance,
    McAcPfSolution, Objective, ObjectiveTerm, Residuals, ScucDeviceOutputs, ScucNetworkOutputs,
    Termination, ThreeWindingTransformerTerminalActivePower, ThreeWindingTransformerTerminalPower,
};
use powerio_tx::{
    Branch, Bus, BusId, BusType, GenCost, Generator, Impedance, Load, Transformer3W, Winding,
};

mod helpers;
use helpers::{deserialize_module_text as deserialize, serialize_module_text as serialize};

fn network() -> BalancedNetwork {
    let mut bus1 = Bus::new(BusId(1), BusType::Ref, 230.0);
    bus1.vm = 1.01;
    let bus2 = Bus::new(BusId(2), BusType::Pq, 230.0);
    let mut net = BalancedNetwork::in_memory(
        "calc",
        100.0,
        vec![bus1, bus2],
        vec![Branch::new(BusId(1), BusId(2), 0.01, 0.1)],
    );
    net.loads_mut().push(Load::new(BusId(2), 40.0, 10.0));
    let mut generator = Generator::new(BusId(1));
    generator.pg = 42.0;
    generator.pmax = 100.0;
    generator.cost = Some(GenCost::new(2, 0.0, 0.0, vec![0.01, 10.0, 0.0]));
    net.generators_mut().push(generator);
    net
}

fn mc_network() -> powerio_dist::MulticonductorNetwork {
    let mut net = powerio_dist::MulticonductorNetwork::named("calc-mc");
    net.buses_mut().push(powerio_dist::DistBus::new(
        "src",
        vec!["1".into(), "2".into(), "3".into()],
    ));
    net.sources_mut().push(powerio_dist::VoltageSource::new(
        "vs",
        "src",
        vec!["1".into(), "2".into(), "3".into()],
        vec![240.0, 240.0, 240.0],
        vec![0.0, -2.094, 2.094],
    ));
    net
}

fn initial_point(net: &BalancedNetwork) -> powerio_prob::OperatingPoint<BalancedNetwork> {
    powerio_prob::BalancedOperatingPointBuilder::for_point(net.clone())
        .load_active_powers(vec![40.0])
        .generator_active_powers(vec![42.0])
        .build_point()
        .unwrap()
}

fn round_trip(value: PioValue, label: &str) -> String {
    let type_name = value.type_name().to_owned();
    let module = PioModule::new(value);
    let text = serialize(&module).unwrap();
    let raw: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(raw["value"]["type"], type_name);
    let back = deserialize(&text).unwrap();
    assert_eq!(back.value.type_name(), type_name);
    assert_eq!(
        serialize(&back).unwrap(),
        text,
        "{label} is not byte stable"
    );
    text
}

#[test]
fn every_instance_kind_round_trips() {
    let net = network();
    let objective = Objective::default().with_term(ObjectiveTerm::NetworkGeneratorCost);

    round_trip(
        PioValue::DcPfInstance(
            DcPfInstance::from_network(net.clone())
                .unwrap()
                .with_initial_point(initial_point(&net)),
        ),
        "dc_pf_instance",
    );
    round_trip(
        PioValue::AcPfInstance(AcPfInstance::from_network(net.clone()).unwrap()),
        "ac_pf_instance",
    );
    round_trip(
        PioValue::DcOpfInstance(
            DcOpfInstance::from_network(net.clone())
                .unwrap()
                .with_objective(objective.clone()),
        ),
        "dc_opf_instance",
    );
    round_trip(
        PioValue::AcOpfInstance(
            AcOpfInstance::from_network(net.clone())
                .unwrap()
                .with_objective(objective.clone()),
        ),
        "ac_opf_instance",
    );
    round_trip(
        PioValue::McAcPfInstance(McAcPfInstance::from_network(mc_network()).unwrap()),
        "mc_ac_pf_instance",
    );
    round_trip(
        PioValue::McAcOpfInstance(
            McAcOpfInstance::from_network(mc_network())
                .unwrap()
                .with_objective(objective),
        ),
        "mc_ac_opf_instance",
    );
}

#[test]
fn ac_pf_instance_round_trip_keeps_explicit_bus_specifications() {
    let net = network();
    let specifications = vec![
        powerio_prob::AcBusSpecification::Reference { vm: 1.07, va: 8.0 },
        powerio_prob::AcBusSpecification::Pq { p: -37.5, q: -9.25 },
    ];
    let text = round_trip(
        PioValue::AcPfInstance(
            AcPfInstance::new(net, specifications.clone()).expect("explicit AC PF instance"),
        ),
        "ac_pf_instance_explicit_specifications",
    );
    let back = deserialize(&text).unwrap();
    let PioValue::AcPfInstance(back) = &back.value else {
        panic!("expected an AC PF instance");
    };
    assert_eq!(back.specifications(), specifications);
}

fn three_winding_network() -> BalancedNetwork {
    let mut net = BalancedNetwork::in_memory(
        "three-winding-solutions",
        100.0,
        vec![
            Bus::new(BusId(1), BusType::Ref, 230.0),
            Bus::new(BusId(2), BusType::Pq, 115.0),
            Bus::new(BusId(3), BusType::Pq, 13.8),
        ],
        Vec::new(),
    );
    net.transformers_3w_mut().push(Transformer3W::new(
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
    ));
    let mut generator = Generator::new(BusId(1));
    generator.pmax = 200.0;
    generator.cost = Some(GenCost::new(2, 0.0, 0.0, vec![0.01, 10.0, 0.0]));
    net.generators_mut().push(generator);
    net
}

#[test]
#[allow(clippy::too_many_lines)]
fn three_winding_terminal_powers_round_trip_on_every_balanced_solution() {
    let terminal_active_power =
        ThreeWindingTransformerTerminalActivePower::new([50.0, -30.0, -20.0]);
    let terminal_power =
        ThreeWindingTransformerTerminalPower::new([50.0, -30.0, -19.5], [8.0, -4.0, -3.5]);
    let net = three_winding_network();

    let dc_pf_instance = Arc::new(DcPfInstance::from_network(net.clone()).unwrap());
    let dc_pf = DcPfSolution::new(
        dc_pf_instance,
        Termination::Converged,
        vec![0.0; 3],
        vec![0.0; 3],
        Vec::new(),
        Vec::new(),
        vec![terminal_active_power],
    )
    .unwrap();
    let text = round_trip(PioValue::DcPfSolution(dc_pf), "dc_pf_three_winding_power");
    let back = deserialize(&text).unwrap();
    let PioValue::DcPfSolution(back) = &back.value else {
        panic!("expected a DC PF solution");
    };
    assert_eq!(
        back.three_winding_transformer_terminal_active_powers(),
        &[terminal_active_power]
    );

    let dc_opf_instance = Arc::new(DcOpfInstance::from_network(net.clone()).unwrap());
    let dc_opf = DcOpfSolution::new(
        dc_opf_instance,
        Termination::Converged,
        vec![0.0; 3],
        vec![0.0; 3],
        Vec::new(),
        Vec::new(),
        vec![50.0],
        700.0,
        vec![terminal_active_power],
    )
    .unwrap();
    let text = round_trip(
        PioValue::DcOpfSolution(dc_opf),
        "dc_opf_three_winding_power",
    );
    let back = deserialize(&text).unwrap();
    let PioValue::DcOpfSolution(back) = &back.value else {
        panic!("expected a DC OPF solution");
    };
    assert_eq!(
        back.three_winding_transformer_terminal_active_powers(),
        &[terminal_active_power]
    );

    let pf_instance = Arc::new(AcPfInstance::from_network(net.clone()).unwrap());
    let pf = AcPfSolution::new(
        pf_instance,
        Termination::Converged,
        vec![1.0; 3],
        vec![0.0; 3],
        vec![0.0; 3],
        vec![0.0; 3],
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![terminal_power],
    )
    .unwrap();
    let text = round_trip(PioValue::AcPfSolution(pf), "ac_pf_three_winding_power");
    let back = deserialize(&text).unwrap();
    let PioValue::AcPfSolution(back) = &back.value else {
        panic!("expected an AC PF solution");
    };
    assert_eq!(
        back.three_winding_transformer_terminal_powers(),
        &[terminal_power]
    );

    let opf_instance = Arc::new(AcOpfInstance::from_network(net.clone()).unwrap());
    let opf = AcOpfSolution::new(
        Arc::clone(&opf_instance),
        Termination::Converged,
        vec![1.0; 3],
        vec![0.0; 3],
        vec![0.0; 3],
        vec![0.0; 3],
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![50.0],
        vec![8.0],
        750.0,
        vec![terminal_power],
    )
    .unwrap();
    let text = round_trip(PioValue::AcOpfSolution(opf), "ac_opf_three_winding_power");
    let back = deserialize(&text).unwrap();
    let PioValue::AcOpfSolution(back) = &back.value else {
        panic!("expected an AC OPF solution");
    };
    assert_eq!(
        back.three_winding_transformer_terminal_powers(),
        &[terminal_power]
    );

    let mut values = powerio_prob::solution::SocwrOpfValues::default();
    values.bus_voltage_magnitude_squared = vec![1.0; 3];
    values.generator_active_power = vec![50.0];
    values.generator_reactive_power = vec![8.0];
    values.three_winding_transformer_terminal_powers = vec![terminal_power];
    let socwr = powerio_prob::solution::SocwrOpfSolution::new(
        opf_instance,
        Termination::Converged,
        values,
        700.0,
    )
    .unwrap();
    let text = round_trip(
        PioValue::SocwrOpfSolution(socwr),
        "socwr_three_winding_power",
    );
    let back = deserialize(&text).unwrap();
    let PioValue::SocwrOpfSolution(back) = &back.value else {
        panic!("expected a SOCWR OPF solution");
    };
    assert_eq!(
        back.values().three_winding_transformer_terminal_powers,
        vec![terminal_power]
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn every_solution_kind_round_trips() {
    let net = network();

    let dc_pf = Arc::new(DcPfInstance::from_network(net.clone()).unwrap());
    round_trip(
        PioValue::DcPfSolution(
            DcPfSolution::new(
                dc_pf,
                Termination::Converged,
                vec![0.0, -0.02],
                vec![40.0, -40.0],
                vec![40.0],
                vec![-40.0],
                Vec::new(),
            )
            .unwrap()
            .with_producer("test-solver"),
        ),
        "dc_pf_solution",
    );

    let ac_pf = Arc::new(AcPfInstance::from_network(net.clone()).unwrap());
    round_trip(
        PioValue::AcPfSolution(
            AcPfSolution::new(
                ac_pf,
                Termination::Converged,
                vec![1.01, 0.99],
                vec![0.0, -1.2],
                vec![40.5, -40.0],
                vec![10.4, -10.0],
                vec![40.5],
                vec![10.4],
                vec![-40.0],
                vec![-10.0],
                Vec::new(),
            )
            .unwrap(),
        ),
        "ac_pf_solution",
    );

    let dc_opf = Arc::new(DcOpfInstance::from_network(net.clone()).unwrap());
    round_trip(
        PioValue::DcOpfSolution(
            DcOpfSolution::new(
                dc_opf,
                Termination::Converged,
                vec![0.0, -0.02],
                vec![40.0, -40.0],
                vec![40.0],
                vec![-40.0],
                vec![40.0],
                412.5,
                Vec::new(),
            )
            .unwrap(),
        ),
        "dc_opf_solution",
    );

    let ac_opf = Arc::new(AcOpfInstance::from_network(net.clone()).unwrap());
    round_trip(
        PioValue::AcOpfSolution(
            AcOpfSolution::new(
                ac_opf,
                Termination::IterationLimit,
                vec![1.01, 0.99],
                vec![0.0, -1.2],
                vec![40.5, -40.0],
                vec![10.4, -10.0],
                vec![40.5],
                vec![10.4],
                vec![-40.0],
                vec![-10.0],
                vec![40.5],
                vec![10.4],
                428.0,
                Vec::new(),
            )
            .unwrap(),
        ),
        "ac_opf_solution",
    );

    let mc_pf = Arc::new(McAcPfInstance::from_network(mc_network()).unwrap());
    round_trip(
        PioValue::McAcPfSolution(
            McAcPfSolution::new(
                mc_pf,
                Termination::Converged,
                vec![240.0, 239.9, 240.1],
                vec![0.0, -2.094, 2.094],
                vec![1000.0, 1000.0, 1000.0],
            )
            .unwrap(),
        ),
        "mc_ac_pf_solution",
    );

    let mc_opf = Arc::new(McAcOpfInstance::from_network(mc_network()).unwrap());
    round_trip(
        PioValue::McAcOpfSolution(
            McAcOpfSolution::new(
                mc_opf,
                Termination::Converged,
                vec![240.0, 239.9, 240.1],
                vec![0.0, -2.094, 2.094],
                vec![1000.0, 1000.0, 1000.0],
                Vec::new(),
                12.5,
            )
            .unwrap(),
        ),
        "mc_ac_opf_solution",
    );
}

/// SEC-9: the writer used to fold the default branch susceptance formula
/// (`SeriesSusceptance`) and a genuinely unmapped future variant into the
/// same wildcard arm, so every explicitly requested formula must still
/// round trip under its own name now that the arms are split.
#[test]
fn every_branch_susceptance_formula_round_trips_under_its_own_name() {
    for formula in [
        BranchSusceptanceFormula::SeriesSusceptance,
        BranchSusceptanceFormula::TapAdjustedReactance,
        BranchSusceptanceFormula::ReactanceOnly,
    ] {
        let instance = DcOpfInstance::from_network(network())
            .unwrap()
            .with_branch_susceptance_formula(formula);
        let text = round_trip(PioValue::DcOpfInstance(instance), "dc_opf_instance");
        let raw: serde_json::Value = serde_json::from_str(&text).unwrap();
        let back = deserialize(&text).unwrap();
        let PioValue::DcOpfInstance(back) = &back.value else {
            panic!("expected the dc_opf_instance kind");
        };
        assert_eq!(
            back.branch_susceptance_formula(),
            formula,
            "{}",
            raw["value"]["data"]["approximation"]
        );
    }
}

/// SEC-8: externally defined residual fields still use the stored module's
/// nonfinite adapter and distinguish a stated NaN from an absent value.
#[test]
fn residuals_round_trip_every_nonfinite_value() {
    let net = network();
    let dc_opf = Arc::new(DcOpfInstance::from_network(net).unwrap());
    let solution = DcOpfSolution::new(
        dc_opf,
        Termination::Converged,
        vec![0.0, -0.02],
        vec![40.0, -40.0],
        vec![40.0],
        vec![-40.0],
        vec![40.0],
        412.5,
        Vec::new(),
    )
    .unwrap()
    .with_residuals({
        let mut residuals = Residuals::default();
        residuals.max_active_power_mismatch = Some(f64::NAN);
        residuals.max_reactive_power_mismatch = Some(f64::INFINITY);
        residuals
    });

    let module = PioModule::new(PioValue::DcOpfSolution(solution));
    let text = serialize(&module).unwrap();
    let back = deserialize(&text).unwrap();
    let PioValue::DcOpfSolution(back) = &back.value else {
        panic!("expected the dc_opf_solution kind");
    };

    assert!(back.residuals().max_active_power_mismatch.unwrap().is_nan());
    assert_eq!(
        back.residuals().max_reactive_power_mismatch,
        Some(f64::INFINITY)
    );
    // Some(NAN) is distinct from None: a second solution with the mismatch
    // unstated must read back unstated, not as a smuggled-in NaN.
    let dc_opf = Arc::new(DcOpfInstance::from_network(network()).unwrap());
    let unstated = DcOpfSolution::new(
        dc_opf,
        Termination::Converged,
        vec![0.0, -0.02],
        vec![40.0, -40.0],
        vec![40.0],
        vec![-40.0],
        vec![40.0],
        412.5,
        Vec::new(),
    )
    .unwrap()
    .with_residuals({
        let mut residuals = Residuals::default();
        residuals.max_reactive_power_mismatch = Some(f64::NAN);
        residuals
    });
    let module = PioModule::new(PioValue::DcOpfSolution(unstated));
    let text = serialize(&module).unwrap();
    let back = deserialize(&text).unwrap();
    let PioValue::DcOpfSolution(back) = &back.value else {
        panic!("expected the dc_opf_solution kind");
    };
    assert_eq!(back.residuals().max_active_power_mismatch, None);
    assert!(
        back.residuals()
            .max_reactive_power_mismatch
            .unwrap()
            .is_nan()
    );
}

/// An optimization solver distinguishes proving the constraints empty and
/// proving the objective unbounded from a plain numerical failure. Every
/// termination kind round trips through the stored document under its stable
/// snake_case name, so a consumer reading a stored solution can act on the
/// outcome the producer actually reached.
#[test]
fn every_termination_kind_round_trips() {
    for (termination, name) in [
        (Termination::Converged, "converged"),
        (Termination::IterationLimit, "iteration_limit"),
        (Termination::Infeasible, "infeasible"),
        (Termination::Unbounded, "unbounded"),
        (Termination::Failed, "failed"),
        (Termination::NotReported, "not_reported"),
    ] {
        let dc_opf = Arc::new(DcOpfInstance::from_network(network()).unwrap());
        let solution = DcOpfSolution::new(
            dc_opf,
            termination.clone(),
            vec![0.0, -0.02],
            vec![40.0, -40.0],
            vec![40.0],
            vec![-40.0],
            vec![40.0],
            412.5,
            Vec::new(),
        )
        .unwrap();
        let module = PioModule::new(PioValue::DcOpfSolution(solution));
        let text = serialize(&module).unwrap();
        let raw: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(raw["value"]["data"]["termination"]["kind"], name);
        let back = deserialize(&text).unwrap();
        let PioValue::DcOpfSolution(back) = &back.value else {
            panic!("expected the dc_opf_solution kind");
        };
        assert_eq!(back.termination(), &termination);
    }
}

/// Objective derivatives and the two directional thermal multipliers round
/// trip without assuming a currency or collapsing two KKT multipliers into
/// one signed number.
#[test]
fn opf_economic_outputs_round_trip() {
    let dc_opf = Arc::new(DcOpfInstance::from_network(network()).unwrap());
    let solution = DcOpfSolution::new(
        dc_opf,
        Termination::Converged,
        vec![0.0, -0.02],
        vec![40.0, -40.0],
        vec![40.0],
        vec![-40.0],
        vec![40.0],
        412.5,
        Vec::new(),
    )
    .unwrap()
    .with_bus_active_power_marginals(vec![10.31, 12.05])
    .unwrap()
    .with_branch_thermal_limit_multipliers(vec![0.0], vec![1.74])
    .unwrap();
    let text = round_trip(PioValue::DcOpfSolution(solution), "dc_opf_solution");
    let back = deserialize(&text).unwrap();
    let PioValue::DcOpfSolution(back) = &back.value else {
        panic!("expected the dc_opf_solution kind");
    };
    assert_eq!(back.bus_active_power_marginals(), Some(&[10.31, 12.05][..]));
    assert_eq!(
        back.bus_active_power_marginal(powerio_tx::BusId(2)),
        Some(12.05)
    );
    assert_eq!(back.branch_from_limit_multipliers(), Some(&[0.0][..]));
    let branch_id = back.instance().network().branches()[0]
        .uid
        .as_deref()
        .unwrap();
    assert_eq!(back.branch_to_limit_multiplier(branch_id), Some(1.74));

    let ac_opf = Arc::new(AcOpfInstance::from_network(network()).unwrap());
    let solution = AcOpfSolution::new(
        ac_opf,
        Termination::Converged,
        vec![1.01, 0.99],
        vec![0.0, -1.2],
        vec![40.5, -40.0],
        vec![10.4, -10.0],
        vec![40.5],
        vec![10.4],
        vec![-40.0],
        vec![-10.0],
        vec![40.5],
        vec![10.4],
        428.0,
        Vec::new(),
    )
    .unwrap()
    .with_bus_active_power_marginals(vec![11.2, 11.9])
    .unwrap()
    .with_bus_reactive_power_marginals(vec![0.0, 0.4])
    .unwrap()
    .with_branch_thermal_limit_multipliers(vec![0.3], vec![0.0])
    .unwrap();
    let text = round_trip(PioValue::AcOpfSolution(solution), "ac_opf_solution");
    let back = deserialize(&text).unwrap();
    let PioValue::AcOpfSolution(back) = &back.value else {
        panic!("expected the ac_opf_solution kind");
    };
    assert_eq!(back.bus_active_power_marginals(), Some(&[11.2, 11.9][..]));
    assert_eq!(
        back.bus_reactive_power_marginal(powerio_tx::BusId(2)),
        Some(0.4)
    );
    assert_eq!(back.branch_from_limit_multipliers(), Some(&[0.3][..]));
    let branch_id = back.instance().network().branches()[0]
        .uid
        .as_deref()
        .unwrap();
    assert_eq!(back.branch_to_limit_multiplier(branch_id), Some(0.0));

    // A wrong length is refused at attachment, the same shape rule the
    // primal columns enforce.
    let dc_opf = Arc::new(DcOpfInstance::from_network(network()).unwrap());
    let solution = DcOpfSolution::new(
        dc_opf,
        Termination::Converged,
        vec![0.0, -0.02],
        vec![40.0, -40.0],
        vec![40.0],
        vec![-40.0],
        vec![40.0],
        412.5,
        Vec::new(),
    )
    .unwrap();
    assert!(
        solution
            .clone()
            .with_bus_active_power_marginals(vec![10.31])
            .is_err()
    );
    assert!(
        solution
            .with_branch_thermal_limit_multipliers(vec![-0.1], vec![0.0])
            .is_err()
    );
}

#[test]
fn the_multiconductor_series_round_trips() {
    let net = mc_network();
    let series = powerio_prob::MulticonductorOperatingPointBuilder::new(
        net,
        vec![
            TimePoint::new("h0", None).unwrap(),
            TimePoint::new("h1", None).unwrap(),
        ],
    )
    .terminal_voltage_magnitudes(vec![240.0, 240.0, 240.0, 239.0, 239.0, 239.0])
    .build()
    .unwrap();
    let text = round_trip(
        PioValue::from(series),
        "multiconductor operating point series",
    );
    let back = deserialize(&text).unwrap();
    let PioValue::TimeSeries(series) = &back.value else {
        panic!("wrong kind");
    };
    assert_eq!(series.len(), 2);
    let PioValue::MulticonductorOperatingPoint(point) = series.get(1).unwrap() else {
        panic!("wrong element type");
    };
    assert_eq!(point.terminal_voltage_magnitude("src", "2"), Some(239.0));
}

#[test]
fn the_scuc_pair_round_trips_from_the_goc3_fixture() {
    let instance = goc3_instance();
    let periods = instance.inputs().interval_durations.len();
    let buses = instance.network().buses().len();
    let devices = instance.inputs().devices.len();
    let doc = round_trip(PioValue::AcScucInstance(instance), "ac_scuc_instance");

    let back = deserialize(&doc).unwrap();
    let PioValue::AcScucInstance(instance) = &back.value else {
        panic!("wrong kind");
    };
    let mut network_outputs = ScucNetworkOutputs::default();
    network_outputs.bus_vm = vec![vec![1.0; buses]; periods];
    network_outputs.bus_va = vec![vec![0.0; buses]; periods];
    let mut device_outputs = ScucDeviceOutputs::default();
    device_outputs.on_status = vec![vec![true; devices]; periods];
    round_trip(
        PioValue::AcScucSolution(
            AcScucSolution::new(
                Arc::new(instance.clone()),
                Termination::Converged,
                network_outputs,
                device_outputs,
                Some(1.25e4),
            )
            .unwrap(),
        ),
        "ac_scuc_solution",
    );
}

fn goc3_instance() -> powerio_prob::AcScucInstance {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../powerio-prob/tests/data/goc3_small.json"
    );
    let text = std::fs::read_to_string(path).unwrap();
    let module = powerio::parse_with_options(
        powerio_core::Source::from_memory("goc3_small.json", text.into_bytes()).unwrap(),
        &powerio::ParseOptions::default()
            .format("goc3-json")
            .unwrap(),
    )
    .unwrap();
    let PioValue::AcScucInstance(instance) = module.value else {
        panic!("GO Challenge 3 problem did not produce powerio.AcScucInstance");
    };
    instance
}

fn full_scuc_outputs(
    instance: &powerio_prob::AcScucInstance,
) -> (
    powerio_prob::ScucNetworkOutputs,
    powerio_prob::ScucDeviceOutputs,
) {
    let periods = instance.inputs().interval_durations.len();
    let float_series = |seed: f64, width: usize| {
        vec![
            (0..width)
                .map(|column| seed + column as f64 * 0.5)
                .collect();
            periods
        ]
    };
    let status_series = |value: bool, width: usize| vec![vec![value; width]; periods];
    let buses = instance.network().buses().len();
    let shunts = instance.inputs().shunts.len();
    let ac_lines = instance
        .inputs()
        .branch_switching_costs
        .iter()
        .filter(|row| row.id.component_type() == "branch")
        .count();
    let transformers = instance
        .inputs()
        .branch_switching_costs
        .iter()
        .filter(|row| row.id.component_type() == "transformer")
        .count();
    let dc_lines = instance.network().hvdc().len();
    let devices = instance.inputs().devices.len();
    let mut network_outputs = ScucNetworkOutputs::default();
    network_outputs.bus_vm = float_series(1.0, buses);
    network_outputs.bus_va = float_series(2.0, buses);
    network_outputs.shunt_step = vec![vec![3; shunts]; periods];
    network_outputs.ac_line_on_status = status_series(true, ac_lines);
    network_outputs.transformer_tm = float_series(5.0, transformers);
    network_outputs.transformer_ta = float_series(6.0, transformers);
    network_outputs.transformer_on_status = status_series(true, transformers);
    network_outputs.dc_line_pdc_fr = float_series(8.0, dc_lines);
    network_outputs.dc_line_qdc_fr = float_series(9.0, dc_lines);
    network_outputs.dc_line_qdc_to = float_series(10.0, dc_lines);
    let mut device_outputs = ScucDeviceOutputs::default();
    device_outputs.on_status = status_series(true, devices);
    device_outputs.startup_status = status_series(false, devices);
    device_outputs.shutdown_status = status_series(false, devices);
    device_outputs.p_on = float_series(14.0, devices);
    device_outputs.q = float_series(15.0, devices);
    device_outputs.p_reg_res_up = float_series(16.0, devices);
    device_outputs.p_reg_res_down = float_series(17.0, devices);
    device_outputs.p_syn_res = float_series(18.0, devices);
    device_outputs.p_nsyn_res = float_series(19.0, devices);
    device_outputs.p_ramp_res_up_online = float_series(20.0, devices);
    device_outputs.p_ramp_res_up_offline = float_series(21.0, devices);
    device_outputs.p_ramp_res_down_online = float_series(22.0, devices);
    device_outputs.p_ramp_res_down_offline = float_series(23.0, devices);
    device_outputs.q_res_up = float_series(24.0, devices);
    device_outputs.q_res_down = float_series(25.0, devices);
    (network_outputs, device_outputs)
}

/// Every output series the runtime structs define survives the round trip,
/// field by field, and the wire spells exactly the series vocabulary the
/// defining crate exports.
#[test]
fn every_scuc_output_series_round_trips_under_its_exported_name() {
    let instance = goc3_instance();
    let (network_outputs, device_outputs) = full_scuc_outputs(&instance);
    let solution = AcScucSolution::new(
        Arc::new(instance),
        Termination::Converged,
        network_outputs.clone(),
        device_outputs.clone(),
        Some(4.25e3),
    )
    .unwrap();
    let text = serialize(&PioModule::new(PioValue::AcScucSolution(solution))).unwrap();

    let raw: serde_json::Value = serde_json::from_str(&text).unwrap();
    let wire_network: Vec<&str> = raw["value"]["data"]["network_outputs"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let mut expected: Vec<&str> = powerio_prob::SCUC_NETWORK_OUTPUT_SERIES.to_vec();
    expected.sort_unstable();
    assert_eq!(wire_network, expected);
    let wire_devices: Vec<&str> = raw["value"]["data"]["device_outputs"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let mut expected: Vec<&str> = powerio_prob::SCUC_DEVICE_OUTPUT_SERIES.to_vec();
    expected.sort_unstable();
    assert_eq!(wire_devices, expected);

    let back = deserialize(&text).unwrap();
    let PioValue::AcScucSolution(solution) = &back.value else {
        panic!("wrong kind");
    };
    assert_eq!(*solution.network_outputs(), network_outputs);
    assert_eq!(*solution.device_outputs(), device_outputs);
}
