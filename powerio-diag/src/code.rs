//! Diagnostic codes and the stage family their first segment names.

use serde::{Deserialize, Serialize};

/// A stable dotted diagnostic code, e.g. `EMIT.BMOPF.TRANSFORMER_UNSUPPORTED`.
///
/// The grammar is `NAMESPACE.SCOPE.SPECIFIC`: uppercase ASCII letters, digits
/// and `_` inside a segment, `.` between segments, at least three segments. A
/// large scope may use more (`EMIT.BMOPF.TRANSFORMER.TAP_COLLAPSED`), so a
/// consumer reads the first segment and treats the rest as opaque identity.
///
/// The first segment is the namespace and names the stage the finding came
/// from; [`DiagnosticStage`] decodes it. A code carried by a document powerio
/// did not write may use a namespace outside the ten, which is data rather than
/// a failure.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct DiagnosticCode(pub String);

impl DiagnosticCode {
    pub fn new(code: impl Into<String>) -> Self {
        Self(code.into())
    }

    /// The leading dotted segment (the namespace), e.g. `EMIT` for
    /// `EMIT.PSSE.FIELD_DROPPED`.
    pub fn namespace(&self) -> &str {
        self.0.split('.').next().unwrap_or("")
    }

    /// The stage this code names, or `None` when the namespace is outside the
    /// ten powerio emits.
    pub fn stage(&self) -> Option<DiagnosticStage> {
        DiagnosticStage::from_namespace(self.namespace())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this code matches the grammar. Nothing refuses a code that does
    /// not; the registry gates check it so a new code cannot be minted
    /// malformed.
    pub fn is_well_formed(&self) -> bool {
        code_is_well_formed(&self.0)
    }
}

/// Whether `code` matches `[A-Z][A-Z0-9_]*(\.[A-Z0-9_]+)+` with at least three
/// segments.
#[must_use]
pub fn code_is_well_formed(code: &str) -> bool {
    let mut segments = 0usize;
    for (i, segment) in code.split('.').enumerate() {
        segments += 1;
        if segment.is_empty() {
            return false;
        }
        if i == 0 && !segment.starts_with(|c: char| c.is_ascii_uppercase()) {
            return false;
        }
        if !segment
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        {
            return false;
        }
    }
    segments >= 3
}

impl std::fmt::Display for DiagnosticCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for DiagnosticCode {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl From<String> for DiagnosticCode {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// The stage a finding came from, decoded from the first segment of its code.
///
/// `PARSE` is source text or bytes that could not be decoded; `READ` is decoded
/// input that could not be represented in the model, plus read side I/O.
/// `CANONICALIZE` is normalization, `VALIDATE` is the document's own internal
/// consistency, `LOWER` is a transformation between model families, `BUILD` is
/// assembling a derived object (an index, a matrix, a solver table) from a
/// network that already parsed. `EMIT` is serialization and write side I/O.
/// `BIND` is the language boundary itself, `PARTNER` is a partner tool, and
/// `REQUEST` is a call naming something powerio does not provide.
///
/// A failure detectable from an argument's representation alone is `BIND`; one
/// that needs powerio's own vocabulary to detect is `REQUEST`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DiagnosticStage {
    Parse,
    Read,
    Canonicalize,
    Validate,
    Lower,
    Build,
    Emit,
    Bind,
    Partner,
    Request,
}

impl DiagnosticStage {
    /// Every stage, in pipeline order.
    pub const ALL: [DiagnosticStage; 10] = [
        DiagnosticStage::Parse,
        DiagnosticStage::Read,
        DiagnosticStage::Canonicalize,
        DiagnosticStage::Validate,
        DiagnosticStage::Lower,
        DiagnosticStage::Build,
        DiagnosticStage::Emit,
        DiagnosticStage::Bind,
        DiagnosticStage::Partner,
        DiagnosticStage::Request,
    ];

    /// Every namespace powerio emits, for a consumer that wants the set without
    /// hardcoding it. A code whose first segment is outside this set was
    /// written by someone else.
    pub const NAMESPACES: [&'static str; 10] = [
        "PARSE",
        "READ",
        "CANONICALIZE",
        "VALIDATE",
        "LOWER",
        "BUILD",
        "EMIT",
        "BIND",
        "PARTNER",
        "REQUEST",
    ];

    /// The namespace segment this stage owns, e.g. `EMIT`.
    #[must_use]
    pub fn namespace(self) -> &'static str {
        match self {
            DiagnosticStage::Parse => "PARSE",
            DiagnosticStage::Read => "READ",
            DiagnosticStage::Canonicalize => "CANONICALIZE",
            DiagnosticStage::Validate => "VALIDATE",
            DiagnosticStage::Lower => "LOWER",
            DiagnosticStage::Build => "BUILD",
            DiagnosticStage::Emit => "EMIT",
            DiagnosticStage::Bind => "BIND",
            DiagnosticStage::Partner => "PARTNER",
            DiagnosticStage::Request => "REQUEST",
        }
    }

    /// The stage a namespace segment names, or `None` for a namespace outside
    /// the ten.
    #[must_use]
    pub fn from_namespace(namespace: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.namespace() == namespace)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grammar_accepts_shipped_codes_and_refuses_malformed_ones() {
        for code in [
            "EMIT.BMOPF.TRANSFORMER_UNSUPPORTED",
            "READ.DSS.INCLUDE_REFUSED",
            "LOWER.MULTI_TO_BALANCED.UNKNOWN_BUS",
            "EMIT.BMOPF.TRANSFORMER.TAP_COLLAPSED",
            "VALIDATE.PACKAGE.OPERATING_IDENTITY",
        ] {
            assert!(code_is_well_formed(code), "{code}");
        }
        for code in [
            "",
            "EMIT",
            "READ.PACKAGE",
            "read.dss.include_refused",
            "READ..INCLUDE_REFUSED",
            "READ.DSS.INCLUDE REFUSED",
            "READ.DSS.INCLUDE-REFUSED",
            "1READ.DSS.INCLUDE_REFUSED",
            "READ.DSS.INCLUDE_REFUSED.",
        ] {
            assert!(!code_is_well_formed(code), "{code}");
        }
    }

    #[test]
    fn every_namespace_decodes_to_its_stage_and_back() {
        assert_eq!(
            DiagnosticStage::ALL.len(),
            DiagnosticStage::NAMESPACES.len()
        );
        for (stage, namespace) in DiagnosticStage::ALL
            .into_iter()
            .zip(DiagnosticStage::NAMESPACES)
        {
            assert_eq!(stage.namespace(), namespace);
            assert_eq!(DiagnosticStage::from_namespace(namespace), Some(stage));
        }
        assert_eq!(DiagnosticStage::from_namespace("FIDELITY"), None);
    }

    #[test]
    fn a_code_reports_the_stage_of_its_first_segment() {
        let code = DiagnosticCode::new("EMIT.PSSE.FIELD_DROPPED");
        assert_eq!(code.namespace(), "EMIT");
        assert_eq!(code.stage(), Some(DiagnosticStage::Emit));
        assert_eq!(DiagnosticCode::new("E.PSSE.DROPPED").stage(), None);
    }

    #[test]
    fn a_stage_serializes_as_its_lowercase_token() {
        let json = serde_json::to_string(&DiagnosticStage::Request).unwrap();
        assert_eq!(json, "\"request\"");
        assert_eq!(
            serde_json::from_str::<DiagnosticStage>("\"build\"").unwrap(),
            DiagnosticStage::Build
        );
    }
}
