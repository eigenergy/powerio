//! The BMOPF schema version a document is written against, and the earlier
//! versions the reader still accepts.

use serde::{Deserialize, Serialize};

/// The BMOPF schema version PowerIO writes.
///
/// PowerIO writes exactly one version: 0.2.0, as proposed in
/// distribution-system-opt/dsopt-schema#2. It is 0.1.1 plus the classes the
/// task force's draft carried (IBR, control profiles, time series, DC network,
/// the n-winding and regulator transformers, typed geometry), with nothing
/// renamed. The enum stays `#[non_exhaustive]` so a later schema version can
/// be added without breaking callers.
///
/// Documents that state an earlier version (0.1.0, 0.1.1, the `bmopf-report`
/// drafts, or the 0.2.0 draft PowerIO 0.11 wrote) are still read: the reader
/// upgrades them to 0.2.0 and reports `READ.BMOPF.SCHEMA_LEGACY` once.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum BmopfSchemaVersion {
    /// Schema 0.2.0, the proposal in
    /// <https://github.com/distribution-system-opt/dsopt-schema/pull/2>.
    #[default]
    Bmopf020,
}

/// The `$id` of schema 0.2.0: the URL of its bytes at the `schema-v0.2.0` tag.
const SCHEMA_ID_020: &str = "https://raw.githubusercontent.com/distribution-system-opt/dsopt-schema/schema-v0.2.0/schema/bmopf/0.2.0/bmopf.schema.json";

/// Immutable revision of the proposed BMOPF 0.2.0 schema.
pub const BMOPF_PROPOSAL_COMMIT: &str = "6796394f6a4326b61c7fcf5ab5c0d55181b67d25";
/// SHA-256 of the exact UTF-8 schema document at the pinned revision.
pub const BMOPF_PROPOSAL_SHA256: &str =
    "9c9217afe0a62af77506f7d6ce6e0726d03f338f164a1621a7631cb03c638bdc";
/// Immutable retrieval location, distinct from the schema's canonical `$id`.
pub const BMOPF_PROPOSAL_URL: &str = "https://raw.githubusercontent.com/distribution-system-opt/dsopt-schema/6796394f6a4326b61c7fcf5ab5c0d55181b67d25/schema/bmopf/0.2.0/bmopf.schema.json";

/// The PowerIO commit whose vendored schema copies releases 0.11.2 and later
/// stamped into `meta.$schema`.
const POWERIO_ARCHIVE_COMMIT: &str = "5234df55cd13ad31455697cffbdc16ca50662667";

/// The dsopt-schema revisions of the 0.2.0 draft that PowerIO 0.11 pinned
/// before the proposal was reconciled with schema 0.1.1. Releases 0.11.0 and
/// 0.11.1 stamped their permalinks into `meta.$schema`.
const DRAFT_020_COMMITS: [&str; 5] = [
    "2560b3bb9cdfee89321e02a12c320c9fb6519af0",
    "5e692902329b06342913b1a57c546a5fae51b2f1",
    "d83cfc9ce61a81e78c544c0561d0d418fa69a4ff",
    "fe8671a74d2fc1a15a499c5b1f66cbb80fc22e12",
    "664b494f2ee31ee76f8f78e7852cdb1f1c9a8e7d",
];

