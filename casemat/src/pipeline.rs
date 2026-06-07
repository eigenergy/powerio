//! Orchestrates a single case → output directory.
//!
//! Given a parsed `Network`, builds the requested matrix family, writes
//! `.mtx` files, and emits a `meta.json` sidecar describing what was
//! produced. Used by both the `batch` CLI subcommand and the TUI's
//! batch export screen.

use std::path::{Path, PathBuf};

use rand::SeedableRng;
use rand::distr::{Distribution, StandardUniform};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use crate::Result;
use crate::indexed::IndexedNetwork;
use crate::io::meta::{CaseMetadata, MatrixMetadata, write_meta_json};
use crate::io::mtx::{write_mtx, write_vector_mtx};
use crate::matrix::{
    BuildOptions, MatrixStats, build_adjacency, build_bdoubleprime, build_bprime, build_lacpf,
    build_ybus, negate_into, sddm_check,
};
use crate::network::Network;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatrixKind {
    /// FDPF B' (shuntless, taps=1, shifts=0).
    BPrime,
    /// FDPF B'' (with shunts, taps; r=0 if BX scheme).
    BDoublePrime,
    /// `Re(Y_bus)` — full conductance matrix.
    YbusG,
    /// `-Im(Y_bus)` — full susceptance Laplacian (positive convention).
    YbusB,
    /// LACPF block: `[[G, -B], [-B, -G]]`, 2n × 2n indefinite.
    Lacpf,
    /// 0/1 bus adjacency matrix.
    Adjacency,
}

impl MatrixKind {
    pub const ALL: &'static [MatrixKind] = &[
        Self::BPrime,
        Self::BDoublePrime,
        Self::YbusG,
        Self::YbusB,
        Self::Lacpf,
        Self::Adjacency,
    ];

    pub fn slug(self) -> &'static str {
        match self {
            Self::BPrime => "bprime",
            Self::BDoublePrime => "bdoubleprime",
            Self::YbusG => "ybus_real",
            Self::YbusB => "ybus_imag",
            Self::Lacpf => "lacpf",
            Self::Adjacency => "adjacency",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::BPrime => "B' (FDPF, shuntless)",
            Self::BDoublePrime => "B'' (FDPF, with shunts)",
            Self::YbusG => "Re(Y_bus)",
            Self::YbusB => "-Im(Y_bus)",
            Self::Lacpf => "LACPF block (2n×2n)",
            Self::Adjacency => "adjacency (0/1)",
        }
    }
}

/// How to populate the RHS vector(s) emitted alongside each matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum RhsKind {
    #[default]
    None,
    /// Zero-mean Gaussian random (deterministic from `rng_seed`).
    Random,
    /// Power injections from the case: `b = (Pd, Qd) / baseMVA`.
    Injection,
}

#[derive(Debug, Clone)]
pub struct Pipeline {
    pub matrices: Vec<MatrixKind>,
    pub options: BuildOptions,
    pub rhs: RhsKind,
    pub rng_seed: u64,
    pub source_file: Option<PathBuf>,
}

