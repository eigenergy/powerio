//! The corpus harness: the conversion invariants, run over a directory of
//! case files.
//!
//! Three commands, each writing what the next one reads:
//!
//! ```text
//! corpus dir (read-only, outside the repo)
//!   │  powerio corpus ingest <dir> --work <scratch>
//!   ▼
//! buckets: case-000, case-001, …   (grouped by electrical fingerprint,
//!   │                               never by filename)
//!   ▼  powerio corpus compare --work <scratch>
//! per bucket: round trip per sibling, cross-write per sibling pair,
//!   │         sibling agreement between the two readers
//!   ▼  powerio corpus report --work <scratch> -o findings.jsonl
//! findings.jsonl + summary.md      (codes, ordinals, deltas)
//! ```
//!
//! The corpus directory is opened read-only and never written. The work
//! directory is disposable and holds raw values, so it stays on the machine
//! that owns the corpus. The report is the boundary: [`anonymize`] states what
//! may cross it, and the reporter audits its own output against every string
//! the corpus taught the harness before writing a byte.
//!
//! See `docs/src/corpus-harness.md` for the design and the session protocol
//! that consumes the findings.

pub mod anonymize;
pub mod fingerprint;
pub mod walk;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use powerio::BalancedNetwork;
use powerio_tx::TargetFormat;
use serde::{Deserialize, Serialize};

use crate::invariants::{self, YbusUnavailable};
use anonymize::Sanitizer;
use fingerprint::Fingerprint;

const INGEST_FILE: &str = "ingest.json";
const COMPARE_FILE: &str = "comparisons.json";

/// Every key and enum spelling the findings file itself uses. Declared as
/// vocabulary so the leak audit never mistakes the report's own schema for
/// case data; keep it in step with [`Finding`], [`findings_for`] and
/// [`walk_findings`].
const SCHEMA_KEYS: [&str; 83] = [
    // The summary's own prose, title case included: the audit matches exact
    // tokens.
    "Corpus",
    "run",
    "files",
    "seen",
    "unreadable",
    "Buckets",
    "siblings",
    "Findings",
    "count",
    // JSON's own literals: a corpus value spelled "false" must not make the
    // report's booleans unwritable.
    "true",
    "false",
    "null",
    // Every finding code. `allow` licenses each dot- and dash-separated
    // component, so a corpus value spelled "terminal" cannot make
    // `electrical.dc-terminal` unwritable.
    "parse.panic",
    "parse.failure",
    "leg.panic",
    "leg.failure",
    "leg.unresolved-include",
    "sibling.different-data",
    "sibling.status",
    "sibling.ybus",
    "electrical.unavailable",
    "electrical.ybus",
    "electrical.injection",
    "electrical.dc-terminal",
    "core.power",
    "core.regrouped",
    "loss.declared",
    "loss.undeclared",
    "warning.observed",
    "walk.panic",
    "walk.failure",
    "walk.drift",
    "walk.path-dependent",
    "walk.resurrection",
    "walk.absorbed",
    // The remaining severities ("declared" is below with the schema keys).
    "crash",
    "silent-drop",
    "silent-value-change",
    "undeclared-loss",
    // The `core_delta` labels.
    "buses",
    "branches",
    "generators",
    "loads",
    "shunts",
    "load",
    "gen",
    "base",
    "mva",
    "hop",
    "hops",
    "path",
    "origin",
    "direct",
    "emptied",
    "resurrected",
    "seed",
    "revisit_of",
    "walk",
    "bucket",
    "code",
    "severity",
    "leg",
    "from",
    "to",
    "via",
    "from_ordinal",
    "to_ordinal",
    "detail",
    "templates",
    "paths",
    "delta",
    "message",
    "side",
    "status_only",
    "entries_changed",
    "round-trip",
    "cross-write",
    "sibling",
    "declared",
    "elements",
    "compared",
    "unresolved-include",
    "different_case_data",
];

/// One readable case in the corpus, as ingest found it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Member {
    /// Position within the bucket; the only name this file has downstream.
    pub ordinal: usize,
    /// The format powerio read it as. Format vocabulary, not case data.
    pub format: String,
    /// Read back by `compare`. Never leaves the work directory.
    pub path: PathBuf,
    pub warnings: Vec<String>,
}

/// Which model a case parsed into. Kept out of the sibling comparison
/// entirely: a balanced network and a multiconductor one answer different
/// questions and share no invariant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Domain {
    Transmission,
    Distribution,
}

/// Cases that fingerprint alike.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bucket {
    pub id: String,
    pub domain: Domain,
    pub fingerprint: Fingerprint,
    pub members: Vec<Member>,
}

/// A file the harness could not turn into a network.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Unreadable {
    pub path: PathBuf,
    pub error: String,
    /// Set when the reader panicked rather than returning an error.
    pub panicked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ingest {
    pub buckets: Vec<Bucket>,
    pub unreadable: Vec<Unreadable>,
    pub files_seen: usize,
    /// Files no reader claimed and whose name did not announce a case: a
    /// corpus's licences, notes and spreadsheets.
    pub skipped: usize,
    /// Entries that resolved outside the corpus root and were not read.
    pub escaped: usize,
}

/// Whether `path` really lives under `root` once symlinks are resolved.
///
/// Both sides are canonicalized: comparing the walked path textually would
/// accept a symlink whose name sits under the root and whose target does not.
/// A path that cannot be canonicalized is treated as outside, since a file the
/// harness cannot resolve is one it should not open.
fn within(root: &Path, path: &Path) -> bool {
    match (root.canonicalize(), path.canonicalize()) {
        (Ok(root), Ok(path)) => path.starts_with(&root),
        _ => false,
    }
}

