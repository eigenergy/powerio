//! The element map an emission returns: which written row came from which
//! network element.
//!
//! A writer records each bus, branch, and generator row as it writes it, so
//! the map follows the output it describes. Keys are element `uid`s in module
//! order; a value names the target table, the 0-based row position in that
//! table as written, the target's own key for the row when it has one, and,
//! for a branch, whether the row's first end is the element's `to` bus.

use std::collections::HashSet;

use serde::ser::{SerializeMap, SerializeStruct};
use serde::{Serialize, Serializer};

use crate::{Diagnostic, Error};

/// One written row: the target table, the 0-based row position in that table
/// as written, the target's own key for the row when it has one, and, for a
/// branch, whether the row's first end is the element's `to` bus.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RowRef {
    /// The target's table name, such as `bus`, `line`, or `generators`.
    pub table: String,
    /// The 0-based row position in `table` as written.
    pub index: usize,
    /// The target's own name for the row (a bus number, a component name, a
    /// generator id), when it has one.
    pub key: Option<String>,
    /// For a branch: `true` when the row's first end (`from_bus`, `hv_bus`,
    /// `bus0`, ...) is the element's `to` bus. `None` for buses and
    /// generators.
    pub flip: Option<bool>,
}

impl RowRef {
    /// A row at `index` in `table`, with no key.
    #[must_use]
    pub fn new(table: impl Into<String>, index: usize) -> Self {
        Self {
            table: table.into(),
            index,
            key: None,
            flip: None,
        }
    }

    /// The same row with the target's own key for it.
    #[must_use]
    pub fn with_key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        self
    }
}

/// Which written row each bus, branch, and generator became.
///
/// Every element of the three kinds appears exactly once, in module order. An
/// element the target wrote no row for maps to `None`, and the emission
/// carries a diagnostic naming its `uid`. Rows a writer adds with no element
/// behind them are absent. No two elements share a `(table, index)` or a
/// `(table, key)`.
///
/// An emission that returns retained source bytes runs no writer and has no
/// map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ElementMap {
    format: String,
    powerio: String,
    bus: Vec<(String, Option<RowRef>)>,
    branch: Vec<(String, Option<RowRef>)>,
    generator: Vec<(String, Option<RowRef>)>,
}

impl ElementMap {
    /// The target format token, such as `matpower` or `surge-json`.
    #[must_use]
    pub fn format(&self) -> &str {
        &self.format
    }

    /// The PowerIO release that wrote the map.
    #[must_use]
    pub fn powerio_version(&self) -> &str {
        &self.powerio
    }

    /// Each bus `uid` and its row, in module order.
    pub fn bus(&self) -> impl ExactSizeIterator<Item = (&str, Option<&RowRef>)> {
        entries(&self.bus)
    }

    /// Each branch `uid` and its row, in module order.
    pub fn branch(&self) -> impl ExactSizeIterator<Item = (&str, Option<&RowRef>)> {
        entries(&self.branch)
    }

    /// Each generator `uid` and its row, in module order.
    pub fn generator(&self) -> impl ExactSizeIterator<Item = (&str, Option<&RowRef>)> {
        entries(&self.generator)
    }

    /// The map as one JSON object:
    /// `{"format", "powerio", "bus", "branch", "gen"}`, each section an
    /// object from `uid` to `{"table", "index", "key"?, "flip"?}` or `null`,
    /// with keys in module order. `key` is always a string.
    ///
    /// # Panics
    /// Never: every field is a string, an integer, a boolean, or null, which
    /// JSON always encodes.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string(&JsonMap(self)).expect("an element map always encodes as JSON")
    }
}

fn entries(
    slots: &[(String, Option<RowRef>)],
) -> impl ExactSizeIterator<Item = (&str, Option<&RowRef>)> {
    slots.iter().map(|(uid, row)| (uid.as_str(), row.as_ref()))
}

/// The JSON encoding, kept private so the public type implements no serde
/// trait of its own.
struct JsonMap<'a>(&'a ElementMap);
struct JsonSection<'a>(&'a [(String, Option<RowRef>)]);
struct JsonRow<'a>(&'a RowRef);

impl Serialize for JsonMap<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_struct("ElementMap", 5)?;
        map.serialize_field("format", &self.0.format)?;
        map.serialize_field("powerio", &self.0.powerio)?;
        map.serialize_field("bus", &JsonSection(&self.0.bus))?;
        map.serialize_field("branch", &JsonSection(&self.0.branch))?;
        map.serialize_field("gen", &JsonSection(&self.0.generator))?;
        map.end()
    }
}

impl Serialize for JsonSection<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (uid, row) in self.0 {
            map.serialize_entry(uid, &row.as_ref().map(JsonRow))?;
        }
        map.end()
    }
}

