use std::collections::BTreeMap;
use std::fmt;

use serde_json::Value;

use crate::validation::{valid_nonempty_text, valid_rfc6901_pointer};
use crate::{Error, FormatId};

macro_rules! record_id {
    ($name:ident, $label:literal) => {
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(Box<str>);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, Error> {
                let value = value.into();
                if !valid_nonempty_text(&value) {
                    return Err(Error::new(
                        &crate::codes::REQUEST_RECORD_INVALID_IDENTIFIER,
                        concat!($label, " must be nonempty and bounded"),
                    ));
                }
                Ok(Self(value.into_boxed_str()))
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

record_id!(SourceId, "source ID");
record_id!(DiagnosticId, "diagnostic ID");
record_id!(HistoryId, "history ID");

/// Program identity recorded with a module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Producer {
    name: Box<str>,
    version: Box<str>,
}

impl Producer {
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Result<Self, Error> {
        let name = name.into();
        let version = version.into();
        if !valid_nonempty_text(&name) || !valid_nonempty_text(&version) {
            return Err(Error::new(
                &crate::codes::REQUEST_RECORD_INVALID_IDENTIFIER,
                "producer name and version must be nonempty and bounded",
            ));
        }
        Ok(Self {
            name: name.into_boxed_str(),
            version: version.into_boxed_str(),
        })
    }

    pub(crate) fn powerio() -> Self {
        Self {
            name: "powerio".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        }
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DigestAlgorithm {
    Sha256,
}

impl DigestAlgorithm {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sha256 => "sha256",
        }
    }
}

/// Validated digest attached to a stored source descriptor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Digest {
    algorithm: DigestAlgorithm,
    value: Box<str>,
}

impl Digest {
    pub fn sha256(value: impl Into<String>) -> Result<Self, Error> {
        let value = value.into();
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(Error::new(
                &crate::codes::REQUEST_RECORD_INVALID_DIGEST,
                "a SHA-256 digest must contain 64 lowercase hexadecimal characters",
            ));
        }
        Ok(Self {
            algorithm: DigestAlgorithm::Sha256,
            value: value.into_boxed_str(),
        })
    }

    #[must_use]
    pub const fn algorithm(&self) -> DigestAlgorithm {
        self.algorithm
    }

    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }
}

/// Durable description of one source buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceDescriptor {
    id: SourceId,
    name: Box<str>,
    byte_length: u64,
    format: Option<FormatId>,
    digest: Option<Digest>,
}

impl SourceDescriptor {
    pub fn new(id: SourceId, name: impl Into<String>, byte_length: u64) -> Result<Self, Error> {
        let name = name.into();
        if !valid_nonempty_text(&name) {
            return Err(Error::new(
                &crate::codes::REQUEST_RECORD_INVALID_IDENTIFIER,
                "source name must be nonempty and bounded",
            ));
        }
        Ok(Self {
            id,
            name: name.into_boxed_str(),
            byte_length,
            format: None,
            digest: None,
        })
    }

    #[must_use]
    pub fn id(&self) -> &SourceId {
        &self.id
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn byte_length(&self) -> u64 {
        self.byte_length
    }

    #[must_use]
    pub const fn format(&self) -> Option<&FormatId> {
        self.format.as_ref()
    }

    #[must_use]
    pub const fn digest(&self) -> Option<&Digest> {
        self.digest.as_ref()
    }

    #[must_use]
    pub fn with_format(mut self, format: FormatId) -> Self {
        self.format = Some(format);
        self
    }

    #[must_use]
    pub fn with_digest(mut self, digest: Digest) -> Self {
        self.digest = Some(digest);
        self
    }
}

/// Half open byte range in one module source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceSpan {
    source: SourceId,
    byte_start: u64,
    byte_end: u64,
}

impl SourceSpan {
    pub fn new(source: SourceId, byte_start: u64, byte_end: u64) -> Result<Self, Error> {
        if byte_start > byte_end {
            return Err(Error::new(
                &crate::codes::REQUEST_RECORD_INVALID_SPAN,
                format!("source span {byte_start}..{byte_end} is reversed"),
            ));
        }
        Ok(Self {
            source,
            byte_start,
            byte_end,
        })
    }

    #[must_use]
    pub fn source(&self) -> &SourceId {
        &self.source
    }

    #[must_use]
    pub const fn byte_start(&self) -> u64 {
        self.byte_start
    }

