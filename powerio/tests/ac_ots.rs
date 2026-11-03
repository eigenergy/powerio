//! AC optimal transmission switching through the facade: the module
//! constructions and their records, PowerIO IR round trips, the
//! fixed-topology AC solution assembled from IR as a binding does, and the
//! materialized network a grid format writes.

use powerio::{
    AcOtsInstance, AcOtsOptions, BranchCurrentLimit, BranchSelection, BranchSwitchingCost,
    CurrentLimitPolicy, PioValue,
};
use powerio_core::PioModule;
use powerio_prob::ObjectiveTerm;

mod helpers;
use helpers::{deserialize_module_text as deserialize, serialize_module_text as serialize};

/// case9 with its second branch (4-5) initially open.
fn case9() -> PioModule<PioValue> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/case9.m");
    powerio::parse(path).unwrap().map_value(|value| {
        let PioValue::BalancedNetwork(mut network) = value else {
            panic!("case9 parses to a balanced network")
        };
        network.branches_mut()[1].in_service = false;
        PioValue::BalancedNetwork(network)
    })
}

fn ids(instance: &AcOtsInstance) -> Vec<String> {
    instance.branch_order().map(str::to_owned).collect()
}

fn explicit_options(ids: &[String]) -> AcOtsOptions {
    let mut options = AcOtsOptions::default();
    options.switchable = BranchSelection::All;
    options.max_open = Some(1);
    options.max_switched = Some(2);
    options.switching_costs = vec![
        BranchSwitchingCost::new(ids[1].clone(), 4.0, 7.0),
        BranchSwitchingCost::new(ids[2].clone(), 2.0, 5.0),
    ];
    options.current_limits = CurrentLimitPolicy::Stated;
    options.stated_current_limits = vec![BranchCurrentLimit::new(ids[1].clone(), Some(1.25), None)];
    options
}

fn closed_example() -> Vec<bool> {
    let mut closed = vec![true; 9];
    closed[2] = false;
    closed
}

fn ots_module() -> PioModule<PioValue> {
    let defaults = powerio::to_ac_ots_instance(&case9()).unwrap();
    let options = explicit_options(&ids(defaults.value()));
    powerio::to_ac_ots_instance_with_options(&defaults.map_value(PioValue::from), options)
        .unwrap()
        .map_value(PioValue::from)
}

fn ots_instance(module: &PioModule<PioValue>) -> &AcOtsInstance {
    let PioValue::AcOtsInstance(instance) = module.value() else {
        panic!("the module holds {}", module.value().type_name())
    };
    instance
}

fn switching_terms(instance: &AcOtsInstance) -> usize {
    instance
        .objective()
        .terms()
        .iter()
        .filter(|term| **term == ObjectiveTerm::BranchSwitchingCost)
        .count()
}

#[test]
fn constructions_keep_the_module_records_and_add_the_term_once() {
    let network = case9();
    let defaults = powerio::to_ac_ots_instance(&network).unwrap();
    assert_eq!(defaults.value().branches().len(), 9);
    assert_eq!(defaults.value().options().switchable.kind_name(), "only");
    assert_eq!(switching_terms(defaults.value()), 1);
    assert_eq!(defaults.sources(), network.sources());
    let entry = defaults.history().last().unwrap();
    assert_eq!(entry.name(), "to_ac_ots_instance");
    assert_eq!(entry.input_type(), Some("powerio.BalancedNetwork"));
    assert_eq!(entry.output_type(), Some("powerio.AcOtsInstance"));

    // An AC OTS module comes back unchanged.
    let defaults = defaults.map_value(PioValue::from);
    let again = powerio::to_ac_ots_instance(&defaults).unwrap();
    assert_eq!(ids(again.value()), ids(ots_instance(&defaults)));
    assert_eq!(again.history().len(), defaults.history().len());

    // From an AC OPF module, and reconfigured: the term stays single.
    let opf = powerio::to_ac_opf_instance(&network)
        .unwrap()
        .map_value(PioValue::from);
    let from_opf = powerio::to_ac_ots_instance(&opf).unwrap();
    assert_eq!(
        from_opf.history().last().unwrap().input_type(),
        Some("powerio.AcOpfInstance")
    );
    let module = ots_module();
    let instance = ots_instance(&module);
    assert_eq!(switching_terms(instance), 1);
    assert_eq!(instance.options().max_switched, Some(2));
    assert!(instance.branches().iter().all(|branch| branch.switchable));

    // The low-level construction keeps the base objective exactly.
    let PioValue::AcOpfInstance(base) = opf.value() else {
        unreachable!()
    };
    let explicit = powerio::ac_ots_instance_from_base(&opf, base, AcOtsOptions::default()).unwrap();
    assert_eq!(explicit.value().objective(), base.objective());
}

