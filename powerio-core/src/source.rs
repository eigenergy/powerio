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

/// Deepest resolved or walked path beneath one acquisition root, in segments.
const MAX_REFERENCED_DEPTH: usize = 64;

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

    /// Directory of this buffer relative to the acquisition root, as path
    /// segments. Empty for in-memory buffers and for files sitting directly
    /// in the root. A parser that resolves referenced names itself joins
    /// these onto the root to seed its resolution base.
    pub fn directory_segments(&self) -> impl Iterator<Item = &str> {
        self.0.directory.iter().map(AsRef::as_ref)
    }
}

/// Files acquired beneath one pinned root directory.
///
/// The root is pinned as an open directory handle where the platform supports
/// it, opened once on the first acquisition and reused for every later one, so
/// the number of descriptors a process holds does not grow with the number of
/// live sources that never acquire a referenced file. Referenced names are
/// resolved lexically first and then walked one component at a time relative
/// to that handle with symbolic links refused, so a path component replaced
/// during acquisition cannot redirect the read outside the root. Acquisition
/// is serialized by the one lock, which also makes the cache and the budget
/// exact: a file is read and retained once, and concurrent requests for one
/// name share the same buffer.
#[derive(Debug)]
struct FileAcquisition {
    root_display: PathBuf,
    /// True when the root was selected with [`Source::with_acquisition_root`]
    /// rather than defaulted to the containing directory.
    selected: bool,
    state: Mutex<AcquisitionState>,
}

#[derive(Debug, Default)]
struct AcquisitionState {
    root: Option<platform::RootHandle>,
    cache: BTreeMap<String, SourceBuffer>,
    /// The one directory listing, cached on the first successful walk. Every
    /// caller then reads the same immutable view, and a directory handle
    /// whose stream position a platform shares across duplicated descriptors
    /// cannot silently return an empty second listing.
    listed: Option<Vec<crate::ArtifactPath>>,
    files: usize,
    bytes: u64,
}

