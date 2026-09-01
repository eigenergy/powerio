#![cfg(feature = "gridfm")]

use powerio::{Destination, EmittedOutput, Source};

#[test]
fn universal_emit_writes_gridfm_as_one_directory_inventory() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/case9.m");
    let module = powerio::parse(Source::open(path).unwrap(), Some("matpower")).unwrap();
    let result = powerio::emit(
        &module,
        "gridfm",
        Destination::memory("case9-gridfm").unwrap(),
    )
    .unwrap();

    assert_eq!(result.layout(), powerio::OutputLayout::Directory);
    let EmittedOutput::Memory { artifacts } = result.output() else {
        panic!("memory destination returned path artifacts");
    };
    let names: Vec<_> = artifacts
        .iter()
        .map(|artifact| artifact.name().as_str())
        .collect();
    assert_eq!(
        names,
        [
            "case9-gridfm/raw/branch_data.parquet",
            "case9-gridfm/raw/bus_data.parquet",
            "case9-gridfm/raw/gen_data.parquet",
            "case9-gridfm/raw/gridfm_meta.json",
            "case9-gridfm/raw/y_bus_data.parquet",
        ]
    );
}
