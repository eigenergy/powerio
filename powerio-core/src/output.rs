use std::collections::BTreeSet;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::validation::{
    MAX_ARTIFACT_PATH_BYTES, MAX_ARTIFACT_SEGMENT_BYTES, path_exists_without_following,
};
use crate::{Diagnostic, Error};

static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Portable relative path of one output artifact.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ArtifactPath(Box<str>);

impl ArtifactPath {
    pub fn new(path: impl Into<String>) -> Result<Self, Error> {
        let path = path.into();
        if !valid_artifact_path(&path) {
            return Err(Error::new(
                &crate::codes::REQUEST_OUTPUT_INVALID_ARTIFACT_PATH,
                "artifact paths must be bounded portable relative paths with slash separators",
            ));
        }
        Ok(Self(path.into_boxed_str()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn join(&self, child: &ArtifactPath) -> Result<Self, Error> {
        Self::new(format!("{}/{}", self.0, child.0))
    }
}

impl fmt::Display for ArtifactPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl From<ArtifactPath> for String {
    fn from(path: ArtifactPath) -> Self {
        path.0.into()
    }
}

fn valid_artifact_path(path: &str) -> bool {
    if path.is_empty()
        || path.len() > MAX_ARTIFACT_PATH_BYTES
        || path.starts_with('/')
        || path.contains(['\\', '\0', ':'])
        || path.chars().any(char::is_control)
    {
        return false;
    }
    path.split('/').all(|segment| {
        !segment.is_empty()
            && segment != "."
            && segment != ".."
            && segment.len() <= MAX_ARTIFACT_SEGMENT_BYTES
    })
}

#[derive(Debug)]
enum DestinationKind {
    Path(PathBuf),
    Memory { root: ArtifactPath },
}

/// Owned output destination for one file, one directory, or memory artifacts.
#[derive(Debug)]
pub struct Destination {
    kind: DestinationKind,
}

impl Destination {
    #[must_use]
    pub fn path(path: impl Into<PathBuf>) -> Self {
        Self {
            kind: DestinationKind::Path(path.into()),
        }
    }

    pub fn memory(root: impl Into<String>) -> Result<Self, Error> {
        Ok(Self {
            kind: DestinationKind::Memory {
                root: ArtifactPath::new(root)?,
            },
        })
    }

    /// Commit a complete artifact inventory.
    ///
    /// Writers pass paths relative to a directory output. For a one file
    /// output, the destination itself supplies the returned artifact name.
    #[doc(hidden)]
    pub fn __commit_artifacts(
        self,
        directory: bool,
        mut artifacts: Vec<MemoryArtifact>,
        diagnostics: Vec<Diagnostic>,
    ) -> Result<WriteResult, Error> {
        validate_inventory(directory, &mut artifacts)?;
        let output = match self.kind {
            DestinationKind::Memory { root } => {
                if directory {
                    for artifact in &mut artifacts {
                        artifact.name = root.join(&artifact.name)?;
                    }
                } else {
                    artifacts[0].name = root;
                }
                WrittenOutput::Memory { artifacts }
            }
            DestinationKind::Path(root) => {
                let paths = commit_path_output(&root, directory, &artifacts)?;
                WrittenOutput::Path {
                    root,
                    artifacts: paths,
                }
            }
        };
        Ok(WriteResult {
            output,
            diagnostics,
        })
    }
}

/// One owned memory artifact.
#[derive(Debug, PartialEq, Eq)]
pub struct MemoryArtifact {
    name: ArtifactPath,
    bytes: Vec<u8>,
}

impl MemoryArtifact {
    #[must_use]
    pub const fn new(name: ArtifactPath, bytes: Vec<u8>) -> Self {
        Self { name, bytes }
    }

    #[must_use]
    pub const fn name(&self) -> &ArtifactPath {
        &self.name
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

/// Complete inventory of output owned by the caller.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum WrittenOutput {
    Path {
        root: PathBuf,
        artifacts: Vec<PathBuf>,
    },
    Memory {
        artifacts: Vec<MemoryArtifact>,
    },
}

/// Successful write output plus diagnostics emitted by the writer.
#[derive(Debug)]
pub struct WriteResult {
    output: WrittenOutput,
    diagnostics: Vec<Diagnostic>,
}

impl WriteResult {
    #[must_use]
    pub const fn output(&self) -> &WrittenOutput {
        &self.output
    }

    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    #[must_use]
    pub fn into_output(self) -> WrittenOutput {
        self.output
    }
}

fn validate_inventory(directory: bool, artifacts: &mut [MemoryArtifact]) -> Result<(), Error> {
    if artifacts.is_empty() || (!directory && artifacts.len() != 1) {
        return Err(Error::new(
            &crate::codes::REQUEST_OUTPUT_INVALID_LAYOUT,
            if directory {
                "a directory output must contain at least one artifact"
            } else {
                "a one file output must contain exactly one artifact"
            },
        ));
    }
    artifacts.sort_unstable_by(|left, right| left.name.cmp(&right.name));
    let mut names = BTreeSet::new();
    for artifact in artifacts.iter() {
        if !names.insert(artifact.name.as_str()) {
            return Err(Error::new(
                &crate::codes::REQUEST_OUTPUT_DUPLICATE_ARTIFACT,
                format!("duplicate output artifact '{}'", artifact.name),
            ));
        }
    }
    for pair in artifacts.windows(2) {
        let parent = pair[0].name.as_str();
        let child = pair[1].name.as_str();
        if child
            .strip_prefix(parent)
            .is_some_and(|suffix| suffix.starts_with('/'))
        {
            return Err(Error::new(
                &crate::codes::REQUEST_OUTPUT_INVALID_LAYOUT,
                format!("output artifact '{parent}' is also a directory prefix"),
            ));
        }
    }
    Ok(())
}

fn commit_path_output(
    target: &Path,
    directory: bool,
    artifacts: &[MemoryArtifact],
) -> Result<Vec<PathBuf>, Error> {
    if target.as_os_str().is_empty() {
        return Err(Error::new(
            &crate::codes::REQUEST_OUTPUT_INVALID_LAYOUT,
            "output path cannot be empty",
        ));
    }
    if path_exists_without_following(target).map_err(|cause| {
        Error::new(
            &crate::codes::EMIT_IO_STAGING,
            format!("cannot inspect output target '{}'", target.display()),
        )
        .with_cause(cause)
    })? {
        return Err(Error::new(
            &crate::codes::REQUEST_OUTPUT_COLLISION,
            format!("output target '{}' already exists", target.display()),
        ));
    }
    if let Some(parent) = target.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|cause| {
            Error::new(
                &crate::codes::EMIT_IO_STAGING,
                format!("cannot create output parent '{}'", parent.display()),
            )
            .with_cause(cause)
        })?;
    }

    let mut staging = StagingGuard::create(target, directory)?;
    let result = if directory {
        write_directory_artifacts(staging.path(), artifacts)
    } else {
        write_single_artifact(
            staging
                .file_mut()
                .expect("one file staging owns its open file"),
            &artifacts[0],
        )
    };
    if let Err(error) = result {
        return Err(staging.cleanup_after(error));
    }
    staging.commit(target)?;

    Ok(if directory {
        artifacts
            .iter()
            .map(|artifact| target.join(artifact.name.as_str()))
            .collect()
    } else {
        vec![target.to_path_buf()]
    })
}

fn write_single_artifact(file: &mut File, artifact: &MemoryArtifact) -> Result<(), Error> {
    file.write_all(&artifact.bytes).map_err(|cause| {
        Error::new(
            &crate::codes::EMIT_IO_WRITE,
            format!("cannot write output artifact '{}'", artifact.name),
        )
        .with_cause(cause)
    })?;
    file.sync_all().map_err(|cause| {
        Error::new(
            &crate::codes::EMIT_IO_WRITE,
            format!("cannot flush output artifact '{}'", artifact.name),
        )
        .with_cause(cause)
    })
}

fn write_directory_artifacts(staging: &Path, artifacts: &[MemoryArtifact]) -> Result<(), Error> {
    for artifact in artifacts {
        let path = staging.join(artifact.name.as_str());
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|cause| {
                Error::new(
                    &crate::codes::EMIT_IO_WRITE,
                    format!("cannot create directory for artifact '{}'", artifact.name),
                )
                .with_cause(cause)
            })?;
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|cause| {
                Error::new(
                    &crate::codes::EMIT_IO_WRITE,
                    format!("cannot create output artifact '{}'", artifact.name),
                )
                .with_cause(cause)
            })?;
        write_single_artifact(&mut file, artifact)?;
    }
    Ok(())
}

