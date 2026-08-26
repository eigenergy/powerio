use std::collections::{BTreeMap, BTreeSet, btree_map::Entry};
use std::fmt;

use serde_json::{Map, Value};

use crate::Error;
use crate::records::{DiagnosticId, SourceSpan};
use crate::validation::{MAX_DIAGNOSTIC_CODE_BYTES, sanitize_message, valid_nonempty_text};

fn record_too_large(what: &str, limit: usize) -> Error {
    Error::new(
        &crate::codes::REQUEST_RECORD_TOO_LARGE,
        format!("a diagnostic carries more than {limit} {what}"),
    )
}

fn check_details(details: &Map<String, Value>) -> Result<(), Error> {
    if details.len() > crate::validation::MAX_DIAGNOSTIC_DETAIL_KEYS {
        return Err(record_too_large(
            "detail keys",
            crate::validation::MAX_DIAGNOSTIC_DETAIL_KEYS,
        ));
    }
    if let Some((key, _)) = details
        .iter()
        .find(|(key, _)| !crate::validation::valid_nonempty_text(key))
    {
        return Err(Error::new(
            &crate::codes::REQUEST_RECORD_INVALID_IDENTIFIER,
            format!(
                "diagnostic detail key `{}` is empty, contains NUL, or exceeds its bound",
                bounded_identifier(key)
            ),
        ));
    }
    Ok(())
}

/// Severity of one user facing finding.
///
/// Declared least to most severe so the derived `Ord` gives the dominant
/// severity of a set and a `>= Error` filter keeps meaning.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    /// Adds context to another diagnostic.
    Note,
    /// Reports useful information about a successful operation.
    Remark,
    /// Reports usable data whose semantics were defaulted, approximated, or lost.
    Warning,
    /// Reports invalid data or the finding that ended an operation.
    Error,
}

impl DiagnosticSeverity {
    /// The stable stored and binding spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Note => "note",
            Self::Remark => "remark",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

/// Coarse projection of an operation failure for bindings and exit statuses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErrorCategory {
    Io,
    Request,
    Parse,
    Data,
    Output,
}

impl ErrorCategory {
    pub const ALL: [Self; 5] = [
        Self::Io,
        Self::Request,
        Self::Parse,
        Self::Data,
        Self::Output,
    ];

    pub const TOKENS: [&'static str; 5] = ["io", "request", "parse", "data", "output"];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Io => "io",
            Self::Request => "request",
            Self::Parse => "parse",
            Self::Data => "data",
            Self::Output => "output",
        }
    }
}

/// Pipeline stage encoded by the first segment of a diagnostic code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DiagnosticStage {
    Parse,
    Read,
    Canonicalize,
    Validate,
    Transform,
    Build,
    Emit,
    Bind,
    Partner,
    Request,
}

impl DiagnosticStage {
    pub const ALL: [Self; 10] = [
        Self::Parse,
        Self::Read,
        Self::Canonicalize,
        Self::Validate,
        Self::Transform,
        Self::Build,
        Self::Emit,
        Self::Bind,
        Self::Partner,
        Self::Request,
    ];

    pub const NAMESPACES: [&'static str; 10] = [
        "PARSE",
        "READ",
        "CANONICALIZE",
        "VALIDATE",
        "TRANSFORM",
        "BUILD",
        "EMIT",
        "BIND",
        "PARTNER",
        "REQUEST",
    ];

    #[must_use]
    pub const fn namespace(self) -> &'static str {
        match self {
            Self::Parse => "PARSE",
            Self::Read => "READ",
            Self::Canonicalize => "CANONICALIZE",
            Self::Validate => "VALIDATE",
            Self::Transform => "TRANSFORM",
            Self::Build => "BUILD",
            Self::Emit => "EMIT",
            Self::Bind => "BIND",
            Self::Partner => "PARTNER",
            Self::Request => "REQUEST",
        }
    }

    #[must_use]
    pub fn from_namespace(namespace: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|stage| stage.namespace() == namespace)
    }
}

