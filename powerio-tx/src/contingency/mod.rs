//! PSS/E contingency description files (`.con`).
//!
//! A `.con` file names the outages a contingency analysis runs. It is free
//! format text: optional header comments, then named cases, automatic
//! specifications that expand against a subsystem, `SKIP` rules that exclude
//! elements from that expansion, and solver-specific lines that only the tool
//! that wrote them interprets.
//!
//! ```text
//! CONTINGENCY 'L_000001ODES'
//! OPEN LINE FROM BUS   1001 TO BUS   1064 CIRCUIT 1
//! END
//! SINGLE BRANCH IN SUBSYSTEM 'WOA'
//! END
//! ```
//!
//! [`ContingencySet::parse`] reads UTF-8 text and touches no filesystem.
//! Statements outside the grammar below keep their line, with surrounding
//! whitespace dropped, and are reported, so a file reads completely rather
//! than failing on the first line a tool wrote for itself. [`ContingencySet::to_con`] writes the set back in
//! one canonical spelling. Only a case that never closes, a case that starts
//! inside another, and a block left open at end of input are errors.
//!
//! The grammar, its evidence, and the writer's spellings are in `FORMAT.md`
//! next to this file. Reading holds what the file states and touches no
//! network. [`ContingencySet::resolve`] is the separate step that binds a set
//! to the elements of a [`crate::network::BalancedNetwork`]; `resolve.rs`
//! holds it.
//!
//! The other two files a contingency analysis reads have their own modules
//! beside this one: `sub.rs` for the subsystem description file
//! ([`SubsystemSet`]) and `mon.rs` for the monitored element file
//! ([`MonitoredSet`]). [`ContingencySet::expand`] turns this file's automatic
//! specifications into explicit cases against a network and a subsystem set;
//! `expand.rs` holds it.

mod expand;
mod lexer;
pub mod mon;
mod resolve;
pub mod sub;

use std::cmp::Ordering;

pub use expand::Expanded;
use lexer::{LexedLine, LineKind, lex};
pub use mon::{
    BranchRef, InterfaceMember, MonitorScope, MonitorStatement, MonitoredParsed,
    MonitoredResolution, MonitoredSet, ResolvedInterface, ResolvedVoltageScope, UnresolvedMonitor,
    UnresolvedMonitorReason,
};
pub use resolve::{
    ContingencyResolution, PsseEquipmentIndex, ResolvedCase, ResolvedComponent, UnresolvedAction,
    UnresolvedReason,
};
pub use sub::{
    JoinName, SelectorGroup, Subsystem, SubsystemParsed, SubsystemSelector, SubsystemSet,
};

use crate::diagnostics::{Diagnostic, DiagnosticInfo, codes};
use crate::network::BusId;
use crate::{Error, Result};

const FMT: &str = "psse contingency";

/// Reader notes are bounded so that a file of unrecognized lines cannot grow
/// the note list without limit.
const MAX_READER_NOTES: usize = 16;

/// Record one reader note, within the note budget the three contingency
/// analysis readers share. A file with exactly the budget of findings gets
/// that many notes and no marker; the first note past the budget is replaced
/// by one marker under the reader's own truncation code, recorded once.
fn note_within_budget(
    diagnostics: &mut Vec<Diagnostic>,
    info: &'static DiagnosticInfo,
    truncated: &'static DiagnosticInfo,
    message: String,
) {
    match diagnostics.len().cmp(&MAX_READER_NOTES) {
        Ordering::Less => diagnostics.push(Diagnostic::of(info, message)),
        Ordering::Equal => {
            diagnostics.push(Diagnostic::of(truncated, "further reader notes suppressed"));
        }
        Ordering::Greater => {}
    }
}

/// One contingency description file.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ContingencySet {
    /// Comment lines ahead of the first statement, as written. PSS/E leads a
    /// generated file with its `/PSS(R)E` stamp and a `COM` banner.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub header: Vec<String>,
    /// Named cases, in file order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cases: Vec<ContingencyCase>,
    /// Specifications that expand into cases against a named subsystem.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub automatic: Vec<AutomaticSpec>,
    /// Branches excluded from automatic expansion.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skips: Vec<SkipRule>,
    /// File level statements outside the grammar, kept as their trimmed line.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retained: Vec<RetainedStatement>,
}

/// One named case: every action applies together.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ContingencyCase {
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<ContingencyAction>,
}

