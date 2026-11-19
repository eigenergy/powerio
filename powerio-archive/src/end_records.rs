//! Compatibility preflight for adapters using ZIP reader fallback.
//! This bounds metadata allocation; it does not validate payloads or names.

use memchr::memmem;

const END_RECORD: &[u8] = b"PK\x05\x06";
const ZIP64_END_LOCATOR: &[u8] = b"PK\x06\x07";
const ZIP64_END_RECORD: &[u8] = b"PK\x06\x06";

/// The entry counts an archive's end records declare.
#[derive(Debug, PartialEq, Eq)]
pub struct DeclaredEntries {
    /// The count of the end record zip 8.6 examines first: the last end
    /// record signature whose comment fits in the archive, or the Zip64
    /// record its locator names when a 16 bit field is saturated.
    pub first: u64,
    /// The largest count of any Zip64 record an end record's locator names,
    /// zero when there is none.
    pub largest_zip64: u64,
}

/// Why an archive's end records were refused before opening it.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum EndRecordRefusal {
    /// The reader would accept no end record.
    Missing,
    /// The archive holds more than `max_end_records` end records.
    TooMany,
}

/// What one end record declares.
enum EndCount {
    /// The record's own 16 bit count.
    Zip32(u64),
    /// The largest count of the Zip64 records its locator names.
    Zip64(u64),
}

/// Read every end record zip 8.6 may examine, last first.
///
/// These checks are a subset of those the reader applies before it reserves
/// a table, so it may reject a record accepted here and fall back to an
/// earlier one. The count comparison in the adapter then refuses the
/// archive unless the counts agree, and every Zip64 count the reader could
/// reach that way is in `largest_zip64`.
pub fn declared_entries(
    bytes: &[u8],
    max_end_records: usize,
) -> std::result::Result<DeclaredEntries, EndRecordRefusal> {
    // memchr searches forward several times faster than in reverse.
    let signatures: Vec<usize> = memmem::find_iter(bytes, END_RECORD)
        .take(max_end_records.saturating_add(1))
        .collect();
    let mut examined = signatures.len();
    if examined > max_end_records {
        return Err(EndRecordRefusal::TooMany);
    }
    let mut first = None;
    let mut largest_zip64 = 0;
    for &at in signatures.iter().rev() {
        let count = end_record_count(bytes, at, &mut examined, max_end_records);
        if examined > max_end_records {
            return Err(EndRecordRefusal::TooMany);
        }
        let count = match count {
            Some(EndCount::Zip32(count)) => count,
            Some(EndCount::Zip64(count)) => {
                largest_zip64 = largest_zip64.max(count);
                count
            }
            None => continue,
        };
        first.get_or_insert(count);
    }
    let first = first.ok_or(EndRecordRefusal::Missing)?;
    Ok(DeclaredEntries {
        first,
        largest_zip64,
    })
}

fn end_record_count(
    bytes: &[u8],
    at: usize,
    examined: &mut usize,
    max_end_records: usize,
) -> Option<EndCount> {
    let record = bytes.get(at..at.checked_add(22)?)?;
    let comment = usize::from(u16::from_le_bytes([record[20], record[21]]));
    if at + 22 + comment > bytes.len() {
        return None;
    }
    let on_disk = le_u16(record, 8);
    let total = le_u16(record, 10);
    let directory_offset = le_u32(record, 16);
    let saturated = total == u64::from(u16::MAX)
        || le_u32(record, 12) == u64::from(u32::MAX)
        || directory_offset == u64::from(u32::MAX);
    let locator = at
        .checked_sub(20)
        .filter(|&locator| saturated && bytes[locator..].starts_with(ZIP64_END_LOCATOR));
    if let Some(locator) = locator {
        return zip64_entry_count(bytes, locator, examined, max_end_records).map(EndCount::Zip64);
    }
    (total == 0 || directory_offset < at as u64).then_some(EndCount::Zip32(on_disk))
}

/// The largest count among the Zip64 records the reader may accept for the
/// locator at `locator`, adding to `examined` each record it would read.
fn zip64_entry_count(
    bytes: &[u8],
    locator: usize,
    examined: &mut usize,
    max_end_records: usize,
) -> Option<u64> {
    let fields = &bytes[locator..locator + 20];
    let disk_with_directory = le_u32(fields, 4);
    let declared_at = le_u64(fields, 8);
    if declared_at >= locator as u64 || le_u32(fields, 16) > 1 {
        return None;
    }
    // Prepended bytes move the record forward, so the reader searches forward
    // from the declared offset up to the locator and takes the first
    // consistent record. The largest count is kept here, so that order never
    // matters.
    let from = usize::try_from(declared_at).ok()?;
    let mut largest = None;
    for found in memmem::find_iter(&bytes[from..locator], ZIP64_END_RECORD) {
        let at = from + found;
        let Some(record) = bytes.get(at..at + 56) else {
            continue;
        };
        // The reader reads a record whole, extensible data included, once
        // its stated size fits before the locator.
        let record_size = le_u64(record, 4);
        let room = (locator - at) as u64;
        if record_size < 40 || record_size.saturating_add(12) > room {
            continue;
        }
        *examined += 1;
        if *examined > max_end_records {
            return None;
        }
        let total = le_u64(record, 32);
        if record_size + 12 == room
            && le_u32(record, 20) == disk_with_directory
            && total.saturating_mul(46).saturating_add(le_u64(record, 48)) <= at as u64
            && le_u64(record, 24) <= total
        {
            largest = largest.max(Some(total));
        }
    }
    largest
}

// Record fields widened to the u64 the entry arithmetic uses.
fn le_u16(bytes: &[u8], at: usize) -> u64 {
    u64::from(crate::preflight::u16_at(bytes, at))
}

fn le_u32(bytes: &[u8], at: usize) -> u64 {
    u64::from(crate::preflight::u32_at(bytes, at))
}

fn le_u64(bytes: &[u8], at: usize) -> u64 {
    crate::preflight::u64_at(bytes, at)
}

#[cfg(test)]
mod tests {
    #[test]
    fn maximum_caller_budget_does_not_overflow() {
        assert!(super::declared_entries(b"", usize::MAX).is_err());
    }
}
