//! The BMOPF schema version a document is written against.

use serde::{Deserialize, Serialize};

/// The BMOPF schema version a document is read as or written against.
///
/// A schema version fixes which element classes exist and where they live.
/// `0.1.0` declares the ten element classes and four transformer subtypes the
/// historical baseline contains, sets `additionalProperties: false` on every
/// object, and permits free-form `extras` and `meta.provenance`; the classes outside it
/// travel there. `0.2.0` declares those classes at the top level and gives the
/// transformer taps, winding neutral impedance, and no load admittance their
/// own slots. Unsupported malformed records still require diagnostics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum BmopfSchemaVersion {
    /// Schema 0.1.0, the historical baseline.
    Bmopf010,
    /// Schema 0.2.0, the proposal in
    /// <https://github.com/distribution-system-opt/dsopt-schema>. The default
    /// output version, because it states every class the model carries.
    #[default]
    Bmopf020,
}

/// The `$id` of schema 0.1.0.
const SCHEMA_ID_010: &str = "https://raw.githubusercontent.com/distribution-system-opt/dsopt-schema/main/schema/bmopf/0.1.0/bmopf.schema.json";

/// The `$id` of schema 0.2.0.
const SCHEMA_ID_020: &str = "https://raw.githubusercontent.com/distribution-system-opt/dsopt-schema/main/schema/bmopf/0.2.0/bmopf.schema.json";

/// Immutable revision of the proposed BMOPF 0.2.0 schema.
pub const BMOPF_PROPOSAL_COMMIT: &str = "655aad45aa345071a69f4a41a88b324f9d0678ac";
/// SHA-256 of the exact UTF-8 schema document at the pinned revision.
pub const BMOPF_PROPOSAL_SHA256: &str =
    "01d022c038e0518e3a54c5fbf7378522fd4120b5ba8a5308b3a70a31af774ae4";
/// Immutable retrieval location, distinct from the schema's canonical `$id`.
pub const BMOPF_PROPOSAL_URL: &str = "https://raw.githubusercontent.com/distribution-system-opt/dsopt-schema/655aad45aa345071a69f4a41a88b324f9d0678ac/schema/bmopf/0.2.0/bmopf.schema.json";

/// The upstream revision that released schema 0.1.0.
const BASELINE_COMMIT_010: &str = "b75d0d6669c670dca37ae3ec778b3ac405c361ba";
/// SHA-256 of the released 0.1.0 schema document.
const BASELINE_SHA256_010: &str =
    "221cfd98cdea69636ba236a5a5e39a75b482736aeb7579045387eb5e5f2a5567";
/// Immutable retrieval location of the released 0.1.0 schema document.
const BASELINE_URL_010: &str = "https://raw.githubusercontent.com/distribution-system-opt/dsopt-schema/b75d0d6669c670dca37ae3ec778b3ac405c361ba/schema/bmopf/0.1.0/bmopf.schema.json";

/// The PowerIO archive locations that releases up to 0.11 stamped into
/// `meta.$schema`. They remain readable aliases of their versions.
const ARCHIVE_URL_010: &str = "https://raw.githubusercontent.com/eigenergy/powerio/5234df55cd13ad31455697cffbdc16ca50662667/powerio-dist/schemas/bmopf/0.1.0/bmopf.schema.json";
const ARCHIVE_URL_020: &str = "https://raw.githubusercontent.com/eigenergy/powerio/5234df55cd13ad31455697cffbdc16ca50662667/powerio-dist/schemas/bmopf/0.2.0/bmopf.schema.json";

