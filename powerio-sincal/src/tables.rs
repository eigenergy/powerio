//! Internal typed acquisition records, not a native database exchange format.
//!
//! No tool is spawned here. The explicit MDB Tools adapter supplies records;
//! claimed source hashes are provenance metadata, not an authenticity proof.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{Connection, params_from_iter, types::Value};
use serde::Deserialize;

use crate::{MAX_BYTES, Result, format_error};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TableRecords {
    format: String,
    version: u32,
    transport: String,
    source: Origin,
    tools: BTreeMap<String, String>,
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
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
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        Ok(Self(match value {
            serde_json::Value::Null => Value::Null,
            serde_json::Value::String(text) if text.len() <= 1 << 20 => Value::Text(text),
            serde_json::Value::Number(number) if number.is_i64() => {
                Value::Integer(number.as_i64().expect("checked signed number"))
            }
            serde_json::Value::Number(number) if number.is_f64() => {
                Value::Real(number.as_f64().expect("checked float"))
            }
            _ => {
                return Err(serde::de::Error::custom(
                    "unsupported or oversized native cell",
                ));
            }
        }))
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
            || self.source.bytes == 0
            || self.source.bytes > 256 * 1024 * 1024
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
        if self.tables.is_empty() || self.tables.len() > 512 {
            return Err(format_error("invalid acquired table count"));
        }
        let mut cells = 0_usize;
        for table in &self.tables {
            let mut columns = BTreeSet::new();
            if !identifier(&table.name)
                || table.columns.is_empty()
                || table.columns.len() > 2048
                || table.rows.len() > 100_000
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
                if cells > 4_000_000 {
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
        assert!(crate::DatabaseSnapshot::decode(&bytes, None).is_err());
        let db = crate::DatabaseSnapshot::decode_records(&bytes, None).unwrap();
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
            crate::DatabaseSnapshot::decode_records(&serde_json::to_vec(&changed).unwrap(), None)
                .is_err()
        );
        d["tables"][0]["rows"][0][1] = 12.8.into();
        assert!(
            crate::DatabaseSnapshot::decode_records(&serde_json::to_vec(&d).unwrap(), None)
                .is_err()
        );
    }
}
