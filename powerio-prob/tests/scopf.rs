use powerio::{BusId, Goc3Document};
use powerio_prob::scopf::wire::{SCOPF_WIRE_SCHEMA, SCOPF_WIRE_VERSION, to_wire_value};
use powerio_prob::{
    ScopfError, ScopfInstance, build_scopf_instance, build_scopf_instance_from_str,
};
use serde_json::Value;

const SMALL: &str = include_str!("data/goc3_small.json");
const REAL_14_BUS: &str = include_str!("data/goc3_14bus_20220707.json");

fn small_instance() -> ScopfInstance {
    build_scopf_instance_from_str(SMALL).expect("build small SCOPF instance")
}

#[test]
fn small_instance_preserves_source_ids_and_uses_zero_based_indices() {
    let instance = small_instance();
    let lengths = instance.lengths;
    assert_eq!(lengths.l_j_ln, 2);
    assert_eq!(lengths.l_j_xf, 1);
    assert_eq!(lengths.l_j_ac, 3);
    assert_eq!(lengths.l_j_dc, 1);
    assert_eq!(lengths.l_j_br, 4);
    assert_eq!(lengths.l_j_pr, 1);
    assert_eq!(lengths.l_j_cs, 1);
    assert_eq!(lengths.l_j_cspr, 2);
    assert_eq!(lengths.i, 2);
    assert_eq!(lengths.l_t, 2);
    assert_eq!(lengths.l_n_p, 1);
    assert_eq!(lengths.l_n_q, 1);

    let data = &instance.static_data;
    assert_eq!(
        data.bus.iter().map(|bus| bus.i).collect::<Vec<_>>(),
        vec![BusId(1), BusId(2)]
    );
    assert_eq!(
        data.bus
            .iter()
            .map(|bus| bus.uid.as_str())
            .collect::<Vec<_>>(),
        vec!["bus_00", "bus_01"]
    );
    assert_eq!(
        data.acl_branch
            .iter()
            .map(|branch| branch.j_ln)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert_eq!(data.acx_branch[0].j_xf, 0);
    assert_eq!(data.dc_branch[0].j_dc, 0);
    assert_eq!(data.fpd[0].j_xf, 0);
    assert_eq!(data.fwr[0].j_xf, 0);
    assert_eq!(data.active_reserve[0].n_p, 0);
    assert_eq!(data.reactive_reserve[0].n_q, 0);
    assert_eq!(data.prod[0].uid, "sd_00");
    assert_eq!(data.cons[0].uid, "sd_01");

    assert_eq!(
        instance
            .energy_windows
            .t_w_en_max_pr
            .iter()
            .map(|row| row.t)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert_eq!(
        instance
            .price_blocks
            .producer
            .iter()
            .map(|row| (row.flat_k, row.t, row.m))
            .collect::<Vec<_>>(),
        vec![(0, 0, 0), (1, 1, 0)]
    );
    assert_eq!(instance.ac_contingency_survivors.ln[0][0].ctg, 0);
    assert_eq!(instance.ac_contingency_survivors.ln[0][0].j_ln, 1);
    assert_eq!(
        instance
            .dc_contingency_flows
            .iter()
            .map(|row| (row.flat_jtk_dc, row.ctg, row.j_dc, row.t))
            .collect::<Vec<_>>(),
        vec![(0, 0, 0, 0), (1, 0, 0, 1), (2, 2, 0, 0), (3, 2, 0, 1)]
    );
}

#[test]
fn shared_document_builds_the_same_instance() {
    let document = Goc3Document::parse(SMALL).expect("parse shared document");
    let from_document = build_scopf_instance(&document).expect("build from document");
    assert_eq!(from_document, small_instance());
    assert!(document.network().is_ok());
    assert!(document.time_series_input().is_ok());
    assert!(document.reliability().is_some());
}

#[test]
fn julia_wire_adapter_is_versioned_and_one_based() {
    let instance = small_instance();
    let internal = serde_json::to_value(&instance).expect("serialize internal instance");
    assert!(internal.get("static_data").is_some());
    assert!(internal.get("static").is_none());
    assert!(internal["lengths"].get("l_j_ln").is_some());
    assert!(
        internal["static_data"]["active_reserve"][0]
            .get("sigma_rgu")
            .is_some()
    );

    let wire = to_wire_value(&instance).expect("serialize wire value");
    assert_eq!(wire["schema"], SCOPF_WIRE_SCHEMA);
    assert_eq!(wire["schema_version"], SCOPF_WIRE_VERSION);
    assert_eq!(wire["index_base"], 1);
    assert_eq!(wire["instance"]["static"]["acl_branch"][0]["j_ln"], 1);
    assert_eq!(wire["instance"]["static"]["active_reserve"][0]["n_p"], 1);
    assert_eq!(wire["instance"]["price_blocks"]["producer"][0]["t"], 1);
    assert_eq!(wire["instance"]["static"]["bus"][0]["i"], 1);
    assert!(
        wire["instance"]["static"]["active_reserve"][0]
            .get("σ_rgu")
            .is_some()
    );
    assert!(wire["instance"]["lengths"].get("L_J_ln").is_some());
}

#[test]
fn arbitrary_uids_preserve_document_order() {
    let mut value: Value = serde_json::from_str(SMALL).expect("parse fixture JSON");
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

    let first = build_scopf_instance_from_str(&text).expect("build renamed fixture");
    let second = build_scopf_instance_from_str(&text).expect("rebuild renamed fixture");
    assert_eq!(first, second);
    assert_eq!(first.static_data.acl_branch[0].uid, "line-zeta");
    assert_eq!(first.static_data.acl_branch[0].j_ln, 0);
    assert_eq!(first.static_data.acl_branch[1].uid, "line-alpha");
    assert_eq!(first.static_data.acl_branch[1].j_ln, 1);
    assert_eq!(first.static_data.prod[0].uid, "producer-main");
}

#[test]
fn duplicate_uid_is_rejected() {
    let mut value: Value = serde_json::from_str(SMALL).expect("parse fixture JSON");
    let duplicate = value["network"]["bus"][0].clone();
    value["network"]["bus"]
        .as_array_mut()
        .expect("bus array")
        .push(duplicate);
    let error = build_from_value(&value).expect_err("reject duplicate UID");
    assert!(error.to_string().contains("duplicate bus uid `bus_00`"));
}

#[test]
fn missing_reference_is_rejected() {
    let mut value: Value = serde_json::from_str(SMALL).expect("parse fixture JSON");
    value["network"]["ac_line"][0]["to_bus"] = Value::String("missing-bus".into());
    let error = build_from_value(&value).expect_err("reject missing bus reference");
    assert!(error.to_string().contains("unknown bus uid `missing-bus`"));
}

#[test]
fn period_mismatch_is_rejected() {
    let mut value: Value = serde_json::from_str(SMALL).expect("parse fixture JSON");
    value["time_series_input"]["simple_dispatchable_device"][0]["p_ub"]
        .as_array_mut()
        .expect("p_ub array")
        .pop();
    let error = build_from_value(&value).expect_err("reject period mismatch");
    assert!(
        error
            .to_string()
            .contains("`p_ub` has 1 periods; expected 2")
    );
}

#[test]
fn parse_errors_use_the_scopf_error_type() {
    let result: Result<ScopfInstance, ScopfError> = build_scopf_instance_from_str("{");
    assert!(matches!(result, Err(ScopfError::Source(_))));
}

#[test]
fn vendored_real_case_runs_in_normal_tests() {
    let instance = build_scopf_instance_from_str(REAL_14_BUS).expect("build real 14-bus case");
    let lengths = instance.lengths;
    assert_eq!(lengths.i, 14);
    assert_eq!((lengths.l_j_ln, lengths.l_j_xf, lengths.l_j_dc), (17, 3, 0));
    assert_eq!((lengths.l_j_pr, lengths.l_j_cs, lengths.l_t), (6, 11, 24));
    assert_eq!(instance.static_data.prod.len(), 6);
    assert_eq!(instance.static_data.cons.len(), 11);
    assert_eq!(instance.price_blocks.producer.len(), 720);
    assert_eq!(instance.price_blocks.consumer.len(), 1056);
    assert_eq!(instance.ac_contingency_survivors.ln.len(), 19);
    assert_eq!(instance.ac_contingency_survivors.xf.len(), 19);
    assert!(instance.dc_contingency_flows.is_empty());
}

fn build_from_value(value: &Value) -> Result<ScopfInstance, ScopfError> {
    let text = serde_json::to_string(value).expect("serialize test document");
    build_scopf_instance_from_str(&text)
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
