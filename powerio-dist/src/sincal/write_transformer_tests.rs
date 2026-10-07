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
                    assert!(recovered.transformers().is_empty());
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
                    let value = serde_json::json!({"input":net,"fresh_readback":recovered,"bus_ids":output.bus_ids});
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
