use super::write::{ExperimentalMulticonductorOptions, write_experimental_multiconductor};
use crate::{DistBus, DistTransformer, DistWinding, DistWindingConn, MulticonductorNetwork};

fn constructed(
    kind: usize,
    step_up: bool,
    lead: bool,
    tapped: bool,
) -> (MulticonductorNetwork, ExperimentalMulticonductorOptions) {
    let mut net = MulticonductorNetwork::new();
    *net.base_frequency_mut() = 50.0;
    let mut windings = Vec::new();
    for side in 0..2 {
        let name = format!("port-{side}");
        let conn = if kind == side + 1 {
            DistWindingConn::Wye
        } else {
            DistWindingConn::Delta
        };
        let mut terminals = ["1", "2", "3"].map(str::to_owned).to_vec();
        if conn == DistWindingConn::Wye {
            terminals.push("earth".into());
        }
        let mut bus = DistBus::new(name.clone(), terminals.clone());
        if conn == DistWindingConn::Wye {
            bus.grounded.push("earth".into());
        }
        net.buses_mut().push(bus);
        let v = if (side == 0) ^ step_up {
            11000.0
        } else {
            400.0
        };
        let mut w = DistWinding::new(name, terminals, conn, v, 100_000.0);
        w.r_pct = if side == 0 { 0.3 } else { 0.7 };
        w.tap = if tapped { [1.05, 0.975][side] } else { 1.0 };
        windings.push(w);
    }
    let mut t = DistTransformer::new("tx", windings, vec![4.0], 3);
    if lead {
        t.extras.insert("leadlag".into(), serde_json::json!("Euro"));
    }
    net.transformers_mut().push(t);
    let options = ExperimentalMulticonductorOptions {
        nominal_ll_volts: net.transformers()[0]
            .windings
            .iter()
            .map(|w| (w.bus.clone(), w.v_ref))
            .collect(),
    };
    (net, options)
}

#[test]
fn finite_transformers_preserve_conductor_primitive_and_winding_order() {
    for kind in 0..3 {
        for step_up in [false, true] {
            for lead in [false, true] {
                for tapped in [false, true] {
                    let (net, options) = constructed(kind, step_up, lead, tapped);
                    let output = write_experimental_multiconductor(&net, &options).unwrap();
                    let snapshot =
                        powerio_sincal::DatabaseSnapshot::decode(&output.database, None).unwrap();
                    let code: i64 = snapshot
                        .connection
                        .query_row("SELECT VecGrp FROM TwoWindingTransformer", [], |r| r.get(0))
                        .unwrap();
                    let reverse = step_up ^ lead;
                    assert_eq!(
                        code,
                        match (kind, reverse) {
                            (0, _) => 1,
                            (1, false) => 14,
                            (1, true) => 61,
                            (2, false) => 10,
                            (2, true) => 59,
                            _ => unreachable!(),
                        }
                    );
                    let recovered = super::read_snapshot(snapshot).unwrap();
                    assert_eq!(recovered.shunts().len(), 1);
                    assert_eq!(recovered.switches().len(), 2);
                    assert_eq!(recovered.transformers().as_slice(), []);
                    // Recreate from source-free serde data, including an edit.
                    let json = serde_json::to_vec(&net).unwrap();
                    let mut restored: MulticonductorNetwork =
                        serde_json::from_slice(&json).unwrap();
                    restored.transformers_mut()[0].xsc_pct[0] = 5.0;
                    assert_ne!(
                        output.database,
                        write_experimental_multiconductor(&restored, &options)
                            .unwrap()
                            .database
                    );
                }
            }
        }
    }
}

#[test]
fn transformer_unknown_physics_and_malformed_ports_fail_atomically() {
    let (net, options) = constructed(2, false, false, false);
    for mutate in [
        |t: &mut DistTransformer| {
            t.windings.clear();
        },
        |t: &mut DistTransformer| {
            t.xsc_pct.clear();
        },
        |t: &mut DistTransformer| {
            t.windings[1].terminal_map.clear();
        },
        |t: &mut DistTransformer| {
            t.windings[1].terminal_map[3] = "1".into();
        },
        |t: &mut DistTransformer| {
            t.windings[1].bus = "missing".into();
        },
        |t: &mut DistTransformer| {
            t.windings[0].v_ref = f64::NAN;
        },
        |t: &mut DistTransformer| {
            t.windings[1].s_rating *= 2.0;
        },
        |t: &mut DistTransformer| {
            t.windings[1].tap = 0.0;
        },
        |t: &mut DistTransformer| {
            t.windings[1].r_neutral = Some(1.0);
        },
        |t: &mut DistTransformer| {
            t.windings[0].r_pct = -1.0;
        },
        |t: &mut DistTransformer| {
            t.xsc_pct[0] = -1.0;
        },
        |t: &mut DistTransformer| {
            t.extras.insert("%imag".into(), serde_json::json!(2.0));
        },
        |t: &mut DistTransformer| {
            t.extras
                .insert("leadlag".into(), serde_json::json!("unknown"));
        },
        |t: &mut DistTransformer| {
            t.extras
                .insert("no_load_shunt".into(), serde_json::json!({"g":0.01}));
        },
    ] {
        let mut invalid = net.clone();
        mutate(&mut invalid.transformers_mut()[0]);
        assert!(write_experimental_multiconductor(&invalid, &options).is_err());
    }
}

