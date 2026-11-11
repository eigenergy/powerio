//! A three-phase pandapower network reads as a MulticonductorNetwork under
//! the one `pandapower-json` token. A balanced one still reads balanced.

use powerio::{Destination, EmittedOutput, PioModule, PioValue, Source};

const TINY: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../tests/data/dist/pandapower/tiny_3ph.json"
);

fn bytes(result: powerio::EmitResult) -> Vec<u8> {
    let EmittedOutput::Memory { artifacts } = result.into_output() else {
        panic!("memory output")
    };
    artifacts[0].bytes().to_vec()
}

fn multiconductor(module: &PioModule<PioValue>) -> &powerio::MulticonductorNetwork {
    let PioValue::MulticonductorNetwork(network) = module.value() else {
        panic!("a three-phase network reads multiconductor");
    };
    network
}

#[test]
fn the_fixture_reads_with_its_element_counts() {
    let module = powerio::parse(TINY).unwrap();
    let network = multiconductor(&module);
    assert_eq!(network.buses().len(), 5);
    assert_eq!(network.lines().len(), 2);
    assert_eq!(network.line_codes().len(), 1);
    assert_eq!(network.transformers().len(), 0);
    // the asymmetric load, the delta load and the wye load
    assert_eq!(network.loads().len(), 3);
    // the asymmetric sgen, not the sgen without a type
    assert_eq!(network.generators().len(), 1);
    assert_eq!(network.sources().len(), 1);
    assert_eq!(network.switches().len(), 0);
    assert_eq!(network.shunts().len(), 0);
    let mut codes: Vec<_> = module
        .diagnostics()
        .iter()
        .map(powerio::Diagnostic::code)
        .collect();
    codes.sort_unstable();
    assert_eq!(
        codes,
        [
            "READ.DIST.PANDAPOWER_APPROXIMATED",
            "READ.DIST.PANDAPOWER_ELEMENT_UNSUPPORTED",
            "READ.DIST.PANDAPOWER_RETAINED_SOURCE_ONLY",
            "READ.DIST.PANDAPOWER_TABLE_UNSUPPORTED",
            "READ.DIST.PANDAPOWER_TABLE_UNSUPPORTED",
            "READ.DIST.PANDAPOWER_TABLE_UNSUPPORTED",
        ]
    );
    // The transformer, shunt and switch tables have no mapping yet.
    for table in ["trafo", "shunt", "switch"] {
        assert!(
            module.diagnostics().iter().any(|d| {
                d.code() == "READ.DIST.PANDAPOWER_TABLE_UNSUPPORTED"
                    && d.message().starts_with(&format!("`{table}` table"))
            }),
            "{table}"
        );
    }
}

#[test]
fn a_declared_alias_reads_the_same_network() {
    let source = Source::open(TINY)
        .unwrap()
        .with_format(powerio::FormatId::new("pp").unwrap());
    let module = powerio::parse(source).unwrap();
    assert_eq!(multiconductor(&module).buses().len(), 5);
}

#[test]
fn a_same_format_write_echoes_the_source_bytes() {
    let module = powerio::parse(TINY).unwrap();
    let echoed = bytes(
        powerio::emit(
            &module,
            "pandapower-json",
            Destination::memory("case").unwrap(),
        )
        .unwrap(),
    );
    assert_eq!(echoed, std::fs::read(TINY).unwrap());
}

#[test]
fn the_ir_keeps_the_source_format() {
    let module = powerio::parse(TINY).unwrap();
    let ir =
        bytes(powerio::serialize(&module, Destination::memory("case.pio.json").unwrap()).unwrap());
    let restored = powerio::deserialize(Source::from_memory("case.pio.json", ir).unwrap()).unwrap();
    let network = multiconductor(&restored);
    assert_eq!(
        network.source_format().as_ref().map(|format| format.name()),
        Some("pandapower-json")
    );
    assert_eq!(network.shunts().len(), 0);
}

#[test]
fn a_network_without_its_source_has_no_pandapower_writer() {
    let module = powerio::parse(TINY).unwrap();
    let fresh = PioModule::new(module.value().clone());
    let error = powerio::emit(
        &fresh,
        "pandapower-json",
        Destination::memory("case").unwrap(),
    )
    .unwrap_err();
    assert_eq!(
        error.info().map(|info| info.code),
        Some("EMIT.DIST.PANDAPOWER_UNSUPPORTED")
    );
}

#[test]
fn a_balanced_network_still_reads_balanced() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tests/data/pandapower/example.json"
    );
    let module = powerio::parse(path).unwrap();
    let PioValue::BalancedNetwork(network) = module.value() else {
        panic!("a balanced pandapower network reads balanced");
    };
    assert_eq!(network.buses().len(), 7);
    let echoed = bytes(
        powerio::emit(
            &module,
            "pandapower-json",
            Destination::memory("case").unwrap(),
        )
        .unwrap(),
    );
    assert_eq!(echoed, std::fs::read(path).unwrap());
}
