//! Bounded, transactional interpretation of MATPOWER's unit-conversion idiom.
//!
//! This is not a MATLAB interpreter. Only straight-line scalar definitions,
//! verified idx_bus/idx_brch outputs, and all-row self-scaling of demand and
//! series impedance are accepted. Any unknown executable statement invalidates
//! the entire plan; the caller retains the indexed-assignment diagnostics.

use std::collections::HashMap;

use super::{locate::Assignment, matlab, matlab_continuation_end, matlab_statement_end, tokens};
use crate::network::BalancedNetwork;

const LIMIT: usize = 128;

#[derive(Clone, Copy, PartialEq)]
enum Table {
    Bus,
    Branch,
}

#[derive(Clone, PartialEq)]
struct Selection {
    table: Table,
    columns: Vec<usize>,
}

struct Scaling {
    selection: Selection,
    factor: f64,
    divide: bool,
}

impl Scaling {
    fn value(&self, value: f64) -> f64 {
        if self.divide {
            value / self.factor
        } else {
            value * self.factor
        }
    }
}

/// No network values change unless every statement and result is supported.
/// Source bytes are never rewritten, so unchanged same-format emission keeps
/// the conversion statements while canonical emission writes converted values.
pub(super) fn apply(
    source: &str,
    assignments: &[Assignment<'_>],
    net: &mut BalancedNetwork,
) -> bool {
    let Some(plan) = plan(source, assignments, net) else {
        return false;
    };
    // Preflight in source order, including intermediate overflow. Do not turn
    // a partially interpreted case into an apparently successful conversion.
    for load in net.loads() {
        if !finite_result(&plan, Table::Bus, 3, load.p)
            || !finite_result(&plan, Table::Bus, 4, load.q)
        {
            return false;
        }
    }
    for branch in net.branches() {
        if !finite_result(&plan, Table::Branch, 3, branch.r)
            || !finite_result(&plan, Table::Branch, 4, branch.x)
        {
            return false;
        }
    }
    for scale in plan {
        match scale.selection.table {
            Table::Bus => {
                for load in net.loads_mut() {
                    if scale.selection.columns.contains(&3) {
                        load.p = scale.value(load.p);
                    }
                    if scale.selection.columns.contains(&4) {
                        load.q = scale.value(load.q);
                    }
                }
            }
            Table::Branch => {
                for branch in net.branches_mut() {
                    if scale.selection.columns.contains(&3) {
                        branch.r = scale.value(branch.r);
                    }
                    if scale.selection.columns.contains(&4) {
                        branch.x = scale.value(branch.x);
                    }
                }
            }
        }
    }
    true
}

fn finite_result(plan: &[Scaling], table: Table, column: usize, mut value: f64) -> bool {
    for scale in plan {
        if scale.selection.table == table && scale.selection.columns.contains(&column) {
            value = scale.value(value);
            if !value.is_finite() {
                return false;
            }
        }
    }
    true
}

#[allow(
    clippy::too_many_lines,
    reason = "transactional statement-order validation stays in one pass"
)]
fn plan(
    source: &str,
    assignments: &[Assignment<'_>],
    net: &BalancedNetwork,
) -> Option<Vec<Scaling>> {
    let mut vars = HashMap::new();
    let mut seen = Vec::new();
    let mut scales = Vec::new();
    let mut at = 0;
    let mut first = true;
    let mut ended = false;
    let mut literals_end = 0;
    // The locator returns assignments in source order, so one cursor pairs
    // each statement with its assignment instead of searching them all.
    let starts: Vec<usize> = assignments
        .iter()
        .map(|a| a.start + a.text.len() - a.text.trim_start().len())
        .collect();
    debug_assert!(starts.is_sorted());
    let mut next = 0;
    while at < source.len() {
        at = skip_trivia(source, at)?;
        if at == source.len() {
            break;
        }
        let end = matlab_statement_end(source, at);
        let raw = &source[at..end];
        // The existing literal reader owns these statements. Require its exact
        // assignment start, one assignment per field, and no literals after
        // executable definitions. In particular a later baseMVA/bus overwrite
        // cannot be mistaken for the values used by an earlier conversion.
        while starts.get(next).is_some_and(|&start| start < at) {
            next += 1;
        }
        if let Some(assignment) = (starts.get(next) == Some(&at)).then(|| &assignments[next]) {
            if ended || !vars.is_empty() || !scales.is_empty() || seen.contains(&assignment.field) {
                return None;
            }
            if ![
                "version",
                "baseMVA",
                "bus",
                "branch",
                "gen",
                "gencost",
                "dcline",
                "dclinecost",
                "storage",
                "areas",
                "bus_name",
                "gentype",
                "genfuel",
            ]
            .contains(&assignment.field)
            {
                return None;
            }
            if !literal_assignment(raw, assignment.field) {
                return None;
            }
            seen.push(assignment.field);
            literals_end = end;
        } else {
            let tokens = tokenize(raw)?;
            if first && tokens.first() == Some(&"function") {
                // Only a single, argument-free case function is in this profile.
                if tokens.len() < 4
                    || tokens[1..3] != ["mpc", "="]
                    || !identifier(tokens[3])
                    || ["idx_bus", "idx_brch"].contains(&tokens[3])
                    || !(tokens.len() == 4 || tokens[4..] == ["(", ")"])
                {
                    return None;
                }
            } else if tokens == ["end"] && !ended {
                ended = true;
            } else {
                if ended
                    || at < literals_end
                    || !["baseMVA", "bus", "branch"]
                        .iter()
                        .all(|field| seen.contains(field))
                {
                    return None;
                }
                let mut parser = Parser {
                    tokens: &tokens,
                    at: 0,
                    vars: &vars,
                    net,
                };
                if tokens.first() == Some(&"mpc") {
                    let selection = parser.selection()?;
                    parser.expect("=")?;
                    if selection != parser.selection()? {
                        return None;
                    }
                    let divide = match parser.next()? {
                        "/" | "./" => true,
                        "*" | ".*" => false,
                        _ => return None,
                    };
                    let factor = parser.atom(0)?;
                    if !parser.done() || factor <= 0.0 || scales.len() == LIMIT {
                        return None;
                    }
                    scales.push(Scaling {
                        selection,
                        factor,
                        divide,
                    });
                } else if tokens.first() == Some(&"[") {
                    let bindings = index_bindings(&tokens)?;
                    for (name, value) in bindings {
                        vars.insert(name, value);
                    }
                } else {
                    let name = parser.next()?;
                    if !local_name(name) {
                        return None;
                    }
                    parser.expect("=")?;
                    let value = parser.expression(0)?;
                    if !parser.done() {
                        return None;
                    }
                    vars.insert(name, value);
                }
                if vars.len() > LIMIT {
                    return None;
                }
            }
        }
        first = false;
        at = end;
    }
    (seen.len() == assignments.len()).then_some(scales)
}

/// A located assignment is not necessarily a literal: the legacy locator also
/// returns `[rows] / scale` and ignored metadata expressions. Never suppress
/// diagnostics unless the entire RHS is a literal we can prove inert.
fn literal_assignment(raw: &str, field: &str) -> bool {
    let Some((_, rhs)) = raw.split_once('=') else {
        return false;
    };
    let rhs = rhs.trim_start();
    match field {
        "baseMVA" => tokenize(rhs).is_some_and(|tokens| {
            tokens.len() == 1 && tokens[0].parse::<f64>().is_ok_and(f64::is_finite)
        }),
        "version" => matlab::scalar_from_assignment(raw, field).is_ok_and(|value| value.is_some()),
        "bus_name" | "gentype" | "genfuel" => string_cell_literal(rhs),
        _ => {
            if !rhs.starts_with('[') {
                return false;
            }
            let mut offset = 0;
            let mut close = None;
            for line in rhs.split_inclusive('\n') {
                if let Some(at) = tokens::comment_split(line).0.find(']') {
                    close = Some(offset + at);
                    break;
                }
                offset += line.len();
            }
            let Some(close) = close else {
                return false;
            };
            if !tokenize(&rhs[close + 1..]).is_some_and(|tokens| tokens.is_empty()) {
                return false;
            }
            matlab::for_each_matrix_row(
                raw,
                0,
                field,
                &mut crate::collect::Diagnostics::new(),
                |_, _| Ok(()),
            )
            .is_ok()
        }
    }
}

fn string_cell_literal(rhs: &str) -> bool {
    let b = rhs.as_bytes();
    if b.first() != Some(&b'{') {
        return false;
    }
    let mut at = 1;
    while at < b.len() {
        if b[at].is_ascii_whitespace() || matches!(b[at], b',' | b';') {
            at += 1;
            continue;
        }
        if let Some(end) = matlab_continuation_end(b, at) {
            at = end;
            continue;
        }
        if b[at] == b'%' {
            at = rhs[at..].find('\n').map_or(b.len(), |n| at + n);
            continue;
        }
        if b[at] == b'}' {
            return tokenize(&rhs[at + 1..]).is_some_and(|tokens| tokens.is_empty());
        }
        let quote = b[at];
        if !matches!(quote, b'\'' | b'"') {
            return false;
        }
        at += 1;
        let mut closed = false;
        while at < b.len() {
            if b[at] == quote {
                at += 1;
                if b.get(at) == Some(&quote) {
                    at += 1;
                } else {
                    closed = true;
                    break;
                }
            } else {
                at += 1;
            }
        }
        if !closed {
            return false;
        }
    }
    false
}

/// Skip comments only at statement boundaries. Block delimiters must be alone
/// on their line, as in MATLAB; malformed blocks make the profile unsupported.
fn skip_trivia(source: &str, mut at: usize) -> Option<usize> {
    let b = source.as_bytes();
    loop {
        while at < b.len() && (b[at].is_ascii_whitespace() || matches!(b[at], b';' | b',')) {
            at += 1;
        }
        if b.get(at) != Some(&b'%') {
            return Some(at);
        }
        let end = source[at..].find('\n').map_or(b.len(), |n| at + n + 1);
        if source[at..end].trim() == "%{"
            && source[source[..at].rfind('\n').map_or(0, |i| i + 1)..at]
                .trim()
                .is_empty()
        {
            let mut depth = 1;
            at = end;
            while at < b.len() {
                let end = source[at..].find('\n').map_or(b.len(), |n| at + n + 1);
                match source[at..end].trim() {
                    "%{" => depth += 1,
                    "%}" => depth -= 1,
                    _ => {}
                }
                if depth > LIMIT {
                    return None;
                }
                at = end;
                if depth == 0 {
                    break;
                }
            }
            if depth != 0 {
                return None;
            }
        } else {
            at = end;
        }
    }
}

/// Token slices retain their boundaries: `1 2` must never become `12`.
fn tokenize(raw: &str) -> Option<Vec<&str>> {
    let b = raw.as_bytes();
    let mut tokens = Vec::new();
    let mut at = 0;
    while at < b.len() {
        if b[at].is_ascii_whitespace() {
            at += 1;
            continue;
        }
        if let Some(end) = matlab_continuation_end(b, at) {
            at = end;
            continue;
        }
        if b[at] == b'%' {
            at = raw[at..].find('\n').map_or(b.len(), |n| at + n);
            continue;
        }
        let start = at;
        if b[at].is_ascii_alphabetic() || b[at] == b'_' {
            at += 1;
            while at < b.len() && (b[at].is_ascii_alphanumeric() || b[at] == b'_') {
                at += 1;
            }
        } else if b[at].is_ascii_digit()
            || (b[at] == b'.' && b.get(at + 1).is_some_and(u8::is_ascii_digit))
        {
            at += 1;
            while at < b.len() && (b[at].is_ascii_digit() || b[at] == b'.') {
                if b[at] == b'.'
                    && b.get(at + 1)
                        .is_some_and(|c| matches!(c, b'*' | b'/' | b'^'))
                {
                    break;
                }
                at += 1;
            }
            if b.get(at).is_some_and(|c| matches!(c, b'e' | b'E')) {
                at += 1;
                if b.get(at).is_some_and(|c| matches!(c, b'+' | b'-')) {
                    at += 1;
                }
                while at < b.len() && b[at].is_ascii_digit() {
                    at += 1;
                }
            }
        } else if b[at] == b'.'
            && b.get(at + 1)
                .is_some_and(|c| matches!(c, b'*' | b'/' | b'^'))
        {
            at += 2;
        } else if b".()[],:;=*/^+-~".contains(&b[at]) {
            at += 1;
        } else {
            return None;
        }
        tokens.push(&raw[start..at]);
        if tokens.len() > LIMIT {
            return None;
        }
    }
    if tokens
        .last()
        .is_some_and(|token| matches!(*token, ";" | ","))
    {
        tokens.pop();
    }
    Some(tokens)
}

fn local_name(name: &str) -> bool {
    identifier(name)
        && ![
            "mpc",
            "idx_bus",
            "idx_brch",
            "if",
            "else",
            "elseif",
            "end",
            "for",
            "parfor",
            "while",
            "switch",
            "case",
            "otherwise",
            "try",
            "catch",
            "function",
            "return",
            "break",
            "continue",
            "global",
            "persistent",
            "classdef",
            "spmd",
        ]
        .contains(&name)
}

fn identifier(name: &str) -> bool {
    name.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

/// idx_* returns values by output position, not by the caller's variable names.
fn index_bindings<'a>(tokens: &[&'a str]) -> Option<Vec<(&'a str, f64)>> {
    let close = tokens.iter().position(|token| *token == "]")?;
    if tokens.get(close + 1) != Some(&"=") {
        return None;
    }
    let values: &[u8] = match *tokens.get(close + 2)? {
        "idx_bus" => &[
            1, 2, 3, 4, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17,
        ],
        "idx_brch" => &[
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 14, 15, 16, 17, 18, 19, 12, 13, 20, 21,
        ],
        _ => return None,
    };
    if !(tokens.len() == close + 3 || tokens[close + 3..] == ["(", ")"]) {
        return None;
    }
    let mut out = Vec::new();
    let mut index = 0;
    let mut comma = false;
    for &name in &tokens[1..close] {
        if name == "," {
            if index == 0 || comma {
                return None;
            }
            comma = true;
            continue;
        }
        if !(local_name(name) || name == "~") {
            return None;
        }
        let value = f64::from(*values.get(index)?);
        if name != "~" {
            out.push((name, value));
        }
        index += 1;
        comma = false;
    }
    if index == 0 || comma {
        return None;
    }
    Some(out)
}

