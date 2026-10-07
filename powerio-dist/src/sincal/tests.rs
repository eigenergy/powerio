use std::io::{Cursor, Write};

use powerio_core::Source;
use rusqlite::Connection;
use zip::{ZipWriter, write::SimpleFileOptions};

use super::{acquisition::database_bytes, read, schema::NativeDatabase};

const AUTHENTIC: &[u8] = include_bytes!("../../../tests/data/sincal/1-LV-rural1--0-sw.sinx");
pub(super) const MARKER: &[u8] = b"[Main]\nAppVersion=PSS SINCAL\nNetworkType=Electro\n";

// Deliberately minimal topology only. This is not a claim that SINCAL accepts
// the synthetic database as a complete electrical project.
pub(super) fn database(edit: &str) -> Vec<u8> {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(
            "
        CREATE TABLE Version (Version_ID INTEGER, Version_No REAL, Calc_Type INTEGER);
        INSERT INTO Version VALUES (1, 14.8, 1);
        CREATE TABLE Variant (Variant_ID INTEGER, ParentVariant_ID INTEGER, Flag_Variant INTEGER);
        INSERT INTO Variant VALUES (1, NULL, 1);
        CREATE TABLE Node (Node_ID INTEGER, Variant_ID INTEGER);
        INSERT INTO Node VALUES (10, 1), (20, 1);
        CREATE TABLE Element (Element_ID INTEGER, Variant_ID INTEGER, Type TEXT);
        INSERT INTO Element VALUES (30, 1, 'Line');
        CREATE TABLE Terminal (Terminal_ID INTEGER, Variant_ID INTEGER, Element_ID INTEGER,
                               Node_ID INTEGER, TerminalNo INTEGER);
        INSERT INTO Terminal VALUES (40, 1, 30, 10, 1), (50, 1, 30, 20, 2);
    ",
        )
        .unwrap();
    connection.execute_batch(edit).unwrap();
    connection.serialize("main").unwrap().to_vec()
}

