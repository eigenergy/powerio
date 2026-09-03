use std::collections::BTreeSet;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::validation::{MAX_ARTIFACT_PATH_BYTES, MAX_ARTIFACT_SEGMENT_BYTES};
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

/// Physical shape of one completed emission.
///
/// Layout is independent of fidelity: both a one-file format and a directory
/// format can be either an exact same-format echo or a canonical serialization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputLayout {
    File,
    Directory,
}

impl OutputLayout {
    #[must_use]
    const fn from_directory(directory: bool) -> Self {
        if directory {
            Self::Directory
        } else {
            Self::File
        }
    }
}

/// How an emitted artifact inventory relates to the module supplied to its
/// writer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fidelity {
    /// The unchanged module retained this format's source bytes and emitted
    /// them byte for byte.
    ExactSameFormat,
    /// The writer serialized the typed value. Projection losses, when any,
    /// are reported as diagnostics rather than encoded in this enum.
    Canonical,
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
        let root = ArtifactPath::new(root)?;
        // The root prefixes every returned artifact name, so it meets the
        // same portability rule validate_inventory applies to writer-supplied
        // names.
        if !portable_output_path(root.as_str()) {
            return Err(Error::new(
                &crate::codes::REQUEST_OUTPUT_INVALID_ARTIFACT_PATH,
                format!("output root '{root}' is not portable across platforms"),
            ));
        }
        Ok(Self {
            kind: DestinationKind::Memory { root },
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
        fidelity: Fidelity,
        mut artifacts: Vec<MemoryArtifact>,
        diagnostics: Vec<Diagnostic>,
    ) -> Result<EmitResult, Error> {
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
                EmittedOutput::Memory { artifacts }
            }
            DestinationKind::Path(root) => {
                let paths = commit_path_output(&root, directory, &artifacts)?;
                EmittedOutput::Path {
                    root,
                    artifacts: paths,
                }
            }
        };
        Ok(EmitResult {
            output,
            layout: OutputLayout::from_directory(directory),
            fidelity,
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
pub enum EmittedOutput {
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
pub struct EmitResult {
    output: EmittedOutput,
    layout: OutputLayout,
    fidelity: Fidelity,
    diagnostics: Vec<Diagnostic>,
}

impl EmitResult {
    #[must_use]
    pub const fn output(&self) -> &EmittedOutput {
        &self.output
    }

    #[must_use]
    pub const fn layout(&self) -> OutputLayout {
        self.layout
    }

    #[must_use]
    pub const fn fidelity(&self) -> Fidelity {
        self.fidelity
    }

    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Add diagnostics produced by a facade before or after a family writer.
    ///
    /// This is an implementation hook for dispatchers that prepare a value
    /// for a grid exchange writer. It is not part of the user facing API.
    #[doc(hidden)]
    #[must_use]
    pub fn __with_diagnostics(mut self, diagnostics: impl IntoIterator<Item = Diagnostic>) -> Self {
        self.diagnostics.extend(diagnostics);
        self
    }

    #[must_use]
    pub fn into_output(self) -> EmittedOutput {
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
        if !portable_output_path(artifact.name.as_str()) {
            return Err(Error::new(
                &crate::codes::REQUEST_OUTPUT_INVALID_ARTIFACT_PATH,
                format!(
                    "output artifact '{}' is not portable across platforms",
                    artifact.name
                ),
            ));
        }
        if !names.insert(artifact.name.as_str()) {
            return Err(Error::new(
                &crate::codes::REQUEST_OUTPUT_DUPLICATE_ARTIFACT,
                format!("duplicate output artifact '{}'", artifact.name),
            ));
        }
    }
    // Every proper `/`-delimited ancestor of every name is checked against the
    // full name set, so an artifact can never also be a directory of another,
    // whatever the names sort like.
    for artifact in artifacts.iter() {
        let name = artifact.name.as_str();
        for (offset, _) in name.match_indices('/') {
            let ancestor = &name[..offset];
            if names.contains(ancestor) {
                return Err(Error::new(
                    &crate::codes::REQUEST_OUTPUT_INVALID_LAYOUT,
                    format!("output artifact '{ancestor}' is also a directory prefix"),
                ));
            }
        }
    }
    Ok(())
}

/// True when every segment of a committed artifact name designates the same
/// filesystem entry on every supported platform: no segment ends in a dot or a
/// space, and no segment's stem is a Windows reserved device name. Source
/// entry listing deliberately does not apply this predicate; it constrains
/// only what a destination commits.
fn portable_output_path(path: &str) -> bool {
    path.split('/').all(|segment| {
        if segment.ends_with('.') || segment.ends_with(' ') {
            return false;
        }
        let stem = segment.split('.').next().unwrap_or(segment);
        !reserved_windows_stem(stem)
    })
}

fn reserved_windows_stem(stem: &str) -> bool {
    if stem.eq_ignore_ascii_case("con")
        || stem.eq_ignore_ascii_case("prn")
        || stem.eq_ignore_ascii_case("aux")
        || stem.eq_ignore_ascii_case("nul")
    {
        return true;
    }
    let mut characters = stem.chars();
    let prefix: String = characters.by_ref().take(3).collect();
    if !(prefix.eq_ignore_ascii_case("com") || prefix.eq_ignore_ascii_case("lpt")) {
        return false;
    }
    matches!(characters.next(), Some(digit) if digit.is_ascii_digit())
        && characters.next().is_none()
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

    // The commit itself is the collision check: the staged output is moved
    // onto the target with a rename that refuses an existing entry, so a
    // target created at any point before the commit is never replaced. This
    // early inspection only refuses obvious collisions before staging work
    // begins; correctness does not depend on it.
    if std::fs::symlink_metadata(target).is_ok() {
        return Err(collision(target));
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

/// Commit one already staged file onto `target` without replacing an entry
/// that exists there: the same no-replace rename (and refuse-on-exist
/// `hard_link` fallback) the destination commit uses. On refusal or failure
/// the staged file is removed, so a refused write leaves nothing beside the
/// target. For a streaming writer whose artifact must never be materialized
/// in memory; everything else commits through [`Destination`].
#[doc(hidden)]
pub fn __commit_staged_file(staged: &Path, target: &Path) -> Result<(), Error> {
    let remove_staged = || {
        let _ = std::fs::remove_file(staged);
    };
    match rename_no_replace(staged, target) {
        Ok(()) => Ok(()),
        Err(cause) if commit_collision(&cause) => {
            remove_staged();
            Err(collision(target))
        }
        Err(cause) if no_replace_unsupported(&cause) => match std::fs::hard_link(staged, target) {
            Ok(()) => {
                remove_staged();
                Ok(())
            }
            Err(cause) if commit_collision(&cause) => {
                remove_staged();
                Err(collision(target))
            }
            Err(cause) => {
                remove_staged();
                Err(Error::new(
                        &crate::codes::EMIT_IO_COMMIT,
                        format!(
                            "this filesystem cannot commit '{}' without risking replacement of a concurrently created target",
                            target.display()
                        ),
                    )
                    .with_cause(cause))
            }
        },
        Err(cause) => {
            remove_staged();
            Err(Error::new(
                &crate::codes::EMIT_IO_COMMIT,
                format!(
                    "cannot move complete staging output '{}' into '{}'",
                    staged.display(),
                    target.display()
                ),
            )
            .with_cause(cause))
        }
    }
}

fn collision(target: &Path) -> Error {
    Error::new(
        &crate::codes::REQUEST_OUTPUT_COLLISION,
        format!("output target '{}' already exists", target.display()),
    )
}

/// Move a complete staged output onto the target without replacing an entry
/// that exists at commit time.
///
/// An ordinary `rename` replaces a regular file at the target, so a target
/// substituted between the collision inspection and the commit would be
/// silently overwritten by an output that refused to overwrite anything. The
/// platform no-replace rename closes that window: `renamex_np(RENAME_EXCL)` on
/// macOS, `renameat2(RENAME_NOREPLACE)` on Linux, and `MoveFileExW` without
/// `MOVEFILE_REPLACE_EXISTING` on Windows all fail atomically when the target
/// entry exists, including when that entry is a dangling symbolic link. On a
/// filesystem whose rename cannot refuse (old NFS and FAT report the flag as
/// unsupported), a one file output falls back to `hard_link` plus staging
/// removal, which is equally refuse-on-exist; a directory output has no such
/// portable primitive and is refused with a clear error rather than committed
/// through a race.
fn rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    platform_rename_no_replace(from, to)
}

#[cfg(target_os = "macos")]
fn platform_rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    let from = path_to_c_string(from)?;
    let to = path_to_c_string(to)?;
    // SAFETY: both pointers reference NUL-terminated buffers owned by the
    // `CString` values above, which outlive the call.
    let status = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
    if status == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(target_os = "linux")]
fn platform_rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    let from = path_to_c_string(from)?;
    let to = path_to_c_string(to)?;
    // SAFETY: both pointers reference NUL-terminated buffers owned by the
    // `CString` values above, which outlive the call.
    let status = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(unix)]
fn path_to_c_string(path: &Path) -> std::io::Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))
}

