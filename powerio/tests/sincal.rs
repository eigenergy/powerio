mod helpers;

use powerio::{Destination, EmittedOutput, Fidelity, ParseOptions, PioValue, Source};

const ARCHIVE: &[u8] = include_bytes!("../../tests/data/sincal/1-LV-rural1--0-sw.sinx");

fn source() -> Source {
    Source::from_memory("case.sinx", ARCHIVE.to_vec()).unwrap()
}
fn parsed() -> powerio::PioModule<PioValue> {
    powerio::parse_with_options(
        source(),
        &ParseOptions::default().format("sincal-balanced").unwrap(),
    )
    .unwrap()
}
fn bytes(result: powerio::EmitResult) -> Vec<u8> {
    let EmittedOutput::Memory { mut artifacts } = result.into_output() else {
        panic!("memory output")
    };
    assert_eq!(artifacts.len(), 1);
    artifacts.remove(0).into_bytes()
}

#[test]
fn balanced_profile_produces_the_existing_network_type_and_retained_binary_source() {
    let module = parsed();
    let PioValue::BalancedNetwork(net) = module.value() else {
        panic!("balanced profile type")
    };
    assert_eq!(net.buses().len(), 15);
    assert_eq!(net.loads().len(), 13);
    assert_eq!(net.branches().len(), 14);
    assert_eq!(net.generators().len(), 5);
    assert_eq!(net.source_format(), powerio::SourceFormat::Sincal);
    assert!(
        module
            .diagnostics()
            .iter()
            .any(|d| d.code() == "READ.SINCAL.RETAINED_SOURCE_ONLY")
    );
    assert!(
        module.sources().len() > 1,
        "archive companions have provenance"
    );
    for format in ["sincal", "sincal-balanced", "SINCAL_BALANCED"] {
        let info = powerio::resolve_format(format).unwrap();
        assert!(!info.can_emit, "source echo is not a fresh writer");
        let result =
            powerio::emit(&module, format, Destination::memory("copy.sinx").unwrap()).unwrap();
        assert_eq!(result.fidelity(), Fidelity::ExactSameFormat);
        assert!(result.diagnostics().is_empty());
        assert_eq!(bytes(result), ARCHIVE);
    }
}

#[test]
fn an_undeclared_family_is_refused_before_electrical_mapping() {
    for format in [None, Some("sincal")] {
        let options = match format {
            None => ParseOptions::default(),
            Some(f) => ParseOptions::default().format(f).unwrap(),
        };
        let error = powerio::parse_with_options(source(), &options).unwrap_err();
        assert!(
            error
                .diagnostics()
                .iter()
                .any(|d| d.code() == "REQUEST.SINCAL.PROFILE_REQUIRED")
        );
        assert_eq!(
            error
                .retained_source()
                .unwrap()
                .primary_buffer()
                .unwrap()
                .bytes(),
            ARCHIVE
        );
    }
    assert_eq!(
        powerio_tx::format::routing::classify_format_name("sincal"),
        powerio::Detection::Ambiguous
    );
    // A declaration for another format takes precedence over a .sinx suffix.
    let error = powerio::parse_with_options(
        source(),
        &ParseOptions::default().format("matpower").unwrap(),
    )
    .unwrap_err();
    assert!(
        !error
            .diagnostics()
            .iter()
            .any(|d| d.code() == "REQUEST.SINCAL.PROFILE_REQUIRED")
    );
}

#[test]
fn changed_or_ir_restored_modules_cannot_echo_stale_native_bytes() {
    let original = parsed();
    let text = helpers::serialize_module_text(&original).unwrap();
    let restored = helpers::deserialize_module_text(&text).unwrap();
    // deserialize retains its IR document for provenance, never the native
    // project that the original module retained.
    assert_ne!(
        restored.source().unwrap().primary_buffer().unwrap().bytes(),
        ARCHIVE
    );
    assert_eq!(restored.value().type_name(), original.value().type_name());
    let mut changed = original.clone();
    let PioValue::BalancedNetwork(net) = changed.value_mut() else {
        unreachable!()
    };
    net.loads_mut()[0].p += 0.001;
    for module in [&changed, &restored] {
        let output = tempfile::tempdir().unwrap();
        let path = output.path().join("refused.sinx");
        let error = powerio::emit(module, "sincal", &path).unwrap_err();
        assert!(
            error
                .diagnostics()
                .iter()
                .any(|d| d.code() == "EMIT.SINCAL.FRESH_UNSUPPORTED")
        );
        assert!(!path.exists());
        let converted =
            powerio::emit(module, "matpower", Destination::memory("case.m").unwrap()).unwrap();
        assert_eq!(
            converted
                .diagnostics()
                .iter()
                .filter(|d| d.code() == "EMIT.SINCAL.RETAINED_SOURCE_OMITTED")
                .count(),
            1
        );
        let reloaded =
            powerio::parse(Source::from_memory("case.m", bytes(converted)).unwrap()).unwrap();
        assert!(matches!(reloaded.value(), PioValue::BalancedNetwork(_)));
    }
    assert_eq!(
        bytes(
            powerio::emit(
                &original,
                "sincal",
                Destination::memory("original.sinx").unwrap()
            )
            .unwrap()
        ),
        ARCHIVE
    );
}