/// One statement inside a case.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ContingencyAction {
    /// A line or two winding transformer, keyed by its terminal buses and
    /// circuit id.
    OpenBranch {
        from: BusId,
        to: BusId,
        circuit: String,
    },
    /// A three winding transformer, keyed by its three buses and circuit id.
    OpenThreeWinding { buses: [BusId; 3], circuit: String },
    /// A machine leaves service; its dispatch leaves the balance.
    RemoveMachine { bus: BusId, id: String },
    /// A machine enters service.
    AddMachine { bus: BusId, id: String },
    /// A fixed shunt leaves service; without an id, every fixed shunt at the
    /// bus.
    RemoveShunt {
        bus: BusId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    /// The switched shunt at the bus leaves service.
    RemoveSwitchedShunt { bus: BusId },
    /// A load leaves service; without an id, every load at the bus.
    RemoveLoad {
        bus: BusId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    /// Every element at the bus leaves service.
    DisconnectBus { bus: BusId },
    /// The bus load moves by the stated amount.
    ChangeLoad { bus: BusId, change: Change },
    /// The bus generation moves by the stated amount.
    ChangeGeneration { bus: BusId, change: Change },
    /// A statement outside the grammar, kept as its trimmed line. A nested
    /// dispatch block keeps all of its trimmed lines, joined with newlines.
    Unrecognized { text: String },
}

/// How much a load or generation statement moves, and in what unit.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Change {
    /// Whether the amount adds to, subtracts from, or replaces the value.
    pub op: ChangeOp,
    /// How far the value moves, in `unit`. Finite: a line stating a non-finite
    /// amount does not read as a change and keeps its text, because no written
    /// form of one reads back.
    pub amount: f64,
    /// The unit the amount is stated in.
    pub unit: ChangeUnit,
}

/// Whether a change adds to, subtracts from, or replaces the present value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ChangeOp {
    Increase,
    Decrease,
    Set,
}

/// The unit a change is stated in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ChangeUnit {
    Mw,
    Percent,
}

/// A specification that expands into one case per element of a subsystem.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AutomaticSpec {
    pub order: AutomaticOrder,
    pub target: AutomaticTarget,
    /// The subsystem the expansion draws elements from.
    pub subsystem: String,
    /// `3WLOWVOLTAGE`: include the low voltage winding of a three winding
    /// transformer in a branch expansion.
    pub low_voltage_3w: bool,
}

/// How many elements an automatic specification outages at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AutomaticOrder {
    Single,
    Double,
}

/// The element family an automatic specification expands over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AutomaticTarget {
    Branch,
    Unit,
    /// Branches crossing the subsystem border.
    Tie,
}

/// One branch an automatic expansion leaves alone.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SkipRule {
    pub from: BusId,
    pub to: BusId,
    pub circuit: String,
}

/// A file level statement outside the grammar, kept as text.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RetainedStatement {
    /// The 1-based line the statement was read from, so the lowest line a
    /// statement can carry is 1.
    #[cfg_attr(feature = "schema", schemars(range(min = 1)))]
    pub line: usize,
    /// The statement's line, with leading and trailing whitespace dropped.
    pub text: String,
    /// Whether the line follows the file level `END`. The writer states these
    /// after the `END` it writes, so reading the written file places them
    /// after the terminator again rather than reading them as grammar.
    #[serde(default)]
    pub after_end: bool,
}

/// Output of a tolerant contingency read: the set plus the reader's notes on
/// statements it kept as text.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ContingencyParsed {
    pub set: ContingencySet,
    /// The reader's notes as structured records.
    pub diagnostics: Vec<Diagnostic>,
}

impl ContingencyParsed {
    fn note(&mut self, info: &'static DiagnosticInfo, message: String) {
        note_within_budget(
            &mut self.diagnostics,
            info,
            &codes::READ_CON_NOTES_TRUNCATED,
            message,
        );
    }

    fn unrecognized(&mut self, number: usize, text: &str) {
        self.note(
            &codes::READ_CON_STATEMENT_UNRECOGNIZED,
            format!("line {number}: statement kept as text: {text}"),
        );
    }

    /// Report a statement naming a value no written line states, which is
    /// therefore kept as text.
    fn unwritable(&mut self, number: usize, text: &str) {
        self.note(
            &codes::READ_CON_SOURCE_MALFORMED,
            format!(
                "line {number}: a value holding both quote characters has no written form, and the statement was kept as text: {text}"
            ),
        );
    }

