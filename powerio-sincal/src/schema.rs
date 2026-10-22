//! Bounded SQLite snapshot validation and variant-local native identities.
//!
//! No phase, sequence, load, source or model-family interpretation occurs here.
//! A validated snapshot is not a successfully parsed electrical network.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{Connection, limits::Limit};

use crate::{MAX_BYTES, Result, SQLITE_MAGIC, format_error};

const MAX_ROWS: usize = 100_000;

/// One validated base variant. IDs remain native IDs, never dense indices.
pub struct DatabaseSnapshot {
    pub connection: Connection,
    pub version: f64,
    pub variant: i64,
    pub nodes: BTreeSet<i64>,
    pub elements: BTreeMap<i64, String>,
    pub terminals: BTreeMap<i64, TerminalIdentity>,
    /// Present in the native catalog but deliberately not acquired.
    pub excluded_tables: Vec<String>,
    /// Claimed original MDB digest for a tool-assisted acquisition, not a
    /// cryptographic attestation that the internal records are authentic.
    pub source_digest: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct TerminalIdentity {
    pub element: i64,
    pub node: i64,
    pub position: i64,
}

impl DatabaseSnapshot {
    pub fn decode(bytes: &[u8], requested_variant: Option<i64>) -> Result<Self> {
        let connection = connect(bytes)?;
        Self::from_connection(connection, requested_variant, &[14.8, 15.0, 15.5, 16.0])
    }

    /// Decode explicit Access acquisition records. This does not admit Access
    /// bytes through `decode` or reinterpret records as a native SQLite export.
    pub fn decode_records(bytes: &[u8], requested_variant: Option<i64>) -> Result<Self> {
        let records = crate::TableRecords::decode(bytes)?;
        let digest = records.source_sha256().to_owned();
        let excluded = records.excluded_tables().to_vec();
        let connection = records.into_connection()?;
        configure_query_snapshot(&connection)?;
        let mut snapshot = Self::from_connection(connection, requested_variant, &[11.5, 12.8])?;
        snapshot.excluded_tables = excluded;
        snapshot.source_digest = Some(digest);
        Ok(snapshot)
    }

    fn from_connection(
        connection: Connection,
        requested_variant: Option<i64>,
        supported_versions: &[f64],
    ) -> Result<Self> {
        for (table, columns) in [
            ("Version", &["Version_ID", "Version_No", "Calc_Type"][..]),
            (
                "Variant",
                &["Variant_ID", "ParentVariant_ID", "Flag_Variant"],
            ),
            ("Node", &["Node_ID", "Variant_ID"]),
            ("Element", &["Element_ID", "Variant_ID", "Type"]),
            (
                "Terminal",
                &[
                    "Terminal_ID",
                    "Variant_ID",
                    "Element_ID",
                    "Node_ID",
                    "TerminalNo",
                ],
            ),
        ] {
            require_table(&connection, table, columns)?;
        }
        let version = read_version(&connection, supported_versions)?;
        let variant = select_variant(&connection, requested_variant)?;

        let nodes = read_rows(
            &connection,
            "SELECT Node_ID FROM Node WHERE Variant_ID=?1",
            variant,
            |row| row.get::<_, i64>(0),
        )?;
        let mut node_ids = BTreeSet::new();
        for id in nodes {
            if !node_ids.insert(id) {
                return Err(format_error(format!("duplicate Node_ID {id}")));
            }
        }
        let elements = read_rows(
            &connection,
            "SELECT Element_ID, Type FROM Element WHERE Variant_ID=?1",
            variant,
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )?;
        let mut element_ids = BTreeMap::new();
        for (id, kind) in elements {
            // Older exports pad the symbolic type to a fixed field width.
            // Only trailing ASCII spaces are padding, not arbitrary whitespace.
            let kind = kind.trim_end_matches(' ').to_owned();
            if kind.is_empty() {
                return Err(format_error(format!("empty element type at Element {id}")));
            }
            if element_ids.insert(id, kind).is_some() {
                return Err(format_error(format!("duplicate Element_ID {id}")));
            }
        }
        let terminals = read_rows(
            &connection,
            "SELECT Terminal_ID, Element_ID, Node_ID, TerminalNo FROM Terminal WHERE Variant_ID=?1",
            variant,
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    TerminalIdentity {
                        element: row.get(1)?,
                        node: row.get(2)?,
                        position: row.get(3)?,
                    },
                ))
            },
        )?;
        let mut terminal_ids = BTreeMap::new();
        let mut positions = BTreeSet::new();
        for (id, terminal) in terminals {
            if !node_ids.contains(&terminal.node) || !element_ids.contains_key(&terminal.element) {
                return Err(format_error(format!(
                    "unresolved Terminal {id} in variant {variant}"
                )));
            }
            if terminal.position < 1 || !positions.insert((terminal.element, terminal.position)) {
                return Err(format_error(format!(
                    "invalid or duplicate terminal position at Terminal {id}"
                )));
            }
            if terminal_ids.insert(id, terminal).is_some() {
                return Err(format_error(format!("duplicate Terminal_ID {id}")));
            }
        }
        Ok(Self {
            connection,
            version,
            variant,
            nodes: node_ids,
            elements: element_ids,
            terminals: terminal_ids,
            excluded_tables: Vec::new(),
            source_digest: None,
        })
    }
}

