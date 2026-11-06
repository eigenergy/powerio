//! Shared immutable inputs and independently observed source selections.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use super::{Error, Source, SourceBuffer, SourceId, SourceProvider};
use crate::ArtifactPath;

/// Explicit admission limits for an in-memory tree or one selected view.
///
/// These limits do not read environment variables. They bound admitted files,
/// their combined byte length, and path depth relative to the selected root.
/// View limits apply to exposed files; a view keeps the shared tree owner alive,
/// whose total storage is bounded by the limits used to construct the tree.
/// Parser work and decoded model allocations have their own limits.
/// Defaults are 4096 files, 1 GiB, and 64 path segments.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceTreeLimits {
    files: usize,
    bytes: u64,
    depth: usize,
}

impl Default for SourceTreeLimits {
    fn default() -> Self {
        Self {
            files: super::MAX_REFERENCED_FILES,
            bytes: super::DEFAULT_SOURCE_BYTES,
            depth: super::MAX_REFERENCED_DEPTH,
        }
    }
}

impl SourceTreeLimits {
    /// Set positive file, byte, and depth limits. Depth cannot exceed the
    /// source resolver's 64-segment bound; bytes must fit this platform.
    pub fn new(files: usize, bytes: u64, depth: usize) -> Result<Self, Error> {
        if files == 0
            || bytes == 0
            || isize::try_from(bytes).is_err()
            || depth == 0
            || depth > super::MAX_REFERENCED_DEPTH
        {
            return Err(Error::new(
                &crate::codes::REQUEST_SOURCE_INVALID_LIMIT,
                "tree limits require positive file/byte counts, platform-sized bytes, and depth in 1..=64",
            ));
        }
        Ok(Self {
            files,
            bytes,
            depth,
        })
    }

    #[must_use]
    pub const fn max_files(self) -> usize {
        self.files
    }

    #[must_use]
    pub const fn max_bytes(self) -> u64 {
        self.bytes
    }

    #[must_use]
    pub const fn max_depth(self) -> usize {
        self.depth
    }

    fn check(self, count: usize, bytes: u64, path: &str) -> Result<(), Error> {
        if count > self.files || bytes > self.bytes || path.split('/').count() > self.depth {
            return Err(Error::new(
                &crate::codes::READ_IO_REFERENCE_BUDGET,
                format!(
                    "source tree exceeds {} files, {} bytes, or {} path segments",
                    self.files, self.bytes, self.depth
                ),
            ));
        }
        Ok(())
    }
}

/// Named immutable files shared by independently selectable sources.
///
/// Construct once, then call [`Self::source`] for each primary or directory.
/// Views share the inventory and byte allocations. Each call creates a fresh
/// dependency observation set; clones of that source share its observations.
/// No view can acquire files outside its selected root or from the filesystem.
///
/// ```
/// use std::sync::Arc;
/// use powerio_core::{ArtifactPath, SourceTree, SourceTreeLimits};
///
/// let limits = SourceTreeLimits::new(100, 8 << 20, 16)?;
/// let primary = ArtifactPath::new("masters/day.dss")?;
/// let tree = SourceTree::new("study", [
///     (primary.clone(), Arc::<[u8]>::from(&b"Redirect ../shared/lines.inc"[..])),
///     (ArtifactPath::new("shared/lines.inc")?, Arc::<[u8]>::from(&b"line data"[..])),
/// ], limits)?;
/// let source = tree.source(None, Some(&primary), limits)?;
/// let master = source.primary_buffer()?;
/// source.referenced_buffer(&master, "../shared/lines.inc")?;
/// assert_eq!(source.dependencies().len(), 1);
/// assert!(tree.source(None, Some(&primary), limits)?.dependencies().is_empty());
/// # Ok::<(), powerio_core::Error>(())
/// ```
#[derive(Clone, Debug)]
pub struct SourceTree {
    name: Arc<str>,
    files: Arc<BTreeMap<String, SourceBuffer>>,
}

impl SourceTree {
    /// Retain unique portable paths under explicit limits. File/directory
    /// collisions are refused. Paths are case sensitive and never renamed.
    pub fn new(
        name: impl Into<String>,
        files: impl IntoIterator<Item = (ArtifactPath, Arc<[u8]>)>,
        limits: SourceTreeLimits,
    ) -> Result<Self, Error> {
        let name = name.into();
        if !super::valid_nonempty_text(&name) {
            return Err(Error::new(
                &crate::codes::REQUEST_SOURCE_INVALID_NAME,
                "a source tree requires a nonempty bounded name",
            ));
        }
        let mut retained = BTreeMap::new();
        let mut total = 0u64;
        for (path, bytes) in files {
            let segments = super::resolve_segments(&[], path.as_str())?;
            total = total.saturating_add(bytes.len() as u64);
            limits.check(retained.len().saturating_add(1), total, path.as_str())?;
            let key = path.as_str().to_owned();
            let buffer = SourceBuffer::new(
                SourceId::new(&key)?,
                &key,
                bytes,
                segments[..segments.len() - 1].to_vec(),
            );
            if retained.insert(key.clone(), buffer).is_some() {
                return Err(invalid_path(format!("duplicate source tree file `{key}`")));
            }
        }
        for name in retained.keys() {
            let mut child = name.as_str();
            while let Some((parent, _)) = child.rsplit_once('/') {
                if retained.contains_key(parent) {
                    return Err(invalid_path(format!(
                        "source tree file `{parent}` is also a directory"
                    )));
                }
                child = parent;
            }
        }
        Ok(Self {
            name: name.into(),
            files: Arc::new(retained),
        })
    }

