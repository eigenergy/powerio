//! The crate-private collector an emitting pass threads through its call tree.
//!
//! No public PowerIO operation accepts or returns this type, and the 1.0
//! baseline keeps the mutable collector out of every public surface, so each
//! emitting crate carries its own copy of this file. Keep the copies byte
//! identical; the shared record types come from `powerio-core`.

use powerio_core::{
    Diagnostic, DiagnosticInfo, DiagnosticSeverity, SourceId, SourceSpan, render_diagnostics,
};

/// The buffer a reader decodes and the record it is on. While a record is
/// set, every finding the collector records carries that record's byte range
/// as a span into the buffer.
#[derive(Clone, Debug, PartialEq)]
#[allow(
    dead_code,
    reason = "the collector copies stay identical across crates"
)]
pub(crate) struct RecordLocation {
    source: SourceId,
    /// Byte offset of the decoded text within the retained buffer: the
    /// length of the byte order mark the reader never sees.
    base: u64,
    /// Half open byte range of the current record, relative to the decoded
    /// text.
    record: Option<(usize, usize)>,
}

/// An ordered set of findings, built up as a reader, a lowering pass, or a
/// writer runs.
///
/// A site names a registered [`DiagnosticInfo`] rather than a loose code, so
/// every emitted code is registered by construction. The text lines a channel
/// carries are rendered from the records by [`Diagnostics::lines`], never
/// collected alongside them.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Diagnostics {
    records: Vec<Diagnostic>,
    location: Option<RecordLocation>,
}

#[allow(
    dead_code,
    reason = "the collector copies stay identical across crates"
)]
impl Diagnostics {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            records: Vec::new(),
            location: None,
        }
    }

    /// Record a finding at its registered default severity.
    pub(crate) fn push(&mut self, info: &'static DiagnosticInfo, message: impl Into<String>) {
        let diagnostic = self.located(Diagnostic::of(info, message));
        self.records.push(diagnostic);
    }

    /// Record a finding at a severity this site raises or lowers.
    pub(crate) fn push_at(
        &mut self,
        info: &'static DiagnosticInfo,
        severity: DiagnosticSeverity,
        message: impl Into<String>,
    ) {
        let diagnostic = self.located(Diagnostic::of(info, message).with_severity(severity));
        self.records.push(diagnostic);
    }

    /// Record a finding built with the record's own builders. A finding that
    /// already names a span keeps it; one without receives the current
    /// record's span.
    pub(crate) fn record(&mut self, diagnostic: Diagnostic) {
        let diagnostic = self.located(diagnostic);
        self.records.push(diagnostic);
    }

    /// Record every finding of another set, in order.
    pub(crate) fn absorb(&mut self, other: impl IntoIterator<Item = Diagnostic>) {
        self.records.extend(other);
    }

    /// Put `other`'s findings ahead of this set's, which is what a conversion
    /// does with the read side.
    pub(crate) fn prepend(&mut self, other: impl IntoIterator<Item = Diagnostic>) {
        let mut front: Vec<_> = other.into_iter().collect();
        front.append(&mut self.records);
        self.records = front;
    }

    /// Name the buffer the running reader decodes so record spans can be
    /// attached. `base` is the byte offset of the decoded text within the
    /// retained buffer.
    pub(crate) fn locate_in(&mut self, source: SourceId, base: u64) {
        self.location = Some(RecordLocation {
            source,
            base,
            record: None,
        });
    }

    /// Take the location off the collector for a reader that decodes text
    /// other than the retained buffer; [`Diagnostics::resume_location`] puts
    /// it back.
    pub(crate) fn suspend_location(&mut self) -> Option<RecordLocation> {
        self.location.take()
    }

    pub(crate) fn resume_location(&mut self, location: Option<RecordLocation>) {
        self.location = location;
    }

    /// Mark the record being decoded, as a half open byte range of the
    /// decoded text. A no-op when no buffer is located.
    pub(crate) fn enter_record(&mut self, start: usize, end: usize) {
        if let Some(location) = &mut self.location {
            location.record = Some((start, end.max(start)));
        }
    }

    /// Extend the current record to `end`, for a record that continues over
    /// more than one line.
    pub(crate) fn extend_record(&mut self, end: usize) {
        if let Some(location) = &mut self.location
            && let Some((_, record_end)) = &mut location.record
        {
            *record_end = end.max(*record_end);
        }
    }

    /// Leave the current record: findings recorded next carry no span.
    pub(crate) fn leave_record(&mut self) {
        if let Some(location) = &mut self.location {
            location.record = None;
        }
    }

    /// The span of the record being decoded, in the retained buffer's bytes.
    #[must_use]
    pub(crate) fn record_span(&self) -> Option<SourceSpan> {
        let location = self.location.as_ref()?;
        let (start, end) = location.record?;
        SourceSpan::new(
            location.source.clone(),
            location.base + start as u64,
            location.base + end as u64,
        )
        .ok()
    }

    fn located(&self, diagnostic: Diagnostic) -> Diagnostic {
        match self.record_span() {
            Some(span) if diagnostic.spans().is_empty() => diagnostic
                .with_span(span)
                .expect("a finding with no spans is below the span limit"),
            _ => diagnostic,
        }
    }

    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.records.len()
    }

    #[must_use]
    pub(crate) fn records(&self) -> &[Diagnostic] {
        &self.records
    }

    #[must_use]
    pub(crate) fn into_records(self) -> Vec<Diagnostic> {
        self.records
    }

    /// The `CODE: message` lines for the text channels.
    #[must_use]
    pub(crate) fn lines(&self) -> Vec<String> {
        render_diagnostics(&self.records)
    }

    /// The worst severity recorded, or `None` when nothing was.
    #[must_use]
    pub(crate) fn worst_severity(&self) -> Option<DiagnosticSeverity> {
        self.records.iter().map(Diagnostic::severity).max()
    }
}

