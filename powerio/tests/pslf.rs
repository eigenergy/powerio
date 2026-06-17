use std::path::PathBuf;

use powerio::{
    SourceFormat, TargetFormat, parse_file, parse_pslf, parse_psse, parse_str,
    target_format_from_name, write_as, write_pslf,
};

const EPC: &str = r#"title
two bus
!
solution parameters
sbase 100.0000
!
bus data  [2] ty vsched volt angle ar zone vmax vmin date_in date_out pid L own st
1 "Slack       " 230.0000 : 0 1.0000 1.0000 0.0 1 1 1.1 0.9 400101 391231 0 0 1 0
2 "Load        " 230.0000 : 1 1.0000 1.0000 -1.0 1 1 1.1 0.9 400101 391231 0 0 1 0
branch data  [1] ck se long_id st resist react charge rate1 rate2 rate3 rate4 aloss lngth
1 "Slack       " 230.00 2 "Load        " 230.00 "1 " 1 "line" : 1 0.01 0.05 0.001 100 90 80 0 0 1 /
1 1 0 0
load data  [1] id long_id st mw mvar mw_i mvar_i mw_z mvar_z ar zone
2 "Load        " 230.00 "1 " "load" : 1 10 3 0 0 0 0 1 1
end
"#;

#[test]
fn parse_str_accepts_pslf_aliases() {
    for alias in ["pslf", "PSLF", "epc", "EPC", "pslf-epc", "Pslf_Epc"] {
        let parsed = parse_str(EPC, alias).unwrap();
        assert_eq!(parsed.network.source_format, SourceFormat::Pslf);
        assert_eq!(parsed.network.buses.len(), 2);
        assert_eq!(parsed.network.branches.len(), 1);
        assert_eq!(parsed.network.loads.len(), 1);
    }
}

#[test]
fn parse_file_infers_uppercase_epc_extension() {
    let path = temp_path("case.EPC");
    std::fs::write(&path, EPC).unwrap();

    let parsed = parse_file(&path, None).unwrap();

    assert_eq!(parsed.network.source_format, SourceFormat::Pslf);
    assert_eq!(
        parsed.network.source.as_deref().map(String::as_str),
        Some(EPC)
    );
}

#[test]
fn parse_file_accepts_case_insensitive_pslf_hint() {
    let path = temp_path("case.txt");
    std::fs::write(&path, EPC).unwrap();

    for hint in ["PSLF", "EPC", "Pslf_Epc"] {
        let parsed = parse_file(&path, Some(hint)).unwrap();
        assert_eq!(parsed.network.source_format, SourceFormat::Pslf);
    }
}

#[test]
fn pslf_is_a_write_target() {
    assert_eq!(target_format_from_name("pslf"), Some(TargetFormat::Pslf));
    assert_eq!(target_format_from_name("epc"), Some(TargetFormat::Pslf));
}

#[test]
fn pslf_write_read_round_trip_preserves_the_core() {
    // .epc → Network → .epc → Network keeps the power flow core. (The two-winding
    // transformer and ZIP load split exercise the multi-line record and the
    // replayed pslf_* extras.)
    let net0 = parse_pslf(EPC_WITH_TRANSFORMER).unwrap();
    let text = write_pslf(&net0).text;
    let net1 = parse_pslf(&text).unwrap();

    assert_eq!(net1.buses.len(), net0.buses.len());
    assert_eq!(net1.branches.len(), net0.branches.len());
    assert_eq!(net1.loads.len(), net0.loads.len());
    assert_eq!(net1.generators.len(), net0.generators.len());
    assert_eq!(net1.shunts.len(), net0.shunts.len());

    let sum = |xs: &[f64]| xs.iter().sum::<f64>();
    let p0 = sum(&net0.loads.iter().map(|l| l.p).collect::<Vec<_>>());
    let p1 = sum(&net1.loads.iter().map(|l| l.p).collect::<Vec<_>>());
    assert!((p0 - p1).abs() < 1e-9, "load P changed: {p0} != {p1}");
    // The transformer survives the round trip with its tap.
    let tap0 = net0
        .branches
        .iter()
        .find(|b| b.is_transformer())
        .unwrap()
        .tap;
    let tap1 = net1
        .branches
        .iter()
        .find(|b| b.is_transformer())
        .unwrap()
        .tap;
    assert!((tap0 - tap1).abs() < 1e-9, "tap changed: {tap0} != {tap1}");
}

#[test]
fn pslf_same_format_write_echoes_source() {
    // A PSLF-sourced network writes back byte-for-byte through the retained source.
    let parsed = parse_str(EPC, "pslf").unwrap();
    assert_eq!(write_as(&parsed.network, TargetFormat::Pslf).text, EPC);
}

#[test]
fn pslf_write_reports_dropped_transformer_control() {
    // A PSS/E regulating transformer carries control the .epc record can't hold,
    // so the PSLF writer must report the loss rather than drop it silently.
    let raw = "0, 100.00, 33, 0, 0, 60.00 / x\nCASE\nCOMMENT\n\
        1,'B1          ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9\n\
        2,'B2          ', 138.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9\n\
        3,'B3          ', 13.8,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9\n\
        0 / END OF BUS DATA, BEGIN LOAD DATA\n\
        0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA\n\
        0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA\n\
        0 / END OF GENERATOR DATA, BEGIN BRANCH DATA\n\
        0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA\n\
        1, 2, 0, '1', 1, 1, 1, 0, 0, 2, 'REG         ', 1, 1, 1, 0, 1, 0, 1, 0, 1, '            '\n\
        0.01, 0.10, 100.0\n\
        1.025, 0, 2.5, 100.0, 90.0, 80.0, 1, 3, 1.08, 0.92, 1.05, 0.98, 17, 0, 0, 0, 0\n\
        1.0, 0\n\
        0 / END OF TRANSFORMER DATA, BEGIN AREA DATA\nQ\n";
    let net = parse_psse(raw).unwrap();
    assert!(net.branches[0].control.is_some());

    let conv = write_pslf(&net);
    assert!(
        conv.warnings
            .iter()
            .any(|w| w.contains("regulating control")),
        "expected a control-drop warning, got {:?}",
        conv.warnings
    );
}

