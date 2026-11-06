mod helpers;

use powerio_dist::DistTargetFormat;
use serde_json::{Value, json};
use std::path::PathBuf;

fn case(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data/dist")
        .join(rel)
}

#[test]
fn source_impedance_virtual_bus_grounding_matches_source_connections() {
    let mut net = helpers::parse_file(case("pmd/case3_balanced.json"), Some("pmd-json")).unwrap();
    let source = &mut net.sources_mut()[0];
    source.terminal_map = vec!["2".into()];
    source.v_magnitude = vec![source.v_magnitude[1]];
    source.v_angle = vec![source.v_angle[1]];
    source.extras.insert("rs".into(), json!([[0.0]]));
    source.extras.insert("xs".into(), json!([[1.0]]));
    let out = net.emit_value(DistTargetFormat::PmdMathJson);
    let doc: Value = serde_json::from_str(&out.text).unwrap();
    let bus = doc["bus"]
        .as_object()
        .unwrap()
        .values()
        .find(|b| {
            b["name"]
                .as_str()
                .unwrap()
                .starts_with("_virtual_bus.voltage_source.")
        })
        .unwrap();
    assert_eq!(bus["terminals"], json!([2]));
    assert_eq!(
        bus["grounded"],
        json!([false]),
        "virtual bus: {bus}; warnings: {:?}",
        out.warnings
    );
}

#[test]
fn ravens_unrated_line_does_not_inherit_another_segments_limit() {
    let mut net =
        helpers::parse_file(case("ravens/case3_balanced.json"), Some("ravens-json")).unwrap();
    let mut other = net.lines()[0].clone();
    other.name = "zz_unrated".into();
    other.extras.clear();
    other.i_max = None;
    let code_name = other.linecode.clone();
    net.line_codes_mut()
        .iter_mut()
        .find(|c| c.name == code_name)
        .unwrap()
        .i_max = None;
    net.lines_mut()[0].i_max = Some(vec![100.0; 3]);
    net.lines_mut().push(other);
    let out = net.emit_value(DistTargetFormat::RavensJson);
    let back = helpers::parse_str(&out.text, "ravens-json").unwrap();
    let line = back
        .lines()
        .iter()
        .find(|l| l.name == "zz_unrated")
        .unwrap();
    let limit = line
        .i_max
        .as_ref()
        .or(back.linecode(&line.linecode).unwrap().i_max.as_ref());
    assert_eq!(
        limit, None,
        "warnings: {:?}; {:?}",
        out.warnings, back.warnings
    );
}
