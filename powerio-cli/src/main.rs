//! The `powerio` binary: a clap CLI and a ratatui TUI over `powerio-matrix`.
//!
//! Subcommands: `batch` (matrix families), `gen` (synthetic cases), `verify`,
//! `dcopf` (DC OPF bundle), `sensitivities` (PTDF/LODF), `gridfm` (gridfm-datakit
//! Parquet), `module` (`.pio.json`), and `convert`. With no subcommand it launches the TUI. Run
//! `powerio --help` for the full surface.

use std::path::{Path, PathBuf};

use anyhow::Context;
use clap::{Parser, Subcommand, ValueEnum};
use powerio_matrix::io::gridfm::{GridfmOptions, numbered_snapshots, write_gridfm_batch};
use powerio_matrix::matrix::{BuildOptions, DcConvention, Scheme, sddm_check};
use powerio_matrix::pipeline::{MatrixKind, Pipeline, RhsKind};
use powerio_matrix::synth::{SynthSpec, Topology};
use powerio_matrix::{
    DcOpfAssemblyOptions, DcOpfBundleMetadata, DcOpfBundleOptions, Units, write_dcopf_bundle,
};
use powerio_matrix::{MissingGenCostPolicy, SensitivityOptions, SensitivitySolver, WriteOptions};
use serde_json::json;
mod cases;
mod compat;
mod tui;

use cases::infer_input_family;
use powerio_matrix::format::routing::SourceFormat as DetectedFormat;

#[derive(Parser, Debug)]
#[command(name = "powerio", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Launch the interactive TUI (default if no subcommand is given).
    Tui {
        /// Directory scanned recursively for case files (.m, .raw, .aux,
        /// .epc, .pwb, .json, .dss).
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
        /// Transmission case file (any readable format, or a stored `.pio.json`).
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
        /// Transmission case file (any readable format, or a stored `.pio.json`).
        input: PathBuf,
        /// Override the inferred input format.
        #[arg(long, value_enum)]
        from: Option<FormatArg>,
        /// Output directory; the bundle lands in `<output>/<case>_dcopf/`.
        #[arg(short, long)]
        output: PathBuf,
        /// DC susceptance convention. 0.8 defaulted to `paper-pure`, now
        /// spelled `reactance-only`; the default is `series-susceptance`, a
        /// different formula, so an unqualified run changes numbers against
        /// 0.8. The 0.9 spellings stay as aliases.
        #[arg(long, value_enum, default_value = "series-susceptance")]
        convention: DcConvArg,
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
        /// Transmission case file (any readable format, or a stored `.pio.json`).
        input: PathBuf,
        /// Override the inferred input format.
        #[arg(long, value_enum)]
        from: Option<FormatArg>,
        /// Output directory; writes `<case>_ptdf.mtx` and `<case>_lodf.mtx`.
        #[arg(short, long)]
        output: PathBuf,
        /// DC susceptance convention. 0.8 defaulted to `paper-pure`, now
        /// spelled `reactance-only`; the default is `series-susceptance`, a
        /// different formula, so an unqualified run changes numbers against
        /// 0.8. The 0.9 spellings stay as aliases.
        #[arg(long, value_enum, default_value = "series-susceptance")]
        convention: DcConvArg,
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
        input: PathBuf,
        /// Override the inferred input format.
        #[arg(long, value_enum)]
        from: Option<FormatArg>,
        /// With `--from gridfm`, which scenario to summarize.
        #[arg(long, default_value_t = 0)]
        scenario: i64,
    },
    /// Emit the stored `.pio.json` module for one input.
    Module {
        /// Input case file, PyPSA CSV folder, or gridfm dataset directory.
        input: PathBuf,
        /// Output file; `-` or omitted writes to stdout.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Override the inferred input format.
        #[arg(long, value_enum)]
        from: Option<FormatArg>,
        /// Export one scenario of a scenario set input (a gridfm dataset)
        /// as a static module; omitted, the module stores the whole set.
        #[arg(long)]
        scenario: Option<i64>,
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
        /// `.aux`, `.dss`) unless `--from` is given.
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

    fn write_options(self) -> anyhow::Result<WriteOptions> {
        write_options(
            self.missing_gen_cost,
            self.default_gen_cost,
            self.gen_cost_csv,
        )
    }
}

/// A case format, for `--to` / `--from`. `gridfm`, `goc3-json`, `opfdata-json`,
/// and `pwb` are read-only here: `convert --from gridfm` reads a Parquet dataset,
/// but writing a gridfm dataset is the dedicated `gridfm` subcommand, GO Challenge
/// 3 and OPFData JSON are source documents, and PowerWorld `.pwb` has no writer.
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
    /// Write PSS/E `.raw` at revision 34.
    #[value(name = "psse34")]
    Psse34,
    /// Write PSS/E `.raw` at revision 35.
    #[value(name = "psse35")]
    Psse35,
    #[value(name = "powerworld", alias = "aux")]
    PowerWorld,
    #[value(name = "pandapower-json", alias = "pandapower", alias = "pp")]
    PandapowerJson,
    #[value(name = "pypsa-csv", alias = "pypsa")]
    PypsaCsv,
    /// GE PSLF .epc case (read and write).
    #[value(name = "pslf", alias = "epc")]
    Pslf,
    /// ARPA-E GO Challenge 3 JSON input data (read only).
    #[value(name = "goc3-json", alias = "goc3", alias = "go3", alias = "c3")]
    Goc3Json,
    /// Surge native JSON network document.
    #[value(name = "surge-json", alias = "surge")]
    SurgeJson,
    /// JSON document from a DeepMind OPFData release (read only).
    #[value(
        name = "opfdata-json",
        alias = "opfdata",
        alias = "deepmind-opfdata-json",
        alias = "deepmind-opfdata",
        alias = "gridopt-json",
        alias = "gridopt"
    )]
    DeepMindOpfDataJson,
    /// Read a gridfm-datakit Parquet dataset directory (read only).
    #[value(name = "gridfm")]
    Gridfm,
    /// Read a PowerWorld .pwb binary case (read only).
    #[value(name = "pwb")]
    Pwb,
    /// OpenDSS `.dss` distribution case (read and write).
    #[value(name = "dss", alias = "opendss")]
    Dss,
    /// PowerModelsDistribution ENGINEERING JSON (read and write).
    #[value(name = "pmd-json", alias = "pmd", alias = "engineering")]
    PmdJson,
    /// IEEE BMOPF JSON distribution case (read and write).
    #[value(name = "bmopf-json", alias = "bmopf")]
    BmopfJson,
}

