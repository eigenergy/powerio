//! PSS/E monitored element files (`.mon`).
//!
//! A `.mon` file names what a contingency analysis reports on: branch flows,
//! interface flows, and bus voltages. Every scope that names a subsystem is
//! resolved against a `.sub` file's [`SubsystemSet`].
//!
//! ```text
//! MONITOR VOLTAGE RANGE SUBSYSTEM 'ILLINOIS200' 0.950 1.050
//! MONITOR BRANCHES IN SUBSYSTEM 'ILLINOIS200'
//! MONITOR TIES FROM SUBSYSTEM 'ILLINOIS200'
//! END
//! ```
//!
//! [`MonitoredSet::parse`] reads UTF-8 text and touches no filesystem.
//! Statements outside the grammar keep their trimmed line and are reported.
//! [`MonitoredSet::to_mon`] writes the set back in one canonical spelling, and
//! [`MonitoredSet::resolve`] binds it to the rows of a [`BalancedNetwork`].
//!
//! The grammar, its evidence, and the writer's spellings are in `FORMAT.md`
//! next to this file.

use std::collections::BTreeSet;

use super::expand::low_voltage_bus;
use super::lexer::{LexedLine, LineKind, lex};
use super::sub::{SubsystemSet, decimal};
use super::{PsseEquipmentIndex, RetainedStatement, field, note_within_budget};
use crate::diagnostics::{Diagnostic, codes};
use crate::network::{BalancedNetwork, BusId};
use crate::{Error, Result};

const FMT: &str = "psse monitored elements";

/// Base kV values this far apart name the same voltage level.
const KV_TOLERANCE: f64 = 1e-6;

/// One monitored element file.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MonitoredSet {
    /// Comment lines ahead of the first statement, as written.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub header: Vec<String>,
    /// The monitor statements, in file order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub statements: Vec<MonitorStatement>,
    /// Statements outside the grammar, kept as their trimmed line.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retained: Vec<RetainedStatement>,
}

/// One monitor statement.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum MonitorStatement {
    /// Every branch with both terminals in the subsystem.
    BranchesInSubsystem {
        subsystem: String,
        /// `3WLOWVOLTAGE`: also the three winding transformers whose lowest
        /// voltage winding sits in the subsystem.
        low_voltage_3w: bool,
    },
    /// Every branch with exactly one terminal in the subsystem.
    TiesFromSubsystem { subsystem: String },
    /// The branches a `MONITOR BRANCHES` block lists by terminal pair.
    Branches {
        branches: Vec<BranchRef>,
        /// The block's lines that state no branch, kept as text. The writer
        /// states them inside the block, before its `END`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        retained: Vec<RetainedStatement>,
    },
    /// A named interface and the branches whose flows sum over it.
    Interface {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rating_mw: Option<f64>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        branches: Vec<BranchRef>,
        /// The block's lines that state no branch, kept as text. The writer
        /// states them inside the block, before its `END`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        retained: Vec<RetainedStatement>,
    },
    /// A voltage magnitude band over a scope of buses.
    VoltageRange {
        scope: MonitorScope,
        vmin: f64,
        vmax: f64,
    },
    /// A voltage deviation band over a scope of buses. A statement naming one
    /// value states the downward limit alone.
    VoltageDeviation {
        scope: MonitorScope,
        down: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        up: Option<f64>,
    },
}

/// One branch named by its terminal buses and circuit id.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct BranchRef {
    pub from: BusId,
    pub to: BusId,
    /// An absent circuit id in the source reads as `1`.
    pub circuit: String,
}

/// The buses a voltage statement applies to.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum MonitorScope {
    AllBuses,
    Subsystem(String),
    Bus(BusId),
    Area(usize),
    Zone(usize),
    Owner(usize),
    /// Every bus at this base kV.
    Kv(f64),
}

/// Output of a tolerant monitored element read.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct MonitoredParsed {
    pub set: MonitoredSet,
    /// The reader's notes as structured records.
    pub diagnostics: Vec<Diagnostic>,
}

