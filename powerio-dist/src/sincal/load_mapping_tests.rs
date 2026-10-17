use num_complex::Complex64;

use super::{
    load::{LoadInput, LoadZeroSequence, PowerInput},
    schema::NativeDatabase,
    tests::load_database,
};
use crate::{Configuration, DistBus, DistLoadVoltageModel, MulticonductorNetwork};

fn native(edit: &str) -> NativeDatabase {
    let bytes = load_database(&format!(
        "ALTER TABLE Load ADD COLUMN Flag_Z0_Input INTEGER DEFAULT 3;
         ALTER TABLE Load ADD COLUMN Z0_Z1 REAL DEFAULT 2;
         ALTER TABLE Load ADD COLUMN R0_X0 REAL DEFAULT 0.5;
         ALTER TABLE Load ADD COLUMN R0 REAL DEFAULT 0.2;
         ALTER TABLE Load ADD COLUMN X0 REAL DEFAULT 0.4;
         ALTER TABLE Load ADD COLUMN Pneg REAL DEFAULT 0;
         ALTER TABLE Load ADD COLUMN Qneg REAL DEFAULT 0;
         UPDATE Element SET Flag_Input=6;
         {edit}"
    ));
    NativeDatabase::decode(&bytes, None).unwrap()
}

fn input(edit: &str) -> LoadInput {
    native(edit).load_input(30).unwrap()
}

fn bus() -> DistBus {
    let mut bus = DistBus::new("10", ["1", "2", "3", "4"].map(str::to_owned).to_vec());
    bus.grounded.push("4".into());
    bus
}

fn near(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() <= 1e-11 * expected.abs().max(1.0),
        "{actual} != {expected}"
    );
}

#[test]
fn load_switches_preserve_every_supported_connection_and_voltage_model() {
    let native_bus = DistBus::new("10", ["3", "n", "1", "2"].map(str::to_owned).to_vec());
    for code in 1..=7 {
        for model in 1..=3 {
            let edit = format!(
                "UPDATE Terminal SET Flag_Terminal={code}; UPDATE Load SET Flag_LoadType={model}"
            );
            let closed = input(&edit).circuit(&native_bus, 400.0).unwrap();
            let open = input(&format!("{edit}; UPDATE Terminal SET Flag_State=0"))
                .circuit(&native_bus, 400.0)
                .unwrap();
            assert_eq!(closed.load, open.load);
            assert_eq!(closed.bus, open.bus);
            assert!(!closed.switch.open && open.switch.open);
            assert_eq!(open.switch.bus_from, native_bus.id);
            assert_eq!(open.switch.bus_to, open.load.bus);
            assert_eq!(open.switch.terminal_map_from, open.switch.terminal_map_to);
            assert_eq!(native_bus.grounded, Vec::<String>::new());
            let phases = super::semantics::Connection::decode(code)
                .unwrap()
                .phases()
                .unwrap();
            let expected: Vec<String> = phases.iter().map(|p| (p + 1).to_string()).collect();
            assert_eq!(open.switch.terminal_map_from, expected);
            assert_eq!(open.bus.grounded.is_empty(), phases.len() == 2);
            assert!(
                !open
                    .switch
                    .terminal_map_to
                    .iter()
                    .any(|p| p == "n" || p == "0")
            );
            near(open.load.p_nom.iter().sum(), 12_000.0);
            near(open.load.q_nom.iter().sum(), 1_000.0);
            for voltage in open.load.voltage_model.v_nom() {
                near(
                    *voltage,
                    if phases.len() == 2 {
                        400.0
                    } else {
                        400.0 / 3.0_f64.sqrt()
                    },
                );
            }
            let mut net = MulticonductorNetwork::new();
            net.buses_mut().extend([native_bus.clone(), open.bus]);
            net.loads_mut().push(open.load);
            net.switches_mut().push(open.switch);
            crate::require_electrical_readiness(&net).unwrap();
            let graph = net.to_graph();
            assert!(!graph.edges[0].closed);
            near(
                graph.buses.iter().find(|b| b.id == "10").unwrap().load_kw,
                0.0,
            );
            near(
                graph
                    .buses
                    .iter()
                    .find(|b| b.id == net.loads()[0].bus)
                    .unwrap()
                    .load_kw,
                12.0,
            );
            let output = crate::convert::emit_value_text(&net, crate::DistTargetFormat::PmdJson);
            let parsed = crate::testkit::parse_str(&output.text, "pmd-json").unwrap();
            crate::require_electrical_readiness(&parsed).unwrap();
            assert!(parsed.switches()[0].open);
            assert_eq!(parsed.loads()[0].p_nom, net.loads()[0].p_nom);
            assert_eq!(parsed.loads()[0].q_nom, net.loads()[0].q_nom);
            assert_eq!(parsed.loads()[0].terminal_map, net.loads()[0].terminal_map);
            assert_eq!(
                parsed.loads()[0].voltage_model,
                net.loads()[0].voltage_model
            );
        }
    }
}