#[test]
fn an_instance_round_trips_through_powerio_ir() {
    let module = ots_module();
    let text = serialize(&module).unwrap();
    let document: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(document["value"]["type"], "powerio.AcOtsInstance");
    let options = &document["value"]["data"]["options"];
    assert_eq!(options["switchable"]["select"], "all");
    assert_eq!(options["islanding"], "forbidden");
    assert_eq!(options["current_limits"], "stated");
    assert_eq!(options["max_open"], 1);
    assert_eq!(
        document["value"]["data"]["base"]["objective"]["terms"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["term"],
        "branch_switching_cost"
    );

    let restored = deserialize(&text).unwrap();
    let (before, after) = (ots_instance(&module), ots_instance(&restored));
    assert_eq!(after.branches(), before.branches());
    assert_eq!(after.options(), before.options());
    assert_eq!(after.objective(), before.objective());
    assert_eq!(serialize(&restored).unwrap(), text);
}

/// The AC OPF solution document a binding writes for the fixed-topology
/// problem, as PowerIO.jl's test assembles it: the fixed instance's stored
/// data with synthetic values that are not an AC feasibility certificate.
fn ac_solution_data(fixed: &PioModule<PioValue>, objective: f64) -> serde_json::Value {
    let document: serde_json::Value = serde_json::from_str(&serialize(fixed).unwrap()).unwrap();
    let PioValue::AcOpfInstance(instance) = fixed.value() else {
        panic!("fix_ac_ots_topology returns an AC OPF instance")
    };
    let network = instance.network();
    let (buses, branches, generators) = (
        network.buses().len(),
        network.branches().len(),
        network.generators().len(),
    );
    serde_json::json!({
        "instance": document["value"]["data"],
        "termination": {"kind": "not_reported"},
        "residuals": {"max_active_power_mismatch": null, "max_reactive_power_mismatch": null},
        "bus_voltage_magnitude": vec![1.01; buses],
        "bus_voltage_angle": vec![0.0; buses],
        "bus_active_injection": vec![0.0; buses],
        "bus_reactive_injection": vec![0.0; buses],
        "branch_from_active_flow": vec![0.0; branches],
        "branch_to_active_flow": vec![0.0; branches],
        "branch_from_reactive_flow": vec![0.0; branches],
        "branch_to_reactive_flow": vec![0.0; branches],
        "generator_active_power": vec![12.0; generators],
        "generator_reactive_power": vec![3.0; generators],
        "three_winding_transformer_terminal_powers": [],
        "objective": objective,
    })
}

fn with_value(text: &str, type_name: &str, data: &serde_json::Value) -> String {
    let mut document: serde_json::Value = serde_json::from_str(text).unwrap();
    document["value"] = serde_json::json!({"type": type_name, "data": data.clone()});
    document.to_string()
}

#[test]
fn a_solution_assembled_from_ir_answers_the_fixed_topology() {
    let module = ots_module();
    let instance = ots_instance(&module);
    let closed = closed_example();
    let fixed = powerio::fix_ac_ots_topology(&module, instance, &closed)
        .unwrap()
        .map_value(PioValue::from);
    assert!(
        !fixed.diagnostics().is_empty(),
        "fixing the topology reports what the AC OPF leaves out"
    );
    let PioValue::AcOpfInstance(fixed_instance) = fixed.value() else {
        unreachable!()
    };
    let statuses: Vec<bool> = fixed_instance
        .network()
        .branches()
        .iter()
        .map(|branch| branch.in_service)
        .collect();
    assert_eq!(statuses, closed);

    // The AC solution crosses PowerIO IR before it is attached, as a binding
    // that solves the fixed problem elsewhere hands it back.
    let fixed_text = serialize(&fixed).unwrap();
    let ac_data = ac_solution_data(&fixed, 40.0);
    let ac_module =
        deserialize(&with_value(&fixed_text, "powerio.AcOpfSolution", &ac_data)).unwrap();
    let PioValue::AcOpfSolution(ac) = ac_module.value() else {
        panic!("the document holds an AC OPF solution")
    };
    let solution = powerio::ac_ots_solution(&module, instance, closed.clone(), ac.clone()).unwrap();
    let solution = solution.value();
    assert_eq!(solution.objective(), 49.0);
    assert_eq!(solution.branches_open(), 1);
    assert_eq!(solution.branches_switched(), 2);
    assert!(
        powerio::ac_ots_solution(&module, instance, vec![true; 9], ac.clone()).is_err(),
        "a different topology is a different problem"
    );

    // The AC OTS solution document reads back and checks the nested
    // fixed-topology solution exactly.
    let instance_text = serialize(&module).unwrap();
    let instance_document: serde_json::Value = serde_json::from_str(&instance_text).unwrap();
    let mut solution_data = serde_json::json!({
        "instance": instance_document["value"]["data"],
        "branch_closed": closed,
        "ac_solution": ac_data,
    });
    let solution_text = with_value(&instance_text, "powerio.AcOtsSolution", &solution_data);
    let restored = deserialize(&solution_text).unwrap();
    let PioValue::AcOtsSolution(restored_solution) = restored.value() else {
        panic!("the document holds an AC OTS solution")
    };
    assert_eq!(restored_solution.objective(), 49.0);
    assert_eq!(restored_solution.branch_closed(), closed.as_slice());
    let again = deserialize(&serialize(&restored).unwrap()).unwrap();
    let PioValue::AcOtsSolution(again) = again.value() else {
        unreachable!()
    };
    assert_eq!(again.branch_closed(), closed.as_slice());
    solution_data["branch_closed"][2] = serde_json::Value::Bool(true);
    let tampered = with_value(&instance_text, "powerio.AcOtsSolution", &solution_data);
    assert!(deserialize(&tampered).is_err());
}

#[test]
fn a_materialized_solution_writes_its_final_topology() {
    let module = ots_module();
    let instance = ots_instance(&module);
    let closed = closed_example();
    let fixed = powerio::fix_ac_ots_topology(&module, instance, &closed)
        .unwrap()
        .map_value(PioValue::from);
    let ac_module = deserialize(&with_value(
        &serialize(&fixed).unwrap(),
        "powerio.AcOpfSolution",
        &ac_solution_data(&fixed, 40.0),
    ))
    .unwrap();
    let PioValue::AcOpfSolution(ac) = ac_module.value() else {
        unreachable!()
    };
    let solution = powerio::ac_ots_solution(&module, instance, closed.clone(), ac.clone())
        .unwrap()
        .map_value(PioValue::from);
    let PioValue::AcOtsSolution(ots) = solution.value() else {
        unreachable!()
    };
    // The solution's network keeps the initial statuses.
    assert!(!ots.network().branches()[1].in_service);
    assert!(ots.network().branches()[2].in_service);

    // Emitting the solution itself is refused: materialize it explicitly.
    let refused = powerio::emit(
        &solution,
        "matpower",
        powerio::Destination::memory("case.m").unwrap(),
    );
    assert!(refused.is_err());

    let materialized = powerio::materialize_ac_ots_solution(&solution, ots).unwrap();
    let statuses: Vec<bool> = materialized
        .value()
        .branches()
        .iter()
        .map(|branch| branch.in_service)
        .collect();
    assert_eq!(statuses, closed);
    assert!(
        materialized
            .value()
            .buses()
            .iter()
            .all(|bus| (bus.vm - 1.01).abs() < 1e-12)
    );
    let materialized = materialized.map_value(PioValue::from);
    let written = powerio::emit(
        &materialized,
        "matpower",
        powerio::Destination::memory("case.m").unwrap(),
    )
    .unwrap();
    let powerio::EmittedOutput::Memory { mut artifacts } = written.into_output() else {
        unreachable!("a memory destination returns memory artifacts")
    };
    let bytes = artifacts.pop().unwrap().into_bytes();
    let reparsed = powerio::parse(powerio::Source::from_memory("case.m", bytes).unwrap()).unwrap();
    let PioValue::BalancedNetwork(network) = reparsed.value() else {
        panic!("MATPOWER parses to a balanced network")
    };
    assert!(!network.branches()[2].in_service);
    assert!(network.branches()[1].in_service);
}
