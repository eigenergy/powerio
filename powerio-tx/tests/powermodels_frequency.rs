//! Explicit PowerIO frequency metadata survives the dictionary and RAW bridge.
mod helpers;
use helpers::*;
use powerio_tx::{DEFAULT_BASE_FREQUENCY, TargetFormat};

fn document(frequency: Option<serde_json::Value>) -> serde_json::Value {
    let mut value = serde_json::json!({"baseMVA":100,"per_unit":true,
        "bus":{"1":{"bus_i":1,"bus_type":3,"base_kv":220}}});
    if let Some(frequency) = frequency {
        value["base_frequency"] = frequency;
    }
    value
}

#[test]
#[allow(clippy::float_cmp)] // Exact stated metadata, not a computed quantity.
fn explicit_frequency_survives_public_parse_and_fresh_emission() {
    for frequency in [50.0, 60.0, 16.7] {
        let text = document(Some(serde_json::json!(frequency))).to_string();
        let net = parse_powermodels_json(&text).unwrap();
        assert_eq!(net.base_frequency(), frequency);
        for target in [
            TargetFormat::PowerModelsJson,
            TargetFormat::Psse { rev: 33 },
        ] {
            let written = emit_value(&net, target).unwrap();
            assert!(
                !written
                    .render_diagnostics()
                    .iter()
                    .any(|d| d.contains("frequency"))
            );
            let back = match target {
                TargetFormat::PowerModelsJson => {
                    let json: serde_json::Value = serde_json::from_str(&written.text).unwrap();
                    assert_eq!(json["base_frequency"], frequency);
                    parse_powermodels_json(&written.text).unwrap()
                }
                _ => parse_psse(&written.text).unwrap(),
            };
            assert_eq!(back.base_frequency(), frequency);
        }
    }
}

#[test]
#[allow(clippy::float_cmp)]
fn omitted_frequency_keeps_the_existing_default() {
    let net = parse_powermodels_json(&document(None).to_string()).unwrap();
    assert_eq!(net.base_frequency(), DEFAULT_BASE_FREQUENCY);
}

#[test]
fn invalid_stated_frequency_does_not_silently_default() {
    for value in [
        serde_json::json!(0),
        serde_json::json!(-50),
        serde_json::json!("50"),
        serde_json::json!("NaN"),
        serde_json::json!(true),
        serde_json::Value::Null,
        serde_json::json!([]),
        serde_json::json!({}),
    ] {
        assert!(
            parse_powermodels_json(&document(Some(value.clone())).to_string()).is_err(),
            "accepted {value}"
        );
    }
    // JSON cannot encode nonfinite f64 values; reject their spellings too.
    for value in ["1e999", "NaN", "Infinity"] {
        let text = format!("{{\"baseMVA\":100,\"base_frequency\":{value}}}");
        assert!(parse_powermodels_json(&text).is_err(), "accepted {value}");
    }
}
