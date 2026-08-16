//! The anonymization boundary.
//!
//! Findings leave the machine; corpus bytes do not. Everything the harness
//! emits passes through this module, which states three things and only three:
//! class ordinals in place of identifiers, ratios and orders of magnitude in
//! place of values, and masked templates in place of any text that came from a
//! file.
//!
//! The last line of defense is [`Sanitizer::audit`], which re-reads the emitted
//! report and fails the run if any string the corpus taught the harness
//! survived into it. The harness knows every name in every case it parsed and
//! every path it walked, so this is a decidable check rather than a heuristic.

use std::collections::{BTreeMap, BTreeSet};

/// A string from the corpus that reached the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leak {
    pub secret: String,
    pub line: usize,
}

impl std::fmt::Display for Leak {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Names the length and the line, never the string itself: a panic
        // message is as public as the report is.
        write!(
            f,
            "line {}: a {}-character string from the corpus reached the report",
            self.line,
            self.secret.chars().count()
        )
    }
}

/// Replaces corpus identifiers with ordinals and corpus values with
/// magnitudes.
#[derive(Debug, Default, Clone)]
pub struct Sanitizer {
    /// Bus id to dense ordinal, in sorted bus-id order.
    bus_ordinal: BTreeMap<usize, usize>,
    /// Every string the corpus taught us: element names, uids, extras values,
    /// case names, and every path component walked.
    secrets: BTreeSet<String>,
    /// Tokens the harness itself emits that a corpus may also happen to use.
    /// A corpus directory named `psse/` must not make the format token `psse`
    /// unreportable.
    allowed: BTreeSet<String>,
}

impl Sanitizer {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Learn every string in a parsed network, and give its buses ordinals.
    ///
    /// Walks the serialized model rather than a hand-written field list: a new
    /// name-bearing field then joins the secret set the day it is added, with
    /// no second place to remember.
    pub fn learn_network(&mut self, value: &serde_json::Value) {
        collect_strings(value, &mut self.secrets);
    }

    /// Assign ordinals to a bus id set. Called once per bucket with the
    /// reference member's buses, then again per sibling so a renumbering
    /// format still lands on ordinals.
    pub fn learn_buses(&mut self, ids: impl IntoIterator<Item = usize>) {
        let mut ids: Vec<usize> = ids.into_iter().collect();
        ids.sort_unstable();
        for id in ids {
            let next = self.bus_ordinal.len();
            self.bus_ordinal.entry(id).or_insert(next);
        }
    }

    /// Declare a token as the harness's own vocabulary. Format names are the
    /// case that matters: a corpus laid out as `psse/`, `matpower/`, `dss/`
    /// teaches those words as path components, and without this the report
    /// could not name the format a finding belongs to.
    pub fn allow(&mut self, token: &str) {
        self.allowed.insert(token.to_string());
        // `egret-json` also licenses the word `egret`, which is what a corpus
        // laid out by format actually names its directories.
        for part in token.split(['-', '_', '.']) {
            if !part.is_empty() {
                self.allowed.insert(part.to_string());
            }
        }
    }

    /// Learn a path the harness walked, component by component, so neither the
    /// full path nor a directory or file name can reach the report.
    pub fn learn_path(&mut self, path: &std::path::Path) {
        self.secrets.insert(path.to_string_lossy().into_owned());
        for part in path.components() {
            let part = part.as_os_str().to_string_lossy();
            self.secrets.insert(part.to_string());
            if let Some((stem, _)) = part.rsplit_once('.') {
                self.secrets.insert(stem.to_string());
            }
        }
    }

    /// A bus as a class ordinal. An unseen id gets the next ordinal rather
    /// than passing through.
    pub fn bus(&mut self, id: usize) -> String {
        let next = self.bus_ordinal.len();
        let ordinal = *self.bus_ordinal.entry(id).or_insert(next);
        format!("bus#{ordinal}")
    }

    /// Free text reduced to a template: names become `<name>`, digit runs
    /// become `#`, and quoted spans become `'…'`.
    ///
    /// Warning prose is format vocabulary and is worth keeping; the record
    /// values a warning quotes are case data and are not.
    #[must_use]
    pub fn template(&self, text: &str) -> String {
        let mut out = mask_quoted(text);
        // Longest first, so a name that contains another is replaced whole.
        let mut names: Vec<&String> = self.identifying().collect();
        names.sort_by_key(|n| std::cmp::Reverse(n.len()));
        for name in names {
            out = replace_tokens(&out, name, "<name>");
        }
        mask_digits(&out)
    }

    /// The secrets worth checking for: long enough to identify, carrying a
    /// letter, and not part of powerio's own vocabulary.
    fn identifying(&self) -> impl Iterator<Item = &String> {
        let mut vocabulary = vocabulary();
        vocabulary.extend(self.allowed.iter().cloned());
        self.secrets.iter().filter(move |s| {
            s.chars().count() >= 4
                && s.chars().any(char::is_alphabetic)
                && !vocabulary.contains(s.as_str())
        })
    }

    /// Confirm nothing the corpus taught us survived into `emitted`.
    ///
    /// This is the backstop, and it is decidable rather than heuristic: the
    /// harness parsed every case and walked every path, so it knows the exact
    /// set of strings that must not appear.
    ///
    /// # Errors
    ///
    /// Returns every secret found, with the line it appeared on.
    pub fn audit(&self, emitted: &str) -> Result<(), Vec<Leak>> {
        let secrets: Vec<&String> = self.identifying().collect();
        let mut leaks = Vec::new();
        for (i, line) in emitted.lines().enumerate() {
            for secret in &secrets {
                if contains_token(line, secret) {
                    leaks.push(Leak {
                        secret: (*secret).clone(),
                        line: i + 1,
                    });
                }
            }
        }
        if leaks.is_empty() { Ok(()) } else { Err(leaks) }
    }
}

