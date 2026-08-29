//! The `.pio.json` stored module: one versioned wire for `PioModule<PioValue>`.
//!
//! `.pio.json` is not a case format. The reader dispatches on the `schema`
//! and `version` header, then decodes the selected exact typed DTO; released
//! 0.9.x `NetworkPackage` documents upgrade one way, and the pre 0.9 lineage
//! is refused. There is one document version and no per value version.

mod convert;
mod dto;
mod upgrade;

pub use convert::{read_module, write_module};
pub use dto::{SCHEMA_NAME, SCHEMA_VERSION, StoredModuleV1, StoredValueV1};
