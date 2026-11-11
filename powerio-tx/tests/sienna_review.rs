//! Retained edits preserve native identity without reviving obsolete metadata.
mod helpers;
use helpers::{emit_module, emit_value};
use powerio_core::{FormatId, PioModule, Source};
use powerio_tx::TargetFormat;
use powerio_tx::network::{BalancedNetwork, BranchCharging, GenCost, GeneratorEnergySource};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

const KEY: &str = "org.eigenergy.powerio";

fn format(version: u8) -> TargetFormat {
    if version == 0 {
        TargetFormat::PowerSystemsJson
    } else {
        TargetFormat::SiennaJson { version }
    }
}

fn network() -> BalancedNetwork {
    powerio_tx::parse(
        Source::open(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data/case9.m"),
        )
        .unwrap(),
    )
    .unwrap()
    .into_value()
}

fn transformer_network() -> BalancedNetwork {
    powerio_tx::parse(
        Source::open(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data/case14.m"),
        )
        .unwrap(),
    )
    .unwrap()
    .into_value()
}

fn parse(doc: &Value, version: u8) -> PioModule<BalancedNetwork> {
    powerio_tx::parse(
        Source::from_memory("case.json", serde_json::to_vec(doc).unwrap())
            .unwrap()
            .with_format(FormatId::new(format(version).token()).unwrap()),
    )
    .unwrap()
}

fn document(net: &BalancedNetwork, version: u8) -> Value {
    serde_json::from_str(&emit_value(net, format(version)).unwrap().text).unwrap()
}

fn rows<'a>(doc: &'a Value, version: u8, kind: &str) -> Vec<&'a Value> {
    if version == 0 {
        doc["data"]["components"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["__metadata__"]["type"] == kind)
            .collect()
    } else {
        doc["components"][kind]
            .as_array()
            .into_iter()
            .flatten()
            .collect()
    }
}

fn row_mut<'a>(doc: &'a mut Value, version: u8, kind: &str, index: usize) -> &'a mut Value {
    if version == 0 {
        doc["data"]["components"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .filter(|r| r["__metadata__"]["type"] == kind)
            .nth(index)
            .unwrap()
    } else {
        &mut doc["components"][kind][index]
    }
}

fn identity(row: &Value, version: u8) -> Value {
    if version == 0 {
        row["internal"]["uuid"]["value"].clone()
    } else {
        row["id"].clone()
    }
}

fn extra<'a>(doc: &'a Value, version: u8, kind: &str, index: usize) -> &'a Value {
    let row = rows(doc, version, kind)[index];
    if version == 0 {
        &row["ext"][KEY]
    } else {
        &doc["ext"][row["id"].to_string()][KEY]
    }
}

fn extra_mut<'a>(doc: &'a mut Value, version: u8, kind: &str, index: usize) -> &'a mut Value {
    if version == 0 {
        &mut row_mut(doc, version, kind, index)["ext"][KEY]
    } else {
        let id = rows(doc, version, kind)[index]["id"].to_string();
        &mut doc["ext"][id][KEY]
    }
}

fn edited(
    module: &mut PioModule<BalancedNetwork>,
    version: u8,
) -> (Value, Vec<powerio_core::Diagnostic>) {
    module.value_mut().buses_mut()[0].vm += 0.001;
    let out = emit_module(module, format(version)).unwrap();
    (serde_json::from_str(&out.text).unwrap(), out.diagnostics)
}

#[test]
fn unlimited_transformer_primary_rating_is_numeric_and_round_trips() {
    // case9.m has no transformer; case14.m includes transformer branches.
    let mut net = transformer_network();
    let transformer = net
        .branches_mut()
        .iter_mut()
        .find(|branch| branch.is_transformer())
        .expect("case9 fixture contains a transformer");
    transformer.rate_a = f64::INFINITY;

    for version in [0, 1, 2, 3, 4] {
        let doc = document(&net, version);
        let kind = if version == 0 {
            "TapTransformer"
        } else {
            "TransformerCircuit"
        };
        let row = rows(&doc, version, kind)
            .into_iter()
            .next()
            .expect("transformer circuit is emitted");
        let rating = row["rating"]
            .as_f64()
            .expect("required primary rating is numeric");
        assert!(
            rating.is_finite() && rating > 0.0,
            "version {version}: {rating}"
        );

        let recovered = parse(&doc, version);
        let restored = recovered
            .value()
            .branches()
            .iter()
            .find(|branch| branch.is_transformer())
            .expect("transformer is restored");
        assert!(
            restored.rate_a.is_infinite(),
            "version {version}: {}",
            restored.rate_a
        );
    }
}

