use powerio::BusId;
use powerio_prob::scopf::json::{SCOPF_SCHEMA, to_json_value};
use powerio_prob::{ScopfDeviceClassLayout, ScopfError, ScopfInstance, parse_scopf_str};
use serde_json::Value;

const SMALL: &str = include_str!("data/goc3_small.json");

fn small_instance() -> ScopfInstance {
    parse_scopf_str(SMALL, "goc3-json").expect("build small SCOPF instance")
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
    assert_eq!(instance.dt, vec![1.0, 1.0]);
    assert_eq!((data.prod[0].j_dev, data.prod[0].j_sdd), (0, 0));
    assert_eq!((data.cons[0].j_dev, data.cons[0].j_sdd), (0, 1));
    // `initial_status.on_status`, straight off the document: the fixture
    // starts acl_00 on and acl_01 off.
    assert_eq!(
        data.acl_branch.iter().map(|b| b.u_0).collect::<Vec<_>>(),
        vec![1, 0]
    );
    assert_eq!(data.acx_branch[0].u_0, 1);
    assert_eq!(data.prod[0].u_0, 1);
    assert_eq!(data.cons[0].u_0, 1);

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
fn the_scopf_document_states_its_powerio_version_and_is_one_based() {
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

    let doc = to_json_value(&instance).expect("serialize the document");
    assert_eq!(doc["schema"], SCOPF_SCHEMA);
    assert_eq!(doc["powerio_version"], powerio::VERSION);
    assert_eq!(doc["index_base"], 1);
    assert_eq!(doc["instance"]["static"]["acl_branch"][0]["j_ln"], 1);
    assert_eq!(doc["instance"]["static"]["active_reserve"][0]["n_p"], 1);
    assert_eq!(doc["instance"]["price_blocks"]["producer"][0]["t"], 1);
    assert_eq!(doc["instance"]["static"]["bus"][0]["i"], 1);
    assert!(
        doc["instance"]["static"]["active_reserve"][0]
            .get("σ_rgu")
            .is_some()
    );
    assert!(doc["instance"]["lengths"].get("L_J_ln").is_some());
    // Renumbering is per declared field: counts and value fields pass
    // through unchanged even where a name doubles as an index elsewhere
    // (`p_max` on price blocks vs devices, `L_T` vs `t`).
    assert_eq!(
        doc["instance"]["lengths"]["L_T"],
        u64::try_from(instance.lengths.l_t).expect("period count")
    );
    assert_eq!(
        doc["instance"]["price_blocks"]["producer"][0]["p_max"],
        instance.price_blocks.producer[0].p_max
    );
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

    let first = parse_scopf_str(&text, "goc3-json").expect("build renamed fixture");
    let second = parse_scopf_str(&text, "goc3-json").expect("rebuild renamed fixture");
    assert_eq!(first, second);
    assert_eq!(first.static_data.acl_branch[0].uid, "line-zeta");
    assert_eq!(first.static_data.acl_branch[0].j_ln, 0);
    assert_eq!(first.static_data.acl_branch[1].uid, "line-alpha");
    assert_eq!(first.static_data.acl_branch[1].j_ln, 1);
    assert_eq!(first.static_data.prod[0].uid, "producer-main");
    // The device ordinals come from enumeration, so a uid with no digits at
    // all still addresses its rows.
    assert_eq!(first.static_data.prod[0].j_dev, 0);
    assert_eq!(first.static_data.prod[0].j_sdd, 0);
    assert_eq!(first.static_data.cons[0].uid, "consumer-main");
    assert_eq!(first.static_data.cons[0].j_dev, 0);
    assert_eq!(first.static_data.cons[0].j_sdd, 1);
    for row in &first.static_data.active_reserve_set_pr {
        assert_eq!((row.j_dev, row.j_sdd), (0, 0), "producer-main's ordinals");
    }
    for row in &first.static_data.reactive_reserve_set_cs {
        assert_eq!((row.j_dev, row.j_sdd), (0, 1), "consumer-main's ordinals");
    }
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
    let result: Result<ScopfInstance, ScopfError> = parse_scopf_str("{", "goc3-json");
    assert!(matches!(result, Err(ScopfError::Source(_))));
}

#[test]
fn source_format_is_explicit() {
    let error = parse_scopf_str(SMALL, "matpower").expect_err("reject unsupported SCOPF format");
    assert!(matches!(error, ScopfError::UnsupportedFormat(_)));
}

fn build_from_value(value: &Value) -> Result<ScopfInstance, ScopfError> {
    let text = serde_json::to_string(value).expect("serialize test document");
    parse_scopf_str(&text, "goc3-json")
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

/// Append a clone of a section's first row under a new uid, in both the
/// network section and, when present, its time series section.
fn append_zone(doc: &mut Value, section: &str, uid: &str) {
    for parent in ["network", "time_series_input"] {
        if let Some(rows) = doc[parent][section].as_array_mut() {
            let mut row = rows[0].clone();
            row["uid"] = Value::from(uid);
            rows.push(row);
        }
    }
}

/// Two zones whose document order differs from lexicographic order: the
/// reserve rows and the membership sets must assign one zone index per zone.
#[test]
fn reserve_zone_indices_agree_across_tables() {
    let mut doc: Value = serde_json::from_str(SMALL).expect("fixture json");
    // "azq_99"/"rzq_99" sort before the existing zones but come second in
    // document order.
    append_zone(&mut doc, "active_zonal_reserve", "azq_99");
    append_zone(&mut doc, "reactive_zonal_reserve", "rzq_99");
    let bus1 = &mut doc["network"]["bus"][1];
    bus1["active_reserve_uids"] = serde_json::json!(["azq_99"]);
    bus1["reactive_reserve_uids"] = serde_json::json!(["rzq_99"]);
    let text = serde_json::to_string(&doc).expect("serialize");

    let instance = parse_scopf_str(&text, "goc3-json").expect("build");
    let data = &instance.static_data;

    let n_p_of = |uid: &str| {
        data.active_reserve
            .iter()
            .find(|row| row.uid == uid)
            .map(|row| row.n_p)
            .expect("zone row")
    };
    let n_q_of = |uid: &str| {
        data.reactive_reserve
            .iter()
            .find(|row| row.uid == uid)
            .map(|row| row.n_q)
            .expect("zone row")
    };
    assert_eq!(n_p_of("azr_00"), 0);
    assert_eq!(n_p_of("azq_99"), 1);

    // sd_00 (producer) sits at bus_00 in zone azr_00; sd_01 (consumer) sits
    // at bus_01, now in zone azq_99/rzq_99. The membership sets must carry
    // the same zone indices as the reserve rows.
    assert_eq!(data.active_reserve_set_pr.len(), 1);
    assert_eq!(data.active_reserve_set_pr[0].uid, "sd_00");
    assert_eq!(data.active_reserve_set_pr[0].n_p, n_p_of("azr_00"));
    assert_eq!(data.active_reserve_set_cs.len(), 1);
    assert_eq!(data.active_reserve_set_cs[0].uid, "sd_01");
    assert_eq!(data.active_reserve_set_cs[0].n_p, n_p_of("azq_99"));
    assert_eq!(data.reactive_reserve_set_cs[0].n_q, n_q_of("rzq_99"));
}

#[test]
fn zero_series_impedance_is_rejected() {
    let mut doc: Value = serde_json::from_str(SMALL).expect("fixture json");
    doc["network"]["ac_line"][0]["r"] = Value::from(0.0);
    doc["network"]["ac_line"][0]["x"] = Value::from(0.0);
    let text = serde_json::to_string(&doc).expect("serialize");
    let error = parse_scopf_str(&text, "goc3-json").expect_err("zero impedance");
    assert!(error.to_string().contains("zero series impedance"));
}

/// The balanced GOC3 reader defaults an absent `device_type` to `producer`;
/// the SCOPF projection follows the same rule instead of erroring.
#[test]
fn missing_device_type_defaults_to_producer() {
    let mut doc: Value = serde_json::from_str(SMALL).expect("fixture json");
    doc["network"]["simple_dispatchable_device"][0]
        .as_object_mut()
        .expect("device object")
        .remove("device_type");
    let text = serde_json::to_string(&doc).expect("serialize");
    let instance = parse_scopf_str(&text, "goc3-json").expect("build");
    assert_eq!(instance.static_data.prod[0].uid, "sd_00");
    assert_eq!(instance.lengths.l_j_pr, 1);
}

/// The contingency count a client needs to size a per contingency array. It
/// must agree with the survivor groups the same instance carries.
#[test]
fn lengths_carry_the_contingency_count() {
    let instance = small_instance();
    assert_eq!(
        instance.lengths.k,
        instance.ac_contingency_survivors.ln.len()
    );
    assert_eq!(
        instance.lengths.k,
        instance.ac_contingency_survivors.xf.len()
    );
}

/// Shunts carry a per class index in document order, like every other class.
#[test]
fn shunt_rows_carry_a_document_order_index() {
    let instance = small_instance();
    assert_eq!(
        instance
            .static_data
            .shunt
            .iter()
            .map(|row| row.j_sh)
            .collect::<Vec<_>>(),
        (0..instance.lengths.l_j_sh).collect::<Vec<_>>()
    );
}

/// A device declares at most one reactive capability mode. The parameters of a
/// mode it did not declare are absent, so a model cannot read a silent zero.
#[test]
fn reactive_capability_reads_only_the_declared_mode() {
    let bounded = {
        let mut doc: Value = serde_json::from_str(SMALL).expect("fixture json");
        let device = doc["network"]["simple_dispatchable_device"][0]
            .as_object_mut()
            .expect("device object");
        device.insert("q_bound_cap".to_owned(), Value::from(1));
        device.insert("beta_ub".to_owned(), Value::from(0.5));
        device.insert("beta_lb".to_owned(), Value::from(-0.5));
        device.insert("q_0_ub".to_owned(), Value::from(2.0));
        device.insert("q_0_lb".to_owned(), Value::from(-2.0));
        let text = serde_json::to_string(&doc).expect("serialize");
        parse_scopf_str(&text, "goc3-json").expect("build bound cap")
    };
    let row = &bounded.static_data.prod[0];
    assert_eq!(row.q_bound_cap, 1);
    assert_eq!(row.q_linear_cap, 0);
    assert_eq!(row.beta_ub, Some(0.5));
    assert_eq!(row.q_0_lb, Some(-2.0));
    assert_eq!(row.beta, None, "the mode it did not declare stays absent");
    assert_eq!(row.q_p0, None);

    // The linear cap intercept and `initial_status.q` are different
    // quantities. The document spells both `q_0`; the row keeps both.
    let linear = {
        let mut doc: Value = serde_json::from_str(SMALL).expect("fixture json");
        let device = doc["network"]["simple_dispatchable_device"][0]
            .as_object_mut()
            .expect("device object");
        device.insert("q_linear_cap".to_owned(), Value::from(1));
        device.insert("beta".to_owned(), Value::from(0.25));
        device.insert("q_0".to_owned(), Value::from(1.5));
        let text = serde_json::to_string(&doc).expect("serialize");
        parse_scopf_str(&text, "goc3-json").expect("build linear cap")
    };
    let row = &linear.static_data.prod[0];
    assert_eq!(row.beta, Some(0.25));
    assert_eq!(row.q_p0, Some(1.5));
    assert!(
        row.q_0.abs() < 1e-12,
        "initial_status.q, not the capability intercept"
    );
    assert_eq!(row.beta_ub, None);

    // Neither mode: the fixture's own state.
    let neither = small_instance();
    let row = &neither.static_data.prod[0];
    assert_eq!((row.q_bound_cap, row.q_linear_cap), (0, 0));
    assert_eq!(row.beta_ub, None);
    assert_eq!(row.beta, None);
}

#[test]
fn both_reactive_capability_modes_are_rejected() {
    let mut doc: Value = serde_json::from_str(SMALL).expect("fixture json");
    let device = doc["network"]["simple_dispatchable_device"][0]
        .as_object_mut()
        .expect("device object");
    device.insert("q_bound_cap".to_owned(), Value::from(1));
    device.insert("q_linear_cap".to_owned(), Value::from(1));
    let text = serde_json::to_string(&doc).expect("serialize");
    let error = parse_scopf_str(&text, "goc3-json").expect_err("both modes");
    assert!(error.to_string().contains("mutually exclusive"));
}

#[test]
fn a_missing_capability_flag_is_rejected() {
    let mut doc: Value = serde_json::from_str(SMALL).expect("fixture json");
    doc["network"]["simple_dispatchable_device"][0]
        .as_object_mut()
        .expect("device object")
        .remove("q_bound_cap");
    let text = serde_json::to_string(&doc).expect("serialize");
    let error = parse_scopf_str(&text, "goc3-json").expect_err("missing flag");
    assert!(error.to_string().contains("q_bound_cap"));
}

/// Each violation price is separately optional. A price the document omits is
/// absent, so a model that prices that violation cannot read a free one.
#[test]
fn violation_prices_are_each_optional() {
    let instance = small_instance();
    assert_eq!(instance.violation_cost.p_bus, Some(1.0));
    assert_eq!(instance.violation_cost.e, Some(1.0));

    let mut doc: Value = serde_json::from_str(SMALL).expect("fixture json");
    doc["network"]["violation_cost"]
        .as_object_mut()
        .expect("violation cost object")
        .remove("e_vio_cost");
    let text = serde_json::to_string(&doc).expect("serialize");
    let instance = parse_scopf_str(&text, "goc3-json").expect("build");
    assert_eq!(instance.violation_cost.e, None);
    assert_eq!(instance.violation_cost.s, Some(1.0));
}

/// Both facts a model needs before it addresses a device by a per class offset
/// into one stacked variable vector.
#[test]
fn device_class_blocks_are_read_in_document_order() {
    let instance = small_instance();
    assert_eq!(
        instance.device_class_layout,
        ScopfDeviceClassLayout::Contiguous {
            producers_first: true
        }
    );

    let mut doc: Value = serde_json::from_str(SMALL).expect("fixture json");
    let devices = doc["network"]["simple_dispatchable_device"]
        .as_array_mut()
        .expect("device array");
    devices.swap(0, 1);
    let text = serde_json::to_string(&doc).expect("serialize");
    let instance = parse_scopf_str(&text, "goc3-json").expect("build");
    assert_eq!(
        instance.device_class_layout,
        ScopfDeviceClassLayout::Contiguous {
            producers_first: false
        }
    );
}

#[test]
fn interleaved_device_classes_are_reported() {
    let mut doc: Value = serde_json::from_str(SMALL).expect("fixture json");
    let devices = doc["network"]["simple_dispatchable_device"]
        .as_array_mut()
        .expect("device array");
    let mut third = devices[0].clone();
    third["uid"] = Value::from("sd_02");
    devices.push(third);
    let ts = doc["time_series_input"]["simple_dispatchable_device"]
        .as_array_mut()
        .expect("device time series");
    let mut third_ts = ts[0].clone();
    third_ts["uid"] = Value::from("sd_02");
    ts.push(third_ts);
    let text = serde_json::to_string(&doc).expect("serialize");
    let instance = parse_scopf_str(&text, "goc3-json").expect("build");
    assert_eq!(
        instance.device_class_layout,
        ScopfDeviceClassLayout::Interleaved,
        "producer, consumer, producer is three runs"
    );
    assert_eq!(
        instance.device_class_layout.producers_first(),
        None,
        "no offset scheme holds, so there is no order to read"
    );
    // The ordinals do not care that the classes interleave: producers pack
    // first (document order within the class), consumers after them.
    assert_eq!(instance.static_data.prod[0].uid, "sd_00");
    assert_eq!(instance.static_data.prod[1].uid, "sd_02");
    assert_eq!(
        instance
            .static_data
            .prod
            .iter()
            .map(|r| (r.j_dev, r.j_sdd))
            .collect::<Vec<_>>(),
        vec![(0, 0), (1, 1)]
    );
    assert_eq!(instance.static_data.cons[0].uid, "sd_01");
    assert_eq!(
        (
            instance.static_data.cons[0].j_dev,
            instance.static_data.cons[0].j_sdd
        ),
        (0, 2)
    );
}

#[test]
#[allow(deprecated)]
fn the_08_scopf_entry_point_still_answers() {
    let old = powerio_prob::build_scopf_instance_from_str(SMALL, "goc3-json")
        .expect("the 0.8 alias builds");
    let new = parse_scopf_str(SMALL, "goc3-json").expect("build small SCOPF instance");
    assert_eq!(old.lengths, new.lengths);
}
