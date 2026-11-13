//! Versioned Sienna electrical exchange through the public module API.
mod helpers;
use helpers::{emit_module, emit_value};
use powerio_core::{FormatId, Source};
use powerio_tx::TargetFormat;

#[test]
fn export_reference_cases() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data");
    for case in ["case9", "case14", "case30", "case57", "case118"] {
        let source = Source::open(root.join(format!("{case}.m"))).unwrap();
        let module = powerio_tx::parse(source).unwrap();
        for version in 1..=4 {
            let format = TargetFormat::SiennaJson { version };
            let written = emit_value(module.value(), format).unwrap();
            let source =
                Source::from_memory(format!("{case}.json"), written.text.as_bytes().to_vec())
                    .unwrap()
                    .with_format(FormatId::new(format.token()).unwrap());
            let recovered = powerio_tx::parse(source).unwrap();
            assert_eq!(
                recovered.extensions()["org.eigenergy.powerio.sienna"],
                serde_json::from_str::<serde_json::Value>(&written.text).unwrap()
            );
            assert_eq!(
                module.value().buses().len(),
                recovered.value().buses().len()
            );
            assert_eq!(
                module.value().branches().len(),
                recovered.value().branches().len()
            );
            assert_eq!(
                module.value().generators().len(),
                recovered.value().generators().len()
            );
            assert_eq!(
                module.value().shunts().len(),
                recovered.value().shunts().len()
            );
            assert_eq!(emit_module(&recovered, format).unwrap().text, written.text);
            if let Some(dir) = std::env::var_os("POWERIO_SIENNA_OUTPUT_DIR") {
                let dir = std::path::PathBuf::from(dir).join(format!("0.{version}.0"));
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join(format!("{case}.json")), written.text).unwrap();
            }
        }
    }
}

#[test]
fn three_winding_connections_and_pair_bases_survive() {
    use powerio_tx::network::{BusId, Impedance, MagnetizingConnection, Transformer3W, Winding};
    let source = Source::open(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data/case9.m"),
    )
    .unwrap();
    let module = powerio_tx::parse(source).unwrap();
    for placement in [
        MagnetizingConnection::StarPoint,
        MagnetizingConnection::Winding1Terminal,
    ] {
        let mut net = module.value().clone();
        let mut t = Transformer3W::new(
            [
                Winding::new(BusId(1)),
                Winding::new(BusId(2)),
                Winding::new(BusId(3)),
            ],
            [
                Impedance::new(0.01, 0.1, 50.0),
                Impedance::new(0.02, 0.2, 75.0),
                Impedance::new(0.03, 0.3, 80.0),
            ],
        );
        t.mag_g = 0.001;
        t.mag_b = -0.02;
        t.magnetizing_connection = placement;
        t.windings[0].tap = 1.08;
        t.windings[1].shift = 12.0;
        let mut control = powerio_tx::network::TransformerControl::new(
            powerio_tx::network::TransformerControlMode::Voltage,
        );
        control.controlled_bus = Some(BusId(3));
        control.mva_base = 100.0;
        t.windings[0].control = Some(control);
        t.star_vm = 0.98;
        t.star_va = -3.0;
        net.transformers_3w_mut().push(t.clone());
        for version in 0..=4 {
            let format = if version == 0 {
                TargetFormat::PowerSystemsJson
            } else {
                TargetFormat::SiennaJson { version }
            };
            let out = emit_value(&net, format).unwrap();
            save_output(version, &format!("three_winding_{placement:?}"), &out.text);
            let recovered =
                powerio_tx::parse(Source::from_memory("case.json", out.text.into_bytes()).unwrap())
                    .unwrap();
            let actual = &recovered.value().transformers_3w()[0];
            let separate_shunt =
                version == 0 && placement == MagnetizingConnection::Winding1Terminal;
            assert_eq!(
                actual.magnetizing_connection,
                if separate_shunt {
                    MagnetizingConnection::StarPoint
                } else {
                    placement
                }
            );
            for (a, b) in actual.windings.iter().zip(&t.windings) {
                assert_eq!(a.bus, b.bus);
                assert!((a.tap - b.tap).abs() < 1e-12);
                assert!((a.shift - b.shift).abs() < 1e-12);
                assert_eq!(a.control, b.control);
            }
            if separate_shunt {
                assert_eq!(actual.mag_g, 0.0);
                assert_eq!(actual.mag_b, 0.0);
                let shunt = recovered.value().shunts().last().unwrap();
                assert_eq!(shunt.bus, t.windings[0].bus);
                assert!((shunt.g - t.mag_g * net.base_mva()).abs() < 1e-12);
                assert!((shunt.b - t.mag_b * net.base_mva()).abs() < 1e-12);
            } else {
                assert!((actual.mag_g - t.mag_g).abs() < 1e-12);
                assert!((actual.mag_b - t.mag_b).abs() < 1e-12);
            }
            assert_eq!(recovered.value().buses().len(), net.buses().len());
            for (a, b) in actual.z.iter().zip(t.z) {
                assert!((a.r - b.r).abs() < 1e-12);
                assert!((a.x - b.x).abs() < 1e-12);
                assert!((a.base_mva - b.base_mva).abs() < 1e-12);
            }
        }
    }
}

