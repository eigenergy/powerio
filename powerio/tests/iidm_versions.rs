//! Every PowSybl IIDM serialization version reads to the same network, JIIDM
//! reads as XIIDM does, and fresh JIIDM reads back to the module it came
//! from.

mod helpers;

use std::path::PathBuf;

use helpers::{
    deserialize_module_text, load_balanced_case, load_balanced_memory_named, serialize_module_text,
};
use powerio::network::TapChangerRegulationMode;
use powerio::{BalancedNetwork, Destination, EmittedOutput, SourceFormat};

const VERSIONS: [&str; 18] = [
    "1_0", "1_1", "1_2", "1_3", "1_4", "1_5", "1_6", "1_7", "1_8", "1_9", "1_10", "1_11", "1_12",
    "1_13", "1_14", "1_15", "1_16", "1_17",
];
const JIIDM_VERSIONS: [&str; 7] = ["1_11", "1_12", "1_13", "1_14", "1_15", "1_16", "1_17"];

fn fixture(version: &str, name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data/xiidm/powsybl")
        .join(format!("V{version}"))
        .join(name)
}

fn network_document(network: &BalancedNetwork) -> serde_json::Value {
    let mut document: serde_json::Value =
        serde_json::from_str(&network.to_json().unwrap()).unwrap();
    // The encoding a network came from is the one field an XML and a JSON
    // document of the same network state differently.
    document
        .as_object_mut()
        .unwrap()
        .remove("source_format")
        .expect("model JSON names the source format");
    document
}

/// Emit `to` from a freshly parsed source. Serializing and deserializing
/// through PowerIO IR removes the input file content kept for same format
/// emission, so the writer rebuilds the document from the typed value.
fn emit_fresh(source: powerio::Source, from: &str, to: &str) -> String {
    let module = powerio::parse_with_options(
        source,
        &powerio::ParseOptions::default().format(from).unwrap(),
    )
    .unwrap();
    let module = deserialize_module_text(&serialize_module_text(&module).unwrap()).unwrap();
    let emitted = powerio::emit(&module, to, Destination::memory("fresh").unwrap()).unwrap();
    let EmittedOutput::Memory { mut artifacts } = emitted.into_output() else {
        panic!("memory destination returned a path output");
    };
    String::from_utf8(artifacts.pop().unwrap().into_bytes()).unwrap()
}

fn emit_memory(network_path: &std::path::Path, from: &str, to: &str) -> String {
    emit_fresh(powerio::Source::open(network_path).unwrap(), from, to)
}

fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() <= 1e-9 * expected.abs().max(1.0),
        "{actual} differs from {expected}"
    );
}

/// Sort the terminal table by equipment so the comparison does not depend
/// on the order fresh output lists equipment within a voltage level.
fn normalize_document(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, item) in map.iter_mut() {
                if key == "terminals"
                    && let serde_json::Value::Array(terminals) = item
                {
                    terminals.sort_by_cached_key(|terminal| {
                        format!("{}/{}", terminal["equipment"], terminal["terminal"])
                    });
                }
                normalize_document(item);
            }
        }
        serde_json::Value::Array(values) => values.iter_mut().for_each(normalize_document),
        _ => {}
    }
}

/// Compare two network documents, allowing the rounding a unit conversion
/// to ohms and back introduces in fresh output.
fn assert_documents_close(actual: &serde_json::Value, expected: &serde_json::Value, path: &str) {
    match (actual, expected) {
        (serde_json::Value::Number(a), serde_json::Value::Number(b)) => {
            let (a, b) = (a.as_f64().unwrap(), b.as_f64().unwrap());
            assert!(
                (a - b).abs() <= 1e-12 * a.abs().max(b.abs()).max(1.0),
                "{path}: {a} differs from {b}"
            );
        }
        (serde_json::Value::Array(a), serde_json::Value::Array(b)) => {
            assert_eq!(a.len(), b.len(), "{path}: array length");
            for (index, (x, y)) in a.iter().zip(b).enumerate() {
                assert_documents_close(x, y, &format!("{path}[{index}]"));
            }
        }
        (serde_json::Value::Object(a), serde_json::Value::Object(b)) => {
            assert_eq!(
                a.keys().collect::<Vec<_>>(),
                b.keys().collect::<Vec<_>>(),
                "{path}: keys"
            );
            for (key, x) in a {
                assert_documents_close(x, &b[key], &format!("{path}/{key}"));
            }
        }
        _ => assert_eq!(actual, expected, "{path}"),
    }
}

