mod helpers;

use powerio::{
    Destination, EmitOptions, EmitResult, EmittedOutput, Fidelity, ParseOptions, PioModule,
    PioValue, SincalContainer, SincalExperimentalOptions, Source,
};
use std::collections::BTreeMap;

const ARCHIVE: &[u8] = include_bytes!("../../tests/data/sincal/1-LV-rural1--0-sw.sinx");
fn balanced() -> PioModule<PioValue> {
    powerio::parse_with_options(
        Source::from_memory("case.sinx", ARCHIVE.to_vec()).unwrap(),
        &ParseOptions::default().format("sincal-balanced").unwrap(),
    )
    .unwrap()
}
fn bytes(result: EmitResult) -> Vec<u8> {
    let EmittedOutput::Memory { mut artifacts } = result.into_output() else {
        panic!("memory output");
    };
    assert_eq!(artifacts.len(), 1);
    artifacts.remove(0).into_bytes()
}
fn options(container: SincalContainer, levels: BTreeMap<String, f64>) -> EmitOptions {
    let mut request = SincalExperimentalOptions::default();
    request.container = container;
    request.nominal_ll_volts = levels;
    let mut options = EmitOptions::default();
    options.sincal_experimental = Some(request);
    options
}
fn fresh(module: &PioModule<PioValue>, opts: &EmitOptions) -> EmitResult {
    powerio::emit_with_options(
        module,
        "sincal",
        opts,
        Destination::memory("case.out").unwrap(),
    )
    .unwrap()
}

#[test]
fn explicit_fresh_output_and_default_echo_are_separate_and_deterministic() {
    let module = balanced();
    let echoed = powerio::emit_with_options(
        &module,
        "sincal",
        &EmitOptions::default(),
        Destination::memory("copy.sinx").unwrap(),
    )
    .unwrap();
    assert_eq!(echoed.fidelity(), Fidelity::ExactSameFormat);
    assert_eq!(bytes(echoed), ARCHIVE);
    for container in [
        SincalContainer::Sqlite,
        SincalContainer::Archive {
            project_name: "sample".into(),
        },
    ] {
        let opts = options(container, BTreeMap::new());
        let result = fresh(&module, &opts);
        assert_eq!(result.fidelity(), Fidelity::Canonical);
        assert!(
            result
                .diagnostics()
                .iter()
                .any(|d| d.code() == "EMIT.SINCAL.EXPERIMENTAL")
        );
        assert!(
            result
                .diagnostics()
                .iter()
                .any(|d| d.code() == "EMIT.SINCAL.RETAINED_SOURCE_OMITTED")
        );
        let output = bytes(result);
        assert_ne!(output, ARCHIVE);
        assert_eq!(output, bytes(fresh(&module, &opts)));
        let parsed = powerio::parse_with_options(
            Source::from_memory("fresh", output.clone()).unwrap(),
            &ParseOptions::default().format("sincal-balanced").unwrap(),
        )
        .unwrap();
        assert!(matches!(parsed.value(), PioValue::BalancedNetwork(_)));
        assert_eq!(
            bytes(powerio::emit(&parsed, "sincal", Destination::memory("echo").unwrap()).unwrap()),
            output
        );
    }
    assert_eq!(
        bytes(powerio::emit(&module, "sincal", Destination::memory("echo").unwrap()).unwrap()),
        ARCHIVE
    );
    assert!(!powerio::resolve_format("sincal").unwrap().can_emit);
    assert!(!powerio::resolve_format("sincal-balanced").unwrap().can_emit);
}

fn multiconductor() -> PioModule<PioValue> {
    use powerio::dist::{
        Configuration, DistBus, DistLoad, DistLoadVoltageModel, MulticonductorNetwork,
        VoltageSource,
    };
    let mut net = MulticonductorNetwork::new();
    let phases = ["1", "2", "3"].map(str::to_owned).to_vec();
    let mut bus = DistBus::new(
        "bus",
        ["1", "2", "3", "earth", "star"].map(str::to_owned).to_vec(),
    );
    bus.grounded.push("earth".into());
    net.buses_mut().push(bus);
    net.sources_mut().push(
        VoltageSource::new(
            "source",
            "bus",
            phases,
            vec![230.0; 3],
            vec![
                0.0,
                -std::f64::consts::TAU / 3.0,
                std::f64::consts::TAU / 3.0,
            ],
        )
        .with_reference_terminal("star"),
    );
    let mut load = DistLoad::new(
        "load",
        "bus",
        ["1", "2", "3", "earth"].map(str::to_owned).to_vec(),
        Configuration::Wye,
        vec![1000.0, 2000.0, 3000.0],
        vec![100.0, 200.0, 300.0],
    );
    load.voltage_model = DistLoadVoltageModel::ConstantPower {
        v_nom: vec![230.0; 3],
    };
    net.loads_mut().push(load);
    PioModule::new(net.into())
}

