use powerio::{Destination, DistLocationTarget, EmittedOutput, PioValue, Source};

const CASE: &str = r"
module powerflow;
object node { name source; phases AN; bustype SWING; nominal_voltage 240;
    latitude 0; longitude 153.02; }
object line_configuration { name cfg; z11 1+.5j; latitude 27S28:12; }
object overhead_line { name line; from source; to sink; phases AN; length 100 m;
    configuration cfg; latitude 10; longitude 20; }
object load { name sink; phases AN; nominal_voltage 240; constant_impedance_A 20+5j;
    latitude -27; longitude 154; }
object load { name demand; parent sink; phases AN; nominal_voltage 240;
    constant_power_A 20+5j; constant_current_A 1-.2j;
    latitude 28S0; longitude (source.longitude); }
";

fn network(module: &powerio::PioModule<PioValue>) -> &powerio::MulticonductorNetwork {
    let PioValue::MulticonductorNetwork(net) = module.value() else {
        panic!("expected network");
    };
    net
}

fn text(result: powerio::EmitResult) -> Vec<u8> {
    let EmittedOutput::Memory { mut artifacts } = result.into_output() else {
        panic!("expected memory");
    };
    assert_eq!(artifacts.len(), 1);
    artifacts.pop().unwrap().into_bytes()
}

#[test]
fn component_coordinates_survive_ir_canonical_and_edited_output() {
    let module = powerio::parse(Source::from_memory("main.glm", CASE.as_bytes()).unwrap()).unwrap();
    let net = network(&module);
    assert_eq!(net.component_locations().len(), 6);
    assert!(net.geo().is_none());
    assert!(net.buses().iter().all(|b| b.location.is_none()));
    assert!((net.lines()[0].length - 100.0).abs() < 1e-12);
    for p in net.component_locations() {
        assert!(p.point.datum.is_none());
    }
    let config = net
        .component_locations()
        .iter()
        .find(|p| p.target == DistLocationTarget::LineCode)
        .unwrap();
    assert_eq!(config.point.longitude, None);
    assert!((config.point.latitude.unwrap() + 27.47).abs() < 1e-12);
    let loads: Vec<_> = net
        .component_locations()
        .iter()
        .filter(|p| p.target == DistLocationTarget::Load)
        .collect();
    assert_eq!(loads.len(), 2);
    assert!(
        loads
            .iter()
            .all(|p| p.point.latitude == Some(-28.0) && p.point.longitude == Some(153.02))
    );

    let ir = text(powerio::serialize(&module, Destination::memory("case.json").unwrap()).unwrap());
    let mut restored = powerio::deserialize(Source::from_memory("case.json", ir).unwrap()).unwrap();
    assert_eq!(
        network(&restored).component_locations(),
        net.component_locations()
    );
    assert_eq!(
        serde_json::to_value(network(&restored)).unwrap(),
        serde_json::to_value(net).unwrap()
    );
    let PioValue::MulticonductorNetwork(edited) = restored.value_mut() else {
        unreachable!()
    };
    let p = edited
        .component_locations_mut()
        .iter_mut()
        .find(|p| p.target == DistLocationTarget::Bus && p.name == "source")
        .unwrap();
    p.point.latitude = Some(-29.0);
    p.point.longitude = None;
    let output =
        text(powerio::emit(&restored, "glm", Destination::memory("edited").unwrap()).unwrap());
    let reparsed = powerio::parse(Source::from_memory("case.glm", output).unwrap()).unwrap();
    let actual = network(&reparsed);
    let source = actual
        .component_locations()
        .iter()
        .find(|p| p.target == DistLocationTarget::Bus && p.name == "source")
        .unwrap();
    assert_eq!(source.point.latitude, Some(-29.0));
    assert_eq!(source.point.longitude, None);
    let line = actual
        .component_locations()
        .iter()
        .find(|p| p.target == DistLocationTarget::Line)
        .unwrap();
    assert_eq!(line.point.latitude, Some(10.0));
    assert_eq!(line.point.longitude, Some(20.0));
    assert!((actual.lines()[0].length - 100.0).abs() < 1e-12);
}

#[test]
fn coordinate_projection_losses_and_invalid_edits_are_explicit() {
    let mut module =
        powerio::parse(Source::from_memory("case.glm", CASE.as_bytes()).unwrap()).unwrap();
    for format in ["dss", "pmd", "bmopf"] {
        let result = powerio::emit(&module, format, Destination::memory("out").unwrap()).unwrap();
        assert!(
            result
                .diagnostics()
                .iter()
                .any(|d| d.code() == "EMIT.DIST.COMPONENT_COORDINATES_DROPPED")
        );
    }
    let (_, diagnostics) = powerio::dist_geo::to_dist_geo_layer_report(network(&module));
    assert_eq!(
        diagnostics[0].code(),
        "EMIT.DIST.COMPONENT_COORDINATES_DROPPED"
    );
    let PioValue::MulticonductorNetwork(net) = module.value_mut() else {
        unreachable!()
    };
    net.component_locations_mut()[0].point.latitude = Some(100.0);
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("invalid");
    assert!(powerio::emit(&module, "glm", Destination::path(&output)).is_err());
    assert!(!output.exists());
}

#[test]
fn coordinate_parse_errors_remain_attached_to_original_sources() {
    for text in [
        CASE.replace("latitude 0", "latitude 100"),
        CASE.replace("latitude 0", "latitude 3junk"),
    ] {
        let error =
            powerio::parse(Source::from_memory("bad.glm", text.into_bytes()).unwrap()).unwrap_err();
        assert!(error.retained_source().is_some());
    }
}

#[test]
fn located_networks_roundtrip_inside_collections_and_calculations() {
    let module = powerio::parse(Source::from_memory("case.glm", CASE.as_bytes()).unwrap()).unwrap();
    let net = network(&module);
    let series = powerio_core::TimeSeries::new(
        vec![powerio_core::TimePoint::new("t", None).unwrap()],
        vec![net.clone()],
    )
    .unwrap();
    let pf = powerio_prob::McAcPfInstance::from_network(net.clone()).unwrap();
    let opf = powerio_prob::McAcOpfInstance::from_network(net.clone()).unwrap();
    for value in [
        PioValue::from(series),
        PioValue::from(pf),
        PioValue::from(opf),
    ] {
        let original = powerio::PioModule::new(value);
        let bytes =
            text(powerio::serialize(&original, Destination::memory("ir.json").unwrap()).unwrap());
        let restored =
            powerio::deserialize(Source::from_memory("ir.json", bytes.clone()).unwrap()).unwrap();
        let again = text(
            powerio::serialize(&restored, Destination::memory("again.json").unwrap()).unwrap(),
        );
        let a: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let b: serde_json::Value = serde_json::from_slice(&again).unwrap();
        assert_eq!(a["value"], b["value"]);
        assert!(
            String::from_utf8(bytes)
                .unwrap()
                .contains("powerio.LocatedMulticonductorNetwork")
        );
        assert_eq!(original.value().type_name(), restored.value().type_name());
    }
}