/// Every string powerio itself writes into a serialized network that is not
/// case data: enum spellings, format tokens, unit names.
///
/// Derived by serializing a network built here, whose every name is chosen
/// here, rather than from a hand-kept list — a new enum variant joins the
/// vocabulary the day the model gains it. A real case whose element is named
/// after one of these spellings is exempt from the audit; the tradeoff runs
/// the safe way, since a spurious failure would stop a run cold while this
/// costs one masked token.
fn vocabulary() -> BTreeSet<String> {
    use powerio_matrix::{BalancedNetwork, Branch, Bus, BusId, BusType, Generator, Load, Shunt};
    let mut net = BalancedNetwork::new("", 100.0);
    net.buses.push(Bus::new(BusId(1), BusType::Ref, 1.0));
    net.buses.push(Bus::new(BusId(2), BusType::Pv, 1.0));
    net.buses.push(Bus::new(BusId(3), BusType::Pq, 1.0));
    net.buses.push(Bus::new(BusId(4), BusType::Isolated, 1.0));
    net.branches.push(Branch::new(BusId(1), BusId(2), 0.0, 1.0));
    net.generators.push(Generator::new(BusId(1)));
    net.loads.push(Load::new(BusId(1), 0.0, 0.0));
    net.shunts.push(Shunt::new(BusId(1), 0.0, 0.0));
    let mut out = BTreeSet::new();
    if let Ok(value) = serde_json::to_value(&net) {
        collect_strings(&value, &mut out);
    }
    out
}

/// `after / before`, to three significant digits, or `None` when `before` is
/// zero and the ratio would state the new value outright.
#[must_use]
pub fn ratio(before: f64, after: f64) -> Option<f64> {
    if before == 0.0 || !before.is_finite() || !after.is_finite() {
        return None;
    }
    let r = after / before;
    let scale = 10f64.powi(3 - 1 - r.abs().log10().floor() as i32);
    Some((r * scale).round() / scale)
}

/// The decimal order of magnitude of a value: `1` for 42.0, `-3` for 0.004,
/// `None` for zero. Enough to tell a per-unit impedance from a MW total,
/// never enough to state the value.
#[must_use]
pub fn magnitude(x: f64) -> Option<i32> {
    if x == 0.0 || !x.is_finite() {
        return None;
    }
    Some(x.abs().log10().floor() as i32)
}

/// Collapse array indices in a serde path so `.loads[3].p` and `.loads[47].p`
/// become one class, `.loads[#].p`. Both safer (the report stops counting a
/// utility's loads) and more useful (one line per class of loss).
#[must_use]
pub fn collapse_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut in_index = false;
    for ch in path.chars() {
        match ch {
            '[' => {
                in_index = true;
                out.push_str("[#");
            }
            ']' => {
                in_index = false;
                out.push(']');
            }
            _ if in_index => {}
            _ => out.push(ch),
        }
    }
    out
}

/// Whether `needle` appears in `haystack` as a whole word.
///
/// Substring matching is wrong in both directions here. It fires on
/// `egret` inside the format token `egret-json`, which is vocabulary rather
/// than a leak; and a corpus name that happens to be two letters would
/// otherwise match half the report. A word is what a reader means when they
/// ask whether their filename appears.
fn word_boundaries(haystack: &str, needle: &str) -> Vec<usize> {
    if needle.is_empty() {
        return Vec::new();
    }
    let bytes = haystack.as_bytes();
    let part = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    haystack
        .match_indices(needle)
        .filter(|(i, m)| {
            let before = *i == 0 || !part(bytes[*i - 1]);
            let end = *i + m.len();
            let after = end >= bytes.len() || !part(bytes[end]);
            before && after
        })
        .map(|(i, _)| i)
        .collect()
}

fn contains_token(haystack: &str, needle: &str) -> bool {
    !word_boundaries(haystack, needle).is_empty()
}

fn replace_tokens(haystack: &str, needle: &str, with: &str) -> String {
    let hits = word_boundaries(haystack, needle);
    if hits.is_empty() {
        return haystack.to_string();
    }
    let mut out = String::with_capacity(haystack.len());
    let mut cursor = 0;
    for start in hits {
        if start < cursor {
            continue;
        }
        out.push_str(&haystack[cursor..start]);
        out.push_str(with);
        cursor = start + needle.len();
    }
    out.push_str(&haystack[cursor..]);
    out
}

fn mask_quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut quote: Option<char> = None;
    for ch in text.chars() {
        match quote {
            Some(open) if ch == open => {
                out.push('…');
                out.push(ch);
                quote = None;
            }
            Some(_) => {}
            None => {
                if ch == '\'' || ch == '"' || ch == '`' {
                    quote = Some(ch);
                }
                out.push(ch);
            }
        }
    }
    out
}

fn mask_digits(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_run = false;
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            if !in_run {
                out.push('#');
                in_run = true;
            }
        } else {
            in_run = false;
            out.push(ch);
        }
    }
    out
}

fn collect_strings(value: &serde_json::Value, out: &mut BTreeSet<String>) {
    match value {
        serde_json::Value::String(s) if !s.is_empty() => {
            out.insert(s.clone());
        }
        serde_json::Value::Array(xs) => {
            for x in xs {
                collect_strings(x, out);
            }
        }
        serde_json::Value::Object(xs) => {
            // Keys are field names from powerio's own model, so they are
            // vocabulary rather than case data; only values are learned.
            for x in xs.values() {
                collect_strings(x, out);
            }
        }
        _ => {}
    }
}