/// Whether a path announces itself as a case file. Used only to decide whether
/// a parse failure is worth reporting, never to decide whether to try.
fn names_a_case(path: &Path) -> bool {
    const CASE_EXTENSIONS: [&str; 9] = [
        "m", "raw", "rawx", "epc", "aux", "pwb", "uct", "json", "dss",
    ];
    path.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|ext| CASE_EXTENSIONS.contains(&ext.as_str()))
}

/// Walk `corpus`, parse everything readable, and bucket it by electrical
/// fingerprint.
///
/// `max_bytes` skips any file larger, counted into `skipped`: the pairwise
/// compare is quadratic in a bucket and a walk re-parses per hop, so one
/// interconnection scale case in an otherwise moderate corpus would own the
/// whole run. `None` reads everything.
///
/// # Errors
///
/// Fails when the corpus directory cannot be walked or the work directory
/// cannot be written.
pub fn ingest(corpus: &Path, work: &Path, max_bytes: Option<u64>) -> Result<Ingest> {
    std::fs::create_dir_all(work)
        .with_context(|| format!("create work directory {}", work.display()))?;
    let mut files_seen = 0usize;
    let mut skipped = 0usize;
    let mut escaped = 0usize;
    let mut unreadable = Vec::new();
    let mut parsed: Vec<Parsed> = Vec::new();

    for entry in walkdir::WalkDir::new(corpus)
        .follow_links(false)
        .sort_by_file_name()
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                unreadable.push(Unreadable {
                    path: err.path().unwrap_or(corpus).to_path_buf(),
                    error: err.to_string(),
                    panicked: false,
                });
                continue;
            }
        };
        let path = entry.path();
        // `follow_links(false)` stops the walk descending a symlinked
        // directory, but a symlinked FILE is still handed over and reading it
        // reads its target. A corpus is not trusted to point only at itself,
        // so anything resolving outside the root is skipped rather than read.
        if !within(corpus, path) {
            escaped += 1;
            continue;
        }
        let is_pypsa_dir = entry.file_type().is_dir() && path.join("network.csv").is_file();
        if entry.file_type().is_dir() && !is_pypsa_dir {
            continue;
        }
        files_seen += 1;
        if let Some(cap) = max_bytes {
            let over = entry.metadata().is_ok_and(|m| m.is_file() && m.len() > cap);
            if over {
                skipped += 1;
                continue;
            }
        }
        match read_case(path) {
            // A file that parses to no buses is a library, not a case: dss
            // linecode and wiredata decks parse happily and carry no network.
            // Bucketing them groups every such library in a corpus into one
            // meaningless bucket.
            Ok(case) if case.fingerprint().buses == 0 => skipped += 1,
            Ok(case) => {
                parsed.push(Parsed {
                    domain: case.domain(),
                    fingerprint: case.fingerprint(),
                    format: case.format(),
                    path: path.to_path_buf(),
                    warnings: case.warnings(),
                });
            }
            // Everything is tried, so a case under an unexpected extension is
            // still found; only a file that announced itself as a case is
            // reported when it fails. Otherwise a corpus's licences and notes
            // would bury the findings that matter.
            Err(bad) if names_a_case(path) || bad.panicked => unreadable.push(bad),
            Err(_) => skipped += 1,
        }
    }

    let mut buckets = bucket(parsed);
    buckets.sort_by(|a, b| {
        a.fingerprint
            .primary()
            .cmp(&b.fingerprint.primary())
            .then_with(|| a.fingerprint.cmp(&b.fingerprint))
    });
    for (i, bucket) in buckets.iter_mut().enumerate() {
        bucket.id = format!("case-{i:03}");
        for (ordinal, member) in bucket.members.iter_mut().enumerate() {
            member.ordinal = ordinal;
        }
    }

    let ingest = Ingest {
        buckets,
        unreadable,
        files_seen,
        skipped,
        escaped,
    };
    write_json(&work.join(INGEST_FILE), &ingest)?;
    Ok(ingest)
}

struct Parsed {
    domain: Domain,
    fingerprint: Fingerprint,
    format: String,
    path: PathBuf,
    warnings: Vec<String>,
}

/// Group by the exact half of the fingerprint, then split each group where the
/// tolerant half disagrees.
fn bucket(parsed: Vec<Parsed>) -> Vec<Bucket> {
    let mut groups: BTreeMap<(Domain, fingerprint::PrimaryKey), Vec<Parsed>> = BTreeMap::new();
    for item in parsed {
        groups
            .entry((item.domain, item.fingerprint.primary()))
            .or_default()
            .push(item);
    }
    let mut out = Vec::new();
    for ((domain, _), group) in groups {
        // Within one primary key, the first member of each split is its
        // representative; a later member joins the first split it agrees with.
        let mut splits: Vec<Bucket> = Vec::new();
        for item in group {
            let member = Member {
                ordinal: 0,
                format: item.format,
                path: item.path,
                warnings: item.warnings,
            };
            match splits
                .iter_mut()
                .find(|split| split.fingerprint.agrees_with(&item.fingerprint))
            {
                Some(split) => split.members.push(member),
                None => splits.push(Bucket {
                    id: String::new(),
                    domain,
                    fingerprint: item.fingerprint,
                    members: vec![member],
                }),
            }
        }
        out.extend(splits);
    }
    out
}

/// One parsed case, in whichever model claimed it.
pub enum Case {
    Balanced(Box<BalancedNetwork>, Vec<String>),
    Multiconductor(Box<powerio_dist::MulticonductorNetwork>, Vec<String>),
}

impl Case {
    fn domain(&self) -> Domain {
        match self {
            Self::Balanced(..) => Domain::Transmission,
            Self::Multiconductor(..) => Domain::Distribution,
        }
    }

    fn fingerprint(&self) -> Fingerprint {
        match self {
            Self::Balanced(net, _) => Fingerprint::of(net),
            Self::Multiconductor(net, _) => Fingerprint::of_distribution(net),
        }
    }

