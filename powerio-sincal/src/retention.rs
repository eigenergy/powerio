//! Model-neutral source inventory; adapters declare which tables they interpret.
use crate::{DatabaseSnapshot, Result, format_error};
use powerio_core::{Diagnostic, DiagnosticInfo};
use serde_json::json;

impl DatabaseSnapshot {
    /// Report source-only tables and explicitly enumerated source-only fields.
    /// Counts apply to the selected variant, or all rows for unversioned tables.
    /// Excluded Access tables have unknown counts: never report them as empty.
    ///
    /// # Errors
    /// Invalid source schema, exhausted query budget, or invalid diagnostic.
    ///
    /// # Panics
    /// Never on external input: diagnostic details are constructed as JSON objects.
    pub fn retention_diagnostics(
        &self,
        code: &'static DiagnosticInfo,
        interpreted_tables: &[&str],
        retained_fields: &[(&str, &[&str])],
    ) -> Result<Vec<Diagnostic>> {
        let mut statement = self.connection.prepare(
            "SELECT name FROM pragma_table_list WHERE schema='main' AND type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name LIMIT 513"
        ).map_err(format_error)?;
        let names: Vec<String> = statement
            .query_map([], |r| r.get(0))
            .map_err(format_error)?
            .collect::<rusqlite::Result<_>>()
            .map_err(format_error)?;
        if names.len() > 512 {
            return Err(format_error("source inventory exceeds table budget"));
        }
        let mut diagnostics = Vec::new();
        for name in names {
            let quote = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
            let columns: Vec<String> = self
                .connection
                .prepare("SELECT name FROM pragma_table_info(?1)")
                .map_err(format_error)?
                .query_map([&name], |r| r.get(0))
                .map_err(format_error)?
                .collect::<rusqlite::Result<_>>()
                .map_err(format_error)?;
            let fields: Vec<_> = retained_fields
                .iter()
                .filter(|(table, _)| *table == name)
                .flat_map(|(_, fields)| *fields)
                .filter(|field| columns.iter().any(|c| c == **field))
                .copied()
                .collect();
            let whole_table = !interpreted_tables.contains(&name.as_str());
            if !whole_table && fields.is_empty() {
                continue;
            }
            let variant_scoped = columns.iter().any(|c| c == "Variant_ID");
            let predicate = if variant_scoped {
                " WHERE Variant_ID=?1"
            } else {
                ""
            };
            let sql = format!("SELECT count(*) FROM {}{predicate}", quote(&name));
            let count: i64 = if variant_scoped {
                self.connection
                    .query_row(&sql, [self.variant], |r| r.get(0))
            } else {
                self.connection.query_row(&sql, [], |r| r.get(0))
            }
            .map_err(format_error)?;
            if count == 0 {
                continue;
            }
            let message = if whole_table {
                format!("{name}: {count} native rows remain only in retained source")
            } else {
                format!(
                    "{name}: fields {} in {count} native rows remain only in retained source (including NULL/default cells)",
                    fields.join(", ")
                )
            };
            let details = json!({"table": name, "row_count": count,
                "scope": if variant_scoped { "selected_variant" } else { "all_rows" },
                "variant": self.variant, "fields": fields, "whole_table": whole_table,
                "retention": "original_source_only"});
            diagnostics.push(
                Diagnostic::of(code, message)
                    .with_details(details.as_object().unwrap().clone())
                    .map_err(format_error)?,
            );
        }
        if !self.excluded_tables.is_empty() {
            diagnostics.push(Diagnostic::of(code,
                format!("{} native tables were excluded from Access acquisition; row counts and field contents are unknown, and remain only in the original MDB", self.excluded_tables.len()))
                .with_details(json!({"excluded_tables":self.excluded_tables,"row_count":null,"retention":"original_mdb_only","inventory_complete":false}).as_object().unwrap().clone())
                .map_err(format_error)?);
        }
        Ok(diagnostics)
    }
}

/// Add the parse-time inventory to an existing cross-format loss diagnostic.
/// The inventory survives IR in diagnostics; it never reconstructs source bytes.
///
/// # Errors
/// A diagnostic record exceeds the shared record limits.
pub fn attach_retention_details(
    read: &[Diagnostic],
    emitted: &mut [Diagnostic],
    loss_code: &str,
) -> std::result::Result<(), powerio_core::Error> {
    let inventory: Vec<_> = read
        .iter()
        .filter(|d| {
            matches!(
                d.details().get("retention").and_then(|v| v.as_str()),
                Some("original_source_only" | "original_mdb_only")
            )
        })
        .map(|d| json!(d.details()))
        .collect();
    for diagnostic in emitted.iter_mut().filter(|d| d.code() == loss_code) {
        diagnostic.insert_detail("source_inventory", json!(inventory))?;
        diagnostic.insert_detail(
            "inventory_scope",
            json!("parse_time_tables_and_enumerated_fields"),
        )?;
    }
    Ok(())
}
