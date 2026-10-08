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
pub use retention::attach_retention_details;
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

fn format_error(message: impl std::fmt::Display) -> Error {
    Error(message.to_string())
}
