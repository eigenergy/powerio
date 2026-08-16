//! Orchestrates a single case → output directory.
//!
//! Given a parsed `BalancedNetwork`, builds the requested matrix family, writes
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
    BuildOptions, MatrixStats, ZeroImpedanceRule, ZeroImpedanceSkips, build_adjacency,
    build_bdoubleprime, build_bprime, build_lacpf, build_ybus, negate_into, sddm_check,
    skipped_zero_impedance,
};
use crate::network::BalancedNetwork;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum MatrixKind {
    /// MATPOWER FDPF Bp matrix.
    BPrime,
    /// MATPOWER FDPF Bpp matrix.
    BDoublePrime,
    /// `Re(Y_bus)` — full conductance matrix.
    YbusG,
    /// `-Im(Y_bus)` — full bus susceptance matrix (positive convention).
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
            Self::BPrime => "MATPOWER Bp (FDPF)",
            Self::BDoublePrime => "MATPOWER Bpp (FDPF)",
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

/// Longest sanitized stem before truncation. Output filenames append fixed
/// suffixes (`_sensitivity_meta.json` is the longest at 22 bytes) and the
/// collision hash adds 9 more, so the longest filename stays well under the
/// common 255 byte component limit.
const MAX_STEM_LEN: usize = 120;

/// Hex digits of the SHA-256 disambiguator appended to a sanitized stem.
/// 64 bits, so a batch export cannot be steered into an overwrite by
/// searching for a second name that hashes into an existing stem.
const DIGEST_LEN: usize = 16;

/// Filenames Windows reserves for devices, matched against the part of a
/// name before its first dot, case insensitively: `con` and `con.4` both
/// resolve to the CON device.
const WINDOWS_RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Reduces a network name to a single safe filename stem. The name comes from
/// input content, so an unsanitized value like `../../etc/x` or `/abs/x` used
/// in `out_dir.join(...)` would write outside `out_dir`. Keeps ASCII
/// alphanumerics, `-`, `_`, and `.`; every other character (path separators
/// included) becomes `_`. Leading dots are stripped so the result is never
/// `.`, `..`, or a hidden file; trailing dots are trimmed and Windows
/// reserved device names get a leading `_`, both invalid as Windows
/// filenames; the length is capped at 120 bytes; and an empty result
/// falls back to `case`.
///
/// A name the sanitizer had to change carries a hash of the original, so two
/// distinct names that would sanitize identically (`a/b` and `a_b`, say)
/// cannot silently overwrite each other's files in a multi case export. A
/// name that already ends in the suffix shape is hashed too, so a case can
/// never be named to impersonate another case's disambiguated stem: the
/// suffixed and unsuffixed name spaces stay disjoint. Any other safe name
/// passes through unchanged.
pub fn sanitize_stem(name: &str) -> String {
    // Skipping leading dots before mapping is equivalent to trimming them
    // after: the map is the identity on `.` and never produces one, so the set
    // of leading dots is the same either way.
    let mut stem: String = name
        .chars()
        .skip_while(|&c| c == '.')
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    while stem.ends_with('.') {
        stem.pop();
    }
    let pre_dot = stem.split('.').next().unwrap_or("");
    if WINDOWS_RESERVED
        .iter()
        .any(|r| pre_dot.eq_ignore_ascii_case(r))
    {
        stem.insert(0, '_');
    }
    if stem.is_empty() {
        stem.push_str("case");
    }
    if stem == name && stem.len() <= MAX_STEM_LEN && !ends_with_digest(&stem) {
        return stem;
    }
    // The map above only emits ASCII, so byte truncation cannot split a
    // character, and the budget leaves room for the suffix.
    stem.truncate(MAX_STEM_LEN - DIGEST_LEN - 1);
    stem.push('-');
    stem.push_str(&sha256_hex(name.as_bytes())[..DIGEST_LEN]);
    stem
}

/// Whether a stem already carries the disambiguating suffix shape. Such a
/// name gets a suffix of its own, so no name can be chosen to collide with
/// another name's disambiguated stem.
fn ends_with_digest(stem: &str) -> bool {
    stem.len() > DIGEST_LEN
        && stem.as_bytes()[stem.len() - DIGEST_LEN - 1] == b'-'
        && stem[stem.len() - DIGEST_LEN..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl Pipeline {
    pub fn run(&self, net: &BalancedNetwork, out_dir: impl AsRef<Path>) -> Result<PipelineOutputs> {
        let out_dir = out_dir.as_ref();
        std::fs::create_dir_all(out_dir)?;

        let view = IndexedNetwork::new(net);
        // The network name comes from input content, so it must not steer the
        // output path: a name like `../../x` or `/abs/x` would otherwise write
        // outside `out_dir`. Filenames use the sanitized stem; the metadata
        // keeps the original name.
        let stem = sanitize_stem(view.name());

        let mut files = Vec::new();
        let mut matrices_meta = Vec::new();
        let mut ybus_cache = None;

        for &kind in &self.matrices {
            let matrix_path = out_dir.join(format!("{stem}_{}.mtx", kind.slug()));
            let matrix = self.build_for_run(&view, kind, &mut ybus_cache)?;
            write_mtx(&matrix, &matrix_path)?;
            let stats = matrix_stats_for_kind(&matrix, &view, kind, &self.options);
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
                let rhs_path = out_dir.join(format!("{stem}_{}_rhs.mtx", kind.slug()));
                write_vector_mtx(&rhs, &rhs_path)?;
                files.push(rhs_path);
            }
        }

        // Shunt vector as a sidecar (not always meaningful, but cheap).
        let shunt_path = out_dir.join(format!("{stem}_shunt.mtx"));
        let base = view.per_unit_base();
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
            powerio_version: env!("CARGO_PKG_VERSION").to_string(),
        };
        let meta_path = out_dir.join(format!("{stem}_meta.json"));
        write_meta_json(&metadata, &meta_path)?;
        files.push(meta_path);

        Ok(PipelineOutputs {
            case_name: view.name().to_string(),
            files,
            metadata,
        })
    }

    fn build_for_run(
        &self,
        case: &IndexedNetwork,
        kind: MatrixKind,
        ybus_cache: &mut Option<YbusCache>,
    ) -> Result<sprs::CsMat<f64>> {
        match kind {
            MatrixKind::YbusG => take_ybus_g(case, &self.options, ybus_cache),
            MatrixKind::YbusB => take_ybus_b(case, &self.options, ybus_cache),
            _ => build_kind(case, kind, &self.options),
        }
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
                let base = case.per_unit_base();
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

struct YbusCache {
    g: Option<sprs::CsMat<f64>>,
    b: Option<sprs::CsMat<f64>>,
}

fn fill_ybus_cache(
    view: &IndexedNetwork,
    opts: &BuildOptions,
    ybus_cache: &mut Option<YbusCache>,
) -> Result<()> {
    let parts = build_ybus(view, opts)?;
    *ybus_cache = Some(YbusCache {
        g: Some(parts.g),
        b: Some(parts.b),
    });
    Ok(())
}

fn take_ybus_g(
    view: &IndexedNetwork,
    opts: &BuildOptions,
    ybus_cache: &mut Option<YbusCache>,
) -> Result<sprs::CsMat<f64>> {
    if ybus_cache.as_ref().is_none_or(|c| c.g.is_none()) {
        fill_ybus_cache(view, opts, ybus_cache)?;
    }
    Ok(ybus_cache
        .as_mut()
        .and_then(|c| c.g.take())
        .expect("Ybus cache was just filled, so the real part must be present"))
}

fn take_ybus_b(
    view: &IndexedNetwork,
    opts: &BuildOptions,
    ybus_cache: &mut Option<YbusCache>,
) -> Result<sprs::CsMat<f64>> {
    if ybus_cache.as_ref().is_none_or(|c| c.b.is_none()) {
        fill_ybus_cache(view, opts, ybus_cache)?;
    }
    let b = ybus_cache
        .as_mut()
        .and_then(|c| c.b.take())
        .expect("Ybus cache was just filled, so the imaginary part must be present");
    Ok(negate_into(b))
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

pub fn zero_impedance_rule_for_kind(
    kind: MatrixKind,
    opts: &BuildOptions,
) -> Option<ZeroImpedanceRule> {
    match kind {
        MatrixKind::BPrime => Some(match opts.scheme {
            crate::matrix::Scheme::Bx => ZeroImpedanceRule::Series,
            crate::matrix::Scheme::Xb => ZeroImpedanceRule::Reactance,
        }),
        MatrixKind::BDoublePrime => Some(match opts.scheme {
            crate::matrix::Scheme::Bx => ZeroImpedanceRule::Reactance,
            crate::matrix::Scheme::Xb => ZeroImpedanceRule::Series,
        }),
        MatrixKind::YbusG | MatrixKind::YbusB | MatrixKind::Lacpf => {
            Some(ZeroImpedanceRule::Series)
        }
        MatrixKind::Adjacency => None,
    }
}

pub fn zero_impedance_skips_for_kind(
    view: &IndexedNetwork,
    kind: MatrixKind,
    opts: &BuildOptions,
) -> ZeroImpedanceSkips {
    if !opts.skip_zero_impedance {
        return ZeroImpedanceSkips::default();
    }
    zero_impedance_rule_for_kind(kind, opts).map_or_else(ZeroImpedanceSkips::default, |rule| {
        skipped_zero_impedance(view, rule)
    })
}

pub fn matrix_stats_for_kind(
    matrix: &sprs::CsMat<f64>,
    view: &IndexedNetwork,
    kind: MatrixKind,
    opts: &BuildOptions,
) -> MatrixStats {
    MatrixStats::from_csr(matrix)
        .with_zero_impedance_skips(zero_impedance_skips_for_kind(view, kind, opts))
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

#[cfg(test)]
mod tests {
    use super::sanitize_stem;
    use std::path::Path;

    #[test]
    fn sanitize_stem_confines_names_to_out_dir() {
        // Every result must be a single component with no separators, so
        // `out_dir.join(stem)` cannot escape `out_dir`.
        for name in [
            "../../etc/passwd",
            "/abs/path",
            "..",
            ".",
            "..\\..\\win",
            "",
            "a/b/c",
        ] {
            let stem = sanitize_stem(name);
            let joined = Path::new("out").join(&stem);
            assert_eq!(
                joined.components().count(),
                2,
                "{name:?} -> {stem:?} escaped out_dir as {joined:?}"
            );
            assert!(!stem.is_empty());
            assert!(stem != "." && stem != "..");
        }
    }

    #[test]
    fn sanitize_stem_keeps_ordinary_names() {
        assert_eq!(sanitize_stem("case118"), "case118");
        assert_eq!(sanitize_stem("ieee-13_bus.v2"), "ieee-13_bus.v2");
    }

    #[test]
    fn sanitize_stem_separates_names_that_sanitize_alike() {
        // `a/b` must not overwrite `a_b`'s files in a multi case export.
        assert_ne!(sanitize_stem("a/b"), sanitize_stem("a_b"));
        assert_ne!(sanitize_stem("a/b"), sanitize_stem("a\\b"));
        assert_eq!(sanitize_stem("a_b"), "a_b");
    }

    #[test]
    fn a_safe_name_cannot_impersonate_a_disambiguated_stem() {
        // `.foo` sanitizes to `foo` and so carries a suffix. Naming a second
        // case exactly that suffixed stem would overwrite it if safe names
        // passed through unconditionally, and the suffix is derived from a
        // published hash, so the second name takes no search to construct.
        let unsafe_stem = sanitize_stem(".foo");
        assert_ne!(sanitize_stem(&unsafe_stem), unsafe_stem);
        assert_ne!(sanitize_stem(&unsafe_stem), sanitize_stem(".foo"));
        // A name that merely contains a hyphen is untouched.
        assert_eq!(sanitize_stem("ieee-13"), "ieee-13");
        assert_eq!(sanitize_stem("case-deadbeef"), "case-deadbeef");
    }

    #[test]
    fn sanitize_stem_applies_windows_filename_rules() {
        // Trailing dots and reserved device names are invalid on Windows.
        let trailing = sanitize_stem("case.");
        assert!(!trailing.ends_with('.'), "{trailing:?}");
        for name in ["con", "CON", "aux.4", "lpt9"] {
            let stem = sanitize_stem(name);
            let pre_dot = stem.split('.').next().unwrap();
            assert!(
                !super::WINDOWS_RESERVED
                    .iter()
                    .any(|r| pre_dot.eq_ignore_ascii_case(r)),
                "{name:?} -> {stem:?} is still a reserved device name"
            );
        }
    }

    #[test]
    fn sanitize_stem_caps_the_length() {
        let long = "x".repeat(4096);
        // Stem plus hash suffix stays within a filename component budget.
        assert!(sanitize_stem(&long).len() <= super::MAX_STEM_LEN);
    }
}
