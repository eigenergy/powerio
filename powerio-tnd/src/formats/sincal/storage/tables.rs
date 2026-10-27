//! Internal typed acquisition records, not a native database exchange format.
//!
//! No tool is spawned here. The explicit MDB Tools adapter supplies records;
//! claimed source hashes are provenance metadata, not an authenticity proof.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::marker::PhantomData;

use rusqlite::{Connection, params_from_iter, types::Value};
use serde::Deserialize;
use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use sha2::{Digest, Sha256};

use crate::formats::sincal::storage::{MAX_BYTES, Result, format_error};

const MAX_TABLES: usize = 512;
const MAX_COLUMNS: usize = 2048;
const MAX_TABLE_ROWS: usize = 100_000;
const MAX_CELLS: usize = 4_000_000;
const MAX_TEXT_BYTES: usize = 1 << 20;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TableRecords {
    format: String,
    version: u32,
    transport: String,
    source: Origin,
    tools: BTreeMap<String, String>,
    // The table, row, column and cell caps apply while the document is
    // read, so an oversized document stops at the first element past a cap
    // instead of being deserialized whole and checked afterwards.
    #[serde(deserialize_with = "bounded_tables")]
    tables: Vec<Table>,
    excluded_tables: Vec<String>,
    absent_requested_tables: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Origin {
    name: String,
    sha256: String,
    bytes: u64,
}

struct Table {
    name: String,
    columns: Vec<Column>,
    rows: Vec<Vec<Cell>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Column {
    name: String,
    native_type: String,
}

// Do not coerce strings such as "0" into numbers or boolean flags into 0/1.
// Preserve integer overflow as an error rather than falling back to f64.
struct Cell(Value);

impl<'de> Deserialize<'de> for Cell {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        deserializer.deserialize_any(CellVisitor)
    }
}

/// Accept only scalar cells. An array or object fails at its first token
/// rather than after it has been read into a generic JSON value.
struct CellVisitor;

impl Visitor<'_> for CellVisitor {
    type Value = Cell;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a native cell: null, a bounded string, an i64 integer or a float")
    }

    fn visit_unit<E: de::Error>(self) -> std::result::Result<Cell, E> {
        Ok(Cell(Value::Null))
    }

    fn visit_none<E: de::Error>(self) -> std::result::Result<Cell, E> {
        Ok(Cell(Value::Null))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Cell, E> {
        Ok(Cell(Value::Integer(v)))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Cell, E> {
        i64::try_from(v)
            .map(|v| Cell(Value::Integer(v)))
            .map_err(|_| E::custom("unsupported or oversized native cell"))
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> std::result::Result<Cell, E> {
        Ok(Cell(Value::Real(v)))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Cell, E> {
        self.visit_string(v.to_owned())
    }

    fn visit_string<E: de::Error>(self, v: String) -> std::result::Result<Cell, E> {
        if v.len() <= MAX_TEXT_BYTES {
            Ok(Cell(Value::Text(v)))
        } else {
            Err(E::custom("unsupported or oversized native cell"))
        }
    }
}

fn bounded_tables<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Vec<Table>, D::Error> {
    struct Tables;
    impl<'de> Visitor<'de> for Tables {
        type Value = Vec<Table>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a bounded list of acquired tables")
        }

        fn visit_seq<A: SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> std::result::Result<Vec<Table>, A::Error> {
            let mut cells = 0;
            let mut tables = Vec::new();
            while let Some(table) = seq.next_element_seed(TableSeed { cells: &mut cells })? {
                if tables.len() == MAX_TABLES {
                    return Err(de::Error::custom("invalid acquired table count"));
                }
                tables.push(table);
            }
            Ok(tables)
        }
    }
    deserializer.deserialize_seq(Tables)
}

/// One table; `cells` counts every cell of the document read so far.
struct TableSeed<'a> {
    cells: &'a mut usize,
}

#[derive(Deserialize)]
#[serde(field_identifier, rename_all = "lowercase")]
enum TableField {
    Name,
    Columns,
    Rows,
}

impl<'de> DeserializeSeed<'de> for TableSeed<'_> {
    type Value = Table;

    fn deserialize<D: Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<Table, D::Error> {
        deserializer.deserialize_struct("Table", &["name", "columns", "rows"], self)
    }
}