#[test]
fn open_loads_keep_unequal_wye_and_delta_branches_and_still_reject_unresolved_physics() {
    for mode in [13, 14, 15] {
        let input = input(&format!(
            "UPDATE Load SET Flag_Lf={mode}; UPDATE Terminal SET Flag_State=0"
        ));
        let circuit = input.circuit(&bus(), 400.0).unwrap();
        assert!(circuit.switch.open);
        assert_eq!(
            circuit.load.configuration,
            if mode == 13 {
                Configuration::Wye
            } else {
                Configuration::Delta
            }
        );
        assert_eq!(circuit.bus.grounded.is_empty(), mode != 13);
        let mut closed = input;
        closed.terminal.state = super::semantics::State::On;
        assert_eq!(closed.circuit(&bus(), 400.0).unwrap().load, circuit.load);
    }
    for edit in [
        "UPDATE Load SET DayOpSer_ID=1",
        "UPDATE Load SET Stp_ID=1",
        "UPDATE Load SET Flag_Z0_Input=1",
        "UPDATE Element SET Flag_State=0",
        "UPDATE Element SET Flag_Input=14; UPDATE Load SET Pneg=0.001",
    ] {
        let input = input(&format!("UPDATE Terminal SET Flag_State=0;{edit}"));
        assert!(input.circuit(&bus(), 400.0).is_err(), "accepted {edit}");
    }
    let input = input("UPDATE Terminal SET Flag_State=0");
    let mut missing = bus();
    missing.terminals.retain(|p| p != "2");
    assert!(input.circuit(&missing, 400.0).is_err());
    assert!(input.circuit(&bus(), f64::INFINITY).is_err());
    let mut wrong = bus();
    wrong.id = "20".into();
    assert!(input.circuit(&wrong, 400.0).is_err());
}

#[test]
fn aggregate_loads_allocate_total_power_and_use_connection_voltage_bases() {
    let wye = input("").lower_static(&bus(), 400.0, Some("4")).unwrap();
    assert_eq!(wye.configuration, Configuration::Wye);
    assert_eq!(wye.terminal_map, ["1", "2", "3", "4"]);
    assert_eq!(wye.p_nom, [4000.0; 3]);
    for q in &wye.q_nom {
        near(*q, 1000.0 / 3.0);
    }
    for v in wye.voltage_model.v_nom() {
        near(*v, 400.0 / 3f64.sqrt());
    }
    let mut permuted_bus = bus();
    permuted_bus.terminals = ["3", "1", "4", "2"].map(str::to_owned).to_vec();
    let permuted = input("")
        .lower_static(&permuted_bus, 400.0, Some("4"))
        .unwrap();
    assert_eq!(permuted, wye);
    let single =
        input("UPDATE Terminal SET Flag_Terminal=2; UPDATE Load SET Flag_LoadType=3,u=110")
            .lower_static(&bus(), 400.0, Some("4"))
            .unwrap();
    assert_eq!(single.configuration, Configuration::SinglePhase);
    assert_eq!(single.terminal_map, ["2", "4"]);
    assert_eq!(single.p_nom, [12000.0]);
    assert_eq!(single.q_nom, [1000.0]);
    assert!(matches!(
        single.voltage_model,
        DistLoadVoltageModel::ConstantCurrent { .. }
    ));
    near(single.voltage_model.v_nom()[0], 440.0 / 3f64.sqrt());
    let pair = input(
        "UPDATE Terminal SET Flag_Terminal=6; UPDATE Load SET Flag_LoadType=1,Flag_Lf=2,u=NULL",
    )
    .lower_static(&bus(), 400.0, None)
    .unwrap();
    assert_eq!(pair.configuration, Configuration::SinglePhase);
    assert_eq!(pair.terminal_map, ["3", "1"]);
    assert_eq!(pair.p_nom, [12000.0]);
    assert!(matches!(
        pair.voltage_model,
        DistLoadVoltageModel::ConstantImpedance { .. }
    ));
    near(pair.voltage_model.v_nom()[0], 433.0);
}

