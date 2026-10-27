//! Native enums documented by the April 2014 database/input manuals.
//!
//! These codes are not bitmasks. Interpretation also depends on the element:
//! L12 is two conductors on a line but one phase-to-phase branch on a load.
//! On a transformer the positions select coil pairs, whose vector group
//! determines the actual bus conductors at each side.
//! Keep that distinction here rather than guessing from the node phase flag.

use std::collections::BTreeMap;

use super::{format_error, schema::NativeDatabase};
use powerio_dist::Result;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Connection {
    L1,
    L2,
    L3,
    L12,
    L23,
    L31,
    L123,
    Neutral,
}

impl Connection {
    pub fn decode(code: i64) -> Result<Self> {
        match code {
            1 => Ok(Self::L1),
            2 => Ok(Self::L2),
            3 => Ok(Self::L3),
            4 => Ok(Self::L12),
            5 => Ok(Self::L23),
            6 => Ok(Self::L31),
            7 => Ok(Self::L123),
            8 => Ok(Self::Neutral),
            _ => Err(format_error(format!("unknown terminal connection {code}"))),
        }
    }

    /// Ordered zero-based phase positions, excluding the neutral. The neutral
    /// case is explicit so it cannot be mistaken for an empty phase set.
    /// Transformer callers must interpret these as coil positions and expand
    /// the vector-group connections before collecting bus conductors.
    pub fn phases(self) -> Option<&'static [usize]> {
        match self {
            Self::L1 => Some(&[0]),
            Self::L2 => Some(&[1]),
            Self::L3 => Some(&[2]),
            Self::L12 => Some(&[0, 1]),
            Self::L23 => Some(&[1, 2]),
            Self::L31 => Some(&[2, 0]),
            Self::L123 => Some(&[0, 1, 2]),
            Self::Neutral => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum State {
    Off,
    On,
}

impl State {
    pub fn decode(code: i64) -> Result<Self> {
        match code {
            0 => Ok(Self::Off),
            1 => Ok(Self::On),
            _ => Err(format_error(format!("unknown service/switch state {code}"))),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct ElectricalTerminal {
    pub id: i64,
    pub node: i64,
    pub position: i64,
    pub connection: Connection,
    /// This is the terminal's switch state, independent of element service.
    pub state: State,
}

impl NativeDatabase {
    pub fn element_state(&self, element: i64) -> Result<State> {
        let code = self
            .connection
            .query_row(
                "SELECT Flag_State FROM Element WHERE Element_ID=?1 AND Variant_ID=?2",
                [element, self.variant],
                |row| row.get(0),
            )
            .map_err(format_error)?;
        State::decode(code)
    }

    /// Return ports in native TerminalNo order, not database or ID order.
    /// Structural decoding has already verified uniqueness and references.
    pub fn electrical_terminals(&self, element: i64) -> Result<Vec<ElectricalTerminal>> {
        if !self.elements.contains_key(&element) {
            return Err(format_error(format!("unknown Element {element}")));
        }
        let index = self.terminal_index()?;
        let mut ports = BTreeMap::new();
        for raw in index.get(&element).into_iter().flatten() {
            let terminal = self
                .terminals
                .get(&raw.id)
                .ok_or_else(|| format_error(format!("unknown Terminal {}", raw.id)))?;
            ports.insert(
                terminal.position,
                ElectricalTerminal {
                    id: raw.id,
                    node: terminal.node,
                    position: terminal.position,
                    connection: Connection::decode(raw.connection.clone().map_err(format_error)?)?,
                    state: State::decode(raw.state.clone().map_err(format_error)?)?,
                },
            );
        }
        Ok(ports.into_values().collect())
    }

    fn terminal_index(&self) -> Result<&TerminalIndex> {
        if let Some(index) = self.terminal_index.get() {
            return Ok(index);
        }
        let index = self.read_terminal_index()?;
        Ok(self.terminal_index.get_or_init(|| index))
    }

    /// Read the selected variant's `Terminal` rows once and group them by
    /// element. Native `Terminal` has no `Element_ID` index, so one query per
    /// element would scan the whole table each time. Field decoding stays
    /// per lookup: a malformed terminal fails only the element that uses it.
    fn read_terminal_index(&self) -> Result<TerminalIndex> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT Terminal_ID, Element_ID, Flag_Terminal, Flag_State FROM Terminal
             WHERE Variant_ID=?1",
            )
            .map_err(format_error)?;
        let mut rows = statement.query([self.variant]).map_err(format_error)?;
        let mut index = TerminalIndex::new();
        while let Some(row) = rows.next().map_err(format_error)? {
            let element: i64 = row.get(1).map_err(format_error)?;
            let field = |i| row.get::<_, i64>(i).map_err(|e| e.to_string());
            index.entry(element).or_default().push(RawTerminal {
                id: row.get(0).map_err(format_error)?,
                connection: field(2),
                state: field(3),
            });
        }
        Ok(index)
    }
}

/// `Terminal` rows of the selected variant by `Element_ID`, in query order.
pub(super) type TerminalIndex = BTreeMap<i64, Vec<RawTerminal>>;

/// One terminal's undecoded electrical fields. A conversion failure is kept
/// as its message and reported when an element asks for the terminal.
pub(super) struct RawTerminal {
    id: i64,
    connection: std::result::Result<i64, String>,
    state: std::result::Result<i64, String>,
}

/// Element.Flag_Input has independent category bits. Requiring equality to
/// 7 incorrectly rejects valid rows carrying, for example, regulator data.
pub(super) fn require_input_categories(value: i64, required: i64) -> Result<()> {
    if value < 0 || value & required != required {
        return Err(format_error(format!(
            "missing required input categories {required:#x} in Flag_Input={value}"
        )));
    }
    Ok(())
}
