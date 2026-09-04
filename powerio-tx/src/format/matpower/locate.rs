//! Locate `mpc.<field> = …;` assignments in MATPOWER `.m` source.
//!
//! The parser borrows each assignment's raw text straight from the source and
//! hands it to the typed row/scalar/cell parsers. Lossless round-trip needs no
//! structured model here: [`BalancedNetwork`](crate::BalancedNetwork) keeps the original source
//! text and the writer echoes it, so this module only has to find where each
//! field's text begins and ends.

use super::tokens;

/// A line's text with its `\n`/`\r\n` terminator trimmed off (`str::lines`
/// semantics) given a `split_inclusive('\n')` piece.
#[inline]
pub(super) fn trim_eol(piece: &str) -> &str {
    piece
        .strip_suffix('\n')
        .map_or(piece, |s| s.strip_suffix('\r').unwrap_or(s))
}

/// One `mpc.<field> = ...;` assignment: its field name, its complete text,
/// and the byte offset of that text within the source.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Assignment<'a> {
    pub(crate) field: &'a str,
    pub(crate) text: &'a str,
    pub(crate) start: usize,
}

impl Assignment<'_> {
    /// The half open byte range of the assignment text within the source.
    pub(crate) fn range(&self) -> (usize, usize) {
        (self.start, self.start + self.text.len())
    }
}

/// Locate each `mpc.<field> = <rhs>;` assignment's text, borrowing the field
/// name and the complete assignment from `content` in source order. For a
/// numeric `[ ... ]` matrix it scans for the closing `]` directly (numeric
/// bodies never nest brackets) and uses the quote-aware depth FSM only for
/// `{ ... }` cell arrays (whose strings may hold `]`/`}`). Infallible: an
/// unclosed block runs to EOF and [`super::matlab::for_each_matrix_row`]
/// reports the truncation.
///
/// One forward pass over `content.split_inclusive('\n')` with a running byte
/// offset, so no `Vec` of every line is materialized (which on a 56 MB /
/// 192k-bus case is tens of MB written before a single field is found). A
/// multi-line block consumes following lines from the same iterator, so the
/// next assignment starts after the block's closing line.
pub(crate) fn locate_assignments(content: &str) -> Vec<Assignment<'_>> {
    let mut out = Vec::new();
    let mut off = 0usize;
    let mut lines = content.split_inclusive('\n');
    while let Some(piece) = lines.next() {
        let start = off;
        off += piece.len();
        let line = trim_eol(piece);
        let mut end = start + line.len();
        let (code, _comment) = tokens::comment_split(line);
        if let Some((field, rhs)) = parse_assignment_start(code) {
            if rhs.starts_with('[') {
                // Numeric matrix: the first un-commented `]` closes it. The opening
                // line's `code` already holds the `]` for a single-line matrix.
                if !code.contains(']') {
                    for piece in lines.by_ref() {
                        let s = off;
                        off += piece.len();
                        let l = trim_eol(piece);
                        end = s + l.len();
                        if tokens::comment_split(l).0.contains(']') {
                            break;
                        }
                    }
                }
            } else if rhs.starts_with('{') {
                let mut depth = net_bracket_depth(code);
                while depth > 0 {
                    let Some(piece) = lines.next() else { break };
                    let s = off;
                    off += piece.len();
                    let l = trim_eol(piece);
                    end = s + l.len();
                    depth += net_bracket_depth(tokens::comment_split(l).0);
                }
            }
            out.push(Assignment {
                field,
                text: &content[start..end],
                start,
            });
        }
    }
    out
}

/// Extract the quoted strings from a `{ '...'; '...' }` cell array assignment,
/// in order. Used for `mpc.bus_name` / `gentype` / `genfuel`. Tolerant: it
/// scans the raw assignment text for `'…'` (or `"…"`) runs, so the field name
/// and the braces/semicolons are simply skipped. A doubled quote (`''`) is the
/// MATLAB escape for a literal quote inside the string and is unescaped.
pub(crate) fn parse_string_cell(raw: &str) -> Vec<String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let q = bytes[i];
        if q == b'\'' || q == b'"' {
            let start = i + 1;
            let mut j = start;
            let mut escaped = false;
            // Close on a quote that isn't doubled; skip `''` escape pairs.
            while j < bytes.len() {
                if bytes[j] == q {
                    if bytes.get(j + 1) == Some(&q) {
                        j += 2;
                        escaped = true;
                        continue;
                    }
                    break;
                }
                j += 1;
            }
            let content = &raw[start..j.min(bytes.len())];
            // Common case (no `''`): one owned String, no format!/replace churn.
            out.push(if escaped {
                let qc = q as char;
                content.replace(&format!("{qc}{qc}"), &qc.to_string())
            } else {
                content.to_owned()
            });
            i = (j + 1).min(bytes.len());
        } else {
            i += 1;
        }
    }
    out
}

