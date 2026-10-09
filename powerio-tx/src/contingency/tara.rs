//! The TARA statements a `.con` or `.sub` file states beyond the PSS/E
//! grammar, read at analysis time.
//!
//! The readers keep these statements as text: a statement naming a bus by
//! name stays an [`ContingencyAction::Unrecognized`] action, a dispatch block
//! stays one text action or one retained statement, and a subsystem's dispatch
//! lines stay retained on the subsystem. The PowerIO IR layouts of the three
//! sets are fixed within 0.11, so the typed forms here are views over that
//! text rather than fields of the sets:
//!
//! - **Names.** A quoted token after `BUS` names a bus by its name and base kV
//!   (`'NAME 345'`), the form TARA reads after `BUSNAMES`.
//!   [`ContingencySet::resolve`] binds such a statement by name, and a
//!   statement naming one quoted branch, the form read after `BRANCHNAMES`,
//!   by the branch's name.
//! - **Dispatch.** [`ContingencySet::calc_default_dispatch`] reads the
//!   `DEFAULT DISPATCH [UP | DOWN | FIRSTLEVEL]` blocks, and
//!   [`ContingencyAction::calc_dispatched_action`] a case action that closes
//!   with `DISPATCH` and the block of lines under it.
//! - **Subsystem rules.** [`Subsystem::calc_dispatch_rules`] reads `PARTICIPATE`,
//!   `SCALE`, `BASELOAD`, `TURBINETYPE`, and `EXCEPT` lines.
//!
//! A name matches exactly first, then without regard to letter case and runs
//! of whitespace; a name that matches several elements at either stage is
//! ambiguous and binds none.

use std::collections::HashMap;

use crate::network::{BalancedNetwork, BusId};

use super::lexer::{LexedLine, LineKind, lex};
use super::resolve::UnresolvedReason;
use super::sub::Subsystem;
use super::{ContingencyAction, ContingencySet, parse_action};

/// Base kV values this far apart name the same voltage level.
const KV_TOLERANCE: f64 = 1e-6;

/// Which `DEFAULT DISPATCH` block a set states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DispatchLevel {
    /// `DEFAULT DISPATCH` with no qualifier.
    Default,
    /// `DEFAULT DISPATCH UP`.
    Up,
    /// `DEFAULT DISPATCH DOWN`.
    Down,
    /// `DEFAULT DISPATCH FIRSTLEVEL`.
    FirstLevel,
}

/// One line of a dispatch block.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum DispatchEntry {
    /// `SUBSYSTEM name [x]`: a subsystem, with the number the line states
    /// after it, when it states one.
    Subsystem { name: String, share: Option<f64> },
    /// `BUS n [x]`: one bus, with the number the line states after it.
    Bus { bus: BusId, share: Option<f64> },
    /// `PARTICIPATING MACHINES`.
    ParticipatingMachines,
}

/// The lines of one dispatch block, between its opening line and its `END`.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct DispatchBlock {
    pub entries: Vec<DispatchEntry>,
    /// Lines outside the entry grammar, trimmed, in block order.
    pub unread: Vec<String>,
}

/// A `DEFAULT DISPATCH` block of a contingency set.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct DefaultDispatch {
    pub level: DispatchLevel,
    pub block: DispatchBlock,
    /// The line the block opened on.
    pub line: usize,
}

/// A case action that closes with `DISPATCH`, and the block it states.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct DispatchedAction {
    /// The action its opening line states ahead of `DISPATCH`.
    pub action: ContingencyAction,
    pub block: DispatchBlock,
}

/// What a `SCALE ALL` line scales.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScaledQuantity {
    /// `SCALE ALL GENERATION`.
    Generation,
    /// `SCALE ALL LOAD`.
    Load,
    /// `SCALE ALL FOR IMPORT`.
    Import,
    /// `SCALE ALL FOR EXPORT`.
    Export,
}

/// One TARA dispatch line inside a subsystem.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum SubsystemDispatchRule {
    /// `PARTICIPATE [INCLUDE OFFLINE]`.
    Participate { include_offline: bool },
    /// `SCALE ALL GENERATION | LOAD | FOR IMPORT | FOR EXPORT`, with
    /// `WITH PMAX GREATER x MW`, `INCLUDE OFFLINE`, and
    /// `INCLUDE NONCONFORMING` when stated.
    Scale {
        quantity: ScaledQuantity,
        pmax_greater_mw: Option<f64>,
        include_offline: bool,
        include_nonconforming: bool,
    },
    /// `BASELOAD n`.
    Baseload { code: i64 },
    /// `TURBINETYPE n`.
    TurbineType { code: i64 },
    /// `EXCEPT` and the words that follow it, as written.
    Except { words: Vec<String> },
}

