//! Bounded, model-neutral ZIP transport. See [`Archive`] for validation stages.

use std::collections::BTreeMap;
use std::io::{Cursor, Read};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use powerio_core::{ArtifactPath, Error};
use zip::{
    CompressionMethod, ZipArchive,
    read::{ArchiveOffset, Config},
};

mod local;
mod preflight;

/// Stable diagnostic identities for archive transport.
pub mod codes {
    powerio_core::diagnostic_codes! {
        INVALID_LIMIT = "REQUEST.ARCHIVE.INVALID_LIMIT", Error,
            "archive limits must be positive and fit this platform", category = Request;
        MALFORMED = "READ.ARCHIVE.MALFORMED", Error,
            "ZIP metadata or payload integrity is invalid", category = Data;
        UNSAFE = "READ.ARCHIVE.UNSAFE", Error,
            "the archive namespace or member layout is unsafe", category = Data;
        LIMIT = "READ.ARCHIVE.LIMIT", Error,
            "the archive exceeds an explicit resource limit", category = Data;
        UNSUPPORTED = "READ.ARCHIVE.UNSUPPORTED", Error,
            "the archive uses an unsupported transport feature", category = Data;
        UNKNOWN_ENTRY = "REQUEST.ARCHIVE.UNKNOWN_ENTRY", Error,
            "the selected archive member does not exist", category = Request;
    }
}

type Result<T> = std::result::Result<T, Error>;
fn malformed(message: impl Into<String>) -> Error {
    Error::new(&codes::MALFORMED, message)
}
fn unsafe_archive(message: impl Into<String>) -> Error {
    Error::new(&codes::UNSAFE, message)
}
fn limit(message: impl Into<String>) -> Error {
    Error::new(&codes::LIMIT, message)
}
fn unsupported(message: impl Into<String>) -> Error {
    Error::new(&codes::UNSUPPORTED, message)
}

/// Explicit input, entry-count, cumulative expansion, and per-member ratio caps.
/// Defaults: 64 MiB input/expanded bytes, 4096 entries, expansion ratio 200:1.
/// These limits do not read the environment or bound downstream model memory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArchiveLimits {
    input: u64,
    entries: usize,
    expanded: u64,
    ratio: u64,
}

impl Default for ArchiveLimits {
    fn default() -> Self {
        Self {
            input: 64 << 20,
            entries: 4096,
            expanded: 64 << 20,
            ratio: 200,
        }
    }
}
impl ArchiveLimits {
    /// Set positive limits. Byte counts and the index capacity must fit this
    /// platform. Paths additionally have fixed 4096-byte/64-segment bounds.
    pub fn new(input: u64, entries: usize, expanded: u64, ratio: u64) -> Result<Self> {
        if input == 0
            || entries == 0
            || expanded == 0
            || ratio == 0
            || isize::try_from(input).is_err()
            || isize::try_from(expanded).is_err()
            || entries
                .checked_mul(std::mem::size_of::<ArchiveEntry>())
                .is_none_or(|n| n > isize::MAX as usize)
        {
            return Err(Error::new(
                &codes::INVALID_LIMIT,
                "archive limits must be positive and platform sized",
            ));
        }
        Ok(Self {
            input,
            entries,
            expanded,
            ratio,
        })
    }
}

/// An indexed member. Names retain their exact UTF-8 spelling; directories
/// have their terminal slash removed. Indexing does not validate payload CRCs.
#[derive(Clone, Debug)]
pub struct ArchiveEntry {
    path: ArtifactPath,
    directory: bool,
    size: u64,
    compressed: u64,
    index: usize,
    flags: u16,
    method: u16,
    crc: u32,
    header: usize,
    data: usize,
}
impl ArchiveEntry {
    #[must_use]
    pub fn path(&self) -> &ArtifactPath {
        &self.path
    }
    #[must_use]
    pub const fn is_directory(&self) -> bool {
        self.directory
    }
    #[must_use]
    pub const fn size(&self) -> u64 {
        self.size
    }
    #[must_use]
    pub const fn compressed_size(&self) -> u64 {
        self.compressed
    }
}

