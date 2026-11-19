//! Bounded collections with independent, explicitly selected parser inputs.
//!
//! Inspection classifies files without parsing electrical values. Candidates
//! may overlap: a companion is never removed from the independent inventory.
//! Acquisition verifies every ZIP payload once; it does not recursively open
//! transport ZIPs. Parser state and dependency observations are per selection.

use crate::format::routing::{Detection, JsonClass, classify_json_bytes};
use crate::{
    ArtifactPath, Error, FormatId, ParseOptions, PioModule, PioValue, Result, Source, SourceTree,
    SourceTreeLimits,
};
pub use powerio_archive::ArchiveLimits;
pub use powerio_archive::session::Cancellation;
mod manifest;
mod nested;
pub use manifest::{
    CollectionCase, CollectionManifest, CollectionOrigin, CollectionReaderOptions, SelectionOrigin,
    SourceBundle, SourceBundleScope,
};
use nested::{Session, Work};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

/// Stable collection failures, independent of electrical parser findings.
pub mod codes {
    powerio_core::diagnostic_codes! {
        SELECTION = "REQUEST.COLLECTION.SELECTION", Error,
            "the collection selection is invalid or incomplete", category = Request;
        IO = "READ.COLLECTION.IO", Error,
            "collection input could not be read", category = Data;
        LIMIT = "READ.COLLECTION.LIMIT", Error,
            "collection acquisition or inspection exceeds its budget", category = Data;
    }
}
fn selection_error(message: impl Into<String>) -> Error {
    Error::new(&codes::SELECTION, message)
}

/// Explicit host limits. Parser/model limits remain independently active.
///
/// The collection types are `#[non_exhaustive]` so that 0.12.x can add
/// fields and variants. Start from the default and set fields:
///
/// ```
/// let mut limits = powerio::CollectionLimits::default();
/// limits.work_bytes = 1 << 20;
/// ```
///
/// ```compile_fail
/// let limits = powerio::CollectionLimits {
///     work_bytes: 1 << 20,
///     ..powerio::CollectionLimits::default()
/// };
/// ```
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct CollectionLimits {
    pub tree: SourceTreeLimits,
    pub archive: ArchiveLimits,
    /// Maximum bytes examined per classification probe.
    pub probe_bytes: u64,
    /// Maximum bytes examined by all classification probes together.
    pub inspection_bytes: u64,
    /// Maximum ZIP decoding layers, including native CGMES/SINCAL archives.
    pub max_zip_layers: u32,
    /// Input and decompression bytes one operation may touch, charged
    /// conservatively. Every operation starts from zero.
    pub work_bytes: u64,
}
impl Default for CollectionLimits {
    fn default() -> Self {
        Self {
            tree: SourceTreeLimits::new(4096, 64 << 20, 64).expect("valid defaults"),
            archive: ArchiveLimits::default(),
            probe_bytes: 1 << 20,
            inspection_bytes: 8 << 20,
            max_zip_layers: 2,
            work_bytes: 256 << 20,
        }
    }
}

/// One file, including unrecognized assets retained for explicit selection.
#[derive(Clone, Debug, Serialize)]
#[non_exhaustive]
pub struct CollectionEntry {
    pub path: String,
    pub bytes: u64,
}

/// The operation to perform; stored PowerIO IR is never grid-format parsing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum CollectionOperation {
    #[default]
    Parse,
    Deserialize,
}

/// An explicit primary relative to a permitted root. `None` means collection
/// root. Nonstandard file names work with an explicit format. DSS/GLM can read
/// companions throughout the root, as do explicit SINCAL MDB selections with
/// acquired tables. Other standalone formats receive only the primary.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CollectionSelection {
    pub root: Option<String>,
    pub primary: Option<String>,
    /// Explicit members relative to root, for a document or role group.
    #[serde(default)]
    pub members: Option<Vec<String>>,
    pub format: Option<String>,
    #[serde(default)]
    pub operation: CollectionOperation,
}
/// A format name in its registered spelling, or as given when unregistered.
fn canonical_token(name: &str) -> &str {
    crate::resolve_format(name).map_or(name, |format| format.token)
}

