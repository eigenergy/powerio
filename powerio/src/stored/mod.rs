//! PowerIO IR for one `PioModule<PioValue>`.
//!
//! `.pio.json` is not a case format. The reader dispatches on the `schema`
//! and `version` header, then decodes the exact typed representation. There is
//! one document version and no per value version.

mod convert;
mod dto;

pub(crate) use convert::{emit_module, read_module};
#[cfg(feature = "schema")]
pub(crate) use dto::StoredModule;
