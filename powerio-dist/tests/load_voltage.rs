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

#[test]
fn per_phase_voltage_laws_and_generated_names_survive_canonical_output() {
    let mut module = parse(
        "New Circuit.c phases=3 bus1=b.1.2.3 basekv=.416\n\
         New Load.bank phases=3 bus1=b.1.2.3.0 conn=wye kv=.416 kw=3 kvar=1.5 model=8 zipv=[.2 .3 .5 .1 .4 .5 .8]\n\
         New Load.BANK_1 phases=1 bus1=b.1.0 kv=.24 kw=.1 kvar=0 model=2\n",
    );
    let DistLoadVoltageModel::BoundedZip {
        v_nom,
        alpha_z,
        alpha_p,
        ..
    } = &mut module.value_mut().loads_mut()[0].voltage_model
    else {
        panic!()
    };
    *v_nom = vec![240., 250., 260.];
    *alpha_z = vec![0.2, 0.3, 0.4];
    *alpha_p = vec![0.5, 0.4, 0.3];
    let expected = module.value().loads()[0].clone();
    let emitted = emit_value(module.value(), DistTargetFormat::Dss);
    let back = parse(&emitted.text);
    assert_eq!(back.value().loads().len(), 4, "{}", emitted.text);
    assert!(
        emitted.text.contains("New Load.bank_1_2 "),
        "{}",
        emitted.text
    );
    for (phase, terminal) in ["1", "2", "3"].iter().enumerate() {
        let load = back
            .value()
            .loads()
            .iter()
            .find(|l| l.name != "bank_1" && l.terminal_map[0] == *terminal)
            .unwrap();
        assert_eq!(load.p_nom, [expected.p_nom[phase]]);
        assert_eq!(load.q_nom, [expected.q_nom[phase]]);
        assert_eq!(
            load.voltage_model.v_nom(),
            [expected.voltage_model.v_nom()[phase]]
        );
        for voltage in [90., 180., 235., 260., 310.] {
            assert_eq!(
                load.voltage_model.power_scale(0, voltage).unwrap(),
                expected.voltage_model.power_scale(phase, voltage).unwrap()
            );
        }
    }
}

