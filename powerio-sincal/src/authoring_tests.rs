use crate::{
    DatabaseSnapshot,
    authoring::{
        ColumnKind::{Integer as I, Real as R, Text as T},
        InputDatabase, candidate_archive,
    },
    database_bytes,
};
use rusqlite::{Connection, types::Value};
use std::io::{Cursor, Read};

fn topology(end: i64) -> InputDatabase {
    let mut db = InputDatabase::new().unwrap();
    db.create_table("Node", &[("Node_ID", I), ("Variant_ID", I), ("Name", T)])
        .unwrap();
    for id in [101, 500] {
        db.insert(
            "Node",
            &[
                Value::Integer(id),
                Value::Integer(1),
                Value::Text("quoted ' name; DROP TABLE Node;\nλ".into()),
            ],
        )
        .unwrap();
    }
    db.create_table(
        "Element",
        &[("Element_ID", I), ("Variant_ID", I), ("Type", T)],
    )
    .unwrap();
    db.insert(
        "Element",
        &[
            Value::Integer(7),
            Value::Integer(1),
            Value::Text("Line".into()),
        ],
    )
    .unwrap();
    db.create_table(
        "Terminal",
        &[
            ("Terminal_ID", I),
            ("Variant_ID", I),
            ("Element_ID", I),
            ("Node_ID", I),
            ("TerminalNo", I),
        ],
    )
    .unwrap();
    for (id, node, position) in [(9, 101, 1), (10, end, 2)] {
        db.insert(
            "Terminal",
            &[
                Value::Integer(id),
                Value::Integer(1),
                Value::Integer(7),
                Value::Integer(node),
                Value::Integer(position),
            ],
        )
        .unwrap();
    }
    db
}

#[test]
fn fresh_databases_and_archives_are_deterministic_and_contain_only_input_metadata() {
    let first = topology(500).finish().unwrap();
    assert_eq!(first, topology(500).finish().unwrap());
    let snapshot = DatabaseSnapshot::decode(&first, None).unwrap();
    assert_eq!(
        snapshot.nodes.iter().copied().collect::<Vec<_>>(),
        [101, 500]
    );
    assert_eq!(snapshot.terminals[&10].node, 500);
    let name: String = snapshot
        .connection
        .query_row("SELECT Name FROM Node WHERE Node_ID=101", [], |r| r.get(0))
        .unwrap();
    assert_eq!(name, "quoted ' name; DROP TABLE Node;\nλ");
    let package = candidate_archive(&first, "fresh_case").unwrap();
    assert_eq!(package, candidate_archive(&first, "fresh_case").unwrap());
    assert_eq!(database_bytes(&package).unwrap().as_ref(), first);
    let mut zip = zip::ZipArchive::new(Cursor::new(package)).unwrap();
    assert_eq!(zip.len(), 3);
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).unwrap();
        assert!(
            !std::path::Path::new(entry.name())
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("sin"))
        );
        assert_eq!(entry.last_modified().unwrap().year(), 1980);
        if entry.name().ends_with("database.ini") {
            let mut ini = String::new();
            entry.read_to_string(&mut ini).unwrap();
            assert!(ini.contains("FILE=database.db\r\n"));
            assert!(!ini.contains("/tmp/"));
        }
    }
}

#[test]
fn mismatched_cells_are_rejected_before_sqlite_can_apply_affinity_casts() {
    let mut db = topology(500);
    db.create_table("Line", &[("Element_ID", I), ("r", R), ("note", T)])
        .unwrap();
    for row in [
        vec![Value::Text("7".into()), Value::Real(0.1), Value::Null],
        vec![Value::Integer(7), Value::Text("0.1".into()), Value::Null],
        vec![Value::Integer(7), Value::Real(f64::NAN), Value::Null],
        vec![Value::Integer(7), Value::Real(f64::INFINITY), Value::Null],
        vec![Value::Integer(7), Value::Real(0.1), Value::Blob(vec![0])],
        vec![Value::Integer(7), Value::Real(0.1)],
        vec![
            Value::Integer(7),
            Value::Real(0.1),
            Value::Text("a".repeat((1 << 20) + 1)),
        ],
    ] {
        assert!(db.insert("Line", &row).is_err());
    }
    db.insert("Line", &[Value::Integer(7), Value::Real(0.0), Value::Null])
        .unwrap();
    let snapshot = DatabaseSnapshot::decode(&db.finish().unwrap(), None).unwrap();
    let (count, number, text): (i64, String, String) = snapshot
        .connection
        .query_row(
            "SELECT count(*),typeof(r),typeof(note) FROM Line",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!((count, number.as_str(), text.as_str()), (1, "real", "null"));
}

#[test]
fn malformed_topology_and_noninput_objects_never_produce_a_package() {
    assert!(topology(999).finish().is_err());
    assert!(InputDatabase::new().unwrap().finish().is_err());
    let mut db = topology(500);
    assert!(
        db.insert(
            "Node",
            &[Value::Integer(101), Value::Integer(1), Value::Null]
        )
        .is_ok()
    );
    assert!(db.finish().is_err());
    for name in [
        "ULFNodeResult",
        "MyTable",
        "Node; DROP TABLE Node",
        "../Node",
    ] {
        assert!(
            InputDatabase::new()
                .unwrap()
                .create_table(name, &[("Node_ID", I)])
                .is_err()
        );
    }
    for fields in [
        vec![("Node_ID", I), ("node_id", I)],
        vec![("Bad\"Name", I)],
        vec![],
    ] {
        assert!(
            InputDatabase::new()
                .unwrap()
                .create_table("Node", &fields)
                .is_err()
        );
    }
    let bytes = topology(500).finish().unwrap();
    for name in ["", "../escape", "a/b", "a\\b", "x.ini", "λ"] {
        assert!(candidate_archive(&bytes, name).is_err());
    }
    // Simulate an upstream author accidentally adding results/a view after
    // construction. The packaging boundary still rejects it.
    for sql in [
        "CREATE TABLE ULFNodeResult (Result_ID INTEGER)",
        "CREATE VIEW Extra AS SELECT * FROM Node",
        "INSERT INTO Variant VALUES (2,NULL,1)",
    ] {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .deserialize_read_exact("main", &bytes[..], bytes.len(), false)
            .unwrap();
        connection.execute_batch(sql).unwrap();
        let changed = connection.serialize("main").unwrap();
        assert!(
            candidate_archive(&changed, "fresh").is_err(),
            "accepted {sql}"
        );
    }
}
