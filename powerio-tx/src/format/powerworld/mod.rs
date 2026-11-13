//! Parse and emit PowerWorld auxiliary `.aux` files.
//!
//! The implementation is layered. The AUX grammar decoder produces the
//! generic [`AuxFile`] — every `DATA` and `SCRIPT` section, with field lists,
//! value rows, and `SUBDATA` blocks intact — and knows the grammar from the
//! official format guide: legacy and concise headers, comma delimited (CSV)
//! sections, multiline field lists and value rows, `//` comments, quoting,
//! and `variablename:location` field suffixes. On top of it, the [`BalancedNetwork`]
//! mapping consumes the power flow core types (Bus, Load, Shunt, Gen,
//! Branch) by field name, so column order and extra columns don't matter.
//! Object types outside the core stay reachable through [`aux_sections`] and
//! survive the same format round trip byte for byte when the parsed module is
//! passed to [`crate::emit`].
//!
//! The serializer emits `DATA (Object, [fields]) { … }` blocks for the core
//! types, values in MW/MVAr/degrees, status as `Closed`/`Open`. Generator
//! cost, HVDC, and storage are not represented and are reported on emission.
//!
//! `.pwb` binary cases are parsed but cannot be emitted; see that module for
//! the decoded vintages and parity evidence. `.pwd` files carry no case data,
//! only the diagram, and read as a diagram space layer.
//!
//! [`BalancedNetwork`]: crate::network::BalancedNetwork

mod auxiliary;
pub(in crate::format) mod map;
mod objects;
mod pwb;
mod pwd;

#[cfg(test)]
mod tests;

pub use auxiliary::{AuxFile, AuxObject, AuxRow, AuxScript, AuxSection, AuxSubData};

pub use map::aux_sections;
pub(crate) use map::write_powerworld;
pub use objects::{Contingency, contingencies, rating_set_names};
pub(crate) use pwb::parse_pwb_collecting;
pub use pwd::{PwdDisplay, PwdSubstation};

#[doc(hidden)]
pub use auxiliary::{emit_aux as __emit_aux, parse_aux as __parse_aux};
#[doc(hidden)]
pub use pwb::{parse_pwb as __parse_pwb, parse_pwb_with_warnings as __parse_pwb_with_warnings};
#[doc(hidden)]
pub use pwd::{
    parse_pwd as __parse_pwd, parse_pwd_display as __parse_pwd_display,
    parse_pwd_file as __parse_pwd_file, parse_pwd_layer as __parse_pwd_layer,
};

use std::collections::HashMap;

use crate::network::{BusId, Extras};

/// The bus pair a PowerWorld branch identity names, in ascending order.
///
/// Simulator identifies a branch by its two buses in either order plus its
/// circuit: loading the branch 2 to 1 circuit 1 updates the branch 1 to 2
/// circuit 1 rather than creating a second one, and logs nothing. The aux
/// writer's circuit allocator counts parallel branches on this key so that
/// reversed parallels get distinct circuits, and both readers' positional
/// circuit rule counts on the same key so the three agree.
pub(super) fn branch_pair(from: BusId, to: BusId) -> (BusId, BusId) {
    if from <= to { (from, to) } else { (to, from) }
}

/// A bus name as read, or `None` for the placeholder the aux writer states
/// for an unnamed bus.
///
/// Simulator refuses to create a bus whose `BusName` is blank, so the aux
/// writer names an unnamed bus by its number. A name that states exactly the
/// bus number (padding aside) is indistinguishable from that placeholder and
/// reads as no name, as a positional device id does in
/// [`drop_positional_id`]; a blank name reads as no name too. Shared by both
/// PowerWorld readers so they agree on one case.
pub(super) fn bus_name(name: &str, id: BusId) -> Option<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed == id.to_string() {
        None
    } else {
        Some(name.to_string())
    }
}

/// Drop each device's retained id when it states exactly the positional
/// default the aux writer's allocator would hand it: its 1-based position
/// among `devices` at the same bus.
///
/// Simulator's load, shunt, and generator ids are two characters and it
/// truncates a longer one, so the writer numbers devices per bus rather than
/// over the whole table: a table position of 100 would collide with 10.
/// `parts` returns a device's bus and extras.
pub(super) fn drop_positional_ids<T>(
    devices: &mut [T],
    keys: &[&str],
    parts: impl Fn(&mut T) -> (BusId, &mut Extras),
) {
    let mut at_bus: HashMap<BusId, usize> = HashMap::new();
    for device in devices {
        let (bus, extras) = parts(device);
        let position = at_bus.entry(bus).or_insert(0);
        drop_positional_id(extras, keys, *position);
        *position += 1;
    }
}

/// Drop a retained device id that states exactly the positional default the
/// aux writer's allocator would hand the element at 0-based `index`.
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
