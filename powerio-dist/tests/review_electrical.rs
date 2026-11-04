//! Electrical regressions from the RAVENS and PMD MATHEMATICAL review.
mod helpers;

use powerio_dist::{DistTargetFormat, MulticonductorNetwork};
use serde_json::{Value, json};

fn document(net: &MulticonductorNetwork, target: DistTargetFormat) -> Value {
    serde_json::from_str(&helpers::emit_value(net, target).text).unwrap()
}

fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-9 * expected.abs().max(1.0),
        "{actual} != {expected}"
    );
}

fn bounds(value: &Value, out: &mut Vec<f64>) {
    if let Some(v) = value.get("VoltageLimit.value").and_then(Value::as_f64) {
        out.push(v);
    }
    match value {
        Value::Object(o) => {
            for v in o.values() {
                bounds(v, out);
            }
        }
        Value::Array(a) => {
            for v in a {
                bounds(v, out);
            }
        }
        _ => {}
    }
}

#[test]
fn nonkron_transformer_loss_branches_use_internal_phase_terminals() {
    for phase in [2, 3] {
        for kron_reduce in [false, true] {
            // An ungrounded neutral also exercises the default non-Kron path.
            let net = helpers::parse_dss_str(&format!(
                "clear\nnew circuit.c phases=1 basekv=7.2 bus1=p.{phase}\nnew transformer.t phases=1 windings=2 buses=(p.{phase}.4,s.{phase}.4) conns=(wye,wye) kvs=(7.2,0.24) kvas=(25,25) xhl=2 %rs=(0.5,0.5) wdg=1 rneut=-1 wdg=2 rneut=-1\n"
            ));
            let mut options = powerio_dist::EmitOptions::default();
            options.pmd_math.kron_reduce = kron_reduce;
            let result = powerio_dist::emit_with_options(
                &powerio_core::PioModule::new((*net).clone()),
                DistTargetFormat::PmdMathJson,
                &options,
                powerio_core::Destination::memory("out").unwrap(),
            )
            .unwrap();
            let powerio_core::EmittedOutput::Memory { mut artifacts } = result.into_output() else {
                panic!()
            };
            let doc: Value =
                serde_json::from_slice(&artifacts.pop().unwrap().into_bytes()).unwrap();
            assert_eq!(doc["is_kron_reduced"], false);
            let mut checked = 0;
            for branch in doc["branch"].as_object().unwrap().values() {
                if !branch["name"]
                    .as_str()
                    .unwrap_or("")
                    .starts_with("_virtual_branch.transformer.")
                {
                    continue;
                }
                for (bus_key, connections) in
                    [("f_bus", "f_connections"), ("t_bus", "t_connections")]
                {
                    let bus = &doc["bus"][branch[bus_key].as_i64().unwrap().to_string()];
                    for terminal in branch[connections].as_array().unwrap() {
                        let at = bus["terminals"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .position(|t| t == terminal)
                            .expect("connection exists");
                        assert_eq!(
                            bus["grounded"][at], false,
                            "{} connects to grounded terminal {terminal} on {}",
                            branch["name"], bus["name"]
                        );
                        checked += 1;
                    }
                }
            }
            assert!(checked > 0);
        }
    }
}

#[test]
fn ravens_line_to_line_voltage_limits_become_phase_voltage_limits() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data/dist/ravens/case3_balanced.json");
    let net = helpers::parse_file(path, Some("ravens-json")).unwrap();
    let doc = document(&net, DistTargetFormat::PmdJson);
    assert_close(
        doc["bus"]["loadbus"]["vm_lb"][0].as_f64().unwrap(),
        0.38 / 3f64.sqrt(),
    );
    assert_close(
        doc["bus"]["loadbus"]["vm_ub"][0].as_f64().unwrap(),
        0.42 / 3f64.sqrt(),
    );
}

