use super::write::{ExperimentalMulticonductorOptions, write_experimental_multiconductor};
use crate::{
    Configuration, DistBus, DistLine, DistLineCode, DistLoad, DistLoadVoltageModel, DistSwitch,
    MulticonductorNetwork, VoltageSource,
};
use std::f64::consts::TAU;

fn names(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| (*v).into()).collect()
}
fn constructed(floating: bool) -> (MulticonductorNetwork, ExperimentalMulticonductorOptions) {
    let mut net = MulticonductorNetwork::new();
    *net.base_frequency_mut() = 50.0;
    let mut source_bus = DistBus::new("supply", names(&["1", "2", "3"]));
    if floating {
        source_bus.terminals.push("local-star".into());
    }
    net.buses_mut().push(source_bus);
    let mut load_bus = DistBus::new("load", names(&["1", "2", "3", "earth"]));
    load_bus.grounded.push("earth".into());
    net.buses_mut().push(load_bus);
    let mut source = VoltageSource::new(
        "supply",
        "supply",
        names(&["1", "2", "3"]),
        vec![230.0; 3],
        vec![0.0, -TAU / 3.0, TAU / 3.0],
    );
    if floating {
        source.reference_terminal = Some("local-star".into());
    }
    net.sources_mut().push(source);
    let mut code = DistLineCode::new(
        "cable",
        vec![
            vec![0.0004, 0.0001, 0.0001],
            vec![0.0001, 0.0004, 0.0001],
            vec![0.0001, 0.0001, 0.0004],
        ],
        vec![
            vec![0.0003, 0.0001, 0.0001],
            vec![0.0001, 0.0003, 0.0001],
            vec![0.0001, 0.0001, 0.0003],
        ],
    );
    code.i_max = Some(vec![200.0; 3]);
    net.line_codes_mut().push(code);
    net.lines_mut().push(DistLine::new(
        "feeder",
        "supply",
        "load",
        names(&["1", "2", "3"]),
        names(&["1", "2", "3"]),
        "cable",
        120.0,
    ));
    let mut load = DistLoad::new(
        "unequal",
        "load",
        names(&["1", "2", "3", "earth"]),
        Configuration::Wye,
        vec![1000.0, 2000.0, 3000.0],
        vec![200.0, -100.0, 700.0],
    );
    load.voltage_model = DistLoadVoltageModel::ConstantImpedance {
        v_nom: vec![230.0; 3],
    };
    net.loads_mut().push(load);
    let options = ExperimentalMulticonductorOptions {
        nominal_ll_volts: net.buses().iter().map(|b| (b.id.clone(), 400.0)).collect(),
    };
    (net, options)
}
fn read(bytes: &[u8]) -> MulticonductorNetwork {
    super::read_snapshot(powerio_sincal::DatabaseSnapshot::decode(bytes, None).unwrap()).unwrap()
}
fn close(a: f64, b: f64) {
    assert!(
        (a - b).abs() < 1e-10 * a.abs().max(b.abs()).max(1e-15),
        "{a} vs {b}"
    );
}