pub(super) fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        writer
            .start_file(
                *name,
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn decode_error(edit: &str, variant: Option<i64>) -> String {
    match NativeDatabase::decode(&database(edit), variant) {
        Ok(_) => panic!("invalid database accepted: {edit}"),
        Err(error) => error.to_string(),
    }
}

#[test]
fn authentic_archive_and_raw_snapshot_have_identical_topology() {
    let source = Source::from_memory("case.sinx", AUTHENTIC).unwrap();
    let decoded = read(&source, None).unwrap();
    assert_eq!(
        (
            decoded.nodes.len(),
            decoded.elements.len(),
            decoded.terminals.len()
        ),
        (15, 32, 46)
    );
    assert_eq!(
        decoded
            .elements
            .values()
            .filter(|kind| *kind == "DCInfeeder")
            .count(),
        4
    );
    assert_eq!(decoded.variant, 1);
    assert_eq!(decoded.version.to_bits(), 14.8_f64.to_bits());
    let bytes = database_bytes(AUTHENTIC).unwrap();
    let raw = NativeDatabase::decode(&bytes, Some(1)).unwrap();
    assert_eq!(raw.nodes, decoded.nodes);
    assert_eq!(raw.elements, decoded.elements);
    assert_eq!(raw.terminals, decoded.terminals);
    assert_eq!(source.primary_buffer().unwrap().bytes(), AUTHENTIC);
    super::schema::verify_read_only(&bytes);
}

#[test]
fn identities_and_references_are_variant_local() {
    let edits = "INSERT INTO Variant VALUES (2, NULL, 1);
        INSERT INTO Node VALUES (10, 2), (99, 2);
        INSERT INTO Element VALUES (30, 2, 'Load');
        INSERT INTO Terminal VALUES (40, 2, 30, 99, 1);";
    assert!(decode_error(edits, None).contains("select a variant"));
    let second = NativeDatabase::decode(&database(edits), Some(2)).unwrap();
    assert_eq!(second.terminals[&40].node, 99);
    assert_eq!(second.elements[&30], "Load");
    let bad = format!("{edits} UPDATE Terminal SET Node_ID=99 WHERE Variant_ID=1;");
    assert!(decode_error(&bad, Some(1)).contains("unresolved Terminal"));
    assert!(decode_error("", Some(99)).contains("unknown variant"));
    assert!(decode_error("UPDATE Variant SET ParentVariant_ID=2", None).contains("inheritance"));
}

#[test]
fn duplicate_identity_and_terminal_positions_are_rejected() {
    for (edit, expected) in [
        ("INSERT INTO Node VALUES (10, 1)", "duplicate Node_ID"),
        (
            "INSERT INTO Element VALUES (30, 1, 'Load')",
            "duplicate Element_ID",
        ),
        (
            "UPDATE Terminal SET Terminal_ID=40",
            "duplicate Terminal_ID",
        ),
        (
            "UPDATE Terminal SET TerminalNo=1",
            "duplicate terminal position",
        ),
        (
            "UPDATE Terminal SET TerminalNo=0",
            "invalid or duplicate terminal position",
        ),
        (
            "INSERT INTO Variant VALUES (1, NULL, 1)",
            "duplicate variant",
        ),
        ("UPDATE Node SET Node_ID=NULL", "Invalid column type"),
    ] {
        let error = decode_error(edit, None);
        assert!(error.contains(expected), "{edit}: {error}");
    }
}

#[test]
fn refuses_unknown_schema_and_non_electrical_databases() {
    for (edit, expected) in [
        (
            "UPDATE Version SET Version_No=99",
            "unsupported database schema",
        ),
        ("UPDATE Version SET Calc_Type=2", "electrical Version"),
        (
            "INSERT INTO Version VALUES (2, 14.8, 1)",
            "electrical Version",
        ),
        ("DELETE FROM Version", "missing Version"),
        (
            "ALTER TABLE Node RENAME COLUMN Node_ID TO wrong",
            "missing Node.Node_ID",
        ),
        (
            "DROP TABLE Node; CREATE VIEW Node AS SELECT 10 AS Node_ID, 1 AS Variant_ID",
            "expected native table Node",
        ),
    ] {
        assert!(decode_error(edit, None).contains(expected), "{edit}");
    }
    assert!(NativeDatabase::decode(b"not SQLite", None).is_err());
    let mut wal = database("");
    wal[18] = 2;
    assert!(NativeDatabase::decode(&wal, None).is_err());
    assert!(NativeDatabase::decode(&wal[..100], None).is_err());
}

#[test]
fn selects_project_database_without_following_ini_paths() {
    let db = database("");
    let bytes = archive(&[
        ("SIArchive.ini", MARKER),
        ("case_files/database.db", &db),
        ("case_files/database.ini", b"FILE=C:\\other\\database.db"),
        ("case_files/DIA/dia.001.db", b"unrelated diagram database"),
    ]);
    assert_eq!(database_bytes(&bytes).unwrap().as_ref(), db);
    for entries in [
        vec![
            ("SIArchive.ini", MARKER),
            ("case_files/DIA/database.db", &db),
        ],
        vec![
            ("SIArchive.ini", MARKER),
            ("a_files/database.db", &db),
            ("b_files/database.db", &db),
        ],
        vec![("case_files/database.db", &db)],
        vec![
            (
                "SIArchive.ini",
                b"[Main]\nAppVersion=Other\nNetworkType=Electro".as_slice(),
            ),
            ("case_files/database.db", &db),
        ],
    ] {
        assert!(database_bytes(&archive(&entries)).is_err());
    }
}

#[test]
fn archive_paths_and_marker_are_unambiguous() {
    let db = database("");
    for name in [
        "../escape",
        "/absolute",
        "C:/drive",
        "a\\b",
        "a//b",
        "a/./b",
        "a/../b",
    ] {
        let bytes = archive(&[
            ("SIArchive.ini", MARKER),
            ("case_files/database.db", &db),
            (name, b""),
        ]);
        assert!(database_bytes(&bytes).is_err(), "{name}");
    }
    let bytes = archive(&[
        ("SIArchive.ini", MARKER),
        ("case_files/database.db", &db),
        ("CASE_FILES/DATABASE.DB", b""),
    ]);
    assert!(database_bytes(&bytes).is_err());
    for marker in [
        b"[Other]\nAppVersion=PSS SINCAL\nNetworkType=Electro\n".as_slice(),
        b"[Main]\nAppVersion=PSS SINCAL\nNetworkType=Electro\nNetworkType=Gas\n",
    ] {
        assert!(
            database_bytes(&archive(&[
                ("SIArchive.ini", marker),
                ("case_files/database.db", &db)
            ]))
            .is_err()
        );
    }
}

#[test]
fn rejects_archive_symlinks_and_excessive_expansion() {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .add_symlink("link", "elsewhere", SimpleFileOptions::default())
        .unwrap();
    assert!(database_bytes(&writer.finish().unwrap().into_inner()).is_err());
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file(
            "bomb",
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated),
        )
        .unwrap();
    writer.write_all(&vec![0; 1 << 20]).unwrap();
    let error = database_bytes(&writer.finish().unwrap().into_inner())
        .unwrap_err()
        .to_string();
    assert!(error.contains("compression ratio"));
}

