//! Extract `mpc.<field> = ...;` values from a MATPOWER case.
//!
//! Scalars are found by a linear `mpc.<field> =` scan. Matrices are parsed by
//! [`for_each_matrix_row`], which reads a single assignment's text, strips
//! comments inline per line, splits rows on `;`, and yields each row into a
//! reused buffer — no whole-section copy and no `Vec<Vec<f64>>` intermediate.

use crate::{Error, Result};

/// The first `mpc.<field> = <scalar>` RHS (a matrix RHS is skipped), trimmed.
/// The identifier must match exactly so a search for `gen` skips `gencost`.
fn find_scalar_rhs<'a>(source: &'a str, field: &str) -> Option<&'a str> {
    let mut from = 0;
    while let Some(rel) = source[from..].find("mpc.") {
        let after_dot = from + rel + "mpc.".len();
        from = after_dot; // advance past this `mpc.` so we don't re-find it
        let Some(tail) = source[after_dot..].strip_prefix(field) else {
            continue;
        };
        if tail
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            continue;
        }
        let Some(rhs) = tail.trim_start().strip_prefix('=') else {
            continue; // `mpc.<field>` not used as an assignment target here
        };
        let rhs = rhs.trim_start();
        if rhs.starts_with('[') {
            continue; // a matrix RHS, not a scalar
        }
        return Some(rhs);
    }
    None
}

/// Find `mpc.<field> = <number>;` and return the parsed value, if present.
pub(crate) fn find_scalar(source: &str, field: &str) -> Result<Option<f64>> {
    let Some(rhs) = find_scalar_rhs(source, field) else {
        return Ok(None);
    };
    let Some(end) = rhs.find(';') else {
        return Ok(None);
    };
    // Only the tail before `;` can have trailing space; then strip surrounding
    // quotes (e.g. `mpc.version = '2';`).
    let value = rhs[..end]
        .trim_end()
        .trim_matches(|c| c == '\'' || c == '"');
    value.parse::<f64>().map(Some).map_err(|_| Error::BadFloat {
        field: leak_field(field),
        row: 0,
        value: value.to_string(),
    })
}

/// Parse the scalar from a single `mpc.<field> = <number>;` assignment's text.
pub(crate) fn scalar_from_assignment(raw: &str, field: &str) -> Result<Option<f64>> {
    let stripped = super::tokens::strip_comments(raw);
    find_scalar(&stripped, field)
}

/// Stream the rows of an `mpc.<field> = [ … ];` assignment. `assignment` is the
/// raw (comment-bearing, possibly multi-line) source of one assignment from the
/// document. Comments are stripped per line, rows split on `;`, tokens parsed
/// into a reused buffer, and `f` is invoked per non-empty row — so the caller
/// builds its typed `Vec` directly, with no `Vec<Vec<f64>>` and no whole-section
/// comment-strip copy. `f` receives the same 0-based non-empty-row index the
/// old `Vec<Vec<f64>>` path passed to `from_row`.
pub(crate) fn for_each_matrix_row<F>(assignment: &str, field: &str, mut f: F) -> Result<()>
where
    F: FnMut(&[f64], usize) -> Result<()>,
{
    let mut buf: Vec<f64> = Vec::with_capacity(24);
    let mut row = 0usize;
    let mut inside = false; // have we passed the opening `[`?
    let mut done = false; // have we hit the closing `]`?
    for line in assignment.lines() {
        if done {
            break;
        }
        let mut code = super::tokens::comment_split(line).0;
        if !inside {
            let Some(open) = code.find('[') else {
                continue;
            };
            code = &code[open + 1..];
            inside = true;
        }
        // Numeric matrix bodies have no nested `[`, so the first `]` closes it.
        if let Some(close) = code.find(']') {
            code = &code[..close];
            done = true;
        }
        // One byte-level pass over the line's code: `;` ends a row, ASCII
        // whitespace separates tokens, and a trailing comma is stripped (MATPOWER
        // rows are space/semicolon-delimited). This replaces split(';') +
        // split_ascii_whitespace — the generic Unicode searcher was the dominant
        // tokenizing cost — and feeds raw bytes straight to the float parser.
        //
        // MATLAB also ends a matrix row at the line break itself unless the
        // line ends with a `...` continuation; PowerWorld's `.m` exports write
        // no semicolons at all, so the end-of-line flush below is what splits
        // their rows.
        let mut continuation = false;
        let trimmed = code.trim_end();
        if let Some(stripped) = trimmed.strip_suffix("...") {
            code = stripped;
            continuation = true;
        }
        let bytes = code.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            if b == b';' {
                if !buf.is_empty() {
                    f(&buf, row)?;
                    row += 1;
                    buf.clear();
                }
                i += 1;
                continue;
            }
            if b.is_ascii_whitespace() {
                i += 1;
                continue;
            }
            let start = i;
            while i < bytes.len() && bytes[i] != b';' && !bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            let mut tok = &bytes[start..i];
            while tok.last() == Some(&b',') {
                tok = &tok[..tok.len() - 1];
            }
            if tok.is_empty() {
                continue;
            }
            buf.push(parse_float(tok).ok_or_else(|| Error::BadFloat {
                field: leak_field(field),
                row,
                value: String::from_utf8_lossy(tok).into_owned(),
            })?);
        }
        // End of line ends the row, unless continued with `...`.
        if !continuation && !buf.is_empty() {
            f(&buf, row)?;
            row += 1;
            buf.clear();
        }
    }
    // Entered the matrix (`[`) but never saw the closing `]`: the assignment is
    // truncated. The old `find_matrix` rejected this; keep that behavior rather
    // than silently accepting a partial matrix. A closed matrix sets `done`, so
    // a legitimate last row with no trailing `;` (handled by the flush below)
    // does not trip this.
    if inside && !done {
        return Err(Error::UnbalancedBrackets(leak_field(field)));
    }
    // A final row not terminated by `;` (e.g. the last row before `];`).
    if !buf.is_empty() {
        f(&buf, row)?;
    }
    Ok(())
}

fn parse_float(tok: &[u8]) -> Option<f64> {
    match tok {
        b"Inf" | b"inf" | b"+Inf" | b"+inf" => Some(f64::INFINITY),
        b"-Inf" | b"-inf" => Some(f64::NEG_INFINITY),
        b"NaN" | b"nan" => Some(f64::NAN),
        // Float tokens dominate large case parse time. `lexical_core` takes raw
        // bytes, so the tokenizer passes a slice with no &str round trip.
        _ => lexical_core::parse::<f64>(tok).ok(),
    }
}

/// `Error::BadFloat`/`MissingField` want `&'static str`. We accept user input
/// field names but only ever pass through known names here, so the fallback is
/// bounded to the small set MATPOWER itself defines.
fn leak_field(field: &str) -> &'static str {
    match field {
        "baseMVA" => "baseMVA",
        "bus" => "bus",
        "branch" => "branch",
        "dcline" => "dcline",
        "gen" => "gen",
        "gencost" => "gencost",
        "storage" => "storage",
        "version" => "version",
        _ => "(unknown)",
    }
}
