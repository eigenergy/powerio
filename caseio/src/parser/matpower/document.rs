//! A faithful, re-serializable view of a MATPOWER `.m` file.
//!
//! The parser builds this alongside the typed [`MpcCase`](crate::MpcCase) so a
//! case can be written back out losslessly. Every line of the source becomes a
//! [`DocItem`]: either an `mpc.<field> = …;` assignment captured as its exact
//! source text, or a verbatim line (comments, blanks, the `function` header,
//! stray code). Concatenating the items reproduces the file modulo trailing
//! whitespace — including fields caseio never interprets, in-matrix column
//! header comments, and exact numeric tokens like `7e-05` that an `f64`
//! round-trip would mangle.
//!
//! The model stores assignment *text*, not parsed numbers, so it carries no
//! interpretation of its own; the typed layer parses the values it needs from
//! the same source separately.

use std::ops::Range;

use super::tokens;

/// An ordered, re-serializable view of a MATPOWER case file. Holds one owned
/// copy of the source `text` plus byte ranges into it — no per-line String
/// allocations — so a large case round-trips with minimal copying.
#[derive(Debug, Clone)]
pub struct MatpowerDocument {
    text: String,
    items: Vec<Item>,
}

#[derive(Debug, Clone)]
enum Item {
    /// A line round-tripped verbatim but not interpreted.
    Verbatim(Range<usize>),
    /// `mpc.<field> = <rhs>;`: the field-name range and the whole (possibly
    /// multi-line) assignment range, both into `text`.
    Assignment { field: Range<usize>, full: Range<usize> },
}

impl Item {
    fn range(&self) -> Range<usize> {
        match self {
            Item::Verbatim(r) | Item::Assignment { full: r, .. } => r.clone(),
        }
    }
}

impl MatpowerDocument {
    /// The raw source text of the first `mpc.<field>` assignment, if present.
    #[must_use]
    pub fn assignment(&self, field: &str) -> Option<&str> {
        self.items.iter().find_map(|it| match it {
            Item::Assignment { field: fr, full } if &self.text[fr.clone()] == field => {
                Some(&self.text[full.clone()])
            }
            _ => None,
        })
    }

    /// Field names of every assignment, in source order (duplicates kept).
    #[must_use]
    pub fn fields(&self) -> Vec<&str> {
        self.items
            .iter()
            .filter_map(|it| match it {
                Item::Assignment { field, .. } => Some(&self.text[field.clone()]),
                Item::Verbatim(_) => None,
            })
            .collect()
    }
}

impl std::fmt::Display for MatpowerDocument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for item in &self.items {
            writeln!(f, "{}", &self.text[item.range()])?;
        }
        Ok(())
    }
}

/// Build the document from raw `.m` source. Infallible: a malformed file still
/// yields a faithful document; the typed parser reports the actual errors.
#[must_use]
pub fn build_document(content: &str) -> MatpowerDocument {
    let base = content.as_ptr() as usize;
    // Logical lines as (range, slice) with terminators trimmed off the range,
    // so `Display` reproduces the `\n`-joined, single-trailing-`\n` output the
    // round-trip tests pin (matching `str::lines` semantics).
    let lines: Vec<(Range<usize>, &str)> = {
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
    };

    let mut items = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        let (line_range, line) = (lines[i].0.clone(), lines[i].1);
        let (code, _comment) = tokens::comment_split(line);
        if let Some((field, rhs)) = parse_assignment_start(code) {
            // The field name borrows from `content`; its offset into `text`
            // (== content byte-for-byte) is pointer arithmetic, not a re-search.
            let field_off = field.as_ptr() as usize - base;
            let mut full = line_range;
            // A `[ … ]` / `{ … }` RHS can span lines; extend until balanced.
            if rhs.starts_with('[') || rhs.starts_with('{') {
                let mut depth = net_bracket_depth(code);
                while depth > 0 && i + 1 < lines.len() {
                    i += 1;
                    full.end = lines[i].0.end;
                    depth += net_bracket_depth(tokens::comment_split(lines[i].1).0);
                }
            }
            items.push(Item::Assignment {
                field: field_off..field_off + field.len(),
                full,
            });
        } else {
            items.push(Item::Verbatim(line_range));
        }
        i += 1;
    }

    MatpowerDocument {
        text: content.to_owned(),
        items,
    }
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