fn save_output(version: u8, name: &str, text: &str) {
    if let Some(dir) = std::env::var_os("POWERIO_SIENNA_OUTPUT_DIR") {
        let dir = std::path::PathBuf::from(dir).join(if version == 0 {
            "powersystems-5".to_owned()
        } else {
            format!("0.{version}.0")
        });
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{name}.json")), text).unwrap();
    }
}

#[test]
fn transformer_control_modes_and_generation_types() {
    use powerio_tx::network::{
        GeneratorEnergySource as Energy, TransformerControl, TransformerControlMode as Mode,
    };
    let source = Source::open(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data/case14.m"),
    )
    .unwrap();
    let module = powerio_tx::parse(source).unwrap();
    for mode in [
        Mode::Fixed,
        Mode::Voltage,
        Mode::ReactiveFlow,
        Mode::ActiveFlow,
        Mode::DcLineQuantity,
        Mode::AsymmetricActiveFlow,
    ] {
        for enabled in [false, true] {
            if mode == Mode::Fixed && enabled {
                continue;
            }
            let mut net = module.value().clone();
            let branch = net
                .branches_mut()
                .iter_mut()
                .find(|b| b.is_transformer())
                .unwrap();
            let mut control = TransformerControl::new(mode);
            control.enabled = enabled;
            control.mva_base = 100.0;
            control.controlled_bus = Some(powerio_tx::network::BusId(4));
            control.controlled_bus_on_winding_side = true;
            control.ntp = 17;
            branch.control = Some(control.clone());
            net.generators_mut()[0].energy_source = Energy::Wind;
            net.generators_mut()[1].energy_source = Energy::Solar;
            net.generators_mut()[2].energy_source = Energy::Hydro;
            net.generators_mut()[3].energy_source = Energy::Nuclear;
            for version in 1..=4 {
                let out = emit_value(&net, TargetFormat::SiennaJson { version }).unwrap();
                save_output(version, &format!("control_{mode:?}_{enabled}"), &out.text);
                let recovered = powerio_tx::parse(
                    Source::from_memory("case.json", out.text.into_bytes()).unwrap(),
                )
                .unwrap();
                let actual = recovered
                    .value()
                    .branches()
                    .iter()
                    .find_map(|b| b.control.as_ref())
                    .unwrap();
                assert_eq!(actual.mode, control.mode);
                assert_eq!(actual.enabled, control.enabled);
                assert_eq!(actual.controlled_bus, control.controlled_bus);
                assert_eq!(
                    actual.controlled_bus_on_winding_side,
                    control.controlled_bus_on_winding_side
                );
                assert!((actual.tap_min - control.tap_min).abs() < 1e-12);
                assert!((actual.band_max - control.band_max).abs() < 1e-12);
                let energies: Vec<_> = recovered
                    .value()
                    .generators()
                    .iter()
                    .map(|g| g.energy_source)
                    .collect();
                for energy in [Energy::Wind, Energy::Solar, Energy::Hydro, Energy::Nuclear] {
                    assert!(energies.contains(&energy));
                }
            }
        }
    }
}