impl<'de> Visitor<'de> for TableSeed<'_> {
    type Value = Table;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an acquired table")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<Table, A::Error> {
        let (mut name, mut columns, mut rows) = (None, None, None);
        while let Some(field) = map.next_key()? {
            match field {
                TableField::Name if name.is_none() => name = Some(map.next_value()?),
                TableField::Columns if columns.is_none() => {
                    columns = Some(map.next_value_seed(Bounded::<Column>::new(
                        MAX_COLUMNS,
                        "acquired table dimensions exceed limits",
                    ))?);
                }
                TableField::Rows if rows.is_none() => {
                    rows = Some(map.next_value_seed(RowsSeed {
                        cells: &mut *self.cells,
                    })?);
                }
                TableField::Name => return Err(de::Error::duplicate_field("name")),
                TableField::Columns => return Err(de::Error::duplicate_field("columns")),
                TableField::Rows => return Err(de::Error::duplicate_field("rows")),
            }
        }
        Ok(Table {
            name: name.ok_or_else(|| de::Error::missing_field("name"))?,
            columns: columns.ok_or_else(|| de::Error::missing_field("columns"))?,
            rows: rows.ok_or_else(|| de::Error::missing_field("rows"))?,
        })
    }
}

/// A list of at most `max` elements.
struct Bounded<T> {
    max: usize,
    message: &'static str,
    element: PhantomData<T>,
}

impl<T> Bounded<T> {
    fn new(max: usize, message: &'static str) -> Self {
        Self {
            max,
            message,
            element: PhantomData,
        }
    }
}

impl<'de, T: Deserialize<'de>> DeserializeSeed<'de> for Bounded<T> {
    type Value = Vec<T>;

    fn deserialize<D: Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<Vec<T>, D::Error> {
        deserializer.deserialize_seq(self)
    }
}

impl<'de, T: Deserialize<'de>> Visitor<'de> for Bounded<T> {
    type Value = Vec<T>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "a list of at most {} elements", self.max)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Vec<T>, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = seq.next_element()? {
            if values.len() == self.max {
                return Err(de::Error::custom(self.message));
            }
            values.push(value);
        }
        Ok(values)
    }
}

/// The rows of one table, at most `MAX_TABLE_ROWS`.
struct RowsSeed<'a> {
    cells: &'a mut usize,
}

impl<'de> DeserializeSeed<'de> for RowsSeed<'_> {
    type Value = Vec<Vec<Cell>>;

    fn deserialize<D: Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<Self::Value, D::Error> {
        deserializer.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for RowsSeed<'_> {
    type Value = Vec<Vec<Cell>>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a bounded list of acquired rows")
    }

    fn visit_seq<A: SeqAccess<'de>>(
        self,
        mut seq: A,
    ) -> std::result::Result<Self::Value, A::Error> {
        let mut rows = Vec::new();
        while let Some(row) = seq.next_element_seed(RowSeed {
            cells: &mut *self.cells,
        })? {
            if rows.len() == MAX_TABLE_ROWS {
                return Err(de::Error::custom("acquired table dimensions exceed limits"));
            }
            rows.push(row);
        }
        Ok(rows)
    }
}

/// One row, counted toward the document's cell cap cell by cell. A row
/// wider than the column cap can never match its declared schema.
struct RowSeed<'a> {
    cells: &'a mut usize,
}

impl<'de> DeserializeSeed<'de> for RowSeed<'_> {
    type Value = Vec<Cell>;

    fn deserialize<D: Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<Vec<Cell>, D::Error> {
        deserializer.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for RowSeed<'_> {
    type Value = Vec<Cell>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an acquired row")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Vec<Cell>, A::Error> {
        let mut row = Vec::new();
        while let Some(cell) = seq.next_element()? {
            if row.len() == MAX_COLUMNS {
                return Err(de::Error::custom(
                    "acquired row width differs from declared schema",
                ));
            }
            *self.cells += 1;
            if *self.cells > MAX_CELLS {
                return Err(de::Error::custom("acquired cell limit exceeded"));
            }
            row.push(cell);
        }
        Ok(row)
    }
}