struct Parser<'a, 'b> {
    tokens: &'b [&'a str],
    at: usize,
    vars: &'b HashMap<&'a str, f64>,
    net: &'b BalancedNetwork,
}

impl<'a> Parser<'a, '_> {
    fn next(&mut self) -> Option<&'a str> {
        let token = *self.tokens.get(self.at)?;
        self.at += 1;
        Some(token)
    }

    fn expect(&mut self, token: &str) -> Option<()> {
        (self.next()? == token).then_some(())
    }

    fn done(&self) -> bool {
        self.at == self.tokens.len()
    }

    fn number(&self, token: &str) -> Option<f64> {
        token
            .parse::<f64>()
            .ok()
            .or_else(|| self.vars.get(token).copied())
            .filter(|value| value.is_finite())
    }

    fn selection(&mut self) -> Option<Selection> {
        self.expect("mpc")?;
        self.expect(".")?;
        let table = match self.next()? {
            "bus" => Table::Bus,
            "branch" => Table::Branch,
            _ => return None,
        };
        self.expect("(")?;
        self.expect(":")?;
        self.expect(",")?;
        let mut columns = Vec::new();
        let first = self.next()?;
        if first == "[" {
            let mut comma = false;
            loop {
                let token = self.next()?;
                if token == "]" {
                    if comma {
                        return None;
                    }
                    break;
                }
                if token == "," {
                    if columns.is_empty() || comma {
                        return None;
                    }
                    comma = true;
                    continue;
                }
                columns.push(self.column(token)?);
                comma = false;
            }
        } else {
            columns.push(self.column(first)?);
        }
        self.expect(")")?;
        if columns.is_empty()
            || columns.len() > 2
            || (columns.len() == 2 && columns[0] == columns[1])
        {
            return None;
        }
        Some(Selection { table, columns })
    }

    #[allow(clippy::float_cmp)]
    fn column(&self, token: &str) -> Option<usize> {
        match self.number(token)? {
            3.0 => Some(3),
            4.0 => Some(4),
            _ => None,
        }
    }

    // Scalar products/quotients with parentheses and squaring are sufficient
    // for Vbase^2/Sbase. No calls, indexing other than base voltage, general
    // powers, addition, or arbitrary expression evaluation is permitted.
    fn expression(&mut self, depth: usize) -> Option<f64> {
        if depth > 16 {
            return None;
        }
        let mut value = self.atom(depth)?;
        while let Some(&operator @ ("*" | ".*" | "/" | "./")) = self.tokens.get(self.at) {
            self.at += 1;
            let rhs = self.atom(depth)?;
            value = if matches!(operator, "*" | ".*") {
                value * rhs
            } else {
                value / rhs
            };
            if !value.is_finite() {
                return None;
            }
        }
        Some(value)
    }

    #[allow(
        clippy::float_cmp,
        reason = "MATLAB column indices must be exact integers"
    )]
    fn atom(&mut self, depth: usize) -> Option<f64> {
        let token = self.next()?;
        let mut value = match token {
            "(" => {
                let value = self.expression(depth + 1)?;
                self.expect(")")?;
                value
            }
            "mpc" => {
                self.expect(".")?;
                match self.next()? {
                    "baseMVA" => {
                        let value = self.net.base_mva();
                        if value <= 0.0 {
                            return None;
                        }
                        value
                    }
                    "bus" => {
                        self.expect("(")?;
                        // Row one is MATLAB row position, never the source bus ID.
                        self.expect("1")?;
                        self.expect(",")?;
                        let column = self.next()?;
                        if self.number(column)? != 10.0 {
                            return None;
                        }
                        self.expect(")")?;
                        {
                            let value = self.net.buses().first()?.base_kv;
                            if value <= 0.0 {
                                return None;
                            }
                            value
                        }
                    }
                    _ => return None,
                }
            }
            "+" => {
                let number = self.next()?;
                self.number(number)?
            }
            "-" => {
                let number = self.next()?;
                -self.number(number)?
            }
            _ => self.number(token)?,
        };
        if self
            .tokens
            .get(self.at)
            .is_some_and(|token| matches!(*token, "^" | ".^"))
        {
            if matches!(token, "+" | "-") {
                return None;
            }
            self.at += 1;
            self.expect("2")?;
            value *= value;
        }
        value.is_finite().then_some(value)
    }
}
