use std::borrow::Cow;
use std::collections::BTreeSet;
use std::io::{Cursor, Read};

use crate::{Result, format_error};

pub const MAX_BYTES: u64 = 64 << 20;
const MAX_ENTRIES: usize = 4096;
const MAX_RATIO: u64 = 200;
// Match the core Source path/depth limits before archive ancestry checks.
const MAX_PATH_BYTES: usize = 4096;
const MAX_PATH_DEPTH: usize = 64;
pub const SQLITE_MAGIC: &[u8] = b"SQLite format 3\0";

pub fn database_bytes(bytes: &[u8]) -> Result<Cow<'_, [u8]>> {
    if bytes.len() as u64 > MAX_BYTES {
        return Err(format_error("input exceeds 64 MiB limit"));
    }
    if bytes.starts_with(SQLITE_MAGIC) {
        return Ok(Cow::Borrowed(bytes));
    }
    let (mut archive, index) = validated_archive(bytes)?;
    let mut entry = archive.by_index(index).map_err(format_error)?;
    Ok(Cow::Owned(read_bounded(&mut entry)?))
}

pub(crate) fn validated_archive(bytes: &[u8]) -> Result<(zip::ZipArchive<Cursor<&[u8]>>, usize)> {
    if bytes.len() as u64 > MAX_BYTES {
        return Err(format_error("input exceeds 64 MiB limit"));
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(format_error)?;
    if archive.len() > MAX_ENTRIES {
        return Err(format_error("archive entry limit exceeded"));
    }
    let mut names = BTreeSet::new();
    let mut files = BTreeSet::new();
    let mut expanded = 0_u64;
    let mut database = None;
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(format_error)?;
        let name = entry.name();
        let path = name.strip_suffix('/').unwrap_or(name);
        if path.len() > MAX_PATH_BYTES || path.split('/').count() > MAX_PATH_DEPTH {
            return Err(format_error("archive path length/depth limit exceeded"));
        }
        if path.is_empty()
            || path.contains(['\\', ':', '\0'])
            || path.split('/').any(|part| matches!(part, "" | "." | ".."))
        {
            return Err(format_error(format!("unsafe archive path: {name:?}")));
        }
        // Treat case collisions and file/directory collisions as ambiguous on
        // every platform, not only on case-insensitive host filesystems.
        if !names.insert(path.to_lowercase()) {
            return Err(format_error(format!("duplicate archive path: {name:?}")));
        }
        if !entry.is_dir() {
            files.insert(path.to_lowercase());
        }
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170_000 == 0o120_000)
        {
            return Err(format_error("archive symbolic links are not supported"));
        }
        if entry.encrypted() {
            return Err(format_error("encrypted archives are not supported"));
        }
        expanded = expanded
            .checked_add(entry.size())
            .ok_or_else(|| format_error("archive expanded byte limit exceeded"))?;
        if expanded > MAX_BYTES {
            return Err(format_error("archive expanded byte limit exceeded"));
        }
        if entry.size() > entry.compressed_size().max(1).saturating_mul(MAX_RATIO) {
            return Err(format_error("archive compression ratio exceeded"));
        }
        let parts: Vec<_> = name.split('/').collect();
        if parts.len() == 2
            && parts[0].ends_with("_files")
            && parts[1] == "database.db"
            && database.replace(index).is_some()
        {
            return Err(format_error("ambiguous native database in archive"));
        }
    }
    for name in &names {
        let mut child = name.as_str();
        while let Some((parent, _)) = child.rsplit_once('/') {
            if files.contains(parent) {
                return Err(format_error("archive file is also a parent directory"));
            }
            child = parent;
        }
    }
    let marker = read_entry(&mut archive, "SIArchive.ini")?;
    validate_marker(&marker)?;
    let index = database.ok_or_else(|| format_error("missing native SQLite database"))?;
    Ok((archive, index))
}

fn read_entry(archive: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str) -> Result<Vec<u8>> {
    let mut entry = archive.by_name(name).map_err(format_error)?;
    read_bounded(&mut entry)
}

pub(crate) fn read_bounded(reader: &mut impl Read) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(format_error)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(format_error("expanded payload exceeds 64 MiB limit"));
    }
    Ok(bytes)
}

fn validate_marker(bytes: &[u8]) -> Result<()> {
    let text = std::str::from_utf8(bytes).map_err(format_error)?;
    let mut main = false;
    let mut fields = std::collections::BTreeMap::new();
    for line in text.trim_start_matches('\u{feff}').lines().map(str::trim) {
        if line.starts_with('[') {
            main = line == "[Main]";
        } else if main
            && !line.starts_with([';', '#'])
            && let Some((key, value)) = line.split_once('=')
            && fields.insert(key.trim(), value.trim()).is_some()
        {
            return Err(format_error("duplicate archive marker field"));
        }
    }
    if fields.get("AppVersion") != Some(&"PSS SINCAL")
        || fields.get("NetworkType") != Some(&"Electro")
    {
        return Err(format_error("not a SINCAL electrical archive"));
    }
    Ok(())
}
