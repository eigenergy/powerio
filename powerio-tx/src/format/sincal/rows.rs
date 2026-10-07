//! Strict native fields: absence, NULL and numeric zero remain distinct.
use std::collections::BTreeMap;

use powerio_sincal::DatabaseSnapshot;
use rusqlite::types::Value;

use super::{Result, error};

pub(super) struct NativeRow {
    pub label: String,
    fields: BTreeMap<String, Value>,
}

impl NativeRow {
    pub fn number(&self, field: &str) -> Result<f64> {
        let value = match self.fields.get(&field.to_ascii_lowercase()) {
            Some(Value::Integer(v)) => *v as f64,
            Some(Value::Real(v)) => *v,
            _ => return Err(self.bad(field, "expected a non-NULL number")),
        };
        if value.is_finite() {
            Ok(value)
        } else {
            Err(self.bad(field, "nonfinite number"))
        }
    }

    pub fn integer(&self, field: &str) -> Result<i64> {
        match self.fields.get(&field.to_ascii_lowercase()) {
            Some(Value::Integer(v)) => Ok(*v),
            _ => Err(self.bad(field, "expected a non-NULL integer")),
        }
    }

    pub fn text(&self, field: &str) -> Result<String> {
        match self.fields.get(&field.to_ascii_lowercase()) {
            Some(Value::Text(v)) => Ok(v.clone()),
            _ => Err(self.bad(field, "expected non-NULL text")),
        }
    }

    pub fn positive(&self, field: &str) -> Result<f64> {
        let v = self.number(field)?;
        if v > 0.0 {
            Ok(v)
        } else {
            Err(self.bad(field, "expected positive value"))
        }
    }

    pub fn nonnegative(&self, field: &str) -> Result<f64> {
        let v = self.number(field)?;
        if v >= 0.0 {
            Ok(v)
        } else {
            Err(self.bad(field, "expected nonnegative value"))
        }
    }

    pub fn equals(&self, field: &str, expected: i64) -> Result<()> {
        if self.integer(field)? == expected {
            Ok(())
        } else {
            Err(self.bad(field, format!("supported value is {expected}")))
        }
    }

    pub fn state(&self, field: &str) -> Result<bool> {
        match self.integer(field)? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(self.bad(field, "expected native off/on state 0/1")),
        }
    }

    /// References/optional inactive modes may be explicitly NULL, but a
    /// missing column is a schema failure, never an assumed zero.
    pub fn inactive(&self, fields: &[&str]) -> Result<()> {
        for field in fields {
            match self.fields.get(&field.to_ascii_lowercase()) {
                Some(Value::Null | Value::Integer(0)) => (),
                Some(Value::Real(v)) if *v == 0.0 => (),
                _ => return Err(self.bad(field, "active or unavailable input requires mapping")),
            }
        }
        Ok(())
    }

    pub fn bad(&self, field: &str, reason: impl std::fmt::Display) -> crate::Error {
        error(format!("{}.{field}: {reason}", self.label))
    }
}

pub(super) fn table(
    db: &DatabaseSnapshot,
    name: &str,
    id: &str,
) -> Result<BTreeMap<i64, NativeRow>> {
    // Both identifiers are adapter constants, never native strings.
    powerio_sincal::require_table(&db.connection, name, &[id, "Variant_ID"]).map_err(error)?;
    let mut stmt = db
        .connection
        .prepare(&format!("SELECT * FROM \"{name}\" WHERE Variant_ID=?1"))
        .map_err(error)?;
    let cols = stmt
        .column_names()
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut rows = stmt.query([db.variant]).map_err(error)?;
    let mut result = BTreeMap::new();
    while let Some(row) = rows.next().map_err(error)? {
        if result.len() == 100_000 {
            return Err(error(format!("{name}: row limit exceeded")));
        }
        let key: i64 = row.get(id).map_err(error)?;
        let fields = cols
            .iter()
            .enumerate()
            .map(|(i, col)| {
                Ok((
                    col.to_ascii_lowercase(),
                    row.get::<_, Value>(i).map_err(error)?,
                ))
            })
            .collect::<Result<_>>()?;
        let value = NativeRow {
            label: format!("{name}[{key}]"),
            fields,
        };
        if result.insert(key, value).is_some() {
            return Err(error(format!("duplicate {name}[{key}]")));
        }
    }
    Ok(result)
}

pub(super) fn get<'a>(
    rows: &'a BTreeMap<i64, NativeRow>,
    id: i64,
    table: &str,
) -> Result<&'a NativeRow> {
    rows.get(&id)
        .ok_or_else(|| error(format!("missing {table}[{id}]")))
}
