//! The `powerio` binary: a clap CLI and a ratatui TUI over `powerio-matrix`.
//!
//! Subcommands: `batch` (matrix families), `gen` (synthetic cases), `verify`,
//! `dcopf` (DC OPF bundle), `sensitivities` (PTDF/LODF), `gridfm` (gridfm-datakit
//! Parquet), and `convert`. With no subcommand it launches the TUI. Run
//! `powerio --help` for the full surface.

use std::path::{Path, PathBuf};

use anyhow::Context;
use clap::{Parser, Subcommand, ValueEnum};
use powerio_matrix::io::gridfm::{GridfmOptions, numbered_snapshots, write_gridfm_batch};
use powerio_matrix::matrix::{BuildOptions, DcConvention, Scheme, Units, sddm_check};
use powerio_matrix::opf_pipeline::{DcOpfOptions, write_dcopf_bundle};
use powerio_matrix::pipeline::{MatrixKind, Pipeline, RhsKind};
use powerio_matrix::synth::{SynthSpec, Topology};
mod tui;

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
    },
    /// Convert a case file to another format through the neutral hub.
    Convert {
        /// Input case file, or a gridfm dataset directory with `--from gridfm`.
        /// The format is inferred from the extension (`.m`, `.json`, `.raw`,
        /// `.aux`) unless `--from` is given.
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
    },
}

/// A case interchange format, for `--to` / `--from`. `gridfm` is read-only here:
/// `convert --from gridfm` reads a Parquet dataset, but writing a gridfm dataset
/// is the dedicated `gridfm` subcommand, so `--to gridfm` is rejected.
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
    #[value(name = "powerworld", alias = "aux")]
    PowerWorld,
    #[value(name = "pandapower-json", alias = "pandapower", alias = "pp")]
    PandapowerJson,
    #[value(name = "pypsa-csv", alias = "pypsa")]
    PypsaCsv,
    /// Read a gridfm-datakit Parquet dataset directory (read-only).
    #[value(name = "gridfm")]
    Gridfm,
    /// Read a PowerWorld .pwb binary case (read-only).
    #[value(name = "pwb")]
    Pwb,
}

impl FormatArg {
    /// The write target this format maps to. `gridfm` has no convert-writer (use
    /// the `gridfm` subcommand), so it errors here rather than silently misrouting.
    fn to_target(self) -> anyhow::Result<powerio_matrix::TargetFormat> {
        use powerio_matrix::TargetFormat;
        Ok(match self {
            FormatArg::Matpower => TargetFormat::Matpower,
            FormatArg::PowerModelsJson => TargetFormat::PowerModelsJson,
            FormatArg::EgretJson => TargetFormat::EgretJson,
            FormatArg::Psse => TargetFormat::Psse,
            FormatArg::PowerWorld => TargetFormat::PowerWorld,
            FormatArg::PandapowerJson => TargetFormat::PandapowerJson,
            FormatArg::PypsaCsv => anyhow::bail!(
                "`convert` cannot return a PyPSA CSV folder as text; pass `--to pypsa-csv -o <dir>`"
            ),
            FormatArg::Gridfm => anyhow::bail!(
                "`convert` cannot write a gridfm dataset; use the `gridfm` subcommand"
            ),
            FormatArg::Pwb => anyhow::bail!("PowerWorld .pwb is read only; it cannot be a target"),
        })
    }

    /// The canonical format name. For the five classical formats this is the name
    /// `target_format_from_name` accepts, used to force a text reader; `gridfm` is
    /// parquet-only and never routes through that hub (the callers guard it first),
    /// so its name is for diagnostics only.
    fn name(self) -> &'static str {
        match self {
            FormatArg::Matpower => "matpower",
            FormatArg::PowerModelsJson => "powermodels-json",
            FormatArg::EgretJson => "egret-json",
            FormatArg::Psse => "psse",
            FormatArg::PowerWorld => "powerworld",
            FormatArg::PandapowerJson => "pandapower-json",
            FormatArg::PypsaCsv => "pypsa-csv",
            FormatArg::Gridfm => "gridfm",
            FormatArg::Pwb => "pwb",
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
    match cli.command.unwrap_or(Command::Tui {
        data_dir: None,
        out_dir: None,
    }) {
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
        } => run_gen(
            topology.into(),
            n,
            r_over_x,
            mean_x,
            seed,
            &output,
            matrices,
        ),
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
        } => run_dcopf(&input, &output, convention.into(), units.into()),
        Command::Sensitivities {
            input,
            output,
            convention,
        } => run_sensitivities(&input, &output, convention.into()),
        Command::Gridfm {
            inputs,
            output,
            from,
            scenario,
        } => run_gridfm(&inputs, &output, from, scenario),
        Command::Convert {
            input,
            to,
            output,
            from,
            scenario,
        } => run_convert(&input, to, output.as_deref(), from, scenario),
    }
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