impl CollectionSelection {
    #[must_use]
    pub fn file(primary: impl Into<String>) -> Self {
        Self {
            primary: Some(primary.into()),
            ..Self::default()
        }
    }
    pub(super) fn format_token(&self) -> Option<&str> {
        self.format.as_deref().map(canonical_token)
    }

    pub(super) fn has_companions(&self) -> bool {
        match self.format_token() {
            Some("dss" | "gridlabd" | "gridlabd-arras") => true,
            Some(_) => false,
            None => self.primary.as_deref().is_some_and(|name| {
                name.rsplit_once('.').is_some_and(|(_, ext)| {
                    ext.eq_ignore_ascii_case("dss") || ext.eq_ignore_ascii_case("glm")
                })
            }),
        }
    }

    // Access MDB is not a native ZIP. Its explicitly acquired tables and any
    // referenced profiles must remain inside the selected collection root.
    pub(super) fn has_sincal_tables(&self, bytes: &[u8]) -> bool {
        matches!(
            self.format_token(),
            Some("sincal" | "sincal-balanced" | "sincal-multiconductor")
        ) && bytes.starts_with(b"\0\x01\0\0Standard Jet DB\0")
    }

    #[must_use]
    pub fn format(mut self, format: impl Into<String>) -> Self {
        self.format = Some(format.into());
        self
    }
}

/// Discovery readiness is not dependency completeness or solver suitability.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum CandidateStatus {
    Selectable,
    NeedsOptions,
    NeedsFormat,
    ProbeLimit,
    UnsupportedBuild,
    Incomplete,
}
#[derive(Clone, Debug, Serialize)]
#[non_exhaustive]
pub struct CollectionCandidate {
    pub selection: CollectionSelection,
    pub status: CandidateStatus,
}

/// Parsed selections remain independent modules, including their parser-defined
/// time/scenario structure. A successful parse does not certify study completeness.
#[derive(Debug)]
#[non_exhaustive]
pub struct CollectionResult {
    pub module: PioModule<PioValue>,
    pub dependencies: Vec<String>,
    /// Missing references reported as warnings by an adapter.
    pub incomplete: bool,
}

/// Shared immutable collection bytes. Cloning shares payload allocations.
///
/// ```no_run
/// use powerio::{Collection, CollectionLimits, CollectionSelection, ParseOptions};
/// let collection = Collection::from_directory("study", CollectionLimits::default())?;
/// for candidate in collection.inspect()? {
///     println!("{:?}", candidate);
/// }
/// let selection = CollectionSelection::file("masters/weekday.dss").format("dss");
/// let result = collection.parse(&selection, &ParseOptions::default())?;
/// println!("observed dependencies: {:?}", result.dependencies);
/// # Ok::<(), powerio::Error>(())
/// ```
#[derive(Clone, Debug)]
pub struct Collection {
    tree: SourceTree,
    inventory: Arc<[CollectionEntry]>,
    limits: CollectionLimits,
    name: String,
    original: Option<Arc<[u8]>>,
    session: Session,
    zip_layers: u32,
    cancel: Cancellation,
    containers: Vec<String>,
    origin_name: String,
}
impl Collection {
    /// Admit a named memory tree. Does not execute a parser or read the host.
    pub fn from_files(
        name: impl Into<String>,
        files: impl IntoIterator<Item = (ArtifactPath, Arc<[u8]>)>,
        limits: CollectionLimits,
    ) -> Result<Self> {
        if limits.max_zip_layers == 0 || limits.work_bytes == 0 {
            return Err(selection_error(
                "collection session limits must be positive",
            ));
        }
        let name = name.into();
        let tree = SourceTree::new(&name, files, limits.tree)?;
        let source = tree.source(None, None, limits.tree)?;
        let inventory: Arc<[CollectionEntry]> = source
            .entry_names()?
            .into_iter()
            .map(|path| {
                let bytes = source.buffer(&path)?.bytes().len() as u64;
                Ok(CollectionEntry {
                    path: path.to_string(),
                    bytes,
                })
            })
            .collect::<Result<Vec<_>>>()?
            .into();
        let session = Session::new(&inventory);
        Ok(Self {
            origin_name: name.clone(),
            session,
            zip_layers: 0,
            cancel: Cancellation::default(),
            containers: Vec::new(),
            tree,
            inventory,
            limits,
            name,
            original: None,
        })
    }