#[test]
fn unequal_phase_and_delta_powers_keep_branch_order() {
    let wye = input("UPDATE Load SET Flag_Lf=13,Flag_LoadType=1")
        .lower_static(&bus(), 400.0, Some("4"))
        .unwrap();
    assert_eq!(wye.p_nom, [2000.0, 4000.0, 6000.0]);
    assert_eq!(wye.q_nom, [50.0, 100.0, 150.0]);
    let delta = input("UPDATE Load SET Flag_Lf=14,Flag_LoadType=1")
        .lower_static(&bus(), 400.0, None)
        .unwrap();
    assert_eq!(delta.configuration, Configuration::Delta);
    assert_eq!(delta.terminal_map, ["1", "2", "3"]);
    assert_eq!(delta.p_nom, [8000.0, 10000.0, 12000.0]);
    assert_eq!(delta.q_nom, [-500.0, 0.0, 500.0]);
    assert_eq!(delta.voltage_model.v_nom(), [400.0; 3]);
    // At balanced voltage unequal wye powers draw nonzero return current;
    // delta branch currents circulate between phases and need no return wire.
    let mut wye_return = Complex64::new(0.0, 0.0);
    let mut delta_phase = [Complex64::new(0.0, 0.0); 3];
    let v = [
        0.0,
        -std::f64::consts::TAU / 3.0,
        std::f64::consts::TAU / 3.0,
    ]
    .map(|a| Complex64::from_polar(400.0 / 3f64.sqrt(), a));
    for i in 0..3 {
        wye_return -= Complex64::new(wye.p_nom[i], -wye.q_nom[i]) * v[i]
            / wye.voltage_model.v_nom()[i].powi(2);
        let j = (i + 1) % 3;
        let current =
            Complex64::new(delta.p_nom[i], -delta.q_nom[i]) * (v[i] - v[j]) / 400.0_f64.powi(2);
        delta_phase[i] += current;
        delta_phase[j] -= current;
    }
    assert!(wye_return.norm() > 10.0);
    assert!(delta_phase.iter().sum::<Complex64>().norm() < 1e-12);
}

#[test]
fn aggregate_delta_mode_does_not_turn_into_grounded_wye() {
    let decoded = input("UPDATE Load SET Flag_Lf=15");
    assert_eq!(
        decoded.power,
        PowerInput::DeltaTotal {
            p: 12000.0,
            q: 1000.0
        }
    );
    let delta = decoded.lower_static(&bus(), 400.0, None).unwrap();
    assert_eq!(delta.configuration, Configuration::Delta);
    assert_eq!(delta.p_nom, [4000.0; 3]);
    assert_eq!(delta.terminal_map, ["1", "2", "3"]);
    assert_eq!(delta.voltage_model.v_nom(), [400.0; 3]);
    let pair = input("UPDATE Load SET Flag_Lf=15; UPDATE Terminal SET Flag_Terminal=4")
        .lower_static(&bus(), 400.0, None)
        .unwrap();
    assert_eq!(pair.terminal_map, ["1", "2"]);
    assert_eq!(pair.p_nom, [12000.0]);
    assert!(
        input("UPDATE Load SET Flag_Lf=15; UPDATE Terminal SET Flag_Terminal=1")
            .lower_static(&bus(), 400.0, None)
            .is_err()
    );
}

