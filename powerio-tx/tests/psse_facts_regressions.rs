mod helpers;
use powerio_tx::{BusId, StaticVarCompensator, TargetFormat};
use serde_json::{Value, json};
fn source() -> Value {
    json!({"network":{
     "caseid":{"fields":["rev","sbase","basfrq","title1"],"data":[35,100,60,"review"]},
     "bus":{"fields":["ibus","name","baskv","ide","vm","va"],"data":[[1,"HV",230,3,1,0],[2,"LV",115,1,1,0]]},
     "load":{"fields":["ibus","loadid","pl","ql","stat"],"data":[[2,"L1",0,0,1]]},
     "facts":{"fields":["name","ibus","jbus","mode","vset","shmx","fcreg","nreg"],"data":[["F1",1,0,1,1.02,100,2,1]]},
     "sub":{"fields":["isub","name","lati","long","srg"],"data":[[1,"SUB",0,0,0]]},
     "subnode":{"fields":["isub","inode","name","ibus","stat","vm","va"],"data":[[1,1,"REMOTE",2,1,1,0],[1,2,"LOCAL",1,1,1,0]]},
     "subterm":{"fields":["isub","inode","type","eqid","ibus","jbus","kbus"],"data":[[1,1,"L","L1",2,0,0],[1,2,"A","F1",1,0,0]]}
    }})
}
fn parse(s: &Value) -> helpers::Parsed {
    helpers::parse_str(&s.to_string(), "rawx").unwrap()
}
fn field(root: &Value, table: &str, row: usize, key: &str) -> Value {
    let t = &root["network"][table];
    let at = t["fields"]
        .as_array()
        .unwrap()
        .iter()
        .position(|v| v == key)
        .unwrap();
    t["data"][row][at].clone()
}
#[test]
fn remote_vset_is_on_the_regulated_voltage_base_on_read() {
    let raw = "0,100,35,0,1,60 / synthetic
CASE
COMMENT
0 / END OF SYSTEM-WIDE DATA, BEGIN BUS DATA
1,'HV',230,3,1,1,1,1,0
2,'LV',115,1,1,1,1,1,0
0 / END OF BUS DATA, BEGIN LOAD DATA
2,'L1',1,1,1,0,0
0 / END OF LOAD DATA, BEGIN FACTS DEVICE DATA
'F1',1,0,1,0,0,1.02,100,9999,0.9,1.1,1,0,0.05,100,1,0,0,0,2,1,''
0 / END OF FACTS DEVICE DATA, BEGIN SUBSTATION DATA
1,'SUB',0,0,0
1,'REMOTE',2,1,1,0
0 / END OF SUBSTATION NODE DATA
0 / END OF SUBSTATION SWITCHING DEVICE DATA
2,1,'L','L1'
0 / END OF SUBSTATION EQUIPMENT TERMINAL DATA
0 / END OF SUBSTATION DATA
Q
";
    let p = helpers::parse_str(raw, "psse").unwrap();
    let svc = &p.network.static_var_compensators()[0];
    assert!(svc.regulating_terminal.is_some());
    assert!(
        (svc.voltage_setpoint_kv - 117.3).abs() < 1e-10,
        "remote 115 kV setpoint must be 1.02*115, got {}",
        svc.voltage_setpoint_kv
    );
}
#[test]
fn remote_vset_is_on_the_regulated_voltage_base_on_write() {
    let mut p = parse(&source());
    let t = &p
        .network
        .detailed_connectivity()
        .as_ref()
        .unwrap()
        .terminals[0];
    let reference = serde_json::from_value::<powerio_tx::TerminalReference>(
        json!({"equipment":t.equipment,"terminal":t.terminal}),
    )
    .unwrap();
    p.network.static_var_compensators_mut()[0].regulating_terminal = Some(reference);
    p.network.static_var_compensators_mut()[0].voltage_setpoint_kv = 117.3;
    let out = helpers::emit_value(&p.network, TargetFormat::PsseRawx).unwrap();
    let root: Value = serde_json::from_str(&out.text).unwrap();
    assert_eq!(field(&root, "facts", 0, "fcreg"), json!(2));
    assert!(
        (field(&root, "facts", 0, "vset").as_f64().unwrap() - 1.02).abs() < 1e-10,
        "writer uses local rather than regulated bus base: {}",
        field(&root, "facts", 0, "vset")
    );
}
#[test]
fn facts_terminal_is_attached_to_the_typed_compensator() {
    let p = parse(&source());
    let svc = &p.network.static_var_compensators()[0];
    let detailed = p.network.detailed_connectivity().as_ref().unwrap();
    assert!(
        detailed
            .terminals
            .iter()
            .any(|t| t.equipment.component_type() == "static_var_compensator"
                && Some(t.equipment.local_id()) == svc.uid.as_deref()),
        "FACTS terminal remains an unrelated generic equipment identity"
    );
}
#[test]
fn generated_names_do_not_collide_with_retained_facts_names() {
    let mut s = source();
    s["network"]["facts"]["data"][0][0] = json!("SVC2");
    s["network"].as_object_mut().unwrap().remove("subterm");
    let mut p = parse(&s);
    let mut svc = StaticVarCompensator::new(BusId(2), -0.001, 0.001);
    svc.regulating = true;
    svc.voltage_setpoint_kv = 115.0;
    p.network.static_var_compensators_mut().push(svc);
    let out = helpers::emit_value(&p.network, TargetFormat::PsseRawx).unwrap();
    let root: Value = serde_json::from_str(&out.text).unwrap();
    let a = field(&root, "facts", 0, "name");
    let b = field(&root, "facts", 1, "name");
    assert_ne!(a, b, "writer emitted duplicate FACTS names");
}
#[test]
fn current_limit_projection_has_a_runtime_fidelity_diagnostic() {
    let mut s = source();
    s["network"]["facts"]["data"][0][6] = json!(0);
    s["network"]["facts"]["data"][0][7] = json!(0);
    for key in ["sub", "subnode", "subterm"] {
        s["network"].as_object_mut().unwrap().remove(key);
    }
    let p = parse(&s);
    let svc = &p.network.static_var_compensators()[0];
    let bound = svc.b_max_siemens * (230.0_f64 * 0.95).powi(2);
    assert!((bound - 90.25).abs() < 1e-10);
    assert!(
        p.render_diagnostics()
            .iter()
            .any(|w| w.to_ascii_lowercase().contains("current")
                && w.to_ascii_lowercase().contains("susceptance")),
        "STATCOM current limit projected to fixed B without a fidelity diagnostic"
    );
}
#[test]
fn local_facts_limits_status_and_rawx_roundtrip_control() {
    let mut s = source();
    s["network"]["facts"]["data"][0][6] = json!(0);
    s["network"]["facts"]["data"][0][7] = json!(0);
    s["network"]["facts"]["data"][0][3] = json!(0);
    for key in ["sub", "subnode", "subterm"] {
        s["network"].as_object_mut().unwrap().remove(key);
    }
    let p = parse(&s);
    let svc = &p.network.static_var_compensators()[0];
    assert!(!svc.in_service);
    assert!((svc.voltage_setpoint_kv - 234.6).abs() < 1e-10);
    assert!((svc.b_max_siemens - 100.0 / 230.0_f64.powi(2)).abs() < 1e-15);
    let out = helpers::emit_value(&p.network, TargetFormat::PsseRawx).unwrap();
    let back = helpers::parse_str(&out.text, "rawx").unwrap();
    assert_eq!(
        p.network.static_var_compensators(),
        back.network.static_var_compensators()
    );
}