impl Default for Pipeline {
    fn default() -> Self {
        Self {
            matrices: vec![MatrixKind::BPrime],
            options: BuildOptions::default(),
            rhs: RhsKind::None,
            rng_seed: 0x00C0_FFEE,
            source_file: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PipelineOutputs {
    pub case_name: String,
    pub files: Vec<PathBuf>,
    pub metadata: CaseMetadata,
}

impl Pipeline {
    pub fn run(&self, net: &Network, out_dir: impl AsRef<Path>) -> Result<PipelineOutputs> {
        let out_dir = out_dir.as_ref();
        std::fs::create_dir_all(out_dir)?;

        let view = IndexedNetwork::new(net);

        let mut files = Vec::new();
        let mut matrices_meta = Vec::new();

        for &kind in &self.matrices {
            let matrix_path = out_dir.join(format!("{}_{}.mtx", view.name(), kind.slug()));
            let matrix = self.build(&view, kind)?;
            write_mtx(&matrix, &matrix_path)?;
            let stats = MatrixStats::from_csr(&matrix);
            let sddm = sddm_check(&matrix);
            matrices_meta.push(MatrixMetadata {
                kind: kind.slug().to_string(),
                file: matrix_path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string(),
                stats,
                sddm,
            });
            files.push(matrix_path);

            // RHS for matrices that take a RHS of length n (skip LACPF which is 2n).
            if let Some(rhs) = self.build_rhs(&view, kind) {
                let rhs_path = out_dir.join(format!("{}_{}_rhs.mtx", view.name(), kind.slug()));
                write_vector_mtx(&rhs, &rhs_path)?;
                files.push(rhs_path);
            }
        }

        // Shunt vector as a sidecar (not always meaningful, but cheap).
        let shunt_path = out_dir.join(format!("{}_shunt.mtx", view.name()));
        let base = view.base_mva();
        let shunt: Vec<f64> = view.bs().iter().map(|&b| b / base).collect();
        write_vector_mtx(&shunt, &shunt_path)?;
        files.push(shunt_path);

        let metadata = CaseMetadata {
            case_name: view.name().to_string(),
            source_file: self
                .source_file
                .as_ref()
                .and_then(|p| p.to_str())
                .map(str::to_string),
            source_sha256: self
                .source_file
                .as_ref()
                .and_then(|p| std::fs::read(p).ok())
                .map(|b| sha256_hex(&b)),
            base_mva: view.base_mva(),
            n_buses: view.n(),
            n_branches: view.branches().len(),
            build_options: self.options.clone(),
            matrices: matrices_meta,
            casemat_version: env!("CARGO_PKG_VERSION").to_string(),
        };
        let meta_path = out_dir.join(format!("{}_meta.json", view.name()));
        write_meta_json(&metadata, &meta_path)?;
        files.push(meta_path);

        Ok(PipelineOutputs {
            case_name: view.name().to_string(),
            files,
            metadata,
        })
    }

    fn build(&self, case: &IndexedNetwork, kind: MatrixKind) -> Result<sprs::CsMat<f64>> {
        build_kind(case, kind, &self.options)
    }

    fn build_rhs(&self, case: &IndexedNetwork, kind: MatrixKind) -> Option<Vec<f64>> {
        // No meaningful RHS for the 2n LACPF block or the structural adjacency.
        if matches!(self.rhs, RhsKind::None)
            || matches!(kind, MatrixKind::Lacpf | MatrixKind::Adjacency)
        {
            return None;
        }
        let n = case.n();
        let v = match self.rhs {
            RhsKind::Random => {
                let mut rng = ChaCha8Rng::seed_from_u64(self.rng_seed.wrapping_add(kind as u64));
                let dist = StandardUniform;
                let mut v: Vec<f64> = (0..n)
                    .map(|_| {
                        let u: f64 = dist.sample(&mut rng);
                        u - 0.5
                    })
                    .collect();
                let mean = v.iter().sum::<f64>() / n as f64;
                for x in &mut v {
                    *x -= mean; // zero-mean for Laplacian compatibility
                }
                v
            }
            RhsKind::Injection => {
                let base = case.base_mva();
                match kind {
                    MatrixKind::BPrime | MatrixKind::YbusG | MatrixKind::YbusB => {
                        case.pd().iter().map(|&p| -p / base).collect()
                    }
                    MatrixKind::BDoublePrime => case.qd().iter().map(|&q| -q / base).collect(),
                    MatrixKind::Lacpf | MatrixKind::Adjacency => unreachable!(),
                }
            }
            RhsKind::None => unreachable!(),
        };
        Some(v)
    }
}

/// Build the square matrix for one [`MatrixKind`] from an indexed network. The
/// single dispatch shared by the [`Pipeline`], the `verify` CLI command, and the
/// TUI inspect screen, so the `YbusB = -Im(Y_bus)` sign lives in one place.
pub fn build_kind(
    view: &IndexedNetwork,
    kind: MatrixKind,
    opts: &BuildOptions,
) -> Result<sprs::CsMat<f64>> {
    match kind {
        MatrixKind::BPrime => build_bprime(view, opts),
        MatrixKind::BDoublePrime => build_bdoubleprime(view, opts),
        MatrixKind::YbusG => build_ybus(view, opts).map(|p| p.g),
        MatrixKind::YbusB => build_ybus(view, opts).map(|p| negate_into(p.b)),
        MatrixKind::Lacpf => build_lacpf(view, opts),
        MatrixKind::Adjacency => build_adjacency(view),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}