#[test]
fn malformed_native_bytes_report_registered_sincal_error_with_source() {
    let input = Source::from_memory("broken.sinx", b"not a ZIP or SQLite file".to_vec()).unwrap();
    let error = powerio::parse_with_options(
        input,
        &ParseOptions::default().format("sincal-balanced").unwrap(),
    )
    .unwrap_err();
    assert!(
        error
            .diagnostics()
            .iter()
            .any(|d| d.code() == "PARSE.SINCAL.MALFORMED")
    );
    assert!(error.retained_source().is_some());
}

#[test]
fn ir_input_cannot_be_relabelled_as_native_echo() {
    let text = helpers::serialize_module_text(&parsed()).unwrap();
    let source = Source::from_memory("case.pio.json", text.into_bytes())
        .unwrap()
        .with_format(powerio::FormatId::new("sincal-balanced").unwrap());
    let restored = powerio::deserialize(source).unwrap();
    assert!(
        powerio::emit(
            &restored,
            "sincal",
            Destination::memory("bad.sinx").unwrap()
        )
        .is_err()
    );
}

#[test]
fn direct_sqlite_input_has_the_same_value_and_echoes_its_database_bytes() {
    use std::io::Read;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(ARCHIVE)).unwrap();
    let names = archive
        .file_names()
        .filter(|name| name.ends_with("/database.db"))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    assert_eq!(names.len(), 1);
    let mut database = Vec::new();
    archive
        .by_name(&names[0])
        .unwrap()
        .read_to_end(&mut database)
        .unwrap();
    let source = Source::from_memory("database.db", database.clone()).unwrap();
    let module = powerio::parse_with_options(
        source,
        &ParseOptions::default().format("sincal-balanced").unwrap(),
    )
    .unwrap();
    let PioValue::BalancedNetwork(network) = module.value() else {
        panic!("balanced")
    };
    assert_eq!(network.buses().len(), 15);
    let echo = powerio::emit(&module, "sincal", Destination::memory("copy.db").unwrap()).unwrap();
    assert_eq!(bytes(echo), database);
}

#[test]
fn balanced_selections_are_explicit_and_do_not_override_other_families() {
    let mut options = ParseOptions::default();
    options.sincal_balanced = Some(powerio_tx::format::SincalBalancedReadOptions::default());
    for format in [
        None,
        Some("sincal"),
        Some("sincal-multiconductor"),
        Some("matpower"),
    ] {
        options.format = format.map(|s| powerio::FormatId::new(s).unwrap());
        let error = powerio::parse_with_options(source(), &options).unwrap_err();
        assert!(
            error
                .diagnostics()
                .iter()
                .any(|d| d.code() == "REQUEST.SINCAL.PROFILE_REQUIRED")
        );
    }
    options = options.format("sincal-balanced").unwrap();
    assert!(matches!(
        powerio::parse_with_options(source(), &options)
            .unwrap()
            .value(),
        PioValue::BalancedNetwork(_)
    ));
    options.sincal_balanced.as_mut().unwrap().variant = Some(999_999);
    assert!(powerio::parse_with_options(source(), &options).is_err());
    options.sincal_balanced.as_mut().unwrap().variant = None;
    options.sincal_balanced.as_mut().unwrap().acquired_tables = Some("missing.json".into());
    assert!(powerio::parse_with_options(source(), &options).is_err());
    options.sincal_balanced.as_mut().unwrap().acquired_tables = None;
    let native = Source::from_memory(
        "original.mdb",
        b"\0\x01\0\0Standard Jet DB\0synthetic".to_vec(),
    )
    .unwrap();
    assert!(
        powerio::parse_with_options(native, &options)
            .unwrap_err()
            .to_string()
            .contains("acquired_tables")
    );
}

