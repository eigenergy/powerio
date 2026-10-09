use powerio_core::{Destination, PioModule, Source};
use powerio_dist::{DistTargetFormat, audit_electrical_readiness};

#[test]
fn generator_user_models_fail_readiness_and_canonical_writing() {
    for model in ["6", "(3 2 *)", "6.1"] {
        let text = format!(
            "New Circuit.c basekv=.24\nNew Generator.motor bus1=sourcebus kv=.24 kva=25 kw=-18.65 model={model} usermodel=IndMach012a userdata=(rs=.03 slip=.035 option=fixed)\n"
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
