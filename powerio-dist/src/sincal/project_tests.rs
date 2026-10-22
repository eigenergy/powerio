use powerio_core::Source;

use super::{
    project::NativeProject,
    tests::{MARKER, archive, database},
};

const PAYLOAD: &[u8] = b"Original synthetic opaque coupling bytes\0\xff\r\n";

fn database_with_coupling(edit: &str) -> Vec<u8> {
    database(&format!(
        "CREATE TABLE CouplingData (CoupData_ID INTEGER, Variant_ID INTEGER, Flag_Variant INTEGER, ProjName TEXT);
         INSERT INTO CouplingData VALUES (7,1,1,'circuit.cpl'); {edit}"
    ))
}

#[test]
fn archive_project_retains_original_bytes_and_resolves_its_own_coupling_file() {
    let db = database_with_coupling("");
    let bytes = archive(&[
        ("SIArchive.ini", MARKER),
        ("case_files/database.db", &db),
        (
            "case_files/database.ini",
            b"FILE=C:\\unrelated\\database.db",
        ),
        ("case_files/Leika/circuit.cpl", PAYLOAD),
        ("case_files/DIA/dia.001.db", b"unrelated display"),
    ]);
    let source = Source::from_memory("case.sinx", bytes.clone()).unwrap();
    let project = NativeProject::read(&source, None).unwrap();
    assert_eq!(project.source.primary_buffer().unwrap().bytes(), bytes);
    assert_eq!(
        project
            .source
            .root_buffer("case_files/database.db")
            .unwrap()
            .bytes(),
        db
    );
    assert_eq!(project.coupling_buffer(7).unwrap().bytes(), PAYLOAD);
    assert_eq!(project.source.acquired_buffers().len(), 6); // primary plus five entries
    assert_eq!(source.acquired_buffers().len(), 1); // original source is unchanged
}

#[test]
fn raw_memory_project_uses_only_supplied_database_relative_buffers() {
    let source = Source::from_memory("database.db", database_with_coupling(""))
        .unwrap()
        .with_named_buffer("Leika/circuit.cpl", PAYLOAD.to_vec())
        .unwrap();
    let project = NativeProject::read(&source, None).unwrap();
    assert_eq!(project.coupling_buffer(7).unwrap().bytes(), PAYLOAD);
    let missing = Source::from_memory("database.db", database_with_coupling("")).unwrap();
    let error = NativeProject::read(&missing, None)
        .unwrap()
        .coupling_buffer(7)
        .unwrap_err()
        .to_string();
    assert!(error.contains("CouplingData 7"), "{error}");
    assert!(error.contains("not supplied"), "{error}");
}

#[test]
fn archived_reference_never_falls_back_to_caller_buffers() {
    let db = database_with_coupling("");
    let bytes = archive(&[("SIArchive.ini", MARKER), ("case_files/database.db", &db)]);
    let source = Source::from_memory("case.sinx", bytes)
        .unwrap()
        .with_named_buffer("case_files/Leika/circuit.cpl", PAYLOAD.to_vec())
        .unwrap();
    let project = NativeProject::read(&source, None).unwrap();
    assert!(project.coupling_buffer(7).is_err());
}

#[test]
fn coupling_selection_is_variant_local_and_windows_subdirectories_are_relative() {
    let db = database_with_coupling(
        "INSERT INTO Variant VALUES (2,NULL,1);
         INSERT INTO CouplingData VALUES (7,2,1,'nested\\other.cpl');",
    );
    let source = Source::from_memory("database.db", db)
        .unwrap()
        .with_named_buffer("Leika/circuit.cpl", PAYLOAD.to_vec())
        .unwrap()
        .with_named_buffer("Leika/nested/other.cpl", b"second variant".to_vec())
        .unwrap();
    assert_eq!(
        NativeProject::read(&source, Some(1))
            .unwrap()
            .coupling_buffer(7)
            .unwrap()
            .bytes(),
        PAYLOAD
    );
    assert_eq!(
        NativeProject::read(&source, Some(2))
            .unwrap()
            .coupling_buffer(7)
            .unwrap()
            .bytes(),
        b"second variant"
    );
}

#[test]
fn coupling_rows_must_be_unambiguous_active_records() {
    for edit in [
        "DELETE FROM CouplingData",
        "UPDATE CouplingData SET Variant_ID=2",
        "UPDATE CouplingData SET Flag_Variant=0",
        "UPDATE CouplingData SET Flag_Variant=NULL",
        "UPDATE CouplingData SET ProjName=NULL",
        "INSERT INTO CouplingData SELECT * FROM CouplingData",
        "DROP TABLE CouplingData; CREATE VIEW CouplingData AS SELECT 7 AS CoupData_ID, 1 AS Variant_ID, 1 AS Flag_Variant, 'circuit.cpl' AS ProjName",
    ] {
        let source = Source::from_memory("database.db", database_with_coupling(edit))
            .unwrap()
            .with_named_buffer("Leika/circuit.cpl", PAYLOAD.to_vec())
            .unwrap();
        let project = NativeProject::read(&source, None).unwrap();
        assert!(project.coupling_buffer(7).is_err(), "{edit}");
    }
    let source = Source::from_memory("database.db", database_with_coupling("")).unwrap();
    let project = NativeProject::read(&source, None).unwrap();
    for id in [-1, 0, 8] {
        assert!(project.coupling_buffer(id).is_err());
    }
}

