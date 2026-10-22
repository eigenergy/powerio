//! PSS/E subsystem description files (`.sub`).
//!
//! A `.sub` file names the bus groups a contingency analysis works over. An
//! automatic specification in a `.con` file and a monitored element statement
//! in a `.mon` file both name a subsystem stated here.
//!
//! ```text
//! SUBSYSTEM 'WOA'
//!    AREA 1
//! END
//! END
//! ```
//!
//! [`SubsystemSet::parse`] reads UTF-8 text and touches no filesystem.
//! Statements outside the grammar keep their trimmed line and are reported,
//! so a file a tool wrote for itself still reads.
//! [`SubsystemSet::to_sub`] writes the set back in one canonical spelling.
//! [`Subsystem::select_buses`] is the separate step that names the buses of a
//! subsystem in a [`BalancedNetwork`].
//!
//! The grammar, its evidence, and the writer's spellings are in `FORMAT.md`
//! next to this file.

use std::collections::BTreeSet;

use super::lexer::{LexedLine, LineKind, lex};
use super::{RetainedStatement, field, note_within_budget};
use crate::diagnostics::{Diagnostic, codes};
use crate::network::{BalancedNetwork, Bus, BusId};
use crate::{Error, Result};

const FMT: &str = "psse subsystem";

/// One subsystem description file.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SubsystemSet {
    /// Comment lines ahead of the first statement, as written. PSS/E leads a
    /// generated file with its `/PSS(R)E` stamp and a `COM` banner.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub header: Vec<String>,
    /// The subsystems, in file order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subsystems: Vec<Subsystem>,
    /// File level statements outside the grammar, kept as their trimmed line.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retained: Vec<RetainedStatement>,
}

/// One named subsystem: the union of its selector groups.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Subsystem {
    pub name: String,
    /// The groups whose bus sets are unioned. The implicit group, holding the
    /// selectors stated outside any `JOIN`, comes first when it has any.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<SelectorGroup>,
    /// Statements inside this subsystem, and outside any `JOIN` group, that
    /// are outside the grammar, kept as their trimmed line.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retained: Vec<RetainedStatement>,
}

/// One group of selectors whose bus sets intersect across selector types.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SelectorGroup {
    /// How the file stated the group: absent for the implicit group, which
    /// holds the selectors stated outside any `JOIN`, and present for a `JOIN`
    /// block whether or not that block states a name. Two `JOIN` blocks with
    /// no name are two groups, and their bus sets union rather than intersect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub join: Option<JoinName>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selectors: Vec<SubsystemSelector>,
    /// Statements read while this `JOIN` was open that are outside the
    /// grammar, kept as their trimmed line: a nested `JOIN`, a selector line
    /// whose values are not numbers, and any other unrecognized line. The
    /// writer states them inside the group, before its `END`, so they read
    /// back into the same group. The implicit group holds none, because a line
    /// stated outside any `JOIN` is kept on the subsystem.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retained: Vec<RetainedStatement>,
}

/// What a `JOIN` statement stated after the keyword.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum JoinName {
    /// `JOIN` with no name, which the writer states as `JOIN`.
    Anonymous,
    /// `JOIN name`.
    Named { name: String },
}

/// One bus selector. A statement naming a single value reads as a range whose
/// `from` and `to` are equal; both ends are inclusive.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SubsystemSelector {
    Area {
        from: usize,
        to: usize,
    },
    Zone {
        from: usize,
        to: usize,
    },
    Owner {
        from: usize,
        to: usize,
    },
    Bus {
        from: BusId,
        to: BusId,
    },
    /// A base kV band, inclusive at both ends.
    KvRange {
        lo: f64,
        hi: f64,
    },
}

/// Output of a tolerant subsystem read: the set plus the reader's notes on
/// statements it kept as text.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct SubsystemParsed {
    pub set: SubsystemSet,
    /// The reader's notes as structured records.
    pub diagnostics: Vec<Diagnostic>,
}

impl SubsystemParsed {
    fn note(&mut self, info: &'static crate::diagnostics::DiagnosticInfo, message: String) {
        note_within_budget(
            &mut self.diagnostics,
            info,
            &codes::READ_SUB_NOTES_TRUNCATED,
            message,
        );
    }

    fn unrecognized(&mut self, number: usize, text: &str) {
        self.note(
            &codes::READ_SUB_STATEMENT_UNRECOGNIZED,
            format!("line {number}: statement kept as text: {text}"),
        );
    }

