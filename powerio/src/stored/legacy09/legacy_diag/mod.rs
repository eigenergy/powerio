//! The frozen 0.9 stored diagnostic record.
//!
//! The 1.0 runtime record lives in `powerio-core`: four MLIR severities, a
//! target and byte spans instead of a `SourceRef`, and private fields. A 0.9
//! `.pio.json` document was written against the five-severity record with its
//! seven-field source pointer, so the one-way 0.9 reader needs the shape it was
//! written in. Only that shape lives here. Codes, the code grammar, the
//! registry, and the error category are the workspace's, not this crate's, so
//! they come from `powerio-core` and this crate's registry stays inside the
//! shared workspace gate.
//!
//! This module leaves with the crate when the facade takes over the stored
//! document.

pub mod code;
pub mod collect;
pub mod record;
pub mod render;

pub use code::{DiagnosticCode, DiagnosticStage};
pub use record::{DiagnosticSeverity, SourceRef, StructuredDiagnostic};
pub use render::render_lines;