    #[must_use]
    pub const fn byte_end(&self) -> u64 {
        self.byte_end
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SourceRelation {
    Exact,
    Defaulted,
    Inferred,
    ConvertedUnits,
    Aggregated,
    Split,
    Synthetic,
    Transformed,
    RetainedExtra,
}

impl SourceRelation {
    #[must_use]
    pub const fn allows_empty_spans(self) -> bool {
        matches!(self, Self::Defaulted | Self::Synthetic | Self::Transformed)
    }
}

/// Relation between one typed value target and its source bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceMapEntry {
    target: Box<str>,
    relation: SourceRelation,
    spans: Vec<SourceSpan>,
}

impl SourceMapEntry {
    pub fn new(
        target: impl Into<String>,
        relation: SourceRelation,
        spans: Vec<SourceSpan>,
    ) -> Result<Self, Error> {
        let target = target.into();
        if !valid_rfc6901_pointer(&target) {
            return Err(Error::new(
                &crate::codes::REQUEST_RECORD_INVALID_POINTER,
                "a source map target must be an RFC 6901 pointer",
            ));
        }
        if spans.is_empty() && !relation.allows_empty_spans() {
            return Err(Error::new(
                &crate::codes::REQUEST_RECORD_INVALID_SPAN,
                "this source relation requires at least one byte span",
            ));
        }
        Ok(Self {
            target: target.into_boxed_str(),
            relation,
            spans,
        })
    }

    #[must_use]
    pub fn target(&self) -> &str {
        &self.target
    }

    #[must_use]
    pub const fn relation(&self) -> SourceRelation {
        self.relation
    }

    #[must_use]
    pub fn spans(&self) -> &[SourceSpan] {
        &self.spans
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum HistoryKind {
    Parse,
    Upgrade,
    Transform,
    Edit,
    Repair,
}

/// Structured description of an operation that produced the current value.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryEntry {
    id: HistoryId,
    kind: HistoryKind,
    name: Box<str>,
    input_kind: Option<Box<str>>,
    output_kind: Option<Box<str>>,
    parameters: BTreeMap<String, Value>,
    assumptions: Vec<String>,
    losses: Vec<String>,
}

impl HistoryEntry {
    pub fn new(id: HistoryId, kind: HistoryKind, name: impl Into<String>) -> Result<Self, Error> {
        let name = name.into();
        if !valid_nonempty_text(&name) {
            return Err(Error::new(
                &crate::codes::REQUEST_RECORD_INVALID_IDENTIFIER,
                "history operation name must be nonempty and bounded",
            ));
        }
        Ok(Self {
            id,
            kind,
            name: name.into_boxed_str(),
            input_kind: None,
            output_kind: None,
            parameters: BTreeMap::new(),
            assumptions: Vec::new(),
            losses: Vec::new(),
        })
    }

    #[must_use]
    pub fn id(&self) -> &HistoryId {
        &self.id
    }

    #[must_use]
    pub const fn kind(&self) -> HistoryKind {
        self.kind
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn input_kind(&self) -> Option<&str> {
        self.input_kind.as_deref()
    }

    #[must_use]
    pub fn output_kind(&self) -> Option<&str> {
        self.output_kind.as_deref()
    }

    #[must_use]
    pub const fn parameters(&self) -> &BTreeMap<String, Value> {
        &self.parameters
    }

    #[must_use]
    pub fn assumptions(&self) -> &[String] {
        &self.assumptions
    }

    #[must_use]
    pub fn losses(&self) -> &[String] {
        &self.losses
    }

    pub fn with_input_kind(mut self, kind: impl Into<String>) -> Result<Self, Error> {
        self.input_kind = Some(validated_kind(kind.into())?);
        Ok(self)
    }

    pub fn with_output_kind(mut self, kind: impl Into<String>) -> Result<Self, Error> {
        self.output_kind = Some(validated_kind(kind.into())?);
        Ok(self)
    }

    #[must_use]
    pub fn with_parameters(mut self, parameters: BTreeMap<String, Value>) -> Self {
        self.parameters = parameters;
        self
    }

    #[must_use]
    pub fn with_assumption(mut self, assumption: impl Into<String>) -> Self {
        self.assumptions.push(assumption.into());
        self
    }

    #[must_use]
    pub fn with_loss(mut self, loss: impl Into<String>) -> Self {
        self.losses.push(loss.into());
        self
    }
}

fn validated_kind(kind: String) -> Result<Box<str>, Error> {
    if !valid_nonempty_text(&kind) {
        return Err(Error::new(
            &crate::codes::REQUEST_RECORD_INVALID_IDENTIFIER,
            "a history value kind must be nonempty and bounded",
        ));
    }
    Ok(kind.into_boxed_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_and_digests_are_strict() {
        assert!(SourceId::new("").is_err());
        assert!(SourceId::new("x\0y").is_err());
        assert!(SourceId::new("x".repeat(65_537)).is_err());
        assert!(SourceId::new("Case A").is_ok());
        assert!(Digest::sha256("a".repeat(64)).is_ok());
        assert!(Digest::sha256("A".repeat(64)).is_err());
        assert!(Digest::sha256("a".repeat(63)).is_err());
    }

    #[test]
    fn source_map_spans_obey_relation_rules() {
        let id = SourceId::new("input").unwrap();
        assert!(SourceSpan::new(id.clone(), 2, 1).is_err());
        assert!(SourceMapEntry::new("/bus/0", SourceRelation::Exact, Vec::new()).is_err());
        assert!(SourceMapEntry::new("/bus/0", SourceRelation::Defaulted, Vec::new()).is_ok());
        assert!(SourceMapEntry::new("bad", SourceRelation::Synthetic, Vec::new()).is_err());
    }
}