impl AcquisitionState {
    /// The pinned root handle, opened on first use with symbolic links at the
    /// root refused and the directory confirmed on the opened descriptor.
    fn pinned_root(&mut self, root_display: &Path) -> Result<&platform::RootHandle, Error> {
        if self.root.is_none() {
            let root = platform::open_root(root_display)
                .map_err(|cause| open_error(&crate::codes::READ_IO_OPEN, root_display, cause))?;
            self.root = Some(root);
        }
        Ok(self.root.as_ref().expect("pinned above"))
    }
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
/// The primary buffer's reserved identity. The leading slash is a spelling
/// [`resolve_segments`] can never produce (a referenced name must be
/// relative), so an acquired or named buffer's identity is disjoint from the
/// primary's by construction.
pub const PRIMARY_SOURCE_ID: &str = "/input";

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
        let bytes = read_open_file(file, &name, u64::MAX)?;
        let root_display = canonical_parent(&path)?;
        let primary = SourceBuffer::new(
            SourceId::new(PRIMARY_SOURCE_ID)?,
            name.to_string(),
            bytes,
            Vec::new(),
        );
        Ok(Self {
            name,
            provider: Arc::new(SourceProvider::File {
                primary,
                acquisition: FileAcquisition {
                    root_display,
                    selected: false,
                    state: Mutex::new(AcquisitionState::default()),
                },
            }),
            declared_format: None,
        })
    }

    fn open_directory(name: Arc<str>, path: &Path) -> Result<Self, Error> {
        let root_display = std::fs::canonicalize(path)
            .map_err(|cause| open_error(&crate::codes::READ_IO_METADATA, path, cause))?;
        Ok(Self {
            name,
            provider: Arc::new(SourceProvider::Directory {
                acquisition: FileAcquisition {
                    root_display,
                    selected: false,
                    state: Mutex::new(AcquisitionState::default()),
                },
            }),
            declared_format: None,
        })
    }

    /// Retain a caller-owned binary or text buffer. An `Arc<[u8]>` argument
    /// is retained without copying; a `Vec<u8>` is copied once into the
    /// shared buffer, since `Arc<[u8]>` needs its own allocation.
    pub fn from_bytes(name: impl Into<String>, bytes: impl Into<Arc<[u8]>>) -> Result<Self, Error> {
        let name = name.into();
        if !valid_nonempty_text(&name) {
            return Err(Error::new(
                &crate::codes::REQUEST_SOURCE_INVALID_NAME,
                "an in-memory source requires a nonempty bounded name",
            ));
        }
        let primary = SourceBuffer::new(
            SourceId::new(PRIMARY_SOURCE_ID)?,
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
            let Some(segment) = segment.to_str().filter(|text| plain_segment(text)) else {
                return Err(Error::new(
                    &crate::codes::REQUEST_SOURCE_INVALID_PATH,
                    "the acquisition root does not resolve to a plain prefix of the case directory",
                ));
            };
            directory.push(segment.to_owned());
        }
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
                    root_display: canonical,
                    selected: true,
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

    /// Acquire and retain one file beneath the acquisition root by its root
    /// relative name, for a parser that resolves referenced names itself and
    /// hands over the resolved result. An in-memory source consults the
    /// buffers the caller supplied.
    pub fn root_buffer(&self, name: &str) -> Result<SourceBuffer, Error> {
        match &*self.provider {
            SourceProvider::Memory { named, .. } => {
                let segments = resolve_segments(&[], name)?;
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
                let segments = resolve_segments(&[], name)?;
                acquisition
                    .acquire(&segments)
                    .map_err(|error| error.with_source(self.clone()))
            }
        }
    }

    /// The canonical root selected with [`Source::with_acquisition_root`],
    /// `None` when the root defaulted to the containing directory or the
    /// source is not file backed. Read-only context for a parser's own
    /// resolution and refusal wording; acquisition itself always goes
    /// through this source.
    #[must_use]
    pub fn selected_acquisition_root(&self) -> Option<&Path> {
        match &*self.provider {
            SourceProvider::Memory { .. } | SourceProvider::Directory { .. } => None,
            SourceProvider::File { acquisition, .. } => acquisition
                .selected
                .then_some(acquisition.root_display.as_path()),
        }
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

    /// The root relative file names of a directory source, in sorted order,
    /// so a directory format can report files outside its profile without
    /// touching the filesystem itself. Symbolic links are listed by name and
    /// refused if acquired. The listing is bounded by the referenced file
    /// budget; a directory holding more entries is refused.
    #[allow(clippy::too_many_lines)] // one bounded walk, framed and budgeted in place
    pub fn entry_names(&self) -> Result<Vec<crate::ArtifactPath>, Error> {
        match &*self.provider {
            SourceProvider::Memory { named, .. } => named
                .keys()
                .map(|name| crate::ArtifactPath::new(name.clone()))
                .collect(),
            SourceProvider::File { .. } => Err(Error::new(
                &crate::codes::REQUEST_SOURCE_DIRECTORY_REQUIRED,
                "entry listing requires a directory source",
            )
            .with_source(self.clone())),
            SourceProvider::Directory { acquisition } => {
                // The walk runs against the pinned root handle so a directory
                // component swapped for a symbolic link mid-listing fails the
                // descriptor walk rather than redirecting the listing outside
                // the root. The walk is depth first over frames, one open
                // handle per level: a child is opened from its parent's live
                // handle, and a frame's handle closes when its subdirectories
                // are exhausted, so the descriptors held at any moment are
                // bounded by the depth bound, never by the entry budget or a
                // directory's fan-out. Every budget bounds the work before it
                // is incurred: entries stop being read the moment the
                // remaining allowance is exhausted, and a prefix at the depth
                // bound is refused before it is walked. The lock is held
                // across the walk, like acquisition.
                struct Frame<Handle> {
                    prefix: Vec<String>,
                    directory: Handle,
                    /// Subdirectory names discovered and not yet descended.
                    subdirectories: Vec<String>,
                }

                let mut state = acquisition.lock();
                if let Some(listed) = &state.listed {
                    return Ok(listed.clone());
                }
                let root = state.pinned_root(&acquisition.root_display)?;
                let budget_refusal = || {
                    Error::new(
                        &crate::codes::READ_IO_REFERENCE_BUDGET,
                        format!(
                            "the source directory holds more than {MAX_REFERENCED_FILES} entries"
                        ),
                    )
                };
                let root_handle = root.duplicate_handle().map_err(|cause| {
                    Error::new(
                        &crate::codes::READ_IO_METADATA,
                        "cannot list the source directory root",
                    )
                    .with_cause(cause)
                })?;
                let mut names = Vec::new();
                // Subdirectory names discovered across every frame and not
                // yet listed; each still charges the entry budget.
                let mut undescended = 0usize;
                let mut frames = Vec::new();
                let mut arriving = Some((Vec::<String>::new(), root_handle));
                while let Some((prefix, directory)) = arriving.take() {
                    let allowance = MAX_REFERENCED_FILES
                        .checked_sub(names.len() + undescended)
                        .filter(|allowance| *allowance > 0)
                        .ok_or_else(budget_refusal)?;
                    let entries =
                        platform::list_entries(&directory, allowance).map_err(|cause| {
                            if platform::is_entry_budget(&cause) {
                                budget_refusal()
                            } else {
                                listing_error(&prefix.join("/"), cause)
                            }
                        })?;
                    let mut subdirectories = Vec::new();
                    for (name, is_directory) in entries {
                        if names.len() + undescended + subdirectories.len() >= MAX_REFERENCED_FILES
                        {
                            return Err(budget_refusal());
                        }
                        if is_directory {
                            if prefix.len() + 1 >= MAX_REFERENCED_DEPTH {
                                return Err(Error::new(
                                    &crate::codes::READ_IO_REFERENCE_BUDGET,
                                    format!(
                                        "the source directory nests more than {MAX_REFERENCED_DEPTH} levels deep"
                                    ),
                                ));
                            }
                            subdirectories.push(name);
                        } else {
                            let mut child = prefix.clone();
                            child.push(name);
                            names.push(crate::ArtifactPath::new(child.join("/"))?);
                        }
                    }
                    undescended += subdirectories.len();
                    frames.push(Frame {
                        prefix,
                        directory,
                        subdirectories,
                    });

                    // Descend into the deepest frame's next subdirectory;
                    // pop and drop a frame — closing its handle — as soon as
                    // its subdirectories are exhausted, before any sibling
                    // opens.
                    while let Some(frame) = frames.last_mut() {
                        let Some(name) = frame.subdirectories.pop() else {
                            frames.pop();
                            continue;
                        };
                        undescended -= 1;
                        let mut child = frame.prefix.clone();
                        let handle = platform::open_child_directory(&frame.directory, &name)
                            .map_err(|cause| {
                                child.push(name.clone());
                                listing_error(&child.join("/"), cause)
                            })?;
                        child.push(name);
                        arriving = Some((child, handle));
                        break;
                    }
                }
                names.sort();
                state.listed = Some(names.clone());
                Ok(names)
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
    /// concurrent requests. The remaining byte budget bounds the allocation
    /// itself: it is computed before the read and handed to the reader, so a
    /// file that would overrun the budget is refused before its bytes are
    /// reserved.
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
        let remaining = MAX_REFERENCED_BYTES.saturating_sub(state.bytes);
        let root = state.pinned_root(&self.root_display)?;
        let file = root.open_beneath(segments).map_err(|error| {
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
        let bytes = read_open_file(file, &key, remaining)?;
        let directory = segments[..segments.len() - 1].to_vec();
        let buffer = SourceBuffer::new(SourceId::new(&key)?, key.clone(), bytes, directory);
        state.files += 1;
        state.bytes += buffer.bytes().len() as u64;
        state.cache.insert(key, buffer.clone());
        Ok(buffer)
    }
}

/// True when `segment` is a single plain file name on every supported
/// platform: it reduces to exactly one normal path component equal to itself,
/// so it carries no separator of any platform, no drive or root prefix, no
/// NUL, and is neither `.` nor `..` nor empty. Joining such segments one at a
/// time onto a directory cannot reach outside that directory.
fn plain_segment(segment: &str) -> bool {
    if segment.is_empty()
        || segment == "."
        || segment == ".."
        || segment.contains(['/', '\\', '\0', ':'])
    {
        return false;
    }
    let mut components = Path::new(segment).components();
    matches!(components.next(), Some(Component::Normal(text)) if text == std::ffi::OsStr::new(segment))
        && components.next().is_none()
}

/// Resolve a referenced name against the referring directory, lexically and
/// before any filesystem access. The name is split on both separators, `.` is
/// dropped, `..` pops within the root and is refused past it, a leading
/// separator or drive spelling is refused, and every remaining segment must be
/// a single plain file name on the platform under test. The result is the
/// exact component list the platform walk opens one at a time.
fn resolve_segments(referrer_directory: &[&str], name: &str) -> Result<Vec<String>, Error> {
    if name.is_empty()
        || name.len() > crate::validation::MAX_ARTIFACT_PATH_BYTES
        || name.contains('\0')
        || name.starts_with(['/', '\\'])
    {
        return Err(Error::new(
            &crate::codes::REQUEST_SOURCE_INVALID_PATH,
            "a referenced name must be a nonempty bounded relative path",
        ));
    }
    let mut segments: Vec<String> = referrer_directory
        .iter()
        .map(|segment| (*segment).to_owned())
        .collect();
    for raw in name.split(['/', '\\']) {
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
                if segment.len() > crate::validation::MAX_ARTIFACT_PATH_BYTES
                    || !plain_segment(segment)
                {
                    return Err(Error::new(
                        &crate::codes::REQUEST_SOURCE_INVALID_PATH,
                        format!("referenced name `{name}` is not a portable relative path"),
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
    if segments.len() > MAX_REFERENCED_DEPTH {
        return Err(Error::new(
            &crate::codes::READ_IO_REFERENCE_BUDGET,
            format!("referenced name `{name}` nests more than {MAX_REFERENCED_DEPTH} levels deep"),
        ));
    }
    Ok(segments)
}

/// An absolute referenced name is accepted only when it sits lexically beneath
/// the canonical root; the walk then reopens it component by component from
/// the pinned root handle. Returns `None` for a relative name. The segment
/// list is built from the platform's own path components, one segment per
/// directory component, each required to be a plain file name.
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
    let mut segments = Vec::new();
    for component in remainder.components() {
        let text = match component {
            Component::Normal(text) => text.to_str(),
            _ => None,
        };
        let Some(text) = text.filter(|text| plain_segment(text)) else {
            return Some(Err(Error::new(
                &crate::codes::REQUEST_SOURCE_INVALID_PATH,
                format!("referenced name `{name}` is not a portable path beneath the root"),
            )));
        };
        segments.push(text.to_owned());
    }
    if segments.is_empty() {
        return Some(Err(Error::new(
            &crate::codes::REQUEST_SOURCE_INVALID_PATH,
            format!("referenced name `{name}` does not name a file"),
        )));
    }
    if segments.len() > MAX_REFERENCED_DEPTH {
        return Some(Err(Error::new(
            &crate::codes::READ_IO_REFERENCE_BUDGET,
            format!("referenced name `{name}` nests more than {MAX_REFERENCED_DEPTH} levels deep"),
        )));
    }
    Some(Ok(segments))
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

/// A listing walk failure at one root relative directory: a symbolic link is
/// the acquisition refusal; anything else is an I/O failure.
fn listing_error(display: &str, cause: std::io::Error) -> Error {
    if platform::is_symlink_refusal(&cause) {
        Error::new(
            &crate::codes::REQUEST_SOURCE_SYMLINK_REFUSED,
            format!("source directory `{display}` crosses a symbolic link"),
        )
    } else {
        Error::new(
            &crate::codes::READ_IO_METADATA,
            format!("cannot list source directory `{display}`"),
        )
        .with_cause(cause)
    }
}

/// Read an already opened regular file completely. The handle was opened with
/// symbolic links refused, and the regular file check runs on the open
/// descriptor, so no path is consulted twice. `max_bytes` bounds the
/// allocation itself: a file whose declared length exceeds it is refused
/// before any bytes are reserved, and the reader is capped so a file that
/// grows past the bound during the read is refused rather than read.
fn read_open_file(file: std::fs::File, name: &str, max_bytes: u64) -> Result<Arc<[u8]>, Error> {
    let metadata = file.metadata().map_err(|cause| {
        Error::new(
            &crate::codes::READ_IO_METADATA,
            format!("cannot inspect source buffer `{name}`"),
        )
        .with_cause(cause)
    })?;
    if !metadata.is_file() {
        return Err(Error::new(
            &crate::codes::REQUEST_SOURCE_NOT_A_FILE,
            format!("source buffer `{name}` is not a regular file"),
        ));
    }
    let declared_length = metadata.len();
    if declared_length > max_bytes {
        return Err(Error::new(
            &crate::codes::READ_IO_REFERENCE_BUDGET,
            format!(
                "referenced file `{name}` would take this source past its {MAX_REFERENCED_BYTES} byte acquisition budget"
            ),
        ));
    }
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
    let read_limit = declared_length
        .checked_add(1)
        .ok_or_else(|| {
            Error::new(
                &crate::codes::READ_IO_ALLOCATION_REFUSED,
                format!("source buffer `{name}` is too large to read safely"),
            )
        })?
        .min(max_bytes.saturating_add(1));
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
    //! open descriptor, and every referenced component is opened relative to
    //! it with `O_NOFOLLOW`, so replacing a component with a symbolic link
    //! during acquisition fails instead of redirecting the read outside the
    //! root. Every open also carries `O_NONBLOCK`, so the open call itself
    //! never waits on another process (a FIFO with no writer opens
    //! immediately and is then refused by the regular file check on the
    //! descriptor); the flag is cleared before any read.

    use std::ffi::CString;
    use std::fs::File;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    #[derive(Debug)]
    pub(super) struct RootHandle(OwnedFd);

    /// Open the named path with symbolic links at the final component refused
    /// by the kernel and without the open itself blocking on another process.
    pub(super) fn open_no_follow(path: &Path) -> std::io::Result<File> {
        let path = c_string(path.as_os_str().as_bytes())?;
        // SAFETY: the pointer references a NUL-terminated buffer owned by
        // `path`, which outlives the call; the returned descriptor is owned
        // exclusively by the `File` constructed below.
        let fd = unsafe {
            libc::open(
                path.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `fd` is a freshly opened descriptor this function owns.
        let file = unsafe { File::from_raw_fd(fd) };
        clear_nonblock(&file)?;
        Ok(file)
    }

    /// Open a root directory with a symbolic link at the final component
    /// refused, and the directory confirmed on the opened descriptor. Not
    /// `O_DIRECTORY | O_NOFOLLOW`: Darwin reports that combination on a
    /// symbolic link as `ENOTDIR`, hiding the refusal reason.
    pub(super) fn open_root(path: &Path) -> std::io::Result<RootHandle> {
        let path = c_string(path.as_os_str().as_bytes())?;
        // SAFETY: as in `open_no_follow`.
        let fd = unsafe {
            libc::open(
                path.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `fd` is a freshly opened descriptor this function owns.
        let file = unsafe { File::from_raw_fd(fd) };
        if !file.metadata()?.is_dir() {
            return Err(std::io::Error::from(std::io::ErrorKind::NotADirectory));
        }
        clear_nonblock(&file)?;
        Ok(RootHandle(file.into()))
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
                    libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
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
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            )?;
            let file = File::from(fd);
            clear_nonblock(&file)?;
            Ok(file)
        }

        /// A fresh open file description of the root, for a listing walk
        /// that opens each child directory relative to its parent. `dup`
        /// would share the directory stream position with the pinned root
        /// (and with every other walk), so a walk that stops early would
        /// leave the next one a shorter listing; `openat(fd, ".")` yields an
        /// independent description of the same directory with no path in
        /// between.
        pub(super) fn duplicate_handle(&self) -> std::io::Result<DirectoryHandle> {
            let segment = c_string(b".")?;
            // SAFETY: the root descriptor is live for the call and the
            // pointer references a NUL-terminated buffer owned by `segment`.
            let fd = unsafe {
                libc::openat(
                    self.0.as_raw_fd(),
                    segment.as_ptr(),
                    libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY,
                )
            };
            if fd < 0 {
                return Err(std::io::Error::last_os_error());
            }
            // SAFETY: `fd` is a freshly opened descriptor this function owns.
            Ok(unsafe { OwnedFd::from_raw_fd(fd) })
        }

        fn open_at(
            &self,
            directory: Option<&OwnedFd>,
            segment: &str,
            flags: libc::c_int,
        ) -> std::io::Result<OwnedFd> {
            if !super::plain_segment(segment) {
                return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput));
            }
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

    /// An open directory the listing walk can read entries from and open
    /// children relative to.
    pub(super) type DirectoryHandle = OwnedFd;

    /// Open one child directory of an already opened directory, with a
    /// symbolic link at the child refused and the directory confirmed on the
    /// opened descriptor.
    pub(super) fn open_child_directory(
        parent: &DirectoryHandle,
        name: &str,
    ) -> std::io::Result<DirectoryHandle> {
        if !super::plain_segment(name) {
            return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput));
        }
        let segment = c_string(name.as_bytes())?;
        // SAFETY: `parent` is a live descriptor for the duration of the call,
        // and the pointer references the NUL-terminated buffer owned by
        // `segment`.
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                segment.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `fd` is a freshly opened descriptor this function owns.
        let file = unsafe { File::from_raw_fd(fd) };
        if !file.metadata()?.is_dir() {
            return Err(std::io::Error::from(std::io::ErrorKind::NotADirectory));
        }
        Ok(file.into())
    }

    const ENTRY_BUDGET_MARKER: &str = "directory entry allowance exhausted";

    fn entry_budget_error() -> std::io::Error {
        std::io::Error::other(ENTRY_BUDGET_MARKER)
    }

    /// True when a listing failed because it reached the caller's entry
    /// allowance rather than a real I/O failure.
    pub(super) fn is_entry_budget(error: &std::io::Error) -> bool {
        error.kind() == std::io::ErrorKind::Other && error.to_string().contains(ENTRY_BUDGET_MARKER)
    }

    /// Read the entries of an open directory, at most `max` of them: the
    /// bound is enforced inside the read loop, before the entry that would
    /// cross it is accepted, so the work and the memory of a listing are
    /// bounded by the allowance rather than by the directory's true entry
    /// count. Returns each UTF-8 entry name with whether it is a directory;
    /// symbolic links are listed by name and refused when acquired.
    pub(super) fn list_entries(
        directory: &DirectoryHandle,
        max: usize,
    ) -> std::io::Result<Vec<(String, bool)>> {
        use std::os::fd::IntoRawFd;

        // The stream takes ownership of a duplicate, so the caller's handle
        // stays usable for opening children.
        let raw = directory.try_clone()?.into_raw_fd();
        // SAFETY: `raw` is a live directory descriptor whose ownership
        // transfers to the returned stream; on failure it is closed here.
        let stream = unsafe { libc::fdopendir(raw) };
        if stream.is_null() {
            let error = std::io::Error::last_os_error();
            // SAFETY: `raw` is still owned by this function when `fdopendir`
            // fails.
            unsafe { libc::close(raw) };
            return Err(error);
        }
        let mut entries = Vec::new();
        loop {
            // SAFETY: `stream` is the live directory stream opened above.
            errno_clear();
            let entry = unsafe { libc::readdir(stream) };
            if entry.is_null() {
                let error = std::io::Error::last_os_error();
                // SAFETY: `stream` is the live directory stream opened above;
                // it is closed exactly once.
                unsafe { libc::closedir(stream) };
                if error.raw_os_error().is_some_and(|code| code != 0) {
                    return Err(error);
                }
                break;
            }
            // SAFETY: `entry` is valid until the next `readdir` on this
            // stream, and only the NUL-terminated name within the entry is
            // read: the raw pointer to the array's first element is followed
            // to its terminator, never the whole declared array.
            let name_bytes = unsafe {
                std::ffi::CStr::from_ptr((&raw const (*entry).d_name).cast::<libc::c_char>())
            };
            let Ok(name) = name_bytes.to_str() else {
                continue;
            };
            if name == "." || name == ".." {
                continue;
            }
            if entries.len() == max {
                // SAFETY: as above; the stream is closed exactly once.
                unsafe { libc::closedir(stream) };
                return Err(entry_budget_error());
            }
            // SAFETY: as above; `d_type` is a plain byte field read by copy.
            let kind = unsafe { (*entry).d_type };
            let is_directory = match kind {
                libc::DT_DIR => true,
                libc::DT_UNKNOWN => {
                    // A filesystem without `d_type` support: ask the
                    // descriptor, without following a symbolic link.
                    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
                    let segment =
                        c_string(name.as_bytes()).expect("directory entry names carry no NUL");
                    // SAFETY: `stream` is live, so `dirfd` is a live
                    // descriptor; the name pointer references the
                    // NUL-terminated buffer owned by `segment`.
                    let status = unsafe {
                        libc::fstatat(
                            libc::dirfd(stream),
                            segment.as_ptr(),
                            &raw mut stat,
                            libc::AT_SYMLINK_NOFOLLOW,
                        )
                    };
                    status == 0 && stat.st_mode & libc::S_IFMT == libc::S_IFDIR
                }
                _ => false,
            };
            entries.push((name.to_owned(), is_directory));
        }
        Ok(entries)
    }

    fn errno_clear() {
        // SAFETY: writing 0 to the calling thread's errno location.
        unsafe {
            *errno_location() = 0;
        }
    }

    #[cfg(target_os = "macos")]
    fn errno_location() -> *mut libc::c_int {
        // SAFETY: `__error` returns the calling thread's errno location.
        unsafe { libc::__error() }
    }

    #[cfg(not(target_os = "macos"))]
    fn errno_location() -> *mut libc::c_int {
        // SAFETY: `__errno_location` returns the calling thread's errno
        // location.
        unsafe { libc::__errno_location() }
    }

    /// Clear `O_NONBLOCK` on an opened descriptor before it is read.
    fn clear_nonblock(file: &File) -> std::io::Result<()> {
        let fd = file.as_raw_fd();
        // SAFETY: `fd` is a live descriptor owned by `file` for the duration
        // of both calls.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: as above.
        let status = unsafe { libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK) };
        if status < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
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

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn open_root_refuses_a_symbolic_link_to_a_directory() {
            let base = std::env::temp_dir().join(format!(
                "powerio-core-open-root-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&base).unwrap();
            assert!(open_root(&base).is_ok());
            let link = base.join("link");
            std::os::unix::fs::symlink(&base, &link).unwrap();
            let error = open_root(&link).unwrap_err();
            assert!(
                is_symlink_refusal(&error) || error.kind() == std::io::ErrorKind::NotADirectory,
                "{error:?}"
            );
            std::fs::remove_dir_all(&base).unwrap();
        }
    }
}

#[cfg(not(unix))]
mod platform {
    //! Windows and other platforms have no `openat`. The walk opens every
    //! intermediate directory into a handle held for the remainder of the
    //! walk, with reparse points refused on the opened handle and the share
    //! mode excluding delete, so a held component can be neither replaced by
    //! a link nor renamed away while a child is opened beneath it — the same
    //! invariant the Unix descriptor walk enforces.

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
        // Verified at open; each walk re-pins the root for its own duration,
        // so the source's lifetime holds no lock that would block legitimate
        // tree changes between walks.
        drop(open_directory_pinned(path)?);
        Ok(RootHandle {
            root: path.to_path_buf(),
        })
    }

    impl RootHandle {
        pub(super) fn open_beneath(&self, segments: &[String]) -> std::io::Result<File> {
            let mut path = self.root.clone();
            let (file_segment, directories) =
                segments.split_last().expect("resolution yields a file");
            // Every handle from the root down stays alive until the final
            // open: the next component is opened only while every ancestor
            // is still held, and all release when the walk returns.
            let mut held = Vec::with_capacity(directories.len() + 1);
            held.push(open_directory_pinned(&self.root)?);
            for segment in directories {
                push_plain_segment(&mut path, segment)?;
                held.push(open_directory_pinned(&path)?);
            }
            push_plain_segment(&mut path, file_segment)?;
            let file = open_reparse_refused(&path)?;
            drop(held);
            Ok(file)
        }

        /// The root as a listing handle: the pinned handle plus the verified
        /// path each child extends.
        pub(super) fn duplicate_handle(&self) -> std::io::Result<DirectoryHandle> {
            let handle = open_directory_pinned(&self.root)?;
            Ok(DirectoryHandle {
                path: self.root.clone(),
                _handle: handle,
            })
        }
    }

    /// A verified directory the listing walk extends one plain component at
    /// a time, holding its own pinned handle; the walk keeps a frame per
    /// level, so every ancestor of an open frame stays held.
    #[derive(Debug)]
    pub(super) struct DirectoryHandle {
        path: PathBuf,
        _handle: File,
    }

    /// Extend the walk by one child directory, opened while the parent's
    /// handle is held, with a reparse point at the child refused on the
    /// opened handle.
    pub(super) fn open_child_directory(
        parent: &DirectoryHandle,
        name: &str,
    ) -> std::io::Result<DirectoryHandle> {
        let mut path = parent.path.clone();
        push_plain_segment(&mut path, name)?;
        let handle = open_directory_pinned(&path)?;
        Ok(DirectoryHandle {
            path,
            _handle: handle,
        })
    }

    const ENTRY_BUDGET_MARKER: &str = "directory entry allowance exhausted";

    fn entry_budget_error() -> std::io::Error {
        std::io::Error::other(ENTRY_BUDGET_MARKER)
    }

    /// True when a listing failed because it reached the caller's entry
    /// allowance rather than a real I/O failure.
    pub(super) fn is_entry_budget(error: &std::io::Error) -> bool {
        error.kind() == std::io::ErrorKind::Other && error.to_string().contains(ENTRY_BUDGET_MARKER)
    }

    /// Read the entries of one held directory, at most `max` of them: the
    /// bound is enforced inside the read loop, before the entry that would
    /// cross it is accepted. The held handle keeps the listed path the
    /// verified directory for the duration.
    pub(super) fn list_entries(
        directory: &DirectoryHandle,
        max: usize,
    ) -> std::io::Result<Vec<(String, bool)>> {
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(&directory.path)? {
            let entry = entry?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if entries.len() == max {
                return Err(entry_budget_error());
            }
            let is_directory = entry.file_type()?.is_dir();
            entries.push((name, is_directory));
        }
        Ok(entries)
    }

    /// Refuse a segment that is not a single plain file name before it is
    /// joined onto the walked path.
    fn push_plain_segment(path: &mut PathBuf, segment: &str) -> std::io::Result<()> {
        if !super::plain_segment(segment) {
            return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput));
        }
        path.push(segment);
        Ok(())
    }

    /// Open one directory into a held handle with the reparse point refused
    /// on the handle itself. On Windows the open uses backup semantics (a
    /// directory needs it), keeps `FILE_SHARE_DELETE` out of the share mode
    /// so the held component cannot be renamed or deleted, and refuses a
    /// handle whose attributes carry the reparse flag.
    #[cfg(windows)]
    fn open_directory_pinned(path: &Path) -> std::io::Result<File> {
        use std::os::windows::fs::{MetadataExt, OpenOptionsExt};

        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;

        let file = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)?;
        let attributes = file.metadata()?.file_attributes();
        if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(symlink_error());
        }
        if attributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
            return Err(std::io::Error::from(std::io::ErrorKind::NotADirectory));
        }
        Ok(file)
    }

    #[cfg(not(windows))]
    fn open_directory_pinned(path: &Path) -> std::io::Result<File> {
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() {
            return Err(symlink_error());
        }
        if !metadata.is_dir() {
            return Err(std::io::Error::from(std::io::ErrorKind::NotADirectory));
        }
        File::open(path)
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

    /// Serializes the tests that measure process wide resources (open
    /// descriptor counts, descriptor limits), so a measurement compares
    /// against a baseline taken under the same guard rather than a free
    /// running count other tests move.
    static PROCESS_RESOURCE_TESTS: Mutex<()> = Mutex::new(());

    fn process_resource_guard() -> MutexGuard<'static, ()> {
        PROCESS_RESOURCE_TESTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Byte counting global allocator, scoped to the one measuring thread so
    /// parallel tests never pollute a measurement.
    struct CountingAllocator;

    static ALLOCATED_BYTES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    thread_local! {
        static MEASURING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    fn measured_bytes<T>(work: impl FnOnce() -> T) -> (T, usize) {
        let _ = MEASURING.try_with(|flag| flag.set(true));
        let before = ALLOCATED_BYTES.load(std::sync::atomic::Ordering::Relaxed);
        let value = work();
        let after = ALLOCATED_BYTES.load(std::sync::atomic::Ordering::Relaxed);
        let _ = MEASURING.try_with(|flag| flag.set(false));
        (value, after.saturating_sub(before))
    }

    // SAFETY: delegates every operation to the system allocator; the counter
    // is a side effect on the measuring thread only.
    unsafe impl std::alloc::GlobalAlloc for CountingAllocator {
        unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
            if MEASURING.try_with(std::cell::Cell::get).unwrap_or(false) {
                ALLOCATED_BYTES.fetch_add(layout.size(), std::sync::atomic::Ordering::Relaxed);
            }
            unsafe { std::alloc::System.alloc(layout) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
            unsafe { std::alloc::System.dealloc(ptr, layout) }
        }

        unsafe fn realloc(
            &self,
            ptr: *mut u8,
            layout: std::alloc::Layout,
            new_size: usize,
        ) -> *mut u8 {
            if MEASURING.try_with(std::cell::Cell::get).unwrap_or(false) {
                ALLOCATED_BYTES.fetch_add(new_size, std::sync::atomic::Ordering::Relaxed);
            }
            unsafe { std::alloc::System.realloc(ptr, layout, new_size) }
        }
    }

    #[global_allocator]
    static COUNTING: CountingAllocator = CountingAllocator;

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

    #[cfg(unix)]
    #[test]
    fn a_named_pipe_is_refused_promptly_and_siblings_still_acquire() {
        use std::os::unix::ffi::OsStrExt;

        let root = test_root("fifo");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("real.csv"), b"real").unwrap();
        let fifo = root.join("pipe.dat");
        let c_path = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: the pointer references the NUL-terminated buffer owned by
        // `c_path`, which outlives the call.
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o644) }, 0);

        // No process is attached to the pipe, so a blocking open would hang;
        // each operation is driven from a worker with a bounded wait so a
        // regression fails the test rather than hanging it.
        let (sender, receiver) = std::sync::mpsc::channel();
        let opened_root = root.clone();
        let worker = std::thread::spawn(move || {
            let open_error = Source::open(opened_root.join("pipe.dat")).map(|_| ());
            let directory = Source::open(&opened_root).unwrap();
            let buffer_error = directory
                .buffer(&ArtifactPath::new("pipe.dat").unwrap())
                .map(|_| ());
            let sibling = directory
                .buffer(&ArtifactPath::new("real.csv").unwrap())
                .map(|buffer| buffer.bytes().to_vec());
            sender.send((open_error, buffer_error, sibling)).unwrap();
        });
        let (open_error, buffer_error, sibling) = receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("acquisition on a writerless pipe completes promptly");
        worker.join().unwrap();
        assert_eq!(
            open_error.unwrap_err().category(),
            crate::ErrorCategory::Request
        );
        assert_eq!(
            buffer_error.unwrap_err().category(),
            crate::ErrorCategory::Request
        );
        assert_eq!(sibling.unwrap(), b"real");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_over_budget_referenced_file_is_refused_before_allocation() {
        let root = test_root("budget");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("master.dss"), b"master").unwrap();
        // A sparse file whose declared length far exceeds the acquisition
        // budget; no bytes are written, so the refusal must come from the
        // declared length, before any reservation.
        let big = std::fs::File::create(root.join("big.dat")).unwrap();
        big.set_len(MAX_REFERENCED_BYTES * 4).unwrap();
        drop(big);

        let source = Source::open(root.join("master.dss")).unwrap();
        let primary = source.primary_buffer().unwrap();
        // The refusal happens before the reserve: the bytes allocated on this
        // thread during the refused acquisition are a tiny fraction of the
        // declared length. The measurement is thread scoped, so it fails when
        // the pre-reserve refusal is removed and never passes by accident.
        let (error, allocated) =
            measured_bytes(|| source.referenced_buffer(&primary, "big.dat").unwrap_err());
        assert!(error.to_string().contains("acquisition budget"), "{error}");
        assert!(
            (allocated as u64) < MAX_REFERENCED_BYTES / 16,
            "the refused acquisition allocated {allocated} bytes"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn racing_entry_listing_never_names_files_outside_the_root() {
        use std::os::unix::fs::symlink;

        let root = test_root("race-list");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/inside.txt"), b"inside").unwrap();
        let outside = test_root("race-list-outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("outside-only.txt"), b"outside").unwrap();

        let source = Source::open(&root).unwrap();
        let stop = std::sync::atomic::AtomicBool::new(false);
        std::thread::scope(|scope| {
            let flipper = scope.spawn(|| {
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let _ = std::fs::remove_dir_all(root.join("sub"));
                    let _ = symlink(&outside, root.join("sub"));
                    let _ = std::fs::remove_file(root.join("sub"));
                    let _ = std::fs::create_dir(root.join("sub"));
                    let _ = std::fs::write(root.join("sub/inside.txt"), b"inside");
                }
            });
            for _ in 0..50 {
                // Either the real subtree lists, or the walk fails; a name
                // that exists only outside the root never appears.
                if let Ok(names) = source.entry_names() {
                    assert!(
                        names
                            .iter()
                            .all(|name| !name.as_str().contains("outside-only")),
                        "{names:?}"
                    );
                }
            }
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
            flipper.join().unwrap();
        });
        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&outside).ok();
    }

    #[test]
    fn referenced_names_must_be_portable_relative_paths() {
        let root = test_root("portable-names");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("master.dss"), b"master").unwrap();
        std::fs::write(root.join("sub/feeder.dss"), b"feeder").unwrap();
        let source = Source::open(root.join("master.dss")).unwrap();
        let primary = source.primary_buffer().unwrap();

        // A climb spelled with the platform's alternate separator is the same
        // climb; a leading separator and a drive spelling are refused.
        for name in ["..\\escape.dss", "\\escape.dss", "C:\\escape.dss", "C:x"] {
            let error = source.referenced_buffer(&primary, name).unwrap_err();
            assert_eq!(
                error.category(),
                crate::ErrorCategory::Request,
                "{name}: {error}"
            );
        }
        assert!(source.root_buffer("..\\master.dss").is_err());
        assert!(source.root_buffer("\\master.dss").is_err());

        // Ordinary relative names keep resolving.
        let feeder = source
            .referenced_buffer(&primary, "sub/feeder.dss")
            .unwrap();
        assert_eq!(feeder.bytes(), b"feeder");

        // An absolute in-root name resolves to one segment per directory
        // component: it lands on the same cached buffer the relative name
        // produced, proving the per component walk ran on the same key. The
        // acquisition root is the canonical containing directory, so the
        // absolute spelling is canonical too.
        let absolute = root.canonicalize().unwrap().join("sub").join("feeder.dss");
        let again = source
            .referenced_buffer(&primary, absolute.to_str().unwrap())
            .unwrap();
        assert_eq!(again.bytes().as_ptr(), feeder.bytes().as_ptr());
        // No acquisition returned bytes from outside the root.
        assert!(
            source
                .acquired_buffers()
                .iter()
                .all(|buffer| !buffer.name().contains("escape"))
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn live_sources_hold_no_directory_descriptors_before_acquisition() {
        fn open_descriptor_count() -> usize {
            let table = if cfg!(target_os = "macos") {
                "/dev/fd"
            } else {
                "/proc/self/fd"
            };
            std::fs::read_dir(table).unwrap().count()
        }

        let _guard = process_resource_guard();
        let root = test_root("fd-count");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("case.m"), b"case").unwrap();
        std::fs::write(root.join("ref.csv"), b"ref").unwrap();

        let before = open_descriptor_count();
        let sources: Vec<Source> = (0..300)
            .map(|_| Source::open(root.join("case.m")).unwrap())
            .collect();
        let held = open_descriptor_count();
        assert!(
            held <= before + 4,
            "{} sources hold {} descriptors over the baseline {}",
            sources.len(),
            held - before,
            before
        );

        // Sources still acquire afterwards, and one source's two acquisitions
        // resolve through the same pinned root to one buffer. Acquisition
        // pins one descriptor per source, so this runs on a subset that stays
        // under the default descriptor limit.
        for source in sources.iter().take(32) {
            let primary = source.primary_buffer().unwrap();
            let first = source.referenced_buffer(&primary, "ref.csv").unwrap();
            let second = source.referenced_buffer(&primary, "ref.csv").unwrap();
            assert_eq!(first.bytes().as_ptr(), second.bytes().as_ptr());
        }
        drop(sources);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn entry_listing_returns_names_of_every_length_exactly() {
        // Guard on the entry-name read: names are read to their terminator,
        // whatever length the entry actually occupies.
        let root = test_root("name-lengths");
        std::fs::create_dir_all(&root).unwrap();
        let long = "n".repeat(200);
        for name in ["a", "medium-name.csv", long.as_str()] {
            std::fs::write(root.join(name), b"x").unwrap();
        }
        let source = Source::open(&root).unwrap();
        let mut names: Vec<String> = source
            .entry_names()
            .unwrap()
            .iter()
            .map(|name| name.as_str().to_owned())
            .collect();
        names.sort();
        let mut expected = vec!["a".to_owned(), "medium-name.csv".to_owned(), long];
        expected.sort();
        assert_eq!(names, expected);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_nested_past_the_depth_bound_is_refused_promptly() {
        use std::os::fd::{AsRawFd, FromRawFd};

        // The unwind below holds one descriptor per level; serialize with the
        // other descriptor sensitive tests so their measurements stay exact.
        let _guard = process_resource_guard();

        let root = test_root("deep-chain");
        std::fs::create_dir_all(&root).unwrap();
        // Build the chain by relative creation from inside each level, so the
        // tree reaches past any absolute path length limit.
        let name = std::ffi::CString::new("d").unwrap();
        let mut level = std::fs::File::open(&root).unwrap();
        for _ in 0..(MAX_REFERENCED_DEPTH + 40) {
            // SAFETY: `level` owns a live directory descriptor for both
            // calls, and the pointer references the NUL-terminated buffer
            // owned by `name`.
            unsafe {
                assert_eq!(libc::mkdirat(level.as_raw_fd(), name.as_ptr(), 0o755), 0);
                let fd = libc::openat(
                    level.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_CLOEXEC,
                );
                assert!(fd >= 0);
                level = std::fs::File::from_raw_fd(fd);
            }
        }
        drop(level);

        let (sender, receiver) = std::sync::mpsc::channel();
        let listed_root = root.clone();
        let worker = std::thread::spawn(move || {
            let source = Source::open(&listed_root).unwrap();
            sender.send(source.entry_names().map(|_| ())).unwrap();
        });
        let outcome = receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the depth refusal returns promptly");
        worker.join().unwrap();
        let error = outcome.expect_err("a chain past the depth bound refuses");
        assert!(error.to_string().contains("levels deep"), "{error}");

        // The chain is deeper than remove_dir_all's own recursion budget on
        // some platforms; unwind it level by level with the same descriptors.
        let mut fds = vec![std::fs::File::open(&root).unwrap()];
        loop {
            let last = fds.last().unwrap();
            // SAFETY: as above.
            let fd = unsafe {
                libc::openat(
                    last.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                break;
            }
            // SAFETY: `fd` is a freshly opened descriptor.
            fds.push(unsafe { std::fs::File::from_raw_fd(fd) });
        }
        while fds.len() > 1 {
            let parent = &fds[fds.len() - 2];
            // SAFETY: as above; AT_REMOVEDIR removes the empty directory.
            unsafe {
                libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR);
            }
            fds.pop();
        }
        drop(fds);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_directory_past_the_entry_budget_is_refused_with_bounded_memory() {
        let root = test_root("entry-budget");
        std::fs::create_dir_all(&root).unwrap();
        // Four times the budget: an unbounded read would materialize four
        // times the names the allowance admits, which the thread scoped
        // allocation measurement separates decisively from the bounded read.
        let excess = MAX_REFERENCED_FILES * 4;
        for index in 0..excess {
            std::fs::write(root.join(format!("f{index:05}.csv")), b"").unwrap();
        }
        let source = Source::open(&root).unwrap();
        let (error, allocated) = measured_bytes(|| source.entry_names().unwrap_err());
        assert!(error.to_string().contains("entries"), "{error}");
        // The listing stopped reading at the allowance: the bytes allocated
        // on this thread are bounded by the entry budget, never by the
        // directory's true entry count. Removing the in-loop bound reads all
        // `excess` names and fails this assertion.
        assert!(
            allocated < MAX_REFERENCED_FILES * 192,
            "the refused listing allocated {allocated} bytes"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn listing_breadth_never_scales_open_descriptors() {
        fn open_descriptor_count() -> usize {
            let table = if cfg!(target_os = "macos") {
                "/dev/fd"
            } else {
                "/proc/self/fd"
            };
            std::fs::read_dir(table).unwrap().count()
        }

        let _guard = process_resource_guard();
        let root = test_root("breadth");
        // Far more immediate subdirectories than the lowered descriptor
        // limit, each holding one file, plus one nested chain, so the walk
        // proves its held descriptors follow depth rather than breadth.
        let breadth = 400usize;
        for index in 0..breadth {
            let sub = root.join(format!("s{index:03}"));
            std::fs::create_dir_all(&sub).unwrap();
            std::fs::write(sub.join("data.csv"), b"x").unwrap();
        }
        std::fs::create_dir_all(root.join("nested/a/b/c")).unwrap();
        std::fs::write(root.join("nested/a/b/c/deep.csv"), b"x").unwrap();

        // Lower the descriptor soft limit for the duration, so a walk whose
        // descriptor use scales with breadth fails here rather than passing
        // on a machine with a raised limit.
        // SAFETY: `getrlimit` fills the zeroed out-parameter; the lowered
        // limit is restored below.
        let mut original: libc::rlimit = unsafe { std::mem::zeroed() };
        assert_eq!(
            // SAFETY: as above.
            unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &raw mut original) },
            0
        );
        let lowered = libc::rlimit {
            rlim_cur: 256,
            rlim_max: original.rlim_max,
        };
        // SAFETY: lowering the soft limit for this process; restored below.
        assert_eq!(
            unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &raw const lowered) },
            0
        );

        let baseline = open_descriptor_count();
        let peak = std::sync::atomic::AtomicUsize::new(0);
        let stop = std::sync::atomic::AtomicBool::new(false);
        let names = std::thread::scope(|scope| {
            let sampler = scope.spawn(|| {
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let count = open_descriptor_count();
                    peak.fetch_max(count, std::sync::atomic::Ordering::Relaxed);
                }
            });
            let source = Source::open(&root).unwrap();
            let names = source.entry_names().unwrap();
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
            sampler.join().unwrap();
            drop(source);
            names
        });
        // SAFETY: restoring the limit read above.
        assert_eq!(
            unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &raw const original) },
            0
        );

        assert_eq!(names.len(), breadth + 1, "every file listed");
        let sampled_peak = peak.load(std::sync::atomic::Ordering::Relaxed);
        assert!(
            sampled_peak <= baseline + MAX_REFERENCED_DEPTH + 16,
            "the walk held {sampled_peak} descriptors over a baseline of {baseline}"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_directory_listing_still_names_windows_reserved_spellings() {
        // The output predicate refusing reserved device stems must not narrow
        // what a source directory can list.
        let root = test_root("reserved-listing");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("aux.dss"), b"content").unwrap();
        let source = Source::open(&root).unwrap();
        let names = source.entry_names().unwrap();
        assert_eq!(names.len(), 1);
        assert_eq!(names[0].as_str(), "aux.dss");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_directory_listing_repeats_and_survives_acquisition() {
        // The walk runs once and its result is the source's one immutable
        // listing: a second call between or after buffer acquisitions
        // returns the same names rather than an empty second walk from a
        // directory stream a platform shares across duplicated descriptors.
        let root = test_root("repeat-listing");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("network.csv"), b"name\nseq\n").unwrap();
        std::fs::write(root.join("buses.csv"), b"name\nB1\n").unwrap();
        let source = Source::open(&root).unwrap();
        let first = source.entry_names().unwrap();
        assert_eq!(first.len(), 2);
        let name = ArtifactPath::new("network.csv").unwrap();
        source.buffer(&name).unwrap();
        let second = source.entry_names().unwrap();
        assert_eq!(first, second);
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
    #[cfg(unix)]
    #[test]
    fn a_refused_listing_never_shortens_the_next_one() {
        // A walk that stops early (the entry budget) must not leave the next
        // walk a partially consumed directory stream: every retry reads the
        // whole directory again and refuses the same way.
        let root = test_root("refused-listing");
        std::fs::create_dir_all(&root).unwrap();
        for index in 0..(MAX_REFERENCED_FILES + 5) {
            std::fs::write(root.join(format!("f{index}.txt")), b"x").unwrap();
        }
        let source = Source::open(&root).unwrap();
        let first = source.entry_names().unwrap_err();
        let second = source.entry_names().unwrap_err();
        assert_eq!(
            first.diagnostics().first().map(|d| d.code().to_owned()),
            second.diagnostics().first().map(|d| d.code().to_owned()),
            "the refusal repeats rather than shrinking into a partial listing"
        );
        drop(source);
        let _ = std::fs::remove_dir_all(&root);
    }
}