#[test]
#[ignore = "exports original synthetic circuits for the independent OpenDSS primitive oracle"]
fn export_transformer_writer_oracle() {
    let directory = std::path::PathBuf::from(
        std::env::var("POWERIO_SINCAL_WRITER_ORACLE_DIR").expect("explicit output directory"),
    );
    std::fs::create_dir_all(&directory).unwrap();
    for kind in 0..3 {
        for step_up in [false, true] {
            for lead in [false, true] {
                for tapped in [false, true] {
                    let (net, options) = constructed(kind, step_up, lead, tapped);
                    let output = write_experimental_multiconductor(&net, &options).unwrap();
                    let recovered = super::read_snapshot(
                        powerio_sincal::DatabaseSnapshot::decode(&output.database, None).unwrap(),
                    )
                    .unwrap();
                    let case = format!("transformer-{kind}-{step_up}-{lead}-{tapped}");
                    let levels = rewrite_options(&recovered, &options, &output.bus_ids);
                    let rewritten = write_experimental_multiconductor(&recovered, &levels).unwrap();
                    let mut edited = recovered.clone();
                    let shunt = &mut edited.shunts_mut()[0];
                    shunt.extras.clear();
                    for value in shunt.g.iter_mut().chain(&mut shunt.b).flatten() {
                        *value *= 1.2;
                    }
                    let changed = write_experimental_multiconductor(&edited, &levels).unwrap();
                    let value = serde_json::json!({"input":net,"fresh_readback":recovered,"bus_ids":output.bus_ids,
                        "rewritten_readback":read(&rewritten.database), "rewrite_bus_ids":rewritten.bus_ids,
                        "edited_readback":read(&changed.database), "edit_bus_ids":changed.bus_ids});
                    std::fs::write(
                        directory.join(format!("{case}.json")),
                        serde_json::to_vec_pretty(&value).unwrap(),
                    )
                    .unwrap();
                }
            }
        }
    }
}

fn read(bytes: &[u8]) -> MulticonductorNetwork {
    super::read_snapshot(powerio_sincal::DatabaseSnapshot::decode(bytes, None).unwrap()).unwrap()
}
fn rewrite_options(
    net: &MulticonductorNetwork,
    original: &ExperimentalMulticonductorOptions,
    ids: &std::collections::BTreeMap<String, i64>,
) -> ExperimentalMulticonductorOptions {
    let levels = ids
        .iter()
        .map(|(name, id)| (id.to_string(), original.nominal_ll_volts[name]))
        .collect::<std::collections::BTreeMap<_, _>>();
    ExperimentalMulticonductorOptions {
        nominal_ll_volts: net
            .buses()
            .iter()
            .filter_map(|b| levels.get(&b.id).map(|v| (b.id.clone(), *v)))
            .collect(),
    }
}
fn compare_primitives(a: &MulticonductorNetwork, b: &MulticonductorNetwork) {
    for (a, b) in a.shunts().iter().zip(b.shunts()) {
        for (a, b) in [&a.g, &a.b].into_iter().zip([&b.g, &b.b]) {
            let scale = a.iter().flatten().map(|v| v.abs()).fold(0.0_f64, f64::max);
            for (a, b) in a.iter().flatten().zip(b.iter().flatten()) {
                assert!((a - b).abs() <= 1e-10 * scale, "{a} != {b}");
            }
        }
    }
}

#[test]
fn transformer_primitives_rewrite_after_edits_without_provenance() {
    for kind in 0..3 {
        for up in [false, true] {
            for lead in [false, true] {
                for tapped in [false, true] {
                    let (net, options) = constructed(kind, up, lead, tapped);
                    let first = write_experimental_multiconductor(&net, &options).unwrap();
                    let recovered = read(&first.database);
                    let levels = rewrite_options(&recovered, &options, &first.bus_ids);
                    let second = write_experimental_multiconductor(&recovered, &levels).unwrap();
                    assert!(!second.bus_ids.contains_key(&recovered.shunts()[0].bus));
                    compare_primitives(&recovered, &read(&second.database));
                    let mut edited = recovered.clone();
                    edited.shunts_mut()[0].extras.clear();
                    let shunt = &mut edited.shunts_mut()[0];
                    for v in shunt.g.iter_mut().chain(&mut shunt.b).flatten() {
                        *v *= 1.2;
                    }
                    let third = write_experimental_multiconductor(&edited, &levels).unwrap();
                    assert_ne!(second.database, third.database);
                    compare_primitives(&edited, &read(&third.database));
                }
            }
        }
    }
}