    /// Fully verify a bounded ZIP once. Original container and member bytes are
    /// retained separately. Nested containers remain ordinary opaque members.
    pub fn from_zip(
        name: impl Into<String>,
        bytes: Arc<[u8]>,
        limits: CollectionLimits,
    ) -> Result<Self> {
        Self::from_zip_cancellable(name, bytes, limits, Cancellation::default())
    }

    /// Acquire a ZIP with a caller-owned cancellation signal.
    pub fn from_zip_cancellable(
        name: impl Into<String>,
        bytes: Arc<[u8]>,
        limits: CollectionLimits,
        cancel: Cancellation,
    ) -> Result<Self> {
        cancel.check()?;
        let mut archive = powerio_archive::Archive::open(Arc::clone(&bytes), limits.archive)?;
        let archive_entries = archive.entries().len();
        let directories: Vec<_> = archive
            .entries()
            .filter(|e| e.is_directory())
            .map(|e| e.path().clone())
            .collect();
        let entries = archive
            .entries()
            .filter(|e| !e.is_directory())
            .map(|e| (e.path().clone(), e.size()))
            .collect::<Vec<_>>();
        if entries.len() > limits.tree.max_files()
            || entries
                .iter()
                .try_fold(0u64, |n, (_, size)| n.checked_add(*size))
                .is_none_or(|n| n > limits.tree.max_bytes())
        {
            return Err(Error::new(
                &codes::LIMIT,
                "ZIP exceeds collection tree limits",
            ));
        }
        let expanded: u64 = entries.iter().map(|(_, size)| size).sum();
        if (bytes.len() as u64).saturating_add(expanded) > limits.work_bytes {
            return Err(Error::new(
                &codes::LIMIT,
                "ZIP exceeds collection work limit",
            ));
        }
        for path in directories {
            archive.read_cancellable(&path, 0, &cancel)?;
        }
        let files = entries
            .into_iter()
            .map(|(path, _)| {
                archive
                    .read_cancellable(&path, limits.tree.max_bytes(), &cancel)
                    .map(|bytes| (path, bytes))
            })
            .collect::<Result<Vec<_>>>()?;
        let mut collection = Self::from_files(name, files, limits)?;
        collection.zip_layers = 1;
        collection.cancel = cancel;
        collection.session.initialize_archive(archive_entries)?;
        collection.original = Some(bytes);
        Ok(collection)
    }

    /// Acquire a folder through core's confined source reader. Once acquired,
    /// selections cannot fall back to disk or see later filesystem changes.
    pub fn from_directory(path: impl AsRef<Path>, limits: CollectionLimits) -> Result<Self> {
        let source = Source::open(path.as_ref())?;
        if !source.is_directory() {
            return Err(selection_error("a collection folder must be a directory"));
        }
        let names = source.entry_names()?;
        if names.len() > limits.tree.max_files() {
            return Err(Error::new(&codes::LIMIT, "collection file count exceeded"));
        }
        let mut remaining = limits.tree.max_bytes();
        let mut files = Vec::new();
        for name in names {
            if name.as_str().split('/').count() > limits.tree.max_depth() {
                return Err(Error::new(&codes::LIMIT, "collection path depth exceeded"));
            }
            let buffer = source.root_buffer_with_limit(name.as_str(), remaining)?;
            remaining -= buffer.bytes().len() as u64;
            files.push((name, buffer.shared_bytes()));
        }
        Self::from_files(source.name(), files, limits)
    }
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
    #[must_use]
    pub fn entries(&self) -> &[CollectionEntry] {
        &self.inventory
    }
    #[must_use]
    pub fn original_zip(&self) -> Option<Arc<[u8]>> {
        self.original.clone()
    }