fn identifier(name: &str) -> bool {
    let mut bytes = name.bytes();
    !name.is_empty()
        && name.len() <= 128
        && bytes
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
        && bytes.all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

impl TableRecords {
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() as u64 > MAX_BYTES {
            return Err(format_error("record document exceeds 64 MiB limit"));
        }
        let records: Self = serde_json::from_slice(bytes).map_err(format_error)?;
        records.validate()?;
        Ok(records)
    }

    pub fn source_sha256(&self) -> &str {
        &self.source.sha256
    }

    /// Bind caller-supplied acquisition records to the retained original MDB.
    /// This detects accidentally paired files; it does not attest that a
    /// caller or external acquisition tool exported every cell faithfully.
    pub fn verify_source(&self, bytes: &[u8]) -> Result<()> {
        if !bytes.starts_with(b"\0\x01\0\0Standard Jet DB\0")
            || bytes.len() as u64 != self.source.bytes
            || !format!("{:x}", Sha256::digest(bytes)).eq_ignore_ascii_case(&self.source.sha256)
        {
            return Err(format_error(
                "acquired tables do not match the original Access MDB bytes",
            ));
        }
        Ok(())
    }

    /// Tables deliberately not acquired, distinct from absent native tables.
    pub fn excluded_tables(&self) -> &[String] {
        &self.excluded_tables
    }

    fn validate(&self) -> Result<()> {
        if self.format != "powerio-sincal-tables"
            || self.version != 1
            || self.transport != "access-mdbtools"
            || self.source.name.is_empty()
            || self.source.name.len() > 1024
            // The MDB itself is bounded by the core primary-file limit, and
            // verify_source binds this declared size to its exact length.
            || self.source.bytes == 0
            || self.source.sha256.len() != 64
            || !self.source.sha256.bytes().all(|c| c.is_ascii_hexdigit())
            || self.tools.len() > 8
            || !["mdb-json", "mdb-schema", "mdb-tables"].iter().all(|tool| {
                self.tools
                    .get(*tool)
                    .is_some_and(|v| !v.is_empty() && v.len() <= 256)
            })
        {
            return Err(format_error("invalid acquisition identity/provenance"));
        }
        let mut names = BTreeSet::new();
        for name in self
            .tables
            .iter()
            .map(|t| &t.name)
            .chain(&self.excluded_tables)
            .chain(&self.absent_requested_tables)
        {
            if name.is_empty()
                || name.len() > 128
                || name.chars().any(char::is_control)
                || !names.insert(name.to_lowercase())
                || names.len() > 4096
            {
                return Err(format_error("ambiguous or invalid acquired table catalog"));
            }
        }
        if self.tables.is_empty() || self.tables.len() > MAX_TABLES {
            return Err(format_error("invalid acquired table count"));
        }
        let mut cells = 0_usize;
        for table in &self.tables {
            let mut columns = BTreeSet::new();
            if !identifier(&table.name)
                || table.columns.is_empty()
                || table.columns.len() > MAX_COLUMNS
                || table.rows.len() > MAX_TABLE_ROWS
            {
                return Err(format_error("acquired table dimensions exceed limits"));
            }
            for column in &table.columns {
                if !identifier(&column.name)
                    || !columns.insert(column.name.to_ascii_lowercase())
                    || column.native_type.is_empty()
                    || column.native_type.len() > 128
                {
                    return Err(format_error("ambiguous or invalid acquired column"));
                }
            }
            for row in &table.rows {
                if row.len() != table.columns.len() {
                    return Err(format_error(
                        "acquired row width differs from declared schema",
                    ));
                }
                cells += row.len();
                if cells > MAX_CELLS {
                    return Err(format_error("acquired cell limit exceeded"));
                }
            }
        }
        Ok(())
    }

    /// Materialize an internal query snapshot without SQLite affinity casts.
    /// Native declarations are metadata only and are never executed as SQL.
    pub(crate) fn into_connection(self) -> Result<Connection> {
        let mut connection = Connection::open_in_memory().map_err(format_error)?;
        let transaction = connection.transaction().map_err(format_error)?;
        for table in self.tables {
            let columns = table
                .columns
                .iter()
                .map(|c| format!("\"{}\"", c.name))
                .collect::<Vec<_>>()
                .join(",");
            transaction
                .execute_batch(&format!("CREATE TABLE \"{}\" ({columns})", table.name))
                .map_err(format_error)?;
            let placeholders = vec!["?"; table.columns.len()].join(",");
            let mut insert = transaction
                .prepare(&format!(
                    "INSERT INTO \"{}\" VALUES ({placeholders})",
                    table.name
                ))
                .map_err(format_error)?;
            for row in table.rows {
                insert
                    .execute(params_from_iter(row.iter().map(|cell| &cell.0)))
                    .map_err(format_error)?;
            }
            // Tool records omit native indexes. Repeated component lookups
            // otherwise scan whole tables and exhaust the shared query budget
            // on valid large feeders. These nonunique internal indexes preserve
            // duplicate/NULL evidence for the structural and electrical checks.
            if table.columns.iter().any(|c| c.name == "Variant_ID") {
                for field in [
                    "Element_ID",
                    "Node_ID",
                    "VoltLevel_ID",
                    "Line_ID",
                    "OpSer_ID",
                ] {
                    if table.columns.iter().any(|c| c.name == field) {
                        transaction
                            .execute_batch(&format!(
                                "CREATE INDEX \"pio_{}_{}\" ON \"{}\" (Variant_ID, \"{field}\")",
                                table.name, field, table.name,
                            ))
                            .map_err(format_error)?;
                    }
                }
            }
        }
        transaction.commit().map_err(format_error)?;
        Ok(connection)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document() -> serde_json::Value {
        serde_json::json!({
            "format":"powerio-sincal-tables", "version":1, "transport":"access-mdbtools",
            "source":{"name":"synthetic.mdb","sha256":"a".repeat(64),"bytes":1024},
            "tools":{"mdb-json":"test", "mdb-schema":"test", "mdb-tables":"test"},
            "tables":[{"name":"Load", "columns":[
                {"name":"id","native_type":"INTEGER"},
                {"name":"P","native_type":"REAL"},
                {"name":"Name","native_type":"varchar"}],
                "rows":[[1,0.0,"0"],[2,null,""]]}],
            "excluded_tables":["ULFNodeResult"],"absent_requested_tables":["LineSeg"]
        })
    }

    /// The document up to the open `rows` list of its one table, then
    /// `body`, then nothing: the document is cut off and is not valid JSON.
    fn truncated_after(body: &str) -> Vec<u8> {
        let mut doc = document();
        doc["tables"][0]["rows"] = serde_json::json!("ROWS");
        let text = serde_json::to_string(&doc).unwrap();
        let prefix = &text[..text.find("\"ROWS\"").unwrap()];
        format!("{prefix}[{body}").into_bytes()
    }

    fn decode_error(bytes: &[u8]) -> String {
        match TableRecords::decode(bytes) {
            Ok(_) => panic!("over-cap records accepted"),
            Err(error) => error.to_string(),
        }
    }

    #[test]
    fn decoding_stops_at_the_first_element_past_a_cap() {
        // Each document is cut off after its over-cap element. A decode that
        // read the whole document first would report the truncation instead.
        let complete = "[],".repeat(MAX_TABLE_ROWS);
        assert!(decode_error(&truncated_after(&complete)).contains("EOF"));
        let over = "[],".repeat(MAX_TABLE_ROWS + 1);
        assert!(
            decode_error(&truncated_after(&over))
                .contains("acquired table dimensions exceed limits")
        );
        let wide = format!("[{}null]", "null,".repeat(MAX_COLUMNS));
        assert!(
            decode_error(&truncated_after(&wide))
                .contains("acquired row width differs from declared schema")
        );
        // A nested cell fails at its first token, not after being read.
        assert!(decode_error(&truncated_after("[[[[[[[[")).contains("expected a native cell"));
        let mut doc = document();
        doc["tables"] = serde_json::json!("TABLES");
        let text = serde_json::to_string(&doc).unwrap();
        let prefix = &text[..text.find("\"TABLES\"").unwrap()];
        let table = r#"{"name":"T","columns":[{"name":"a","native_type":"x"}],"rows":[]},"#;
        let tables = format!("{prefix}[{}", table.repeat(MAX_TABLES + 1));
        assert!(decode_error(tables.as_bytes()).contains("invalid acquired table count"));
    }

    #[test]
    fn a_large_declared_mdb_is_bounded_by_the_primary_limit_not_a_record_cap() {
        // A 300 MiB MDB passes the default 1 GiB primary limit; the record
        // only has to state its exact length, which verify_source checks.
        let mut doc = document();
        doc["source"]["bytes"] = serde_json::json!(300_u64 << 20);
        let records = TableRecords::decode(&serde_json::to_vec(&doc).unwrap()).unwrap();
        assert!(
            records
                .verify_source(b"\0\x01\0\0Standard Jet DB\0")
                .is_err()
        );
    }

    #[test]
    fn acquired_origin_checks_header_length_and_digest_without_claiming_attestation() {
        // Synthetic header-shaped bytes, not a purported native fixture.
        let mut source = b"\0\x01\0\0Standard Jet DB\0synthetic origin".to_vec();
        let mut doc = document();
        doc["source"]["bytes"] = serde_json::json!(source.len());
        doc["source"]["sha256"] = serde_json::json!(format!("{:x}", Sha256::digest(&source)));
        let records = TableRecords::decode(&serde_json::to_vec(&doc).unwrap()).unwrap();
        records.verify_source(&source).unwrap();
        source[20] ^= 1;
        assert!(records.verify_source(&source).is_err());
        source.push(0);
        assert!(records.verify_source(&source).is_err());
        doc["source"]["bytes"] = serde_json::json!(source.len());
        source[0] = 1;
        doc["source"]["sha256"] = serde_json::json!(format!("{:x}", Sha256::digest(&source)));
        assert!(
            TableRecords::decode(&serde_json::to_vec(&doc).unwrap())
                .unwrap()
                .verify_source(&source)
                .is_err()
        );
    }

    #[test]
    fn component_lookups_stay_bounded_without_hiding_duplicate_or_null_cells() {
        let mut d = document();
        let mut rows: Vec<_> = (0..5000).map(|id| serde_json::json!([id, 1, id])).collect();
        rows.push(serde_json::json!([7, 2, null]));
        rows.push(serde_json::json!([7, 2, "0"]));
        d["tables"] = serde_json::json!([{
            "name": "Line", "columns": [
                {"name": "Element_ID", "native_type": "INTEGER"},
                {"name": "Variant_ID", "native_type": "INTEGER"},
                {"name": "r", "native_type": "REAL"}], "rows": rows
        }]);
        let connection = TableRecords::decode(&serde_json::to_vec(&d).unwrap())
            .unwrap()
            .into_connection()
            .unwrap();
        let duplicates: (i64, i64, i64) = connection.query_row(
            "SELECT count(*), sum(r IS NULL), sum(typeof(r)='text') FROM Line WHERE Variant_ID=2 AND Element_ID=7",
            [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        ).unwrap();
        assert_eq!(duplicates, (2, 1, 1));
        let mut remaining = 1000_u32;
        connection
            .progress_handler(
                1000,
                Some(move || {
                    remaining = remaining.saturating_sub(1);
                    remaining == 0
                }),
            )
            .unwrap();
        for id in 0..1000_i64 {
            let value: i64 = connection
                .query_row(
                    "SELECT r FROM Line WHERE Variant_ID=1 AND Element_ID=?1",
                    [id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(value, id);
        }
    }

    #[test]
    fn materialization_preserves_null_real_zero_text_and_missing_columns() {
        let record = TableRecords::decode(&serde_json::to_vec(&document()).unwrap()).unwrap();
        assert_eq!(record.excluded_tables(), ["ULFNodeResult"]);
        let connection = record.into_connection().unwrap();
        let cells: (String, f64, String) = connection
            .query_row("SELECT typeof(P), P, Name FROM Load WHERE id=1", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!(cells.0, "real");
        assert_eq!(cells.1.to_bits(), 0.0_f64.to_bits());
        assert_eq!(cells.2, "0");
        assert!(
            connection
                .query_row("SELECT P FROM Load WHERE id=2", [], |r| r
                    .get::<_, Option<f64>>(0))
                .unwrap()
                .is_none()
        );
        assert!(connection.prepare("SELECT absent FROM Load").is_err());
    }

    #[test]
    fn refuses_coercions_catalog_collisions_and_invalid_row_shapes() {
        let original = document();
        let mut cases = Vec::new();
        for value in [
            serde_json::json!(true),
            serde_json::json!(u64::MAX),
            serde_json::json!({}),
        ] {
            let mut d = original.clone();
            d["tables"][0]["rows"][0][1] = value;
            cases.push(d);
        }
        let mut d = original.clone();
        d["excluded_tables"] = serde_json::json!(["load"]);
        cases.push(d);
        let mut d = original.clone();
        d["tables"][0]["rows"][0] = serde_json::json!([1]);
        cases.push(d);
        let mut d = original;
        d["tables"][0]["name"] = "Load\"; DROP TABLE Node;".into();
        cases.push(d);
        for d in cases {
            assert!(TableRecords::decode(&serde_json::to_vec(&d).unwrap()).is_err());
        }
    }

    #[test]
    fn imported_schema_has_versioned_read_only_identity_validation() {
        fn table(name: &str, columns: &[&str], rows: serde_json::Value) -> serde_json::Value {
            let mut value = serde_json::json!({"name":name, "columns":columns.iter().map(|name|
                serde_json::json!({"name":name,"native_type":"synthetic"})).collect::<Vec<_>>(),
                "rows":null});
            value["rows"] = rows;
            value
        }
        let mut d = document();
        d["excluded_tables"] = serde_json::json!(["Paste Errors"]);
        d["tables"] = serde_json::json!([
            table(
                "Version",
                &["Version_ID", "Version_No", "Calc_Type"],
                serde_json::json!([[1, 11.5, 1]])
            ),
            table(
                "Variant",
                &["Variant_ID", "ParentVariant_ID", "Flag_Variant"],
                serde_json::json!([[1, null, 1]])
            ),
            table(
                "Node",
                &["Node_ID", "Variant_ID"],
                serde_json::json!([[100, 1], [500, 1]])
            ),
            table(
                "Element",
                &["Element_ID", "Variant_ID", "Type"],
                serde_json::json!([[90, 1, "Line"]])
            ),
            table(
                "Terminal",
                &[
                    "Terminal_ID",
                    "Variant_ID",
                    "Element_ID",
                    "Node_ID",
                    "TerminalNo"
                ],
                serde_json::json!([[15, 1, 90, 100, 1], [16, 1, 90, 500, 2]])
            )
        ]);
        let bytes = serde_json::to_vec(&d).unwrap();
        assert!(crate::formats::sincal::storage::DatabaseSnapshot::decode(&bytes, None).is_err());
        let db = crate::formats::sincal::storage::DatabaseSnapshot::decode_records(&bytes, None)
            .unwrap();
        assert_eq!(db.nodes, BTreeSet::from([100, 500]));
        assert_eq!(db.terminals[&16].node, 500);
        assert_eq!(db.excluded_tables, ["Paste Errors"]);
        assert_eq!(db.source_digest.as_deref(), Some("a".repeat(64).as_str()));
        assert!(db.connection.execute("DELETE FROM Node", []).is_err());
        assert!(
            db.connection
                .execute("ATTACH ':memory:' AS other", [])
                .is_err()
        );
        let mut changed = d.clone();
        changed["tables"][4]["rows"][0][3] = 999.into();
        assert!(
            crate::formats::sincal::storage::DatabaseSnapshot::decode_records(
                &serde_json::to_vec(&changed).unwrap(),
                None
            )
            .is_err()
        );
        d["tables"][0]["rows"][0][1] = 12.8.into();
        let newer = crate::formats::sincal::storage::DatabaseSnapshot::decode_records(
            &serde_json::to_vec(&d).unwrap(),
            None,
        )
        .unwrap();
        assert_eq!(newer.version.to_bits(), 12.8_f64.to_bits());
        assert_eq!(newer.nodes, db.nodes);
        assert_eq!(newer.terminals, db.terminals);
        d["tables"][0]["rows"][0][1] = 12.9.into();
        assert!(
            crate::formats::sincal::storage::DatabaseSnapshot::decode_records(
                &serde_json::to_vec(&d).unwrap(),
                None
            )
            .is_err()
        );
    }
}