fn run_dcopf(
    input: &Path,
    output: &Path,
    convention: DcConvention,
    units: Units,
) -> anyhow::Result<()> {
    let mpc = powerio_matrix::parse_matpower_file(input)
        .with_context(|| format!("parse {}", input.display()))?;
    let opts = DcOpfOptions { convention, units };
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

    let opts = GridfmOptions::default();
    let outputs = write_gridfm_batch(&snapshots, output, &opts)
        .with_context(|| format!("export gridfm dataset for {} scenario(s)", snapshots.len()))?;
    if outputs.dropped_zero_impedance > 0 || outputs.degenerate_cost_gens > 0 {
        tracing::warn!(
            zeroed_branches = outputs.dropped_zero_impedance,
            degenerate_cost_gens = outputs.degenerate_cost_gens,
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
    let stats = powerio_matrix::matrix::MatrixStats::from_csr(&matrix);
    let sddm = sddm_check(&matrix);
    println!(
        "{} ({}): n={} nnz={} min_diag={:.3e} max_diag={:.3e} dd_margin={:.3e} M-sign={} ‖A‖_F={:.3e} SDDM={}",
        kind.label(),
        mpc.name,
        stats.n,
        stats.nnz,
        stats.min_diag,
        stats.max_diag,
        stats.min_dd_margin,
        stats.m_matrix_sign,
        stats.frobenius_norm,
        sddm
    );
    Ok(())
}

fn run_convert(
    input: &std::path::Path,
    to: FormatArg,
    output: Option<&std::path::Path>,
    from: Option<FormatArg>,
    scenario: i64,
) -> anyhow::Result<()> {
    if to == FormatArg::PypsaCsv {
        let Some(out_dir) = output else {
            anyhow::bail!("`--to pypsa-csv` requires `-o <output-dir>`");
        };
        if out_dir.as_os_str() == "-" {
            anyhow::bail!("`--to pypsa-csv` writes a directory and cannot write to stdout");
        }
        let net = if from == Some(FormatArg::Gridfm) {
            let read = powerio_matrix::read_gridfm_dataset(input, scenario)
                .with_context(|| format!("reading gridfm dataset {}", input.display()))?;
            for w in &read.warnings {
                eprintln!("fidelity: {w}");
            }
            read.network
        } else {
            read_network(input, from)?
        };
        let outputs = powerio_matrix::write_pypsa_csv_folder(&net, out_dir)
            .with_context(|| format!("writing PyPSA CSV folder {}", out_dir.display()))?;
        for w in &outputs.warnings {
            eprintln!("fidelity: {w}");
        }
        eprintln!("wrote {}", outputs.dir.display());
        return Ok(());
    }
    let target = to.to_target()?;
    // gridfm reads a Parquet dataset directory (the parquet-free `parse_file`
    // can't), so it routes through powerio-matrix's reader, surfacing its fidelity
    // notes.
    let net = if from == Some(FormatArg::Gridfm) {
        let read = powerio_matrix::read_gridfm_dataset(input, scenario)
            .with_context(|| format!("reading gridfm dataset {}", input.display()))?;
        for w in &read.warnings {
            eprintln!("fidelity: {w}");
        }
        read.network
    } else {
        read_network(input, from)?
    };
    let conv = powerio_matrix::write_as(&net, target)
        .with_context(|| format!("serializing to {target}"))?;
    for w in &conv.warnings {
        eprintln!("fidelity: {w}");
    }
    match output {
        Some(p) if p.as_os_str() != "-" => {
            std::fs::write(p, &conv.text).with_context(|| format!("writing {}", p.display()))?;
            eprintln!("wrote {}", p.display());
        }
        _ => print!("{}", conv.text),
    }
    Ok(())
}

/// Read `input` into the neutral [`powerio_matrix::Network`] through the shared
/// format hub, which picks the reader from `from` or the extension (sniffing a
/// `.json` for the pandapower vs egret vs PowerModels shape). Read fidelity
/// warnings print to stderr like the write side's.
fn read_network(
    input: &std::path::Path,
    from: Option<FormatArg>,
) -> anyhow::Result<powerio_matrix::Network> {
    let parsed = powerio_matrix::parse_file(input, from.map(FormatArg::name))
        .with_context(|| format!("reading {}", input.display()))?;
    for w in &parsed.warnings {
        eprintln!("fidelity: {w}");
    }
    Ok(parsed.network)
}