struct StagingGuard {
    path: PathBuf,
    directory: bool,
    file: Option<File>,
    committed: bool,
}

impl StagingGuard {
    fn create(target: &Path, directory: bool) -> Result<Self, Error> {
        let parent = target.parent().unwrap_or_else(|| Path::new("."));
        let name = target
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("powerio-output");
        for _ in 0..32 {
            let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!(
                ".{name}.powerio-tmp-{}-{sequence}",
                std::process::id()
            ));
            let created = if directory {
                std::fs::create_dir(&path).map(|()| None)
            } else {
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .map(Some)
            };
            match created {
                Ok(file) => {
                    return Ok(Self {
                        path,
                        directory,
                        file,
                        committed: false,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(cause) => {
                    return Err(Error::new(
                        &crate::codes::EMIT_IO_STAGING,
                        "cannot create sibling output staging path",
                    )
                    .with_cause(cause));
                }
            }
        }
        Err(Error::new(
            &crate::codes::EMIT_IO_STAGING,
            "could not choose an unused sibling output staging path",
        ))
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn file_mut(&mut self) -> Option<&mut File> {
        self.file.as_mut()
    }

    fn commit(mut self, target: &Path) -> Result<(), Error> {
        self.file.take();
        if let Err(cause) = std::fs::rename(&self.path, target) {
            let error = Error::new(
                &crate::codes::EMIT_IO_COMMIT,
                format!(
                    "cannot move complete staging output '{}' into '{}'",
                    self.path.display(),
                    target.display()
                ),
            )
            .with_cause(cause);
            return Err(self.cleanup_after(error));
        }
        self.committed = true;
        Ok(())
    }

    fn cleanup_after(mut self, original: Error) -> Error {
        self.file.take();
        match remove_staging(&self.path, self.directory) {
            Ok(()) => {
                self.committed = true;
                original
            }
            Err(cause) => {
                self.committed = true;
                Error::new(
                    &crate::codes::EMIT_IO_CLEANUP,
                    format!(
                        "output failed and staging path '{}' could not be removed: {original}",
                        self.path.display()
                    ),
                )
                .with_cause(cause)
                .with_diagnostics(original.into_diagnostics())
            }
        }
    }
}

impl Drop for StagingGuard {
    fn drop(&mut self) {
        if !self.committed {
            self.file.take();
            let _ = remove_staging(&self.path, self.directory);
        }
    }
}

fn remove_staging(path: &Path, directory: bool) -> std::io::Result<()> {
    if directory {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
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
            "powerio-core-output-{name}-{}-{nonce}",
            std::process::id()
        ))
    }

    fn artifact(name: &str, bytes: &[u8]) -> MemoryArtifact {
        MemoryArtifact::new(ArtifactPath::new(name).unwrap(), bytes.to_vec())
    }

    #[test]
    fn artifact_paths_reject_traversal_and_platform_spelling() {
        for path in [
            "",
            "/root",
            "../escape",
            "a/../b",
            "a/./b",
            "a//b",
            "a\\b",
            "C:drive",
            "nul\0byte",
        ] {
            assert!(ArtifactPath::new(path).is_err(), "{path:?}");
        }
        assert!(ArtifactPath::new("a".repeat(MAX_ARTIFACT_SEGMENT_BYTES + 1)).is_err());
        assert!(ArtifactPath::new("case/buses.csv").is_ok());
    }

    #[test]
    fn memory_output_owns_sorted_complete_artifacts() {
        let result = Destination::memory("case")
            .unwrap()
            .__commit_artifacts(
                true,
                vec![
                    artifact("lines.csv", b"lines"),
                    artifact("buses.csv", b"buses"),
                ],
                Vec::new(),
            )
            .unwrap();
        let WrittenOutput::Memory { artifacts } = result.into_output() else {
            panic!("memory output")
        };
        assert_eq!(
            artifacts
                .iter()
                .map(|artifact| artifact.name().as_str())
                .collect::<Vec<_>>(),
            ["case/buses.csv", "case/lines.csv"]
        );
        assert_eq!(artifacts[0].bytes(), b"buses");
    }

    #[test]
    fn path_output_refuses_collisions_and_does_not_overwrite() {
        let path = test_root("collision");
        std::fs::write(&path, b"existing").unwrap();
        let error = Destination::path(&path)
            .__commit_artifacts(false, vec![artifact("case.m", b"new")], Vec::new())
            .unwrap_err();
        assert_eq!(error.category(), crate::ErrorCategory::Request);
        assert_eq!(std::fs::read(&path).unwrap(), b"existing");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn complete_directory_output_is_committed_at_once() {
        let path = test_root("directory");
        let result = Destination::path(&path)
            .__commit_artifacts(
                true,
                vec![
                    artifact("buses.csv", b"buses"),
                    artifact("nested/lines.csv", b"lines"),
                ],
                Vec::new(),
            )
            .unwrap();
        let WrittenOutput::Path { root, artifacts } = result.into_output() else {
            panic!("path output")
        };
        assert_eq!(root, path);
        assert_eq!(
            artifacts,
            [path.join("buses.csv"), path.join("nested/lines.csv")]
        );
        assert_eq!(
            std::fs::read(path.join("nested/lines.csv")).unwrap(),
            b"lines"
        );
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn abandoned_staging_output_is_removed() {
        let target = test_root("cleanup");
        let staging_path = {
            let staging = StagingGuard::create(&target, true).unwrap();
            let path = staging.path().to_path_buf();
            std::fs::write(path.join("partial"), b"partial").unwrap();
            path
        };
        assert!(!staging_path.exists());
        assert!(!target.exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_at_the_target_is_a_collision() {
        use std::os::unix::fs::symlink;

        let target = test_root("symlink");
        let missing = target.with_extension("missing");
        symlink(&missing, &target).unwrap();
        let error = Destination::path(&target)
            .__commit_artifacts(false, vec![artifact("case.m", b"new")], Vec::new())
            .unwrap_err();
        assert_eq!(error.category(), crate::ErrorCategory::Request);
        assert!(
            std::fs::symlink_metadata(&target)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        std::fs::remove_file(target).unwrap();
    }

    #[test]
    fn duplicate_and_prefix_collisions_are_rejected_before_writing() {
        let duplicate = Destination::memory("case").unwrap().__commit_artifacts(
            true,
            vec![artifact("a", b"1"), artifact("a", b"2")],
            Vec::new(),
        );
        assert!(duplicate.is_err());
        let prefix = Destination::memory("case").unwrap().__commit_artifacts(
            true,
            vec![artifact("a", b"1"), artifact("a/b", b"2")],
            Vec::new(),
        );
        assert!(prefix.is_err());
    }
}
