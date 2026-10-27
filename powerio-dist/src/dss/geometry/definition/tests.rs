use super::*;
fn raw(text: &str) -> RawDss {
    crate::dss::raw::__parse_raw_with(text, "geometry.dss", &mut |_: &std::path::Path| {
        Err(std::io::Error::other("no includes"))
    })
}

#[test]
fn wire_reference_is_bound_at_assignment_not_repaired_by_later_declaration() {
    let raw = raw(
        "New LineGeometry.g nconds=1 nphases=1 cond=1 wire=later x=0 h=10\nNew WireData.later radius=0.01 rac=0.0002\n",
    );
    assert!(
        resolve(&raw, raw.line_snapshot(0))
            .unwrap_err()
            .contains("did not exist")
    );
}

#[test]
fn rating_copies_and_live_wire_values_have_different_time_semantics() {
    let raw = raw(
        "New WireData.a radius=0.01 rac=0.0002 normamps=100\nNew LineGeometry.g nconds=1 nphases=1 cond=1 wire=a units=m x=0 h=10\nEdit WireData.a radius=0.02 rac=0.0004 normamps=200\n",
    );
    let definition = resolve(&raw, raw.line_snapshot(1)).unwrap();
    assert!((definition.normamps - 100.0).abs() < 1e-12);
    let geometry = definition
        .materialize(&raw, EarthModel::Carson, 100.0, |object| {
            raw.line_snapshot(object)
        })
        .unwrap();
    assert!((geometry.wires[0].radius - 0.02).abs() < 1e-12);
    assert!((geometry.wires[0].rac - 0.0004).abs() < 1e-12);
    assert!((geometry.wires[0].capacitance_radius - 0.01).abs() < 1e-12);
}

#[test]
#[ignore = "requires pinned native geometry definition observations"]
fn pinned_geometry_definitions() {
    #[derive(serde::Deserialize)]
    struct Matrices {
        r: Vec<Vec<f64>>,
        x: Vec<Vec<f64>>,
        b: Vec<Vec<f64>>,
    }
    #[derive(serde::Deserialize)]
    struct Expected {
        units: Vec<i32>,
        wires: Vec<String>,
        x: Vec<f64>,
        height: Vec<f64>,
        phases: usize,
        reduce: bool,
        normamps: f64,
        emergamps: f64,
        active: usize,
        matrices: Option<Matrices>,
    }
    #[derive(serde::Deserialize)]
    struct Case {
        script: String,
        geometries: std::collections::BTreeMap<String, Expected>,
    }
    let path = std::env::var("POWERIO_GEOMETRY_DEFINITION_ORACLE").expect("native observations");
    let cases: std::collections::BTreeMap<String, Case> =
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert!(!cases.is_empty());
    for (case, expected) in cases {
        let raw = raw(&expected.script);
        for (name, expected) in expected.geometries {
            let index = raw
                .objects
                .iter()
                .position(|o| o.class == "linegeometry" && o.name.eq_ignore_ascii_case(&name))
                .unwrap();
            let actual = resolve(&raw, raw.line_snapshot(index))
                .unwrap_or_else(|e| panic!("{case}/{name}: {e}"));
            assert_eq!(actual.phases, expected.phases, "{case}/{name}/phases");
            assert_eq!(actual.reduce, expected.reduce, "{case}/{name}/reduce");
            assert_eq!(actual.active + 1, expected.active, "{case}/{name}/active");
            assert!(
                (actual.normamps - expected.normamps).abs() < 1e-12,
                "{case}/{name}/normamps"
            );
            assert!(
                (actual.emergamps - expected.emergamps).abs() < 1e-12,
                "{case}/{name}/emergamps"
            );
            assert_eq!(actual.conductors.len(), expected.wires.len());
            for (i, conductor) in actual.conductors.iter().enumerate() {
                assert_eq!(
                    conductor.units.map_or(-1, |u| i32::try_from(u).unwrap()),
                    expected.units[i],
                    "{case}/{name}/units/{i}"
                );
                assert!(
                    conductor
                        .wire
                        .map_or("", |idx| raw.objects[idx].name.as_str())
                        .eq_ignore_ascii_case(&expected.wires[i]),
                    "{case}/{name}/wire/{i}"
                );
                assert!(
                    (conductor.x - expected.x[i]).abs() < 1e-12,
                    "{case}/{name}/x/{i}"
                );
                assert!(
                    (conductor.height - expected.height[i]).abs() < 1e-12,
                    "{case}/{name}/h/{i}"
                );
            }
            if let Some(expected) = expected.matrices {
                let got = actual
                    .materialize(&raw, EarthModel::Carson, 100.0, |idx| {
                        raw.line_snapshot(idx)
                    })
                    .unwrap()
                    .calculate(60.0)
                    .unwrap();
                for (label, got, expected, absolute) in [
                    ("r", got.r, expected.r, 1e-12),
                    ("x", got.x, expected.x, 1e-12),
                    ("b", got.b, expected.b, 1e-18),
                ] {
                    assert_eq!(got.len(), expected.len());
                    for (a, b) in got.iter().flatten().zip(expected.iter().flatten()) {
                        assert!(
                            (a - b).abs() <= absolute + 1e-10 * b.abs(),
                            "{case}/{name}/{label}: {a} != {b}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn spacing_missing_at_assignment_cannot_be_repaired_later() {
    let raw = raw(
        "New LineGeometry.g nconds=1 nphases=1 spacing=missing\nNew LineSpacing.missing nconds=1 x=[0] h=[10]\n",
    );
    assert!(
        resolve(&raw, raw.line_snapshot(0))
            .unwrap_err()
            .contains("did not exist")
    );
}

#[test]
fn spacing_dimension_mismatch_and_uninitialized_growth_are_refused() {
    for text in [
        "New LineSpacing.s nconds=2 x=[0 1] h=[10 11]\nNew LineGeometry.g nconds=1 nphases=1 spacing=s\n",
        "New LineSpacing.s nconds=4\nNew LineGeometry.g nconds=4 nphases=3 spacing=s\n",
    ] {
        let raw = raw(text);
        assert!(resolve(&raw, raw.line_snapshot(1)).is_err());
    }
}

#[test]
fn copied_spacing_is_a_snapshot_and_geometry_ignores_its_phase_count() {
    let raw = raw(
        "New LineSpacing.s nconds=2 nphases=1 units=m x=[0 2] h=[10 11]\nNew LineSpacing.t like=s\nEdit LineSpacing.s x=[3 4]\nNew LineGeometry.g nconds=2 nphases=2 spacing=t\nEdit LineSpacing.t x=[5 6]\n",
    );
    let g = resolve(&raw, raw.line_snapshot(2)).unwrap();
    assert_eq!(g.phases, 2);
    assert_eq!(
        g.conductors.iter().map(|c| c.x).collect::<Vec<_>>(),
        [0., 2.]
    );
}

#[test]
fn cn_forward_references_and_native_geometry_copy_failure_are_explicit() {
    let missing =
        raw("New LineGeometry.g nconds=1 nphases=1 cncables=[later]\nNew CNData.later rac=.001\n");
    assert!(
        resolve(&missing, missing.line_snapshot(0))
            .unwrap_err()
            .contains("did not exist")
    );
    let copied = raw(
        "New CNData.a rac=.001\nNew LineGeometry.g nconds=1 nphases=1 cncables=[a]\nNew LineGeometry.h like=g\n",
    );
    assert!(
        resolve(&copied, copied.line_snapshot(2))
            .unwrap_err()
            .contains("cannot copy")
    );
}