    fn format(&self) -> String {
        match self {
            Self::Balanced(net, _) => net.source_format().name().to_string(),
            Self::Multiconductor(net, _) => net
                .source_format()
                .as_ref()
                .map_or("dss", |f| f.name())
                .to_string(),
        }
    }

    fn warnings(&self) -> Vec<String> {
        match self {
            Self::Balanced(_, warnings) | Self::Multiconductor(_, warnings) => warnings.clone(),
        }
    }
}

/// Parse one path, turning a reader panic into a result rather than taking the
/// process down. A crash on real input is the most valuable finding the
/// harness can produce, so it must survive to the report.
///
/// The balanced reader is tried first and the multiconductor reader second:
/// the two claim disjoint formats, and the balanced one names the other in its
/// error when a distribution file reaches it.
fn read_case(path: &Path) -> std::result::Result<Case, Unreadable> {
    let unreadable = |error: String, panicked: bool| Unreadable {
        path: path.to_path_buf(),
        error,
        panicked,
    };
    let balanced_error = match catch_panic(|| crate::module_io::load_balanced_module(path, None)) {
        Ok(Ok(parsed)) => {
            let rendered = powerio_core::render_diagnostics(&parsed.diagnostics);
            return Ok(Case::Balanced(Box::new(parsed.into_value()), rendered));
        }
        Err(message) => return Err(unreadable(message, true)),
        Ok(Err(err)) => err.to_string(),
    };
    match catch_panic(|| crate::module_io::load_multiconductor_module(path, None)) {
        Ok(Ok(parsed)) => {
            let warnings = powerio_core::render_diagnostics(&parsed.diagnostics);
            Ok(Case::Multiconductor(
                Box::new(parsed.into_value()),
                warnings,
            ))
        }
        // A `.m` that failed the MATPOWER parse used to report "unknown
        // distribution format `m`": the fallback reader's refusal displaced
        // the diagnosis. When the distribution reader does not even claim the
        // format, the transmission error is the one that says what is wrong.
        Ok(Err(err)) => {
            let message = err.to_string();
            Err(unreadable(
                if message.contains("unknown distribution format")
                    || message.contains("REQUEST.FORMAT.UNKNOWN")
                {
                    balanced_error
                } else {
                    message
                },
                false,
            ))
        }
        Err(message) => Err(unreadable(message, true)),
    }
}

/// Re-read a member as a balanced network, or nothing when it is not one.
fn balanced(path: &Path) -> Option<BalancedNetwork> {
    match read_case(path) {
        Ok(Case::Balanced(net, _)) => Some(*net),
        _ => None,
    }
}

/// Re-read a member as a multiconductor network, or nothing when it is not
/// one.
fn multiconductor(path: &Path) -> Option<powerio_dist::MulticonductorNetwork> {
    match read_case(path) {
        Ok(Case::Multiconductor(net, _)) => Some(*net),
        _ => None,
    }
}

/// Run `f`, returning the panic message instead of unwinding. The default hook
/// is silenced for the duration so a corpus of unparseable files does not
/// print a backtrace per file.
fn catch_panic<T>(
    f: impl FnOnce() -> T + std::panic::UnwindSafe,
) -> std::result::Result<T, String> {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = std::panic::catch_unwind(f);
    std::panic::set_hook(previous);
    result.map_err(|payload| {
        payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "panic".to_string())
    })
}

/// How two networks in one bucket were brought together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Via {
    /// Parse, write in the same format, read back.
    RoundTrip,
    /// Parse one sibling, write it in another sibling's format, read back.
    CrossWrite,
    /// Two independent reads of the same case in two formats, compared
    /// directly. The only leg that can catch a reader both directions of a
    /// round trip agree about.
    Sibling,
    /// One step of a [`walk`] chain. Its `from` is whatever the walk was in,
    /// which is a network several formats deep rather than a corpus file, and
    /// the ordinals are hop positions rather than bucket members.
    Walk,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Leg {
    pub from: String,
    pub to: String,
    pub via: Via,
    pub from_ordinal: usize,
    pub to_ordinal: usize,
}

/// Everything one leg produced, still in raw values. Stays in the work
/// directory; [`report`] is what sanitizes it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Comparison {
    pub bucket: String,
    pub leg: Leg,
    pub warnings: Vec<String>,
    pub core_changed: Option<String>,
    pub ybus: Option<String>,
    pub ybus_unavailable: Option<String>,
    pub injection: Option<String>,
    pub injection_status_only: bool,
    /// A DC line's terminal power moved. Its own field because HVDC is an
    /// injection no admittance matrix sees and several formats cannot state at
    /// all, so it is graded separately from AC power.
    pub dc_terminal: Option<String>,
    /// Serde paths that changed, already collapsed to their class.
    pub model_diffs: Vec<String>,
    pub failure: Option<String>,
    /// Two members of one bucket describe related cases but contain different
    /// limits or other declared case data.
    pub different_case_data: bool,
    /// A written deck that pulls in other files, which a string readback
    /// cannot resolve. Recorded for the same reason as
    /// [`different_case_data`](Self::different_case_data).
    pub unresolved_include: bool,
    /// Elements the two sides disagree about the service status of.
    pub status_changed: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Comparisons {
    pub comparisons: Vec<Comparison>,
}

/// Run the invariants over every bucket in the work directory.
///
/// # Errors
///
/// Fails when the work directory has no ingest output or cannot be written.
pub fn compare(work: &Path) -> Result<Comparisons> {
    let ingest: Ingest = read_json(&work.join(INGEST_FILE))?;
    let mut comparisons = Vec::new();

    for bucket in &ingest.buckets {
        match bucket.domain {
            Domain::Transmission => compare_transmission(bucket, &mut comparisons),
            Domain::Distribution => compare_distribution(bucket, &mut comparisons),
        }
    }

    let out = Comparisons { comparisons };
    write_json(&work.join(COMPARE_FILE), &out)?;
    Ok(out)
}

