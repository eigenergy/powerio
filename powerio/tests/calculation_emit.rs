use std::sync::Arc;

use powerio::{
    AcOpfInstance, AcOpfSolution, DcOpfInstance, PioModule, PioValue, Source, Termination, emit,
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
