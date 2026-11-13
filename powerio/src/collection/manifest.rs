use super::{
    ArtifactPath, Collection, CollectionLimits, CollectionResult, CollectionSelection, Error,
    ParseOptions, Result, codes, selection_error,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, sync::Arc};

/// Structured transport origin, separate from network values and display names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CollectionOrigin {
    pub collection: String,
    /// One relative member name per explicitly opened ZIP boundary.
    pub containers: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SelectionOrigin {
    pub origin: CollectionOrigin,
    pub selection: CollectionSelection,
}

/// Native options recorded with a selection, without host acquisition or code.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CollectionReaderOptions {
    pub variant: Option<i64>,
    pub snapshot_hours: Option<f64>,
    pub acquired_tables: Option<String>,
    #[serde(default)]
    pub assume_inactive_source_controls: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CollectionCase {
    pub id: String,
    #[serde(default)]
    pub containers: Vec<String>,
    pub selection: CollectionSelection,
    /// Explicit associations only; attachments are not silently parsed/merged.
    #[serde(default)]
    pub attachments: Vec<String>,
    pub reader_options: Option<CollectionReaderOptions>,
}
/// Optional untrusted selection catalog. It cannot change host limits or features.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CollectionManifest {
    pub schema: String,
    pub version: u32,
    pub cases: Vec<CollectionCase>,
}
impl CollectionManifest {
    pub fn new(cases: Vec<CollectionCase>) -> Self {
        Self {
            schema: "powerio-collection".into(),
            version: 1,
            cases,
        }
    }
    fn validate(&self, limits: CollectionLimits) -> Result<()> {
        if self.schema != "powerio-collection" || self.version != 1 {
            return Err(selection_error(
                "unsupported collection manifest identity/version",
            ));
        }
        if self.cases.len() > limits.tree.max_files() {
            return Err(selection_error("manifest case limit exceeded"));
        }
        let mut ids = BTreeSet::new();
        for case in &self.cases {
            if case.id.is_empty()
                || case.id.len() > 128
                || !case
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
                || !case.id.as_bytes()[0].is_ascii_alphanumeric()
                || !ids.insert(&case.id)
            {
                return Err(selection_error(
                    "manifest IDs must be unique bounded portable identifiers",
                ));
            }
            if case.containers.len() > limits.max_zip_layers as usize
                || case.attachments.len() > limits.tree.max_files()
            {
                return Err(selection_error("manifest scope limit exceeded"));
            }
            for path in case
                .containers
                .iter()
                .chain(case.attachments.iter())
                .chain(case.selection.root.iter())
                .chain(case.selection.primary.iter())
                .chain(case.selection.members.iter().flatten())
            {
                ArtifactPath::new(path)?;
            }
            if case
                .selection
                .members
                .as_ref()
                .is_some_and(|m| m.len() > limits.tree.max_files())
            {
                return Err(selection_error("manifest member limit exceeded"));
            }
            case.options()?;
        }
        Ok(())
    }
    /// Deterministic metadata encoding. Case order remains user-controlled.
    pub fn to_json(&self) -> Result<Vec<u8>> {
        // This bound also applies to manually constructed catalogs.
        struct Limited(Vec<u8>);
        impl std::io::Write for Limited {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if bytes.len() > (1 << 20) - self.0.len() {
                    return Err(std::io::Error::other("manifest exceeds 1 MiB"));
                }
                self.0.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut writer = Limited(Vec::new());
        serde_json::to_writer_pretty(&mut writer, self)
            .map_err(|e| selection_error(e.to_string()))?;
        let bytes = writer.0;
        Ok(bytes)
    }
}
impl CollectionCase {
    /// A case with no containers, attachments, or reader options. The other
    /// public fields can be set on the returned value.
    #[must_use]
    pub fn new(id: impl Into<String>, selection: CollectionSelection) -> Self {
        Self {
            id: id.into(),
            containers: Vec::new(),
            selection,
            attachments: Vec::new(),
            reader_options: None,
        }
    }
    fn options(&self) -> Result<ParseOptions> {
        #[cfg(feature = "sincal")]
        let mut options = ParseOptions::default();
        #[cfg(not(feature = "sincal"))]
        let options = ParseOptions::default();
        if let Some(reader) = &self.reader_options {
            if reader
                .snapshot_hours
                .is_some_and(|v| !v.is_finite() || !(0.0..=24.0).contains(&v))
            {
                return Err(selection_error(
                    "snapshot_hours must be finite and in [0,24]",
                ));
            }
            if let Some(path) = &reader.acquired_tables {
                ArtifactPath::new(path)?;
            }
            match self.selection.format_token() {
                #[cfg(feature = "sincal")]
                Some("sincal-balanced") => {
                    if reader.assume_inactive_source_controls {
                        return Err(selection_error(
                            "inactive controls apply only to the multiconductor profile",
                        ));
                    }
                    let mut selected =
                        powerio_tnd::formats::sincal::SincalBalancedReadOptions::default();
                    selected.variant = reader.variant;
                    selected.snapshot_hours = reader.snapshot_hours;
                    selected.acquired_tables.clone_from(&reader.acquired_tables);
                    options.sincal_balanced = Some(selected);
                }
                #[cfg(feature = "sincal")]
                Some("sincal-multiconductor") => {
                    let mut selected = powerio_tnd::formats::sincal::SincalReadOptions::default();
                    selected.variant = reader.variant;
                    selected.snapshot_hours = reader.snapshot_hours;
                    selected.acquired_tables.clone_from(&reader.acquired_tables);
                    selected.assume_inactive_source_controls =
                        reader.assume_inactive_source_controls;
                    options.sincal_multiconductor = Some(selected);
                }
                _ => {
                    return Err(selection_error(
                        "reader_options require a supported explicit SINCAL profile",
                    ));
                }
            }
        }
        Ok(options)
    }
}

/// Explicit export policy. No policy infers editable-project completeness from
/// dependency observations; use ExplicitMembers to export an observed subset.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum SourceBundleScope {
    /// Only the primary or explicit group. Directory roots need another scope.
    SelectedMembers,
    /// All companions inside the selection's permitted root, including unused files.
    Project,
    /// Exactly these names relative to the selection's root. No implicit additions.
    ExplicitMembers(Vec<String>),
}
#[derive(Clone, Debug)]
pub struct SourceBundle {
    pub origin: SelectionOrigin,
    files: Vec<(ArtifactPath, Arc<[u8]>)>,
}
impl SourceBundle {
    pub fn files(&self) -> &[(ArtifactPath, Arc<[u8]>)] {
        &self.files
    }
    /// Package exact member bytes into a new deterministic ZIP. It is not an
    /// original archive echo and does not rewrite names, references or encodings.
    pub fn deterministic_zip(&self, limits: super::ArchiveLimits) -> Result<Vec<u8>> {
        powerio_archive::package::deterministic_zip(&self.files, limits)
    }
}
impl Collection {
    pub fn origin(&self) -> CollectionOrigin {
        CollectionOrigin {
            collection: self.origin_name.clone(),
            containers: self.containers.clone(),
        }
    }
    pub fn selection_origin(&self, selection: &CollectionSelection) -> SelectionOrigin {
        SelectionOrigin {
            origin: self.origin(),
            selection: selection.clone(),
        }
    }
    /// Read bounded metadata only. Nested paths are validated syntactically;
    /// opening them and validating selected members waits for explicit use.
    pub fn read_manifest(&self, bytes: &[u8]) -> Result<CollectionManifest> {
        self.cancel.check()?;
        if bytes.len() > (1 << 20) || bytes.len() as u64 > self.limits.inspection_bytes {
            return Err(Error::new(
                &codes::LIMIT,
                "manifest metadata limit exceeded",
            ));
        }
        super::nested::Work::new(self.limits.work_bytes).charge(bytes.len() as u64)?;
        let manifest: CollectionManifest =
            serde_json::from_slice(bytes).map_err(|e| selection_error(e.to_string()))?;
        manifest.validate(self.limits)?;
        for case in &manifest.cases {
            if case.containers.is_empty() {
                self.validate_case_scope(case)?;
            }
        }
        Ok(manifest)
    }
    fn validate_case_scope(&self, case: &CollectionCase) -> Result<()> {
        self.source(&case.selection)?;
        let root = case
            .selection
            .root
            .as_ref()
            .map(ArtifactPath::new)
            .transpose()?;
        let source = self.tree.source(root.as_ref(), None, self.limits.tree)?;
        for attachment in &case.attachments {
            source.root_buffer(attachment)?;
        }
        Ok(())
    }
    /// Execute one catalog case explicitly; siblings remain independent.
    pub fn parse_manifest_case(
        &self,
        manifest: &CollectionManifest,
        id: &str,
    ) -> Result<CollectionResult> {
        self.cancel.check()?;
        let metadata = manifest.to_json()?;
        if metadata.len() as u64 > self.limits.inspection_bytes {
            return Err(Error::new(
                &codes::LIMIT,
                "manifest metadata limit exceeded",
            ));
        }
        super::nested::Work::new(self.limits.work_bytes).charge(metadata.len() as u64)?;
        manifest.validate(self.limits)?;
        let case = manifest
            .cases
            .iter()
            .find(|c| c.id == id)
            .ok_or_else(|| selection_error("unknown manifest case ID"))?;
        let mut collection = self.clone();
        for member in &case.containers {
            collection = collection.open_nested(member)?;
        }
        collection.validate_case_scope(case)?;
        collection.parse(&case.selection, &case.options()?)
    }
    /// Export an explicitly requested source scope. Bytes are shared, and the
    /// collection need not stay alive after the bundle has been returned.
    pub fn source_bundle(
        &self,
        selection: &CollectionSelection,
        scope: SourceBundleScope,
    ) -> Result<SourceBundle> {
        self.cancel.check()?;
        self.source(selection)?;
        let root = selection.root.as_ref().map(ArtifactPath::new).transpose()?;
        let source = self.tree.source(root.as_ref(), None, self.limits.tree)?;
        let names = match scope {
            SourceBundleScope::SelectedMembers => selection
                .primary
                .as_ref()
                .map(|p| vec![p.clone()])
                .or_else(|| selection.members.clone())
                .ok_or_else(|| {
                    selection_error("dataset export requires explicit Project or member scope")
                })?,
            SourceBundleScope::Project => source
                .entry_names()?
                .into_iter()
                .map(String::from)
                .collect(),
            SourceBundleScope::ExplicitMembers(names) => names,
        };
        if names.len() > self.limits.tree.max_files() {
            return Err(selection_error("source bundle file limit exceeded"));
        }
        let mut unique = BTreeSet::new();
        let mut files = Vec::new();
        for name in names {
            let path = ArtifactPath::new(&name)?;
            if !unique.insert(path.clone()) {
                return Err(selection_error("duplicate source bundle member"));
            }
            files.push((path, source.root_buffer(&name)?.shared_bytes()));
        }
        files.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(SourceBundle {
            origin: self.selection_origin(selection),
            files,
        })
    }
}