#[test]
fn absent_cost_survives_retained_edits_but_a_real_zero_cost_clears_the_marker() {
    for version in 0..=4 {
        let mut net = network();
        for g in net.generators_mut() {
            g.cost = None;
        }
        let mut module = parse(&document(&net, version), version);
        for _ in 0..3 {
            assert!(module.value().generators().iter().all(|g| g.cost.is_none()));
            let (doc, diagnostics) = edited(&mut module, version);
            assert_eq!(
                extra(&doc, version, "ThermalStandard", 0)["operation_cost_absent"],
                true
            );
            assert!(
                diagnostics
                    .iter()
                    .any(|d| d.code().ends_with("VALUE_DEFAULTED")
                        && d.message().contains("operation_cost defaults to zero"))
            );
            module = parse(&doc, version);
        }
        for g in module.value_mut().generators_mut() {
            g.cost = Some(GenCost::new(2, 0.0, 0.0, vec![0.0, 0.0]));
        }
        let (doc, _) = edited(&mut module, version);
        assert!(extra(&doc, version, "ThermalStandard", 0)["operation_cost_absent"].is_null());
        assert!(
            parse(&doc, version)
                .value()
                .generators()
                .iter()
                .all(|g| g.cost.is_some())
        );
    }
}

#[test]
fn a_native_cost_edit_overrides_a_stale_absent_marker() {
    let mut net = network();
    net.generators_mut()[0].cost = None;
    for opaque in [false, true] {
        let mut doc = document(&net, 4);
        let cost = &mut doc["components"]["ThermalStandard"][0]["operation_cost"];
        if opaque {
            *cost = json!({"cost_type":"THERMAL","fixed":3.0,"start_up":2.0,"shut_down":1.0,
                "variable_operation_cost":{"variable_cost_type":"COST","power_units":"NATURAL_UNITS",
                "value_curve":{"curve_type":"INPUT_OUTPUT","function_data":{"function_type":"PIECEWISE_STEP","x_coords":[0.0,50.0],"y_coords":[10.0]}}}});
        } else {
            cost["variable_operation_cost"]["value_curve"]["function_data"]["proportional_term"] =
                json!(7.0);
        }
        let expected = cost.clone();
        let mut module = parse(&doc, 4);
        assert_eq!(module.value().generators()[0].cost.is_none(), opaque);
        let (out, diagnostics) = edited(&mut module, 4);
        assert_eq!(
            out["components"]["ThermalStandard"][0]["operation_cost"],
            expected
        );
        assert!(extra(&out, 4, "ThermalStandard", 0)["operation_cost_absent"].is_null());
        assert!(
            !diagnostics
                .iter()
                .any(|d| d.message().contains("operation_cost defaults to zero"))
        );
    }
}

fn charging_network() -> BalancedNetwork {
    let mut net = network();
    let branch = &mut net.branches_mut()[0];
    branch.tap = 1.02;
    branch.charging = Some(BranchCharging::new(0.0, 0.0, 0.001, 0.02));
    branch.b = 0.02;
    net
}

#[test]
fn secondary_shunt_identity_and_markers_stay_stable_across_retained_edits() {
    for version in 0..=4 {
        let doc = document(&charging_network(), version);
        let original_id = identity(rows(&doc, version, "FixedAdmittance")[0], version);
        let mut module = parse(&doc, version);
        for _ in 0..3 {
            assert_eq!(module.value().shunts().as_slice(), []);
            let (out, _) = edited(&mut module, version);
            assert_eq!(rows(&out, version, "FixedAdmittance").len(), 1);
            assert_eq!(
                identity(rows(&out, version, "FixedAdmittance")[0], version),
                original_id
            );
            if version != 0 {
                let marked = out["ext"]
                    .as_object()
                    .unwrap()
                    .values()
                    .filter(|v| v[KEY]["transformer_circuit"].is_number())
                    .count();
                assert_eq!(marked, 1);
            }
            module = parse(&out, version);
            let charge = module
                .value()
                .branches()
                .iter()
                .find(|b| b.is_transformer())
                .unwrap()
                .charging
                .unwrap();
            assert!((charge.g_to - 0.001).abs() < 1e-12);
            assert!((charge.b_to - 0.02).abs() < 1e-12);
        }
    }
}