#[test]
fn edited_multiconductor_ir_uses_its_own_backend_and_requires_nominal_levels() {
    let mut module = helpers::deserialize_module_text(
        &helpers::serialize_module_text(&multiconductor()).unwrap(),
    )
    .unwrap();
    let PioValue::MulticonductorNetwork(net) = module.value_mut() else {
        panic!("family");
    };
    net.loads_mut()[0].p_nom[1] = 2500.0;
    let missing = options(SincalContainer::Sqlite, BTreeMap::new());
    assert!(
        powerio::emit_with_options(
            &module,
            "sincal",
            &missing,
            Destination::memory("no.db").unwrap()
        )
        .is_err()
    );
    for container in [
        SincalContainer::Sqlite,
        SincalContainer::Archive {
            project_name: "unbalanced".into(),
        },
    ] {
        let opts = options(container, [("bus".into(), 400.0)].into());
        let result = fresh(&module, &opts);
        assert_eq!(result.fidelity(), Fidelity::Canonical);
        assert!(
            result
                .diagnostics()
                .iter()
                .any(|d| d.code() == "EMIT.DIST.SINCAL_EXPERIMENTAL")
        );
        let output = bytes(result);
        let explicit = powerio::emit_with_options(
            &module,
            "sincal-multiconductor",
            &opts,
            Destination::memory("explicit").unwrap(),
        )
        .unwrap();
        assert_eq!(bytes(explicit), output);
        let parsed = powerio::parse_with_options(
            Source::from_memory("fresh", output).unwrap(),
            &ParseOptions::default()
                .format("sincal-multiconductor")
                .unwrap(),
        )
        .unwrap();
        let PioValue::MulticonductorNetwork(recovered) = parsed.value() else {
            panic!("reader changed the electrical family");
        };
        let powers = recovered
            .loads()
            .iter()
            .map(|l| l.p_nom[0])
            .collect::<Vec<_>>();
        assert_eq!(powers, [1000.0, 2500.0, 3000.0]);
        assert!(recovered.sources()[0].reference_terminal.is_some());
        let error = powerio::emit_with_options(
            &module,
            "sincal-balanced",
            &opts,
            Destination::memory("wrong.db").unwrap(),
        )
        .unwrap_err();
        assert!(
            error
                .diagnostics()
                .iter()
                .any(|d| d.code() == "REQUEST.EMIT.INVALID_OPTIONS")
        );
    }
}

#[test]
fn default_options_preserve_other_emitters_and_experimental_options_do_not_leak() {
    let module = balanced();
    let a = powerio::emit(&module, "matpower", Destination::memory("case.m").unwrap()).unwrap();
    let b = powerio::emit_with_options(
        &module,
        "matpower",
        &EmitOptions::default(),
        Destination::memory("case.m").unwrap(),
    )
    .unwrap();
    assert_eq!(a.fidelity(), b.fidelity());
    assert_eq!(a.diagnostics(), b.diagnostics());
    assert_eq!(bytes(a), bytes(b));
    for format in ["matpower", "dss", "not-a-format", "sincal-multiconductor"] {
        let error = powerio::emit_with_options(
            &module,
            format,
            &options(SincalContainer::Sqlite, BTreeMap::new()),
            Destination::memory("no").unwrap(),
        )
        .unwrap_err();
        assert!(
            error
                .diagnostics()
                .iter()
                .any(|d| d.code() == "REQUEST.EMIT.INVALID_OPTIONS")
        );
    }
    let error = powerio::emit_with_options(
        &module,
        "sincal",
        &options(SincalContainer::Sqlite, [("unused".into(), 400.0)].into()),
        Destination::memory("no").unwrap(),
    )
    .unwrap_err();
    assert!(
        error
            .diagnostics()
            .iter()
            .any(|d| d.code() == "REQUEST.EMIT.INVALID_OPTIONS")
    );
}

