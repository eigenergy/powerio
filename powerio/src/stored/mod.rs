//! PowerIO IR for one `PioModule<PioValue>`.
//!
//! `.pio.json` is not a case format. The reader dispatches on the `schema`
//! and `version` header, then decodes the exact typed representation. The
//! generation is independent of the PowerIO release; the producer record
//! names the release that wrote the document.

mod convert;
mod dto;

pub(crate) use convert::{emit_module, encode_diagnostics, read_module};
#[cfg(feature = "schema")]
pub(crate) use dto::StoredModule;
