use num_complex::Complex64;
use powerio_core::{Error, Source};
use powerio_dist::glm::{
    GlmDialect, GlmQuantity, GlmQuantityKind, PreprocessOptions, parse_project,
};

fn quantity(
    text: &str,
    kind: GlmQuantityKind,
    unit: Option<&str>,
    dialect: GlmDialect,
) -> Result<GlmQuantity, Error> {
    let source = format!("object probe {{ value {text}; }}");
    let project = parse_project(
        Source::from_memory("case.glm", source.as_bytes()).unwrap(),
        PreprocessOptions {
            dialect,
            ..Default::default()
        },
    )?;
    let index = project.index_objects()?;
    project.quantity(&index.objects()[0].properties()[0], kind, unit)
}

#[test]
fn real_units_preserve_dimensions_defaults_prefixes_and_native_offsets() {
    for (text, unit, expected) in [
        ("1.609344 km", "ft", 5280.0),
        ("240", "V", 240.0),
        (".24kV", "V", 240.0),
        ("1 Ohm/km", "Ohm/mile", 1.609_344),
        ("1 uF/km", "nF/mile", 1609.344),
        ("25 %", "pu", 0.25),
        ("0 degC", "K", 273.14),
        ("1 m^2", "sf", 1.0 / 0.3048_f64.powi(2)),
        ("1 V*A", "W", 1.0),
    ] {
        let result =
            quantity(text, GlmQuantityKind::Real, Some(unit), GlmDialect::Pnnl5_3).unwrap();
        assert!(
            (result.value.re - expected).abs() <= 1e-10 * expected.abs().max(1.0),
            "{text}"
        );
        assert_eq!(result.value.im.to_bits(), 0.0_f64.to_bits());
        assert!(result.diagnostics.is_empty());
    }
}

#[test]
fn rectangular_polar_and_exponent_signs_produce_the_same_phasor() {
    for text in [
        ".003+.004j kV",
        "3e-3+4e-3i kV",
        "5+53.13010235415598d V",
        "5+0.9272952180016122r V",
        "3 4j V",
        "\"3+4j V\"",
    ] {
        let result = quantity(
            text,
            GlmQuantityKind::Complex,
            Some("V"),
            GlmDialect::Pnnl5_3,
        )
        .unwrap();
        assert!(
            (result.value - Complex64::new(3.0, 4.0)).norm() < 1e-12,
            "{text}"
        );
        assert!(result.diagnostics.is_empty());
    }
}

#[test]
fn native_ignored_suffixes_are_diagnosed_without_changing_the_value() {
    for text in ["\"5 kVA\"", "\"5j\""] {
        let result = quantity(
            text,
            GlmQuantityKind::Complex,
            Some("VA"),
            GlmDialect::Pnnl5_3,
        )
        .unwrap();
        assert_eq!(result.value, Complex64::new(5.0, 0.0));
        assert_eq!(result.diagnostics[0].code(), "READ.GLM.UNIT_IGNORED");
        assert_eq!(result.diagnostics[0].spans()[0].source().as_str(), "/input");
    }
    let result = quantity(
        "3+4j kVA",
        GlmQuantityKind::Complex,
        None,
        GlmDialect::Pnnl5_3,
    )
    .unwrap();
    assert_eq!(result.value, Complex64::new(3.0, 4.0));
    assert_eq!(result.diagnostics.len(), 1);
}

#[test]
fn parenthesized_complex_literals_require_the_arras_profile() {
    let result = quantity(
        "\"(3+4j)\"",
        GlmQuantityKind::Complex,
        Some("V"),
        GlmDialect::Arras4_3,
    )
    .unwrap();
    assert_eq!(result.value, Complex64::new(3.0, 4.0));
    assert!(
        quantity(
            "\"(3+4j)\"",
            GlmQuantityKind::Complex,
            Some("V"),
            GlmDialect::Pnnl5_3
        )
        .is_err()
    );
    for text in ["(3+4j) kV", "(3+4j kV)", "(3+4j", "(3+4j))"] {
        assert!(
            quantity(
                text,
                GlmQuantityKind::Complex,
                Some("V"),
                GlmDialect::Arras4_3
            )
            .is_err()
        );
    }
}

#[test]
fn malformed_nonfinite_overflowing_and_dimensionally_wrong_values_fail() {
    for text in [
        "NaN",
        "inf",
        "1e999",
        "1 kA",
        "1 mystery",
        "1e",
        "1e+",
        "1 V^1000",
        "1 degC/m",
        "1 V*m/m/m/m/m/m/m/m/m/m/m/m/m/m/m/m/m/m/m/m/m/m/m/m/m/m/m/m/m/m/m/m/m",
        "1e308 YV",
    ] {
        let error =
            quantity(text, GlmQuantityKind::Real, Some("V"), GlmDialect::Pnnl5_3).unwrap_err();
        assert_eq!(
            error.diagnostics()[0].code(),
            "PARSE.GLM.QUANTITY",
            "{text}"
        );
        assert!(error.retained_source().is_some());
    }
    for text in ["1+2", "1+2+3j", "1+1e999j", "1e308+1e308j YV", "1+2x"] {
        assert!(
            quantity(
                text,
                GlmQuantityKind::Complex,
                Some("V"),
                GlmDialect::Pnnl5_3
            )
            .is_err(),
            "{text}"
        );
    }
    assert!(quantity("\"1 V\"", GlmQuantityKind::Real, None, GlmDialect::Pnnl5_3).is_err());
}

#[test]
fn quantity_error_and_warning_spans_refer_to_original_included_bytes() {
    let source = Source::from_memory("main.glm", b"#include \"part.glm\"\n".as_slice())
        .unwrap()
        .with_named_buffer("part.glm", b"object probe { value \"1 kV\"; }".as_slice())
        .unwrap();
    let project = parse_project(source, PreprocessOptions::default()).unwrap();
    let index = project.index_objects().unwrap();
    let property = &index.objects()[0].properties()[0];
    let error = project
        .quantity(property, GlmQuantityKind::Real, Some("A"))
        .unwrap_err();
    assert_eq!(
        error.diagnostics()[0].spans()[0].source().as_str(),
        "part.glm"
    );
    let warning = project
        .quantity(property, GlmQuantityKind::Complex, Some("V"))
        .unwrap();
    assert_eq!(
        warning.diagnostics[0].spans()[0].source().as_str(),
        "part.glm"
    );
}
