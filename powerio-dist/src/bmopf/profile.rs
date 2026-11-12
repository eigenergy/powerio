//! The one BMOPF schema PowerIO writes, and the older drafts the reader still
//! accepts.
//!
//! [`BMOPF_SCHEMA_ID`] is the only place PowerIO names the schema. The
//! release a written document states in `meta.schema_version`, the schema's
//! path in the Task Force repository, and the retrieval permalink are all
//! read off it, so a schema update changes one constant (plus the pinned
//! commit and digest).

use std::sync::LazyLock;

/// The `$id` of the BMOPF schema PowerIO writes: the URL of its bytes at the
/// Task Force's release tag in
/// <https://github.com/distribution-system-opt/dsopt-schema>, as proposed in
/// <https://github.com/distribution-system-opt/dsopt-schema/pull/2>.
///
/// Output records it under `meta.provenance.powerio_bmopf.schema_id`.
pub const BMOPF_SCHEMA_ID: &str = "https://raw.githubusercontent.com/distribution-system-opt/dsopt-schema/schema-v0.2.0/schema/bmopf/0.2.0/bmopf.schema.json";

/// The dsopt-schema commit the vendored BMOPF schema is taken from.
pub const BMOPF_PROPOSAL_COMMIT: &str = "6796394f6a4326b61c7fcf5ab5c0d55181b67d25";
/// SHA-256 of the exact UTF-8 schema document at the pinned revision.
pub const BMOPF_PROPOSAL_SHA256: &str =
    "9c9217afe0a62af77506f7d6ce6e0726d03f338f164a1621a7631cb03c638bdc";

/// The repository part of [`BMOPF_SCHEMA_ID`], ahead of the release tag.
const UPSTREAM_RAW: &str =
    "https://raw.githubusercontent.com/distribution-system-opt/dsopt-schema/";

/// The PowerIO commit whose vendored schema copies releases 0.11.2 and later
/// stamped into `meta.$schema`.
const POWERIO_ARCHIVE_COMMIT: &str = "5234df55cd13ad31455697cffbdc16ca50662667";

/// The dsopt-schema revisions of the early draft that PowerIO 0.11 pinned
/// before the proposal was reconciled with the Task Force's schema. Releases
/// 0.11.0 and 0.11.1 stamped their permalinks into `meta.$schema`.
const POWERIO_DRAFT_COMMITS: [&str; 5] = [
    "2560b3bb9cdfee89321e02a12c320c9fb6519af0",
    "5e692902329b06342913b1a57c546a5fae51b2f1",
    "d83cfc9ce61a81e78c544c0561d0d418fa69a4ff",
    "fe8671a74d2fc1a15a499c5b1f66cbb80fc22e12",
    "664b494f2ee31ee76f8f78e7852cdb1f1c9a8e7d",
];

/// The schema's path in the Task Force repository: the part of
/// [`BMOPF_SCHEMA_ID`] after the release tag.
fn upstream_path() -> &'static str {
    BMOPF_SCHEMA_ID
        .strip_prefix(UPSTREAM_RAW)
        .and_then(|rest| rest.split_once('/'))
        .map(|(_tag, path)| path)
        .expect("the schema id is a dsopt-schema raw URL")
}

/// The release a written document states in `meta.schema_version`: the
/// directory the schema document sits in.
pub(super) fn release() -> &'static str {
    upstream_path()
        .rsplit('/')
        .nth(1)
        .expect("the schema id names its release directory")
}

/// The release line `meta.schema_version` values share with [`release`]:
/// everything up to and including its last `.`.
fn release_line() -> &'static str {
    let release = release();
    &release[..=release
        .rfind('.')
        .expect("the release is MAJOR.MINOR.PATCH")]
}

static RETRIEVAL_URL: LazyLock<String> =
    LazyLock::new(|| format!("{UPSTREAM_RAW}{BMOPF_PROPOSAL_COMMIT}/{}", upstream_path()));