#[test]
fn static_load_refuses_unresolved_physics_and_does_not_change_bus() {
    for edit in [
        "UPDATE Load SET DayOpSer_ID=1",
        "UPDATE Load SET WeekOpSer_ID=2",
        "UPDATE Load SET Stp_ID=3",
        "UPDATE Element SET Flag_State=0",
        "UPDATE Terminal SET Flag_State=0",
        "UPDATE Element SET Flag_Input=2",
        "UPDATE Load SET Flag_Z0_Input=1",
        "UPDATE Load SET Flag_Z0_Input=2",
        "UPDATE Element SET Flag_Input=14; UPDATE Load SET Pneg=0.001",
        "UPDATE Load SET Flag_Lf=13; UPDATE Terminal SET Flag_Terminal=1",
        "UPDATE Load SET Flag_Lf=14; UPDATE Terminal SET Flag_Terminal=4",
    ] {
        let b = bus();
        let original = b.clone();
        assert!(
            input(edit).lower_static(&b, 400.0, Some("4")).is_err(),
            "accepted {edit}"
        );
        assert_eq!(b, original);
    }
    let decoded = input("");
    assert!(decoded.lower_static(&bus(), 400.0, None).is_err());
    let mut b = bus();
    b.grounded.clear();
    assert!(decoded.lower_static(&b, 400.0, Some("4")).is_err());
    b = bus();
    b.id = "20".into();
    assert!(decoded.lower_static(&b, 400.0, Some("4")).is_err());
    b = bus();
    b.terminals.retain(|t| t != "2");
    assert!(decoded.lower_static(&b, 400.0, Some("4")).is_err());
    b = bus();
    b.grounded.push("1".into());
    assert!(decoded.lower_static(&b, 400.0, Some("1")).is_err());
    for voltage in [0.0, -1.0, f64::INFINITY] {
        assert!(decoded.lower_static(&bus(), voltage, Some("4")).is_err());
    }
    assert!(
        input("UPDATE Load SET u=200")
            .lower_static(&bus(), f64::MAX, Some("4"))
            .is_err()
    );
    // A floating native neutral remains distinct from the caller's earth node.
    b = bus();
    b.terminals.push("n".into());
    assert!(decoded.lower_static(&b, 400.0, Some("n")).is_err());
    assert!(decoded.lower_static(&b, 400.0, Some("4")).is_ok());
    assert!(!b.grounded.contains(&"n".to_owned()));
}

#[test]
fn load_sequence_inputs_read_only_declared_categories() {
    let missing = input(
        "UPDATE Element SET Flag_Input=2; UPDATE Load SET Flag_Z0_Input=NULL,Pneg=NULL,Qneg=NULL",
    );
    assert_eq!(missing.zero_sequence, LoadZeroSequence::NotDeclared);
    assert_eq!(missing.negative_sequence_power, None);
    let ratio = input("UPDATE Load SET Flag_Z0_Input=1,R0=NULL,X0=NULL");
    assert_eq!(
        ratio.zero_sequence,
        LoadZeroSequence::MagnitudeRatio {
            z0_over_z1: 2.0,
            r0_over_x0: 0.5
        }
    );
    let direct = input("UPDATE Load SET Flag_Z0_Input=2,Z0_Z1=NULL,R0_X0=NULL");
    assert_eq!(
        direct.zero_sequence,
        LoadZeroSequence::DirectOhms(Complex64::new(0.2, 0.4))
    );
    let same = input("UPDATE Load SET Flag_Z0_Input=3,R0=NULL,X0=NULL,Z0_Z1=NULL,R0_X0=NULL");
    assert_eq!(same.zero_sequence, LoadZeroSequence::SameAsPositive);
    let negative =
        input("UPDATE Element SET Flag_Input=14; UPDATE Load SET Pneg=0.001,Qneg=-0.002");
    assert_eq!(
        negative.negative_sequence_power,
        Some(Complex64::new(1000.0, -2000.0))
    );
}