    fn malformed(&mut self, number: usize, text: &str) {
        self.note(
            &codes::READ_SUB_SOURCE_MALFORMED,
            format!("line {number}: a selector states no number and was kept as text: {text}"),
        );
    }
}

fn bad(message: String) -> Error {
    Error::FormatRead {
        format: FMT,
        message,
    }
}

impl SubsystemSet {
    /// Read a `.sub` file from UTF-8 text. Keywords are case insensitive.
    ///
    /// A statement the grammar does not cover keeps its trimmed line where the
    /// file stated it, and is reported: in [`SelectorGroup::retained`] inside
    /// an open `JOIN`, in [`Subsystem::retained`] inside a subsystem, and in
    /// [`SubsystemSet::retained`] at file level. Lines after a file level
    /// `END` are kept at file level, marked
    /// [`RetainedStatement::after_end`], and reported once.
    ///
    /// # Errors
    /// [`Error::FormatRead`] when a `SUBSYSTEM` starts inside another, or when
    /// a subsystem or a `JOIN` group is still open at end of input. The
    /// message names the 1-based line.
    pub fn parse(text: &str) -> Result<SubsystemParsed> {
        let mut reader = Reader::new();
        for line in lex(text) {
            reader.read_line(&line)?;
        }
        reader.finish()
    }

    /// Write the set as `.sub` text: the header lines as written, the
    /// subsystems in order with their selectors and `JOIN` groups, the file
    /// level statements kept from before the file `END`, a final `END`, and
    /// then the statements read after that `END`. Every line ends with a
    /// newline.
    ///
    /// Each statement kept as text is written where it was read: inside its
    /// `JOIN` group before that group's `END`, inside its subsystem before the
    /// subsystem's `END`, or at file level. Reading the result back gives the
    /// same set, except that a statement kept as text reads back from a
    /// different line when the writer's order differs from the source's.
    #[must_use]
    pub fn to_sub(&self) -> String {
        use std::fmt::Write as _;

        let mut out = String::new();
        for line in &self.header {
            out.push_str(line);
            out.push('\n');
        }
        for subsystem in &self.subsystems {
            let _ = writeln!(out, "SUBSYSTEM '{}'", subsystem.name);
            for group in &subsystem.groups {
                match &group.join {
                    None => {
                        for selector in &group.selectors {
                            let _ = writeln!(out, "   {}", write_selector(selector));
                        }
                        write_retained(&mut out, &group.retained);
                    }
                    Some(join) => {
                        match join {
                            JoinName::Anonymous => out.push_str("   JOIN\n"),
                            JoinName::Named { name } => {
                                let _ = writeln!(out, "   JOIN '{name}'");
                            }
                        }
                        for selector in &group.selectors {
                            let _ = writeln!(out, "   {}", write_selector(selector));
                        }
                        write_retained(&mut out, &group.retained);
                        out.push_str("   END\n");
                    }
                }
            }
            write_retained(&mut out, &subsystem.retained);
            out.push_str("END\n");
        }
        for statement in self.retained.iter().filter(|kept| !kept.after_end) {
            out.push_str(&statement.text);
            out.push('\n');
        }
        out.push_str("END\n");
        for statement in self.retained.iter().filter(|kept| kept.after_end) {
            out.push_str(&statement.text);
            out.push('\n');
        }
        out
    }

    /// The subsystem of this name, matched without case and without
    /// surrounding whitespace, as a `.con` or `.mon` statement names it.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Subsystem> {
        let wanted = name.trim();
        self.subsystems
            .iter()
            .find(|subsystem| subsystem.name.trim().eq_ignore_ascii_case(wanted))
    }
}

/// A `JOIN` group whose `END` has not been read yet.
struct OpenJoin {
    name: JoinName,
    opened: usize,
    selectors: Vec<SubsystemSelector>,
    retained: Vec<RetainedStatement>,
}

/// A subsystem whose `END` has not been read yet.
struct OpenSubsystem {
    name: String,
    opened: usize,
    /// Selectors stated outside any `JOIN`.
    implicit: Vec<SubsystemSelector>,
    groups: Vec<SelectorGroup>,
    retained: Vec<RetainedStatement>,
    join: Option<OpenJoin>,
}

impl OpenSubsystem {
    /// The subsystem as it closes: the implicit group first when it holds any
    /// selector, then the `JOIN` groups in the order they were read.
    fn close(self) -> Subsystem {
        let mut groups = Vec::with_capacity(self.groups.len() + 1);
        if !self.implicit.is_empty() {
            groups.push(SelectorGroup {
                join: None,
                selectors: self.implicit,
                retained: Vec::new(),
            });
        }
        groups.extend(self.groups);
        Subsystem {
            name: self.name,
            groups,
            retained: self.retained,
        }
    }
}