/// The largest bucket compared pairwise.
///
/// Every member is written into every other member's format and compared
/// against every other member, so the work is quadratic in the bucket size and
/// each leg re-parses. An archive holding hundreds of spellings of one case
/// would otherwise run for hours with no way to tell it apart from a hang.
/// Bigger buckets keep the first [`MAX_BUCKET_MEMBERS`] members and say so.
const MAX_BUCKET_MEMBERS: usize = 24;

/// The members of a bucket that take part in the pairwise comparison.
fn compared_members(bucket: &Bucket) -> &[Member] {
    let n = bucket.members.len().min(MAX_BUCKET_MEMBERS);
    &bucket.members[..n]
}

fn compare_transmission(bucket: &Bucket, out: &mut Vec<Comparison>) {
    // Re-read rather than trusting a cached copy: the corpus is the authority
    // and reading it again costs nothing next to the writes.
    let members = compared_members(bucket);
    let networks: Vec<Option<BalancedNetwork>> =
        members.iter().map(|m| balanced(&m.path)).collect();
    for (i, member) in members.iter().enumerate() {
        let Some(source) = networks[i].as_ref() else {
            continue;
        };
        for (j, other) in members.iter().enumerate() {
            let Some(target) = powerio_tx::format::parse_target_format(&other.format) else {
                continue;
            };
            let via = if i == j {
                Via::RoundTrip
            } else {
                Via::CrossWrite
            };
            out.push(convert_leg(bucket, member, other, via, source, target));
        }
        for (j, other) in members.iter().enumerate().skip(i + 1) {
            let Some(sibling) = networks[j].as_ref() else {
                continue;
            };
            // Two files in one bucket that state different limits are a case
            // and its derivative, and their differences are honest. They are
            // still compared — a reader defect hides just as well between a
            // case and its derivative — but the pair is labelled so the severity
            // says which kind of difference this is.
            let twin = fingerprint::same_case_data(source, sibling);
            let mut leg = sibling_leg(bucket, member, other, source, sibling);
            leg.different_case_data = !twin;
            out.push(leg);
        }
    }
}

/// The distribution half runs the two properties that mean anything for a
/// multiconductor model: element counts and the typed-model diff. There is no
/// `Y_bus` or per-bus injection here — a distribution line carries an
/// impedance matrix per phase pair, and powerio builds no admittance matrix
/// for it.
fn compare_distribution(bucket: &Bucket, out: &mut Vec<Comparison>) {
    let members = compared_members(bucket);
    let networks: Vec<Option<powerio_dist::MulticonductorNetwork>> =
        members.iter().map(|m| multiconductor(&m.path)).collect();
    for (i, member) in members.iter().enumerate() {
        let Some(source) = networks[i].as_ref() else {
            continue;
        };
        for (j, other) in members.iter().enumerate() {
            let Some(target) = powerio_dist::parse_dist_target_format(&other.format) else {
                continue;
            };
            let via = if i == j {
                Via::RoundTrip
            } else {
                Via::CrossWrite
            };
            out.push(dist_convert_leg(bucket, member, other, via, source, target));
        }
    }
}

fn dist_convert_leg(
    bucket: &Bucket,
    from: &Member,
    to: &Member,
    via: Via,
    source: &powerio_dist::MulticonductorNetwork,
    target: powerio_dist::DistTargetFormat,
) -> Comparison {
    let mut out = empty_comparison(
        bucket,
        Leg {
            from: from.format.clone(),
            to: to.format.clone(),
            via,
            from_ordinal: from.ordinal,
            to_ordinal: to.ordinal,
        },
    );
    let source_module = powerio_core::PioModule::new(source.clone());
    let emission = match catch_panic(|| {
        crate::module_io::emit_multiconductor_module(&source_module, target)
    }) {
        Ok(Ok(emission)) => emission,
        Ok(Err(error)) => {
            out.failure = Some(format!("emit: {error}"));
            return out;
        }
        Err(message) => {
            out.failure = Some(format!("emit panicked: {message}"));
            return out;
        }
    };
    out.warnings.extend(emission.render_diagnostics());
    let token = to.format.clone();
    let text = emission.text;
    // A deck that pulls in other files cannot be read back from a string:
    // `Redirect` and `Compile` resolve against a directory, and the corpus is
    // read-only, so there is nowhere to put the written master beside its
    // includes. Reading it anyway loses every object the includes carry and
    // reports it as a conversion loss, which is a statement about the harness
    // rather than about powerio.
    if has_include(&text) {
        out.unresolved_include = true;
        return out;
    }
    let parsed = match catch_panic(|| crate::module_io::load_multiconductor_memory(&text, &token)) {
        Ok(Ok(parsed)) => parsed,
        Ok(Err(err)) => {
            out.failure = Some(format!("readback: {err}"));
            return out;
        }
        Err(message) => {
            out.failure = Some(format!("readback panicked: {message}"));
            return out;
        }
    };
    // The readback's own declarations count toward warning parity, exactly as
    // the transmission leg counts them; without this a loss the reader states
    // on re-parse was graded undeclared.
    out.warnings
        .extend(powerio_core::render_diagnostics(&parsed.diagnostics));
    let before = invariants::distribution_core(source);
    let after = invariants::distribution_core(parsed.value());
    if before != after {
        out.core_changed = Some(dist_core_delta(&before, &after));
    }
    let clean = parsed.into_value();
    let diffs = invariants::model_diffs(
        &invariants::distribution_value(source),
        &invariants::distribution_value(&clean),
    );
    out.model_diffs = diffs
        .iter()
        .map(|d| anonymize::collapse_path(&d.path))
        .collect();
    out.model_diffs.sort();
    out.model_diffs.dedup();
    out
}