#[test]
fn pslf_reads_and_writes_a_three_winding_transformer() {
    let net = parse_pslf(EPC_3W).unwrap();
    assert_eq!(net.transformers_3w.len(), 1, "the tertiary record was read");
    assert!(net.branches.is_empty(), "a 3W is not folded into branches");
    let t = &net.transformers_3w[0];
    assert_eq!(
        [
            t.windings[0].bus.0,
            t.windings[1].bus.0,
            t.windings[2].bus.0
        ],
        [1, 2, 3]
    );
    // z12 = primary-secondary, z23 = secondary-tertiary, z31 = tertiary-primary.
    assert!((t.z[0].r - 0.01).abs() < 1e-9);
    assert!((t.z[1].r - 0.03).abs() < 1e-9);
    assert!((t.z[2].r - 0.02).abs() < 1e-9);
    assert!((t.windings[0].tap - 1.05).abs() < 1e-9);

    // Round trip through the writer keeps the buses, impedances, and primary tap.
    let net2 = parse_pslf(&write_pslf(&net).text).unwrap();
    assert_eq!(net2.transformers_3w.len(), 1);
    assert!(net2.branches.is_empty());
    let t2 = &net2.transformers_3w[0];
    assert!((t2.z[2].x - 0.07).abs() < 1e-9);
    assert!((t2.windings[0].tap - 1.05).abs() < 1e-9);
    assert_eq!(t2.windings[2].bus.0, 3);
}

/// A 3-bus EPC with a tertiary (3-winding) transformer record.
const EPC_3W: &str = r#"title
t3w
!
solution parameters
sbase 100.0000
!
bus data  [3] ty vsched volt angle ar zone vmax vmin
1 "B1          " 230.0000 : 0 1.0 1.0 0.0 1 1 1.1 0.9
2 "B2          " 138.0000 : 1 1.0 1.0 0.0 1 1 1.1 0.9
3 "B3          " 13.8000 : 1 1.0 1.0 0.0 1 1 1.1 0.9
transformer data  [1]
1 "B1          " 230.00 2 "B2          " 138.00 "1 " 1 "xf3" : 1 0 0 0 0 0 0 0 0 3 0 0 0 0 100 0.01 0.06 0.02 0.07 0.03 0.08 /
0 0 0 0 0 0 100 90 80 0 0.0 0 0 0 0 0 1.05
end
"#;

/// A two-winding transformer EPC plus a ZIP load, for the round-trip test.
const EPC_WITH_TRANSFORMER: &str = r#"title
xfmr case
!
solution parameters
sbase 100.0000
!
bus data  [2] ty vsched volt angle ar zone vmax vmin
1 "Slack       " 230.0000 : 0 1.0000 1.0000 0.0 1 1 1.1 0.9
2 "Load        " 138.0000 : 1 1.0000 1.0000 -1.0 1 1 1.1 0.9
transformer data  [1]
1 "Slack       " 230.00 2 "Load        " 138.00 "1 " 1 "xf" : 1 0 0 0 0 0 0 0 0 0 0 0 0 0 100 0.02 0.06 0 0 0 0 /
0 0 0 0 0 0 90 80 70 0 0.05 0 0 0 0 0 1.025
load data  [1] id long_id st mw mvar mw_i mvar_i mw_z mvar_z ar zone
2 "Load        " 138.00 "1 " "load" : 1 10 3 1 0.5 2 1.5 1 1
end
"#;

#[test]
fn malformed_pslf_input_returns_errors_without_panics() {
    for (text, expected) in [
        ("", "no buses"),
        ("title\nunterminated\n", "no buses"),
        (
            "bus data [1]\nnot-a-bus\nend\n",
            "bus id missing or invalid",
        ),
        (
            "bus data [1]\n1 \"A\" 230 : bad 1 1 0 1 1 1.1 0.9\nend\n",
            "bus type field 0 value",
        ),
    ] {
        let outcome = std::panic::catch_unwind(|| parse_str(text, "pslf"));
        assert!(outcome.is_ok(), "PSLF parser panicked on {text:?}");
        let err = outcome
            .unwrap()
            .expect_err("malformed PSLF input parsed successfully");
        assert!(
            err.to_string().contains(expected),
            "expected {expected:?} in {err}"
        );
    }
}

#[test]
fn pslf_missing_end_marker_warns_without_panic() {
    let text = "bus data [1]\n1 \"A\" 230 : 0 1 1 0 1 1 1.1 0.9 /\n";
    let outcome = std::panic::catch_unwind(|| parse_str(text, "pslf"));
    assert!(
        outcome.is_ok(),
        "PSLF parser panicked on missing end marker"
    );
    let parsed = outcome.unwrap().expect("single bus PSLF case should parse");
    assert!(
        parsed
            .warnings
            .iter()
            .any(|warning| warning.contains("no end marker")),
        "expected no end marker warning, got {:?}",
        parsed.warnings
    );
}

fn temp_path(name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "powerio-pslf-test-{}-{name}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    path
}