#[test]
fn powersystems_native_reference_cases() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data");
    for case in ["case9", "case14", "case30", "case57", "case118"] {
        let module =
            powerio_tx::parse(Source::open(root.join(format!("{case}.m"))).unwrap()).unwrap();
        let out = emit_value(module.value(), TargetFormat::PowerSystemsJson).unwrap();
        let recovered = powerio_tx::parse(
            Source::from_memory("system.json", out.text.as_bytes().to_vec()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            recovered.extensions()["org.eigenergy.powerio.powersystems"],
            serde_json::from_str::<serde_json::Value>(&out.text).unwrap()
        );
        assert_eq!(
            recovered.value().buses().len(),
            module.value().buses().len()
        );
        assert_eq!(
            recovered.value().branches().len(),
            module.value().branches().len()
        );
        assert_eq!(
            recovered.value().generators().len(),
            module.value().generators().len()
        );
        assert_eq!(
            recovered.value().loads().len(),
            module.value().loads().len()
        );
        assert_eq!(
            recovered.value().shunts().len(),
            module.value().shunts().len()
        );
        if let Some(dir) = std::env::var_os("POWERIO_SIENNA_OUTPUT_DIR") {
            let dir = std::path::PathBuf::from(dir).join("powersystems-5");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(format!("{case}.json")), out.text).unwrap();
        }
    }
}

#[test]
fn switched_shunt_controls_and_native_edits_survive() {
    use powerio_tx::network::{BusId, Shunt, ShuntBlock, SwitchedShuntControl, SwitchedShuntMode};
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data/case9.m");
    let module = powerio_tx::parse(Source::open(root).unwrap()).unwrap();
    let mut net = module.value().clone();
    let mut shunt = Shunt::new(BusId(4), 0.0, 12.5);
    let mut control = SwitchedShuntControl::new(
        SwitchedShuntMode::Discrete,
        1.04,
        0.97,
        vec![ShuntBlock::new(3, 5.0), ShuntBlock::new(2, -2.5)],
    );
    control.control_bus = Some(BusId(5));
    shunt.control = Some(control.clone());
    net.shunts_mut().push(shunt);
    for version in 0..=4 {
        let format = if version == 0 {
            TargetFormat::PowerSystemsJson
        } else {
            TargetFormat::SiennaJson { version }
        };
        let out = emit_value(&net, format).unwrap();
        save_output(version, "switched_shunt", &out.text);
        let actual = powerio_tx::parse(
            Source::from_memory("shunt.json", out.text.as_bytes().to_vec()).unwrap(),
        )
        .unwrap();
        let sh = actual.value().shunts().last().unwrap();
        assert_eq!(sh.control.as_ref().unwrap(), &control);
        assert!((sh.b - 12.5).abs() < 1e-12);
        if version == 0 {
            let mut legacy: serde_json::Value = serde_json::from_str(&out.text).unwrap();
            let row = legacy["data"]["components"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|r| r["__metadata__"]["type"] == "SwitchedAdmittance")
                .unwrap();
            row.as_object_mut().unwrap().remove("control_mode");
            row.as_object_mut().unwrap().remove("regulated_bus_number");
            let read = powerio_tx::parse(
                Source::from_memory("legacy.json", legacy.to_string().into_bytes()).unwrap(),
            )
            .unwrap();
            assert_eq!(
                read.value()
                    .shunts()
                    .last()
                    .unwrap()
                    .control
                    .as_ref()
                    .unwrap(),
                &control
            );
            let row = legacy["data"]["components"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|r| r["__metadata__"]["type"] == "SwitchedAdmittance")
                .unwrap();
            row["control_mode"] = serde_json::json!("FIXED");
            row["regulated_bus_number"] = serde_json::json!(6);
            let read = powerio_tx::parse(
                Source::from_memory("edited.json", legacy.to_string().into_bytes()).unwrap(),
            )
            .unwrap();
            let actual = read
                .value()
                .shunts()
                .last()
                .unwrap()
                .control
                .as_ref()
                .unwrap();
            assert_eq!(actual.mode, SwitchedShuntMode::Locked);
            assert_eq!(actual.control_bus, Some(BusId(6)));
        } else {
            let mut edited: serde_json::Value = serde_json::from_str(&out.text).unwrap();
            edited["components"]["SwitchedAdmittance"][0]["solved_admittance"] =
                serde_json::json!(17.5);
            let read = powerio_tx::parse(
                Source::from_memory("edited.json", edited.to_string().into_bytes()).unwrap(),
            )
            .unwrap();
            assert!((read.value().shunts().last().unwrap().b - 17.5).abs() < 1e-12);
        }
    }
}

