//! The `powerio` binary: a clap CLI and a ratatui TUI over `powerio-matrix`.
//!
//! Subcommands: `batch` (matrix families), `gen` (synthetic cases), `verify`,
//! `dcopf` (DC OPF bundle), `sensitivities` (PTDF/LODF), `gridfm` (gridfm-datakit
//! Parquet), `serialize` (PowerIO IR), and `convert`. With no subcommand it launches the TUI. Run
//! `powerio --help` for the full surface.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use anyhow::Context;
use clap::{Parser, Subcommand, ValueEnum};
use powerio_core::ErrorCategory;
use powerio_matrix::io::gridfm::{GridfmOptions, emit_gridfm_batch, number_snapshots};
use powerio_matrix::matrix::{BranchSusceptanceFormula, BuildOptions, Scheme, check_sddm};
use powerio_matrix::pipeline::{MatrixKind, Pipeline, RhsKind};
use powerio_matrix::synth::{SynthSpec, Topology};
use powerio_matrix::{
    DcOpfAssemblyOptions, DcOpfBundleMetadata, DcOpfBundleOptions, Units, emit_dcopf_bundle,
};
use powerio_matrix::{SensitivityOptions, SensitivitySolver};
use powerio_tx::{EmitOptions, MissingGenCostPolicy};
use serde_json::json;
mod cases;
mod codes;
mod module_io;
mod tui;

use powerio_tx::format::routing::SourceFormat as DetectedFormat;

#[derive(Parser, Debug)]
#[command(name = "powerio", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// How diagnostics and failures are written on stderr: `text` prints
    /// one `CODE: message` line per diagnostic and `Error:` lines for a
    /// failure; `json` prints one JSON array of PowerIO IR diagnostic
    /// records when the command ends, and nothing else.
    #[arg(long, global = true, value_enum, default_value_t = DiagnosticsFormat::Text)]
    diagnostics_format: DiagnosticsFormat,
}

/// The stderr rendering of diagnostics, selected once per run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
enum DiagnosticsFormat {
    /// One `CODE: message` line per diagnostic, `Error:` and `Caused by:`
    /// lines for a failure, and progress lines such as `wrote <path>`.
    #[default]
    Text,
    /// One JSON array of PowerIO IR diagnostic records, printed when the
    /// command ends; no other line reaches stderr.
    Json,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Launch the interactive TUI (default if no subcommand is given).
    Tui {
        /// Directory scanned recursively for case files (.m, .raw, .aux,
        /// .epc, .pwb, .uct, .cdf, .json, .dss).
        #[arg(short, long)]
        data_dir: Option<PathBuf>,
        /// Default output directory for batch exports.
        #[arg(short, long)]
        out_dir: Option<PathBuf>,
    },
    /// Batch export matrix datasets for every case file under a directory.
    Batch {
        /// Input directory (scanned recursively for case files) or a single
        /// case file.
        #[arg(short, long)]
        input: PathBuf,
        /// Output directory.
        #[arg(short, long)]
        output: PathBuf,
        /// Comma-separated matrix kinds to emit.
        #[arg(short, long, value_delimiter = ',', default_values = ["bprime"])]
        matrices: Vec<MatrixKindArg>,
        #[arg(long, value_enum, default_value = "bx")]
        scheme: SchemeArg,
        #[arg(long, value_enum, default_value = "none")]
        rhs: RhsArg,
        #[arg(long, default_value_t = 0xC0FFEE)]
        seed: u64,
    },
    /// Generate a synthetic case and emit its matrices.
    Gen {
        #[arg(long, value_enum)]
        topology: TopologyArg,
        #[arg(long, default_value_t = 64)]
        n: usize,
        #[arg(long, default_value_t = 0.1)]
        r_over_x: f64,
        #[arg(long, default_value_t = 0.05)]
        mean_x: f64,
        #[arg(long, default_value_t = 0xC0FFEE)]
        seed: u64,
        #[arg(short, long)]
        output: PathBuf,
        #[arg(short, long, value_delimiter = ',', default_values = ["bprime"])]
        matrices: Vec<MatrixKindArg>,
    },
    /// Print matrix stats and the SDDM check for one case.
    Verify {
        /// Transmission case file or serialized PowerIO IR (`.pio.json`);
        /// `-` reads standard input, which requires `--from`.
        input: PathBuf,
        /// Override the inferred input format.
        #[arg(long, value_enum)]
        from: Option<FormatArg>,
        #[arg(long, value_enum, default_value = "bprime")]
        kind: MatrixKindArg,
        #[arg(long, value_enum, default_value = "bx")]
        scheme: SchemeArg,
    },
    /// Emit the static DC OPF matrix/vector bundle for one case.
    #[command(name = "dcopf", visible_alias = "dc-opf")]
    DcOpf {
        /// Transmission case file or serialized PowerIO IR (`.pio.json`);
        /// `-` reads standard input, which requires `--from`.
        input: PathBuf,
        /// Override the inferred input format.
        #[arg(long, value_enum)]
        from: Option<FormatArg>,
        /// Output directory; the bundle lands in `<output>/<case>_dcopf/`.
        #[arg(short, long)]
        output: PathBuf,
        /// Formula used to calculate each DC branch susceptance.
        #[arg(long, value_enum, default_value = "series-susceptance")]
        formula: BranchSusceptanceFormulaArg,
        /// Unit system for power/cost quantities.
        #[arg(long, value_enum, default_value = "per-unit")]
        units: UnitsArg,
        /// Policy for in-service generators with no cost row.
        #[arg(long, value_enum, default_value = "require")]
        missing_gen_cost: MissingGenCostArg,
        /// Default polynomial cost as `c2,c1,c0` for `--missing-gen-cost quadratic`.
        #[arg(long)]
        default_gen_cost: Option<String>,
        /// CSV with columns gen_index,bus,c2,c1,c0 and optional startup,shutdown.
        #[arg(long)]
        gen_cost_csv: Option<PathBuf>,
    },
    /// Emit DC sensitivity matrices (PTDF, LODF) for one case.
    Sensitivities {
        /// Transmission case file or serialized PowerIO IR (`.pio.json`);
        /// `-` reads standard input, which requires `--from`.
        input: PathBuf,
        /// Override the inferred input format.
        #[arg(long, value_enum)]
        from: Option<FormatArg>,
        /// Output directory; writes `<case>_ptdf.mtx` and `<case>_lodf.mtx`.
        #[arg(short, long)]
        output: PathBuf,
        /// Formula used to calculate each DC branch susceptance.
        #[arg(long, value_enum, default_value = "series-susceptance")]
        formula: BranchSusceptanceFormulaArg,
        /// Sensitivity solve path.
        #[arg(long, value_enum, default_value = "auto")]
        solver: SensitivitySolverArg,
        /// Omit written PTDF/LODF entries with absolute value at or below this.
        #[arg(long, default_value_t = 1e-12)]
        drop_tolerance: f64,
    },
    /// Print the canonical network summary JSON.
    Summary {
        /// Input case file, PyPSA CSV folder, or gridfm dataset directory.
        /// `-` reads the case from standard input, which requires `--from`.
        input: PathBuf,
        /// Override the inferred input format.
        #[arg(long, value_enum)]
        from: Option<FormatArg>,
        /// With `--from gridfm`, which scenario to summarize.
        #[arg(long, default_value_t = 0)]
        scenario: i64,
    },
    /// Serialize one input as PowerIO IR (`.pio.json`).
    Serialize {
        /// Input case file, PyPSA CSV folder, or gridfm dataset directory.
        /// `-` reads the case from standard input, which requires `--from`.
        input: PathBuf,
        /// Output file; `-` or omitted writes to stdout.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Override the inferred input format.
        #[arg(long, value_enum)]
        from: Option<FormatArg>,
    },
    /// Run the conversion invariants over a corpus of case files.
    ///
    /// The corpus directory is only ever read; the work directory is scratch
    /// and holds raw values, so it stays on the machine that owns the corpus.
    /// The report is the only output meant to travel, and it states codes,
    /// class ordinals and magnitudes rather than anything from the cases.
    ///
    /// See the corpus harness guide for the session protocol that turns a
    /// finding into a synthetic reproducer.
    Corpus {
        #[command(subcommand)]
        action: CorpusCommand,
    },
    /// Write the gridfm-datakit Parquet dataset for one or more cases.
    ///
    /// Each input is one scenario (an operating point on a shared base element
    /// set); multiple inputs stack into one dataset keyed by the `scenario`
    /// column. A single input reproduces the one-snapshot dataset.
    Gridfm {
        /// Input case files; the k-th is stamped `scenario + k` (format inferred
        /// from each extension unless `--from`). All inputs must share the same
        /// bus, branch, and generator counts in the same bus order; load,
        /// dispatch, branch status, and costs may vary per scenario.
        #[arg(required = true, num_args = 1..)]
        inputs: Vec<PathBuf>,
        /// Output directory; the dataset lands in `<output>/<case>/raw/`.
        #[arg(short, long)]
        output: PathBuf,
        /// Override the inferred input format (applied to every input).
        #[arg(long, value_enum)]
        from: Option<FormatArg>,
        /// Base scenario id; the k-th input is stamped `scenario + k`.
        #[arg(long, default_value_t = 0)]
        scenario: i64,
        /// Policy for generators with no cost row.
        #[arg(long, value_enum, default_value = "preserve")]
        missing_gen_cost: MissingGenCostArg,
        /// Default polynomial cost as `c2,c1,c0` for `--missing-gen-cost quadratic`.
        #[arg(long)]
        default_gen_cost: Option<String>,
        /// CSV with columns gen_index,bus,c2,c1,c0 and optional startup,shutdown.
        #[arg(long)]
        gen_cost_csv: Option<PathBuf>,
    },
    /// Convert a case file to another format. Transmission formats convert
    /// through the neutral hub; distribution formats (dss, pmd-json,
    /// bmopf-json) through the wire coordinate distribution model. The two
    /// families do not mix.
    Convert {
        /// Input case file, or a gridfm dataset directory with `--from gridfm`.
        /// The format is inferred from the extension (`.m`, `.json`, `.raw`,
        /// `.aux`, `.dss`) unless `--from` is given. `-` reads the case from
        /// standard input, which requires `--from`.
        input: PathBuf,
        /// Target format.
        #[arg(long, value_enum)]
        to: FormatArg,
        /// Output file; `-` or omitted writes to stdout.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Override the inferred input format. `gridfm` reads a Parquet dataset
        /// directory (see `--scenario`).
        #[arg(long, value_enum)]
        from: Option<FormatArg>,
        /// With `--from gridfm`, which scenario to read from the dataset.
        #[arg(long, default_value_t = 0)]
        scenario: i64,
        /// Policy for generators with no cost row.
        #[arg(long, value_enum, default_value = "preserve")]
        missing_gen_cost: MissingGenCostArg,
        /// Default polynomial cost as `c2,c1,c0` for `--missing-gen-cost quadratic`.
        #[arg(long)]
        default_gen_cost: Option<String>,
        /// CSV with columns gen_index,bus,c2,c1,c0 and optional startup,shutdown.
        #[arg(long)]
        gen_cost_csv: Option<PathBuf>,
    },
    /// Extract, apply, or normalize standalone geographic layers (.geo.json).
    Geo {
        #[command(subcommand)]
        command: GeoCommand,
    },
}