/// Immutable upstream retrieval URL of the exact schema document PowerIO
/// vendors: a dsopt-schema commit permalink, whose bytes hash to
/// [`BMOPF_PROPOSAL_SHA256`]. Distinct from the canonical [`BMOPF_SCHEMA_ID`];
/// both go in `meta.provenance`.
#[must_use]
pub fn bmopf_proposal_url() -> &'static str {
    &RETRIEVAL_URL
}

/// Whether a `meta.schema_version` value belongs to the schema PowerIO
/// writes: any patch release of its line, spelled canonically.
pub(super) fn states_current_release(text: &str) -> bool {
    text.strip_prefix(release_line()).is_some_and(|patch| {
        patch == "0"
            || (!patch.is_empty()
                && !patch.starts_with('0')
                && patch.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// Whether a `meta.$schema` value names the schema PowerIO writes. Documents
/// written before `meta.schema_version` existed identify themselves this way.
/// A released schema document carries its release in its own `$id`, so the
/// release is the directory the schema document sits in. A location that
/// names an older draft, including the early draft PowerIO 0.11 wrote,
/// answers `false`.
pub(super) fn names_current_schema(id: &str) -> bool {
    OlderDraft::from_schema_id(id).is_none()
        && (id == BMOPF_SCHEMA_ID || id.contains(&format!("/{}/", release())))
}

/// An older BMOPF draft a document can state. The reader reads it as the
/// schema PowerIO writes and reports `READ.BMOPF.SCHEMA_LEGACY` once.
///
/// The identifiers below are the ones those drafts' documents carry; the
/// reader needs them to recognize that input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OlderDraft {
    /// The first published schema, including the draft locations of the
    /// `bmopf-report` repository, which only ever held it.
    First,
    /// Its revision, which renamed the bus bounds `vpg_min`/`vpg_max`.
    Revised,
    /// The early draft PowerIO 0.11 wrote, before the proposal was
    /// reconciled with the Task Force's schema.
    PowerIo,
}

impl OlderDraft {
    /// The release number the draft's documents state, compared with
    /// `meta.schema_version`.
    pub(super) fn release(self) -> &'static str {
        match self {
            Self::First => "0.1.0",
            Self::Revised => "0.1.1",
            Self::PowerIo => release(),
        }
    }

    /// The older draft a `meta.schema_version` value names, if any.
    pub(super) fn from_release(text: &str) -> Option<Self> {
        [Self::First, Self::Revised]
            .into_iter()
            .find(|draft| draft.release() == text)
    }

    /// The older draft a `meta.$schema` value names, if any.
    pub(super) fn from_schema_id(id: &str) -> Option<Self> {
        let directory = |draft: Self| id.contains(&format!("/{}/", draft.release()));
        if directory(Self::Revised) {
            Some(Self::Revised)
        } else if directory(Self::First)
            || id.contains("bmopf-report")
            || id.contains("draft_bmopf_schema")
        {
            Some(Self::First)
        } else if directory(Self::PowerIo)
            && (id.contains(POWERIO_ARCHIVE_COMMIT)
                || POWERIO_DRAFT_COMMITS
                    .iter()
                    .any(|commit| id.contains(commit)))
        {
            Some(Self::PowerIo)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    #[test]
    fn the_release_and_paths_come_from_the_schema_id() {
        assert!(BMOPF_SCHEMA_ID.ends_with(&format!("/{}", upstream_path())));
        assert!(upstream_path().ends_with(&format!("/{}/bmopf.schema.json", release())));
        assert!(states_current_release(release()));
        assert!(release_line().ends_with('.'));
        assert!(release().starts_with(release_line()));
    }

    #[test]
    fn the_schema_resolves_from_its_own_id_and_retrieval_url() {
        for id in [
            BMOPF_SCHEMA_ID.to_owned(),
            bmopf_proposal_url().to_owned(),
            format!("file:///cases/schema/bmopf/{}/bmopf.schema.json", release()),
        ] {
            assert!(names_current_schema(&id), "{id}");
            assert_eq!(OlderDraft::from_schema_id(&id), None, "{id}");
        }
    }

    #[test]
    fn older_draft_locations_are_legacy() {
        let powerio_archive = format!(
            "https://raw.githubusercontent.com/eigenergy/powerio/{POWERIO_ARCHIVE_COMMIT}/powerio-dist/schemas/bmopf/{}/bmopf.schema.json",
            release()
        );
        let powerio_draft = format!(
            "{UPSTREAM_RAW}{}/schema/bmopf/{}/bmopf.schema.json",
            POWERIO_DRAFT_COMMITS[3],
            release()
        );
        for (id, expected) in [
            (
                "https://raw.githubusercontent.com/distribution-system-opt/dsopt-schema/main/schema/bmopf/0.1.0/bmopf.schema.json",
                OlderDraft::First,
            ),
            (
                "https://raw.githubusercontent.com/distribution-system-opt/dsopt-schema/b75d0d6669c670dca37ae3ec778b3ac405c361ba/schema/bmopf/0.1.0/bmopf.schema.json",
                OlderDraft::First,
            ),
            (
                "https://raw.githubusercontent.com/distribution-system-opt/dsopt-schema/main/schema/bmopf/0.1.1/bmopf.schema.json",
                OlderDraft::Revised,
            ),
            // The two locations the published bmopf-report examples state.
            (
                "https://github.com/frederikgeth/bmopf-report/draft_schema_and_networks",
                OlderDraft::First,
            ),
            (
                "https://raw.githubusercontent.com/frederikgeth/bmopf-report/main/draft_schema_and_networks/draft_bmopf_schema.json",
                OlderDraft::First,
            ),
            // The PowerIO 0.11 archive URLs.
            (
                "https://raw.githubusercontent.com/eigenergy/powerio/5234df55cd13ad31455697cffbdc16ca50662667/powerio-dist/schemas/bmopf/0.1.0/bmopf.schema.json",
                OlderDraft::First,
            ),
            (powerio_archive.as_str(), OlderDraft::PowerIo),
            // The draft permalink PowerIO 0.11.0 and 0.11.1 stamped.
            (powerio_draft.as_str(), OlderDraft::PowerIo),
        ] {
            assert_eq!(OlderDraft::from_schema_id(id), Some(expected), "{id}");
            assert!(!names_current_schema(id), "{id}");
        }
    }

    #[test]
    fn a_schema_location_naming_no_release_resolves_to_none() {
        for id in ["", "https://example.org/bmopf/schema/v1/bmopf.json"] {
            assert!(!names_current_schema(id));
            assert_eq!(OlderDraft::from_schema_id(id), None);
        }
    }

    #[test]
    fn the_proposal_pin_names_the_commit_in_its_retrieval_url() {
        assert!(bmopf_proposal_url().contains(BMOPF_PROPOSAL_COMMIT));
        assert!(bmopf_proposal_url().ends_with(upstream_path()));
        assert!(!POWERIO_DRAFT_COMMITS.contains(&BMOPF_PROPOSAL_COMMIT));
    }

    /// The pin in this module, the archive manifest, and the vendored bytes
    /// agree, so a schema update that misses one of them fails here.
    #[test]
    fn the_pins_match_the_vendored_schema_archive() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("schemas/bmopf");
        let manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(root.join("manifest.json")).unwrap())
                .unwrap();
        let entries = manifest["schemas"].as_array().unwrap();
        assert_eq!(entries.len(), 1, "the archive ships one schema");
        let entry = &entries[0];
        assert_eq!(entry["sha256"], BMOPF_PROPOSAL_SHA256);
        assert_eq!(entry["upstream_commit"], BMOPF_PROPOSAL_COMMIT);
        assert_eq!(entry["schema_id"], BMOPF_SCHEMA_ID);
        let path = root.join(entry["path"].as_str().unwrap());
        let bytes = std::fs::read(&path).unwrap();
        let digest = <sha2::Sha256 as sha2::Digest>::digest(&bytes);
        let hex = digest.iter().fold(String::new(), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        });
        assert_eq!(hex, BMOPF_PROPOSAL_SHA256, "{}", path.display());
        let schema: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(schema["$id"], BMOPF_SCHEMA_ID);
    }
}
