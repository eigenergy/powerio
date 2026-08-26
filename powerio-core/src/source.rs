use std::collections::BTreeMap;
use std::fmt;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::validation::{MAX_FORMAT_ID_BYTES, valid_nonempty_text};
use crate::{Error, SourceId};

/// Referenced files one source may acquire. Matches the OpenDSS include
/// budget the distribution reader has enforced since 0.7.
const MAX_REFERENCED_FILES: usize = 4_096;

/// Total bytes of referenced files one source may acquire.
const MAX_REFERENCED_BYTES: u64 = 64 << 20;

const UTF8_BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];

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
    /// Directory of this buffer relative to the acquisition root, as the
    /// segments a referenced name resolves against. Empty for memory buffers
    /// and for files sitting directly in the root.
    directory: Box<[Box<str>]>,
}

/// Immutable named bytes retained by a [`Source`].
#[derive(Clone, Debug)]
pub struct SourceBuffer(Arc<SourceBufferData>);

impl SourceBuffer {
    fn new(
        id: SourceId,
        name: impl Into<String>,
        bytes: Arc<[u8]>,
        directory: Vec<String>,
    ) -> Self {
        Self(Arc::new(SourceBufferData {
            id,
            name: name.into().into_boxed_str(),
            bytes,
            directory: directory.into_iter().map(String::into_boxed_str).collect(),
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

    /// The exact retained bytes, including a UTF-8 byte order mark when the
    /// input carried one. Same format writing echoes these bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.0.bytes
    }

    #[must_use]
    pub fn shared_bytes(&self) -> Arc<[u8]> {
        Arc::clone(&self.0.bytes)
    }

    /// True when the retained bytes begin with a UTF-8 byte order mark.
    #[must_use]
    pub fn has_utf8_bom(&self) -> bool {
        self.0.bytes.starts_with(&UTF8_BOM)
    }

    /// The bytes a parser decodes: the retained bytes with a leading UTF-8
    /// byte order mark skipped. This is a subslice of the one retained buffer,
    /// never a second decoded copy.
    #[must_use]
    pub fn content_bytes(&self) -> &[u8] {
        let bytes: &[u8] = &self.0.bytes;
        if bytes.starts_with(&UTF8_BOM) {
            &bytes[UTF8_BOM.len()..]
        } else {
            bytes
        }
    }

    fn directory_segments(&self) -> impl Iterator<Item = &str> {
        self.0.directory.iter().map(AsRef::as_ref)
    }
}

/// Files acquired beneath one pinned root directory.
///
/// The root is held as an open directory handle where the platform supports
/// it, and referenced names are resolved lexically first and then walked one
/// component at a time relative to that handle with symbolic links refused, so
/// a path component replaced during acquisition cannot redirect the read
/// outside the root. Acquisition is serialized by the one lock, which also
/// makes the cache and the budget exact: a file is read and retained once, and
/// concurrent requests for one name share the same buffer.
#[derive(Debug)]
struct FileAcquisition {
    root: platform::RootHandle,
    root_display: PathBuf,
    state: Mutex<AcquisitionState>,
}

#[derive(Debug, Default)]
struct AcquisitionState {
    cache: BTreeMap<String, SourceBuffer>,
    files: usize,
    bytes: u64,
}

#[derive(Debug)]
enum SourceProvider {
    Memory {
        primary: SourceBuffer,
        named: BTreeMap<String, SourceBuffer>,
    },
    File {
        primary: SourceBuffer,
        acquisition: FileAcquisition,
    },
    Directory {
        acquisition: FileAcquisition,
    },
}

/// Opaque owner or provider of named immutable input buffers.
///
/// File acquisition policy belongs here rather than to parser entry points.
/// [`Source::open`] on a file retains the primary bytes and permits
/// constrained acquisition of referenced files beneath the file's canonical
/// containing directory; [`Source::with_acquisition_root`] widens that root at
/// construction, and never from a parser. [`Source::from_bytes`] grants no
/// filesystem access; referenced content reaches an in-memory source only
/// through [`Source::with_named_buffer`].
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
        let name: Arc<str> = path.to_string_lossy().into_owned().into();

        // The open itself refuses a symbolic link at the named path, so there
        // is no metadata inspection that a concurrent replacement could
        // invalidate before the read; the file or directory decision reads
        // the already opened handle.
        let file = match platform::open_no_follow(&path) {
            Ok(file) => file,
            Err(error) if platform::is_symlink_refusal(&error) => {
                return Err(Error::new(
                    &crate::codes::REQUEST_SOURCE_SYMLINK_REFUSED,
                    format!("source `{}` is a symbolic link", path.display()),
                ));
            }
            Err(error) if platform::is_directory_open_failure(&error, &path) => {
                return Self::open_directory(name, &path);
            }
            Err(cause) => return Err(open_error(&crate::codes::READ_IO_OPEN, &path, cause)),
        };
        let metadata = file
            .metadata()
            .map_err(|cause| open_error(&crate::codes::READ_IO_METADATA, &path, cause))?;
        if metadata.is_dir() {
            drop(file);
            return Self::open_directory(name, &path);
        }
        let bytes = read_open_file(file, &name)?;
        let root_display = canonical_parent(&path)?;
        let root = platform::open_root(&root_display)
            .map_err(|cause| open_error(&crate::codes::READ_IO_OPEN, &root_display, cause))?;
        let primary =
            SourceBuffer::new(SourceId::new("input")?, name.to_string(), bytes, Vec::new());
        Ok(Self {
            name,
            provider: Arc::new(SourceProvider::File {
                primary,
                acquisition: FileAcquisition {
                    root,
                    root_display,
                    state: Mutex::new(AcquisitionState::default()),
                },
            }),
            declared_format: None,
        })
    }

    fn open_directory(name: Arc<str>, path: &Path) -> Result<Self, Error> {
        let root_display = std::fs::canonicalize(path)
            .map_err(|cause| open_error(&crate::codes::READ_IO_METADATA, path, cause))?;
        let root = platform::open_root(&root_display)
            .map_err(|cause| open_error(&crate::codes::READ_IO_OPEN, &root_display, cause))?;
        Ok(Self {
            name,
            provider: Arc::new(SourceProvider::Directory {
                acquisition: FileAcquisition {
                    root,
                    root_display,
                    state: Mutex::new(AcquisitionState::default()),
                },
            }),
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
        let primary = SourceBuffer::new(
            SourceId::new("input")?,
            name.clone(),
            bytes.into(),
            Vec::new(),
        );
        Ok(Self {
            name: name.into(),
            provider: Arc::new(SourceProvider::Memory {
                primary,
                named: BTreeMap::new(),
            }),
            declared_format: None,
        })
    }

    /// Supply one referenced buffer to an in-memory source under the relative
    /// name a format uses to refer to it. This is the only way referenced
    /// content reaches a source built by [`Source::from_bytes`]; such a source
    /// never touches the filesystem.
    pub fn with_named_buffer(
        self,
        name: impl Into<String>,
        bytes: impl Into<Arc<[u8]>>,
    ) -> Result<Self, Error> {
        let name = name.into();
        let segments = resolve_segments(&[], &name)?;
        let key = segments.join("/");
        let mut provider = Arc::try_unwrap(self.provider).map_err(|_| {
            Error::new(
                &crate::codes::REQUEST_SOURCE_INVALID_NAME,
                "named buffers are supplied while constructing a source, before it is shared",
            )
        })?;
        let SourceProvider::Memory { named, .. } = &mut provider else {
            return Err(Error::new(
                &crate::codes::REQUEST_SOURCE_INVALID_NAME,
                "named buffers belong to in-memory sources; a file source acquires referenced files beneath its root",
            ));
        };
        let directory = segments[..segments.len() - 1].to_vec();
        let buffer = SourceBuffer::new(SourceId::new(&key)?, key.clone(), bytes.into(), directory);
        named.insert(key, buffer);
        Ok(Self {
            name: self.name,
            provider: Arc::new(provider),
            declared_format: self.declared_format,
        })
    }

    /// Widen the acquisition root of a file source to a directory that
    /// contains the file, selected while constructing the source. A parser
    /// can never widen the root.
    pub fn with_acquisition_root(self, root: impl Into<PathBuf>) -> Result<Self, Error> {
        let requested = root.into();
        let SourceProvider::File {
            primary,
            acquisition,
        } = &*self.provider
        else {
            return Err(Error::new(
                &crate::codes::REQUEST_SOURCE_INVALID_PATH,
                "an acquisition root applies to a file source",
            ));
        };
        let canonical = std::fs::canonicalize(&requested)
            .map_err(|cause| open_error(&crate::codes::READ_IO_METADATA, &requested, cause))?;
        let Ok(remainder) = acquisition.root_display.strip_prefix(&canonical) else {
            return Err(Error::new(
                &crate::codes::REQUEST_SOURCE_INVALID_PATH,
                format!(
                    "the case file directory {} is outside the requested acquisition root {}",
                    acquisition.root_display.display(),
                    canonical.display()
                ),
            ));
        };
        let mut directory = Vec::new();
        for component in remainder.components() {
            let Component::Normal(segment) = component else {
                return Err(Error::new(
                    &crate::codes::REQUEST_SOURCE_INVALID_PATH,
                    "the acquisition root does not resolve to a plain prefix of the case directory",
                ));
            };
            directory.push(segment.to_string_lossy().into_owned());
        }
        let root = platform::open_root(&canonical)
            .map_err(|cause| open_error(&crate::codes::READ_IO_OPEN, &canonical, cause))?;
        let primary = SourceBuffer::new(
            primary.id().clone(),
            primary.name().to_owned(),
            primary.shared_bytes(),
            directory,
        );
        Ok(Self {
            name: self.name,
            provider: Arc::new(SourceProvider::File {
                primary,
                acquisition: FileAcquisition {
                    root,
                    root_display: canonical,
                    state: Mutex::new(AcquisitionState::default()),
                },
            }),
            declared_format: self.declared_format,
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
        matches!(&*self.provider, SourceProvider::Directory { .. })
    }

    /// Borrow the sole primary buffer of a file or memory source.
    pub fn primary_buffer(&self) -> Result<SourceBuffer, Error> {
        match &*self.provider {
            SourceProvider::Memory { primary, .. } | SourceProvider::File { primary, .. } => {
                Ok(primary.clone())
            }
            SourceProvider::Directory { .. } => Err(Error::new(
                &crate::codes::REQUEST_SOURCE_DIRECTORY_REQUIRED,
                "a directory source has no implicit primary buffer",
            )
            .with_source(self.clone())),
        }
    }

    /// Acquire and retain one file of a directory source by its root relative
    /// name.
    pub fn buffer(&self, name: &crate::ArtifactPath) -> Result<SourceBuffer, Error> {
        let SourceProvider::Directory { acquisition } = &*self.provider else {
            return Err(Error::new(
                &crate::codes::REQUEST_SOURCE_DIRECTORY_REQUIRED,
                "named child buffers require a directory source",
            )
            .with_source(self.clone()));
        };
        let segments = resolve_segments(&[], name.as_str())
            .map_err(|error| error.with_source(self.clone()))?;
        acquisition
            .acquire(&segments)
            .map_err(|error| error.with_source(self.clone()))
    }

    /// Acquire and retain one file referenced by `referrer`, resolved against
    /// the referring file's directory and confined beneath the acquisition
    /// root. An in-memory source resolves the same name against the buffers
    /// the caller supplied and never touches the filesystem.
    pub fn referenced_buffer(
        &self,
        referrer: &SourceBuffer,
        name: &str,
    ) -> Result<SourceBuffer, Error> {
        let referrer_directory: Vec<&str> = referrer.directory_segments().collect();
        match &*self.provider {
            SourceProvider::Memory { named, .. } => {
                let segments = resolve_segments(&referrer_directory, name)?;
                let key = segments.join("/");
                named.get(&key).cloned().ok_or_else(|| {
                    Error::new(
                        &crate::codes::REQUEST_SOURCE_UNKNOWN_BUFFER,
                        format!(
                            "referenced buffer `{key}` was not supplied to this in-memory source"
                        ),
                    )
                    .with_source(self.clone())
                })
            }
            SourceProvider::File { acquisition, .. }
            | SourceProvider::Directory { acquisition } => {
                let segments = match absolute_to_root_relative(&acquisition.root_display, name) {
                    Some(root_relative) => root_relative?,
                    None => resolve_segments(&referrer_directory, name)?,
                };
                acquisition
                    .acquire(&segments)
                    .map_err(|error| error.with_source(self.clone()))
            }
        }
    }

    /// Buffers already retained by this source, in deterministic name order.
    #[must_use]
    pub fn acquired_buffers(&self) -> Vec<SourceBuffer> {
        match &*self.provider {
            SourceProvider::Memory { primary, named } => {
                let mut buffers = vec![primary.clone()];
                buffers.extend(named.values().cloned());
                buffers
            }
            SourceProvider::File {
                primary,
                acquisition,
            } => {
                let mut buffers = vec![primary.clone()];
                buffers.extend(acquisition.lock().cache.values().cloned());
                buffers
            }
            SourceProvider::Directory { acquisition } => {
                acquisition.lock().cache.values().cloned().collect()
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

impl FileAcquisition {
    fn lock(&self) -> MutexGuard<'_, AcquisitionState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Read and retain one file beneath the root. The lock is held across the
    /// read so a file is read once, retained once, and charged once even under
    /// concurrent requests.
    fn acquire(&self, segments: &[String]) -> Result<SourceBuffer, Error> {
        let key = segments.join("/");
        let mut state = self.lock();
        if let Some(buffer) = state.cache.get(&key) {
            return Ok(buffer.clone());
        }
        if state.files >= MAX_REFERENCED_FILES {
            return Err(Error::new(
                &crate::codes::READ_IO_REFERENCE_BUDGET,
                format!("this source already acquired {MAX_REFERENCED_FILES} referenced files"),
            ));
        }
        let file = self.root.open_beneath(segments).map_err(|error| {
            if platform::is_symlink_refusal(&error) {
                Error::new(
                    &crate::codes::REQUEST_SOURCE_SYMLINK_REFUSED,
                    format!("referenced file `{key}` crosses a symbolic link"),
                )
            } else {
                Error::new(
                    &crate::codes::READ_IO_OPEN,
                    format!("cannot open referenced file `{key}`"),
                )
                .with_cause(error)
            }
        })?;
        let bytes = read_open_file(file, &key)?;
        let remaining = MAX_REFERENCED_BYTES.saturating_sub(state.bytes);
        if bytes.len() as u64 > remaining {
            return Err(Error::new(
                &crate::codes::READ_IO_REFERENCE_BUDGET,
                format!(
                    "referenced file `{key}` would take this source past its {MAX_REFERENCED_BYTES} byte acquisition budget"
                ),
            ));
        }
        state.files += 1;
        state.bytes += bytes.len() as u64;
        let directory = segments[..segments.len() - 1].to_vec();
        let buffer = SourceBuffer::new(SourceId::new(&key)?, key.clone(), bytes, directory);
        state.cache.insert(key, buffer.clone());
        Ok(buffer)
    }
}

/// Resolve a referenced name against the referring directory, lexically and
/// before any filesystem access: `.` is dropped, `..` pops within the root and
/// is refused past it, and both separators are rejected inside one segment by
/// construction. The result is the exact component list the platform walk
/// opens one at a time.
fn resolve_segments(referrer_directory: &[&str], name: &str) -> Result<Vec<String>, Error> {
    if name.is_empty()
        || name.len() > crate::validation::MAX_ARTIFACT_PATH_BYTES
        || name.contains('\0')
    {
        return Err(Error::new(
            &crate::codes::REQUEST_SOURCE_INVALID_PATH,
            "a referenced name must be a nonempty bounded path",
        ));
    }
    let mut segments: Vec<String> = referrer_directory
        .iter()
        .map(|segment| (*segment).to_owned())
        .collect();
    for raw in name.split('/') {
        match raw {
            "" | "." => {}
            ".." => {
                if segments.pop().is_none() {
                    return Err(Error::new(
                        &crate::codes::REQUEST_SOURCE_ESCAPES_ROOT,
                        format!("referenced name `{name}` resolves outside the acquisition root"),
                    ));
                }
            }
            segment => {
                if segment.len() > crate::validation::MAX_ARTIFACT_PATH_BYTES {
                    return Err(Error::new(
                        &crate::codes::REQUEST_SOURCE_INVALID_PATH,
                        "a referenced name component exceeds its bound",
                    ));
                }
                segments.push(segment.to_owned());
            }
        }
    }
    if segments.is_empty() {
        return Err(Error::new(
            &crate::codes::REQUEST_SOURCE_INVALID_PATH,
            format!("referenced name `{name}` does not name a file"),
        ));
    }
    Ok(segments)
}

/// An absolute referenced name is accepted only when it sits lexically beneath
/// the canonical root; the walk then reopens it component by component from
/// the pinned root handle. Returns `None` for a relative name.
fn absolute_to_root_relative(root: &Path, name: &str) -> Option<Result<Vec<String>, Error>> {
    let path = Path::new(name);
    if !path.is_absolute() {
        return None;
    }
    let Ok(remainder) = path.strip_prefix(root) else {
        return Some(Err(Error::new(
            &crate::codes::REQUEST_SOURCE_ESCAPES_ROOT,
            format!("referenced name `{name}` resolves outside the acquisition root"),
        )));
    };
    Some(resolve_segments(&[], &remainder.to_string_lossy()))
}

fn canonical_parent(path: &Path) -> Result<PathBuf, Error> {
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    std::fs::canonicalize(parent)
        .map_err(|cause| open_error(&crate::codes::READ_IO_METADATA, parent, cause))
}

fn open_error(info: &'static crate::DiagnosticInfo, path: &Path, cause: std::io::Error) -> Error {
    Error::new(info, format!("cannot open source `{}`", path.display())).with_cause(cause)
}

/// Read an already opened regular file completely. The handle was opened with
/// symbolic links refused, and the regular file check runs on the open
/// descriptor, so no path is consulted twice.
fn read_open_file(file: std::fs::File, name: &str) -> Result<Arc<[u8]>, Error> {
    let metadata = file.metadata().map_err(|cause| {
        Error::new(
            &crate::codes::READ_IO_METADATA,
            format!("cannot inspect source buffer `{name}`"),
        )
        .with_cause(cause)
    })?;
    if !metadata.is_file() {
        return Err(Error::new(
            &crate::codes::REQUEST_SOURCE_INVALID_PATH,
            format!("source buffer `{name}` is not a regular file"),
        ));
    }
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
    let mut file = file;
    file.by_ref()
        .take(read_limit)
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
    Ok(bytes.into())
}

#[cfg(unix)]
mod platform {
    //! Descriptor-relative acquisition: the root directory is pinned by an
    //! open descriptor at construction, and every referenced component is
    //! opened relative to it with `O_NOFOLLOW`, so replacing a component with
    //! a symbolic link during acquisition fails instead of redirecting the
    //! read outside the root.

    use std::ffi::CString;
    use std::fs::File;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    #[derive(Debug)]
    pub(super) struct RootHandle(OwnedFd);

    /// Open the named path with symbolic links at the final component
    /// refused by the kernel.
    pub(super) fn open_no_follow(path: &Path) -> std::io::Result<File> {
        let path = c_string(path.as_os_str().as_bytes())?;
        // SAFETY: the pointer references a NUL-terminated buffer owned by
        // `path`, which outlives the call; the returned descriptor is owned
        // exclusively by the `File` constructed below.
        let fd = unsafe {
            libc::open(
                path.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `fd` is a freshly opened descriptor this function owns.
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    pub(super) fn open_root(path: &Path) -> std::io::Result<RootHandle> {
        let path = c_string(path.as_os_str().as_bytes())?;
        // SAFETY: as in `open_no_follow`; `O_DIRECTORY` refuses anything but
        // a directory atomically.
        let fd = unsafe {
            libc::open(
                path.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `fd` is a freshly opened descriptor this function owns.
        Ok(RootHandle(unsafe { OwnedFd::from_raw_fd(fd) }))
    }

    impl RootHandle {
        pub(super) fn open_beneath(&self, segments: &[String]) -> std::io::Result<File> {
            let mut directory: Option<OwnedFd> = None;
            let (file_segment, directories) =
                segments.split_last().expect("resolution yields a file");
            for segment in directories {
                // `O_NOFOLLOW` alone, then a directory check on the opened
                // descriptor: Darwin reports `O_NOFOLLOW | O_DIRECTORY` on a
                // symbolic link as `ENOTDIR`, which would hide the refusal
                // reason, and checking the descriptor cannot race.
                let next = self.open_at(
                    directory.as_ref(),
                    segment,
                    libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )?;
                let next = File::from(next);
                if !next.metadata()?.is_dir() {
                    return Err(std::io::Error::from(std::io::ErrorKind::NotADirectory));
                }
                directory = Some(next.into());
            }
            let fd = self.open_at(
                directory.as_ref(),
                file_segment,
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )?;
            Ok(File::from(fd))
        }

        fn open_at(
            &self,
            directory: Option<&OwnedFd>,
            segment: &str,
            flags: libc::c_int,
        ) -> std::io::Result<OwnedFd> {
            let at = directory.map_or_else(|| self.0.as_raw_fd(), AsRawFd::as_raw_fd);
            let segment = c_string(segment.as_bytes())?;
            // SAFETY: `at` is a live descriptor owned by `self` or by the
            // caller's `directory` for the duration of the call, and the
            // pointer references a NUL-terminated buffer owned by `segment`.
            let fd = unsafe { libc::openat(at, segment.as_ptr(), flags) };
            if fd < 0 {
                return Err(std::io::Error::last_os_error());
            }
            // SAFETY: `fd` is a freshly opened descriptor this function owns.
            Ok(unsafe { OwnedFd::from_raw_fd(fd) })
        }
    }

    pub(super) fn is_symlink_refusal(error: &std::io::Error) -> bool {
        matches!(error.raw_os_error(), Some(libc::ELOOP | libc::EMLINK))
    }

    pub(super) fn is_directory_open_failure(error: &std::io::Error, path: &Path) -> bool {
        // Opening a directory read-only succeeds on Linux and macOS, so the
        // usual route is the metadata check on the opened handle; this covers
        // a platform whose open refuses directories outright.
        let _ = path;
        error.raw_os_error() == Some(libc::EISDIR)
    }

    fn c_string(bytes: &[u8]) -> std::io::Result<CString> {
        CString::new(bytes).map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))
    }
}

#[cfg(not(unix))]
mod platform {
    //! Windows and other platforms have no `openat`. The walk opens each
    //! component by an incrementally extended absolute path with reparse
    //! points refused on the opened handle, which refuses a symbolic link at
    //! any component; a parent directory swapped between two component opens
    //! remains detectable only by the next component's no-follow open. This is
    //! the documented platform implementation, not the descriptor pinned walk
    //! Unix uses.

    use std::fs::File;
    use std::path::{Path, PathBuf};

    #[derive(Debug)]
    pub(super) struct RootHandle {
        root: PathBuf,
    }

    pub(super) fn open_no_follow(path: &Path) -> std::io::Result<File> {
        let file = open_reparse_refused(path)?;
        Ok(file)
    }

    pub(super) fn open_root(path: &Path) -> std::io::Result<RootHandle> {
        let metadata = std::fs::symlink_metadata(path)?;
        if !metadata.is_dir() {
            return Err(std::io::Error::from(std::io::ErrorKind::NotADirectory));
        }
        Ok(RootHandle {
            root: path.to_path_buf(),
        })
    }

    impl RootHandle {
        pub(super) fn open_beneath(&self, segments: &[String]) -> std::io::Result<File> {
            let mut path = self.root.clone();
            let (file_segment, directories) =
                segments.split_last().expect("resolution yields a file");
            for segment in directories {
                path.push(segment);
                let metadata = std::fs::symlink_metadata(&path)?;
                if metadata.file_type().is_symlink() {
                    return Err(symlink_error());
                }
                if !metadata.is_dir() {
                    return Err(std::io::Error::from(std::io::ErrorKind::NotADirectory));
                }
            }
            path.push(file_segment);
            open_reparse_refused(&path)
        }
    }

    #[cfg(windows)]
    fn open_reparse_refused(path: &Path) -> std::io::Result<File> {
        use std::os::windows::fs::{MetadataExt, OpenOptionsExt};

        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        let attributes = file.metadata()?.file_attributes();
        if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(symlink_error());
        }
        Ok(file)
    }

    #[cfg(not(windows))]
    fn open_reparse_refused(path: &Path) -> std::io::Result<File> {
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() {
            return Err(symlink_error());
        }
        File::open(path)
    }

    fn symlink_error() -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "symbolic link refused")
    }

    pub(super) fn is_symlink_refusal(error: &std::io::Error) -> bool {
        error.kind() == std::io::ErrorKind::InvalidData
            && error.to_string().contains("symbolic link refused")
    }

    pub(super) fn is_directory_open_failure(error: &std::io::Error, path: &Path) -> bool {
        let _ = error;
        std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir())
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::ArtifactPath;

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
    fn a_bom_is_retained_and_skipped_for_the_parser_without_a_second_buffer() {
        let bytes: Vec<u8> = [0xEF, 0xBB, 0xBF, b'm', b'p', b'c'].to_vec();
        let source = Source::from_bytes("case.m", bytes).unwrap();
        let buffer = source.primary_buffer().unwrap();
        assert!(buffer.has_utf8_bom());
        assert_eq!(buffer.bytes().len(), 6);
        assert_eq!(buffer.content_bytes(), b"mpc");
        // The parser slice points into the one retained buffer.
        assert_eq!(
            buffer.content_bytes().as_ptr(),
            buffer.bytes()[3..].as_ptr()
        );

        let plain = Source::from_bytes("case.m", b"mpc".to_vec()).unwrap();
        let plain = plain.primary_buffer().unwrap();
        assert!(!plain.has_utf8_bom());
        assert_eq!(plain.content_bytes(), plain.bytes());
    }

    #[test]
    fn a_memory_source_resolves_named_buffers_and_never_the_filesystem() {
        let source = Source::from_bytes("master.dss", b"redirect sub/feeder.dss".to_vec())
            .unwrap()
            .with_named_buffer("sub/feeder.dss", b"feeder".to_vec())
            .unwrap();
        let primary = source.primary_buffer().unwrap();
        let feeder = source
            .referenced_buffer(&primary, "sub/feeder.dss")
            .unwrap();
        assert_eq!(feeder.bytes(), b"feeder");

        // Referrer-relative resolution: from inside `sub/`, a sibling name
        // resolves beneath `sub/` and `..` climbs within the supplied names.
        let sibling = source
            .referenced_buffer(&feeder, "../sub/feeder.dss")
            .unwrap();
        assert_eq!(sibling.bytes(), b"feeder");

        assert!(source.referenced_buffer(&primary, "missing.dss").is_err());
        let escape = source.referenced_buffer(&primary, "../outside.dss");
        assert_eq!(
            escape.unwrap_err().category(),
            crate::ErrorCategory::Request
        );
    }

    #[test]
    fn a_file_source_acquires_referenced_files_beneath_its_containing_directory() {
        let root = test_root("file-refs");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("master.dss"), b"master").unwrap();
        std::fs::write(root.join("sub/feeder.dss"), b"feeder").unwrap();
        std::fs::write(root.join("sub/coords.csv"), b"coords").unwrap();

        let source = Source::open(root.join("master.dss")).unwrap();
        let primary = source.primary_buffer().unwrap();
        assert_eq!(primary.bytes(), b"master");

        let feeder = source
            .referenced_buffer(&primary, "sub/feeder.dss")
            .unwrap();
        assert_eq!(feeder.bytes(), b"feeder");
        // Names resolve against the referring file: from the feeder, a bare
        // sibling name lands in `sub/`.
        let coords = source.referenced_buffer(&feeder, "coords.csv").unwrap();
        assert_eq!(coords.bytes(), b"coords");
        // And `..` climbs back toward the root but never past it.
        let master_again = source.referenced_buffer(&feeder, "../master.dss").unwrap();
        assert_eq!(master_again.bytes(), b"master");
        let escape = source.referenced_buffer(&primary, "../escape.dss");
        assert_eq!(
            escape.unwrap_err().category(),
            crate::ErrorCategory::Request
        );

        // The same file is retained once.
        let again = source
            .referenced_buffer(&primary, "sub/feeder.dss")
            .unwrap();
        assert_eq!(again.bytes().as_ptr(), feeder.bytes().as_ptr());
        assert_eq!(source.acquired_buffers().len(), 4);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_explicitly_wider_root_admits_shared_files_and_still_confines() {
        let root = test_root("wider-root");
        std::fs::create_dir_all(root.join("cases")).unwrap();
        std::fs::create_dir_all(root.join("shared")).unwrap();
        std::fs::write(root.join("cases/master.dss"), b"master").unwrap();
        std::fs::write(root.join("shared/wires.dss"), b"wires").unwrap();

        // Confined by default: the sibling directory is outside.
        let narrow = Source::open(root.join("cases/master.dss")).unwrap();
        let primary = narrow.primary_buffer().unwrap();
        assert!(
            narrow
                .referenced_buffer(&primary, "../shared/wires.dss")
                .is_err()
        );

        // The wider root is selected while constructing the source.
        let wide = Source::open(root.join("cases/master.dss"))
            .unwrap()
            .with_acquisition_root(&root)
            .unwrap();
        let primary = wide.primary_buffer().unwrap();
        let wires = wide
            .referenced_buffer(&primary, "../shared/wires.dss")
            .unwrap();
        assert_eq!(wires.bytes(), b"wires");
        assert!(
            wide.referenced_buffer(&primary, "../../etc/passwd")
                .is_err()
        );

        // A root that does not contain the case file is refused.
        let outside = test_root("wider-root-outside");
        std::fs::create_dir_all(&outside).unwrap();
        assert!(
            Source::open(root.join("cases/master.dss"))
                .unwrap()
                .with_acquisition_root(&outside)
                .is_err()
        );
        std::fs::remove_dir_all(outside).ok();
        std::fs::remove_dir_all(root).unwrap();
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

        // A symlinked intermediate directory is refused by the component walk.
        std::fs::create_dir_all(root.join("real-dir")).unwrap();
        std::fs::write(root.join("real-dir/inner.bin"), b"inner").unwrap();
        symlink(root.join("real-dir"), root.join("link-dir")).unwrap();
        let error = source
            .buffer(&ArtifactPath::new("link-dir/inner.bin").unwrap())
            .unwrap_err();
        assert_eq!(error.category(), crate::ErrorCategory::Request);

        let root_link = root.with_extension("link");
        symlink(&root, &root_link).unwrap();
        assert!(Source::open(&root_link).is_err());
        std::fs::remove_file(root_link).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_acquisition_retains_one_buffer_for_one_name() {
        let root = test_root("concurrent");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("shared.csv"), b"shared").unwrap();
        let source = Source::open(&root).unwrap();
        let name = ArtifactPath::new("shared.csv").unwrap();
        let buffers: Vec<_> = std::thread::scope(|scope| {
            (0..8)
                .map(|_| {
                    let source = source.clone();
                    let name = name.clone();
                    scope.spawn(move || source.buffer(&name).unwrap())
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect()
        });
        let pointer = buffers[0].bytes().as_ptr();
        assert!(
            buffers
                .iter()
                .all(|buffer| buffer.bytes().as_ptr() == pointer)
        );
        assert_eq!(source.acquired_buffers().len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }
}