#[test]
fn ravens_voltage_limit_emission_preserves_phase_bounds_and_is_stable() {
    for phases in [1, 3] {
        let mut net = helpers::parse_dss_str(&format!(
            "clear\nnew circuit.c phases={phases} basekv=0.4 bus1=b\nnew load.l phases={phases} bus1=b kv=0.4 kw=30\n"
        ));
        let bus = net.buses_mut().iter_mut().find(|b| b.id == "b").unwrap();
        bus.v_min = Some(200.0);
        bus.v_max = Some(250.0);
        let out = helpers::emit_value(&net, DistTargetFormat::RavensJson);
        let back = helpers::parse_str(&out.text, "ravens-json").unwrap();
        assert_close(back.bus("b").unwrap().v_min.unwrap(), 200.0);
        assert_close(back.bus("b").unwrap().v_max.unwrap(), 250.0);
        let doc: Value = serde_json::from_str(&out.text).unwrap();
        let mut limits = Vec::new();
        bounds(&doc, &mut limits);
        assert!(
            limits
                .iter()
                .any(|v| (*v - 200.0 * 3f64.sqrt()).abs() < 1e-9)
        );
        assert_eq!(
            out.text,
            helpers::emit_value(&back, DistTargetFormat::RavensJson).text
        );
    }
}

