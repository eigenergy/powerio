//! Real export fidelity tests for the PowerWorld aux reader, against the
//! vendored ACTIVSg200 complete case export (see
//! `tests/data/powerworld/README.md`).

use std::path::{Path, PathBuf};

use powerio::format::powerworld::{AuxSection, parse_aux, write_aux};
use powerio::{TargetFormat, parse_file};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data/powerworld")
        .join(name)
}

fn activsg200() -> String {
    std::fs::read_to_string(fixture("ACTIVSg200.aux")).unwrap()
}

/// Every DATA block in the real export is accounted for: the inventory below
/// is the complete object census of the file. A parser regression that starts
/// dropping or merging blocks fails loudly here.
#[test]
fn activsg200_inventory_is_complete() {
    let aux = parse_aux(&activsg200()).unwrap();

    // (object type, row count, field count), in file order.
    let expected: &[(&str, usize, usize)] = &[
        ("PWCaseInformation", 1, 1),
        ("Owner", 1, 3),
        ("Substation", 111, 8),
        ("Limit_Monitoring_Options_Value", 1, 2),
        ("LimitSet", 1, 19),
        ("RatingSetNameBus", 4, 3),
        ("RatingSetNameBranch", 15, 3),
        ("RatingSetNameInterface", 15, 3),
        ("Bus", 200, 36),
        ("Gen", 49, 62),
        ("Load", 160, 25),
        ("Branch", 180, 55),
        ("Branch", 66, 76),
        ("Shunt", 4, 23),
        ("Area", 1, 21),
        ("BalancingAuthority", 200, 7),
        ("Zone", 7, 6),
        ("Sim_Solution_Options_Value", 69, 2),
        ("PostPowerFlowActions", 1, 1),
        ("GICXFormer", 66, 15),
        ("ContingencyElement", 245, 11),
        ("Contingency", 245, 32),
    ];

    let got: Vec<(String, usize, usize)> = aux
        .data()
        .map(|d| (d.object_type.clone(), d.rows.len(), d.fields.len()))
        .collect();
    let want: Vec<(String, usize, usize)> = expected
        .iter()
        .map(|&(t, r, f)| (t.to_string(), r, f))
        .collect();
    assert_eq!(got, want, "object inventory of ACTIVSg200.aux changed");

    // No SCRIPT sections in this export; every section is DATA.
    assert_eq!(aux.sections.len(), 22);
    assert!(
        aux.sections
            .iter()
            .all(|s| matches!(s, AuxSection::Data(_)))
    );

    // The contingency SUBDATA payload survives: 245 contingencies carrying
    // 490 SUBDATA blocks (CTGElement and LimitViol).
    let ctg = aux.data_of("Contingency").next().unwrap();
    let subdata: usize = ctg.rows.iter().map(|r| r.subdata.len()).sum();
    assert_eq!(subdata, 490);
}

/// The 20+ digit substation coordinates and quoted names come through the
/// tokenizer exactly.
#[test]
fn activsg200_values_survive_tokenizing() {
    let aux = parse_aux(&activsg200()).unwrap();
    let sub = aux.data_of("Substation").next().unwrap();
    let lat = sub.field_index("Latitude").unwrap();
    let name = sub.field_index("SubName").unwrap();
    // Substation 1 in the file: a quoted name with an embedded space and a
    // full precision latitude.
    let row = &sub.rows[0];
    assert_eq!(row.values[name], "CREVE COEUR");
    assert_eq!(row.values[lat], "40.64211600000000150000");
}

/// Same format echo of the real export is byte exact (retained source).
#[test]
fn activsg200_echo_is_byte_exact() {
    let net = parse_file(fixture("ACTIVSg200.aux"), None).unwrap().network;
    let echo = net.to_format(TargetFormat::PowerWorld);
    assert!(echo.warnings.is_empty());
    assert_eq!(echo.text, activsg200());
}

/// Canonical generic serialization is idempotent on the real export.
#[test]
fn activsg200_canonical_write_is_idempotent() {
    let first = write_aux(&parse_aux(&activsg200()).unwrap());
    let again = write_aux(&parse_aux(&first).unwrap());
    assert_eq!(first, again);
}

/// The typed Network mapping reads the real export's power flow core.
#[test]
fn activsg200_maps_the_power_flow_core() {
    let net = parse_file(fixture("ACTIVSg200.aux"), None).unwrap().network;
    assert_eq!(net.buses.len(), 200);
    assert_eq!(net.generators.len(), 49);
    assert_eq!(net.loads.len(), 160);
    assert_eq!(net.shunts.len(), 4);
    assert_eq!(net.branches.len(), 246, "180 lines + 66 transformers");
    // The line impedances live past the first header line; the gap list's
    // headline failure was all-zero reactance.
    let nonzero_x = net.branches.iter().filter(|b| b.x != 0.0).count();
    assert!(
        nonzero_x >= 180,
        "expected line reactances to be read, got {nonzero_x} nonzero"
    );
}
