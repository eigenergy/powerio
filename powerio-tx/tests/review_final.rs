//! Fresh-path regressions for the final RAVENS review.
mod helpers;
use powerio_tx::{BalancedNetwork, Branch, Bus, BusId, BusType, Load, TargetFormat};
use serde_json::{Value, json};
fn document() -> Value {
    let mut n = BalancedNetwork::in_memory(
        "review",
        100.0,
        vec![
            Bus::new(BusId(1), BusType::Ref, 11.0),
            Bus::new(BusId(2), BusType::Pq, 11.0),
        ],
        vec![Branch::new(BusId(1), BusId(2), 0.01, 0.1)],
    );
    n.loads_mut().push(Load::new(BusId(2), 10.0, 2.0));
    serde_json::from_str(
        &helpers::emit_value(&n, TargetFormat::RavensJson)
            .unwrap()
            .text,
    )
    .unwrap()
}
fn mutate(v: &mut Value, kind: &str, f: &mut impl FnMut(&mut serde_json::Map<String, Value>)) {
    match v {
        Value::Object(o) => {
            if o.get("Ravens.cimObjectType").and_then(Value::as_str) == Some(kind) {
                f(o);
            }
            for v in o.values_mut() {
                mutate(v, kind, f);
            }
        }
        Value::Array(a) => {
            for v in a {
                mutate(v, kind, f);
            }
        }
        _ => {}
    }
}
#[test]
fn explicit_invalid_base_mva_is_rejected() {
    for invalid in [json!(0), json!(-100), json!(null), json!("100")] {
        let mut d = document();
        mutate(&mut d, "AlgorithmSettings", &mut |o| {
            for s in o["ApplicationSettings.Settings"].as_array_mut().unwrap() {
                if s["name"] == "baseMVA" {
                    s["value"] = invalid.clone();
                }
            }
        });
        let error =
            helpers::parse_str(&d.to_string(), "ravens-json").expect_err("invalid base must fail");
        assert!(error.to_string().contains("baseMVA"), "{error}");
    }
}
#[test]
fn absent_base_mva_still_defaults_and_valid_nondefault_base_round_trips() {
    let mut d = document();
    mutate(&mut d, "AlgorithmSettings", &mut |o| {
        o["ApplicationSettings.Settings"]
            .as_array_mut()
            .unwrap()
            .retain(|s| s["name"] != "baseMVA");
    });
    let n = helpers::parse_str(&d.to_string(), "ravens-json").unwrap();
    assert_eq!(n.network.base_mva().to_bits(), 100.0_f64.to_bits());
    assert!(
        n.diagnostics
            .iter()
            .any(|d| d.code() == "READ.RAVENS.VALUE_DEFAULTED" && d.message().contains("baseMVA"))
    );
    let mut d = document();
    mutate(&mut d, "AlgorithmSettings", &mut |o| {
        for s in o["ApplicationSettings.Settings"].as_array_mut().unwrap() {
            if s["name"] == "baseMVA" {
                s["value"] = json!(50.0);
            }
        }
    });
    let n = helpers::parse_str(&d.to_string(), "ravens-json").unwrap();
    assert_eq!(n.network.base_mva().to_bits(), 50.0_f64.to_bits());
    assert!(
        n.network
            .branches()
            .iter()
            .all(|b| b.r.is_finite() && b.x.is_finite() && b.b.is_finite())
    );
}
#[test]
fn unusable_equipment_terminals_always_name_the_dropped_record() {
    for (kind, family, count) in [
        ("EnergyConsumer", "load", 1),
        ("LinearShuntCompensator", "shunt", 1),
        ("SynchronousMachine", "generator", 1),
        ("AsynchronousMachine", "generator", 1),
        ("EnergySource", "source", 1),
        ("ACLineSegment", "line", 2),
    ] {
        for malformed in [json!([]), json!([{}]), json!(null)] {
            let mut d = document();
            d[kind] = json!({"missing-port":{"Ravens.cimObjectType":kind,"ConductingEquipment.Terminals":malformed}});
            let n = helpers::parse_str(&d.to_string(), "ravens-json").unwrap();
            assert!(
                n.diagnostics
                    .iter()
                    .any(|d| d.code() == "READ.RAVENS.RECORD_DROPPED"
                        && d.message().starts_with(&format!("{family} missing-port:"))
                        && d.message().contains(&format!("requires {count}"))),
                "{kind}: {:?}",
                n.diagnostics
            );
        }
    }
    // The original reproduction removes the property entirely.
    let mut d = document();
    mutate(&mut d, "EnergyConsumer", &mut |o| {
        o.remove("ConductingEquipment.Terminals");
    });
    let n = helpers::parse_str(&d.to_string(), "ravens-json").unwrap();
    assert_eq!(n.network.loads().len(), 0);
    assert!(
        n.diagnostics
            .iter()
            .any(|d| d.code() == "READ.RAVENS.RECORD_DROPPED" && d.message().starts_with("load "))
    );
}
#[test]
fn transformer_missing_winding_terminal_is_diagnosed() {
    let mut d = document();
    d["PowerTransformer"] = json!({"t":{"Ravens.cimObjectType":"PowerTransformer","PowerTransformer.PowerTransformerEnd":[{"TransformerEnd.endNumber":1},{"TransformerEnd.endNumber":2}]}});
    let n = helpers::parse_str(&d.to_string(), "ravens-json").unwrap();
    assert!(
        n.diagnostics
            .iter()
            .any(|d| d.code() == "READ.RAVENS.RECORD_DROPPED"
                && d.message().starts_with("transformer t:")
                && d.message().contains("terminal"))
    );
}
