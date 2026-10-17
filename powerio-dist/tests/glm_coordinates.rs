use powerio_core::Source;
use powerio_dist::glm::{GlmCoordinates, PreprocessOptions, parse_project};

fn coordinates(text: &str) -> Result<Vec<GlmCoordinates>, powerio_core::Error> {
    let project = parse_project(
        Source::from_memory("coordinates.glm", text.as_bytes()).unwrap(),
        PreprocessOptions::default(),
    )?;
    project.index_objects()?.declared_coordinates()
}

#[test]
fn signed_decimal_hemisphere_and_quoted_dms_share_degree_values() {
    let points = coordinates(
        r#"
        object node { latitude -27.47; longitude 153.02; }
        object node { latitude 27S28.2; longitude 153E1.2; }
        object node { latitude "27S28:12"; longitude "153E1:12"; }
        object node { latitude 45N30; longitude 120W15; }
        object node { latitude 9e1; longitude -1.8e2; }
        "#,
    )
    .unwrap();
    for point in &points[..3] {
        assert!((point.latitude.unwrap() + 27.47).abs() < 1e-12);
        assert!((point.longitude.unwrap() - 153.02).abs() < 1e-12);
    }
    assert_eq!(points[3].latitude, Some(45.5));
    assert_eq!(points[3].longitude, Some(-120.25));
    assert_eq!(points[4].latitude, Some(90.0));
    assert_eq!(points[4].longitude, Some(-180.0));
}

#[test]
fn missing_partial_zero_and_reference_assignment_time_are_distinct() {
    let points = coordinates(
        r"
        object node { name empty; }
        object node { name zero; latitude 0; }
        object node { name source; latitude 1;
            object node { name copy; latitude (source.latitude); longitude (empty.longitude); }
            latitude 2;
        }
        object node { latitude (source.latitude); longitude 0; }
        ",
    )
    .unwrap();
    assert_eq!(points[0], GlmCoordinates::default());
    assert_eq!(points[1].latitude, Some(0.0));
    assert_eq!(points[1].longitude, None);
    assert_eq!(points[2].latitude, Some(2.0));
    assert_eq!(points[3].latitude, Some(1.0));
    assert_eq!(points[3].longitude, None);
    assert_eq!(points[4].latitude, Some(2.0));
    assert_eq!(points[4].longitude, Some(0.0));
}

#[test]
fn forward_and_cross_axis_references_are_not_reinterpreted() {
    for text in [
        "object node { latitude (later.latitude); } object node { name later; latitude 1; }",
        "object node { name a; longitude 1; } object node { latitude (a.longitude); }",
        "object node { latitude (a.latitude); name a; }",
        "object node { latitude object node {}; }",
        "object node { latitude; }",
        "object node { longitude; }",
    ] {
        assert!(coordinates(text).is_err(), "{text}");
    }
}

#[test]
fn invalid_numbers_ranges_and_native_prefix_acceptance_fail_explicitly() {
    for value in [
        "NaN",
        "inf",
        "1e999",
        "91",
        "-91",
        "27garbage",
        "27E2",
        "-27S2",
        "27S60",
        "27S2:60",
        "27S-2",
        "27S2:-1",
        "90N0.1",
        "27S2:3junk",
        "27S",
    ] {
        let input = format!("object node {{ latitude {value}; }}");
        let error = coordinates(&input).unwrap_err();
        assert!(error.retained_source().is_some());
        assert_eq!(error.diagnostics()[0].code(), "PARSE.GLM.OBJECT");
    }
    assert!(coordinates("object node { longitude 181; }").is_err());
}

#[test]
fn coordinate_errors_point_into_included_source() {
    let source = Source::from_memory("main.glm", b"#include \"part.glm\"\n".as_slice())
        .unwrap()
        .with_named_buffer("part.glm", b"object node { latitude 99; }".as_slice())
        .unwrap();
    let project = parse_project(source, PreprocessOptions::default()).unwrap();
    let error = project
        .index_objects()
        .unwrap()
        .declared_coordinates()
        .unwrap_err();
    let span = &error.diagnostics()[0].spans()[0];
    assert_eq!(span.source().as_str(), "part.glm");
    assert_eq!(span.byte_start(), 14);
}