    /// Select a directory root and optionally a primary relative to that root.
    /// `None` selects the tree root or a directory source, respectively.
    ///
    /// All files below the root remain available as retained companions and
    /// count against `limits`, including the primary. Names, source IDs, and
    /// reference directories are rebased to that root; the primary has ID
    /// `/input`. Selecting a file creates no dependency observation.
    pub fn source(
        &self,
        root: Option<&ArtifactPath>,
        primary: Option<&ArtifactPath>,
        limits: SourceTreeLimits,
    ) -> Result<Source, Error> {
        let prefix = root.map_or_else(String::new, |path| format!("{}/", path.as_str()));
        let mut count = 0usize;
        let mut total = 0u64;
        for (path, buffer) in self.files.range(prefix.clone()..) {
            let Some(relative) = path.strip_prefix(&prefix) else {
                break;
            };
            count += 1;
            total = total.saturating_add(buffer.bytes().len() as u64);
            limits.check(count, total, relative)?;
        }
        if root.is_some() && count == 0 {
            return Err(invalid_path(
                "selected source tree directory does not exist",
            ));
        }
        let view = TreeView {
            files: Arc::clone(&self.files),
            prefix,
            primary: primary.cloned(),
            limits,
            buffers: Mutex::new(BTreeMap::new()),
        };
        if let Some(primary) = primary {
            view.buffer(primary.as_str(), u64::MAX)?;
        }
        let name = primary.map_or_else(
            || {
                root.map_or_else(
                    || self.name.to_string(),
                    |root| format!("{}/{}", self.name, root.as_str()),
                )
            },
            |primary| primary.as_str().to_owned(),
        );
        Ok(Source {
            name: name.into(),
            provider: Arc::new(SourceProvider::Tree(view)),
            declared_format: None,
            dependencies: Arc::default(),
        })
    }
}

fn invalid_path(message: impl Into<String>) -> Error {
    Error::new(&crate::codes::REQUEST_SOURCE_INVALID_PATH, message)
}

#[derive(Debug)]
pub(super) struct TreeView {
    files: Arc<BTreeMap<String, SourceBuffer>>,
    prefix: String,
    pub primary: Option<ArtifactPath>,
    pub limits: SourceTreeLimits,
    buffers: Mutex<BTreeMap<String, SourceBuffer>>,
}

impl TreeView {
    pub fn retained_buffers(&self) -> Vec<SourceBuffer> {
        let mut buffers: Vec<_> = self
            .names()
            .iter()
            .map(|path| {
                self.buffer(path.as_str(), u64::MAX)
                    .expect("validated tree view")
            })
            .collect();
        if let Some(index) = buffers
            .iter()
            .position(|buffer| buffer.id().as_str() == super::PRIMARY_SOURCE_ID)
        {
            let primary = buffers.remove(index);
            buffers.insert(0, primary);
        }
        buffers
    }

    pub fn names(&self) -> Vec<ArtifactPath> {
        self.files
            .range(self.prefix.clone()..)
            .take_while(|(path, _)| path.starts_with(&self.prefix))
            .map(|(path, _)| {
                ArtifactPath::new(&path[self.prefix.len()..]).expect("validated tree path")
            })
            .collect()
    }

    pub fn buffer(&self, name: &str, max_bytes: u64) -> Result<SourceBuffer, Error> {
        let segments = super::resolve_segments(&[], name)?;
        let key = segments.join("/");
        let original = self
            .files
            .get(&format!("{}{key}", self.prefix))
            .ok_or_else(|| {
                Error::new(
                    &crate::codes::REQUEST_SOURCE_UNKNOWN_BUFFER,
                    format!("file `{key}` was not supplied beneath the selected tree root"),
                )
            })?;
        if original.bytes().len() as u64 > max_bytes {
            return Err(super::ReadBudget::ReferencedBuffer(max_bytes).exceeded(&key));
        }
        let mut buffers = self
            .buffers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(buffers
            .entry(key.clone())
            .or_insert_with(|| {
                let id = if self.primary.as_ref().is_some_and(|p| p.as_str() == key) {
                    super::PRIMARY_SOURCE_ID
                } else {
                    &key
                };
                SourceBuffer::new(
                    SourceId::new(id).expect("validated tree source ID"),
                    &key,
                    original.shared_bytes(),
                    segments[..segments.len() - 1].to_vec(),
                )
            })
            .clone())
    }
}