impl ContingencySet {
    /// The `DEFAULT DISPATCH` blocks the set states at file level, in file
    /// order. The reader keeps each block as one retained statement holding
    /// every line; this reads its level and entries.
    #[must_use]
    pub fn calc_default_dispatch(&self) -> Vec<DefaultDispatch> {
        self.retained
            .iter()
            .filter_map(|statement| {
                let lines = lex(&statement.text);
                let (head, body) = split_block(&lines)?;
                let upper: Vec<String> = head.keywords();
                if upper.len() > 3
                    || upper[0] != "DEFAULT"
                    || upper.get(1).map(String::as_str) != Some("DISPATCH")
                {
                    return None;
                }
                let level = match upper.get(2).map(String::as_str) {
                    None => DispatchLevel::Default,
                    Some("UP") => DispatchLevel::Up,
                    Some("DOWN") => DispatchLevel::Down,
                    Some("FIRSTLEVEL") => DispatchLevel::FirstLevel,
                    Some(_) => return None,
                };
                Some(DefaultDispatch {
                    level,
                    block: read_block(body),
                    line: statement.line,
                })
            })
            .collect()
    }
}

impl ContingencyAction {
    /// The action and dispatch block of a case action that closes with
    /// `DISPATCH`, which the reader keeps as one text action holding every
    /// line of the block. `None` for any other action, and for a block whose
    /// opening line names a bus by name: that one binds through
    /// [`ContingencySet::resolve`], which can look the name up.
    #[must_use]
    pub fn calc_dispatched_action(&self) -> Option<DispatchedAction> {
        let Self::Unrecognized { text } = self else {
            return None;
        };
        let lines = lex(text);
        let (head, body) = split_block(&lines)?;
        let upper = head.keywords();
        if upper.last().map(String::as_str) != Some("DISPATCH") {
            return None;
        }
        let words = head.words();
        let n = upper.len() - 1;
        let action = parse_action(&upper[..n], &words[..n])?;
        Some(DispatchedAction {
            action,
            block: read_block(body),
        })
    }
}

impl Subsystem {
    /// The TARA dispatch lines the subsystem states, in the order the
    /// subsystem keeps them: its own retained lines, then each `JOIN` group's.
    /// A line outside this grammar is not listed and stays text only.
    #[must_use]
    pub fn calc_dispatch_rules(&self) -> Vec<SubsystemDispatchRule> {
        self.retained
            .iter()
            .chain(self.groups.iter().flat_map(|group| group.retained.iter()))
            .filter_map(|statement| {
                let lines = lex(&statement.text);
                let line = lines.iter().find(|l| l.kind == LineKind::Statement)?;
                parse_subsystem_rule(&line.keywords(), &line.words())
            })
            .collect()
    }
}

/// The opening statement line of a block and the statement lines under it,
/// up to and excluding its `END`.
fn split_block<'a, 'b>(
    lines: &'b [LexedLine<'a>],
) -> Option<(&'b LexedLine<'a>, &'b [LexedLine<'a>])> {
    let statements: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.kind == LineKind::Statement)
        .map(|(index, _)| index)
        .collect();
    let (&first, rest) = statements.split_first()?;
    let last = rest.last().copied().filter(|&index| {
        let tokens = &lines[index].tokens;
        tokens.len() == 1 && tokens[0].text.eq_ignore_ascii_case("END")
    });
    let end = last.unwrap_or(lines.len());
    Some((&lines[first], &lines[first + 1..end]))
}