/// The reader's state while it walks the lines of one file.
struct Reader {
    parsed: SubsystemParsed,
    subsystem: Option<OpenSubsystem>,
    /// Whether a statement line has been read; comment lines before the first
    /// one are the file header.
    seen_statement: bool,
    /// Whether the file level `END` has been read.
    ended: bool,
    noted_text_after_end: bool,
}

impl Reader {
    fn new() -> Self {
        Reader {
            parsed: SubsystemParsed {
                set: SubsystemSet::default(),
                diagnostics: Vec::new(),
            },
            subsystem: None,
            seen_statement: false,
            ended: false,
            noted_text_after_end: false,
        }
    }

    fn read_line(&mut self, line: &LexedLine<'_>) -> Result<()> {
        if self.ended {
            self.keep_after_end(line);
            return Ok(());
        }
        if !self.take_header(line) {
            return Ok(());
        }
        let upper = line.keywords();
        let words = line.words();
        if self.subsystem.is_some() {
            return self.read_subsystem_line(line, &upper, &words);
        }
        self.read_file_line(line, &upper, &words);
        Ok(())
    }

    fn finish(self) -> Result<SubsystemParsed> {
        if let Some(open) = self.subsystem {
            if let Some(join) = open.join {
                return Err(bad(format!("line {}: JOIN has no END", join.opened)));
            }
            return Err(bad(format!(
                "line {}: SUBSYSTEM '{}' has no END",
                open.opened, open.name
            )));
        }
        Ok(self.parsed)
    }

    /// Collect a comment line ahead of the first statement into the header.
    /// Returns whether the caller should read this line as a statement.
    fn take_header(&mut self, line: &LexedLine<'_>) -> bool {
        if self.seen_statement {
            return line.kind == LineKind::Statement;
        }
        match line.kind {
            LineKind::Blank => false,
            LineKind::Comment => {
                self.parsed.set.header.push(line.text.to_owned());
                false
            }
            LineKind::Statement => {
                self.seen_statement = true;
                true
            }
        }
    }

    /// Keep a statement line that follows the file level `END`, and report the
    /// first one. A further bare `END` states nothing and is dropped, because
    /// files carry one or two of them.
    ///
    /// The statement carries `after_end`, and `to_sub` states it after the
    /// `END` it writes. A line stated there that opens a subsystem would
    /// otherwise read back as grammar rather than as text.
    fn keep_after_end(&mut self, line: &LexedLine<'_>) {
        if line.kind != LineKind::Statement || is_end(line) {
            return;
        }
        if !self.noted_text_after_end {
            self.noted_text_after_end = true;
            self.parsed.note(
                &codes::READ_SUB_TEXT_AFTER_END,
                format!("line {}: text follows the file END", line.number),
            );
        }
        self.parsed.set.retained.push(RetainedStatement {
            line: line.number,
            text: line.trimmed().to_owned(),
            after_end: true,
        });
    }

    fn read_file_line(&mut self, line: &LexedLine<'_>, upper: &[String], words: &[&str]) {
        match upper[0].as_str() {
            "SUBSYSTEM" | "SYSTEM" => self.open_subsystem(line, upper, words),
            "END" if upper.len() == 1 => self.ended = true,
            _ => {
                self.parsed.unrecognized(line.number, line.trimmed());
                self.parsed.set.retained.push(RetainedStatement {
                    line: line.number,
                    text: line.trimmed().to_owned(),
                    after_end: false,
                });
            }
        }
    }

    /// Open a subsystem and read the selectors stated on the same line. A
    /// trailing `END` there closes the subsystem at once.
    fn open_subsystem(&mut self, line: &LexedLine<'_>, upper: &[String], words: &[&str]) {
        let name = words
            .get(1)
            .map_or_else(String::new, |word| word.trim().to_owned());
        self.subsystem = Some(OpenSubsystem {
            name,
            opened: line.number,
            implicit: Vec::new(),
            groups: Vec::new(),
            retained: Vec::new(),
            join: None,
        });
        if upper.len() <= 2 {
            return;
        }
        self.read_selector_tail(line, upper, words, 2);
    }