#[test]
fn primitive_port_directions_are_electrical_but_extra_connections_are_not_discarded() {
    let (mut original, options) = constructed(2, false, false, false);
    for w in &mut original.transformers_mut()[0].windings {
        w.r_pct = 0.0;
    }
    let first = write_experimental_multiconductor(&original, &options).unwrap();
    let mut net = read(&first.database);
    let levels = rewrite_options(&net, &options, &first.bus_ids);
    for s in net.switches_mut() {
        std::mem::swap(&mut s.bus_from, &mut s.bus_to);
        std::mem::swap(&mut s.terminal_map_from, &mut s.terminal_map_to);
    }
    let out = write_experimental_multiconductor(&net, &levels).unwrap();
    compare_primitives(&net, &read(&out.database));
    for mutate in [
        |n: &mut MulticonductorNetwork| {
            n.switches_mut()[0].open = true;
        },
        |n: &mut MulticonductorNetwork| {
            n.shunts_mut()[0].g[0][1] += 0.01;
        },
        |n: &mut MulticonductorNetwork| {
            n.shunts_mut()[0].b[3][3] += 0.001;
        },
        |n: &mut MulticonductorNetwork| {
            let other = n.switches()[0].clone();
            n.switches_mut().push(other);
        },
        |n: &mut MulticonductorNetwork| {
            n.shunts_mut()[0].g.pop();
        },
        |n: &mut MulticonductorNetwork| {
            let mut other = n.shunts()[0].clone();
            other.name = "other".into();
            n.shunts_mut().push(other);
        },
        |n: &mut MulticonductorNetwork| {
            let shunt = &n.shunts()[0];
            let load = crate::DistLoad::new(
                "external",
                shunt.bus.clone(),
                shunt.terminal_map[..2].to_vec(),
                crate::Configuration::SinglePhase,
                vec![10.0],
                vec![0.0],
            );
            n.loads_mut().push(load);
        },
    ] {
        let mut invalid = net.clone();
        mutate(&mut invalid);
        assert!(write_experimental_multiconductor(&invalid, &levels).is_err());
    }
}

#[test]
fn galvanic_y0_primitive_is_not_rewritten_as_an_isolated_transformer() {
    let db = super::legacy_tests::legacy(
        "UPDATE TwoWindingTransformer SET VecGrp=71,Vfe=0,i0=0,Flag_Z0_Input=3,R0_R1=1,X0_X1=1,uk=5,ur=1;",
    );
    let mut net = db.network().unwrap();
    let options = ExperimentalMulticonductorOptions {
        nominal_ll_volts: net
            .buses()
            .iter()
            .map(|b| (b.id.clone(), if b.id == "20" { 11000.0 } else { 400.0 }))
            .collect(),
    };
    for retain_provenance in [true, false] {
        if !retain_provenance {
            for shunt in net.shunts_mut() {
                shunt.extras.clear();
            }
        }
        // The conductor matrix itself must prevent DD/DY/YD reconstruction,
        // including after IR restoration or removal of source provenance.
        let error = super::write_primitive::canonicalize(&net, &options)
            .err()
            .expect("galvanic primitive must reject isolated transformer candidates");
        assert!(
            error
                .to_string()
                .contains("no verified delta/delta or solid delta/Wye")
        );
        assert!(write_experimental_multiconductor(&net, &options).is_err());
    }
}

#[test]
fn partial_delta_coils_are_not_rewritten_as_a_full_three_phase_transformer() {
    for group in [1, 35] {
        for selection in 1..=6 {
            let db = super::legacy_tests::legacy(&format!(
                "UPDATE TwoWindingTransformer SET VecGrp={group},Vfe=0,i0=0;
                 UPDATE Terminal SET Flag_Terminal={selection} WHERE Element_ID=33;"
            ));
            let mut net = db.network().unwrap();
            let options = ExperimentalMulticonductorOptions {
                nominal_ll_volts: net
                    .buses()
                    .iter()
                    .map(|b| (b.id.clone(), if b.id == "20" { 11000.0 } else { 400.0 }))
                    .collect(),
            };
            for retain_provenance in [true, false] {
                if !retain_provenance {
                    for shunt in net.shunts_mut() {
                        shunt.extras.clear();
                    }
                }
                // One coil has four coordinates; two coils have six but their
                // non-circulant primitive still cannot become a full winding.
                assert!(super::write_primitive::canonicalize(&net, &options).is_err());
                assert!(write_experimental_multiconductor(&net, &options).is_err());
            }
        }
    }
}
