mod helpers;

use powerio::{Destination, EmittedOutput, Fidelity, FormatId, ParseOptions, PioValue, Source};

fn source(edit: &str) -> Source {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    connection
        .execute_batch(include_str!(
            "../../tests/data/sincal/synthetic-multiconductor.sql"
        ))
        .unwrap();
    connection.execute_batch(edit).unwrap();
    Source::from_memory(
        "synthetic.db",
        connection.serialize("main").unwrap().to_vec(),
    )
    .unwrap()
}

fn parse(edit: &str) -> powerio::PioModule<PioValue> {
    powerio::parse_with_options(
        source(edit),
        &ParseOptions::default()
            .format("sincal-multiconductor")
            .unwrap(),
    )
    .unwrap()
}

fn bytes(result: powerio::EmitResult) -> Vec<u8> {
    let EmittedOutput::Memory { mut artifacts } = result.into_output() else {
        panic!("memory")
    };
    assert_eq!(artifacts.len(), 1);
    artifacts.remove(0).into_bytes()
}

#[test]
fn explicit_family_and_binary_echo_preserve_conductors_without_balancing() {
    for edit in [
        "",
        "UPDATE Load SET P1=0.001,P2=0.001,P3=0.001,Q1=0,Q2=0,Q3=0",
    ] {
        let module = parse(edit);
        let PioValue::MulticonductorNetwork(net) = module.value() else {
            panic!("conductor family")
        };
        assert_eq!(net.loads().len(), 1);
        powerio_dist::require_electrical_readiness(net).unwrap();
        let admittance =
            powerio_matrix::matrix::multiconductor::calc_multiconductor_admittance_matrix(net)
                .unwrap();
        assert!(admittance.diagnostics().is_empty());
        powerio::to_mc_ac_pf_instance(&module).unwrap();
        for format in ["sincal", "sincal-multiconductor", "SINCAL_MULTICONDUCTOR"] {
            assert!(!powerio::resolve_format(format).unwrap().can_emit);
            let result =
                powerio::emit(&module, format, Destination::memory("copy.db").unwrap()).unwrap();
            assert_eq!(result.fidelity(), Fidelity::ExactSameFormat);
            assert_eq!(
                bytes(result),
                module.source().unwrap().primary_buffer().unwrap().bytes()
            );
        }
        assert!(
            powerio::emit(
                &module,
                "sincal-balanced",
                Destination::memory("bad.db").unwrap()
            )
            .is_err()
        );
    }
    let archive = include_bytes!("../../tests/data/sincal/1-LV-rural1--0-sw.sinx");
    let native = Source::from_memory("case.sinx", archive.to_vec()).unwrap();
    let balanced = powerio::parse_with_options(
        native.clone(),
        &ParseOptions::default().format("sincal-balanced").unwrap(),
    )
    .unwrap();
    assert!(
        powerio::emit(
            &balanced,
            "sincal-multiconductor",
            Destination::memory("bad.sinx").unwrap()
        )
        .is_err()
    );
    let refused = powerio::parse_with_options(
        native,
        &ParseOptions::default()
            .format("sincal-multiconductor")
            .unwrap(),
    )
    .unwrap_err();
    assert!(
        refused
            .diagnostics()
            .iter()
            .any(|d| d.code() == "PARSE.DIST.SINCAL")
    );
    let module = parse("");
    let PioValue::MulticonductorNetwork(net) = module.value() else {
        unreachable!()
    };
    assert_eq!(net.loads()[0].p_nom, [2000.0, 4000.0, 6000.0]);
}