#[test]
fn fresh_unbalanced_network_keeps_phase_powers_and_reference_kind() {
    for floating in [false, true] {
        let (net, options) = constructed(floating);
        let output = write_experimental_multiconductor(&net, &options).unwrap();
        let snapshot = powerio_sincal::DatabaseSnapshot::decode(&output.database, None).unwrap();
        let flags = snapshot
            .connection
            .query_row(
                "SELECT Flag_LFmet, Flag_UsymElm, Flag_DIType FROM CalcParameter",
                [],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(flags, (8, 3, 0));
        let out = read(&output.database);
        assert_eq!(out.loads().len(), 3);
        for (i, load) in out.loads().iter().enumerate() {
            assert_eq!(load.terminal_map, [format!("{}", i + 1), "0".into()]);
            close(load.p_nom[0], net.loads()[0].p_nom[i]);
            close(load.q_nom[0], net.loads()[0].q_nom[i]);
        }
        assert_eq!(out.sources()[0].reference_terminal.is_some(), floating);
        close(out.lines()[0].length, 120.0);
        assert_eq!(
            output.database,
            write_experimental_multiconductor(&net, &options)
                .unwrap()
                .database
        );
        let archive =
            powerio_sincal::authoring::candidate_archive(&output.database, "unbalanced").unwrap();
        assert_eq!(
            powerio_sincal::database_bytes(&archive).unwrap(),
            output.database
        );
    }
}

#[test]
fn closed_device_switches_can_be_canonicalized_and_rewritten() {
    let (net, options) = constructed(true);
    let first = write_experimental_multiconductor(&net, &options).unwrap();
    let mut out = read(&first.database);
    let options = ExperimentalMulticonductorOptions {
        nominal_ll_volts: out.buses().iter().map(|b| (b.id.clone(), 400.0)).collect(),
    };
    out.loads_mut()[1].p_nom[0] = 2500.0;
    let second = write_experimental_multiconductor(&out, &options).unwrap();
    assert_eq!(second.bus_ids.len(), out.buses().len());
    let readback = read(&second.database);
    close(readback.loads()[1].p_nom[0], 2500.0);
    assert!(readback.sources()[0].reference_terminal.is_some());
}

#[test]
fn single_phase_and_delta_branches_preserve_voltage_models() {
    for model in [1, 2, 3] {
        let (mut net, options) = constructed(false);
        let load = &mut net.loads_mut()[0];
        load.configuration = Configuration::Delta;
        load.terminal_map = names(&["1", "2", "3"]);
        load.voltage_model = match model {
            1 => DistLoadVoltageModel::ConstantPower {
                v_nom: vec![400.0; 3],
            },
            2 => DistLoadVoltageModel::ConstantCurrent {
                v_nom: vec![400.0; 3],
            },
            _ => DistLoadVoltageModel::ConstantImpedance {
                v_nom: vec![400.0; 3],
            },
        };
        let out = read(
            &write_experimental_multiconductor(&net, &options)
                .unwrap()
                .database,
        );
        for (load, pair) in out.loads().iter().zip([["1", "2"], ["2", "3"], ["3", "1"]]) {
            assert_eq!(load.terminal_map, pair);
            close(load.voltage_model.v_nom()[0], 400.0);
        }
    }
    let (mut net, options) = constructed(false);
    net.buses_mut()[1].terminals = names(&["3", "earth"]);
    net.lines_mut()[0].terminal_map_from = names(&["3"]);
    net.lines_mut()[0].terminal_map_to = names(&["3"]);
    let mut code = DistLineCode::new("cable", vec![vec![0.0004]], vec![vec![0.0003]]);
    code.i_max = Some(vec![200.0]);
    net.line_codes_mut()[0] = code;
    let load = &mut net.loads_mut()[0];
    load.configuration = Configuration::SinglePhase;
    load.terminal_map = names(&["3", "earth"]);
    load.p_nom = vec![1000.0];
    load.q_nom = vec![200.0];
    load.voltage_model = DistLoadVoltageModel::ConstantPower { v_nom: vec![] };
    let out = read(
        &write_experimental_multiconductor(&net, &options)
            .unwrap()
            .database,
    );
    assert_eq!(out.lines()[0].terminal_map_to, ["3"]);
    assert_eq!(out.loads()[0].terminal_map, ["3", "0"]);
}

#[test]
fn malformed_and_unsupported_inputs_fail_without_panics() {
    type Edit = fn(&mut MulticonductorNetwork, &mut ExperimentalMulticonductorOptions);
    let edits: &[Edit] = &[
        |_, o| {
            o.nominal_ll_volts.remove("load");
        },
        |n, _| n.sources_mut()[0].v_magnitude.clear(),
        |n, _| n.sources_mut()[0].v_magnitude[1] = 225.0,
        |n, _| n.sources_mut()[0].bus = "unknown".into(),
        |n, _| n.loads_mut()[0].q_nom.clear(),
        |n, _| n.loads_mut()[0].bus = "unknown".into(),
        |n, _| n.loads_mut()[0].terminal_map.clear(),
        |n, _| n.buses_mut()[1].grounded.clear(),
        |n, _| n.line_codes_mut()[0].i_max = Some(vec![]),
        |n, _| n.line_codes_mut()[0].r_series[0][0] = 0.0005,
        |n, _| n.line_codes_mut()[0].b_from[0][0] = f64::NAN,
        |n, _| {
            n.switches_mut().push(DistSwitch::new(
                "partial",
                "supply",
                "load",
                names(&["1"]),
                names(&["1"]),
                false,
            ));
        },
        |n, _| {
            n.switches_mut().push(DistSwitch::new(
                "open",
                "supply",
                "load",
                names(&["1", "2", "3"]),
                names(&["1", "2", "3"]),
                true,
            ));
        },
    ];
    for edit in edits {
        let (mut net, mut options) = constructed(false);
        edit(&mut net, &mut options);
        assert!(write_experimental_multiconductor(&net, &options).is_err());
    }
}

#[test]
#[ignore = "exports fresh writer/readback circuits for independent validation"]
fn export_writer_oracle() {
    let dir = std::path::PathBuf::from(
        std::env::var_os("POWERIO_SINCAL_WRITER_ORACLE_DIR").expect("output directory"),
    );
    std::fs::create_dir_all(&dir).unwrap();
    for (name, floating) in [("grounded", false), ("floating", true)] {
        let (net, options) = constructed(floating);
        let output = write_experimental_multiconductor(&net, &options).unwrap();
        let recovered = read(&output.database);
        let document = serde_json::json!({"input":net,"fresh_readback":recovered,"bus_ids":output.bus_ids,"native_acceptance":false});
        std::fs::write(
            dir.join(format!("{name}.json")),
            serde_json::to_vec_pretty(&document).unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn phase_pair_series_coupling_and_distinct_load_voltages_survive() {
    let (mut net, options) = constructed(false);
    net.buses_mut()[1].terminals = names(&["3", "1", "earth"]);
    net.lines_mut()[0].terminal_map_from = names(&["3", "1"]);
    net.lines_mut()[0].terminal_map_to = names(&["3", "1"]);
    let mut code = DistLineCode::new(
        "cable",
        vec![vec![0.0004, 0.0001], vec![0.0001, 0.0004]],
        vec![vec![0.0003, 0.0001], vec![0.0001, 0.0003]],
    );
    code.i_max = Some(vec![200.0; 2]);
    net.line_codes_mut()[0] = code;
    let load = &mut net.loads_mut()[0];
    load.terminal_map = names(&["3", "1", "earth"]);
    load.p_nom = vec![1000.0, 3000.0];
    load.q_nom = vec![100.0, -400.0];
    load.voltage_model = DistLoadVoltageModel::ConstantCurrent {
        v_nom: vec![230.0, 225.0],
    };
    let out = read(
        &write_experimental_multiconductor(&net, &options)
            .unwrap()
            .database,
    );
    assert_eq!(out.lines()[0].terminal_map_to, ["3", "1"]);
    assert_eq!(out.loads()[0].terminal_map, ["3", "0"]);
    assert_eq!(out.loads()[1].terminal_map, ["1", "0"]);
    close(out.loads()[1].voltage_model.v_nom()[0], 225.0);
    close(out.line_codes()[0].r_series[0][1], 0.0001);
}

#[test]
fn sequence_capacitance_and_ampacity_survive_unit_conversion() {
    let (mut net, options) = constructed(false);
    let code = &mut net.line_codes_mut()[0];
    code.b_from = vec![
        vec![2e-9, -0.5e-9, -0.5e-9],
        vec![-0.5e-9, 2e-9, -0.5e-9],
        vec![-0.5e-9, -0.5e-9, 2e-9],
    ];
    code.b_to.clone_from(&code.b_from);
    let out = read(
        &write_experimental_multiconductor(&net, &options)
            .unwrap()
            .database,
    );
    let code = &out.line_codes()[0];
    close(code.b_from[0][0], 2e-9);
    close(code.b_from[0][1], -0.5e-9);
    assert_eq!(code.i_max.as_ref().unwrap(), &[200.0; 3]);
}