pub(super) fn sequence_database(edit: &str) -> Vec<u8> {
    database(&format!(
        "
        ALTER TABLE Element ADD COLUMN Flag_Input INTEGER;
        UPDATE Element SET Flag_Input=7;
        CREATE TABLE CalcParameter (Variant_ID INTEGER, Flag_LFZ0 INTEGER);
        INSERT INTO CalcParameter VALUES (1, 1);
        CREATE TABLE Line (Element_ID INTEGER, Variant_ID INTEGER, Flag_Z0_Input INTEGER,
            Typ_ID INTEGER, CoupData_ID INTEGER, ParSys REAL, fr REAL, l REAL, Ith REAL,
            r REAL, x REAL, c REAL, r0 REAL, x0 REAL, c0 REAL, fn REAL,
            R0_R1 REAL, X0_X1 REAL);
        INSERT INTO Line VALUES (30, 1, 2, 0, 0, 1, 1, 0.25, 0.2,
            0.3, 0.1, 10, 0.9, 0.4, 4, 50, NULL, NULL);
        {edit}
    "
    ))
}

fn near(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() <= 1e-12 * expected.abs().max(1e-12),
        "{actual} != {expected}"
    );
}

#[test]
fn sequence_line_preserves_unequal_phase_drops_and_charging() {
    let database = NativeDatabase::decode(&sequence_database(""), None).unwrap();
    let line = database.sequence_line(30).unwrap();
    assert_eq!(line.element_id, 30);
    near(line.length_m, 250.0);
    assert_eq!(line.code.n_conductors, 3); // No invented neutral wire.
    assert_eq!(line.code.i_max, Some(vec![200.0; 3]));
    // Independently evaluated drops for phase currents [10, 4, 0] A.
    // Native self/mutual impedance is (0.5+j0.2)/(0.2+j0.1) ohm/km.
    let currents = [10.0, 4.0, 0.0];
    for (i, (resistive, reactive)) in [(1.45, 0.6), (1.0, 0.45), (0.7, 0.35)]
        .into_iter()
        .enumerate()
    {
        let drop_r: f64 = line.code.r_series[i]
            .iter()
            .zip(currents)
            .map(|(r, current)| r * current * line.length_m)
            .sum();
        let drop_x: f64 = line.code.x_series[i]
            .iter()
            .zip(currents)
            .map(|(x, current)| x * current * line.length_m)
            .sum();
        near(drop_r, resistive);
        near(drop_x, reactive);
    }
    // Row sum gives zero-sequence capacitance; diagonal minus mutual gives
    // positive sequence. Sum both pi halves before comparing total charging.
    let charging = &line.code.b_from;
    let omega = std::f64::consts::TAU * 50.0;
    near(charging[0].iter().sum::<f64>() * 2.0, omega * 4e-12);
    near((charging[0][0] - charging[0][1]) * 2.0, omega * 10e-12);
    assert_eq!(line.code.b_from, line.code.b_to);
    assert_eq!(line.code.g_from, vec![vec![0.0; 3]; 3]);
}

