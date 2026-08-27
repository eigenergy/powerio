//! Stored round trips for the calculation kinds: the seven instances, the
//! seven solutions, and the multiconductor operating point series. Every
//! kind writes byte stably, reads back, and has a committed fixture.

use std::sync::Arc;

use powerio::stored::{read_module, write_module};
use powerio::{BalancedNetwork, PioValue};
use powerio_core::{PioModule, TimePoint};
use powerio_prob::{
    AcOpfInstance, AcOpfSolution, AcPfInstance, AcPfSolution, AcScucSolution, DcOpfInstance,
    DcOpfSolution, DcPfInstance, DcPfSolution, McAcOpfInstance, McAcOpfSolution, McAcPfInstance,
    McAcPfSolution, Objective, ObjectiveTerm, ScucDeviceOutputs, ScucNetworkOutputs, Termination,
};
use powerio_tx::{Branch, Bus, BusId, BusType, GenCost, Generator, Load};

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

fn initial_state(net: &BalancedNetwork) -> powerio_prob::OperatingPoint<BalancedNetwork> {
    let series = powerio_prob::BalancedStateBuilder::new(
        net.clone(),
        vec![TimePoint::new("initial", None).unwrap()],
    )
    .load_active_powers(vec![40.0])
    .generator_active_powers(vec![42.0])
    .build()
    .unwrap();
    series.values()[0].clone()
}

fn round_trip(value: PioValue, kind: &str) -> String {
    let module = PioModule::new(value);
    let text = write_module(&module).unwrap();
    let raw: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(raw["value"]["kind"], kind);
    let back = read_module(&text).unwrap();
    assert_eq!(back.value().kind().as_str(), kind);
    assert_eq!(
        write_module(&back).unwrap(),
        text,
        "{kind} is not byte stable"
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
                .with_initial_state(initial_state(&net)),
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

#[test]
fn the_multiconductor_series_round_trips() {
    let net = mc_network();
    let series = powerio_prob::MulticonductorStateBuilder::new(
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
        PioValue::MulticonductorOperatingPointTimeSeries(series),
        "multiconductor_operating_point_time_series",
    );
    let back = read_module(&text).unwrap();
    let PioValue::MulticonductorOperatingPointTimeSeries(series) = back.value() else {
        panic!("wrong kind");
    };
    assert_eq!(series.len(), 2);
    assert_eq!(
        series.values()[1].terminal_voltage_magnitude("src", "2"),
        Some(239.0)
    );
}

#[test]
fn the_scuc_pair_round_trips_from_the_goc3_fixture() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../powerio-prob/tests/data/goc3_small.json"
    );
    let text = std::fs::read_to_string(path).unwrap();
    let module = powerio_prob::parse_goc3_instance(
        powerio_core::Source::from_bytes("goc3_small.json", text.into_bytes()).unwrap(),
    )
    .unwrap();
    let instance = module.value().clone();
    let periods = instance.inputs().dt.len();
    let buses = instance.network().buses().len();
    let devices =
        instance.inputs().static_data.prod.len() + instance.inputs().static_data.cons.len();
    let doc = round_trip(PioValue::AcScucInstance(instance), "ac_scuc_instance");

    let back = read_module(&doc).unwrap();
    let PioValue::AcScucInstance(instance) = back.value() else {
        panic!("wrong kind");
    };
    let mut network_outputs = ScucNetworkOutputs::default();
    network_outputs.bus_vm = vec![vec![1.0; buses]; periods];
    network_outputs.bus_va = vec![vec![0.0; buses]; periods];
    let mut device_outputs = ScucDeviceOutputs::default();
    device_outputs.on_status = vec![vec![1.0; devices]; periods];
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
    powerio_prob::parse_goc3_instance(
        powerio_core::Source::from_bytes("goc3_small.json", text.into_bytes()).unwrap(),
    )
    .unwrap()
    .into_value()
}