/// Stable dotted identity of one diagnostic.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DiagnosticCode(Box<str>);

impl DiagnosticCode {
    /// Validate a code read from an external producer.
    pub fn new(code: impl Into<String>) -> Result<Self, crate::Error> {
        let code = code.into();
        if !code_is_well_formed(&code) || code.len() > MAX_DIAGNOSTIC_CODE_BYTES {
            return Err(crate::Error::new(
                &crate::codes::REQUEST_DIAGNOSTIC_INVALID_CODE,
                format!("invalid diagnostic code `{}`", bounded_identifier(&code)),
            ));
        }
        Ok(Self(code.into_boxed_str()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn namespace(&self) -> &str {
        self.0.split('.').next().unwrap_or("")
    }

    #[must_use]
    pub fn stage(&self) -> Option<DiagnosticStage> {
        DiagnosticStage::from_namespace(self.namespace())
    }
}

impl fmt::Display for DiagnosticCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Whether a diagnostic identity is still emitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeStatus {
    Active,
    Retired { since: &'static str },
}

/// One entry in a crate's diagnostic registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiagnosticInfo {
    pub code: &'static str,
    pub severity: DiagnosticSeverity,
    pub category: Option<ErrorCategory>,
    pub summary: &'static str,
    pub status: CodeStatus,
}

impl DiagnosticInfo {
    #[must_use]
    pub const fn new(
        code: &'static str,
        severity: DiagnosticSeverity,
        summary: &'static str,
    ) -> Self {
        Self {
            code,
            severity,
            category: None,
            summary,
            status: CodeStatus::Active,
        }
    }

    #[must_use]
    pub const fn with_category(mut self, category: ErrorCategory) -> Self {
        self.category = Some(category);
        self
    }

    #[must_use]
    pub const fn retired(mut self, since: &'static str) -> Self {
        self.status = CodeStatus::Retired { since };
        self
    }

    #[must_use]
    pub fn namespace(&self) -> &'static str {
        self.code.split('.').next().unwrap_or("")
    }

    #[must_use]
    pub fn stage(&self) -> Option<DiagnosticStage> {
        DiagnosticStage::from_namespace(self.namespace())
    }
}

#[derive(Clone, Debug)]
enum DiagnosticIdentity {
    Registered(&'static DiagnosticInfo),
    External(DiagnosticCode),
}

/// One coded user facing finding.
#[derive(Clone, Debug)]
pub struct Diagnostic {
    identity: DiagnosticIdentity,
    id: Option<DiagnosticId>,
    severity: DiagnosticSeverity,
    message: String,
    target: Option<String>,
    spans: Vec<SourceSpan>,
    related: Vec<DiagnosticId>,
    details: Map<String, Value>,
    suggested_action: Option<String>,
}

impl Diagnostic {
    /// Create a finding carrying a code from an external producer.
    #[must_use]
    pub fn new(
        code: DiagnosticCode,
        severity: DiagnosticSeverity,
        message: impl Into<String>,
    ) -> Self {
        Self {
            identity: DiagnosticIdentity::External(code),
            id: None,
            severity,
            message: sanitize_message(message),
            target: None,
            spans: Vec::new(),
            related: Vec::new(),
            details: Map::new(),
            suggested_action: None,
        }
    }

    /// Create a finding from its registered identity and default severity.
    #[must_use]
    pub fn of(info: &'static DiagnosticInfo, message: impl Into<String>) -> Self {
        Self {
            identity: DiagnosticIdentity::Registered(info),
            id: None,
            severity: info.severity,
            message: sanitize_message(message),
            target: None,
            spans: Vec::new(),
            related: Vec::new(),
            details: Map::new(),
            suggested_action: None,
        }
    }

    #[must_use]
    pub fn code(&self) -> &str {
        match &self.identity {
            DiagnosticIdentity::Registered(info) => info.code,
            DiagnosticIdentity::External(code) => code.as_str(),
        }
    }

    #[must_use]
    pub fn registered_info(&self) -> Option<&'static DiagnosticInfo> {
        match self.identity {
            DiagnosticIdentity::Registered(info) => Some(info),
            DiagnosticIdentity::External(_) => None,
        }
    }

    #[must_use]
    pub fn stage(&self) -> Option<DiagnosticStage> {
        DiagnosticStage::from_namespace(self.code().split('.').next().unwrap_or(""))
    }

    #[must_use]
    pub const fn id(&self) -> Option<&DiagnosticId> {
        self.id.as_ref()
    }

    #[must_use]
    pub const fn severity(&self) -> DiagnosticSeverity {
        self.severity
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    #[must_use]
    pub fn target(&self) -> Option<&str> {
        self.target.as_deref()
    }

    #[must_use]
    pub fn spans(&self) -> &[SourceSpan] {
        &self.spans
    }

    #[must_use]
    pub fn related(&self) -> &[DiagnosticId] {
        &self.related
    }

    #[must_use]
    pub const fn details(&self) -> &Map<String, Value> {
        &self.details
    }

    #[must_use]
    pub fn with_id(mut self, id: DiagnosticId) -> Self {
        self.id = Some(id);
        self
    }

    #[must_use]
    pub fn with_severity(mut self, severity: DiagnosticSeverity) -> Self {
        self.severity = severity;
        self
    }

    /// Name the element this finding is about.
    ///
    /// A locator is identity, so it is stored complete or refused: a truncated
    /// RFC 6901 pointer names a different element, or none. An empty,
    /// oversized, or NUL-bearing locator is a visible error, and the caller
    /// decides whether to keep the finding without one.
    pub fn with_target(mut self, target: impl Into<String>) -> Result<Self, Error> {
        self.set_target(target)?;
        Ok(self)
    }

    /// True when the target is a pointer the stored document can write as is.
    #[must_use]
    pub fn target_is_pointer(&self) -> bool {
        self.target
            .as_deref()
            .is_some_and(crate::validation::valid_rfc6901_pointer)
    }

    pub fn with_span(mut self, span: SourceSpan) -> Result<Self, Error> {
        if self.spans.len() >= crate::validation::MAX_DIAGNOSTIC_SPANS {
            return Err(record_too_large(
                "source spans",
                crate::validation::MAX_DIAGNOSTIC_SPANS,
            ));
        }
        self.spans.push(span);
        Ok(self)
    }

    pub fn with_related(mut self, related: DiagnosticId) -> Result<Self, Error> {
        if self.related.len() >= crate::validation::MAX_DIAGNOSTIC_RELATED {
            return Err(record_too_large(
                "related records",
                crate::validation::MAX_DIAGNOSTIC_RELATED,
            ));
        }
        self.related.push(related);
        Ok(self)
    }

    /// Add one detail entry to a finding already built, under the same key and
    /// count limits the decoder enforces.
    pub fn insert_detail(&mut self, key: impl Into<String>, value: Value) -> Result<(), Error> {
        let key = key.into();
        if !crate::validation::valid_nonempty_text(&key) {
            return Err(Error::new(
                &crate::codes::REQUEST_RECORD_INVALID_IDENTIFIER,
                "a diagnostic detail key must be nonempty and bounded",
            ));
        }
        if !self.details.contains_key(&key)
            && self.details.len() >= crate::validation::MAX_DIAGNOSTIC_DETAIL_KEYS
        {
            return Err(record_too_large(
                "detail keys",
                crate::validation::MAX_DIAGNOSTIC_DETAIL_KEYS,
            ));
        }
        self.details.insert(key, value);
        Ok(())
    }

    /// Replace the target of a finding already built. Same rule as
    /// [`Diagnostic::with_target`]: the complete locator is stored, or the
    /// call fails visibly.
    pub fn set_target(&mut self, target: impl Into<String>) -> Result<(), Error> {
        let target = target.into();
        if !crate::validation::valid_diagnostic_target(&target) {
            return Err(Error::new(
                &crate::codes::REQUEST_RECORD_INVALID_IDENTIFIER,
                "a diagnostic target must be a nonempty bounded locator",
            ));
        }
        self.target = Some(target);
        Ok(())
    }

    pub fn with_details(mut self, details: Map<String, Value>) -> Result<Self, Error> {
        self.set_details(details)?;
        Ok(self)
    }

    /// Replace the details of a finding already built, under the same key and
    /// count limits the decoder enforces.
    pub fn set_details(&mut self, details: Map<String, Value>) -> Result<(), Error> {
        check_details(&details)?;
        self.details = details;
        Ok(())
    }

    /// What a user can do about this finding.
    #[must_use]
    pub fn suggested_action(&self) -> Option<&str> {
        self.suggested_action.as_deref()
    }

    #[must_use]
    pub fn with_suggested_action(mut self, action: impl Into<String>) -> Self {
        self.suggested_action = Some(sanitize_message(action));
        self
    }
}

impl PartialEq for Diagnostic {
    fn eq(&self, other: &Self) -> bool {
        self.code() == other.code()
            && self.id == other.id
            && self.severity == other.severity
            && self.message == other.message
            && self.target == other.target
            && self.spans == other.spans
            && self.related == other.related
            && self.details == other.details
            && self.suggested_action == other.suggested_action
    }
}

/// Check grammar, namespace, uniqueness, summaries, and fatal category data.
pub fn check_registry<'a, I>(entries: I) -> Vec<String>
where
    I: IntoIterator<Item = &'a DiagnosticInfo>,
{
    let mut problems = Vec::new();
    let mut seen = BTreeSet::new();
    for entry in entries {
        let retired = matches!(entry.status, CodeStatus::Retired { .. });
        if !retired
            && (!code_is_well_formed(entry.code) || entry.code.len() > MAX_DIAGNOSTIC_CODE_BYTES)
        {
            problems.push(format!("{}: does not match the code grammar", entry.code));
        } else if !retired && entry.stage().is_none() {
            problems.push(format!(
                "{}: namespace {} is not one PowerIO emits",
                entry.code,
                entry.namespace()
            ));
        }
        if !valid_nonempty_text(entry.summary) {
            problems.push(format!("{}: has no bounded summary", entry.code));
        }
        // A category is the binding and exit status projection of a failure, so
        // it belongs only to a code that can end an operation. An error
        // severity diagnostic does not by itself mean the operation failed, so
        // the reverse is not required.
        if entry.category.is_some() && entry.severity != DiagnosticSeverity::Error {
            problems.push(format!(
                "{}: {} severity declares an error category",
                entry.code,
                entry.severity.as_str()
            ));
        }
        if !seen.insert(entry.code) {
            problems.push(format!("{}: registered twice", entry.code));
        }
    }
    problems
}

/// Check that two crates do not claim the same `NAMESPACE.SCOPE` prefix.
pub fn check_scope_ownership(registries: &[(&str, &[&DiagnosticInfo])]) -> Vec<String> {
    let mut owners: BTreeMap<(&str, &str), &str> = BTreeMap::new();
    let mut problems = Vec::new();
    for (crate_name, entries) in registries {
        for entry in *entries {
            if matches!(entry.status, CodeStatus::Retired { .. }) {
                continue;
            }
            let mut segments = entry.code.split('.');
            let (Some(namespace), Some(scope)) = (segments.next(), segments.next()) else {
                continue;
            };
            match owners.entry((namespace, scope)) {
                Entry::Vacant(slot) => {
                    slot.insert(crate_name);
                }
                Entry::Occupied(slot) if *slot.get() != *crate_name => problems.push(format!(
                    "{namespace}.{scope}: claimed by both {} and {crate_name}",
                    slot.get()
                )),
                Entry::Occupied(_) => {}
            }
        }
    }
    problems
}

/// Check `[A-Z][A-Z0-9_]*(\.[A-Z0-9_]+){2,}`.
#[must_use]
pub fn code_is_well_formed(code: &str) -> bool {
    let mut segments = 0usize;
    for (index, segment) in code.split('.').enumerate() {
        segments += 1;
        if segment.is_empty() {
            return false;
        }
        if index == 0 && !segment.starts_with(|character: char| character.is_ascii_uppercase()) {
            return false;
        }
        if !segment
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return false;
        }
    }
    segments >= 3
}

#[must_use]
pub fn render_diagnostic(diagnostic: &Diagnostic) -> String {
    format!("{}: {}", diagnostic.code(), diagnostic.message())
}

#[must_use]
pub fn render_diagnostics(diagnostics: &[Diagnostic]) -> Vec<String> {
    diagnostics.iter().map(render_diagnostic).collect()
}

fn bounded_identifier(value: &str) -> String {
    const LIMIT: usize = 160;
    if value.len() <= LIMIT {
        return value.to_owned();
    }
    let mut end = LIMIT;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &value[..end])
}

#[cfg(test)]
mod tests {

