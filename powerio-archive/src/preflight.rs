//! Bound the decoder's metadata work before it constructs its index.

use std::io::{self, Cursor, Read, Seek, SeekFrom};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use crate::{ArchiveLimits, Result, limit, malformed, unsupported};

pub(crate) fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(bytes[at..at + 2].try_into().expect("checked record"))
}
pub(crate) fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("checked record"))
}
pub(crate) fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().expect("checked record"))
}

pub(crate) struct Directory {
    pub start: usize,
    pub headers: Vec<(u16, u16)>,
}

fn usize_at(value: u64) -> Result<usize> {
    usize::try_from(value).map_err(|_| limit("ZIP offset exceeds this platform"))
}

pub(crate) fn inspect(bytes: &[u8], limits: ArchiveLimits) -> Result<Directory> {
    let mut end = None;
    // A terminal end record has a 16-bit comment length. Requiring its comment
    // to end at EOF rules out trailing data and ambiguous terminal candidates.
    for at in bytes.len().saturating_sub(65_557)..bytes.len().saturating_sub(21) {
        if bytes[at..].starts_with(b"PK\x05\x06")
            && at + 22 + usize::from(u16_at(bytes, at + 20)) == bytes.len()
            && end.replace(at).is_some()
        {
            return Err(malformed("ambiguous terminal ZIP end records"));
        }
    }
    let end = end.ok_or_else(|| malformed("missing terminal ZIP end record"))?;
    if u16_at(bytes, end + 4) != 0 || u16_at(bytes, end + 6) != 0 {
        return Err(unsupported("multi-volume ZIP archives are unsupported"));
    }
    let mut count = u64::from(u16_at(bytes, end + 10));
    let mut size = u64::from(u32_at(bytes, end + 12));
    let mut offset = u64::from(u32_at(bytes, end + 16));
    let mut footer = end;
    if end >= 20 && bytes[end - 20..].starts_with(b"PK\x06\x07") {
        let locator = end - 20;
        if u32_at(bytes, locator + 4) != 0 || u32_at(bytes, locator + 16) != 1 {
            return Err(unsupported("multi-volume ZIP64 archives are unsupported"));
        }
        footer = usize_at(u64_at(bytes, locator + 8))?;
        let record = bytes
            .get(footer..locator)
            .filter(|record| record.len() >= 56 && record.starts_with(b"PK\x06\x06"))
            .ok_or_else(|| malformed("invalid ZIP64 end-record location"))?;
        if u64_at(record, 4).checked_add(12) != Some(record.len() as u64)
            || u32_at(record, 16) != 0
            || u32_at(record, 20) != 0
            || u64_at(record, 24) != u64_at(record, 32)
        {
            return Err(malformed("inconsistent ZIP64 end record"));
        }
        let values = [u64_at(record, 32), u64_at(record, 40), u64_at(record, 48)];
        for (legacy, wide, sentinel) in [
            (count, values[0], u64::from(u16::MAX)),
            (size, values[1], u64::from(u32::MAX)),
            (offset, values[2], u64::from(u32::MAX)),
        ] {
            if legacy != sentinel && legacy != wide {
                return Err(malformed("ZIP32 and ZIP64 end records disagree"));
            }
        }
        [count, size, offset] = values;
    } else if count == u64::from(u16::MAX)
        || size == u64::from(u32::MAX)
        || offset == u64::from(u32::MAX)
    {
        return Err(malformed("missing ZIP64 end records"));
    }
    if u16_at(bytes, end + 8) != u16_at(bytes, end + 10) {
        return Err(malformed("end-record entry counts disagree"));
    }
    if count > limits.entries as u64 {
        return Err(limit("ZIP entry-count limit exceeded"));
    }
    let start = usize_at(offset)?;
    if offset.checked_add(size) != Some(footer as u64) || start > footer {
        return Err(malformed("central directory does not meet its end records"));
    }
    // The index reader below masks member data, so embedded ZIP signatures in
    // opaque payloads cannot trigger decoder fallback. Metadata has one EOCD.
    if bytes[start..]
        .windows(4)
        .enumerate()
        .any(|(at, magic)| magic == b"PK\x05\x06" && start + at != end)
    {
        return Err(malformed("ambiguous ZIP end record in metadata"));
    }
    let mut at = start;
    let mut headers = Vec::new();
    headers
        .try_reserve_exact(usize_at(count)?)
        .map_err(|_| limit("cannot reserve ZIP index"))?;
    for _ in 0..count {
        let header = bytes
            .get(at..footer)
            .filter(|h| h.len() >= 46 && h.starts_with(b"PK\x01\x02"))
            .ok_or_else(|| malformed("missing central-directory member header"))?;
        if u16_at(header, 34) != 0 {
            return Err(unsupported("multi-volume ZIP member"));
        }
        let name = usize::from(u16_at(header, 28));
        if name == 0 || name > 4096 {
            return Err(limit("ZIP member path length limit exceeded"));
        }
        headers.push((u16_at(header, 8), u16_at(header, 10)));
        at += 46 + name + usize::from(u16_at(header, 30)) + usize::from(u16_at(header, 32));
        if at > footer {
            return Err(malformed("central-directory member exceeds its bounds"));
        }
        validate_extra(&header[46 + name..46 + name + usize::from(u16_at(header, 30))])?;
    }
    if at != footer {
        return Err(malformed(
            "central-directory length or entry count disagrees",
        ));
    }
    Ok(Directory { start, headers })
}

fn validate_extra(mut fields: &[u8]) -> Result<()> {
    let mut zip64 = false;
    while !fields.is_empty() {
        if fields.len() < 4 {
            return Err(malformed("truncated ZIP central extra field"));
        }
        let length = 4 + usize::from(u16_at(fields, 2));
        if length > fields.len() {
            return Err(malformed("ZIP central extra field exceeds its header"));
        }
        if u16_at(fields, 0) == 1 {
            if zip64 {
                return Err(malformed("duplicate ZIP64 central extra field"));
            }
            zip64 = true;
        }
        fields = &fields[length..];
    }
    Ok(())
}

/// While ZIP indexes metadata, bytes before the central directory read as
/// zero. This hides all fallback end-record candidates inside opaque payloads
/// without copying or rewriting those payloads. Local headers and member data
/// are visible again before validation/reads; no unsafe metadata transplant.
#[derive(Debug)]
pub(crate) struct IndexReader {
    pub cursor: Cursor<Arc<[u8]>>,
    pub indexing: Arc<AtomicBool>,
    pub central_start: usize,
}

impl Read for IndexReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let start = usize::try_from(self.cursor.position()).unwrap_or(usize::MAX);
        let n = self.cursor.read(output)?;
        if self.indexing.load(Ordering::Relaxed) {
            let hidden = self.central_start.saturating_sub(start).min(n);
            output[..hidden].fill(0);
        }
        Ok(n)
    }
}
impl Seek for IndexReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.cursor.seek(position)
    }
}