#[test]
fn malformed_phase_arrays_cannot_be_silently_truncated() {
    let mut module = parse(
        "New Circuit.c phases=3 basekv=.416\n\
         New Load.l phases=3 bus1=b.1.2.3.0 kv=.416 kw=3 kvar=1.5 model=8 zipv=[.2 .3 .5 .1 .4 .5 .8]\n",
    );
    let DistLoadVoltageModel::BoundedZip { alpha_z, .. } =
        &mut module.value_mut().loads_mut()[0].voltage_model
    else {
        panic!()
    };
    alpha_z.push(0.7);
    // No retained source: exercise the public canonical validation path.
    let canonical = powerio_core::PioModule::new(module.value().clone());
    let error = powerio_dist::emit(
        &canonical,
        DistTargetFormat::Dss,
        powerio_core::Destination::memory("out").unwrap(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("voltage-law array"), "{error}");
}

#[test]
fn uniform_typed_voltage_edits_override_retained_bank_kv() {
    for phases in [1, 3] {
        let text = format!(
            "New Circuit.c phases=3 basekv=.416\nNew Load.l phases={phases} bus1=b.1.2.3.0 kv=.24 kw=3 kvar=1 model=5\n"
        );
        let mut module = parse(&text);
        let DistLoadVoltageModel::BoundedZip { v_nom, .. } =
            &mut module.value_mut().loads_mut()[0].voltage_model
        else {
            panic!()
        };
        *v_nom = vec![260.; phases];
        let output = emit_value(module.value(), DistTargetFormat::Dss);
        let back = parse(&output.text);
        assert_eq!(back.value().loads().len(), 1);
        for v in back.value().loads()[0].voltage_model.v_nom() {
            assert!((*v - 260.).abs() < 1e-10);
        }
    }
}

#[test]
fn exponential_bands_preserve_native_defaults_and_typed_edits() {
    for (code, expected_p) in [(3, 0.0), (4, 1.0)] {
        let mut module = parse(&format!(
            "New Circuit.c phases=1 basekv=.24\nNew Load.l phases=1 bus1=b.1.0 kv=.24 kw=1 kvar=.5 model={code}\n"
        ));
        let load = &mut module.value_mut().loads_mut()[0];
        let DistLoadVoltageModel::BoundedExponential {
            gamma_p,
            gamma_q,
            v_min_pu,
            v_max_pu,
            ..
        } = &mut load.voltage_model
        else {
            panic!("missing typed exponential law")
        };
        assert_eq!(gamma_p, &[expected_p]);
        assert_eq!(gamma_q, &[2.0]);
        *gamma_p = vec![0.7];
        *gamma_q = vec![1.8];
        *v_min_pu = 0.9;
        *v_max_pu = 1.1;
        load.extras.insert("model".into(), 2.into());
        load.extras.insert("cvrwatts".into(), 9.0.into());
        load.extras.insert("cvrvars".into(), 9.0.into());
        load.extras.insert("vminpu".into(), 0.8.into());
        let expected = load.voltage_model.clone();
        let output = emit_value(module.value(), DistTargetFormat::Dss);
        let back = parse(&output.text);
        assert_eq!(back.value().loads()[0].voltage_model, expected);
        assert_eq!(expected.power_scale(0, 120.).unwrap(), [0.25; 2]);
        // Outside the band both powers follow the same fallback, regardless of exponent.
        let low = expected.power_scale(0, 168.).unwrap();
        assert!((low[0] - 0.563_888_888_888_888_9).abs() < 1e-14);
        assert_eq!(low[0], low[1]);
        let high = expected.power_scale(0, 288.).unwrap();
        assert!((high[0] - (1.2_f64 / 1.1).powi(2)).abs() < 1e-14);
        assert_eq!(high[0], high[1]);
        assert!(expected.power_scale(1, 240.).is_err());
        for target in [DistTargetFormat::BmopfJson, DistTargetFormat::PmdJson] {
            let result = emit_value(module.value(), target);
            assert!(
                result
                    .warnings
                    .iter()
                    .any(|s| s.contains("voltage-band transitions"))
            );
        }
    }
}

#[test]
fn per_branch_exponents_survive_dss_decomposition() {
    let mut module = parse(
        "New Circuit.c phases=3 basekv=.416\nNew Load.l phases=3 bus1=b.1.2.3.0 kv=.416 kw=3 kvar=1.5 model=4\n",
    );
    let DistLoadVoltageModel::BoundedExponential {
        gamma_p, gamma_q, ..
    } = &mut module.value_mut().loads_mut()[0].voltage_model
    else {
        panic!()
    };
    *gamma_p = vec![0.0, 0.7, 1.5];
    *gamma_q = vec![2.0, 1.8, 3.0];
    let expected = module.value().loads()[0].voltage_model.clone();
    let output = emit_value(module.value(), DistTargetFormat::Dss);
    let back = parse(&output.text);
    assert_eq!(back.value().loads().len(), 3);
    for (phase, load) in back.value().loads().iter().enumerate() {
        for voltage in [90., 180., 235., 260., 310.] {
            assert_eq!(
                load.voltage_model.power_scale(0, voltage).unwrap(),
                expected.power_scale(phase, voltage).unwrap()
            );
        }
    }
}

#[test]
fn exponential_laws_reject_nonfinite_parameters_and_misaligned_arrays() {
    let mut module = parse(
        "New Circuit.c phases=1 basekv=.24\nNew Load.l phases=1 bus1=b.1.0 kv=.24 kw=1 kvar=.5 model=4\n",
    );
    let DistLoadVoltageModel::BoundedExponential { gamma_p, .. } =
        &mut module.value_mut().loads_mut()[0].voltage_model
    else {
        panic!()
    };
    gamma_p[0] = f64::NAN;
    assert!(
        module.value().loads()[0]
            .voltage_model
            .power_scale(0, 100.)
            .is_err()
    );
    assert!(
        powerio_dist::audit_electrical_readiness(module.value())
            .findings
            .iter()
            .any(|f| f.code == "READINESS.LOAD.VOLTAGE_LAW")
    );
    let DistLoadVoltageModel::BoundedExponential { gamma_p, .. } =
        &mut module.value_mut().loads_mut()[0].voltage_model
    else {
        panic!()
    };
    *gamma_p = vec![1., 2.];
    let canonical = powerio_core::PioModule::new(module.value().clone());
    let error = powerio_dist::emit(
        &canonical,
        DistTargetFormat::Dss,
        powerio_core::Destination::memory("out").unwrap(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("voltage-law array"), "{error}");
}

#[test]
fn empty_exponential_laws_fail_before_canonical_writing() {
    let mut module = parse(
        "New Circuit.c phases=1 basekv=.24\nNew Load.l phases=1 bus1=b.1.0 kv=.24 kw=1 kvar=.5 model=4\n",
    );
    let load = &mut module.value_mut().loads_mut()[0];
    load.p_nom.clear();
    load.q_nom.clear();
    let DistLoadVoltageModel::BoundedExponential {
        v_nom,
        gamma_p,
        gamma_q,
        ..
    } = &mut load.voltage_model
    else {
        panic!()
    };
    v_nom.clear();
    gamma_p.clear();
    gamma_q.clear();
    let canonical = powerio_core::PioModule::new(module.value().clone());
    let error = powerio_dist::emit(
        &canonical,
        DistTargetFormat::Dss,
        powerio_core::Destination::memory("out").unwrap(),
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("READINESS.LOAD.VOLTAGE_LAW"),
        "{error}"
    );
}

#[test]
fn idle_storage_is_a_typed_bounded_load_with_current_limit() {
    let mut module = parse(
        "New Circuit.c basekv=.48\nNew Storage.s bus1=b.1.2.3.0 phases=3 conn=w kv=.48 kwrated=250 kva=250 state=idling limitcurrent=yes pf=-1\n",
    );
    let load = &module.value().loads()[0];
    assert!((load.p_nom.iter().sum::<f64>() - 2500.0).abs() < 1e-10);
    assert!(load.q_nom.iter().all(|q| *q == 0.0));
    let law = &load.voltage_model;
    for (u, scale) in [
        (0.0, 0.0),
        (0.45, 0.25),
        (0.9, 1.0),
        (1.0, 1.0),
        (1.1, 1.0),
        (1.21, 1.21),
        (1.8, 2.0),
    ] {
        assert!((law.power_scale(0, law.v_nom()[0] * u).unwrap()[0] - scale).abs() < 1e-12);
    }
    // Typed changes govern output, including the law outside the solved point.
    module.value_mut().loads_mut()[0].p_nom = vec![400.0, 500.0, 600.0];
    let emitted = emit_value(module.value(), DistTargetFormat::Dss);
    let back = parse(&emitted.text);
    assert_eq!(back.value().loads().len(), 3);
    assert_eq!(
        back.value()
            .loads()
            .iter()
            .map(|l| l.p_nom[0])
            .collect::<Vec<_>>(),
        [400.0, 500.0, 600.0]
    );
    assert!(back.value().loads().iter().all(|l| matches!(
        l.voltage_model,
        DistLoadVoltageModel::BoundedConstantPower { .. }
    )));
    for target in [DistTargetFormat::PmdJson, DistTargetFormat::BmopfJson] {
        assert!(
            emit_value(module.value(), target)
                .warnings
                .iter()
                .any(|w| w.contains("current limits"))
        );
    }
}

#[test]
fn unsupported_storage_keeps_electrical_blockers() {
    for extra in [
        "state=unknown",
        "model=2",
        "conn=delta phases=1",
        "balanced=yes",
        "effcurve=c",
        "%idlingkw=-1",
        "vminpu=0",
        "vmaxpu=.8",
        "pf=.0001",
        "like=another",
    ] {
        let module = parse(&format!(
            "New Circuit.c basekv=.48\nNew Storage.s bus1=b.1.2.3.0 kv=.48 kwrated=250 kva=250 {extra}\n"
        ));
        assert!(module.value().loads().is_empty(), "{extra}");
        assert_eq!(
            module
                .value()
                .untyped_objects()
                .iter()
                .filter(|o| o.class == "storage")
                .count(),
            1,
            "{extra}"
        );
    }
}

#[test]
fn static_storage_replays_soc_and_kw_edit_boundaries() {
    for (commands, watts) in [
        ("state=charging", 100.0),
        ("state=charging %stored=50", 10000.0),
        ("state=charging\nEdit Storage.s %stored=50", 100.0),
        ("%stored=10 state=discharging", -10000.0),
        ("%stored=10 %reserve=20 state=discharging", 100.0),
        ("kwhRated=50 %stored=10 state=discharging", 100.0),
        ("%stored=50 kw=-3", 3000.0),
        ("%stored=50 kw=3", -3000.0),
        ("%stored=50 kw=-3 kwrated=20", 6000.0),
        ("%stored=50 state=c %charge=200", 10000.0),
        ("%stored=50 state=d %discharge=200", -10000.0),
        ("%stored=50 state=charging %charge=0", 0.0),
        ("%idlingkw=0 kvar=2", 0.0),
        ("%stored=50 kw=3 kw=0", 100.0),
        (
            "%stored=50 state=charging\nEdit Storage.s %stored=100 %stored=50",
            10000.0,
        ),
        (
            "%stored=50 state=charging\nEdit Storage.s %stored=100\nEdit Storage.s %stored=50",
            100.0,
        ),
    ] {
        let module = parse(&format!(
            "New Circuit.c basekv=.48\nNew Storage.s bus1=b.1.2.3.0 kv=.48 kwrated=10 kva=40 {commands}\n"
        ));
        assert_eq!(module.value().loads().len(), 1, "{commands}");
        let load = &module.value().loads()[0];
        assert!(
            (load.p_nom.iter().sum::<f64>() - watts).abs() < 1e-9,
            "{commands}: {load:?}"
        );
        let emitted = emit_value(module.value(), DistTargetFormat::Dss);
        let back = parse(&emitted.text);
        let actual = &back.value().loads()[0];
        for (a, b) in load
            .p_nom
            .iter()
            .zip(&actual.p_nom)
            .chain(load.q_nom.iter().zip(&actual.q_nom))
        {
            assert!((a - b).abs() < 1e-9, "{commands}: {emitted:?}");
        }
    }
}

#[test]
fn edited_storage_can_mix_generation_consumption_and_reactive_only_branches() {
    let mut module = parse("New Circuit.c basekv=.48\nNew Storage.s bus1=b.1.2.3.0 kv=.48\n");
    module.value_mut().loads_mut()[0].p_nom = vec![-400.0, 500.0, 0.0];
    module.value_mut().loads_mut()[0].q_nom = vec![40.0, -50.0, 60.0];
    let emitted = emit_value(module.value(), DistTargetFormat::Dss);
    let back = parse(&emitted.text);
    let actual: Vec<_> = back
        .value()
        .loads()
        .iter()
        .map(|l| (l.p_nom[0], l.q_nom[0]))
        .collect();
    assert_eq!(actual, [(-400.0, 40.0), (500.0, -50.0), (0.0, 60.0)]);
}
