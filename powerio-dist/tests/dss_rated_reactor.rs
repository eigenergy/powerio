use powerio_core::{Destination, PioModule, Source};
use powerio_dist::{DistTargetFormat, MulticonductorNetwork, audit_electrical_readiness};

fn parse(text: &str) -> PioModule<MulticonductorNetwork> {
    powerio_dist::parse(Source::from_memory("rated.dss", text.as_bytes()).unwrap()).unwrap()
}

fn canonical(net: MulticonductorNetwork) -> MulticonductorNetwork {
    let output = powerio_dist::emit(
        &PioModule::new(net),
        DistTargetFormat::Dss,
        Destination::memory("out").unwrap(),
    )
    .unwrap();
    let powerio_core::EmittedOutput::Memory { artifacts } = output.into_output() else {
        panic!("memory output expected")
    };
    let source = artifacts
        .iter()
        .find(|a| a.bytes().starts_with(b"Clear"))
        .unwrap();
    parse(std::str::from_utf8(source.bytes()).unwrap()).into_value()
}

#[test]
fn resistance_does_not_replace_the_rated_reactor_specification() {
    let net = parse("New Circuit.c basekv=138\nNew Reactor.bank bus1=sourcebus phases=3 kv=138 kvar=40000 r=0\n").into_value();
    assert!(audit_electrical_readiness(&net).is_ready());
    assert_eq!(net.shunts().len(), 1);
    assert_eq!(net.untyped_objects().as_slice(), []);
    let shunt = &net.shunts()[0];
    assert!(shunt.g.iter().flatten().all(|&v| v == 0.0));
    assert!((shunt.b[0][0] + 40_000_000.0 / 138_000.0_f64.powi(2)).abs() < 1e-17);
}

#[test]
fn canonical_shunts_respect_matrix_edits_after_reimport() {
    for declaration in [
        "New Reactor.bank bus1=sourcebus phases=3 kv=12.47 kvar=120 r=0",
        "New Reactor.bank bus1=sourcebus phases=3 kv=12.47 kvar=120 r=2 rp=300",
        "New Reactor.bank bus1=sourcebus phases=3 kv=12.47 kvar=120 rp=300",
        "New Capacitor.bank bus1=sourcebus phases=3 conn=delta kv=12.47 kvar=120",
    ] {
        let net = parse(&format!("New Circuit.c basekv=12.47\n{declaration}\n")).into_value();
        let mut edited = canonical(net);
        for row in &mut edited.shunts_mut()[0].b {
            for value in row {
                *value *= 1.17;
            }
        }
        // A cached nameplate rating is not a replacement for the typed matrix.
        edited.shunts_mut()[0]
            .extras
            .insert("kvar".into(), 999.into());
        let actual = canonical(edited.clone());
        for (expected, got) in edited.shunts()[0]
            .b
            .iter()
            .flatten()
            .zip(actual.shunts()[0].b.iter().flatten())
        {
            assert!((expected - got).abs() < 1e-12);
        }
        for (expected, got) in edited.shunts()[0]
            .g
            .iter()
            .flatten()
            .zip(actual.shunts()[0].g.iter().flatten())
        {
            assert!((expected - got).abs() < 1e-12);
        }
    }
}

#[test]
fn unsupported_rated_reactor_states_remain_explicit() {
    for suffix in [
        " conn=delta",
        " lcurve=curve",
        " kvar=0",
        " rp=-1",
        "\nNew Reactor.copy like=bank",
    ] {
        let net = parse(&format!(
            "New Circuit.c\nNew Reactor.bank bus1=sourcebus kv=12.47 kvar=120 r=0{suffix}\n"
        ))
        .into_value();
        assert!(!audit_electrical_readiness(&net).is_ready(), "{suffix}");
    }
}

#[test]
#[ignore = "external native rated-reactor cases; writes edited typed shunts for independent validation"]
fn export_edited_rated_reactor_matrices() {
    let root = std::path::PathBuf::from(std::env::var("POWERIO_RATED_REACTOR_CASES").unwrap());
    let cases: Vec<String> =
        serde_json::from_slice(&std::fs::read(root.join("cases.json")).unwrap()).unwrap();
    for case in cases {
        let folder = root.join(case);
        let mut net = powerio_dist::parse(Source::open(folder.join("native.dss")).unwrap())
            .unwrap()
            .into_value();
        assert_eq!(net.shunts().len(), 1);
        let shunt = &mut net.shunts_mut()[0];
        for matrix in [&mut shunt.g, &mut shunt.b] {
            for value in matrix.iter_mut().flatten() {
                *value *= 1.17;
            }
        }
        powerio_dist::emit(
            &PioModule::new(net),
            DistTargetFormat::Dss,
            Destination::path(folder.join("typed-edited")),
        )
        .unwrap();
    }
}