/// The solved eurostag network as every version states it: the same element
/// counts and the same electrical values.
#[test]
fn every_iidm_version_reads_the_same_network() {
    let reference = load_balanced_case(fixture("1_17", "eurostag-tutorial1-lf.xml"), None).unwrap();
    let reference_document = network_document(&reference.network);
    for version in VERSIONS {
        let parsed = load_balanced_case(fixture(version, "eurostag-tutorial1-lf.xml"), None)
            .unwrap_or_else(|error| panic!("IIDM {version}: {error}"));
        let network = &parsed.network;
        assert_eq!(network.source_format(), SourceFormat::Xiidm, "{version}");
        assert_eq!(network.buses().len(), 4, "{version}");
        assert_eq!(network.branches().len(), 4, "{version}");
        assert_eq!(network.generators().len(), 1, "{version}");
        assert_eq!(network.loads().len(), 1, "{version}");
        let generator_bus = network.generators()[0].bus;
        let bus = network
            .buses()
            .iter()
            .find(|bus| bus.id == generator_bus)
            .unwrap();
        assert_close(bus.vm, 24.500_000_610_351_563 / 24.0);
        assert_close(bus.va, 2.325_976_371_765_136_7);
        assert_close(network.generators()[0].pg, 607.0);
        assert_close(network.loads()[0].p, 600.0);
        let detailed = network.detailed_connectivity().as_ref().unwrap();
        let tap = detailed
            .tap_changers
            .iter()
            .find(|tap| tap.transformer.local_id() == "NHV2_NLOAD")
            .unwrap_or_else(|| panic!("IIDM {version} lost the ratio tap changer"));
        assert_eq!(tap.tap_position, Some(1), "{version}");
        assert_eq!(
            tap.regulation_mode,
            Some(TapChangerRegulationMode::Voltage),
            "{version}"
        );
        assert_eq!(tap.regulation_value, Some(158.0), "{version}");
        assert!(tap.regulating, "{version}");
        assert_eq!(tap.steps.len(), 3, "{version}");
        assert_eq!(
            network_document(network),
            reference_document,
            "IIDM {version} reads to a different network than 1.17"
        );
        let compatibility = parsed
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code() == "READ.XIIDM.VERSION.COMPATIBILITY")
            .count();
        assert_eq!(compatibility, usize::from(version != "1_17"), "{version}");
    }
}

/// PowSybl's JSON layout of a network reads to the same balanced network as
/// its XML layout, in every version PowSybl ships both for.
#[test]
fn jiidm_reads_as_the_matching_xiidm_document() {
    for version in JIIDM_VERSIONS {
        let xml = load_balanced_case(fixture(version, "eurostag-tutorial1-lf.xml"), None).unwrap();
        let json = load_balanced_case(fixture(version, "eurostag-tutorial1-lf.json"), None)
            .unwrap_or_else(|error| panic!("JIIDM {version}: {error}"));
        assert_eq!(
            json.network.source_format(),
            SourceFormat::Jiidm,
            "{version}"
        );
        assert_eq!(
            network_document(&json.network),
            network_document(&xml.network),
            "JIIDM {version} differs from XIIDM {version}"
        );
        let xml_codes = xml
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code().to_owned())
            .collect::<Vec<_>>();
        let json_codes = json
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(json_codes, xml_codes, "{version}");
    }
    let xml = load_balanced_case(fixture("1_17", "fictitiousSwitchRef.xml"), None).unwrap();
    let json = load_balanced_case(fixture("1_17", "fictitiousSwitchRef.jiidm"), None).unwrap();
    assert_eq!(
        network_document(&json.network),
        network_document(&xml.network)
    );
    let oldest = load_balanced_case(fixture("1_11", "fictitiousSwitchRef.jiidm"), None).unwrap();
    assert_eq!(oldest.network.source_format(), SourceFormat::Jiidm);
    assert_eq!(oldest.network.buses().len(), xml.network.buses().len());
    assert_eq!(
        oldest.network.branches().len(),
        xml.network.branches().len()
    );
    assert!(
        oldest
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code() == "READ.XIIDM.VERSION.COMPATIBILITY")
    );
}

/// A `.json` source is recognized as JIIDM without a declared format.
#[test]
fn bare_json_is_classified_as_jiidm() {
    let text = std::fs::read_to_string(fixture("1_17", "eurostag-tutorial1-lf.json")).unwrap();
    let parsed = load_balanced_memory_named(&text, "jiidm", Some("network.json")).unwrap();
    assert_eq!(parsed.network.source_format(), SourceFormat::Jiidm);
    let source = powerio::Source::from_memory("network.json", text.into_bytes()).unwrap();
    let module = powerio::parse(source).unwrap();
    assert_eq!(
        module
            .source()
            .unwrap()
            .format()
            .map(powerio::FormatId::as_str),
        Some("jiidm")
    );
}