    /// Read the selectors stated after a `SUBSYSTEM` or `JOIN` keyword and its
    /// name. A trailing `END` closes what the line opened.
    ///
    /// A tail outside the grammar keeps its own line, so that writing the
    /// subsystem back states the opening keyword and the tail separately and
    /// reading that again gives the same subsystem.
    fn read_selector_tail(
        &mut self,
        line: &LexedLine<'_>,
        upper: &[String],
        words: &[&str],
        at: usize,
    ) {
        match take_selectors(upper, at) {
            SelectorParse::Read { selectors, ended } => {
                self.add_selectors(selectors);
                if ended {
                    self.close_join_or_subsystem();
                }
            }
            SelectorParse::Malformed => {
                let tail = tail_text(words, at);
                self.parsed.malformed(line.number, &tail);
                self.keep_in_subsystem(line.number, tail);
            }
            SelectorParse::Unrecognized => {
                let tail = tail_text(words, at);
                self.parsed.unrecognized(line.number, &tail);
                self.keep_in_subsystem(line.number, tail);
            }
        }
    }

    /// Open a `JOIN` group and read the rest of its line. The token after the
    /// keyword is the group's name unless it opens a selector, so `JOIN AREA 1`
    /// opens a group with no name over area 1.
    fn open_join(&mut self, line: &LexedLine<'_>, upper: &[String], words: &[&str]) {
        let named = upper
            .get(1)
            .is_some_and(|word| word != "END" && !is_selector_keyword(word));
        let name = if named {
            JoinName::Named {
                name: words[1].trim().to_owned(),
            }
        } else {
            JoinName::Anonymous
        };
        let at = usize::from(named) + 1;
        if let Some(open) = self.subsystem.as_mut() {
            open.join = Some(OpenJoin {
                name,
                opened: line.number,
                selectors: Vec::new(),
                retained: Vec::new(),
            });
        }
        if at < upper.len() {
            self.read_selector_tail(line, upper, words, at);
        }
    }

    fn read_subsystem_line(
        &mut self,
        line: &LexedLine<'_>,
        upper: &[String],
        words: &[&str],
    ) -> Result<()> {
        if is_end(line) {
            self.close_join_or_subsystem();
            return Ok(());
        }
        if matches!(upper[0].as_str(), "SUBSYSTEM" | "SYSTEM") {
            let name = self
                .subsystem
                .as_ref()
                .map_or("", |open| open.name.as_str());
            return Err(bad(format!(
                "line {}: SUBSYSTEM starts before subsystem '{name}' reached END",
                line.number
            )));
        }
        if upper[0] == "JOIN"
            && self
                .subsystem
                .as_ref()
                .is_some_and(|open| open.join.is_none())
        {
            self.open_join(line, upper, words);
            return Ok(());
        }
        match take_selectors(upper, 0) {
            SelectorParse::Read { selectors, ended } => {
                self.add_selectors(selectors);
                if ended {
                    self.close_join_or_subsystem();
                }
            }
            SelectorParse::Malformed => {
                self.parsed.malformed(line.number, line.trimmed());
                self.keep_in_subsystem(line.number, line.trimmed().to_owned());
            }
            SelectorParse::Unrecognized => {
                self.parsed.unrecognized(line.number, line.trimmed());
                self.keep_in_subsystem(line.number, line.trimmed().to_owned());
            }
        }
        Ok(())
    }

    fn add_selectors(&mut self, selectors: Vec<SubsystemSelector>) {
        let Some(open) = self.subsystem.as_mut() else {
            return;
        };
        match open.join.as_mut() {
            Some(join) => join.selectors.extend(selectors),
            None => open.implicit.extend(selectors),
        }
    }

    /// Keep one line as text where the file stated it: inside the open `JOIN`
    /// group when there is one, on the subsystem otherwise.
    fn keep_in_subsystem(&mut self, line: usize, text: String) {
        let Some(open) = self.subsystem.as_mut() else {
            return;
        };
        let kept = RetainedStatement {
            line,
            text,
            after_end: false,
        };
        match open.join.as_mut() {
            Some(join) => join.retained.push(kept),
            None => open.retained.push(kept),
        }
    }

    /// An `END` closes the open `JOIN` group when there is one, and the
    /// subsystem otherwise.
    fn close_join_or_subsystem(&mut self) {
        if let Some(open) = self.subsystem.as_mut()
            && let Some(join) = open.join.take()
        {
            open.groups.push(SelectorGroup {
                join: Some(join.name),
                selectors: join.selectors,
                retained: join.retained,
            });
            return;
        }
        self.close_subsystem();
    }

