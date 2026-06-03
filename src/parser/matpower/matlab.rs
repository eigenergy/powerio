//! Extract `mpc.<field> = ...;` values (scalar, string, matrix) from a
//! comment stripped source. Parses only the small MATLAB subset MATPOWER
//! case files use.
//!
//! Matrix values come back row by row as `Vec<Vec<f64>>` so the domain
//! layer can map columns to `Bus` / `Branch` without further parsing.

use std::sync::OnceLock;

use regex::Regex;

use crate::{Error, Result};

/// Find `mpc.<field> = <number>;` and return the parsed value, if present.
pub(crate) fn find_scalar(source: &str, field: &str) -> Result<Option<f64>> {
    let re = scalar_regex();
    for cap in re.captures_iter(source) {
        if &cap[1] == field {
            let raw = cap[2].trim();
            // Strip surrounding quotes (e.g. mpc.version = '2';)
            let value = raw.trim_matches(|c| c == '\'' || c == '"');
            return value
                .parse::<f64>()
                .map(Some)
                .map_err(|_| Error::BadFloat {
                    field: leak_field(field),
                    row: 0,
                    value: value.to_string(),
                });
        }
    }
    Ok(None)
}

/// Find `mpc.<field> = [ ... ];` and return parsed rows, if present.
///
/// Each row is a `Vec<f64>`; rows are separated by `;` inside the brackets.
/// Whitespace and newlines within a row are ignored.
pub(crate) fn find_matrix(source: &str, field: &str) -> Result<Option<Vec<Vec<f64>>>> {
    let re = matrix_open_regex();
    let mut iter = re.captures_iter(source);

    let cap = match iter.find(|c| &c[1] == field) {
        Some(c) => c,
        None => return Ok(None),
    };

    let m = cap.get(0).expect("regex captures group 0");
    let after_open = m.end();
    // Walk forward tracking `[` / `]` balance. We've already consumed the
    // opening `[`, so depth starts at 1.
    let bytes = source.as_bytes();
    let mut depth = 1i32;
    let mut end = after_open;
    while end < bytes.len() && depth > 0 {
        match bytes[end] {
            b'[' => depth += 1,
            b']' => depth -= 1,
            _ => {}
        }
        if depth == 0 {
            break;
        }
        end += 1;
    }
    if depth != 0 {
        return Err(Error::UnbalancedBrackets(leak_field(field)));
    }

    let body = &source[after_open..end];
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

fn scalar_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // mpc.<field>  =  <value>  ;
        // Value is anything up to the terminating ';'. We intentionally do
        // not match `[...]` — matrices are handled separately.
        Regex::new(r"mpc\.(\w+)\s*=\s*([^\[;]*?)\s*;").expect("scalar regex compiles")
    })
}

fn matrix_open_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"mpc\.(\w+)\s*=\s*\[").expect("matrix-open regex compiles")
    })
}

/// `Error::BadFloat`/`MissingField` want `&'static str`. We accept user
/// input field names but only ever pass through known names here, so leak
/// is bounded to the small set MATPOWER itself defines.
fn leak_field(field: &str) -> &'static str {
    match field {
        "baseMVA" => "baseMVA",
        "bus" => "bus",
        "branch" => "branch",
        "gen" => "gen",
        "gencost" => "gencost",
        "version" => "version",
        _ => "(unknown)",
    }
}
