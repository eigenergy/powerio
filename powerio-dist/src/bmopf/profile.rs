//! The BMOPF schema version a document is written against.

use serde::{Deserialize, Serialize};

/// The BMOPF schema version a document is read as or written against.
///
/// A schema version fixes which element classes exist and where they live.
/// `0.1.0` declares the ten element classes and four transformer subtypes the
/// task force accepts today, sets `additionalProperties: false` on every
/// object, and permits free-form `extras` and `meta.provenance`; the classes outside it
/// travel there. `0.2.0` declares those classes at the top level and gives the
/// transformer taps, winding neutral impedance, and no load admittance their
/// own slots. Unsupported malformed records still require diagnostics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum BmopfProfile {
    /// Schema 0.1.0, the version the task force accepts.
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
pub const BMOPF_PROPOSAL_COMMIT: &str = "2560b3bb9cdfee89321e02a12c320c9fb6519af0";
/// SHA-256 of the exact UTF-8 schema document at the pinned revision.
pub const BMOPF_PROPOSAL_SHA256: &str =
    "7f8ab08476b610d7bc9ecf3539d209b3bc212e8472bc38987afedc1163d71bce";
/// Immutable retrieval location, distinct from the schema's canonical `$id`.
pub const BMOPF_PROPOSAL_URL: &str = "https://raw.githubusercontent.com/distribution-system-opt/dsopt-schema/2560b3bb9cdfee89321e02a12c320c9fb6519af0/schema/bmopf/0.2.0/bmopf.schema.json";

impl BmopfProfile {
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

    /// Retrieval URL for fresh output. Proposal output pins an immutable commit.
    #[must_use]
    pub const fn retrieval_url(self) -> &'static str {
        match self {
            Self::Bmopf010 => SCHEMA_ID_010,
            Self::Bmopf020 => BMOPF_PROPOSAL_URL,
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

    #[test]
    fn the_default_output_version_is_the_one_that_states_every_class() {
        assert_eq!(BmopfProfile::default(), BmopfProfile::Bmopf020);
        assert_eq!(BmopfProfile::default().version(), "0.2.0");
    }

    #[test]
    fn a_version_resolves_from_its_own_schema_id() {
        for expected in [BmopfProfile::Bmopf010, BmopfProfile::Bmopf020] {
            assert_eq!(
                BmopfProfile::from_schema_id(expected.schema_id()),
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
                BmopfProfile::from_schema_id(id),
                Some(BmopfProfile::Bmopf010),
                "{id}"
            );
        }
    }

    #[test]
    fn a_schema_location_naming_no_version_resolves_to_none() {
        assert_eq!(BmopfProfile::from_schema_id(""), None);
        assert_eq!(
            BmopfProfile::from_schema_id("https://example.org/bmopf/schema/v1/bmopf.json"),
            None
        );
    }

    #[test]
    fn a_version_resolves_from_a_relocated_schema_of_the_same_version() {
        assert_eq!(
            BmopfProfile::from_schema_id("file:///cases/schema/bmopf/0.2.0/bmopf.schema.json"),
            Some(BmopfProfile::Bmopf020)
        );
    }
}