    fn close_subsystem(&mut self) {
        if let Some(open) = self.subsystem.take() {
            self.parsed.set.subsystems.push(open.close());
        }
    }
}

/// Write each kept statement as its own line.
fn write_retained(out: &mut String, statements: &[RetainedStatement]) {
    for statement in statements {
        out.push_str(&statement.text);
        out.push('\n');
    }
}

/// The tokens from `at` to the end of the line, rejoined as one line. A token
/// holding whitespace is quoted, so the rejoined line lexes back into the same
/// tokens.
fn tail_text(words: &[&str], at: usize) -> String {
    words[at..]
        .iter()
        .map(|word| field(word))
        .collect::<Vec<String>>()
        .join(" ")
}

/// Whether the line is a bare `END`.
fn is_end(line: &LexedLine<'_>) -> bool {
    line.kind == LineKind::Statement
        && line.tokens.len() == 1
        && line.tokens[0].text.eq_ignore_ascii_case("END")
}

// ---------------------------------------------------------------------------
// Selector grammar
// ---------------------------------------------------------------------------

/// What a run of selector tokens read as.
enum SelectorParse {
    Read {
        selectors: Vec<SubsystemSelector>,
        /// Whether a trailing `END` on the same line closed the subsystem.
        ended: bool,
    },
    /// A selector keyword whose values are not the numbers it needs.
    Malformed,
    /// The first token is not a selector keyword.
    Unrecognized,
}

/// Read every selector from `at` to the end of the line. A trailing `END`
/// closes the subsystem, which is how a one line subsystem is written.
fn take_selectors(upper: &[String], at: usize) -> SelectorParse {
    let mut selectors = Vec::new();
    let mut index = at;
    while index < upper.len() {
        if upper[index] == "END" && index + 1 == upper.len() {
            return SelectorParse::Read {
                selectors,
                ended: true,
            };
        }
        let Some((selector, next)) = take_selector(upper, index) else {
            return if index == at && !is_selector_keyword(&upper[index]) {
                SelectorParse::Unrecognized
            } else {
                SelectorParse::Malformed
            };
        };
        selectors.push(selector);
        index = next;
    }
    SelectorParse::Read {
        selectors,
        ended: false,
    }
}

/// Whether the word opens a selector, whatever follows it.
fn is_selector_keyword(word: &str) -> bool {
    matches!(
        word,
        "AREA" | "AREAS" | "ZONE" | "ZONES" | "OWNER" | "OWNERS" | "BUS" | "BUSES" | "KVRANGE"
    )
}

