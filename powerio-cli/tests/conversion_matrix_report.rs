//! The conversion matrix: every source format converted into every target,
//! with the losses accounted for. This file is both the CI gate and the
//! generator of the PR comment; the notes here are for whoever works on a
//! cell next.
//!
//! # What the tables cover
//!
//! The transmission matrix has a source row for every supported output plus
//! four read-only source rows. Its target columns cover the balanced case
//! formats, the PowSybl and ENTSO-E exchange formats (XIIDM, JIIDM, CGMES,
//! UCTE-DEF), and the dataset directories (PyPSA CSV and GridFM Parquet). The
//! read-only rows cover PSS/E RAW revision 32, IEEE CDF, GO Challenge 3, and
//! DeepMind OPFData. That is nineteen rows and fifteen columns, which no PR
//! comment renders readably as one table, so the columns are grouped by what
//! the formats are for and each group keeps every source row. The grouping
//! changes the layout and not the cells: every source is still run into every
//! target.
//!
//! One parse only input is named here rather than run. The only vendored
//! PowerWorld `.pwb` export states no located bus type, so its network has no
//! reference bus. GridFM requires exactly one reference bus and therefore
//! cannot write that source; a PWB row cannot satisfy the all-target contract
//! until a redistributable fixture states a slack bus or the binary record
//! carrying that designation is understood.
//!
//! A geographic layer is a `powerio.GeoLayer` rather than a network, and no
//! grid exchange format states a standalone layer, so it has its own one cell
//! table instead of a row here.
//!
//! # The rule a nonzero cell states
//!
//! A nonzero cell means the target format cannot carry what the source stated,
//! and every warning names the field or record it cannot carry: on the write
//! and readback legs a field of the target format, and on the source parse
//! leg, whose warnings every cell of that row shares, what the source document
//! itself leaves unstated. A warning that named a writer PowerIO could extend
//! instead of a limit of the format would be a defect in this table, not a
//! yellow cell.
//!
//! # What a cell asserts
//!
//! Each cell runs source parse → target write → target readback over every
//! case, and holds four properties:
//!
//! 1. **Warning parity** — observed warnings equal the reviewed baseline
//!    (the `*_WARNING_BASELINE` arrays below), text-attributed in the details
//!    report.
//! 2. **Core survival** — element counts and total load/generation survive.
//! 3. **Electrical survival** — the admittance matrix, entry for entry by bus
//!    id pair, and the per-bus load/generation injections. Warnings account
//!    for *dropped* data, never for corrupted electrics, so these hold on
//!    yellow cells too; together they pin the power flow problem itself
//!    (same `Y_bus`, same injections, same solution), which is the cheap,
//!    dependency-free stand-in for cross-validating an AC solve.
//!
//!    Three allowances, each a property of a target format rather than of a
//!    writer, and each stated at its own site: a format that identifies a bus
//!    by a name rather than a number states no bus number, so the comparison
//!    falls back to the same problem up to bus relabeling
//!    (`electrical_change_up_to_relabeling`); a format that states each value
//!    in a fixed width field returns fewer digits than the value carries
//!    (`electrical_tolerance`); and a format with no record for an element
//!    changes the problem by dropping it, so the comparison runs against the
//!    network the format can hold (`network_a_target_holds`). Every one of
//!    them names a record or field absent from the target's own definition,
//!    and the writer reports the same loss as a warning.
//! 4. **Lossless means lossless** — a cell claiming green (zero warnings)
//!    must leave the full typed model bit-identical (to a last-ulp float
//!    tolerance), field for field, extras included. A cell that is silent
//!    while the model changed fails outright. This is what keeps the matrix
//!    from going farcically green: suppressing a warning without carrying
//!    the data trips the parity gate.
//!
//! # How to make a cell greener (in order of preference)
//!
//! 1. **Carry the data.** Most wins here were reader/writer asymmetries: a
//!    writer refusing to emit a block its own reader parses (`mpc.dcline`,
//!    `mpc.bus_name`, the 21-column gen row, egret `startup_cost`,
//!    pandapower `dcline`/`res_gen`). Check the reader first — if it reads a
//!    field, the writer can almost always state it.
//! 2. **Stop retaining restatements.** A reader that keeps what powerio's
//!    own writer synthesizes (positional ids, zero component splits, default
//!    converter tails, raw record echoes) makes every downstream hop "drop"
//!    data that never said anything. Retain an extra only when the record
//!    states more than a rewrite would produce; compare against the writer's
//!    actual defaults, not a guess.
//! 3. **Warn only on losses.** Absent source data is not a drop
//!    (`mpc.gencost` on a costless network); synthesized-defaults
//!    disclosures are (pandapower `vn_kv = 1`); derived fields that restate
//!    a typed field are not separate data (pandapower `max_i_ka` vs
//!    `rate_a`).
//!
//! Never delete a warning without carrying the data or proving it restated
//! the model — the parity gate will catch the former, but only on cells that
//! reach zero.
//!
//! # Gotchas that cost time
//!
//! - **Payload prep drops silently.** `transmission_payloads` converts each
//!   MATPOWER case *into the source format* first, and that leg's warnings
//!   are not counted. A fidelity fix upstream (e.g. caps or dclines
//!   surviving into the payload) therefore changes *other* rows' counts —
//!   that is the "drop late, loudly" trade, and it is intentional: rows pay
//!   for the data they now carry.
//! - **Baseline edits must be derived, not tuned.** Regenerate with
//!   `MAX_WARNING_DETAILS_PER_PAIR` lifted, attribute every count delta to a
//!   warning text, and write the derivation into the comment above the
//!   arrays. The test asserting observed == baseline on both `main` and the
//!   branch is what makes a diff reviewable.
//! - **Element order is not identity.** pandapower regroups lines and
//!   trafos; several formats sort by id. The parity check sorts by identity
//!   key before diffing; anything order-sensitive you add must do the same.
//! - **`charging: None` and the symmetric split are one fact**, as are a
//!   rating in MVA and the same rating in amps through the file's own
//!   voltage. Canonicalize representations before comparing.
//! - **Formats state solved values in different places.** pandapower puts
//!   reactive output in `res_gen`, not the `gen` input table; PSS/E states a
//!   DC line's received power only through `SETVL`/`RDC`/`VSCHD`. Look for a
//!   result-table or derived spelling before concluding a format "cannot
//!   carry" a field.
//!
//! # Known structural gaps (checked, not worth faking)
//!
//! PowerWorld aux HVDC is unimplemented because no vendored export states
//! its DC vocabulary — inventing field names would fake a green. dss deck
//! tokens (`vminpu`, `pf`, source `MVAsc`) and BMOPF named terminals have no
//! slots in their neighbors; those rows stay honestly yellow.
//!
//! CGMES identifies every object by an mRID and states classes, containers,
//! and limit types as objects of their own. The writer derives that required
//! exchange context when a case format provides only a bus-branch model; the
//! warnings that remain name source data CGMES cannot represent.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use powerio::BalancedNetwork;
use powerio_cli::invariants::{
    DistributionCore, YbusUnavailable, distribution_core, distribution_value,
    electrical_change_up_to_relabeling, injection_change_within, model_diffs, transmission_core,
    transmission_value, ybus_change_within,
};
use powerio_dist::{DistTargetFormat, MulticonductorNetwork};

