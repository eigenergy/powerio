#![allow(clippy::float_cmp)]
mod helpers;
use helpers::emit_value;
use powerio_core::Source;
use powerio_dist::{DistLoadVoltageModel, DistTargetFormat};

fn parse(text: &str) -> powerio_core::PioModule<powerio_dist::MulticonductorNetwork> {
    powerio_dist::parse(Source::from_memory("load.dss", text.as_bytes().to_vec()).unwrap()).unwrap()
}

#[test]
fn native_limits_are_typed_and_edits_override_source_extras() {
    let mut module = parse(
        "New Circuit.c phases=1 basekv=0.24\nNew Load.l phases=1 bus1=b.1.0 kv=0.24 kw=1 kvar=0.5 model=1\n",
    );
    let load = &mut module.value_mut().loads_mut()[0];
    let DistLoadVoltageModel::BoundedZip {
        v_low_pu,
        v_min_pu,
        v_max_pu,
        ..
    } = &mut load.voltage_model
    else {
        panic!("missing typed voltage law")
    };
    assert_eq!((*v_low_pu, *v_min_pu, *v_max_pu), (0.5, 0.95, 1.05));
    *v_min_pu = 0.9;
    *v_max_pu = 1.1;
    load.extras.insert("vminpu".into(), 0.8.into());
    let emitted = emit_value(module.value(), DistTargetFormat::Dss);
    let back = parse(&emitted.text);
    assert_eq!(
        back.value().loads()[0].voltage_model,
        module.value().loads()[0].voltage_model
    );
    let law = &back.value().loads()[0].voltage_model;
    assert_eq!(law.power_scale(0, 120.).unwrap(), [0.25, 0.25]);
    assert_eq!(law.power_scale(0, 240.).unwrap(), [1., 1.]);
    assert!(law.power_scale(0, -1.).is_err());
    assert!(law.power_scale(1, 240.).is_err());
}

#[test]
fn dropout_is_typed_and_applies_only_above_the_low_voltage_threshold() {
    let module = parse(
        "New Circuit.c phases=1 basekv=0.24\nNew Load.l phases=1 bus1=b.1.0 kv=0.24 kw=1 kvar=0.5 model=8 zipv=[0 0 1 0 0 1 0.8]\n",
    );
    let law = &module.value().loads()[0].voltage_model;
    assert_eq!(law.power_scale(0, 120.).unwrap(), [0.25, 0.25]);
    assert!(law.power_scale(0, 168.).unwrap()[0] < 1e-12);
    let emitted = emit_value(module.value(), DistTargetFormat::Dss);
    assert!(emitted.text.contains("model=8"));
    assert_eq!(parse(&emitted.text).value().loads()[0].voltage_model, *law);
}

#[test]
fn bounded_laws_report_loss_when_projected_to_nominal_json_models() {
    let module = parse(
        "New Circuit.c phases=1 basekv=0.24\nNew Load.l phases=1 bus1=b.1.0 kv=0.24 kw=1 kvar=0.5 model=5\n",
    );
    for target in [DistTargetFormat::BmopfJson, DistTargetFormat::PmdJson] {
        let emitted = emit_value(module.value(), target);
        assert!(
            emitted
                .warnings
                .iter()
                .any(|s| s.contains("voltage-band transitions"))
        );
    }
}