fn stale_shunt(doc: &mut Value, version: u8, edit: &str) {
    let marker = if version == 0 {
        "transformer_uuid"
    } else {
        "transformer_circuit"
    };
    match edit {
        "missing" => {
            let target = extra(doc, version, "FixedAdmittance", 0)[marker].clone();
            if version == 0 {
                doc["data"]["components"]
                    .as_array_mut()
                    .unwrap()
                    .retain(|r| identity(r, version) != target);
            } else {
                doc["components"]["TransformerCircuit"]
                    .as_array_mut()
                    .unwrap()
                    .clear();
                doc["components"]["TwoWindingTransformer"]
                    .as_array_mut()
                    .unwrap()
                    .clear();
            }
        }
        "kind" => {
            let target = identity(rows(doc, version, "Line")[0], version);
            extra_mut(doc, version, "FixedAdmittance", 0)[marker] = target;
        }
        "bus" => {
            let bus = rows(doc, version, "ACBus")
                .into_iter()
                .find(|r| r["number"] == 1)
                .unwrap();
            let id = identity(bus, version);
            row_mut(doc, version, "FixedAdmittance", 0)["bus"] = if version == 0 {
                json!({"value":id})
            } else {
                id
            };
        }
        "status" => row_mut(doc, version, "FixedAdmittance", 0)["available"] = json!(false),
        _ => panic!("unknown edit"),
    }
}

#[test]
fn stale_secondary_markers_preserve_native_shunts_and_do_not_reactivate() {
    for version in [0, 4] {
        for edit in ["missing", "kind", "bus", "status"] {
            let mut doc = document(&charging_network(), version);
            stale_shunt(&mut doc, version, edit);
            let mut module = parse(&doc, version);
            assert!(
                module
                    .diagnostics()
                    .iter()
                    .any(|d| d.code().ends_with("RETAINED_SOURCE_ONLY")
                        && d.message().contains("secondary-shunt marker"))
            );
            let shunt = module.value().shunts()[0].clone();
            assert_eq!(shunt.bus.0, if edit == "bus" { 1 } else { 4 });
            assert_eq!(shunt.in_service, edit != "status");
            assert!(shunt.uid.is_some());
            assert!((shunt.g - 0.1).abs() < 1e-12);
            assert!((shunt.b - 2.0).abs() < 1e-12);
            let (out, _) = edited(&mut module, version);
            let meta = extra(&out, version, "FixedAdmittance", 0);
            assert!(meta["transformer_circuit"].is_null());
            assert!(meta["transformer_uuid"].is_null());
            let recovered = parse(&out, version);
            assert_eq!(recovered.value().shunts()[0], shunt);
            assert!(
                !recovered
                    .diagnostics()
                    .iter()
                    .any(|d| d.message().contains("secondary-shunt marker"))
            );
        }
    }
}

#[test]
fn invalid_native_bus_references_still_error() {
    for version in [0, 4] {
        let mut doc = document(&charging_network(), version);
        row_mut(&mut doc, version, "FixedAdmittance", 0)["bus"] = if version == 0 {
            json!({"value":"missing-bus"})
        } else {
            json!(999_999)
        };
        let source = Source::from_memory("case.json", serde_json::to_vec(&doc).unwrap())
            .unwrap()
            .with_format(FormatId::new(format(version).token()).unwrap());
        assert!(powerio_tx::parse(source).is_err());
    }
}

#[test]
fn new_secondary_charging_does_not_reuse_an_independent_shunt_identity() {
    for version in [0, 4] {
        let mut doc = document(&charging_network(), version);
        stale_shunt(&mut doc, version, "status");
        let independent_id = identity(rows(&doc, version, "FixedAdmittance")[0], version);
        let mut module = parse(&doc, version);
        let independent = module.value().shunts()[0].clone();
        module
            .value_mut()
            .branches_mut()
            .iter_mut()
            .find(|b| b.is_transformer())
            .unwrap()
            .charging = Some(BranchCharging::new(0.0, 0.0, 0.003, 0.04));
        let (out, _) = edited(&mut module, version);
        let shunts = rows(&out, version, "FixedAdmittance");
        assert_eq!(shunts.len(), 2);
        assert_ne!(identity(shunts[0], version), identity(shunts[1], version));
        assert!(
            shunts
                .iter()
                .any(|r| identity(r, version) == independent_id)
        );
        let recovered = parse(&out, version);
        assert_eq!(recovered.value().shunts().as_slice(), &[independent]);
        let charge = recovered
            .value()
            .branches()
            .iter()
            .find(|b| b.is_transformer())
            .unwrap()
            .charging
            .unwrap();
        assert!((charge.g_to - 0.003).abs() < 1e-12);
        assert!((charge.b_to - 0.04).abs() < 1e-12);
    }
}

