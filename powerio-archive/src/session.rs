//! Shared cancellation and bounded verification for adapter-owned ZIP namespaces.
use crate::{ArchiveLimits, Result, end_records, limit, malformed};
use std::io::{Cursor, Read};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// Cooperative cancellation. Clones signal the same operation/session.
#[derive(Clone, Default, Debug)]
pub struct Cancellation(Arc<AtomicBool>);
impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn check(&self) -> Result<()> {
        if self.0.load(Ordering::Relaxed) {
            Err(limit("archive operation cancelled"))
        } else {
            Ok(())
        }
    }
}
pub(crate) struct CancellableReader<'a, R>(pub &'a mut R, pub &'a Cancellation);
impl<R: Read> Read for CancellableReader<'_, R> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.1.check().map_err(std::io::Error::other)?;
        // Bound cancellation latency even when read_to_end offers a large slice.
        let n = bytes.len().min(64 << 10);
        self.0.read(&mut bytes[..n])
    }
}

/// Verify an adapter-owned ZIP under session limits without changing its names
/// or flattening its contents. Allows the ZIP reader's compatible layouts and
/// compression methods; the adapter still owns namespace/format validation.
/// Every payload is checked against its declared size and CRC before delegation.
/// Returns total entries and expansion, including opaque native companions.
pub fn verify_native(
    bytes: &[u8],
    limits: ArchiveLimits,
    cancel: &Cancellation,
    reserve: impl FnOnce(usize, u64) -> Result<()>,
) -> Result<(usize, u64)> {
    cancel.check()?;
    if bytes.len() as u64 > limits.max_input_bytes() {
        return Err(limit("native ZIP input limit exceeded"));
    }
    let declared = end_records::declared_entries(bytes, 8)
        .map_err(|_| malformed("invalid native ZIP end records"))?;
    // A legacy fallback can allocate a 16-bit entry table before the actual
    // count is checked. Its fixed maximum is bounded; all ZIP64 reservations
    // are checked before constructing the reader.
    if declared.first > limits.max_entries() as u64
        || declared.largest_zip64 > limits.max_entries() as u64
    {
        return Err(limit("native ZIP entry limit exceeded"));
    }
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| malformed(e.to_string()))?;
    if zip.len() > limits.max_entries() || zip.len() as u64 != declared.first {
        return Err(limit("native ZIP count differs from bounded end record"));
    }
    let mut total = 0u64;
    for i in 0..zip.len() {
        cancel.check()?;
        let entry = zip.by_index(i).map_err(|e| malformed(e.to_string()))?;
        let size = entry.size();
        total = total
            .checked_add(size)
            .ok_or_else(|| limit("native ZIP expanded byte overflow"))?;
        if total > limits.max_expanded_bytes()
            || size
                > entry
                    .compressed_size()
                    .max(1)
                    .saturating_mul(limits.max_ratio())
        {
            return Err(limit("native ZIP expansion limit exceeded"));
        }
    }
    // Commit the entire declared allowance before decompression. The caller's
    // counters remain charged even if a later size/CRC check fails.
    reserve(zip.len(), total)?;
    for i in 0..zip.len() {
        cancel.check()?;
        let mut entry = zip.by_index(i).map_err(|e| malformed(e.to_string()))?;
        let size = entry.size();
        let read = std::io::copy(
            &mut CancellableReader(&mut entry, cancel).take(size.saturating_add(1)),
            &mut std::io::sink(),
        )
        .map_err(|e| malformed(e.to_string()))?;
        if read != size {
            return Err(malformed("native ZIP declared size mismatch"));
        }
    }
    cancel.check()?;
    Ok((zip.len(), total))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_interrupts_between_chunks() {
        struct CancelAfterRead(Cancellation);
        impl Read for CancelAfterRead {
            fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
                bytes.fill(1);
                self.0.cancel();
                Ok(bytes.len())
            }
        }
        let cancel = Cancellation::default();
        let mut reader = CancelAfterRead(cancel.clone());
        let mut wrapped = CancellableReader(&mut reader, &cancel);
        let mut bytes = vec![0; 128 << 10];
        assert_eq!(wrapped.read(&mut bytes).unwrap(), 64 << 10);
        assert!(wrapped.read(&mut bytes).is_err());
    }
}