    /// Report a dispatch block, whose lines are kept as one statement.
    fn dispatch_block(&mut self, number: usize, text: &str) {
        self.note(
            &codes::READ_CON_STATEMENT_UNRECOGNIZED,
            format!("line {number}: dispatch block kept as text: {text}"),
        );
    }
}

fn bad(message: String) -> Error {
    Error::FormatRead {
        format: FMT,
        message,
    }
}

/// A case whose `END` has not been read yet.
struct OpenCase {
    name: String,
    opened: usize,
    actions: Vec<ContingencyAction>,
}

/// A dispatch block whose `END` has not been read yet.
struct OpenBlock {
    opened: usize,
    lines: Vec<String>,
}

impl ContingencySet {
    /// Read a `.con` file from UTF-8 text. Keywords are case insensitive.
    ///
    /// A statement the grammar does not cover keeps its line, with
    /// surrounding whitespace dropped, in
    /// [`ContingencyAction::Unrecognized`] inside a case or in
    /// [`ContingencySet::retained`] at file level, and is reported. A
    /// statement naming a value that no written line states, one holding both
    /// `'` and `"`, is kept the same way. Lines after a file level `END` are
    /// also kept as text, marked [`RetainedStatement::after_end`], and
    /// reported once.
    ///
    /// # Errors
    /// [`Error::FormatRead`] when a `CONTINGENCY` starts before the previous
    /// case reached `END`, or when a case, a `SKIP` block, or a dispatch block
    /// is still open at end of input. The message names the 1-based line.
    pub fn parse(text: &str) -> Result<ContingencyParsed> {
        let mut reader = Reader::new();
        for line in lex(text) {
            reader.read_line(&line)?;
        }
        reader.finish()
    }

    /// Write the set as `.con` text: the header lines as written, the
    /// automatic specifications, one `SKIP` block, the cases in order, the
    /// file level statements kept as text, a final `END`, and then the
    /// statements read after the file `END`. Every line ends with a newline.
    ///
    /// Reading the result back gives the same set, except that a statement
    /// kept from the middle of a file is written after the cases and so reads
    /// back from a different line. Writing that second set gives the same
    /// text.
    #[must_use]
    pub fn to_con(&self) -> String {
        use std::fmt::Write as _;

        let mut out = String::new();
        for line in &self.header {
            out.push_str(line);
            out.push('\n');
        }
        for spec in &self.automatic {
            out.push_str(&write_automatic(spec));
            out.push('\n');
        }
        if !self.skips.is_empty() {
            out.push_str("SKIP\n");
            for rule in &self.skips {
                let circuit = field(&rule.circuit);
                let _ = writeln!(
                    out,
                    "{:>6} TO {:>6} CIRCUIT {circuit}",
                    rule.from.0, rule.to.0
                );
            }
            out.push_str("END\n");
        }
        for case in &self.cases {
            let _ = writeln!(out, "CONTINGENCY {}", quoted(&case.name));
            for action in &case.actions {
                out.push_str(&write_action(action));
                out.push('\n');
            }
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
}

/// The reader's state while it walks the lines of one file.
struct Reader {
    parsed: ContingencyParsed,
    case: Option<OpenCase>,
    block: Option<OpenBlock>,
    skip_opened: Option<usize>,
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
            parsed: ContingencyParsed {
                set: ContingencySet::default(),
                diagnostics: Vec::new(),
            },
            case: None,
            block: None,
            skip_opened: None,
            seen_statement: false,
            ended: false,
            noted_text_after_end: false,
        }
    }

    fn read_line(&mut self, line: &LexedLine<'_>) -> Result<()> {
        if self.block.is_some() {
            self.read_block_line(line);
            return Ok(());
        }
        if self.ended {
            self.keep_after_end(line);
            return Ok(());
        }
        if !self.take_header(line) {
            return Ok(());
        }
        let upper = line.keywords();
        let words = line.words();
        if self.skip_opened.is_some() {
            self.read_skip_line(line, &upper, &words);
            return Ok(());
        }
        if self.case.is_some() {
            return self.read_case_line(line, &upper, &words);
        }
        self.read_file_line(line, &upper, &words);
        Ok(())
    }