/// Whether a written OpenDSS deck pulls in files a string readback cannot
/// resolve. Shared by the pairwise compare and the walk, which must skip the
/// same decks for the same reason.
fn has_include(text: &str) -> bool {
    text.lines().any(|line| {
        let word = line.split_whitespace().next().unwrap_or("");
        word.eq_ignore_ascii_case("redirect")
            || word.eq_ignore_ascii_case("compile")
            // A `Buscoords` reference names a sidecar file the same way, so a
            // string readback would lose every location and report it as a
            // conversion loss that is really the harness's own limitation.
            || word.eq_ignore_ascii_case("buscoords")
    })
}

fn dist_core_delta(
    before: &invariants::DistributionCore,
    after: &invariants::DistributionCore,
) -> String {
    let mut parts = Vec::new();
    for (label, a, b) in [
        ("buses", before.buses, after.buses),
        ("loads", before.loads, after.loads),
        ("generators", before.generators, after.generators),
        ("shunts", before.shunts, after.shunts),
    ] {
        if a != b {
            parts.push(format!("{label} {}", count_delta(a, b)));
        }
    }
    if before.load_p != after.load_p {
        parts.push("load p".to_string());
    }
    if before.load_q != after.load_q {
        parts.push("load q".to_string());
    }
    parts.join(", ")
}

fn empty_comparison(bucket: &Bucket, leg: Leg) -> Comparison {
    Comparison {
        bucket: bucket.id.clone(),
        leg,
        warnings: Vec::new(),
        core_changed: None,
        ybus: None,
        ybus_unavailable: None,
        injection: None,
        injection_status_only: false,
        dc_terminal: None,
        model_diffs: Vec::new(),
        failure: None,
        different_case_data: false,
        unresolved_include: false,
        status_changed: 0,
    }
}

fn convert_leg(
    bucket: &Bucket,
    from: &Member,
    to: &Member,
    via: Via,
    source: &BalancedNetwork,
    target: TargetFormat,
) -> Comparison {
    let mut out = empty_comparison(
        bucket,
        Leg {
            from: from.format.clone(),
            to: to.format.clone(),
            via,
            from_ordinal: from.ordinal,
            to_ordinal: to.ordinal,
        },
    );
    let source_module = powerio_core::PioModule::new(source.clone());
    let emission = match catch_panic(|| {
        crate::module_io::emit_balanced_module(
            &source_module,
            target,
            &powerio_tx::EmitOptions::default(),
        )
    }) {
        Ok(Ok(emission)) => emission,
        Ok(Err(err)) => {
            out.failure = Some(format!("emit: {err}"));
            return out;
        }
        Err(message) => {
            out.failure = Some(format!("emit panicked: {message}"));
            return out;
        }
    };
    out.warnings.extend(emission.render_diagnostics());
    let token = to.format.clone();
    let text = emission.text;
    let parsed = match catch_panic(|| crate::module_io::load_balanced_memory(&text, &token)) {
        Ok(Ok(parsed)) => parsed,
        Ok(Err(err)) => {
            out.failure = Some(format!("readback: {err}"));
            return out;
        }
        Err(message) => {
            out.failure = Some(format!("readback panicked: {message}"));
            return out;
        }
    };
    out.warnings
        .extend(powerio_core::render_diagnostics(&parsed.diagnostics));
    fill_invariants(&mut out, source, parsed.value());
    out
}

fn sibling_leg(
    bucket: &Bucket,
    from: &Member,
    to: &Member,
    source: &BalancedNetwork,
    sibling: &BalancedNetwork,
) -> Comparison {
    let mut out = empty_comparison(
        bucket,
        Leg {
            from: from.format.clone(),
            to: to.format.clone(),
            via: Via::Sibling,
            from_ordinal: from.ordinal,
            to_ordinal: to.ordinal,
        },
    );
    // Two readers, one case: only the electrical invariants apply. Two formats
    // legitimately carry different fields, so a typed-model diff between them
    // says nothing.
    //
    // Service status is compared by count rather than through the injections,
    // because a machine that is out of service usually states zero output: the
    // injections agree while the two files disagree about what the operator
    // can dispatch. That difference is invisible to every other property here.
    out.status_changed = invariants::status_disagreements(source, sibling);
    let before = invariants::transmission_core(source);
    let after = invariants::transmission_core(sibling);
    if before != after {
        out.core_changed = Some(core_delta(&before, &after));
    }
    match invariants::ybus_change(source, sibling) {
        Ok(Some(change)) => out.ybus = Some(change.to_string()),
        Ok(None) => {}
        Err(side) => out.ybus_unavailable = Some(unavailable_label(&side)),
    }
    if let Some(change) = invariants::injection_change(source, sibling) {
        out.injection_status_only = change.status_only;
        out.injection = Some(change.to_string());
    }
    if let Some(change) = invariants::dc_terminal_change(source, sibling) {
        out.dc_terminal = Some(change.to_string());
    }
    out
}

fn fill_invariants(out: &mut Comparison, source: &BalancedNetwork, result: &BalancedNetwork) {
    let before = invariants::transmission_core(source);
    let after = invariants::transmission_core(result);
    if before != after {
        out.core_changed = Some(core_delta(&before, &after));
    }
    match invariants::ybus_change(source, result) {
        Ok(Some(change)) => out.ybus = Some(change.to_string()),
        Ok(None) => {}
        Err(side) => out.ybus_unavailable = Some(unavailable_label(&side)),
    }
    if let Some(change) = invariants::injection_change(source, result) {
        out.injection_status_only = change.status_only;
        out.injection = Some(change.to_string());
    }
    if let Some(change) = invariants::dc_terminal_change(source, result) {
        out.dc_terminal = Some(change.to_string());
    }
    let diffs = invariants::model_diffs(
        &invariants::transmission_value(source),
        &invariants::transmission_value(result),
    );
    out.model_diffs = diffs
        .iter()
        .map(|d| anonymize::collapse_path(&d.path))
        .collect();
    out.model_diffs.sort();
    out.model_diffs.dedup();
}