impl MonitoredParsed {
    fn note(&mut self, info: &'static crate::diagnostics::DiagnosticInfo, message: String) {
        note_within_budget(
            &mut self.diagnostics,
            info,
            &codes::READ_MON_NOTES_TRUNCATED,
            message,
        );
    }

    fn unrecognized(&mut self, number: usize, text: &str) {
        self.note(
            &codes::READ_MON_STATEMENT_UNRECOGNIZED,
            format!("line {number}: statement kept as text: {text}"),
        );
    }
}

fn bad(message: String) -> Error {
    Error::FormatRead {
        format: FMT,
        message,
    }
}

impl MonitoredSet {
    /// Read a `.mon` file from UTF-8 text. Keywords are case insensitive.
    ///
    /// A statement the grammar does not cover keeps its trimmed line where the
    /// file stated it, and is reported: on the statement inside an open
    /// `MONITOR BRANCHES` or `MONITOR INTERFACE` block, and in
    /// [`MonitoredSet::retained`] outside one. Lines after a file level `END`
    /// are kept at file level, marked [`RetainedStatement::after_end`], and
    /// reported once.
    ///
    /// # Errors
    /// [`Error::FormatRead`] when a `MONITOR BRANCHES` or `MONITOR INTERFACE`
    /// block is still open at end of input. The message names the 1-based
    /// line.
    pub fn parse(text: &str) -> Result<MonitoredParsed> {
        let mut reader = Reader::new();
        for line in lex(text) {
            reader.read_line(&line);
        }
        reader.finish()
    }