    /// The set and its notes, once every line has been read.
    fn finish(self) -> Result<ContingencyParsed> {
        if let Some(open) = self.block {
            return Err(bad(format!(
                "line {}: a dispatch block has no END",
                open.opened
            )));
        }
        if let Some(open) = self.case {
            return Err(bad(format!(
                "line {}: CONTINGENCY '{}' has no END",
                open.opened, open.name
            )));
        }
        if let Some(opened) = self.skip_opened {
            return Err(bad(format!("line {opened}: SKIP has no END")));
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

    /// Add one line to the open dispatch block, and close the block on its
    /// `END`. The text holds every line of the block, including that `END`, so
    /// writing it and reading it again gives the same block.
    fn read_block_line(&mut self, line: &LexedLine<'_>) {
        let closed = is_end(line);
        let text = line.trimmed().to_owned();
        if let Some(open) = self.block.as_mut() {
            open.lines.push(text);
        }
        if !closed {
            return;
        }
        let Some(open) = self.block.take() else {
            return;
        };
        let text = open.lines.join("\n");
        match self.case.as_mut() {
            Some(case) => case.actions.push(ContingencyAction::Unrecognized { text }),
            None => self.parsed.set.retained.push(RetainedStatement {
                line: open.opened,
                text,
                after_end: false,
            }),
        }
    }

    /// Keep a statement line that follows the file level `END`, and report the
    /// first one. A blank or comment line there states nothing and is dropped,
    /// as one before the `END` is, so the written file reads back the same.
    fn keep_after_end(&mut self, line: &LexedLine<'_>) {
        if line.kind != LineKind::Statement {
            return;
        }
        if !self.noted_text_after_end {
            self.noted_text_after_end = true;
            self.parsed.note(
                &codes::READ_CON_TEXT_AFTER_END,
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

    fn read_skip_line(&mut self, line: &LexedLine<'_>, upper: &[String], words: &[&str]) {
        if is_end(line) {
            self.skip_opened = None;
            return;
        }
        match parse_skip_rule(upper, words) {
            Some(rule) if writable(&rule.circuit) => {
                self.parsed.set.skips.push(rule);
                return;
            }
            Some(_) => self.parsed.unwritable(line.number, line.trimmed()),
            None => self.parsed.note(
                &codes::READ_CON_SOURCE_MALFORMED,
                format!(
                    "line {}: a SKIP line states no branch and was kept as text: {}",
                    line.number,
                    line.trimmed()
                ),
            ),
        }
        self.keep_statement(line, false);
    }

    fn read_case_line(
        &mut self,
        line: &LexedLine<'_>,
        upper: &[String],
        words: &[&str],
    ) -> Result<()> {
        if is_end(line) {
            if let Some(open) = self.case.take() {
                self.parsed.set.cases.push(ContingencyCase {
                    name: open.name,
                    actions: open.actions,
                });
            }
            return Ok(());
        }
        if upper[0] == "CONTINGENCY" {
            let name = self.case.as_ref().map_or("", |open| open.name.as_str());
            return Err(bad(format!(
                "line {}: CONTINGENCY starts before case '{name}' reached END",
                line.number
            )));
        }
        if opens_block(upper) {
            self.open_block(line);
            return Ok(());
        }
        let action = match parse_action(upper, words) {
            Some(action) if action_writable(&action) => action,
            recognized => {
                if recognized.is_some() {
                    self.parsed.unwritable(line.number, line.trimmed());
                } else {
                    self.parsed.unrecognized(line.number, line.trimmed());
                }
                ContingencyAction::Unrecognized {
                    text: line.trimmed().to_owned(),
                }
            }
        };
        if let Some(open) = self.case.as_mut() {
            open.actions.push(action);
        }
        Ok(())
    }

    fn read_file_line(&mut self, line: &LexedLine<'_>, upper: &[String], words: &[&str]) {
        match upper[0].as_str() {
            "CONTINGENCY" => self.open_case(line),
            "END" if upper.len() == 1 => self.ended = true,
            "SKIP" if upper.len() == 1 => self.skip_opened = Some(line.number),
            _ if opens_block(upper) => self.open_block(line),
            _ => match parse_automatic(upper, words) {
                Some(spec) if writable(&spec.subsystem) => self.parsed.set.automatic.push(spec),
                recognized => {
                    if recognized.is_some() {
                        self.parsed.unwritable(line.number, line.trimmed());
                    } else {
                        self.parsed.unrecognized(line.number, line.trimmed());
                    }
                    self.keep_statement(line, false);
                }
            },
        }
    }

    /// Open a case named by the first token after the `CONTINGENCY` keyword.
    ///
    /// A further token is reported and dropped: a line that opened no case
    /// would leave that case's `END` to terminate the file and every statement
    /// after it to read as text. A name no written line states keeps the whole
    /// line as text instead, because a case the writer cannot name back does
    /// not hold the writer's fixed point.
    fn open_case(&mut self, line: &LexedLine<'_>) {
        let name = case_name_of(line);
        if !writable(&name) {
            self.parsed.unwritable(line.number, line.trimmed());
            self.keep_statement(line, false);
            return;
        }
        if line.tokens.len() > 2 {
            let extra = line.words()[2..].join(" ");
            self.parsed.note(
                &codes::READ_CON_SOURCE_MALFORMED,
                format!(
                    "line {}: CONTINGENCY states more than a case name, and the tokens after it are not kept: {extra}",
                    line.number
                ),
            );
        }
        self.case = Some(OpenCase {
            name,
            opened: line.number,
            actions: Vec::new(),
        });
    }

    fn open_block(&mut self, line: &LexedLine<'_>) {
        self.parsed.dispatch_block(line.number, line.trimmed());
        self.block = Some(OpenBlock {
            opened: line.number,
            lines: vec![line.trimmed().to_owned()],
        });
    }
}

/// Whether the line is a bare `END`, which closes a case, a `SKIP`, or a
/// dispatch block.
fn is_end(line: &LexedLine<'_>) -> bool {
    line.kind == LineKind::Statement
        && line.tokens.len() == 1
        && line.tokens[0].text.eq_ignore_ascii_case("END")
}

/// Whether the line opens a nested block that runs to the next `END`. A line
/// ending in `DISPATCH` opens one, and so does a `DEFAULT DISPATCH` line with
/// a direction or level after it (`DEFAULT DISPATCH DOWN`,
/// `DEFAULT DISPATCH FIRSTLEVEL`).
fn opens_block(upper: &[String]) -> bool {
    if upper.last().is_some_and(|word| word == "DISPATCH") {
        return true;
    }
    upper.first().is_some_and(|word| word == "DEFAULT")
        && upper.get(1).is_some_and(|word| word == "DISPATCH")
}

/// The case name on a `CONTINGENCY` line: the token after the keyword,
/// trimmed. The tokenizer's quote rule keeps a name holding an apostrophe
/// whole, so `CONTINGENCY 'L_000022O'~1'` names `L_000022O'~1`, and a trailing
/// comment stays out of the name. A line stating no name names nothing, and a
/// line stating more than one token names the first.
fn case_name_of(line: &LexedLine<'_>) -> String {
    line.tokens
        .get(1)
        .map_or_else(String::new, |token| token.text.trim().to_owned())
}

// ---------------------------------------------------------------------------
// Statement grammar
// ---------------------------------------------------------------------------

fn parse_bus(token: &str) -> Option<BusId> {
    token.parse::<usize>().ok().map(BusId)
}

/// Read `BUS i` or a bare `i` at `at`, advancing past it.
fn take_bus(upper: &[String], at: &mut usize) -> Option<BusId> {
    if upper.get(*at).is_some_and(|word| word == "BUS") {
        *at += 1;
    }
    let bus = parse_bus(upper.get(*at)?)?;
    *at += 1;
    Some(bus)
}

fn take_keyword(upper: &[String], at: &mut usize, keyword: &str) -> Option<()> {
    if upper.get(*at)? != keyword {
        return None;
    }
    *at += 1;
    Some(())
}

/// Read an optional `CIRCUIT c` tail. An absent tail means circuit `1`; a
/// trailing token that is not a circuit tail rejects the line.
fn take_circuit(upper: &[String], words: &[&str], at: &mut usize) -> Option<String> {
    if *at == upper.len() {
        return Some("1".to_owned());
    }
    if !matches!(
        upper[*at].as_str(),
        "CIRCUIT" | "CKT" | "CIRCUITS" | "CIRCUT"
    ) {
        return None;
    }
    let value = words.get(*at + 1)?.trim();
    *at += 2;
    if *at != upper.len() {
        return None;
    }
    Some(if value.is_empty() {
        "1".to_owned()
    } else {
        value.to_owned()
    })
}

/// Read an element id, which may be quoted and may carry padding.
fn take_id(words: &[&str], at: &mut usize) -> Option<String> {
    let id = words.get(*at)?.trim();
    *at += 1;
    Some(id.to_owned())
}

fn parse_action(upper: &[String], words: &[&str]) -> Option<ContingencyAction> {
    let verb = upper.first()?.as_str();
    let noun = upper.get(1).map_or("", String::as_str);
    match (verb, noun) {
        ("OPEN" | "TRIP" | "DISCONNECT", "LINE" | "BRANCH") => parse_open_branch(upper, words),
        ("OPEN" | "TRIP" | "DISCONNECT", "THREEWINDING") => parse_three_winding(upper, words),
        ("DISCONNECT", "BUS") => {
            let mut at = 1;
            let bus = take_bus(upper, &mut at)?;
            (at == upper.len()).then_some(ContingencyAction::DisconnectBus { bus })
        }
        ("REMOVE" | "TRIP", "MACHINE" | "UNIT") => {
            let (bus, id) = parse_id_from_bus(upper, words)?;
            Some(ContingencyAction::RemoveMachine { bus, id })
        }
        ("ADD", "MACHINE" | "UNIT") => {
            let mut at = 2;
            let id = take_id(words, &mut at)?;
            take_keyword(upper, &mut at, "TO")?;
            let bus = take_bus(upper, &mut at)?;
            (at == upper.len()).then_some(ContingencyAction::AddMachine { bus, id })
        }
        ("REMOVE" | "TRIP", "SHUNT") => {
            let (bus, id) = parse_optional_id_from_bus(upper, words)?;
            Some(ContingencyAction::RemoveShunt { bus, id })
        }
        ("REMOVE" | "TRIP", "LOAD") => {
            let (bus, id) = parse_optional_id_from_bus(upper, words)?;
            Some(ContingencyAction::RemoveLoad { bus, id })
        }
        ("REMOVE" | "TRIP", "SWSHUNT") => {
            let mut at = 2;
            take_keyword(upper, &mut at, "FROM")?;
            let bus = take_bus(upper, &mut at)?;
            (at == upper.len()).then_some(ContingencyAction::RemoveSwitchedShunt { bus })
        }
        ("INCREASE" | "RAISE" | "DECREASE" | "SET", _) => parse_change(upper),
        _ => None,
    }
}

/// `OPEN LINE FROM BUS i TO BUS j [CIRCUIT c]`, or the three winding spelling
/// that states a third bus in place of the circuit tail.
fn parse_open_branch(upper: &[String], words: &[&str]) -> Option<ContingencyAction> {
    let mut at = 2;
    take_keyword(upper, &mut at, "FROM")?;
    let from = take_bus(upper, &mut at)?;
    take_keyword(upper, &mut at, "TO")?;
    let to = take_bus(upper, &mut at)?;
    if upper.get(at).is_some_and(|word| word == "TO") {
        at += 1;
        let third = take_bus(upper, &mut at)?;
        let circuit = take_circuit(upper, words, &mut at)?;
        return Some(ContingencyAction::OpenThreeWinding {
            buses: [from, to, third],
            circuit,
        });
    }
    let circuit = take_circuit(upper, words, &mut at)?;
    Some(ContingencyAction::OpenBranch { from, to, circuit })
}

/// `OPEN THREEWINDING AT BUS a TO BUS b TO BUS c [CIRCUIT c]`.
fn parse_three_winding(upper: &[String], words: &[&str]) -> Option<ContingencyAction> {
    let mut at = 2;
    take_keyword(upper, &mut at, "AT")?;
    let first = take_bus(upper, &mut at)?;
    take_keyword(upper, &mut at, "TO")?;
    let second = take_bus(upper, &mut at)?;
    take_keyword(upper, &mut at, "TO")?;
    let third = take_bus(upper, &mut at)?;
    let circuit = take_circuit(upper, words, &mut at)?;
    Some(ContingencyAction::OpenThreeWinding {
        buses: [first, second, third],
        circuit,
    })
}

/// `REMOVE MACHINE id FROM BUS i`.
fn parse_id_from_bus(upper: &[String], words: &[&str]) -> Option<(BusId, String)> {
    let mut at = 2;
    let id = take_id(words, &mut at)?;
    take_keyword(upper, &mut at, "FROM")?;
    let bus = take_bus(upper, &mut at)?;
    (at == upper.len()).then_some((bus, id))
}

/// `REMOVE SHUNT [id] FROM BUS i`.
fn parse_optional_id_from_bus(upper: &[String], words: &[&str]) -> Option<(BusId, Option<String>)> {
    let mut at = 2;
    let id = if upper.get(at).is_some_and(|word| word == "FROM") {
        None
    } else {
        Some(take_id(words, &mut at)?)
    };
    take_keyword(upper, &mut at, "FROM")?;
    let bus = take_bus(upper, &mut at)?;
    (at == upper.len()).then_some((bus, id))
}

/// `INCREASE BUS i LOAD BY x PERCENT` and its synonyms, and the `SET ... TO`
/// spelling.
fn parse_change(upper: &[String]) -> Option<ContingencyAction> {
    let op = match upper[0].as_str() {
        "INCREASE" | "RAISE" => ChangeOp::Increase,
        "DECREASE" => ChangeOp::Decrease,
        "SET" => ChangeOp::Set,
        _ => return None,
    };
    let mut at = 1;
    let bus = take_bus(upper, &mut at)?;
    let load = match upper.get(at)?.as_str() {
        "LOAD" => true,
        "GENERATION" => false,
        _ => return None,
    };
    at += 1;
    take_keyword(
        upper,
        &mut at,
        if op == ChangeOp::Set { "TO" } else { "BY" },
    )?;
    let stated = upper.get(at)?.as_str();
    at += 1;
    let (amount, unit) = if let Some(head) = stated.strip_suffix('%') {
        (head.parse::<f64>().ok()?, ChangeUnit::Percent)
    } else {
        let amount = stated.parse::<f64>().ok()?;
        let unit = match upper.get(at)?.as_str() {
            "MW" => ChangeUnit::Mw,
            "PERCENT" | "%" => ChangeUnit::Percent,
            _ => return None,
        };
        at += 1;
        (amount, unit)
    };
    if at != upper.len() || !amount.is_finite() {
        return None;
    }
    let change = Change { op, amount, unit };
    Some(if load {
        ContingencyAction::ChangeLoad { bus, change }
    } else {
        ContingencyAction::ChangeGeneration { bus, change }
    })
}

/// `SINGLE BRANCH IN SUBSYSTEM name [3WLOWVOLTAGE]`.
fn parse_automatic(upper: &[String], words: &[&str]) -> Option<AutomaticSpec> {
    let order = match upper.first()?.as_str() {
        "SINGLE" => AutomaticOrder::Single,
        "DOUBLE" => AutomaticOrder::Double,
        _ => return None,
    };
    let target = match upper.get(1)?.as_str() {
        "BRANCH" | "LINE" => AutomaticTarget::Branch,
        "UNIT" | "MACHINE" => AutomaticTarget::Unit,
        "TIE" => AutomaticTarget::Tie,
        _ => return None,
    };
    let mut at = 2;
    if !matches!(upper.get(at)?.as_str(), "IN" | "FROM") {
        return None;
    }
    at += 1;
    take_keyword(upper, &mut at, "SUBSYSTEM")?;
    let subsystem = words.get(at)?.trim().to_owned();
    at += 1;
    let low_voltage_3w = upper.get(at).is_some_and(|word| word == "3WLOWVOLTAGE");
    if low_voltage_3w {
        at += 1;
    }
    (at == upper.len()).then_some(AutomaticSpec {
        order,
        target,
        subsystem,
        low_voltage_3w,
    })
}

/// `i TO j [CIRCUIT c]` or `FROM BUS i TO BUS j [CIRCUIT c]` inside a `SKIP`
/// block.
fn parse_skip_rule(upper: &[String], words: &[&str]) -> Option<SkipRule> {
    let mut at = 0;
    if upper.first().is_some_and(|word| word == "FROM") {
        at = 1;
    }
    let from = take_bus(upper, &mut at)?;
    take_keyword(upper, &mut at, "TO")?;
    let to = take_bus(upper, &mut at)?;
    let circuit = take_circuit(upper, words, &mut at)?;
    Some(SkipRule { from, to, circuit })
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

/// The quote character a written line closes `value` with: `"` when the value
/// holds an apostrophe, `'` otherwise. A value holding both has none, because
/// a quoted token ends at the first quote of its own character that whitespace
/// or the end of the line follows.
fn delimiter(value: &str) -> Option<char> {
    match (value.contains('\''), value.contains('"')) {
        (true, true) => None,
        (true, false) => Some('"'),
        (false, _) => Some('\''),
    }
}

/// Whether a written line states `value` as one token that reads back
/// unchanged. The reader keeps a statement naming a value without one as
/// text, so a set read from a file names only values that have one.
fn writable(value: &str) -> bool {
    delimiter(value).is_some()
}

/// A name as written: quoted, as PSS/E quotes a case name and a subsystem
/// name, with the delimiter the value does not hold. A value holding both
/// quote characters reaches the writer only from a set built in memory, and is
/// stated single quoted.
fn quoted(value: &str) -> String {
    let quote = delimiter(value).unwrap_or('\'');
    format!("{quote}{value}{quote}")
}

/// A field as written. A value that is empty, holds whitespace, opens with
/// `/`, or holds a quote character is quoted, so the line states it as one
/// token: an unquoted token opening with `/` would end the statement and leave
/// the rest of the line a comment. Every other value is written bare, as
/// PSS/E writes an id and a circuit.
pub(crate) fn field(value: &str) -> String {
    if value.is_empty()
        || value.contains(char::is_whitespace)
        || value.starts_with('/')
        || value.contains(['\'', '"'])
    {
        quoted(value)
    } else {
        value.to_owned()
    }
}

/// Whether every value the action states has a written form.
fn action_writable(action: &ContingencyAction) -> bool {
    match action {
        ContingencyAction::OpenBranch { circuit, .. }
        | ContingencyAction::OpenThreeWinding { circuit, .. } => writable(circuit),
        ContingencyAction::RemoveMachine { id, .. } | ContingencyAction::AddMachine { id, .. } => {
            writable(id)
        }
        ContingencyAction::RemoveShunt { id, .. } | ContingencyAction::RemoveLoad { id, .. } => {
            id.as_deref().is_none_or(writable)
        }
        _ => true,
    }
}

fn write_action(action: &ContingencyAction) -> String {
    match action {
        ContingencyAction::OpenBranch { from, to, circuit } => format!(
            "OPEN LINE FROM BUS {:>6} TO BUS {:>6} CIRCUIT {}",
            from.0,
            to.0,
            field(circuit)
        ),
        ContingencyAction::OpenThreeWinding { buses, circuit } => format!(
            "OPEN THREEWINDING AT BUS {:>6} TO BUS {:>6} TO BUS {:>6} CIRCUIT {}",
            buses[0].0,
            buses[1].0,
            buses[2].0,
            field(circuit)
        ),
        ContingencyAction::RemoveMachine { bus, id } => {
            format!("REMOVE MACHINE {} FROM BUS {:>6}", field(id), bus.0)
        }
        ContingencyAction::AddMachine { bus, id } => {
            format!("ADD MACHINE {} TO BUS {:>6}", field(id), bus.0)
        }
        ContingencyAction::RemoveShunt { bus, id } => match id {
            Some(id) => format!("REMOVE SHUNT {} FROM BUS {:>6}", field(id), bus.0),
            None => format!("REMOVE SHUNT FROM BUS {:>6}", bus.0),
        },
        ContingencyAction::RemoveSwitchedShunt { bus } => {
            format!("REMOVE SWSHUNT FROM BUS {:>6}", bus.0)
        }
        ContingencyAction::RemoveLoad { bus, id } => match id {
            Some(id) => format!("REMOVE LOAD {} FROM BUS {:>6}", field(id), bus.0),
            None => format!("REMOVE LOAD FROM BUS {:>6}", bus.0),
        },
        ContingencyAction::DisconnectBus { bus } => format!("DISCONNECT BUS {:>6}", bus.0),
        ContingencyAction::ChangeLoad { bus, change } => write_change(*bus, "LOAD", change),
        ContingencyAction::ChangeGeneration { bus, change } => {
            write_change(*bus, "GENERATION", change)
        }
        ContingencyAction::Unrecognized { text } => text.clone(),
    }
}

fn write_change(bus: BusId, target: &str, change: &Change) -> String {
    let unit = match change.unit {
        ChangeUnit::Mw => "MW",
        ChangeUnit::Percent => "PERCENT",
    };
    let amount = change.amount;
    let bus = bus.0;
    match change.op {
        ChangeOp::Increase => format!("INCREASE BUS {bus} {target} BY {amount} {unit}"),
        ChangeOp::Decrease => format!("DECREASE BUS {bus} {target} BY {amount} {unit}"),
        ChangeOp::Set => format!("SET BUS {bus} {target} TO {amount} {unit}"),
    }
}

fn write_automatic(spec: &AutomaticSpec) -> String {
    let order = match spec.order {
        AutomaticOrder::Single => "SINGLE",
        AutomaticOrder::Double => "DOUBLE",
    };
    let (target, preposition) = match spec.target {
        AutomaticTarget::Branch => ("BRANCH", "IN"),
        AutomaticTarget::Unit => ("UNIT", "IN"),
        AutomaticTarget::Tie => ("TIE", "FROM"),
    };
    let tail = if spec.low_voltage_3w {
        " 3WLOWVOLTAGE"
    } else {
        ""
    };
    format!(
        "{order} {target} {preposition} SUBSYSTEM {}{tail}",
        quoted(&spec.subsystem)
    )
}