#[test]
fn invalid_profile_and_container_never_create_or_replace_a_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("candidate.sinx");
    let opts = options(
        SincalContainer::Archive {
            project_name: "../escape".into(),
        },
        BTreeMap::new(),
    );
    let module = balanced();
    let error = powerio::emit_with_options(&module, "sincal", &opts, &path).unwrap_err();
    assert!(
        error
            .diagnostics()
            .iter()
            .any(|d| d.code() == "EMIT.MODULE.SINCAL_PACKAGING_FAILED")
    );
    assert!(!path.exists());
    std::fs::write(&path, b"existing artifact").unwrap();
    assert!(powerio::emit_with_options(&module, "sincal", &opts, &path).is_err());
    let mut unsupported = module.clone();
    let PioValue::BalancedNetwork(net) = unsupported.value_mut() else {
        panic!("family");
    };
    net.branches_mut()[0].r = f64::NAN;
    assert!(
        powerio::emit_with_options(
            &unsupported,
            "sincal",
            &options(SincalContainer::Sqlite, BTreeMap::new()),
            &path
        )
        .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"existing artifact");
    let collision = powerio::emit_with_options(
        &module,
        "sincal",
        &options(SincalContainer::Sqlite, BTreeMap::new()),
        &path,
    )
    .unwrap_err();
    assert!(
        collision
            .diagnostics()
            .iter()
            .any(|d| d.code() == "REQUEST.OUTPUT.COLLISION")
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"existing artifact");
    let path = directory.path().join("explicit-container.out");
    let result = powerio::emit_with_options(
        &module,
        "sincal",
        &options(SincalContainer::Sqlite, BTreeMap::new()),
        &path,
    )
    .unwrap();
    assert_eq!(result.fidelity(), Fidelity::Canonical);
    assert!(
        std::fs::read(&path)
            .unwrap()
            .starts_with(b"SQLite format 3\0")
    );
}

#[test]
fn typed_balanced_modules_and_edited_ir_write_but_non_network_values_do_not() {
    let module = balanced();
    let mut restored =
        helpers::deserialize_module_text(&helpers::serialize_module_text(&module).unwrap())
            .unwrap();
    let PioValue::BalancedNetwork(net) = restored.value_mut() else {
        panic!("family");
    };
    net.loads_mut()[0].p *= 1.5;
    let expected = net.loads()[0].p;
    let typed = restored.map_value(|v| match v {
        PioValue::BalancedNetwork(net) => net,
        _ => panic!("family"),
    });
    let result = powerio::emit_with_options(
        &typed,
        "sincal-balanced",
        &options(SincalContainer::Sqlite, BTreeMap::new()),
        Destination::memory("typed.db").unwrap(),
    )
    .unwrap();
    let reread = powerio::parse_with_options(
        Source::from_memory("typed.db", bytes(result)).unwrap(),
        &ParseOptions::default().format("sincal-balanced").unwrap(),
    )
    .unwrap();
    let PioValue::BalancedNetwork(net) = reread.value() else {
        panic!("family");
    };
    assert!((net.loads()[0].p - expected).abs() < 1e-12);
    let other = PioModule::new(powerio::GeoLayer {
        space: powerio::CoordinateSpace::Diagram { canvas: None },
        kind: None,
        features: vec![],
    });
    let error = powerio::emit_with_options(
        &other,
        "sincal",
        &options(SincalContainer::Sqlite, BTreeMap::new()),
        Destination::memory("unsupported.db").unwrap(),
    )
    .unwrap_err();
    assert!(
        error
            .diagnostics()
            .iter()
            .any(|d| d.code() == "REQUEST.MODULE.WRONG_MODEL_KIND")
    );
}

#[test]
fn open_phase_switch_survives_ir_edit_and_experimental_native_output() {
    let mut original = multiconductor();
    let PioValue::MulticonductorNetwork(net) = original.value_mut() else {
        panic!("family")
    };
    net.buses_mut()
        .push(powerio::dist::DistBus::new("isolated", vec!["2".into()]));
    net.switches_mut().push(powerio::dist::DistSwitch::new(
        "open-phase",
        "bus",
        "isolated",
        vec!["2".into()],
        vec!["2".into()],
        true,
    ));
    let mut restored =
        helpers::deserialize_module_text(&helpers::serialize_module_text(&original).unwrap())
            .unwrap();
    let PioValue::MulticonductorNetwork(net) = restored.value_mut() else {
        panic!("family")
    };
    net.loads_mut()[0].p_nom[0] = 1500.0;
    for container in [
        SincalContainer::Sqlite,
        SincalContainer::Archive {
            project_name: "open-switch".into(),
        },
    ] {
        let opts = options(
            container,
            [("bus".into(), 400.0), ("isolated".into(), 400.0)].into(),
        );
        let output = bytes(fresh(&restored, &opts));
        let recovered = powerio::parse_with_options(
            Source::from_memory("fresh", output).unwrap(),
            &ParseOptions::default()
                .format("sincal-multiconductor")
                .unwrap(),
        )
        .unwrap();
        let PioValue::MulticonductorNetwork(net) = recovered.value() else {
            panic!("family")
        };
        let open: Vec<_> = net.switches().iter().filter(|s| s.open).collect();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].terminal_map_from, ["2"]);
        assert_eq!(net.loads()[0].p_nom, [1500.0]);
    }
}