impl BmopfSchemaVersion {
    /// The schema version string, as `meta.schema_version` states it.
    #[must_use]
    pub const fn version(self) -> &'static str {
        match self {
            Self::Bmopf020 => "0.2.0",
        }
    }

    /// The canonical `$id` of the schema document, independent of retrieval revision.
    #[must_use]
    pub const fn schema_id(self) -> &'static str {
        match self {
            Self::Bmopf020 => SCHEMA_ID_020,
        }
    }

    /// Immutable upstream retrieval URL of the exact schema document PowerIO
    /// vendors: a dsopt-schema commit permalink, whose bytes hash to
    /// [`Self::sha256`]. Distinct from the canonical [`Self::schema_id`],
    /// which the writer stamps into `meta.$schema`; the retrieval URL goes in
    /// `meta.provenance`.
    #[must_use]
    pub const fn retrieval_url(self) -> &'static str {
        match self {
            Self::Bmopf020 => BMOPF_PROPOSAL_URL,
        }
    }

    /// The upstream dsopt-schema commit the vendored schema document is taken from.
    #[must_use]
    pub const fn upstream_commit(self) -> &'static str {
        match self {
            Self::Bmopf020 => BMOPF_PROPOSAL_COMMIT,
        }
    }

    /// SHA-256 of the vendored schema document's exact UTF-8 bytes.
    #[must_use]
    pub const fn sha256(self) -> &'static str {
        match self {
            Self::Bmopf020 => BMOPF_PROPOSAL_SHA256,
        }
    }

    /// The line a `meta.schema_version` release number belongs to: any 0.2.x
    /// release is 0.2.0's line. `None` for a number outside it.
    #[must_use]
    pub fn from_release(release: &str) -> Option<Self> {
        let patch = release.strip_prefix("0.2.")?;
        let canonical = patch == "0"
            || (!patch.is_empty()
                && !patch.starts_with('0')
                && patch.bytes().all(|b| b.is_ascii_digit()));
        canonical.then_some(Self::Bmopf020)
    }

    /// The version a `meta.$schema` value names, or `None` when it names none.
    /// Documents written before 0.2.0 identify themselves this way; a 0.2.x
    /// document states `meta.schema_version` instead.
    ///
    /// A released schema document carries its version in its own `$id`, so the
    /// version is the directory the schema document sits in. A location that
    /// names an earlier schema, including the 0.2.0 draft PowerIO 0.11 wrote,
    /// answers `None`; the reader reads such a document as 0.2.0 and reports
    /// the version it stated.
    #[must_use]
    pub fn from_schema_id(id: &str) -> Option<Self> {
        if LegacySchema::from_schema_id(id).is_some() {
            return None;
        }
        let candidate = Self::Bmopf020;
        (id == candidate.schema_id() || id.contains(&format!("/{}/", candidate.version())))
            .then_some(candidate)
    }
}

/// An earlier BMOPF schema a document can state. The reader reads it as
/// 0.2.0 and reports `READ.BMOPF.SCHEMA_LEGACY` once, naming it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LegacySchema {
    /// Schema 0.1.0, including the draft locations of the `bmopf-report`
    /// repository, which only ever held 0.1.0.
    V010,
    /// Schema 0.1.1, which renamed the bus bounds `vpg_min`/`vpg_max`.
    V011,
    /// The 0.2.0 draft PowerIO 0.11 wrote, before the proposal was
    /// reconciled with 0.1.1.
    Draft020,
}

