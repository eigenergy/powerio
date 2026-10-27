mod helpers;
use helpers::{deserialize_module_text, serialize_module_text};
use powerio::{PioModule, PioValue, Source};
use powerio_dist::DistLoadVoltageModel;

#[test]
fn bounded_load_law_survives_ir_in_network_and_calculation() {
    for (model, tag) in [
        (5, "bounded_zip"),
        (3, "bounded_exponential"),
        (4, "bounded_exponential"),
    ] {
        let source = Source::from_memory("load.dss", format!("New Circuit.c phases=1 bus1=b.1 basekv=.24\nNew Load.l phases=1 bus1=b.1.0 kv=.24 kw=1 kvar=.5 model={model} vminpu=.9 vmaxpu=1.1 vlowpu=.4\n").into_bytes()).unwrap();
        let module = powerio_dist::parse(source).unwrap();
        let net = module.value();
        for value in [
            PioValue::from(net.clone()),
            PioValue::from(powerio_prob::McAcPfInstance::from_network(net.clone()).unwrap()),
        ] {
            let text = serialize_module_text(&PioModule::new(value)).unwrap();
            assert!(text.contains(tag));
            let back = deserialize_module_text(&text).unwrap();
            let again = serialize_module_text(&back).unwrap();
            let first: serde_json::Value = serde_json::from_str(&text).unwrap();
            let second: serde_json::Value = serde_json::from_str(&again).unwrap();
            assert_eq!(first["value"], second["value"]);
        }
    }
}

#[test]
#[ignore = "requires pinned native samples from evals/gridlabd/load_voltage_oracle.py"]
fn typed_laws_match_native_load_currents() {
    let path = std::env::var("POWERIO_LOAD_VOLTAGE_SAMPLES").unwrap();
    let data: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    for sample in data["cases"].as_array().unwrap() {
        let module = deserialize_module_text(
            &std::fs::read_to_string(sample["ir"].as_str().unwrap()).unwrap(),
        )
        .unwrap();
        let PioValue::MulticonductorNetwork(net) = module.value() else {
            panic!("expected network")
        };
        let load = &net.loads()[0];
        assert!(matches!(
            load.voltage_model,
            DistLoadVoltageModel::BoundedExponential { .. }
                | DistLoadVoltageModel::BoundedZip { .. }
                | DistLoadVoltageModel::ConstantImpedance { .. }
        ));
        let complex = |v: &serde_json::Value| {
            num_complex::Complex64::new(v[0].as_f64().unwrap(), v[1].as_f64().unwrap())
        };
        let voltage = complex(&sample["original"]["voltage"]);
        let current = complex(&sample["original"]["current"]);
        let [active, reactive] = load.voltage_model.power_scale(0, voltage.norm()).unwrap();
        let expected =
            (num_complex::Complex64::new(load.p_nom[0] * active, load.q_nom[0] * reactive)
                / voltage)
                .conj();
        assert!(
            (current - expected).norm() <= 1e-9 + 1e-9 * current.norm(),
            "{}: native {current:?}, typed {expected:?}",
            sample["id"]
        );
    }
}

#[test]
fn idle_storage_law_survives_ir_and_calculation_instances() {
    let source = Source::from_memory("storage.dss", b"New Circuit.c basekv=.48\nNew Storage.s bus1=sourcebus kv=.48 kwrated=250 kva=250 limitcurrent=yes\n".to_vec()).unwrap();
    let module = powerio_dist::parse(source).unwrap();
    for value in [
        PioValue::from(module.value().clone()),
        PioValue::from(powerio_prob::McAcPfInstance::from_network(module.value().clone()).unwrap()),
    ] {
        let text = serialize_module_text(&PioModule::new(value)).unwrap();
        assert!(text.contains("bounded_constant_power"));
        let back = deserialize_module_text(&text).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&text).unwrap()["value"],
            serde_json::from_str::<serde_json::Value>(&serialize_module_text(&back).unwrap())
                .unwrap()["value"]
        );
    }
}

#[test]
#[ignore = "requires pinned native samples from evals/gridlabd/dss_storage_oracle.py"]
fn typed_static_storage_powers_match_native_voltage_sweeps() {
    let path = std::env::var("POWERIO_STORAGE_SAMPLES").unwrap();
    let data: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    for (name, sample) in data["cases"].as_object().unwrap() {
        let sample = &sample["typed_sample"];
        let module = deserialize_module_text(
            &std::fs::read_to_string(sample["ir"].as_str().unwrap()).unwrap(),
        )
        .unwrap();
        let PioValue::MulticonductorNetwork(net) = module.value() else {
            panic!("expected network")
        };
        let load = net
            .loads()
            .iter()
            .find(|load| {
                matches!(
                    load.voltage_model,
                    DistLoadVoltageModel::BoundedConstantPower { .. }
                )
            })
            .unwrap();
        let complex = |v: &serde_json::Value| {
            num_complex::Complex64::new(v[0].as_f64().unwrap(), v[1].as_f64().unwrap())
        };
        let volts = sample["branch_voltage"].as_array().unwrap();
        assert_eq!(volts.len(), load.p_nom.len());
        let expected = volts
            .iter()
            .enumerate()
            .map(|(phase, voltage)| {
                let [p, q] = load
                    .voltage_model
                    .power_scale(phase, complex(voltage).norm())
                    .unwrap();
                num_complex::Complex64::new(p * load.p_nom[phase], q * load.q_nom[phase])
            })
            .sum::<num_complex::Complex64>();
        let actual = complex(&sample["total_power"]);
        assert!(
            (expected - actual).norm() <= 1e-7 + 1e-9 * actual.norm(),
            "{name}: typed {expected}, native {actual}"
        );
    }
}
