//! The GOC3 input/problem data file decoder, tested below its public
//! [`crate::AcScucInstance`] entry point.

#![allow(clippy::float_cmp)]

use serde_json::Value;

use super::error::Goc3Error;
use super::projection::decode_goc3_str;
use crate::instance::scuc_inputs::{ScucDeviceKind, ScucInputs, ScucReactiveCapability};

const SMALL: &str = include_str!("../../tests/data/goc3_small.json");
type JsonMutation = fn(&mut Value);

fn small_inputs() -> ScucInputs {
    decode_goc3_str(SMALL, "goc3-json").expect("build small AC SCUC inputs")
}

#[test]
fn small_document_builds_source_neutral_nested_inputs() {
    let inputs = small_inputs();
    assert_eq!(inputs.interval_durations(), [1.0, 1.0]);
    assert_eq!(inputs.devices.len(), 2);

    let producer = inputs.device("sd_00").expect("producer");
    assert_eq!(producer.id.component_type(), "generator");
    assert_eq!(producer.kind, ScucDeviceKind::Producer);
    assert_eq!(producer.minimum_up_time, 1.0);
    assert_eq!(producer.minimum_down_time, 1.0);
    assert_eq!(producer.initial_commitment.accumulated_up_time, 4.0);
    assert_eq!(producer.initial_commitment.accumulated_down_time, 0.0);
    assert_eq!(producer.startup_limits[0].maximum_startups, 1);
    assert_eq!(producer.energy_upper_bounds[0].energy, 9.0);
    assert_eq!(producer.energy_lower_bounds[0].energy, 1.0);
    assert_eq!(producer.periods.len(), 2);
    assert!(producer.periods[0].on_status_min);
    assert!(!producer.periods[1].on_status_min);
    assert_eq!(
        producer.periods[1].energy_cost_blocks[0].marginal_cost,
        11.0
    );
    assert_eq!(producer.periods[1].energy_cost_blocks[0].block_size, 6.0);

    let consumer = inputs.device("sd_01").expect("consumer");
    assert_eq!(consumer.id.component_type(), "load");
    assert_eq!(consumer.kind, ScucDeviceKind::Consumer);
    assert_eq!(inputs.producers().count(), 1);
    assert_eq!(inputs.consumers().count(), 1);

    let shunt = inputs.shunt("sh_00").expect("shunt");
    assert_eq!(shunt.id.component_type(), "shunt");
    assert_eq!(
        (shunt.step_min, shunt.initial_step, shunt.step_max),
        (0, 1, 4)
    );

    assert_eq!(inputs.branch_switching_costs.len(), 3);
    assert_eq!(inputs.transformer_controls.len(), 1);
    let transformer = &inputs.transformer_controls[0];
    assert_eq!(transformer.id.local_id(), "xf_00");
    assert_eq!(
        (transformer.tap_ratio_min, transformer.tap_ratio_max),
        (1.0, 1.0)
    );

    assert_eq!(inputs.active_reserve_zones.len(), 1);
    assert_eq!(inputs.reactive_reserve_zones.len(), 1);
    assert_eq!(
        inputs.active_reserve_zones[0]
            .buses
            .iter()
            .map(powerio_core::ComponentId::local_id)
            .collect::<Vec<_>>(),
        ["bus_00", "bus_01"]
    );

    assert_eq!(inputs.contingencies.len(), 3);
    let outage = inputs.contingency("ctg_02").expect("transformer outage");
    assert_eq!(outage.id.component_type(), "contingency");
    assert_eq!(outage.components[0].component_type(), "transformer");
    assert_eq!(outage.components[0].local_id(), "xf_00");

    assert_eq!(inputs.violation_costs.active_power_balance, 1.0);
    assert_eq!(inputs.violation_costs.energy_requirement, 1.0);
}

#[test]
fn stored_shape_has_no_solver_rows_or_duplicated_network_tables() {
    let text = serde_json::to_string(&small_inputs()).expect("serialize inputs");
    for removed in [
        "static_data",
        "lengths",
        "price_blocks",
        "device_class_layout",
        "j_dev",
        "j_sdd",
        "j_ln",
        "j_xf",
        "g_sr",
        "b_sr",
        "fr_bus",
        "to_bus",
        "survivor",
    ] {
        assert!(
            !text.contains(removed),
            "removed solver field `{removed}` returned"
        );
    }
}

