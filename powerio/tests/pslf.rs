use std::path::PathBuf;

use powerio::{SourceFormat, parse_file, parse_str, target_format_from_name};

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
fn pslf_is_not_a_write_target() {
    assert_eq!(target_format_from_name("pslf"), None);
    assert_eq!(target_format_from_name("epc"), None);
}

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