/// An immutable ZIP input with a validated index and lazily verified members.
///
/// Opening validates namespace, counts, declared expansion, local headers,
/// descriptors, and nonoverlapping member regions before decompressing files.
/// Reading validates actual size and CRC before publishing/caching bytes.
/// Repeated reads return the same immutable allocation. Nothing is extracted,
/// executed, discovered as a case, or recursively opened as another archive.
///
/// The supported profile is single-volume ZIP32/ZIP64 with stored/deflated
/// members, UTF-8 paths and no prepended executable. Ambiguous metadata end
/// records, encryption, symlinks, special files and unsafe paths are refused.
#[derive(Debug)]
pub struct Archive {
    bytes: Arc<[u8]>,
    zip: ZipArchive<preflight::IndexReader>,
    entries: BTreeMap<ArtifactPath, ArchiveEntry>,
    cache: BTreeMap<ArtifactPath, Arc<[u8]>>,
}
impl Archive {
    /// Index caller-owned bytes under explicit limits without filesystem I/O.
    /// An Arc input is retained without copying. Unread payload CRCs are not
    /// certified. Expansion limits apply to the sum of all declared members.
    pub fn open(bytes: impl AsRef<[u8]> + Into<Arc<[u8]>>, limits: ArchiveLimits) -> Result<Self> {
        if bytes.as_ref().len() as u64 > limits.input {
            return Err(limit("ZIP input byte limit exceeded"));
        }
        let bytes = bytes.into();
        let directory = preflight::inspect(&bytes, limits)?;
        let indexing = Arc::new(AtomicBool::new(true));
        let reader = preflight::IndexReader {
            cursor: Cursor::new(Arc::clone(&bytes)),
            indexing: Arc::clone(&indexing),
            central_start: directory.start,
        };
        let mut zip = ZipArchive::with_config(
            Config {
                archive_offset: ArchiveOffset::Known(0),
            },
            reader,
        )
        .map_err(|e| malformed(format!("cannot index ZIP: {e}")))?;
        indexing.store(false, Ordering::Relaxed);
        if zip.len() != directory.headers.len()
            || zip.central_directory_start() != directory.start as u64
        {
            return Err(malformed(
                "ZIP index disagrees with its bounded central directory",
            ));
        }
        let mut entries = BTreeMap::new();
        let mut nodes = BTreeMap::new();
        let mut expanded = 0u64;
        let mut regions = Vec::new();
        for (index, &(flags, method)) in directory.headers.iter().enumerate() {
            let file = zip
                .by_index_raw(index)
                .map_err(|e| malformed(format!("invalid ZIP member: {e}")))?;
            validate_type(&file, flags)?;
            let name = std::str::from_utf8(file.name_raw())
                .map_err(|_| unsupported("ZIP member names must be UTF-8"))?;
            if file.name().map_err(|e| malformed(e.to_string()))?.as_ref() != name {
                return Err(unsafe_archive(
                    "ZIP Unicode name override disagrees with its raw name",
                ));
            }
            let is_dir = file.is_dir();
            expanded = expanded
                .checked_add(file.size())
                .ok_or_else(|| limit("ZIP expansion size overflow"))?;
            if expanded > limits.expanded {
                return Err(limit("ZIP cumulative expanded byte limit exceeded"));
            }
            if file.size() > 0
                && (file.compressed_size() == 0
                    || file.size() > file.compressed_size().saturating_mul(limits.ratio))
            {
                return Err(limit("ZIP member compression ratio exceeded"));
            }
            let path = validate_path(name, is_dir, &mut nodes)?;
            let entry = ArchiveEntry {
                path: path.clone(),
                directory: is_dir,
                size: file.size(),
                compressed: file.compressed_size(),
                index,
                flags,
                method,
                crc: file.crc32(),
                header: usize::try_from(file.header_start())
                    .map_err(|_| malformed("invalid local header offset"))?,
                data: file
                    .data_start()
                    .and_then(|n| usize::try_from(n).ok())
                    .ok_or_else(|| malformed("invalid member data offset"))?,
            };
            regions.push(local::validate(
                &bytes,
                &entry,
                name.as_bytes(),
                directory.start,
            )?);
            entries.insert(path, entry);
        }
        local::validate_regions(&mut regions, directory.start)?;
        Ok(Self {
            bytes,
            zip,
            entries,
            cache: BTreeMap::new(),
        })
    }

    /// Members in deterministic path order, including explicit directories.
    pub fn entries(&self) -> impl ExactSizeIterator<Item = &ArchiveEntry> {
        self.entries.values()
    }

    /// Exact original ZIP bytes, including its comment and compression data.
    #[must_use]
    pub fn original_bytes(&self) -> Arc<[u8]> {
        Arc::clone(&self.bytes)
    }

    /// Read and CRC-verify a member, caching only a complete valid payload.
    pub fn read(&mut self, path: &ArtifactPath) -> Result<Arc<[u8]>> {
        self.read_with_limit(path, u64::MAX)
    }

