//! Read and write PowerWorld auxiliary `.aux` files.
//!
//! The reader is layered. [`parse_aux`] parses any auxiliary file into the
//! generic [`AuxFile`] — every `DATA` and `SCRIPT` section, with field lists,
//! value rows, and `SUBDATA` blocks intact — and knows the grammar from the
//! official format guide: legacy and concise headers, comma delimited (CSV)
//! sections, multiline field lists and value rows, `//` comments, quoting,
//! and `variablename:location` field suffixes. On top of it, the [`Network`]
//! mapping consumes the power flow core types (Bus, Load, Shunt, Gen,
//! Branch) by field name, so column order and extra columns don't matter.
//! Object types outside the core stay reachable through [`aux_sections`] and
//! survive the same format round trip byte for byte via the retained source
//! (see [`crate::write_as`]).
//!
//! The writer emits `DATA (Object, [fields]) { … }` blocks for the core
//! types, values in MW/MVAr/degrees, status as `Closed`/`Open`. Generator
//! cost, HVDC, and storage are not represented and are reported on write.
//! The `.pwb` binary case format is not handled here yet.
//!
//! [`Network`]: crate::network::Network

mod aux;
mod map;

#[cfg(test)]
mod tests;

use std::sync::Arc;

pub use aux::{
    AuxFile, AuxObject, AuxRow, AuxScript, AuxSection, AuxSubData, parse_aux, write_aux,
};
pub(crate) use map::parse_powerworld_source;
pub use map::{aux_sections, write_powerworld};

use crate::Result;
use crate::network::Network;

/// Parse a PowerWorld `.aux` into a [`Network`], reading the Bus/Load/Shunt/
/// Gen/Branch `DATA` blocks by their declared field lists.
///
/// # Errors
/// [`crate::Error::FormatRead`] on malformed input or when the file has no
/// `DATA` sections.
pub fn parse_powerworld(content: &str) -> Result<Network> {
    // The caller owns `content` as a borrow, so retention needs one copy.
    parse_powerworld_source(Arc::new(content.to_owned()), None)
}
