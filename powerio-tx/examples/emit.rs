//! Emit a parsed case in a target format. For converter validation/debugging.
//! `cargo run -q --example emit -- <file.m> [powermodels|egret]`
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args
        .get(1)
        .expect("usage: emit <file.m> [powermodels|egret]");
    let fmt = args.get(2).map_or("powermodels", String::as_str);
    let source = powerio_core::Source::open(path).unwrap();
    let net = powerio_tx::parse(source).unwrap().into_value();
    let conv = match fmt {
        "egret" => powerio_tx::write_egret_json(&net),
        _ => powerio_tx::write_powermodels_json(&net),
    };
    let findings = conv.rendered_diagnostics();
    if !findings.is_empty() {
        eprintln!("findings: {findings:?}");
    }
    print!("{}", conv.text);
}