fn native_identities(doc: &Value) -> BTreeMap<String, Value> {
    doc["data"]["components"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            let refs: BTreeMap<_, _> = ["bus", "arc", "area", "load_zone", "from", "to"]
                .into_iter()
                .filter_map(|key| r.get(key).map(|v| (key, v.clone())))
                .collect();
            (
                identity(r, 0).as_str().unwrap().to_owned(),
                json!({"kind":r["__metadata__"]["type"],"references":refs}),
            )
        })
        .collect()
}

fn assert_native_references(doc: &Value) {
    let components = doc["data"]["components"].as_array().unwrap();
    let ids: BTreeSet<_> = components
        .iter()
        .map(|r| r["internal"]["uuid"]["value"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), components.len());
    for row in components {
        for field in [
            "bus",
            "arc",
            "area",
            "load_zone",
            "from",
            "to",
            "dynamic_injector",
        ] {
            if let Some(id) = row[field]["value"].as_str() {
                assert!(ids.contains(id), "{field}: {id}");
            }
        }
    }
}

#[test]
fn native_identity_and_unknown_extension_keys_survive_reordered_edit_cycles() {
    let mut doc = document(&network(), 0);
    row_mut(&mut doc, 0, "Line", 0)["ext"]["org.example.consumer"] = json!({"owner":"example"});
    extra_mut(&mut doc, 0, "Line", 0)["future_metadata"] = json!({"label":"retained"});
    let line_id = identity(rows(&doc, 0, "Line")[0], 0);
    let expected = native_identities(&doc);
    doc["data"]["components"].as_array_mut().unwrap().reverse();
    let mut module = parse(&doc, 0);
    for _ in 0..3 {
        let (out, _) = edited(&mut module, 0);
        assert_eq!(native_identities(&out), expected);
        assert_native_references(&out);
        let line = rows(&out, 0, "Line")
            .into_iter()
            .find(|r| identity(r, 0) == line_id)
            .unwrap();
        assert_eq!(
            line["ext"]["org.example.consumer"],
            json!({"owner":"example"})
        );
        assert_eq!(
            line["ext"][KEY]["future_metadata"],
            json!({"label":"retained"})
        );
        module = parse(&out, 0);
    }
}

#[test]
fn extensions_follow_live_components_and_retained_supplemental_attributes() {
    let mut doc = document(&network(), 4);
    let line = rows(&doc, 4, "Line")[0]["id"].to_string();
    doc["ext"][&line][KEY]["future_metadata"] = json!({"label":"retained"});
    doc["ext"][&line]["org.example.consumer"] = json!({"owner":"example"});
    doc["components"]["OnlineReserve"] = json!([{"id":9000,"name":"reserve"}]);
    doc["supplemental_attributes"] = json!([{"id":9001,"type":"annotation"}]);
    for id in ["9000", "9001", "9002"] {
        doc["ext"][id][KEY] = json!({"uid":"source-only","future_metadata":true});
    }
    let mut module = parse(&doc, 4);
    let (out, diagnostics) = edited(&mut module, 4);
    assert_eq!(
        out["ext"][&line][KEY]["future_metadata"],
        json!({"label":"retained"})
    );
    assert_eq!(
        out["ext"][&line]["org.example.consumer"],
        json!({"owner":"example"})
    );
    for id in ["9000", "9001"] {
        assert_eq!(out["ext"][id], doc["ext"][id]);
    }
    assert!(out["ext"].get("9002").is_none());
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code().ends_with("FIELD_DROPPED")
                && d.message().contains("extension entries for removed"))
    );
    let mut module = parse(&out, 4);
    module.value_mut().branches_mut().clear();
    let (removed, _) = edited(&mut module, 4);
    assert!(removed["ext"].get(&line).is_none());
}

#[test]
fn monitored_line_kind_is_preserved_but_a_native_kind_edit_takes_precedence() {
    let mut doc = document(&network(), 0);
    let row = row_mut(&mut doc, 0, "Line", 0);
    row["__metadata__"]["type"] = json!("MonitoredLine");
    row["flow_limits"] = json!({"from_to":2.0,"to_from":3.0});
    let mut module = parse(&doc, 0);
    for _ in 0..3 {
        let (out, _) = edited(&mut module, 0);
        assert_eq!(rows(&out, 0, "MonitoredLine").len(), 1);
        assert_eq!(
            rows(&out, 0, "MonitoredLine")[0]["flow_limits"],
            json!({"from_to":2.0,"to_from":3.0})
        );
        module = parse(&out, 0);
        doc = out;
    }
    row_mut(&mut doc, 0, "MonitoredLine", 0)["__metadata__"]["type"] = json!("Line");
    let mut module = parse(&doc, 0);
    let (out, _) = edited(&mut module, 0);
    assert_eq!(rows(&out, 0, "MonitoredLine").len(), 0);
}