#[test]
fn arbitrary_uids_remain_stable_and_preserve_source_order() {
    let mut value: Value = serde_json::from_str(SMALL).expect("fixture JSON");
    let replacements = [
        ("bus_00", "north"),
        ("bus_01", "south"),
        ("acl_00", "line-zeta"),
        ("acl_01", "line-alpha"),
        ("xf_00", "transformer-main"),
        ("dc_00", "dc-tie"),
        ("sd_00", "producer-main"),
        ("sd_01", "consumer-main"),
        ("azr_00", "active-zone"),
        ("rzr_00", "reactive-zone"),
        ("ctg_00", "first-outage"),
        ("ctg_01", "second-outage"),
        ("ctg_02", "third-outage"),
    ];
    replace_exact_strings(&mut value, &replacements);
    let text = serde_json::to_string(&value).expect("serialize renamed fixture");

    let first = decode_goc3_str(&text, "goc3-json").expect("build renamed fixture");
    let second = decode_goc3_str(&text, "goc3-json").expect("rebuild renamed fixture");
    assert_eq!(first, second);
    assert_eq!(
        first
            .devices
            .iter()
            .map(|device| device.id.local_id())
            .collect::<Vec<_>>(),
        ["producer-main", "consumer-main"]
    );
    assert_eq!(
        first
            .branch_switching_costs
            .iter()
            .map(|branch| branch.id.local_id())
            .collect::<Vec<_>>(),
        ["line-zeta", "line-alpha", "transformer-main"]
    );
}

#[test]
fn startup_costs_and_energy_cost_blocks_keep_the_document_nesting() {
    let mut document: Value = serde_json::from_str(SMALL).expect("fixture JSON");
    document["network"]["simple_dispatchable_device"][0]["startup_states"] =
        serde_json::json!([[-20.0, 3.0], [-10.0, 8.0], [0.0, 24.0]]);
    document["time_series_input"]["simple_dispatchable_device"][0]["cost"][1] =
        serde_json::json!([[11.0, 2.0], [13.0, 4.0]]);
    let inputs = build_from_value(&document).expect("build nested costs");
    let producer = inputs.device("sd_00").unwrap();
    assert_eq!(producer.startup_cost_adjustments.len(), 3);
    assert_eq!(producer.startup_cost_adjustments[0].cost, -20.0);
    assert_eq!(producer.startup_cost_adjustments[0].maximum_down_time, 3.0);
    assert_eq!(producer.periods[1].energy_cost_blocks.len(), 2);
    assert_eq!(
        producer.periods[1].energy_cost_blocks[1].marginal_cost,
        13.0
    );
    assert_eq!(producer.periods[1].energy_cost_blocks[1].block_size, 4.0);
}

#[test]
fn reserve_membership_uses_bus_identities_not_device_positions() {
    let mut document: Value = serde_json::from_str(SMALL).expect("fixture JSON");
    append_zone(&mut document, "active_zonal_reserve", "active-second");
    append_zone(&mut document, "reactive_zonal_reserve", "reactive-second");
    document["network"]["bus"][0]["active_reserve_uids"] = serde_json::json!(["azr_00"]);
    document["network"]["bus"][0]["reactive_reserve_uids"] = serde_json::json!(["rzr_00"]);
    document["network"]["bus"][1]["active_reserve_uids"] = serde_json::json!(["active-second"]);
    document["network"]["bus"][1]["reactive_reserve_uids"] = serde_json::json!(["reactive-second"]);
    let inputs = build_from_value(&document).expect("build two zones");
    assert_eq!(inputs.active_reserve_zones[0].buses[0].local_id(), "bus_00");
    assert_eq!(inputs.active_reserve_zones[1].buses[0].local_id(), "bus_01");
    assert_eq!(
        inputs.reactive_reserve_zones[1].buses[0].local_id(),
        "bus_01"
    );
}

#[test]
fn reactive_capability_is_one_typed_choice() {
    let mut bounded: Value = serde_json::from_str(SMALL).expect("fixture JSON");
    let device = bounded["network"]["simple_dispatchable_device"][0]
        .as_object_mut()
        .unwrap();
    device.insert("q_bound_cap".into(), Value::from(1));
    device.insert("beta_ub".into(), Value::from(0.5));
    device.insert("beta_lb".into(), Value::from(-0.5));
    device.insert("q_0_ub".into(), Value::from(2.0));
    device.insert("q_0_lb".into(), Value::from(-2.0));
    let inputs = build_from_value(&bounded).expect("bounded capability");
    assert_eq!(
        inputs.device("sd_00").unwrap().reactive_capability,
        ScucReactiveCapability::Bounded {
            reactive_power_at_zero_active_power_min: -2.0,
            reactive_power_at_zero_active_power_max: 2.0,
            slope_min: -0.5,
            slope_max: 0.5,
        }
    );

    let mut linear: Value = serde_json::from_str(SMALL).expect("fixture JSON");
    let device = linear["network"]["simple_dispatchable_device"][0]
        .as_object_mut()
        .unwrap();
    device.insert("q_linear_cap".into(), Value::from(1));
    device.insert("q_0".into(), Value::from(1.5));
    device.insert("beta".into(), Value::from(0.25));
    let inputs = build_from_value(&linear).expect("linear capability");
    assert_eq!(
        inputs.device("sd_00").unwrap().reactive_capability,
        ScucReactiveCapability::Linear {
            reactive_power_at_zero_active_power: 1.5,
            slope: 0.25,
        }
    );
}