#[test]
fn load_projection_agrees_with_dss_and_survives_pmd_emission() {
    for (native_mode, dss_model) in [(1, 2), (2, 1), (3, 5)] {
        let mapped = input(&format!("UPDATE Load SET Flag_LoadType={native_mode}"))
            .lower_static(&bus(), 400.0, Some("4"))
            .unwrap();
        let other = crate::testkit::parse_str(
            &format!(
                "New Load.l bus1=10.1.2.3.0 phases=3 conn=wye kV=0.4 kW=12 kvar=1 model={dss_model}"
            ),
            "dss",
        )
        .unwrap();
        let dss = &other.loads()[0];
        assert_eq!(mapped.configuration, dss.configuration);
        assert_eq!(mapped.terminal_map, dss.terminal_map);
        assert_eq!(
            std::mem::discriminant(&mapped.voltage_model),
            std::mem::discriminant(&dss.voltage_model)
        );
        for (actual, expected) in mapped
            .p_nom
            .iter()
            .chain(&mapped.q_nom)
            .chain(mapped.voltage_model.v_nom())
            .zip(
                dss.p_nom
                    .iter()
                    .chain(&dss.q_nom)
                    .chain(dss.voltage_model.v_nom()),
            )
        {
            near(*actual, *expected);
        }
    }
    for mode in [13, 14, 15] {
        let b = bus();
        let mapped = input(&format!("UPDATE Load SET Flag_LoadType=1,Flag_Lf={mode}"))
            .lower_static(&b, 400.0, Some("4"))
            .unwrap();
        let mut net = MulticonductorNetwork::new();
        net.buses_mut().push(b);
        net.loads_mut().push(mapped.clone());
        let emitted = crate::convert::emit_value_text(&net, crate::DistTargetFormat::PmdJson);
        assert!(emitted.diagnostics.is_empty(), "{:?}", emitted.diagnostics);
        let parsed = crate::testkit::parse_str(&emitted.text, "pmd-json").unwrap();
        let actual = &parsed.loads()[0];
        assert_eq!(actual.configuration, mapped.configuration);
        assert_eq!(actual.terminal_map, mapped.terminal_map);
        assert_eq!(actual.p_nom, mapped.p_nom);
        for (q, expected) in actual.q_nom.iter().zip(&mapped.q_nom) {
            near(*q, *expected);
        }
        for (v, expected) in actual
            .voltage_model
            .v_nom()
            .iter()
            .zip(mapped.voltage_model.v_nom())
        {
            near(*v, *expected);
        }
        assert!(matches!(
            actual.voltage_model,
            DistLoadVoltageModel::ConstantImpedance { .. }
        ));
    }
}

#[test]
fn load_sequence_inputs_reject_invalid_active_values() {
    for edit in [
        "UPDATE Load SET Flag_Z0_Input=4",
        "UPDATE Load SET Flag_Z0_Input=1,Z0_Z1=-1",
        "UPDATE Load SET Flag_Z0_Input=1,R0_X0=NULL",
        "UPDATE Load SET Flag_Z0_Input=2,R0=-1",
        "UPDATE Load SET Flag_Z0_Input=2,X0=NULL",
        "UPDATE Load SET Flag_Z0_Input=2,X0=1e999",
        "UPDATE Element SET Flag_Input=14; UPDATE Load SET Pneg=NULL",
        "UPDATE Element SET Flag_Input=14; UPDATE Load SET Pneg=1e308",
    ] {
        assert!(native(edit).load_input(30).is_err(), "accepted {edit}");
    }
}

#[test]
fn undeclared_zero_sequence_does_not_invent_a_star_for_phase_to_phase_loads() {
    for code in [4, 5, 6, 7] {
        for mode in [1, 15] {
            if code == 7 && mode == 1 {
                continue;
            }
            for model in 1..=3 {
                let decoded = input(&format!(
                    "UPDATE Element SET Flag_Input=2; UPDATE Terminal SET Flag_Terminal={code}; UPDATE Load SET Flag_Lf={mode},Flag_LoadType={model},Flag_Z0_Input=NULL,R0=NULL,X0=NULL,Z0_Z1=NULL,R0_X0=NULL"
                ));
                let circuit = decoded.circuit(&bus(), 400.0).unwrap();
                assert_eq!(circuit.bus.grounded, Vec::<String>::new());
                assert!(!circuit.load.terminal_map.contains(&"0".into()));
                assert_eq!(
                    circuit.load.terminal_map.len(),
                    if code == 7 { 3 } else { 2 }
                );
                assert_eq!(circuit.load.p_nom.len(), if code == 7 { 3 } else { 1 });
            }
        }
    }
    assert!(
        input("UPDATE Element SET Flag_Input=2; UPDATE Terminal SET Flag_Terminal=7")
            .circuit(&bus(), 400.0)
            .is_err()
    );
    for edit in [
        "UPDATE Load SET Stp_ID=1",
        "UPDATE Load SET Flag_Z0_Input=2",
    ] {
        assert!(
            input(&format!("UPDATE Terminal SET Flag_Terminal=4; {edit}"))
                .circuit(&bus(), 400.0)
                .is_err()
        );
    }
}

