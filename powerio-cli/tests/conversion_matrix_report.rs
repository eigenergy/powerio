//! The conversion matrix: every source format written as every target and
//! read back. This file is the CI gate and writes the PR comment.
//!
//! # What a cell asserts
//!
//! A cell writes each case of its source row as the target and reads the
//! output back. Every finding on those two legs has a fidelity level, taken
//! from the last segment of its code in [`warning_level`], and the cell shows
//! the worst one:
//!
//! - exact: the diagonal only; the parsed module re-emits its retained source
//!   byte for byte, and the fresh write is lossless.
//! - lossless: no warning and no change to the typed model.
//! - restated: the target required a value the source does not state, and the
//!   writer supplied it (a default, a remapped name or rating set, a
//!   relabeled element). Any remark lands here too.
//! - approximate: a stated value was changed to fit the target.
//! - dropped: source data has no target field.
//! - failed: an error, a write or read failure, an invariant failure, or a
//!   change against the baseline.
//!
//! A warning whose code has no level fails the test, so a new code must be
//! classified before it can land. Source parse findings are the same for
//! every cell of a row, so they are graded once, in the Source column.
//!
//! The cell also holds these properties:
//!
//! 1. Its level and finding codes equal its line in
//!    `conversion_matrix.baseline`. `POWERIO_MATRIX_BLESS=1` rewrites that
//!    file from the current run.
//! 2. Element counts and total load and generation survive the explicitly
//!    described target projection. All nine distribution fixtures run in
//!    every cell, including the ten-conductor case.
//! 3. For transmission, the admittance matrix, entry for entry by bus id pair,
//!    and the per bus injections survive. Three allowances, each a property of the target
//!    format and stated at its own site: a format that names buses rather
//!    than numbering them is compared up to relabeling
//!    (`electrical_change_up_to_relabeling`), a fixed width format is held
//!    to the digits its fields carry (`electrical_tolerance`), and a format
//!    with no record for an element is compared against the network it can
//!    hold (`network_a_target_holds`).
//! 4. Distribution checks independently compare conductor-keyed line R/X/G/B,
//!    switch poles, load/generator P/Q branches, shunt G/B, and transformer
//!    winding ratios and ohmic R/X. Source constraints are checked separately.
//!    Exact fixture/field projections require named loss diagnostics and
//!    their exact recovered values. An ambiguous fixture transformer is
//!    explicitly marked unverified, component by component. Neither warning
//!    severity nor unrelated metadata losses waive these checks.
//! 5. Typed-model differences are always shown with fixture identity, without
//!    the corpus reporter's truncation. A fixture with changes but no loss
//!    finding makes its cell failed. The baseline records such a cell, so it
//!    stays visibly red until its writer reports or carries the data. These
//!    finite physical checks are not a proof of universal equivalence.
//!
//! # Making a cell better
//!
//! 1. Carry the data. Most gains were reader and writer asymmetries: a writer
//!    refusing a block its own reader parses. If the reader reads a field, the
//!    writer can almost always state it.
//! 2. Stop retaining restatements. A reader that keeps what PowerIO's own
//!    writer synthesizes (positional ids, zero component splits, default
//!    converter tails) makes every later hop drop data that never said
//!    anything.
//! 3. Report only losses. Absent source data is not a drop.
//!
//! # Gotchas
//!
//! - A source row's payloads are the MATPOWER cases written into that format
//!   first, and that leg is not graded. A fidelity fix upstream therefore
//!   moves other rows: rows pay for the data they now carry.
//! - Element order is not identity. pandapower regroups lines and
//!   transformers and several formats sort by id; the model comparison sorts
//!   by identity key.
//! - `charging: None` and the symmetric split are one fact, as are a rating
//!   in MVA and the same rating in amps through the file's own voltage.
//! - Formats state solved values in different places: pandapower puts
//!   reactive output in `res_gen`, and PSS/E states a DC line's received
//!   power through `SETVL`, `RDC`, and `VSCHD`.
//!
//! One parse only input is not run. The only vendored PowerWorld `.pwb`
//! export states no located bus type, so its network has no reference bus,
//! and GridFM requires exactly one.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use powerio::BalancedNetwork;
use powerio_cli::invariants::{
    DistributionCore, YbusUnavailable, all_model_diffs, distribution_core, distribution_value,
    electrical_change_up_to_relabeling, injection_change_within, model_diffs, transmission_core,
    transmission_value, ybus_change_within,
};
use powerio_core::{Diagnostic, DiagnosticSeverity};
use powerio_dist::{DistTargetFormat, MulticonductorNetwork};

#[path = "../../powerio-dist/tests/helpers/electrical_projection.rs"]
mod electrical_projection;
#[path = "../../powerio-dist/tests/helpers/source_equivalents.rs"]
mod source_equivalents;

const REPORT_ENV: &str = "POWERIO_CONVERSION_MATRIX_REPORT";
const DETAILS_ENV: &str = "POWERIO_CONVERSION_MATRIX_DETAILS";
const BLESS_ENV: &str = "POWERIO_MATRIX_BLESS";
const REPORT_MARKER: &str = "<!-- powerio-conversion-matrix-report -->";
const DETAILS_MARKER: &str = "<!-- powerio-conversion-matrix-details -->";
const BASELINE_FILE: &str = "tests/conversion_matrix.baseline";
static DSS_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

const LEGEND: [&str; 4] = [
    "Each cell writes the source as the target and reads it back. The symbol is the worst finding.",
    "⚪ exact  🟢 lossless  🔵 restated: only values the target requires were synthesized",
    "🟡 approximate: values changed to fit  🟠 dropped: source data has no target field",
    "🔴 failed: error, invariant, or baseline change",
];

/// How faithfully a conversion kept what the source stated, best first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Level {
    Exact,
    Lossless,
    Restated,
    Approximate,
    Dropped,
    Failed,
}

impl Level {
    fn symbol(self) -> &'static str {
        match self {
            Self::Exact => "⚪",
            Self::Lossless => "🟢",
            Self::Restated => "🔵",
            Self::Approximate => "🟡",
            Self::Dropped => "🟠",
            Self::Failed => "🔴",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Lossless => "lossless",
            Self::Restated => "restated",
            Self::Approximate => "approximate",
            Self::Dropped => "dropped",
            Self::Failed => "failed",
        }
    }
}

/// Where in a cell a finding was reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Leg {
    /// Parsing the source payload. Shared by every cell of a row.
    Source,
    /// Writing the target.
    Write,
    /// Reading the target output back.
    Readback,
}

impl Leg {
    fn name(self) -> &'static str {
        match self {
            Self::Source => "source parse",
            Self::Write => "target write",
            Self::Readback => "target readback",
        }
    }
}

/// The level and kind a warning's code states, or `None` when the code has
/// no level yet. The level comes from the last code segment, which every
/// family names the same way.
fn warning_level(segment: &str, leg: Leg) -> Option<(Level, String)> {
    let (level, kind) = match segment {
        // A value the target requires and the source does not state is
        // synthesized on write. Read back, a defaulted value is one the file
        // did not state, which changes what the model says.
        "VALUE_DEFAULTED" if leg == Leg::Write => (Level::Restated, "value"),
        "NAME_REMAPPED" => (Level::Restated, "name"),
        "RATING_SET_REMAPPED" => (Level::Restated, "rating set"),
        "ELEMENT_RELABELED" => (Level::Restated, "element"),
        "VALUE_DEFAULTED" | "VALUE_SUBSTITUTED" | "VALUE_APPROXIMATED" | "VALUE_TRUNCATED"
        | "VALUE_COLLAPSED" | "VALUE_INFERRED" | "VALUE_UNSUPPORTED" => {
            (Level::Approximate, "value")
        }
        "NOT_A_NUMBER" => (Level::Approximate, "nonfinite"),
        "REFERENCE_MISSING" => (Level::Approximate, "reference"),
        "SECTION_UNSUPPORTED" => (Level::Dropped, "section"),
        "TABLE_UNSUPPORTED" => (Level::Dropped, "table"),
        "RECORD_IGNORED" => (Level::Dropped, "record"),
        "RETAINED_SOURCE_ONLY" => (Level::Dropped, "source only"),
        "DOWNGRADED" => (Level::Dropped, "downgraded"),
        "EXPERIMENTAL_LOSS" => (Level::Dropped, "experimental"),
        "TRANSFORMER_UNSUPPORTED" => (Level::Dropped, "transformer"),
        // The sources are all written, but the BMOPF formulation reads only
        // one of them.
        // An unbalanced source is written as one single phase source per
        // conductor: the same terminal phasors, more source records.
        "SOURCE_COUNT" | "SOURCE_SPLIT" => (Level::Approximate, "source"),
        // A three phase wye-wye transformer becomes one BMOPF unit per
        // phase: the same windings, more transformer records.
        "TRANSFORMER_WYE_WYE_DECOMPOSED" => (Level::Approximate, "transformer"),
        // Untyped source objects pass through the neutral reduction
        // unchanged; they stay source only.
        "KRON_UNTYPED_RETAINED" => (Level::Dropped, "untyped"),
        _ => {
            let stem = segment
                .strip_suffix("_DROPPED")
                .or_else(|| segment.strip_suffix("_UNMAPPED"))?;
            return Some((Level::Dropped, kind_words(stem)));
        }
    };
    Some((level, kind.to_owned()))
}

fn kind_words(stem: &str) -> String {
    stem.to_ascii_lowercase().replace('_', " ")
}

/// A finding's level and kind. `Ok(None)` for a note, which adds context to
/// another finding and is not graded; `Err` names a warning code with no
/// level.
fn grade_finding(leg: Leg, diagnostic: &Diagnostic) -> Result<Option<(Level, String)>, String> {
    let code = diagnostic.code();
    let segment = code.rsplit('.').next().unwrap_or(code);
    match diagnostic.severity() {
        DiagnosticSeverity::Note => Ok(None),
        DiagnosticSeverity::Error => Ok(Some((Level::Failed, "error".to_owned()))),
        DiagnosticSeverity::Remark => Ok(Some((
            Level::Restated,
            warning_level(segment, leg).map_or_else(|| kind_words(segment), |(_, kind)| kind),
        ))),
        DiagnosticSeverity::Warning => warning_level(segment, leg)
            .map(Some)
            .ok_or_else(|| code.to_owned()),
    }
}

#[test]
fn every_level_has_a_finding_shape() {
    let leveled = |segment| warning_level(segment, Leg::Write).map(|(level, _)| level);
    assert_eq!(leveled("VALUE_DEFAULTED"), Some(Level::Restated));
    assert_eq!(
        warning_level("VALUE_DEFAULTED", Leg::Readback).map(|(level, _)| level),
        Some(Level::Approximate)
    );
    assert_eq!(leveled("VALUE_SUBSTITUTED"), Some(Level::Approximate));
    assert_eq!(
        warning_level("RATING_SET_DROPPED", Leg::Write),
        Some((Level::Dropped, "rating set".to_owned()))
    );
    assert_eq!(leveled("FIELD_UNMAPPED"), Some(Level::Dropped));
    assert_eq!(leveled("SOURCE_MALFORMED"), None);
}

