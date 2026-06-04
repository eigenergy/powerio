//! Extract `mpc.<field> = ...;` values (scalar, string, matrix) from a
//! comment stripped source. Parses only the small MATLAB subset MATPOWER
//! case files use.
//!
//! A single linear scan per field locates `mpc.<field> =` and reads the RHS
//! directly. No regex: the field finders are the parser hot path, and a hand
//! scan over `&str` is several times faster than running a regex engine.
//!
//! Matrix values come back row by row as `Vec<Vec<f64>>` so the domain layer
//! can map columns to `Bus` / `Branch` without further parsing.

use crate::{Error, Result};

/// Which kind of right-hand side a finder wants. A scalar finder skips an
/// assignment whose RHS opens with `[` (it's a matrix), and vice versa, so the
/// two finders pick the first occurrence of the kind they care about.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Rhs {
    Scalar,
    Matrix,
}

/// Right-hand side of the first `mpc.<field> = …` assignment of the wanted
/// kind. The returned slice starts at the first non-space character after `=`.
fn find_rhs<'a>(source: &'a str, field: &str, want: Rhs) -> Option<&'a str> {
    let mut from = 0;
    while let Some(rel) = source[from..].find("mpc.") {
        let after_dot = from + rel + "mpc.".len();
        from = after_dot; // always advance past this `mpc.` to avoid re-finding it

        // The identifier must be exactly `field`, not a longer name it prefixes,
        // so a search for `gen` skips `gencost`.
        let Some(tail) = source[after_dot..].strip_prefix(field) else {
            continue;
        };
        if tail.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_') {
            continue;
        }
        let Some(rhs) = tail.trim_start().strip_prefix('=') else {
            continue; // `mpc.<field>` not used as an assignment target here
        };
        let rhs = rhs.trim_start();
        let is_matrix = rhs.starts_with('[');
        let wanted = match want {
            Rhs::Matrix => is_matrix,
            Rhs::Scalar => !is_matrix,
        };
        if wanted {
            return Some(rhs);
        }
    }
    None
}

/// Find `mpc.<field> = <number>;` and return the parsed value, if present.
pub(crate) fn find_scalar(source: &str, field: &str) -> Result<Option<f64>> {
    let Some(rhs) = find_rhs(source, field, Rhs::Scalar) else {
        return Ok(None);
    };
    // Value is everything up to the terminating `;`.
    let Some(end) = rhs.find(';') else {
        return Ok(None);
    };
    // `find_rhs` already trimmed the front; only the tail before `;` can have
    // trailing space. Then strip surrounding quotes (e.g. mpc.version = '2';).
    let value = rhs[..end].trim_end().trim_matches(|c| c == '\'' || c == '"');
    value.parse::<f64>().map(Some).map_err(|_| Error::BadFloat {
        field: leak_field(field),
        row: 0,
        value: value.to_string(),
    })
}

/// Find `mpc.<field> = [ ... ];` and return parsed rows, if present.
///
/// Each row is a `Vec<f64>`; rows are separated by `;` inside the brackets.
/// Whitespace and newlines within a row are ignored.
pub(crate) fn find_matrix(source: &str, field: &str) -> Result<Option<Vec<Vec<f64>>>> {
    let Some(rhs) = find_rhs(source, field, Rhs::Matrix) else {
        return Ok(None);
    };

    // `rhs` begins with the opening `[`. Walk forward tracking bracket balance
    // (matrices can nest, e.g. a string column) until the matching close.
    let bytes = rhs.as_bytes();
    let mut depth = 0i32;
    let mut close = None;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(close) = close else {
        return Err(Error::UnbalancedBrackets(leak_field(field)));
    };

    let body = &rhs[1..close];
    Ok(Some(parse_matrix_body(body, field)?))
}

fn parse_matrix_body(body: &str, field: &str) -> Result<Vec<Vec<f64>>> {
    let mut rows = Vec::new();
    for (row_idx, raw) in body.split(';').enumerate() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut vals = Vec::with_capacity(16);
        for tok in trimmed.split_ascii_whitespace() {
            let t = tok.trim_end_matches(',');
            if t.is_empty() {
                continue;
            }
            let v = parse_float(t).ok_or_else(|| Error::BadFloat {
                field: leak_field(field),
                row: row_idx,
                value: t.to_string(),
            })?;
            vals.push(v);
        }
        if !vals.is_empty() {
            rows.push(vals);
        }
    }
    Ok(rows)
}

fn parse_float(tok: &str) -> Option<f64> {
    match tok {
        "Inf" | "inf" | "+Inf" | "+inf" => Some(f64::INFINITY),
        "-Inf" | "-inf" => Some(f64::NEG_INFINITY),
        "NaN" | "nan" => Some(f64::NAN),
        _ => tok.parse::<f64>().ok(),
    }
}

/// `Error::BadFloat`/`MissingField` want `&'static str`. We accept user
/// input field names but only ever pass through known names here, so the
/// fallback is bounded to the small set MATPOWER itself defines.
fn leak_field(field: &str) -> &'static str {
    match field {
        "baseMVA" => "baseMVA",
        "bus" => "bus",
        "branch" => "branch",
        "gen" => "gen",
        "gencost" => "gencost",
        "storage" => "storage",
        "version" => "version",
        _ => "(unknown)",
    }
}