/// Fresh JIIDM is PowSybl's 1.17 layout and reads back to the module it was
/// written from; fresh XIIDM from either encoding is the same document.
#[test]
fn fresh_jiidm_reads_back_to_the_same_module() {
    for name in ["eurostag-tutorial1-lf.xml", "fictitiousSwitchRef.xml"] {
        let path = fixture("1_17", name);
        let original = load_balanced_case(&path, None).unwrap();
        let jiidm = emit_memory(&path, "xiidm", "jiidm");
        assert!(
            jiidm.starts_with("{\n  \"version\" : \"1.17\",\n"),
            "{name}"
        );
        let reread = load_balanced_memory_named(&jiidm, "jiidm", Some("fresh.jiidm")).unwrap();
        assert_eq!(
            reread.network.source_format(),
            SourceFormat::Jiidm,
            "{name}"
        );
        let mut reread_document = network_document(&reread.network);
        let mut original_document = network_document(&original.network);
        normalize_document(&mut reread_document);
        normalize_document(&mut original_document);
        assert_documents_close(&reread_document, &original_document, name);
        let fresh_xiidm_from_xml = emit_memory(&path, "xiidm", "xiidm");
        let fresh_xiidm_from_json = emit_fresh(
            powerio::Source::from_memory("fresh.jiidm", jiidm.into_bytes()).unwrap(),
            "jiidm",
            "xiidm",
        );
        assert_eq!(fresh_xiidm_from_json, fresh_xiidm_from_xml, "{name}");
    }
}

/// The JIIDM element and attribute order matches the PowSybl fixture for the
/// same network, which is what PowSybl's sequential JSON reader requires.
#[test]
fn fresh_jiidm_follows_the_powsybl_field_order() {
    fn key_orders(value: &serde_json::Value, path: &str, out: &mut Vec<(String, Vec<String>)>) {
        let serde_json::Value::Object(map) = value else {
            return;
        };
        let scalar_keys = map
            .iter()
            .filter(|(_, item)| match item {
                serde_json::Value::Array(values) => values.iter().all(|v| !v.is_object()),
                other => !other.is_object(),
            })
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        out.push((path.to_owned(), scalar_keys));
        for (key, item) in map {
            match item {
                serde_json::Value::Object(_) => key_orders(item, &format!("{path}/{key}"), out),
                serde_json::Value::Array(values) => {
                    for entry in values.iter().filter(|entry| entry.is_object()) {
                        key_orders(entry, &format!("{path}/{key}[]"), out);
                    }
                }
                _ => {}
            }
        }
    }

    for name in ["eurostag-tutorial1-lf", "fictitiousSwitchRef"] {
        let xml_path = fixture("1_17", &format!("{name}.xml"));
        let json_name = if name == "fictitiousSwitchRef" {
            format!("{name}.jiidm")
        } else {
            format!("{name}.json")
        };
        let reference: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(fixture("1_17", &json_name)).unwrap())
                .unwrap();
        let fresh: serde_json::Value =
            serde_json::from_str(&emit_memory(&xml_path, "xiidm", "jiidm")).unwrap();
        let mut reference_orders = Vec::new();
        key_orders(&reference, "", &mut reference_orders);
        let mut fresh_orders = Vec::new();
        key_orders(&fresh, "", &mut fresh_orders);
        assert_eq!(fresh_orders.len(), reference_orders.len(), "{name}");
        for ((fresh_path, fresh_keys), (reference_path, reference_keys)) in
            fresh_orders.iter().zip(&reference_orders)
        {
            assert_eq!(fresh_path, reference_path, "{name}");
            let common_fresh = fresh_keys
                .iter()
                .filter(|key| reference_keys.contains(key))
                .collect::<Vec<_>>();
            let common_reference = reference_keys
                .iter()
                .filter(|key| fresh_keys.contains(key))
                .collect::<Vec<_>>();
            assert_eq!(common_fresh, common_reference, "{name}: {fresh_path}");
        }
    }
}

