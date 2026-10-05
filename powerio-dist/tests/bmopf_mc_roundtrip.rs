//! Synthetic witnesses of physical coefficients and conversion after canonical edits.
mod helpers;
use helpers::{emit_bmopf_json, parse_bmopf_str};
use serde_json::{Value, json};
fn cap() -> Value {
    json!({"meta":{"frequency":60}, "bus":{"b":{"terminal_names":["a","b","c","n"],"perfectly_grounded_terminals":["n"]}}, "capacitor":{"cap":{"bus":"b","terminal_map":["a","b","c","n"],"configuration":"WYE","q_rated":[13,25,7],"v_nom":230}}})
}
fn open_delta() -> Value {
    serde_json::from_str(r#"{"bus": {"s": {"terminal_names": ["a", "b", "c"]}, "b": {"terminal_names": ["x", "y", "z"]}}, "voltage_source": {"grid": {"bus": "s", "terminal_map": ["a", "b", "c"], "v_magnitude": [230, 230, 230], "v_angle": [0, -2.0943951023931953, 2.0943951023931953]}}, "meta": {"$schema": "https://raw.githubusercontent.com/distribution-system-opt/dsopt-schema/main/schema/bmopf/0.1.0/bmopf.schema.json", "frequency": 60}, "transformer": {"open_delta_regulator": {"tx": {"bus_from": "s", "bus_to": "b", "terminal_map_from": ["a", "b", "c"], "terminal_map_to": ["x", "y", "z"], "connection": "CABA", "regulator_type": "B", "tap_ratio": [1.01, 0.99], "s_rating": 2000, "r_series_from": 0.1, "r_series_to": 0.1, "x_series_from": 0.03, "x_series_to": 0.03, "g_no_load": 1e-05, "b_no_load": -2e-05}}}}"#).unwrap()
}
fn n_winding() -> Value {
    serde_json::from_str(r#"{"meta": {"$schema": "https://raw.githubusercontent.com/distribution-system-opt/dsopt-schema/main/schema/bmopf/0.1.0/bmopf.schema.json", "frequency": 60}, "bus": {"s": {"terminal_names": ["a", "n"], "perfectly_grounded_terminals": ["n"]}, "b": {"terminal_names": ["a", "n"], "perfectly_grounded_terminals": ["n"], "vpn_min": [100], "vpn_max": [130]}, "c": {"terminal_names": ["a", "n"], "perfectly_grounded_terminals": ["n"], "vpn_min": [50], "vpn_max": [70]}}, "voltage_source": {"grid": {"bus": "s", "terminal_map": ["a", "n"], "v_magnitude": [230, 0], "v_angle": [0, 0], "cost": [0.2]}}, "transformer": {"n_winding": {"tx": {"s_rating": 1000, "windings": [{"bus": "s", "terminal_map": ["a", "n"], "connection": "WYE", "v_nom": 230, "r_winding": 0.2, "s_rating": 1000, "s_max": 1000, "i_max": 20}, {"bus": "b", "terminal_map": ["a", "n"], "connection": "WYE", "v_nom": 115, "r_winding": 0.05, "s_rating": 1000, "s_max": 1000, "i_max": 20}, {"bus": "c", "terminal_map": ["a", "n"], "connection": "WYE", "v_nom": 57.5, "r_winding": 0.0125, "s_rating": 1000, "s_max": 1000, "i_max": 20}], "x_sc": {"1_2": 0.2, "1_3": 0.2, "2_3": 0.2}, "g_no_load": 1e-05, "b_no_load": -2e-05}}}}"#).unwrap()
}
fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-12, "{a} != {b}");
}