    /// Apply an additional per-read byte cap, including on cached members.
    /// A refusal does not populate the cache. Zero permits only empty members.
    pub fn read_with_limit(&mut self, path: &ArtifactPath, max_bytes: u64) -> Result<Arc<[u8]>> {
        let entry = self.entries.get(path).ok_or_else(|| {
            Error::new(
                &codes::UNKNOWN_ENTRY,
                format!("unknown ZIP member `{path}`"),
            )
        })?;
        if entry.size > max_bytes {
            return Err(limit("ZIP member exceeds the caller's byte limit"));
        }
        if let Some(bytes) = self.cache.get(path) {
            return Ok(Arc::clone(bytes));
        }
        let mut file = self
            .zip
            .by_index(entry.index)
            .map_err(|e| malformed(format!("cannot open ZIP member `{path}`: {e}")))?;
        let mut content = Vec::new();
        content
            .try_reserve_exact(
                usize::try_from(entry.size)
                    .map_err(|_| limit("member length exceeds this platform"))?,
            )
            .map_err(|_| limit("cannot reserve ZIP member bytes"))?;
        content.resize(entry.size as usize, 0);
        file.read_exact(&mut content)
            .map_err(|e| malformed(format!("cannot verify ZIP member `{path}`: {e}")))?;
        // Force EOF even for zero-length members, so CRC validation runs.
        if file
            .read(&mut [0u8; 1])
            .map_err(|e| malformed(e.to_string()))?
            != 0
        {
            return Err(malformed("ZIP member inflates beyond its declared size"));
        }
        let content: Arc<[u8]> = content.into();
        self.cache.insert(path.clone(), Arc::clone(&content));
        Ok(content)
    }
}

// Store segments rather than every full ancestor prefix: namespace storage
// stays linear in supplied name bytes even for deep, disjoint paths.
struct NameNode {
    id: usize,
    spelling: String,
    directory: bool,
    explicit: bool,
}

fn validate_path(
    name: &str,
    directory: bool,
    nodes: &mut BTreeMap<(usize, String), NameNode>,
) -> Result<ArtifactPath> {
    let name = if directory {
        name.strip_suffix('/').unwrap_or(name)
    } else {
        name
    };
    let path =
        ArtifactPath::new(name).map_err(|_| unsafe_archive("ZIP member has an unsafe path"))?;
    if name.split('/').count() > 64 {
        return Err(limit("ZIP member path depth exceeded"));
    }
    let parts: Vec<_> = name.split('/').collect();
    let mut parent = 0;
    for (index, part) in parts.iter().enumerate() {
        let stem = part
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let device_number = stem
            .strip_prefix("com")
            .or_else(|| stem.strip_prefix("lpt"))
            .is_some_and(|suffix| {
                suffix.chars().count() == 1
                    && suffix
                        .chars()
                        .all(|c| c.is_ascii_digit() || matches!(c, '¹' | '²' | '³'))
            });
        if part.ends_with(['.', ' '])
            || part.contains(['<', '>', '"', '|', '?', '*'])
            || matches!(stem.as_str(), "con" | "prn" | "aux" | "nul")
            || device_number
        {
            return Err(unsafe_archive(
                "ZIP path is not portable across supported platforms",
            ));
        }
        let is_directory = directory || index + 1 < parts.len();
        let next_id = nodes.len() + 1;
        let node = nodes
            .entry((parent, part.to_lowercase()))
            .or_insert_with(|| NameNode {
                id: next_id,
                spelling: (*part).to_owned(),
                directory: is_directory,
                explicit: false,
            });
        if node.spelling != *part || node.directory != is_directory {
            return Err(unsafe_archive(
                "ZIP file/directory or ancestor case collision",
            ));
        }
        if index + 1 == parts.len() {
            if node.explicit {
                return Err(unsafe_archive("duplicate ZIP member path"));
            }
            node.explicit = true;
        }
        parent = node.id;
    }
    Ok(path)
}

fn validate_type(file: &zip::read::ZipFile<'_, impl Read>, flags: u16) -> Result<()> {
    if file.encrypted() || flags & (1 | 64 | 8192) != 0 {
        return Err(unsupported("encrypted ZIP members are unsupported"));
    }
    if !matches!(
        file.compression(),
        CompressionMethod::Stored | CompressionMethod::Deflated
    ) {
        return Err(unsupported(
            "only stored and deflated ZIP members are supported",
        ));
    }
    if let Some(mode) = file.unix_mode() {
        let kind = mode & 0o170_000;
        if !matches!(kind, 0 | 0o100_000 | 0o040_000)
            || (kind != 0 && (kind == 0o040_000) != file.is_dir())
        {
            return Err(unsafe_archive(
                "ZIP links and special files are unsupported",
            ));
        }
    }
    if file.is_dir() && file.size() != 0 {
        return Err(unsafe_archive("directory member contains file data"));
    }
    Ok(())
}
