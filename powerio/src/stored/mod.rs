//! PowerIO IR for one `PioModule<PioValue>`.
//!
//! `.pio.json` is not a case format. The reader dispatches on the `schema`
//! and `version` header, then decodes the exact typed representation. The
//! version names the release whose schema the document follows, and the
//! reader accepts its own release series; the producer record names the
//! software that wrote the document.

mod convert;
mod dto;

pub(crate) use convert::{emit_module, encode_diagnostics, read_module};
#[cfg(feature = "schema")]
pub(crate) use dto::StoredModule;