#[test]
#[ignore = "exports authentic CSIRO06 phase-pair load components for independent validation; not a complete network"]
fn export_csiro_phase_pair_loads() {
    let path = std::env::var("POWERIO_SINCAL_LOAD_RECORDS").expect("explicit records path");
    let output = std::env::var("POWERIO_SINCAL_LOAD_EXPORT").expect("explicit export path");
    let bytes = std::fs::read(path).unwrap();
    let db = NativeDatabase::from_snapshot(
        powerio_sincal::DatabaseSnapshot::decode_records(&bytes, Some(1)).unwrap(),
    )
    .unwrap();
    let nodes = db.node_inputs().unwrap();
    let mut components = Vec::new();
    for (&id, kind) in &db.elements {
        if kind != "Load" {
            continue;
        }
        let input = db.load_input(id).unwrap();
        if input.terminal.connection.phases().unwrap().len() != 2 {
            continue;
        }
        let node = &nodes[&input.terminal.node];
        let bus = DistBus::new(
            node.id.to_string(),
            ["1", "2", "3"].map(str::to_owned).to_vec(),
        );
        let circuit = input.circuit(&bus, node.nominal_ll_volts).unwrap();
        components.push(serde_json::json!({"element":id,"load":circuit.load,"bus":circuit.bus,"switch":circuit.switch}));
    }
    assert_eq!(components.len(), 18);
    let export = serde_json::json!({"scope":"component mapping only; not a complete parsed network","components":components});
    std::fs::write(output, serde_json::to_vec_pretty(&export).unwrap()).unwrap();
}

#[test]
fn undeclared_sequence_single_phase_load_uses_explicit_earth_without_grounding_neutral() {
    for code in 1..=3 {
        for model in 1..=3 {
            let edit = format!(
                "UPDATE Element SET Flag_Input=2; UPDATE Terminal SET Flag_Terminal={code}; UPDATE Load SET Flag_LoadType={model},Flag_Z0_Input=NULL,R0=NULL,X0=NULL"
            );
            let native_bus = DistBus::new("10", ["3", "n", "1", "2"].map(str::to_owned).to_vec());
            let original = native_bus.clone();
            let mapped = input(&edit).circuit(&native_bus, 400.0).unwrap();
            assert_eq!(native_bus, original);
            assert_eq!(mapped.bus.terminals, [code.to_string(), "0".into()]);
            assert_eq!(mapped.bus.grounded, ["0"]);
            assert_eq!(mapped.load.terminal_map, mapped.bus.terminals);
            assert_eq!(mapped.load.configuration, Configuration::SinglePhase);
            assert_eq!(mapped.switch.terminal_map_from, [code.to_string()]);
            assert_eq!(mapped.switch.terminal_map_to, [code.to_string()]);
            near(mapped.load.p_nom[0], 12000.0);
            near(mapped.load.q_nom[0], 1000.0);
            near(mapped.load.voltage_model.v_nom()[0], 400.0 / 3.0_f64.sqrt());
            let open = input(&format!("{edit}; UPDATE Terminal SET Flag_State=0"))
                .circuit(&native_bus, 400.0)
                .unwrap();
            assert!(open.switch.open);
            assert_eq!(open.load, mapped.load);
            for invalid in [
                "UPDATE Load SET Stp_ID=1",
                "UPDATE Element SET Flag_Input=6; UPDATE Load SET Flag_Z0_Input=1",
                "UPDATE Element SET Flag_Input=6; UPDATE Load SET Flag_Z0_Input=2,R0=1,X0=1",
                "UPDATE Element SET Flag_Input=10; UPDATE Load SET Pneg=0.001",
            ] {
                assert!(
                    input(&format!("{edit}; {invalid}"))
                        .circuit(&native_bus, 400.0)
                        .is_err()
                );
            }
            let mut net = MulticonductorNetwork::new();
            net.buses_mut().extend([native_bus, mapped.bus]);
            net.loads_mut().push(mapped.load);
            net.switches_mut().push(mapped.switch);
            crate::require_electrical_readiness(&net).unwrap();
            let emitted = crate::convert::emit_value_text(&net, crate::DistTargetFormat::PmdJson);
            let restored = crate::testkit::parse_str(&emitted.text, "pmd-json").unwrap();
            assert_eq!(
                restored.loads()[0].terminal_map,
                net.loads()[0].terminal_map
            );
            assert_eq!(
                restored.loads()[0].voltage_model,
                net.loads()[0].voltage_model
            );
            assert_eq!(restored.loads()[0].p_nom, net.loads()[0].p_nom);
            assert_eq!(restored.buses()[0].grounded, net.buses()[0].grounded);
            crate::require_electrical_readiness(&restored).unwrap();
        }
    }
}