/// One source and target pair, or a row's Source column.
#[derive(Default)]
struct Cell {
    findings: Vec<(&'static str, Leg, Diagnostic)>,
    cases: BTreeSet<&'static str>,
    /// Write and read failures and invariant failures.
    failures: Vec<String>,
    /// Typed model fields the conversion changed.
    model_diffs: Vec<(&'static str, String)>,
    /// Narrow field equivalences; the original measured differences stay visible.
    model_restatements: BTreeMap<(&'static str, String), &'static str>,
    /// Explicit physical projections and component-scoped unavailable checks.
    verification_notes: Vec<(&'static str, String)>,
    /// On the diagonal, whether every payload re-emitted its retained source
    /// byte for byte.
    echo_exact: Option<bool>,
}

/// What a cell's findings add up to, before the baseline comparison.
struct Grade {
    level: Level,
    /// Findings at `level`.
    count: usize,
    /// The most common kind at `level`.
    kind: String,
    /// Every graded finding's code and how often it appears.
    codes: BTreeMap<String, usize>,
    /// Warning codes with no level.
    unclassified: BTreeSet<String>,
    /// Findings at error severity.
    errors: usize,
    /// Their codes, for the report.
    error_codes: BTreeSet<String>,
    /// The typed model changed while no finding said so.
    unreported_change: bool,
}

impl Cell {
    fn record(&mut self, case: &'static str, leg: Leg, diagnostics: &[Diagnostic]) {
        self.cases.insert(case);
        self.findings
            .extend(diagnostics.iter().map(|d| (case, leg, d.clone())));
    }

    fn grade(&self) -> Grade {
        let mut graded = Vec::new();
        let mut codes = BTreeMap::new();
        let mut unclassified = BTreeSet::new();
        for (_, leg, diagnostic) in &self.findings {
            match grade_finding(*leg, diagnostic) {
                Ok(None) => continue,
                Ok(Some(found)) => graded.push(found),
                Err(code) => {
                    unclassified.insert(code);
                }
            }
            *codes.entry(diagnostic.code().to_owned()).or_insert(0) += 1;
        }
        let mut level = graded
            .iter()
            .map(|(level, _)| *level)
            .max()
            .unwrap_or(Level::Lossless);
        if !self.model_restatements.is_empty() {
            level = level.max(Level::Restated);
        }
        if !self.verification_notes.is_empty() {
            level = level.max(Level::Approximate);
        }
        if level == Level::Lossless && self.echo_exact == Some(true) {
            level = Level::Exact;
        }
        let errors = graded
            .iter()
            .filter(|(level, _)| *level == Level::Failed)
            .count();
        let error_codes: BTreeSet<String> = self
            .findings
            .iter()
            .filter(|(_, _, d)| d.severity() == DiagnosticSeverity::Error)
            .map(|(_, _, d)| d.code().to_owned())
            .collect();
        // One fixture's loss never authorizes a change in another fixture.
        // Independent electrical checks below also apply when a fixture has
        // metadata-loss findings of its own.
        let unreported_change = self.model_diffs.iter().any(|(case, difference)| {
            !self
                .model_restatements
                .contains_key(&(*case, difference.clone()))
                && !self.findings.iter().any(|(found_case, leg, finding)| {
                    found_case == case
                        && grade_finding(*leg, finding)
                            .ok()
                            .flatten()
                            .is_some_and(|(level, _)| level > Level::Restated)
                })
        });
        if unreported_change || !self.failures.is_empty() {
            level = Level::Failed;
        }
        let mut kinds = BTreeMap::<&str, usize>::new();
        for (_, kind) in graded.iter().filter(|(found, _)| *found == level) {
            *kinds.entry(kind).or_insert(0) += 1;
        }
        let count = kinds.values().sum();
        // The first most common kind in alphabetical order, so the label is
        // stable across runs.
        let kind = kinds
            .iter()
            .max_by(|(a, x), (b, y)| x.cmp(y).then_with(|| b.cmp(a)))
            .map_or_else(String::new, |(kind, _)| (*kind).to_owned());
        Grade {
            level,
            count,
            kind,
            codes,
            unclassified,
            errors,
            error_codes,
            unreported_change,
        }
    }
}

impl Grade {
    /// The cell's line in the baseline file, after its key.
    fn baseline_value(&self) -> String {
        let codes = if self.codes.is_empty() {
            "-".to_owned()
        } else {
            self.codes
                .iter()
                .map(|(code, count)| format!("{code}={count}"))
                .collect::<Vec<_>>()
                .join(" ")
        };
        format!("{} | {codes}", self.level.name())
    }
}

/// Every reason a cell fails the test, given its expected baseline value.
///
/// A model change that no finding reports makes the cell failed. The
/// baseline records such a cell as `failed`, so the cell shows red in the
/// report until the writer or reader reports or carries the data, and a new
/// one changes the baseline and fails the test. Every other failure fails the
/// test outright.
fn cell_problems(cell: &Cell, grade: &Grade, expected: Option<&str>) -> Vec<String> {
    let mut problems = cell.failures.clone();
    for code in &grade.unclassified {
        problems.push(format!(
            "{code} has no fidelity level; classify its last segment in `warning_level`"
        ));
    }
    if grade.errors > 0 {
        problems.push(format!(
            "{} finding(s) at error severity: {}",
            grade.errors,
            grade
                .error_codes
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let observed = grade.baseline_value();
    if expected != Some(observed.as_str()) {
        problems.push(format!(
            "baseline changed: expected `{}`, observed `{observed}`",
            expected.unwrap_or("<no line>")
        ));
    }
    problems
}

fn render_cell(grade: &Grade, problems: &[String]) -> String {
    if !problems.is_empty() {
        let only_baseline = problems.iter().all(|p| p.starts_with("baseline changed"));
        return if only_baseline {
            format!("🔴 baseline: now {}", grade.level.name())
        } else {
            format!("🔴 {} failure", problems.len())
        };
    }
    if grade.unreported_change {
        return "🔴 unreported change".to_owned();
    }
    match grade.level {
        Level::Exact | Level::Lossless => grade.level.symbol().to_owned(),
        level => format!("{} {} {}", level.symbol(), grade.count, grade.kind),
    }
}

/// One matrix: sources by targets, with the targets grouped into tables.
struct Matrix {
    sources: Vec<&'static str>,
    targets: Vec<&'static str>,
    /// Each entry a table title and the target columns it renders. Every
    /// table keeps every source row.
    groups: Vec<(&'static str, Vec<usize>)>,
    /// The Source column: source parse findings per row.
    source_cells: Vec<Cell>,
    cells: Vec<Vec<Cell>>,
}

fn source_key(source: &str) -> String {
    format!("{source} (source parse)")
}

fn pair_key(source: &str, target: &str) -> String {
    format!("{source} -> {target}")
}

/// A graded cell with its baseline verdict.
struct Judged {
    key: String,
    grade: Grade,
    problems: Vec<String>,
}

impl Matrix {
    fn keyed_cells(&self) -> Vec<(String, &Cell)> {
        let mut cells = Vec::new();
        for (row, source) in self.sources.iter().enumerate() {
            cells.push((source_key(source), &self.source_cells[row]));
            for (column, target) in self.targets.iter().enumerate() {
                cells.push((pair_key(source, target), &self.cells[row][column]));
            }
        }
        cells
    }
}

#[test]
fn conversion_matrix_report_matches_baseline() {
    let matrices = [
        ("Transmission", run_transmission_matrix()),
        ("Distribution", run_distribution_matrix()),
    ];
    let baseline_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(BASELINE_FILE);
    let observed: BTreeMap<String, String> = matrices
        .iter()
        .flat_map(|(_, matrix)| matrix.keyed_cells())
        .map(|(key, cell)| (key, cell.grade().baseline_value()))
        .collect();
    let bless = std::env::var_os(BLESS_ENV).is_some_and(|value| value == "1");
    if bless {
        std::fs::write(&baseline_path, render_baseline(&observed)).unwrap();
    }
    let expected = read_baseline(&baseline_path);

    let mut judged = BTreeMap::new();
    let mut failures = Vec::new();
    for (_, matrix) in &matrices {
        for (key, cell) in matrix.keyed_cells() {
            let grade = cell.grade();
            let problems = cell_problems(cell, &grade, expected.get(&key).map(String::as_str));
            for problem in &problems {
                failures.push(format!("{key}: {}", sanitize_report_text(problem)));
            }
            judged.insert(
                key.clone(),
                Judged {
                    key,
                    grade,
                    problems,
                },
            );
        }
    }
    for key in expected.keys().filter(|key| !observed.contains_key(*key)) {
        failures.push(format!(
            "{key}: the baseline names a cell the matrix no longer runs"
        ));
    }

    let report = render_report(&matrices, &judged);
    let details = render_details(&matrices, &judged);
    assert_no_report_path_leaks(&report);
    assert_no_report_path_leaks(&details);
    write_env_report(REPORT_ENV, &report);
    write_env_report(DETAILS_ENV, &details);
    println!("{report}");
    assert_eq!(
        failures,
        Vec::<String>::new(),
        "rerun with {BLESS_ENV}=1 to accept an intended baseline change\n\n{report}"
    );
}

fn render_baseline(observed: &BTreeMap<String, String>) -> String {
    let mut lines: Vec<String> = observed
        .iter()
        .map(|(key, value)| format!("{key} | {value}"))
        .collect();
    lines.sort();
    let mut text = format!(
        "# One line per conversion matrix cell. Regenerate with {BLESS_ENV}=1 cargo test -p powerio-cli --test conversion_matrix_report\n"
    );
    for line in lines {
        text.push_str(&line);
        text.push('\n');
    }
    text
}

fn read_baseline(path: &Path) -> BTreeMap<String, String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    text.lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.split_once(" | "))
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
}

fn render_report(matrices: &[(&str, Matrix)], judged: &BTreeMap<String, Judged>) -> String {
    let mut markdown = String::new();
    writeln!(markdown, "{REPORT_MARKER}").unwrap();
    writeln!(markdown).unwrap();
    writeln!(markdown, "## Conversion Matrix").unwrap();
    for line in LEGEND {
        writeln!(markdown).unwrap();
        writeln!(markdown, "{line}").unwrap();
    }
    for (title, matrix) in matrices {
        for (group, columns) in &matrix.groups {
            writeln!(markdown).unwrap();
            writeln!(markdown, "### {title}: {group}").unwrap();
            writeln!(markdown).unwrap();
            write!(markdown, "| From ↓ / to → | Source |").unwrap();
            for column in columns {
                write!(markdown, " {} |", matrix.targets[*column]).unwrap();
            }
            writeln!(markdown).unwrap();
            write!(markdown, "| --- | --- |").unwrap();
            for _ in columns {
                write!(markdown, " --- |").unwrap();
            }
            writeln!(markdown).unwrap();
            for source in &matrix.sources {
                let shown = |key: String| {
                    let judged = &judged[&key];
                    render_cell(&judged.grade, &judged.problems)
                };
                write!(markdown, "| {source} | {} |", shown(source_key(source))).unwrap();
                for column in columns {
                    let target = matrix.targets[*column];
                    write!(markdown, " {} |", shown(pair_key(source, target))).unwrap();
                }
                writeln!(markdown).unwrap();
            }
        }
    }
    markdown
}

/// The finding text behind each cell, for the workflow's artifact.
fn render_details(matrices: &[(&str, Matrix)], judged: &BTreeMap<String, Judged>) -> String {
    let mut markdown = String::new();
    writeln!(markdown, "{DETAILS_MARKER}").unwrap();
    writeln!(markdown).unwrap();
    writeln!(markdown, "## Conversion Matrix Findings").unwrap();
    for (title, matrix) in matrices {
        writeln!(markdown).unwrap();
        writeln!(markdown, "### {title}").unwrap();
        writeln!(markdown).unwrap();
        for (key, cell) in matrix.keyed_cells() {
            let judged = &judged[&key];
            if cell.findings.is_empty() && judged.problems.is_empty() && cell.model_diffs.is_empty()
            {
                continue;
            }
            writeln!(
                markdown,
                "- **{}** ({}, {} fixtures)",
                judged.key,
                judged.grade.level.name(),
                cell.cases.len()
            )
            .unwrap();
            for problem in &judged.problems {
                writeln!(
                    markdown,
                    "  - failure: {}",
                    one_line(&sanitize_report_text(problem))
                )
                .unwrap();
            }
            // Always expose measured changes, including cells with declared
            // losses. Otherwise an unrelated metadata warning hides numerical
            // corruption from the detailed artifact as well as the summary.
            for (case, note) in &cell.verification_notes {
                writeln!(
                    markdown,
                    "  - electrical verification [{case}]: {}",
                    one_line(note)
                )
                .unwrap();
            }
            for (case, difference) in &cell.model_diffs {
                writeln!(
                    markdown,
                    "  - model change [{case}]: {}{}",
                    one_line(&sanitize_report_text(difference)),
                    cell.model_restatements
                        .get(&(*case, difference.clone()))
                        .map_or_else(String::new, |reason| format!(
                            " (equivalent restatement: {reason})"
                        ))
                )
                .unwrap();
            }
            let mut texts = BTreeMap::<(String, String), usize>::new();
            for (case, leg, diagnostic) in &cell.findings {
                let level = grade_finding(*leg, diagnostic)
                    .ok()
                    .flatten()
                    .map_or("unclassified", |(level, _)| level.name());
                let text = format!(
                    "[{case}] {}: {}",
                    diagnostic.code(),
                    sanitize_report_text(diagnostic.message())
                );
                *texts
                    .entry((format!("{} {level}", leg.name()), text))
                    .or_insert(0) += 1;
            }
            let mut texts: Vec<_> = texts.into_iter().collect();
            texts.sort_by(|(a, x), (b, y)| y.cmp(x).then_with(|| a.cmp(b)));
            for ((leg, text), count) in &texts {
                writeln!(markdown, "  - {leg}: {count}x {}", one_line(text)).unwrap();
            }
        }
    }
    markdown
}

fn write_env_report(env: &str, markdown: &str) {
    let Ok(path) = std::env::var(env) else {
        return;
    };
    let path = PathBuf::from(path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, markdown).unwrap();
}

fn assert_no_report_path_leaks(markdown: &str) {
    let temp_dir = std::env::temp_dir().to_string_lossy().into_owned();
    assert!(
        !markdown.contains(&temp_dir),
        "report leaked temp dir: {temp_dir}"
    );
    assert!(
        !markdown.contains(env!("CARGO_MANIFEST_DIR")),
        "report leaked cargo manifest dir"
    );
    assert!(
        !markdown.contains("powerio-conversion-matrix/"),
        "report leaked generated temp file path"
    );
}

fn one_line(text: &str) -> String {
    text.replace('\n', " ")
}

fn sanitize_report_text(text: &str) -> String {
    const GENERATED_DSS_DIR: &str = "powerio-conversion-matrix/";
    if let Some(dir_idx) = text.find(GENERATED_DSS_DIR) {
        let prefix = &text[..dir_idx];
        let path_start = prefix
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_whitespace())
            .map_or(0, |(idx, c)| idx + c.len_utf8());
        let prelude = prefix[..path_start].trim_end();
        let suffix = &text[dir_idx + GENERATED_DSS_DIR.len()..];
        if prelude.is_empty() {
            return format!("generated DSS {suffix}");
        }
        return format!("{prelude} generated DSS {suffix}");
    }

    let temp_dir = std::env::temp_dir().to_string_lossy().into_owned();
    text.replace(&temp_dir, "<tmp>")
        .replace(env!("CARGO_MANIFEST_DIR"), "<crate>")
}

#[test]
fn sanitize_report_text_handles_multibyte_whitespace_before_generated_dss_dir() {
    // A non-breaking space (U+00A0, 2 UTF-8 bytes) immediately before the
    // generated DSS dir marker must not panic on a non char boundary slice.
    let text = "wrote\u{a0}/tmp/xyz/powerio-conversion-matrix/case.dss";
    let sanitized = sanitize_report_text(text);
    assert_eq!(sanitized, "wrote generated DSS case.dss");
}

struct TextEmission {
    text: String,
    /// The other artifacts of the emission (an OpenDSS bus coordinate file),
    /// by file name, which a readback of the text needs beside it.
    sidecars: Vec<(String, String)>,
    diagnostics: Vec<Diagnostic>,
}

fn text_emission(
    result: powerio_core::EmitResult,
    primary_suffix: Option<&str>,
) -> Result<TextEmission, String> {
    let diagnostics = result.diagnostics().to_vec();
    let powerio_core::EmittedOutput::Memory { artifacts } = result.into_output() else {
        return Err("memory destination returned path output".to_owned());
    };
    let mut texts = Vec::new();
    for artifact in artifacts {
        let name = artifact.name().as_str().to_owned();
        let text = String::from_utf8(artifact.into_bytes())
            .map_err(|err| format!("emitted text is not UTF-8: {err}"))?;
        texts.push((name, text));
    }
    // The case text. A same format OpenDSS echo of a project with sidecars
    // returns the retained files under `source/` behind a redirect stub at
    // the destination; the retained case file is the text then.
    let candidates: Vec<usize> = texts
        .iter()
        .enumerate()
        .filter(|(_, (name, _))| primary_suffix.is_none_or(|suffix| name.ends_with(suffix)))
        .map(|(i, _)| i)
        .collect();
    let chosen = match candidates.as_slice() {
        [one] => *one,
        [] => return Err("emission returned no primary artifact".to_owned()),
        several => *several
            .iter()
            .find(|i| texts[**i].0.contains("source/"))
            .ok_or_else(|| "emission returned several primary artifacts".to_owned())?,
    };
    let mut primary = None;
    let mut sidecars = Vec::new();
    for (i, (name, text)) in texts.into_iter().enumerate() {
        if i == chosen {
            primary = Some(text);
        } else {
            let file = name.rsplit('/').next().unwrap_or(&name).to_owned();
            sidecars.push((file, text));
        }
    }
    Ok(TextEmission {
        text: primary.ok_or_else(|| "emission returned no primary artifact".to_owned())?,
        sidecars,
        diagnostics,
    })
}

/// One emitted case, held where the format's reader takes it from.
enum Emitted {
    /// One document, read back from memory.
    Document(String),
    /// A directory of documents, read back from the temporary directory this
    /// value owns.
    Directory(tempfile::TempDir),
}

impl Emitted {
    /// Every emitted file's bytes by its path under the case.
    fn files(&self) -> Result<BTreeMap<String, Vec<u8>>, String> {
        match self {
            Self::Document(text) => Ok(BTreeMap::from([(String::new(), text.as_bytes().to_vec())])),
            Self::Directory(directory) => {
                let root = directory.path().join(DIRECTORY_CASE_NAME);
                let mut files = BTreeMap::new();
                collect_files(&root, &root, &mut files)?;
                Ok(files)
            }
        }
    }
}

fn collect_files(
    root: &Path,
    dir: &Path,
    files: &mut BTreeMap<String, Vec<u8>>,
) -> Result<(), String> {
    for entry in std::fs::read_dir(dir).map_err(|err| err.to_string())? {
        let path = entry.map_err(|err| err.to_string())?.path();
        if path.is_dir() {
            collect_files(root, &path, files)?;
        } else {
            let bytes = std::fs::read(&path).map_err(|err| err.to_string())?;
            let relative = path
                .strip_prefix(root)
                .map_err(|err| err.to_string())?
                .to_string_lossy()
                .into_owned();
            files.insert(relative, bytes);
        }
    }
    Ok(())
}

struct TransmissionEmission {
    emitted: Emitted,
    diagnostics: Vec<Diagnostic>,
}

/// Write a module as `format` through the facade, which is the operation a
/// caller has: one token selects the writer, and a directory format writes
/// its inventory rather than one document. A module that retains its source
/// echoes it when `format` is the source's own.
fn emit_transmission<T>(
    module: &powerio_core::PioModule<T>,
    format: TransmissionFormat,
) -> Result<TransmissionEmission, String>
where
    T: Clone + Into<powerio::PioValue>,
{
    if format.emission == Emission::Directory {
        let directory = tempfile::tempdir().map_err(|err| err.to_string())?;
        let result = powerio::emit(
            module,
            format.token,
            powerio_core::Destination::path(directory.path().join(DIRECTORY_CASE_NAME)),
        )
        .map_err(|err| err.to_string())?;
        return Ok(TransmissionEmission {
            emitted: Emitted::Directory(directory),
            diagnostics: result.diagnostics().to_vec(),
        });
    }
    let destination = powerio_core::Destination::memory("case").map_err(|err| err.to_string())?;
    let result = powerio::emit(module, format.token, destination).map_err(|err| err.to_string())?;
    let diagnostics = result.diagnostics().to_vec();
    let powerio_core::EmittedOutput::Memory { mut artifacts } = result.into_output() else {
        return Err("a memory destination returned path output".to_owned());
    };
    let artifact = artifacts
        .pop()
        .filter(|_| artifacts.is_empty())
        .ok_or_else(|| "a one document format returned several artifacts".to_owned())?;
    let text = String::from_utf8(artifact.into_bytes())
        .map_err(|err| format!("emitted text is not UTF-8: {err}"))?;
    Ok(TransmissionEmission {
        emitted: Emitted::Document(text),
        diagnostics,
    })
}

/// The directory name a directory format writes under, inside a temporary
/// directory that holds nothing else.
const DIRECTORY_CASE_NAME: &str = "case";

/// Read one emitted case back as its own format.
fn parse_transmission_emitted(
    emitted: &Emitted,
    format: TransmissionFormat,
) -> Result<ParsedTransmission, String> {
    match emitted {
        Emitted::Document(text) => {
            let source = powerio_core::Source::from_memory("<memory>", text.as_bytes().to_vec())
                .map_err(|err| err.to_string())?;
            parse_transmission_source(source, format.token)
        }
        Emitted::Directory(directory) => {
            let source = powerio_core::Source::open(directory.path().join(DIRECTORY_CASE_NAME))
                .map_err(|err| err.to_string())?;
            parse_transmission_source(source, format.token)
        }
    }
}

fn emit_distribution_module(
    module: &powerio_core::PioModule<MulticonductorNetwork>,
    target: DistTargetFormat,
) -> Result<TextEmission, String> {
    let destination = powerio_core::Destination::memory("case").map_err(|err| err.to_string())?;
    let result = powerio_dist::emit(module, target, destination).map_err(|err| err.to_string())?;
    let suffix = (target == DistTargetFormat::Dss).then_some(".dss");
    text_emission(result, suffix)
}

fn emit_distribution_value(
    network: &MulticonductorNetwork,
    target: DistTargetFormat,
) -> Result<TextEmission, String> {
    emit_distribution_module(&powerio_core::PioModule::new(network.clone()), target)
}

/// Which table a target column renders in. One table of every source against
/// every target is too wide for a PR comment, so the columns are grouped by
/// what the formats are for and every group keeps every source row.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Family {
    /// Balanced case formats: one document stating one network.
    Case,
    /// The PowSybl and ENTSO-E exchange formats, which state absolute units.
    Exchange,
    /// Dataset directories.
    Dataset,
}

impl Family {
    const ALL: [Self; 3] = [Self::Case, Self::Exchange, Self::Dataset];

    fn title(self) -> &'static str {
        match self {
            Self::Case => "into a case format",
            Self::Exchange => "into an exchange format",
            Self::Dataset => "into a dataset directory",
        }
    }
}

/// How a format's output is produced and read back.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Emission {
    /// One document, emitted and read back in memory.
    Document,
    /// A directory of documents whose identity is the directory, so it is
    /// emitted into and read back from a temporary directory.
    Directory,
    /// A read only format: a source row and no target column.
    ReadOnly,
}

#[derive(Clone, Copy)]
struct TransmissionFormat {
    name: &'static str,
    token: &'static str,
    emission: Emission,
    family: Family,
    /// The vendored case a read only source parses. Every other source's
    /// payloads are the MATPOWER cases written into that format first.
    fixture: Option<&'static str>,
}

const fn transmission(
    name: &'static str,
    token: &'static str,
    emission: Emission,
    family: Family,
) -> TransmissionFormat {
    TransmissionFormat {
        name,
        token,
        emission,
        family,
        fixture: None,
    }
}

const fn read_only(
    name: &'static str,
    token: &'static str,
    family: Family,
    fixture: &'static str,
) -> TransmissionFormat {
    TransmissionFormat {
        name,
        token,
        emission: Emission::ReadOnly,
        family,
        fixture: Some(fixture),
    }
}

/// Every 0.11 transmission format. The writable ones are both source rows and
/// target columns, in this order; the read only ones are source rows only and
/// come last, so a target column index indexes this array directly.
const TRANSMISSION_FORMATS: [TransmissionFormat; 20] = [
    transmission("MATPOWER .m", "matpower", Emission::Document, Family::Case),
    transmission(
        "PowerModels JSON",
        "powermodels-json",
        Emission::Document,
        Family::Case,
    ),
    transmission("PSS/E .raw 33", "psse", Emission::Document, Family::Case),
    transmission(
        "PSS/E RAWX 35",
        "psse-rawx",
        Emission::Document,
        Family::Case,
    ),
    transmission(
        "PowerWorld .aux",
        "powerworld",
        Emission::Document,
        Family::Case,
    ),
    transmission("egret JSON", "egret-json", Emission::Document, Family::Case),
    transmission(
        "pandapower JSON",
        "pandapower-json",
        Emission::Document,
        Family::Case,
    ),
    transmission("Surge JSON", "surge-json", Emission::Document, Family::Case),
    transmission(
        "MG-RAVENS JSON (balanced)",
        "ravens-json",
        Emission::Document,
        Family::Case,
    ),
    transmission("PSLF .epc", "pslf", Emission::Document, Family::Case),
    transmission("XIIDM 1.17", "xiidm", Emission::Document, Family::Exchange),
    transmission("JIIDM 1.17", "jiidm", Emission::Document, Family::Exchange),
    transmission("CGMES 3.0", "cgmes", Emission::Directory, Family::Exchange),
    transmission(
        "UCTE-DEF .uct",
        "ucte",
        Emission::Document,
        Family::Exchange,
    ),
    transmission(
        "PyPSA CSV",
        "pypsa-csv",
        Emission::Directory,
        Family::Dataset,
    ),
    transmission(
        "GridFM Parquet",
        "gridfm",
        Emission::Directory,
        Family::Dataset,
    ),
    read_only(
        "PSS/E .raw 32",
        "psse",
        Family::Case,
        "psse/ExampleVersion32_exported.raw",
    ),
    read_only(
        "IEEE CDF",
        "ieee-cdf",
        Family::Case,
        "ieee-cdf/ieee14cdf.txt",
    ),
    read_only(
        "GO Challenge 3 JSON",
        "goc3-json",
        Family::Case,
        "goc3/goc3_small.json",
    ),
    read_only(
        "DeepMind OPFData JSON",
        "opfdata-json",
        Family::Dataset,
        "opfdataset/example_0.json",
    ),
];

fn transmission_targets() -> impl Iterator<Item = (usize, TransmissionFormat)> {
    TRANSMISSION_FORMATS
        .into_iter()
        .enumerate()
        .filter(|(_, format)| format.emission != Emission::ReadOnly)
}

const TRANSMISSION_CASES: [(&str, &str); 6] = [
    ("case9", "case9.m"),
    ("case14", "case14.m"),
    ("case30", "case30.m"),
    ("dcline", "t_case9_dcline.m"),
    ("out of service", "t_case9_oos.m"),
    ("PGLib 5", "pglib/pglib_opf_case5_pjm.m"),
];

struct TransmissionPayload {
    label: &'static str,
    network: BalancedNetwork,
    /// The parsed module, which retains the source it was read from.
    module: powerio_core::PioModule<powerio::PioValue>,
    /// The case the module was read from, for a writable format. A directory
    /// source reads some files only when it echoes them, so the directory
    /// lives as long as the payload.
    emitted: Option<Emitted>,
    parse_findings: Vec<Diagnostic>,
}

fn run_transmission_matrix() -> Matrix {
    let sources = TRANSMISSION_FORMATS.iter().map(|fmt| fmt.name).collect();
    let targets = transmission_targets().map(|(_, fmt)| fmt.name).collect();
    let groups = Family::ALL
        .into_iter()
        .map(|family| {
            let columns = transmission_targets()
                .enumerate()
                .filter(|(_, (_, format))| format.family == family)
                .map(|(column, _)| column)
                .collect();
            (family.title(), columns)
        })
        .collect();
    let mut source_cells = Vec::new();
    let mut cells = Vec::new();

    for (source_idx, source) in TRANSMISSION_FORMATS.iter().enumerate() {
        let payloads = transmission_payloads(*source);
        let mut source_cell = Cell::default();
        match &payloads {
            Ok(payloads) => {
                for payload in payloads {
                    source_cell.record(payload.label, Leg::Source, &payload.parse_findings);
                }
            }
            Err(err) => source_cell.failures.push(err.clone()),
        }
        source_cells.push(source_cell);
        let mut row = Vec::new();
        for (target_idx, target) in transmission_targets() {
            let mut cell = Cell::default();
            match &payloads {
                Ok(payloads) => {
                    for payload in payloads {
                        validate_transmission_pair(payload, target, &mut cell);
                        if source_idx == target_idx {
                            echo_transmission(payload, target, &mut cell);
                        }
                    }
                }
                Err(err) => cell.failures.push(err.clone()),
            }
            row.push(cell);
        }
        cells.push(row);
    }

    Matrix {
        sources,
        targets,
        groups,
        source_cells,
        cells,
    }
}

/// The payloads a source row converts from: every MATPOWER case written into
/// that source format and read back, so the row measures what that format
/// carries rather than what MATPOWER carries. A read only format has no writer,
/// so its row parses one vendored case instead.
fn transmission_payloads(format: TransmissionFormat) -> Result<Vec<TransmissionPayload>, String> {
    if let Some(fixture) = format.fixture {
        let source = powerio_core::Source::open(data(fixture))
            .map_err(|err| format!("open {fixture}: {err}"))?;
        let parsed = parse_transmission_source(source, format.token)
            .map_err(|err| format!("parse {fixture}: {err}"))?;
        return Ok(vec![TransmissionPayload {
            label: fixture,
            network: parsed.network,
            module: parsed.module,
            emitted: None,
            parse_findings: parsed.diagnostics,
        }]);
    }
    TRANSMISSION_CASES
        .iter()
        .map(|(label, rel)| {
            let base =
                parse_matpower_file(data(rel)).map_err(|err| format!("parse {rel}: {err}"))?;
            let rendered = emit_transmission(&powerio_core::PioModule::new(base), format)
                .map_err(|err| format!("write {rel} as {}: {err}", format.name))?;
            let parsed = parse_transmission_emitted(&rendered.emitted, format)
                .map_err(|err| format!("read generated {rel} as {}: {err}", format.name))?;
            Ok(TransmissionPayload {
                label,
                network: parsed.network,
                module: parsed.module,
                emitted: Some(rendered.emitted),
                parse_findings: parsed.diagnostics,
            })
        })
        .collect()
}

/// The diagonal's echo leg: the parsed module, with its retained source,
/// written as its own format returns the source byte for byte.
fn echo_transmission(payload: &TransmissionPayload, target: TransmissionFormat, cell: &mut Cell) {
    let Some(source) = &payload.emitted else {
        return;
    };
    let expected = match source.files() {
        Ok(files) => files,
        Err(err) => {
            cell.failures
                .push(format!("{} source files unreadable: {err}", payload.label));
            return;
        }
    };
    let echoed = emit_transmission(&payload.module, target).and_then(|echo| {
        cell.record(payload.label, Leg::Write, &echo.diagnostics);
        echo.emitted.files()
    });
    let exact = match echoed {
        Ok(files) if files == expected => true,
        Ok(files) => {
            let differing: Vec<_> = expected
                .keys()
                .chain(files.keys())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .filter(|path| expected.get(*path) != files.get(*path))
                .map(|path| {
                    if path.is_empty() {
                        "the document"
                    } else {
                        path
                    }
                })
                .collect();
            cell.failures.push(format!(
                "{} did not re-emit its retained source byte for byte as {} ({})",
                payload.label,
                target.name,
                differing.join(", ")
            ));
            false
        }
        Err(err) => {
            cell.failures.push(format!(
                "{} did not echo its retained source as {}: {err}",
                payload.label, target.name
            ));
            false
        }
    };
    cell.echo_exact = Some(cell.echo_exact.unwrap_or(true) && exact);
}

fn validate_transmission_pair(
    payload: &TransmissionPayload,
    target: TransmissionFormat,
    cell: &mut Cell,
) {
    let conversion = match emit_transmission(
        &powerio_core::PioModule::new(payload.network.clone()),
        target,
    ) {
        Ok(conversion) => conversion,
        Err(err) => {
            cell.failures.push(format!(
                "{} did not write as {}: {err}",
                payload.label, target.name
            ));
            return;
        }
    };
    cell.record(payload.label, Leg::Write, &conversion.diagnostics);
    let parsed = match parse_transmission_emitted(&conversion.emitted, target) {
        Ok(parsed) => parsed,
        Err(err) => {
            cell.failures.push(format!(
                "{} output did not parse as {}: {err}",
                payload.label, target.name
            ));
            return;
        }
    };
    cell.record(payload.label, Leg::Readback, &parsed.diagnostics);
    let held = network_a_target_holds(&payload.network, target);
    let tolerance = electrical_tolerance(target, &held);
    let actual = transmission_core(&parsed.network);
    let expected = transmission_core(&held);
    if !expected.agrees_within(&actual, tolerance) {
        cell.failures.push(format!(
            "{} core changed for {}: before {expected:?}, after {actual:?}",
            payload.label, target.name
        ));
    }
    record_transmission_model_diffs(
        payload.label,
        &transmission_value(&payload.network),
        &transmission_value(&parsed.network),
        cell,
    );
    // The electrical comparison runs against what the target's record set can
    // hold, which is what its writer said it wrote.
    //
    // The admittance matrix and the injections are compared by bus id first.
    // A format that identifies a bus by a name rather than a number (IIDM's
    // `<bus id="...">`, CIM's mRID, a UCTE-DEF node code) states no bus number,
    // so its reader numbers the buses it reads and the id keyed comparison is
    // asking the wrong question; the same power flow problem up to bus
    // relabeling is what such a conversion can promise, and the renumbering
    // itself stays a declared loss through the model comparison above.
    let numbered = match ybus_change_within(&held, &parsed.network, tolerance) {
        Ok(Some(diff)) => Some(format!("Y_bus changed: {diff}")),
        Ok(None) => injection_change_within(&held, &parsed.network, tolerance)
            .map(|diff| format!("bus injections moved: {diff}")),
        // A network with no buildable admittance matrix is a conversion
        // failure. Reporting it as agreement is how an invariant passes
        // without checking anything.
        Err(side) => {
            cell.failures.push(format!(
                "{} Y_bus could not be built for {} ({})",
                payload.label,
                target.name,
                match side {
                    YbusUnavailable::Before => "source",
                    YbusUnavailable::After => "result",
                }
            ));
            return;
        }
    };
    if numbered.is_none() {
        return;
    }
    if states_impedances_the_target_cannot(&held, target) {
        return;
    }
    if let Some(diff) = electrical_change_up_to_relabeling(&held, &parsed.network, tolerance) {
        cell.failures.push(format!(
            "{} electrical problem changed for {}: {diff}",
            payload.label, target.name
        ));
    }
}

/// Whether the target's own numeric fields cannot state this network's
/// impedances at all.
///
/// A UCTE-DEF element record states resistance and reactance in six
/// characters, at the scale of a transmission line in ohms. A network whose
/// nominal voltage sits below the lowest UCTE voltage level states impedances
/// of thousandths of an ohm, which those fields round to one digit or to zero,
/// so the admittance the reader rebuilds is a different one. The element counts
/// and the power totals still hold, and the writer reports the voltage level it
/// wrote each bus under.
fn states_impedances_the_target_cannot(
    network: &BalancedNetwork,
    target: TransmissionFormat,
) -> bool {
    /// The lowest of the ten UCTE-DEF voltage levels, in kV.
    const LOWEST_UCTE_LEVEL_KV: f64 = 27.0;
    target.token == "ucte"
        && network
            .buses()
            .iter()
            .any(|bus| bus.base_kv > 0.0 && bus.base_kv < LOWEST_UCTE_LEVEL_KV)
}

/// The source network reduced to what IIDM's topology can state.
///
/// IIDM states connectivity in a node breaker voltage level through switch
/// positions: a disconnected terminal is one whose switches to the busbar are
/// open. A level with no switch has no position to open, so an element that
/// states an out of service terminal there comes back in service. The writer
/// reports the same loss.
fn network_iidm_holds(network: &BalancedNetwork) -> BalancedNetwork {
    let Some(detailed) = network.detailed_connectivity().as_deref() else {
        return network.clone();
    };
    let levels_with_switches = detailed
        .switches
        .iter()
        .map(|switch| switch.voltage_level.clone())
        .collect::<std::collections::HashSet<_>>();
    let unstated = |component_type: &str, identity: Option<&str>| {
        let Some(identity) = identity else {
            return false;
        };
        detailed.terminals.iter().any(|terminal| {
            terminal.node.is_some()
                && !terminal.connected
                && !levels_with_switches.contains(&terminal.voltage_level)
                && terminal.equipment.component_type() == component_type
                && terminal.equipment.local_id() == identity
        })
    };
    let mut held = network.clone();
    for branch in held.branches_mut() {
        if unstated("branch", branch.uid.as_deref()) {
            branch.in_service = true;
        }
    }
    for generator in held.generators_mut() {
        if unstated("generator", generator.uid.as_deref()) {
            generator.in_service = true;
        }
    }
    for load in held.loads_mut() {
        if unstated("load", load.uid.as_deref()) {
            load.in_service = true;
        }
    }
    for shunt in held.shunts_mut() {
        if unstated("shunt", shunt.uid.as_deref()) {
            shunt.in_service = true;
        }
    }
    for storage in held.storage_mut() {
        if unstated("storage", storage.uid.as_deref()) {
            storage.in_service = true;
        }
    }
    held
}

/// The relative tolerance the electrical comparison holds a target to.
///
/// A format that states each value in a fixed width field cannot return more
/// digits than the field holds. A UCTE-DEF element record states resistance,
/// reactance, and susceptance in six characters, so a value comes back rounded
/// to about five significant digits and a diagonal entry, which sums several of
/// them, loses another decade. A network whose nominal voltage sits below the
/// lowest UCTE voltage level keeps far fewer: at one kilovolt a line impedance
/// is thousandths of an ohm and six characters hold one or two digits of it.
/// Every other target states its values as decimal numbers of the width the
/// value needs.
fn electrical_tolerance(target: TransmissionFormat, network: &BalancedNetwork) -> f64 {
    /// The lowest of the ten UCTE-DEF voltage levels, in kV.
    const LOWEST_UCTE_LEVEL_KV: f64 = 27.0;
    if target.token != "ucte" {
        return 1e-8;
    }
    let below_the_lowest_level = network
        .buses()
        .iter()
        .any(|bus| bus.base_kv > 0.0 && bus.base_kv < LOWEST_UCTE_LEVEL_KV);
    if below_the_lowest_level { 5e-2 } else { 1e-3 }
}

/// The source network reduced to the records the target format defines.
///
/// A field the target cannot state is a warning and a model difference, and
/// the electrical invariants still hold across it. A whole record the target
/// has no place for is different: dropping it changes the power flow problem,
/// so the comparison runs against the network the format can hold and the
/// writer reports the same loss. Every reduction here names a record absent
/// from the target format's own definition, and no reduction moves power
/// between buses.
fn network_a_target_holds(
    network: &BalancedNetwork,
    target: TransmissionFormat,
) -> BalancedNetwork {
    if matches!(target.token, "xiidm" | "jiidm") {
        return network_iidm_holds(network);
    }
    if target.token == "matpower" || target.token == "gridfm" {
        let mut held = network.clone();
        let mut merged = BTreeMap::<powerio_tx::BusId, powerio_tx::Shunt>::new();
        for shunt in held.shunts().iter().filter(|shunt| shunt.in_service) {
            let aggregate = merged
                .entry(shunt.bus)
                .or_insert_with(|| powerio_tx::Shunt::new(shunt.bus, 0.0, 0.0));
            aggregate.g += shunt.g;
            aggregate.b += shunt.b;
        }
        *held.shunts_mut() = merged.into_values().collect();
        return held;
    }
    if target.token == "pandapower-json" {
        let mut held = network.clone();
        let generator_buses = held
            .generators()
            .iter()
            .map(|generator| generator.bus)
            .collect::<std::collections::HashSet<_>>();
        let empty_references = held
            .buses()
            .iter()
            .filter(|bus| {
                bus.kind == powerio_tx::BusType::Ref && !generator_buses.contains(&bus.id)
            })
            .map(|bus| bus.id)
            .collect::<Vec<_>>();
        for bus in empty_references {
            held.generators_mut().push(powerio_tx::Generator::new(bus));
        }
        return held;
    }
    if target.token != "ucte" {
        return network.clone();
    }
    // UCTE-DEF states one node record per bus with one generation set and no
    // shunt record: a bus shunt has no record at all, a generator has no
    // status field. Only live machines contribute; a PV/REF node itself can
    // require a zero-output generator, but an inactive-only PQ node does not.
    // Several live machines at one bus state one generation.
    let mut held = network.clone();
    held.shunts_mut().clear();
    held.branches_mut()
        .retain(|branch| branch.from != branch.to);
    held.generators_mut()
        .retain(|generator| generator.in_service);
    let mut merged: Vec<powerio_tx::Generator> = Vec::new();
    for generator in held.generators() {
        match merged
            .iter_mut()
            .find(|existing| existing.bus == generator.bus)
        {
            Some(existing) => {
                existing.pg += generator.pg;
                existing.qg += generator.qg;
            }
            None => merged.push(generator.clone()),
        }
    }
    *held.generators_mut() = merged;
    let generator_buses = held
        .generators()
        .iter()
        .map(|generator| generator.bus)
        .collect::<std::collections::HashSet<_>>();
    let empty_references = held
        .buses()
        .iter()
        .filter(|bus| {
            matches!(bus.kind, powerio_tx::BusType::Ref | powerio_tx::BusType::Pv)
                && !generator_buses.contains(&bus.id)
        })
        .map(|bus| bus.id)
        .collect::<Vec<_>>();
    for bus in empty_references {
        held.generators_mut().push(powerio_tx::Generator::new(bus));
    }
    held
}

/// Record every typed model field the conversion changed. A cell whose
/// findings declare a loss keeps these as context; a cell at restated or
/// better fails on any of them, so a quiet writer cannot pass by dropping
/// data without saying so.
fn record_transmission_model_diffs(
    label: &'static str,
    before: &serde_json::Value,
    after: &serde_json::Value,
    cell: &mut Cell,
) {
    let mut equivalent_polynomials = BTreeSet::new();
    if let (Some(a), Some(b)) = (
        before["generators"].as_array(),
        after["generators"].as_array(),
    ) {
        for (index, (a, b)) in a.iter().zip(b).enumerate() {
            let (a, b) = (&a["cost"], &b["cost"]);
            let (Some(x), Some(y)) = (a["coeffs"].as_array(), b["coeffs"].as_array()) else {
                continue;
            };
            if a["model"] != 2
                || b["model"] != 2
                || a["ncost"].as_u64() != Some(x.len() as u64)
                || b["ncost"].as_u64() != Some(y.len() as u64)
            {
                continue;
            }
            let trim = |coefficients: &[serde_json::Value]| -> Option<Vec<f64>> {
                let numbers: Option<Vec<_>> =
                    coefficients.iter().map(serde_json::Value::as_f64).collect();
                let numbers = numbers?;
                let first = numbers
                    .iter()
                    .position(|x| *x != 0.0)
                    .unwrap_or(numbers.len());
                Some(numbers[first..].to_vec())
            };
            if trim(x).is_some() && trim(x) == trim(y) {
                equivalent_polynomials.insert(format!(".generators[{index}].cost"));
            }
        }
    }
    for difference in all_model_diffs(before, after) {
        let reason = if difference.path == ".generated_uids"
            && before["generated_uids"].is_array()
            && after.get("generated_uids").is_none()
        {
            Some(
                "the internal generated-identifier provenance ledger is omitted; identifiers themselves are compared separately",
            )
        } else if equivalent_polynomials.iter().any(|prefix| {
            difference.path == format!("{prefix}.ncost")
                || difference.path.starts_with(&format!("{prefix}.coeffs"))
        }) {
            Some("leading zero coefficients change polynomial storage, not the cost function")
        } else {
            None
        };
        let rendered = difference.to_string();
        if let Some(reason) = reason {
            cell.model_restatements
                .insert((label, rendered.clone()), reason);
        }
        cell.model_diffs.push((label, rendered));
    }
}

fn record_model_diffs(
    label: &'static str,
    before: &serde_json::Value,
    after: &serde_json::Value,
    cell: &mut Cell,
) {
    cell.model_diffs.extend(
        all_model_diffs(before, after)
            .iter()
            .map(|d| (label, d.to_string())),
    );
}

fn data(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data")
        .join(rel)
}

#[derive(Clone, Copy)]
struct DistributionFormat {
    name: &'static str,
    token: &'static str,
    target: DistTargetFormat,
}

const DISTRIBUTION_FORMATS: [DistributionFormat; 5] = [
    DistributionFormat {
        name: "OpenDSS .dss",
        token: "dss",
        target: DistTargetFormat::Dss,
    },
    DistributionFormat {
        name: "BMOPF JSON",
        token: "bmopf-json",
        target: DistTargetFormat::BmopfJson,
    },
    DistributionFormat {
        name: "PMD JSON",
        token: "pmd-json",
        target: DistTargetFormat::PmdJson,
    },
    DistributionFormat {
        name: "MG-RAVENS JSON (multiconductor)",
        token: "ravens-json",
        target: DistTargetFormat::RavensJson,
    },
    DistributionFormat {
        name: "PMD MATHEMATICAL JSON",
        token: "pmd-math-json",
        target: DistTargetFormat::PmdMathJson,
    },
];

const DISTRIBUTION_CASES: [(&str, &str, DistributionFormat); 9] = [
    (
        "single phase transformer",
        "dist/micro/xfmr_single_phase.dss",
        DISTRIBUTION_FORMATS[0],
    ),
    (
        "center tap transformer",
        "dist/micro/xfmr_center_tap.dss",
        DISTRIBUTION_FORMATS[0],
    ),
    (
        "switch states",
        "dist/micro/switch.dss",
        DISTRIBUTION_FORMATS[0],
    ),
    (
        "four wire linecode",
        "dist/micro/fourwire_linecode.dss",
        DISTRIBUTION_FORMATS[0],
    ),
    (
        "ten conductor linecode",
        "dist/micro/linecode_10x10.dss",
        DISTRIBUTION_FORMATS[0],
    ),
    (
        "BMOPF IEEE 13",
        "dist/bmopf/example_ieee13.json",
        DISTRIBUTION_FORMATS[1],
    ),
    (
        "PMD four wire",
        "dist/pmd/fourwire_linecode.json",
        DISTRIBUTION_FORMATS[2],
    ),
    (
        "MG-RAVENS case3",
        "dist/ravens/case3_balanced.json",
        DISTRIBUTION_FORMATS[3],
    ),
    (
        "PMD MATH two winding",
        "dist/pmd/math/ut_trans_2w_yy_math.json",
        DISTRIBUTION_FORMATS[4],
    ),
];

struct DistributionPayload {
    label: &'static str,
    network: MulticonductorNetwork,
    module: powerio_core::PioModule<MulticonductorNetwork>,
    /// The source text the module was read from.
    text: String,
    parse_findings: Vec<Diagnostic>,
}

/// A geographic layer is a `powerio.GeoLayer`, not a network, and no grid
/// exchange format states a standalone layer, so it has no matrix cell. Every
/// substation an aux file places survives the canonical `.geo.json` document.
#[test]
fn geo_layer_round_trips_through_geo_json() {
    let text = std::fs::read_to_string(data("powerworld/ACTIVSg200.aux")).unwrap();
    let layer = powerio::to_geo_layer_from_aux_text(&text).unwrap();
    assert_ne!(layer.features.as_slice(), []);
    let module = powerio_core::PioModule::new(powerio::PioValue::GeoLayer(layer.clone()));
    let destination = powerio_core::Destination::memory("layer").unwrap();
    let result = powerio::emit(&module, "geo-json", destination).unwrap();
    assert_eq!(
        powerio_core::render_diagnostics(result.diagnostics()),
        Vec::<String>::new()
    );
    let powerio_core::EmittedOutput::Memory { mut artifacts } = result.into_output() else {
        panic!("a memory destination returned path output");
    };
    assert_eq!(artifacts.len(), 1);
    let artifact = artifacts.pop().unwrap();
    let source =
        powerio_core::Source::from_memory("layer.geo.json", artifact.into_bytes()).unwrap();
    let options = powerio::ParseOptions::default().format("geo-json").unwrap();
    let back = powerio::parse_with_options(source, &options).unwrap();
    assert_eq!(
        powerio_core::render_diagnostics(back.diagnostics()),
        Vec::<String>::new()
    );
    let powerio::PioValue::GeoLayer(read_back) = &back.value() else {
        panic!(
            "the geo layer document parsed as {}",
            back.value().type_name()
        );
    };
    let diffs: Vec<String> = model_diffs(
        &serde_json::to_value(&layer).unwrap(),
        &serde_json::to_value(read_back).unwrap(),
    )
    .iter()
    .map(ToString::to_string)
    .collect();
    assert_eq!(diffs, Vec::<String>::new());
}

fn run_distribution_matrix() -> Matrix {
    let formats: Vec<_> = DISTRIBUTION_FORMATS.iter().map(|fmt| fmt.name).collect();
    let mut source_cells = Vec::new();
    let mut cells = Vec::new();

    for (source_idx, source) in DISTRIBUTION_FORMATS.iter().enumerate() {
        let payloads = distribution_payloads(*source);
        let mut source_cell = Cell::default();
        match &payloads {
            Ok(payloads) => {
                for payload in payloads {
                    source_cell.record(payload.label, Leg::Source, &payload.parse_findings);
                }
            }
            Err(err) => source_cell.failures.push(err.clone()),
        }
        source_cells.push(source_cell);
        let mut row = Vec::new();
        for (target_idx, target) in DISTRIBUTION_FORMATS.iter().enumerate() {
            let mut cell = Cell::default();
            match &payloads {
                Ok(payloads) => {
                    for payload in payloads {
                        let diagonal = source_idx == target_idx;
                        validate_distribution_pair(
                            payload,
                            source.name,
                            *target,
                            diagonal,
                            &mut cell,
                        );
                    }
                    let expected: BTreeSet<_> = payloads.iter().map(|p| p.label).collect();
                    if cell.cases != expected {
                        cell.failures.push(format!(
                            "incomplete fixture coverage: expected {expected:?}, checked {:?}",
                            cell.cases
                        ));
                    }
                }
                Err(err) => cell.failures.push(err.clone()),
            }
            row.push(cell);
        }
        cells.push(row);
    }

    let groups = vec![("into a distribution format", (0..formats.len()).collect())];
    Matrix {
        sources: formats.clone(),
        targets: formats,
        groups,
        source_cells,
        cells,
    }
}

fn distribution_payloads(format: DistributionFormat) -> Result<Vec<DistributionPayload>, String> {
    DISTRIBUTION_CASES
        .iter()
        .map(|(label, rel, native_format)| {
            let mut base = load_distribution_case(data(rel), native_format.token)
                .map_err(|err| format!("parse {rel}: {err}"))?;
            *base.network.source_format_mut() = None;
            let rendered = emit_distribution_value(&base.network, format.target)?;
            let parsed = parse_distribution_text(&rendered, format)
                .map_err(|err| format!("read generated {rel} as {}: {err}", format.name))?;
            Ok(DistributionPayload {
                label,
                network: parsed.network,
                module: parsed.module,
                text: rendered.text,
                parse_findings: parsed.diagnostics,
            })
        })
        .collect()
}

/// The canonical write and readback legs start from the electrical value,
/// without retained bytes. The diagonal independently checks the module echo;
/// source-byte equality must never stand in for exercising the writer.
fn validate_distribution_pair(
    payload: &DistributionPayload,
    row: &str,
    target: DistributionFormat,
    diagonal: bool,
    cell: &mut Cell,
) {
    cell.cases.insert(payload.label);
    if diagonal {
        match emit_distribution_module(&payload.module, target.target) {
            Ok(echo) => {
                let exact = echo.text == payload.text;
                cell.echo_exact = Some(cell.echo_exact.unwrap_or(true) && exact);
                if !exact {
                    cell.failures.push(format!(
                        "{} did not re-emit its retained source byte for byte as {}",
                        payload.label, target.name
                    ));
                }
            }
            Err(error) => cell.failures.push(format!(
                "{} echo as {}: {error}",
                payload.label, target.name
            )),
        }
    }
    let conversion = match emit_distribution_value(&payload.network, target.target) {
        Ok(conversion) => conversion,
        Err(err) => {
            cell.failures.push(format!(
                "{} canonical emit as {}: {err}",
                payload.label, target.name
            ));
            return;
        }
    };
    validate_distribution_emission(payload, row, target, false, cell, &conversion);
}

fn validate_distribution_emission(
    payload: &DistributionPayload,
    row: &str,
    target: DistributionFormat,
    diagonal: bool,
    cell: &mut Cell,
    conversion: &TextEmission,
) {
    cell.record(payload.label, Leg::Write, &conversion.diagnostics);
    let echoed = diagonal && conversion.text == payload.text;
    if diagonal {
        if !echoed {
            cell.failures.push(format!(
                "{} did not re-emit its retained source byte for byte as {}",
                payload.label, target.name
            ));
        }
        cell.echo_exact = Some(cell.echo_exact.unwrap_or(true) && echoed);
    }
    match parse_distribution_text(conversion, target) {
        Ok(parsed) => {
            if !echoed {
                cell.record(payload.label, Leg::Readback, &parsed.diagnostics);
            }
            let before = distribution_target_projection(
                &payload.network,
                target.target,
                conversion,
                payload.label,
                cell,
            );
            let before = source_equivalents::contract_source_pairs(&before);
            let after = source_equivalents::contract_source_pairs(&parsed.network);
            let before = source_equivalents::contract_transformer_pairs(&before);
            let after = source_equivalents::contract_transformer_pairs(&after);
            source_equivalents::assert_source_constraints(
                &before,
                &after,
                &format!("{} as {row} to {}", payload.label, target.name),
            );
            let actual = distribution_core(&after);
            let expected = distribution_core(&before);
            if !core_survives(&expected, &actual, target.target) {
                cell.failures.push(format!(
                    "{} core changed for {}: before {:?}, after {:?}",
                    payload.label, target.name, expected, actual
                ));
            }
            let (checked_before, checked_after) =
                distribution_check_scope(before.clone(), after.clone(), payload.label, cell);
            cell.failures.extend(
                electrical_projection::differences(&checked_before, &checked_after)
                    .into_iter()
                    .map(|problem| {
                        format!(
                            "{} electrical change for {}: {problem}",
                            payload.label, target.name
                        )
                    }),
            );
            record_model_diffs(
                payload.label,
                &distribution_report_value(&payload.network),
                &distribution_report_value(&parsed.network),
                cell,
            );
        }
        Err(err) => cell.failures.push(format!(
            "{} output did not parse as {}: {err}",
            payload.label, target.name
        )),
    }
}

/// Component arrays have identity, not positional semantics. Sort only these
/// top-level tables; conductor, phase and winding arrays remain in their stated
/// order, so a real terminal permutation remains a measured difference.
fn distribution_report_value(network: &MulticonductorNetwork) -> serde_json::Value {
    let mut value = distribution_value(network);
    for table in value.as_object_mut().unwrap().values_mut() {
        let Some(rows) = table.as_array_mut() else {
            continue;
        };
        let key = |row: &serde_json::Value| {
            row.get("name")
                .or_else(|| row.get("id"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        };
        if rows.iter().all(|row| key(row).is_some()) {
            rows.sort_by_key(key);
        }
    }
    value
}

/// Explicit target projections, never inferred from the presence of a generic
/// warning. Unsupported RAVENS conductors remove precisely the affected device,
/// and every removal must name that device in a RECORD_DROPPED finding. The
/// remainder of that fixture still undergoes all checks and contributes its
/// findings to the baseline.
#[allow(clippy::too_many_lines)] // each fixture-specific projection states and verifies its loss
fn distribution_target_projection(
    network: &MulticonductorNetwork,
    target: DistTargetFormat,
    emission: &TextEmission,
    label: &'static str,
    cell: &mut Cell,
) -> MulticonductorNetwork {
    let mut held = network.clone();
    if target == DistTargetFormat::RavensJson {
        let (projected, problems) =
            electrical_projection::ravens_projection(&held, &emission.diagnostics);
        held = projected;
        for transformer in held.transformers_mut() {
            // These are measured, documented losses in two exact fixtures,
            // not permission to waive other transformer parameters.
            if label == "center tap transformer"
                && transformer.name == "t1"
                && transformer.windings.len() == 3
                && transformer.xsc_pct.len() == 3
                && transformer.xsc_pct[1..].iter().any(|x| *x != 0.0)
            {
                let prefix =
                    "transformer t1: only the first winding pair's short-circuit reactance";
                if !emission.diagnostics.iter().any(|d| {
                    d.code() == "EMIT.RAVENS_DIST.VALUE_COLLAPSED"
                        && d.message().starts_with(prefix)
                }) {
                    cell.failures.push(format!(
                        "{label}: missing named multi-winding reactance loss finding"
                    ));
                }
                cell.verification_notes.push((label, format!("known RAVENS projection: transformer t1 leakage pairs 1 and 2 {:?} -> [0, 0]; exact projected values checked; electrical equivalence is not claimed", &transformer.xsc_pct[1..])));
                transformer.xsc_pct[1..].fill(0.0);
            }
            if label == "PMD MATH two winding"
                && ["tx1", "tx1_1", "tx1_2", "tx1_3"].contains(&transformer.name.as_str())
            {
                for (index, winding) in transformer.windings.iter_mut().enumerate() {
                    if winding.tap.total_cmp(&1.0).is_eq() {
                        continue;
                    }
                    let prefix = format!(
                        "transformer {}: winding {} tap {} is dropped",
                        transformer.name,
                        index + 1,
                        winding.tap
                    );
                    if !emission.diagnostics.iter().any(|d| {
                        d.code() == "EMIT.RAVENS_DIST.FIELD_DROPPED"
                            && d.message().starts_with(&prefix)
                    }) {
                        cell.failures.push(format!(
                            "{label}: missing named winding {} tap loss finding",
                            index + 1
                        ));
                    }
                    cell.verification_notes.push((label, format!("known RAVENS projection: transformer {} winding {} tap {} -> 1; exact projected ratio checked; electrical equivalence is not claimed", transformer.name, index + 1, winding.tap)));
                    winding.tap = 1.0;
                }
            }
        }
        cell.failures.extend(
            problems
                .into_iter()
                .map(|problem| format!("{label}: {problem}")),
        );
    }
    if target == DistTargetFormat::Dss && label == "BMOPF IEEE 13" {
        // In these two buses the ungrounded named return conductor is bus-local;
        // DSS's diagnosed n -> 2 rename is bijective (the other conductor is
        // respectively 3 or 1). Preserve the branch, not a global phase alias.
        for (bus_id, phase) in [("611", "3"), ("652", "1")] {
            if held
                .bus(bus_id)
                .is_some_and(|bus| bus.terminals == [phase, "n"] && bus.grounded.is_empty())
            {
                let message = format!(
                    "bus {bus_id}: terminal `n` is not a dss node number; emitted as node 2, its position on the bus"
                );
                if !emission
                    .diagnostics
                    .iter()
                    .any(|d| d.code() == "EMIT.DSS.VALUE_SUBSTITUTED" && d.message() == message)
                {
                    cell.failures.push(format!(
                        "{label}: missing named terminal rename finding for bus {bus_id}"
                    ));
                }
                let bus = held
                    .buses_mut()
                    .iter_mut()
                    .find(|bus| bus.id == bus_id)
                    .unwrap();
                "2".clone_into(&mut bus.terminals[1]);
                for load in held
                    .loads_mut()
                    .iter_mut()
                    .filter(|load| load.bus == bus_id)
                {
                    for terminal in &mut load.terminal_map {
                        if terminal == "n" {
                            "2".clone_into(terminal);
                        }
                    }
                }
                cell.verification_notes.push((label, format!("equivalent DSS conductor rename: bus {bus_id} ungrounded n -> 2, with phase {phase} unchanged; the two-terminal injection and all other ports remain checked")));
            }
        }
    }
    if target == DistTargetFormat::PmdMathJson {
        let document: serde_json::Value = serde_json::from_str(&emission.text).unwrap();
        if document["is_kron_reduced"] == true {
            match powerio_dist::neutral_kron_reduce(&held, &powerio_dist::NeutralKronOptions::default()) {
                Ok(reduced) => {
                    // Eliminating a grounded neutral cannot reverse a winding.
                    // Preserve the original transformer circuit for the physical
                    // check instead of sharing the writer's winding projection.
                    let mut projected = reduced.into_parts().0;
                    restore_transformer_circuits(&held, &mut projected);
                    held = projected;
                }
                Err(error) => cell.failures.push(format!(
                    "{label}: output claims Kron reduction but the source does not permit it: {error}"
                )),
            }
        }
    }
    held
}

/// Retain each winding's original orientation and the grounding identity of
/// its eliminated return. Reintroducing a perfectly grounded port adds no
/// electrical degree of freedom; forgetting its grounding would instead
/// compare terminal 4 against earth as though they were different nodes.
fn restore_transformer_circuits(
    original: &MulticonductorNetwork,
    projected: &mut MulticonductorNetwork,
) {
    projected
        .transformers_mut()
        .clone_from(original.transformers());
    for winding in original.transformers().iter().flat_map(|t| &t.windings) {
        let Some(source_bus) = original.bus(&winding.bus) else {
            continue;
        };
        let Some(bus) = projected
            .buses_mut()
            .iter_mut()
            .find(|bus| bus.id.eq_ignore_ascii_case(&winding.bus))
        else {
            continue;
        };
        for terminal in &winding.terminal_map {
            if source_bus.grounded.contains(terminal) {
                if !bus.terminals.contains(terminal) {
                    bus.terminals.push(terminal.clone());
                }
                if !bus.grounded.contains(terminal) {
                    bus.grounded.push(terminal.clone());
                }
            }
        }
    }
}

/// This vendored BMOPF source declares a single-phase transformer with three
/// ungrounded terminals per winding. It does not identify the winding branch.
/// State the unavailable check instead of inventing a circuit; the exception
/// covers only this named component, never another transformer or device.
fn distribution_check_scope(
    mut before: MulticonductorNetwork,
    mut after: MulticonductorNetwork,
    label: &'static str,
    cell: &mut Cell,
) -> (MulticonductorNetwork, MulticonductorNetwork) {
    // The source row is itself a generated conversion of this fixture.
    // Preparation may already have guessed a phase count or terminal map, but
    // it cannot turn the original ambiguous winding into independent evidence.
    if label == "BMOPF IEEE 13" {
        cell.verification_notes.push((label, "UNVERIFIED component xfm2: source declares one phase with winding terminals [1, 2, 3]; no branch identity is specified. Transformer electrical comparison excludes only xfm2; its typed changes and all other component checks remain visible.".to_owned()));
        let unchecked = ["xfm2", "xfm2_1", "xfm2_2", "xfm2_3"];
        before
            .transformers_mut()
            .retain(|t| !unchecked.contains(&t.name.as_str()));
        after
            .transformers_mut()
            .retain(|t| !unchecked.contains(&t.name.as_str()));
    }
    (before, after)
}

struct DistParsed {
    network: MulticonductorNetwork,
    diagnostics: Vec<Diagnostic>,
    module: powerio_core::PioModule<MulticonductorNetwork>,
}

fn dist_module_to_parsed(module: powerio_core::PioModule<MulticonductorNetwork>) -> DistParsed {
    DistParsed {
        diagnostics: module.diagnostics().to_vec(),
        network: module.value().clone(),
        module,
    }
}

fn load_distribution_case(
    path: impl AsRef<std::path::Path>,
    from: &str,
) -> Result<DistParsed, powerio_core::Error> {
    let source = powerio_core::Source::open(path.as_ref())?.with_format(
        powerio_core::FormatId::new(from.to_ascii_lowercase().replace('_', "-"))?,
    );
    powerio_dist::parse(source).map(dist_module_to_parsed)
}

/// Read an emission back. OpenDSS text goes through a file of its own, with
/// the emission's sidecars (a bus coordinate file the text includes) beside
/// it, in a directory per call so the two tests of this file do not race.
fn parse_distribution_text(
    emission: &TextEmission,
    format: DistributionFormat,
) -> Result<DistParsed, powerio_core::Error> {
    if format.target != DistTargetFormat::Dss {
        let source =
            powerio_core::Source::from_memory("<memory>", emission.text.as_bytes().to_vec())?
                .with_format(powerio_core::FormatId::new(
                    format.token.to_ascii_lowercase().replace('_', "-"),
                )?);
        return powerio_dist::parse(source).map(dist_module_to_parsed);
    }

    let dir = std::env::temp_dir()
        .join("powerio-conversion-matrix")
        .join(format!(
            "case-{}-{}",
            std::process::id(),
            DSS_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
    std::fs::create_dir_all(&dir).unwrap();
    // Not `case.dss`: a same format echo returns the retained file under
    // its own name beside the destination's `case.dss`.
    let path = dir.join("input.dss");
    std::fs::write(&path, &emission.text).unwrap();
    for (file, text) in &emission.sidecars {
        std::fs::write(dir.join(file), text).unwrap();
    }
    let parsed = load_distribution_case(&path, format.token);
    let _ = std::fs::remove_dir_all(dir);
    parsed
}

/// Whether the core survived the round trip.
///
/// Source-equivalent series pairs are already contracted after checking their
/// exact cancellation and exclusive internal nodes. Everything else must match,
/// with one allowance: a dss target splits a load whose
/// phases carry different power into one single phase `Load` per phase (#266),
/// because a dss `Load` divides its `kw` evenly and cannot state the profile
/// otherwise. That grows the object count and nothing else — the power the
/// loads carry is compared exactly, on this leg as on every other.
fn core_survives(
    before: &DistributionCore,
    after: &DistributionCore,
    target: DistTargetFormat,
) -> bool {
    if before == after {
        return true;
    }
    target == DistTargetFormat::Dss
        && after.loads >= before.loads
        && after.buses == before.buses
        && after.generators == before.generators
        && after.shunts == before.shunts
        && after.load_p == before.load_p
        && after.load_q == before.load_q
}

/// Every corpus fixture, serialized as PowerIO IR and deserialized, per
/// source format.
///
/// The matrix above measures what survives a case format hop; this measures
/// what survives PowerIO IR, which must be everything. It is the per-fixture
/// lossless signal over the same corpus the matrix walks rather than over
/// synthetic fixtures: a serde rename, a dropped `serde(default)`, or a field
/// that fails to serialize shows up on the first case that carries it.
#[test]
fn every_fixture_round_trips_through_ir() {
    let mut failures: Vec<String> = Vec::new();

    for format in TRANSMISSION_FORMATS {
        let payloads = match transmission_payloads(format) {
            Ok(payloads) => payloads,
            Err(err) => {
                failures.push(err);
                continue;
            }
        };
        for payload in payloads {
            let where_ = format!("{} as {}", payload.label, format.name);
            if let Err(err) = ir_preserves_network(payload.network) {
                failures.push(format!("{where_}: {err}"));
            }
        }
    }

    for format in DISTRIBUTION_FORMATS {
        let payloads = match distribution_payloads(format) {
            Ok(payloads) => payloads,
            Err(err) => {
                failures.push(err);
                continue;
            }
        };
        for payload in payloads {
            let module = powerio_core::PioModule::new(powerio::PioValue::MulticonductorNetwork(
                payload.network,
            ));
            if let Err(err) = ir_round_trip(&module) {
                failures.push(format!("{} as {}: {err}", payload.label, format.name));
            }
        }
    }

    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A balanced network survives PowerIO IR serialization and deserialization
/// unchanged.
fn ir_preserves_network(net: BalancedNetwork) -> Result<(), String> {
    let before = transmission_value(&net);
    let module = powerio_core::PioModule::new(powerio::PioValue::BalancedNetwork(net));
    let back = ir_round_trip(&module)?;
    let powerio::PioValue::BalancedNetwork(net_back) = &back.value() else {
        return Err("the module came back with a different value kind".to_owned());
    };
    let after = transmission_value(net_back);
    if before != after {
        return Err(format!(
            "the PowerIO IR round trip changed the model: {:?}",
            model_diffs(&before, &after)
        ));
    }
    Ok(())
}

/// The document itself is stable: serialize, deserialize, serialize again,
/// and the two texts agree byte for byte.
fn ir_round_trip(
    module: &powerio_core::PioModule<powerio::PioValue>,
) -> Result<powerio_core::PioModule<powerio::PioValue>, String> {
    let first = serialize_ir(module)?;
    let source = powerio::Source::from_memory("module.pio.json", first.as_bytes().to_vec())
        .map_err(|err| format!("create IR source: {err}"))?;
    let back = powerio::deserialize(source).map_err(|err| format!("deserialize: {err}"))?;
    let second = serialize_ir(&back)?;
    if first != second {
        return Err("the PowerIO IR document is not serialization stable".to_owned());
    }
    Ok(back)
}

fn serialize_ir(module: &powerio_core::PioModule<powerio::PioValue>) -> Result<String, String> {
    let destination = powerio::Destination::memory("module.pio.json")
        .map_err(|err| format!("create IR destination: {err}"))?;
    let result =
        powerio::serialize(module, destination).map_err(|err| format!("serialize: {err}"))?;
    let powerio::EmittedOutput::Memory { mut artifacts } = result.into_output() else {
        return Err("PowerIO IR memory destination returned path output".to_owned());
    };
    let artifact = artifacts
        .pop()
        .filter(|_| artifacts.is_empty())
        .ok_or_else(|| "PowerIO IR serialization returned more than one artifact".to_owned())?;
    String::from_utf8(artifact.into_bytes())
        .map_err(|err| format!("PowerIO IR is not valid UTF-8: {err}"))
}

fn parse_matpower_file(
    path: impl AsRef<std::path::Path>,
) -> Result<powerio_matrix::BalancedNetwork, powerio_core::Error> {
    let source = powerio_core::Source::open(path.as_ref())?
        .with_format(powerio_core::FormatId::new("matpower")?);
    powerio_tx::format::parse(source).map(powerio_core::PioModule::into_value)
}

struct ParsedTransmission {
    network: BalancedNetwork,
    module: powerio_core::PioModule<powerio::PioValue>,
    diagnostics: Vec<Diagnostic>,
}

/// Parse one source under a declared format through the facade, which is the
/// operation a caller has and the only one that reads a dataset directory.
///
/// A dataset directory states one network per scenario and a PyPSA folder one
/// per snapshot; the matrix writes one snapshot, so it reads the first entry
/// back.
fn parse_transmission_source(
    source: powerio_core::Source,
    from: &str,
) -> Result<ParsedTransmission, String> {
    let format = powerio_core::FormatId::new(from.to_ascii_lowercase().replace('_', "-"))
        .map_err(|err| err.to_string())?;
    let options = powerio::ParseOptions::default().format_id(format);
    let module = powerio::parse_with_options(source, &options).map_err(|err| err.to_string())?;
    let diagnostics = module.diagnostics().to_vec();
    let network = balanced_network(module.value())
        .ok_or_else(|| format!("{from} parsed as {}", module.value().type_name()))?
        .clone();
    Ok(ParsedTransmission {
        network,
        module,
        diagnostics,
    })
}

/// The balanced network a parsed module carries, whatever collection the format
/// states it in.
fn balanced_network(value: &powerio::PioValue) -> Option<&BalancedNetwork> {
    match value {
        powerio::PioValue::BalancedNetwork(network) => Some(network),
        powerio::PioValue::ScenarioSet(scenarios) => scenarios.get_at(0).and_then(balanced_network),
        powerio::PioValue::TimeSeries(series) => series.get(0).and_then(balanced_network),
        powerio::PioValue::BalancedOperatingPoint(point) => Some(point.network()),
        // A release of solved cases (DeepMind OPFData) parses as the solution
        // it states; the network it solves is the snapshot a case format
        // carries.
        powerio::PioValue::AcOpfSolution(solution) => Some(solution.network()),
        powerio::PioValue::AcPfSolution(solution) => Some(solution.network()),
        // A problem statement (GO Challenge 3) parses as the calculation it
        // states; the network it commits units on is the snapshot a case
        // format carries.
        powerio::PioValue::AcScucInstance(instance) => Some(instance.network()),
        _ => None,
    }
}

#[test]
fn finding_artifact_preserves_rare_findings_and_case_identity() {
    let mut cell = Cell::default();
    for index in 0..8 {
        let finding = Diagnostic::new(
            powerio_core::DiagnosticCode::new("EMIT.TEST.FIELD_DROPPED").unwrap(),
            DiagnosticSeverity::Warning,
            format!("field {index} was dropped"),
        );
        cell.record("transformer with taps", Leg::Write, &[finding]);
    }
    let matrix = Matrix {
        sources: vec!["source"],
        targets: vec!["target"],
        groups: vec![],
        source_cells: vec![Cell::default()],
        cells: vec![vec![cell]],
    };
    let judged = matrix
        .keyed_cells()
        .into_iter()
        .map(|(key, cell)| {
            let entry = Judged {
                key: key.clone(),
                grade: cell.grade(),
                problems: vec![],
            };
            (key, entry)
        })
        .collect();
    let details = render_details(&[("Test", matrix)], &judged);
    for index in 0..8 {
        assert!(details.contains(&format!(
            "[transformer with taps] EMIT.TEST.FIELD_DROPPED: field {index} was dropped"
        )));
    }
}

#[test]
fn a_loss_in_one_fixture_does_not_authorize_another_fixtures_change() {
    let mut cell = Cell::default();
    cell.record(
        "metadata fixture",
        Leg::Write,
        &[Diagnostic::new(
            powerio_core::DiagnosticCode::new("EMIT.TEST.EXTRAS_DROPPED").unwrap(),
            DiagnosticSeverity::Warning,
            "a source comment was dropped",
        )],
    );
    record_model_diffs(
        "electrical fixture",
        &serde_json::json!({"resistance": 0.1}),
        &serde_json::json!({"resistance": 777_123.0}),
        &mut cell,
    );
    let grade = cell.grade();
    assert_eq!(grade.level, Level::Failed);
    assert!(grade.unreported_change);
}

#[test]
fn metadata_losses_cannot_hide_a_corrupted_distribution_impedance() {
    fn corrupt(value: &mut serde_json::Value) -> usize {
        match value {
            serde_json::Value::Object(fields) => {
                let mut count = 0;
                if let Some(resistance) = fields.get_mut("PhaseImpedanceData.r") {
                    *resistance = serde_json::json!(777_123.0);
                    count += 1;
                }
                count + fields.values_mut().map(corrupt).sum::<usize>()
            }
            serde_json::Value::Array(values) => values.iter_mut().map(corrupt).sum(),
            _ => 0,
        }
    }
    let source = DISTRIBUTION_FORMATS[2];
    let target = DISTRIBUTION_FORMATS[3];
    let payloads = distribution_payloads(source).unwrap();
    let payload = payloads
        .iter()
        .find(|payload| payload.label == "four wire linecode")
        .unwrap();
    let mut conversion = emit_distribution_module(&payload.module, target.target).unwrap();
    let mut control = Cell::default();
    validate_distribution_emission(
        payload,
        source.name,
        target,
        false,
        &mut control,
        &conversion,
    );
    assert!(control.failures.is_empty(), "{:?}", control.failures);

    let mut document: serde_json::Value = serde_json::from_str(&conversion.text).unwrap();
    assert!(corrupt(&mut document) > 0);
    conversion.text = document.to_string();
    let mut corrupted = Cell::default();
    validate_distribution_emission(
        payload,
        source.name,
        target,
        false,
        &mut corrupted,
        &conversion,
    );
    // Before the physical checks, the fixture's pre-existing EXTRAS_DROPPED
    // warning left this mutation at exactly the same orange baseline level.
    assert_eq!(corrupted.grade().codes, control.grade().codes);
    assert!(
        corrupted
            .failures
            .iter()
            .any(|failure| { failure.contains("R (ohm)") && failure.contains("777123") }),
        "{:?}",
        corrupted.failures
    );
    assert_eq!(corrupted.grade().level, Level::Failed);

    // Even a declared-loss cell must expose all measured typed differences.
    let matrix = Matrix {
        sources: vec!["PMD"],
        targets: vec!["RAVENS"],
        groups: vec![],
        source_cells: vec![Cell::default()],
        cells: vec![vec![corrupted]],
    };
    let judged = matrix
        .keyed_cells()
        .into_iter()
        .map(|(key, cell)| {
            let entry = Judged {
                key: key.clone(),
                grade: cell.grade(),
                problems: vec![],
            };
            (key, entry)
        })
        .collect();
    let details = render_details(&[("Distribution", matrix)], &judged);
    assert!(details.contains("model change [four wire linecode]: .linecodes"));
    assert!(details.contains("777123"));
}

#[test]
fn matrix_model_change_details_are_not_truncated_at_the_corpus_limit() {
    let before = serde_json::json!((0..40).map(|_| 1.0).collect::<Vec<_>>());
    let after = serde_json::json!((0..40).map(|_| 2.0).collect::<Vec<_>>());
    assert_eq!(model_diffs(&before, &after).len(), 24);
    let mut cell = Cell::default();
    record_model_diffs("many changed impedances", &before, &after, &mut cell);
    assert_eq!(cell.model_diffs.len(), 40);
    assert!(cell.model_diffs.last().unwrap().1.contains("[39]"));
    assert!(
        cell.model_diffs
            .iter()
            .all(|(case, _)| *case == "many changed impedances")
    );
    let resized = serde_json::json!([777_123.0]);
    let differences = all_model_diffs(&before, &resized);
    assert!(differences.iter().any(|d| d.path == "[0]"));
    assert!(differences.iter().any(|d| d.path == "[39]"));
}

#[test]
fn kron_projection_preserves_ground_identity_without_waiving_winding_orientation() {
    let original = load_distribution_case(
        data("dist/pmd/math/ut_trans_2w_yy_math.json"),
        "pmd-math-json",
    )
    .unwrap()
    .network;
    let mut projected =
        powerio_dist::neutral_kron_reduce(&original, &powerio_dist::NeutralKronOptions::default())
            .unwrap()
            .into_parts()
            .0;
    restore_transformer_circuits(&original, &mut projected);
    assert_eq!(
        electrical_projection::differences(&original, &projected),
        Vec::<String>::new()
    );
    let terminals = &mut projected.transformers_mut()[0].windings[0].terminal_map;
    let last = terminals.len() - 1;
    terminals.swap(0, last);
    assert_ne!(
        electrical_projection::differences(&original, &projected),
        Vec::<String>::new()
    );
}

#[test]
fn an_unverified_transformer_cannot_mask_a_changed_line_or_another_transformer() {
    let (_, fixture, format) = DISTRIBUTION_CASES
        .iter()
        .find(|(label, _, _)| *label == "BMOPF IEEE 13")
        .unwrap();
    let parsed = load_distribution_case(data(fixture), format.token).unwrap();
    let mut cell = Cell::default();
    let (before, mut after) = distribution_check_scope(
        parsed.network.clone(),
        parsed.network,
        "BMOPF IEEE 13",
        &mut cell,
    );
    assert_eq!(cell.verification_notes.len(), 1);
    assert!(
        cell.verification_notes[0]
            .1
            .contains("UNVERIFIED component xfm2")
    );
    assert!(!before.transformers().iter().any(|t| t.name == "xfm2"));
    assert_eq!(
        electrical_projection::differences(&before, &after),
        Vec::<String>::new()
    );
    after.line_codes_mut()[0].r_series[0][0] = 777_123.0;
    after.transformers_mut()[0].windings[1].tap = 1.25;
    let differences = electrical_projection::differences(&before, &after);
    assert!(
        differences
            .iter()
            .any(|difference| difference.contains("R (ohm)")),
        "{differences:?}"
    );
    assert!(
        differences
            .iter()
            .any(|difference| difference.contains("voltage ratio")),
        "{differences:?}"
    );
}

#[test]
fn transmission_restatements_do_not_waive_other_fields() {
    let before = serde_json::json!({"generated_uids": ["g1"], "generators": [{"uid": "g1", "cost": {"model": 2, "ncost": 2, "coeffs": [2.0, 3.0]}}]});
    let after = serde_json::json!({"generators": [{"uid": "g1", "cost": {"model": 2, "ncost": 3, "coeffs": [0.0, 2.0, 3.0]}}]});
    let mut restated = Cell::default();
    record_transmission_model_diffs("polynomial", &before, &after, &mut restated);
    assert_eq!(restated.grade().level, Level::Restated);
    assert_eq!(
        restated.model_diffs.len(),
        restated.model_restatements.len()
    );
    let mut changed = after.clone();
    changed["generators"][0]["cost"]["coeffs"][2] = serde_json::json!(4.0);
    changed["generators"][0]["uid"] = serde_json::json!("different generator");
    let mut corrupted = Cell::default();
    record_transmission_model_diffs("polynomial", &before, &changed, &mut corrupted);
    assert_eq!(corrupted.grade().level, Level::Failed);
    assert_eq!(corrupted.model_restatements.len(), 1);
}

#[test]
fn distribution_diagonal_checks_canonical_emission_as_well_as_byte_echo() {
    let format = DISTRIBUTION_FORMATS[3];
    let mut payloads = distribution_payloads(format).unwrap();
    let payload = payloads
        .iter_mut()
        .find(|p| p.label == "four wire linecode")
        .unwrap();
    // The retained module remains intact. Only the value passed to the fresh
    // writer has this name, which RAVENS must diagnose as unrepresentable.
    *payload.network.name_mut() = Some("canonical writer sentinel".to_owned());
    let mut cell = Cell::default();
    validate_distribution_pair(payload, format.name, format, true, &mut cell);
    assert_eq!(cell.echo_exact, Some(true));
    assert!(
        cell.findings
            .iter()
            .any(|(_, leg, diagnostic)| *leg == Leg::Write
                && diagnostic.message().contains("canonical writer sentinel"))
    );
}

#[test]
fn distribution_report_orders_components_but_preserves_conductor_order() {
    let parsed = load_distribution_case(data("dist/micro/fourwire_linecode.dss"), "dss").unwrap();
    let mut reordered = parsed.network.clone();
    reordered.buses_mut().reverse();
    reordered.loads_mut().reverse();
    reordered.lines_mut().reverse();
    assert_eq!(
        distribution_report_value(&parsed.network),
        distribution_report_value(&reordered)
    );
    reordered.lines_mut()[0].terminal_map_from.reverse();
    assert_ne!(
        distribution_report_value(&parsed.network),
        distribution_report_value(&reordered)
    );
}