#[cfg(windows)]
fn platform_rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }

    let encode = |path: &Path| -> std::io::Result<Vec<u16>> {
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // An interior zero unit would truncate the name handed to the
        // platform move; refuse it so the moved name is always the complete
        // requested target name.
        if wide[..wide.len() - 1].contains(&0) {
            return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput));
        }
        Ok(wide)
    };
    let from = encode(from)?;
    let to = encode(to)?;
    // No MOVEFILE_REPLACE_EXISTING: the move fails when the target exists.
    // SAFETY: both pointers reference NUL-terminated wide buffers owned by the
    // vectors above, which outlive the call.
    let status = unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0) };
    if status != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(any(
    all(unix, not(any(target_os = "macos", target_os = "linux"))),
    not(any(unix, windows))
))]
fn platform_rename_no_replace(_from: &Path, _to: &Path) -> std::io::Result<()> {
    Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
}

/// True when the filesystem reported the no-replace rename flag itself as
/// unsupported, rather than reporting a real collision or I/O failure.
fn no_replace_unsupported(error: &std::io::Error) -> bool {
    if error.kind() == std::io::ErrorKind::Unsupported {
        return true;
    }
    #[cfg(unix)]
    if matches!(
        error.raw_os_error(),
        Some(libc::EINVAL | libc::ENOSYS | libc::ENOTSUP)
    ) {
        return true;
    }
    false
}

