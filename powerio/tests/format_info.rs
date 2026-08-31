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
        ("powerworld", Some("aux"), false, true),
        ("pandapower-json", Some("json"), false, true),
        ("pypsa-csv", None, true, true),
        ("pslf", Some("epc"), false, true),
        ("pwb", Some("pwb"), false, false),
        ("gridfm", None, true, false),
        ("goc3-json", Some("json"), false, false),
        ("surge-json", Some("json"), false, true),
        ("opfdata-json", Some("json"), false, false),
        ("dss", Some("dss"), true, true),
        ("pmd-json", Some("json"), false, true),
        ("bmopf-json", Some("json"), false, true),
        ("pio-json", Some("pio.json"), false, true),
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
        ("pio_json", "pio-json"),
    ] {
        assert_eq!(resolve_format(alias).map(|info| info.token), Some(token));
    }

    for name in ["", "json", "model-json", "geojson", "not-a-format"] {
        assert_eq!(resolve_format(name), None, "{name}");
    }
}