    /// Bounded suggestions, without electrical parsing or filename precedence.
    /// Every inventory file remains explicitly selectable even if omitted here.
    pub fn inspect(&self) -> Result<Vec<CollectionCandidate>> {
        self.cancel.check()?;
        let mut work = Work::new(self.limits.work_bytes);
        work.charge(self.entries().iter().map(|e| e.path.len() as u64 + 1).sum())?;
        let source = self.tree.source(None, None, self.limits.tree)?;
        let mut remaining = self.limits.inspection_bytes;
        let mut candidates = Vec::new();
        for entry in self.entries() {
            self.cancel.check()?;
            let extension = entry
                .path
                .rsplit('.')
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase();
            let mut selection = CollectionSelection::file(&entry.path);
            let mut status = CandidateStatus::Selectable;
            selection.format = match extension.as_str() {
                "dss" => Some("dss"),
                "glm" => Some("gridlabd"),
                "m" => Some("matpower"),
                "raw" => Some("psse"),
                "rawx" => Some("psse-rawx"),
                "aux" => Some("powerworld"),
                "epc" => Some("pslf"),
                "uct" | "ucte" => Some("ucte"),
                "cdf" => Some("ieee-cdf"),
                "pwb" => Some("pwb"),
                "xiidm" => Some("xiidm"),
                "con" => Some("psse-con"),
                "sub" => Some("psse-sub"),
                "mon" => Some("psse-mon"),
                "json" | "jiidm" => {
                    if entry.bytes > self.limits.probe_bytes || entry.bytes > remaining {
                        status = CandidateStatus::ProbeLimit;
                        None
                    } else {
                        work.charge(entry.bytes)?;
                        remaining -= entry.bytes;
                        let bytes = source.root_buffer(&entry.path)?;
                        match classify_json_bytes(bytes.content_bytes()) {
                            JsonClass::Module => {
                                selection.operation = CollectionOperation::Deserialize;
                                None
                            }
                            JsonClass::Case(Detection::Known(format)) => Some(format.name()),
                            JsonClass::Case(_) => {
                                status = CandidateStatus::NeedsFormat;
                                None
                            }
                        }
                    }
                }
                "xml" | "sinx" | "zip" => {
                    status = CandidateStatus::NeedsOptions;
                    None
                }
                _ => continue,
            }
            .map(str::to_owned);
            candidates.push(CollectionCandidate { selection, status });
        }
        candidates.extend(self.dataset_candidates());
        Ok(candidates)
    }

    /// Build one confined source. Each call starts fresh observations; retained
    /// companions share bytes. Generic ZIP traversal requires another operation.
    pub fn source(&self, selection: &CollectionSelection) -> Result<Source> {
        let root = selection
            .root
            .as_deref()
            .map(ArtifactPath::new)
            .transpose()?;
        if selection.members.is_some() {
            return self.group_source(selection, root.as_ref());
        }
        let Some(primary) = selection.primary.as_deref() else {
            return self.dataset_source(selection, root.as_ref());
        };
        let primary = ArtifactPath::new(primary)?;
        let scoped = self
            .tree
            .source(root.as_ref(), Some(&primary), self.limits.tree)?;
        let format = selection.format_token();
        if primary.as_str().to_ascii_lowercase().ends_with(".zip") && format != Some("cgmes") {
            return Err(selection_error(
                "transport ZIP members require explicit nested inspection; CGMES ZIPs require format cgmes",
            ));
        }
        let mut source = if selection.has_companions()
            || selection.has_sincal_tables(scoped.primary_buffer()?.content_bytes())
        {
            scoped
        } else {
            let bytes = scoped.primary_buffer()?.shared_bytes();
            Source::from_memory(primary.as_str(), bytes)?
        };
        if let Some(format) = format {
            source = source.with_format(FormatId::new(format)?);
        }
        Ok(source)
    }