#[test]
fn two_sincal_selection_families_cannot_silently_shadow_each_other() {
    for format in ["sincal-balanced", "sincal-multiconductor"] {
        let mut options = ParseOptions::default().format(format).unwrap();
        options.sincal_balanced = Some(powerio_tx::format::SincalBalancedReadOptions::default());
        options.sincal_multiconductor = Some(powerio::dist::SincalReadOptions::default());
        let error = powerio::parse_with_options(source(), &options).unwrap_err();
        assert!(
            error
                .diagnostics()
                .iter()
                .any(|d| d.code() == "REQUEST.PARSE.SINCAL_OPTIONS_PROFILE")
        );
        assert!(error.retained_source().is_some());
    }
}

#[test]
fn experimental_balanced_backend_writes_edited_ir_without_native_source() {
    let original = parsed();
    let text = helpers::serialize_module_text(&original).unwrap();
    let mut restored = helpers::deserialize_module_text(&text).unwrap();
    let PioValue::BalancedNetwork(network) = restored.value_mut() else {
        panic!("balanced IR value")
    };
    network.loads_mut()[0].p += 0.017;
    let expected_power = network.loads()[0].p;
    // The candidate backend takes only the typed value. The module's native
    // source bytes are absent after IR transport and cannot enter this API.
    let candidate = powerio_tx::format::__write_sincal_balanced_experimental(network).unwrap();
    assert!(
        candidate
            .diagnostics
            .iter()
            .any(|d| d.code() == "EMIT.SINCAL.EXPERIMENTAL")
    );
    assert_ne!(candidate.database, ARCHIVE);
    let fresh = powerio::parse_with_options(
        Source::from_memory("fresh.db", candidate.database.clone()).unwrap(),
        &ParseOptions::default().format("sincal-balanced").unwrap(),
    )
    .unwrap();
    let PioValue::BalancedNetwork(network) = fresh.value() else {
        panic!("fresh output retained balanced family")
    };
    assert!((network.loads()[0].p - expected_power).abs() < 1e-15);
    assert_eq!(network.branches().len(), 14);
    let echoed = powerio::emit(&fresh, "sincal", Destination::memory("copy.db").unwrap()).unwrap();
    assert_eq!(echoed.fidelity(), Fidelity::ExactSameFormat);
    assert_eq!(bytes(echoed), candidate.database);
    // Ordinary emission stays source-echo-only. Fresh output requires the
    // explicit facade experimental options, tested in sincal_emit.rs.
    assert!(!powerio::resolve_format("sincal-balanced").unwrap().can_emit);
    assert!(
        powerio::emit(
            &restored,
            "sincal",
            Destination::memory("no-implicit-write.db").unwrap()
        )
        .is_err()
    );
}

