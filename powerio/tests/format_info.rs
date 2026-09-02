use powerio::resolve_format;

#[test]
fn every_canonical_format_reports_its_artifact_shape() {
    let expected = [
        ("matpower", Some("m"), false, true),
        ("powermodels-json", Some("json"), false, true),
        ("egret-json", Some("json"), false, true),
        ("psse", Some("raw"), false, true),
        ("psse34", Some("raw"), false, true),
        ("psse35", Some("raw"), false, true),
        ("psse-rawx", Some("rawx"), false, true),
        ("xiidm", Some("xiidm"), false, true),
        ("jiidm", Some("jiidm"), false, true),
        ("cgmes", None, true, true),
        ("ucte", Some("uct"), false, true),
        ("powerworld", Some("aux"), false, true),
        ("pandapower-json", Some("json"), false, true),
        ("pypsa-csv", None, true, true),
        ("pslf", Some("epc"), false, true),
        ("pwb", Some("pwb"), false, false),
        ("gridfm", None, true, true),
        ("goc3-json", Some("json"), false, true),
        ("surge-json", Some("json"), false, true),
        ("opfdata-json", Some("json"), false, false),
        ("dss", Some("dss"), true, true),
        ("pmd-json", Some("json"), false, true),
        ("bmopf-json", Some("json"), false, true),
    ];

    for (token, extension, is_directory, can_emit) in expected {
        let info = resolve_format(token).unwrap();
        assert_eq!(info.token, token);
        assert_eq!(info.extension, extension, "{token}");
        assert_eq!(info.is_directory, is_directory, "{token}");
        assert_eq!(info.can_emit, can_emit, "{token}");
    }
}

#[test]
fn common_aliases_resolve_without_exposing_component_enums() {
    for (alias, token) in [
        ("m", "matpower"),
        ("pm", "powermodels-json"),
        ("raw34", "psse34"),
        ("AUX", "powerworld"),
        ("pp", "pandapower-json"),
        ("pypsa", "pypsa-csv"),
        ("epc", "pslf"),
        ("opendss", "dss"),
        ("engineering", "pmd-json"),
        ("bmopf", "bmopf-json"),
        ("gridopt", "opfdata-json"),
        ("uct", "ucte"),
    ] {
        assert_eq!(resolve_format(alias).map(|info| info.token), Some(token));
    }

    for name in [
        "",
        "json",
        "model-json",
        "geojson",
        "rawx",
        "iidm",
        "not-a-format",
    ] {
        assert_eq!(resolve_format(name), None, "{name}");
    }
}

/// Without the `gridfm` feature the descriptor still resolves the token, and
/// emission refuses it by naming the missing build feature.
#[cfg(not(feature = "gridfm"))]
#[test]
fn gridfm_emission_without_the_feature_names_the_feature() {
    let source = powerio::Source::open("../tests/data/case9.m").unwrap();
    let module = powerio::parse(source, None).unwrap();
    let dir = std::env::temp_dir().join(format!("powerio-gridfm-absent-{}", std::process::id()));
    let error = powerio::emit(&module, "gridfm", powerio::Destination::path(&dir)).unwrap_err();
    let message = error.to_string();
    assert!(message.contains("REQUEST.EMIT.UNKNOWN_FORMAT"), "{message}");
    assert!(
        message.contains("gridfm") && message.contains("feature"),
        "{message}"
    );
}