    /// Parse or deserialize one explicit selection. Call independently for each
    /// selected case; one error never prevents processing another selection.
    pub fn parse(
        &self,
        selection: &CollectionSelection,
        options: &ParseOptions,
    ) -> Result<CollectionResult> {
        if options.acquisition_root.is_some() {
            return Err(selection_error(
                "collection sources cannot acquire host files",
            ));
        }
        if options
            .format
            .as_ref()
            .is_some_and(|f| Some(canonical_token(f.as_str())) != selection.format_token())
        {
            return Err(selection_error(
                "set the format on the collection selection",
            ));
        }
        self.cancel.check()?;
        self.charge_selection(selection)?;
        let source = self.source(selection)?;
        let module = match selection.operation {
            CollectionOperation::Parse => crate::parse_with_options(&source, options)?,
            CollectionOperation::Deserialize => {
                if selection.format.is_some() {
                    return Err(selection_error("deserialize does not accept a grid format"));
                }
                crate::deserialize(&source)?
            }
        };
        let unresolved = [
            &powerio_dist::diagnostics::codes::READ_DSS_INCLUDE_LOAD_FAILED,
            &powerio_dist::diagnostics::codes::READ_DSS_INCLUDE_DEPTH_LIMIT,
        ];
        let incomplete = module
            .diagnostics()
            .iter()
            .any(|d| unresolved.iter().any(|info| d.code() == info.code));
        Ok(CollectionResult {
            module,
            dependencies: source
                .dependencies()
                .iter()
                .map(|b| b.name().to_owned())
                .collect(),
            incomplete,
        })
    }
}

impl Collection {
    fn dataset_candidates(&self) -> Vec<CollectionCandidate> {
        let names: std::collections::BTreeSet<_> =
            self.entries().iter().map(|e| e.path.as_str()).collect();
        self.entries()
            .iter()
            .filter_map(|entry| {
                let (root, leaf) = entry
                    .path
                    .rsplit_once('/')
                    .map_or((None, entry.path.as_str()), |(r, l)| {
                        (Some(r.to_owned()), l)
                    });
                let format = match leaf {
                    "network.csv" => "pypsa-csv",
                    "bus_data.parquet" => "gridfm",
                    _ => return None,
                };
                let prefix = root.as_ref().map_or_else(String::new, |r| format!("{r}/"));
                let required: &[&str] = if format == "gridfm" {
                    &["gen_data.parquet", "branch_data.parquet"]
                } else {
                    &["buses.csv"]
                };
                let status = if format == "gridfm" && !cfg!(feature = "gridfm") {
                    CandidateStatus::UnsupportedBuild
                } else if required
                    .iter()
                    .any(|leaf| !names.contains(format!("{prefix}{leaf}").as_str()))
                {
                    CandidateStatus::Incomplete
                } else {
                    CandidateStatus::Selectable
                };
                Some(CollectionCandidate {
                    selection: CollectionSelection {
                        root,
                        primary: None,
                        members: None,
                        format: Some(format.into()),
                        operation: CollectionOperation::Parse,
                    },
                    status,
                })
            })
            .collect()
    }