#[test]
fn coupling_project_names_cannot_escape_or_alias_the_leika_directory() {
    for name in [
        "",
        "../circuit.cpl",
        "nested/../../circuit.cpl",
        "/circuit.cpl",
        "C:\\circuit.cpl",
        "\\\\host\\circuit.cpl",
        "./circuit.cpl",
        "nested//circuit.cpl",
        "circuit.cpl.",
        "circuit.cpl ",
        "file:stream",
        "x\ncircuit.cpl",
    ] {
        let db = database_with_coupling(&format!("UPDATE CouplingData SET ProjName='{name}'"));
        let source = Source::from_memory("database.db", db).unwrap();
        let error = NativeProject::read(&source, None)
            .unwrap()
            .coupling_buffer(7)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("unsafe coupling project name"),
            "{name:?}: {error}"
        );
    }
}

#[test]
fn file_project_retains_sidecar_snapshot_and_never_follows_ini_database_paths() {
    let temp = tempfile::tempdir().unwrap();
    let folder = temp.path().join("case_files");
    std::fs::create_dir_all(folder.join("Leika")).unwrap();
    std::fs::write(folder.join("database.db"), database_with_coupling("")).unwrap();
    std::fs::write(folder.join("database.ini"), "FILE=../../wrong.db").unwrap();
    let coupling = folder.join("Leika/circuit.cpl");
    std::fs::write(&coupling, PAYLOAD).unwrap();
    let source = Source::open(folder.join("database.db")).unwrap();
    let project = NativeProject::read(&source, None).unwrap();
    assert_eq!(project.coupling_buffer(7).unwrap().bytes(), PAYLOAD);
    std::fs::write(coupling, b"changed after acquisition").unwrap();
    assert_eq!(project.coupling_buffer(7).unwrap().bytes(), PAYLOAD);
    assert_eq!(source.acquired_buffers().len(), 2);
}

#[cfg(unix)]
#[test]
fn coupling_file_symlinks_are_refused_by_source_acquisition() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("Leika")).unwrap();
    std::fs::write(temp.path().join("database.db"), database_with_coupling("")).unwrap();
    std::fs::write(temp.path().join("outside.cpl"), PAYLOAD).unwrap();
    std::os::unix::fs::symlink("../outside.cpl", temp.path().join("Leika/circuit.cpl")).unwrap();
    let source = Source::open(temp.path().join("database.db")).unwrap();
    let project = NativeProject::read(&source, None).unwrap();
    assert!(project.coupling_buffer(7).is_err());
}

#[test]
fn archived_files_cannot_also_be_parent_directories() {
    let db = database_with_coupling("");
    for parent in ["case_files", "case_files/Leika", "CASE_FILES/LEIKA"] {
        let bytes = archive(&[
            ("SIArchive.ini", MARKER),
            ("case_files/database.db", &db),
            ("case_files/Leika/circuit.cpl", PAYLOAD),
            (parent, b"file not directory"),
        ]);
        let source = Source::from_memory("case.sinx", bytes).unwrap();
        let error = NativeProject::read(&source, None)
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("parent directory"), "{error}");
    }
}

#[test]
fn authentic_archive_project_retains_every_packaged_file() {
    let bytes = include_bytes!("../../../tests/data/sincal/1-LV-rural1--0-sw.sinx");
    let source = Source::from_memory("native.sinx", bytes.to_vec()).unwrap();
    let project = NativeProject::read(&source, None).unwrap();
    assert_eq!(project.source.primary_buffer().unwrap().bytes(), bytes);
    assert_eq!(project.database.elements.len(), 32);
    assert!(project.source.acquired_buffers().len() > 2);
    assert!(project.coupling_buffer(1).is_err()); // no invented coupling record
}

#[test]
fn caller_supplied_coupling_payloads_obey_the_byte_limit() {
    let oversized = vec![0; super::acquisition::MAX_BYTES as usize + 1];
    let source = Source::from_memory("database.db", database_with_coupling(""))
        .unwrap()
        .with_named_buffer("Leika/circuit.cpl", oversized)
        .unwrap();
    let project = NativeProject::read(&source, None).unwrap();
    let error = project.coupling_buffer(7).unwrap_err().to_string();
    assert!(error.contains("64 MiB limit"), "{error}");
}

#[test]
fn archive_paths_are_bounded_before_ancestor_checks() {
    let db = database_with_coupling("");
    for (path, admitted) in [
        ("x".repeat(4096), true),
        ("x".repeat(4097), false),
        ("x/".repeat(63) + "leaf", true),
        ("x/".repeat(64) + "leaf", false),
        ("界".repeat(1366), false), // UTF-8 bytes, not character count
    ] {
        let bytes = archive(&[
            ("SIArchive.ini", MARKER),
            ("case_files/database.db", &db),
            (&path, b"bounded entry"),
        ]);
        let source = Source::from_memory("case.sinx", bytes).unwrap();
        let result = NativeProject::read(&source, None);
        if admitted {
            assert!(
                result.is_ok(),
                "{} bytes: {}",
                path.len(),
                result.err().unwrap()
            );
        } else {
            let error = result.err().unwrap().to_string();
            assert!(error.contains("path length/depth limit"), "{error}");
        }
    }
}
