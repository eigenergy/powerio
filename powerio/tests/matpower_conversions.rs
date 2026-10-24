//! Unit conversions affect interpreted values, never retained source bytes.
use powerio::{BalancedNetwork, PioModule, PioValue, emit, parse};
use powerio_core::{Destination, EmitResult, EmittedOutput, Source};

const CASE33BW: &[u8] = include_bytes!("../../tests/data/case33bw.m");

fn network(module: &PioModule<PioValue>) -> &BalancedNetwork {
    let PioValue::BalancedNetwork(net) = module.value() else {
        panic!("expected a balanced network");
    };
    net
}

fn bytes(result: &EmitResult) -> &[u8] {
    let EmittedOutput::Memory { artifacts } = result.output() else {
        panic!("expected memory output");
    };
    assert_eq!(artifacts.len(), 1);
    artifacts[0].bytes()
}

fn case33bw() -> PioModule<PioValue> {
    parse(Source::from_memory("case33bw.m", CASE33BW.to_vec()).unwrap()).unwrap()
}

fn verify_values(module: &PioModule<PioValue>, demand_mw: f64) {
    let net = network(module);
    assert!((net.loads().iter().map(|load| load.p).sum::<f64>() - demand_mw).abs() < 1e-12);
    assert!((net.loads().iter().map(|load| load.q).sum::<f64>() - 2.3).abs() < 1e-12);
    let zbase = 12.66_f64.powi(2) / 10.0;
    assert!((net.branches()[0].r - 0.0922 / zbase).abs() < 1e-12);
    assert!((net.branches()[0].x - 0.047 / zbase).abs() < 1e-12);
}

#[test]
fn conversion_and_unchanged_source_echo_are_independent() {
    for source in [
        CASE33BW.to_vec(),
        String::from_utf8(CASE33BW.to_vec())
            .unwrap()
            .replace('\n', "\r\n")
            .into_bytes(),
    ] {
        let module = parse(Source::from_memory("case33bw.m", source.clone()).unwrap()).unwrap();
        assert_eq!(module.diagnostics(), []);
        verify_values(&module, 3.715);
        let result = emit(&module, "matpower", Destination::memory("echo.m").unwrap()).unwrap();
        assert_eq!(bytes(&result), source);
        let reparsed =
            parse(Source::from_memory("echo.m", bytes(&result).to_vec()).unwrap()).unwrap();
        verify_values(&reparsed, 3.715);
        assert_eq!(reparsed.diagnostics(), []);
    }
}

#[test]
fn cross_format_and_edited_matpower_exports_do_not_double_convert() {
    let mut module = case33bw();
    let result = emit(
        &module,
        "powermodels",
        Destination::memory("case.json").unwrap(),
    )
    .unwrap();
    let roundtrip =
        parse(Source::from_memory("case.json", bytes(&result).to_vec()).unwrap()).unwrap();
    verify_values(&roundtrip, 3.715);

    let PioValue::BalancedNetwork(net) = module.value_mut() else {
        unreachable!()
    };
    net.loads_mut()[0].p += 0.1;
    assert!(module.source().is_none());
    let result = emit(
        &module,
        "matpower",
        Destination::memory("canonical.m").unwrap(),
    )
    .unwrap();
    let text = std::str::from_utf8(bytes(&result)).unwrap();
    assert!(!text.contains("idx_bus"));
    assert!(!text.contains("Vbase"));
    assert!(!text.contains("mpc.bus(:,"));
    let roundtrip =
        parse(Source::from_memory("canonical.m", bytes(&result).to_vec()).unwrap()).unwrap();
    verify_values(&roundtrip, 3.815);
    assert_eq!(roundtrip.diagnostics(), []);
}

#[test]
fn unsupported_postprocessing_remains_visible_at_the_facade() {
    let mutation = "mpc.bus(:, 3) = unknown();";
    let mut source = CASE33BW.to_vec();
    source.extend_from_slice(mutation.as_bytes());
    let module = parse(Source::from_memory("unsupported.m", source.clone()).unwrap()).unwrap();
    assert!(
        (network(&module)
            .loads()
            .iter()
            .map(|load| load.p)
            .sum::<f64>()
            - 3715.0)
            .abs()
            < 1e-10
    );
    assert!((network(&module).branches()[0].r - 0.0922).abs() < 1e-14);
    assert_eq!(module.diagnostics().len(), 3);
    for diagnostic in module.diagnostics() {
        assert_eq!(diagnostic.code(), "READ.MATPOWER.INDEXED_ASSIGNMENT");
        assert_eq!(
            diagnostic.severity(),
            powerio_core::DiagnosticSeverity::Warning
        );
        assert_ne!(diagnostic.spans(), []);
    }
    let span = &module.diagnostics()[2].spans()[0];
    assert_eq!(span.byte_start(), CASE33BW.len() as u64);
    assert_eq!(span.byte_end(), source.len() as u64);
    let result = emit(&module, "matpower", Destination::memory("echo.m").unwrap()).unwrap();
    assert_eq!(bytes(&result), source);
}