/// If `code` begins (after leading whitespace) with `mpc.<ident> =`, return the
/// field name and the trimmed right-hand side. The identifier must be followed
/// by `=` so `mpc.bus_name` isn't mistaken for `mpc.bus`.
fn parse_assignment_start(code: &str) -> Option<(&str, &str)> {
    let rest = code.trim_start().strip_prefix("mpc.")?;
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    let field = &rest[..end];
    let rhs = rest[end..].trim_start().strip_prefix('=')?.trim_start();
    Some((field, rhs))
}

/// Net `[`+`{` minus `]`+`}` over a comment-stripped code fragment, skipping
/// brackets inside quoted strings (a `'Bus]1'` label must not unbalance).
fn net_bracket_depth(code: &str) -> i32 {
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    for &b in code.as_bytes() {
        match (quote, b) {
            (None, b'\'') => quote = Some(b'\''),
            (None, b'"') => quote = Some(b'"'),
            (Some(q), c) if c == q => quote = None,
            (None, b'[' | b'{') => depth += 1,
            (None, b']' | b'}') => depth -= 1,
            _ => {}
        }
    }
    depth
}

#[cfg(test)]
mod tests {
    use super::*;

    fn located<'a>(src: &'a str, field: &str) -> Option<&'a str> {
        locate_assignments(src)
            .into_iter()
            .find(|assignment| assignment.field == field)
            .map(|assignment| assignment.text)
    }

    #[test]
    fn locate_finds_scalar_and_matrix_fields() {
        let src = "mpc.baseMVA = 100;\n\
                   mpc.bus = [\n\
                   \t1\t3;\n\
                   \t2\t1;\n\
                   ];\n\
                   mpc.branch = [\n\t1\t2\t0.1;\n];\n";
        let fields: Vec<&str> = locate_assignments(src)
            .into_iter()
            .map(|assignment| assignment.field)
            .collect();
        assert_eq!(fields, vec!["baseMVA", "bus", "branch"]);
        assert_eq!(located(src, "baseMVA"), Some("mpc.baseMVA = 100;"));
        let bus = located(src, "bus").unwrap();
        assert!(bus.starts_with("mpc.bus = ["));
        assert!(bus.ends_with("];"));
        assert!(bus.contains("2\t1"));
    }

    #[test]
    fn each_assignment_records_its_byte_offset_in_the_source() {
        let src =
            "% header\nmpc.baseMVA = 100;\r\nmpc.bus = [\n\t1\t3;\n];\nmpc.branch = [1 2 0.1];\n";
        for assignment in locate_assignments(src) {
            let (start, end) = assignment.range();
            assert_eq!(&src[start..end], assignment.text, "{}", assignment.field);
        }
        let branch = locate_assignments(src)
            .into_iter()
            .find(|assignment| assignment.field == "branch")
            .unwrap();
        assert_eq!(branch.start, src.find("mpc.branch").unwrap());
    }

    #[test]
    fn locate_single_line_matrix() {
        let src = "mpc.baseMVA = 100;\nmpc.bus = [1 3; 2 1];\n";
        assert_eq!(located(src, "bus"), Some("mpc.bus = [1 3; 2 1];"));
    }

    #[test]
    fn locate_ignores_bracket_in_comment() {
        // A `]` inside a `%` comment must not close the matrix early.
        let src = "mpc.bus = [\n\t1\t3;  % stray ] here\n\t2\t1;\n];\n";
        let bus = located(src, "bus").unwrap();
        assert!(bus.contains("2\t1"), "matrix closed early: {bus:?}");
        assert!(bus.trim_end().ends_with("];"));
    }

    #[test]
    fn locate_steps_over_cell_array_with_quoted_bracket() {
        // `bus_name` holds a `]` inside a quoted string; the locator must skip the
        // whole `{ … }` and still find the field that follows it.
        let src = "mpc.bus_name = {\n\t'Bus ]1';\n\t'Bus 2';\n};\nmpc.baseMVA = 100;\n";
        let fields: Vec<&str> = locate_assignments(src)
            .into_iter()
            .map(|assignment| assignment.field)
            .collect();
        assert_eq!(fields, vec!["bus_name", "baseMVA"]);
        assert!(located(src, "bus_name").unwrap().contains("Bus 2"));
    }

    #[test]
    fn locate_skips_commented_out_assignment() {
        // A `%`-commented line that looks like an assignment is not located.
        let src = "% mpc.bus = [fake];\nmpc.baseMVA = 100;\n";
        let fields: Vec<&str> = locate_assignments(src)
            .into_iter()
            .map(|assignment| assignment.field)
            .collect();
        assert_eq!(fields, vec!["baseMVA"]);
    }
}