impl FormatArg {
    /// The writable transmission hub target: `None` for the distribution
    /// formats and for gridfm, which has no convert writer (the `gridfm`
    /// subcommand writes datasets).
    fn transmission(self) -> Option<powerio_matrix::TargetFormat> {
        use powerio_matrix::TargetFormat;
        Some(match self {
            FormatArg::Matpower => TargetFormat::Matpower,
            FormatArg::PowerModelsJson => TargetFormat::PowerModelsJson,
            FormatArg::EgretJson => TargetFormat::EgretJson,
            FormatArg::Psse => TargetFormat::Psse { rev: 33 },
            FormatArg::Psse34 => TargetFormat::Psse { rev: 34 },
            FormatArg::Psse35 => TargetFormat::Psse { rev: 35 },
            FormatArg::PowerWorld => TargetFormat::PowerWorld,
            FormatArg::PandapowerJson => TargetFormat::PandapowerJson,
            FormatArg::Pslf => TargetFormat::Pslf,
            FormatArg::Goc3Json => TargetFormat::Goc3Json,
            FormatArg::SurgeJson => TargetFormat::SurgeJson,
            FormatArg::DeepMindOpfDataJson => TargetFormat::DeepMindOpfDataJson,
            // PypsaCsv is a transmission format, but it writes a directory, not a
            // text target; `run_convert` handles it before reaching here. gridfm
            // is read only here, and Pwb is read only. The distribution formats
            // belong to `distribution()`. All return `None` from this method.
            FormatArg::PypsaCsv
            | FormatArg::Gridfm
            | FormatArg::Pwb
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
            | FormatArg::PowerWorld
            | FormatArg::PandapowerJson
            | FormatArg::PypsaCsv
            | FormatArg::Pslf
            | FormatArg::Goc3Json
            | FormatArg::SurgeJson
            | FormatArg::DeepMindOpfDataJson
            | FormatArg::Gridfm
            | FormatArg::Pwb => None,
        }
    }

    /// The canonical name the format dispatchers accept, for forcing a reader.
    fn name(self) -> &'static str {
        match self {
            FormatArg::Matpower => "matpower",
            FormatArg::PowerModelsJson => "powermodels-json",
            FormatArg::EgretJson => "egret-json",
            FormatArg::Psse => "psse",
            FormatArg::Psse34 => "psse34",
            FormatArg::Psse35 => "psse35",
            FormatArg::PowerWorld => "powerworld",
            FormatArg::PandapowerJson => "pandapower-json",
            FormatArg::PypsaCsv => "pypsa-csv",
            FormatArg::Pslf => "pslf",
            FormatArg::Goc3Json => "goc3-json",
            FormatArg::SurgeJson => "surge-json",
            FormatArg::DeepMindOpfDataJson => "opfdata-json",
            FormatArg::Gridfm => "gridfm",
            FormatArg::Pwb => "pwb",
            FormatArg::Dss => "dss",
            FormatArg::PmdJson => "pmd-json",
            FormatArg::BmopfJson => "bmopf-json",
        }
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
enum DcConvArg {
    /// The whole series impedance: `imag(inv(r + jx))`, with phase shift
    /// injections.
    #[value(
        name = "series-susceptance",
        alias = "series",
        alias = "series-impedance"
    )]
    SeriesSusceptance,
    /// `-1/(x tau)`, with phase shift injections, matching MATPOWER
    /// `makeBdc`.
    #[value(name = "tap-adjusted-reactance", alias = "matpower")]
    TapAdjustedReactance,
    /// `-1/x`, ignoring resistance, taps, and shifts: the textbook DC
    /// linearization a published result reproduces.
    #[value(name = "reactance-only")]
    ReactanceOnly,
}