#[test]
fn removing_a_projected_generator_cost_does_not_restore_the_retained_curve() {
    for version in 0..=4 {
        let mut module = parse(&document(&network(), version), version);
        assert!(module.value().generators().iter().all(|g| g.cost.is_some()));
        for g in module.value_mut().generators_mut() {
            g.cost = None;
        }
        let (out, diagnostics) = edited(&mut module, version);
        assert_eq!(
            extra(&out, version, "ThermalStandard", 0)["operation_cost_absent"],
            true
        );
        assert!(
            diagnostics
                .iter()
                .any(|d| d.message().contains("operation_cost defaults to zero"))
        );
        assert!(
            parse(&out, version)
                .value()
                .generators()
                .iter()
                .all(|g| g.cost.is_none())
        );
    }
}

#[test]
fn changing_cost_basis_keeps_the_staged_startup_loss_diagnostic() {
    let mut doc = document(&network(), 4);
    let cost = &mut doc["components"]["ThermalStandard"][0]["operation_cost"];
    cost["start_up"] = json!({"hot":2.0,"warm":3.0,"cold":4.0});
    cost["variable_operation_cost"]["power_units"] = json!("COMPONENT_BASE");
    let mut module = parse(&doc, 4);
    module.value_mut().generators_mut()[0].mbase *= 2.0;
    let (_, diagnostics) = edited(&mut module, 4);
    assert!(diagnostics.iter().any(
        |d| d.code().ends_with("FIELD_DROPPED") && d.message().contains("staged startup costs")
    ));
}

#[test]
fn partial_zero_cost_projections_do_not_hide_native_cost_edits() {
    let mut net = network();
    net.generators_mut()[0].cost = None;
    for staged in [false, true] {
        let mut doc = document(&net, 4);
        let cost = &mut doc["components"]["ThermalStandard"][0]["operation_cost"];
        if staged {
            cost["start_up"] = json!({"hot":2.0,"warm":3.0,"cold":4.0});
        } else {
            cost["variable_operation_cost"]["vom_cost"]["function_data"] = json!({"function_type":"QUADRATIC","quadratic_term":2.0,"proportional_term":0.0,"constant_term":0.0});
        }
        let expected = cost.clone();
        let mut module = parse(&doc, 4);
        assert!(module.value().generators()[0].cost.is_some());
        assert!(
            module
                .diagnostics()
                .iter()
                .any(|d| d.code().ends_with("RETAINED_SOURCE_ONLY"))
        );
        let (out, _) = edited(&mut module, 4);
        assert_eq!(
            out["components"]["ThermalStandard"][0]["operation_cost"],
            expected
        );
        assert!(extra(&out, 4, "ThermalStandard", 0)["operation_cost_absent"].is_null());
    }
}

#[test]
fn an_energy_source_edit_between_renewables_replaces_the_retained_prime_mover() {
    // Solar to wind keeps the RenewableDispatch component, but the prime
    // mover and power factor belong to the old source; retaining them would
    // read the generator back as solar.
    for version in 0..=4 {
        let mut net = network();
        net.generators_mut()[0].energy_source = GeneratorEnergySource::Solar;
        let mut doc = document(&net, version);
        row_mut(&mut doc, version, "RenewableDispatch", 0)["power_factor"] = json!(0.9);
        let mut module = parse(&doc, version);
        let generator = module
            .value_mut()
            .generators_mut()
            .iter_mut()
            .find(|g| g.energy_source == GeneratorEnergySource::Solar)
            .unwrap();
        generator.energy_source = GeneratorEnergySource::Wind;
        let (out, diagnostics) = edited(&mut module, version);
        let row = rows(&out, version, "RenewableDispatch")[0];
        assert_eq!(row["prime_mover_type"], "WT", "version {version}");
        assert_eq!(row["power_factor"], 1.0, "version {version}");
        assert!(
            diagnostics
                .iter()
                .any(|d| d.message().contains("changed energy source"))
        );
        let sources: Vec<_> = parse(&out, version)
            .value()
            .generators()
            .iter()
            .map(|g| g.energy_source)
            .collect();
        assert!(
            sources.contains(&GeneratorEnergySource::Wind),
            "{sources:?}"
        );
        assert!(
            !sources.contains(&GeneratorEnergySource::Solar),
            "{sources:?}"
        );
    }
}
