//! Reproductions from Frederik Geth's reviews of PR #746, with boundary
//! cases for source impedance and per-segment current limits.

mod helpers;

use powerio_dist::{DistSwitch, DistTargetFormat};
use serde_json::{Value, json};
use std::path::PathBuf;

fn case(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data/dist")
        .join(rel)
}

#[test]
fn pmd_math_source_impedance_components_and_ideal_control() {
    for (resistance, reactance) in [
        (Some(0.0), Some(1.0)),
        (Some(1.0), Some(0.0)),
        (None, Some(1.0)),
        (Some(1.0), None),
        (Some(0.5), Some(1.0)),
        (Some(0.0), Some(0.0)),
        (None, None),
    ] {
        let mut net =
            helpers::parse_file(case("pmd/case3_balanced.json"), Some("pmd-json")).unwrap();
        let n = net.sources()[0].terminal_map.len();
        for (key, diagonal) in [("rs", resistance), ("xs", reactance)] {
            net.sources_mut()[0].extras.remove(key);
            if let Some(diagonal) = diagonal {
                let matrix: Vec<Vec<f64>> = (0..n)
                    .map(|i| {
                        (0..n)
                            .map(|j| if i == j { diagonal } else { 0.0 })
                            .collect()
                    })
                    .collect();
                net.sources_mut()[0]
                    .extras
                    .insert(key.into(), json!(matrix));
            }
        }
        let out = net.emit_value(DistTargetFormat::PmdMathJson);
        let doc: Value = serde_json::from_str(&out.text).unwrap();
        let branches: Vec<_> = doc["branch"]
            .as_object()
            .unwrap()
            .values()
            .filter(|branch| {
                branch["name"]
                    .as_str()
                    .unwrap()
                    .starts_with("_virtual_branch.voltage_source.")
            })
            .collect();
        let has_impedance = resistance.unwrap_or(0.0) != 0.0 || reactance.unwrap_or(0.0) != 0.0;
        assert_eq!(branches.len(), usize::from(has_impedance));
        if let Some(branch) = branches.first() {
            // Independently undo the PMD per-unit scaling and check every
            // matrix entry, including the component that must remain zero.
            let base = branch["vbase"].as_f64().unwrap();
            let zbase = base * base * doc["settings"]["voltage_scale_factor"].as_f64().unwrap()
                / doc["settings"]["sbase"].as_f64().unwrap();
            for (key, diagonal) in [("br_r", resistance), ("br_x", reactance)] {
                let rows = branch[key].as_array().unwrap();
                assert_eq!(
                    rows.len(),
                    branch["f_connections"].as_array().unwrap().len()
                );
                for (i, row) in rows.iter().enumerate() {
                    assert_eq!(row.as_array().unwrap().len(), rows.len());
                    for (j, entry) in row.as_array().unwrap().iter().enumerate() {
                        let expected = if i == j { diagonal.unwrap_or(0.0) } else { 0.0 };
                        assert!((entry.as_f64().unwrap() * zbase - expected).abs() < 1e-12);
                    }
                }
            }
            let back = helpers::parse_str(&out.text, "pmd-math-json").unwrap();
            assert_eq!(back.sources()[0].bus, net.sources()[0].bus);
            for (key, diagonal) in [("rs", resistance), ("xs", reactance)] {
                for (i, row) in back.sources()[0].extras[key]
                    .as_array()
                    .unwrap()
                    .iter()
                    .enumerate()
                {
                    for (j, entry) in row.as_array().unwrap().iter().enumerate() {
                        let expected = if i == j { diagonal.unwrap_or(0.0) } else { 0.0 };
                        assert!((entry.as_f64().unwrap() - expected).abs() < 1e-12);
                    }
                }
            }
        }
    }
}

#[test]
fn ravens_line_rating_precedence_and_fallback() {
    for override_amps in [None, Some(0.0), Some(100.0), Some(900.0)] {
        let mut net =
            helpers::parse_file(case("ravens/case3_balanced.json"), Some("ravens-json")).unwrap();
        let expected = override_amps.unwrap_or(600.0);
        net.lines_mut()[0].i_max = override_amps.map(|amps| vec![amps; 3]);
        let name = net.lines()[0].name.clone();
        let out = net.emit_value(DistTargetFormat::RavensJson);
        let back = helpers::parse_str(&out.text, "ravens-json").unwrap();
        let line = back.lines().iter().find(|line| line.name == name).unwrap();
        let limit = line
            .i_max
            .as_ref()
            .or(back.linecode(&line.linecode).unwrap().i_max.as_ref());
        assert_eq!(limit, Some(&vec![expected; 3]));
    }
}