/// A signed count difference, stated without a cast that could wrap on a
/// corpus large enough to matter.
fn count_delta(before: usize, after: usize) -> String {
    if after >= before {
        format!("+{}", after - before)
    } else {
        format!("-{}", before - after)
    }
}

fn unavailable_label(side: &YbusUnavailable) -> String {
    match side {
        YbusUnavailable::Before => "source".to_string(),
        YbusUnavailable::After => "result".to_string(),
    }
}

/// Element-count and total deltas, never the absolute counts: how many loads a
/// utility runs is case data, how many a conversion lost is a defect.
/// Whether a [`core_delta`] string reports power moving rather than elements
/// being regrouped.
///
/// Counts alone moving is a merge or a split, which formats do legitimately
/// (MATPOWER states one bus demand where PSS/E states three loads). Power
/// moving is a loss. The labels live in `core_delta` and `dist_core_delta`
/// just above, so the test belongs beside them: read off the delta in two
/// places, it is one rule that a fifth power-bearing field would silently
/// break in whichever site nobody updated.
pub(super) fn power_moved(core: &str) -> bool {
    ["load p", "load q", "gen p", "base mva"]
        .iter()
        .any(|label| core.contains(label))
}

fn core_delta(
    before: &invariants::TransmissionCore,
    after: &invariants::TransmissionCore,
) -> String {
    let mut parts = Vec::new();
    let mut delta = |label: &str, a: usize, b: usize| {
        if a != b {
            parts.push(format!("{label} {}", count_delta(a, b)));
        }
    };
    delta("buses", before.buses, after.buses);
    delta("branches", before.branches, after.branches);
    delta("generators", before.generators, after.generators);
    delta("loads", before.loads, after.loads);
    delta("shunts", before.shunts, after.shunts);
    if before.load_p != after.load_p {
        parts.push("load p".to_string());
    }
    if before.load_q != after.load_q {
        parts.push("load q".to_string());
    }
    if before.gen_p != after.gen_p {
        parts.push("gen p".to_string());
    }
    if before.base_mva != after.base_mva {
        parts.push("base mva".to_string());
    }
    parts.join(", ")
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let text = serde_json::to_string_pretty(value)?;
    std::fs::write(path, text).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let text = std::fs::read_to_string(path).with_context(|| {
        format!(
            "read {} (run `powerio corpus ingest` first)",
            path.display()
        )
    })?;
    Ok(serde_json::from_str(&text)?)
}

/// Sanitize the comparisons into findings and write them out.
///
/// # Errors
///
/// Fails when the work directory is incomplete, when the output cannot be
/// written, or when the emitted report fails its own leak audit.
pub fn report(work: &Path, findings_path: &Path, summary_path: Option<&Path>) -> Result<usize> {
    let ingest: Ingest = read_json(&work.join(INGEST_FILE))?;
    // Either analysis feeds the report on its own: a run may compare, walk, or
    // both. Only both missing means there is nothing to report yet.
    let comparisons: Option<Comparisons> = read_json(&work.join(COMPARE_FILE)).ok();
    let walks: Option<walk::Walks> = read_json(&work.join(walk::WALK_FILE)).ok();
    if comparisons.is_none() && walks.is_none() {
        anyhow::bail!(
            "nothing to report in {}: run `powerio corpus compare` or `powerio corpus walk` first",
            work.display()
        );
    }
    let comparisons = comparisons.unwrap_or(Comparisons {
        comparisons: Vec::new(),
    });

    let mut sanitizer = Sanitizer::new();
    // The report's own schema. Without this a deck stating `delta` for a
    // winding makes the harness's `delta` key look like a leak, and the run
    // stops on its own field names.
    for key in SCHEMA_KEYS {
        sanitizer.allow(key);
    }
    for bucket in &ingest.buckets {
        sanitizer.allow(&bucket.id);
        for member in &bucket.members {
            sanitizer.allow(&member.format);
            sanitizer.learn_path(&member.path);
            match read_case(&member.path) {
                Ok(Case::Balanced(net, _)) => {
                    sanitizer.learn_buses(net.buses().iter().map(|b| b.id.0));
                    sanitizer.learn_network(&serde_json::to_value(&*net)?);
                }
                Ok(Case::Multiconductor(net, _)) => {
                    sanitizer.learn_network(&serde_json::to_value(&net)?);
                }
                Err(_) => {}
            }
        }
    }
    for bad in &ingest.unreadable {
        sanitizer.learn_path(&bad.path);
    }

    let mut findings = Vec::new();
    for bad in &ingest.unreadable {
        findings.push(Finding {
            bucket: None,
            code: if bad.panicked {
                "parse.panic"
            } else {
                "parse.failure"
            },
            severity: if bad.panicked { "crash" } else { "declared" },
            leg: None,
            detail: serde_json::json!({ "message": sanitizer.template(&bad.error) }),
        });
    }
    for comparison in &comparisons.comparisons {
        findings.extend(findings_for(comparison, &sanitizer));
    }
    if let Some(walks) = &walks {
        for w in &walks.walks {
            findings.extend(walk_findings(w, &sanitizer));
        }
    }

    let jsonl = render_jsonl(&findings)?;
    audit_or_bail(&sanitizer, &jsonl, findings_path)?;
    std::fs::write(findings_path, &jsonl)
        .with_context(|| format!("write {}", findings_path.display()))?;

    if let Some(path) = summary_path {
        let summary = render_summary(&ingest, &findings);
        audit_or_bail(&sanitizer, &summary, path)?;
        std::fs::write(path, summary).with_context(|| format!("write {}", path.display()))?;
    }
    Ok(findings.len())
}