#[test]
fn balanced_native_fixture_does_not_supply_zero_sequence_data() {
    let source = Source::from_memory("case.sinx", AUTHENTIC).unwrap();
    let native = read(&source, None).unwrap();
    let mut count = 0;
    for (id, kind) in &native.elements {
        if kind == "Line" {
            let Err(error) = native.sequence_line(*id) else {
                panic!("balanced fixture cannot establish zero-sequence data");
            };
            assert!(
                error
                    .to_string()
                    .contains("explicit zero-sequence data required")
            );
            count += 1;
        }
    }
    assert_eq!(count, 13);
}

#[test]
fn sequence_line_refuses_unresolved_models_and_calculation_overrides() {
    for edit in [
        "UPDATE CalcParameter SET Flag_LFZ0=2",
        "UPDATE CalcParameter SET Variant_ID=2",
        "INSERT INTO CalcParameter VALUES (1, 1)",
        "UPDATE Element SET Flag_Input=3",
        "UPDATE Line SET Flag_Z0_Input=1",
        "UPDATE Line SET Typ_ID=4",
        "UPDATE Line SET CoupData_ID=4",
        "UPDATE Line SET ParSys=0",
        "UPDATE Line SET fr=0",
        "UPDATE Line SET r0=NULL",
        "UPDATE Line SET x0=-1",
        "UPDATE Line SET c0=-1",
        "UPDATE Line SET fn=0",
        "UPDATE Line SET l=0",
        "UPDATE Line SET Ith=0",
        "UPDATE Line SET Variant_ID=2",
        "INSERT INTO Line SELECT * FROM Line",
    ] {
        let native = NativeDatabase::decode(&sequence_database(edit), None).unwrap();
        assert!(native.sequence_line(30).is_err(), "{edit}");
    }
}

#[test]
fn sequence_line_agrees_with_existing_dss_conversion() {
    let native = NativeDatabase::decode(&sequence_database(""), None).unwrap();
    let code = native.sequence_line(30).unwrap().code;
    let dss = crate::testkit::parse_str(
        "Set DefaultBaseFrequency=50\nNew Linecode.lc nphases=3 units=km r1=0.3 x1=0.1 c1=10 r0=0.9 x0=0.4 c0=4 basefreq=50 normamps=200",
        "dss").unwrap();
    let other = &dss.line_codes()[0];
    for (a, b) in [
        (&code.r_series, &other.r_series),
        (&code.x_series, &other.x_series),
        (&code.b_from, &other.b_from),
        (&code.b_to, &other.b_to),
    ] {
        for (a, b) in a.iter().flatten().zip(b.iter().flatten()) {
            near(*a, *b);
        }
    }
}

#[test]
fn sequence_ratios_read_only_the_active_impedance_fields() {
    let direct = NativeDatabase::decode(
        &sequence_database("UPDATE Line SET R0_R1=-1, X0_X1=NULL"),
        None,
    )
    .unwrap()
    .sequence_line(30)
    .unwrap();
    let ratios = NativeDatabase::decode(
        &sequence_database("UPDATE Line SET Flag_Z0_Input=1, R0_R1=3, X0_X1=4, r0=NULL, x0=-1"),
        None,
    )
    .unwrap()
    .sequence_line(30)
    .unwrap();
    for (a, b) in [
        (&direct.code.r_series, &ratios.code.r_series),
        (&direct.code.x_series, &ratios.code.x_series),
        (&direct.code.b_from, &ratios.code.b_from),
    ] {
        for (a, b) in a.iter().flatten().zip(b.iter().flatten()) {
            near(*a, *b);
        }
    }
}

