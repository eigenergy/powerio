use powerio_dist::{
    DistLoadVoltageModel, parse_bmopf_str, parse_dss_str, parse_pmd_str, write_bmopf_json,
    write_dss, write_pmd_json,
};

fn load_model<'a>(net: &'a powerio_dist::DistNetwork, name: &str) -> &'a DistLoadVoltageModel {
    &net.loads
        .iter()
        .find(|l| l.name.eq_ignore_ascii_case(name))
        .unwrap_or_else(|| panic!("missing load {name}"))
        .voltage_model
}

#[test]
fn rich_bmopf_load_voltage_models_preserve_all_variants() {
    let text = r#"{
        "bus": {
            "b1": {"terminal_names": ["1", "2", "3", "4"], "perfectly_grounded_terminals": ["4"]}
        },
        "voltage_source": {
            "source": {
                "bus": "b1", "terminal_map": ["1", "2", "3", "4"],
                "v_magnitude": [7200.0, 7200.0, 7200.0, 0.0],
                "v_angle": [0.0, -120.0, 120.0, 0.0]
            }
        },
        "load": {
            "cp": {
                "bus": "b1", "terminal_map": ["1", "2", "3", "4"],
                "configuration": "WYE", "p_nom": [1.0, 1.0, 1.0], "q_nom": [0.1, 0.1, 0.1],
                "model": "constant_power", "v_nom": [7200.0, 7200.0, 7200.0]
            },
            "ci": {
                "bus": "b1", "terminal_map": ["1", "2", "3", "4"],
                "configuration": "WYE", "p_nom": [1.0, 1.0, 1.0], "q_nom": [0.1, 0.1, 0.1],
                "model": "constant_current", "v_nom": [7200.0, 7200.0, 7200.0]
            },
            "cz": {
                "bus": "b1", "terminal_map": ["1", "2", "3", "4"],
                "configuration": "WYE", "p_nom": [1.0, 1.0, 1.0], "q_nom": [0.1, 0.1, 0.1],
                "model": "constant_impedance", "v_nom": [7200.0, 7200.0, 7200.0]
            },
            "zip": {
                "bus": "b1", "terminal_map": ["1", "2", "3", "4"],
                "configuration": "WYE", "p_nom": [1.0, 2.0, 3.0], "q_nom": [0.1, 0.2, 0.3],
                "model": "zip", "v_nom": [7200.0, 7200.0, 7200.0],
                "alpha_z": [0.2, 0.2, 0.2], "alpha_i": [0.3, 0.3, 0.3], "alpha_p": [0.5, 0.5, 0.5],
                "beta_z": [0.1, 0.1, 0.1], "beta_i": [0.4, 0.4, 0.4], "beta_p": [0.5, 0.5, 0.5]
            },
            "exp": {
                "bus": "b1", "terminal_map": ["1", "2", "3", "4"],
                "configuration": "WYE", "p_nom": [1.0, 1.0, 1.0], "q_nom": [0.0, 0.0, 0.0],
                "model": "exponential", "v_nom": [7200.0, 7200.0, 7200.0],
                "gamma_p": [1.2, 1.3, 1.4], "gamma_q": [2.1, 2.2, 2.3]
            }
        }
    }"#;
    let net = parse_bmopf_str(text).unwrap();
    assert!(matches!(
        load_model(&net, "cp"),
        DistLoadVoltageModel::ConstantPower
    ));
    assert!(matches!(
        load_model(&net, "ci"),
        DistLoadVoltageModel::ConstantCurrent { v_nom } if v_nom == &vec![7200.0, 7200.0, 7200.0]
    ));
    assert!(matches!(
        load_model(&net, "cz"),
        DistLoadVoltageModel::ConstantImpedance { v_nom } if v_nom == &vec![7200.0, 7200.0, 7200.0]
    ));
    assert!(matches!(
        load_model(&net, "zip"),
        DistLoadVoltageModel::Zip { alpha_i, beta_z, .. }
            if alpha_i == &vec![0.3, 0.3, 0.3] && beta_z == &vec![0.1, 0.1, 0.1]
    ));
    assert!(matches!(
        load_model(&net, "exp"),
        DistLoadVoltageModel::Exponential { gamma_p, gamma_q, .. }
            if gamma_p == &vec![1.2, 1.3, 1.4] && gamma_q == &vec![2.1, 2.2, 2.3]
    ));

    let out = write_bmopf_json(&net);
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    let back = parse_bmopf_str(&out.text).unwrap();
    assert_eq!(net.loads, back.loads);
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    assert_eq!(
        doc["load"]["exp"]["gamma_p"],
        serde_json::json!([1.2, 1.3, 1.4])
    );
    assert_eq!(
        doc["load"]["zip"]["alpha_p"],
        serde_json::json!([0.5, 0.5, 0.5])
    );
}