#[test]
fn storage_energy_and_cost_coefficients_keep_their_units() {
    use powerio_tx::network::{BusId, Storage};
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data/case9.m");
    let module = powerio_tx::parse(Source::open(path).unwrap()).unwrap();
    let mut net = module.value().clone();
    let mut storage = Storage::new(BusId(4));
    storage.energy_rating = 300.0;
    storage.energy = 75.0;
    storage.ps = 30.0;
    storage.qs = -4.0;
    storage.thermal_rating = 60.0;
    storage.charge_rating = 50.0;
    storage.discharge_rating = 55.0;
    storage.charge_efficiency = 0.9;
    storage.discharge_efficiency = 0.92;
    net.storage_mut().push(storage);
    for version in 0..=4 {
        let format = if version == 0 {
            TargetFormat::PowerSystemsJson
        } else {
            TargetFormat::SiennaJson { version }
        };
        let out = emit_value(&net, format).unwrap();
        save_output(version, "storage_costs", &out.text);
        let read =
            powerio_tx::parse(Source::from_memory("storage.json", out.text.into_bytes()).unwrap())
                .unwrap();
        let actual = &read.value().storage()[0];
        assert!((actual.energy_rating - 300.0).abs() < 1e-12);
        assert!((actual.energy - 75.0).abs() < 1e-12);
        assert!((actual.ps - 30.0).abs() < 1e-12);
        for (a, e) in read.value().generators().iter().zip(net.generators()) {
            assert_eq!(a.cost, e.cost);
        }
    }
}

/// A transformer's secondary-terminal charging is written as a marked
/// `FixedAdmittance`. The marker folds it back only while the shunt and the
/// transformer share one status; a native edit that disables the generated
/// shunt leaves a disabled bus shunt and an uncharged branch, in Sienna 0.4
/// and in PowerSystems JSON.
#[test]
fn a_disabled_secondary_shunt_stops_charging_its_transformer() {
    use powerio_tx::network::BranchCharging;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data/case9.m");
    let module = powerio_tx::parse(Source::open(root).unwrap()).unwrap();
    let mut net = module.value().clone();
    let branch = &mut net.branches_mut()[0];
    branch.tap = 1.02;
    branch.charging = Some(BranchCharging::new(0.0, 0.0, 0.0, 0.02));
    branch.b = 0.02;
    let (from, to) = (branch.from, branch.to);
    let shunt_count = net.shunts().len();
    for version in [0, 4] {
        let format = if version == 0 {
            TargetFormat::PowerSystemsJson
        } else {
            TargetFormat::SiennaJson { version }
        };
        let out = emit_value(&net, format).unwrap();
        let parse = |text: &str| {
            powerio_tx::parse(Source::from_memory("edited.json", text.as_bytes().to_vec()).unwrap())
                .unwrap()
        };
        let find = |net: &powerio_tx::BalancedNetwork| {
            net.branches()
                .iter()
                .find(|b| b.from == from && b.to == to)
                .unwrap()
                .clone()
        };
        // Unedited, the marker folds the shunt back into the branch.
        let same = parse(&out.text);
        let charging = find(same.value()).charging.unwrap();
        assert!((charging.b_to - 0.02).abs() < 1e-12);
        assert_eq!(same.value().shunts().len(), shunt_count);

        let mut edited: serde_json::Value = serde_json::from_str(&out.text).unwrap();
        let generated = if version == 0 {
            edited["data"]["components"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|r| r["__metadata__"]["type"] == "FixedAdmittance")
                .unwrap()
        } else {
            edited["components"]["FixedAdmittance"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|r| r["name"].as_str().unwrap().ends_with("_secondary_shunt"))
                .unwrap()
        };
        assert_eq!(generated["available"], serde_json::json!(true));
        generated["available"] = serde_json::json!(false);
        let read = parse(&edited.to_string());
        let branch = find(read.value());
        assert!(branch.in_service);
        assert_eq!(branch.charging.map_or(0.0, |c| c.b_to), 0.0);
        assert_eq!(branch.b, 0.0);
        let shunts = read.value().shunts();
        assert_eq!(shunts.len(), shunt_count + 1);
        let shunt = shunts.iter().find(|s| s.bus == to).unwrap();
        assert!(!shunt.in_service);
        assert!(
            (shunt.b - 0.02 * net.base_mva()).abs() < 1e-9,
            "{}",
            shunt.b
        );
    }
}