#[test]
fn parallel_and_rating_factors_preserve_unbalanced_voltage_drops() {
    let db = NativeDatabase::decode(&sequence_database("UPDATE Line SET ParSys=2, fr=0.8"), None)
        .unwrap();
    let line = db.sequence_line(30).unwrap();
    near(line.length_m, 250.0);
    assert_eq!(line.code.i_max, Some(vec![320.0; 3]));
    // Total currents [20, 8, 0] A split equally between identical circuits.
    // Their drops equal the independently calculated single-circuit values.
    for (i, (r, x)) in [(1.45, 0.6), (1.0, 0.45), (0.7, 0.35)]
        .into_iter()
        .enumerate()
    {
        for (matrix, expected) in [(&line.code.r_series, r), (&line.code.x_series, x)] {
            let drop: f64 = matrix[i]
                .iter()
                .zip([20.0, 8.0, 0.0])
                .map(|(z, current)| z * current * line.length_m)
                .sum();
            near(drop, expected);
        }
    }
    let omega = std::f64::consts::TAU * 50.0;
    near(line.code.b_from[0].iter().sum::<f64>(), omega * 4e-12);
    near(
        line.code.b_from[0][0] - line.code.b_from[0][1],
        omega * 10e-12,
    );
    assert_eq!(line.code.b_from, line.code.b_to);
    // Rating derating must not alter the electrical circuit.
    let unscaled_rating =
        NativeDatabase::decode(&sequence_database("UPDATE Line SET ParSys=2"), None)
            .unwrap()
            .sequence_line(30)
            .unwrap();
    assert_eq!(line.code.r_series, unscaled_rating.code.r_series);
    assert_eq!(line.code.x_series, unscaled_rating.code.x_series);
    assert_eq!(line.code.b_from, unscaled_rating.code.b_from);
}

#[test]
fn operating_frequency_changes_reactance_and_charging_only() {
    let db = NativeDatabase::decode(&sequence_database(""), None).unwrap();
    let rated = db.sequence_line(30).unwrap();
    let changed = db.sequence_line(30).unwrap().at_frequency(60.0).unwrap();
    assert_eq!(rated.code.r_series, changed.code.r_series);
    assert_eq!(rated.code.i_max, changed.code.i_max);
    near(rated.length_m, changed.length_m);
    for (a, b) in [
        (&rated.code.x_series, &changed.code.x_series),
        (&rated.code.b_from, &changed.code.b_from),
        (&rated.code.b_to, &changed.code.b_to),
    ] {
        for (a, b) in a.iter().flatten().zip(b.iter().flatten()) {
            near(*b, *a * 1.2);
        }
    }
    let unchanged = changed.at_frequency(60.0).unwrap();
    let restored = unchanged.at_frequency(50.0).unwrap();
    for (a, b) in rated
        .code
        .x_series
        .iter()
        .flatten()
        .zip(restored.code.x_series.iter().flatten())
    {
        near(*a, *b);
    }
    for invalid in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::from_bits(1)] {
        assert!(db.sequence_line(30).unwrap().at_frequency(invalid).is_err());
    }
}

#[test]
fn line_scaling_rejects_invalid_or_unrepresentable_parameters() {
    for edit in [
        "UPDATE Line SET ParSys=-1",
        "UPDATE Line SET ParSys=1e999",
        "UPDATE Line SET ParSys=1e-320",
        "UPDATE Line SET fr=-1",
        "UPDATE Line SET fr=1e999",
        "UPDATE Line SET fr=1e308",
        "UPDATE Line SET Flag_Z0_Input=1, R0_R1=-1, X0_X1=4",
        "UPDATE Line SET Flag_Z0_Input=1, R0_R1=3, X0_X1=NULL",
        "UPDATE Line SET Flag_Z0_Input=1, R0_R1=1e308, r=10, X0_X1=4",
    ] {
        let db = NativeDatabase::decode(&sequence_database(edit), None).unwrap();
        assert!(db.sequence_line(30).is_err(), "accepted {edit}");
    }
}