    #[test]
    fn suggested_action_is_part_of_a_finding_identity() {
        let base = Diagnostic::of(&crate::codes::VALIDATE_TIME_SERIES_SHAPE, "same message");
        let advised = Diagnostic::of(&crate::codes::VALIDATE_TIME_SERIES_SHAPE, "same message")
            .with_suggested_action("rebuild the series");
        assert_ne!(base, advised, "advice changes what the record says to do");
        assert_eq!(
            advised,
            Diagnostic::of(&crate::codes::VALIDATE_TIME_SERIES_SHAPE, "same message")
                .with_suggested_action("rebuild the series")
        );
    }

    #[test]
    fn a_target_is_stored_complete_or_refused() {
        let long = format!("/model/buses/{}", "a".repeat(4_000));
        let kept = Diagnostic::of(&crate::codes::VALIDATE_TIME_SERIES_SHAPE, "long target")
            .with_target(long.clone())
            .unwrap();
        assert_eq!(kept.target(), Some(long.as_str()));

        // Past the bound the call fails visibly, and the record is unchanged:
        // a shortened pointer would name a different element, and a silent
        // drop would change the finding's identity behind the caller's back.
        let oversize = "/".repeat(crate::validation::MAX_DIAGNOSTIC_TARGET_BYTES + 1);
        let mut record = Diagnostic::of(&crate::codes::VALIDATE_TIME_SERIES_SHAPE, "oversize");
        assert!(record.set_target(oversize).is_err());
        assert!(record.set_target("").is_err());
        assert!(record.set_target("with\0nul").is_err());
        assert_eq!(record.target(), None);
        assert!(
            Diagnostic::of(&crate::codes::VALIDATE_TIME_SERIES_SHAPE, "oversize")
                .with_target("/".repeat(crate::validation::MAX_DIAGNOSTIC_TARGET_BYTES + 1))
                .is_err()
        );
    }