#[derive(Subcommand, Debug)]
enum CorpusCommand {
    /// Parse every readable file under a corpus and bucket it by electrical
    /// fingerprint. Reads the corpus, writes only the work directory.
    Ingest {
        /// Corpus directory, walked recursively and never written.
        corpus: PathBuf,
        /// Scratch directory for this run. Disposable; put it outside any
        /// repository.
        #[arg(short, long)]
        work: PathBuf,
        /// Skip files larger than this many bytes (counted as skipped). The
        /// compare is quadratic per bucket and a walk re-parses per hop, so
        /// one interconnection scale case would own the whole run.
        #[arg(long)]
        max_bytes: Option<u64>,
    },
    /// Run the invariants over every bucket the ingest found.
    Compare {
        #[arg(short, long)]
        work: PathBuf,
    },
    /// Chain each case through random cycles of formats, and hold the chain to
    /// the properties one leg cannot state: that the route does not change the
    /// destination, that conversion settles, and that an emptied table stays
    /// empty.
    ///
    /// Each run keeps a ledger of what every format pair has taught it and
    /// draws the next path toward the pairs that have taught the least. The
    /// ledger lives in the work directory and carries across runs.
    Walk {
        #[arg(short, long)]
        work: PathBuf,
        /// Walks per bucket, before the settle rule cuts the run short.
        #[arg(long, default_value_t = 8)]
        walks: usize,
        /// Formats per walk. Six is two full cycles of the three format
        /// families, which is where composition starts to show.
        #[arg(long, default_value_t = 6)]
        hops: usize,
        /// Seed for the path draw. The same seed and ledger replay the run.
        #[arg(long, default_value_t = 0x5EED)]
        seed: u64,
        /// Stop after this many consecutive walks that teach the ledger
        /// nothing.
        #[arg(long, default_value_t = 12)]
        settle: usize,
    },
    /// Write the sanitized findings. Refuses to write anything that still
    /// carries a string the corpus taught it.
    Report {
        #[arg(short, long)]
        work: PathBuf,
        /// Findings file, one JSON object per line.
        #[arg(short, long, default_value = "findings.jsonl")]
        output: PathBuf,
        /// Optional markdown roll-up of the same findings.
        #[arg(long)]
        summary: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
enum GeoCommand {
    /// Extract a case's coordinates as a canonical .geo.json layer.
    Extract {
        /// Input case file (transmission or distribution).
        input: PathBuf,
        /// Output file; `-` or omitted writes to stdout.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Override the inferred input format.
        #[arg(long, value_enum)]
        from: Option<FormatArg>,
    },
    /// Apply a geographic sidecar onto a case and write the case back.
    Apply {
        /// Input case file (transmission or distribution).
        input: PathBuf,
        /// Geographic sidecar: GeoJSON, aliased CSV/JSON records, or
        /// headerless buscoords CSV.
        layer: PathBuf,
        /// Output case file; `-` or omitted writes to stdout.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Target case format; defaults to the input's own format.
        #[arg(long, value_enum)]
        to: Option<FormatArg>,
        /// Override the inferred input format.
        #[arg(long, value_enum)]
        from: Option<FormatArg>,
    },
    /// Normalize a tolerant geographic sidecar to the canonical .geo.json
    /// form.
    Convert {
        /// Input sidecar: GeoJSON, aliased CSV/JSON records, or headerless
        /// buscoords CSV.
        input: PathBuf,
        /// Output file; `-` or omitted writes to stdout.
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum MissingGenCostArg {
    Preserve,
    Require,
    Zero,
    Quadratic,
}

#[derive(Clone, Copy, Debug)]
struct GenCostCliOptions<'a> {
    missing_gen_cost: MissingGenCostArg,
    default_gen_cost: Option<&'a str>,
    gen_cost_csv: Option<&'a Path>,
}

impl<'a> GenCostCliOptions<'a> {
    const fn new(
        missing_gen_cost: MissingGenCostArg,
        default_gen_cost: Option<&'a str>,
        gen_cost_csv: Option<&'a Path>,
    ) -> Self {
        Self {
            missing_gen_cost,
            default_gen_cost,
            gen_cost_csv,
        }
    }

    #[cfg(test)]
    const fn preserve() -> Self {
        Self {
            missing_gen_cost: MissingGenCostArg::Preserve,
            default_gen_cost: None,
            gen_cost_csv: None,
        }
    }

    fn emit_options(self) -> anyhow::Result<EmitOptions> {
        emit_options(
            self.missing_gen_cost,
            self.default_gen_cost,
            self.gen_cost_csv,
        )
    }
}

/// A grid exchange format for `--to` / `--from`. `gridfm`, `goc3-json`,
/// `opfdata-json`, `pwb`, and `ieee-cdf` are parse only here: `convert --from
/// gridfm` parses a Parquet dataset, while the dedicated `gridfm` subcommand
/// emits a dataset. GO Challenge 3 and OPFData JSON are source documents, and
/// PowerWorld `.pwb` and the IEEE CDF have no emitter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum FormatArg {
    #[value(name = "matpower", alias = "m")]
    Matpower,
    #[value(name = "powermodels-json", alias = "powermodels", alias = "pm")]
    PowerModelsJson,
    #[value(name = "egret-json", alias = "egret")]
    EgretJson,
    #[value(name = "psse", alias = "raw")]
    Psse,
    /// Emit PSS/E `.raw` at revision 34.
    #[value(name = "psse34")]
    Psse34,
    /// Emit PSS/E `.raw` at revision 35.
    #[value(name = "psse35")]
    Psse35,
    /// Emit PSS/E RAWX revision 35 JSON.
    #[value(name = "psse-rawx")]
    PsseRawx,
    /// Accepted as an input spelling for `psse-rawx`.
    #[value(name = "rawx")]
    RawxInput,
    /// PowSybl XIIDM 1.17 XML.
    #[value(name = "xiidm")]
    Xiidm,
    /// Accepted as an input spelling for `xiidm`.
    #[value(name = "iidm")]
    IidmInput,
    /// PowSybl JIIDM 1.17 JSON.
    #[value(name = "jiidm")]
    Jiidm,
    /// IEC CIM CGMES profile set.
    #[value(name = "cgmes")]
    Cgmes,
    /// ENTSO-E UCTE-DEF `.uct`; fresh output uses revision 2007.05.01.
    #[value(name = "ucte", alias = "uct")]
    Ucte,
    #[value(name = "powerworld", alias = "aux")]
    PowerWorld,
    #[value(name = "pandapower-json", alias = "pandapower", alias = "pp")]
    PandapowerJson,
    #[value(name = "pypsa-csv", alias = "pypsa")]
    PypsaCsv,
    /// GE PSLF `.epc` case (parse and emit).
    #[value(name = "pslf", alias = "epc")]
    Pslf,
    /// DOE GO Challenge 3 JSON problem or solution data.
    #[value(name = "goc3-json", alias = "goc3", alias = "go3", alias = "c3")]
    Goc3Json,
    /// Surge native JSON network document.
    #[value(name = "surge-json", alias = "surge")]
    SurgeJson,
    /// JSON document from a DeepMind OPFData release (parse only).
    #[value(
        name = "opfdata-json",
        alias = "opfdata",
        alias = "deepmind-opfdata-json",
        alias = "deepmind-opfdata",
        alias = "gridopt-json",
        alias = "gridopt"
    )]
    DeepMindOpfDataJson,
    /// Parse a gridfm-datakit Parquet dataset directory (parse only).
    #[value(name = "gridfm")]
    Gridfm,
    /// Parse a PowerWorld `.pwb` binary case (parse only).
    #[value(name = "pwb")]
    Pwb,
    /// Parse an IEEE Common Data Format case (parse only).
    #[value(name = "ieee-cdf", alias = "cdf")]
    IeeeCdf,
    /// OpenDSS `.dss` distribution case (parse and emit).
    #[value(name = "dss", alias = "opendss")]
    Dss,
    /// PowerModelsDistribution ENGINEERING JSON (parse and emit).
    #[value(name = "pmd-json", alias = "pmd", alias = "engineering")]
    PmdJson,
    /// IEEE BMOPF JSON distribution case (parse and emit).
    #[value(name = "bmopf-json", alias = "bmopf")]
    BmopfJson,
}

impl FormatArg {
    /// The writable transmission hub target: `None` for the distribution
    /// formats and for gridfm, which has no convert writer (the `gridfm`
    /// subcommand writes datasets).
    fn transmission(self) -> Option<powerio_tx::TargetFormat> {
        use powerio_tx::TargetFormat;
        Some(match self {
            FormatArg::Matpower => TargetFormat::Matpower,
            FormatArg::PowerModelsJson => TargetFormat::PowerModelsJson,
            FormatArg::EgretJson => TargetFormat::EgretJson,
            FormatArg::Psse => TargetFormat::Psse { rev: 33 },
            FormatArg::Psse34 => TargetFormat::Psse { rev: 34 },
            FormatArg::Psse35 => TargetFormat::Psse { rev: 35 },
            FormatArg::PsseRawx | FormatArg::RawxInput => TargetFormat::PsseRawx,
            FormatArg::Xiidm | FormatArg::IidmInput => TargetFormat::Xiidm,
            FormatArg::Jiidm => TargetFormat::Jiidm,
            FormatArg::Cgmes => TargetFormat::Cgmes,
            FormatArg::Ucte => TargetFormat::Ucte,
            FormatArg::PowerWorld => TargetFormat::PowerWorld,
            FormatArg::PandapowerJson => TargetFormat::PandapowerJson,
            FormatArg::Pslf => TargetFormat::Pslf,
            FormatArg::Goc3Json => TargetFormat::Goc3Json,
            FormatArg::SurgeJson => TargetFormat::SurgeJson,
            FormatArg::DeepMindOpfDataJson => TargetFormat::DeepMindOpfDataJson,
            // PypsaCsv is a transmission format, but it writes a directory, not a
            // text target; `run_convert` handles it before reaching here. gridfm
            // is read only here, and Pwb and IeeeCdf are read only. The
            // distribution formats belong to `distribution()`. All return
            // `None` from this method.
            FormatArg::PypsaCsv
            | FormatArg::Gridfm
            | FormatArg::Pwb
            | FormatArg::IeeeCdf
            | FormatArg::Dss
            | FormatArg::PmdJson
            | FormatArg::BmopfJson => return None,
        })
    }

    /// The distribution target, or `None` outside that family. For every
    /// writable format exactly one of this and [`FormatArg::transmission`]
    /// is `Some`, so adding one without wiring its family is a compile
    /// error; gridfm is read only and returns `None` from both.
    fn distribution(self) -> Option<powerio_dist::DistTargetFormat> {
        use powerio_dist::DistTargetFormat;
        match self {
            FormatArg::Dss => Some(DistTargetFormat::Dss),
            FormatArg::PmdJson => Some(DistTargetFormat::PmdJson),
            FormatArg::BmopfJson => Some(DistTargetFormat::BmopfJson),
            FormatArg::Matpower
            | FormatArg::PowerModelsJson
            | FormatArg::EgretJson
            | FormatArg::Psse
            | FormatArg::Psse34
            | FormatArg::Psse35
            | FormatArg::PsseRawx
            | FormatArg::RawxInput
            | FormatArg::Xiidm
            | FormatArg::IidmInput
            | FormatArg::Jiidm
            | FormatArg::Cgmes
            | FormatArg::Ucte
            | FormatArg::PowerWorld
            | FormatArg::PandapowerJson
            | FormatArg::PypsaCsv
            | FormatArg::Pslf
            | FormatArg::Goc3Json
            | FormatArg::SurgeJson
            | FormatArg::DeepMindOpfDataJson
            | FormatArg::Gridfm
            | FormatArg::Pwb
            | FormatArg::IeeeCdf => None,
        }
    }