#[test]
fn unequal_capacitors_have_exact_terminal_susceptance() {
    let first = parse_bmopf_str(&cap().to_string()).unwrap();
    assert!(first.capacitors().is_empty());
    let sh = &first.shunts()[0];
    assert_eq!(sh.terminal_map, ["a", "b", "c", "n"]);
    for (k, q) in [13.0, 25.0, 7.0].into_iter().enumerate() {
        close(sh.b[k][k], q / 52900.0);
        close(sh.b[k][3], -q / 52900.0);
        close(sh.b[3][k], -q / 52900.0);
    }
    close(sh.b[3][3], 45.0 / 52900.0);
    let second = parse_bmopf_str(&emit_bmopf_json(&first).text).unwrap();
    assert_eq!(first.shunts(), second.shunts());
    let lowered = powerio_dist::prepare_lindist3flow_network(
        &first,
        powerio_dist::LinDist3FlowPreparationPolicy::Lower,
    )
    .unwrap();
    let sh = &lowered.network().shunts()[0];
    for (k, q) in [13.0, 25.0, 7.0].into_iter().enumerate() {
        close(sh.b[k][k], q / 52900.0);
    }
}
#[test]
fn capacitor_connection_and_rating_edits_are_not_overwritten() {
    let mut net = parse_bmopf_str(&cap().to_string()).unwrap();
    let sh = &mut net.shunts_mut()[0];
    sh.terminal_map.swap(0, 1);
    for row in &mut sh.b {
        for b in row {
            *b *= 2.0;
        }
    }
    let after = parse_bmopf_str(&emit_bmopf_json(&net).text).unwrap();
    assert_eq!(net.shunts(), after.shunts());
    close(after.shunts()[0].b[0][0], 26.0 / 52900.0);
    assert_eq!(after.shunts()[0].terminal_map[0], "b");
}
#[test]
fn delta_two_wire_and_zero_coils_have_exact_incidence() {
    let mut d = cap();
    d["capacitor"]["cap"]["configuration"] = json!("DELTA");
    d["capacitor"]["cap"]["terminal_map"] = json!(["a", "b", "c"]);
    let net = parse_bmopf_str(&d.to_string()).unwrap();
    let b = &net.shunts()[0].b;
    close(b[0][0], 20.0 / 52900.0);
    close(b[1][1], 38.0 / 52900.0);
    close(b[2][2], 32.0 / 52900.0);
    close(b[0][1], -13.0 / 52900.0);
    close(b[1][2], -25.0 / 52900.0);
    close(b[2][0], -7.0 / 52900.0);
    for config in ["DELTA", "WYE", "SINGLE_PHASE"] {
        d["capacitor"]["cap"]["configuration"] = json!(config);
        d["capacitor"]["cap"]["terminal_map"] = json!(["a", "b"]);
        d["capacitor"]["cap"]["q_rated"] = json!([13]);
        let net = parse_bmopf_str(&d.to_string()).unwrap();
        close(net.shunts()[0].b[0][1], -13.0 / 52900.0);
        d["capacitor"]["cap"]["q_rated"] = json!([0]);
        let net = parse_bmopf_str(&d.to_string()).unwrap();
        assert!(net.shunts()[0].b.iter().flatten().all(|x| *x == 0.0));
    }
}
#[test]
fn malformed_capacitor_arrays_fail_before_losing_physics() {
    for (key, value) in [
        ("q_rated", json!([1, 2])),
        ("q_rated", json!([-1, 2, 3])),
        ("q_rated", json!([null, 2, 3])),
        ("v_nom", json!(0)),
        ("v_nom", json!(1e-300)),
        ("configuration", json!("UNKNOWN")),
    ] {
        let mut d = cap();
        d["capacitor"]["cap"][key] = value;
        assert!(parse_bmopf_str(&d.to_string()).is_err(), "{d}");
    }
}
#[test]
fn synthesized_shunt_names_do_not_overwrite_declared_objects() {
    let mut d = cap();
    d["shunt"] =
        json!({"__bmopf_capacitor_cap":{"bus":"b","terminal_map":["a"],"G_1_1":0,"B_1_1":0.25}});
    let n = parse_bmopf_str(&d.to_string()).unwrap();
    assert_eq!(n.shunts().len(), 2);
    assert_ne!(n.shunts()[0].name, n.shunts()[1].name);
    assert!(n.shunts().iter().any(|s| s.b == vec![vec![0.25]]));
}
#[test]
fn open_delta_uses_custom_terminal_maps_on_both_sides() {
    let n = parse_bmopf_str(&open_delta().to_string()).unwrap();
    assert_eq!(n.transformers()[0].windings[0].terminal_map, ["c", "a"]);
    assert_eq!(n.transformers()[0].windings[1].terminal_map, ["z", "x"]);
    assert_eq!(n.transformers()[1].windings[0].terminal_map, ["b", "a"]);
    let after = parse_bmopf_str(&emit_bmopf_json(&n).text).unwrap();
    assert_eq!(n.transformers(), after.transformers());
}
#[test]
fn edited_open_delta_maps_do_not_restore_source_connections() {
    let mut n = parse_bmopf_str(&open_delta().to_string()).unwrap();
    n.transformers_mut()[0].windings[0].terminal_map.swap(0, 1);
    let output = emit_bmopf_json(&n);
    let d: Value = serde_json::from_str(&output.text).unwrap();
    assert!(d["transformer"]["open_delta_regulator"]["tx"].is_null());
    assert_eq!(
        d["transformer"]["single_phase_autotransformer"]["tx"]["terminal_map_from"],
        json!(["a", "c"])
    );
    assert!(
        output
            .diagnostics
            .iter()
            .any(|d| d.code() == "EMIT.BMOPF.VALUE_SUBSTITUTED")
    );
}
#[test]
fn n_winding_limits_survive_with_their_original_axis() {
    let n = parse_bmopf_str(&n_winding().to_string()).unwrap();
    let out = emit_bmopf_json(&n);
    let d: Value = serde_json::from_str(&out.text).unwrap();
    let after = parse_bmopf_str(&out.text).unwrap();
    assert_eq!(n.transformers(), after.transformers());
    for k in 0..3 {
        assert_eq!(
            d["transformer"]["n_winding"]["tx"]["windings"][k]["s_max"],
            json!(1000)
        );
    }
    let metadata = &n.transformers()[0].extras;
    for k in ["0", "1", "2"] {
        assert_eq!(metadata["bmopf_winding_metadata"][k]["s_max"], json!(1000));
    }
}