#[test]
fn ravens_shared_linecode_retains_each_segments_override() {
    let mut net =
        helpers::parse_file(case("ravens/case3_balanced.json"), Some("ravens-json")).unwrap();
    let mut other = net.lines()[0].clone();
    other.name = "zz_shared_code".into();
    other.extras.clear();
    other.i_max = Some(vec![250.0; 3]);
    let original_name = net.lines()[0].name.clone();
    net.lines_mut()[0].i_max = Some(vec![100.0; 3]);
    net.lines_mut().push(other);
    let out = net.emit_value(DistTargetFormat::RavensJson);
    let back = helpers::parse_str(&out.text, "ravens-json").unwrap();
    for (name, expected) in [(original_name.as_str(), 100.0), ("zz_shared_code", 250.0)] {
        let line = back.lines().iter().find(|line| line.name == name).unwrap();
        let limit = line
            .i_max
            .as_ref()
            .or(back.linecode(&line.linecode).unwrap().i_max.as_ref());
        assert_eq!(limit, Some(&vec![expected; 3]), "{name}");
    }
    assert!(
        !back
            .warnings
            .iter()
            .any(|warning| warning.contains("VALUE_CONFLICT"))
    );
}

#[test]
fn ravens_nonuniform_line_override_is_diagnosed() {
    let mut net =
        helpers::parse_file(case("ravens/case3_balanced.json"), Some("ravens-json")).unwrap();
    net.lines_mut()[0].i_max = Some(vec![100.0, 200.0, 300.0]);
    let name = net.lines()[0].name.clone();
    let out = net.emit_value(DistTargetFormat::RavensJson);
    assert!(out.warnings.iter().any(|warning| {
        warning.contains(&format!("line {name}:"))
            && warning.contains("VALUE_COLLAPSED")
            && warning.contains("ampacity")
    }));
}

#[test]
fn ravens_preserves_line_rating_override() {
    let mut net =
        helpers::parse_file(case("ravens/case3_balanced.json"), Some("ravens-json")).unwrap();
    net.lines_mut()[0].i_max = Some(vec![100.0; 3]);
    let out = net.emit_value(DistTargetFormat::RavensJson);
    let back = helpers::parse_str(&out.text, "ravens-json").unwrap();
    let line = back
        .lines()
        .iter()
        .find(|l| l.name == net.lines()[0].name)
        .unwrap();
    let limit = line
        .i_max
        .as_ref()
        .or(back.linecode(&line.linecode).unwrap().i_max.as_ref());
    assert_eq!(limit, Some(&vec![100.0; 3]), "warnings: {:?}", out.warnings);
}

#[test]
fn ravens_preserves_switch_neutral() {
    let mut net =
        helpers::parse_file(case("ravens/case3_balanced.json"), Some("ravens-json")).unwrap();
    let terminals = vec!["1".into(), "2".into(), "3".into(), "4".into()];
    net.switches_mut().push(DistSwitch::new(
        "fourwire",
        "sourcebus",
        "loadbus",
        terminals.clone(),
        terminals.clone(),
        false,
    ));
    let out = net.emit_value(DistTargetFormat::RavensJson);
    let back = helpers::parse_str(&out.text, "ravens-json").unwrap();
    assert_eq!(
        back.switches()[0].terminal_map_from,
        terminals,
        "warnings: {:?}",
        out.warnings
    );
}

