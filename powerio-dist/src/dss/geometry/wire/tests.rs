use super::*;
use std::fmt::Write as _;
fn raw(text: &str) -> RawDss {
    crate::dss::raw::__parse_raw_with(text, "wire.dss", &mut |_: &std::path::Path| {
        Err(std::io::Error::other("no includes"))
    })
}

#[test]
fn historical_copy_and_sticky_radius_survive_source_edits() {
    let raw = raw(
        "New WireData.a rac=0.306 runits=mi gmrac=0.0244 gmrunits=ft diam=0.721 radunits=in\nNew WireData.b radius=2 like=a\nEdit WireData.a radius=1\nEdit WireData.b radius=0.5\n",
    );
    let b = resolve(&raw, raw.line_snapshot(1)).unwrap();
    assert!((b.capacitance_radius - 0.3605).abs() < 1e-15);
    assert!((b.radius - 0.5).abs() < 1e-15);
    let wire = b.at(0.0, 10.0);
    assert!((wire.radius - 0.0127).abs() < 1e-15);
    assert!((wire.gmr - 0.0244 * 0.3048).abs() < 1e-15);
    assert!((wire.rac - 0.306 / 1609.344).abs() < 1e-15);
    assert!((wire.rdc - wire.rac / 1.02).abs() < 1e-15);
}

#[test]
fn empty_and_self_copies_keep_native_defaults_without_recursion() {
    let mut text =
        "New WireData.a radius=0.1\nEdit WireData.a like=a\nNew WireData.b like=a\n".to_owned();
    for i in 0..512 {
        writeln!(
            text,
            "New WireData.c{i} like={}",
            if i == 0 {
                "b".into()
            } else {
                format!("c{}", i - 1)
            }
        )
        .unwrap();
    }
    let raw = raw(&text);
    let wire = resolve(&raw, raw.line_snapshot(raw.objects.len() - 1)).unwrap();
    assert!((wire.capacitance_radius - 0.1).abs() < 1e-15);
    assert!((wire.rac + 1.0).abs() < 1e-15);
}

#[test]
#[ignore = "requires pinned native WireData observations"]
fn pinned_wire_definitions() {
    #[derive(serde::Deserialize)]
    struct Case {
        script: String,
        wires: std::collections::BTreeMap<String, serde_json::Value>,
    }
    let path = std::env::var("POWERIO_WIRE_ORACLE").expect("native observations");
    let cases: std::collections::BTreeMap<String, Case> =
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert!(!cases.is_empty());
    for (case, expected) in cases {
        let raw = raw(&expected.script);
        for (name, fields) in expected.wires {
            let index = raw
                .objects
                .iter()
                .position(|o| o.class == "wiredata" && o.name.eq_ignore_ascii_case(&name))
                .unwrap();
            let actual =
                serde_json::to_value(resolve(&raw, raw.line_snapshot(index)).unwrap()).unwrap();
            for (field, value) in fields.as_object().unwrap() {
                let expected = value.as_f64().unwrap();
                let actual = actual[field].as_f64().unwrap();
                assert!(
                    (actual - expected).abs() <= 1e-14 + 1e-12 * expected.abs(),
                    "{case}/{name}/{field}: {actual} != {expected}"
                );
            }
        }
    }
}