fn audit_or_bail(sanitizer: &Sanitizer, text: &str, path: &Path) -> Result<()> {
    if let Err(leaks) = sanitizer.audit(text) {
        // Names neither the string nor the file it came from: this message is
        // as public as the report would have been.
        anyhow::bail!(
            "refusing to write {}: {} corpus string(s) reached it, first at {}",
            path.display(),
            leaks.len(),
            leaks[0]
        );
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bucket: Option<String>,
    pub code: &'static str,
    pub severity: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub leg: Option<Leg>,
    pub detail: serde_json::Value,
}

// One arm per finding class; splitting it would scatter the severity rules
// that only make sense read together.
#[allow(clippy::too_many_lines)]
fn findings_for(comparison: &Comparison, sanitizer: &Sanitizer) -> Vec<Finding> {
    let mut out = Vec::new();
    let bucket = Some(comparison.bucket.clone());
    let leg = Some(comparison.leg.clone());
    let mut push = |code: &'static str, severity: &'static str, detail: serde_json::Value| {
        out.push(Finding {
            bucket: bucket.clone(),
            code,
            severity,
            leg: leg.clone(),
            detail,
        });
    };

    if comparison.unresolved_include {
        push(
            "leg.unresolved-include",
            "declared",
            serde_json::json!({ "compared": false }),
        );
        return out;
    }
    if comparison.different_case_data {
        push(
            "sibling.different-data",
            "declared",
            serde_json::json!({ "compared": true }),
        );
    }
    if let Some(failure) = &comparison.failure {
        let panicked = failure.contains("panicked");
        push(
            if panicked { "leg.panic" } else { "leg.failure" },
            if panicked { "crash" } else { "silent-drop" },
            serde_json::json!({ "message": sanitizer.template(failure) }),
        );
    }
    if let Some(side) = &comparison.ybus_unavailable {
        push(
            "electrical.unavailable",
            "silent-value-change",
            serde_json::json!({ "side": side }),
        );
    }
    if comparison.ybus.is_some() {
        let honest_difference = comparison.leg.via == Via::Sibling
            && (comparison.different_case_data || comparison.status_changed > 0);
        // The changed entry's bus pair and magnitudes stay in the work
        // directory: a `Y_bus` entry is grid data. The finding states that the
        // admittance moved on this leg, which is what makes it actionable.
        push(
            if honest_difference {
                "sibling.ybus"
            } else {
                "electrical.ybus"
            },
            if honest_difference {
                "declared"
            } else {
                "silent-value-change"
            },
            serde_json::json!({ "entries_changed": 1 }),
        );
    }
    if comparison.injection.is_some() {
        // On a conversion leg powerio moved the power itself, so any move is a
        // defect. On a sibling leg the two files may honestly hold different
        // operating points — a base case and its contingency case sit side
        // by side in every planning archive — so a move that is only a status
        // disagreement is reported for triage rather than as a defect.
        let honest_difference = comparison.leg.via == Via::Sibling
            && (comparison.injection_status_only || comparison.different_case_data);
        push(
            if honest_difference {
                "sibling.status"
            } else {
                "electrical.injection"
            },
            if honest_difference {
                "declared"
            } else {
                "silent-value-change"
            },
            serde_json::json!({ "status_only": comparison.injection_status_only }),
        );
    }
    if comparison.dc_terminal.is_some() {
        let declared = !comparison.warnings.is_empty();
        push(
            "electrical.dc-terminal",
            if declared {
                "declared"
            } else {
                "silent-value-change"
            },
            serde_json::json!({ "declared": declared }),
        );
    }
    if let Some(core) = &comparison.core_changed {
        let power_moved = power_moved(core);
        push(
            if power_moved {
                "core.power"
            } else {
                "core.regrouped"
            },
            if power_moved {
                "silent-drop"
            } else {
                "declared"
            },
            serde_json::json!({ "delta": core }),
        );
    }
    if comparison.status_changed > 0 {
        push(
            "sibling.status",
            "declared",
            serde_json::json!({ "elements": comparison.status_changed }),
        );
    }
    if !comparison.model_diffs.is_empty() {
        // A serde path is mostly powerio's own field names, but an `Extras`
        // map contributes whatever token the source stated, so the path is
        // templated like any other text the corpus touched.
        let paths: Vec<String> = comparison
            .model_diffs
            .iter()
            .map(|p| sanitizer.template(p))
            .collect();
        let declared = !comparison.warnings.is_empty();
        push(
            if declared {
                "loss.declared"
            } else {
                "loss.undeclared"
            },
            if declared {
                "declared"
            } else {
                "undeclared-loss"
            },
            serde_json::json!({ "paths": paths }),
        );
    }
    let mut templates: BTreeMap<String, usize> = BTreeMap::new();
    for warning in &comparison.warnings {
        *templates.entry(sanitizer.template(warning)).or_default() += 1;
    }
    if !templates.is_empty() {
        push(
            "warning.observed",
            "declared",
            serde_json::json!({ "templates": templates }),
        );
    }
    out
}

/// Findings for one walk.
///
/// Only the properties a chain can state are reported here. A hop's own loss
/// is the same quantity [`findings_for`] already grades, and re-reporting it
/// per hop would bury the three findings that are new under a copy of the
/// pairwise report.
///
/// Every finding carries the path that produced it, as format tokens, and the
/// seed that replays it. Both are harness vocabulary rather than case data.
fn walk_findings(w: &walk::Walk, sanitizer: &Sanitizer) -> Vec<Finding> {
    let mut out = Vec::new();
    let path: Vec<String> = w.hops.iter().map(|h| h.to.clone()).collect();
    for (index, hop) in w.hops.iter().enumerate() {
        let leg = Some(Leg {
            from: if index == 0 {
                w.origin.clone()
            } else {
                w.hops[index - 1].to.clone()
            },
            to: hop.to.clone(),
            via: Via::Walk,
            from_ordinal: index,
            to_ordinal: index + 1,
        });
        let mut push =
            |code: &'static str, severity: &'static str, mut detail: serde_json::Value| {
                if let Some(map) = detail.as_object_mut() {
                    map.insert("origin".into(), serde_json::json!(w.origin));
                    map.insert("path".into(), serde_json::json!(path));
                    map.insert("hop".into(), serde_json::json!(index));
                    map.insert("seed".into(), serde_json::json!(w.seed.to_string()));
                }
                out.push(Finding {
                    bucket: Some(w.bucket.clone()),
                    code,
                    severity,
                    leg: leg.clone(),
                    detail,
                });
            };

        if let Some(failure) = &hop.failure {
            let panicked = failure.contains("panicked");
            push(
                if panicked {
                    "walk.panic"
                } else {
                    "walk.failure"
                },
                if panicked { "crash" } else { "silent-drop" },
                serde_json::json!({ "message": sanitizer.template(failure) }),
            );
        }
        if let Some(drift) = &hop.drift {
            // Conversion should settle. A second pass through a format that
            // lands somewhere else means the reader and writer are not a
            // projection, and every later hop inherits the drift. Drift in
            // retained extras alone is churn worth seeing; drift in an
            // electrical property is a defect.
            push(
                "walk.drift",
                if hop.drift_electrical {
                    "silent-value-change"
                } else {
                    "declared"
                },
                serde_json::json!({
                    "revisit_of": hop.revisit_of,
                    "delta": sanitizer.template(drift),
                }),
            );
        }
        if let Some(delta) = &hop.path_dependent {
            // The destination format is the same and the answer is not, so the
            // route through the graph changed the result. No single leg can
            // state this, which is the whole reason walks exist. Retention
            // honestly thins along a chain, so extras-only path dependence is
            // reported for triage rather than as a defect.
            push(
                "walk.path-dependent",
                if hop.path_dependent_electrical {
                    "silent-value-change"
                } else {
                    "declared"
                },
                serde_json::json!({ "direct": sanitizer.template(delta) }),
            );
        }
        if !hop.resurrected.is_empty() {
            // A table was empty and is not. When the hop's own electrical
            // properties held, the rows are a regrouping the target format
            // demands (pandapower states transformer charging as bus shunts);
            // when they moved too, a writer invented data no reader gave it.
            push(
                "walk.resurrection",
                if hop.leg.electrical() {
                    "silent-value-change"
                } else {
                    "declared"
                },
                serde_json::json!({ "resurrected": hop.resurrected }),
            );
        }
        if !hop.emptied.is_empty() {
            // Not a defect. It says the hops after this one are graded against
            // an empty table, so their silence is not evidence.
            push(
                "walk.absorbed",
                "declared",
                serde_json::json!({ "emptied": hop.emptied }),
            );
        }
    }
    out
}

fn render_jsonl(findings: &[Finding]) -> Result<String> {
    let mut out = String::new();
    for finding in findings {
        out.push_str(&serde_json::to_string(finding)?);
        out.push('\n');
    }
    Ok(out)
}

fn render_summary(ingest: &Ingest, findings: &[Finding]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "# Corpus run\n");
    let _ = writeln!(
        out,
        "{} files seen, {} buckets, {} unreadable.\n",
        ingest.files_seen,
        ingest.buckets.len(),
        ingest.unreadable.len()
    );
    let mut sizes: BTreeMap<usize, usize> = BTreeMap::new();
    for bucket in &ingest.buckets {
        *sizes.entry(bucket.members.len()).or_default() += 1;
    }
    let _ = writeln!(out, "## Buckets by sibling count\n");
    let _ = writeln!(out, "| siblings | buckets |");
    let _ = writeln!(out, "| --- | --- |");
    for (size, count) in &sizes {
        let _ = writeln!(out, "| {size} | {count} |");
    }
    let mut by_code: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for finding in findings {
        *by_code.entry((finding.severity, finding.code)).or_default() += 1;
    }
    let _ = writeln!(out, "\n## Findings\n");
    let _ = writeln!(out, "| severity | code | count |");
    let _ = writeln!(out, "| --- | --- | --- |");
    for ((severity, code), count) in &by_code {
        let _ = writeln!(out, "| {severity} | {code} | {count} |");
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::Path;

    use super::{Case, read_case};

    /// Every serde key of `value` outside maps whose keys come from case data.
    fn model_keys(value: &serde_json::Value, in_case_map: bool, out: &mut BTreeSet<String>) {
        match value {
            serde_json::Value::Array(xs) => {
                for x in xs {
                    model_keys(x, in_case_map, out);
                }
            }
            serde_json::Value::Object(xs) => {
                for (key, x) in xs {
                    if !in_case_map {
                        out.insert(key.clone());
                    }
                    model_keys(
                        x,
                        in_case_map || matches!(key.as_str(), "extras" | "properties"),
                        out,
                    );
                }
            }
            _ => {}
        }
    }

    /// The vocabulary's completeness gate: parse every fixture the readers
    /// accept, union their serde keys, and require every one to be
    /// vocabulary. Keys in `extras` and `properties` maps come from case data
    /// and remain subject to anonymization.
    #[test]
    fn every_fixture_field_name_is_vocabulary() {
        let data = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("tests/data");
        let mut keys = BTreeSet::new();
        let mut parsed = 0usize;
        for entry in walkdir::WalkDir::new(&data)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file())
        {
            let Ok(case) = read_case(entry.path()) else {
                continue;
            };
            let value = match case {
                Case::Balanced(net, _) => serde_json::to_value(&*net),
                Case::Multiconductor(net, _) => serde_json::to_value(&*net),
            }
            .expect("a parsed fixture serializes");
            model_keys(&value, false, &mut keys);
            parsed += 1;
        }
        assert!(parsed >= 18, "only {parsed} fixtures parsed");
        let vocabulary = super::anonymize::vocabulary();
        let missing: Vec<&String> = keys.iter().filter(|k| !vocabulary.contains(*k)).collect();
        assert!(
            missing.is_empty(),
            "{} fixture field name(s) outside the vocabulary would mask as corpus secrets: {missing:?}",
            missing.len()
        );
    }
}
