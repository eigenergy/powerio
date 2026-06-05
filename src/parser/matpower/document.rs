//! A faithful, re-serializable view of a MATPOWER `.m` file.
//!
//! The parser builds this alongside the typed [`MpcCase`](crate::MpcCase) so a
//! case can be written back out losslessly. Every line of the source becomes a
//! [`DocItem`]: either an `mpc.<field> = …;` assignment captured as its exact
//! source text, or a verbatim line (comments, blanks, the `function` header,
//! stray code). Concatenating the items reproduces the file modulo trailing
//! whitespace — including fields netmat never interprets, in-matrix column
//! header comments, and exact numeric tokens like `7e-05` that an `f64`
//! round-trip would mangle.
//!
//! The model stores assignment *text*, not parsed numbers, so it carries no
//! interpretation of its own; the typed layer parses the values it needs from
//! the same source separately.

use super::tokens;

/// An ordered, re-serializable view of a MATPOWER case file.
#[derive(Debug, Clone)]
pub struct MatpowerDocument {
    items: Vec<DocItem>,
}

#[derive(Debug, Clone)]
enum DocItem {
    /// A line we round-trip verbatim but don't interpret.
    Verbatim(String),
    /// `mpc.<field> = <rhs>;`, captured as its exact (possibly multi-line)
    /// source text. `field` lets a future editor locate and rewrite a section
    /// without disturbing the rest of the document.
    Assignment { field: String, raw: String },
}

impl DocItem {
    fn raw(&self) -> &str {
        match self {
            DocItem::Verbatim(s) => s,
            DocItem::Assignment { raw, .. } => raw,
        }
    }
}

impl MatpowerDocument {
    /// The raw source text of the first `mpc.<field>` assignment, if present.
    /// Used by the typed layer for fields it reads only for round-trip extras
    /// (e.g. `bus_name`).
    #[must_use]
    pub fn assignment(&self, field: &str) -> Option<&str> {
        self.items.iter().find_map(|it| match it {
            DocItem::Assignment { field: f, raw } if f == field => Some(raw.as_str()),
            _ => None,
        })
    }

    /// Field names of every assignment, in source order (duplicates kept).
    #[must_use]
    pub fn fields(&self) -> Vec<&str> {
        self.items
            .iter()
            .filter_map(|it| match it {
                DocItem::Assignment { field, .. } => Some(field.as_str()),
                DocItem::Verbatim(_) => None,
            })
            .collect()
    }
}

impl std::fmt::Display for MatpowerDocument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for item in &self.items {
            writeln!(f, "{}", item.raw())?;
        }
        Ok(())
    }
}

/// Build the document from raw `.m` source. Infallible: a malformed file still
/// yields a faithful (if not semantically valid) document; the typed parser
/// reports the actual errors.
#[must_use]
pub fn build_document(source: &str) -> MatpowerDocument {
    let lines: Vec<&str> = source.lines().collect();
    let mut items = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let (code, _comment) = tokens::comment_split(line);
        if let Some((field, rhs)) = parse_assignment_start(code) {
            let mut raw = String::from(line);
            // A `[ … ]` / `{ … }` right-hand side can span many lines; pull
            // them in until the brackets balance.
            if rhs.starts_with('[') || rhs.starts_with('{') {
                let mut depth = net_bracket_depth(code);
                while depth > 0 && i + 1 < lines.len() {
                    i += 1;
                    let next = lines[i];
                    raw.push('\n');
                    raw.push_str(next);
                    depth += net_bracket_depth(tokens::comment_split(next).0);
                }
            }
            items.push(DocItem::Assignment {
                field: field.to_string(),
                raw,
            });
        } else {
            items.push(DocItem::Verbatim(line.to_string()));
        }
        i += 1;
    }
    MatpowerDocument { items }
}

/// Extract the quoted strings from a `{ '...'; '...' }` cell array assignment,
/// in order. Used for `mpc.bus_name` / `gentype` / `genfuel`. Tolerant: it
/// scans the raw assignment text for `'…'` (or `"…"`) runs, so the field name
/// and the braces/semicolons are simply skipped.
pub(crate) fn parse_string_cell(raw: &str) -> Vec<String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\'' || b == b'"' {
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j] != b {
                j += 1;
            }
            out.push(raw[start..j].to_string());
            i = j + 1;
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

    #[test]
    fn roundtrips_a_small_case() {
        let src = "function mpc = toy\n\
                   mpc.version = '2';\n\
                   mpc.baseMVA = 100;\n\
                   mpc.bus = [\n\
                   %\tid\ttype\n\
                   \t1\t3;\n\
                   \t2\t1;\n\
                   ];\n";
        let doc = build_document(src);
        assert_eq!(doc.to_string(), src);
        assert_eq!(doc.fields(), vec!["version", "baseMVA", "bus"]);
        assert!(doc.assignment("bus").unwrap().contains("type"));
    }

    #[test]
    fn idempotent_render() {
        let src = "function mpc = toy\nmpc.baseMVA = 100;\nmpc.bus = [\n\t1\t3;\n];\n";
        let once = build_document(src).to_string();
        let twice = build_document(&once).to_string();
        assert_eq!(once, twice);
    }

    #[test]
    fn preserves_scientific_notation_tokens() {
        let src = "mpc.branch = [\n\t1\t2\t7e-05\t6e-05;\n];\n";
        assert!(build_document(src).to_string().contains("7e-05"));
    }

    #[test]
    fn bracket_inside_quotes_does_not_unbalance() {
        let src = "mpc.bus_name = {\n\t'Bus ]1';\n};\nmpc.baseMVA = 100;\n";
        let doc = build_document(src);
        // The cell array is one assignment; baseMVA is a separate one after it.
        assert_eq!(doc.fields(), vec!["bus_name", "baseMVA"]);
    }

    #[test]
    fn comment_line_stays_verbatim() {
        let src = "% mpc.bus = [fake];\nmpc.baseMVA = 100;\n";
        let doc = build_document(src);
        assert_eq!(doc.fields(), vec!["baseMVA"]); // the comment is not an assignment
        assert_eq!(doc.to_string(), src);
    }
}