impl From<DcConvArg> for DcConvention {
    fn from(value: DcConvArg) -> Self {
        match value {
            DcConvArg::SeriesSusceptance => Self::SeriesSusceptance,
            DcConvArg::TapAdjustedReactance => Self::TapAdjustedReactance,
            DcConvArg::ReactanceOnly => Self::ReactanceOnly,
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
    install_tracing();
    let cli = Cli::parse();
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
            convention,
            units,
            missing_gen_cost,
            default_gen_cost,
            gen_cost_csv,
        } => run_dcopf(
            &input,
            from,
            &output,
            convention.into(),
            units.into(),
            missing_gen_cost,
            default_gen_cost.as_deref(),
            gen_cost_csv.as_deref(),
        ),
        Command::Sensitivities {
            input,
            from,
            output,
            convention,
            solver,
            drop_tolerance,
        } => run_sensitivities(&input, from, &output, convention, solver, drop_tolerance),
        Command::Summary {
            input,
            from,
            scenario,
        } => run_summary(&input, from, scenario),
        Command::Corpus { action } => run_corpus(action),
        Command::Module {
            input,
            output,
            from,
            scenario,
        } => run_module(&input, output.as_deref(), from, scenario),
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
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            print_error_chain(&error);
            std::process::ExitCode::FAILURE
        }
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
    let mut previous: Option<String> = None;
    let mut prefix = "Error";
    for cause in error.chain() {
        let text = cause.to_string();
        let repeats_the_frame_above = previous
            .as_deref()
            .is_some_and(|above| above.ends_with(&text));
        if !repeats_the_frame_above {
            eprintln!("{prefix}: {text}");
            prefix = "Caused by";
        }
        previous = Some(text);
    }
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

fn install_tracing() {
    use tracing_subscriber::EnvFilter;
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
        anyhow::bail!(
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
        anyhow::bail!(
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
    convention: DcConvArg,
    solver: SensitivitySolverArg,
    drop_tolerance: f64,
) -> anyhow::Result<()> {
    let mpc = balanced_case(input, from).with_context(|| format!("parse {}", input.display()))?;
    std::fs::create_dir_all(output)?;
    let view = powerio_matrix::IndexedNetwork::new(&mpc);
    let options = SensitivityOptions {
        convention: convention.into(),
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
    let metadata = powerio_matrix::io::write_sensitivity_mtx_with_options(
        &view, &options, &ptdf_path, &lodf_path,
    )
    .with_context(|| format!("DC sensitivities for {}", input.display()))?;
    let meta = json!({
        "case": view.name(),
        "convention": options.convention,
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
                anyhow::bail!("--default-gen-cost is only valid with --missing-gen-cost quadratic");
            }
            Ok(MissingGenCostPolicy::Preserve)
        }
        MissingGenCostArg::Require => {
            if default_gen_cost.is_some() {
                anyhow::bail!("--default-gen-cost is only valid with --missing-gen-cost quadratic");
            }
            Ok(MissingGenCostPolicy::Require)
        }
        MissingGenCostArg::Zero => {
            if default_gen_cost.is_some() {
                anyhow::bail!("--default-gen-cost is only valid with --missing-gen-cost quadratic");
            }
            Ok(MissingGenCostPolicy::zero())
        }
        MissingGenCostArg::Quadratic => {
            let value = default_gen_cost
                .context("--missing-gen-cost quadratic requires --default-gen-cost C2,C1,C0")?;
            let [c2, c1, c0] = parse_cost_triple(value)?;
            Ok(MissingGenCostPolicy::quadratic(c2, c1, c0))
        }
    }
}

fn parse_cost_triple(value: &str) -> anyhow::Result<[f64; 3]> {
    let parts: Vec<_> = value.split(',').map(str::trim).collect();
    if parts.len() != 3 {
        anyhow::bail!("--default-gen-cost expects exactly three comma-separated values: C2,C1,C0");
    }
    let mut out = [0.0; 3];
    for (slot, part) in out.iter_mut().zip(parts) {
        *slot = part
            .parse::<f64>()
            .with_context(|| format!("parse --default-gen-cost value `{part}`"))?;
        if !slot.is_finite() {
            anyhow::bail!("--default-gen-cost values must be finite");
        }
    }
    Ok(out)
}

fn write_options(
    arg: MissingGenCostArg,
    default_gen_cost: Option<&str>,
    gen_cost_csv: Option<&Path>,
) -> anyhow::Result<WriteOptions> {
    let missing_gen_cost = missing_gen_cost_policy(arg, default_gen_cost)?;
    let gen_cost_patches = match gen_cost_csv {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("reading generator cost CSV {}", path.display()))?;
            powerio_matrix::parse_gen_cost_csv(&text)
                .with_context(|| format!("parsing generator cost CSV {}", path.display()))?
        }
        None => Vec::new(),
    };
    Ok(WriteOptions {
        missing_gen_cost,
        gen_cost_patches,
    })
}

#[allow(clippy::too_many_arguments)]
fn run_dcopf(
    input: &Path,
    from: Option<FormatArg>,
    output: &Path,
    convention: DcConvention,
    units: Units,
    missing_gen_cost: MissingGenCostArg,
    default_gen_cost: Option<&str>,
    gen_cost_csv: Option<&Path>,
) -> anyhow::Result<()> {
    let mpc = balanced_case(input, from).with_context(|| format!("parse {}", input.display()))?;
    let cost_opts = write_options(missing_gen_cost, default_gen_cost, gen_cost_csv)?;
    let mut policy_network = mpc.clone();
    let cost_report = policy_network
        .apply_gen_cost_policy(&cost_opts.gen_cost_patches, cost_opts.missing_gen_cost)?;
    let instance = powerio_prob::DcOpfInstance::from_network(policy_network)
        .with_context(|| format!("build DC OPF instance for {}", input.display()))?
        .with_approximation(convention);
    let mut assembly = DcOpfAssemblyOptions::default();
    assembly.units = units;
    let bundle_options = DcOpfBundleOptions {
        assembly,
        metadata: DcOpfBundleMetadata {
            cost_policy: cost_opts.missing_gen_cost,
            cost_report,
        },
    };
    let outputs = write_dcopf_bundle(&instance, output, &bundle_options)
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
        anyhow::bail!(
            "the `gridfm` subcommand writes a gridfm dataset from classical cases; \
             to read a gridfm dataset back, use `convert --from gridfm`"
        );
    }
    // Parse every input first so the snapshots can borrow the owned networks for
    // the batch. Each input becomes one scenario, stamped `base + position` by the
    // shared `numbered_snapshots` builder (same rule as the Python binding).
    let nets = inputs
        .iter()
        .map(|p| read_network(p, from))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let net_refs: Vec<_> = nets.iter().collect();
    let snapshots = numbered_snapshots(&net_refs, base_scenario)?;

