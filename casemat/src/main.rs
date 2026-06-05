use std::path::PathBuf;

use anyhow::Context;
use clap::{Parser, Subcommand, ValueEnum};
use casemat::matrix::{BuildOptions, DcConvention, Scheme, Units, sddm_check};
use casemat::opf_pipeline::{DcOpfOptions, write_dcopf_bundle};
use casemat::pipeline::{MatrixKind, Pipeline, RhsKind};
use casemat::synth::{SynthSpec, Topology};
use casemat::tui;

#[derive(Parser, Debug)]
#[command(name = "casemat", version, about)]
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
        /// Comma separated matrix kinds to emit.
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
    /// Print B' / B'' / Y_bus stats and SDDM check for one case.
    Verify {
        /// MATPOWER `.m` file.
        input: PathBuf,
        #[arg(long, value_enum, default_value = "bprime")]
        kind: MatrixKindArg,
        #[arg(long, value_enum, default_value = "bx")]
        scheme: SchemeArg,
    },
    /// Emit the static DC-OPF matrix/vector bundle for one case.
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
        } => run_batch(input, output, matrices, scheme.into(), rhs.into(), seed),
        Command::Gen {
            topology,
            n,
            r_over_x,
            mean_x,
            seed,
            output,
            matrices,
        } => run_gen(topology.into(), n, r_over_x, mean_x, seed, output, matrices),
        Command::Verify {
            input,
            kind,
            scheme,
        } => run_verify(input, kind.into(), scheme.into()),
        Command::DcOpf {
            input,
            output,
            convention,
            units,
        } => run_dcopf(input, output, convention.into(), units.into()),
        Command::Sensitivities {
            input,
            output,
            convention,
        } => run_sensitivities(input, output, convention.into()),
    }
}

fn install_tracing() {
    use tracing_subscriber::EnvFilter;
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_writer(std::io::stderr)
        .try_init();
}

fn run_batch(
    input: PathBuf,
    output: PathBuf,
    matrices: Vec<MatrixKindArg>,
    scheme: Scheme,
    rhs: RhsKind,
    seed: u64,
) -> anyhow::Result<()> {
    let cases: Vec<PathBuf> = if input.is_file() {
        vec![input.clone()]
    } else {
        walkdir::WalkDir::new(&input)
            .max_depth(2)
            .into_iter()
            .filter_map(std::result::Result::ok)
            .filter(|e| e.file_type().is_file())
            .filter(|e| e.path().extension().is_some_and(|x| x == "m"))
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
        let mpc = casemat::parse_matpower_file(case_path)
            .with_context(|| format!("parse {}", case_path.display()))?;
        let mut p = pipeline.clone();
        p.source_file = Some(case_path.clone());
        let outputs = p
            .run(&mpc, &output)
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
    output: PathBuf,
    matrices: Vec<MatrixKindArg>,
) -> anyhow::Result<()> {
    let spec = SynthSpec {
        topology,
        n,
        r_over_x,
        mean_x,
        seed,
    };
    let case = casemat::synth::generate(&spec);
    let pipeline = Pipeline {
        matrices: matrices.into_iter().map(MatrixKind::from).collect(),
        ..Default::default()
    };
    let outputs = pipeline.run(&case, &output)?;
    tracing::info!(
        case = %outputs.case_name,
        n = outputs.metadata.n_buses,
        files = outputs.files.len(),
        "synthesized"
    );
    Ok(())
}

fn run_sensitivities(
    input: PathBuf,
    output: PathBuf,
    convention: DcConvention,
) -> anyhow::Result<()> {
    let mpc = casemat::parse_matpower_file(&input)
        .with_context(|| format!("parse {}", input.display()))?;
    std::fs::create_dir_all(&output)?;
    let ptdf = casemat::build_ptdf(&mpc, convention)
        .with_context(|| format!("PTDF for {}", input.display()))?;
    let lodf = casemat::build_lodf(&mpc, convention)
        .with_context(|| format!("LODF for {}", input.display()))?;
    let ptdf_path = output.join(format!("{}_ptdf.mtx", mpc.name));
    let lodf_path = output.join(format!("{}_lodf.mtx", mpc.name));
    casemat::io::mtx::write_mtx(&ptdf, &ptdf_path)?;
    casemat::io::mtx::write_mtx(&lodf, &lodf_path)?;
    tracing::info!(
        case = %mpc.name,
        ptdf = %ptdf_path.display(),
        lodf = %lodf_path.display(),
        "wrote DC sensitivities"
    );
    Ok(())
}

fn run_dcopf(
    input: PathBuf,
    output: PathBuf,
    convention: DcConvention,
    units: Units,
) -> anyhow::Result<()> {
    let mpc = casemat::parse_matpower_file(&input)
        .with_context(|| format!("parse {}", input.display()))?;
    let opts = DcOpfOptions { convention, units };
    let outputs = write_dcopf_bundle(&mpc, &output, &opts)
        .with_context(|| format!("export DC-OPF bundle for {}", input.display()))?;
    tracing::info!(
        case = %mpc.name,
        dir = %outputs.dir.display(),
        files = outputs.files.len(),
        "wrote DC-OPF bundle"
    );
    Ok(())
}

fn run_verify(input: PathBuf, kind: MatrixKind, scheme: Scheme) -> anyhow::Result<()> {
    let mpc = casemat::parse_matpower_file(&input)?;
    let opts = BuildOptions {
        scheme,
        ..Default::default()
    };
    let matrix = match kind {
        MatrixKind::BPrime => casemat::build_bprime(&mpc, &opts)?,
        MatrixKind::BDoublePrime => casemat::build_bdoubleprime(&mpc, &opts)?,
        MatrixKind::YbusG => casemat::build_ybus(&mpc, &opts)?.g,
        MatrixKind::YbusB => {
            let mut b = casemat::build_ybus(&mpc, &opts)?.b;
            for v in b.data_mut() {
                *v = -*v;
            }
            b
        }
        MatrixKind::Lacpf => casemat::build_lacpf(&mpc, &opts)?,
        MatrixKind::Adjacency => casemat::build_adjacency(&mpc)?,
    };
    let stats = casemat::matrix::MatrixStats::from_csr(&matrix);
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
