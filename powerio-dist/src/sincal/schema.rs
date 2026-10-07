//! Family-local ownership and error adaptation for shared schema validation.

use std::ops::Deref;

use powerio_sincal::DatabaseSnapshot;
use rusqlite::Connection;

use super::format_error;
use crate::Result;

// Keep electrical interpretation as inherent methods of this local owner.
// The shared snapshot supplies identities and the bounded query-only database.
pub(super) struct NativeDatabase(DatabaseSnapshot);

impl NativeDatabase {
    pub fn from_snapshot(snapshot: DatabaseSnapshot) -> Result<Self> {
        // Structural admission of modern balanced schemas does not authorize
        // their conductor semantics here. Keep family support independent.
        if ![11.5_f64, 14.8]
            .iter()
            .any(|v| v.to_bits() == snapshot.version.to_bits())
        {
            return Err(format_error(format!(
                "unsupported multiconductor electrical schema {}",
                snapshot.version
            )));
        }
        Ok(Self(snapshot))
    }

    /// Schema 11.5 retains the legacy line-line input convention when the
    /// later voltage-kind selector is NULL. Siemens General Input Data,
    /// April 2014 pp.18,28 explicitly defines network-level Un as line-line,
    /// including single-phase networks. Do not extend this legacy default to
    /// newer schemas or confuse a missing referenced row with a NULL field.
    pub fn line_line_voltage_basis(&self, selector: Option<i64>) -> Result<bool> {
        match selector {
            Some(1) => Ok(false),
            None if self.version.to_bits() == 11.5_f64.to_bits() => Ok(true),
            _ => Err(format_error(
                "voltage level requires line-line Flag_Volt=1 or the recorded schema-11.5 legacy default",
            )),
        }
    }

    /// Standard-type selection fills the equipment's stored fields. Read
    /// those materialized electrical values strictly; do not follow the UI's
    /// local/global library path or substitute values from another catalog.
    pub fn materialized_type(row: &rusqlite::Row<'_>) -> Result<()> {
        let id = super::transformer::reference(row, "Typ_ID")?;
        if id.is_some() {
            let scope: i64 = row
                .get("Flag_Typ_ID")
                .map_err(|e| format_error(format!("Typ_ID requires valid Flag_Typ_ID: {e}")))?;
            if !matches!(scope, 1 | 2) {
                return Err(format_error(
                    "Typ_ID requires local/global materialized type selection",
                ));
            }
        }
        Ok(())
    }

    pub fn legacy_integer(
        &self,
        row: &rusqlite::Row<'_>,
        field: &str,
        default: i64,
    ) -> Result<i64> {
        let value: Option<i64> = row.get(field).map_err(format_error)?;
        match value {
            Some(value) => Ok(value),
            None if matches!(
                field,
                "Flag_Tap" | "Flag_Ct" | "Flag_ConNode" | "Flag_Macro"
            ) && self.version.to_bits() == 11.5_f64.to_bits() =>
            {
                Ok(default)
            }
            None => Err(format_error(format!(
                "NULL {field} outside schema-11.5 legacy profile"
            ))),
        }
    }

    /// Explicitly enumerated zero defaults in the April 2014 Database
    /// Description pp.45–46. Required ratings and impedances never use this
    /// path. Retain NULL in the source and record its interpretation later.
    pub fn legacy_transformer_number(&self, row: &rusqlite::Row<'_>, field: &str) -> Result<f64> {
        let value: Option<f64> = row.get(field).map_err(format_error)?;
        match value {
            Some(value) if value.is_finite() => Ok(value),
            None if self.version.to_bits() == 11.5_f64.to_bits()
                && matches!(
                    field,
                    "AddRotate" | "roh" | "rohm" | "ukr" | "alpha" | "phi" | "Vfe" | "i0"
                ) =>
            {
                Ok(0.0)
            }
            _ => Err(format_error(format!(
                "invalid or unresolved transformer {field}"
            ))),
        }
    }

    /// A control added after 11.5 is not an active input in that layout.
    /// Call sites enumerate the affected controls; missing fields in newer
    /// schemas and explicit NULL/non-numeric values still fail validation.
    pub fn newer_integer(&self, row: &rusqlite::Row<'_>, field: &str, legacy: i64) -> Result<i64> {
        if matches!(field, "Flag_Pctrl" | "Flag_Boost" | "Flag_Lf")
            && self.version.to_bits() == 11.5_f64.to_bits()
            && row.as_ref().column_index(field).is_err()
        {
            return Ok(legacy);
        }
        super::transformer::integer(row, field)
    }
    pub fn newer_number(&self, row: &rusqlite::Row<'_>, field: &str) -> Result<f64> {
        if matches!(field, "Rlf" | "Xlf" | "C01" | "C02")
            && self.version.to_bits() == 11.5_f64.to_bits()
            && row.as_ref().column_index(field).is_err()
        {
            return Ok(0.0);
        }
        super::transformer::number(row, field)
    }
    pub fn newer_reference(&self, row: &rusqlite::Row<'_>, field: &str) -> Result<Option<i64>> {
        if matches!(
            field,
            "ElemLoading_ID" | "Ctrl_OpSer_ID" | "Ctrl_OpPnt_ID" | "CtrlRange_ID"
        ) && self.version.to_bits() == 11.5_f64.to_bits()
            && row.as_ref().column_index(field).is_err()
        {
            return Ok(None);
        }
        super::transformer::reference(row, field)
    }

    pub fn decode(bytes: &[u8], requested_variant: Option<i64>) -> Result<Self> {
        Self::from_snapshot(
            DatabaseSnapshot::decode(bytes, requested_variant).map_err(format_error)?,
        )
    }
}

impl Deref for NativeDatabase {
    type Target = DatabaseSnapshot;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub(super) fn require_table(connection: &Connection, table: &str, required: &[&str]) -> Result<()> {
    powerio_sincal::require_table(connection, table, required).map_err(format_error)
}

#[cfg(test)]
pub(super) fn verify_read_only(bytes: &[u8]) {
    let database = NativeDatabase::decode(bytes, None).unwrap();
    assert!(database.connection.execute("DELETE FROM Node", []).is_err());
    assert!(
        database
            .connection
            .execute("ATTACH DATABASE ':memory:' AS other", [])
            .is_err()
    );
}
