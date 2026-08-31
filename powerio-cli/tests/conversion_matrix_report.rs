//! The conversion matrix: every source format converted into every target,
//! with the losses accounted for. This file is both the CI gate and the
//! generator of the PR comment; the notes here are for whoever works on a
//! cell next.
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

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use powerio::BalancedNetwork;
use powerio_cli::invariants::{
    DistributionCore, TransmissionCore, YbusUnavailable, distribution_core, distribution_value,
    injection_change, model_diffs, transmission_core, transmission_value, ybus_change,
};
use powerio_dist::{DistTargetFormat, MulticonductorNetwork};
use powerio_tx::TargetFormat;

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

fn emit_transmission_value(
    network: &BalancedNetwork,
    target: TargetFormat,
) -> Result<TextEmission, String> {
    let module = powerio_core::PioModule::new(network.clone());
    let destination = powerio_core::Destination::memory("case").map_err(|err| err.to_string())?;
    let result = powerio_tx::emit(&module, target, destination).map_err(|err| err.to_string())?;
    text_emission(result, None)
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
    let mut failures = Vec::new();
    failures.extend(transmission.failures.clone());
    failures.extend(distribution.failures.clone());

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
    case_count: usize,
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
    writeln!(markdown, "{} cases.", report.case_count).unwrap();
    writeln!(markdown).unwrap();
    write!(markdown, "| Source ↓ / target → |").unwrap();
    for format in &report.targets {
        write!(markdown, " {format} |").unwrap();
    }
    writeln!(markdown).unwrap();
    write!(markdown, "| --- |").unwrap();
    for _ in &report.targets {
        write!(markdown, " --- |").unwrap();
    }
    writeln!(markdown).unwrap();
    for (source, row) in report.sources.iter().zip(&report.cells) {
        write!(markdown, "| {source} |").unwrap();
        for cell in row {
            write!(markdown, " {} |", cell_summary(cell)).unwrap();
        }
        writeln!(markdown).unwrap();
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
    writeln!(markdown, "#### {title} Warning Details").unwrap();
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
        details.push(format!("{omitted} more warning texts"));
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

#[derive(Clone, Copy)]
struct TransmissionFormat {
    name: &'static str,
    token: &'static str,
    target: TargetFormat,
}

const TRANSMISSION_FORMATS: [TransmissionFormat; 8] = [
    TransmissionFormat {
        name: "MATPOWER .m",
        token: "matpower",
        target: TargetFormat::Matpower,
    },
    TransmissionFormat {
        name: "PowerModels JSON",
        token: "powermodels-json",
        target: TargetFormat::PowerModelsJson,
    },
    TransmissionFormat {
        name: "PSS/E .raw",
        token: "psse",
        target: TargetFormat::Psse { rev: 33 },
    },
    TransmissionFormat {
        name: "PowerWorld .aux",
        token: "powerworld",
        target: TargetFormat::PowerWorld,
    },
    TransmissionFormat {
        name: "egret JSON",
        token: "egret-json",
        target: TargetFormat::EgretJson,
    },
    TransmissionFormat {
        name: "pandapower JSON",
        token: "pandapower-json",
        target: TargetFormat::PandapowerJson,
    },
    TransmissionFormat {
        name: "Surge JSON",
        token: "surge-json",
        target: TargetFormat::SurgeJson,
    },
    TransmissionFormat {
        name: "PSLF .epc",
        token: "pslf",
        target: TargetFormat::Pslf,
    },
];

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
// pandapower piecewise costs map both ways (#360): a piecewise case's cost
// now survives the into-pandapower hop as `pwl_cost` ranges. Reading that
// payload declares the unstated absolute level once per parse, which every
// cell of the source row counts (+1 across the row; the pandapower target
// cell counts it twice, parse and readback), and the targets with no
// piecewise slot declare the drop they used to get for free. The MATPOWER
// cell nets zero: the curve it now receives replaces the cost drop it used
// to declare.
//
// The MATPOWER reader reads `mpc.areas` and the writer emits it, so the one
// case that states an area (PGLib 5) carries it into every payload built from
// the .m file. MATPOWER and PSS/E hold the table (their cells do not move);
// the six targets with no area table each declare the drop — +1 on the
// MATPOWER and PSS/E source rows' PowerModels, PowerWorld, egret, pandapower,
// Surge, and PSLF cells. The PowerModels row does not move: its reader reads
// no areas, so its payloads carry none.
const TRANSMISSION_WARNING_BASELINE: [[usize; 8]; 8] = [
    [0, 1, 15, 15, 7, 15, 23, 14],
    [0, 0, 15, 14, 6, 14, 22, 13],
    [0, 1, 0, 2, 1, 3, 1, 2],
    [0, 0, 0, 0, 0, 2, 0, 0],
    [0, 0, 9, 9, 0, 8, 1, 7],
    [1, 1, 8, 8, 1, 2, 1, 8],
    [0, 0, 9, 9, 0, 7, 0, 7],
    [0, 0, 1, 1, 0, 4, 3, 0],
];

const DEEPMIND_OPFDATA_WARNING_BASELINE: [usize; 8] = [3, 2, 5, 5, 3, 4, 2, 4];

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
    core: TransmissionCore,
}

fn run_transmission_matrix() -> MatrixReport {
    let mut sources: Vec<_> = TRANSMISSION_FORMATS.iter().map(|fmt| fmt.name).collect();
    let targets = TRANSMISSION_FORMATS.iter().map(|fmt| fmt.name).collect();
    let mut cells = Vec::new();
    let mut failures = Vec::new();

    for (source_idx, source) in TRANSMISSION_FORMATS.iter().enumerate() {
        let payloads = transmission_payloads(*source);
        let mut row = Vec::new();
        for (target_idx, target) in TRANSMISSION_FORMATS.iter().enumerate() {
            let mut cell = Cell::new(TRANSMISSION_WARNING_BASELINE[source_idx][target_idx]);
            match &payloads {
                Ok(payloads) => {
                    for payload in payloads {
                        cell.record_warnings(SOURCE_PARSE, &payload.parse_warnings);
                        validate_transmission_pair(payload, *target, &mut cell);
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

    let source = "DeepMind OPFData JSON";
    let payload = deepmind_opfdata_payload();
    let mut row = Vec::new();
    for (target_idx, target) in TRANSMISSION_FORMATS.iter().enumerate() {
        let mut cell = Cell::new(DEEPMIND_OPFDATA_WARNING_BASELINE[target_idx]);
        match &payload {
            Ok(payload) => {
                cell.record_warnings(SOURCE_PARSE, &payload.parse_warnings);
                validate_transmission_pair(payload, *target, &mut cell);
            }
            Err(err) => cell.failures.push(err.clone()),
        }
        if !cell.ok() {
            failures.push(format!(
                "transmission {source} -> {}: observed {} warnings, baseline {}; {}{}",
                target.name,
                cell.observed_warnings,
                cell.baseline_warnings,
                cell.failures.join("; "),
                silent_loss_note(&cell),
            ));
        }
        row.push(cell);
    }
    sources.push(source);
    cells.push(row);

    MatrixReport {
        sources,
        targets,
        case_count: TRANSMISSION_CASES.len() + 1,
        cells,
        failures,
    }
}

fn deepmind_opfdata_payload() -> Result<TransmissionPayload, String> {
    let parsed = parse_transmission_file(data("opfdataset/example_0.json"), Some("opfdata-json"))
        .map_err(|err| format!("parse DeepMind OPFData fixture: {err}"))?;
    let core = transmission_core(&parsed.network);
    Ok(TransmissionPayload {
        label: "official case 14 example",
        network: parsed.network,
        parse_warnings: parsed.warnings,
        core,
    })
}

fn transmission_payloads(format: TransmissionFormat) -> Result<Vec<TransmissionPayload>, String> {
    TRANSMISSION_CASES
        .iter()
        .map(|(label, rel)| {
            let base =
                parse_matpower_file(data(rel)).map_err(|err| format!("parse {rel}: {err}"))?;
            let rendered = emit_transmission_value(&base, format.target)
                .map_err(|err| format!("write {rel} as {}: {err}", format.name))?;
            let parsed = parse_transmission_str(&rendered.text, format.token)
                .map_err(|err| format!("read generated {rel} as {}: {err}", format.name))?;
            let core = transmission_core(&parsed.network);
            Ok(TransmissionPayload {
                label,
                network: parsed.network,
                parse_warnings: parsed.warnings,
                core,
            })
        })
        .collect()
}

fn validate_transmission_pair(
    payload: &TransmissionPayload,
    target: TransmissionFormat,
    cell: &mut Cell,
) {
    match emit_transmission_value(&payload.network, target.target) {
        Ok(conversion) => {
            cell.record_warnings(TARGET_WRITE, &conversion.render_diagnostics());
            match parse_transmission_str(&conversion.text, target.token) {
                Ok(parsed) => {
                    cell.record_warnings(TARGET_READBACK, &parsed.warnings);
                    let actual = transmission_core(&parsed.network);
                    if actual != payload.core {
                        cell.failures.push(format!(
                            "{} core changed for {}: before {:?}, after {:?}",
                            payload.label, target.name, payload.core, actual
                        ));
                    }
                    record_model_diffs(
                        payload.label,
                        &transmission_value(&payload.network),
                        &transmission_value(&parsed.network),
                        cell,
                    );
                    match ybus_change(&payload.network, &parsed.network) {
                        Ok(Some(diff)) => cell.failures.push(format!(
                            "{} Y_bus changed for {}: {diff}",
                            payload.label, target.name
                        )),
                        Ok(None) => {}
                        // A network with no buildable admittance matrix is a
                        // conversion failure. Reporting it as agreement is how
                        // an invariant passes without checking anything.
                        Err(side) => cell.failures.push(format!(
                            "{} Y_bus could not be built for {} ({}) ",
                            payload.label,
                            target.name,
                            match side {
                                YbusUnavailable::Before => "source",
                                YbusUnavailable::After => "result",
                            }
                        )),
                    }
                    if let Some(diff) = injection_change(&payload.network, &parsed.network) {
                        cell.failures.push(format!(
                            "{} bus injections moved for {}: {diff}",
                            payload.label, target.name
                        ));
                    }
                }
                Err(err) => cell.failures.push(format!(
                    "{} output did not parse as {}: {err}",
                    payload.label, target.name
                )),
            }
        }
        Err(err) => cell.failures.push(format!(
            "{} did not write as {}: {err}",
            payload.label, target.name
        )),
    }
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

// BMOPF schema 0.1.0: the dss→BMOPF drop drops (taps, no load, neutral
// impedance now ride along under `extras.transformer`), and the BMOPF source
// rows reflect the re-vendored 0.1.0 example fixtures.
// The dss leg splits an unbalanced load into one single phase `Load` per phase
// (#266 item 2), so a case carrying one arrives at the next hop as several. Each
// part restates the `kv`/`phases`/`conn` extras the dss reader attaches, and
// each of those is dropped again on the way into BMOPF or PMD: +8 on the two
// rows whose source is a dss deck, +2 on the BMOPF→dss row.
// BMOPF -> PMD drops by 12: the scalar bus voltage bounds now ride PMD's
// `vm_lb`/`vm_ub` (one entry per terminal, volts over `voltage_scale_factor`)
// instead of being reported as having no ENGINEERING field. PMD -> dss rises
// by the same 12: the PMD hop used to lose those bounds silently, so the dss
// leg had nothing to report; now they arrive and dss — which has no per-bus
// voltage bound — drops them loudly. What remains on the BMOPF source row is
// the genuine superset-to-subset loss: named terminals and generator cost,
// which neither dss nor ENGINEERING states, and the bounds on the dss leg.
// The PMD reader no longer copies an array vm_nom into the `kv` extra: the
// typed voltage model carries those volts entry for entry, so the copy only
// fed a token dss could not parse and BMOPF could only drop — −1 on both
// cells of the PMD source row.
// The dss→bmopf and pmd→bmopf cells dropped when the BMOPF writer began
// aggregating its per-field extras drops into one finding per field (#377).
const DISTRIBUTION_WARNING_BASELINE: [[usize; 3]; 3] = [[0, 66, 88], [20, 0, 15], [26, 42, 0]];

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

    MatrixReport {
        sources: formats.clone(),
        targets: formats,
        case_count: DISTRIBUTION_CASES.len(),
        cells,
        failures,
    }
}

fn distribution_payloads(format: DistributionFormat) -> Result<Vec<DistributionPayload>, String> {
    DISTRIBUTION_CASES
        .iter()
        .map(|(label, rel, native_format)| {
            let mut base = dist_parse_file(data(rel), native_format.token)
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

fn dist_parse_file(
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
        let source = powerio_core::Source::from_bytes("<memory>", text.as_bytes().to_vec())?
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
    let parsed = dist_parse_file(&path, format.token);
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

/// Every corpus fixture, written to a stored `.pio.json` module and read back, per
/// source format.
///
/// The matrix above measures what survives a case format hop; this measures
/// what survives powerio's own document, which must be everything. It is the
/// per fixture lossless signal, over the same corpus the matrix walks rather
/// than over synthetic fixtures: a serde rename, a dropped `serde(default)`,
/// or a field that fails to serialize shows up on the first case that carries
/// it. The model JSON leg is checked alongside, since the module carries the
/// same payload under `model.balanced_network`.
#[test]
fn every_fixture_echoes_through_a_package() {
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
            if let Err(err) = model_json_echoes(&payload.network) {
                failures.push(format!("{where_}: {err}"));
            }
            if let Err(err) = package_echoes(payload.network) {
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
            if let Err(err) = module_json_echoes(&module) {
                failures.push(format!("{} as {}: {err}", payload.label, format.name));
            }
        }
    }

    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A balanced network survives a model JSON write and readback unchanged.
fn model_json_echoes(net: &BalancedNetwork) -> Result<(), String> {
    let text = net
        .to_json()
        .map_err(|err| format!("model JSON write: {err}"))?;
    let back =
        BalancedNetwork::from_json(&text).map_err(|err| format!("model JSON readback: {err}"))?;
    if transmission_value(&back) != transmission_value(net) {
        return Err("model JSON changed the model".to_owned());
    }
    Ok(())
}

/// A balanced network survives a stored `.pio.json` write and readback
/// unchanged.
fn package_echoes(net: BalancedNetwork) -> Result<(), String> {
    let before = transmission_value(&net);
    let module = powerio_core::PioModule::new(powerio::PioValue::BalancedNetwork(net));
    let back = module_json_echoes(&module)?;
    let powerio::PioValue::BalancedNetwork(net_back) = back.value() else {
        return Err("the module came back with a different value kind".to_owned());
    };
    let after = transmission_value(net_back);
    if before != after {
        return Err(format!(
            "the stored round trip changed the model: {:?}",
            model_diffs(&before, &after)
        ));
    }
    Ok(())
}

/// The document itself is round-trip stable: write, read, write again, and the
/// two texts agree byte for byte.
fn module_json_echoes(
    module: &powerio_core::PioModule<powerio::PioValue>,
) -> Result<powerio_core::PioModule<powerio::PioValue>, String> {
    let first = powerio::stored::emit_module(module).map_err(|err| format!("write: {err}"))?;
    let back = powerio::stored::read_module(&first).map_err(|err| format!("read: {err}"))?;
    let second = powerio::stored::emit_module(&back).map_err(|err| format!("rewrite: {err}"))?;
    if first != second {
        return Err("the .pio.json document is not round-trip stable".to_owned());
    }
    Ok(back)
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

fn parse_module_from(
    source: powerio_core::Source,
) -> Result<ParsedTransmission, powerio_core::Error> {
    let module = powerio_tx::format::parse(source)?;
    let warnings = module
        .diagnostics()
        .iter()
        .map(|d| format!("{}: {}", d.code(), d.message()))
        .collect();
    Ok(ParsedTransmission {
        warnings,
        network: module.into_value(),
    })
}

fn parse_transmission_file(
    path: impl AsRef<std::path::Path>,
    from: Option<&str>,
) -> Result<ParsedTransmission, powerio_core::Error> {
    let mut source = powerio_core::Source::open(path.as_ref())?;
    if let Some(token) = from {
        source = source.with_format(powerio_core::FormatId::new(
            token.to_ascii_lowercase().replace('_', "-"),
        )?);
    }
    parse_module_from(source)
}

fn parse_transmission_str(
    text: &str,
    from: &str,
) -> Result<ParsedTransmission, powerio_core::Error> {
    let source = powerio_core::Source::from_bytes("<memory>", text.as_bytes().to_vec())?
        .with_format(powerio_core::FormatId::new(
            from.to_ascii_lowercase().replace('_', "-"),
        )?);
    parse_module_from(source)
}