#[test]
fn rawx_facts_nreg_is_resolved_and_roundtrips() {
    let mut s = source();
    s["network"]["subterm"]["data"]
        .as_array_mut()
        .unwrap()
        .truncate(1);
    let p = parse(&s);
    let svc = &p.network.static_var_compensators()[0];
    let out = helpers::emit_value(&p.network, TargetFormat::PsseRawx).unwrap();
    let root: Value = serde_json::from_str(&out.text).unwrap();
    assert!(
        svc.regulating_terminal.is_some(),
        "valid FACTS NREG ignored without a diagnostic"
    );
    assert_eq!(field(&root, "facts", 0, "nreg"), json!(1));
}

#[test]
fn remote_regulation_survives_raw_revisions_and_serde() {
    let parsed = parse(&source());
    let network = helpers::serde_round_trip(&parsed.network);
    for revision in [33, 34, 35] {
        let out = helpers::emit_psse_rev(&network, revision);
        let back = helpers::parse_str(&out.text, "psse").unwrap();
        let svc = &back.network.static_var_compensators()[0];
        assert!((svc.voltage_setpoint_kv - 117.3).abs() < 1e-10);
        assert!((svc.b_max_siemens - 100.0 / 230.0_f64.powi(2)).abs() < 1e-15);
        assert_eq!(svc.regulating_terminal.is_some(), revision >= 34);
        if revision == 33 {
            assert!(
                out.render_diagnostics()
                    .iter()
                    .any(|line| line.contains("NREG"))
            );
        } else {
            let rawx = helpers::emit_value(&back.network, TargetFormat::PsseRawx).unwrap();
            let root: Value = serde_json::from_str(&rawx.text).unwrap();
            assert_eq!(field(&root, "facts", 0, "nreg"), json!(1));
            assert_eq!(field(&root, "facts", 0, "fcreg"), json!(2));
        }
    }
}

