//! The collector an emitting pass threads through its call tree.

use powerio_core::DiagnosticInfo;

use crate::legacy_diag::{DiagnosticSeverity, StructuredDiagnostic, render_lines};

/// An ordered set of findings, built up as a reader, a lowering pass, or a
/// writer runs.
///
/// A site names a registered [`DiagnosticInfo`] rather than a loose code, so
/// every emitted code is registered by construction. The text lines a channel
/// carries are rendered from the records by [`Diagnostics::lines`], never
/// collected alongside them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Diagnostics(Vec<StructuredDiagnostic>);

impl Diagnostics {
    #[must_use]
    pub fn new() -> Self {
        Self(Vec::new())
    }

    /// Record a finding at its registered default severity.
    pub fn push(&mut self, info: &'static DiagnosticInfo, message: impl Into<String>) {
        self.0.push(StructuredDiagnostic::of(info, message));
    }

    /// Record a finding at a severity this site raises or lowers.
    pub fn push_at(
        &mut self,
        info: &'static DiagnosticInfo,
        severity: DiagnosticSeverity,
        message: impl Into<String>,
    ) {
        self.0
            .push(StructuredDiagnostic::of(info, message).with_severity(severity));
    }

    /// Record a finding built with the record's own builders.
    pub fn record(&mut self, diagnostic: StructuredDiagnostic) {
        self.0.push(diagnostic);
    }

    /// Record every finding of another set, in order.
    pub fn absorb(&mut self, other: impl IntoIterator<Item = StructuredDiagnostic>) {
        self.0.extend(other);
    }

    /// Put `other`'s findings ahead of this set's, which is what a conversion
    /// does with the read side.
    pub fn prepend(&mut self, other: impl IntoIterator<Item = StructuredDiagnostic>) {
        let mut front: Vec<_> = other.into_iter().collect();
        front.append(&mut self.0);
        self.0 = front;
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn records(&self) -> &[StructuredDiagnostic] {
        &self.0
    }

    #[must_use]
    pub fn into_records(self) -> Vec<StructuredDiagnostic> {
        self.0
    }

    /// The `CODE: message` lines for the text channels.
    #[must_use]
    pub fn lines(&self) -> Vec<String> {
        render_lines(&self.0)
    }

    /// The worst severity recorded, or `None` when nothing was.
    #[must_use]
    pub fn worst_severity(&self) -> Option<DiagnosticSeverity> {
        self.0.iter().map(|d| d.severity).max()
    }
}

impl From<Vec<StructuredDiagnostic>> for Diagnostics {
    fn from(records: Vec<StructuredDiagnostic>) -> Self {
        Self(records)
    }
}

impl From<Diagnostics> for Vec<StructuredDiagnostic> {
    fn from(diagnostics: Diagnostics) -> Self {
        diagnostics.0
    }
}

impl IntoIterator for Diagnostics {
    type Item = StructuredDiagnostic;
    type IntoIter = std::vec::IntoIter<StructuredDiagnostic>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DROPPED: DiagnosticInfo = DiagnosticInfo::new(
        "EMIT.PSSE.FIELD_DROPPED",
        powerio_core::DiagnosticSeverity::Warning,
        "a field with no PSS/E record was dropped",
    );
    const REFUSED: DiagnosticInfo = DiagnosticInfo::new(
        "READ.DSS.INCLUDE_REFUSED",
        powerio_core::DiagnosticSeverity::Error,
        "an include escaping the case directory was refused",
    );

    #[test]
    fn a_site_takes_the_registered_severity_unless_it_names_one() {
        let mut d = Diagnostics::new();
        d.push(&DROPPED, "gencost dropped");
        d.push_at(&DROPPED, DiagnosticSeverity::Info, "areas dropped");
        assert_eq!(d.records()[0].severity, DiagnosticSeverity::Warning);
        assert_eq!(d.records()[1].severity, DiagnosticSeverity::Info);
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
}