/// The IIDM 1.0 forms of shunts, static VAR compensators, and batteries map
/// to the same tables as the 1.17 forms.
#[test]
fn iidm_1_0_legacy_injection_forms_map_to_the_current_tables() {
    let shunt = load_balanced_case(fixture("1_0", "nonLinearShuntRoundTripRef.xml"), None)
        .unwrap()
        .network;
    assert_eq!(shunt.shunts().len(), 1);
    assert_close(shunt.shunts()[0].b, 1.0e-5 * 380.0 * 380.0);
    assert_eq!(shunt.shunts()[0].section_count, Some(1));

    let svc = load_balanced_case(fixture("1_0", "staticVarCompensatorRoundTripRef.xml"), None)
        .unwrap()
        .network;
    assert_eq!(svc.static_var_compensators().len(), 1);
    assert_close(svc.static_var_compensators()[0].voltage_setpoint_kv, 390.0);
    assert!(svc.static_var_compensators()[0].regulating);

    let battery = load_balanced_case(fixture("1_0", "batteryRoundTripRef.xml"), None)
        .unwrap()
        .network;
    let storage = battery.storage();
    assert_eq!(storage.len(), 2);
    let second = storage
        .iter()
        .find(|item| item.uid.as_deref() == Some("BAT2"))
        .unwrap();
    assert_close(second.ps, 100.0);
    assert_close(second.qs, 200.0);
}

/// The IIDM 1.0 forms of three winding transformers, inline tie lines, and
/// busbar section voltages map to the same tables as the 1.17 forms.
#[test]
fn iidm_1_0_legacy_branch_forms_map_to_the_current_tables() {
    let transformer = load_balanced_case(
        fixture("1_0", "threeWindingsTransformerRoundTripRef.xml"),
        None,
    )
    .unwrap();
    let network = &transformer.network;
    assert_eq!(network.transformers_3w().len(), 1);
    let detailed = network.detailed_connectivity().as_ref().unwrap();
    let taps = detailed
        .tap_changers
        .iter()
        .filter(|tap| tap.transformer.local_id() == "3WT")
        .collect::<Vec<_>>();
    assert_eq!(taps.len(), 2);
    assert!(
        taps.iter()
            .all(|tap| tap.regulation_mode == Some(TapChangerRegulationMode::Voltage))
    );
    let limits = detailed
        .operational_limit_groups
        .iter()
        .filter(|group| group.equipment.local_id() == "3WT")
        .collect::<Vec<_>>();
    assert_eq!(limits.len(), 3);
    assert!(
        limits
            .iter()
            .all(|group| group.selected && group.id == "DEFAULT")
    );
    assert_eq!(
        limits
            .iter()
            .map(|group| group.current_limits.as_ref().unwrap().permanent_limit)
            .collect::<Vec<_>>(),
        vec![Some(1000.0), Some(100.0), Some(10.0)]
    );

    let tie = load_balanced_case(fixture("1_0", "tl-loading-limits.xml"), None).unwrap();
    let detailed = tie.network.detailed_connectivity().as_ref().unwrap();
    assert_eq!(detailed.tie_lines.len(), 2);
    assert_eq!(detailed.boundary_lines.len(), 4);
    assert!(
        detailed
            .boundary_lines
            .iter()
            .all(|line| matches!(line.pairing_key.as_deref(), Some("X1" | "X2")))
    );
    let first_half = detailed
        .boundary_lines
        .iter()
        .find(|line| line.component.local_id() == "NHV1_NHV2_1.1")
        .unwrap();
    assert_close(first_half.resistance_ohm, 1.5);
    assert_close(first_half.susceptance_siemens, 2.0 * 9.65e-5);
    let half_limits = detailed
        .operational_limit_groups
        .iter()
        .filter(|group| group.equipment.local_id().starts_with("NHV1_NHV2_1."))
        .collect::<Vec<_>>();
    assert_eq!(half_limits.len(), 2);
    assert!(half_limits.iter().all(|group| {
        group.selected
            && group.current_limits.as_ref().unwrap().permanent_limit == Some(350.0)
            && group
                .current_limits
                .as_ref()
                .unwrap()
                .temporary_limits
                .len()
                == 2
    }));
    assert!(
        tie.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message().contains("xnodeP_1"))
    );

    let node_breaker = load_balanced_case(fixture("1_0", "fictitiousSwitchRef.xml"), None)
        .unwrap()
        .network;
    let current = load_balanced_case(fixture("1_17", "fictitiousSwitchRef.xml"), None)
        .unwrap()
        .network;
    assert_eq!(node_breaker.buses().len(), current.buses().len());
    let bus = node_breaker
        .buses()
        .iter()
        .find(|bus| bus.uid.as_deref() == Some("D"))
        .unwrap();
    assert_close(bus.vm, 234.40912 / 225.0);
}
