//! The WebAssembly producer smoke: parse named in-memory bytes with no
//! filesystem assumption, match the concrete value, and
//! emit the byte exact echo. Runs under any wasip1 runtime:
//!
//!     cargo build --target wasm32-wasip1 --example wasm_smoke -p powerio
//!     wasmtime target/wasm32-wasip1/debug/examples/wasm_smoke.wasm
//!
//! and natively as an ordinary example.

fn main() {
    let bytes = include_bytes!("../../tests/data/case9.m").to_vec();
    let source = powerio::Source::from_memory("case9.m", bytes.clone())
        .expect("named in-memory bytes acquire");
    let module = powerio::parse(source, None).expect("parse");
    let powerio::PioValue::BalancedNetwork(network) = &module.value else {
        panic!(
            "expected a balanced network, found {}",
            module.value.type_name()
        );
    };
    assert_eq!(network.buses().len(), 9);
    let emitted = powerio::emit(
        &module,
        "matpower",
        powerio::Destination::memory("case9.m").expect("memory destination"),
    )
    .expect("same format emission");
    let powerio::EmittedOutput::Memory { artifacts } = emitted.output() else {
        panic!("memory emission returned a path output");
    };
    assert_eq!(artifacts[0].bytes(), &bytes[..], "byte exact echo");
    println!("wasm smoke OK: powerio.BalancedNetwork, 9 buses, byte exact echo");
}