#[test]
fn edits_ir_and_cross_format_emission_keep_existing_fidelity_contract() {
    let original = parse("");
    let restored =
        helpers::deserialize_module_text(&helpers::serialize_module_text(&original).unwrap())
            .unwrap();
    let (PioValue::MulticonductorNetwork(a), PioValue::MulticonductorNetwork(b)) =
        (original.value(), restored.value())
    else {
        panic!("IR family changed")
    };
    assert_eq!(
        serde_json::to_value(a).unwrap(),
        serde_json::to_value(b).unwrap()
    );
    let mut edited = original.clone();
    let PioValue::MulticonductorNetwork(net) = edited.value_mut() else {
        unreachable!()
    };
    net.loads_mut()[0].p_nom[0] += 100.0;
    for module in [&edited, &restored] {
        assert!(powerio::emit(module, "sincal", Destination::memory("bad.db").unwrap()).is_err());
        let result = powerio::emit(
            module,
            "pmd-json",
            Destination::memory("case.json").unwrap(),
        )
        .unwrap();
        assert_eq!(
            result
                .diagnostics()
                .iter()
                .filter(|d| matches!(
                    d.code(),
                    "EMIT.SINCAL.RETAINED_SOURCE_OMITTED"
                        | "EMIT.DIST.SINCAL_RETAINED_SOURCE_OMITTED"
                ))
                .count(),
            1
        );
        let reloaded =
            powerio::parse(Source::from_memory("case.json", bytes(result)).unwrap()).unwrap();
        let PioValue::MulticonductorNetwork(net) = reloaded.value() else {
            panic!("conductor family")
        };
        let PioValue::MulticonductorNetwork(expected) = module.value() else {
            unreachable!()
        };
        assert_eq!(net.loads()[0].p_nom, expected.loads()[0].p_nom);
    }
}

#[test]
fn invalid_components_and_conflicting_options_never_fall_back() {
    let error = powerio::parse_with_options(
        source("UPDATE Load SET Flag_LoadType=4"),
        &ParseOptions::default()
            .format("sincal-multiconductor")
            .unwrap(),
    )
    .unwrap_err();
    assert!(
        error
            .diagnostics()
            .iter()
            .any(|d| d.code() == "PARSE.DIST.SINCAL")
    );
    assert!(error.retained_source().is_some());
    for token in ["sincal", "sincal-balanced", "dss"] {
        let mut options = ParseOptions::default().format(token).unwrap();
        options.sincal_multiconductor = Some(powerio_dist::SincalReadOptions::default());
        let error = powerio::parse_with_options(source(""), &options).unwrap_err();
        assert!(
            error
                .diagnostics()
                .iter()
                .any(|d| d.code() == "REQUEST.PARSE.SINCAL_OPTIONS_PROFILE")
        );
    }
    let serialized = helpers::serialize_module_text(&parse("")).unwrap();
    let relabelled = Source::from_memory("case.json", serialized.into_bytes())
        .unwrap()
        .with_format(FormatId::new("sincal-multiconductor").unwrap());
    let restored = powerio::deserialize(relabelled).unwrap();
    assert!(powerio::emit(&restored, "sincal", Destination::memory("bad.db").unwrap()).is_err());
}

#[test]
fn native_isolated_bus_is_retained_and_generic_calculation_requires_resolution() {
    let module = parse(
        "INSERT INTO Node VALUES(40,1,'isolated',2,0); UPDATE Terminal SET Node_ID=40 WHERE Terminal_ID=41",
    );
    let PioValue::MulticonductorNetwork(net) = module.value() else {
        panic!("conductor family")
    };
    assert!(net.buses().iter().any(|bus| bus.id == "40"));
    let admittance =
        powerio_matrix::matrix::multiconductor::calc_multiconductor_admittance_matrix(net).unwrap();
    assert!(admittance.diagnostics().is_empty());
    let error = powerio::to_mc_ac_pf_instance(&module).unwrap_err();
    // The load port is the first bus in this island; native bus 40 remains.
    assert!(error.diagnostics().iter().any(|d| {
        d.code() == "BUILD.INSTANCE.SHAPE_MISMATCH"
            && d.message()
                .contains("island containing bus `sincal:load:31` has no voltage source")
    }));
}
