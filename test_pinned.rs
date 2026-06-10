use powerio_dist::{parse_bmopf_str, write_bmopf_json};

fn main() {
    let text = std::fs::read_to_string("/tmp/test_pinned_warn.json").unwrap();
    let net = parse_bmopf_str(&text).unwrap();
    println!("Parsed gen: p_nom={:?}, p_min={:?}, p_max={:?}", net.generators[0].p_nom, net.generators[0].p_min, net.generators[0].p_max);
    
    let conv = write_bmopf_json(&net);
    println!("Warnings:");
    for w in &conv.warnings {
        println!("  {}", w);
    }
}