/// True when the rename failed because the target entry already exists.
fn commit_collision(error: &std::io::Error) -> bool {
    if matches!(
        error.kind(),
        std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::DirectoryNotEmpty
    ) {
        return true;
    }
    #[cfg(unix)]
    if matches!(
        error.raw_os_error(),
        Some(libc::EEXIST | libc::ENOTEMPTY | libc::EISDIR)
    ) {
        return true;
    }
    #[cfg(windows)]
    // ERROR_ALREADY_EXISTS and ERROR_FILE_EXISTS.
    if matches!(error.raw_os_error(), Some(183 | 80)) {
        return true;
    }
    false
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
        match rename_no_replace(&self.path, target) {
            Ok(()) => {
                self.committed = true;
                Ok(())
            }
            Err(cause) if commit_collision(&cause) => Err(self.cleanup_after(collision(target))),
            Err(cause) if no_replace_unsupported(&cause) && !self.directory => {
                // Refuse-on-exist commit for filesystems without a no-replace
                // rename: `hard_link` fails when the target entry exists, and
                // the staging entry is removed only after the link succeeds.
                match std::fs::hard_link(&self.path, target) {
                    Ok(()) => {
                        let _ = std::fs::remove_file(&self.path);
                        self.committed = true;
                        Ok(())
                    }
                    Err(cause) if commit_collision(&cause) => {
                        Err(self.cleanup_after(collision(target)))
                    }
                    Err(cause) => {
                        let error = Error::new(
                            &crate::codes::EMIT_IO_COMMIT,
                            format!(
                                "this filesystem cannot commit '{}' without risking replacement of a concurrently created target",
                                target.display()
                            ),
                        )
                        .with_cause(cause);
                        Err(self.cleanup_after(error))
                    }
                }
            }
            Err(cause) if no_replace_unsupported(&cause) => {
                let error = Error::new(
                    &crate::codes::EMIT_IO_COMMIT,
                    format!(
                        "this filesystem has no rename that refuses an existing entry; a directory output at '{}' cannot be committed without risking replacement of a concurrently created target",
                        target.display()
                    ),
                )
                .with_cause(cause);
                Err(self.cleanup_after(error))
            }
            Err(cause) => {
                let error = Error::new(
                    &crate::codes::EMIT_IO_COMMIT,
                    format!(
                        "cannot move complete staging output '{}' into '{}'",
                        self.path.display(),
                        target.display()
                    ),
                )
                .with_cause(cause);
                Err(self.cleanup_after(error))
            }
        }
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

/// One output a write operation can produce.
///
/// A string or path names the file or directory to write. A [`Destination`]
/// passes through, which is how a memory destination and its artifact root
/// name reach the same operations as a file name.
///
/// A caller's own output type reaches the same operations by implementing
/// it. It carries this one method and gains no further required method:
/// options belong to the operation, not to the output.
pub trait IntoDestination {
    /// Resolve the output.
    ///
    /// # Errors
    /// The output cannot be named.
    fn into_destination(self) -> Result<Destination, Error>;
}

impl IntoDestination for Destination {
    fn into_destination(self) -> Result<Destination, Error> {
        Ok(self)
    }
}

macro_rules! into_destination_by_name {
    ($($output:ty),* $(,)?) => {
        $(
            impl IntoDestination for $output {
                fn into_destination(self) -> Result<Destination, Error> {
                    Ok(Destination::path(PathBuf::from(self)))
                }
            }
        )*
    };
}

into_destination_by_name!(&str, &String, String, &Path, &PathBuf, PathBuf);

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
                Fidelity::Canonical,
                vec![
                    artifact("lines.csv", b"lines"),
                    artifact("buses.csv", b"buses"),
                ],
                Vec::new(),
            )
            .unwrap();
        let EmittedOutput::Memory { artifacts } = result.into_output() else {
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
    fn an_existing_target_is_refused_and_never_replaced() {
        let path = test_root("refused");
        let target = path.join("case.m");
        commit_path_output(&target, false, &[artifact("case.m", b"one")]).unwrap();

        // A second write finds the name taken and refuses, and the first
        // output is still there byte for byte.
        let error = commit_path_output(&target, false, &[artifact("case.m", b"two")])
            .expect_err("an existing target is a collision");
        assert_eq!(error.category(), crate::ErrorCategory::Request);
        assert_eq!(std::fs::read(&target).unwrap(), b"one");

        // A nonempty directory at the target is also a refusal, and its
        // contents survive.
        let directory = path.join("as-a-directory");
        std::fs::create_dir_all(&directory).unwrap();
        let blocked = directory.join("out");
        std::fs::create_dir(&blocked).unwrap();
        std::fs::write(blocked.join("keep"), b"kept").unwrap();
        assert!(commit_path_output(&blocked, true, &[artifact("a.csv", b"a")]).is_err());
        assert_eq!(std::fs::read(blocked.join("keep")).unwrap(), b"kept");

        std::fs::remove_dir_all(&path).ok();
    }

    #[test]
    fn the_commit_refuses_a_target_created_after_staging_began() {
        // A target that appears between the collision inspection and the
        // commit must not be replaced. The commit primitive itself is the
        // guarantee: it fails on an existing entry instead of renaming over
        // it, for a file, a directory, and an empty directory target.
        let path = test_root("late-target");
        std::fs::create_dir_all(&path).unwrap();

        let staged = path.join("staged.m");
        std::fs::write(&staged, b"staged").unwrap();
        let target = path.join("case.m");
        std::fs::write(&target, b"foreign").unwrap();
        let error = rename_no_replace(&staged, &target).expect_err("existing file target");
        assert!(commit_collision(&error), "{error:?}");
        assert_eq!(std::fs::read(&target).unwrap(), b"foreign");
        assert_eq!(std::fs::read(&staged).unwrap(), b"staged");

        // An empty directory created at the target after staging is likewise
        // never replaced; a plain rename would have swapped a staged
        // directory straight over it.
        let staged_dir = path.join("staged-dir");
        std::fs::create_dir(&staged_dir).unwrap();
        let target_dir = path.join("out-dir");
        std::fs::create_dir(&target_dir).unwrap();
        let error = rename_no_replace(&staged_dir, &target_dir).expect_err("existing dir target");
        assert!(commit_collision(&error), "{error:?}");
        assert!(target_dir.is_dir());
        assert!(staged_dir.is_dir());

        // With no target entry the same primitive commits.
        let fresh = path.join("fresh.m");
        rename_no_replace(&staged, &fresh).unwrap();
        assert_eq!(std::fs::read(&fresh).unwrap(), b"staged");

        std::fs::remove_dir_all(&path).ok();
    }

    #[test]
    fn path_output_refuses_collisions_and_does_not_overwrite() {
        let path = test_root("collision");
        std::fs::write(&path, b"existing").unwrap();
        let error = Destination::path(&path)
            .__commit_artifacts(
                false,
                Fidelity::Canonical,
                vec![artifact("case.m", b"new")],
                Vec::new(),
            )
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
                Fidelity::Canonical,
                vec![
                    artifact("buses.csv", b"buses"),
                    artifact("nested/lines.csv", b"lines"),
                ],
                Vec::new(),
            )
            .unwrap();
        let EmittedOutput::Path { root, artifacts } = result.into_output() else {
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
            .__commit_artifacts(
                false,
                Fidelity::Canonical,
                vec![artifact("case.m", b"new")],
                Vec::new(),
            )
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
            Fidelity::Canonical,
            vec![artifact("a", b"1"), artifact("a", b"2")],
            Vec::new(),
        );
        assert!(duplicate.is_err());
        let prefix = Destination::memory("case").unwrap().__commit_artifacts(
            true,
            Fidelity::Canonical,
            vec![artifact("a", b"1"), artifact("a/b", b"2")],
            Vec::new(),
        );
        assert!(prefix.is_err());
    }

    #[test]
    fn a_prefix_collision_is_refused_whatever_sorts_between() {
        // Names whose first differing byte sorts below `/` separate the
        // ancestor from its child in sorted order, so an adjacent-pair scan
        // would miss the conflict; the ancestor check must not.
        let separated = vec![
            artifact("a", b"1"),
            artifact("a b", b"2"), // space (0x20) < '/'
            artifact("a-x", b"3"), // '-' (0x2D) < '/'
            artifact("a.csv", b"4"),
            artifact("a/b", b"5"),
        ];
        let memory = Destination::memory("case").unwrap().__commit_artifacts(
            true,
            Fidelity::Canonical,
            separated
                .iter()
                .map(|a| artifact(a.name().as_str(), a.bytes()))
                .collect(),
            Vec::new(),
        );
        let error = memory.expect_err("the ancestor conflict is refused");
        assert_eq!(error.category(), crate::ErrorCategory::Request);

        let target = test_root("prefix-separated");
        let path = Destination::path(&target).__commit_artifacts(
            true,
            Fidelity::Canonical,
            separated,
            Vec::new(),
        );
        let error = path.expect_err("the path destination refuses identically");
        assert_eq!(error.category(), crate::ErrorCategory::Request);
        // Nothing was created and no staging entry was left beside the target.
        assert!(!target.exists());
        let parent = target.parent().unwrap();
        let residue: Vec<String> = std::fs::read_dir(parent)
            .unwrap()
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(target.file_name().unwrap().to_str().unwrap()))
            .collect();
        assert!(residue.is_empty(), "{residue:?}");
    }

    #[test]
    fn reserved_and_nonportable_spellings_are_refused_at_commit() {
        let refused = [
            "con",
            "CON",
            "con.txt",
            "PRN.csv",
            "aux",
            "AUX.dss",
            "nul.m",
            "com1",
            "COM9.raw",
            "lpt0",
            "LPT5.csv",
            "trailing.",
            "trailing ",
            "nested/aux.csv",
            "aux/nested.csv",
        ];
        for name in refused {
            let memory = Destination::memory("case").unwrap().__commit_artifacts(
                true,
                Fidelity::Canonical,
                vec![artifact(name, b"x"), artifact("keep.csv", b"y")],
                Vec::new(),
            );
            let error = memory.expect_err(name);
            assert_eq!(error.category(), crate::ErrorCategory::Request, "{name}");

            let target = test_root("reserved");
            let path = Destination::path(&target).__commit_artifacts(
                true,
                Fidelity::Canonical,
                vec![artifact(name, b"x")],
                Vec::new(),
            );
            assert!(path.is_err(), "{name}");
            assert!(!target.exists(), "{name}");
        }
        // Ordinary inventories still commit, reserved-looking stems included
        // only when they are not reserved (`config`, `auxiliary`).
        let accepted = Destination::memory("case").unwrap().__commit_artifacts(
            true,
            Fidelity::Canonical,
            vec![
                artifact("case.dss", b"a"),
                artifact("buscoords.csv", b"b"),
                artifact("network.csv", b"c"),
                artifact("nested/lines.csv", b"d"),
                artifact("config.json", b"e"),
                artifact("auxiliary.csv", b"f"),
                artifact("com10.csv", b"g"),
            ],
            Vec::new(),
        );
        assert!(accepted.is_ok());
    }

    #[test]
    fn a_memory_root_meets_the_same_portability_rule_as_artifact_names() {
        for root in ["aux", "AUX.case", "trailing.", "trailing ", "nested/nul"] {
            let refused = Destination::memory(root);
            let error = refused.expect_err(root);
            assert_eq!(error.category(), crate::ErrorCategory::Request, "{root}");
        }
        // An ordinary root still commits and still prefixes every name, in
        // both the one file and the directory form.
        let one = Destination::memory("case.m")
            .unwrap()
            .__commit_artifacts(
                false,
                Fidelity::ExactSameFormat,
                vec![artifact("case.m", b"x")],
                Vec::new(),
            )
            .unwrap();
        assert_eq!(one.layout(), OutputLayout::File);
        assert_eq!(one.fidelity(), Fidelity::ExactSameFormat);
        let EmittedOutput::Memory { artifacts } = one.into_output() else {
            panic!("memory output")
        };
        assert_eq!(artifacts[0].name().as_str(), "case.m");
        let directory = Destination::memory("case")
            .unwrap()
            .__commit_artifacts(
                true,
                Fidelity::Canonical,
                vec![artifact("buses.csv", b"x")],
                Vec::new(),
            )
            .unwrap();
        assert_eq!(directory.layout(), OutputLayout::Directory);
        assert_eq!(directory.fidelity(), Fidelity::Canonical);
        let EmittedOutput::Memory { artifacts } = directory.into_output() else {
            panic!("memory output")
        };
        assert_eq!(artifacts[0].name().as_str(), "case/buses.csv");
    }

    #[cfg(windows)]
    #[test]
    fn a_windows_commit_refuses_an_interior_nul_in_the_target_name() {
        use std::os::windows::ffi::OsStringExt;

        let base = test_root("wide-nul");
        std::fs::create_dir_all(&base).unwrap();
        let staged = base.join("staged.m");
        std::fs::write(&staged, b"staged").unwrap();
        let hostile: std::path::PathBuf =
            std::ffi::OsString::from_wide(&[b'c' as u16, 0, b'x' as u16]).into();
        let error = rename_no_replace(&staged, &base.join(hostile)).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        // No file appeared at the truncated name.
        assert!(!base.join("c").exists());
        std::fs::remove_dir_all(&base).ok();
    }
}
