//! Enumerated optional line defaults from Database Description (April 2014),
//! pp.18–19. Required impedances, lengths, voltage and sequence declarations
//! never pass through this path. Source NULLs remain unchanged.
use rusqlite::Row;

use super::{format_error, schema::NativeDatabase};
use crate::Result;

impl NativeDatabase {
    pub(super) fn line_optional_flag(
        &self,
        row: &Row<'_>,
        field: &'static str,
        defaulted: &mut Vec<&'static str>,
    ) -> Result<i64> {
        if !matches!(field, "Flag_Ll" | "Flag_Ground" | "Flag_Macro") {
            return Err(format_error(
                "field has no declared optional line flag default",
            ));
        }
        match row.get::<_, Option<i64>>(field).map_err(format_error)? {
            Some(value) => Ok(value),
            None if self.version.to_bits() == 11.5_f64.to_bits() => {
                defaulted.push(field);
                Ok(0)
            }
            None => Err(format_error(format!(
                "NULL line {field} outside schema-11.5 profile"
            ))),
        }
    }

    pub(super) fn line_optional_number(
        &self,
        row: &Row<'_>,
        field: &'static str,
        defaulted: &mut Vec<&'static str>,
    ) -> Result<f64> {
        let fallback = match field {
            "ParSys" | "fr" => 1.0,
            "va" => 0.0,
            "alpha" => 0.004,
            "fn" => 50.0,
            _ => {
                return Err(format_error(
                    "field has no declared optional line numeric default",
                ));
            }
        };
        match row.get::<_, Option<f64>>(field).map_err(format_error)? {
            Some(value) if value.is_finite() => Ok(value),
            None if self.version.to_bits() == 11.5_f64.to_bits() => {
                defaulted.push(field);
                Ok(fallback)
            }
            _ => Err(format_error(format!("invalid or unresolved line {field}"))),
        }
    }
}