fn read_block(lines: &[LexedLine<'_>]) -> DispatchBlock {
    let mut block = DispatchBlock::default();
    for line in lines.iter().filter(|l| l.kind == LineKind::Statement) {
        let upper = line.keywords();
        let words = line.words();
        let share = |at: usize| -> Option<Option<f64>> {
            match words.get(at) {
                None => Some(None),
                Some(word) => word.parse::<f64>().ok().filter(|v| v.is_finite()).map(Some),
            }
        };
        let entry = match upper[0].as_str() {
            "SUBSYSTEM" | "SYSTEM" if words.len() <= 3 => {
                words
                    .get(1)
                    .zip(share(2))
                    .map(|(name, share)| DispatchEntry::Subsystem {
                        name: name.trim().to_owned(),
                        share,
                    })
            }
            "BUS" if words.len() <= 3 => words
                .get(1)
                .and_then(|bus| bus.parse::<usize>().ok())
                .zip(share(2))
                .map(|(bus, share)| DispatchEntry::Bus {
                    bus: BusId(bus),
                    share,
                }),
            "PARTICIPATING" if upper.len() == 2 && upper[1] == "MACHINES" => {
                Some(DispatchEntry::ParticipatingMachines)
            }
            _ => None,
        };
        match entry {
            Some(entry) => block.entries.push(entry),
            None => block.unread.push(line.trimmed().to_owned()),
        }
    }
    block
}

fn parse_subsystem_rule(upper: &[String], words: &[&str]) -> Option<SubsystemDispatchRule> {
    let include = |at: usize, what: &str| {
        upper.get(at).is_some_and(|w| w == "INCLUDE")
            && upper.get(at + 1).is_some_and(|w| w == what)
    };
    match upper.first()?.as_str() {
        "PARTICIPATE" => match upper.len() {
            1 => Some(SubsystemDispatchRule::Participate {
                include_offline: false,
            }),
            3 if include(1, "OFFLINE") => Some(SubsystemDispatchRule::Participate {
                include_offline: true,
            }),
            _ => None,
        },
        "BASELOAD" | "TURBINETYPE" if upper.len() == 2 => {
            let code = upper[1].parse::<i64>().ok()?;
            Some(if upper[0] == "BASELOAD" {
                SubsystemDispatchRule::Baseload { code }
            } else {
                SubsystemDispatchRule::TurbineType { code }
            })
        }
        "EXCEPT" => Some(SubsystemDispatchRule::Except {
            words: words[1..].iter().map(|w| (*w).to_owned()).collect(),
        }),
        "SCALE" => parse_scale(upper),
        _ => None,
    }
}

/// `SCALE ALL GENERATION [WITH PMAX GREATER x MW]`,
/// `SCALE ALL LOAD [INCLUDE NONCONFORMING]`, and
/// `SCALE ALL FOR IMPORT | EXPORT [WITH PMAX GREATER x MW] [INCLUDE OFFLINE]`.
fn parse_scale(upper: &[String]) -> Option<SubsystemDispatchRule> {
    if upper.get(1)? != "ALL" {
        return None;
    }
    let (quantity, mut at) = match upper.get(2)?.as_str() {
        "GENERATION" => (ScaledQuantity::Generation, 3),
        "LOAD" | "LOAD_ID" => (ScaledQuantity::Load, 3),
        "FOR" => match upper.get(3)?.as_str() {
            "IMPORT" => (ScaledQuantity::Import, 4),
            "EXPORT" => (ScaledQuantity::Export, 4),
            _ => return None,
        },
        _ => return None,
    };
    let mut pmax_greater_mw = None;
    let mut include_offline = false;
    let mut include_nonconforming = false;
    while at < upper.len() {
        match upper[at].as_str() {
            "WITH"
                if upper.get(at + 1).is_some_and(|w| w == "PMAX")
                    && upper.get(at + 2).is_some_and(|w| w == "GREATER")
                    && upper.get(at + 4).is_some_and(|w| w == "MW") =>
            {
                pmax_greater_mw = Some(upper.get(at + 3)?.parse::<f64>().ok()?);
                at += 5;
            }
            "INCLUDE" if upper.get(at + 1).is_some_and(|w| w == "OFFLINE") => {
                include_offline = true;
                at += 2;
            }
            "INCLUDE" if upper.get(at + 1).is_some_and(|w| w == "NONCONFORMING") => {
                include_nonconforming = true;
                at += 2;
            }
            _ => return None,
        }
    }
    Some(SubsystemDispatchRule::Scale {
        quantity,
        pmax_greater_mw,
        include_offline,
        include_nonconforming,
    })
}

// ---------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------

/// A name compared without regard to letter case and runs of whitespace.
fn normalize_name(name: &str) -> String {
    name.split_whitespace()
        .map(str::to_ascii_uppercase)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The buses and branches of a network by name, for the TARA name forms.
#[derive(Debug, Clone, Default)]
pub(super) struct NameIndex {
    /// Normalized bus name to `(name as stated, base kV, bus)`.
    buses: HashMap<String, Vec<(String, f64, BusId)>>,
    /// Normalized branch name to `(name as stated, branch row)`.
    branches: HashMap<String, Vec<(String, usize)>>,
}

impl NameIndex {
    pub(super) fn new(net: &BalancedNetwork) -> Self {
        let mut index = Self::default();
        for bus in net.buses() {
            if let Some(name) = bus.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
                index.buses.entry(normalize_name(name)).or_default().push((
                    name.to_owned(),
                    bus.base_kv,
                    bus.id,
                ));
            }
        }
        for (row, branch) in net.branches().iter().enumerate() {
            if let Some(name) = branch
                .name
                .as_deref()
                .map(str::trim)
                .filter(|n| !n.is_empty())
            {
                index
                    .branches
                    .entry(normalize_name(name))
                    .or_default()
                    .push((name.to_owned(), row));
            }
        }
        index
    }

    /// The bus a quoted `NAME [kV]` token names. The last whitespace
    /// separated word is the base kV when it reads as a number.
    fn bus(&self, token: &str) -> Result<BusId, UnresolvedReason> {
        let token = token.trim();
        let (name, kv) = match token.rsplit_once(char::is_whitespace) {
            Some((name, kv)) => match kv.parse::<f64>() {
                Ok(kv) if kv.is_finite() => (name.trim(), Some(kv)),
                _ => (token, None),
            },
            None => (token, None),
        };
        let candidates: Vec<&(String, f64, BusId)> = self
            .buses
            .get(&normalize_name(name))
            .into_iter()
            .flatten()
            .filter(|(_, base, _)| kv.is_none_or(|kv| (base - kv).abs() <= KV_TOLERANCE))
            .collect();
        let exact: Vec<BusId> = candidates
            .iter()
            .filter(|(stated, ..)| stated == name)
            .map(|(.., bus)| *bus)
            .collect();
        let all: Vec<BusId> = candidates.iter().map(|(.., bus)| *bus).collect();
        pick(&exact, &all).map_err(|matches| match matches {
            0 => UnresolvedReason::NoSuchBusName,
            matches => UnresolvedReason::AmbiguousBusName { matches },
        })
    }

    /// The branch rows a quoted branch name names.
    fn branch(&self, name: &str) -> Result<usize, UnresolvedReason> {
        let name = name.trim();
        let candidates = self
            .branches
            .get(&normalize_name(name))
            .map_or(&[][..], Vec::as_slice);
        let exact: Vec<usize> = candidates
            .iter()
            .filter(|(stated, _)| stated == name)
            .map(|(_, row)| *row)
            .collect();
        let all: Vec<usize> = candidates.iter().map(|(_, row)| *row).collect();
        pick(&exact, &all).map_err(|matches| match matches {
            0 => UnresolvedReason::NoSuchBranchName,
            matches => UnresolvedReason::AmbiguousBranchName { matches },
        })
    }
}

/// The one element a name names: the exact match when there is one, else
/// the one match without regard to case and whitespace. On failure, how many
/// elements matched at the stage that decided: zero, or more than one.
fn pick<T: Copy>(exact: &[T], all: &[T]) -> Result<T, usize> {
    match (exact, all) {
        ([one], _) | ([], [one]) => Ok(*one),
        ([], many) | (many, _) => Err(many.len()),
    }
}

/// What a statement kept as text names, when it is a TARA form this module
/// reads.
pub(super) enum TextAction {
    /// A statement in the PSS/E grammar once its named buses are numbers,
    /// or the action ahead of a `DISPATCH` block.
    Action(ContingencyAction),
    /// `OPEN | TRIP | DISCONNECT [BRANCH | LINE] 'name'`: one branch by name.
    Branch(usize),
}

/// Read a statement kept as text: a dispatch block's opening action, a
/// statement naming buses by name, or one branch named by name. `None` when
/// the text is none of these, so it stays unrecognized.
pub(super) fn read_text_action(
    names: &NameIndex,
    text: &str,
) -> Option<Result<TextAction, UnresolvedReason>> {
    let lines = lex(text);
    let (head, _) = split_block(&lines)?;
    let mut upper = head.keywords();
    let mut words: Vec<String> = head.words().iter().map(|w| (*w).to_owned()).collect();
    if upper.last().is_some_and(|w| w == "DISPATCH") && upper.len() > 1 {
        upper.pop();
        words.pop();
    }
    // A branch named by name.
    let quoted: Vec<usize> = head
        .tokens
        .iter()
        .enumerate()
        .filter(|(_, token)| token.quoted)
        .map(|(at, _)| at)
        .collect();
    if matches!(upper[0].as_str(), "OPEN" | "TRIP" | "DISCONNECT")
        && quoted.len() == 1
        && quoted[0] == upper.len() - 1
        && (upper.len() == 2
            || (upper.len() == 3 && matches!(upper[1].as_str(), "BRANCH" | "LINE")))
    {
        return Some(names.branch(&words[quoted[0]]).map(TextAction::Branch));
    }
    // Buses named by name: each quoted token after `BUS`.
    let mut named = false;
    for &at in &quoted {
        if at > 0 && at < upper.len() && upper[at - 1] == "BUS" {
            match names.bus(&words[at]) {
                Ok(bus) => {
                    words[at] = bus.0.to_string();
                    upper[at].clone_from(&words[at]);
                    named = true;
                }
                Err(reason) => return Some(Err(reason)),
            }
        }
    }
    let views: Vec<&str> = words.iter().map(String::as_str).collect();
    let action = parse_action(&upper, &views)?;
    let dispatched = head.keywords().last().is_some_and(|w| w == "DISPATCH");
    (named || dispatched).then_some(Ok(TextAction::Action(action)))
}
