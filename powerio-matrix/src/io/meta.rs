//! Per case metadata: matrices emitted, their stats, source file digest,
//! build options. Used by the TUI Inspect screen and as a sidecar for
//! downstream tooling.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::matrix::{BuildOptions, MatrixStats};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseMetadata {
    pub case_name: String,
    pub source_file: Option<String>,
    pub source_sha256: Option<String>,
    pub base_mva: f64,
    pub n_buses: usize,
    pub n_branches: usize,
    pub build_options: BuildOptions,
    pub matrices: Vec<MatrixMetadata>,
    pub powerio_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatrixMetadata {
    pub kind: String,
    pub file: String,
    pub stats: MatrixStats,
    pub sddm: bool,
}

pub fn emit_meta_json(meta: &CaseMetadata, path: impl AsRef<Path>) -> Result<()> {
    super::mtx::commit_one_file(path.as_ref(), meta_json_bytes(meta)?)
}

/// The complete metadata document as pretty JSON bytes.
pub(crate) fn meta_json_bytes(meta: &CaseMetadata) -> Result<Vec<u8>> {
    let json = serde_json::to_string_pretty(meta).map_err(|e| crate::Error::Mtx(e.to_string()))?;
    Ok(json.into_bytes())
}
