//! Experimental source-free input database construction shared by both families.
//!
//! This layer knows native storage and identity records, not electrical models.
//! The owning backend must validate its complete fresh output electrically before
//! returning it. A structurally valid SQLite database or archive is not evidence
//! that SINCAL's desktop opens or calculates the proposed exchange profile.

use crate::{DatabaseSnapshot, MAX_BYTES, Result, format_error};
use rusqlite::{Connection, params_from_iter, types::Value};
use std::{
    collections::BTreeMap,
    io::{Cursor, Write},
};
use zip::{CompressionMethod, DateTime, ZipWriter, write::SimpleFileOptions};

#[derive(Clone, Copy)]
pub enum ColumnKind {
    Integer,
    Real,
    Text,
}

impl ColumnKind {
    fn sql(self) -> &'static str {
        match self {
            Self::Integer => "INTEGER",
            Self::Real => "REAL",
            Self::Text => "TEXT",
        }
    }
    fn accepts(self, value: &Value) -> bool {
        match (self, value) {
            (_, Value::Null) | (Self::Integer, Value::Integer(_)) => true,
            (Self::Real, Value::Real(v)) => v.is_finite(),
            (Self::Text, Value::Text(s)) => s.len() <= 1 << 20,
            _ => false,
        }
    }
}

struct Table {
    columns: Vec<ColumnKind>,
    rows: usize,
}

/// Bounded, deterministic construction in memory. No template, file lookup or
/// source database can enter this API. Tables/fields and values are separately
/// validated; values always travel through SQLite bound parameters.
pub struct InputDatabase {
    connection: Connection,
    tables: BTreeMap<String, Table>,
    cells: usize,
}