fn full_scuc_outputs(
    periods: usize,
) -> (
    powerio_prob::ScucNetworkOutputs,
    powerio_prob::ScucDeviceOutputs,
) {
    let series = |seed: f64| vec![vec![seed, seed + 0.5]; periods];
    let mut network_outputs = ScucNetworkOutputs::default();
    network_outputs.bus_vm = series(1.0);
    network_outputs.bus_va = series(2.0);
    network_outputs.shunt_step = series(3.0);
    network_outputs.ac_line_on_status = series(4.0);
    network_outputs.transformer_tm = series(5.0);
    network_outputs.transformer_ta = series(6.0);
    network_outputs.transformer_on_status = series(7.0);
    network_outputs.dc_line_pdc_fr = series(8.0);
    network_outputs.dc_line_qdc_fr = series(9.0);
    network_outputs.dc_line_qdc_to = series(10.0);
    let mut device_outputs = ScucDeviceOutputs::default();
    device_outputs.on_status = series(11.0);
    device_outputs.p_on = series(12.0);
    device_outputs.q = series(13.0);
    device_outputs.p_reg_res_up = series(14.0);
    device_outputs.p_reg_res_down = series(15.0);
    device_outputs.p_syn_res = series(16.0);
    device_outputs.p_nsyn_res = series(17.0);
    device_outputs.p_ramp_res_up_online = series(18.0);
    device_outputs.p_ramp_res_down_online = series(19.0);
    device_outputs.q_res_up = series(20.0);
    device_outputs.q_res_down = series(21.0);
    (network_outputs, device_outputs)
}

/// Every output series the runtime structs define survives the round trip,
/// field by field, and the wire spells exactly the series vocabulary the
/// defining crate exports.
#[test]
fn every_scuc_output_series_round_trips_under_its_exported_name() {
    let instance = goc3_instance();
    let periods = instance.inputs().dt.len();
    let (network_outputs, device_outputs) = full_scuc_outputs(periods);
    let solution = AcScucSolution::new(
        Arc::new(instance),
        Termination::Converged,
        network_outputs.clone(),
        device_outputs.clone(),
        Some(4.25e3),
    )
    .unwrap();
    let text = write_module(&PioModule::new(PioValue::AcScucSolution(solution))).unwrap();

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

    let back = read_module(&text).unwrap();
    let PioValue::AcScucSolution(solution) = back.value() else {
        panic!("wrong kind");
    };
    assert_eq!(*solution.network_outputs(), network_outputs);
    assert_eq!(*solution.device_outputs(), device_outputs);
}

/// Every kind string is stable and every fixture on disk rereads. The
/// fixtures are written by the ignored generator below; regenerating them is
/// a deliberate decision.
#[test]
fn committed_calculation_fixtures_reread() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/module-v1");
    let mut kinds = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let module = read_module(&text).unwrap();
        assert_eq!(
            write_module(&module).unwrap(),
            text,
            "{} is not byte stable",
            path.display()
        );
        kinds.push(module.value().kind().as_str().to_string());
    }
    kinds.sort();
    assert_eq!(
        kinds,
        vec![
            "ac_opf_instance",
            "ac_opf_solution",
            "ac_pf_instance",
            "ac_pf_solution",
            "ac_scuc_instance",
            "ac_scuc_solution",
            "dc_opf_instance",
            "dc_opf_solution",
            "dc_pf_instance",
            "dc_pf_solution",
            "mc_ac_opf_instance",
            "mc_ac_opf_solution",
            "mc_ac_pf_instance",
            "mc_ac_pf_solution",
            "multiconductor_operating_point_time_series",
        ]
    );
}

