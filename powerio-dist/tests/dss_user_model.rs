use powerio_core::{Destination, PioModule, Source};
use powerio_dist::{DistTargetFormat, audit_electrical_readiness};

#[test]
fn generator_user_models_fail_readiness_and_canonical_writing() {
    for model in ["6", "(3 2 *)", "6.1"] {
        let text = format!(
            "New Circuit.c basekv=.24\nNew Generator.motor bus1=sourcebus kv=.24 kva=25 kw=-18.65 model={model} usermodel=IndMach012a userdata=(rs=.03 slip=.035 option=variable)\n"
        );
        let source = Source::from_memory("motor.dss", text.as_bytes()).unwrap();
        let module = powerio_dist::parse(source).unwrap();
        let report = audit_electrical_readiness(module.value());
        assert!(
            report
                .blockers()
                .any(|f| f.code == "READINESS.DSS.GENERATOR_USER_MODEL_UNRESOLVED"),
            "{model}"
        );
        assert_eq!(module.value().generators()[0].extras["model"], 6);
        // Retained same-format bytes remain available, but a fresh module
        // must not claim that nominal generator power captures the library.
        let echo = powerio_dist::emit(
            &module,
            DistTargetFormat::Dss,
            Destination::memory("echo").unwrap(),
        )
        .unwrap();
        let powerio_core::EmittedOutput::Memory { artifacts } = echo.into_output() else {
            panic!("memory output expected")
        };
        assert!(
            artifacts
                .iter()
                .any(|artifact| artifact.bytes() == text.as_bytes())
        );
        let error = powerio_dist::emit(
            &PioModule::new(module.value().clone()),
            DistTargetFormat::Dss,
            Destination::memory("out").unwrap(),
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("GENERATOR_USER_MODEL_UNRESOLVED")
        );
        let wire = serde_json::to_vec(module.value()).unwrap();
        let back: powerio_dist::MulticonductorNetwork = serde_json::from_slice(&wire).unwrap();
        assert!(!audit_electrical_readiness(&back).is_ready());
    }
}

#[test]
fn ordinary_generators_do_not_require_a_user_model_equivalent() {
    let module = powerio_dist::parse(
        Source::from_memory(
            "generator.dss",
            b"New Circuit.c\nNew Generator.g bus1=sourcebus kw=10 kvar=2 model=1\n".to_vec(),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(audit_electrical_readiness(module.value()).is_ready());
}

fn motor(text: &str) -> powerio_dist::MulticonductorNetwork {
    powerio_dist::parse(Source::from_memory("motor.dss", text.as_bytes()).unwrap())
        .unwrap()
        .into_value()
}

const MOTOR: &str = "New Circuit.c basekv=.24\nNew Generator.m bus1=sourcebus kv=.24 kva=25 conn=delta model=6 usermodel=IndMach012a ";

#[test]
fn fixed_motor_replays_slip_limits_library_creation_and_edits() {
    let a = motor(&format!(
        "{MOTOR}userdata=(slip=.2 maxslip=.3 option=fixed)"
    ));
    let b = motor(&format!("{MOTOR}userdata=(slip=.1 option=fixed)"));
    assert_eq!(a.generators().as_slice(), []);
    assert!(audit_electrical_readiness(&a).is_ready());
    assert_eq!(a.shunts(), b.shunts());
    let c = motor(&format!(
        "{MOTOR}userdata=(maxslip=.3 slip=.2 option=fixed)"
    ));
    assert_ne!(a.shunts()[0].g, c.shunts()[0].g);
    let reset = motor(&format!(
        "{MOTOR}userdata=(rs=.9 slip=.05 option=fixed)\nEdit Generator.m usermodel=IndMach012a userdata=(option=fixed)"
    ));
    let defaults = motor(&format!("{MOTOR}userdata=(option=fixed)"));
    assert_eq!(reset.shunts(), defaults.shunts());
    let early = motor(
        "New Circuit.c basekv=.24\nNew Generator.m bus1=sourcebus kv=.24 kva=25 conn=delta model=6 userdata=(rs=.9 slip=.05) usermodel=IndMach012a userdata=(option=fixed)",
    );
    assert_eq!(early.shunts(), defaults.shunts());
    let split = motor(&format!(
        "{MOTOR}userdata=(rs=.03)\nEdit Generator.m userdata=(slip=.04 option=fixed)"
    ));
    let together = motor(&format!("{MOTOR}userdata=(rs=.03 slip=.04 option=fixed)"));
    assert_eq!(split.shunts(), together.shunts());
    let shunt = &together.shunts()[0];
    assert!((shunt.g[0][1] - shunt.g[1][0]).abs() > 0.01);
    for matrix in [&shunt.g, &shunt.b] {
        assert!(
            matrix
                .iter()
                .all(|row| row.iter().sum::<f64>().abs() < 1e-14)
        );
    }
}

#[test]
fn unresolved_motor_states_never_become_a_fixed_shunt() {
    for tail in [
        "userdata=(option=variable)",
        "userdata=(option=fixed) enabled=no",
        "userdata=(option=fixed) phases=1",
        "userdata=(option=fixed) conn=wye",
        "userdata=(option=fixed rs=NaN)",
        "userdata=(option=fixed maxslip=-1)",
        "userdata=(option=fixed rs=(.01 2 *))",
        "userdata=(option=fixed)\nOpen Generator.m 1 1",
        "userdata=(option=fixed)\nNew Generator.copy like=m",
    ] {
        let source = format!("{MOTOR}{tail}");
        let Ok(module) =
            powerio_dist::parse(Source::from_memory("motor.dss", source.as_bytes()).unwrap())
        else {
            assert!(tail.contains("Open Generator"));
            continue;
        };
        let net = module.into_value();
        assert!(!audit_electrical_readiness(&net).is_ready(), "{tail}");
    }
}

#[test]
#[ignore = "requires external native motor cases; emits edited typed matrices for the oracle"]
fn export_edited_fixed_motor_matrices() {
    let root = std::path::PathBuf::from(std::env::var("POWERIO_FIXED_MOTOR_CASES").unwrap());
    let cases: Vec<String> =
        serde_json::from_slice(&std::fs::read(root.join("cases.json")).unwrap()).unwrap();
    for case in cases {
        let folder = root.join(case);
        let module = powerio_dist::parse(Source::open(folder.join("native.dss")).unwrap()).unwrap();
        let mut net = module.into_value();
        assert_eq!(net.generators().as_slice(), []);
        assert_eq!(net.shunts().len(), 1);
        let shunt = &mut net.shunts_mut()[0];
        for matrix in [&mut shunt.g, &mut shunt.b] {
            for entry in matrix.iter_mut().flatten() {
                *entry *= 1.17;
            }
        }
        // No retained source and no user library properties drive this output.
        assert!(audit_electrical_readiness(&net).is_ready());
        powerio_dist::emit(
            &PioModule::new(net),
            DistTargetFormat::Dss,
            Destination::path(folder.join("typed-edited")),
        )
        .unwrap();
    }
}
