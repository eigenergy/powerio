//! Raw layer over the vendored fixtures: redirects resolve, every object
//! materializes, and nothing warns unexpectedly.

use std::path::PathBuf;

use powerio_dist::dss::{RawDss, parse_raw_file};

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data/dist")
        .join(rel)
}

fn parse(rel: &str) -> RawDss {
    parse_raw_file(fixture(rel)).expect("fixture readable")
}

fn count(raw: &RawDss, class: &str) -> usize {
    raw.of_class(class).count()
}

#[test]
fn ieee13() {
    let raw = parse("opendss/ieee13/IEEE13Nodeckt.dss");
    assert_eq!(raw.warnings, Vec::<String>::new());
    assert_eq!(raw.circuit_name.as_deref(), Some("IEEE13Nodeckt"));
    assert_eq!(count(&raw, "vsource"), 1);
    assert_eq!(count(&raw, "line"), 12);
    assert_eq!(count(&raw, "load"), 15);
    assert_eq!(count(&raw, "transformer"), 5);
    assert_eq!(count(&raw, "capacitor"), 2);
    assert_eq!(count(&raw, "regcontrol"), 3);
    // 7 mtx codes inline plus 29 from IEEELineCodes.DSS, which is a stub
    // redirecting to the shared file one directory up; reaching those 29
    // proves nested redirects resolve relative to the including file.
    assert_eq!(count(&raw, "linecode"), 36);
    assert_eq!(raw.buscoords.len(), 16);

    let switch = raw.find("line", "671692").expect("switch line");
    assert_eq!(switch.get("switch").unwrap().text, "y");
    let xfm1 = raw.find("transformer", "XFM1").expect("XFM1");
    assert!(xfm1.get("xhl").is_some());
}

#[test]
fn ieee34() {
    let raw = parse("opendss/ieee34/ieee34Mod1.dss");
    assert_eq!(raw.warnings, Vec::<String>::new());
    assert_eq!(count(&raw, "line"), 32);
    assert_eq!(count(&raw, "load"), 68);
    assert_eq!(count(&raw, "transformer"), 8);
    assert_eq!(count(&raw, "capacitor"), 2);
    assert_eq!(count(&raw, "regcontrol"), 6);
}

#[test]
fn ieee123() {
    let raw = parse("opendss/ieee123/IEEE123Master.dss");
    assert_eq!(raw.warnings, Vec::<String>::new());
    assert_eq!(count(&raw, "line"), 126);
    // Loads come from the redirected IEEE123Loads.DSS.
    assert_eq!(count(&raw, "load"), 91);
    assert_eq!(count(&raw, "linecode"), 29);
    // Regulator transformers and controls come from IEEE123Regulators.DSS.
    assert!(count(&raw, "transformer") >= 2);
    assert!(count(&raw, "regcontrol") >= 1);
}

#[test]
fn micro_cases_parse_without_warnings() {
    for case in [
        "micro/xfmr_single_phase.dss",
        "micro/xfmr_center_tap.dss",
        "micro/xfmr_wye_delta.dss",
        "micro/xfmr_delta_wye.dss",
        "micro/switch.dss",
        "micro/fourwire_linecode.dss",
        "micro/neutral_grounding_reactor.dss",
        "micro/onephase_cvr_load.dss",
        "micro/onephase_zip_load.dss",
        "micro/defaults_degenerate.dss",
        "micro/linecode_10x10.dss",
    ] {
        let raw = parse(case);
        assert_eq!(raw.warnings, Vec::<String>::new(), "{case}");
        assert_eq!(count(&raw, "vsource"), 1, "{case}");
    }
}

#[test]
#[allow(clippy::float_cmp)]
fn ten_conductor_matrix() {
    let raw = parse("micro/linecode_10x10.dss");
    let lc = raw.find("linecode", "lc10").expect("lc10");
    let rows = lc.get("rmatrix").unwrap().to_rows(None).unwrap();
    assert_eq!(rows.len(), 10);
    assert_eq!(rows[9].len(), 10);
    assert_eq!(rows[9][9], 0.25);
}