#[test]
fn scalar_ibr_bounds_remain_present_including_zero_availability() {
    let input = json!({"bus":{"b":{"terminal_names":["a","n"]}},"ibr":{"pv":{
        "bus":"b","terminal_map":["a","n"],"topology":"SINGLE_PHASE","prime_mover":"PV",
        "s_max":5250,"i_max":[30,30],"p_avail":0,"p_min":0,"p_max":0,"q_min":-5250,"q_max":5250
    }}});
    let net = parse_bmopf_str(&input.to_string()).unwrap();
    let inv = &net.ibrs()[0];
    assert_eq!(inv.s_max, vec![5250.0]);
    assert_eq!(inv.p_min, Some(vec![0.0]));
    assert_eq!(inv.p_max, Some(vec![0.0]));
    assert_eq!(inv.q_min, Some(vec![-5250.0]));
    assert_eq!(inv.q_max, Some(vec![5250.0]));
    assert_eq!(inv.p_avail, Some(0.0));
    let after = parse_bmopf_str(&emit_bmopf_json(&net).text).unwrap();
    assert_eq!(net.ibrs(), after.ibrs());
    let mut malformed = input;
    malformed["ibr"]["pv"]["p_max"] = json!("not a bound");
    let bad = parse_bmopf_str(&malformed.to_string()).unwrap();
    assert!(bad.ibrs()[0].p_max.as_ref().unwrap()[0].is_nan());
}
