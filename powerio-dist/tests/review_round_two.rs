//! Regressions for the neutral, bounds and load-law review follow-up.
mod helpers;
#[allow(dead_code)]
#[path = "helpers/source_equivalents.rs"]
mod source_equivalents;
use powerio_dist::{DistTargetFormat, MulticonductorNetwork};
use serde_json::{Value, json};

fn close(a: f64, b: f64) {
    assert!(
        (a - b).abs() < 1e-9 * a.abs().max(b.abs()).max(1.0),
        "{a} != {b}"
    );
}

fn switch_network() -> MulticonductorNetwork {
    let text = json!({"data_model":"ENGINEERING","bus":{"a":{"terminals":[1,2,3,4],"grounded":[4]},"b":{"terminals":[1,2,3,4],"grounded":[4]}},"settings":{"vbases_default":{"a":0.23}},"switch":{"s":{"f_bus":"a","t_bus":"b","f_connections":[1,2,3,4],"t_connections":[1,2,3,4],"rs":[[4,0,0,2],[0,4,0,2],[0,0,4,2],[2,2,2,4]],"xs":[[0,0,0,0],[0,0,0,0],[0,0,0,0],[0,0,0,0]],"state":"CLOSED","status":"ENABLED"}}});
    (*helpers::parse_str(&text.to_string(), "pmd-json").unwrap()).clone()
}

#[test]
fn kron_reduces_switch_impedance_instead_of_truncating_it() {
    let net = switch_network();
    let reduced = powerio_dist::kron::neutral_kron_reduce(
        &net,
        &powerio_dist::kron::NeutralKronOptions::default(),
    )
    .unwrap();
    let raw = &reduced.network().switches()[0].extras["pmd_rs"];
    assert_eq!(raw.as_array().unwrap().len(), 3);
    close(raw[0][0].as_f64().unwrap(), 3.0);
    close(raw[0][1].as_f64().unwrap(), -1.0);
    let out = helpers::emit_value(&net, DistTargetFormat::PmdMathJson);
    let doc: Value = serde_json::from_str(&out.text).unwrap();
    assert_eq!(doc["is_kron_reduced"], true);
    let branch = doc["branch"]
        .as_object()
        .unwrap()
        .values()
        .find(|b| b["name"] == "_virtual_branch.switch.s")
        .unwrap();
    let base = doc["settings"]["sbase"].as_f64().unwrap();
    let vbase = doc["bus"][branch["f_bus"].as_i64().unwrap().to_string()]["vbase"]
        .as_f64()
        .unwrap();
    let scale = vbase * vbase / base * 1000.0;
    close(branch["br_r"][0][0].as_f64().unwrap() * scale, 3.0);
    close(branch["br_r"][0][1].as_f64().unwrap() * scale, -1.0);
}

#[test]
fn ravens_restores_ground_returns_after_kron_reduction() {
    let net = helpers::parse_dss_str(
        "clear\nnew circuit.c phases=3 basekv=7.2 bus1=b\nnew load.l phases=3 bus1=b.1.2.3.0 kv=7.2 kw=30 kvar=3\nnew load.one phases=1 bus1=b.1.0 kv=4.1569 kw=3 kvar=0.3\nnew generator.g phases=3 bus1=b.1.2.3.0 kv=7.2 kw=9 kvar=0\nnew transformer.t phases=3 windings=2 buses=(b.1.2.3.0,s.1.2.3.0) conns=(wye,wye) kvs=(7.2,0.4) kvas=(100,100) xhl=2\n",
    );
    let reduced = powerio_dist::kron::neutral_kron_reduce(
        &net,
        &powerio_dist::kron::NeutralKronOptions::default(),
    )
    .unwrap();
    let out = helpers::emit_value(reduced.network(), DistTargetFormat::RavensJson);
    let back = helpers::parse_str(&out.text, "ravens-json").unwrap();
    assert_eq!(back.loads().len(), 2, "{:?}", out.warnings);
    assert_eq!(back.generators().len(), 1, "{:?}", out.warnings);
    for bus in ["b", "s"] {
        assert!(
            back.bus(bus).unwrap().grounded.contains(&"4".to_string()),
            "{bus}: {:?}",
            back.bus(bus)
        );
    }
    for load in back.loads() {
        assert_eq!(load.terminal_map.last().unwrap(), "4");
    }
    assert_eq!(back.generators()[0].terminal_map.last().unwrap(), "4");
}

