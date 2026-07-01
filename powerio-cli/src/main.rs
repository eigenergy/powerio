//! The `powerio` binary: a clap CLI and a ratatui TUI over `powerio-matrix`.
//!
//! Subcommands: `batch` (matrix families), `gen` (synthetic cases), `verify`,
//! `dcopf` (DC OPF bundle), `sensitivities` (PTDF/LODF), `gridfm` (gridfm-datakit
//! Parquet), `package` (`.pio.json`), and `convert`. With no subcommand it launches the TUI. Run
//! `powerio --help` for the full surface.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use clap::{Parser, Subcommand, ValueEnum};
use powerio_matrix::format::routing::{Detection, SourceFormat as DetectedFormat};
use powerio_matrix::io::gridfm::{GridfmOptions, numbered_snapshots, write_gridfm_batch};
use powerio_matrix::matrix::{BuildOptions, DcConvention, Scheme, Units, sddm_check};
use powerio_matrix::opf_pipeline::{DcOpfOptions, write_dcopf_bundle};
use powerio_matrix::pipeline::{MatrixKind, Pipeline, RhsKind};
use powerio_matrix::synth::{SynthSpec, Topology};
use powerio_matrix::{MissingGenCostPolicy, WriteOptions};
use powerio_pkg::{
    CompilerPackage, DiagnosticSeverity, DiagnosticStage, Origin, SourceDescriptor,
    StructuredDiagnostic, ValidationSummary,
};
use serde_json::json;
mod tui;