impl LegacySchema {
    /// The version number the document states, compared with
    /// `meta.schema_version`.
    pub(super) const fn version(self) -> &'static str {
        match self {
            Self::V010 => "0.1.0",
            Self::V011 => "0.1.1",
            Self::Draft020 => "0.2.0",
        }
    }

    /// The earlier release a `meta.schema_version` value names, if any.
    pub(super) fn from_release(release: &str) -> Option<Self> {
        match release {
            "0.1.0" => Some(Self::V010),
            "0.1.1" => Some(Self::V011),
            _ => None,
        }
    }

    /// The name a finding gives the stated schema.
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::V010 => "BMOPF 0.1.0",
            Self::V011 => "BMOPF 0.1.1",
            Self::Draft020 => "the BMOPF 0.2.0 draft PowerIO 0.11 wrote",
        }
    }

    /// The earlier schema a `meta.$schema` value names, if any.
    pub(super) fn from_schema_id(id: &str) -> Option<Self> {
        if id.contains("/0.1.1/") {
            Some(Self::V011)
        } else if id.contains("/0.1.0/")
            || id.contains("bmopf-report")
            || id.contains("draft_bmopf_schema")
        {
            Some(Self::V010)
        } else if id.contains("/0.2.0/")
            && (id.contains(POWERIO_ARCHIVE_COMMIT)
                || DRAFT_020_COMMITS.iter().any(|commit| id.contains(commit)))
        {
            Some(Self::Draft020)
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
    fn the_output_version_is_0_2_0() {
        assert_eq!(BmopfSchemaVersion::default(), BmopfSchemaVersion::Bmopf020);
        assert_eq!(BmopfSchemaVersion::default().version(), "0.2.0");
    }

    #[test]
    fn the_version_resolves_from_its_own_schema_id_and_retrieval_url() {
        let version = BmopfSchemaVersion::Bmopf020;
        for id in [
            version.schema_id(),
            version.retrieval_url(),
            BMOPF_PROPOSAL_URL,
            "file:///cases/schema/bmopf/0.2.0/bmopf.schema.json",
        ] {
            assert_eq!(
                BmopfSchemaVersion::from_schema_id(id),
                Some(version),
                "{id}"
            );
            assert_eq!(LegacySchema::from_schema_id(id), None, "{id}");
        }
    }

    #[test]
    fn earlier_schema_locations_are_legacy() {
        for (id, expected) in [
            (
                "https://raw.githubusercontent.com/distribution-system-opt/dsopt-schema/main/schema/bmopf/0.1.0/bmopf.schema.json",
                LegacySchema::V010,
            ),
            (
                "https://raw.githubusercontent.com/distribution-system-opt/dsopt-schema/b75d0d6669c670dca37ae3ec778b3ac405c361ba/schema/bmopf/0.1.0/bmopf.schema.json",
                LegacySchema::V010,
            ),
            (
                "https://raw.githubusercontent.com/distribution-system-opt/dsopt-schema/main/schema/bmopf/0.1.1/bmopf.schema.json",
                LegacySchema::V011,
            ),
            // The two locations the published bmopf-report examples state.
            (
                "https://github.com/frederikgeth/bmopf-report/draft_schema_and_networks",
                LegacySchema::V010,
            ),
            (
                "https://raw.githubusercontent.com/frederikgeth/bmopf-report/main/draft_schema_and_networks/draft_bmopf_schema.json",
                LegacySchema::V010,
            ),
            // The PowerIO 0.11 archive URLs.
            (
                "https://raw.githubusercontent.com/eigenergy/powerio/5234df55cd13ad31455697cffbdc16ca50662667/powerio-dist/schemas/bmopf/0.1.0/bmopf.schema.json",
                LegacySchema::V010,
            ),
            (
                "https://raw.githubusercontent.com/eigenergy/powerio/5234df55cd13ad31455697cffbdc16ca50662667/powerio-dist/schemas/bmopf/0.2.0/bmopf.schema.json",
                LegacySchema::Draft020,
            ),
            // The draft permalink PowerIO 0.11.0 and 0.11.1 stamped.
            (
                "https://raw.githubusercontent.com/distribution-system-opt/dsopt-schema/fe8671a74d2fc1a15a499c5b1f66cbb80fc22e12/schema/bmopf/0.2.0/bmopf.schema.json",
                LegacySchema::Draft020,
            ),
        ] {
            assert_eq!(LegacySchema::from_schema_id(id), Some(expected), "{id}");
            assert_eq!(BmopfSchemaVersion::from_schema_id(id), None, "{id}");
        }
    }

    #[test]
    fn a_schema_location_naming_no_version_resolves_to_none() {
        for id in ["", "https://example.org/bmopf/schema/v1/bmopf.json"] {
            assert_eq!(BmopfSchemaVersion::from_schema_id(id), None);
            assert_eq!(LegacySchema::from_schema_id(id), None);
        }
    }

    #[test]
    fn the_proposal_pin_names_the_commit_in_its_retrieval_url() {
        let version = BmopfSchemaVersion::Bmopf020;
        assert!(BMOPF_PROPOSAL_URL.contains(BMOPF_PROPOSAL_COMMIT));
        assert_eq!(version.retrieval_url(), BMOPF_PROPOSAL_URL);
        assert_eq!(version.sha256(), BMOPF_PROPOSAL_SHA256);
        assert_eq!(version.upstream_commit(), BMOPF_PROPOSAL_COMMIT);
        assert!(version.retrieval_url().ends_with(&format!(
            "/schema/bmopf/{}/bmopf.schema.json",
            version.version()
        )));
        assert!(!DRAFT_020_COMMITS.contains(&BMOPF_PROPOSAL_COMMIT));
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
        assert_eq!(entries.len(), 1, "the archive ships one schema version");
        let version = BmopfSchemaVersion::Bmopf020;
        let entry = &entries[0];
        assert_eq!(entry["version"], version.version());
        assert_eq!(entry["sha256"], version.sha256());
        assert_eq!(entry["upstream_commit"], version.upstream_commit());
        assert_eq!(entry["schema_id"], version.schema_id());
        let path = root.join(entry["path"].as_str().unwrap());
        let bytes = std::fs::read(&path).unwrap();
        let digest = <sha2::Sha256 as sha2::Digest>::digest(&bytes);
        let hex = digest.iter().fold(String::new(), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        });
        assert_eq!(hex, version.sha256(), "{}", path.display());
        let schema: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(schema["$id"], version.schema_id());
    }
}
