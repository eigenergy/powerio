//! Internal, model-neutral SINCAL transport and schema support for family adapters.
//!
//! Acquisition and structural validation do not choose a network family.
//! These implementation interfaces are not the public PowerIO parse API.

mod acquisition;
#[doc(hidden)]
pub mod authoring;
#[cfg(test)]
mod authoring_tests;
mod geometry;
#[cfg(test)]
mod geometry_tests;
mod geometry_view;
mod project;
pub use geometry::{DrawingGeometry, DrawingPoint, GRAPHICS_TABLES};
mod retention;
mod schema;
#[cfg(test)]
mod schema_tests;
mod tables;
#[cfg(test)]
mod tests;

pub use acquisition::{MAX_BYTES, SQLITE_MAGIC, database_bytes};
pub use project::AcquiredProject;
// The inventory helper is model neutral and needs no SQLite, so it lives in
// powerio-core; the re-export keeps the adapter API unchanged.
pub use powerio_core::attach_retention_details;
pub use schema::{DatabaseSnapshot, TerminalIdentity, require_table};
pub use tables::TableRecords;

/// Transport/schema failure. The owning adapter supplies its registered diagnostic.
#[derive(Debug)]
pub struct Error(String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

fn format_error<T: std::fmt::Display + 'static>(message: T) -> Error {
    Error(failure_message(&message))
}

/// Render a storage failure. SQLite interrupts a query once the parse's
/// shared step budget is spent, which means the project is larger than this
/// reader handles; a bare "interrupted" would not say so.
pub(crate) fn failure_message<T: std::fmt::Display + 'static>(message: &T) -> String {
    let interrupted = (message as &dyn std::any::Any)
        .downcast_ref::<rusqlite::Error>()
        .and_then(rusqlite::Error::sqlite_error_code)
        == Some(rusqlite::ErrorCode::OperationInterrupted);
    if interrupted {
        "the project exceeds this reader's bounded query work; SINCAL projects of a \
         few hundred elements or more may be too large for this release"
            .to_owned()
    } else {
        message.to_string()
    }
}