const SUMMARY_SCHEMA_VERSION: &str = "0.1";

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
        /// Directory holding `.m` MATPOWER cases.
        #[arg(short, long)]
        data_dir: Option<PathBuf>,
        /// Default output directory for batch exports.
        #[arg(short, long)]
        out_dir: Option<PathBuf>,
    },
    /// Batch export matrix datasets for every `.m` case in a directory.
    Batch {
        /// Input directory (or single `.m` file).
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
        /// MATPOWER `.m` file.
        input: PathBuf,
        #[arg(long, value_enum, default_value = "bprime")]
        kind: MatrixKindArg,
        #[arg(long, value_enum, default_value = "bx")]
        scheme: SchemeArg,
    },
    /// Emit the static DC OPF matrix/vector bundle for one case.
    #[command(name = "dcopf", visible_alias = "dc-opf")]
    DcOpf {
        /// MATPOWER `.m` file.
        input: PathBuf,
        /// Output directory; the bundle lands in `<output>/<case>_dcopf/`.
        #[arg(short, long)]
        output: PathBuf,
        /// DC susceptance convention.
        #[arg(long, value_enum, default_value = "paper-pure")]
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
        /// MATPOWER `.m` file.
        input: PathBuf,
        /// Output directory; writes `<case>_ptdf.mtx` and `<case>_lodf.mtx`.
        #[arg(short, long)]
        output: PathBuf,
        /// DC susceptance convention.
        #[arg(long, value_enum, default_value = "paper-pure")]
        convention: DcConvArg,
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
    /// Emit the `.pio.json` compiler package for one input.
    #[command(visible_alias = "pkg")]
    Package {
        /// Input case file, PyPSA CSV folder, or gridfm dataset directory.
        input: PathBuf,
        /// Output file; `-` or omitted writes to stdout.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Override the inferred input format.
        #[arg(long, value_enum)]
        from: Option<FormatArg>,
        /// With `--from gridfm`, which scenario to package.
        #[arg(long, default_value_t = 0)]
        scenario: i64,
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

/// A case interchange format, for `--to` / `--from`. `gridfm`, `goc3-json`, and `pwb` are
/// read-only here: `convert --from gridfm` reads a Parquet dataset, but writing
/// a gridfm dataset is the dedicated `gridfm` subcommand, GO Challenge 3 JSON is a
/// unit commitment input document, and PowerWorld `.pwb` has no writer.
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
    /// The canonical lossless snapshot (`Network` as validated JSON).
    #[value(name = "powerio-json", alias = "powerio", alias = "json")]
    PowerioJson,
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
            FormatArg::PowerioJson => TargetFormat::PowerioJson,
            FormatArg::Pslf => TargetFormat::Pslf,
            FormatArg::Goc3Json => TargetFormat::Goc3Json,
            FormatArg::SurgeJson => TargetFormat::SurgeJson,
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
            | FormatArg::PowerioJson
            | FormatArg::PypsaCsv
            | FormatArg::Pslf
            | FormatArg::Goc3Json
            | FormatArg::SurgeJson
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
            FormatArg::PowerioJson => "powerio-json",
            FormatArg::PypsaCsv => "pypsa-csv",
            FormatArg::Pslf => "pslf",
            FormatArg::Goc3Json => "goc3-json",
            FormatArg::SurgeJson => "surge-json",
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
    PaperPure,
    Matpower,
}

impl From<DcConvArg> for DcConvention {
    fn from(value: DcConvArg) -> Self {
        match value {
            DcConvArg::PaperPure => Self::PaperPure,
            DcConvArg::Matpower => Self::Matpower,
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

fn main() -> anyhow::Result<()> {
    install_tracing();
    let cli = Cli::parse();
    match cli.command.unwrap_or_else(default_command) {
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
            kind,
            scheme,
        } => run_verify(&input, kind.into(), scheme.into()),
        Command::DcOpf {
            input,
            output,
            convention,
            units,
            missing_gen_cost,
            default_gen_cost,
            gen_cost_csv,
        } => run_dcopf(
            &input,
            &output,
            convention.into(),
            units.into(),
            missing_gen_cost,
            default_gen_cost.as_deref(),
            gen_cost_csv.as_deref(),
        ),
        Command::Sensitivities {
            input,
            output,
            convention,
        } => run_sensitivities(&input, &output, convention.into()),
        Command::Summary {
            input,
            from,
            scenario,
        } => run_summary(&input, from, scenario),
        Command::Package {
            input,
            output,
            from,
            scenario,
        } => run_package(&input, output.as_deref(), from, scenario),
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
    let cases: Vec<PathBuf> = if input.is_file() {
        vec![input.to_path_buf()]
    } else {
        walkdir::WalkDir::new(input)
            .max_depth(2)
            .into_iter()
            .filter_map(std::result::Result::ok)
            .filter(|e| e.file_type().is_file())
            .filter(|e| {
                e.path()
                    .extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("m"))
            })
            .map(|e| e.path().to_path_buf())
            .collect()
    };

    if cases.is_empty() {
        anyhow::bail!("no `.m` files found under {}", input.display());
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

    for case_path in &cases {
        let mpc = powerio_matrix::parse_matpower_file(case_path)
            .with_context(|| format!("parse {}", case_path.display()))?;
        let mut p = pipeline.clone();
        p.source_file = Some(case_path.clone());
        let outputs = p
            .run(&mpc, output)
            .with_context(|| format!("export {}", case_path.display()))?;
        tracing::info!(
            case = %outputs.case_name,
            n = outputs.metadata.n_buses,
            files = outputs.files.len(),
            "exported"
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

fn run_sensitivities(input: &Path, output: &Path, convention: DcConvention) -> anyhow::Result<()> {
    let mpc = powerio_matrix::parse_matpower_file(input)
        .with_context(|| format!("parse {}", input.display()))?;
    std::fs::create_dir_all(output)?;
    let view = powerio_matrix::IndexedNetwork::new(&mpc);
    let (ptdf, lodf) = powerio_matrix::build_ptdf_lodf(&view, convention)
        .with_context(|| format!("DC sensitivities for {}", input.display()))?;
    let ptdf_path = output.join(format!("{}_ptdf.mtx", view.name()));
    let lodf_path = output.join(format!("{}_lodf.mtx", view.name()));
    powerio_matrix::io::mtx::write_mtx(&ptdf, &ptdf_path)?;
    powerio_matrix::io::mtx::write_mtx(&lodf, &lodf_path)?;
    tracing::info!(
        case = %view.name(),
        ptdf = %ptdf_path.display(),
        lodf = %lodf_path.display(),
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

fn run_dcopf(
    input: &Path,
    output: &Path,
    convention: DcConvention,
    units: Units,
    missing_gen_cost: MissingGenCostArg,
    default_gen_cost: Option<&str>,
    gen_cost_csv: Option<&Path>,
) -> anyhow::Result<()> {
    let mpc = powerio_matrix::parse_matpower_file(input)
        .with_context(|| format!("parse {}", input.display()))?;
    let cost_opts = write_options(missing_gen_cost, default_gen_cost, gen_cost_csv)?;
    let opts = DcOpfOptions {
        convention,
        units,
        missing_gen_cost: cost_opts.missing_gen_cost,
        gen_cost_patches: cost_opts.gen_cost_patches,
    };
    let outputs = write_dcopf_bundle(&mpc, output, &opts)
        .with_context(|| format!("export DC OPF bundle for {}", input.display()))?;
    tracing::info!(
        case = %mpc.name,
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
        case = %nets[0].name,
        scenarios = snapshots.len(),
        dir = %outputs.dir.display(),
        files = outputs.files.len(),
        "wrote gridfm dataset"
    );
    Ok(())
}

fn run_verify(input: &Path, kind: MatrixKind, scheme: Scheme) -> anyhow::Result<()> {
    let mpc = powerio_matrix::parse_matpower_file(input)?;
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
        mpc.name,
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

fn run_summary(input: &Path, from: Option<FormatArg>, scenario: i64) -> anyhow::Result<()> {
    let value =
        if from == Some(FormatArg::Gridfm) || (from.is_none() && looks_like_gridfm_dir(input)) {
            let read = powerio_matrix::read_gridfm_dataset(input, scenario)
                .with_context(|| format!("reading gridfm dataset {}", input.display()))?;
            transmission_summary_json(&read.network, &read.warnings)
        } else if from.is_some_and(|f| f.distribution().is_some())
            || (from.is_none() && looks_like_distribution_input(input)?)
        {
            let net = powerio_dist::parse_file(input, from.map(FormatArg::name))
                .with_context(|| format!("reading {}", input.display()))?;
            distribution_summary_json(&net)
        } else {
            let parsed = powerio_matrix::parse_file(input, from.map(FormatArg::name))
                .with_context(|| format!("reading {}", input.display()))?;
            transmission_summary_json(&parsed.network, &parsed.warnings)
        };
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn run_package(
    input: &Path,
    output: Option<&Path>,
    from: Option<FormatArg>,
    scenario: i64,
) -> anyhow::Result<()> {
    let text = package_text(input, from, scenario)?;
    match output {
        Some(p) if p.as_os_str() != "-" => {
            std::fs::write(p, &text).with_context(|| format!("writing {}", p.display()))?;
            eprintln!("wrote {}", p.display());
        }
        _ => print!("{text}"),
    }
    Ok(())
}

fn package_text(input: &Path, from: Option<FormatArg>, scenario: i64) -> anyhow::Result<String> {
    let pkg = build_package(input, from, scenario)?;
    let text = pkg
        .to_json_pretty()
        .context("serializing .pio.json package")?;
    CompilerPackage::from_json(&text).context("validating .pio.json package readback")?;
    Ok(text)
}

fn build_package(
    input: &Path,
    from: Option<FormatArg>,
    scenario: i64,
) -> anyhow::Result<CompilerPackage> {
    if from == Some(FormatArg::Gridfm) || (from.is_none() && looks_like_gridfm_dir(input)) {
        let read = powerio_matrix::read_gridfm_dataset(input, scenario)
            .with_context(|| format!("reading gridfm dataset {}", input.display()))?;
        let mut pkg = CompilerPackage::from_balanced(read.network);
        add_read_warning_diagnostics(&mut pkg, "READ.GRIDFM.FIDELITY_WARNING", &read.warnings);
        set_package_source(&mut pkg, input, PackageSourceKind::Folder, "gridfm", false);
        pkg.run_sane_validation();
        return Ok(pkg);
    }

    if from.is_some_and(|f| f.distribution().is_some())
        || (from.is_none() && looks_like_distribution_input(input)?)
    {
        let net = powerio_dist::parse_file(input, from.map(FormatArg::name))
            .with_context(|| format!("reading {}", input.display()))?;
        let format = net
            .source_format
            .map(powerio_dist::DistSourceFormat::name)
            .or_else(|| from.map(FormatArg::name))
            .unwrap_or("unknown");
        let retained_source = net.source.is_some();
        let mut pkg = CompilerPackage::from_multiconductor(net);
        set_package_source(
            &mut pkg,
            input,
            package_source_kind(input, format),
            format,
            retained_source,
        );
        pkg.run_sane_validation();
        return Ok(pkg);
    }

    let parsed = powerio_matrix::parse_file(input, from.map(FormatArg::name))
        .with_context(|| format!("reading {}", input.display()))?;
    let format = balanced_source_format_name(parsed.network.source_format);
    let retained_source = parsed.network.source.is_some();
    let mut pkg = CompilerPackage::from_balanced(parsed.network);
    add_read_warning_diagnostics(
        &mut pkg,
        "READ.TRANSMISSION.PARSE_WARNING",
        &parsed.warnings,
    );
    set_package_source(
        &mut pkg,
        input,
        package_source_kind(input, format),
        format,
        retained_source,
    );
    pkg.run_sane_validation();
    Ok(pkg)
}

fn add_read_warning_diagnostics(pkg: &mut CompilerPackage, code: &str, warnings: &[String]) {
    pkg.diagnostics.extend(warnings.iter().map(|w| {
        StructuredDiagnostic::new(
            code,
            DiagnosticSeverity::Warning,
            DiagnosticStage::Read,
            w.clone(),
        )
    }));
    pkg.validation = ValidationSummary::from_diagnostics(&pkg.diagnostics);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PackageSourceKind {
    File,
    BinaryFile,
    Folder,
}

impl PackageSourceKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::BinaryFile => "binary_file",
            Self::Folder => "folder",
        }
    }
}

fn package_source_kind(input: &Path, format: &str) -> PackageSourceKind {
    if input.is_dir() {
        PackageSourceKind::Folder
    } else if format == "powerworld-pwb" {
        PackageSourceKind::BinaryFile
    } else {
        PackageSourceKind::File
    }
}

fn set_package_source(
    pkg: &mut CompilerPackage,
    input: &Path,
    kind: PackageSourceKind,
    format: &str,
    retained_source: bool,
) {
    let path = input.display().to_string();
    pkg.origin = match kind {
        PackageSourceKind::File => Origin::File {
            path: path.clone(),
            format: format.to_owned(),
            hash: None,
            retained_source,
        },
        PackageSourceKind::BinaryFile => Origin::BinaryFile {
            path: path.clone(),
            format: format.to_owned(),
            hash: None,
            decoded_sections: Vec::new(),
        },
        PackageSourceKind::Folder => Origin::Folder {
            path: path.clone(),
            format: format.to_owned(),
            file_hashes: BTreeMap::new(),
        },
    };
    pkg.sources = vec![SourceDescriptor {
        id: "src0".to_owned(),
        kind: kind.as_str().to_owned(),
        path: Some(path),
        format: Some(format.to_owned()),
        hash: None,
    }];
}

fn balanced_source_format_name(f: powerio_matrix::SourceFormat) -> &'static str {
    match f {
        powerio_matrix::SourceFormat::Matpower => "matpower",
        powerio_matrix::SourceFormat::PowerModelsJson => "powermodels-json",
        powerio_matrix::SourceFormat::EgretJson => "egret-json",
        powerio_matrix::SourceFormat::Psse => "psse",
        powerio_matrix::SourceFormat::PowerWorld => "powerworld",
        powerio_matrix::SourceFormat::PandapowerJson => "pandapower-json",
        powerio_matrix::SourceFormat::Pslf => "pslf",
        powerio_matrix::SourceFormat::Goc3Json => "goc3-json",
        powerio_matrix::SourceFormat::SurgeJson => "surge-json",
        powerio_matrix::SourceFormat::PowerWorldBinary => "powerworld-pwb",
        powerio_matrix::SourceFormat::InMemory => "in-memory",
        powerio_matrix::SourceFormat::Normalized => "normalized",
        powerio_matrix::SourceFormat::Gridfm => "gridfm",
        powerio_matrix::SourceFormat::PypsaCsv => "pypsa-csv",
        _ => "unknown",
    }
}

fn transmission_summary_json(
    net: &powerio_matrix::Network,
    warnings: &[String],
) -> serde_json::Value {
    let view = powerio_matrix::IndexedNetwork::new(net);
    json!({
        "schema": "powerio.summary",
        "schema_version": SUMMARY_SCHEMA_VERSION,
        "domain": "transmission",
        "model": "balanced",
        "name": net.name,
        "source_format": format!("{:?}", net.source_format),
        "json_format": "powerio-json",
        "base_mva": net.base_mva,
        "elements": {
            "buses": net.buses.len(),
            "branches": net.branches.len(),
            "generators": net.generators.len(),
            "loads": net.loads.len(),
            "shunts": net.shunts.len(),
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

fn distribution_summary_json(net: &powerio_dist::DistNetwork) -> serde_json::Value {
    json!({
        "schema": "powerio.summary",
        "schema_version": SUMMARY_SCHEMA_VERSION,
        "domain": "distribution",
        "model": "multiconductor",
        "name": net.name,
        "source_format": net.source_format.map(powerio_dist::DistSourceFormat::name),
        "json_format": "bmopf-json",
        "base_mva": serde_json::Value::Null,
        "elements": {
            "buses": net.buses.len(),
            "branches": serde_json::Value::Null,
            "generators": net.generators.len(),
            "loads": net.loads.len(),
            "shunts": serde_json::Value::Null,
            "lines": net.lines.len(),
            "transformers": net.transformers.len(),
            "sources": net.sources.len(),
        },
        "topology": {
            "connected_components": serde_json::Value::Null,
            "is_radial": serde_json::Value::Null,
            "reference_buses": serde_json::Value::Null,
            "connectivity_report": serde_json::Value::Null,
        },
        "warnings": net.warnings,
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

fn looks_like_distribution_input(input: &Path) -> anyhow::Result<bool> {
    Ok(infer_input_family(input)?.unwrap_or(false))
}

/// Infer the case family from clear extensions or, for `.json`, the shared
/// JSON shape classifier. `Some(true)` is distribution, `Some(false)` is
/// transmission, and `None` means the extension carries no family signal.
fn infer_input_family(input: &Path) -> anyhow::Result<Option<bool>> {
    let ext = input
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("m" | "raw" | "aux" | "epc" | "pwb") => return Ok(Some(false)),
        Some("dss") => return Ok(Some(true)),
        Some("json") => {}
        _ => return Ok(None),
    }
    let text = std::fs::read_to_string(input)
        .with_context(|| format!("reading JSON format markers from {}", input.display()))?;
    match powerio_matrix::format::routing::classify_json_text(&text) {
        Detection::Known(DetectedFormat::Distribution(_)) => Ok(Some(true)),
        Detection::Known(DetectedFormat::Transmission(_)) => Ok(Some(false)),
        Detection::Ambiguous => anyhow::bail!(
            "ambiguous JSON markers in {}; pass --from to choose a format",
            input.display()
        ),
        Detection::Unknown => anyhow::bail!(
            "cannot infer JSON format for {}; pass --from to choose a format",
            input.display()
        ),
        Detection::Known(_) => anyhow::bail!(
            "unrecognized JSON format family in {}; pass --from to choose a format",
            input.display()
        ),
    }
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
    if matches!(to, FormatArg::Goc3Json) {
        anyhow::bail!("`convert` cannot write GO Challenge 3 JSON; it is an input format");
    }
    // PyPSA CSV is a transmission format that writes a directory, not a text
    // target, so it takes the folder path and returns early.
    if to == FormatArg::PypsaCsv {
        return convert_to_pypsa_folder(input, output, from, scenario, gen_cost_options);
    }
    // The two families share no conversion path; say so directly instead of
    // letting the wrong family's reader produce a confusing format error. The
    // input family comes from --from (gridfm reads into the transmission
    // model), from a clear extension, or from the shared JSON classifier.
    let input_is_dist = if let Some(f) = from {
        Some(f.distribution().is_some())
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
    let (text, warnings) = if let Some(target) = to.transmission() {
        let options = gen_cost_options.write_options()?;
        // gridfm reads a Parquet dataset directory (the parquet-free
        // `parse_file` can't), so it routes through powerio-matrix's reader,
        // surfacing its fidelity notes.
        let net = if matches!(from, Some(FormatArg::Gridfm)) {
            let read = powerio_matrix::read_gridfm_dataset(input, scenario)
                .with_context(|| format!("reading gridfm dataset {}", input.display()))?;
            for w in &read.warnings {
                eprintln!("fidelity: {w}");
            }
            read.network
        } else {
            read_network(input, from)?
        };
        let conv = powerio_matrix::write_as_with_options(&net, target, &options)
            .with_context(|| format!("serializing to {target}"))?;
        (conv.text, conv.warnings)
    } else {
        let net = powerio_dist::parse_file(input, from.map(FormatArg::name))
            .with_context(|| format!("reading {}", input.display()))?;
        for w in &net.warnings {
            eprintln!("parse: {w}");
        }
        let target = to
            .distribution()
            .expect("the family check routed a transmission target here");
        let conv = net.to_format(target);
        (conv.text, conv.warnings)
    };
    for w in &warnings {
        eprintln!("fidelity: {w}");
    }
    match output {
        Some(p) if p.as_os_str() != "-" => {
            std::fs::write(p, &text).with_context(|| format!("writing {}", p.display()))?;
            eprintln!("wrote {}", p.display());
        }
        _ => print!("{text}"),
    }
    Ok(())
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
    let mut net = if from == Some(FormatArg::Gridfm) {
        let read = powerio_matrix::read_gridfm_dataset(input, scenario)
            .with_context(|| format!("reading gridfm dataset {}", input.display()))?;
        for w in &read.warnings {
            eprintln!("fidelity: {w}");
        }
        read.network
    } else {
        read_network(input, from)?
    };
    let options = gen_cost_options.write_options()?;
    let report = net.apply_gen_cost_policy(&options.gen_cost_patches, options.missing_gen_cost)?;
    if report.patched > 0 {
        eprintln!(
            "fidelity: generator cost patch applied to {} generator(s)",
            report.patched
        );
    }
    if report.synthesized > 0 {
        eprintln!(
            "fidelity: generator cost synthesized for {} generator(s)",
            report.synthesized
        );
    }
    let outputs = powerio_matrix::write_pypsa_csv_folder(&net, out_dir)
        .with_context(|| format!("writing PyPSA CSV folder {}", out_dir.display()))?;
    for w in &outputs.warnings {
        eprintln!("fidelity: {w}");
    }
    eprintln!("wrote {}", outputs.dir.display());
    Ok(())
}

/// Read `input` into the neutral [`powerio_matrix::Network`] through the shared
/// format hub, which picks the reader from `from` or the extension (sniffing a
/// `.json` with the shared top level shape classifier). The distribution
/// formats are rejected up front: every caller of this function consumes the
/// transmission model, and clap can't express the restriction on the shared
/// `FormatArg`. Read fidelity warnings print to stderr like the write side's.
fn read_network(
    input: &std::path::Path,
    from: Option<FormatArg>,
) -> anyhow::Result<powerio_matrix::Network> {
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
    let parsed = powerio_matrix::parse_file(input, from.map(FormatArg::name))
        .with_context(|| format!("reading {}", input.display()))?;
    for w in &parsed.warnings {
        eprintln!("fidelity: {w}");
    }
    Ok(parsed.network)
}

#[cfg(test)]
mod tests {
    use super::{
        Cli, Command, FormatArg, GenCostCliOptions, build_package, distribution_summary_json,
        infer_input_family, looks_like_distribution_input, package_text, run_convert, run_package,
        transmission_summary_json,
    };
    use clap::Parser;
    use powerio_pkg::{CompilerPackage, MappingKind, Origin, ValidationStatus};
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
        let parsed = powerio_matrix::parse_file(data("case9.m"), None).unwrap();
        let value = transmission_summary_json(&parsed.network, &parsed.warnings);
        assert_eq!(value["schema"], "powerio.summary");
        assert_eq!(value["schema_version"], "0.1");
        assert_eq!(value["domain"], "transmission");
        assert_eq!(value["model"], "balanced");
        assert_eq!(value["json_format"], "powerio-json");
        assert_eq!(value["elements"]["buses"], 9);
        assert_eq!(value["topology"]["connected_components"], 1);
    }

    #[test]
    fn summary_json_matches_canonical_distribution_shape() {
        let net = powerio_dist::parse_file(data("dist/micro/xfmr_single_phase.dss"), None).unwrap();
        let value = distribution_summary_json(&net);
        assert_eq!(value["schema"], "powerio.summary");
        assert_eq!(value["schema_version"], "0.1");
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
    fn package_visible_alias_parses() {
        let cli = Cli::try_parse_from(["powerio", "pkg", "case9.m"]).unwrap();
        match cli.command.unwrap() {
            Command::Package { input, .. } => assert_eq!(input, Path::new("case9.m")),
            other => panic!("expected package command, got {other:?}"),
        }
    }

    #[test]
    fn package_text_matches_balanced_shape_and_provenance() {
        let input = data("case9.m");
        let text = package_text(&input, None, 0).unwrap();
        let pkg = CompilerPackage::from_json(&text).unwrap();
        assert_eq!(pkg.model_kind, powerio_pkg::ModelKind::Balanced);
        assert!(pkg.kind_is_consistent());
        assert_eq!(pkg.as_balanced().unwrap().buses.len(), 9);
        match &pkg.origin {
            Origin::File {
                path,
                format,
                retained_source,
                ..
            } => {
                assert_eq!(path, &input.display().to_string());
                assert_eq!(format, "matpower");
                assert!(*retained_source);
            }
            other => panic!("expected file origin, got {other:?}"),
        }
        assert_eq!(pkg.sources.len(), 1);
        assert_eq!(pkg.sources[0].id, "src0");
        assert_eq!(pkg.sources[0].kind, "file");
        assert_eq!(
            pkg.sources[0].path.as_deref(),
            Some(input.to_str().unwrap())
        );
        assert_eq!(pkg.sources[0].format.as_deref(), Some("matpower"));
        assert!(
            pkg.source_maps.iter().any(|entry| {
                entry.mapping_kind == MappingKind::Exact
                    && entry.element_path == "/model/balanced_network/buses/0/vm"
                    && entry.source_ref.source_id == "src0"
                    && entry.source_ref.record.as_deref() == Some("bus")
                    && entry.source_ref.field.as_deref() == Some("vm")
            }),
            "expected balanced source map entries: {:?}",
            pkg.source_maps
        );
    }

    #[test]
    fn package_command_writes_output_file() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let output = std::env::temp_dir().join(format!("powerio-package-{stamp}.pio.json"));

        run_package(&data("case9.m"), Some(&output), None, 0).unwrap();
        let text = std::fs::read_to_string(&output).unwrap();
        let pkg = CompilerPackage::from_json(&text).unwrap();
        assert_eq!(pkg.model_kind, powerio_pkg::ModelKind::Balanced);
        assert_eq!(pkg.sources[0].format.as_deref(), Some("matpower"));

        let _ = std::fs::remove_file(output);
    }

    #[test]
    fn package_helper_returns_stdout_text() {
        let text = package_text(&data("case9.m"), None, 0).unwrap();
        assert!(text.contains("\"schema\""));
        let pkg = CompilerPackage::from_json(&text).unwrap();
        assert_eq!(pkg.summary.elements["buses"], 9);
    }

    #[test]
    fn package_text_includes_validation_passes() {
        let text = package_text(&data("case9.m"), None, 0).unwrap();
        let pkg = CompilerPackage::from_json(&text).unwrap();
        assert!(
            pkg.validation
                .passes
                .iter()
                .any(|p| p.name == "balanced.structure" && p.status == ValidationStatus::Ok),
            "missing balanced validation pass: {:?}",
            pkg.validation.passes
        );

        let pretty = pkg.to_json_pretty().unwrap();
        let back = CompilerPackage::from_json(&pretty).unwrap();
        assert_eq!(back.validation.passes, pkg.validation.passes);
    }

    #[test]
    fn package_distribution_fixture_keeps_defaulted_source_maps() {
        let input = data("dist/micro/xfmr_single_phase.dss");
        let pkg = build_package(&input, None, 0).unwrap();
        assert_eq!(pkg.model_kind, powerio_pkg::ModelKind::Multiconductor);
        match &pkg.origin {
            Origin::File { path, format, .. } => {
                assert_eq!(path, &input.display().to_string());
                assert_eq!(format, "dss");
            }
            other => panic!("expected file origin, got {other:?}"),
        }
        assert_eq!(
            pkg.sources[0].path.as_deref(),
            Some(input.to_str().unwrap())
        );
        assert!(
            pkg.source_maps
                .iter()
                .any(|e| e.mapping_kind == MappingKind::Defaulted),
            "expected defaulted source map entries: {:?}",
            pkg.source_maps
        );
    }

    #[test]
    fn package_rejects_non_finite_payload_before_writing() {
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

        let err = run_package(&input, Some(&output), None, 0).unwrap_err();
        assert!(
            err.to_string()
                .contains("validating .pio.json package readback"),
            "{err}"
        );
        assert!(!output.exists());

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
        let parsed = powerio_matrix::parse_file(data("case9.m"), None).unwrap();
        let conv = powerio_matrix::write_as(
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
        let parsed = powerio_matrix::parse_file(data("case9.m"), None).unwrap();
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
}
