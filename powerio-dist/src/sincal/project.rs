//! Retained native project inputs. Coupling payload acquisition is separate
//! from interpreting matrix units, conductor order and native line replacement.

use powerio_core::{Source, SourceBuffer};

use super::{
    acquisition::MAX_BYTES,
    format_error,
    schema::{NativeDatabase, require_table},
    transformer::integer,
};
use crate::Result;

pub(super) struct NativeProject {
    pub database: NativeDatabase,
    /// Original primary bytes plus retained referenced/packaged buffers.
    pub source: Source,
    database_buffer: SourceBuffer,
}

impl NativeProject {
    pub fn read(source: &Source, variant: Option<i64>) -> Result<Self> {
        let acquired = powerio_sincal::AcquiredProject::read(source).map_err(format_error)?;
        let source = acquired.source;
        let database_buffer = acquired.database;
        let database = NativeDatabase::decode(database_buffer.bytes(), variant)?;
        Ok(Self {
            database,
            source,
            database_buffer,
        })
    }

    /// Fetch the selected variant's explicit project name under its own
    /// database-directory/Leika. Returning bytes does not establish that a
    /// .cpl/.leika/.mac file is an electrically supported matrix.
    pub fn coupling_buffer(&self, coupling_id: i64) -> Result<SourceBuffer> {
        if coupling_id <= 0 {
            return Err(format_error("invalid coupling-data ID"));
        }
        require_table(
            &self.database.connection,
            "CouplingData",
            &["CoupData_ID", "Variant_ID", "Flag_Variant", "ProjName"],
        )?;
        let mut statement = self.database.connection.prepare(
            "SELECT Flag_Variant, ProjName FROM CouplingData WHERE CoupData_ID=?1 AND Variant_ID=?2"
        ).map_err(format_error)?;
        let mut rows = statement
            .query([coupling_id, self.database.variant])
            .map_err(format_error)?;
        let row = rows.next().map_err(format_error)?.ok_or_else(|| {
            format_error(format!(
                "missing CouplingData {coupling_id} in selected variant"
            ))
        })?;
        if integer(row, "Flag_Variant")? != 1 {
            return Err(format_error(
                "coupling-data row requires variant-state resolution",
            ));
        }
        let project: String = row.get("ProjName").map_err(format_error)?;
        if rows.next().map_err(format_error)?.is_some() {
            return Err(format_error("ambiguous coupling-data project"));
        }
        let project = safe_project_name(&project)?;
        let buffer = self
            .source
            .referenced_buffer(&self.database_buffer, &format!("Leika/{project}"))
            .map_err(|error| {
                format_error(format!(
                    "CouplingData {coupling_id} project {project:?}: {error}"
                ))
            })?;
        // File-backed sources enforce the core acquisition budget while
        // reading. Caller-supplied memory buffers need the size guard too.
        if buffer.bytes().len() as u64 > MAX_BYTES {
            return Err(format_error("coupling payload exceeds 64 MiB limit"));
        }
        Ok(buffer)
    }
}

fn safe_project_name(name: &str) -> Result<String> {
    let path = name.replace('\\', "/");
    if path.is_empty()
        || path.len() > 1024
        || path.contains(':')
        || path.chars().any(char::is_control)
        || path
            .split('/')
            .any(|part| matches!(part, "" | "." | "..") || part.ends_with([' ', '.']))
    {
        return Err(format_error("unsafe coupling project name"));
    }
    Ok(path)
}
