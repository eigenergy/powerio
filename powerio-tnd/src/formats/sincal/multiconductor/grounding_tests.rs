use num_complex::Complex64;

use super::{grounding::NeutralOwner, schema::NativeDatabase, semantics::State, tests::database};

fn neutral_database(edit: &str) -> Vec<u8> {
    database(&format!(
        "CREATE TABLE NeutralPointImp (
             Stp_ID INTEGER, Variant_ID INTEGER, Flag_Type INTEGER,
             Flag_Switch INTEGER DEFAULT 1, ComStp_ID INTEGER DEFAULT 0,
             Element_ID INTEGER, Node_ID INTEGER, Flag_CIrcitryZE INTEGER,
             RE REAL DEFAULT 0, XE REAL DEFAULT 0, Flag_Ground INTEGER DEFAULT 0,
             RG REAL, XG REAL);
         INSERT INTO NeutralPointImp (Stp_ID, Variant_ID, Flag_Type, Element_ID, RE, XE, ComStp_ID)
             VALUES (1, 1, 1, 30, 2, -0.5, 2);
         INSERT INTO NeutralPointImp (Stp_ID, Variant_ID, Flag_Type, Flag_Switch,
             RE, XE, Flag_Ground, RG, XG, Flag_CIrcitryZE)
             VALUES (2, 1, 3, 0, 3, 4, 1, 5, 6, 7);
         {edit}"
    ))
}

#[test]
fn shared_neutral_chain_preserves_topology_switches_and_physical_ohms() {
    let native = NativeDatabase::decode(&neutral_database(""), None).unwrap();
    let chain = native.neutral_chain(1).unwrap();
    assert_eq!(chain.len(), 2);
    assert_eq!(chain[0].id, 1);
    assert_eq!(chain[0].owner, NeutralOwner::Element(30));
    assert_eq!(chain[0].state, State::On);
    assert_eq!(chain[0].shared_neutral, Some(2));
    // No sequence factor of three, and no merging of the shared branch.
    assert!((chain[0].resistance_ohm - 2.0).abs() < 1e-15);
    assert!((chain[0].reactance_ohm + 0.5).abs() < 1e-15);
    assert!(chain[0].earth_ohm.is_none()); // Inactive RG/XG are NULL.
    assert_eq!(chain[1].owner, NeutralOwner::Shared);
    assert_eq!(chain[1].state, State::Off);
    assert!((chain[1].earth_ohm.unwrap() - Complex64::new(5.0, 6.0)).norm() < 1e-15);
    assert_eq!(chain[1].circuitry_code, Some(7)); // Retained, not interpreted.
}

#[test]
fn neutral_owner_and_shared_references_are_variant_local() {
    let bytes = neutral_database(
        "INSERT INTO Variant VALUES (2, NULL, 1);
         INSERT INTO Node VALUES (99, 2);
         INSERT INTO NeutralPointImp (Stp_ID, Variant_ID, Flag_Type, Node_ID) VALUES (1, 2, 2, 99);"
    );
    let second = NativeDatabase::decode(&bytes, Some(2)).unwrap();
    let chain = second.neutral_chain(1).unwrap();
    assert_eq!(chain.len(), 1);
    assert_eq!(chain[0].owner, NeutralOwner::Node(99));
    assert!(second.neutral_point(2).is_err());
    assert!(
        NativeDatabase::decode(
            &neutral_database("UPDATE NeutralPointImp SET Flag_Type=2, Node_ID=99 WHERE Stp_ID=1"),
            None
        )
        .unwrap()
        .neutral_point(1)
        .is_err()
    );
}

#[test]
fn neutral_chain_rejects_cycles_missing_links_and_invalid_active_fields() {
    for edit in [
        "UPDATE NeutralPointImp SET ComStp_ID=2 WHERE Stp_ID=2",
        "UPDATE NeutralPointImp SET ComStp_ID=999 WHERE Stp_ID=1",
        "UPDATE NeutralPointImp SET Flag_Type=1, Element_ID=30 WHERE Stp_ID=2",
        "UPDATE NeutralPointImp SET Element_ID=999 WHERE Stp_ID=1",
        "UPDATE NeutralPointImp SET Flag_Switch=2 WHERE Stp_ID=1",
        "UPDATE NeutralPointImp SET Flag_Ground=2 WHERE Stp_ID=1",
        "UPDATE NeutralPointImp SET RG=NULL WHERE Stp_ID=2",
        "UPDATE NeutralPointImp SET RE=-1 WHERE Stp_ID=1",
        "UPDATE NeutralPointImp SET XE=1e999 WHERE Stp_ID=1",
        "INSERT INTO NeutralPointImp SELECT * FROM NeutralPointImp WHERE Stp_ID=1",
    ] {
        let native = NativeDatabase::decode(&neutral_database(edit), None).unwrap();
        assert!(native.neutral_chain(1).is_err(), "accepted {edit}");
    }
}