    let cost_opts = write_options(missing_gen_cost, default_gen_cost, gen_cost_csv)?;
    let opts = GridfmOptions {
        missing_gen_cost: cost_opts.missing_gen_cost,
        gen_cost_patches: cost_opts.gen_cost_patches,
        ..Default::default()
    };
    let outputs = write_gridfm_batch(&snapshots, output, &opts)
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
    let matrix = powerio_matrix::build_kind(&view, kind, &opts)?;
    let stats = powerio_matrix::matrix_stats_for_kind(&matrix, &view, kind, &opts);
    let sddm = sddm_check(&matrix);
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

fn run_summary(input: &Path, from: Option<FormatArg>, scenario: i64) -> anyhow::Result<()> {
    let value = if from == Some(FormatArg::Gridfm)
        || (from.is_none() && looks_like_gridfm_dir(input))
    {
        let read = powerio::gridfm::read_gridfm_dataset(input, scenario)
            .with_context(|| format!("reading gridfm dataset {}", input.display()))?;
        transmission_summary_json(&read.network, &read.warnings)
    } else {
        match parse_family_case(input, from)? {
            FamilyCase::Distribution(net) => distribution_summary_json(&net.network, &net.warnings),
            FamilyCase::Transmission(parsed) => {
                transmission_summary_json(&parsed.network, &parsed.rendered_diagnostics())
            }
        }
    };
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn run_module(
    input: &Path,
    output: Option<&Path>,
    from: Option<FormatArg>,
    scenario: Option<i64>,
) -> anyhow::Result<()> {
    let (text, parse_errors) = module_text(input, from, scenario)?;
    match output {
        Some(p) if p.as_os_str() != "-" => {
            commit_output_file(p, text.clone().into_bytes())
                .with_context(|| format!("writing {}", p.display()))?;
            eprintln!("wrote {}", p.display());
        }
        _ => print!("{text}"),
    }
    // The module is written either way — it is the record of what the reader
    // saw — but a refused include is an `Error` finding in its own document,
    // so the exit code has to say so, as `convert` does.
    fail_on_parse_errors(&parse_errors)
}

/// The stored module JSON and the `Error`-or-worse findings it carries.
fn module_text(
    input: &Path,
    from: Option<FormatArg>,
    scenario: Option<i64>,
) -> anyhow::Result<(String, Vec<String>)> {
    let mut source = powerio_core::Source::open(input)
        .with_context(|| format!("opening {}", input.display()))?;
    if let Some(format) = from {
        source = source
            .with_format(powerio_core::FormatId::new(format.name()).context("declared format")?);
    }
    let module = powerio::parse(source).with_context(|| format!("parsing {}", input.display()))?;
    let module = match scenario {
        None => module,
        Some(id) => powerio::select::export_state(
            module.value(),
            powerio::select::StateSelector::Scenario(&id.to_string()),
        )
        .with_context(|| format!("exporting scenario {id}"))?,
    };
    let errors = module_error_lines(&module);
    let text = powerio::stored::write_module(&module)
        .context("serializing the stored .pio.json module")?;
    powerio::stored::read_module(&text).context("validating .pio.json module readback")?;
    Ok((text, errors))
}

/// [`parse_error_lines`] for a compiled module's own findings.
fn module_error_lines(module: &powerio_core::PioModule<powerio::PioValue>) -> Vec<String> {
    module
        .diagnostics()
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
            "connected_components": view.n_connected_components(),
            "is_radial": view.is_radial(),
            "reference_buses": view.reference_bus_indices(),
            "connectivity_report": view.connectivity_report(),
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

fn run_convert(
    input: &std::path::Path,
    to: FormatArg,
    output: Option<&std::path::Path>,
    from: Option<FormatArg>,
    scenario: i64,
    gen_cost_options: GenCostCliOptions<'_>,
) -> anyhow::Result<()> {
    // gridfm has no convert writer; the dataset writer is the `gridfm`
    // subcommand.
    if matches!(to, FormatArg::Gridfm) {
        anyhow::bail!("`convert` cannot write a gridfm dataset; use the `gridfm` subcommand");
    }
    if matches!(to, FormatArg::Pwb) {
        anyhow::bail!(
            "`convert` cannot write PowerWorld .pwb binary cases; use `--to powerworld` for AUX text"
        );
    }
    // goc3-json is read only, but the library still echoes a goc3 source to a
    // goc3 target byte for byte; every other case gets its precise
    // WriteUnsupported error, so no CLI-level bail here.
    // PyPSA CSV is a transmission format that writes a directory, not a text
    // target, so it takes the folder path and returns early.
    if to == FormatArg::PypsaCsv {
        return convert_to_pypsa_folder(input, output, from, scenario, gen_cost_options);
    }
    if from.is_none() && cases::stored_json(input)?.is_some() {
        return convert_stored(input, to, output, &gen_cost_options);
    }
    // A `.json` with no --from is read and DOM-classified once here; the
    // family check below uses the verdict and the typed parse reuses the text.
    let classified = if from.is_none() {
        cases::classified_json(input)?
    } else {
        None
    };
    // The two families share no conversion path; say so directly instead of
    // letting the wrong family's reader produce a confusing format error. The
    // input family comes from --from (gridfm reads into the transmission
    // model), from a clear extension, or from the shared JSON classifier.
    let input_is_dist = if let Some(f) = from {
        Some(f.distribution().is_some())
    } else if let Some(case) = &classified {
        Some(case.is_distribution())
    } else {
        infer_input_family(input)?
    };
    if input_is_dist.is_some_and(|dist| dist != to.transmission().is_none()) {
        anyhow::bail!(
            "no conversion path between the transmission and distribution format families \
             ({} to `{}`)",
            from.map_or_else(
                || format!("`{}` input", input.display()),
                |f| format!("`{}`", f.name())
            ),
            to.name()
        );
    }
    let (text, sidecars, warnings, parse_errors) = if let Some(target) = to.transmission() {
        let options = gen_cost_options.write_options()?;
        // gridfm reads a Parquet dataset directory (the parquet-free
        // `parse_file` can't), so it routes through powerio-matrix's reader,
        // surfacing its fidelity notes.
        let conv = if matches!(from, Some(FormatArg::Gridfm)) {
            let read = powerio::gridfm::read_gridfm_dataset(input, scenario)
                .with_context(|| format!("reading gridfm dataset {}", input.display()))?;
            for w in &read.warnings {
                eprintln!("{w}");
            }
            powerio_matrix::write_as_with_options(
                &powerio_core::PioModule::new(read.network),
                target,
                &options,
            )
            .with_context(|| format!("serializing to {target}"))?
        } else if let Some(case) = &classified {
            // The classified text feeds the one-call conversion, so a same
            // format target still echoes the source bytes exactly. Parse it
            // once more first only to give a failure there its own context;
            // `convert_str_with_options` below reparses either way, and a
            // parse failure it reports would otherwise land under
            // "serializing to {target}".
            compat::parse_str_with_name(
                &case.text,
                case.format.name(),
                input.file_stem().and_then(|s| s.to_str()),
            )
            .with_context(|| format!("parsing {}", input.display()))?;
            powerio::convert_str_with_options(&case.text, target, case.format.name(), &options)
                .with_context(|| format!("serializing to {target}"))?
        } else {
            reject_nontransmission_from(from)?;
            // Same reasoning as the classified branch above: a cheap extra
            // parse so a parse failure is not blamed on serialization.
            compat::parse_module(input, from.map(FormatArg::name))
                .with_context(|| format!("parsing {}", input.display()))?;
            powerio::convert_file_with_options(input, target, from.map(FormatArg::name), &options)
                .with_context(|| format!("serializing to {target}"))?
        };
        let rendered = conv.rendered_diagnostics();
        (conv.text, Vec::new(), rendered, Vec::new())
    } else {
        let net = if let Some(case) = &classified {
            match parse_classified_case(case, input)? {
                FamilyCase::Distribution(net) => *net,
                FamilyCase::Transmission(_) => {
                    unreachable!("the family check routed a transmission input here")
                }
            }
        } else {
            compat::dist_parse_file(input, from.map(FormatArg::name))
                .with_context(|| format!("reading {}", input.display()))?
        };
        for w in &net.warnings {
            eprintln!("{w}");
        }
        let target = to
            .distribution()
            .expect("the family check routed a transmission target here");
        let conv = net.to_format(target);
        // Both halves, as `convert.rs`'s own glue does: a writer emits its own
        // structured findings, and an `Error` from either side has to reach
        // the exit code.
        let mut diagnostics = net.diagnostics.clone();
        diagnostics.extend(conv.diagnostics.iter().cloned());
        let rendered = conv.rendered_diagnostics();
        (
            conv.text,
            conv.sidecars,
            rendered,
            parse_error_lines(&diagnostics),
        )
    };
    for w in &warnings {
        eprintln!("{w}");
    }
    write_conversion_output(&text, &sidecars, output)?;
    fail_on_parse_errors(&parse_errors)
}

/// The `Error`-or-worse parse findings, formatted for stderr.
fn parse_error_lines(diagnostics: &[powerio_dist::Diagnostic]) -> Vec<String> {
    diagnostics
        .iter()
        .filter(|d| d.severity() >= powerio_dist::DiagnosticSeverity::Error)
        .map(|d| format!("{}: {}", d.code(), d.message()))
        .collect()
}

/// Exit nonzero after the output is written: the file exists for
/// inspection, but the parse was incomplete and scripts must not treat the
/// run as clean (#275).
fn fail_on_parse_errors(parse_errors: &[String]) -> anyhow::Result<()> {
    if parse_errors.is_empty() {
        return Ok(());
    }
    for e in parse_errors {
        eprintln!("error: {e}");
    }
    anyhow::bail!(
        "{} parse error(s); the output is incomplete (see the error lines above)",
        parse_errors.len()
    )
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
        .__commit_artifacts(false, vec![artifact], Vec::new())
        .map(|_| ())
        .map_err(|error| anyhow::anyhow!("{error}"))
}

/// Write conversion `text` to `output` (stdout on `-` or `None`), placing any
/// `sidecars` next to it. Sidecars cannot follow text to stdout; they are
/// reported instead.
fn write_conversion_output(
    text: &str,
    sidecars: &[powerio_dist::ConversionSidecar],
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
                eprintln!("wrote {}", p.display());
                let base = p.parent().unwrap_or_else(|| std::path::Path::new("."));
                for sidecar in sidecars {
                    // A sidecar path names a file the primary output refers
                    // to, so it must stay under the output directory. Today's
                    // writers emit a fixed name, but the field is a plain
                    // `String` on a public struct, and joining an absolute or
                    // `..` path here would write anywhere the process can
                    // reach.
                    if !is_relative_component_path(&sidecar.path) {
                        anyhow::bail!(
                            "sidecar `{}` is not a relative path under the output directory",
                            sidecar.path
                        );
                    }
                    let path = base.join(&sidecar.path);
                    commit_output_file(&path, sidecar.text.clone().into_bytes())
                        .with_context(|| format!("writing {}", path.display()))?;
                    committed.push(path.clone());
                    eprintln!("wrote {}", path.display());
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
                eprintln!("{}", sidecar.dropped_warning("output is stdout"));
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
        let display = powerio_matrix::parse_display_file(input, None)
            .with_context(|| format!("reading {}", input.display()))?;
        let powerio_matrix::DisplayData::PowerWorld(display) = display else {
            anyhow::bail!("{} did not parse as a .pwd display", input.display());
        };
        let layer = powerio_matrix::geo::geo_layer_from_pwd(&display);
        if layer.features.is_empty() {
            anyhow::bail!("{} carries no substation symbols", input.display());
        }
        return write_conversion_output(&layer.to_geojson(), &[], output);
    }
    let layer = match parse_family_case(input, from)? {
        FamilyCase::Distribution(net) => {
            for w in &net.warnings {
                eprintln!("{w}");
            }
            powerio::dist_geo::dist_geo_layer(&net.network)
        }
        FamilyCase::Transmission(parsed) => {
            for w in &parsed.rendered_diagnostics() {
                eprintln!("{w}");
            }
            parsed.network.geo_layer()
        }
    };
    if layer.features.is_empty() {
        anyhow::bail!("{} carries no coordinates to extract", input.display());
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
    let bytes = std::fs::read(layer_path)
        .with_context(|| format!("reading layer {}", layer_path.display()))?;
    let parsed = powerio_matrix::geo::GeoLayer::parse_bytes(
        &bytes,
        layer_path.file_name().and_then(|n| n.to_str()),
    )
    .with_context(|| format!("parsing layer {}", layer_path.display()))?;
    for w in &parsed.warnings {
        eprintln!("{w}");
    }
    let (text, sidecars, warnings) = match parse_family_case(input, from)? {
        FamilyCase::Distribution(net) => {
            for w in &net.warnings {
                eprintln!("{w}");
            }
            let mut network = net.network;
            report_geo_apply(&powerio::dist_geo::apply_dist_geo_layer(
                &mut network,
                &parsed.layer,
            ));
            let target = match to {
                Some(f) => f.distribution().ok_or_else(|| {
                    anyhow::anyhow!(
                        "`{}` is not a distribution text target; a distribution case writes \
                         back to dss, pmd-json, or bmopf-json",
                        f.name()
                    )
                })?,
                None => network
                    .source_format()
                    .map(|f| f.name().parse())
                    .transpose()?
                    .ok_or_else(|| {
                        anyhow::anyhow!("the input carries no source format; pass --to")
                    })?,
            };
            // The layer edited the typed value; write from it, never the
            // retained source echo.
            let conv = powerio_dist::write_network(&network, target);
            let rendered = conv.rendered_diagnostics();
            (conv.text, conv.sidecars, rendered)
        }
        FamilyCase::Transmission(case) => {
            for w in &case.rendered_diagnostics() {
                eprintln!("{w}");
            }
            let mut net = case.network;
            report_geo_apply(&net.apply_geo_layer(&parsed.layer));
            let target = match to {
                Some(f) => f.transmission().ok_or_else(|| {
                    anyhow::anyhow!(
                        "`{}` is not a transmission text target here; apply writes a single \
                         case file (use `convert` for pypsa-csv and gridfm outputs)",
                        f.name()
                    )
                })?,
                None => {
                    powerio_matrix::target_format_from_name(&format!("{:?}", net.source_format()))
                        .ok_or_else(|| {
                        anyhow::anyhow!(
                            "`{:?}` has no write target; pass --to to choose one",
                            net.source_format()
                        )
                    })?
                }
            };
            let conv = net
                .to_format(target)
                .with_context(|| format!("serializing to {target}"))?;
            let rendered = conv.rendered_diagnostics();
            (conv.text, Vec::new(), rendered)
        }
    };
    for w in &warnings {
        eprintln!("{w}");
    }
    write_conversion_output(&text, &sidecars, output)
}

fn report_geo_apply(report: &powerio_matrix::geo::GeoApplyReport) {
    eprintln!(
        "applied: {} bus point(s), {} branch route(s), {} unmatched feature(s)",
        report.matched_buses, report.matched_branches, report.unmatched_features
    );
    eprintln!(
        "unplaced: {} bus(es) with no location, {} branch(es) with no route",
        report.unlocated_buses, report.unlocated_branches
    );
    // A point sidecar or a substation join states no polylines, so every branch
    // lands in that second count on a run that placed everything it could. Say
    // what it means, rather than leave a successful apply reading as a partial
    // one. `GeoApplyReport::require_located` is the strict gate for a caller
    // that does want a route on every branch.
    if report.unlocated_branches > 0 {
        eprintln!(
            "note: a branch with no route renders from its bus endpoints; only a source \
             stating intermediate geometry gives it one"
        );
    }
    for note in &report.notes {
        eprintln!("note: {note}");
    }
}

fn run_geo_convert(
    input: &std::path::Path,
    output: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    let bytes = std::fs::read(input).with_context(|| format!("reading {}", input.display()))?;
    let parsed = powerio_matrix::geo::GeoLayer::parse_bytes(
        &bytes,
        input.file_name().and_then(|n| n.to_str()),
    )
    .with_context(|| format!("parsing {}", input.display()))?;
    for w in &parsed.warnings {
        eprintln!("{w}");
    }
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
        anyhow::bail!("`--to pypsa-csv` requires `-o <output-dir>`");
    };
    if out_dir.as_os_str() == "-" {
        anyhow::bail!("`--to pypsa-csv` writes a directory and cannot write to stdout");
    }
    let net = if from == Some(FormatArg::Gridfm) {
        let read = powerio::gridfm::read_gridfm_dataset(input, scenario)
            .with_context(|| format!("reading gridfm dataset {}", input.display()))?;
        for w in &read.warnings {
            eprintln!("{w}");
        }
        read.network
    } else {
        read_network(input, from)?
    };
    // The same directory writer the C surface calls, so the cost policy and its
    // findings are stated once for both.
    let options = gen_cost_options.write_options()?;
    let diagnostics = powerio_matrix::write_dir_with_options(&net, "pypsa-csv", out_dir, &options)
        .with_context(|| format!("writing PyPSA CSV folder {}", out_dir.display()))?;
    for w in powerio_matrix::diagnostics::render_diagnostics(&diagnostics) {
        eprintln!("{w}");
    }
    eprintln!("wrote {}", out_dir.display());
    Ok(())
}

/// A single-file case input parsed to its own family model.
enum FamilyCase {
    Transmission(Box<compat::ParsedCase>),
    Distribution(Box<compat::ParsedDist>),
}

/// Parse a single-file case to whichever family model it belongs to. With no
/// `--from`, a `.json` is read and DOM-classified once and the same text feeds
/// the typed parser — the read-once rule #260 established for `batch` and the
/// TUI, extended here to the single-file routes. Warnings stay on the returned
/// value: the callers differ in where they surface them (summary JSON, module
/// diagnostics, stderr).
/// One balanced network from any single case input (a stored `.pio.json`
/// included), for the matrix commands. A distribution input is refused with
/// the family named.
fn balanced_case(
    input: &Path,
    from: Option<FormatArg>,
) -> anyhow::Result<powerio_matrix::BalancedNetwork> {
    match parse_family_case(input, from)? {
        FamilyCase::Transmission(parsed) => Ok(parsed.network),
        FamilyCase::Distribution(_) => anyhow::bail!(
            "{} is a distribution case; this command needs a transmission network",
            input.display()
        ),
    }
}

/// Convert a stored `.pio.json` module's static network to `to`. The write
/// is canonical: the stored document is not a case format, so there is no
/// same format echo to preserve.
fn convert_stored(
    input: &Path,
    to: FormatArg,
    output: Option<&Path>,
    gen_cost_options: &GenCostCliOptions,
) -> anyhow::Result<()> {
    let case = stored_family_case(input)?;
    match (case, to.transmission(), to.distribution()) {
        (FamilyCase::Transmission(parsed), Some(target), _) => {
            let options = gen_cost_options.write_options()?;
            let conv = powerio_matrix::write_as_with_options(
                &powerio_core::PioModule::new(parsed.network),
                target,
                &options,
            )
            .with_context(|| format!("serializing to {target}"))?;
            for w in conv.rendered_diagnostics() {
                eprintln!("{w}");
            }
            write_conversion_output(&conv.text, &[], output)?;
            Ok(())
        }
        (FamilyCase::Distribution(parsed), _, Some(target)) => {
            for w in &parsed.warnings {
                eprintln!("{w}");
            }
            let conv = parsed.to_format(target);
            let mut diagnostics = parsed.diagnostics.clone();
            diagnostics.extend(conv.diagnostics.iter().cloned());
            for w in conv.rendered_diagnostics() {
                eprintln!("{w}");
            }
            write_conversion_output(&conv.text, &conv.sidecars, output)?;
            fail_on_parse_errors(&parse_error_lines(&diagnostics))
        }
        (FamilyCase::Transmission(_), None, _) | (FamilyCase::Distribution(_), _, None) => {
            anyhow::bail!(
                "no conversion path between the transmission and distribution format \
                 families (`{}` input to `{}`)",
                input.display(),
                to.name()
            )
        }
    }
}

/// Load a stored `.pio.json` module and adapt its static value to the CLI's
/// family case. A module storing anything but a static network names the
/// export step instead of guessing a projection.
fn stored_family_case(input: &Path) -> anyhow::Result<FamilyCase> {
    let source = powerio_core::Source::open(input)
        .with_context(|| format!("reading {}", input.display()))?;
    let module = powerio::parse(source).with_context(|| format!("reading {}", input.display()))?;
    match module.value().kind() {
        powerio::PioValueKind::BalancedNetwork => {
            let module: powerio_core::PioModule<powerio_matrix::BalancedNetwork> =
                powerio::try_into_typed(module).expect("the kind was checked");
            Ok(FamilyCase::Transmission(Box::new(
                compat::module_to_parsed(module),
            )))
        }
        powerio::PioValueKind::MulticonductorNetwork => {
            let module: powerio_core::PioModule<powerio_dist::MulticonductorNetwork> =
                powerio::try_into_typed(module).expect("the kind was checked");
            Ok(FamilyCase::Distribution(Box::new(
                compat::dist_module_to_parsed(module),
            )))
        }
        other => anyhow::bail!(
            "{} stores a {} value; export one static item first (`powerio module` with \
             --scenario writes one from a scenario set, and the bindings' export_state \
             selects by time position or scenario)",
            input.display(),
            other.as_str()
        ),
    }
}

fn parse_family_case(input: &Path, from: Option<FormatArg>) -> anyhow::Result<FamilyCase> {
    if let Some(f) = from {
        if f == FormatArg::Gridfm {
            anyhow::bail!(
                "gridfm datasets are read by `convert --from gridfm` or the `gridfm` \
                 subcommand, not this command"
            );
        }
        return if f.distribution().is_some() {
            let net = compat::dist_parse_file(input, Some(f.name()))
                .with_context(|| format!("reading {}", input.display()))?;
            Ok(FamilyCase::Distribution(Box::new(net)))
        } else {
            let parsed = compat::parse_file(input, Some(f.name()))
                .with_context(|| format!("reading {}", input.display()))?;
            Ok(FamilyCase::Transmission(Box::new(parsed)))
        };
    }
    if cases::stored_json(input)?.is_some() {
        return stored_family_case(input);
    }
    if let Some(case) = cases::classified_json(input)? {
        return parse_classified_case(&case, input);
    }
    if cases::looks_like_distribution_input(input)? {
        let net = compat::dist_parse_file(input, None)
            .with_context(|| format!("reading {}", input.display()))?;
        Ok(FamilyCase::Distribution(Box::new(net)))
    } else {
        let parsed = compat::parse_file(input, None)
            .with_context(|| format!("reading {}", input.display()))?;
        Ok(FamilyCase::Transmission(Box::new(parsed)))
    }
}

/// Parse already-classified `.json` text through its family's string parser,
/// keeping the file stem as the name hint the path-based parsers would use.
fn parse_classified_case(case: &cases::ClassifiedCase, input: &Path) -> anyhow::Result<FamilyCase> {
    match case.format {
        DetectedFormat::Distribution(_) => {
            // The shared classifier routes the family; the dist crate owns
            // which documents are cases at all, and its rule is stricter
            // (a BMOPF document needs a `bus` table). Apply it to the text
            // already read, as `powerio_dist::parse_file` does.
            let format = powerio_dist::classify_distribution_json(&case.text)
                .with_context(|| format!("reading {}", input.display()))?;
            let net = compat::dist_parse_str(&case.text, format.name())
                .with_context(|| format!("reading {}", input.display()))?;
            Ok(FamilyCase::Distribution(Box::new(net)))
        }
        DetectedFormat::Transmission(format) => {
            let stem = input.file_stem().and_then(|s| s.to_str());
            let parsed = compat::parse_str_with_name(&case.text, format.name(), stem)
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
            anyhow::bail!(
                "gridfm datasets are read by `convert --from gridfm` or the `gridfm` \
                 subcommand, not this command"
            );
        }
        if f.distribution().is_some() {
            anyhow::bail!(
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
    let parsed = compat::parse_file(input, from.map(FormatArg::name))
        .with_context(|| format!("reading {}", input.display()))?;
    for w in &parsed.rendered_diagnostics() {
        eprintln!("{w}");
    }
    Ok(parsed.network)
}

#[cfg(test)]
mod tests {
    use super::cases::looks_like_distribution_input;
    use super::{
        Cli, Command, DcConvArg, FamilyCase, FormatArg, GenCostCliOptions,
        distribution_summary_json, infer_input_family, module_text, parse_family_case, run_convert,
        run_module, transmission_summary_json,
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

    #[test]
    fn summary_json_matches_canonical_transmission_shape() {
        let parsed = crate::compat::parse_file(data("case9.m"), None).unwrap();
        let value = transmission_summary_json(&parsed.network, &parsed.rendered_diagnostics());
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

        let parsed = crate::compat::parse_file(data("opfdataset/example_0.json"), None).unwrap();
        assert_eq!(
            parsed.network.source_format(),
            powerio_matrix::SourceFormat::DeepMindOpfDataJson
        );
    }

    #[test]
    fn summary_json_matches_canonical_distribution_shape() {
        let net = crate::compat::dist_parse_file(&data("dist/micro/xfmr_single_phase.dss"), None)
            .unwrap();
        let value = distribution_summary_json(&net.network, &net.warnings);
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
                assert_eq!(parsed.network.buses().len(), 9);
                assert_eq!(parsed.network.name(), "myegret");
            }
            FamilyCase::Distribution(_) => panic!("egret JSON classified as distribution"),
        }

        let dist = dir.join("feeder.json");
        std::fs::write(
            &dist,
            r#"{"bus":{"a":{"terminal_names":["1"]}},"meta":{"version":"0.1.0"}}"#,
        )
        .unwrap();
        match parse_family_case(&dist, None).unwrap() {
            FamilyCase::Distribution(net) => assert_eq!(net.network.buses().len(), 1),
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
    fn stored_module_reads_back_as_a_family_case() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("powerio-cli-stored-{stamp}.pio.json"));
        let (text, _) = module_text(&data("case9.m"), None, None).unwrap();
        std::fs::write(&path, text).unwrap();

        match super::stored_family_case(&path).unwrap() {
            super::FamilyCase::Transmission(parsed) => {
                assert_eq!(parsed.network.buses().len(), 9);
            }
            super::FamilyCase::Distribution(_) => panic!("case9 is transmission"),
        }
        let net = super::balanced_case(&path, None).unwrap();
        assert_eq!(net.buses().len(), 9);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn module_command_parses() {
        let cli = Cli::try_parse_from(["powerio", "module", "case9.m"]).unwrap();
        match cli.command.unwrap() {
            Command::Module { input, .. } => assert_eq!(input, Path::new("case9.m")),
            other => panic!("expected module command, got {other:?}"),
        }
    }

    #[test]
    fn package_text_matches_module_shape_and_provenance() {
        let input = data("case9.m");
        let (text, _) = module_text(&input, None, None).unwrap();
        let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(doc["schema"], "powerio.module");
        assert_eq!(doc["version"], 1);
        assert_eq!(doc["value"]["kind"], "balanced_network");
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
        let module = powerio::stored::read_module(&text).unwrap();
        assert_eq!(
            module.value().kind(),
            powerio::PioValueKind::BalancedNetwork
        );
    }

    #[test]
    fn package_command_writes_output_file() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let output = std::env::temp_dir().join(format!("powerio-package-{stamp}.pio.json"));

        run_module(&data("case9.m"), Some(&output), None, None).unwrap();
        let text = std::fs::read_to_string(&output).unwrap();
        let module = powerio::stored::read_module(&text).unwrap();
        assert_eq!(
            module.value().kind(),
            powerio::PioValueKind::BalancedNetwork
        );

        let _ = std::fs::remove_file(output);
    }

    #[test]
    fn package_helper_returns_stdout_text() {
        let (text, _) = module_text(&data("case9.m"), None, None).unwrap();
        let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(doc["producer"]["name"], "powerio");
        assert_eq!(doc["value"]["data"]["buses"].as_array().unwrap().len(), 9);
    }

    #[test]
    fn package_text_round_trips_through_the_stored_reader() {
        let (text, _) = module_text(&data("case9.m"), None, None).unwrap();
        let module = powerio::stored::read_module(&text).unwrap();
        let again = powerio::stored::write_module(&module).unwrap();
        assert_eq!(text, again, "the stored document is write stable");
    }

    #[test]
    fn package_distribution_fixture_stores_the_multiconductor_value() {
        let input = data("dist/micro/xfmr_single_phase.dss");
        let (text, _) = module_text(&input, None, None).unwrap();
        let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(doc["value"]["kind"], "multiconductor_network");
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
    fn package_carries_a_nonfinite_payload_as_string_spellings() {
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

        run_module(&input, Some(&output), None, None).unwrap();
        let text = std::fs::read_to_string(&output).unwrap();
        assert!(text.contains("\"angmin\": \"NaN\""), "{text}");
        assert!(text.contains("\"angmax\": \"Infinity\""), "{text}");
        let module = powerio::stored::read_module(&text).unwrap();
        let module: powerio_core::PioModule<powerio::BalancedNetwork> =
            powerio::try_into_typed(module).unwrap();
        assert!(module.value().branches()[0].angmin.is_nan());
        assert_eq!(module.value().branches()[0].angmax, f64::INFINITY);

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
        let parsed = crate::compat::parse_file(data("case9.m"), None).unwrap();
        let conv = powerio_matrix::write_network(
            &parsed.network,
            powerio_matrix::TargetFormat::PowerModelsJson,
        )
        .unwrap();
        std::fs::write(&input, conv.text).unwrap();

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
        let parsed = crate::compat::parse_file(data("case9.m"), None).unwrap();
        powerio_matrix::write_pypsa_csv_folder(&parsed.network, &input).unwrap();

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
        let mut options = powerio_dist::BmopfWriteOptions::default();
        options.sideload_coordinates = true;
        let bmopf = powerio_dist::write_bmopf_json_with_options(&net, &options);
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

    /// The primary --convention tokens are the 1.0 formula names; the 0.9
    /// spellings survive only as aliases.
    #[test]
    fn convention_tokens_are_the_formula_names() {
        use clap::ValueEnum;
        use powerio_matrix::matrix::DcConvention;
        for arg in DcConvArg::value_variants() {
            let primary = arg.to_possible_value().unwrap().get_name().to_string();
            let parsed = DcConvention::from_formula_name(&primary.replace('-', "_"))
                .unwrap_or_else(|| panic!("{primary} is not a formula name"));
            assert_eq!(parsed, DcConvention::from(*arg));
        }
        for alias in ["series", "series-impedance", "matpower"] {
            assert!(
                DcConvArg::from_str(alias, false).is_ok(),
                "{alias} must stay accepted as an alias"
            );
        }
    }
}
