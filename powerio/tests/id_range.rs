//! Readers refuse an id column value the id space cannot represent (issue
//! #341): `1e300` or a negative id must produce a read error naming the
//! column, never a saturated `usize` id.

use std::path::PathBuf;

use powerio::parse_file;

fn temp_path(name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "powerio-id-range-test-{}-{name}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    path
}

fn read_error(name: &str, text: &str) -> String {
    let path = temp_path(name);
    std::fs::write(&path, text).unwrap();
    let err = parse_file(&path, None).expect_err("out-of-range id must not parse");
    std::fs::remove_file(&path).ok();
    let message = err.to_string();
    // The refusal must carry the source value, never the saturated cast.
    assert!(
        !message.contains("18446744073709551615"),
        "saturated id leaked into: {message}"
    );
    message
}

#[test]
fn matpower_refuses_a_huge_branch_bus_id() {
    let err = read_error(
        "huge.m",
        r"function mpc = huge
mpc.baseMVA = 100;
mpc.bus = [
    1  3  0  0  0  0  1  1.0  0  345  1  1.1  0.9;
    2  1  10 5  0  0  1  1.0  0  345  1  1.1  0.9;
];
mpc.branch = [
    1e300  2  0.01  0.05  0.02  0  0  0  0  0  1  -360  360;
];
",
    );
    assert!(
        err.contains("F_BUS") && err.contains("outside the id range"),
        "got: {err}"
    );
}

#[test]
fn matpower_refuses_a_negative_bus_id() {
    let err = read_error(
        "negative.m",
        r"function mpc = negative
mpc.baseMVA = 100;
mpc.bus = [
    -1  3  0  0  0  0  1  1.0  0  345  1  1.1  0.9;
];
mpc.branch = [
    1  1  0.01  0.05  0.02  0  0  0  0  0  1  -360  360;
];
",
    );
    assert!(
        err.contains("BUS_I") && err.contains("outside the id range"),
        "got: {err}"
    );
}

#[test]
fn psse_refuses_out_of_range_bus_ids() {
    let case5 = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data/psse/case5.raw"),
    )
    .unwrap();
    for (name, poison) in [("huge.raw", "1e300"), ("negative.raw", "-1")] {
        let text = case5.replace(
            "   10,'10          '",
            &format!("   {poison},'10          '"),
        );
        let err = read_error(name, &text);
        assert!(
            err.contains("bus field I") && err.contains("outside the id range"),
            "got: {err}"
        );
    }
}

#[test]
fn pslf_refuses_out_of_range_bus_ids() {
    let epc = |id: &str| {
        format!(
            r#"title
two bus
!
solution parameters
sbase 100.0000
!
bus data  [2] ty vsched volt angle ar zone vmax vmin date_in date_out pid L own st
1 "Slack       " 230.0000 : 0 1.0000 1.0000 0.0 1 1 1.1 0.9 400101 391231 0 0 1 0
{id} "Load        " 230.0000 : 1 1.0000 1.0000 -1.0 1 1 1.1 0.9 400101 391231 0 0 1 0
end
"#
        )
    };
    for (name, poison) in [("huge.epc", "1e300"), ("negative.epc", "-2")] {
        let err = read_error(name, &epc(poison));
        assert!(err.contains("bus id") && err.contains(poison), "got: {err}");
    }
}

#[test]
fn surge_refuses_out_of_range_bus_numbers() {
    let doc = |number: &str| {
        format!(
            r#"{{"format":"surge-json","schema_version":"0.1.0","meta":{{}},"network":{{"buses":[{{"number":{number}}}]}}}}"#
        )
    };
    let err = read_error("huge.surge.json", &doc("1e300"));
    assert!(
        err.contains("`number`") && err.contains("outside the id range"),
        "got: {err}"
    );
    // A negative integer keeps the dedicated nonnegative refusal.
    let err = read_error("negative.surge.json", &doc("-1"));
    assert!(err.contains("`number`"), "got: {err}");
}

#[test]
fn pandapower_refuses_out_of_range_bus_indices() {
    let doc = |index: &str| {
        format!(
            r#"{{
  "_module": "pandapower.auxiliary",
  "_class": "pandapowerNet",
  "_object": {{
    "bus": {{
      "_module": "pandas.core.frame",
      "_class": "DataFrame",
      "_object": "{{\"columns\":[\"name\",\"vn_kv\"],\"index\":[{index}],\"data\":[[\"b1\",110.0]]}}",
      "orient": "split",
      "is_multiindex": false,
      "is_multicolumn": false
    }}
  }}
}}"#
        )
    };
    // serde_json renders the huge float back as `1e+300`.
    for (name, poison, shown) in [
        ("huge.pp.json", "1e300", "1e+300"),
        ("negative.pp.json", "-1", "-1"),
    ] {
        let err = read_error(name, &doc(poison));
        assert!(
            err.contains("`bus`") && err.contains("index") && err.contains(shown),
            "got: {err}"
        );
    }
}

#[test]
fn egret_refuses_out_of_range_load_bus_references() {
    let doc = |bus: &str| {
        format!(
            r#"{{"system":{{"baseMVA":100.0}},"elements":{{"bus":{{"1":{{"matpower_bustype":"ref"}}}},"load":{{"load_1":{{"bus":{bus},"p_load":1.0}}}}}}}}"#
        )
    };
    // serde_json renders the huge float back as `1e+300`.
    for (name, poison, shown) in [
        ("huge.egret.json", "1e300", "1e+300"),
        ("negative.egret.json", "-1", "-1"),
    ] {
        let err = read_error(name, &doc(poison));
        assert!(err.contains("`bus`") && err.contains(shown), "got: {err}");
    }
}