const REPORT_ENV: &str = "POWERIO_CONVERSION_MATRIX_REPORT";
const DETAILS_ENV: &str = "POWERIO_CONVERSION_MATRIX_DETAILS";
const REPORT_MARKER: &str = "<!-- powerio-conversion-matrix-report -->";
const DETAILS_MARKER: &str = "<!-- powerio-conversion-matrix-details -->";
const MAX_WARNING_DETAILS_PER_PAIR: usize = 6;
const SOURCE_PARSE: &str = "source parse";
const TARGET_WRITE: &str = "target write";
const TARGET_READBACK: &str = "target readback";
static DSS_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TextEmission {
    text: String,
    diagnostics: Vec<powerio_core::Diagnostic>,
}

impl TextEmission {
    fn render_diagnostics(&self) -> Vec<String> {
        powerio_tx::diagnostics::render_diagnostics(&self.diagnostics)
    }
}

fn text_emission(
    result: powerio_core::EmitResult,
    primary_suffix: Option<&str>,
) -> Result<TextEmission, String> {
    let diagnostics = result.diagnostics().to_vec();
    let powerio_core::EmittedOutput::Memory { artifacts } = result.into_output() else {
        return Err("memory destination returned path output".to_owned());
    };
    let mut primary = None;
    for artifact in artifacts {
        let is_primary =
            primary_suffix.is_none_or(|suffix| artifact.name().as_str().ends_with(suffix));
        if is_primary {
            if primary.is_some() {
                return Err("emission returned several primary artifacts".to_owned());
            }
            primary = Some(
                String::from_utf8(artifact.into_bytes())
                    .map_err(|err| format!("emitted text is not UTF-8: {err}"))?,
            );
        }
    }
    Ok(TextEmission {
        text: primary.ok_or_else(|| "emission returned no primary artifact".to_owned())?,
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

struct TransmissionEmission {
    emitted: Emitted,
    diagnostics: Vec<String>,
}

/// Write `network` as `format` through the facade, which is the operation a
/// caller has: one token selects the writer, and a directory format writes its
/// inventory rather than one document.
fn emit_transmission(
    network: &BalancedNetwork,
    format: TransmissionFormat,
) -> Result<TransmissionEmission, String> {
    let module = powerio_core::PioModule::new(network.clone());
    if format.emission == Emission::Directory {
        let directory = tempfile::tempdir().map_err(|err| err.to_string())?;
        let result = powerio::emit(
            &module,
            format.token,
            powerio_core::Destination::path(directory.path().join(DIRECTORY_CASE_NAME)),
        )
        .map_err(|err| err.to_string())?;
        let diagnostics = powerio_core::render_diagnostics(result.diagnostics());
        return Ok(TransmissionEmission {
            emitted: Emitted::Directory(directory),
            diagnostics,
        });
    }
    let destination = powerio_core::Destination::memory("case").map_err(|err| err.to_string())?;
    let result =
        powerio::emit(&module, format.token, destination).map_err(|err| err.to_string())?;
    let diagnostics = powerio_core::render_diagnostics(result.diagnostics());
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
    let suffix = (target == DistTargetFormat::Dss).then_some("case.dss");
    text_emission(result, suffix)
}

fn emit_distribution_value(
    network: &MulticonductorNetwork,
    target: DistTargetFormat,
) -> Result<TextEmission, String> {
    emit_distribution_module(&powerio_core::PioModule::new(network.clone()), target)
}

#[test]
fn conversion_matrix_report_matches_baseline() {
    let report = build_report();
    assert_no_report_path_leaks(&report.markdown);
    assert_no_report_path_leaks(&report.details_markdown);
    write_env_report(REPORT_ENV, &report.markdown);
    write_env_report(DETAILS_ENV, &report.details_markdown);
    assert!(
        report.failures.is_empty(),
        "{}\n\n{}\n\n{}",
        report.failures.join("\n"),
        report.markdown,
        report.details_markdown
    );
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

struct Report {
    markdown: String,
    details_markdown: String,
    failures: Vec<String>,
}

fn build_report() -> Report {
    let transmission = run_transmission_matrix();
    let distribution = run_distribution_matrix();
    let geo = run_geo_layer_matrix();
    let mut failures = Vec::new();
    failures.extend(transmission.failures.clone());
    failures.extend(distribution.failures.clone());
    failures.extend(geo.failures.clone());

    let mut markdown = String::new();
    writeln!(markdown, "{REPORT_MARKER}").unwrap();
    writeln!(markdown).unwrap();
    writeln!(markdown, "## Conversion Matrix").unwrap();
    writeln!(markdown).unwrap();
    writeln!(markdown, "### Legend").unwrap();
    writeln!(markdown).unwrap();
    writeln!(
        markdown,
        "Cells show `X/Y`: observed warnings / expected warnings. Counts include source parse, target write, and target readback."
    )
    .unwrap();
    writeln!(markdown).unwrap();
    writeln!(
        markdown,
        "A nonzero cell means the target format cannot carry what the source stated, and every warning names the field or record it cannot carry. A write or readback warning names a field of the target format; a source parse warning, which every cell of that row shares, names what the source document itself leaves unstated. A warning that named a writer PowerIO could extend instead of a limit of the format would be a defect in this table."
    )
    .unwrap();
    writeln!(markdown).unwrap();
    writeln!(
        markdown,
        "- 🟢 `0/0`: no warnings and checked invariants held."
    )
    .unwrap();
    writeln!(
        markdown,
        "- 🟡 `X=Y`: observed warnings match the reviewed expected count, and that count is nonzero."
    )
    .unwrap();
    writeln!(
        markdown,
        "- 🔴 `X!=Y` or invariant failure: behavior changed. If warnings decreased because fidelity improved, update the expected counts in the same PR."
    )
    .unwrap();
    writeln!(
        markdown,
        "- Expected counts are the `*_WARNING_BASELINE` arrays in `powerio-cli/tests/conversion_matrix_report.rs`; accept an intentional change by editing the matching source/target entry in the same PR."
    )
    .unwrap();
    writeln!(markdown).unwrap();
    write_matrix_section(&mut markdown, "Transmission", &transmission);
    writeln!(markdown).unwrap();
    write_matrix_section(&mut markdown, "Distribution", &distribution);
    writeln!(markdown).unwrap();
    write_matrix_section(&mut markdown, "Geographic Layer", &geo);
    writeln!(markdown).unwrap();
    writeln!(
        markdown,
        "A geographic layer is a `powerio.GeoLayer`, not a network: no grid exchange format states a standalone layer, so the layer has its own table rather than a row of the network matrix. Its source is the substation coordinates of a vendored PowerWorld auxiliary file."
    )
    .unwrap();

    let mut details_markdown = String::new();
    writeln!(details_markdown, "{DETAILS_MARKER}").unwrap();
    writeln!(details_markdown).unwrap();
    writeln!(details_markdown, "## Conversion Matrix Warning Details").unwrap();
    writeln!(details_markdown).unwrap();
    writeln!(
        details_markdown,
        "This file is generated by the conversion matrix workflow. It records warning text by source to target pair for the same run that posted the PR comment."
    )
    .unwrap();
    writeln!(
        details_markdown,
        "Warning lines are tagged by phase: source parse, target write, or target readback."
    )
    .unwrap();
    writeln!(
        details_markdown,
        "Expected counts live in `powerio-cli/tests/conversion_matrix_report.rs`; update the matching baseline value in the same PR when warning count changes are intentional."
    )
    .unwrap();
    writeln!(details_markdown).unwrap();
    write_warning_summary(&mut details_markdown, "Transmission", &transmission);
    writeln!(details_markdown).unwrap();
    write_warning_summary(&mut details_markdown, "Distribution", &distribution);
    writeln!(details_markdown).unwrap();
    write_warning_summary(&mut details_markdown, "Geographic Layer", &geo);

    Report {
        markdown,
        details_markdown,
        failures,
    }
}

#[derive(Clone)]
struct MatrixReport {
    sources: Vec<&'static str>,
    targets: Vec<&'static str>,
    /// Target columns split into tables, each entry a table title and the
    /// column indices it renders. Every table keeps every source row, so the
    /// split changes only how the same cells are laid out.
    groups: Vec<(&'static str, Vec<usize>)>,
    coverage: &'static str,
    cells: Vec<Vec<Cell>>,
    failures: Vec<String>,
}

#[derive(Clone)]
struct Cell {
    observed_warnings: usize,
    baseline_warnings: usize,
    failures: Vec<String>,
    warning_counts: BTreeMap<(String, String), usize>,
    /// Typed-model fields that changed across the conversion despite the leg
    /// reporting no warning. Harmless for a yellow cell (its losses are
    /// declared); fatal for a green claim — see [`Cell::ok`].
    silent_model_diffs: Vec<String>,
}

impl Cell {
    fn new(baseline_warnings: usize) -> Self {
        Self {
            observed_warnings: 0,
            baseline_warnings,
            failures: Vec::new(),
            warning_counts: BTreeMap::new(),
            silent_model_diffs: Vec::new(),
        }
    }

    fn ok(&self) -> bool {
        self.failures.is_empty()
            && self.observed_warnings == self.baseline_warnings
            && !self.claims_silent_loss()
    }

    /// A green cell asserts the pair converts losslessly, so it must earn it:
    /// zero warnings AND a typed model that survives field for field. A cell
    /// that warns is yellow and its diffs are the declared losses; a cell that
    /// is silent while the model changed is lying, and fails outright.
    fn claims_silent_loss(&self) -> bool {
        self.observed_warnings == 0 && !self.silent_model_diffs.is_empty()
    }

    fn parity(&self) -> bool {
        self.ok() && self.observed_warnings == 0
    }

    fn record_warnings(&mut self, phase: &str, warnings: &[String]) {
        self.observed_warnings += warnings.len();
        for warning in warnings {
            *self
                .warning_counts
                .entry((phase.to_string(), sanitize_report_text(warning)))
                .or_default() += 1;
        }
    }
}

/// The silent-loss appendix for a failing cell: names the fields that changed
/// under a green claim, or stays empty when the failure is a plain count/
/// invariant mismatch.
fn silent_loss_note(cell: &Cell) -> String {
    if !cell.claims_silent_loss() {
        return String::new();
    }
    format!(
        " [claims lossless but the model changed: {}]",
        cell.silent_model_diffs.join("; ")
    )
}

fn write_matrix_section(markdown: &mut String, title: &str, report: &MatrixReport) {
    writeln!(markdown, "### {title}").unwrap();
    writeln!(markdown).unwrap();
    writeln!(markdown, "{}", report.coverage).unwrap();
    for (group, columns) in &report.groups {
        if columns.is_empty() {
            continue;
        }
        writeln!(markdown).unwrap();
        writeln!(markdown, "#### {group}").unwrap();
        writeln!(markdown).unwrap();
        write!(markdown, "| Source ↓ / target → |").unwrap();
        for column in columns {
            write!(markdown, " {} |", report.targets[*column]).unwrap();
        }
        writeln!(markdown).unwrap();
        write!(markdown, "| --- |").unwrap();
        for _ in columns {
            write!(markdown, " --- |").unwrap();
        }
        writeln!(markdown).unwrap();
        for (source, row) in report.sources.iter().zip(&report.cells) {
            write!(markdown, "| {source} |").unwrap();
            for column in columns {
                write!(markdown, " {} |", cell_summary(&row[*column])).unwrap();
            }
            writeln!(markdown).unwrap();
        }
    }
}

fn cell_summary(cell: &Cell) -> String {
    let icon = if cell.parity() {
        "🟢"
    } else if cell.ok() {
        "🟡"
    } else {
        "🔴"
    };
    format!(
        "{icon} {}/{}",
        cell.observed_warnings, cell.baseline_warnings
    )
}

fn write_warning_summary(markdown: &mut String, title: &str, report: &MatrixReport) {
    writeln!(markdown, "### {title} Warning Details").unwrap();
    writeln!(markdown).unwrap();
    writeln!(
        markdown,
        "Rows list only source to target pairs that produced warnings or failed an invariant."
    )
    .unwrap();
    writeln!(markdown).unwrap();
    let mut wrote_row = false;
    for (source, row) in report.sources.iter().zip(&report.cells) {
        for (target, cell) in report.targets.iter().zip(row) {
            if cell.observed_warnings == 0 && cell.failures.is_empty() {
                continue;
            }
            wrote_row = true;
            writeln!(
                markdown,
                "- **{source} → {target}** (`{}/{}`)",
                cell.observed_warnings, cell.baseline_warnings
            )
            .unwrap();
            write_cell_details(markdown, cell);
        }
    }

    if !wrote_row {
        writeln!(markdown, "No warnings observed.").unwrap();
    }
}

fn write_cell_details(markdown: &mut String, cell: &Cell) {
    let mut details = Vec::new();
    for failure in &cell.failures {
        details.push(format!("failure: {}", sanitize_report_text(failure)));
    }

    let mut warnings: Vec<_> = cell.warning_counts.iter().collect();
    warnings.sort_by(
        |((phase_a, warning_a), count_a), ((phase_b, warning_b), count_b)| {
            count_b
                .cmp(count_a)
                .then_with(|| phase_order(phase_a).cmp(&phase_order(phase_b)))
                .then_with(|| warning_a.cmp(warning_b))
        },
    );
    for ((phase, warning), count) in warnings.iter().take(MAX_WARNING_DETAILS_PER_PAIR) {
        details.push(format!("{phase}: {count}x {warning}"));
    }
    let omitted = warnings.len().saturating_sub(MAX_WARNING_DETAILS_PER_PAIR);
    if omitted > 0 {
        let noun = if omitted == 1 { "text" } else { "texts" };
        details.push(format!("{omitted} more warning {noun}"));
    }

    if details.is_empty() {
        writeln!(markdown, "  - No warning text recorded.").unwrap();
        return;
    }
    for detail in details {
        writeln!(markdown, "  - {}", markdown_list_text(&detail)).unwrap();
    }
}

fn phase_order(phase: &str) -> usize {
    match phase {
        SOURCE_PARSE => 0,
        TARGET_WRITE => 1,
        TARGET_READBACK => 2,
        _ => 3,
    }
}

fn markdown_list_text(text: &str) -> String {
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
    let text = text
        .replace(&temp_dir, "<tmp>")
        .replace(env!("CARGO_MANIFEST_DIR"), "<crate>");
    code_object_name(&text)
}

#[test]
fn sanitize_report_text_handles_multibyte_whitespace_before_generated_dss_dir() {
    // A non-breaking space (U+00A0, 2 UTF-8 bytes) immediately before the
    // generated DSS dir marker must not panic on a non char boundary slice.
    let text = "wrote\u{a0}/tmp/xyz/powerio-conversion-matrix/case.dss";
    let sanitized = sanitize_report_text(text);
    assert_eq!(sanitized, "wrote generated DSS case.dss");
}

fn code_object_name(text: &str) -> String {
    const OBJECT_PREFIXES: &[&str] = &[
        "voltage source",
        "vsource",
        "load",
        "capacitor",
        "reactor",
        "generator",
        "shunt",
        "transformer",
        "switch",
        "linecode",
        "line",
        "bus",
    ];

    for prefix in OBJECT_PREFIXES {
        let Some(rest) = text.strip_prefix(prefix).and_then(|s| s.strip_prefix(' ')) else {
            continue;
        };
        let Some((name, suffix)) = rest.split_once(':') else {
            continue;
        };
        if name.contains(char::is_whitespace) || name.starts_with('`') {
            continue;
        }
        return format!("{prefix} `{name}`:{suffix}");
    }

    text.to_string()
}

/// Which table a target column renders in. One table of every source against
/// every target is 19 rows by 15 columns, which no PR comment renders
/// readably, so the columns are grouped by what the formats are for and every
/// group keeps every source row. No cell is dropped by the grouping.
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
            Self::Case => "Into a case format",
            Self::Exchange => "Into an exchange format",
            Self::Dataset => "Into a dataset directory",
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
const TRANSMISSION_FORMATS: [TransmissionFormat; 19] = [
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

/// The number of writable formats, which is the number of target columns.
const TRANSMISSION_TARGETS: usize = 15;

fn transmission_targets() -> impl Iterator<Item = (usize, TransmissionFormat)> {
    TRANSMISSION_FORMATS
        .into_iter()
        .enumerate()
        .filter(|(_, format)| format.emission != Emission::ReadOnly)
}

// The `→ PSLF .epc` column drops by 5 on every source whose buses carry no base
// kV: the generator voltage setpoint now rides the bus `vsched` column, which is
// per unit and needs no base, instead of being lost with `reg_kv`.
//
// The egret writer emits `dc_branch`, which its reader already read, so the four
// dclines survive a conversion into egret instead of being dropped there. The
// `egret JSON` source row pays for it: those dclines now reach the next hop and
// report what each target does with them, where before the row had none to
// carry. Same trade on `→ Surge JSON`, where the reactive limits, the loss
// model, and both terminal voltage setpoints now round trip.
//
// The `→ PowerModels JSON` column loses a blanket dcline warning that named no
// loss: every `Hvdc` field has a PowerModels slot and reads back exactly.
//
// The PSLF dc reader warns about control fields retained only in extras when
// the record states some — a real GE export with firing angles or taps — and
// no longer for the all-zero shape powerio's own writer emits, where nothing
// is retained. That is −4 (the dcline case, once per dc line) on every cell of
// the `PSLF .epc` source row, and −4 more on every `→ PSLF .epc` cell whose
// payload carries dclines (PowerModels, PSS/E, egret, Surge; −8 on PSLF→PSLF,
// which paid on both legs). What remains in those cells is the genuine EPC
// loss: no cost data, and one rate1 column for an asymmetric pmin/pmax pair.
// The canonical MATPOWER writer emits `mpc.dcline` and `mpc.dclinecost` — the
// same blocks its reader reads — so the `→ MATPOWER .m` column stops dropping
// dclines (−1 on the PowerModels, PSS/E, egret, Surge, and PSLF source rows;
// egret and Surge land on zero). The `MATPOWER .m` source row pays the honest
// price: the dcline case's lines now survive the payload-prep hop, so every
// target that cannot carry them (or their `mpc.dclinecost` usage cost, stated
// on one line and read since the reader learned the block) reports it —
// PSS/E +2 (converter detail, cost), PowerWorld +1 (drop), pandapower +1
// (drop), egret +1 (cost), Surge +2 (one Pt off its own loss model, one cost),
// PSLF +2 (asymmetric pmin, cost). The PowerModels row carries the cost too,
// +1 wherever the target has no slot for it (PSS/E, egret, Surge, PSLF).
//
// The MATPOWER writer no longer warns when a costless network omits
// `mpc.gencost`: absence is the source's own shape, not a drop, and no other
// writer warns on it. That is −6 (one per case) on the PSS/E, PowerWorld, and
// PSLF source rows, whose payloads carry no cost data. What remains in those
// three cells is the passthrough-extras drop alone.
//
// The PSS/E reader keeps an extra only when the record states more than the
// writer would synthesize on its own: circuit id `1`, a load's zero I/Y
// components restating typed p/q, and a DC record's positional name, status
// MDC, zero RDC/VSCHD, and default converter tails are not retained. A file
// powerio itself wrote reads back with no extras at all, so the PSS/E source
// row reaches MATPOWER with nothing left to drop — that cell lands on zero.
// The dropped-converter-detail warning now keys on the line's own fields (a
// received power off the setpoint, terminal voltages, reactive limits, a
// power band, a loss model) instead of the retained name, so its counts are
// unchanged everywhere. The PowerWorld readers apply the same rule (circuit
// and device ids at their positional defaults, a device type the tap already
// encodes), and the PSLF reader stops echoing whole records into extras —
// only tokens beyond the mapped fields are retained now — so all three of
// those source rows reach MATPOWER with nothing left to drop.
//
// The canonical MATPOWER writer emits the 21-column gen row (Pc1..APF) —
// standard MATPOWER, and the reader has read it all along — whenever any
// generator carries capability/ramp columns. The PowerModels source row's
// MATPOWER cell lands on zero, and the MATPOWER source row pays honestly:
// caps now survive payload prep, so the five cases that state them (all but
// PGLib 5) report the drop at every target without a slot — +5 on PSS/E,
// PowerWorld, pandapower, and PSLF, +20 on Surge (it reports per generator).
// The egret and PSLF writers dropped caps silently before; both now say so,
// which is the +5 on the MATPOWER and PowerModels rows' egret and PSLF cells.
// The MATPOWER and PowerModels rows read identically now — same payload
// data, same honest outcomes.
//
// The pandapower writer emits the `dcline` and `res_gen` tables its reader
// has read all along. Dclines now carry the sending power, the MATPOWER-
// shaped loss pair, terminal voltage setpoints, reactive limits, and the
// power cap; what the table has no column for is warned (a pmin floor, a
// received power off the line's own loss model, a usage cost) — that is the
// −1/+N reshuffle on every `→ pandapower JSON` cell with dclines, and the +1
// on the pandapower source row's PSS/E, PowerWorld, and PSLF cells, whose
// payloads now carry dclines to drop honestly. Generator reactive output
// rides `res_gen.q_mvar` — pandapower states Q as a power flow result, not
// an input — so the solved snapshot's qg survives with no warning at all.
// pandapower also enforces one voltage setpoint per bus, which MATPOWER's
// dcline rows do not: the dcline case states vf 1.01 against a generator's
// vg 1.0 at one bus and two dclines disagree at another, so the writer
// coerces to the bus's controlling setpoint and says so — +1 on the rows
// whose payloads carry those conflicting setpoints (MATPOWER, PowerModels,
// egret, Surge; the PSS/E and PSLF payloads carry uniform setpoints).
//
// A pandapower line states one rating, `max_i_ka`, carried as `rate_a`
// through the file's own vn_kv; the reader no longer stores a second copy of
// the same fact in amps, so the five per-case restatement drops leave every
// pandapower source cell (−5 each on MATPOWER, PSS/E, PowerWorld, egret,
// PSLF; egret lands on zero). What remains on that row is genuine: costs the
// targets cannot carry and the one-of-three cost set MATPOWER's
// all-or-nothing gencost drops.
// pandapower piecewise costs map both ways as `pwl_cost` ranges. The format
// states slopes over contiguous intervals and no additive objective constant;
// the reader integrates from zero at the first breakpoint. An additive
// constant does not change dispatch, so this canonical origin is not a
// conversion loss. Targets without a piecewise slot report the curve they
// cannot carry.
//
// The MATPOWER reader reads `mpc.areas` and the writer emits it, so the one
// case that states an area (PGLib 5) carries it into every payload built from
// the .m file. MATPOWER and PSS/E hold the table (their cells do not move);
// the six targets with no area table each declare the drop — +1 on the
// MATPOWER and PSS/E source rows' PowerModels, PowerWorld, egret, pandapower,
// Surge, and PSLF cells. The PowerModels row does not move: its reader reads
// no areas, so its payloads carry none.
//
// A PSS/E area is an interchange control area. The neutral area record now
// carries that classification so XIIDM can preserve it. MATPOWER's legacy
// `mpc.areas` table has only the area number and reference bus, so the PSS/E
// source row's MATPOWER cell adds one declared field drop.
// Rows are sources and columns are targets, both in `TRANSMISSION_FORMATS`
// order. Each cell sums the source parse, target write, and target readback
// warnings over the six MATPOWER cases, or over the one vendored case a read
// only source parses.
//
// The eight formats this table already held keep the counts recorded above,
// except where a fix named in this file's history changed them. The eight
// columns added here are new counts, derived by running the matrix and
// attributing each one to its warning texts in the details report:
//
// - The `→ PSS/E RAWX 35` column matches `→ PSS/E .raw 33` wherever the two
//   revisions state the same fields, and differs where revision 35 states
//   more: a RAWX file carries branch names and twelve rating sets, and the
//   revision 33 record layout carries neither.
// - The `→ XIIDM 1.17` and `→ JIIDM 1.17` columns state, per case, the fields
//   IIDM has no attribute for (generator and DC line dispatch cost, angle
//   bounds, area swing bus and tolerance, the reverse power bound) and the
//   values IIDM requires and a case format leaves unstated (the case date,
//   the forecast distance, the source format, the validation level, and the
//   substation and voltage level hierarchy the writer derives from buses).
//   XIIDM and JIIDM apply the same conversion diagnostics.
// - The `→ CGMES 3.0` column reports source fields absent from CGMES. Required
//   mRIDs, classes, containers, state records, and limit helper objects are
//   deterministic output structure and do not count as source-data losses.
// - The `→ UCTE-DEF .uct` column states the ten voltage levels a node code
//   admits, the node code itself, the absent shunt and cost records, and the
//   50 Hz synchronous area frequency.
// - The `→ PyPSA CSV` column states the reactive limits, capability columns,
//   and rating sets a PyPSA component CSV has no column for.
// - The `→ GridFM Parquet` column states the detected projection into GridFM's
//   four fixed tables: dense bus renumbering, nodal load and shunt aggregation,
//   absent equipment and metadata fields, fixed quadratic costs, and branch
//   flows evaluated when the source states no solution. Generated component
//   identities are indexing data rather than source fields and do not count as
//   a projection loss. A native GridFM network writes without findings.
// - The `IEEE CDF`, `GO Challenge 3 JSON`, and `DeepMind OPFData JSON` rows
//   parse one vendored case each, so their counts are per case rather than per
//   six.
//
// The counts below are the same table after the per format warning review, and
// every delta is attributed to one of these changes:
//
// - A CGMES document set whose FullModel states this writer's own modeling
//   authority was produced by fresh emission, so its header identity, version,
//   creation time, and dependency references, and the containers, limit types,
//   islands, and subordinate mRIDs in its body, are values the writer
//   synthesized and not data a source case stated. Reporting them as unmapped
//   made every conversion through CGMES declare a loss of something never
//   stated. That is the whole of the `→ CGMES 3.0` column's collapse (301 to
//   64 on the MATPOWER row) and of the `CGMES 3.0` row's (244 to 7 on the
//   MATPOWER cell). A third party set, whose authority differs, still reports
//   every one of them.
// - `rate_b` and `rate_c` are written as CGMES temporary limits admissible for
//   twenty minutes and one minute, the durations the reference importer
//   assigns those two ratings, instead of a TATL and a tripping current. A
//   tripping current states a protection setting rather than an admissible
//   loading, and the reader canonicalized it back to a TATL and said so once
//   per limit: 74 per `→ CGMES 3.0` cell.
// - The UCTE reader no longer keeps a second copy of a permanent current limit
//   in `current_ratings`: `rate_a` in MVA and the same limit in ampere through
//   the element's own voltage are one fact, and the writer divides by the
//   voltage the reader multiplied by. That is −5 on eight cells of the
//   `UCTE-DEF .uct` row and −71 on its CGMES cell, all of them a target
//   reporting the drop of a restatement.
// - A PowerWorld `.aux` Gen row states a generator's cost as a cubic
//   input-output curve (`GenCostModel`, `GenFuelCost`, `GenFixedCost`,
//   `GenIOB`, `GenIOC`, `GenIOD`), the vocabulary the vendored export states,
//   so a polynomial cost now survives a conversion into aux at a unit fuel
//   price. The `→ PowerWorld .aux` column pays +3 per cell for what the row
//   still has no field for (a piecewise curve, a startup and shutdown cost,
//   and the coefficient count a four slot curve returns padded), and the
//   `PowerWorld .aux` source row pays honestly for the data it now carries:
//   +6 wherever the target has no cost field, +23 on XIIDM and JIIDM, which
//   report it per generator, and +1 on MATPOWER, where the dcline case's two
//   piecewise rows do not survive the aux hop and `mpc.gencost` is
//   all-or-nothing.
// - `GO Challenge 3 JSON` is a new source row. A problem statement parses as
//   the security constrained commitment it states, and the row runs the
//   network that problem is stated on into every target.
// - `PSS/E .raw 32` is a new read-only source row over PowSybl's eight-bus
//   export. Its two source warnings are the unmodeled OWNER and ZONE sections.
//   Target-specific warnings name area attributes, switched-shunt and tap
//   control, the self-loop UCTE cannot state, and each target's synthesized
//   defaults. The electrical and core invariants hold after reducing the
//   source to records the target format actually defines.
// - MATPOWER carries only aggregate bus GS/BS values. Two IIDM payload cases
//   contain switched shunt controls, so the XIIDM, JIIDM, and CGMES source
//   rows each add two warnings in the MATPOWER column rather than silently
//   discarding the control records.
//
// The current exchange-format baselines follow these representation rules:
//
// - XIIDM, CGMES, and UCTE state electrical quantities in physical units and
//   do not state a system MVA base. Their readers use 100 MVA for the IR's
//   internal per-unit normalization without reporting a missing source datum.
//   XIIDM emission reports a non-100 MVA IR base because conversion then
//   changes the chosen normalization.
// - Generated CGMES operational-limit helper objects do not become source
//   metadata on readback. Limit type objects keep the PATL or TATL name common
//   importers require, while generated limit set objects omit an unrepresented
//   display name. Typed groups retain loading values, durations, and names.
//   Third-party helper metadata still produces a diagnostic when no typed
//   field can retain it.
// - CGMES retains every source Substation. XIIDM and JIIDM require the voltage
//   levels at both transformer ends to share one substation, so their writers
//   join only the output container groups and report that target-specific
//   hierarchy change; transformer electrical data remains unchanged.
// - UCTE generation bounds must contain the dispatch. Emission widens an
//   inconsistent interval and reports the substitution once; readback then
//   satisfies the format rule. UCTE-derived country areas use their names and
//   classifications rather than a synthetic source identity, and an
//   out-of-service generator still contributes its plant-type letter.
// - Bus-branch targets and PyPSA report one grouped warning when case metadata,
//   detailed topology, source-assigned identities, geographic metadata, or
//   solver metadata crosses into a format without a complete exchange model.
// - RAWX `subterm` rows use the type, buses, and local identifier selected for
//   their electrical equipment row. Missing PSS/E node numbers are allocated
//   before exact regulation targets are written, and that target requirement
//   produces one grouped default diagnostic.
const TRANSMISSION_WARNING_BASELINE: [[usize; TRANSMISSION_TARGETS]; 19] = [
    [0, 1, 15, 15, 18, 7, 14, 23, 14, 70, 70, 57, 42, 15, 31], // MATPOWER .m
    [0, 0, 15, 15, 17, 6, 13, 22, 13, 69, 69, 56, 41, 14, 30], // PowerModels JSON
    [1, 1, 0, 0, 2, 1, 3, 1, 2, 32, 32, 12, 29, 9, 28],        // PSS/E .raw 33
    [1, 1, 0, 0, 2, 1, 3, 1, 2, 32, 32, 12, 29, 9, 28],        // PSS/E RAWX 35
    [1, 0, 6, 6, 0, 0, 8, 0, 6, 54, 54, 30, 33, 13, 27],       // PowerWorld .aux
    [0, 0, 9, 9, 12, 0, 7, 1, 7, 68, 68, 36, 36, 9, 25],       // egret JSON
    [0, 0, 7, 7, 8, 0, 0, 0, 7, 55, 55, 35, 52, 8, 18],        // pandapower JSON
    [0, 0, 9, 9, 12, 0, 6, 0, 7, 67, 67, 36, 36, 9, 25],       // Surge JSON
    [0, 0, 1, 1, 1, 0, 4, 3, 0, 38, 38, 11, 28, 8, 27],        // PSLF .epc
    [9, 7, 33, 33, 8, 7, 9, 7, 8, 0, 0, 179, 60, 15, 53],      // XIIDM 1.17
    [9, 7, 33, 33, 8, 7, 9, 7, 8, 0, 0, 179, 60, 15, 53],      // JIIDM 1.17
    [8, 6, 67, 42, 6, 6, 7, 6, 6, 44, 44, 0, 51, 13, 51],      // CGMES 3.0
    [18, 12, 6, 6, 18, 12, 6, 12, 18, 36, 36, 12, 0, 12, 36],  // UCTE-DEF .uct
    [0, 6, 14, 14, 14, 6, 9, 6, 12, 70, 70, 38, 38, 0, 15],    // PyPSA CSV
    [6, 0, 14, 14, 14, 6, 9, 0, 12, 64, 64, 32, 40, 12, 0],    // GridFM Parquet
    [5, 4, 2, 2, 4, 4, 5, 4, 6, 15, 15, 9, 10, 6, 12],         // PSS/E .raw 32
    [8, 8, 6, 6, 8, 8, 9, 8, 8, 13, 13, 8, 14, 9, 11],         // IEEE CDF
    [6, 5, 7, 7, 9, 5, 7, 5, 7, 8, 8, 7, 14, 9, 11],           // GO Challenge 3 JSON
    [3, 2, 5, 5, 5, 3, 4, 2, 4, 32, 32, 8, 32, 5, 4],          // DeepMind OPFData JSON
];

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
    parse_warnings: Vec<String>,
}

fn run_transmission_matrix() -> MatrixReport {
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
    let mut cells = Vec::new();
    let mut failures = Vec::new();

    for (source_idx, source) in TRANSMISSION_FORMATS.iter().enumerate() {
        let payloads = transmission_payloads(*source);
        let mut row = Vec::new();
        for (column, (target_idx, target)) in transmission_targets().enumerate() {
            let mut cell = Cell::new(TRANSMISSION_WARNING_BASELINE[source_idx][target_idx]);
            debug_assert_eq!(column, target_idx);
            match &payloads {
                Ok(payloads) => {
                    for payload in payloads {
                        cell.record_warnings(SOURCE_PARSE, &payload.parse_warnings);
                        validate_transmission_pair(payload, target, &mut cell);
                    }
                }
                Err(err) => cell.failures.push(err.clone()),
            }
            if !cell.ok() {
                failures.push(format!(
                    "transmission {} -> {}: observed {} warnings, baseline {}; {}{}",
                    source.name,
                    target.name,
                    cell.observed_warnings,
                    cell.baseline_warnings,
                    cell.failures.join("; "),
                    silent_loss_note(&cell),
                ));
            }
            row.push(cell);
        }
        cells.push(row);
    }

    MatrixReport {
        sources,
        targets,
        groups,
        coverage: "Six generated cases per writable source row; one vendored case per read-only source row.",
        cells,
        failures,
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
            parse_warnings: parsed.warnings,
        }]);
    }
    TRANSMISSION_CASES
        .iter()
        .map(|(label, rel)| {
            let base =
                parse_matpower_file(data(rel)).map_err(|err| format!("parse {rel}: {err}"))?;
            let rendered = emit_transmission(&base, format)
                .map_err(|err| format!("write {rel} as {}: {err}", format.name))?;
            let parsed = parse_transmission_emitted(&rendered.emitted, format)
                .map_err(|err| format!("read generated {rel} as {}: {err}", format.name))?;
            Ok(TransmissionPayload {
                label,
                network: parsed.network,
                parse_warnings: parsed.warnings,
            })
        })
        .collect()
}

fn validate_transmission_pair(
    payload: &TransmissionPayload,
    target: TransmissionFormat,
    cell: &mut Cell,
) {
    let conversion = match emit_transmission(&payload.network, target) {
        Ok(conversion) => conversion,
        Err(err) => {
            cell.failures.push(format!(
                "{} did not write as {}: {err}",
                payload.label, target.name
            ));
            return;
        }
    };
    cell.record_warnings(TARGET_WRITE, &conversion.diagnostics);
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
    cell.record_warnings(TARGET_READBACK, &parsed.warnings);
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
    record_model_diffs(
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
    // status field, so an out of service machine states zero generation at a
    // node the reader still sees as a generator bus, and several machines at
    // one bus state one generation.
    let mut held = network.clone();
    held.shunts_mut().clear();
    held.branches_mut()
        .retain(|branch| branch.from != branch.to);
    for generator in held.generators_mut() {
        if !generator.in_service {
            generator.in_service = true;
            generator.pg = 0.0;
            generator.qg = 0.0;
        }
    }
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
        .filter(|bus| bus.kind == powerio_tx::BusType::Ref && !generator_buses.contains(&bus.id))
        .map(|bus| bus.id)
        .collect::<Vec<_>>();
    for bus in empty_references {
        held.generators_mut().push(powerio_tx::Generator::new(bus));
    }
    held
}

/// Record every typed-model field the conversion changed. Yellow cells keep
/// these as context (their warnings declare the losses); a cell claiming green
/// fails on any of them — that is what keeps the matrix from going farcically
/// green by suppressing warnings instead of carrying data.
fn record_model_diffs(
    label: &str,
    before: &serde_json::Value,
    after: &serde_json::Value,
    cell: &mut Cell,
) {
    cell.silent_model_diffs.extend(
        model_diffs(before, after)
            .iter()
            .map(|d| format!("{label}: {d}")),
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

const DISTRIBUTION_FORMATS: [DistributionFormat; 3] = [
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
];

// Counts include conductor remapping, explicit neutral materialization, and
// fields the target cannot represent. Phase voltage limits survive the PMD
// projection; OpenDSS reports their omission. Retained PMD neutral limits
// require an additional diagnostic only when they carry nondefault data.
// PMD emergency transformer ratings remain available for targets to report.
const DISTRIBUTION_WARNING_BASELINE: [[usize; 3]; 3] = [[0, 66, 88], [21, 0, 16], [27, 44, 0]];

const DISTRIBUTION_CASES: [(&str, &str, DistributionFormat); 7] = [
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
];

struct DistributionPayload {
    label: &'static str,
    network: MulticonductorNetwork,
    module: powerio_core::PioModule<MulticonductorNetwork>,
    parse_warnings: Vec<String>,
    core: DistributionCore,
}

/// The geographic layer's own matrix: one source and one target.
///
/// A layer is a `powerio.GeoLayer` rather than a network, and no grid exchange
/// format states a standalone layer, so there is no cell to share with the
/// network matrix. The one cell holds the canonical `.geo.json` document to the
/// same rule every other cell answers to: what the source stated survives, and
/// a warning names what the target cannot state.
fn run_geo_layer_matrix() -> MatrixReport {
    const SOURCE: &str = "PowerWorld .aux substation coordinates";
    const TARGET: &str = "geo-json";
    let mut cell = Cell::new(GEO_LAYER_WARNING_BASELINE);
    let mut failures = Vec::new();
    match geo_layer_cell(&mut cell) {
        Ok(()) => {}
        Err(err) => cell.failures.push(err),
    }
    if !cell.ok() {
        failures.push(format!(
            "geo layer {SOURCE} -> {TARGET}: observed {} warnings, baseline {}; {}{}",
            cell.observed_warnings,
            cell.baseline_warnings,
            cell.failures.join("; "),
            silent_loss_note(&cell),
        ));
    }
    MatrixReport {
        sources: vec![SOURCE],
        targets: vec![TARGET],
        groups: vec![("Into the geographic layer document", vec![0])],
        coverage: "One case per source row.",
        cells: vec![vec![cell]],
        failures,
    }
}

/// The layer states no loss of its own: every feature the aux substation block
/// places has a `.geo.json` feature.
const GEO_LAYER_WARNING_BASELINE: usize = 0;

fn geo_layer_cell(cell: &mut Cell) -> Result<(), String> {
    let text = std::fs::read_to_string(data("powerworld/ACTIVSg200.aux"))
        .map_err(|err| format!("read the aux fixture: {err}"))?;
    let layer = powerio::to_geo_layer_from_aux_text(&text)
        .map_err(|err| format!("read the aux substation coordinates: {err}"))?;
    if layer.features.is_empty() {
        return Err("the aux fixture states no substation coordinates".to_owned());
    }
    let module = powerio_core::PioModule::new(powerio::PioValue::GeoLayer(layer.clone()));
    let destination = powerio_core::Destination::memory("layer").map_err(|err| err.to_string())?;
    let result = powerio::emit(&module, "geo-json", destination).map_err(|err| err.to_string())?;
    cell.record_warnings(
        TARGET_WRITE,
        &powerio_core::render_diagnostics(result.diagnostics()),
    );
    let powerio_core::EmittedOutput::Memory { mut artifacts } = result.into_output() else {
        return Err("a memory destination returned path output".to_owned());
    };
    let artifact = artifacts
        .pop()
        .filter(|_| artifacts.is_empty())
        .ok_or_else(|| "geo layer emission returned several artifacts".to_owned())?;
    let source = powerio_core::Source::from_memory("layer.geo.json", artifact.into_bytes())
        .map_err(|err| err.to_string())?;
    let options = powerio::ParseOptions::default()
        .format("geo-json")
        .map_err(|err| err.to_string())?;
    let back = powerio::parse_with_options(source, &options).map_err(|err| err.to_string())?;
    cell.record_warnings(
        TARGET_READBACK,
        &powerio_core::render_diagnostics(back.diagnostics()),
    );
    let powerio::PioValue::GeoLayer(read_back) = &back.value() else {
        return Err(format!(
            "the geo layer document parsed as {}",
            back.value().type_name()
        ));
    };
    record_model_diffs(
        "aux substation coordinates",
        &serde_json::to_value(&layer).map_err(|err| err.to_string())?,
        &serde_json::to_value(read_back).map_err(|err| err.to_string())?,
        cell,
    );
    Ok(())
}

fn run_distribution_matrix() -> MatrixReport {
    let formats: Vec<_> = DISTRIBUTION_FORMATS.iter().map(|fmt| fmt.name).collect();
    let mut cells = Vec::new();
    let mut failures = Vec::new();

    for (source_idx, source) in DISTRIBUTION_FORMATS.iter().enumerate() {
        let payloads = distribution_payloads(*source);
        let mut row = Vec::new();
        for (target_idx, target) in DISTRIBUTION_FORMATS.iter().enumerate() {
            let mut cell = Cell::new(DISTRIBUTION_WARNING_BASELINE[source_idx][target_idx]);
            match &payloads {
                Ok(payloads) => {
                    for payload in payloads {
                        cell.record_warnings(SOURCE_PARSE, &payload.parse_warnings);
                        validate_distribution_pair(payload, *target, &mut cell);
                    }
                }
                Err(err) => cell.failures.push(err.clone()),
            }
            if !cell.ok() {
                failures.push(format!(
                    "distribution {} -> {}: observed {} warnings, baseline {}; {}{}",
                    source.name,
                    target.name,
                    cell.observed_warnings,
                    cell.baseline_warnings,
                    cell.failures.join("; "),
                    silent_loss_note(&cell),
                ));
            }
            row.push(cell);
        }
        cells.push(row);
    }

    let groups = vec![("Into a distribution format", (0..formats.len()).collect())];
    MatrixReport {
        sources: formats.clone(),
        targets: formats,
        groups,
        coverage: "Seven cases per source row.",
        cells,
        failures,
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
            let parsed = parse_distribution_text(&rendered.text, format)
                .map_err(|err| format!("read generated {rel} as {}: {err}", format.name))?;
            let core = distribution_core(&parsed.network);
            Ok(DistributionPayload {
                label,
                network: parsed.network,
                module: parsed.module,
                parse_warnings: parsed.warnings,
                core,
            })
        })
        .collect()
}

fn validate_distribution_pair(
    payload: &DistributionPayload,
    target: DistributionFormat,
    cell: &mut Cell,
) {
    // The diagonal echoes the retained source through the module, the same
    // byte identity tier the library promises; off diagonal writes are
    // canonical either way.
    let conversion = match emit_distribution_module(&payload.module, target.target) {
        Ok(conversion) => conversion,
        Err(err) => {
            cell.failures
                .push(format!("{} emit as {}: {err}", payload.label, target.name));
            return;
        }
    };
    cell.record_warnings(TARGET_WRITE, &conversion.render_diagnostics());
    match parse_distribution_text(&conversion.text, target) {
        Ok(parsed) => {
            cell.record_warnings(TARGET_READBACK, &parsed.warnings);
            let actual = distribution_core(&parsed.network);
            if !core_survives(&payload.core, &actual, target.target) {
                cell.failures.push(format!(
                    "{} core changed for {}: before {:?}, after {:?}",
                    payload.label, target.name, payload.core, actual
                ));
            }
            record_model_diffs(
                payload.label,
                &distribution_value(&payload.network),
                &distribution_value(&parsed.network),
                cell,
            );
        }
        Err(err) => cell.failures.push(format!(
            "{} output did not parse as {}: {err}",
            payload.label, target.name
        )),
    }
}

struct DistParsed {
    network: MulticonductorNetwork,
    warnings: Vec<String>,
    module: powerio_core::PioModule<MulticonductorNetwork>,
}

fn dist_module_to_parsed(module: powerio_core::PioModule<MulticonductorNetwork>) -> DistParsed {
    DistParsed {
        warnings: powerio_dist::diagnostics::render_diagnostics(module.diagnostics()),
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

fn parse_distribution_text(
    text: &str,
    format: DistributionFormat,
) -> Result<DistParsed, powerio_core::Error> {
    if format.target != DistTargetFormat::Dss {
        let source = powerio_core::Source::from_memory("<memory>", text.as_bytes().to_vec())?
            .with_format(powerio_core::FormatId::new(
                format.token.to_ascii_lowercase().replace('_', "-"),
            )?);
        return powerio_dist::parse(source).map(dist_module_to_parsed);
    }

    let dir = std::env::temp_dir().join("powerio-conversion-matrix");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!(
        "case-{}.dss",
        DSS_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, text).unwrap();
    let parsed = load_distribution_case(&path, format.token);
    let _ = std::fs::remove_file(path);
    parsed
}

/// Whether the core survived the round trip.
///
/// Everything must match, with one allowance: a dss target splits a load whose
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
    warnings: Vec<String>,
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
    let warnings = module
        .diagnostics()
        .iter()
        .map(|d| format!("{}: {}", d.code(), d.message()))
        .collect();
    let network = balanced_network(module.value())
        .ok_or_else(|| format!("{from} parsed as {}", module.value().type_name()))?
        .clone();
    Ok(ParsedTransmission { network, warnings })
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