#[test]
fn ravens_reads_disabled_line_as_disabled() {
    fn disable(v: &mut Value) {
        match v {
            Value::Object(o) => {
                if o.get("Ravens.cimObjectType").and_then(Value::as_str) == Some("ACLineSegment") {
                    o.insert("Equipment.inService".into(), json!(false));
                }
                for child in o.values_mut() {
                    disable(child);
                }
            }
            Value::Array(a) => {
                for child in a {
                    disable(child);
                }
            }
            _ => {}
        }
    }
    fn collect_inactive(value: &Value, found: &mut Vec<String>) {
        match value {
            Value::Object(object) => {
                if object.get("Ravens.cimObjectType").and_then(Value::as_str)
                    == Some("ACLineSegment")
                    && object.get("Equipment.inService") == Some(&json!(false))
                {
                    found.push(object["IdentifiedObject.name"].as_str().unwrap().to_owned());
                }
                for child in object.values() {
                    collect_inactive(child, found);
                }
            }
            Value::Array(values) => {
                for child in values {
                    collect_inactive(child, found);
                }
            }
            _ => {}
        }
    }
    let text = std::fs::read_to_string(case("ravens/case3_balanced.json")).unwrap();
    let mut doc: Value = serde_json::from_str(&text).unwrap();
    disable(&mut doc);
    let net = helpers::parse_str(&doc.to_string(), "ravens-json").unwrap();
    let out = net.emit_value(DistTargetFormat::PmdJson);
    let doc: Value = serde_json::from_str(&out.text).unwrap();
    for name in ["ohline", "quad"] {
        // The revised reader retains inactive equipment untyped and omits
        // it from active topology. Unrelated generated source branches can
        // legitimately remain enabled.
        let line = &doc["line"][name];
        assert!(line.is_null() || line["status"] == "DISABLED");
        assert!(!net.lines().iter().any(|line| line.name == name));
        assert!(
            net.untyped_objects()
                .iter()
                .any(|object| object.class == "ACLineSegment" && object.name == name)
        );
        assert!(
            out.warnings
                .iter()
                .any(|warning| warning.contains(name) && warning.contains("RECORD_DROPPED"))
        );
    }
    let canonical = net.emit_value(DistTargetFormat::RavensJson);
    let canonical: Value = serde_json::from_str(&canonical.text).unwrap();
    let mut found = Vec::new();
    collect_inactive(&canonical, &mut found);
    assert!(found.iter().any(|name| name == "ohline"));
    assert!(found.iter().any(|name| name == "quad"));
}

#[test]
fn pmd_math_preserves_purely_reactive_source_impedance() {
    let mut net = helpers::parse_file(case("pmd/case3_balanced.json"), Some("pmd-json")).unwrap();
    let n = net.sources()[0].terminal_map.len();
    net.sources_mut()[0]
        .extras
        .insert("rs".into(), json!(vec![vec![0.0; n]; n]));
    let xs: Vec<Vec<f64>> = (0..n)
        .map(|i| (0..n).map(|j| if i == j { 1.0 } else { 0.0 }).collect())
        .collect();
    net.sources_mut()[0].extras.insert("xs".into(), json!(xs));
    let out = net.emit_value(DistTargetFormat::PmdMathJson);
    let doc: Value = serde_json::from_str(&out.text).unwrap();
    let virtual_branches: Vec<_> = doc["branch"]
        .as_object()
        .unwrap()
        .values()
        .filter(|b| {
            b["name"]
                .as_str()
                .unwrap()
                .starts_with("_virtual_branch.voltage_source.")
        })
        .collect();
    assert_eq!(
        virtual_branches.len(),
        1,
        "source reactance must not become zero; warnings: {:?}",
        out.warnings
    );
}

#[test]
fn pmd_math_restores_kron_reduced_wye_transformer_neutral() {
    let eng =
        helpers::parse_file(case("pmd/math/ut_trans_2w_yy_eng.json"), Some("pmd-json")).unwrap();
    let math = helpers::parse_file(
        case("pmd/math/ut_trans_2w_yy_math.json"),
        Some("pmd-math-json"),
    )
    .unwrap();
    let out = math.emit_value(DistTargetFormat::PmdJson);
    let back = helpers::parse_str(&out.text, "pmd-json").unwrap();
    assert_eq!(
        back.transformers()[0].phases,
        eng.transformers()[0].phases,
        "MATHEMATICAL -> ENGINEERING must not turn a three phase transformer into two phases"
    );
    for (e, m) in eng.transformers()[0]
        .windings
        .iter()
        .zip(&math.transformers()[0].windings)
    {
        assert_eq!(
            m.terminal_map, e.terminal_map,
            "Kron reduced wye windings require their grounded return; math diagnostics: {:?}",
            math.warnings
        );
    }
}
