//! The one renderer behind every text warning channel.

use crate::StructuredDiagnostic;

/// Render a finding as one `CODE: message` line.
///
/// The code leads so the position is fixed and a consumer splits at the first
/// `": "`: the left side matches the code grammar, which cannot contain a
/// colon, while messages contain colons freely. The line never contains a
/// newline, because the text channels join findings with one.
#[must_use]
pub fn render_line(diagnostic: &StructuredDiagnostic) -> String {
    // A message is built from runtime data, so the one line invariant is
    // enforced here rather than trusted at every emission site.
    let message = diagnostic
        .message
        .split(['\n', '\r'])
        .map(str::trim)
        .filter(|chunk| !chunk.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    format!("{}: {message}", diagnostic.code)
}

/// Render every finding, in order.
#[must_use]
pub fn render_lines(diagnostics: &[StructuredDiagnostic]) -> Vec<String> {
    diagnostics.iter().map(render_line).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DiagnosticSeverity;

    fn diagnostic(code: &str, message: &str) -> StructuredDiagnostic {
        StructuredDiagnostic::new(code, DiagnosticSeverity::Warning, message)
    }

    #[test]
    fn the_code_leads_and_the_split_recovers_it() {
        let d = diagnostic(
            "PARSE.MATPOWER.BOM_STRIPPED",
            "leading UTF-8 byte order mark removed: a same-format write returns the text without it",
        );
        let line = render_line(&d);
        let (code, message) = line.split_once(": ").expect("a rendered line splits");
        assert_eq!(code, "PARSE.MATPOWER.BOM_STRIPPED");
        assert_eq!(message, d.message);
    }

    #[test]
    fn a_rendered_line_never_contains_a_newline() {
        let d = diagnostic(
            "READ.DSS.INCLUDE_REFUSED",
            "refused\n  ../shared.dss\r\nescapes",
        );
        let line = render_line(&d);
        assert!(!line.contains('\n') && !line.contains('\r'), "{line}");
        assert_eq!(
            line,
            "READ.DSS.INCLUDE_REFUSED: refused ../shared.dss escapes"
        );
    }

    #[test]
    fn the_code_appears_once() {
        let d = diagnostic("EMIT.PSSE.DOWNGRADED", "rev 35 written as rev 33");
        assert_eq!(render_line(&d).matches("EMIT.PSSE.DOWNGRADED").count(), 1);
    }

    #[test]
    fn lines_keep_their_order() {
        let lines = render_lines(&[
            diagnostic("EMIT.PSSE.FIELD_DROPPED", "first"),
            diagnostic("EMIT.PSSE.DOWNGRADED", "second"),
        ]);
        assert_eq!(
            lines,
            [
                "EMIT.PSSE.FIELD_DROPPED: first",
                "EMIT.PSSE.DOWNGRADED: second"
            ]
        );
    }
}
