use std::collections::BTreeMap;
use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::validation::{MAX_FORMAT_ID_BYTES, valid_nonempty_text};
use crate::{ArtifactPath, Error, SourceId};

/// Open stable identifier used to select a parser or writer.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FormatId(Box<str>);

impl FormatId {
    /// Validate the shared C, Python, Julia, JSON, MCP, and Rust spelling.
    pub fn new(id: impl Into<String>) -> Result<Self, Error> {
        let id = id.into();
        if !valid_format_id(&id) {
            return Err(Error::new(
                &crate::codes::REQUEST_FORMAT_INVALID_ID,
                "a format ID must be bounded lower case ASCII segments separated by single hyphens",
            ));
        }
        Ok(Self(id.into_boxed_str()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for FormatId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

fn valid_format_id(id: &str) -> bool {
    if id.is_empty() || id.len() > MAX_FORMAT_ID_BYTES {
        return false;
    }
    let bytes = id.as_bytes();
    if !bytes[0].is_ascii_lowercase() || bytes.last() == Some(&b'-') {
        return false;
    }
    let mut previous_hyphen = false;
    for byte in bytes {
        if *byte == b'-' {
            if previous_hyphen {
                return false;
            }
            previous_hyphen = true;
        } else if byte.is_ascii_lowercase() || byte.is_ascii_digit() {
            previous_hyphen = false;
        } else {
            return false;
        }
    }
    true
}

#[derive(Debug)]
struct SourceBufferData {
    id: SourceId,
    name: Box<str>,
    bytes: Arc<[u8]>,
}

/// Immutable named bytes retained by a [`Source`].
#[derive(Clone, Debug)]
pub struct SourceBuffer(Arc<SourceBufferData>);

impl SourceBuffer {
    fn new(id: SourceId, name: impl Into<String>, bytes: Arc<[u8]>) -> Self {
        Self(Arc::new(SourceBufferData {
            id,
            name: name.into().into_boxed_str(),
            bytes,
        }))
    }

    #[must_use]
    pub fn id(&self) -> &SourceId {
        &self.0.id
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.0.name
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.0.bytes
    }

    #[must_use]
    pub fn shared_bytes(&self) -> Arc<[u8]> {
        Arc::clone(&self.0.bytes)
    }
}

#[derive(Debug)]
struct DirectoryProvider {
    root: PathBuf,
    cache: Mutex<BTreeMap<ArtifactPath, SourceBuffer>>,
}

#[derive(Debug)]
enum SourceProvider {
    Single(SourceBuffer),
    Directory(DirectoryProvider),
}

/// Opaque owner or provider of named immutable input buffers.
#[derive(Clone)]
pub struct Source {
    name: Arc<str>,
    provider: Arc<SourceProvider>,
    declared_format: Option<FormatId>,
}

impl Source {
    /// Acquire a file eagerly or a directory lazily.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, Error> {
        let path = path.into();
        if path.as_os_str().is_empty() {
            return Err(Error::new(
                &crate::codes::REQUEST_SOURCE_INVALID_PATH,
                "source path cannot be empty",
            ));
        }
        let metadata = std::fs::symlink_metadata(&path).map_err(|cause| {
            Error::new(
                &crate::codes::READ_IO_METADATA,
                format!("cannot inspect source `{}`", path.display()),
            )
            .with_cause(cause)
        })?;
        if metadata.file_type().is_symlink() {
            return Err(Error::new(
                &crate::codes::REQUEST_SOURCE_SYMLINK_REFUSED,
                format!("source `{}` is a symbolic link", path.display()),
            ));
        }
        let name: Arc<str> = path.to_string_lossy().into_owned().into();
        let provider = if metadata.is_file() {
            let buffer = read_regular_file(&path, SourceId::new("input")?, name.to_string())?;
            SourceProvider::Single(buffer)
        } else if metadata.is_dir() {
            SourceProvider::Directory(DirectoryProvider {
                root: path,
                cache: Mutex::new(BTreeMap::new()),
            })
        } else {
            return Err(Error::new(
                &crate::codes::REQUEST_SOURCE_INVALID_PATH,
                "source path must name a regular file or directory",
            ));
        };
        Ok(Self {
            name,
            provider: Arc::new(provider),
            declared_format: None,
        })
    }

    /// Retain a caller-owned binary or text buffer without copying it.
    pub fn from_bytes(name: impl Into<String>, bytes: impl Into<Arc<[u8]>>) -> Result<Self, Error> {
        let name = name.into();
        if !valid_nonempty_text(&name) {
            return Err(Error::new(
                &crate::codes::REQUEST_SOURCE_INVALID_NAME,
                "an in-memory source requires a nonempty bounded name",
            ));
        }
        let buffer = SourceBuffer::new(SourceId::new("input")?, name.clone(), bytes.into());
        Ok(Self {
            name: name.into(),
            provider: Arc::new(SourceProvider::Single(buffer)),
            declared_format: None,
        })
    }

    /// Select one parser explicitly while retaining the same source owner.
    #[must_use]
    pub fn with_format(mut self, format: FormatId) -> Self {
        self.declared_format = Some(format);
        self
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn format(&self) -> Option<&FormatId> {
        self.declared_format.as_ref()
    }

    #[must_use]
    pub fn is_directory(&self) -> bool {
        matches!(&*self.provider, SourceProvider::Directory(_))
    }

    /// Borrow the sole buffer of a file or memory source.
    pub fn primary_buffer(&self) -> Result<SourceBuffer, Error> {
        match &*self.provider {
            SourceProvider::Single(buffer) => Ok(buffer.clone()),
            SourceProvider::Directory(_) => Err(Error::new(
                &crate::codes::REQUEST_SOURCE_DIRECTORY_REQUIRED,
                "a directory source has no implicit primary buffer",
            )
            .with_source(self.clone())),
        }
    }

    /// Acquire and retain one relative child of a directory source.
    pub fn buffer(&self, name: &ArtifactPath) -> Result<SourceBuffer, Error> {
        let SourceProvider::Directory(directory) = &*self.provider else {
            return Err(Error::new(
                &crate::codes::REQUEST_SOURCE_DIRECTORY_REQUIRED,
                "named child buffers require a directory source",
            )
            .with_source(self.clone()));
        };
        if let Some(buffer) = lock_cache(&directory.cache).get(name).cloned() {
            return Ok(buffer);
        }
        let path = checked_directory_child(&directory.root, name)
            .map_err(|error| error.with_source(self.clone()))?;
        let buffer = read_regular_file(
            &path,
            SourceId::new(name.as_str())?,
            name.as_str().to_owned(),
        )
        .map_err(|error| error.with_source(self.clone()))?;
        let mut cache = lock_cache(&directory.cache);
        Ok(cache
            .entry(name.clone())
            .or_insert_with(|| buffer.clone())
            .clone())
    }

    /// Buffers already retained by this source, in deterministic name order.
    #[must_use]
    pub fn acquired_buffers(&self) -> Vec<SourceBuffer> {
        match &*self.provider {
            SourceProvider::Single(buffer) => vec![buffer.clone()],
            SourceProvider::Directory(directory) => {
                lock_cache(&directory.cache).values().cloned().collect()
            }
        }
    }
}

impl fmt::Debug for Source {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Source")
            .field("name", &self.name)
            .field("is_directory", &self.is_directory())
            .field("declared_format", &self.declared_format)
            .field("acquired_buffer_count", &self.acquired_buffers().len())
            .finish_non_exhaustive()
    }
}

fn lock_cache(
    cache: &Mutex<BTreeMap<ArtifactPath, SourceBuffer>>,
) -> std::sync::MutexGuard<'_, BTreeMap<ArtifactPath, SourceBuffer>> {
    cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn checked_directory_child(root: &Path, name: &ArtifactPath) -> Result<PathBuf, Error> {
    let mut path = root.to_path_buf();
    let segment_count = name.as_str().split('/').count();
    for (index, segment) in name.as_str().split('/').enumerate() {
        path.push(segment);
        let metadata = std::fs::symlink_metadata(&path).map_err(|cause| {
            Error::new(
                &crate::codes::READ_IO_METADATA,
                format!("cannot inspect source buffer `{}`", name.as_str()),
            )
            .with_cause(cause)
        })?;
        if metadata.file_type().is_symlink() {
            return Err(Error::new(
                &crate::codes::REQUEST_SOURCE_SYMLINK_REFUSED,
                format!("source buffer `{}` crosses a symbolic link", name.as_str()),
            ));
        }
        let final_segment = index + 1 == segment_count;
        if (!final_segment && !metadata.is_dir()) || (final_segment && !metadata.is_file()) {
            return Err(Error::new(
                &crate::codes::REQUEST_SOURCE_INVALID_PATH,
                format!("source buffer `{}` is not a regular file", name.as_str()),
            ));
        }
    }
    Ok(path)
}

fn read_regular_file(path: &Path, id: SourceId, name: String) -> Result<SourceBuffer, Error> {
    let file = File::open(path).map_err(|cause| {
        Error::new(
            &crate::codes::READ_IO_OPEN,
            format!("cannot open source buffer `{name}`"),
        )
        .with_cause(cause)
    })?;
    let metadata = file.metadata().map_err(|cause| {
        Error::new(
            &crate::codes::READ_IO_METADATA,
            format!("cannot inspect source buffer `{name}`"),
        )
        .with_cause(cause)
    })?;
    let declared_length = metadata.len();
    let capacity = usize::try_from(declared_length).map_err(|cause| {
        Error::new(
            &crate::codes::READ_IO_ALLOCATION_REFUSED,
            format!("source buffer `{name}` is too large for this platform"),
        )
        .with_cause(cause)
    })?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(capacity).map_err(|cause| {
        Error::new(
            &crate::codes::READ_IO_ALLOCATION_REFUSED,
            format!("cannot reserve {declared_length} bytes for source buffer `{name}`"),
        )
        .with_cause(cause)
    })?;
    let read_limit = declared_length.checked_add(1).ok_or_else(|| {
        Error::new(
            &crate::codes::READ_IO_ALLOCATION_REFUSED,
            format!("source buffer `{name}` is too large to read safely"),
        )
    })?;
    file.take(read_limit)
        .read_to_end(&mut bytes)
        .map_err(|cause| {
            Error::new(
                &crate::codes::READ_IO_READ,
                format!("cannot read source buffer `{name}`"),
            )
            .with_cause(cause)
        })?;
    if bytes.len() != capacity {
        return Err(Error::new(
            &crate::codes::READ_IO_SOURCE_CHANGED,
            format!("source buffer `{name}` changed length while it was read"),
        ));
    }
    Ok(SourceBuffer::new(id, name, bytes.into()))
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn test_root(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "powerio-core-{name}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn format_ids_use_the_exact_open_grammar() {
        for id in ["matpower", "psse-raw", "doe-go-3", "x1"] {
            assert_eq!(FormatId::new(id).unwrap().as_str(), id);
        }
        for id in ["", "1matpower", "MATPOWER", "psse--raw", "psse-", "p_sse"] {
            assert!(FormatId::new(id).is_err(), "{id}");
        }
        assert!(FormatId::new("a".repeat(MAX_FORMAT_ID_BYTES + 1)).is_err());
    }

    #[test]
    fn memory_sources_retain_arbitrary_binary_bytes_without_copying() {
        let bytes: Arc<[u8]> = vec![0, 255, 0, 128].into();
        let pointer = bytes.as_ptr();
        let source = Source::from_bytes("input.bin", Arc::clone(&bytes))
            .unwrap()
            .with_format(FormatId::new("pwb").unwrap());
        let buffer = source.primary_buffer().unwrap();
        assert_eq!(buffer.bytes(), [0, 255, 0, 128]);
        assert_eq!(buffer.bytes().as_ptr(), pointer);
        assert_eq!(source.format().unwrap().as_str(), "pwb");
        assert!(Source::from_bytes("", Vec::new()).is_err());
        assert!(Source::from_bytes("x\0y", Vec::new()).is_err());
    }

    #[test]
    fn directory_buffers_are_lazy_cached_and_binary_safe() {
        let root = test_root("directory");
        std::fs::create_dir_all(root.join("nested")).unwrap();
        std::fs::write(root.join("nested/data.bin"), [0, 255, 7]).unwrap();
        let source = Source::open(&root).unwrap();
        assert!(source.is_directory());
        assert!(source.acquired_buffers().is_empty());
        let name = ArtifactPath::new("nested/data.bin").unwrap();
        let first = source.buffer(&name).unwrap();
        let second = source.buffer(&name).unwrap();
        assert_eq!(first.bytes(), [0, 255, 7]);
        assert_eq!(first.bytes().as_ptr(), second.bytes().as_ptr());
        assert_eq!(source.acquired_buffers().len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn source_acquisition_refuses_root_and_child_symlinks() {
        use std::os::unix::fs::symlink;

        let root = test_root("symlink");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("real.bin"), b"real").unwrap();
        symlink(root.join("real.bin"), root.join("link.bin")).unwrap();
        let source = Source::open(&root).unwrap();
        let error = source
            .buffer(&ArtifactPath::new("link.bin").unwrap())
            .unwrap_err();
        assert_eq!(error.category(), crate::ErrorCategory::Request);

        let root_link = root.with_extension("link");
        symlink(&root, &root_link).unwrap();
        assert!(Source::open(&root_link).is_err());
        std::fs::remove_file(root_link).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