    #[test]
    fn builder_paths_enforce_the_same_count_limits_as_the_decoder() {
        let mut record = Diagnostic::of(&crate::codes::VALIDATE_TIME_SERIES_SHAPE, "caps");
        for index in 0..crate::validation::MAX_DIAGNOSTIC_RELATED {
            record = record
                .with_related(DiagnosticId::new(format!("d{index}")).unwrap())
                .unwrap();
        }
        assert!(
            record
                .clone()
                .with_related(DiagnosticId::new("one-too-many").unwrap())
                .is_err()
        );

        for index in 0..crate::validation::MAX_DIAGNOSTIC_DETAIL_KEYS {
            record
                .insert_detail(format!("k{index}"), serde_json::Value::Null)
                .unwrap();
        }
        assert!(
            record
                .insert_detail("one-too-many", serde_json::Value::Null)
                .is_err()
        );
        // Replacing an existing key is not growth.
        assert!(
            record
                .insert_detail("k0", serde_json::Value::Bool(true))
                .is_ok()
        );
        assert!(
            record
                .insert_detail("bad\0key", serde_json::Value::Null)
                .is_err()
        );
    }

    #[test]
    fn stored_findings_meet_the_limits_the_constructors_enforce() {
        let mut document = serde_json::json!({
            "code": "PARTNER.TEST.FINDING",
            "severity": "warning",
            "message": "line one\nline two",
        });
        let record: Diagnostic = serde_json::from_value(document.clone()).unwrap();
        assert_eq!(record.message(), "line one line two");

        document["related"] = serde_json::Value::Array(
            (0..=crate::validation::MAX_DIAGNOSTIC_RELATED)
                .map(|index| serde_json::Value::String(format!("d{index}")))
                .collect(),
        );
        assert!(serde_json::from_value::<Diagnostic>(document.clone()).is_err());

        document["related"] = serde_json::Value::Array(Vec::new());
        document["target"] = serde_json::Value::String(String::new());
        assert!(serde_json::from_value::<Diagnostic>(document).is_err());
    }