impl From<Vec<Diagnostic>> for Diagnostics {
    fn from(records: Vec<Diagnostic>) -> Self {
        Self {
            records,
            location: None,
        }
    }
}

impl From<Diagnostics> for Vec<Diagnostic> {
    fn from(diagnostics: Diagnostics) -> Self {
        diagnostics.records
    }
}

impl IntoIterator for Diagnostics {
    type Item = Diagnostic;
    type IntoIter = std::vec::IntoIter<Diagnostic>;

    fn into_iter(self) -> Self::IntoIter {
        self.records.into_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DROPPED: DiagnosticInfo = DiagnosticInfo::new(
        "EMIT.PSSE.FIELD_DROPPED",
        DiagnosticSeverity::Warning,
        "a field with no PSS/E record was dropped",
    );
    const REFUSED: DiagnosticInfo = DiagnosticInfo::new(
        "READ.DSS.INCLUDE_REFUSED",
        DiagnosticSeverity::Error,
        "an include escaping the case directory was refused",
    );

    #[test]
    fn a_site_takes_the_registered_severity_unless_it_names_one() {
        let mut d = Diagnostics::new();
        d.push(&DROPPED, "gencost dropped");
        d.push_at(&DROPPED, DiagnosticSeverity::Remark, "areas dropped");
        assert_eq!(d.records()[0].severity(), DiagnosticSeverity::Warning);
        assert_eq!(d.records()[1].severity(), DiagnosticSeverity::Remark);
        assert_eq!(d.worst_severity(), Some(DiagnosticSeverity::Warning));
    }

    #[test]
    fn the_text_channel_is_rendered_from_the_records() {
        let mut d = Diagnostics::new();
        d.push(&DROPPED, "gencost dropped");
        d.push(&REFUSED, "../shared.dss escapes the case directory");
        assert_eq!(
            d.lines(),
            [
                "EMIT.PSSE.FIELD_DROPPED: gencost dropped",
                "READ.DSS.INCLUDE_REFUSED: ../shared.dss escapes the case directory",
            ]
        );
    }

    #[test]
    fn the_read_side_goes_ahead_of_the_write_side() {
        let mut write = Diagnostics::new();
        write.push(&DROPPED, "write");
        let mut read = Diagnostics::new();
        read.push(&REFUSED, "read");
        write.prepend(read);
        assert_eq!(
            write.lines(),
            [
                "READ.DSS.INCLUDE_REFUSED: read",
                "EMIT.PSSE.FIELD_DROPPED: write",
            ]
        );
        assert_eq!(write.len(), 2);
        assert!(!write.is_empty());
    }

    #[test]
    fn findings_carry_the_current_record_span_while_located() {
        let source = SourceId::new("/input").unwrap();
        let mut d = Diagnostics::new();
        d.enter_record(0, 4);
        d.push(&DROPPED, "before any buffer is located");
        assert!(d.records()[0].spans().is_empty());

        d.locate_in(source.clone(), 3);
        d.enter_record(10, 20);
        d.extend_record(25);
        d.push(&DROPPED, "inside a record");
        let span = &d.records()[1].spans()[0];
        assert_eq!(span.source(), &source);
        assert_eq!((span.byte_start(), span.byte_end()), (13, 28));
        assert_eq!(d.record_span().as_ref(), Some(span));

        d.leave_record();
        d.push(&DROPPED, "between records");
        assert!(d.records()[2].spans().is_empty());
        assert_eq!(d.record_span(), None);

        d.enter_record(30, 31);
        let location = d.suspend_location();
        d.push(&DROPPED, "while suspended");
        assert!(d.records()[3].spans().is_empty());
        d.resume_location(location);
        d.push(&DROPPED, "after resuming");
        assert_eq!(d.records()[4].spans()[0].byte_start(), 33);
    }
}