#[test]
fn split_ravens_source_preserves_the_coupled_thevenin_matrix() {
    let mut net = helpers::parse_str(r#"{"data_model":"ENGINEERING","bus":{"port":{"terminals":[1,2,3]}},"voltage_source":{"source":{"bus":"port","connections":[1,2,3],"vm":[0.23,0.22,0.24],"va":[0,-120,120],"rs":[[3,1,1],[1,3,1],[1,1,3]],"xs":[[5,2,2],[2,5,2],[2,2,5]]}}}"#, "pmd-json").unwrap();
    for sequence in [false, true] {
        if sequence {
            let extras = &mut net.sources_mut()[0].extras;
            extras.remove("rs");
            extras.remove("xs");
            for (key, value) in [("r1", 2.0), ("r0", 5.0), ("x1", 3.0), ("x0", 9.0)] {
                extras.insert(key.into(), json!(value));
            }
        }
        let out = helpers::emit_value(&net, DistTargetFormat::RavensJson);
        let back = helpers::parse_str(&out.text, "ravens-json").unwrap();
        assert_eq!(back.sources().len(), 3);
        assert_eq!(
            back.lines().len(),
            1,
            "one coupled branch, not independent per-phase branches"
        );
        let line = &back.lines()[0];
        assert_eq!(line.bus_to, "port");
        let code = back.linecode(&line.linecode).unwrap();
        assert_eq!(
            code.r_series,
            vec![
                vec![3.0, 1.0, 1.0],
                vec![1.0, 3.0, 1.0],
                vec![1.0, 1.0, 3.0]
            ]
        );
        assert_eq!(
            code.x_series,
            vec![
                vec![5.0, 2.0, 2.0],
                vec![2.0, 5.0, 2.0],
                vec![2.0, 2.0, 5.0]
            ]
        );
        for source in back.sources() {
            assert_eq!(source.bus, line.bus_from);
            let terminal = &source.terminal_map[0];
            let index = net.sources()[0]
                .terminal_map
                .iter()
                .position(|t| t == terminal)
                .unwrap();
            assert_close(source.v_magnitude[0], net.sources()[0].v_magnitude[index]);
            assert_close(source.v_angle[0], net.sources()[0].v_angle[index]);
        }
        assert_eq!(
            out.text,
            helpers::emit_value(&back, DistTargetFormat::RavensJson).text
        );
    }
}

#[test]
fn split_source_rejects_impedance_it_cannot_preserve() {
    let mut net = helpers::parse_dss_str("clear\nnew circuit.c phases=3 basekv=0.4 bus1=b\n");
    // Use an ideal source at the existing bus, with unequal phase EMFs.
    net.sources_mut().clear();
    let mut source = powerio_dist::VoltageSource::new(
        "bad",
        "b",
        vec!["1".into(), "2".into(), "3".into()],
        vec![230.0, 220.0, 240.0],
        vec![0.0, -2.0, 2.0],
    );
    for matrix in [
        json!([[1, 2, 0], [0, 1, 0], [0, 0, 1]]),
        json!([[1, 0], [0, 1]]),
        json!([[1, 0, 0], [0, 1, 0], [0, 0, null]]),
    ] {
        source.extras.insert("rs".into(), matrix);
        net.sources_mut().clear();
        net.sources_mut().push(source.clone());
        let error = powerio_dist::emit(
            &powerio_core::PioModule::new((*net).clone()),
            DistTargetFormat::RavensJson,
            powerio_core::Destination::memory("out").unwrap(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("Thevenin impedance"), "{error}");
    }
}

#[test]
fn split_source_auxiliary_identity_does_not_alias_existing_buses() {
    let mut net = helpers::parse_str(r#"{"data_model":"ENGINEERING","bus":{"port":{"terminals":[1,2,3]},"_PIO_RAVENS_source_EMF":{"terminals":[1]}},"voltage_source":{"source":{"bus":"port","connections":[1,2,3],"vm":[0.23,0.22,0.24],"va":[0,-120,120],"rs":[[3,1,1],[1,3,1],[1,1,3]],"xs":[[5,2,2],[2,5,2],[2,2,5]]}}}"#, "pmd-json").unwrap();
    let out = helpers::emit_value(&net, DistTargetFormat::RavensJson);
    let back = helpers::parse_str(&out.text, "ravens-json").unwrap();
    assert!(
        back.sources()
            .iter()
            .all(|s| !s.bus.eq_ignore_ascii_case("_PIO_RAVENS_source_EMF"))
    );
    // A later edit must use the new matrix, not a prepared profile from an earlier emission.
    net.sources_mut()[0]
        .extras
        .insert("rs".into(), json!([[6, 2, 2], [2, 6, 2], [2, 2, 6]]));
    let out = helpers::emit_value(&net, DistTargetFormat::RavensJson);
    let back = helpers::parse_str(&out.text, "ravens-json").unwrap();
    assert_close(back.line_codes()[0].r_series[0][0], 6.0);
}

#[test]
fn split_source_keeps_explicit_neutral_coupling() {
    let rs = json!([
        [3.0, 1.0, 1.0, 0.5],
        [1.0, 3.0, 1.0, 0.5],
        [1.0, 1.0, 3.0, 0.5],
        [0.5, 0.5, 0.5, 4.0]
    ]);
    let xs = json!([
        [5.0, 2.0, 2.0, 0.25],
        [2.0, 5.0, 2.0, 0.25],
        [2.0, 2.0, 5.0, 0.25],
        [0.25, 0.25, 0.25, 6.0]
    ]);
    let input = json!({"data_model":"ENGINEERING","bus":{"port":{"terminals":[1,2,3,4]}},"voltage_source":{"source":{"bus":"port","connections":[1,2,3,4],"vm":[0.23,0.22,0.24,0.0],"va":[0,-120,120,0],"rs":rs,"xs":xs}}});
    let net = helpers::parse_str(&input.to_string(), "pmd-json").unwrap();
    let out = helpers::emit_value(&net, DistTargetFormat::RavensJson);
    let back = helpers::parse_str(&out.text, "ravens-json").unwrap();
    assert_eq!(back.lines().len(), 1);
    assert_eq!(back.lines()[0].terminal_map_from, ["1", "2", "3", "4"]);
    let code = back.linecode(&back.lines()[0].linecode).unwrap();
    for (actual, expected) in [(&code.r_series, &rs), (&code.x_series, &xs)] {
        assert_eq!(actual.len(), 4);
        for (i, row) in actual.iter().enumerate() {
            for (j, &value) in row.iter().enumerate() {
                assert_close(value, expected[i][j].as_f64().unwrap());
            }
        }
    }
    assert_eq!(
        out.text,
        helpers::emit_value(&back, DistTargetFormat::RavensJson).text
    );
}
