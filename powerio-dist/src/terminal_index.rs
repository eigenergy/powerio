//! The flat terminal index of a multiconductor network: every declared bus
//! terminal at one position, in bus table order and each bus's stated
//! terminal order. The one terminal indexing every multiconductor calculation
//! reads, with one bus identity rule: bus names compare ASCII case
//! insensitively, terminal names exactly.

use std::collections::BTreeMap;

use crate::model::MulticonductorNetwork;

/// Why a network's terminals cannot be indexed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TerminalIndexError {
    /// Two buses share one identity, compared ASCII case insensitively.
    DuplicateBus { bus: String },
    /// A bus states one terminal twice.
    DuplicateTerminal { bus: String, terminal: String },
}

impl std::fmt::Display for TerminalIndexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateBus { bus } => {
                write!(f, "bus identity `{bus}` is duplicated case-insensitively")
            }
            Self::DuplicateTerminal { bus, terminal } => {
                write!(f, "bus `{bus}` repeats terminal `{terminal}`")
            }
        }
    }
}

impl std::error::Error for TerminalIndexError {}

/// Every declared terminal of a multiconductor network at one flat position.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MulticonductorTerminalIndex {
    /// Position to `(bus, terminal)` in the network's own spelling.
    terminals: Vec<(String, String)>,
    /// Folded bus name to terminal name to position.
    positions: BTreeMap<String, BTreeMap<String, usize>>,
}

impl MulticonductorTerminalIndex {
    /// Index `network`'s terminals: bus table order, each bus's stated
    /// terminal order. Grounded terminals and terminal `0` are indexed like
    /// any other; whether a calculation treats one as ground is its own rule.
    ///
    /// # Errors
    /// Two buses whose identities differ only in ASCII case, or a bus that
    /// states a terminal twice.
    pub fn build(network: &MulticonductorNetwork) -> Result<Self, TerminalIndexError> {
        let mut index = Self::default();
        for bus in network.buses() {
            let key = bus.id.to_ascii_lowercase();
            if index.positions.contains_key(&key) {
                return Err(TerminalIndexError::DuplicateBus {
                    bus: bus.id.clone(),
                });
            }
            let terminals = index.positions.entry(key).or_default();
            for terminal in &bus.terminals {
                if terminals
                    .insert(terminal.clone(), index.terminals.len())
                    .is_some()
                {
                    return Err(TerminalIndexError::DuplicateTerminal {
                        bus: bus.id.clone(),
                        terminal: terminal.clone(),
                    });
                }
                index.terminals.push((bus.id.clone(), terminal.clone()));
            }
        }
        Ok(index)
    }

    /// Position to `(bus, terminal)`, in the network's own spelling.
    #[must_use]
    pub fn terminals(&self) -> &[(String, String)] {
        &self.terminals
    }

    /// The number of indexed terminals.
    #[must_use]
    pub fn len(&self) -> usize {
        self.terminals.len()
    }

    /// Whether the network declares no terminal.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.terminals.is_empty()
    }

    /// The position of terminal `terminal` of bus `bus` (bus compared ASCII
    /// case insensitively), or `None` for an undeclared terminal.
    #[must_use]
    pub fn resolve(&self, bus: &str, terminal: &str) -> Option<usize> {
        self.bus(bus)?.get(terminal).copied()
    }

    /// The positions of one bus's terminals, ascending; empty for an
    /// undeclared bus.
    #[must_use]
    pub fn bus_terminals(&self, bus: &str) -> Vec<usize> {
        let mut positions: Vec<usize> = self
            .bus(bus)
            .map(|terminals| terminals.values().copied().collect())
            .unwrap_or_default();
        positions.sort_unstable();
        positions
    }

    fn bus(&self, bus: &str) -> Option<&BTreeMap<String, usize>> {
        self.positions
            .get(bus)
            .or_else(|| self.positions.get(&bus.to_ascii_lowercase()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::DistBus;

    #[test]
    fn terminals_index_flat_with_case_insensitive_buses() {
        let mut network = MulticonductorNetwork::default();
        network
            .buses_mut()
            .push(DistBus::new("Src", vec!["1".into(), "2".into()]));
        network
            .buses_mut()
            .push(DistBus::new("load", vec!["1".into(), "0".into()]));
        let index = MulticonductorTerminalIndex::build(&network).unwrap();
        assert_eq!(index.len(), 4);
        assert_eq!(index.resolve("src", "2"), Some(1));
        assert_eq!(index.resolve("SRC", "2"), Some(1));
        assert_eq!(index.resolve("load", "0"), Some(3));
        assert_eq!(index.resolve("load", "3"), None);
        assert_eq!(index.bus_terminals("LOAD"), vec![2, 3]);
        network
            .buses_mut()
            .push(DistBus::new("LOAD", vec!["1".into()]));
        assert!(matches!(
            MulticonductorTerminalIndex::build(&network),
            Err(TerminalIndexError::DuplicateBus { .. })
        ));
    }
}