fn identifier(s: &str) -> bool {
    let mut b = s.bytes();
    s.len() <= 128
        && b.next().is_some_and(|c| c.is_ascii_alphabetic())
        && b.all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

// Deliberately an input profile, not a general database or native project copier.
fn input_table(name: &str) -> bool {
    matches!(
        name,
        "Version"
            | "Variant"
            | "Node"
            | "Element"
            | "Terminal"
            | "VoltageLevel"
            | "CalcParameter"
            | "NetworkGroup"
            | "NetworkGroupTrans"
            | "Line"
            | "LineSeg"
            | "Load"
            | "Infeeder"
            | "DCInfeeder"
            | "TwoWindingTransformer"
            | "ShuntImpedance"
            | "ShuntCondensator"
    )
}

impl InputDatabase {
    /// Start an experimental schema-14.8 input database with one base variant.
    ///
    /// # Errors
    /// Returns SQLite allocation or initial construction errors.
    pub fn new() -> Result<Self> {
        let connection = Connection::open_in_memory().map_err(format_error)?;
        connection.execute_batch("PRAGMA page_size=4096; PRAGMA auto_vacuum=NONE; PRAGMA encoding='UTF-8'; PRAGMA max_page_count=16383;")
            .map_err(format_error)?;
        let mut db = Self {
            connection,
            tables: BTreeMap::new(),
            cells: 0,
        };
        db.create_table(
            "Version",
            &[
                ("Version_ID", ColumnKind::Integer),
                ("Version_No", ColumnKind::Real),
                ("Calc_Type", ColumnKind::Integer),
            ],
        )?;
        db.insert(
            "Version",
            &[Value::Integer(1), Value::Real(14.8), Value::Integer(1)],
        )?;
        db.create_table(
            "Variant",
            &[
                ("Variant_ID", ColumnKind::Integer),
                ("ParentVariant_ID", ColumnKind::Integer),
                ("Flag_Variant", ColumnKind::Integer),
            ],
        )?;
        db.insert(
            "Variant",
            &[Value::Integer(1), Value::Null, Value::Integer(1)],
        )?;
        Ok(db)
    }

    /// Declare typed columns; no implicit electrical defaults are supplied.
    ///
    /// # Errors
    /// Rejects unsupported tables, ambiguous/invalid names and oversized schemas.
    pub fn create_table(&mut self, name: &str, columns: &[(&str, ColumnKind)]) -> Result<()> {
        let mut seen = std::collections::BTreeSet::new();
        if !input_table(name)
            || self.tables.contains_key(name)
            || columns.is_empty()
            || columns.len() > 2048
            || columns
                .iter()
                .any(|(n, _)| !identifier(n) || !seen.insert(n.to_ascii_lowercase()))
        {
            return Err(format_error("invalid experimental input table declaration"));
        }
        let declaration = columns
            .iter()
            .map(|(n, k)| format!("\"{n}\" {}", k.sql()))
            .collect::<Vec<_>>()
            .join(",");
        self.connection
            .execute_batch(&format!("CREATE TABLE \"{name}\" ({declaration})"))
            .map_err(format_error)?;
        // Preserve bounded family-reader lookups for larger fresh outputs;
        // nonunique indexes leave duplicate identities visible to validation.
        if columns.iter().any(|(n, _)| *n == "Variant_ID") {
            for field in [
                "Element_ID",
                "Node_ID",
                "VoltLevel_ID",
                "Line_ID",
                "OpSer_ID",
            ] {
                if columns.iter().any(|(n, _)| *n == field) {
                    self.connection.execute_batch(&format!(
                        "CREATE INDEX \"pio_{name}_{field}\" ON \"{name}\" (Variant_ID, \"{field}\")"
                    )).map_err(format_error)?;
                }
            }
        }
        self.tables.insert(
            name.into(),
            Table {
                columns: columns.iter().map(|(_, k)| *k).collect(),
                rows: 0,
            },
        );
        Ok(())
    }

    /// Insert one complete row without implicit numeric/string conversions.
    ///
    /// # Errors
    /// Rejects mismatched cell types, nonfinite values and size limits.
    pub fn insert(&mut self, table: &str, row: &[Value]) -> Result<()> {
        let schema = self
            .tables
            .get_mut(table)
            .ok_or_else(|| format_error("undeclared input table"))?;
        if row.len() != schema.columns.len()
            || schema.rows >= 100_000
            || self.cells + row.len() > 4_000_000
            || !schema.columns.iter().zip(row).all(|(k, v)| k.accepts(v))
        {
            return Err(format_error("invalid or oversized experimental input row"));
        }
        let params = vec!["?"; row.len()].join(",");
        self.connection
            .execute(
                &format!("INSERT INTO \"{table}\" VALUES ({params})"),
                params_from_iter(row),
            )
            .map_err(format_error)?;
        self.cells += row.len();
        schema.rows += 1;
        Ok(())
    }

    /// Finish a structurally checked database. Electrical validation belongs
    /// to the calling backend and must follow this operation.
    ///
    /// # Errors
    /// Rejects malformed identities/references, variants and oversized output.
    pub fn finish(self) -> Result<Vec<u8>> {
        let integrity: String = self
            .connection
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .map_err(format_error)?;
        if integrity != "ok" {
            return Err(format_error("constructed database failed integrity check"));
        }
        let bytes = self
            .connection
            .serialize("main")
            .map_err(format_error)?
            .to_vec();
        if bytes.len() as u64 > MAX_BYTES - 4096 {
            return Err(format_error("experimental database exceeds output budget"));
        }
        let snapshot = DatabaseSnapshot::decode(&bytes, None)?;
        if snapshot.version.to_bits() != 14.8_f64.to_bits() || snapshot.nodes.is_empty() {
            return Err(format_error(
                "experimental output requires schema 14.8 and a nonempty network",
            ));
        }
        Ok(bytes)
    }
}

/// Package a freshly authored, electrically validated database. Fixed metadata
/// avoids timestamps, machine paths, copied results and opaque desktop files.
/// Native desktop acceptance is pending; this is not ordinary source echo.
///
/// # Errors
/// Rejects unsafe project names, excess bytes, non-profile database objects and
/// invalid structural records. It cannot validate a caller's electrical family.
pub fn candidate_archive(database: &[u8], name: &str) -> Result<Vec<u8>> {
    if name.is_empty()
        || name.len() > 128
        || !name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
    {
        return Err(format_error(
            "experimental project name requires 1–128 ASCII letters/digits/hyphens/underscores",
        ));
    }
    if database.len() as u64 > MAX_BYTES - 4096 {
        return Err(format_error("experimental database exceeds archive budget"));
    }
    let snapshot = DatabaseSnapshot::decode(database, None)?;
    if snapshot.version.to_bits() != 14.8_f64.to_bits() {
        return Err(format_error("experimental package requires schema 14.8"));
    }
    let mut objects = snapshot
        .connection
        .prepare("SELECT type,name,tbl_name FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*'")
        .map_err(format_error)?;
    let mut rows = objects.query([]).map_err(format_error)?;
    while let Some(row) = rows.next().map_err(format_error)? {
        let kind: String = row.get(0).map_err(format_error)?;
        let object: String = row.get(1).map_err(format_error)?;
        let owner: String = row.get(2).map_err(format_error)?;
        if !((kind == "table" && input_table(&object)) || (kind == "index" && input_table(&owner)))
        {
            return Err(format_error(
                "experimental archive contains a non-input database object",
            ));
        }
    }
    let entries = [
        ("SIArchive.ini".into(), &b"[Main]\r\nAppVersion=PSS SINCAL\r\nNetworkType=Electro\r\n"[..]),
        (format!("{name}_files/database.db"), database),
        (format!("{name}_files/database.ini"), &b"[Database]\r\nMODE=SQLITE\r\nFILE=database.db\r\n[Config]\r\nDisableStdDocHandling=0\r\n"[..]),
    ];
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .last_modified_time(
            DateTime::from_date_and_time(1980, 1, 1, 0, 0, 0).expect("fixed valid timestamp"),
        )
        .unix_permissions(0o644);
    let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
    for (path, bytes) in entries {
        archive.start_file(path, options).map_err(format_error)?;
        archive.write_all(bytes).map_err(format_error)?;
    }
    let bytes = archive.finish().map_err(format_error)?.into_inner();
    if bytes.len() as u64 > MAX_BYTES {
        return Err(format_error("experimental archive exceeds output budget"));
    }
    Ok(bytes)
}