/// One selector starting at `at`, with the index just past it.
fn take_selector(upper: &[String], at: usize) -> Option<(SubsystemSelector, usize)> {
    let integer = |offset: usize| upper.get(at + offset)?.parse::<usize>().ok();
    let float = |offset: usize| {
        upper
            .get(at + offset)?
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
    };
    match upper[at].as_str() {
        "AREA" => Some((
            SubsystemSelector::Area {
                from: integer(1)?,
                to: integer(1)?,
            },
            at + 2,
        )),
        "AREAS" => Some((
            SubsystemSelector::Area {
                from: integer(1)?,
                to: integer(2)?,
            },
            at + 3,
        )),
        "ZONE" => Some((
            SubsystemSelector::Zone {
                from: integer(1)?,
                to: integer(1)?,
            },
            at + 2,
        )),
        "ZONES" => Some((
            SubsystemSelector::Zone {
                from: integer(1)?,
                to: integer(2)?,
            },
            at + 3,
        )),
        "OWNER" => Some((
            SubsystemSelector::Owner {
                from: integer(1)?,
                to: integer(1)?,
            },
            at + 2,
        )),
        "OWNERS" => Some((
            SubsystemSelector::Owner {
                from: integer(1)?,
                to: integer(2)?,
            },
            at + 3,
        )),
        "BUS" => Some((
            SubsystemSelector::Bus {
                from: BusId(integer(1)?),
                to: BusId(integer(1)?),
            },
            at + 2,
        )),
        "BUSES" => Some((
            SubsystemSelector::Bus {
                from: BusId(integer(1)?),
                to: BusId(integer(2)?),
            },
            at + 3,
        )),
        // A band whose ends run the wrong way round states no range of base
        // kV values, so the line stays text rather than reading as a selector
        // no writer could state again.
        "KVRANGE" => {
            let (lo, hi) = (float(1)?, float(2)?);
            (lo <= hi).then_some((SubsystemSelector::KvRange { lo, hi }, at + 3))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Bus selection
// ---------------------------------------------------------------------------

/// The selector families a group intersects across.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectorKind {
    Area,
    Zone,
    Owner,
    Bus,
    Kv,
}

const SELECTOR_KINDS: [SelectorKind; 5] = [
    SelectorKind::Area,
    SelectorKind::Zone,
    SelectorKind::Owner,
    SelectorKind::Bus,
    SelectorKind::Kv,
];

fn kind_of(selector: &SubsystemSelector) -> SelectorKind {
    match selector {
        SubsystemSelector::Area { .. } => SelectorKind::Area,
        SubsystemSelector::Zone { .. } => SelectorKind::Zone,
        SubsystemSelector::Owner { .. } => SelectorKind::Owner,
        SubsystemSelector::Bus { .. } => SelectorKind::Bus,
        SubsystemSelector::KvRange { .. } => SelectorKind::Kv,
    }
}

/// The PSS/E owner of a bus: the retained owner number, or 1 when the source
/// stated the default the reader drops.
pub(super) fn owner_of(bus: &Bus) -> usize {
    bus.extras
        .get("psse_owner")
        .and_then(serde_json::Value::as_i64)
        .and_then(|owner| usize::try_from(owner).ok())
        .unwrap_or(1)
}

fn selects(selector: &SubsystemSelector, bus: &Bus) -> bool {
    match selector {
        SubsystemSelector::Area { from, to } => (*from..=*to).contains(&bus.area),
        SubsystemSelector::Zone { from, to } => (*from..=*to).contains(&bus.zone),
        SubsystemSelector::Owner { from, to } => (*from..=*to).contains(&owner_of(bus)),
        SubsystemSelector::Bus { from, to } => (from.0..=to.0).contains(&bus.id.0),
        SubsystemSelector::KvRange { lo, hi } => bus.base_kv >= *lo && bus.base_kv <= *hi,
    }
}

impl Subsystem {
    /// The buses of `net` this subsystem names.
    ///
    /// Within one group the buses matching each selector family present are
    /// unioned within the family and intersected across the families, and the
    /// groups of a subsystem are unioned. A group with no selector, and a
    /// subsystem with no group, names no bus.
    #[must_use]
    pub fn select_buses(&self, net: &BalancedNetwork) -> BTreeSet<BusId> {
        let mut selected = BTreeSet::new();
        for group in &self.groups {
            selected.extend(select_group(group, net));
        }
        selected
    }
}

fn select_group(group: &SelectorGroup, net: &BalancedNetwork) -> BTreeSet<BusId> {
    let mut selected: Option<BTreeSet<BusId>> = None;
    for kind in SELECTOR_KINDS {
        let family: Vec<&SubsystemSelector> = group
            .selectors
            .iter()
            .filter(|selector| kind_of(selector) == kind)
            .collect();
        if family.is_empty() {
            continue;
        }
        let matching: BTreeSet<BusId> = net
            .buses()
            .iter()
            .filter(|bus| family.iter().any(|selector| selects(selector, bus)))
            .map(|bus| bus.id)
            .collect();
        selected = Some(match selected {
            None => matching,
            Some(held) => held.intersection(&matching).copied().collect(),
        });
    }
    selected.unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

/// A float in its `Display` form, with `.0` added when that form states no
/// decimal point, so a written kV or per unit value looks like the ones the
/// files state and reads back as the same `f64`.
pub(super) fn decimal(value: f64) -> String {
    let written = value.to_string();
    if written.contains(['.', 'e', 'E', 'i', 'N']) {
        written
    } else {
        format!("{written}.0")
    }
}

fn write_selector(selector: &SubsystemSelector) -> String {
    match selector {
        SubsystemSelector::Area { from, to } if from == to => format!("AREA {from}"),
        SubsystemSelector::Area { from, to } => format!("AREAS {from} {to}"),
        SubsystemSelector::Zone { from, to } if from == to => format!("ZONE {from}"),
        SubsystemSelector::Zone { from, to } => format!("ZONES {from} {to}"),
        SubsystemSelector::Owner { from, to } if from == to => format!("OWNER {from}"),
        SubsystemSelector::Owner { from, to } => format!("OWNERS {from} {to}"),
        SubsystemSelector::Bus { from, to } if from == to => format!("BUS {}", from.0),
        SubsystemSelector::Bus { from, to } => format!("BUSES {} {}", from.0, to.0),
        SubsystemSelector::KvRange { lo, hi } => {
            format!("KVRANGE {} {}", decimal(*lo), decimal(*hi))
        }
    }
}
