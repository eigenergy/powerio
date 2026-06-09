//! Emit a parsed case in a target format. For converter validation/debugging.
//! `cargo run -q --example emit -- <file.m> [powermodels|egret]`
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args
        .get(1)
        .expect("usage: emit <file.m> [powermodels|egret]");
    let fmt = args.get(2).map_or("powermodels", String::as_str);
    let net = powerio::parse_file(path).unwrap();
    let conv = match fmt {
        "egret" => powerio::write_egret_json(&net),
        _ => powerio::write_powermodels_json(&net),
    };
    if !conv.warnings.is_empty() {
        eprintln!("warnings: {:?}", conv.warnings);
    }
    print!("{}", conv.text);
}