fn read_version(connection: &Connection, supported: &[f64]) -> Result<f64> {
    let mut versions = connection
        .prepare("SELECT Version_No, Calc_Type FROM Version")
        .map_err(format_error)?;
    let mut rows = versions.query([]).map_err(format_error)?;
    let row = rows
        .next()
        .map_err(format_error)?
        .ok_or_else(|| format_error("missing Version record"))?;
    let version: f64 = row.get(0).map_err(format_error)?;
    let calculation: i64 = row.get(1).map_err(format_error)?;
    if calculation != 1 || rows.next().map_err(format_error)?.is_some() {
        return Err(format_error("expected one electrical Version record"));
    }
    // This pins the observed database schema, not the product release.
    // Accepting another version requires schema evidence and fixtures.
    if !supported.iter().any(|v| v.to_bits() == version.to_bits()) {
        return Err(format_error(format!(
            "unsupported database schema {version}"
        )));
    }

    Ok(version)
}

fn select_variant(connection: &Connection, requested_variant: Option<i64>) -> Result<i64> {
    let mut variants = BTreeMap::new();
    let mut statement = connection
        .prepare("SELECT Variant_ID, ParentVariant_ID FROM Variant")
        .map_err(format_error)?;
    let mut rows = statement.query([]).map_err(format_error)?;
    while let Some(row) = rows.next().map_err(format_error)? {
        let id: i64 = row.get(0).map_err(format_error)?;
        let parent: Option<i64> = row.get(1).map_err(format_error)?;
        if variants.insert(id, parent).is_some() || variants.len() > MAX_ROWS {
            return Err(format_error(
                "duplicate variant ID or variant limit exceeded",
            ));
        }
    }
    let variant = match requested_variant {
        Some(id) => id,
        None if variants.len() == 1 => *variants.first_key_value().unwrap().0,
        None => {
            return Err(format_error(
                "select a variant explicitly unless exactly one exists",
            ));
        }
    };
    let parent = variants
        .get(&variant)
        .ok_or_else(|| format_error(format!("unknown variant {variant}")))?;
    if !matches!(parent, None | Some(0)) {
        return Err(format_error(
            "derived variant requires documented inheritance semantics",
        ));
    }

    Ok(variant)
}

fn read_rows<T>(
    connection: &Connection,
    sql: &str,
    variant: i64,
    decode: impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
) -> Result<Vec<T>> {
    let mut statement = connection.prepare(sql).map_err(format_error)?;
    let rows = statement
        .query_map([variant], decode)
        .map_err(format_error)?;
    let mut values = Vec::new();
    for row in rows {
        if values.len() == MAX_ROWS {
            return Err(format_error("native table row limit exceeded"));
        }
        values.push(row.map_err(format_error)?);
    }
    Ok(values)
}

pub fn require_table(connection: &Connection, table: &str, required: &[&str]) -> Result<()> {
    // A view or virtual table is not a native schema table. Do not evaluate it.
    let kind: String = connection
        .query_row(
            "SELECT type FROM pragma_table_list WHERE schema='main' AND name=?1",
            [table],
            |row| row.get(0),
        )
        .map_err(format_error)?;
    if kind != "table" {
        return Err(format_error(format!("expected native table {table}")));
    }
    let mut statement = connection
        .prepare("SELECT name FROM pragma_table_info(?1)")
        .map_err(format_error)?;
    let columns: BTreeSet<String> = statement
        .query_map([table], |row| row.get(0))
        .map_err(format_error)?
        .collect::<rusqlite::Result<_>>()
        .map_err(format_error)?;
    for column in required {
        if !columns.contains(*column) {
            return Err(format_error(format!("missing {table}.{column}")));
        }
    }
    Ok(())
}

fn connect(bytes: &[u8]) -> Result<Connection> {
    if bytes.len() as u64 > MAX_BYTES || bytes.len() < 100 || !bytes.starts_with(SQLITE_MAGIC) {
        return Err(format_error("expected bounded SQLite database snapshot"));
    }
    if bytes[18..20] != [1, 1] {
        return Err(format_error(
            "database must be a self-contained snapshot, not WAL",
        ));
    }
    let mut connection = Connection::open_in_memory().map_err(format_error)?;
    connection
        .deserialize_read_exact("main", bytes, bytes.len(), true)
        .map_err(format_error)?;
    configure_query_snapshot(&connection)?;
    Ok(connection)
}

fn configure_query_snapshot(connection: &Connection) -> Result<()> {
    connection
        .set_limit(Limit::SQLITE_LIMIT_LENGTH, 1 << 20)
        .map_err(format_error)?;
    connection
        .set_limit(Limit::SQLITE_LIMIT_SQL_LENGTH, 1 << 20)
        .map_err(format_error)?;
    connection
        .set_limit(Limit::SQLITE_LIMIT_ATTACHED, 0)
        .map_err(format_error)?;
    // The budget spans all queries, including schema inspection.
    let mut remaining = 10_000_u32;
    connection
        .progress_handler(
            1000,
            Some(move || {
                remaining = remaining.saturating_sub(1);
                remaining == 0
            }),
        )
        .map_err(format_error)?;
    connection
        .execute_batch("PRAGMA query_only=ON; PRAGMA trusted_schema=OFF; PRAGMA temp_store=MEMORY;")
        .map_err(format_error)?;
    Ok(())
}