#[test]
fn experimental_multiconductor_backend_writes_edited_ir_with_floating_reference() {
    use powerio_dist::{
        Configuration, DistBus, DistLoad, DistLoadVoltageModel, ExperimentalMulticonductorOptions,
        MulticonductorNetwork, VoltageSource,
    };
    let phases = vec!["1".into(), "2".into(), "3".into()];
    let mut net = MulticonductorNetwork::new();
    let mut bus = DistBus::new(
        "b",
        vec![
            "1".into(),
            "2".into(),
            "3".into(),
            "star".into(),
            "earth".into(),
        ],
    );
    bus.grounded.push("earth".into());
    net.buses_mut().push(bus);
    net.sources_mut().push(
        VoltageSource::new(
            "s",
            "b",
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
        "l",
        "b",
        vec!["1".into(), "2".into(), "3".into(), "earth".into()],
        Configuration::Wye,
        vec![1000.0, 2000.0, 3000.0],
        vec![100.0, 200.0, 300.0],
    );
    load.voltage_model = DistLoadVoltageModel::ConstantImpedance {
        v_nom: vec![230.0; 3],
    };
    net.loads_mut().push(load);
    let module = powerio::PioModule::new(PioValue::from(net));
    let mut restored =
        helpers::deserialize_module_text(&helpers::serialize_module_text(&module).unwrap())
            .unwrap();
    let PioValue::MulticonductorNetwork(net) = restored.value_mut() else {
        panic!("IR changed electrical family")
    };
    net.loads_mut()[0].p_nom[1] = 2500.0;
    let options = ExperimentalMulticonductorOptions {
        nominal_ll_volts: [("b".into(), 400.0)].into(),
    };
    let output = powerio_dist::__write_sincal_multiconductor_experimental(net, &options).unwrap();
    let snapshot = powerio_sincal::DatabaseSnapshot::decode(&output.database, None).unwrap();
    let readback = powerio_dist::__read_sincal_multiconductor_snapshot(snapshot).unwrap();
    assert_eq!(readback.loads().len(), 3);
    assert!((readback.loads()[1].p_nom[0] - 2500.0).abs() < 1e-10);
    let source = &readback.sources()[0];
    let reference = source
        .reference_terminal
        .as_ref()
        .expect("floating star retained");
    assert!(
        !readback
            .bus(&source.bus)
            .unwrap()
            .grounded
            .contains(reference)
    );
    assert!(
        output
            .diagnostics
            .iter()
            .any(|d| d.code() == "EMIT.SINCAL.MULTICONDUCTOR_EXPERIMENTAL")
    );
}

#[test]
fn experimental_transformer_primitives_rewrite_after_real_ir_and_edit() {
    use powerio_dist::{
        DistBus, DistTransformer, DistWinding, DistWindingConn, ExperimentalMulticonductorOptions,
        MulticonductorNetwork,
    };
    let mut net = MulticonductorNetwork::new();
    let phases = ["1", "2", "3"].map(str::to_owned).to_vec();
    net.buses_mut().push(DistBus::new("hv", phases.clone()));
    let mut terminals = phases.clone();
    terminals.push("earth".into());
    let mut lv = DistBus::new("lv", terminals.clone());
    lv.grounded.push("earth".into());
    net.buses_mut().push(lv);
    let mut windings = vec![
        DistWinding::new("hv", phases, DistWindingConn::Delta, 11000.0, 100_000.0),
        DistWinding::new("lv", terminals, DistWindingConn::Wye, 400.0, 100_000.0),
    ];
    windings[0].r_pct = 0.5;
    windings[1].r_pct = 0.5;
    net.transformers_mut()
        .push(DistTransformer::new("tx", windings, vec![4.0], 3));
    let options = ExperimentalMulticonductorOptions {
        nominal_ll_volts: [("hv".into(), 11000.0), ("lv".into(), 400.0)].into(),
    };
    let first = powerio_dist::__write_sincal_multiconductor_experimental(&net, &options).unwrap();
    let read = |bytes: &[u8]| {
        powerio_dist::__read_sincal_multiconductor_snapshot(
            powerio_sincal::DatabaseSnapshot::decode(bytes, None).unwrap(),
        )
        .unwrap()
    };
    let recovered = read(&first.database);
    let levels = ExperimentalMulticonductorOptions {
        nominal_ll_volts: recovered
            .buses()
            .iter()
            .map(|b| {
                let volts = if b.id == first.bus_ids["hv"].to_string() {
                    11000.0
                } else if b.id == first.bus_ids["lv"].to_string() {
                    400.0
                } else {
                    1.0
                };
                (b.id.clone(), volts)
            })
            .collect(),
    };
    let module = powerio::PioModule::new(PioValue::from(recovered));
    let mut restored =
        helpers::deserialize_module_text(&helpers::serialize_module_text(&module).unwrap())
            .unwrap();
    let PioValue::MulticonductorNetwork(net) = restored.value_mut() else {
        panic!("IR changed family");
    };
    let shunt = &mut net.shunts_mut()[0];
    shunt.extras.clear();
    for v in shunt.g.iter_mut().chain(&mut shunt.b).flatten() {
        *v *= 1.2;
    }
    let changed = powerio_dist::__write_sincal_multiconductor_experimental(net, &levels).unwrap();
    let second = read(&changed.database);
    assert_eq!(second.shunts().len(), 1);
    for (a, b) in [&net.shunts()[0].g, &net.shunts()[0].b]
        .into_iter()
        .zip([&second.shunts()[0].g, &second.shunts()[0].b])
    {
        for (a, b) in a.iter().flatten().zip(b.iter().flatten()) {
            assert!((a - b).abs() < 1e-10);
        }
    }
    assert!(
        changed
            .diagnostics
            .iter()
            .any(|d| d.message().contains("canonical 1 MVA"))
    );
}