impl BmopfSchemaVersion {
    /// The schema version string, as `meta.schema_version` states it.
    #[must_use]
    pub const fn version(self) -> &'static str {
        match self {
            Self::Bmopf010 => "0.1.0",
            Self::Bmopf020 => "0.2.0",
        }
    }

    /// The canonical `$id` of the schema document, independent of retrieval revision.
    #[must_use]
    pub const fn schema_id(self) -> &'static str {
        match self {
            Self::Bmopf010 => SCHEMA_ID_010,
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
            Self::Bmopf010 => BASELINE_URL_010,
            Self::Bmopf020 => BMOPF_PROPOSAL_URL,
        }
    }

    /// The upstream dsopt-schema commit the vendored schema document is taken from.
    #[must_use]
    pub const fn upstream_commit(self) -> &'static str {
        match self {
            Self::Bmopf010 => BASELINE_COMMIT_010,
            Self::Bmopf020 => BMOPF_PROPOSAL_COMMIT,
        }
    }

    /// SHA-256 of the vendored schema document's exact UTF-8 bytes.
    #[must_use]
    pub const fn sha256(self) -> &'static str {
        match self {
            Self::Bmopf010 => BASELINE_SHA256_010,
            Self::Bmopf020 => BMOPF_PROPOSAL_SHA256,
        }
    }

    /// The PowerIO archive URL releases up to 0.11 stamped into `meta.$schema`
    /// for this version. Read as an alias of the version; no longer written.
    #[must_use]
    pub const fn legacy_archive_url(self) -> &'static str {
        match self {
            Self::Bmopf010 => ARCHIVE_URL_010,
            Self::Bmopf020 => ARCHIVE_URL_020,
        }
    }

    /// The version a `meta.$schema` value names, or `None` when it names none.
    ///
    /// A released schema document carries its version in its own `$id`, so the
    /// version is the directory the schema document sits in. Documents written
    /// before the schema moved to its own repository name the draft schema of
    /// the `bmopf-report` repository, which only ever held 0.1.0, so those
    /// locations resolve to 0.1.0 whatever shape they take. A location naming
    /// neither answers `None` rather than choosing a version.
    #[must_use]
    pub fn from_schema_id(id: &str) -> Option<Self> {
        for candidate in [Self::Bmopf010, Self::Bmopf020] {
            if id == candidate.schema_id() || id.contains(&format!("/{}/", candidate.version())) {
                return Some(candidate);
            }
        }
        if id.contains("bmopf-report") || id.contains("draft_bmopf_schema") {
            return Some(Self::Bmopf010);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    #[test]
    fn the_default_output_version_is_the_one_that_states_every_class() {
        assert_eq!(BmopfSchemaVersion::default(), BmopfSchemaVersion::Bmopf020);
        assert_eq!(BmopfSchemaVersion::default().version(), "0.2.0");
    }

    #[test]
    fn a_version_resolves_from_its_own_schema_id() {
        for expected in [BmopfSchemaVersion::Bmopf010, BmopfSchemaVersion::Bmopf020] {
            assert_eq!(
                BmopfSchemaVersion::from_schema_id(expected.schema_id()),
                Some(expected)
            );
        }
    }

    #[test]
    fn the_draft_schema_locations_resolve_to_the_version_that_repository_held() {
        // The two locations the published examples state.
        for id in [
            "https://github.com/frederikgeth/bmopf-report/draft_schema_and_networks",
            "https://raw.githubusercontent.com/frederikgeth/bmopf-report/main/draft_schema_and_networks/draft_bmopf_schema.json",
        ] {
            assert_eq!(
                BmopfSchemaVersion::from_schema_id(id),
                Some(BmopfSchemaVersion::Bmopf010),
                "{id}"
            );
        }
    }

    #[test]
    fn a_schema_location_naming_no_version_resolves_to_none() {
        assert_eq!(BmopfSchemaVersion::from_schema_id(""), None);
        assert_eq!(
            BmopfSchemaVersion::from_schema_id("https://example.org/bmopf/schema/v1/bmopf.json"),
            None
        );
    }

    #[test]
    fn the_retrieval_permalinks_and_legacy_archive_urls_resolve_to_their_versions() {
        for expected in [BmopfSchemaVersion::Bmopf010, BmopfSchemaVersion::Bmopf020] {
            for id in [
                expected.retrieval_url(),
                expected.legacy_archive_url(),
                expected.schema_id(),
            ] {
                assert_ne!(id, "");
                assert_eq!(
                    BmopfSchemaVersion::from_schema_id(id),
                    Some(expected),
                    "{id}"
                );
            }
        }
        assert_eq!(
            BmopfSchemaVersion::from_schema_id(BMOPF_PROPOSAL_URL),
            Some(BmopfSchemaVersion::Bmopf020)
        );
    }

    #[test]
    fn the_proposal_pin_names_the_commit_in_its_retrieval_url() {
        assert!(BMOPF_PROPOSAL_URL.contains(BMOPF_PROPOSAL_COMMIT));
        assert_eq!(
            BmopfSchemaVersion::Bmopf020.retrieval_url(),
            BMOPF_PROPOSAL_URL
        );
        assert_eq!(BmopfSchemaVersion::Bmopf020.sha256(), BMOPF_PROPOSAL_SHA256);
        assert_eq!(
            BmopfSchemaVersion::Bmopf020.upstream_commit(),
            BMOPF_PROPOSAL_COMMIT
        );
        for version in [BmopfSchemaVersion::Bmopf010, BmopfSchemaVersion::Bmopf020] {
            assert!(version.retrieval_url().contains(version.upstream_commit()));
            assert!(version.retrieval_url().ends_with(&format!(
                "/schema/bmopf/{}/bmopf.schema.json",
                version.version()
            )));
        }
    }

    /// The pin in this module, the archive manifest, and the vendored bytes
    /// agree, so a schema update that misses one of them fails here.
    #[test]
    fn the_pins_match_the_vendored_schema_archive() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("schemas/bmopf");
        let manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(root.join("manifest.json")).unwrap())
                .unwrap();
        for version in [BmopfSchemaVersion::Bmopf010, BmopfSchemaVersion::Bmopf020] {
            let entry = manifest["schemas"]
                .as_array()
                .unwrap()
                .iter()
                .find(|entry| entry["version"] == version.version())
                .unwrap_or_else(|| panic!("manifest entry for {}", version.version()));
            assert_eq!(entry["sha256"], version.sha256(), "{}", version.version());
            assert_eq!(
                entry["upstream_commit"],
                version.upstream_commit(),
                "{}",
                version.version()
            );
            assert_eq!(
                entry["schema_id"],
                version.schema_id(),
                "{}",
                version.version()
            );
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

    #[test]
    fn a_version_resolves_from_a_relocated_schema_of_the_same_version() {
        assert_eq!(
            BmopfSchemaVersion::from_schema_id(
                "file:///cases/schema/bmopf/0.2.0/bmopf.schema.json"
            ),
            Some(BmopfSchemaVersion::Bmopf020)
        );
    }
}