#[test]
fn pmd_engineering_keeps_zip_coefficients_on_nonzero_later_phases() {
    let mut net=helpers::parse_str(r#"{"data_model":"ENGINEERING","bus":{"b":{"terminals":[1,2,3,4],"grounded":[4]}},"settings":{"vbases_default":{"b":0.23}},"load":{"l":{"bus":"b","connections":[1,2,3,4],"configuration":"WYE","pd_nom":[0,10,20],"qd_nom":[0,4,8],"vm_nom":0.23}}}"#,"pmd-json").unwrap();
    net.loads_mut()[0].voltage_model = powerio_dist::DistLoadVoltageModel::Zip {
        v_nom: vec![230.0; 3],
        alpha_z: vec![0.0, 0.4, 0.6],
        alpha_i: vec![0.0, 0.2, 0.1],
        alpha_p: vec![0.0, 0.4, 0.3],
        beta_z: vec![0.0, 0.3, 0.5],
        beta_i: vec![0.0, 0.3, 0.2],
        beta_p: vec![0.0, 0.4, 0.3],
    };
    let math = helpers::emit_value(&net, DistTargetFormat::PmdMathJson);
    let back = helpers::parse_str(&math.text, "pmd-math-json").unwrap();
    let eng = helpers::emit_value(&back, DistTargetFormat::PmdJson);
    let back = helpers::parse_str(&eng.text, "pmd-json").unwrap();
    for (phase, z, i, p) in [("2", 0.4, 0.2, 0.4), ("3", 0.6, 0.1, 0.3)] {
        let load = back
            .loads()
            .iter()
            .find(|l| l.terminal_map.contains(&phase.to_string()))
            .unwrap();
        let powerio_dist::DistLoadVoltageModel::Zip {
            alpha_z,
            alpha_i,
            alpha_p,
            ..
        } = &load.voltage_model
        else {
            panic!("ZIP changed")
        };
        let at = load.terminal_map.iter().position(|t| t == phase).unwrap();
        close(alpha_z[at], z);
        close(alpha_i[at], i);
        close(alpha_p[at], p);
    }
}

#[test]
fn pmd_pair_voltage_bounds_survive_fresh_emission() {
    let net=helpers::parse_str(r#"{"data_model":"MATHEMATICAL","per_unit":true,"settings":{"sbase":1000,"power_scale_factor":1000,"voltage_scale_factor":1000},"bus":{"1":{"index":1,"bus_i":1,"name":"b","terminals":[1,2,3],"grounded":[false,false,false],"vbase":0.23,"vm_pair_lb":[[1,2,1.5]],"vm_pair_ub":[[1,2,2.0]]}}}"#,"pmd-math-json").unwrap();
    let out = helpers::emit_value(&net, DistTargetFormat::PmdMathJson);
    let doc: Value = serde_json::from_str(&out.text).unwrap();
    let bus = doc["bus"]
        .as_object()
        .unwrap()
        .values()
        .find(|b| b["name"] == "b")
        .unwrap();
    assert_eq!(bus["vm_pair_lb"][0][0], 1);
    assert_eq!(bus["vm_pair_lb"][0][1], 2);
    close(bus["vm_pair_lb"][0][2].as_f64().unwrap(), 1.5);
    close(bus["vm_pair_ub"][0][2].as_f64().unwrap(), 2.0);
    let back = helpers::parse_str(&out.text, "pmd-math-json").unwrap();
    assert_eq!(
        out.text,
        helpers::emit_value(&back, DistTargetFormat::PmdMathJson).text
    );
}

#[test]
fn split_phase_codes_keep_both_conductors_in_either_order() {
    assert_eq!(
        powerio_core::cim::expand_phase_code("PhaseCode.s21N"),
        ["s2", "s1", "N"]
    );
}

#[test]
fn ravens_switch_subclasses_keep_connectivity_and_class() {
    fn terminal(bus: &str) -> Value {
        json!({"Ravens.cimObjectType":"Terminal","Terminal.ConnectivityNode":format!("ConnectivityNode::'{bus}'"),"Terminal.phases":"PhaseCode.ABC"})
    }
    for kind in [
        "Breaker",
        "Recloser",
        "Fuse",
        "Disconnector",
        "Sectionaliser",
        "Jumper",
    ] {
        let input = json!({"ConnectivityNode":{"a":{"Ravens.cimObjectType":"ConnectivityNode"},"b":{"Ravens.cimObjectType":"ConnectivityNode"}},kind:{"s":{"Ravens.cimObjectType":kind,"Switch.open":false,"Equipment.inService":true,"ConductingEquipment.Terminals":[terminal("a"),terminal("b")]}}});
        let net = helpers::parse_str(&input.to_string(), "ravens-json").unwrap();
        assert_eq!(net.switches().len(), 1, "{kind}");
        let out = helpers::emit_value(&net, DistTargetFormat::RavensJson);
        if kind == "Disconnector" {
            assert!(
                out.warnings
                    .iter()
                    .any(|w| w.contains("FIELD_DROPPED") && w.contains("Disconnector")),
                "missing class-loss diagnostic"
            );
        } else {
            assert!(
                out.text
                    .contains(&format!("\"Ravens.cimObjectType\": \"{kind}\"")),
                "{kind} lost"
            );
        }
    }
}

#[test]
fn implicit_earth_return_does_not_ground_an_existing_floating_neutral() {
    let mut net = helpers::parse_str(r#"{"data_model":"ENGINEERING","bus":{"b":{"terminals":[1,2,3,4],"grounded":[]}},"settings":{"vbases_default":{"b":0.23}},"load":{"floating":{"bus":"b","connections":[1,2,3,4],"configuration":"WYE","pd_nom":[1,1,1],"qd_nom":[0,0,0],"vm_nom":0.23},"earth":{"bus":"b","connections":[1,2,3,4],"configuration":"WYE","pd_nom":[2,2,2],"qd_nom":[0,0,0],"vm_nom":0.23}}}"#, "pmd-json").unwrap();
    net.loads_mut()
        .iter_mut()
        .find(|l| l.name == "earth")
        .unwrap()
        .terminal_map
        .pop();
    assert_eq!(net.bus("b").unwrap().grounded, Vec::<String>::new());
    let out = helpers::emit_value(&net, DistTargetFormat::RavensJson);
    let back = helpers::parse_str(&out.text, "ravens-json").unwrap();
    assert_eq!(back.bus("b").unwrap().grounded, Vec::<String>::new());
    assert_eq!(
        back.loads()
            .iter()
            .find(|l| l.name == "floating")
            .unwrap()
            .bus,
        "b"
    );
    let earth = back.loads().iter().find(|l| l.name == "earth").unwrap();
    assert_ne!(earth.bus, "b");
    assert!(
        back.bus(&earth.bus)
            .unwrap()
            .grounded
            .contains(earth.terminal_map.last().unwrap())
    );
    assert_eq!(back.switches().len(), 1);
    assert!(!back.switches()[0].open);
    assert!(!back.switches()[0].terminal_map_to.contains(&"4".into()));
    assert_eq!(
        out.text,
        helpers::emit_value(&back, DistTargetFormat::RavensJson).text
    );
    let contracted = source_equivalents::contract_source_pairs(&back);
    assert_eq!(contracted.buses().len(), 1);
    for mutate in [0, 1, 2] {
        let mut changed = (*back).clone();
        match mutate {
            0 => changed.switches_mut()[0].open = true,
            1 => changed.switches_mut()[0].i_max = Some(vec![10.0; 3]),
            _ => {
                changed.switches_mut()[0]
                    .extras
                    .insert("pmd_rs".into(), json!([[1, 0, 0], [0, 1, 0], [0, 0, 1]]));
            }
        }
        assert_eq!(
            source_equivalents::contract_source_pairs(&changed)
                .buses()
                .len(),
            2
        );
    }
}

fn pair_network(per_unit: bool, neutral_pair: bool) -> MulticonductorNetwork {
    let doc = json!({"data_model":"MATHEMATICAL","per_unit":per_unit,"settings":{"sbase":1000,"power_scale_factor":1000,"voltage_scale_factor":1000},"bus":{"1":{"index":1,"bus_i":1,"name":"b","terminals":[1,2,3,4],"grounded":[false,false,false,true],"vbase":0.23,"vm_pair_lb":[[1,if neutral_pair {4} else {2},if per_unit {1.5} else {0.345}]],"vm_pair_ub":[[1,2,null]]}}});
    (*helpers::parse_str(&doc.to_string(), "pmd-math-json").unwrap()).clone()
}

#[test]
fn pair_bounds_rebase_only_values_and_keep_neutral_constraints() {
    for per_unit in [true, false] {
        let mut net = pair_network(per_unit, true);
        // Change both authoritative base seeds: retained bounds must stay 345 V.
        net.buses_mut()[0]
            .extras
            .insert("pmd_vbase".into(), json!(0.46));
        let mut settings = net.extras()["pmd_settings"].clone();
        settings["vbases_default"] = json!({"b":0.46});
        net.extras_mut().insert("pmd_settings".into(), settings);
        let out = helpers::emit_value(&net, DistTargetFormat::PmdMathJson);
        let doc: Value = serde_json::from_str(&out.text).unwrap();
        assert_eq!(doc["is_kron_reduced"], false);
        let bus = doc["bus"]
            .as_object()
            .unwrap()
            .values()
            .find(|b| b["name"] == "b")
            .unwrap();
        assert_eq!(bus["vm_pair_lb"][0][0], 1);
        assert_eq!(bus["vm_pair_lb"][0][1], 4);
        close(
            bus["vm_pair_lb"][0][2].as_f64().unwrap() * bus["vbase"].as_f64().unwrap() * 1000.0,
            345.0,
        );
        assert!(bus["vm_pair_ub"][0][2].is_null());
        let back = helpers::parse_str(&out.text, "pmd-math-json").unwrap();
        assert_eq!(
            out.text,
            helpers::emit_value(&back, DistTargetFormat::PmdMathJson).text
        );
        for target in [DistTargetFormat::PmdJson, DistTargetFormat::RavensJson] {
            assert!(
                helpers::emit_value(&net, target)
                    .warnings
                    .iter()
                    .any(|w| w.contains("FIELD_DROPPED") && w.contains("pair"))
            );
        }
    }
}

#[test]
fn invalid_retained_pair_constraints_are_refused() {
    for invalid in [json!([[1, 9, 1.0]]), json!([[1, 2]]), json!("bad")] {
        let mut net = pair_network(true, false);
        net.buses_mut()[0]
            .extras
            .insert("pmd_vm_pair_lb".into(), invalid);
        let result = powerio_dist::emit(
            &powerio_core::PioModule::new(net),
            DistTargetFormat::PmdMathJson,
            powerio_core::Destination::memory("out").unwrap(),
        );
        assert!(result.is_err());
    }
}

#[test]
fn phase_zip_parts_keep_delta_pairs_reactive_laws_and_cutoff() {
    let mut net=helpers::parse_str(r#"{"data_model":"ENGINEERING","bus":{"b":{"terminals":[1,2,3],"grounded":[]}},"settings":{"vbases_default":{"b":0.23}},"load":{"l":{"bus":"b","connections":[1,2,3],"configuration":"DELTA","pd_nom":[1,2,3],"qd_nom":[3,2,1],"vm_nom":0.4},"l_phase1":{"bus":"b","connections":[1,2],"configuration":"DELTA","pd_nom":[1],"qd_nom":[0],"vm_nom":0.4}}}"#,"pmd-json").unwrap();
    let load = net.loads_mut().iter_mut().find(|l| l.name == "l").unwrap();
    load.voltage_model = powerio_dist::DistLoadVoltageModel::Zip {
        v_nom: vec![400.0; 3],
        alpha_z: vec![0.4; 3],
        alpha_i: vec![0.2; 3],
        alpha_p: vec![0.4; 3],
        beta_z: vec![0.3, 0.5, 0.7],
        beta_i: vec![0.3, 0.2, 0.1],
        beta_p: vec![0.4, 0.3, 0.2],
    };
    load.extras
        .insert("zipv".into(), json!([1, 0, 0, 1, 0, 0, 0.8]));
    let out = helpers::emit_value(&net, DistTargetFormat::PmdJson);
    let doc: Value = serde_json::from_str(&out.text).unwrap();
    assert_eq!(doc["load"].as_object().unwrap().len(), 4);
    assert_eq!(doc["load"]["l_phase1_1"]["connections"], json!([1, 2]));
    assert_eq!(doc["load"]["l_phase2"]["connections"], json!([2, 3]));
    assert_eq!(doc["load"]["l_phase3"]["connections"], json!([3, 1]));
    for (key, beta) in [("l_phase1_1", 0.3), ("l_phase2", 0.5), ("l_phase3", 0.7)] {
        assert_eq!(doc["load"][key]["configuration"], "DELTA");
        close(doc["load"][key]["zipv"][0].as_f64().unwrap(), 0.4);
        close(doc["load"][key]["zipv"][3].as_f64().unwrap(), beta);
        close(doc["load"][key]["zipv"][6].as_f64().unwrap(), 0.8);
    }
}

#[test]
fn switch_kron_uses_the_original_shared_linecode_and_keeps_limits() {
    let doc = json!({"data_model":"ENGINEERING","settings":{"vbases_default":{"a":0.23}},"bus":{"a":{"terminals":[1,2,3,4],"grounded":[4]},"b":{"terminals":[1,2,3,4],"grounded":[4]}},"linecode":{"z":{"rs":[[4,0,0,2],[0,4,0,2],[0,0,4,2],[2,2,2,4]],"xs":[[2,0,0,1],[0,2,0,1],[0,0,2,1],[1,1,1,2]],"cm_ub":[10,20,30,null]}},"line":{"l":{"f_bus":"a","t_bus":"b","f_connections":[1,2,3,4],"t_connections":[1,2,3,4],"linecode":"z","length":1}},"switch":{"s":{"f_bus":"a","t_bus":"b","f_connections":[1,2,3,4],"t_connections":[1,2,3,4],"linecode":"z","state":"CLOSED","status":"ENABLED"}}});
    let net = helpers::parse_str(&doc.to_string(), "pmd-json").unwrap();
    let reduced = powerio_dist::kron::neutral_kron_reduce(
        &net,
        &powerio_dist::kron::NeutralKronOptions::default(),
    )
    .unwrap();
    let s = &reduced.network().switches()[0];
    close(s.extras["pmd_rs"][0][0].as_f64().unwrap(), 3.0);
    close(s.extras["pmd_rs"][0][1].as_f64().unwrap(), -1.0);
    close(s.extras["pmd_xs"][0][0].as_f64().unwrap(), 1.5);
    assert_eq!(s.i_max, Some(vec![10.0, 20.0, 30.0]));
    let mut ideal_neutral = switch_network();
    ideal_neutral.switches_mut()[0].extras.insert(
        "pmd_rs".into(),
        json!([[4, 0, 0, 0], [0, 4, 0, 0], [0, 0, 4, 0], [0, 0, 0, 0]]),
    );
    let reduced = powerio_dist::kron::neutral_kron_reduce(
        &ideal_neutral,
        &powerio_dist::kron::NeutralKronOptions::default(),
    )
    .unwrap();
    close(
        reduced.network().switches()[0].extras["pmd_rs"][0][0]
            .as_f64()
            .unwrap(),
        4.0,
    );
}

#[test]
fn zero_pmd_switch_matrices_keep_an_ideal_switch_ideal_in_dss() {
    // A PMD parse of an ideal switch carries six zero matrices. OpenDSS
    // cannot invert a Line with r = x = 0 (error 183).
    let zero = json!([[0, 0, 0], [0, 0, 0], [0, 0, 0]]);
    let doc = json!({"data_model":"ENGINEERING","settings":{"vbases_default":{"a":0.23}},"bus":{"a":{"terminals":[1,2,3],"grounded":[]},"b":{"terminals":[1,2,3],"grounded":[]}},"switch":{"s":{"f_bus":"a","t_bus":"b","f_connections":[1,2,3],"t_connections":[1,2,3],"state":"CLOSED","status":"ENABLED","cm_ub":[100.0,100.0,100.0],"rs":zero,"xs":zero,"g_fr":zero,"g_to":zero,"b_fr":zero,"b_to":zero,"length":0.001}}});
    let net = helpers::parse_str(&doc.to_string(), "pmd-json").unwrap();
    let dss = helpers::emit_value(&net, DistTargetFormat::Dss);
    assert!(
        dss.warnings
            .iter()
            .any(|w| w.contains("IDEAL_SWITCH_EQUIVALENT")),
        "{:?}",
        dss.warnings
    );
    let line = dss
        .text
        .lines()
        .find(|line| line.starts_with("New Line.s "))
        .unwrap();
    assert!(line.contains("r1=1 "), "{line}");
}

#[test]
fn identity_metadata_keeps_an_ideal_switch_ideal_in_other_formats() {
    let input = json!({"ConnectivityNode":{"a":{"Ravens.cimObjectType":"ConnectivityNode"},"b":{"Ravens.cimObjectType":"ConnectivityNode"}},"Switch":{"s":{"Ravens.cimObjectType":"Switch","Switch.open":false,"ConductingEquipment.Terminals":[{"Ravens.cimObjectType":"Terminal","Terminal.ConnectivityNode":"ConnectivityNode::'a'","Terminal.phases":"PhaseCode.ABC"},{"Ravens.cimObjectType":"Terminal","Terminal.ConnectivityNode":"ConnectivityNode::'b'","Terminal.phases":"PhaseCode.ABC"}]}}});
    let net = helpers::parse_str(&input.to_string(), "ravens-json").unwrap();
    let eng = helpers::emit_value(&net, DistTargetFormat::PmdJson);
    let doc: Value = serde_json::from_str(&eng.text).unwrap();
    assert!(
        doc["switch"]["s"]["rs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row
                .as_array()
                .unwrap()
                .iter()
                .all(|v| v.as_f64() == Some(0.0)))
    );
    let dss = helpers::emit_value(&net, DistTargetFormat::Dss);
    assert!(
        dss.warnings
            .iter()
            .any(|w| w.contains("IDEAL_SWITCH_EQUIVALENT"))
    );
    let back = helpers::parse_dss_str(&dss.text);
    let recovered = source_equivalents::contract_source_pairs(&back);
    assert_eq!(recovered.switches().len(), 1);
    assert_eq!(recovered.lines().len(), 0);
    assert_eq!(recovered.buses().len(), 2);
    let mut corrupted = back.clone();
    corrupted.line_codes_mut()[0].r_series[0][0] += 0.1;
    assert_eq!(
        source_equivalents::contract_source_pairs(&corrupted)
            .lines()
            .len(),
        2
    );
}

#[test]
fn source_fixed_phase_bounds_are_redundant_but_other_bounds_are_reported() {
    let mut net = helpers::parse_dss_str("New Circuit.c basekv=12.47 bus1=b");
    let source = net.sources()[0].clone();
    let bus = net
        .buses_mut()
        .iter_mut()
        .find(|b| b.id == source.bus)
        .unwrap();
    let mut magnitudes = source.v_magnitude.clone();
    magnitudes[1] = f64::from_bits(magnitudes[1].to_bits() + 1);
    bus.v_min_phase = Some(magnitudes.clone());
    bus.v_max_phase = Some(magnitudes.clone());
    net.sources_mut()[0].v_magnitude = magnitudes;
    let out = helpers::emit_value(&net, DistTargetFormat::RavensJson);
    assert!(
        !out.warnings
            .iter()
            .any(|w| w.contains("per-phase voltage bound"))
    );
    net.sources_mut()[0].v_magnitude[1] *= 0.99;
    let out = helpers::emit_value(&net, DistTargetFormat::RavensJson);
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("per-phase voltage bound"))
    );
}
