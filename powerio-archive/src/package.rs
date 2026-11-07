//! Deterministic rebuilt ZIPs; never a claim of original-container fidelity.
use crate::{ArchiveLimits, Result, limit, malformed, validate_path};
use powerio_core::ArtifactPath;
use std::{
    collections::BTreeMap,
    io::{Cursor, Seek, SeekFrom, Write},
    sync::Arc,
};

struct BoundedOutput {
    inner: Cursor<Vec<u8>>,
    limit: u64,
}
impl Write for BoundedOutput {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.inner.position().saturating_add(bytes.len() as u64) > self.limit {
            return Err(std::io::Error::other("ZIP output byte limit exceeded"));
        }
        self.inner.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl Seek for BoundedOutput {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(position)
    }
}
/// Sorted paths, fixed timestamps/permissions and stored payloads yield the same
/// bytes for the same files with this writer version. Limits apply to both
/// source payloads and the complete output (including ZIP metadata). All names
/// obey the same portable collision policy as archive admission.
pub fn deterministic_zip(
    files: &[(ArtifactPath, Arc<[u8]>)],
    limits: ArchiveLimits,
) -> Result<Vec<u8>> {
    if files.len() > limits.max_entries() {
        return Err(limit("bundle file limit exceeded"));
    }
    let mut sorted: Vec<_> = files.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut nodes = BTreeMap::new();
    let mut total = 0u64;
    for (path, bytes) in &sorted {
        validate_path(path.as_str(), false, &mut nodes)?;
        total = total
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| limit("bundle byte overflow"))?;
        if total > limits.max_expanded_bytes() {
            return Err(limit("bundle payload limit exceeded"));
        }
    }
    let minimum_output = sorted.iter().try_fold(22u64, |total, (path, bytes)| {
        total
            .checked_add(76 + 2 * path.as_str().len() as u64)?
            .checked_add(bytes.len() as u64)
    });
    if minimum_output.is_none_or(|size| size > limits.max_input_bytes()) {
        return Err(limit("bundle plus ZIP metadata exceeds output limit"));
    }
    let mut writer = zip::ZipWriter::new(BoundedOutput {
        inner: Cursor::new(Vec::new()),
        limit: limits.max_input_bytes(),
    });
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .last_modified_time(zip::DateTime::default())
        .unix_permissions(0o644);
    for (path, bytes) in sorted {
        writer
            .start_file(path.as_str(), options)
            .map_err(|e| malformed(e.to_string()))?;
        writer.write_all(bytes).map_err(|e| limit(e.to_string()))?;
    }
    Ok(writer
        .finish()
        .map_err(|e| limit(e.to_string()))?
        .inner
        .into_inner())
}