    fn dataset_source(
        &self,
        selection: &CollectionSelection,
        root: Option<&ArtifactPath>,
    ) -> Result<Source> {
        if selection.operation != CollectionOperation::Parse {
            return Err(selection_error("deserialize requires a primary file"));
        }
        let format = selection.format_token().ok_or_else(|| {
            selection_error("a directory selection requires an explicit dataset format")
        })?;
        let marker = match format {
            "pypsa-csv" => "network.csv",
            "gridfm" if cfg!(feature = "gridfm") => "bus_data.parquet",
            "gridfm" => return Err(selection_error("GridFM support is disabled in this build")),
            _ => {
                return Err(selection_error(
                    "select an explicit primary or a PyPSA/GridFM dataset root",
                ));
            }
        };
        let source = self.tree.source(root, None, self.limits.tree)?;
        source.root_buffer(marker)?;
        // A nested dataset is another selectable input, not a retained part of
        // this dataset. Keep all other companions, including source-only data.
        let nested: Vec<_> = source
            .entry_names()?
            .into_iter()
            .filter_map(|p| {
                let (parent, leaf) = p.as_str().rsplit_once('/')?;
                matches!(leaf, "network.csv" | "bus_data.parquet").then(|| format!("{parent}/"))
            })
            .collect();
        if nested.is_empty() {
            return Ok(source.with_format(FormatId::new(format)?));
        }
        let files = source
            .entry_names()?
            .into_iter()
            .filter(|p| !nested.iter().any(|prefix| p.as_str().starts_with(prefix)))
            .map(|p| source.buffer(&p).map(|b| (p, b.shared_bytes())))
            .collect::<Result<Vec<_>>>()?;
        Ok(SourceTree::new(&self.name, files, self.limits.tree)?
            .source(None, None, self.limits.tree)?
            .with_format(FormatId::new(format)?))
    }
}

impl CollectionSelection {
    /// Select only the named documents, with roles interpreted by the adapter.
    #[must_use]
    pub fn group(format: impl Into<String>, members: Vec<String>) -> Self {
        Self {
            format: Some(format.into()),
            members: Some(members),
            ..Self::default()
        }
    }
}
impl Collection {
    fn group_source(
        &self,
        selection: &CollectionSelection,
        root: Option<&ArtifactPath>,
    ) -> Result<Source> {
        if selection.primary.is_some() || selection.operation != CollectionOperation::Parse {
            return Err(selection_error(
                "document groups require parse and no primary",
            ));
        }
        let format = selection
            .format_token()
            .ok_or_else(|| selection_error("document groups require an explicit format"))?;
        if !matches!(format, "cgmes" | "cim-dist" | "goc3-json") {
            return Err(selection_error(
                "document groups support cgmes, cim-dist, or goc3-json",
            ));
        }
        let members = selection.members.as_ref().expect("checked group");
        if members.is_empty() || members.len() > self.limits.tree.max_files() {
            return Err(selection_error(
                "document group must have a bounded nonempty member list",
            ));
        }
        let scope = self.tree.source(root, None, self.limits.tree)?;
        let mut files = Vec::new();
        for name in members {
            let path = ArtifactPath::new(name)?;
            if format == "cim-dist" && name.contains('/') {
                return Err(selection_error(
                    "CIM group members must be top-level XML files relative to the selected root",
                ));
            }
            let extension = name.rsplit('.').next().unwrap_or_default();
            if !match format {
                "cgmes" => {
                    extension.eq_ignore_ascii_case("xml") || extension.eq_ignore_ascii_case("zip")
                }
                "cim-dist" => extension.eq_ignore_ascii_case("xml"),
                _ => extension.eq_ignore_ascii_case("json"),
            } {
                return Err(selection_error(
                    "group member suffix does not match the explicit format",
                ));
            }
            let bytes = scope.root_buffer(name)?.shared_bytes();
            files.push((path, bytes));
        }
        Ok(SourceTree::new(&self.name, files, self.limits.tree)?
            .source(None, None, self.limits.tree)?
            .with_format(FormatId::new(format)?))
    }
}

impl Collection {
    /// Open a folder or transport ZIP. The compressed input read is bounded
    /// before retention; member acquisition then enforces expansion limits.
    pub fn open(path: impl AsRef<Path>, limits: CollectionLimits) -> Result<Self> {
        use std::io::Read;
        let path = path.as_ref();
        if path.is_dir() {
            return Self::from_directory(path, limits);
        }
        let file = std::fs::File::open(path).map_err(|e| Error::new(&codes::IO, e.to_string()))?;
        let mut bytes = Vec::new();
        file.take(limits.archive.max_input_bytes().saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|e| Error::new(&codes::IO, e.to_string()))?;
        Self::from_zip(path.to_string_lossy(), bytes.into(), limits)
    }
}
