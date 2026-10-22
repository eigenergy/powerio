use std::sync::Arc;

use powerio::{
    AcOpfInstance, AcOpfSolution, DcOpfInstance, LinDist3FlowBuildOptions, LinDist3FlowOpfInstance,
    LinDist3FlowOpfSolution, LinDist3FlowOpfValues, LinDist3FlowPfInstance, PioModule, PioValue,
    Source, Termination, emit,
};
use powerio_core::{Destination, EmittedOutput};

fn network() -> powerio::BalancedNetwork {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/case9.m");
    let module = powerio::parse_with_options(
        Source::open(path).unwrap(),
        &powerio::ParseOptions::default().format("matpower").unwrap(),
    )
    .unwrap();
    let PioValue::BalancedNetwork(network) = module.into_value() else {
        panic!("case9 must parse as a balanced network");
    };
    network
}

fn memory_bytes(result: &powerio_core::EmitResult) -> Vec<u8> {
    let EmittedOutput::Memory { artifacts } = result.output() else {
        panic!("a memory destination must return memory artifacts");
    };
    assert_eq!(artifacts.len(), 1);
    artifacts[0].bytes().to_vec()
}

fn multiconductor_network() -> powerio::MulticonductorNetwork {
    let mut network = powerio::MulticonductorNetwork::named("lindist3flow-emit");
    network
        .buses_mut()
        .push(powerio::dist::DistBus::new("source", vec!["a".to_owned()]));
    network
        .sources_mut()
        .push(powerio::dist::VoltageSource::new(
            "grid",
            "source",
            vec!["a".to_owned()],
            vec![230.0],
            vec![0.0],
        ));
    network
}

#[test]
fn calculation_instance_emits_its_network_with_an_explicit_diagnostic() {
    let instance = DcOpfInstance::from_network(network()).unwrap();
    let result = emit(
        &PioModule::new(instance),
        "matpower",
        Destination::memory("case.m").unwrap(),
    )
    .unwrap();

    assert!(
        std::str::from_utf8(&memory_bytes(&result))
            .unwrap()
            .contains("mpc.baseMVA")
    );
    assert!(
        result
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code() == "EMIT.CALCULATION.DATA_OMITTED")
    );
}

#[test]
fn ac_opf_solution_emits_supported_solved_network_values() {
    let instance = Arc::new(AcOpfInstance::from_network(network()).unwrap());
    let buses = instance.network().buses().len();
    let branches = instance.network().branches().len();
    let generators = instance.network().generators().len();
    let solution = AcOpfSolution::new(
        Arc::clone(&instance),
        Termination::Converged,
        vec![1.02; buses],
        vec![3.0; buses],
        vec![0.0; buses],
        vec![0.0; buses],
        vec![11.0; branches],
        vec![1.5; branches],
        vec![-10.5; branches],
        vec![-1.0; branches],
        vec![25.0; generators],
        vec![4.0; generators],
        123.0,
        Vec::new(),
    )
    .unwrap();
    let result = emit(
        &PioModule::new(solution),
        "powermodels-json",
        Destination::memory("case.json").unwrap(),
    )
    .unwrap();

    assert!(
        result
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code() == "EMIT.SOLUTION.DATA_OMITTED")
    );
    let parsed = powerio::parse_with_options(
        Source::from_memory("case.json", memory_bytes(&result)).unwrap(),
        &powerio::ParseOptions::default()
            .format("powermodels-json")
            .unwrap(),
    )
    .unwrap();
    let PioValue::BalancedNetwork(network) = parsed.value() else {
        panic!("PowerModels JSON must parse as a balanced network");
    };
    assert!(
        network
            .buses()
            .iter()
            .all(|bus| (bus.vm - 1.02).abs() < 1e-12)
    );
    assert!(
        network
            .buses()
            .iter()
            .all(|bus| (bus.va - 3.0).abs() < 1e-12)
    );
    assert!(
        network
            .generators()
            .iter()
            .all(|generator| (generator.pg - 25.0).abs() < 1e-12
                && (generator.qg - 4.0).abs() < 1e-12)
    );
    assert!(network.branches().iter().all(|branch| {
        branch.solution == Some(powerio::BranchSolution::new(11.0, 1.5, -10.5, -1.0))
    }));
}

