//! Read and write PowerWorld auxiliary `.aux` files.
//!
//! The reader is layered. [`parse_aux`] parses any auxiliary file into the
//! generic [`AuxFile`] — every `DATA` and `SCRIPT` section, with field lists,
//! value rows, and `SUBDATA` blocks intact — and knows the grammar from the
//! official format guide: legacy and concise headers, comma delimited (CSV)
//! sections, multiline field lists and value rows, `//` comments, quoting,
//! and `variablename:location` field suffixes. On top of it, the [`BalancedNetwork`]
//! mapping consumes the power flow core types (Bus, Load, Shunt, Gen,
//! Branch) by field name, so column order and extra columns don't matter.
//! Object types outside the core stay reachable through [`aux_sections`] and
//! survive the same format round trip byte for byte via the retained source
//! (see [`crate::write_as`]).
//!
//! The writer emits `DATA (Object, [fields]) { … }` blocks for the core
//! types, values in MW/MVAr/degrees, status as `Closed`/`Open`. Generator
//! cost, HVDC, and storage are not represented and are reported on write.
//!
//! `.pwb` binary cases are read (never written) by [`parse_pwb`]; see that
//! module for the decoded vintages and the parity evidence. `.pwd` display
//! files carry no case data, only the diagram; [`parse_pwd_file`] and
//! [`parse_pwd`] read the decoded substation coordinates.
//!
//! [`BalancedNetwork`]: crate::network::BalancedNetwork

mod auxiliary;
mod map;
mod objects;
mod pwb;
mod pwd;

#[cfg(test)]
mod tests;

use std::sync::Arc;

pub use auxiliary::{
    AuxFile, AuxObject, AuxRow, AuxScript, AuxSection, AuxSubData, parse_aux, write_aux,
};
pub(crate) use map::parse_powerworld_source;
pub use map::{aux_sections, write_powerworld};
pub use objects::{Contingency, contingencies, rating_set_names};
pub use pwb::{parse_pwb, parse_pwb_with_warnings};
pub use pwd::{PwdDisplay, PwdSubstation, parse_pwd, parse_pwd_display, parse_pwd_file};

use crate::Result;
use crate::network::{BalancedNetwork, Extras};

/// Drop a retained device id that states exactly the positional default the
/// aux writer's allocator would hand element `index` anyway.
///
/// Shared by both PowerWorld readers on purpose. The aux reader and the binary
/// reader must agree on what counts as a default, or one keeps an id the other
/// drops and a pwb → aux leg reports the disagreement as a conversion loss.
/// Trimmed before comparing: PowerWorld pads ids for display, and a padded
/// default is still the default.
///
/// `keys` are the field aliases one id can arrive under; an explicit
/// non-default id is kept verbatim, padding included.
pub(super) fn drop_positional_id(extras: &mut Extras, keys: &[&str], index: usize) {
    let default = (index + 1).to_string();
    for key in keys {
        if extras
            .get(*key)
            .and_then(serde_json::Value::as_str)
            .is_some_and(|v| v.trim() == default)
        {
            extras.remove(*key);
        }
    }
}

/// Parse a PowerWorld `.aux` into a [`BalancedNetwork`], reading the Bus/Load/Shunt/
/// Gen/Branch `DATA` blocks by their declared field lists.
///
/// # Errors
/// [`crate::Error::FormatRead`] on malformed input or when the file has no
/// `DATA` sections.
pub fn parse_powerworld(content: &str) -> Result<BalancedNetwork> {
    // The caller owns `content` as a borrow, so retention needs one copy.
    let mut warnings = Vec::new();
    parse_powerworld_source(Arc::new(content.to_owned()), None, &mut warnings)
}