#[test]
fn sequence_input_categories_allow_independent_extra_data() {
    for flag in [6, 7, 4103, 4231] {
        let native = NativeDatabase::decode(
            &sequence_database(&format!("UPDATE Element SET Flag_Input={flag}")),
            None,
        )
        .unwrap();
        assert!(native.sequence_line(30).is_ok(), "Flag_Input={flag}");
    }
    for flag in [0, 2, 3, 4, -1] {
        let native = NativeDatabase::decode(
            &sequence_database(&format!("UPDATE Element SET Flag_Input={flag}")),
            None,
        )
        .unwrap();
        assert!(native.sequence_line(30).is_err(), "Flag_Input={flag}");
    }
}

#[test]
fn electrical_ports_keep_native_order_phases_and_independent_states() {
    use super::semantics::{Connection, State};
    let bytes = database(
        "ALTER TABLE Terminal ADD COLUMN Flag_Terminal INTEGER DEFAULT 6;
         ALTER TABLE Terminal ADD COLUMN Flag_State INTEGER DEFAULT 1;
         ALTER TABLE Element ADD COLUMN Flag_State INTEGER DEFAULT 0;
         UPDATE Terminal SET Terminal_ID=90 WHERE TerminalNo=1;
         UPDATE Terminal SET Flag_State=0, Flag_Terminal=8 WHERE TerminalNo=2;",
    );
    let native = NativeDatabase::decode(&bytes, None).unwrap();
    let ports = native.electrical_terminals(30).unwrap();
    assert_eq!(ports.iter().map(|p| p.id).collect::<Vec<_>>(), [90, 50]);
    assert_eq!(ports[0].connection, Connection::L31);
    assert_eq!(ports[0].connection.phases(), Some(&[2, 0][..]));
    assert_eq!(ports[1].connection, Connection::Neutral);
    assert_eq!(ports[1].connection.phases(), None);
    assert_eq!(ports[0].state, State::On);
    assert_eq!(ports[1].state, State::Off);
    assert_eq!(native.element_state(30).unwrap(), State::Off);
    assert_eq!(Connection::decode(3).unwrap().phases(), Some(&[2][..]));
    assert!(Connection::decode(15).is_err());
    assert!(Connection::decode(0).is_err());
    assert!(State::decode(2).is_err());
}

pub(super) fn load_database(edit: &str) -> Vec<u8> {
    database(&format!(
        "UPDATE Element SET Type='Load';
         DELETE FROM Terminal WHERE TerminalNo=2;
         ALTER TABLE Terminal ADD COLUMN Flag_Terminal INTEGER DEFAULT 7;
         ALTER TABLE Terminal ADD COLUMN Flag_State INTEGER DEFAULT 1;
         ALTER TABLE Element ADD COLUMN Flag_State INTEGER DEFAULT 1;
         ALTER TABLE Element ADD COLUMN Flag_Input INTEGER DEFAULT 2;
         CREATE TABLE Load (
             Element_ID INTEGER, Variant_ID INTEGER, Flag_Load INTEGER DEFAULT 1,
             Flag_LoadType INTEGER DEFAULT 2, Flag_Lf INTEGER DEFAULT 1,
             P REAL DEFAULT 0.006, Q REAL DEFAULT 0.002,
             P1 REAL DEFAULT 0.001, P2 REAL DEFAULT 0.002, P3 REAL DEFAULT 0.003,
             Q1 REAL DEFAULT 0.0001, Q2 REAL DEFAULT 0.0002, Q3 REAL DEFAULT 0.0003,
             P12 REAL DEFAULT 0.004, P23 REAL DEFAULT 0.005, P31 REAL DEFAULT 0.006,
             Q12 REAL DEFAULT -0.001, Q23 REAL DEFAULT 0, Q31 REAL DEFAULT 0.001,
             fP REAL DEFAULT 2, fQ REAL DEFAULT 0.5, fS REAL DEFAULT 0.65,
             S REAL DEFAULT 0.2, cosphi REAL DEFAULT 0.9,
             u REAL DEFAULT 100, Ul REAL DEFAULT 0.433,
             Typ_ID INTEGER, Mpl_ID INTEGER, Gang_ID INTEGER, Load_ID INTEGER,
             IncrSer_ID INTEGER, Macro_ID INTEGER, Ireg REAL DEFAULT 0,
             Flag_Typified INTEGER DEFAULT 0, Stp_ID INTEGER,
             DayOpSer_ID INTEGER, WeekOpSer_ID INTEGER, YearOpSer_ID INTEGER);
         INSERT INTO Load (Element_ID, Variant_ID) VALUES (30, 1);
         {edit}"
    ))
}