    #[test]
    fn stored_detail_keys_meet_the_key_predicate_the_constructors_enforce() {
        let decode = |details: &str| {
            serde_json::from_str::<Diagnostic>(&format!(
                r#"{{"code":"PARTNER.TEST.FINDING","severity":"warning","message":"m","details":{details}}}"#
            ))
        };
        // An empty key and a NUL key are refused at decode, exactly as
        // `insert_detail` refuses them.
        assert!(decode(r#"{"":1}"#).is_err());
        assert!(decode("{\"bad\\u0000key\":1}").is_err());

        // The key count limit holds at the boundary.
        let full: Vec<String> = (0..crate::validation::MAX_DIAGNOSTIC_DETAIL_KEYS)
            .map(|index| format!(r#""k{index}":{index}"#))
            .collect();
        let at_limit = decode(&format!("{{{}}}", full.join(","))).unwrap();
        assert_eq!(
            at_limit.details().len(),
            crate::validation::MAX_DIAGNOSTIC_DETAIL_KEYS
        );
        let over: Vec<String> = (0..=crate::validation::MAX_DIAGNOSTIC_DETAIL_KEYS)
            .map(|index| format!(r#""k{index}":{index}"#))
            .collect();
        assert!(decode(&format!("{{{}}}", over.join(","))).is_err());

        // Anything that decodes can be rebuilt through the constructors.
        let rebuilt = Diagnostic::of(&crate::codes::REQUEST_RECORD_TOO_LARGE, "m")
            .with_details(at_limit.details().clone());
        assert!(rebuilt.is_ok());
    }
    use super::*;

    #[test]
    fn exact_severity_and_category_tokens_are_closed() {
        assert_eq!(
            ErrorCategory::ALL.map(ErrorCategory::as_str),
            ["io", "request", "parse", "data", "output"]
        );
        let severities = [
            DiagnosticSeverity::Error,
            DiagnosticSeverity::Warning,
            DiagnosticSeverity::Remark,
            DiagnosticSeverity::Note,
        ];
        assert_eq!(severities.len(), 4);
    }

    #[test]
    fn transform_namespace_is_exact() {
        assert_eq!(DiagnosticStage::Transform.namespace(), "TRANSFORM");
        assert_eq!(DiagnosticStage::from_namespace("EXTERNAL"), None);
        assert_eq!(DiagnosticStage::ALL.len(), 10);
    }

    #[test]
    fn code_grammar_rejects_malformed_and_oversized_input() {
        for code in [
            "TRANSFORM.DIST.UNKNOWN_BUS",
            "READ.DSS.INCLUDE_REFUSED",
            "EMIT.BMOPF.TRANSFORMER.TAP_COLLAPSED",
        ] {
            assert!(code_is_well_formed(code), "{code}");
            assert!(DiagnosticCode::new(code).is_ok());
        }
        for code in ["", "READ.DSS", "read.dss.bad", "READ..BAD", "1READ.DSS.BAD"] {
            assert!(DiagnosticCode::new(code).is_err(), "{code}");
        }
        let external_namespace = DiagnosticCode::new("EXTERNAL.DIST.UNKNOWN_BUS").unwrap();
        assert_eq!(external_namespace.stage(), None);
        let long = format!("READ.DSS.{}", "A".repeat(MAX_DIAGNOSTIC_CODE_BYTES));
        assert!(DiagnosticCode::new(long).is_err());
    }

    #[test]
    fn rendering_bounds_external_text_and_keeps_one_line() {
        let diagnostic = Diagnostic::of(
            &crate::codes::REQUEST_DIAGNOSTIC_INVALID_CODE,
            format!("first\n{}", "x".repeat(20_000)),
        );
        let line = render_diagnostic(&diagnostic);
        assert!(!line.contains(['\n', '\r']));
        assert!(line.len() < 17_000);
    }
}

// Serialization of a `Diagnostic` is the binding and response form: MCP
// replies, the C ABI's JSON channel, and Python all render the same record.
// It is not the `.pio.json` stored form, which has its own versioned DTO in
// the facade. A record read back carries an external code, because a registry
// entry is a compile time item of whichever crate declared it.
mod wire {
    use serde::de::DeserializeSeed;
    use serde::{Deserialize, Deserializer, Serialize};
    use serde_json::{Map, Value};

    use super::{Diagnostic, DiagnosticCode, DiagnosticIdentity, DiagnosticSeverity};
    use crate::bounded::{BoundedStr, TruncatedStr, bounded_json_map, bounded_vec};
    use crate::validation::{
        MAX_DIAGNOSTIC_CODE_BYTES, MAX_DIAGNOSTIC_DETAIL_KEYS, MAX_DIAGNOSTIC_MESSAGE_DECODE_BYTES,
        MAX_DIAGNOSTIC_RELATED, MAX_DIAGNOSTIC_SPANS, MAX_DIAGNOSTIC_TARGET_BYTES,
        MAX_IDENTIFIER_BYTES,
    };
    use crate::{DiagnosticId, SourceSpan};

    // Each field applies its limit while it decodes, so a hostile document
    // fails at the first excess element or byte instead of after an unbounded
    // `Vec`, map, or `String` has been built.
    #[derive(Serialize, Deserialize)]
    pub(super) struct DiagnosticWire {
        #[serde(deserialize_with = "de_code")]
        code: String,
        severity: DiagnosticSeverity,
        #[serde(deserialize_with = "de_message")]
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<DiagnosticId>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "de_target"
        )]
        target: Option<String>,
        #[serde(
            default,
            skip_serializing_if = "Vec::is_empty",
            deserialize_with = "de_spans"
        )]
        spans: Vec<SourceSpan>,
        #[serde(
            default,
            skip_serializing_if = "Vec::is_empty",
            deserialize_with = "de_related"
        )]
        related: Vec<DiagnosticId>,
        #[serde(
            default,
            skip_serializing_if = "Map::is_empty",
            deserialize_with = "de_details"
        )]
        details: Map<String, Value>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "de_action"
        )]
        suggested_action: Option<String>,
    }

    fn de_code<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
        BoundedStr {
            what: "diagnostic code",
            max_bytes: MAX_DIAGNOSTIC_CODE_BYTES,
        }
        .deserialize(deserializer)
    }

    fn de_message<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
        deserializer.deserialize_str(TruncatedStr {
            max_bytes: MAX_DIAGNOSTIC_MESSAGE_DECODE_BYTES,
        })
    }

    fn de_target<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
        BoundedStr {
            what: "diagnostic target",
            max_bytes: MAX_DIAGNOSTIC_TARGET_BYTES,
        }
        .deserialize(deserializer)
        .map(Some)
    }

    fn de_action<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
        deserializer
            .deserialize_str(TruncatedStr {
                max_bytes: MAX_DIAGNOSTIC_MESSAGE_DECODE_BYTES,
            })
            .map(Some)
    }

    fn de_spans<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<SourceSpan>, D::Error> {
        bounded_vec(deserializer, "source spans", MAX_DIAGNOSTIC_SPANS)
    }

    fn de_related<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<DiagnosticId>, D::Error> {
        bounded_vec(deserializer, "related records", MAX_DIAGNOSTIC_RELATED)
    }

    fn de_details<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Map<String, Value>, D::Error> {
        bounded_json_map(
            deserializer,
            "detail keys",
            MAX_DIAGNOSTIC_DETAIL_KEYS,
            MAX_IDENTIFIER_BYTES,
            crate::validation::valid_nonempty_text,
        )
    }

    impl From<&Diagnostic> for DiagnosticWire {
        fn from(diagnostic: &Diagnostic) -> Self {
            Self {
                code: diagnostic.code().to_owned(),
                severity: diagnostic.severity,
                message: diagnostic.message.clone(),
                id: diagnostic.id.clone(),
                target: diagnostic.target.clone(),
                spans: diagnostic.spans.clone(),
                related: diagnostic.related.clone(),
                details: diagnostic.details.clone(),
                suggested_action: diagnostic.suggested_action.clone(),
            }
        }
    }

    impl TryFrom<DiagnosticWire> for Diagnostic {
        type Error = crate::Error;

        // Serialized input is untrusted. It goes through the same limits the
        // constructors apply, so a document cannot introduce a record the API
        // could not have built.
        fn try_from(wire: DiagnosticWire) -> Result<Self, Self::Error> {
            use crate::validation::{
                MAX_DIAGNOSTIC_RELATED, MAX_DIAGNOSTIC_SPANS, sanitize_message,
                valid_diagnostic_target,
            };

            let refuse = |what: &str, limit: usize| {
                crate::Error::new(
                    &crate::codes::REQUEST_RECORD_TOO_LARGE,
                    format!("a stored diagnostic carries more than {limit} {what}"),
                )
            };
            if wire.spans.len() > MAX_DIAGNOSTIC_SPANS {
                return Err(refuse("source spans", MAX_DIAGNOSTIC_SPANS));
            }
            if wire.related.len() > MAX_DIAGNOSTIC_RELATED {
                return Err(refuse("related records", MAX_DIAGNOSTIC_RELATED));
            }
            // The one details predicate the constructors apply, so the decode
            // limit and the construction limit cannot drift apart.
            super::check_details(&wire.details)?;
            let target = match wire.target {
                Some(target) if !valid_diagnostic_target(&target) => {
                    return Err(crate::Error::new(
                        &crate::codes::REQUEST_RECORD_INVALID_IDENTIFIER,
                        "a stored diagnostic target is empty or oversized",
                    ));
                }
                other => other,
            };
            Ok(Diagnostic {
                identity: DiagnosticIdentity::External(DiagnosticCode::new(wire.code)?),
                id: wire.id,
                severity: wire.severity,
                message: sanitize_message(wire.message),
                target,
                spans: wire.spans,
                related: wire.related,
                details: wire.details,
                suggested_action: wire.suggested_action.map(sanitize_message),
            })
        }
    }
}

impl serde::Serialize for Diagnostic {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        wire::DiagnosticWire::from(self).serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for Diagnostic {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = wire::DiagnosticWire::deserialize(deserializer)?;
        Self::try_from(wire).map_err(serde::de::Error::custom)
    }
}