#[test]
fn rich_pmd_load_voltage_models_keep_model_and_nominal_voltage() {
    let text = r#"{
        "data_model": "ENGINEERING",
        "bus": {"b1": {"terminals": [1, 2, 3, 4], "grounded": [4], "status": "ENABLED"}},
        "load": {
            "ci": {
                "bus": "b1", "connections": [1, 2, 3, 4], "configuration": "WYE",
                "pd_nom": [1.0, 1.0, 1.0], "qd_nom": [0.1, 0.1, 0.1],
                "model": "CURRENT", "vm_nom": [7.2, 7.2, 7.2], "status": "ENABLED"
            },
            "cz": {
                "bus": "b1", "connections": [1, 2, 3, 4], "configuration": "WYE",
                "pd_nom": [1.0, 1.0, 1.0], "qd_nom": [0.1, 0.1, 0.1],
                "model": "IMPEDANCE", "vm_nom": [7.2, 7.2, 7.2], "status": "ENABLED"
            },
            "zip": {
                "bus": "b1", "connections": [1, 2, 3, 4], "configuration": "WYE",
                "pd_nom": [1.0, 1.0, 1.0], "qd_nom": [0.1, 0.1, 0.1],
                "model": "ZIPV", "vm_nom": [7.2, 7.2, 7.2], "status": "ENABLED"
            }
        }
    }"#;
    let net = parse_pmd_str(text).unwrap();
    assert!(matches!(
        load_model(&net, "ci"),
        DistLoadVoltageModel::ConstantCurrent { v_nom } if v_nom == &vec![7.2, 7.2, 7.2]
    ));
    assert!(matches!(
        load_model(&net, "cz"),
        DistLoadVoltageModel::ConstantImpedance { v_nom } if v_nom == &vec![7.2, 7.2, 7.2]
    ));
    assert!(matches!(
        load_model(&net, "zip"),
        DistLoadVoltageModel::Zip { v_nom, .. } if v_nom == &vec![7.2, 7.2, 7.2]
    ));

    let out = write_pmd_json(&net);
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    let back = parse_pmd_str(&out.text).unwrap();
    assert_eq!(load_model(&back, "ci"), load_model(&net, "ci"));
    assert_eq!(load_model(&back, "cz"), load_model(&net, "cz"));
    assert_eq!(load_model(&back, "zip"), load_model(&net, "zip"));
}

#[test]
fn rich_opendss_load_models_and_switches_round_trip() {
    let dss = "New Circuit.rich basekv=12.47\n\
        New LineCode.lc nphases=3 r1=0.1 x1=0.2 c1=3.0 units=km normamps=400\n\
        New Line.sw bus1=sourcebus.1.2.3 bus2=b2.1.2.3 phases=3 switch=y\n\
        New SwtControl.swctrl switchedobj=Line.sw action=open\n\
        New Load.ci bus1=b2.1.2.3 phases=3 conn=wye kv=12.47 kw=90 kvar=45 model=5\n\
        New Load.cz bus1=b2.1.2.3 phases=3 conn=wye kv=12.47 kw=90 kvar=45 model=2\n\
        New Load.zip bus1=b2.1.2.3 phases=3 conn=wye kv=12.47 kw=90 kvar=45 model=8 zipv=[0.2,0.3,0.5,0.1,0.4,0.5,0.8]\n";
    let net = parse_dss_str(dss);
    assert_eq!(net.switches.len(), 1);
    assert!(net.switches[0].open);
    assert!(matches!(
        load_model(&net, "ci"),
        DistLoadVoltageModel::ConstantCurrent { v_nom } if v_nom == &vec![12_470.0, 12_470.0, 12_470.0]
    ));
    assert!(matches!(
        load_model(&net, "cz"),
        DistLoadVoltageModel::ConstantImpedance { v_nom } if v_nom == &vec![12_470.0, 12_470.0, 12_470.0]
    ));
    assert!(matches!(
        load_model(&net, "zip"),
        DistLoadVoltageModel::Zip { alpha_z, beta_i, .. }
            if alpha_z == &vec![0.2, 0.2, 0.2] && beta_i == &vec![0.4, 0.4, 0.4]
    ));

    let bmopf = write_bmopf_json(&net);
    let bmopf_doc: serde_json::Value = serde_json::from_str(&bmopf.text).unwrap();
    assert_eq!(bmopf_doc["load"]["ci"]["model"], "constant_current");
    assert_eq!(bmopf_doc["load"]["cz"]["model"], "constant_impedance");
    assert_eq!(bmopf_doc["load"]["zip"]["model"], "zip");
    assert_eq!(
        bmopf_doc["load"]["zip"]["alpha_z"],
        serde_json::json!([0.2, 0.2, 0.2])
    );
    assert_eq!(bmopf_doc["switch"]["sw"]["open_switch"], true);
    let from_bmopf = parse_bmopf_str(&bmopf.text).unwrap();
    assert_eq!(from_bmopf.switches[0].open, net.switches[0].open);
    assert_eq!(load_model(&from_bmopf, "zip"), load_model(&net, "zip"));

    let dss_back = write_dss(&from_bmopf);
    let reparsed = parse_dss_str(&dss_back.text);
    assert_eq!(reparsed.switches[0].open, net.switches[0].open);
    assert_eq!(load_model(&reparsed, "ci"), load_model(&net, "ci"));
    assert_eq!(load_model(&reparsed, "cz"), load_model(&net, "cz"));
    assert_eq!(load_model(&reparsed, "zip"), load_model(&net, "zip"));
}
