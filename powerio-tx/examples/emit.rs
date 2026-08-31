//! Emit a parsed case in a target format. For converter validation/debugging.
//! `cargo run -q --example emit -- <file.m> [powermodels|egret]`
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args
        .get(1)
        .expect("usage: emit <file.m> [powermodels|egret]");
    let fmt = args.get(2).map_or("powermodels", String::as_str);
    let source = powerio_core::Source::open(path).unwrap();
    let module = powerio_tx::parse(source).unwrap();
    let target = match fmt {
        "egret" => powerio_tx::TargetFormat::EgretJson,
        _ => powerio_tx::TargetFormat::PowerModelsJson,
    };
    let emitted = powerio_tx::emit(
        &module,
        target,
        powerio_core::Destination::memory("case").unwrap(),
    )
    .unwrap();
    let findings = powerio_tx::diagnostics::render_diagnostics(emitted.diagnostics());
    if !findings.is_empty() {
        eprintln!("findings: {findings:?}");
    }
    let powerio_core::EmittedOutput::Memory { artifacts } = emitted.into_output() else {
        unreachable!("memory destination returns memory output")
    };
    print!("{}", String::from_utf8_lossy(artifacts[0].bytes()));
}