#[test]
fn authentic_load_inputs_match_paired_powers() {
    use super::load::{LoadModel, PowerInput, VoltageInput};
    let csv = include_str!("../../../tests/data/sincal/simbench-csv/Load.csv");
    let expected: std::collections::BTreeMap<_, _> = csv
        .lines()
        .skip(1)
        .map(|line| {
            let columns: Vec<_> = line.split(';').collect();
            (
                columns[0],
                (
                    columns[3].parse::<f64>().unwrap() * 1e6,
                    columns[4].parse::<f64>().unwrap() * 1e6,
                ),
            )
        })
        .collect();
    let native = read(&Source::from_memory("case.sinx", AUTHENTIC).unwrap(), None).unwrap();
    let mut count = 0;
    let mut total_p = 0.0;
    for (&id, kind) in &native.elements {
        if kind != "Load" {
            continue;
        }
        let input = native.load_input(id).unwrap();
        assert_eq!(input.element, id);
        assert_eq!(input.model, LoadModel::Power);
        assert_eq!(input.voltage, VoltageInput::Relative(1.0));
        let PowerInput::Total { p, q } = input.power else {
            panic!("expected aggregate input")
        };
        let name: String = native
            .connection
            .query_row(
                "SELECT Name FROM Element WHERE Element_ID=?1 AND Variant_ID=?2",
                [id, native.variant],
                |row| row.get(0),
            )
            .unwrap();
        let (expected_p, expected_q) = expected[name.trim()];
        near(p, expected_p);
        near(q, expected_q);
        total_p += p;
        count += 1;
    }
    assert_eq!(count, 13);
    near(total_p, 80_000.0);
}

#[test]
fn load_modes_select_only_active_fields_and_factors() {
    use super::load::{LoadModel, PowerInput, VoltageInput};
    let edits = "UPDATE Load SET Flag_LoadType=1, Flag_Lf=4,
                 P=NULL, Q=NULL, fP=NULL, fQ=NULL, u=NULL, DayOpSer_ID=8, Stp_ID=9;
                 UPDATE Terminal SET Flag_Terminal=2, Flag_State=0;
                 UPDATE Element SET Flag_State=0;";
    let native = NativeDatabase::decode(&load_database(edits), None).unwrap();
    let input = native.load_input(30).unwrap();
    assert_eq!(input.model, LoadModel::Impedance);
    assert_eq!(input.voltage, VoltageInput::Absolute(433.0));
    assert_eq!(input.operating_series, [Some(8), None, None]);
    assert_eq!(input.neutral_point, Some(9));
    assert_eq!(input.terminal.connection, super::semantics::Connection::L2);
    assert_eq!(input.terminal.state, super::semantics::State::Off);
    assert_eq!(input.state, super::semantics::State::Off);
    let PowerInput::Total { p, q } = input.power else {
        panic!("expected aggregate input")
    };
    near(p, 117_000.0); // 200 kVA * 0.65 * 0.9, not the inactive P/fP fields.
    near(q, 130_000.0 * 0.19_f64.sqrt());

    let native = NativeDatabase::decode(&load_database(
        "UPDATE Load SET Flag_LoadType=3, Flag_Lf=11, cosphi=0.8, S=NULL, fS=NULL, Q=NULL, fQ=NULL;"
    ), None).unwrap();
    let input = native.load_input(30).unwrap();
    assert_eq!(input.model, LoadModel::Current);
    let PowerInput::Total { p, q } = input.power else {
        panic!("expected aggregate input")
    };
    near(p, 12_000.0);
    near(q, 9000.0);
}