#[test]
fn fresh_goc3_problem_emission_is_not_claimed() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tests/data/goc3/goc3_small.json"
    );
    let parsed = powerio::parse_with_options(
        Source::open(path).unwrap(),
        &powerio::ParseOptions::default()
            .format("goc3-json")
            .unwrap(),
    )
    .unwrap();
    let PioValue::AcScucInstance(instance) = parsed.into_value() else {
        panic!("GOC3 problem data must parse as an AC SCUC instance");
    };
    let error = emit(
        &PioModule::new(instance),
        "goc3-json",
        Destination::memory("problem.json").unwrap(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("powerio.AcScucInstance"));
    assert!(error.to_string().contains("goc3-json"));
}

#[test]
fn lindist3flow_instance_and_solution_emit_their_network_with_diagnostics() {
    let instance = Arc::new(
        LinDist3FlowOpfInstance::from_network(
            multiconductor_network(),
            LinDist3FlowBuildOptions::default(),
        )
        .unwrap(),
    );
    let result = emit(
        &PioModule::new(instance.as_ref().clone()),
        "bmopf-json@0.2.0",
        Destination::memory("instance.json").unwrap(),
    )
    .unwrap();
    assert!(
        result
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code() == "EMIT.CALCULATION.DATA_OMITTED")
    );
    assert!(
        std::str::from_utf8(&memory_bytes(&result))
            .unwrap()
            .contains("lindist3flow-emit")
    );

    let mut values = LinDist3FlowOpfValues::default();
    values.terminal_voltage_magnitude_squared = vec![52_900.0];
    values.source_active_power = vec![0.0];
    values.source_reactive_power = vec![0.0];
    let solution =
        LinDist3FlowOpfSolution::new(instance, Termination::Converged, values, 0.0).unwrap();
    let result = emit(
        &PioModule::new(solution),
        "bmopf-json@0.2.0",
        Destination::memory("solution.json").unwrap(),
    )
    .unwrap();
    assert!(
        result
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code() == "EMIT.SOLUTION.DATA_OMITTED")
    );
}

#[test]
fn lindist3flow_instances_emit_the_source_network_not_the_prepared_one() {
    let terminal = vec!["a".to_owned()];
    let mut network = multiconductor_network();
    network
        .buses_mut()
        .push(powerio::dist::DistBus::new("load", terminal.clone()));
    network
        .line_codes_mut()
        .push(powerio::dist::DistLineCode::new(
            "code",
            vec![vec![0.1]],
            vec![vec![0.05]],
        ));
    network.lines_mut().push(powerio::dist::DistLine::new(
        "feeder",
        "source",
        "load",
        terminal.clone(),
        terminal.clone(),
        "code",
        10.0,
    ));
    network
        .capacitors_mut()
        .push(powerio::dist::DistCapacitor::new(
            "bank",
            "load",
            terminal,
            powerio::dist::Configuration::Wye,
            1_000.0,
            230.0,
        ));

    // Preparation lowers the capacitor to a synthetic `__l3f-` shunt; the
    // emitted network is still the one the caller supplied.
    let options = LinDist3FlowBuildOptions::default()
        .with_unsupported(powerio::LinDist3FlowUnsupported::Lower);
    let opf = LinDist3FlowOpfInstance::from_network(network.clone(), options).unwrap();
    let pf = LinDist3FlowPfInstance::from_network(network, options).unwrap();
    assert_eq!(opf.network().capacitors().as_slice(), []);
    for module in [
        PioModule::new(PioValue::from(opf)),
        PioModule::new(PioValue::from(pf)),
    ] {
        let result = emit(&module, "dss", Destination::memory("case.dss").unwrap()).unwrap();
        let text = String::from_utf8(memory_bytes(&result)).unwrap();
        assert!(
            text.to_ascii_lowercase().contains("capacitor.bank"),
            "{text}"
        );
        assert!(!text.contains("__l3f-"), "{text}");
    }
}