#[test]
fn missing_regulating_nodes_are_diagnosed_in_raw_and_rawx() {
    let mut source = source();
    source["network"]["facts"]["data"][0][7] = json!(99);
    let parsed = parse(&source);
    assert!(
        parsed.network.static_var_compensators()[0]
            .regulating_terminal
            .is_none()
    );
    assert!(
        parsed
            .render_diagnostics()
            .iter()
            .any(|line| line.contains("REFERENCE_DROPPED") && line.contains("99"))
    );
    let raw = helpers::emit_psse_rev(&parse(&self::source()).network, 35).text;
    let raw = raw.replace(", 2, 1, ''", ", 2, 99, ''");
    let back = helpers::parse_str(&raw, "psse").unwrap();
    assert!(
        back.network.static_var_compensators()[0]
            .regulating_terminal
            .is_none()
    );
    assert!(
        back.render_diagnostics()
            .iter()
            .any(|line| line.contains("REFERENCE_DROPPED") && line.contains("99"))
    );
}

#[test]
fn series_facts_do_not_shift_shunt_terminal_or_regulation_mapping() {
    let mut source = source();
    source["network"]["facts"]["data"]
        .as_array_mut()
        .unwrap()
        .insert(0, json!(["SERIES", 1, 2, 1, 1.0, 50, 0, 0]));
    let parsed = parse(&source);
    assert_eq!(parsed.network.static_var_compensators().len(), 1);
    let svc = &parsed.network.static_var_compensators()[0];
    assert!(svc.regulating_terminal.is_some());
    assert_eq!(svc.extras["psse_name"], "F1");
    assert!(
        parsed
            .network
            .detailed_connectivity()
            .as_ref()
            .unwrap()
            .terminals
            .iter()
            .any(|terminal| terminal.equipment.component_type() == "static_var_compensator")
    );
    assert!(
        parsed
            .render_diagnostics()
            .iter()
            .any(|line| line.contains("series device"))
    );
}

#[test]
fn retained_names_are_reserved_before_any_generated_name() {
    let mut network = parse(&source()).network;
    network.static_var_compensators_mut()[0]
        .extras
        .remove("psse_name");
    let mut retained = StaticVarCompensator::new(BusId(2), -0.001, 0.001);
    retained.voltage_setpoint_kv = 115.0;
    retained.extras.insert("psse_name".into(), json!("SVC1"));
    network.static_var_compensators_mut().push(retained);
    let emitted = helpers::emit_value(&network, TargetFormat::PsseRawx).unwrap();
    let root: Value = serde_json::from_str(&emitted.text).unwrap();
    assert_ne!(field(&root, "facts", 0, "name"), json!("SVC1"));
    assert_eq!(field(&root, "facts", 1, "name"), json!("SVC1"));
}