#[test]
fn duplicate_uid_is_rejected() {
    let mut value: Value = serde_json::from_str(SMALL).expect("fixture JSON");
    let duplicate = value["network"]["bus"][0].clone();
    value["network"]["bus"]
        .as_array_mut()
        .unwrap()
        .push(duplicate);
    let error = build_from_value(&value).expect_err("reject duplicate UID");
    assert!(error.to_string().contains("uid `bus_00` is repeated"));
}

#[test]
fn period_mismatch_is_rejected() {
    let mut value: Value = serde_json::from_str(SMALL).expect("fixture JSON");
    value["time_series_input"]["simple_dispatchable_device"][0]["p_ub"]
        .as_array_mut()
        .unwrap()
        .pop();
    let error = build_from_value(&value).expect_err("reject period mismatch");
    assert!(
        error
            .to_string()
            .contains("p_ub` has 1 periods; expected 2")
    );
}

#[test]
fn required_challenge3_fields_are_rejected_when_missing() {
    let cases = [
        ("/network/general/base_norm_mva", "base_norm_mva"),
        ("/network/violation_cost/e_vio_cost", "e_vio_cost"),
        ("/network/bus/0/base_nom_volt", "base_nom_volt"),
        ("/network/shunt/0/step_lb", "step_lb"),
        (
            "/network/simple_dispatchable_device/0/startups_ub",
            "startups_ub",
        ),
        (
            "/network/simple_dispatchable_device/0/initial_status/accu_up_time",
            "accu_up_time",
        ),
        (
            "/time_series_input/simple_dispatchable_device/0/on_status_lb",
            "on_status_lb",
        ),
        ("/network/dc_line/0/initial_status", "initial_status"),
    ];
    for (pointer, expected) in cases {
        let mut document: Value = serde_json::from_str(SMALL).expect("fixture JSON");
        remove_field(&mut document, pointer);
        let error = build_from_value(&document).expect_err(pointer);
        assert!(error.to_string().contains(expected), "{pointer}: {error}");
    }
}

#[test]
fn identity_reserve_and_contingency_references_are_validated() {
    let mut extra_series: Value = serde_json::from_str(SMALL).expect("fixture JSON");
    let mut row = extra_series["time_series_input"]["simple_dispatchable_device"][0].clone();
    row["uid"] = Value::from("extra-device");
    extra_series["time_series_input"]["simple_dispatchable_device"]
        .as_array_mut()
        .unwrap()
        .push(row);
    assert!(
        build_from_value(&extra_series)
            .expect_err("extra time series")
            .to_string()
            .contains("uid sets differ")
    );

    let mut reserve: Value = serde_json::from_str(SMALL).expect("fixture JSON");
    reserve["network"]["bus"][0]["active_reserve_uids"] = serde_json::json!(["missing-zone"]);
    assert!(
        build_from_value(&reserve)
            .expect_err("unknown reserve zone")
            .to_string()
            .contains("unknown reserve zone `missing-zone`")
    );

    let mut outage: Value = serde_json::from_str(SMALL).expect("fixture JSON");
    outage["reliability"]["contingency"][0]["components"] = serde_json::json!(["acl_00", "xf_00"]);
    assert!(
        build_from_value(&outage)
            .expect_err("two outages")
            .to_string()
            .contains("exactly one branch outage")
    );
}

