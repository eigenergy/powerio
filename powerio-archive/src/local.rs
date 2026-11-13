//! Local headers and descriptors must agree with the indexed member.
use crate::{
    ArchiveEntry, Result, malformed,
    preflight::{u16_at, u32_at, u64_at},
};

pub(crate) fn validate(
    bytes: &[u8],
    entry: &ArchiveEntry,
    name: &[u8],
    central: usize,
) -> Result<(usize, usize)> {
    let header = bytes
        .get(entry.header..central)
        .filter(|h| h.len() >= 30 && h.starts_with(b"PK\x03\x04"))
        .ok_or_else(|| malformed("missing or misplaced ZIP local header"))?;
    let name_len = usize::from(u16_at(header, 26));
    let extra_len = usize::from(u16_at(header, 28));
    let header_len = 30 + name_len + extra_len;
    if header_len > header.len()
        || header.get(30..30 + name_len) != Some(name)
        || u16_at(header, 6) != entry.flags
        || u16_at(header, 8) != entry.method
        || entry.header.checked_add(header_len) != Some(entry.data)
    {
        return Err(malformed("ZIP local and central member headers disagree"));
    }
    let mut sizes = [u64::from(u32_at(header, 22)), u64::from(u32_at(header, 18))];
    let mut extra = &header[30 + name_len..header_len];
    let mut zip64 = false;
    while !extra.is_empty() {
        if extra.len() < 4 {
            return Err(malformed("truncated ZIP local extra field"));
        }
        let n = usize::from(u16_at(extra, 2));
        let value = extra
            .get(4..4 + n)
            .ok_or_else(|| malformed("ZIP local extra field exceeds its header"))?;
        if u16_at(extra, 0) == 1 {
            if zip64 {
                return Err(malformed("duplicate ZIP64 local extra field"));
            }
            zip64 = true;
            let mut offset = 0;
            for size in &mut sizes {
                if *size == u64::from(u32::MAX) {
                    let wide = value
                        .get(offset..offset + 8)
                        .ok_or_else(|| malformed("truncated ZIP64 local size"))?;
                    *size = u64_at(wide, 0);
                    offset += 8;
                }
            }
        }
        extra = &extra[4 + n..];
    }
    let end = entry
        .data
        .checked_add(
            usize::try_from(entry.compressed)
                .map_err(|_| malformed("invalid ZIP compressed size"))?,
        )
        .filter(|&end| end <= central)
        .ok_or_else(|| malformed("ZIP member leaves the local data region"))?;
    if entry.flags & 8 == 0 {
        if sizes != [entry.size, entry.compressed] || u32_at(header, 14) != entry.crc {
            return Err(malformed(
                "ZIP local sizes or CRC disagree with central directory",
            ));
        }
        return Ok((entry.header, end));
    }
    if sizes
        .iter()
        .zip([entry.size, entry.compressed])
        .any(|(&local, indexed)| local != 0 && local != indexed)
        || ![0, entry.crc].contains(&u32_at(header, 14))
    {
        return Err(malformed(
            "streaming ZIP local sizes or CRC contradict its index",
        ));
    }
    let descriptor = &bytes[end..central];
    let width =
        if zip64 || entry.size > u64::from(u32::MAX) || entry.compressed > u64::from(u32::MAX) {
            8
        } else {
            4
        };
    let matches = |offset: usize| -> Option<usize> {
        let record = descriptor.get(offset..offset + 4 + 2 * width)?;
        let values = if width == 8 {
            [u64_at(record, 4), u64_at(record, 12)]
        } else {
            [u64::from(u32_at(record, 4)), u64::from(u32_at(record, 8))]
        };
        (u32_at(record, 0) == entry.crc && values == [entry.compressed, entry.size])
            .then_some(end + offset + record.len())
    };
    let with_signature = descriptor
        .starts_with(b"PK\x07\x08")
        .then(|| matches(4))
        .flatten();
    let without_signature = matches(0);
    let ((Some(tail), None) | (None, Some(tail))) = (without_signature, with_signature) else {
        return Err(malformed(
            "missing, ambiguous or inconsistent ZIP data descriptor",
        ));
    };
    Ok((entry.header, tail))
}

pub(crate) fn validate_regions(regions: &mut [(usize, usize)], central: usize) -> Result<()> {
    regions.sort_unstable();
    if regions.first().map_or(central != 0, |region| region.0 != 0) {
        return Err(crate::unsupported(
            "prepended data and self-extracting ZIPs are unsupported",
        ));
    }
    if regions.windows(2).any(|pair| pair[1].0 < pair[0].1) {
        return Err(crate::unsafe_archive(
            "ZIP members have overlapping header/data regions",
        ));
    }
    Ok(())
}