#[test]
#[ignore = "fixture generator"]
#[allow(clippy::too_many_lines)]
fn generate_calculation_fixtures() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/module-v1");
    std::fs::create_dir_all(dir).unwrap();
    let net = network();
    let objective = Objective::default().with_term(ObjectiveTerm::NetworkGeneratorCost);
    let write = |name: &str, value: PioValue| {
        let text = write_module(&PioModule::new(value)).unwrap();
        std::fs::write(format!("{dir}/{name}.pio.json"), text).unwrap();
    };
    write(
        "dc-pf-instance",
        PioValue::DcPfInstance(
            DcPfInstance::from_network(net.clone())
                .unwrap()
                .with_initial_state(initial_state(&net)),
        ),
    );
    write(
        "ac-pf-instance",
        PioValue::AcPfInstance(AcPfInstance::from_network(net.clone()).unwrap()),
    );
    write(
        "dc-opf-instance",
        PioValue::DcOpfInstance(
            DcOpfInstance::from_network(net.clone())
                .unwrap()
                .with_objective(objective.clone()),
        ),
    );
    write(
        "ac-opf-instance",
        PioValue::AcOpfInstance(
            AcOpfInstance::from_network(net.clone())
                .unwrap()
                .with_objective(objective.clone()),
        ),
    );
    let scuc = goc3_instance();
    let scuc_periods = scuc.inputs().dt.len();
    write("ac-scuc-instance", PioValue::AcScucInstance(scuc.clone()));
    let (scuc_network_outputs, scuc_device_outputs) = full_scuc_outputs(scuc_periods);
    write(
        "ac-scuc-solution",
        PioValue::AcScucSolution(
            AcScucSolution::new(
                Arc::new(scuc),
                Termination::Converged,
                scuc_network_outputs,
                scuc_device_outputs,
                Some(4.25e3),
            )
            .unwrap(),
        ),
    );
    write(
        "mc-ac-pf-instance",
        PioValue::McAcPfInstance(McAcPfInstance::from_network(mc_network()).unwrap()),
    );
    write(
        "mc-ac-opf-instance",
        PioValue::McAcOpfInstance(
            McAcOpfInstance::from_network(mc_network())
                .unwrap()
                .with_objective(objective),
        ),
    );
    write(
        "dc-pf-solution",
        PioValue::DcPfSolution(
            DcPfSolution::new(
                Arc::new(DcPfInstance::from_network(net.clone()).unwrap()),
                Termination::Converged,
                vec![0.0, -0.02],
                vec![40.0, -40.0],
                vec![40.0],
                vec![-40.0],
            )
            .unwrap(),
        ),
    );
    write(
        "ac-pf-solution",
        PioValue::AcPfSolution(
            AcPfSolution::new(
                Arc::new(AcPfInstance::from_network(net.clone()).unwrap()),
                Termination::Converged,
                vec![1.01, 0.99],
                vec![0.0, -1.2],
                vec![40.5, -40.0],
                vec![10.4, -10.0],
                vec![40.5],
                vec![10.4],
                vec![-40.0],
                vec![-10.0],
            )
            .unwrap(),
        ),
    );
    write(
        "dc-opf-solution",
        PioValue::DcOpfSolution(
            DcOpfSolution::new(
                Arc::new(DcOpfInstance::from_network(net.clone()).unwrap()),
                Termination::Converged,
                vec![0.0, -0.02],
                vec![40.0, -40.0],
                vec![40.0],
                vec![-40.0],
                vec![40.0],
                412.5,
            )
            .unwrap(),
        ),
    );
    write(
        "ac-opf-solution",
        PioValue::AcOpfSolution(
            AcOpfSolution::new(
                Arc::new(AcOpfInstance::from_network(net.clone()).unwrap()),
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
            )
            .unwrap(),
        ),
    );
    write(
        "mc-ac-pf-solution",
        PioValue::McAcPfSolution(
            McAcPfSolution::new(
                Arc::new(McAcPfInstance::from_network(mc_network()).unwrap()),
                Termination::Converged,
                vec![240.0, 239.9, 240.1],
                vec![0.0, -2.094, 2.094],
                vec![1000.0, 1000.0, 1000.0],
            )
            .unwrap(),
        ),
    );
    write(
        "mc-ac-opf-solution",
        PioValue::McAcOpfSolution(
            McAcOpfSolution::new(
                Arc::new(McAcOpfInstance::from_network(mc_network()).unwrap()),
                Termination::Converged,
                vec![240.0, 239.9, 240.1],
                vec![0.0, -2.094, 2.094],
                vec![1000.0, 1000.0, 1000.0],
                Vec::new(),
                12.5,
            )
            .unwrap(),
        ),
    );
    let series = powerio_prob::MulticonductorStateBuilder::new(
        mc_network(),
        vec![
            TimePoint::new("h0", None).unwrap(),
            TimePoint::new("h1", None).unwrap(),
        ],
    )
    .terminal_voltage_magnitudes(vec![240.0, 240.0, 240.0, 239.0, 239.0, 239.0])
    .build()
    .unwrap();
    write(
        "mc-operating-point-series",
        PioValue::MulticonductorOperatingPointTimeSeries(series),
    );
}