    /// The canonical name the format dispatcher accepts for forcing a parser.
    fn name(self) -> &'static str {
        match self {
            FormatArg::Matpower => "matpower",
            FormatArg::PowerModelsJson => "powermodels-json",
            FormatArg::EgretJson => "egret-json",
            FormatArg::Psse => "psse",
            FormatArg::Psse34 => "psse34",
            FormatArg::Psse35 => "psse35",
            FormatArg::PsseRawx => "psse-rawx",
            FormatArg::RawxInput => "rawx",
            FormatArg::Xiidm => "xiidm",
            FormatArg::IidmInput => "iidm",
            FormatArg::Jiidm => "jiidm",
            FormatArg::Cgmes => "cgmes",
            FormatArg::Ucte => "ucte",
            FormatArg::PowerWorld => "powerworld",
            FormatArg::PandapowerJson => "pandapower-json",
            FormatArg::PypsaCsv => "pypsa-csv",
            FormatArg::Pslf => "pslf",
            FormatArg::Goc3Json => "goc3-json",
            FormatArg::SurgeJson => "surge-json",
            FormatArg::DeepMindOpfDataJson => "opfdata-json",
            FormatArg::Gridfm => "gridfm",
            FormatArg::Pwb => "pwb",
            FormatArg::IeeeCdf => "ieee-cdf",
            FormatArg::Dss => "dss",
            FormatArg::PmdJson => "pmd-json",
            FormatArg::BmopfJson => "bmopf-json",
        }
    }

    fn is_input_spelling(self) -> bool {
        matches!(self, Self::RawxInput | Self::IidmInput)
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum MatrixKindArg {
    #[value(name = "bprime", alias = "b1", alias = "b")]
    BPrime,
    #[value(name = "bdoubleprime", alias = "b2", alias = "bpp")]
    BDoublePrime,
    #[value(name = "ybus_real", alias = "g")]
    YbusG,
    #[value(name = "ybus_imag", alias = "negB", alias = "b_lap")]
    YbusB,
    #[value(name = "lacpf")]
    Lacpf,
    #[value(name = "adjacency", alias = "adj")]
    Adjacency,
}

impl From<MatrixKindArg> for MatrixKind {
    fn from(value: MatrixKindArg) -> Self {
        match value {
            MatrixKindArg::BPrime => Self::BPrime,
            MatrixKindArg::BDoublePrime => Self::BDoublePrime,
            MatrixKindArg::YbusG => Self::YbusG,
            MatrixKindArg::YbusB => Self::YbusB,
            MatrixKindArg::Lacpf => Self::Lacpf,
            MatrixKindArg::Adjacency => Self::Adjacency,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum SchemeArg {
    Bx,
    Xb,
}

impl From<SchemeArg> for Scheme {
    fn from(value: SchemeArg) -> Self {
        match value {
            SchemeArg::Bx => Self::Bx,
            SchemeArg::Xb => Self::Xb,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum BranchSusceptanceFormulaArg {
    /// The whole series impedance: `imag(inv(r + jx))`, with phase shift
    /// injections.
    #[value(name = "series-susceptance")]
    SeriesSusceptance,
    /// `-1/(x tau)`, with phase shift injections, matching MATPOWER
    /// `makeBdc`.
    #[value(name = "tap-adjusted-reactance")]
    TapAdjustedReactance,
    /// `-1/x`, ignoring resistance, taps, and shifts: the textbook DC
    /// linearization a published result reproduces.
    #[value(name = "reactance-only")]
    ReactanceOnly,
}

impl From<BranchSusceptanceFormulaArg> for BranchSusceptanceFormula {
    fn from(value: BranchSusceptanceFormulaArg) -> Self {
        match value {
            BranchSusceptanceFormulaArg::SeriesSusceptance => Self::SeriesSusceptance,
            BranchSusceptanceFormulaArg::TapAdjustedReactance => Self::TapAdjustedReactance,
            BranchSusceptanceFormulaArg::ReactanceOnly => Self::ReactanceOnly,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum SensitivitySolverArg {
    Auto,
    Dense,
    Sparse,
}

impl From<SensitivitySolverArg> for SensitivitySolver {
    fn from(value: SensitivitySolverArg) -> Self {
        match value {
            SensitivitySolverArg::Auto => Self::Auto,
            SensitivitySolverArg::Dense => Self::Dense,
            SensitivitySolverArg::Sparse => Self::Sparse,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum UnitsArg {
    PerUnit,
    Native,
}

impl From<UnitsArg> for Units {
    fn from(value: UnitsArg) -> Self {
        match value {
            UnitsArg::PerUnit => Self::PerUnit,
            UnitsArg::Native => Self::Native,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum RhsArg {
    None,
    Random,
    Injection,
}

impl From<RhsArg> for RhsKind {
    fn from(value: RhsArg) -> Self {
        match value {
            RhsArg::None => Self::None,
            RhsArg::Random => Self::Random,
            RhsArg::Injection => Self::Injection,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum TopologyArg {
    Tree,
    Lattice,
    Pegase,
}

impl From<TopologyArg> for Topology {
    fn from(value: TopologyArg) -> Self {
        match value {
            TopologyArg::Tree => Self::Tree,
            TopologyArg::Lattice => Self::Lattice2D,
            TopologyArg::Pegase => Self::PegaseLike,
        }
    }
}

// A flat dispatch: one arm per subcommand, each delegating immediately.
#[expect(clippy::too_many_lines)]
fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    install_tracing(cli.diagnostics_format);
    let _ = DIAGNOSTICS_FORMAT.set(cli.diagnostics_format);
    let result: anyhow::Result<()> = match cli.command.unwrap_or_else(default_command) {
        Command::Tui { data_dir, out_dir } => tui::run(tui::TuiOptions { data_dir, out_dir }),
        Command::Batch {
            input,
            output,
            matrices,
            scheme,
            rhs,
            seed,
        } => run_batch(&input, &output, matrices, scheme.into(), rhs.into(), seed),
        Command::Gen {
            topology,
            n,
            r_over_x,
            mean_x,
            seed,
            output,
            matrices,
        } => run_gen_cli(topology, n, r_over_x, mean_x, seed, &output, matrices),
        Command::Verify {
            input,
            from,
            kind,
            scheme,
        } => run_verify(&input, from, kind.into(), scheme.into()),
        Command::DcOpf {
            input,
            from,
            output,
            formula,
            units,
            missing_gen_cost,
            default_gen_cost,
            gen_cost_csv,
        } => run_dcopf(
            &input,
            from,
            &output,
            formula.into(),
            units.into(),
            missing_gen_cost,
            default_gen_cost.as_deref(),
            gen_cost_csv.as_deref(),
        ),
        Command::Sensitivities {
            input,
            from,
            output,
            formula,
            solver,
            drop_tolerance,
        } => run_sensitivities(&input, from, &output, formula, solver, drop_tolerance),
        Command::Summary {
            input,
            from,
            scenario,
        } => run_summary(&input, from, scenario),
        Command::Corpus { action } => run_corpus(action),
        Command::Serialize {
            input,
            output,
            from,
        } => run_serialize(&input, output.as_deref(), from),
        Command::Gridfm {
            inputs,
            output,
            from,
            scenario,
            missing_gen_cost,
            default_gen_cost,
            gen_cost_csv,
        } => run_gridfm(
            &inputs,
            &output,
            from,
            scenario,
            missing_gen_cost,
            default_gen_cost.as_deref(),
            gen_cost_csv.as_deref(),
        ),
        Command::Convert {
            input,
            to,
            output,
            from,
            scenario,
            missing_gen_cost,
            default_gen_cost,
            gen_cost_csv,
        } => run_convert(
            &input,
            to,
            output.as_deref(),
            from,
            scenario,
            GenCostCliOptions::new(
                missing_gen_cost,
                default_gen_cost.as_deref(),
                gen_cost_csv.as_deref(),
            ),
        ),
        Command::Geo { command } => run_geo(command),
    };
    let status = match &result {
        Ok(()) => 0,
        Err(error) => exit_status(error),
    };
    match diagnostics_format() {
        DiagnosticsFormat::Text => {
            if let Err(error) = &result {
                print_error_chain(error);
            }
        }
        DiagnosticsFormat::Json => {
            if let Err(error) = &result {
                report_diagnostics(&failure_diagnostics(error));
            }
            print_collected_json();
        }
    }
    std::process::ExitCode::from(status)
}

/// A failure the command line raises itself, as an ordinary PowerIO error
/// built from one of the registered `*.CLI.*` codes in [`codes`], so it carries
/// the same diagnostic record and error category as any library failure.
fn cli_failure(
    info: &'static powerio_core::DiagnosticInfo,
    message: impl Into<String>,
) -> anyhow::Error {
    anyhow::Error::new(powerio_core::Error::new(info, message))
}

/// Return a command line failure with the named registered code.
macro_rules! fail_with {
    ($code:ident, $($arg:tt)*) => { return Err(cli_failure(&codes::$code, format!($($arg)*))) };
}

/// A command line failure value with the named registered code, for
/// `ok_or_else` closures.
macro_rules! failure {
    ($code:ident, $($arg:tt)*) => { cli_failure(&codes::$code, format!($($arg)*)) };
}

/// The exit status of a failed run. A failure that carries a PowerIO error
/// category maps to one fixed code per category: `request` 2, `io` 3,
/// `parse` 4, `data` 5, `output` 6. Any other failure exits 1. Clap reports a
/// usage error with 2, the same code as a `request` failure.
fn exit_status(error: &anyhow::Error) -> u8 {
    match error_category(error) {
        Some(ErrorCategory::Request) => 2,
        Some(ErrorCategory::Io) => 3,
        Some(ErrorCategory::Parse) => 4,
        Some(ErrorCategory::Data) => 5,
        Some(ErrorCategory::Output) => 6,
        None => 1,
    }
}

/// The category of the first PowerIO error in the chain, if any.
fn error_category(error: &anyhow::Error) -> Option<ErrorCategory> {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<powerio_core::Error>())
        .map(powerio_core::Error::category)
}

static DIAGNOSTICS_FORMAT: std::sync::OnceLock<DiagnosticsFormat> = std::sync::OnceLock::new();
static COLLECTED_DIAGNOSTICS: std::sync::Mutex<Vec<powerio_core::Diagnostic>> =
    std::sync::Mutex::new(Vec::new());

fn diagnostics_format() -> DiagnosticsFormat {
    DIAGNOSTICS_FORMAT.get().copied().unwrap_or_default()
}

fn collected_diagnostics() -> std::sync::MutexGuard<'static, Vec<powerio_core::Diagnostic>> {
    COLLECTED_DIAGNOSTICS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Report diagnostics on stderr in the run's format: text prints them now,
/// one `CODE: message` line each; JSON collects them for the one array
/// printed when the command ends.
fn report_diagnostics(diagnostics: &[powerio_core::Diagnostic]) {
    match diagnostics_format() {
        DiagnosticsFormat::Text => {
            for line in powerio_core::render_diagnostics(diagnostics) {
                eprintln!("{line}");
            }
        }
        DiagnosticsFormat::Json => collected_diagnostics().extend(diagnostics.iter().cloned()),
    }
}

/// A progress line such as `wrote <path>`. Text format only, so JSON stderr
/// holds nothing but the diagnostics array.
fn report_progress(line: impl std::fmt::Display) {
    if diagnostics_format() == DiagnosticsFormat::Text {
        eprintln!("{line}");
    }
}

fn report_written(path: &Path) {
    report_progress(format!("wrote {}", path.display()));
}

/// Print every diagnostic the run collected as one JSON array of PowerIO IR
/// diagnostic records. When the records cannot be encoded, print them as
/// text and say why.
fn print_collected_json() {
    let collected = std::mem::take(&mut *collected_diagnostics());
    match powerio::serialize_diagnostics(&collected) {
        Ok(text) => eprintln!("{text}"),
        Err(encode_error) => {
            for line in powerio_core::render_diagnostics(&collected) {
                eprintln!("{line}");
            }
            eprintln!("Error: could not encode the diagnostics as JSON: {encode_error}");
        }
    }
}

/// The frames of a failure, outermost first, each paired with whether it is
/// the PowerIO error itself. A frame that only repeats the end of the frame
/// above it is dropped.
fn failure_frames(error: &anyhow::Error) -> Vec<(bool, String)> {
    let mut frames = Vec::new();
    let mut previous: Option<String> = None;
    for cause in error.chain() {
        let text = cause.to_string();
        let repeats_the_frame_above = previous
            .as_deref()
            .is_some_and(|above| above.ends_with(&text));
        if !repeats_the_frame_above {
            let is_powerio_error = cause.downcast_ref::<powerio_core::Error>().is_some();
            frames.push((is_powerio_error, text.clone()));
        }
        previous = Some(text);
    }
    frames
}

/// The diagnostic records of a failure: the PowerIO error's own records, then
/// every other frame of the cause chain as a `note` record whose `related`
/// names the primary record. A failure with no PowerIO error in its chain
/// becomes one `BIND.CLI.UNCLASSIFIED` record.
fn failure_diagnostics(error: &anyhow::Error) -> Vec<powerio_core::Diagnostic> {
    use powerio_core::{Diagnostic, DiagnosticCode, DiagnosticId, DiagnosticSeverity};
    let failure = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<powerio_core::Error>());
    let mut frames = failure_frames(error);
    let mut records: Vec<Diagnostic> = match failure {
        Some(failure) if !failure.diagnostics().is_empty() => failure.diagnostics().to_vec(),
        _ => {
            let (_, message) = frames.remove(0);
            vec![Diagnostic::of(&codes::BIND_CLI_UNCLASSIFIED, message)]
        }
    };
    let primary_id = if let Some(id) = records[0].id() {
        id.clone()
    } else {
        let Ok(id) = DiagnosticId::new("failure") else {
            return records;
        };
        records[0] = records[0].clone().with_id(id.clone());
        id
    };
    let Ok(primary_code) = DiagnosticCode::new(records[0].code()) else {
        return records;
    };
    for (is_powerio_error, text) in frames {
        if is_powerio_error {
            continue;
        }
        let note = Diagnostic::new(primary_code.clone(), DiagnosticSeverity::Note, text);
        if let Ok(note) = note.with_related(primary_id.clone()) {
            records.push(note);
        }
    }
    records
}

/// The input spelling that selects standard input.
fn is_stdin(input: &Path) -> bool {
    input.as_os_str() == "-"
}

/// Read standard input to its end as one in-memory source named `<stdin>`.
fn stdin_source() -> anyhow::Result<powerio_core::Source> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .lock()
        .read_to_end(&mut bytes)
        .context("reading standard input")?;
    powerio_core::Source::from_memory("<stdin>", bytes)
        .context("creating the standard input source")
}

/// The declared format a standard input case needs. Content on a stream has
/// no file name to infer a format from, so `--from` is required, and a gridfm
/// dataset is a directory that cannot arrive on a stream.
fn stdin_format(from: Option<FormatArg>) -> anyhow::Result<FormatArg> {
    match from {
        None => Err(cli_failure(
            &codes::REQUEST_CLI_FORMAT_REQUIRED,
            "reading a case from standard input needs `--from <format>`; a stream has no \
             file name to infer the format from",
        )),
        Some(FormatArg::Gridfm) => Err(cli_failure(
            &codes::REQUEST_CLI_FORMAT_REQUIRED,
            "a gridfm dataset is a directory and cannot be read from standard input",
        )),
        Some(format) => Ok(format),
    }
}

/// Render an error to stderr as `Error: {top}`, then each distinct cause
/// beneath it as `Caused by: {cause}`. A `with_context` frame built over a
/// `powerio_core::Error` retains that error's own cause for
/// `std::error::Error::source`, and that cause's `Display` repeats the same
/// text the wrapping `powerio_core::Error` already renders with its code
/// prefixed, so the default `{:?}` chain print shows the one finding twice.
/// A cause whose text is already a suffix of the frame above it is dropped
/// instead of reprinted.
fn print_error_chain(error: &anyhow::Error) {
    let mut prefix = "Error";
    for (_, text) in failure_frames(error) {
        eprintln!("{prefix}: {text}");
        prefix = "Caused by";
    }
}

/// Read one text file through a PowerIO source, so a missing or unreadable
/// file is the same `READ.IO.OPEN` failure a case input gives.
fn read_text_file(path: &Path) -> anyhow::Result<String> {
    let source = powerio_core::Source::open(path)?;
    let buffer = source.primary_buffer()?;
    String::from_utf8(buffer.content_bytes().to_vec())
        .with_context(|| format!("{} is not UTF-8 text", path.display()))
}

fn run_geo(command: GeoCommand) -> anyhow::Result<()> {
    match command {
        GeoCommand::Extract {
            input,
            output,
            from,
        } => run_geo_extract(&input, output.as_deref(), from),
        GeoCommand::Apply {
            input,
            layer,
            output,
            to,
            from,
        } => run_geo_apply(&input, &layer, output.as_deref(), to, from),
        GeoCommand::Convert { input, output } => run_geo_convert(&input, output.as_deref()),
    }
}

fn default_command() -> Command {
    Command::Tui {
        data_dir: None,
        out_dir: None,
    }
}

fn run_gen_cli(
    topology: TopologyArg,
    n: usize,
    r_over_x: f64,
    mean_x: f64,
    seed: u64,
    output: &Path,
    matrices: Vec<MatrixKindArg>,
) -> anyhow::Result<()> {
    run_gen(topology.into(), n, r_over_x, mean_x, seed, output, matrices)
}

fn install_tracing(diagnostics_format: DiagnosticsFormat) {
    use tracing_subscriber::EnvFilter;
    if diagnostics_format == DiagnosticsFormat::Json {
        return;
    }
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .try_init();
}

fn run_batch(
    input: &Path,
    output: &Path,
    matrices: Vec<MatrixKindArg>,
    scheme: Scheme,
    rhs: RhsKind,
    seed: u64,
) -> anyhow::Result<()> {
    let (found, scanned) = if input.is_file() {
        (vec![input.to_path_buf()], false)
    } else {
        (cases::discover_cases(input, Some(output)), true)
    };

    if found.is_empty() {
        fail_with!(
            REQUEST_CLI_NO_CASES,
            "no case files ({}) found under {}",
            cases::CASE_EXTENSIONS_LABEL,
            input.display()
        );
    }

    let pipeline = Pipeline {
        matrices: matrices.into_iter().map(MatrixKind::from).collect(),
        options: BuildOptions {
            scheme,
            ..Default::default()
        },
        rhs,
        rng_seed: seed,
        source_file: None,
    };

    let mut exported = 0usize;
    for case_path in &found {
        let loaded = match cases::load_network(case_path) {
            Ok(loaded) => loaded,
            // A recursive scan sweeps up files that merely share an extension
            // with a case format (stray .json in particular); skip them
            // instead of aborting the run.
            Err(e) if scanned => {
                tracing::warn!(case = %case_path.display(), error = format!("{e:#}"), "skipping");
                continue;
            }
            Err(e) => return Err(e),
        };
        for w in &loaded.warnings {
            tracing::warn!(case = %case_path.display(), "{w}");
        }
        let mut p = pipeline.clone();
        p.source_file = Some(case_path.clone());
        let outputs = p
            .run(&loaded.network, output)
            .with_context(|| format!("export {}", case_path.display()))?;
        exported += 1;
        tracing::info!(
            case = %outputs.case_name,
            n = outputs.metadata.n_buses,
            files = outputs.files.len(),
            "exported"
        );
    }
    if exported == 0 {
        fail_with!(
            REQUEST_CLI_NO_CASES,
            "no files under {} loaded as case files ({})",
            input.display(),
            cases::CASE_EXTENSIONS_LABEL
        );
    }
    Ok(())
}

fn run_gen(
    topology: Topology,
    n: usize,
    r_over_x: f64,
    mean_x: f64,
    seed: u64,
    output: &Path,
    matrices: Vec<MatrixKindArg>,
) -> anyhow::Result<()> {
    let spec = SynthSpec {
        topology,
        n,
        r_over_x,
        mean_x,
        seed,
    };
    let case = powerio_matrix::synth::generate(&spec);
    let pipeline = Pipeline {
        matrices: matrices.into_iter().map(MatrixKind::from).collect(),
        ..Default::default()
    };
    let outputs = pipeline.run(&case, output)?;
    tracing::info!(
        case = %outputs.case_name,
        n = outputs.metadata.n_buses,
        files = outputs.files.len(),
        "synthesized"
    );
    Ok(())
}

fn run_sensitivities(
    input: &Path,
    from: Option<FormatArg>,
    output: &Path,
    formula: BranchSusceptanceFormulaArg,
    solver: SensitivitySolverArg,
    drop_tolerance: f64,
) -> anyhow::Result<()> {
    let mpc = balanced_case(input, from).with_context(|| format!("parse {}", input.display()))?;
    std::fs::create_dir_all(output)?;
    let view = powerio_matrix::IndexedNetwork::new(&mpc);
    let options = SensitivityOptions {
        formula: formula.into(),
        solver: solver.into(),
        drop_tolerance,
        ..Default::default()
    };
    // The case name is input derived, so sanitize it before it forms a path;
    // otherwise a name like `../../x` would write outside `output`.
    let stem = powerio_matrix::sanitize_stem(view.name());
    let ptdf_path = output.join(format!("{stem}_ptdf.mtx"));
    let lodf_path = output.join(format!("{stem}_lodf.mtx"));
    let meta_path = output.join(format!("{stem}_sensitivity_meta.json"));
    let metadata = powerio_matrix::io::emit_sensitivity_mtx_with_options(
        &view, &options, &ptdf_path, &lodf_path,
    )
    .with_context(|| format!("DC sensitivities for {}", input.display()))?;
    let meta = json!({
        "case": view.name(),
        "branch_susceptance_formula": options.formula.formula_name(),
        "files": {
            "ptdf": ptdf_path.file_name().and_then(|s| s.to_str()).unwrap_or(""),
            "lodf": lodf_path.file_name().and_then(|s| s.to_str()).unwrap_or("")
        },
        "sensitivity": &metadata
    });
    commit_output_file(&meta_path, serde_json::to_vec_pretty(&meta)?)
        .with_context(|| format!("writing {}", meta_path.display()))?;
    tracing::info!(
        case = %view.name(),
        ptdf = %ptdf_path.display(),
        lodf = %lodf_path.display(),
        metadata = %meta_path.display(),
        solver = metadata.solver_path.as_str(),
        ptdf_dropped = metadata.ptdf.dropped_entries,
        lodf_dropped = metadata.lodf.dropped_entries,
        "wrote DC sensitivities"
    );
    Ok(())
}

fn missing_gen_cost_policy(
    arg: MissingGenCostArg,
    default_gen_cost: Option<&str>,
) -> anyhow::Result<MissingGenCostPolicy> {
    match arg {
        MissingGenCostArg::Preserve => {
            if default_gen_cost.is_some() {
                fail_with!(
                    REQUEST_CLI_OPTION_INVALID,
                    "--default-gen-cost is only valid with --missing-gen-cost quadratic"
                );
            }
            Ok(MissingGenCostPolicy::Preserve)
        }
        MissingGenCostArg::Require => {
            if default_gen_cost.is_some() {
                fail_with!(
                    REQUEST_CLI_OPTION_INVALID,
                    "--default-gen-cost is only valid with --missing-gen-cost quadratic"
                );
            }
            Ok(MissingGenCostPolicy::Require)
        }
        MissingGenCostArg::Zero => {
            if default_gen_cost.is_some() {
                fail_with!(
                    REQUEST_CLI_OPTION_INVALID,
                    "--default-gen-cost is only valid with --missing-gen-cost quadratic"
                );
            }
            Ok(MissingGenCostPolicy::zero())
        }
        MissingGenCostArg::Quadratic => {
            let value = default_gen_cost
                .context("--missing-gen-cost quadratic requires --default-gen-cost C2,C1,C0")?;
            let [c2, c1, c0] = parse_cost_triple(value)?;
            Ok(MissingGenCostPolicy::calc_quadratic(c2, c1, c0))
        }
    }
}

fn parse_cost_triple(value: &str) -> anyhow::Result<[f64; 3]> {
    let parts: Vec<_> = value.split(',').map(str::trim).collect();
    if parts.len() != 3 {
        fail_with!(
            REQUEST_CLI_OPTION_INVALID,
            "--default-gen-cost expects exactly three comma-separated values: C2,C1,C0"
        );
    }
    let mut out = [0.0; 3];
    for (slot, part) in out.iter_mut().zip(parts) {
        *slot = part
            .parse::<f64>()
            .with_context(|| format!("parse --default-gen-cost value `{part}`"))?;
        if !slot.is_finite() {
            fail_with!(
                REQUEST_CLI_OPTION_INVALID,
                "--default-gen-cost values must be finite"
            );
        }
    }
    Ok(out)
}

fn emit_options(
    arg: MissingGenCostArg,
    default_gen_cost: Option<&str>,
    gen_cost_csv: Option<&Path>,
) -> anyhow::Result<EmitOptions> {
    let missing_gen_cost = missing_gen_cost_policy(arg, default_gen_cost)?;
    let gen_cost_patches = match gen_cost_csv {
        Some(path) => {
            let text = read_text_file(path)?;
            powerio_tx::parse_gen_cost_csv(&text)
                .with_context(|| format!("parsing generator cost CSV {}", path.display()))?
        }
        None => Vec::new(),
    };
    Ok(EmitOptions {
        missing_gen_cost,
        gen_cost_patches,
    })
}

#[allow(clippy::too_many_arguments)]
fn run_dcopf(
    input: &Path,
    from: Option<FormatArg>,
    output: &Path,
    formula: BranchSusceptanceFormula,
    units: Units,
    missing_gen_cost: MissingGenCostArg,
    default_gen_cost: Option<&str>,
    gen_cost_csv: Option<&Path>,
) -> anyhow::Result<()> {
    let mpc = balanced_case(input, from).with_context(|| format!("parse {}", input.display()))?;
    let cost_opts = emit_options(missing_gen_cost, default_gen_cost, gen_cost_csv)?;
    let mut policy_network = mpc.clone();
    let cost_report = policy_network
        .apply_gen_cost_policy(&cost_opts.gen_cost_patches, cost_opts.missing_gen_cost)?;
    let instance = powerio_prob::DcOpfInstance::from_network(policy_network)
        .with_context(|| format!("build DC OPF instance for {}", input.display()))?
        .with_branch_susceptance_formula(formula);
    let mut assembly = DcOpfAssemblyOptions::default();
    assembly.units = units;
    let bundle_options = DcOpfBundleOptions {
        assembly,
        metadata: DcOpfBundleMetadata {
            cost_policy: cost_opts.missing_gen_cost,
            cost_report,
        },
    };
    let outputs = emit_dcopf_bundle(&instance, output, &bundle_options)
        .with_context(|| format!("export DC OPF bundle for {}", input.display()))?;
    tracing::info!(
        case = %mpc.name(),
        dir = %outputs.dir.display(),
        files = outputs.files.len(),
        "wrote DC OPF bundle"
    );
    Ok(())
}

fn run_gridfm(
    inputs: &[PathBuf],
    output: &Path,
    from: Option<FormatArg>,
    base_scenario: i64,
    missing_gen_cost: MissingGenCostArg,
    default_gen_cost: Option<&str>,
    gen_cost_csv: Option<&Path>,
) -> anyhow::Result<()> {
    // The `gridfm` subcommand writes a dataset from classical cases; `--from gridfm`
    // (reading a dataset) is the inverse and belongs to `convert`. Reject it with a
    // pointer instead of the opaque `UnknownFormat("gridfm")` the text hub would
    // raise (the mirror of `convert`'s `--to gridfm` guard in `FormatArg::to_target`).
    if from == Some(FormatArg::Gridfm) {
        fail_with!(
            REQUEST_CLI_OPTION_INVALID,
            "the `gridfm` subcommand writes a gridfm dataset from classical cases; \
             to read a gridfm dataset back, use `convert --from gridfm`"
        );
    }
    // Parse every input first so the snapshots can borrow the owned networks for
    // the batch. Each input becomes one scenario, stamped `base + position` by the
    // shared `number_snapshots` builder (same rule as the Python binding).
    let nets = inputs
        .iter()
        .map(|p| read_network(p, from))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let net_refs: Vec<_> = nets.iter().collect();
    let snapshots = number_snapshots(&net_refs, base_scenario)?;

    let cost_opts = emit_options(missing_gen_cost, default_gen_cost, gen_cost_csv)?;
    let opts = GridfmOptions {
        missing_gen_cost: cost_opts.missing_gen_cost,
        gen_cost_patches: cost_opts.gen_cost_patches,
        ..Default::default()
    };
    let outputs = emit_gridfm_batch(&snapshots, output, &opts)
        .with_context(|| format!("export gridfm dataset for {} scenario(s)", snapshots.len()))?;
    if outputs.dropped_zero_impedance > 0 || outputs.degenerate_cost_gens > 0 {
        tracing::warn!(
            zeroed_branches = outputs.dropped_zero_impedance,
            degenerate_cost_gens = outputs.degenerate_cost_gens,
            missing_cost_gens = outputs.missing_cost_gens,
            unsupported_cost_gens = outputs.unsupported_cost_gens,
            "gridfm: some columns were zeroed; see gridfm_meta.json"
        );
    }
    tracing::info!(
        case = %nets[0].name(),
        scenarios = snapshots.len(),
        dir = %outputs.dir.display(),
        files = outputs.files.len(),
        "wrote gridfm dataset"
    );
    Ok(())
}

fn run_verify(
    input: &Path,
    from: Option<FormatArg>,
    kind: MatrixKind,
    scheme: Scheme,
) -> anyhow::Result<()> {
    let mpc = balanced_case(input, from)?;
    let opts = BuildOptions {
        scheme,
        ..Default::default()
    };
    let view = powerio_matrix::IndexedNetwork::new(&mpc);
    let matrix = powerio_matrix::calc_matrix(&view, kind, &opts)?;
    let stats = powerio_matrix::calc_matrix_stats_for_kind(&matrix, &view, kind, &opts);
    let sddm = check_sddm(&matrix);
    println!(
        "{} ({}): n={} nnz={} min_diag={:.3e} max_diag={:.3e} dd_margin={:.3e} M-sign={} ‖A‖_F={:.3e} skipped_zero_impedance={} SDDM={}",
        kind.label(),
        mpc.name(),
        stats.n,
        stats.nnz,
        stats.min_diag,
        stats.max_diag,
        stats.min_dd_margin,
        stats.m_matrix_sign,
        stats.frobenius_norm,
        stats.skipped_zero_impedance,
        sddm
    );
    Ok(())
}

/// Dispatch one `powerio corpus` step. Each writes what the next reads, so
/// they run in order against one work directory.
fn run_corpus(action: CorpusCommand) -> anyhow::Result<()> {
    match action {
        CorpusCommand::Ingest {
            corpus,
            work,
            max_bytes,
        } => {
            let out = powerio_cli::corpus::ingest(&corpus, &work, max_bytes)?;
            let members: usize = out.buckets.iter().map(|b| b.members.len()).sum();
            println!(
                "{} files seen, {members} cases in {} buckets, {} unreadable, {} skipped",
                out.files_seen,
                out.buckets.len(),
                out.unreadable.len(),
                out.skipped
            );
        }
        CorpusCommand::Compare { work } => {
            let out = powerio_cli::corpus::compare(&work)?;
            println!("{} comparisons", out.comparisons.len());
        }
        CorpusCommand::Walk {
            work,
            walks,
            hops,
            seed,
            settle,
        } => {
            let out = powerio_cli::corpus::walk::walk(&work, walks, hops, seed, settle)?;
            let steps: usize = out.walks.iter().map(|w| w.hops.len()).sum();
            println!(
                "{} walks, {steps} hops, {} bucket(s) settled early",
                out.walks.len(),
                out.settled_buckets
            );
        }
        CorpusCommand::Report {
            work,
            output,
            summary,
        } => {
            let n = powerio_cli::corpus::report(&work, &output, summary.as_deref())?;
            println!("{n} findings written to {}", output.display());
        }
    }
    Ok(())
}

/// Parse one GridFM scenario through the universal module path. The dataset
/// parser owns diagnostics and the scenario collection; the CLI reads the
/// requested entry directly.
fn parse_gridfm_scenario(
    input: &Path,
    scenario: i64,
) -> anyhow::Result<(
    powerio_matrix::BalancedNetwork,
    Vec<powerio_core::Diagnostic>,
)> {
    let module = powerio::parse_with_options(input, &parse_options(Some("gridfm"))?)
        .with_context(|| format!("parsing gridfm dataset {}", input.display()))?;
    let scenario_id = scenario.to_string();
    let powerio::PioValue::ScenarioSet(scenarios) = module.value() else {
        fail_with!(
            VALIDATE_CLI_INPUT_LACKS_DATA,
            "gridfm produced {}; expected powerio.ScenarioSet<powerio.BalancedNetwork>",
            module.value().type_name()
        );
    };
    let value = scenarios
        .get(&scenario_id)
        .with_context(|| format!("gridfm scenario {scenario} does not exist"))?;
    let powerio::PioValue::BalancedNetwork(network) = value else {
        fail_with!(
            VALIDATE_CLI_INPUT_LACKS_DATA,
            "gridfm scenario {scenario} contains {}; expected powerio.BalancedNetwork",
            value.type_name()
        );
    };
    Ok((network.clone(), module.diagnostics.clone()))
}

fn run_summary(input: &Path, from: Option<FormatArg>, scenario: i64) -> anyhow::Result<()> {
    let value =
        if from == Some(FormatArg::Gridfm) || (from.is_none() && looks_like_gridfm_dir(input)) {
            let (network, diagnostics) = parse_gridfm_scenario(input, scenario)?;
            transmission_summary_json(&network, &powerio_core::render_diagnostics(&diagnostics))
        } else {
            match parse_family_case(input, from)? {
                FamilyCase::Distribution(module) => {
                    let diagnostics = powerio_core::render_diagnostics(&module.diagnostics);
                    distribution_summary_json(module.value(), &diagnostics)
                }
                FamilyCase::Transmission(module) => {
                    let diagnostics = powerio_core::render_diagnostics(&module.diagnostics);
                    transmission_summary_json(module.value(), &diagnostics)
                }
            }
        };
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn run_serialize(
    input: &Path,
    output: Option<&Path>,
    from: Option<FormatArg>,
) -> anyhow::Result<()> {
    let module = load_module(input, from)?;
    report_diagnostics(&module.diagnostics);
    let parse_errors = parse_error_count(&module.diagnostics);
    let text = serialize_module_text(&module)?;
    // This command is the primary producer of PowerIO IR, so a document the
    // deserializer refuses must not reach a file: read the document back
    // before writing it, and name the writer rather than leaving a consumer
    // to discover the refusal with neither the case nor the producer at hand.
    powerio::deserialize(
        powerio::Source::from_memory("module.pio.json", text.as_bytes().to_vec())
            .context("creating the PowerIO IR validation source")?,
    )
    .context("reading back the PowerIO IR just serialized")?;
    write_conversion_output(&text, &[], output)?;
    // The module is serialized either way, as the record of what the reader
    // saw; a refused include is an `Error` finding in its own document, so the
    // exit status says so, as `convert` does.
    fail_on_parse_errors(parse_errors)
}

/// The parse options for an optional `--from` token.
fn parse_options(format: Option<&str>) -> anyhow::Result<powerio::ParseOptions> {
    let mut options = powerio::ParseOptions::default();
    if let Some(format) = format {
        options = options.format(format).map_err(anyhow::Error::new)?;
    }
    Ok(options)
}

fn load_module(
    input: &Path,
    from: Option<FormatArg>,
) -> anyhow::Result<powerio_core::PioModule<powerio::PioValue>> {
    if is_stdin(input) {
        let format = stdin_format(from)?;
        return powerio::parse_with_options(stdin_source()?, &parse_options(Some(format.name()))?)
            .context("parsing standard input");
    }
    powerio::parse_with_options(input, &parse_options(from.map(FormatArg::name))?)
        .with_context(|| format!("parsing {}", input.display()))
}

fn deserialize_module(input: &Path) -> anyhow::Result<powerio_core::PioModule<powerio::PioValue>> {
    let source = powerio_core::Source::open(input)
        .with_context(|| format!("reading {}", input.display()))?;
    powerio::deserialize(source)
        .with_context(|| format!("deserializing PowerIO IR from {}", input.display()))
}

fn serialize_module_text(
    module: &powerio_core::PioModule<powerio::PioValue>,
) -> anyhow::Result<String> {
    let result = powerio::serialize(
        module,
        powerio::Destination::memory("module.pio.json")
            .context("creating the PowerIO IR memory destination")?,
    )
    .context("serializing PowerIO IR")?;
    let powerio::EmittedOutput::Memory { mut artifacts } = result.into_output() else {
        anyhow::bail!("a PowerIO IR memory destination returned path output")
    };
    let artifact = artifacts
        .pop()
        .filter(|_| artifacts.is_empty())
        .context("PowerIO IR serialization did not return exactly one artifact")?;
    String::from_utf8(artifact.into_bytes()).context("PowerIO IR is not valid UTF-8")
}

/// Serialized PowerIO IR and the `Error`-or-worse findings it carries.
#[cfg(test)]
fn serialize_input(input: &Path, from: Option<FormatArg>) -> anyhow::Result<(String, Vec<String>)> {
    let module = load_module(input, from)?;
    let errors = module_error_lines(&module);
    let text = serialize_module_text(&module)?;
    powerio::deserialize(
        powerio::Source::from_memory("module.pio.json", text.as_bytes().to_vec())
            .context("creating the PowerIO IR validation source")?,
    )
    .context("validating PowerIO IR deserialization")?;
    Ok((text, errors))
}

/// The `Error`-or-worse findings of a module as `CODE: message` lines.
#[cfg(test)]
fn module_error_lines(module: &powerio_core::PioModule<powerio::PioValue>) -> Vec<String> {
    module
        .diagnostics
        .iter()
        .filter(|d| d.severity() >= powerio_core::DiagnosticSeverity::Error)
        .map(|d| format!("{}: {}", d.code(), d.message()))
        .collect()
}

fn transmission_summary_json(
    net: &powerio_matrix::BalancedNetwork,
    warnings: &[String],
) -> serde_json::Value {
    let view = powerio_matrix::IndexedNetwork::new(net);
    json!({
        "schema": "powerio.summary",
        powerio::version::VERSION_KEY: powerio::VERSION,
        "domain": "transmission",
        "model": "balanced",
        "name": net.name(),
        "source_format": net.source_format().name(),
        "json_format": "model-json",
        "base_mva": net.base_mva(),
        "elements": {
            "buses": net.buses().len(),
            "branches": net.branches().len(),
            "generators": net.generators().len(),
            "loads": net.loads().len(),
            "shunts": net.shunts().len(),
            "lines": serde_json::Value::Null,
            "transformers": serde_json::Value::Null,
            "sources": serde_json::Value::Null,
        },
        "topology": {
            "connected_components": view.calc_island_count(),
            "is_radial": view.is_radial(),
            "reference_buses": view.reference_bus_indices(),
            "connectivity_report": view.calc_connectivity_report(),
        },
        "warnings": warnings,
    })
}

fn distribution_summary_json(
    net: &powerio_dist::MulticonductorNetwork,
    warnings: &[String],
) -> serde_json::Value {
    json!({
        "schema": "powerio.summary",
        powerio::version::VERSION_KEY: powerio::VERSION,
        "domain": "distribution",
        "model": "multiconductor",
        "name": net.name(),
        "source_format": net.source_format().map(powerio_dist::DistSourceFormat::name),
        "json_format": "bmopf-json",
        "base_mva": serde_json::Value::Null,
        "elements": {
            "buses": net.buses().len(),
            "branches": serde_json::Value::Null,
            "generators": net.generators().len(),
            "loads": net.loads().len(),
            "shunts": serde_json::Value::Null,
            "lines": net.lines().len(),
            "transformers": net.transformers().len(),
            "sources": net.sources().len(),
        },
        "topology": {
            "connected_components": serde_json::Value::Null,
            "is_radial": serde_json::Value::Null,
            "reference_buses": serde_json::Value::Null,
            "connectivity_report": serde_json::Value::Null,
        },
        "warnings": warnings,
    })
}

fn looks_like_gridfm_dir(input: &Path) -> bool {
    input.join("bus_data.parquet").is_file()
        || input.join("raw").join("bus_data.parquet").is_file()
        || std::fs::read_dir(input).is_ok_and(|entries| {
            entries
                .filter_map(std::result::Result::ok)
                .filter(|e| e.path().join("raw").join("bus_data.parquet").is_file())
                .take(2)
                .count()
                == 1
        })
}

// One conversion pipeline stage per block; splitting it would scatter the
// stage order this function exists to show.
#[allow(clippy::too_many_lines)]
fn run_convert(
    input: &std::path::Path,
    to: FormatArg,
    output: Option<&std::path::Path>,
    from: Option<FormatArg>,
    scenario: i64,
    gen_cost_options: GenCostCliOptions<'_>,
) -> anyhow::Result<()> {
    if to.is_input_spelling() {
        return Err(cli_failure(
            &codes::REQUEST_CLI_OUTPUT_REQUIRED,
            format!(
                "`{}` is accepted for input only; use `--to {}`",
                to.name(),
                if to == FormatArg::RawxInput {
                    "psse-rawx"
                } else {
                    "xiidm"
                }
            ),
        ));
    }
    // gridfm has no convert writer; the dataset writer is the `gridfm`
    // subcommand.
    if matches!(to, FormatArg::Gridfm) {
        return Err(cli_failure(
            &codes::REQUEST_CLI_TARGET_UNSUPPORTED,
            "`convert` cannot write a gridfm dataset; use the `gridfm` subcommand",
        ));
    }
    if matches!(to, FormatArg::Pwb) {
        return Err(cli_failure(
            &codes::REQUEST_CLI_OUTPUT_REQUIRED,
            "`convert` cannot write PowerWorld .pwb binary cases; use `--to powerworld` for AUX text",
        ));
    }
    if matches!(to, FormatArg::IeeeCdf) {
        return Err(cli_failure(
            &codes::REQUEST_CLI_TARGET_UNSUPPORTED,
            "`convert` cannot write IEEE CDF cases; the format is read only",
        ));
    }
    // A standard input case needs its declared format before any path based
    // inspection of the input runs.
    if is_stdin(input) {
        stdin_format(from)?;
    }
    if from == Some(FormatArg::Gridfm) {
        // GridFM selects one scenario before it has a scalar module to emit.
        // The directory targets retain their existing writers; text targets
        // use the same balanced module writer as every other scalar network.
        if to == FormatArg::PypsaCsv {
            return convert_to_pypsa_folder(input, output, from, scenario, gen_cost_options);
        }
        if to == FormatArg::Cgmes {
            return convert_to_cgmes(input, output, from, scenario, gen_cost_options);
        }
        let target = to.transmission().ok_or_else(|| {
            cli_failure(
                &codes::REQUEST_CLI_FAMILY_MISMATCH,
                format!(
                    "no conversion path between the transmission and distribution format families \
                 (`gridfm` to `{}`)",
                    to.name()
                ),
            )
        })?;
        let options = gen_cost_options.emit_options()?;
        let (network, diagnostics) = parse_gridfm_scenario(input, scenario)?;
        report_diagnostics(&diagnostics);
        let emission = module_io::emit_balanced_module(
            &powerio_core::PioModule::new(network),
            target,
            &options,
        )
        .with_context(|| format!("emitting {target}"))?;
        report_diagnostics(&emission.diagnostics);
        write_conversion_output(&emission.text, &emission.sidecars, output)?;
        return Ok(());
    }

    // PowerIO IR is decoded rather than parsed, then follows the same typed
    // conversion path as a freshly parsed module.
    if from.is_none() && cases::powerio_ir_text(input)?.is_some() {
        let module = deserialize_module(input)?;
        return convert_parsed_module(module, to, output, &gen_cost_options);
    }

    // Parse once through the facade. Static networks retain their established
    // emit options below; calculation instances and solutions stay typed and
    // go through the facade emitter for every target.
    let module = load_module(input, from)?;
    convert_parsed_module(module, to, output, &gen_cost_options)
}

fn convert_parsed_module(
    module: powerio::PioModule<powerio::PioValue>,
    to: FormatArg,
    output: Option<&Path>,
    gen_cost_options: &GenCostCliOptions<'_>,
) -> anyhow::Result<()> {
    match &module.value() {
        powerio::PioValue::BalancedNetwork(_) => {
            let module = module.map_value(|value| match value {
                powerio::PioValue::BalancedNetwork(network) => network,
                _ => unreachable!("the value variant was checked"),
            });
            convert_balanced_module(&module, to, output, gen_cost_options)
        }
        powerio::PioValue::MulticonductorNetwork(_) => {
            let module = module.map_value(|value| match value {
                powerio::PioValue::MulticonductorNetwork(network) => network,
                _ => unreachable!("the value variant was checked"),
            });
            convert_multiconductor_module(&module, to, output)
        }
        _ => convert_typed_module(&module, to, output),
    }
}

fn convert_balanced_module(
    module: &powerio::PioModule<powerio::BalancedNetwork>,
    to: FormatArg,
    output: Option<&Path>,
    gen_cost_options: &GenCostCliOptions<'_>,
) -> anyhow::Result<()> {
    let options = gen_cost_options.emit_options()?;
    if to == FormatArg::PypsaCsv {
        let Some(output) = output else {
            fail_with!(
                REQUEST_CLI_OUTPUT_REQUIRED,
                "`--to pypsa-csv` requires `-o <output-dir>`"
            );
        };
        if output.as_os_str() == "-" {
            fail_with!(
                REQUEST_CLI_OUTPUT_REQUIRED,
                "`--to pypsa-csv` writes a directory and cannot write to stdout"
            );
        }
        let result = powerio_tx::__emit_pypsa_csv_with_options(
            module,
            &options,
            powerio_core::Destination::path(output),
        )
        .with_context(|| format!("emitting PyPSA CSV folder {}", output.display()))?;
        return finish_path_emission(&module.diagnostics, &result, output);
    }
    if to == FormatArg::Cgmes {
        let Some(output) = output else {
            fail_with!(
                REQUEST_CLI_OUTPUT_REQUIRED,
                "`--to cgmes` requires `-o <output-path>`"
            );
        };
        if output.as_os_str() == "-" {
            fail_with!(
                REQUEST_CLI_OUTPUT_REQUIRED,
                "`--to cgmes` cannot write a profile set to stdout"
            );
        }
        let result = powerio_tx::emit_with_options(
            module,
            powerio_tx::TargetFormat::Cgmes,
            &options,
            powerio_core::Destination::path(output),
        )
        .with_context(|| format!("emitting CGMES to {}", output.display()))?;
        return finish_path_emission(&module.diagnostics, &result, output);
    }
    let target = to.transmission().ok_or_else(|| {
        failure!(
            REQUEST_CLI_FAMILY_MISMATCH,
            "no conversion path between the transmission and distribution format families \
             (balanced network to `{}`)",
            to.name()
        )
    })?;
    let emission = module_io::emit_balanced_module(module, target, &options)
        .with_context(|| format!("emitting {target}"))?;
    finish_memory_emission(&module.diagnostics, &emission, output)
}

fn convert_multiconductor_module(
    module: &powerio::PioModule<powerio::MulticonductorNetwork>,
    to: FormatArg,
    output: Option<&Path>,
) -> anyhow::Result<()> {
    let target = to.distribution().ok_or_else(|| {
        failure!(
            REQUEST_CLI_FAMILY_MISMATCH,
            "no conversion path between the transmission and distribution format families \
             (multiconductor network to `{}`)",
            to.name()
        )
    })?;
    let emission = module_io::emit_multiconductor_module(module, target)
        .with_context(|| format!("emitting {}", target.name()))?;
    finish_memory_emission(&module.diagnostics, &emission, output)
}

fn convert_typed_module(
    module: &powerio::PioModule<powerio::PioValue>,
    to: FormatArg,
    output: Option<&Path>,
) -> anyhow::Result<()> {
    let format = powerio::resolve_format(to.name()).ok_or_else(|| {
        failure!(
            REQUEST_CLI_OUTPUT_REQUIRED,
            "unknown target format `{}`",
            to.name()
        )
    })?;
    if format.is_directory {
        let Some(output) = output else {
            fail_with!(
                REQUEST_CLI_OUTPUT_REQUIRED,
                "`--to {}` requires `-o <output-path>`",
                format.token
            );
        };
        if output.as_os_str() == "-" {
            fail_with!(
                REQUEST_CLI_OUTPUT_REQUIRED,
                "`--to {}` cannot write to stdout",
                format.token
            );
        }
        let result = powerio::emit(
            module,
            format.token,
            powerio_core::Destination::path(output),
        )
        .with_context(|| format!("emitting {} to {}", format.token, output.display()))?;
        return finish_path_emission(&module.diagnostics, &result, output);
    }
    let emission = module_io::emit_module(module, format.token)
        .with_context(|| format!("emitting {}", format.token))?;
    finish_memory_emission(&module.diagnostics, &emission, output)
}

fn finish_memory_emission(
    parse_diagnostics: &[powerio::Diagnostic],
    emission: &module_io::MemoryEmission,
    output: Option<&Path>,
) -> anyhow::Result<()> {
    let mut diagnostics = parse_diagnostics.to_vec();
    diagnostics.extend(emission.diagnostics.iter().cloned());
    report_diagnostics(&diagnostics);
    write_conversion_output(&emission.text, &emission.sidecars, output)?;
    fail_on_parse_errors(parse_error_count(&diagnostics))
}

fn finish_path_emission(
    parse_diagnostics: &[powerio::Diagnostic],
    result: &powerio::EmitResult,
    output: &Path,
) -> anyhow::Result<()> {
    let mut diagnostics = parse_diagnostics.to_vec();
    diagnostics.extend(result.diagnostics().iter().cloned());
    report_diagnostics(&diagnostics);
    report_written(output);
    fail_on_parse_errors(parse_error_count(&diagnostics))
}

/// The number of `Error`-or-worse findings among `diagnostics`.
fn parse_error_count(diagnostics: &[powerio_core::Diagnostic]) -> usize {
    diagnostics
        .iter()
        .filter(|d| d.severity() >= powerio_core::DiagnosticSeverity::Error)
        .count()
}

/// Exit nonzero after the output is written: the file exists for
/// inspection, but the parse was incomplete and scripts must not treat the
/// run as clean (#275). The error records themselves were reported with the
/// other diagnostics.
fn fail_on_parse_errors(parse_errors: usize) -> anyhow::Result<()> {
    if parse_errors == 0 {
        return Ok(());
    }
    Err(cli_failure(
        &codes::PARSE_CLI_ERRORS_REPORTED,
        format!("the reader reported {parse_errors} error(s); the output is incomplete"),
    ))
}

/// Commit one complete output file through the no-replace destination:
/// staged beside the target and moved into place only when no entry exists
/// there, so an existing entry is refused rather than replaced.
fn commit_output_file(path: &std::path::Path, bytes: Vec<u8>) -> anyhow::Result<()> {
    let artifact = powerio_core::MemoryArtifact::new(
        powerio_core::ArtifactPath::new("case").expect("static placeholder name"),
        bytes,
    );
    powerio_core::Destination::path(path)
        .__commit_artifacts(
            false,
            powerio_core::Fidelity::Canonical,
            vec![artifact],
            Vec::new(),
        )
        .map(|_| ())
        .map_err(anyhow::Error::new)
}

/// Write emitted `text` to `output` (stdout on `-` or `None`), placing any
/// `sidecars` next to it. Sidecars cannot follow text to stdout; they are
/// reported instead.
fn write_conversion_output(
    text: &str,
    sidecars: &[module_io::MemorySidecar],
    output: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    match output {
        Some(p) if p.as_os_str() != "-" => {
            // The case file and its sidecars land beside each other in the
            // caller's directory, which may hold unrelated files, so each
            // file commits individually with no replacement and a refusal
            // removes the files this call created.
            let mut committed: Vec<std::path::PathBuf> = Vec::new();
            let result = (|| -> anyhow::Result<()> {
                commit_output_file(p, text.as_bytes().to_vec())
                    .with_context(|| format!("writing {}", p.display()))?;
                committed.push(p.to_path_buf());
                report_written(p);
                let base = p.parent().unwrap_or_else(|| std::path::Path::new("."));
                for sidecar in sidecars {
                    // The case text refers to a sidecar by a relative name, so
                    // the file must stay under the output directory.
                    if !is_relative_component_path(&sidecar.path) {
                        fail_with!(
                            EMIT_CLI_SIDECAR_PATH,
                            "sidecar `{}` is not a relative path under the output directory",
                            sidecar.path
                        );
                    }
                    let path = base.join(&sidecar.path);
                    commit_output_file(&path, sidecar.bytes.clone())
                        .with_context(|| format!("writing {}", path.display()))?;
                    committed.push(path.clone());
                    report_written(&path);
                }
                Ok(())
            })();
            if result.is_err() {
                for created in &committed {
                    let _ = std::fs::remove_file(created);
                }
            }
            result?;
        }
        _ => {
            for sidecar in sidecars {
                report_diagnostics(&[powerio_core::Diagnostic::of(
                    &powerio_dist::diagnostics::codes::EMIT_MULTICONDUCTOR_SIDECAR_DROPPED,
                    format!(
                        "sidecar `{}` was not written because the output is standard output",
                        sidecar.path
                    ),
                )]);
            }
            print!("{text}");
        }
    }
    Ok(())
}

/// Whether `path` is relative and built only from ordinary names, so joining
/// it onto an output directory cannot leave that directory. Rejects absolute
/// paths, `..`, and Windows drive and root prefixes; allows subdirectories.
fn is_relative_component_path(path: &str) -> bool {
    !path.is_empty()
        && std::path::Path::new(path)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}

fn run_geo_extract(
    input: &std::path::Path,
    output: Option<&std::path::Path>,
    from: Option<FormatArg>,
) -> anyhow::Result<()> {
    // A `.pwd` display file promotes to a diagram space layer with
    // substation targets; apply it onto a case with `geo apply`.
    if from.is_none()
        && input
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("pwd"))
    {
        let source = powerio_core::Source::open(input)
            .with_context(|| format!("reading {}", input.display()))?;
        let display = powerio_tx::format::parse_display(source, None)
            .with_context(|| format!("reading {}", input.display()))?;
        let powerio::DisplayData::PowerWorld(display) = display else {
            fail_with!(
                VALIDATE_CLI_INPUT_LACKS_DATA,
                "{} did not parse as a .pwd display",
                input.display()
            );
        };
        let layer = powerio::geo::to_geo_layer_from_pwd(&display);
        if layer.features.is_empty() {
            fail_with!(
                VALIDATE_CLI_INPUT_LACKS_DATA,
                "{} carries no substation symbols",
                input.display()
            );
        }
        return write_conversion_output(&layer.to_geojson(), &[], output);
    }
    let layer = match parse_family_case(input, from)? {
        FamilyCase::Distribution(module) => {
            report_diagnostics(&module.diagnostics);
            powerio::dist_geo::to_dist_geo_layer(module.value())
        }
        FamilyCase::Transmission(module) => {
            report_diagnostics(&module.diagnostics);
            module.value().to_geo_layer()
        }
    };
    if layer.features.is_empty() {
        fail_with!(
            VALIDATE_CLI_INPUT_LACKS_DATA,
            "{} carries no coordinates to extract",
            input.display()
        );
    }
    write_conversion_output(&layer.to_geojson(), &[], output)
}

// Both family branches follow the same five steps (parse, apply, drop the
// retained source so a same-format write re-serializes the placed case,
// resolve the target, serialize); they stay separate because the two model
// families have distinct parse and write APIs.
fn run_geo_apply(
    input: &std::path::Path,
    layer_path: &std::path::Path,
    output: Option<&std::path::Path>,
    to: Option<FormatArg>,
    from: Option<FormatArg>,
) -> anyhow::Result<()> {
    if let Some(format) = to.filter(|format| format.is_input_spelling()) {
        fail_with!(
            REQUEST_CLI_OUTPUT_REQUIRED,
            "`{}` is accepted for input only; use `--to {}`",
            format.name(),
            if format == FormatArg::RawxInput {
                "psse-rawx"
            } else {
                "xiidm"
            }
        );
    }
    let text = read_text_file(layer_path)?;
    let parsed =
        powerio::geo::GeoLayer::parse(&text, layer_path.file_name().and_then(|n| n.to_str()))
            .with_context(|| format!("parsing layer {}", layer_path.display()))?;
    report_diagnostics(&parsed.diagnostics);
    let (text, sidecars, diagnostics) = match parse_family_case(input, from)? {
        FamilyCase::Distribution(module) => {
            report_diagnostics(&module.diagnostics);
            let dynamic = module.map_value(powerio::PioValue::from);
            let (placed, report) = powerio::apply_geo_layer(&dynamic, &parsed.layer)?;
            report_geo_apply(&report);
            let placed = placed.map_value(|value| match value {
                powerio::PioValue::MulticonductorNetwork(network) => network,
                _ => unreachable!("apply_geo_layer preserves the network type"),
            });
            let target = match to {
                Some(f) => f.distribution().ok_or_else(|| {
                    failure!(
                        REQUEST_CLI_TARGET_UNSUPPORTED,
                        "`{}` is not a distribution text target; a distribution case writes \
                         back to dss, pmd-json, or bmopf-json",
                        f.name()
                    )
                })?,
                None => placed
                    .value()
                    .source_format()
                    .map(|f| f.name().parse())
                    .transpose()?
                    .ok_or_else(|| {
                        failure!(
                            REQUEST_CLI_TARGET_UNSUPPORTED,
                            "the input carries no source format; pass --to"
                        )
                    })?,
            };
            let emission = module_io::emit_multiconductor_module(&placed, target)?;
            (emission.text, emission.sidecars, emission.diagnostics)
        }
        FamilyCase::Transmission(module) => {
            report_diagnostics(&module.diagnostics);
            let dynamic = module.map_value(powerio::PioValue::from);
            let (placed, report) = powerio::apply_geo_layer(&dynamic, &parsed.layer)?;
            report_geo_apply(&report);
            let placed = placed.map_value(|value| match value {
                powerio::PioValue::BalancedNetwork(network) => network,
                _ => unreachable!("apply_geo_layer preserves the network type"),
            });
            let target = match to {
                Some(f) => f.transmission().ok_or_else(|| {
                    failure!(
                        REQUEST_CLI_TARGET_UNSUPPORTED,
                        "`{}` is not a transmission text target here; apply writes a single \
                         case file (use `convert` for pypsa-csv and gridfm outputs)",
                        f.name()
                    )
                })?,
                None => powerio_tx::format::parse_target_format(&format!(
                    "{:?}",
                    placed.value().source_format()
                ))
                .ok_or_else(|| {
                    failure!(
                        REQUEST_CLI_TARGET_UNSUPPORTED,
                        "`{:?}` has no write target; pass --to to choose one",
                        placed.value().source_format()
                    )
                })?,
            };
            let emission = module_io::emit_balanced_module(
                &placed,
                target,
                &powerio_tx::EmitOptions::default(),
            )
            .with_context(|| format!("emitting {target}"))?;
            (emission.text, emission.sidecars, emission.diagnostics)
        }
    };
    report_diagnostics(&diagnostics);
    write_conversion_output(&text, &sidecars, output)
}

fn report_geo_apply(report: &powerio::GeoApplyReport) {
    report_progress(format!(
        "applied: {} bus point(s), {} branch route(s), {} unmatched feature(s)",
        report.matched_buses, report.matched_branches, report.unmatched_features
    ));
    report_progress(format!(
        "unplaced: {} bus(es) with no location, {} branch(es) with no route",
        report.unlocated_buses, report.unlocated_branches
    ));
    // A point sidecar or a substation join states no polylines, so every branch
    // lands in that second count on a run that placed everything it could.
    // `GeoApplyReport::require_located` is the strict check for a caller that
    // does want a route on every branch.
    if report.unlocated_branches > 0 {
        report_progress(
            "note: a branch with no route renders from its bus endpoints; only a source \
             stating intermediate geometry gives it one",
        );
    }
    for note in &report.notes {
        report_progress(format!("note: {note}"));
    }
}

fn run_geo_convert(
    input: &std::path::Path,
    output: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    let text = read_text_file(input)?;
    let parsed = powerio::geo::GeoLayer::parse(&text, input.file_name().and_then(|n| n.to_str()))
        .with_context(|| format!("parsing {}", input.display()))?;
    report_diagnostics(&parsed.diagnostics);
    write_conversion_output(&parsed.layer.to_geojson(), &[], output)
}

/// Write `input` out as a PyPSA CSV folder (a directory target, so it never
/// returns text). gridfm input reads through the dataset reader; everything else
/// goes through the shared transmission hub.
fn convert_to_pypsa_folder(
    input: &std::path::Path,
    output: Option<&std::path::Path>,
    from: Option<FormatArg>,
    scenario: i64,
    gen_cost_options: GenCostCliOptions<'_>,
) -> anyhow::Result<()> {
    let Some(out_dir) = output else {
        fail_with!(
            REQUEST_CLI_OUTPUT_REQUIRED,
            "`--to pypsa-csv` requires `-o <output-dir>`"
        );
    };
    if out_dir.as_os_str() == "-" {
        fail_with!(
            REQUEST_CLI_OUTPUT_REQUIRED,
            "`--to pypsa-csv` writes a directory and cannot write to stdout"
        );
    }
    let net = if from == Some(FormatArg::Gridfm) {
        let (network, diagnostics) = parse_gridfm_scenario(input, scenario)?;
        report_diagnostics(&diagnostics);
        network
    } else {
        read_network(input, from)?
    };
    // Use the component bridge so the cost policy and its diagnostics are
    // stated once for every surface.
    let options = gen_cost_options.emit_options()?;
    let result = powerio_tx::__emit_pypsa_csv_with_options(
        &powerio_core::PioModule::new(net),
        &options,
        powerio_core::Destination::path(out_dir),
    )
    .with_context(|| format!("emitting PyPSA CSV folder {}", out_dir.display()))?;
    report_diagnostics(result.diagnostics());
    report_written(out_dir);
    Ok(())
}

/// Write `input` as a CGMES profile set. Fresh output is a directory. An
/// unchanged CGMES ZIP remains the same ZIP, so its output path names a file.
fn convert_to_cgmes(
    input: &std::path::Path,
    output: Option<&std::path::Path>,
    from: Option<FormatArg>,
    scenario: i64,
    gen_cost_options: GenCostCliOptions<'_>,
) -> anyhow::Result<()> {
    let Some(output) = output else {
        fail_with!(
            REQUEST_CLI_OUTPUT_REQUIRED,
            "`--to cgmes` requires `-o <output-path>`"
        );
    };
    if output.as_os_str() == "-" {
        fail_with!(
            REQUEST_CLI_OUTPUT_REQUIRED,
            "`--to cgmes` cannot write a profile set to stdout"
        );
    }

    let module = if from == Some(FormatArg::Gridfm) {
        let (network, diagnostics) = parse_gridfm_scenario(input, scenario)?;
        report_diagnostics(&diagnostics);
        powerio_core::PioModule::new(network)
    } else {
        match parse_family_case(input, from)? {
            FamilyCase::Transmission(module) => *module,
            FamilyCase::Distribution(_) => fail_with!(
                REQUEST_CLI_FAMILY_MISMATCH,
                "{} is a distribution case; CGMES represents a balanced transmission network",
                input.display()
            ),
        }
    };
    let parse_errors = parse_error_count(&module.diagnostics);
    report_diagnostics(&module.diagnostics);
    let options = gen_cost_options.emit_options()?;
    let result = powerio_tx::emit_with_options(
        &module,
        powerio_tx::TargetFormat::Cgmes,
        &options,
        powerio_core::Destination::path(output),
    )
    .with_context(|| format!("emitting CGMES to {}", output.display()))?;
    report_diagnostics(result.diagnostics());
    report_written(output);
    fail_on_parse_errors(parse_errors)
}

/// A single-file case input parsed to its own family model.
enum FamilyCase {
    Transmission(Box<powerio_core::PioModule<powerio::BalancedNetwork>>),
    Distribution(Box<powerio_core::PioModule<powerio::MulticonductorNetwork>>),
}

/// Parse a single-file case to whichever family model it belongs to. With no
/// `--from`, a `.json` is read and DOM-classified once and the same text feeds
/// the typed parser — the read-once rule #260 established for `batch` and the
/// TUI, extended here to the single-file routes. Warnings stay on the returned
/// value: the callers differ in where they surface them (summary JSON, module
/// diagnostics, stderr).
/// One balanced network from any single case input (including PowerIO IR),
/// for the matrix commands. A distribution input is refused with
/// the family named.
fn balanced_case(
    input: &Path,
    from: Option<FormatArg>,
) -> anyhow::Result<powerio_matrix::BalancedNetwork> {
    match parse_family_case(input, from)? {
        FamilyCase::Transmission(module) => Ok(module.into_value()),
        FamilyCase::Distribution(_) => Err(cli_failure(
            &codes::REQUEST_CLI_FAMILY_MISMATCH,
            format!(
                "{} is a distribution case; this command needs a transmission network",
                input.display()
            ),
        )),
    }
}

/// Deserialize PowerIO IR and adapt a static network to the CLI's family case.
/// Other values are rejected instead of guessing a projection.
fn ir_family_case(input: &Path) -> anyhow::Result<FamilyCase> {
    let module = deserialize_module(input)?;
    match &module.value() {
        powerio::PioValue::BalancedNetwork(_) => {
            let module = module.map_value(|value| match value {
                powerio::PioValue::BalancedNetwork(network) => network,
                _ => unreachable!("the value variant was checked"),
            });
            Ok(FamilyCase::Transmission(Box::new(module)))
        }
        powerio::PioValue::MulticonductorNetwork(_) => {
            let module = module.map_value(|value| match value {
                powerio::PioValue::MulticonductorNetwork(network) => network,
                _ => unreachable!("the value variant was checked"),
            });
            Ok(FamilyCase::Distribution(Box::new(module)))
        }
        other => fail_with!(
            REQUEST_CLI_FAMILY_MISMATCH,
            "{} stores {}; this command requires powerio.BalancedNetwork or \
             powerio.MulticonductorNetwork",
            input.display(),
            other.type_name()
        ),
    }
}

fn parse_family_case(input: &Path, from: Option<FormatArg>) -> anyhow::Result<FamilyCase> {
    if is_stdin(input) {
        let f = stdin_format(from)?;
        let source = module_io::declare_format(stdin_source()?, Some(f.name()))
            .context("declaring the standard input format")?;
        return if f.distribution().is_some() {
            let net = powerio_dist::parse(source).context("reading standard input")?;
            Ok(FamilyCase::Distribution(Box::new(net)))
        } else {
            let parsed = powerio_tx::parse(source).context("reading standard input")?;
            Ok(FamilyCase::Transmission(Box::new(parsed)))
        };
    }
    if let Some(f) = from {
        if f == FormatArg::Gridfm {
            return Err(cli_failure(
                &codes::REQUEST_CLI_OPTION_INVALID,
                "gridfm datasets are read by `convert --from gridfm` or the `gridfm` \
                 subcommand, not this command",
            ));
        }
        return if f.distribution().is_some() {
            let net = module_io::load_multiconductor_module(input, Some(f.name()))
                .with_context(|| format!("reading {}", input.display()))?;
            Ok(FamilyCase::Distribution(Box::new(net)))
        } else {
            let parsed = module_io::load_balanced_module(input, Some(f.name()))
                .with_context(|| format!("reading {}", input.display()))?;
            Ok(FamilyCase::Transmission(Box::new(parsed)))
        };
    }
    if cases::powerio_ir_text(input)?.is_some() {
        return ir_family_case(input);
    }
    if let Some(case) = cases::classified_json(input)? {
        return parse_classified_case(&case, input);
    }
    if cases::looks_like_distribution_input(input)? {
        let net = module_io::load_multiconductor_module(input, None)
            .with_context(|| format!("reading {}", input.display()))?;
        Ok(FamilyCase::Distribution(Box::new(net)))
    } else {
        let parsed = module_io::load_balanced_module(input, None)
            .with_context(|| format!("reading {}", input.display()))?;
        Ok(FamilyCase::Transmission(Box::new(parsed)))
    }
}

/// Parse already classified `.json` text through its family's memory source,
/// keeping the file stem as the name hint the path-based parsers would use.
fn parse_classified_case(case: &cases::ClassifiedCase, input: &Path) -> anyhow::Result<FamilyCase> {
    match case.format {
        DetectedFormat::Distribution(_) => {
            // The shared classifier routes the family; the dist crate owns
            // which documents are cases at all, and its rule is stricter
            // (a BMOPF document needs a `bus` table). Apply it to the text
            // already read, as the path source does.
            let format = powerio_dist::classify_distribution_json(&case.text)
                .with_context(|| format!("reading {}", input.display()))?;
            let net = module_io::load_multiconductor_memory(&case.text, format.name())
                .with_context(|| format!("reading {}", input.display()))?;
            Ok(FamilyCase::Distribution(Box::new(net)))
        }
        DetectedFormat::Transmission(format) => {
            let stem = input.file_stem().and_then(|s| s.to_str());
            let parsed = module_io::load_balanced_memory_named(&case.text, format.name(), stem)
                .with_context(|| format!("reading {}", input.display()))?;
            Ok(FamilyCase::Transmission(Box::new(parsed)))
        }
        _ => unreachable!("classified_json returns transmission or distribution formats only"),
    }
}

/// Read `input` into the neutral [`powerio_matrix::BalancedNetwork`] through the shared
/// format hub, which picks the reader from `from` or the extension (sniffing a
/// `.json` with the shared top level shape classifier). The distribution
/// formats are rejected up front: every caller of this function consumes the
/// transmission model, and clap can't express the restriction on the shared
/// `FormatArg`. Read fidelity warnings print to stderr like the write side's.
fn reject_nontransmission_from(from: Option<FormatArg>) -> anyhow::Result<()> {
    if let Some(f) = from {
        if matches!(f, FormatArg::Gridfm) {
            fail_with!(
                REQUEST_CLI_OPTION_INVALID,
                "gridfm datasets are read by `convert --from gridfm` or the `gridfm` \
                 subcommand, not this command"
            );
        }
        if f.distribution().is_some() {
            fail_with!(
                REQUEST_CLI_FAMILY_MISMATCH,
                "`{}` is a distribution format; this command reads transmission cases \
                 (use `convert` to bridge dss, pmd-json, and bmopf-json)",
                f.name()
            );
        }
    }
    Ok(())
}

fn read_network(
    input: &std::path::Path,
    from: Option<FormatArg>,
) -> anyhow::Result<powerio_matrix::BalancedNetwork> {
    reject_nontransmission_from(from)?;
    let parsed = module_io::load_balanced_module(input, from.map(FormatArg::name))
        .with_context(|| format!("reading {}", input.display()))?;
    report_diagnostics(&parsed.diagnostics);
    Ok(parsed.into_value())
}

#[cfg(test)]
mod tests {
    use super::cases::{infer_input_family, looks_like_distribution_input};
    use super::{
        BranchSusceptanceFormulaArg, Cli, Command, FamilyCase, FormatArg, GenCostCliOptions,
        distribution_summary_json, parse_family_case, run_convert, run_serialize, serialize_input,
        transmission_summary_json,
    };
    use clap::Parser;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn data(path: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("tests")
            .join("data")
            .join(path)
    }

    fn deserialize_module(text: &str) -> powerio::PioModule<powerio::PioValue> {
        let source =
            powerio::Source::from_memory("module.pio.json", text.as_bytes().to_vec()).unwrap();
        powerio::deserialize(source).unwrap()
    }

    #[test]
    fn summary_json_matches_canonical_transmission_shape() {
        let parsed = crate::module_io::load_balanced_module(data("case9.m"), None).unwrap();
        let diagnostics = powerio_core::render_diagnostics(&parsed.diagnostics);
        let value = transmission_summary_json(parsed.value(), &diagnostics);
        assert_eq!(value["schema"], "powerio.summary");
        assert_eq!(value[powerio::version::VERSION_KEY], powerio::VERSION);
        assert_eq!(value["domain"], "transmission");
        assert_eq!(value["model"], "balanced");
        assert_eq!(value["json_format"], "model-json");
        assert_eq!(value["elements"]["buses"], 9);
        assert_eq!(value["topology"]["connected_components"], 1);
    }

    #[test]
    fn opfdata_alias_routes_through_the_transmission_hub() {
        let cli = Cli::try_parse_from([
            "powerio",
            "convert",
            "example_0.json",
            "--from",
            "opfdata",
            "--to",
            "matpower",
        ])
        .unwrap();
        match cli.command {
            Some(Command::Convert { from, to, .. }) => {
                assert_eq!(from, Some(FormatArg::DeepMindOpfDataJson));
                assert_eq!(to, FormatArg::Matpower);
            }
            other => panic!("unexpected command: {other:?}"),
        }

        let parsed =
            crate::module_io::load_balanced_module(data("opfdataset/example_0.json"), None)
                .unwrap();
        assert_eq!(
            parsed.value().source_format(),
            powerio_matrix::SourceFormat::DeepMindOpfDataJson
        );
    }

    #[test]
    fn summary_json_matches_canonical_distribution_shape() {
        let net = crate::module_io::load_multiconductor_module(
            &data("dist/micro/xfmr_single_phase.dss"),
            None,
        )
        .unwrap();
        let diagnostics = powerio_core::render_diagnostics(&net.diagnostics);
        let value = distribution_summary_json(net.value(), &diagnostics);
        assert_eq!(value["schema"], "powerio.summary");
        assert_eq!(value[powerio::version::VERSION_KEY], powerio::VERSION);
        assert_eq!(value["domain"], "distribution");
        assert_eq!(value["model"], "multiconductor");
        assert_eq!(value["json_format"], "bmopf-json");
        assert_eq!(value["elements"]["buses"], 2);
        assert!(value["topology"]["connected_components"].is_null());
    }

    #[test]
    fn distribution_json_shape_check_uses_shared_classifier() {
        let tmp = std::env::temp_dir().join(format!(
            "powerio-summary-routing-{}.json",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&tmp, r#"{"bus":{"a":{"terminal_names":["1"]}}}"#).unwrap();
        assert!(looks_like_distribution_input(&tmp).unwrap());
        std::fs::write(
            &tmp,
            std::fs::read_to_string(data("egret/case9.json")).unwrap(),
        )
        .unwrap();
        assert!(!looks_like_distribution_input(&tmp).unwrap());
        let _ = std::fs::remove_file(tmp);
    }

    #[test]
    fn family_case_routes_json_by_classifier_without_from() {
        // The classifier's verdict picks the family and format from one
        // read. The stem still names a nameless transmission case.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let egret = dir.join("myegret.json");
        std::fs::write(
            &egret,
            std::fs::read_to_string(data("egret/case9.json")).unwrap(),
        )
        .unwrap();
        match parse_family_case(&egret, None).unwrap() {
            FamilyCase::Transmission(parsed) => {
                assert_eq!(parsed.value().buses().len(), 9);
                assert_eq!(parsed.value().name(), "myegret");
            }
            FamilyCase::Distribution(_) => panic!("egret JSON classified as distribution"),
        }

        let dist = dir.join("feeder.json");
        std::fs::write(
            &dist,
            r#"{
                "bus":{"a":{"terminal_names":["1"]}},
                "voltage_source":{"source":{
                    "bus":"a",
                    "terminal_map":["1"],
                    "v_magnitude":[1.0],
                    "v_angle":[0.0]
                }},
                "meta":{"version":"0.1.0"}
            }"#,
        )
        .unwrap();
        match parse_family_case(&dist, None).unwrap() {
            FamilyCase::Distribution(net) => assert_eq!(net.value().buses().len(), 1),
            FamilyCase::Transmission(_) => panic!("BMOPF JSON classified as transmission"),
        }
    }

    #[test]
    fn family_case_applies_the_distribution_readers_own_refusal() {
        // The shared classifier calls any document with a `linecode` table
        // distribution, but a BMOPF case needs a `bus` table too. The read
        // once path must apply that rule, not parse a bogus empty network.
        let tmp = tempfile::tempdir().unwrap();
        let orphan = tmp.path().join("orphan.json");
        std::fs::write(&orphan, r#"{"linecode":{"lc1":{"nphases":1}}}"#).unwrap();

        let text = match parse_family_case(&orphan, None) {
            Err(err) => format!("{err:#}"),
            Ok(_) => panic!("a linecode-only document parsed as a distribution case"),
        };
        assert!(
            text.contains("not a recognized distribution document"),
            "{text}"
        );
    }

    #[test]
    fn serialized_module_reads_back_as_a_family_case() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("powerio-cli-stored-{stamp}.pio.json"));
        let (text, _) = serialize_input(&data("case9.m"), None).unwrap();
        std::fs::write(&path, text).unwrap();

        match super::ir_family_case(&path).unwrap() {
            super::FamilyCase::Transmission(parsed) => {
                assert_eq!(parsed.value().buses().len(), 9);
            }
            super::FamilyCase::Distribution(_) => panic!("case9 is transmission"),
        }
        let net = super::balanced_case(&path, None).unwrap();
        assert_eq!(net.buses().len(), 9);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn serialize_command_parses() {
        let cli = Cli::try_parse_from(["powerio", "serialize", "case9.m"]).unwrap();
        match cli.command.unwrap() {
            Command::Serialize { input, .. } => assert_eq!(input, Path::new("case9.m")),
            other => panic!("expected serialize command, got {other:?}"),
        }
    }

    #[test]
    fn ir_text_matches_module_shape_and_provenance() {
        let input = data("case9.m");
        let (text, _) = serialize_input(&input, None).unwrap();
        let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(doc["schema"], "powerio.module");
        assert_eq!(doc["version"], 1);
        assert_eq!(doc["value"]["type"], "powerio.BalancedNetwork");
        assert_eq!(doc["value"]["data"]["buses"].as_array().unwrap().len(), 9);
        let sources = doc["sources"].as_array().unwrap();
        assert!(
            sources.iter().any(|s| {
                s["name"]
                    .as_str()
                    .is_some_and(|name| name.ends_with("case9.m"))
            }),
            "expected the case file among the module sources: {sources:?}"
        );
        let module = deserialize_module(&text);
        assert!(matches!(
            &module.value(),
            powerio::PioValue::BalancedNetwork(_)
        ));
    }

    #[test]
    fn serialize_command_writes_output_file() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let output = std::env::temp_dir().join(format!("powerio-package-{stamp}.pio.json"));

        run_serialize(&data("case9.m"), Some(&output), None).unwrap();
        let text = std::fs::read_to_string(&output).unwrap();
        let module = deserialize_module(&text);
        assert!(matches!(
            &module.value(),
            powerio::PioValue::BalancedNetwork(_)
        ));

        let _ = std::fs::remove_file(output);
    }

    #[test]
    fn module_helper_returns_stdout_text() {
        let (text, _) = serialize_input(&data("case9.m"), None).unwrap();
        let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(doc["producer"]["name"], "powerio");
        assert_eq!(doc["value"]["data"]["buses"].as_array().unwrap().len(), 9);
    }

    #[test]
    fn ir_text_round_trips_through_deserialize() {
        let (text, _) = serialize_input(&data("case9.m"), None).unwrap();
        let module = deserialize_module(&text);
        let again = super::serialize_module_text(&module).unwrap();
        assert_eq!(
            text, again,
            "the PowerIO IR document is serialization stable"
        );
    }

    #[test]
    fn ir_distribution_fixture_stores_the_multiconductor_value() {
        let input = data("dist/micro/xfmr_single_phase.dss");
        let (text, _) = serialize_input(&input, None).unwrap();
        let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(doc["value"]["type"], "powerio.MulticonductorNetwork");
        let sources = doc["sources"].as_array().unwrap();
        assert!(
            sources.iter().any(|s| {
                s["name"]
                    .as_str()
                    .is_some_and(|name| name.ends_with("xfmr_single_phase.dss"))
            }),
            "expected the dss file among the module sources: {sources:?}"
        );
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn ir_carries_a_nonfinite_payload_as_string_spellings() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let input = std::env::temp_dir().join(format!("powerio-package-bad-{stamp}.m"));
        let output = std::env::temp_dir().join(format!("powerio-package-bad-{stamp}.pio.json"));
        std::fs::write(
            &input,
            "\
function mpc = bad
mpc.version = '2';
mpc.baseMVA = 100;
mpc.bus = [
  1 3 0 0 0 0 1 1 0 230 1 1.1 0.9;
  2 1 0 0 0 0 1 1 0 230 1 1.1 0.9;
];
mpc.branch = [
  1 2 0.01 0.1 0 0 0 0 0 0 1 NaN Inf;
];
",
        )
        .unwrap();

        run_serialize(&input, Some(&output), None).unwrap();
        let text = std::fs::read_to_string(&output).unwrap();
        assert!(text.contains("\"angmin\": \"NaN\""), "{text}");
        assert!(text.contains("\"angmax\": \"Infinity\""), "{text}");
        let module = deserialize_module(&text);
        let powerio::PioValue::BalancedNetwork(network) = &module.value() else {
            panic!(
                "expected a balanced network, found {}",
                module.value().type_name()
            );
        };
        assert!(network.branches()[0].angmin.is_nan());
        assert_eq!(network.branches()[0].angmax, f64::INFINITY);

        let _ = std::fs::remove_file(input);
        let _ = std::fs::remove_file(output);
    }

    #[test]
    fn convert_rejects_transmission_json_to_distribution_without_format() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let input = std::env::temp_dir().join(format!("powerio-convert-pm-{stamp}.json"));
        let output = std::env::temp_dir().join(format!("powerio-convert-pm-{stamp}.dss"));
        let parsed = crate::module_io::load_balanced_module(data("case9.m"), None).unwrap();
        let emission = crate::module_io::emit_balanced_module(
            &parsed,
            powerio_tx::TargetFormat::PowerModelsJson,
            &powerio_tx::EmitOptions::default(),
        )
        .unwrap();
        std::fs::write(&input, emission.text).unwrap();

        assert_eq!(infer_input_family(&input).unwrap(), Some(false));
        let err = run_convert(
            &input,
            FormatArg::Dss,
            Some(&output),
            None,
            0,
            GenCostCliOptions::preserve(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("no conversion path"), "{err}");
        assert!(!output.exists());

        let _ = std::fs::remove_file(input);
        let _ = std::fs::remove_file(output);
    }

    #[test]
    fn convert_accepts_pypsa_csv_as_transmission_input() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let input = std::env::temp_dir().join(format!("powerio-convert-pypsa-{stamp}"));
        let output = std::env::temp_dir().join(format!("powerio-convert-pypsa-{stamp}.m"));
        let parsed = crate::module_io::load_balanced_module(data("case9.m"), None).unwrap();
        powerio_tx::__emit_pypsa_csv(&parsed, powerio_core::Destination::path(&input)).unwrap();

        run_convert(
            &input,
            FormatArg::Matpower,
            Some(&output),
            Some(FormatArg::PypsaCsv),
            0,
            GenCostCliOptions::preserve(),
        )
        .unwrap();
        let text = std::fs::read_to_string(&output).unwrap();
        assert!(text.contains("mpc.bus"));

        let _ = std::fs::remove_dir_all(input);
        let _ = std::fs::remove_file(output);
    }

    #[test]
    fn convert_writes_distribution_sidecars_next_to_output_file() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("powerio-convert-sidecar-{stamp}"));
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("geo.bmopf.json");
        let output = dir.join("geo.dss");

        let mut bus = powerio_dist::DistBus::new("sourcebus", vec!["1".to_owned(), "4".to_owned()]);
        bus.grounded = vec!["4".to_owned()];
        bus.location = Some(powerio_dist::DistLocation {
            x: -80.0,
            y: 35.0,
            kind: None,
        });
        let mut net = powerio_dist::MulticonductorNetwork::default();
        *net.geo_mut() = Some(powerio_dist::DistGeoMeta {
            space: powerio_dist::CoordinateSpace::Geographic { crs: None },
            kind: Some(powerio_dist::DistCoordsKind::Source),
        });
        *net.buses_mut() = vec![bus];
        net.sources_mut().push(powerio_dist::VoltageSource::new(
            "source",
            "sourcebus",
            vec!["1".to_owned(), "4".to_owned()],
            vec![7200.0, 0.0],
            vec![0.0, 0.0],
        ));
        let mut options = powerio_dist::EmitOptions::default();
        options.bmopf.sideload_coordinates = true;
        let module = powerio_core::PioModule::new(net);
        let bmopf = crate::module_io::emit_multiconductor_module_with_options(
            &module,
            powerio_dist::DistTargetFormat::BmopfJson,
            &options,
        )
        .unwrap();
        std::fs::write(&input, bmopf.text).unwrap();

        run_convert(
            &input,
            FormatArg::Dss,
            Some(&output),
            Some(FormatArg::BmopfJson),
            0,
            GenCostCliOptions::preserve(),
        )
        .unwrap();

        let dss = std::fs::read_to_string(&output).unwrap();
        let coords = std::fs::read_to_string(dir.join("buscoords.csv")).unwrap();
        assert!(dss.contains("Buscoords buscoords.csv"), "{dss}");
        assert!(coords.contains("sourcebus,-80,35"), "{coords}");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn convert_rejects_pwb_target_before_family_routing() {
        let err = run_convert(
            &data("dist/micro/xfmr_single_phase.dss"),
            FormatArg::Pwb,
            None,
            None,
            0,
            GenCostCliOptions::preserve(),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("cannot write PowerWorld .pwb"),
            "{err}"
        );
    }

    #[test]
    fn convert_rejects_ieee_cdf_target() {
        let err = run_convert(
            &data("case9.m"),
            FormatArg::IeeeCdf,
            None,
            None,
            0,
            GenCostCliOptions::preserve(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("cannot write IEEE CDF"), "{err}");
    }
    #[test]
    fn sidecar_paths_must_stay_under_the_output_directory() {
        for bad in [
            "../escape.csv",
            "a/../../escape.csv",
            "/etc/passwd",
            "",
            "..",
        ] {
            assert!(
                !super::is_relative_component_path(bad),
                "{bad:?} was accepted as a sidecar path"
            );
        }
        for good in ["buscoords.csv", "sub/dir/buscoords.csv", "a.b.c"] {
            assert!(
                super::is_relative_component_path(good),
                "{good:?} was rejected as a sidecar path"
            );
        }
    }

    /// `--formula` accepts only the stable 1.0 formula names.
    #[test]
    fn formula_tokens_are_the_stable_formula_names() {
        use clap::ValueEnum;
        use powerio_matrix::matrix::BranchSusceptanceFormula;
        for arg in BranchSusceptanceFormulaArg::value_variants() {
            let primary = arg.to_possible_value().unwrap().get_name().to_string();
            let parsed = BranchSusceptanceFormula::from_formula_name(&primary.replace('-', "_"))
                .unwrap_or_else(|| panic!("{primary} is not a formula name"));
            assert_eq!(parsed, BranchSusceptanceFormula::from(*arg));
        }
        for alias in ["series", "series-impedance", "matpower"] {
            assert!(
                BranchSusceptanceFormulaArg::from_str(alias, false).is_err(),
                "{alias} must not remain a 1.0 formula alias"
            );
        }
    }
}
