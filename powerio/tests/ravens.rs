//! MG-RAVENS through the facade: one `ravens-json` token covers both
//! profiles. A balanced document parses to a `BalancedNetwork` and a
//! multiconductor one to a `MulticonductorNetwork`, by content, with or
//! without the token; each profile echoes its retained source; and the
//! format catalog names the token once.

use powerio::{ParseOptions, PioValue, emit, parse, parse_with_options, resolve_format};
use powerio_core::{Destination, EmittedOutput, Source};

const MULTICONDUCTOR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../tests/data/dist/ravens/case3_balanced.json"
);

/// The case text of a memory emission: the one artifact for a JSON target,
/// and `case.dss` beside its coordinate sidecar for the OpenDSS target.
fn emitted_text(module: &powerio::PioModule<PioValue>, format: &str) -> String {
    let result = emit(module, format, Destination::memory("case").unwrap()).unwrap();
    let EmittedOutput::Memory { artifacts } = result.into_output() else {
        panic!("a memory destination returned path output");
    };
    let case = artifacts
        .into_iter()
        .find(|artifact| {
            !artifact
                .name()
                .as_str()
                .to_ascii_lowercase()
                .ends_with(".csv")
        })
        .expect("an emission has a case artifact");
    String::from_utf8(case.into_bytes()).unwrap()
}

fn declared(name: &str, text: &str, format: &str) -> Source {
    Source::from_memory(name, text.as_bytes().to_vec())
        .unwrap()
        .with_format(powerio_core::FormatId::new(format).unwrap())
}

#[test]
fn balanced_document_routes_to_the_balanced_network_by_content() {
    let case = parse(
        Source::open(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../tests/data/case9.m"
        ))
        .unwrap(),
    )
    .unwrap();
    let text = emitted_text(&case, "ravens-json");

    // No format hint: the `.json` classifier lands on MG-RAVENS, and the
    // document carries no multiconductor class, so the balanced hub reads it.
    let module =
        parse(Source::from_memory("case9.json", text.as_bytes().to_vec()).unwrap()).unwrap();
    let PioValue::BalancedNetwork(net) = module.value() else {
        panic!(
            "expected a balanced network, got {}",
            module.value().type_name()
        );
    };
    assert_eq!(net.buses().len(), 9);
    assert_eq!(net.generators().len(), 3);
    assert_eq!(net.source_format().name(), "ravens-json");

    // The explicit token routes the same way, and the same-format write
    // echoes the retained bytes.
    let module = parse(declared("case9.json", &text, "ravens-json")).unwrap();
    assert!(matches!(module.value(), PioValue::BalancedNetwork(_)));
    assert_eq!(emitted_text(&module, "ravens-json"), text);
}

#[test]
fn multiconductor_document_routes_to_the_multiconductor_network_by_content() {
    let text = std::fs::read_to_string(MULTICONDUCTOR).unwrap();
    for source in [
        Source::open(MULTICONDUCTOR).unwrap(),
        declared("case3_balanced.json", &text, "ravens-json"),
        declared("case3_balanced.json", &text, "mg-ravens"),
    ] {
        let module = parse(source).unwrap();
        let PioValue::MulticonductorNetwork(net) = module.value() else {
            panic!(
                "expected a multiconductor network, got {}",
                module.value().type_name()
            );
        };
        // The finite-impedance source has its own internal EMF node.
        assert_eq!(net.buses().len(), 4);
        assert_eq!(net.loads().len(), 3);
        assert_eq!(
            net.source_format()
                .map(powerio::dist::DistSourceFormat::name),
            Some("ravens-json")
        );
        assert_eq!(emitted_text(&module, "ravens-json"), text);
    }
    // `ParseOptions::format` is the same selection.
    let options = ParseOptions::default().format("ravens-json").unwrap();
    let module = parse_with_options(Source::open(MULTICONDUCTOR).unwrap(), &options).unwrap();
    assert!(matches!(module.value(), PioValue::MulticonductorNetwork(_)));
}