#[test]
fn invalid_binary_and_reactive_capability_selection_are_rejected() {
    let mut binary: Value = serde_json::from_str(SMALL).expect("fixture JSON");
    binary["time_series_input"]["simple_dispatchable_device"][0]["on_status_ub"][0] =
        Value::from(2);
    assert!(
        build_from_value(&binary)
            .expect_err("invalid binary")
            .to_string()
            .contains("on_status_ub")
    );

    let mut both: Value = serde_json::from_str(SMALL).expect("fixture JSON");
    both["network"]["simple_dispatchable_device"][0]["q_bound_cap"] = Value::from(1);
    both["network"]["simple_dispatchable_device"][0]["q_linear_cap"] = Value::from(1);
    assert!(
        build_from_value(&both)
            .expect_err("two reactive capability forms")
            .to_string()
            .contains("mutually exclusive")
    );

    let mut empty_bounded_set: Value = serde_json::from_str(SMALL).expect("fixture JSON");
    let device = empty_bounded_set["network"]["simple_dispatchable_device"][0]
        .as_object_mut()
        .unwrap();
    device.insert("q_bound_cap".into(), Value::from(1));
    device.insert("beta_ub".into(), Value::from(0.5));
    device.insert("beta_lb".into(), Value::from(0.5));
    device.insert("q_0_ub".into(), Value::from(-1.0));
    device.insert("q_0_lb".into(), Value::from(1.0));
    assert!(
        build_from_value(&empty_bounded_set)
            .expect_err("empty bounded reactive capability")
            .to_string()
            .contains("bounded reactive capability is empty")
    );
}

#[test]
fn empty_device_sections_are_schema_valid() {
    let mut document: Value = serde_json::from_str(SMALL).expect("fixture JSON");
    document["network"]["simple_dispatchable_device"] = serde_json::json!([]);
    document["time_series_input"]["simple_dispatchable_device"] = serde_json::json!([]);
    let inputs = build_from_value(&document).expect("empty device sections");
    assert!(inputs.devices.is_empty());
}

#[test]
fn official_bounds_and_initial_values_are_checked() {
    let cases: [(&str, JsonMutation); 6] = [
        ("bus voltage", |document: &mut Value| {
            document["network"]["bus"][0]["initial_status"]["vm"] = Value::from(1.2);
        }),
        ("active power", |document: &mut Value| {
            document["time_series_input"]["simple_dispatchable_device"][0]["p_lb"][0] =
                Value::from(6.0);
        }),
        ("reactive power", |document: &mut Value| {
            document["time_series_input"]["simple_dispatchable_device"][0]["q_lb"][0] =
                Value::from(2.0);
        }),
        ("transformer tap", |document: &mut Value| {
            document["network"]["two_winding_transformer"][0]["initial_status"]["tm"] =
                Value::from(1.1);
        }),
        ("dc active power", |document: &mut Value| {
            document["network"]["dc_line"][0]["initial_status"]["pdc_fr"] = Value::from(2.0);
        }),
        ("dc reactive power", |document: &mut Value| {
            document["network"]["dc_line"][0]["initial_status"]["qdc_to"] = Value::from(2.0);
        }),
    ];
    for (name, mutate) in cases {
        let mut document: Value = serde_json::from_str(SMALL).expect("fixture JSON");
        mutate(&mut document);
        assert!(build_from_value(&document).is_err(), "{name}");
    }
}

#[test]
fn official_window_and_initial_commitment_rules_are_checked() {
    let cases: [(&str, JsonMutation); 6] = [
        ("negative window start", |document: &mut Value| {
            document["network"]["simple_dispatchable_device"][0]["startups_ub"][0][0] =
                Value::from(-1.0);
        }),
        ("reversed energy window", |document: &mut Value| {
            document["network"]["simple_dispatchable_device"][0]["energy_req_ub"][0][1] =
                Value::from(-1.0);
        }),
        ("negative energy bound", |document: &mut Value| {
            document["network"]["simple_dispatchable_device"][0]["energy_req_lb"][0][2] =
                Value::from(-1.0);
        }),
        ("negative ramp limit", |document: &mut Value| {
            document["network"]["simple_dispatchable_device"][0]["p_ramp_up_ub"] =
                Value::from(-1.0);
        }),
        ("on without accumulated up time", |document: &mut Value| {
            document["network"]["simple_dispatchable_device"][0]["initial_status"]["accu_up_time"] =
                Value::from(0.0);
        }),
        ("off with accumulated up time", |document: &mut Value| {
            let initial =
                &mut document["network"]["simple_dispatchable_device"][0]["initial_status"];
            initial["on_status"] = Value::from(0);
            initial["accu_down_time"] = Value::from(0.0);
        }),
    ];
    for (name, mutate) in cases {
        let mut document: Value = serde_json::from_str(SMALL).expect("fixture JSON");
        mutate(&mut document);
        assert!(build_from_value(&document).is_err(), "{name}");
    }
}

