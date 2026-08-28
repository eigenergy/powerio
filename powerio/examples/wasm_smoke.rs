//! The WebAssembly producer smoke: parse named in-memory bytes with no
//! filesystem assumption, read the detected kind and the typed value, and
//! write the byte exact echo. Runs under any wasip1 runtime:
//!
//!     cargo build --target wasm32-wasip1 --example wasm_smoke -p powerio
//!     wasmtime target/wasm32-wasip1/debug/examples/wasm_smoke.wasm
//!
//! and natively as an ordinary example.

fn main() {
    let bytes = include_bytes!("../../tests/data/case9.m").to_vec();
    let source = powerio_core::Source::from_bytes("case9.m", bytes.clone())
        .expect("named in-memory bytes acquire");
    let module = powerio::parse(source).expect("parse");
    assert_eq!(module.value().kind().as_str(), "balanced_network");
    let typed: powerio_core::PioModule<powerio::BalancedNetwork> =
        powerio::try_into_typed(module).expect("narrow");
    assert_eq!(typed.value().buses().len(), 9);
    let dynamic = typed.map_value(powerio::PioValue::from);
    let (echo, _findings) =
        powerio::write_module_str(&dynamic, "matpower").expect("same format write");
    assert_eq!(echo.as_bytes(), &bytes[..], "byte exact echo");
    println!("wasm smoke OK: balanced_network, 9 buses, byte exact echo");
}