#[test]
fn unequal_wye_and_delta_powers_preserve_branch_order() {
    use super::load::PowerInput;
    for (mode, expected) in [
        (
            13,
            PowerInput::Wye {
                p: [2000.0, 4000.0, 6000.0],
                q: [50.0, 100.0, 150.0],
            },
        ),
        (
            14,
            PowerInput::Delta {
                p: [8000.0, 10_000.0, 12_000.0],
                q: [-500.0, 0.0, 500.0],
            },
        ),
    ] {
        let native = NativeDatabase::decode(
            &load_database(&format!(
                "UPDATE Load SET Flag_Lf={mode}, P=NULL, Q=NULL, cosphi=NULL, S=NULL;"
            )),
            None,
        )
        .unwrap();
        assert_eq!(native.load_input(30).unwrap().power, expected);
    }
}

#[test]
fn load_input_rejects_ambiguous_or_unresolved_active_data() {
    for edit in [
        "UPDATE Load SET Flag_Load=2",
        "UPDATE Load SET Flag_LoadType=4",
        "UPDATE Load SET Flag_Lf=9",
        "UPDATE Load SET Typ_ID=8",
        "UPDATE Load SET Ireg=1",
        "UPDATE Load SET Flag_Typified=1",
        "UPDATE Load SET P=NULL",
        "UPDATE Load SET u=0",
        "UPDATE Load SET Flag_Lf=4, Ul=-1",
        "UPDATE Load SET Flag_Lf=4, cosphi=1.01",
        "UPDATE Load SET Flag_Lf=11, cosphi=0",
        "UPDATE Load SET P=1e308, fP=1e308",
        "UPDATE Load SET DayOpSer_ID=-1",
        "UPDATE Element SET Flag_Input=1",
        "UPDATE Terminal SET Flag_Terminal=99",
        "UPDATE Terminal SET Flag_Terminal=8",
        "UPDATE Load SET Variant_ID=2",
        "INSERT INTO Load SELECT * FROM Load",
        "DELETE FROM Terminal",
    ] {
        let native = NativeDatabase::decode(&load_database(edit), None).unwrap();
        assert!(native.load_input(30).is_err(), "{edit}");
    }
}

#[test]
fn electrical_load_inputs_do_not_mix_variants() {
    use super::load::PowerInput;
    let bytes = load_database(
        "INSERT INTO Variant VALUES (2, NULL, 1);
         INSERT INTO Node VALUES (99, 2);
         INSERT INTO Element (Element_ID, Variant_ID, Type, Flag_State, Flag_Input)
             VALUES (30, 2, 'Load', 1, 2);
         INSERT INTO Terminal (Terminal_ID, Variant_ID, Element_ID, Node_ID, TerminalNo,
                               Flag_Terminal, Flag_State) VALUES (40, 2, 30, 99, 1, 3, 1);
         INSERT INTO Load (Element_ID, Variant_ID, P, Q, fP, fQ)
             VALUES (30, 2, 0.01, 0.001, 1, 1);",
    );
    let first = NativeDatabase::decode(&bytes, Some(1))
        .unwrap()
        .load_input(30)
        .unwrap();
    let second = NativeDatabase::decode(&bytes, Some(2))
        .unwrap()
        .load_input(30)
        .unwrap();
    assert_eq!(first.terminal.node, 10);
    assert_eq!(second.terminal.node, 99);
    assert_eq!(second.terminal.connection, super::semantics::Connection::L3);
    assert_eq!(
        first.power,
        PowerInput::Total {
            p: 12_000.0,
            q: 1000.0
        }
    );
    assert_eq!(
        second.power,
        PowerInput::Total {
            p: 10_000.0,
            q: 1000.0
        }
    );
}

#[test]
fn shared_balanced_schema_admission_does_not_select_conductor_semantics() {
    for version in [15.5, 16.0] {
        let bytes = database(&format!("UPDATE Version SET Version_No={version}"));
        let snapshot = powerio_sincal::DatabaseSnapshot::decode(&bytes, None).unwrap();
        let error = crate::__read_sincal_multiconductor_snapshot(snapshot).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unsupported multiconductor electrical schema")
        );
    }
}