#[test]
fn sanitized_collisions_keep_detailed_terminals_bound_to_unique_devices() {
    let mut source = source();
    source["network"]["facts"]["data"]
        .as_array_mut()
        .unwrap()
        .push(json!(["F2", 1, 0, 1, 1.0, 50, 0, 0]));
    source["network"]["subterm"]["data"]
        .as_array_mut()
        .unwrap()
        .push(json!([1, 2, "A", "F2", 1, 0, 0]));
    let mut network = parse(&source).network;
    network.static_var_compensators_mut()[0]
        .extras
        .insert("psse_name".into(), json!("D/EV"));
    network.static_var_compensators_mut()[1]
        .extras
        .insert("psse_name".into(), json!("D'EV"));
    for target in [
        TargetFormat::Psse { rev: 34 },
        TargetFormat::Psse { rev: 35 },
        TargetFormat::PsseRawx,
    ] {
        let out = helpers::emit_value(&network, target).unwrap();
        let format = if matches!(target, TargetFormat::PsseRawx) {
            "rawx"
        } else {
            "psse"
        };
        let back = helpers::parse_str(&out.text, format).unwrap();
        let svcs = back.network.static_var_compensators();
        assert_ne!(svcs[0].extras["psse_name"], svcs[1].extras["psse_name"]);
        let detailed = back.network.detailed_connectivity().as_ref().unwrap();
        for svc in svcs {
            assert_eq!(
                detailed
                    .terminals
                    .iter()
                    .filter(|terminal| terminal.equipment.component_type()
                        == "static_var_compensator"
                        && Some(terminal.equipment.local_id()) == svc.uid.as_deref())
                    .count(),
                1
            );
        }
        assert!(
            out.render_diagnostics()
                .iter()
                .any(|line| line.contains("collided"))
        );
    }
}

#[test]
fn current_and_susceptance_limits_match_only_at_unity_with_diagnostics_both_ways() {
    let parsed = parse(&source());
    let svc = &parsed.network.static_var_compensators()[0];
    for voltage in [0.95_f64, 1.0, 1.05] {
        let fixed_b_limit = svc.b_max_siemens * (230.0 * voltage).powi(2);
        let current_limit = 100.0 * voltage;
        assert!((fixed_b_limit / current_limit - voltage).abs() < 1e-12);
    }
    assert!(
        parsed
            .render_diagnostics()
            .iter()
            .any(|line| line.contains("current limit") && line.contains("susceptance"))
    );
    for target in [TargetFormat::Psse { rev: 33 }, TargetFormat::PsseRawx] {
        let emitted = helpers::emit_value(&parsed.network, target).unwrap();
        assert!(
            emitted
                .render_diagnostics()
                .iter()
                .any(|line| line.contains("current limit") && line.contains("susceptance"))
        );
    }
    let mut zero = source();
    zero["network"]["facts"]["data"][0][5] = json!(0);
    let parsed = parse(&zero);
    assert!(
        parsed.network.static_var_compensators()[0]
            .b_max_siemens
            .abs()
            < f64::EPSILON
    );
    assert!(
        !parsed
            .render_diagnostics()
            .iter()
            .any(|line| line.contains("current limit"))
    );
}

#[test]
fn retained_rawx_bytes_stay_exact_despite_typed_projection_diagnostics() {
    let source = serde_json::to_string_pretty(&source()).unwrap();
    let parsed = helpers::parse_str(&source, "rawx").unwrap();
    let emitted = helpers::emit_module(&parsed.module, TargetFormat::PsseRawx).unwrap();
    assert_eq!(emitted.text, source);
}
