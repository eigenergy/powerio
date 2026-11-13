//! Bounded payload reads shared by native format adapters.

use std::io::Read;

/// Why an entry's inflated stream was refused.
#[derive(Debug)]
#[non_exhaustive]
pub enum DeclaredSizeFailure {
    /// The reader or inflater failed, including a CRC mismatch.
    Read(std::io::Error),
    /// The declared size does not fit in memory on this platform.
    Allocation,
    /// The stream ended after this many bytes, before the declared size.
    Short(u64),
    /// The stream produced more bytes than declared or than the bound allows.
    Long,
}

impl DeclaredSizeFailure {
    fn from_read_error(error: std::io::Error) -> Self {
        // zip 9.x rejects an inflater that produces more bytes than the
        // central directory declared before our one-byte overrun probe runs.
        // Preserve the reader's stable malformed-archive diagnostic instead
        // of leaking the ZIP implementation's internal error text.
        if error
            .to_string()
            .contains("larger than its declared uncompressed size")
        {
            Self::Long
        } else {
            Self::Read(error)
        }
    }
}

/// Inflate exactly the `size` bytes an entry declares, after the `prefix`
/// already read from it. One more byte is requested to confirm that the
/// stream ends there, which is also when the reader checks the CRC. A stream
/// longer or shorter than declared is refused, so the declared size the
/// ratio and budget checks used is the size produced, and the buffer is
/// reserved once at that size.
pub fn read_declared_size(
    entry: &mut impl Read,
    prefix: &[u8],
    size: u64,
) -> std::result::Result<Vec<u8>, DeclaredSizeFailure> {
    if prefix.len() as u64 > size {
        return Err(DeclaredSizeFailure::Long);
    }
    let capacity = usize::try_from(size).map_err(|_| DeclaredSizeFailure::Allocation)?;
    let mut content = Vec::new();
    content
        .try_reserve_exact(capacity)
        .map_err(|_| DeclaredSizeFailure::Allocation)?;
    content.extend_from_slice(prefix);
    entry
        .by_ref()
        .take(size - prefix.len() as u64)
        .read_to_end(&mut content)
        .map_err(DeclaredSizeFailure::from_read_error)?;
    let produced = content.len() as u64;
    if produced != size {
        return Err(DeclaredSizeFailure::Short(produced));
    }
    if entry
        .read(&mut [0_u8; 1])
        .map_err(DeclaredSizeFailure::from_read_error)?
        != 0
    {
        return Err(DeclaredSizeFailure::Long);
    }
    Ok(content)
}

/// Read through EOF under an actual expanded-byte cap, including a one-byte
/// overrun probe. Does not trust a declared ZIP size.
pub fn read_bounded(
    reader: &mut impl Read,
    max_bytes: u64,
) -> Result<Vec<u8>, DeclaredSizeFailure> {
    let mut bytes = Vec::new();
    reader
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(DeclaredSizeFailure::Read)?;
    if bytes.len() as u64 > max_bytes {
        return Err(DeclaredSizeFailure::Long);
    }
    Ok(bytes)
}