    /// Write the set as `.mon` text: the header lines as written, the
    /// statements in order, the statements kept from before the file `END`, a
    /// final `END`, and then the statements read after that `END`. Every line
    /// ends with a newline.
    ///
    /// A line kept from inside a block is written inside that block, before
    /// its `END`, so it reads back into the same block.
    #[must_use]
    pub fn to_mon(&self) -> String {
        let mut out = String::new();
        for line in &self.header {
            out.push_str(line);
            out.push('\n');
        }
        for statement in &self.statements {
            out.push_str(&write_statement(statement));
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
}

/// A `MONITOR BRANCHES` or `MONITOR INTERFACE` block whose `END` has not been
/// read yet.
struct OpenBlock {
    opened: usize,
    /// The interface this block belongs to, or `None` for a bare branch list.
    interface: Option<(String, Option<f64>)>,
    branches: Vec<BranchRef>,
    retained: Vec<RetainedStatement>,
}

/// The reader's state while it walks the lines of one file.
struct Reader {
    parsed: MonitoredParsed,
    block: Option<OpenBlock>,
    seen_statement: bool,
    ended: bool,
    noted_text_after_end: bool,
}

impl Reader {
    fn new() -> Self {
        Reader {
            parsed: MonitoredParsed {
                set: MonitoredSet::default(),
                diagnostics: Vec::new(),
            },
            block: None,
            seen_statement: false,
            ended: false,
            noted_text_after_end: false,
        }
    }

    fn read_line(&mut self, line: &LexedLine<'_>) {
        if self.block.is_some() {
            self.read_block_line(line);
            return;
        }
        if self.ended {
            self.keep_after_end(line);
            return;
        }
        if !self.take_header(line) {
            return;
        }
        let upper = line.keywords();
        let words = line.words();
        self.read_statement(line, &upper, &words);
    }

    fn finish(self) -> Result<MonitoredParsed> {
        if let Some(open) = self.block {
            let what = match open.interface {
                Some((name, _)) => format!("MONITOR INTERFACE '{name}'"),
                None => "MONITOR BRANCHES".to_owned(),
            };
            return Err(bad(format!("line {}: {what} has no END", open.opened)));
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
    /// The statement carries `after_end`, and `to_mon` states it after the
    /// `END` it writes. A line stated there that opens a monitor statement
    /// would otherwise read back as grammar rather than as text.
    fn keep_after_end(&mut self, line: &LexedLine<'_>) {
        if line.kind != LineKind::Statement || is_end(line) {
            return;
        }
        if !self.noted_text_after_end {
            self.noted_text_after_end = true;
            self.parsed.note(
                &codes::READ_MON_TEXT_AFTER_END,
                format!("line {}: text follows the file END", line.number),
            );
        }
        self.keep_statement(line, true);
    }

    /// Keep one line as a file level statement. `after_end` marks a line the
    /// file states after its `END`, which the writer states after the `END` it
    /// writes.
    fn keep_statement(&mut self, line: &LexedLine<'_>, after_end: bool) {
        self.parsed.set.retained.push(RetainedStatement {
            line: line.number,
            text: line.trimmed().to_owned(),
            after_end,
        });
    }

    /// Add one branch to the open block, and close the block on its `END`.
    fn read_block_line(&mut self, line: &LexedLine<'_>) {
        if line.kind != LineKind::Statement {
            return;
        }
        if is_end(line) {
            let Some(open) = self.block.take() else {
                return;
            };
            self.parsed.set.statements.push(match open.interface {
                Some((name, rating_mw)) => MonitorStatement::Interface {
                    name,
                    rating_mw,
                    branches: open.branches,
                    retained: open.retained,
                },
                None => MonitorStatement::Branches {
                    branches: open.branches,
                    retained: open.retained,
                },
            });
            return;
        }
        let words = line.words();
        if let Some(branch) = parse_branch_ref(&words) {
            if let Some(open) = self.block.as_mut() {
                open.branches.push(branch);
            }
            return;
        }
        self.parsed.note(
            &codes::READ_MON_SOURCE_MALFORMED,
            format!(
                "line {}: a monitored block line states no branch and was kept as text: {}",
                line.number,
                line.trimmed()
            ),
        );
        if let Some(open) = self.block.as_mut() {
            open.retained.push(RetainedStatement {
                line: line.number,
                text: line.trimmed().to_owned(),
                after_end: false,
            });
        }
    }

    fn read_statement(&mut self, line: &LexedLine<'_>, upper: &[String], words: &[&str]) {
        if is_end(line) {
            self.ended = true;
            return;
        }
        if upper[0] == "MONITOR"
            && let Some(read) = self.read_monitor(line, upper, words)
        {
            match read {
                Read::Statement(statement) => self.parsed.set.statements.push(statement),
                Read::OpenedBlock => {}
            }
            return;
        }
        self.parsed.unrecognized(line.number, line.trimmed());
        self.keep_statement(line, false);
    }

    /// One `MONITOR` statement, or `None` when the tail is outside the
    /// grammar.
    fn read_monitor(
        &mut self,
        line: &LexedLine<'_>,
        upper: &[String],
        words: &[&str],
    ) -> Option<Read> {
        match upper.get(1)?.as_str() {
            "BRANCHES" | "LINES" => {
                if upper.len() == 2 {
                    self.block = Some(OpenBlock {
                        opened: line.number,
                        interface: None,
                        branches: Vec::new(),
                        retained: Vec::new(),
                    });
                    return Some(Read::OpenedBlock);
                }
                let (subsystem, low_voltage_3w) = parse_in_subsystem(upper, words, 2)?;
                Some(Read::Statement(MonitorStatement::BranchesInSubsystem {
                    subsystem,
                    low_voltage_3w,
                }))
            }
            "TIES" => {
                let (subsystem, low_voltage_3w) = parse_in_subsystem(upper, words, 2)?;
                (!low_voltage_3w).then_some(Read::Statement(MonitorStatement::TiesFromSubsystem {
                    subsystem,
                }))
            }
            "INTERFACE" => {
                let name = words.get(2)?.trim().to_owned();
                let rating_mw = match parse_rating(upper, 3) {
                    Rating::Absent => None,
                    Rating::Stated(value) => Some(value),
                    Rating::Unreadable => return None,
                };
                self.block = Some(OpenBlock {
                    opened: line.number,
                    interface: Some((name, rating_mw)),
                    branches: Vec::new(),
                    retained: Vec::new(),
                });
                Some(Read::OpenedBlock)
            }
            "VOLTAGE" => parse_voltage(upper, words).map(Read::Statement),
            _ => None,
        }
    }
}

/// What one `MONITOR` line produced.
enum Read {
    Statement(MonitorStatement),
    OpenedBlock,
}

/// Whether the line is a bare `END`.
fn is_end(line: &LexedLine<'_>) -> bool {
    line.kind == LineKind::Statement
        && line.tokens.len() == 1
        && line.tokens[0].text.eq_ignore_ascii_case("END")
}

// ---------------------------------------------------------------------------
// Statement grammar
// ---------------------------------------------------------------------------

/// `IN|FROM SUBSYSTEM name [3WLOWVOLTAGE]` at `at`.
fn parse_in_subsystem(upper: &[String], words: &[&str], at: usize) -> Option<(String, bool)> {
    if !matches!(upper.get(at)?.as_str(), "IN" | "FROM") {
        return None;
    }
    if upper.get(at + 1)? != "SUBSYSTEM" {
        return None;
    }
    let subsystem = words.get(at + 2)?.trim().to_owned();
    let mut next = at + 3;
    let low_voltage_3w = upper.get(next).is_some_and(|word| word == "3WLOWVOLTAGE");
    if low_voltage_3w {
        next += 1;
    }
    (next == upper.len()).then_some((subsystem, low_voltage_3w))
}

/// What the tail after an interface name states.
enum Rating {
    /// The line ends after the name.
    Absent,
    Stated(f64),
    /// A tail that is not a rating, which rejects the line.
    Unreadable,
}

/// An optional `RATING x MW` tail at `at`.
fn parse_rating(upper: &[String], at: usize) -> Rating {
    if at >= upper.len() {
        return Rating::Absent;
    }
    if upper[at] != "RATING" {
        return Rating::Unreadable;
    }
    let Some(value) = upper
        .get(at + 1)
        .and_then(|word| word.parse::<f64>().ok())
        .filter(|value| value.is_finite())
    else {
        return Rating::Unreadable;
    };
    let mut next = at + 2;
    if upper.get(next).is_some_and(|word| word == "MW") {
        next += 1;
    }
    if next == upper.len() {
        Rating::Stated(value)
    } else {
        Rating::Unreadable
    }
}

/// `MONITOR VOLTAGE RANGE scope lo hi` or
/// `MONITOR VOLTAGE DEVIATION scope down [up]`.
fn parse_voltage(upper: &[String], words: &[&str]) -> Option<MonitorStatement> {
    let deviation = match upper.get(2)?.as_str() {
        "RANGE" => false,
        "DEVIATION" => true,
        _ => return None,
    };
    let (scope, at) = parse_scope(upper, words, 3)?;
    let values: Option<Vec<f64>> = upper[at..]
        .iter()
        .map(|word| word.parse::<f64>().ok().filter(|value| value.is_finite()))
        .collect();
    let values = values?;
    match (deviation, values.as_slice()) {
        (false, [vmin, vmax]) => Some(MonitorStatement::VoltageRange {
            scope,
            vmin: *vmin,
            vmax: *vmax,
        }),
        (true, [down]) => Some(MonitorStatement::VoltageDeviation {
            scope,
            down: *down,
            up: None,
        }),
        (true, [down, up]) => Some(MonitorStatement::VoltageDeviation {
            scope,
            down: *down,
            up: Some(*up),
        }),
        _ => None,
    }
}

/// One scope at `at`, with the index just past it.
fn parse_scope(upper: &[String], words: &[&str], at: usize) -> Option<(MonitorScope, usize)> {
    let integer = |offset: usize| upper.get(at + offset)?.parse::<usize>().ok();
    match upper.get(at)?.as_str() {
        "ALL" if upper.get(at + 1).is_some_and(|word| word == "BUSES") => {
            Some((MonitorScope::AllBuses, at + 2))
        }
        "SUBSYSTEM" => Some((
            MonitorScope::Subsystem(words.get(at + 1)?.trim().to_owned()),
            at + 2,
        )),
        "BUS" => Some((MonitorScope::Bus(BusId(integer(1)?)), at + 2)),
        "AREA" => Some((MonitorScope::Area(integer(1)?), at + 2)),
        "ZONE" => Some((MonitorScope::Zone(integer(1)?), at + 2)),
        "OWNER" => Some((MonitorScope::Owner(integer(1)?), at + 2)),
        "KV" => {
            let kv = upper.get(at + 1)?.parse::<f64>().ok()?;
            kv.is_finite().then_some((MonitorScope::Kv(kv), at + 2))
        }
        _ => None,
    }
}

/// `i j [ckt]` inside a monitored block.
fn parse_branch_ref(words: &[&str]) -> Option<BranchRef> {
    if words.len() > 3 {
        return None;
    }
    let from = words.first()?.parse::<usize>().ok()?;
    let to = words.get(1)?.parse::<usize>().ok()?;
    let circuit = words.get(2).map_or("1", |word| word.trim());
    Some(BranchRef {
        from: BusId(from),
        to: BusId(to),
        circuit: if circuit.is_empty() {
            "1".to_owned()
        } else {
            circuit.to_owned()
        },
    })
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

fn write_scope(scope: &MonitorScope) -> String {
    match scope {
        MonitorScope::AllBuses => "ALL BUSES".to_owned(),
        MonitorScope::Subsystem(name) => format!("SUBSYSTEM '{name}'"),
        MonitorScope::Bus(bus) => format!("BUS {}", bus.0),
        MonitorScope::Area(area) => format!("AREA {area}"),
        MonitorScope::Zone(zone) => format!("ZONE {zone}"),
        MonitorScope::Owner(owner) => format!("OWNER {owner}"),
        MonitorScope::Kv(kv) => format!("KV {}", decimal(*kv)),
    }
}

fn write_branch_block(
    head: &str,
    branches: &[BranchRef],
    retained: &[RetainedStatement],
) -> String {
    use std::fmt::Write as _;

    let mut out = format!("{head}\n");
    for branch in branches {
        let _ = writeln!(
            out,
            "{:>6} {:>6} {}",
            branch.from.0,
            branch.to.0,
            field(&branch.circuit)
        );
    }
    for statement in retained {
        out.push_str(&statement.text);
        out.push('\n');
    }
    out.push_str("END\n");
    out
}

fn write_statement(statement: &MonitorStatement) -> String {
    match statement {
        MonitorStatement::BranchesInSubsystem {
            subsystem,
            low_voltage_3w,
        } => {
            let tail = if *low_voltage_3w { " 3WLOWVOLTAGE" } else { "" };
            format!("MONITOR BRANCHES IN SUBSYSTEM '{subsystem}'{tail}\n")
        }
        MonitorStatement::TiesFromSubsystem { subsystem } => {
            format!("MONITOR TIES FROM SUBSYSTEM '{subsystem}'\n")
        }
        MonitorStatement::Branches { branches, retained } => {
            write_branch_block("MONITOR BRANCHES", branches, retained)
        }
        MonitorStatement::Interface {
            name,
            rating_mw,
            branches,
            retained,
        } => {
            let head = match rating_mw {
                Some(rating) => {
                    format!("MONITOR INTERFACE '{name}' RATING {} MW", decimal(*rating))
                }
                None => format!("MONITOR INTERFACE '{name}'"),
            };
            write_branch_block(&head, branches, retained)
        }
        MonitorStatement::VoltageRange { scope, vmin, vmax } => format!(
            "MONITOR VOLTAGE RANGE {} {} {}\n",
            write_scope(scope),
            decimal(*vmin),
            decimal(*vmax)
        ),
        MonitorStatement::VoltageDeviation { scope, down, up } => {
            let tail = match up {
                Some(up) => format!(" {}", decimal(*up)),
                None => String::new(),
            };
            format!(
                "MONITOR VOLTAGE DEVIATION {} {}{tail}\n",
                write_scope(scope),
                decimal(*down)
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Resolution against a network
// ---------------------------------------------------------------------------

/// What a whole monitored element set bound to. Rows are positions in the
/// tables of the network the set was resolved against.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct MonitoredResolution {
    /// Branches monitored for flow, from every statement that names some.
    pub branch_rows: BTreeSet<usize>,
    /// Three winding transformers a `3WLOWVOLTAGE` statement adds.
    pub transformer_3w_rows: BTreeSet<usize>,
    /// Branches monitored because they cross a subsystem border.
    pub tie_rows: BTreeSet<usize>,
    pub interfaces: Vec<ResolvedInterface>,
    pub voltage_ranges: Vec<ResolvedVoltageScope>,
    pub voltage_deviations: Vec<ResolvedVoltageScope>,
    /// The statements, or the branches inside them, that named nothing.
    pub unresolved: Vec<UnresolvedMonitor>,
}

/// One interface and the branches its flow sums over.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResolvedInterface {
    pub name: String,
    pub rating_mw: Option<f64>,
    /// In statement order; a branch named twice appears twice.
    pub members: Vec<InterfaceMember>,
}

/// One branch of an interface, and how the statement stated it against the
/// stored row.
///
/// A `.mon` interface line names its branch in either terminal order, and the
/// flow of a branch is stated from its stored `from` terminal to its stored
/// `to` terminal. A member the statement named the other way round therefore
/// enters the interface sum with its sign flipped, which is what `reversed`
/// states.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InterfaceMember {
    /// The row in `net.branches()`.
    pub row: usize,
    /// Whether the statement named the stored `to` terminal first.
    pub reversed: bool,
}

/// One voltage statement's buses and limits. `high` is absent for a deviation
/// statement that names one value.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResolvedVoltageScope {
    pub bus_rows: BTreeSet<usize>,
    pub low: f64,
    pub high: Option<f64>,
}

/// One statement that named nothing, kept with the reason.
#[derive(Debug, Clone, PartialEq)]
pub struct UnresolvedMonitor {
    pub statement: MonitorStatement,
    pub reason: UnresolvedMonitorReason,
}

/// Why a monitor statement named nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum UnresolvedMonitorReason {
    /// The subsystem set states no subsystem of that name.
    NoSuchSubsystem,
    NoSuchBranch {
        from: BusId,
        to: BusId,
        circuit: String,
    },
    /// The terminal pair and circuit id name more than one branch.
    AmbiguousBranch {
        from: BusId,
        to: BusId,
        circuit: String,
        matches: usize,
    },
}

impl MonitoredResolution {
    /// One `BUILD.MON.STATEMENT_UNRESOLVED` note per unresolved entry.
    #[must_use]
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        self.unresolved
            .iter()
            .map(|entry| {
                Diagnostic::of(
                    &codes::BUILD_MON_STATEMENT_UNRESOLVED,
                    describe(&entry.statement, &entry.reason),
                )
            })
            .collect()
    }
}

/// A one line account of a statement that named nothing.
fn describe(statement: &MonitorStatement, reason: &UnresolvedMonitorReason) -> String {
    let what = match statement {
        MonitorStatement::BranchesInSubsystem { subsystem, .. }
        | MonitorStatement::TiesFromSubsystem { subsystem } => {
            format!("monitored subsystem '{subsystem}'")
        }
        MonitorStatement::Branches { .. } => "monitored branches".to_owned(),
        MonitorStatement::Interface { name, .. } => format!("monitored interface '{name}'"),
        MonitorStatement::VoltageRange { .. } => "monitored voltage range".to_owned(),
        MonitorStatement::VoltageDeviation { .. } => "monitored voltage deviation".to_owned(),
    };
    match reason {
        UnresolvedMonitorReason::NoSuchSubsystem => {
            format!("{what}: the subsystem set states no such subsystem")
        }
        UnresolvedMonitorReason::NoSuchBranch { from, to, circuit } => {
            format!("{what}: no branch {from} to {to} circuit {circuit}")
        }
        UnresolvedMonitorReason::AmbiguousBranch {
            from,
            to,
            circuit,
            matches,
        } => format!("{what}: branch {from} to {to} circuit {circuit} names {matches} branches"),
    }
}

impl MonitoredSet {
    /// Bind every statement to the rows of `net`, reading subsystem names from
    /// `subsystems`.
    ///
    /// Resolution reports rather than refuses: a statement naming a subsystem
    /// or a branch that is not there is kept in
    /// [`MonitoredResolution::unresolved`] with its reason, and the statements
    /// that did bind stay listed. A statement kept as text names nothing and
    /// is not resolved at all.
    ///
    /// Service state does not enter: a monitored element is reported on
    /// whether or not the network states it in service.
    #[must_use]
    pub fn resolve(&self, net: &BalancedNetwork, subsystems: &SubsystemSet) -> MonitoredResolution {
        self.resolve_with(&PsseEquipmentIndex::new(net), subsystems)
    }

    /// [`MonitoredSet::resolve`] against an index built once, for a caller
    /// binding several files to one network.
    ///
    /// The network is the one the index borrows, so the rows it states always
    /// index that network's tables.
    #[must_use]
    pub fn resolve_with(
        &self,
        index: &PsseEquipmentIndex<'_>,
        subsystems: &SubsystemSet,
    ) -> MonitoredResolution {
        let mut out = MonitoredResolution::default();
        for statement in &self.statements {
            resolve_statement(statement, index.network(), subsystems, index, &mut out);
        }
        out
    }
}

fn resolve_statement(
    statement: &MonitorStatement,
    net: &BalancedNetwork,
    subsystems: &SubsystemSet,
    index: &PsseEquipmentIndex,
    out: &mut MonitoredResolution,
) {
    match statement {
        MonitorStatement::BranchesInSubsystem {
            subsystem,
            low_voltage_3w,
        } => {
            let Some(buses) = select(subsystems, subsystem, net) else {
                unresolved_subsystem(statement, out);
                return;
            };
            for (row, branch) in net.branches().iter().enumerate() {
                if buses.contains(&branch.from) && buses.contains(&branch.to) {
                    out.branch_rows.insert(row);
                }
            }
            if *low_voltage_3w {
                for (row, transformer) in net.transformers_3w().iter().enumerate() {
                    if buses.contains(&low_voltage_bus(net, index, transformer)) {
                        out.transformer_3w_rows.insert(row);
                    }
                }
            }
        }
        MonitorStatement::TiesFromSubsystem { subsystem } => {
            let Some(buses) = select(subsystems, subsystem, net) else {
                unresolved_subsystem(statement, out);
                return;
            };
            for (row, branch) in net.branches().iter().enumerate() {
                if buses.contains(&branch.from) != buses.contains(&branch.to) {
                    out.tie_rows.insert(row);
                }
            }
        }
        MonitorStatement::Branches { branches, .. } => {
            for branch in branches {
                match bind_branch(index, branch) {
                    Ok(row) => {
                        out.branch_rows.insert(row);
                    }
                    Err(reason) => out.unresolved.push(UnresolvedMonitor {
                        statement: statement.clone(),
                        reason,
                    }),
                }
            }
        }
        MonitorStatement::Interface {
            name,
            rating_mw,
            branches,
            ..
        } => {
            let mut resolved = ResolvedInterface {
                name: name.clone(),
                rating_mw: *rating_mw,
                members: Vec::new(),
            };
            for branch in branches {
                match bind_branch(index, branch) {
                    Ok(row) => resolved.members.push(member(index, branch, row)),
                    Err(reason) => out.unresolved.push(UnresolvedMonitor {
                        statement: statement.clone(),
                        reason,
                    }),
                }
            }
            out.interfaces.push(resolved);
        }
        MonitorStatement::VoltageRange { scope, vmin, vmax } => {
            let Some(bus_rows) = scope_rows(scope, net, subsystems, index) else {
                unresolved_subsystem(statement, out);
                return;
            };
            out.voltage_ranges.push(ResolvedVoltageScope {
                bus_rows,
                low: *vmin,
                high: Some(*vmax),
            });
        }
        MonitorStatement::VoltageDeviation { scope, down, up } => {
            let Some(bus_rows) = scope_rows(scope, net, subsystems, index) else {
                unresolved_subsystem(statement, out);
                return;
            };
            out.voltage_deviations.push(ResolvedVoltageScope {
                bus_rows,
                low: *down,
                high: *up,
            });
        }
    }
}

fn unresolved_subsystem(statement: &MonitorStatement, out: &mut MonitoredResolution) {
    out.unresolved.push(UnresolvedMonitor {
        statement: statement.clone(),
        reason: UnresolvedMonitorReason::NoSuchSubsystem,
    });
}

fn select(subsystems: &SubsystemSet, name: &str, net: &BalancedNetwork) -> Option<BTreeSet<BusId>> {
    Some(subsystems.get(name)?.select_buses(net))
}

/// One bound branch with its orientation against the stored row. A statement
/// naming the stored `to` terminal first states the flow the other way round.
fn member(index: &PsseEquipmentIndex<'_>, branch: &BranchRef, row: usize) -> InterfaceMember {
    InterfaceMember {
        row,
        reversed: index.network().branches()[row].from != branch.from,
    }
}

fn bind_branch(
    index: &PsseEquipmentIndex,
    branch: &BranchRef,
) -> std::result::Result<usize, UnresolvedMonitorReason> {
    let rows = index.branch_rows(branch.from, branch.to, &branch.circuit);
    match rows.as_slice() {
        [] => Err(UnresolvedMonitorReason::NoSuchBranch {
            from: branch.from,
            to: branch.to,
            circuit: branch.circuit.clone(),
        }),
        [row] => Ok(*row),
        many => Err(UnresolvedMonitorReason::AmbiguousBranch {
            from: branch.from,
            to: branch.to,
            circuit: branch.circuit.clone(),
            matches: many.len(),
        }),
    }
}

/// The bus rows one scope names, or `None` when it names a subsystem the
/// subsystem set does not state. A scope naming an area, a zone, an owner, a
/// bus, or a kV level the network does not hold names no row.
fn scope_rows(
    scope: &MonitorScope,
    net: &BalancedNetwork,
    subsystems: &SubsystemSet,
    index: &PsseEquipmentIndex,
) -> Option<BTreeSet<usize>> {
    let rows_of = |buses: &BTreeSet<BusId>| -> BTreeSet<usize> {
        buses.iter().filter_map(|bus| index.bus_row(*bus)).collect()
    };
    let matching = |keep: &dyn Fn(&crate::network::Bus) -> bool| -> BTreeSet<usize> {
        net.buses()
            .iter()
            .enumerate()
            .filter(|(_, bus)| keep(bus))
            .map(|(row, _)| row)
            .collect()
    };
    Some(match scope {
        MonitorScope::AllBuses => (0..net.buses().len()).collect(),
        MonitorScope::Subsystem(name) => rows_of(&select(subsystems, name, net)?),
        MonitorScope::Bus(bus) => index.bus_row(*bus).into_iter().collect(),
        MonitorScope::Area(area) => matching(&|bus| bus.area == *area),
        MonitorScope::Zone(zone) => matching(&|bus| bus.zone == *zone),
        MonitorScope::Owner(owner) => matching(&|bus| super::sub::owner_of(bus) == *owner),
        MonitorScope::Kv(kv) => matching(&|bus| (bus.base_kv - *kv).abs() <= KV_TOLERANCE),
    })
}
