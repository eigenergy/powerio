//! PowerFactory DGS through the facade: the balanced route, the
//! multiconductor route, the `.pfd` refusal, and cross format emission.

mod helpers;

use helpers::{deserialize_module_text, serialize_module_text};
use powerio::{Destination, EmittedOutput, Fidelity, PioValue, Source, emit, parse};

fn fixture(name: &str) -> String {
    format!(
        "{}/../tests/data/powerfactory/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn memory_bytes(result: powerio::EmitResult) -> Vec<u8> {
    let EmittedOutput::Memory { mut artifacts } = result.into_output() else {
        panic!("memory destination returned a path output");
    };
    artifacts.pop().unwrap().into_bytes()
}

#[test]
fn a_sequence_data_export_routes_to_the_balanced_network() {
    let module = parse(Source::open(fixture("ieee14.dgs")).unwrap(), None).unwrap();
    let PioValue::BalancedNetwork(net) = &module.value else {
        panic!("ieee14.dgs is a balanced export");
    };
    assert_eq!(net.buses().len(), 14);
    assert_eq!(
        module
            .source()
            .and_then(|source| source.format())
            .map(powerio::FormatId::as_str),
        Some("dgs")
    );
    assert!(
        module
            .diagnostics
            .iter()
            .all(|d| d.code().starts_with("READ.DGS.")),
        "{:?}",
        module.diagnostics
    );

    // Cross format emission to MATPOWER reads back as the same case.
    let emitted = emit(&module, "matpower", Destination::memory("case.m").unwrap()).unwrap();
    let text = String::from_utf8(memory_bytes(emitted)).unwrap();
    let back = parse(
        Source::from_memory("case.m", text.into_bytes()).unwrap(),
        None,
    )
    .unwrap();
    let PioValue::BalancedNetwork(back) = &back.value else {
        panic!("MATPOWER parses to a balanced network");
    };
    assert_eq!(back.buses().len(), 14);
    assert_eq!(back.branches().len(), 20);
    assert_eq!(back.generators().len(), 5);

    // Same format emission of the unchanged module returns the source.
    let echoed = emit(&module, "dgs", Destination::memory("copy.dgs").unwrap()).unwrap();
    assert_eq!(echoed.fidelity(), Fidelity::ExactSameFormat);
    assert_eq!(
        memory_bytes(echoed),
        std::fs::read(fixture("ieee14.dgs")).unwrap()
    );
}

#[test]
fn a_nameless_source_routes_by_content() {
    let bytes = std::fs::read(fixture("ieee14.dgs")).unwrap();
    let module = parse(Source::from_memory("<memory>", bytes).unwrap(), None).unwrap();
    assert!(matches!(module.value, PioValue::BalancedNetwork(_)));
    let declared = parse(
        Source::from_memory("export.txt", std::fs::read(fixture("Switches.dgs")).unwrap())
            .unwrap(),
        Some("powerfactory"),
    )
    .unwrap();
    assert!(matches!(declared.value, PioValue::BalancedNetwork(_)));
}

#[test]
fn a_conductor_level_export_lands_in_the_multiconductor_network() {
    let module = parse(Source::open(fixture("lv-feeder.dgs")).unwrap(), None).unwrap();
    let PioValue::MulticonductorNetwork(net) = &module.value else {
        panic!("lv-feeder.dgs carries neutral conductors and per phase demand");
    };
    assert_eq!(net.name().as_deref(), Some("LV Feeder"));
    assert_eq!(net.base_frequency(), 50.0);
    assert_eq!(net.source_format().map(|f| f.name()), Some("dgs"));
    assert_eq!(net.buses().len(), 4);
    assert_eq!(net.linecodes().len(), 2);
    assert_eq!(net.lines().len(), 2);
    assert_eq!(net.loads().len(), 2);
    assert_eq!(net.sources().len(), 1);
    assert_eq!(net.transformers().len(), 1);
    assert!(net.switches().is_empty());

    let lv = net.bus("LV Bus").unwrap();
    assert_eq!(lv.terminals, ["1", "2", "3", "4"]);
    assert_eq!(lv.grounded, ["4"]);
    let house = net.bus("House 1").unwrap();
    assert_eq!(house.terminals, ["1", "4"]);
    assert!(house.grounded.is_empty());

    // The four wire main: phase self and mutual from the sequence values,
    // the neutral from its own parameters, ohm per meter.
    let main = net.linecode("4x95 Al").unwrap();
    assert_eq!(main.n_conductors, 4);
    let r = &main.r_series;
    assert!((r[0][0] - (1.28 + 2.0 * 0.32) / 3.0 * 1e-3).abs() < 1e-12);
    assert!((r[0][1] - (1.28 - 0.32) / 3.0 * 1e-3).abs() < 1e-12);
    assert!((r[3][3] - 0.32e-3).abs() < 1e-12);
    assert!((r[0][3] - 0.16e-3).abs() < 1e-12);
    assert_eq!(main.i_max.as_deref(), Some([250.0; 4].as_slice()));
    let service = net.linecode("2x25 Al").unwrap();
    assert_eq!(service.n_conductors, 2);
    assert!((service.r_series[0][0] - 1.2e-3).abs() < 1e-12);
    assert_eq!(service.r_series[0][1], 0.0);

    let service_line = net.lines().iter().find(|l| l.name == "Service").unwrap();
    assert_eq!(service_line.bus_from, "Pole 1");
    assert_eq!(service_line.bus_to, "House 1");
    // Phase b at the pole end, the single phase at the house end, and the
    // neutral at both.
    assert_eq!(service_line.terminal_map_from, ["2", "4"]);
    assert_eq!(service_line.terminal_map_to, ["1", "4"]);
    assert_eq!(service_line.length, 40.0);

    let pole = net.loads().iter().find(|l| l.name == "Pole load").unwrap();
    assert_eq!(pole.configuration, powerio_dist::Configuration::Wye);
    assert_eq!(pole.terminal_map, ["1", "2", "3"]);
    assert_eq!(pole.p_nom, [10_000.0, 20_000.0, 15_000.0]);
    assert_eq!(pole.q_nom, [3_000.0, 5_000.0, 4_000.0]);
    let house_load = net.loads().iter().find(|l| l.name == "House load").unwrap();
    assert_eq!(
        house_load.configuration,
        powerio_dist::Configuration::SinglePhase
    );
    assert_eq!(house_load.terminal_map, ["1"]);
    assert_eq!(house_load.p_nom, [5_000.0]);

    let grid = &net.sources()[0];
    assert_eq!(grid.bus, "MV Bus");
    assert!((grid.v_magnitude[0] - 1.02 * 10_000.0 / 3f64.sqrt()).abs() < 1e-9);
    assert!((grid.v_angle[1] + 2.0 * std::f64::consts::PI / 3.0).abs() < 1e-12);

    let transformer = &net.transformers()[0];
    assert_eq!(transformer.windings[0].conn, powerio_dist::DistWindingConn::Delta);
    assert_eq!(transformer.windings[1].conn, powerio_dist::DistWindingConn::Wye);
    assert_eq!(transformer.windings[0].v_ref, 10_000.0);
    assert_eq!(transformer.windings[1].v_ref, 400.0);
    assert!((transformer.windings[0].tap - 1.025).abs() < 1e-12);
    assert_eq!(transformer.windings[1].tap, 1.0);
    assert_eq!(transformer.phases, 3);

    let routed = module
        .diagnostics
        .iter()
        .find(|d| d.code() == "READ.DGS.ROUTED_MULTICONDUCTOR")
        .expect("the route is recorded");
    assert!(routed.message().contains("phtech=1"), "{}", routed.message());

    // The module survives PowerIO IR and echoes its source.
    let stored = serialize_module_text(&module).unwrap();
    let restored = deserialize_module_text(&stored).unwrap();
    assert!(matches!(restored.value, PioValue::MulticonductorNetwork(_)));
    let echoed = emit(&module, "dgs", Destination::memory("copy.dgs").unwrap()).unwrap();
    assert_eq!(echoed.fidelity(), Fidelity::ExactSameFormat);
    assert_eq!(
        memory_bytes(echoed),
        std::fs::read(fixture("lv-feeder.dgs")).unwrap()
    );
    // A distribution writer takes the multiconductor network.
    let dss = emit(&module, "pmd-json", Destination::memory("feeder.json").unwrap()).unwrap();
    assert!(!memory_bytes(dss).is_empty());
}

#[test]
fn a_project_file_is_refused_with_a_coded_diagnostic() {
    let error = parse(
        Source::from_memory("project.pfd", vec![0x47, 0x50, 0x0c, 0x2a, 0x07]).unwrap(),
        None,
    )
    .unwrap_err();
    assert_eq!(error.diagnostics()[0].code(), "READ.DGS.ENCRYPTED_PROJECT");
    assert!(error.to_string().contains("export"), "{error}");
    let declared = parse(
        Source::from_memory("project.bin", vec![0x47, 0x50]).unwrap(),
        Some("pfd"),
    )
    .unwrap_err();
    assert_eq!(declared.diagnostics()[0].code(), "READ.DGS.ENCRYPTED_PROJECT");
}

#[test]
fn an_export_without_topology_is_undecided() {
    let text = "$$General;ID(a:40);Descr(a:40);Val(a:40)\n1;Version;5.0\n\
                $$TypLne;ID(a:40);loc_name(a:40);rline(r);xline(r)\n7;typ;0.1;0.4\n";
    let error = parse(
        Source::from_memory("library.dgs", text.as_bytes().to_vec()).unwrap(),
        None,
    )
    .unwrap_err();
    assert_eq!(error.diagnostics()[0].code(), "READ.DGS.ROUTE_UNDECIDED");
    assert!(error.to_string().contains("ElmTerm"), "{error}");
}

#[test]
fn the_component_parser_refuses_a_conductor_level_export_with_guidance() {
    let error = powerio_tx::parse(Source::open(fixture("lv-feeder.dgs")).unwrap()).unwrap_err();
    assert!(error.to_string().contains("powerio::parse"), "{error}");
}
