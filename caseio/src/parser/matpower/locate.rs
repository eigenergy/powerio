//! Locate `mpc.<field> = …;` assignments in MATPOWER `.m` source.
//!
//! The parser borrows each assignment's raw text straight from the source and
//! hands it to the typed row/scalar/cell parsers. Lossless round-trip needs no
//! structured model here: [`Network`](crate::Network) keeps the original source
//! text and the writer echoes it, so this module only has to find where each
//! field's text begins and ends.

use std::ops::Range;

use super::tokens;

/// Logical lines of `content` as `(range, slice)` with the line terminator
/// trimmed off the range — `str::lines` semantics, but keeping each line's byte
/// offsets into `content`.
fn logical_lines(content: &str) -> Vec<(Range<usize>, &str)> {
    let mut out = Vec::new();
    let mut off = 0usize;
    for piece in content.split_inclusive('\n') {
        let start = off;
        off += piece.len();
        let trimmed = piece
            .strip_suffix('\n')
            .map_or(piece, |s| s.strip_suffix('\r').unwrap_or(s));
        out.push((start..start + trimmed.len(), trimmed));
    }
    out
}

/// Locate each `mpc.<field> = <rhs>;` assignment's text, borrowing `(field, full)`
/// slices from `content` in source order. For a numeric `[ … ]` matrix it scans
/// for the closing `]` directly — numeric bodies never nest brackets — and uses
/// the quote-aware depth FSM only for `{ … }` cell arrays (whose strings may hold
/// `]`/`}`). Infallible: an unclosed block runs to EOF and
/// [`super::matlab::for_each_matrix_row`] reports the truncation.
pub(crate) fn locate_assignments(content: &str) -> Vec<(&str, &str)> {
    let lines = logical_lines(content);
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let (code, _comment) = tokens::comment_split(lines[i].1);
        if let Some((field, rhs)) = parse_assignment_start(code) {
            let start = lines[i].0.start;
            let mut end = lines[i].0.end;
            if rhs.starts_with('[') {
                // Numeric matrix: the first un-commented `]` closes it. Reuse the
                // opening line's `code` (it holds the `]` for a single-line matrix).
                if !code.contains(']') {
                    while i + 1 < lines.len() {
                        i += 1;
                        end = lines[i].0.end;
                        if tokens::comment_split(lines[i].1).0.contains(']') {
                            break;
                        }
                    }
                }
            } else if rhs.starts_with('{') {
                let mut depth = net_bracket_depth(code);
                while depth > 0 && i + 1 < lines.len() {
                    i += 1;
                    end = lines[i].0.end;
                    depth += net_bracket_depth(tokens::comment_split(lines[i].1).0);
                }
            }
            out.push((field, &content[start..end]));
        }
        i += 1;
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
            .find(|(f, _)| *f == field)
            .map(|(_, full)| full)
    }

    #[test]
    fn locate_finds_scalar_and_matrix_fields() {
        let src = "mpc.baseMVA = 100;\n\
                   mpc.bus = [\n\
                   \t1\t3;\n\
                   \t2\t1;\n\
                   ];\n\
                   mpc.branch = [\n\t1\t2\t0.1;\n];\n";
        let fields: Vec<&str> = locate_assignments(src).into_iter().map(|(f, _)| f).collect();
        assert_eq!(fields, vec!["baseMVA", "bus", "branch"]);
        assert_eq!(located(src, "baseMVA"), Some("mpc.baseMVA = 100;"));
        let bus = located(src, "bus").unwrap();
        assert!(bus.starts_with("mpc.bus = ["));
        assert!(bus.ends_with("];"));
        assert!(bus.contains("2\t1"));
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
        let fields: Vec<&str> = locate_assignments(src).into_iter().map(|(f, _)| f).collect();
        assert_eq!(fields, vec!["bus_name", "baseMVA"]);
        assert!(located(src, "bus_name").unwrap().contains("Bus 2"));
    }

    #[test]
    fn locate_skips_commented_out_assignment() {
        // A `%`-commented line that looks like an assignment is not located.
        let src = "% mpc.bus = [fake];\nmpc.baseMVA = 100;\n";
        let fields: Vec<&str> = locate_assignments(src).into_iter().map(|(f, _)| f).collect();
        assert_eq!(fields, vec!["baseMVA"]);
    }
}