#[test]
fn energy_cost_blocks_have_the_official_shape_size_and_coverage() {
    let cases: [(&str, JsonMutation); 3] = [
        ("wrong block shape", |document: &mut Value| {
            document["time_series_input"]["simple_dispatchable_device"][0]["cost"][0][0] =
                serde_json::json!([10.0]);
        }),
        ("negative block size", |document: &mut Value| {
            document["time_series_input"]["simple_dispatchable_device"][0]["cost"][0][0][1] =
                Value::from(-1.0);
        }),
        ("does not cover p_ub", |document: &mut Value| {
            document["time_series_input"]["simple_dispatchable_device"][0]["cost"][0][0][1] =
                Value::from(4.0);
        }),
    ];
    for (name, mutate) in cases {
        let mut document: Value = serde_json::from_str(SMALL).expect("fixture JSON");
        mutate(&mut document);
        assert!(build_from_value(&document).is_err(), "{name}");
    }
}

#[test]
fn nonnegative_costs_and_reserve_requirements_follow_the_official_model() {
    let mut zero: Value = serde_json::from_str(SMALL).expect("fixture JSON");
    zero["network"]["violation_cost"]["p_bus_vio_cost"] = Value::from(0.0);
    zero["network"]["active_zonal_reserve"][0]["REG_UP_vio_cost"] = Value::from(0.0);
    zero["time_series_input"]["reactive_zonal_reserve"][0]["REACT_UP"][0] = Value::from(0.0);
    build_from_value(&zero).expect("the official model permits zero costs and requirements");

    let cases: [(&str, JsonMutation); 3] = [
        ("negative violation cost", |document: &mut Value| {
            document["network"]["violation_cost"]["e_vio_cost"] = Value::from(-1.0);
        }),
        ("negative reserve violation cost", |document: &mut Value| {
            document["network"]["active_zonal_reserve"][0]["SYN_vio_cost"] = Value::from(-1.0);
        }),
        ("negative reserve requirement", |document: &mut Value| {
            document["time_series_input"]["active_zonal_reserve"][0]["RAMPING_RESERVE_UP"][0] =
                Value::from(-1.0);
        }),
    ];
    for (name, mutate) in cases {
        let mut document: Value = serde_json::from_str(SMALL).expect("fixture JSON");
        mutate(&mut document);
        assert!(build_from_value(&document).is_err(), "{name}");
    }
}

#[test]
fn timestamps_remain_optional_but_ordered_when_both_are_present() {
    let mut one_timestamp: Value = serde_json::from_str(SMALL).expect("fixture JSON");
    one_timestamp["network"]["general"]["timestamp_start"] = Value::from("2023-01-01T00:00:00");
    build_from_value(&one_timestamp).expect("the official schema makes timestamps optional");

    let mut reversed = one_timestamp;
    reversed["network"]["general"]["timestamp_stop"] = Value::from("2022-12-31T23:00:00");
    assert!(
        build_from_value(&reversed)
            .expect_err("reject reversed timestamps")
            .to_string()
            .contains("timestamp_start < timestamp_stop")
    );
}

#[test]
fn parse_errors_keep_the_typed_error_and_format_is_explicit() {
    let malformed: Result<ScucInputs, Goc3Error> = decode_goc3_str("{", "goc3-json");
    assert!(matches!(malformed, Err(Goc3Error::Source(_))));
    let wrong = decode_goc3_str(SMALL, "matpower").expect_err("wrong format");
    assert!(matches!(wrong, Goc3Error::UnsupportedFormat(_)));
}

fn build_from_value(value: &Value) -> Result<ScucInputs, Goc3Error> {
    let text = serde_json::to_string(value).expect("serialize document");
    decode_goc3_str(&text, "goc3-json")
}

fn remove_field(document: &mut Value, pointer: &str) {
    let (parent, field) = pointer.rsplit_once('/').expect("field pointer");
    document
        .pointer_mut(parent)
        .and_then(Value::as_object_mut)
        .expect("object containing field")
        .remove(field)
        .expect("field exists");
}

fn replace_exact_strings(value: &mut Value, replacements: &[(&str, &str)]) {
    match value {
        Value::String(text) => {
            if let Some((_, replacement)) = replacements.iter().find(|(source, _)| text == source) {
                *text = (*replacement).to_owned();
            }
        }
        Value::Array(values) => {
            for value in values {
                replace_exact_strings(value, replacements);
            }
        }
        Value::Object(object) => {
            for value in object.values_mut() {
                replace_exact_strings(value, replacements);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn append_zone(document: &mut Value, section: &str, uid: &str) {
    for parent in ["network", "time_series_input"] {
        let rows = document[parent][section]
            .as_array_mut()
            .expect("zone array");
        let mut row = rows[0].clone();
        row["uid"] = Value::from(uid);
        rows.push(row);
    }
}