#[test]
fn a_multiconductor_feeder_converts_to_dss_and_a_balanced_case_to_matpower() {
    let feeder = parse(Source::open(MULTICONDUCTOR).unwrap()).unwrap();
    let dss = emitted_text(&feeder, "dss").to_lowercase();
    assert!(dss.contains("line.ohline"), "{dss}");

    let case = parse(
        Source::open(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../tests/data/case14.m"
        ))
        .unwrap(),
    )
    .unwrap();
    let ravens = emitted_text(&case, "ravens-json");
    let module = parse(Source::from_memory("case14.json", ravens.into_bytes()).unwrap()).unwrap();
    let matpower = emitted_text(&module, "matpower");
    assert!(matpower.contains("mpc.bus = ["), "{matpower}");
}

#[test]
fn the_catalog_names_the_token_once_under_every_alias() {
    for alias in [
        "ravens-json",
        "ravens",
        "mg-ravens",
        "mgravens",
        "RavensJson",
        "MG_RAVENS_JSON",
    ] {
        let info = resolve_format(alias).unwrap_or_else(|| panic!("{alias} resolves"));
        assert_eq!(info.token, "ravens-json", "{alias}");
        assert_eq!(info.extension, Some("json"));
        assert!(!info.is_directory);
        assert!(info.can_emit);
    }
    let entries: Vec<_> = powerio::grid_formats()
        .filter(|entry| entry.format.token == "ravens-json")
        .collect();
    assert_eq!(entries.len(), 1, "one catalog row for the two profiles");
    assert!(entries[0].can_read);
}

#[test]
fn profile_routing_ignores_object_names_and_descriptions() {
    let balanced = r#"{
        "ConnectivityNode": {"TransformerTank": {
            "Ravens.cimObjectType": "ConnectivityNode",
            "IdentifiedObject.name": "ACLineSegmentPhase",
            "IdentifiedObject.description": "EnergyConsumerPhase"
        }},
        "PowerSystemResource": {},
        "AssetInfo": {}
    }"#;
    let module = parse(declared("case.json", balanced, "ravens-json")).unwrap();
    assert!(matches!(module.value(), PioValue::BalancedNetwork(_)));
    assert_eq!(emitted_text(&module, "ravens-json"), balanced);

    let mut doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(MULTICONDUCTOR).unwrap()).unwrap();
    doc["ConnectivityNode"]["sourcebus"]["IdentifiedObject.description"] =
        serde_json::json!("PowerTransformerEnd");
    let text = doc.to_string();
    let module = parse(declared("case.json", &text, "ravens-json")).unwrap();
    assert!(matches!(module.value(), PioValue::MulticonductorNetwork(_)));
    assert_eq!(emitted_text(&module, "ravens-json"), text);
}

#[test]
fn profile_routing_decodes_equivalent_json_escapes() {
    let original = std::fs::read_to_string(MULTICONDUCTOR).unwrap();
    let mut text = original.clone();
    for marker in powerio_core::cim::ravens::MULTICONDUCTOR_MARKERS {
        let escaped = format!(
            r#""\u{:04x}{}""#,
            u32::from(marker.as_bytes()[0]),
            &marker[1..]
        );
        text = text.replace(&format!("\"{marker}\""), &escaped);
    }
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&text).unwrap(),
        serde_json::from_str::<serde_json::Value>(&original).unwrap()
    );
    for source in [
        Source::from_memory("case.json", text.as_bytes().to_vec()).unwrap(),
        declared("case.json", &text, "ravens-json"),
    ] {
        let module = parse(source).unwrap();
        assert!(matches!(module.value(), PioValue::MulticonductorNetwork(_)));
    }
}

#[test]
fn genuine_mixed_profiles_and_excessive_nesting_are_refused() {
    let mixed = r#"{
        "ConnectivityNode":{},
        "PerLengthLineParameter":{},
        "MySettings":{"Ravens.cimObjectType":"Algorithm\u0053ettings"}
    }"#;
    let error = parse(declared("case.json", mixed, "ravens-json")).unwrap_err();
    assert_eq!(error.info().unwrap().code, "PARSE.RAVENS.MIXED_PROFILE");

    let nested = format!("{}null{}", "[".repeat(2048), "]".repeat(2048));
    let text = format!(r#"{{"ConnectivityNode":{{}},"nested":{nested}}}"#);
    assert!(parse(declared("case.json", &text, "ravens-json")).is_err());
}
