//! Record interpretations without moving electrical behavior into metadata.

use super::{format_error, schema::NativeDatabase, transformer::reference};
use crate::{MulticonductorNetwork, Result};

impl NativeDatabase {
    pub fn record_interpretation(&self, net: &mut MulticonductorNetwork) -> Result<()> {
        // Equipment may reference a voltage level which no node references.
        let mut levels = self
            .connection
            .prepare(
                "SELECT DISTINCT v.VoltLevel_ID, v.Flag_Volt FROM VoltageLevel v
             JOIN Element e ON e.VoltLevel_ID=v.VoltLevel_ID AND e.Variant_ID=v.Variant_ID
             WHERE e.Variant_ID=?1",
            )
            .map_err(format_error)?;
        let mut rows = levels.query([self.variant]).map_err(format_error)?;
        while let Some(row) = rows.next().map_err(format_error)? {
            if self.line_line_voltage_basis(row.get(1).map_err(format_error)?)? {
                let id: i64 = row.get(0).map_err(format_error)?;
                net.defaulted_mut()
                    .insert(format!("VoltageLevel.{id}"), vec!["Flag_Volt"]);
            }
        }
        let mut types = serde_json::Map::new();
        for (&element, kind) in &self.elements {
            if !matches!(kind.as_str(), "Line" | "TwoWindingTransformer") {
                continue;
            }
            // Table names come only from the two literal matches above.
            let mut statement = self
                .connection
                .prepare(&format!(
                    "SELECT * FROM {kind} WHERE Element_ID=?1 AND Variant_ID=?2"
                ))
                .map_err(format_error)?;
            let mut rows = statement
                .query([element, self.variant])
                .map_err(format_error)?;
            let row = rows
                .next()
                .map_err(format_error)?
                .ok_or_else(|| format_error("missing mapped equipment"))?;
            if let Some(id) = reference(row, "Typ_ID")? {
                let scope: i64 = row.get("Flag_Typ_ID").map_err(format_error)?;
                types.insert(
                    element.to_string(),
                    serde_json::json!({"type_id": id, "scope": scope, "equipment": kind}),
                );
            }
            if kind == "TwoWindingTransformer"
                && row
                    .get::<_, Option<i64>>("Flag_Tap")
                    .map_err(format_error)?
                    .is_none()
            {
                // The electrical decoder has already validated this as the
                // schema-11.5 common-tap default, never an arbitrary NULL.
                net.defaulted_mut()
                    .insert(format!("TwoWindingTransformer.{element}"), vec!["Flag_Tap"]);
            }
        }
        net.extras_mut().insert(
            "sincal".into(),
            serde_json::json!({
                "schema_version": self.version, "variant": self.variant,
                "materialized_types": types,
            }),
        );
        Ok(())
    }
}