impl Serialize for JsonRow<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let row = self.0;
        let fields = 2 + usize::from(row.key.is_some()) + usize::from(row.flip.is_some());
        let mut map = serializer.serialize_map(Some(fields))?;
        map.serialize_entry("table", &row.table)?;
        map.serialize_entry("index", &row.index)?;
        if let Some(key) = &row.key {
            map.serialize_entry("key", key)?;
        }
        if let Some(flip) = row.flip {
            map.serialize_entry("flip", &flip)?;
        }
        map.end()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Bus,
    Branch,
    Generator,
}

impl Kind {
    const fn label(self) -> &'static str {
        match self {
            Self::Bus => "bus",
            Self::Branch => "branch",
            Self::Generator => "generator",
        }
    }
}

/// Records the rows a writer writes and checks the map's rules.
///
/// A writer creates one with every element `uid` in module order, calls
/// [`bus`](Self::bus), [`branch`](Self::branch), or
/// [`generator`](Self::generator) for each row it writes, naming the
/// element's position in its module table, and hands the builder to the
/// emission. Recording never fails; [`finish`](Self::finish) reports a
/// violated rule.
#[derive(Clone, Debug)]
pub struct ElementMapBuilder {
    format: String,
    bus: Vec<(String, Option<RowRef>)>,
    branch: Vec<(String, Option<RowRef>)>,
    generator: Vec<(String, Option<RowRef>)>,
    rows: HashSet<(String, usize)>,
    keys: HashSet<(String, String)>,
    violation: Option<String>,
}

impl ElementMapBuilder {
    /// A builder for `format` over the given element `uid`s, each in module
    /// order.
    pub fn new<B, R, G>(format: impl Into<String>, bus: B, branch: R, generator: G) -> Self
    where
        B: IntoIterator<Item = String>,
        R: IntoIterator<Item = String>,
        G: IntoIterator<Item = String>,
    {
        Self {
            format: format.into(),
            bus: bus.into_iter().map(|uid| (uid, None)).collect(),
            branch: branch.into_iter().map(|uid| (uid, None)).collect(),
            generator: generator.into_iter().map(|uid| (uid, None)).collect(),
            rows: HashSet::new(),
            keys: HashSet::new(),
            violation: None,
        }
    }

    /// Record that the bus at `position` in the module was written as `row`.
    pub fn bus(&mut self, position: usize, mut row: RowRef) {
        row.flip = None;
        self.record(Kind::Bus, position, row);
    }

    /// Record that the branch at `position` in the module was written as
    /// `row`, with `flip` true when the row's first end is the branch's `to`
    /// bus.
    pub fn branch(&mut self, position: usize, mut row: RowRef, flip: bool) {
        row.flip = Some(flip);
        self.record(Kind::Branch, position, row);
    }

    /// Record that the generator at `position` in the module was written as
    /// `row`.
    pub fn generator(&mut self, position: usize, mut row: RowRef) {
        row.flip = None;
        self.record(Kind::Generator, position, row);
    }

    fn record(&mut self, kind: Kind, position: usize, row: RowRef) {
        if self.violation.is_some() {
            return;
        }
        let violation = self.check(kind, position, &row);
        if violation.is_some() {
            self.violation = violation;
            return;
        }
        self.rows.insert((row.table.clone(), row.index));
        if let Some(key) = &row.key {
            self.keys.insert((row.table.clone(), key.clone()));
        }
        self.slots_mut(kind)[position].1 = Some(row);
    }

    fn check(&self, kind: Kind, position: usize, row: &RowRef) -> Option<String> {
        let label = kind.label();
        let Some((uid, existing)) = self.slots(kind).get(position) else {
            return Some(format!(
                "the {} writer recorded a row for {label} position {position}, past the module's {} {label} element(s)",
                self.format,
                self.slots(kind).len()
            ));
        };
        if existing.is_some() {
            return Some(format!(
                "the {} writer recorded {label} '{uid}' twice",
                self.format
            ));
        }
        if self.rows.contains(&(row.table.clone(), row.index)) {
            return Some(format!(
                "the {} writer wrote {label} '{uid}' to {} row {}, which another element already holds",
                self.format, row.table, row.index
            ));
        }
        if let Some(key) = &row.key
            && self.keys.contains(&(row.table.clone(), key.clone()))
        {
            return Some(format!(
                "the {} writer wrote {label} '{uid}' with {} key '{key}', which another element already holds",
                self.format, row.table
            ));
        }
        None
    }

    fn slots(&self, kind: Kind) -> &[(String, Option<RowRef>)] {
        match kind {
            Kind::Bus => &self.bus,
            Kind::Branch => &self.branch,
            Kind::Generator => &self.generator,
        }
    }

    fn slots_mut(&mut self, kind: Kind) -> &mut [(String, Option<RowRef>)] {
        match kind {
            Kind::Bus => &mut self.bus,
            Kind::Branch => &mut self.branch,
            Kind::Generator => &mut self.generator,
        }
    }

    /// Check the recorded rows and build the map.
    ///
    /// Returns the map and one warning per element the writer wrote no row
    /// for. When two elements of one kind share a `uid`, the map cannot key
    /// them apart: the map is `None` and a warning says why.
    ///
    /// # Errors
    /// `EMIT.ELEMENT_MAP.DUPLICATE_ROW` when the writer recorded two elements
    /// for one `(table, index)` or `(table, key)`, one element twice, or a
    /// position outside the module.
    pub fn finish(self) -> Result<(Option<ElementMap>, Vec<Diagnostic>), Error> {
        if let Some(message) = self.violation {
            return Err(Error::new(
                &crate::codes::EMIT_ELEMENT_MAP_DUPLICATE_ROW,
                message,
            ));
        }
        for kind in [Kind::Bus, Kind::Branch, Kind::Generator] {
            let mut seen = HashSet::new();
            if let Some((uid, _)) = self
                .slots(kind)
                .iter()
                .find(|(uid, _)| !seen.insert(uid.as_str()))
            {
                let diagnostic = Diagnostic::of(
                    &crate::codes::EMIT_ELEMENT_MAP_DUPLICATE_UID,
                    format!(
                        "no element map for {}: two {} elements share the uid '{uid}'",
                        self.format,
                        kind.label()
                    ),
                );
                return Ok((None, vec![diagnostic]));
            }
        }
        let mut diagnostics = Vec::new();
        for kind in [Kind::Bus, Kind::Branch, Kind::Generator] {
            for (uid, row) in self.slots(kind) {
                if row.is_none() {
                    diagnostics.push(Diagnostic::of(
                        &crate::codes::EMIT_ELEMENT_MAP_UNMAPPED,
                        format!(
                            "{} '{uid}' has no row in the {} output; its element map entry is null",
                            kind.label(),
                            self.format
                        ),
                    ));
                }
            }
        }
        let map = ElementMap {
            format: self.format,
            powerio: env!("CARGO_PKG_VERSION").to_owned(),
            bus: self.bus,
            branch: self.branch,
            generator: self.generator,
        };
        Ok((Some(map), diagnostics))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn builder() -> ElementMapBuilder {
        ElementMapBuilder::new(
            "matpower",
            ["10".to_owned(), "20".to_owned()],
            ["10-20".to_owned(), "10-20-2".to_owned()],
            ["bus-10".to_owned()],
        )
    }

    #[test]
    fn json_keeps_module_order_and_omits_absent_fields() {
        let mut map = builder();
        map.bus(1, RowRef::new("bus", 1).with_key("20"));
        map.bus(0, RowRef::new("bus", 0).with_key("10"));
        map.branch(0, RowRef::new("branch", 0), false);
        map.branch(1, RowRef::new("branch", 1), true);
        let (map, diagnostics) = map.finish().unwrap();
        let map = map.unwrap();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code(), "EMIT.ELEMENT_MAP.UNMAPPED");
        assert!(diagnostics[0].message().contains("'bus-10'"));
        assert_eq!(
            map.to_json(),
            format!(
                concat!(
                    r#"{{"format":"matpower","powerio":"{}","#,
                    r#""bus":{{"10":{{"table":"bus","index":0,"key":"10"}},"20":{{"table":"bus","index":1,"key":"20"}}}},"#,
                    r#""branch":{{"10-20":{{"table":"branch","index":0,"flip":false}},"10-20-2":{{"table":"branch","index":1,"flip":true}}}},"#,
                    r#""gen":{{"bus-10":null}}}}"#
                ),
                env!("CARGO_PKG_VERSION")
            )
        );
        assert_eq!(map.bus().len(), 2);
        assert_eq!(map.generator().next(), Some(("bus-10", None)));
    }

    #[test]
    fn two_elements_on_one_row_or_key_are_refused() {
        let mut map = builder();
        map.branch(0, RowRef::new("branch", 0), false);
        map.branch(1, RowRef::new("branch", 0), false);
        let error = map.finish().unwrap_err();
        assert_eq!(error.info().unwrap().code, "EMIT.ELEMENT_MAP.DUPLICATE_ROW");

        let mut map = builder();
        map.bus(0, RowRef::new("bus", 0).with_key("10"));
        map.bus(1, RowRef::new("bus", 1).with_key("10"));
        assert!(map.finish().is_err());

        let mut map = builder();
        map.bus(0, RowRef::new("bus", 0));
        map.bus(0, RowRef::new("bus", 1));
        assert!(map.finish().is_err());

        let mut map = builder();
        map.generator(3, RowRef::new("gen", 0));
        assert!(map.finish().is_err());
    }

    #[test]
    fn a_shared_uid_gives_no_map() {
        let map = ElementMapBuilder::new(
            "matpower",
            ["1".to_owned(), "1".to_owned()],
            Vec::new(),
            Vec::new(),
        );
        let (map, diagnostics) = map.finish().unwrap();
        assert!(map.is_none());
        assert_eq!(diagnostics[0].code(), "EMIT.ELEMENT_MAP.DUPLICATE_UID");
    }
}
