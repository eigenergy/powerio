//! The generic auxiliary file grammar: parse any `.aux` into [`AuxFile`] and
//! serialize it back.
//!
//! This layer knows the file format and nothing about power systems. The
//! grammar follows the official guide ("Auxiliary File Format for Simulator
//! 24", PowerWorld Corporation): a file is a sequence of `DATA` and `SCRIPT`
//! sections; both the legacy header (`DATA Name(Object, [fields], CSV, NO)`)
//! and the concise header (`Object Name(fields)`) are read; field lists and
//! value rows may span lines; `//` starts a comment anywhere outside quotes;
//! `<SUBDATA Type> ... </SUBDATA>` blocks attach to the value row above them
//! and their interior lines are kept verbatim.
//!
//! [`write_aux`] emits a canonical form: legacy headers, space delimited
//! values, one row per line. Canonical output is idempotent (parsing it and
//! writing again reproduces it byte for byte) but does not preserve the
//! source's whitespace or comments; the byte exact same format round trip
//! comes from the retained source (see [`crate::write_as`]).

use std::fmt::Write as _;

use crate::{Error, Result};

const FMT: &str = "PowerWorld .aux";

/// A parsed auxiliary file: the ordered `DATA` and `SCRIPT` sections.
#[derive(Debug, Clone, PartialEq)]
pub struct AuxFile {
    pub sections: Vec<AuxSection>,
}

impl AuxFile {
    /// The `DATA` sections, in file order.
    pub fn data(&self) -> impl Iterator<Item = &AuxObject> {
        self.sections.iter().filter_map(|s| match s {
            AuxSection::Data(d) => Some(d),
            AuxSection::Script(_) => None,
        })
    }

    /// The `DATA` sections for one object type (a type may appear more than
    /// once with different field lists; ACTIVSg exports carry two `Branch`
    /// blocks, lines and transformers).
    pub fn data_of<'a>(&'a self, object_type: &'a str) -> impl Iterator<Item = &'a AuxObject> {
        self.data()
            .filter(move |d| d.object_type.eq_ignore_ascii_case(object_type))
    }
}

/// One section of an auxiliary file.
#[derive(Debug, Clone, PartialEq)]
pub enum AuxSection {
    Data(AuxObject),
    Script(AuxScript),
}

/// A `SCRIPT` section, retained verbatim: powerio executes nothing.
#[derive(Debug, Clone, PartialEq)]
pub struct AuxScript {
    pub name: Option<String>,
    /// Body lines between the braces, byte for byte.
    pub lines: Vec<String>,
}

/// One `DATA` section: an object type, its declared field list, and the rows.
#[derive(Debug, Clone, PartialEq)]
pub struct AuxObject {
    pub object_type: String,
    /// Optional section name (callable from `LoadData` scripts).
    pub data_name: Option<String>,
    /// Declared fields, in order, location suffixes preserved (`BusNum:1`).
    pub fields: Vec<String>,
    /// `CREATE_IF_NOT_FOUND` argument when the header carried one
    /// (`YES`/`NO`/`PROMPT`).
    pub create_if_not_found: Option<String>,
    pub rows: Vec<AuxRow>,
}

impl AuxObject {
    /// Position of `field` in the declared field list (case insensitive).
    #[must_use]
    pub fn field_index(&self, field: &str) -> Option<usize> {
        self.fields
            .iter()
            .position(|f| f.eq_ignore_ascii_case(field))
    }
}

/// One value row of a `DATA` section, with any `SUBDATA` blocks that follow it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AuxRow {
    /// One value per declared field, quotes removed.
    pub values: Vec<String>,
    pub subdata: Vec<AuxSubData>,
}

/// A `<SUBDATA Type> ... </SUBDATA>` block. The interior format is fixed per
/// subobject type (some are free text, some are per line records), so the
/// lines are kept verbatim.
#[derive(Debug, Clone, PartialEq)]
pub struct AuxSubData {
    pub name: String,
    pub lines: Vec<String>,
}

// ---- Parser -----------------------------------------------------------------

/// Parse auxiliary file `text` into an [`AuxFile`].
///
/// # Errors
/// [`Error::FormatRead`] with the line number on malformed input: an
/// unterminated section, a row with more values than declared fields, a row cut
/// short at the closing brace, `SUBDATA` with no owning row, or an unknown
/// file type specifier.
pub fn parse_aux(text: &str) -> Result<AuxFile> {
    Parser {
        lines: text.lines().collect(),
        pos: 0,
    }
    .parse()
}

struct Parser<'a> {
    lines: Vec<&'a str>,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn parse(mut self) -> Result<AuxFile> {
        let mut sections = Vec::new();
        while let Some(line) = self.peek_content() {
            if first_word_is(line, "SCRIPT") {
                sections.push(AuxSection::Script(self.script()?));
            } else {
                sections.push(AuxSection::Data(self.data()?));
            }
        }
        Ok(AuxFile { sections })
    }

    /// The next line with content after comment stripping, without consuming
    /// it. Skips blank and comment lines.
    fn peek_content(&mut self) -> Option<&'a str> {
        while self.pos < self.lines.len() {
            let stripped = strip_comment(self.lines[self.pos]).trim();
            if !stripped.is_empty() {
                return Some(stripped);
            }
            self.pos += 1;
        }
        None
    }

    fn err(&self, message: impl Into<String>) -> Error {
        Error::FormatRead {
            format: FMT,
            message: format!(
                "line {}: {}",
                self.pos.min(self.lines.len()),
                message.into()
            ),
        }
    }

    /// Consume a `SCRIPT Name { ... }` section, body verbatim.
    fn script(&mut self) -> Result<AuxScript> {
        let header = strip_comment(self.lines[self.pos]).trim().to_string();
        self.pos += 1;
        let mut rest = header["SCRIPT".len()..].trim();
        let brace_in_header = rest.ends_with('{');
        if brace_in_header {
            rest = rest[..rest.len() - 1].trim();
        }
        let name = (!rest.is_empty()).then(|| rest.to_string());
        if !brace_in_header {
            loop {
                let Some(line) = self.next_line() else {
                    return Err(self.err("SCRIPT section with no `{`"));
                };
                let t = strip_comment(line).trim();
                if t == "{" {
                    break;
                }
                if !t.is_empty() {
                    return Err(self.err("expected `{` after SCRIPT header"));
                }
            }
        }
        let mut lines = Vec::new();
        loop {
            let Some(line) = self.next_line() else {
                return Err(self.err("unterminated SCRIPT section"));
            };
            if line.trim() == "}" {
                return Ok(AuxScript { name, lines });
            }
            lines.push(line.to_string());
        }
    }

    fn next_line(&mut self) -> Option<&'a str> {
        let line = self.lines.get(self.pos).copied();
        if line.is_some() {
            self.pos += 1;
        }
        line
    }

    /// Consume a `DATA` section, legacy or concise header.
    fn data(&mut self) -> Result<AuxObject> {
        let header = self.header_text()?;
        let close = header
            .rfind(')')
            .ok_or_else(|| self.err("header has no `)`"))?;
        let brace_in_header = match header[close + 1..].trim() {
            "" => false,
            "{" => true,
            other => {
                return Err(self.err(format!("unexpected text after section header: {other:?}")));
            }
        };
        let (object_type, data_name, fields, csv, create_if_not_found) =
            self.split_header(&header[..=close])?;
        if !brace_in_header {
            self.expect_open_brace()?;
        }
        let rows = self.body(&fields, csv)?;
        Ok(AuxObject {
            object_type,
            data_name,
            fields,
            create_if_not_found,
            rows,
        })
    }

    /// Accumulate header lines (comments stripped) until the parentheses
    /// balance.
    fn header_text(&mut self) -> Result<String> {
        let start = self.pos;
        let mut text = String::new();
        let mut depth = 0i32;
        let mut opened = false;
        while let Some(line) = self.next_line() {
            let stripped = strip_comment(line).trim();
            if !text.is_empty() && !stripped.is_empty() {
                text.push(' ');
            }
            text.push_str(stripped);
            let mut in_quote = false;
            for c in stripped.chars() {
                match c {
                    '"' => in_quote = !in_quote,
                    '(' if !in_quote => {
                        depth += 1;
                        opened = true;
                    }
                    ')' if !in_quote => depth -= 1,
                    _ => {}
                }
            }
            if opened && depth == 0 {
                return Ok(text);
            }
            if self.pos - start > 200 {
                break;
            }
        }
        Err(self.err("unterminated section header (unbalanced parentheses)"))
    }

    /// Split a balanced header into its parts. Legacy form:
    /// `DATA Name(Object, [fields], specifier, create)`. Concise form:
    /// `Object Name(fields)`.
    #[allow(clippy::type_complexity)]
    fn split_header(
        &self,
        header: &str,
    ) -> Result<(String, Option<String>, Vec<String>, bool, Option<String>)> {
        let open = header
            .find('(')
            .ok_or_else(|| self.err("header has no `(`"))?;
        let close = header
            .rfind(')')
            .ok_or_else(|| self.err("header has no `)`"))?;
        if close <= open {
            return Err(self.err("header `)` precedes `(`"));
        }
        let before = header[..open].trim();
        let inner = &header[open + 1..close];
        let legacy = first_word_is(before, "DATA");

        if legacy {
            let data_name = before["DATA".len()..].trim();
            let data_name = (!data_name.is_empty()).then(|| data_name.to_string());
            // Object type, then `[fields]`, then optional specifier and
            // create_if_not_found.
            let bracket_open = inner
                .find('[')
                .ok_or_else(|| self.err("legacy DATA header has no `[fields]` list"))?;
            let bracket_close = inner
                .rfind(']')
                .ok_or_else(|| self.err("legacy DATA header has no closing `]`"))?;
            let object_type = inner[..bracket_open].trim().trim_end_matches(',').trim();
            if object_type.is_empty() {
                return Err(self.err("legacy DATA header has no object type"));
            }
            let fields = split_fields(&inner[bracket_open + 1..bracket_close]);
            if fields.is_empty() {
                return Err(self.err("empty field list"));
            }
            let mut csv = false;
            let mut create = None;
            for arg in inner[bracket_close + 1..].split(',') {
                let arg = arg.trim();
                if arg.is_empty() {
                    continue;
                }
                match arg.to_ascii_uppercase().as_str() {
                    "AUXCSV" | "CSV" | "CSVAUX" => csv = true,
                    "AUXDEF" | "DEF" => {}
                    "YES" | "NO" | "PROMPT" => create = Some(arg.to_ascii_uppercase()),
                    other => {
                        return Err(self.err(format!("unknown DATA header argument {other:?}")));
                    }
                }
            }
            Ok((object_type.to_string(), data_name, fields, csv, create))
        } else {
            // Concise: `object_type [DataName](fields)`, always space delimited.
            let mut words = before.split_whitespace();
            let object_type = words
                .next()
                .ok_or_else(|| self.err("concise header has no object type"))?
                .to_string();
            let data_name = words.next().map(str::to_string);
            if words.next().is_some() {
                return Err(self.err("concise header has more than two words before `(`"));
            }
            let fields = split_fields(inner);
            if fields.is_empty() {
                return Err(self.err("empty field list"));
            }
            Ok((object_type, data_name, fields, false, None))
        }
    }

    fn expect_open_brace(&mut self) -> Result<()> {
        loop {
            let Some(line) = self.next_line() else {
                return Err(self.err("DATA section with no `{`"));
            };
            let t = strip_comment(line).trim();
            if t == "{" {
                return Ok(());
            }
            if !t.is_empty() {
                return Err(self.err(format!("expected `{{` after DATA header, found {t:?}")));
            }
        }
    }

    /// Parse the value rows between the braces. A row may span lines; it is
    /// complete when it has one value per declared field. `SUBDATA` blocks
    /// attach to the row above them.
    fn body(&mut self, fields: &[String], csv: bool) -> Result<Vec<AuxRow>> {
        let mut rows: Vec<AuxRow> = Vec::new();
        let mut pending: Vec<String> = Vec::new();
        loop {
            let Some(line) = self.next_line() else {
                return Err(self.err("unterminated DATA section (no closing `}`)"));
            };
            let trimmed = line.trim();
            if trimmed == "}" {
                if !pending.is_empty() {
                    return Err(self.err(format!(
                        "row ended with {} of {} values at the closing brace",
                        pending.len(),
                        fields.len()
                    )));
                }
                return Ok(rows);
            }
            if let Some(name) = subdata_open(trimmed) {
                if !pending.is_empty() {
                    return Err(self.err(format!(
                        "SUBDATA after an incomplete row ({} of {} values)",
                        pending.len(),
                        fields.len()
                    )));
                }
                let subdata = self.subdata(name)?;
                let Some(row) = rows.last_mut() else {
                    return Err(self.err("SUBDATA before any value row"));
                };
                row.subdata.push(subdata);
                continue;
            }
            let stripped = strip_comment(line).trim();
            if stripped.is_empty() {
                continue;
            }
            split_values_into(stripped, csv, &mut pending);
            if pending.len() > fields.len() {
                return Err(self.err(format!(
                    "row has {} values for {} declared fields",
                    pending.len(),
                    fields.len()
                )));
            }
            if pending.len() == fields.len() {
                rows.push(AuxRow {
                    values: std::mem::take(&mut pending),
                    subdata: Vec::new(),
                });
            }
        }
    }

    /// Collect a `<SUBDATA name>` block's interior verbatim.
    fn subdata(&mut self, name: &str) -> Result<AuxSubData> {
        let mut lines = Vec::new();
        loop {
            let Some(line) = self.next_line() else {
                return Err(self.err(format!("unterminated SUBDATA {name}")));
            };
            if line.trim().eq_ignore_ascii_case("</SUBDATA>") {
                return Ok(AuxSubData {
                    name: name.to_string(),
                    lines,
                });
            }
            lines.push(line.to_string());
        }
    }
}

/// The `<SUBDATA name>` opener's name, if `line` is one.
fn subdata_open(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("<SUBDATA")?;
    let rest = rest.strip_suffix('>')?;
    let name = rest.trim();
    (!name.is_empty()).then_some(name)
}

/// Does `text` start with `word` as a whole word (case insensitive)?
fn first_word_is(text: &str, word: &str) -> bool {
    // `get` instead of indexing: `word.len()` may land inside a multibyte
    // character on arbitrary input text, where slicing would panic; a non
    // boundary there correctly means the keyword is not present whole.
    text.get(..word.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(word))
        && !text[word.len()..]
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
}

/// Truncate `line` at the first `//` outside quotes.
fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut in_quote = false;
    for i in 0..bytes.len() {
        match bytes[i] {
            b'"' => in_quote = !in_quote,
            b'/' if !in_quote && bytes.get(i + 1) == Some(&b'/') => return &line[..i],
            _ => {}
        }
    }
    line
}

/// Split a field list on commas, trimming each name. Empty entries (a trailing
/// comma before a line break) are dropped.
fn split_fields(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .map(str::to_string)
        .collect()
}

/// Append the values on one line to `out`. Space delimited unless `csv`;
/// quoted strings keep their interior (including embedded spaces and commas)
/// and an empty quoted token (`""`) is preserved as an empty value.
fn split_values_into(line: &str, csv: bool, out: &mut Vec<String>) {
    if csv {
        // Split on top-level commas, then unquote each piece. Whitespace
        // around a piece is insignificant; the quoted interior is verbatim.
        let mut start = 0;
        let mut in_quote = false;
        let bytes = line.as_bytes();
        for i in 0..=bytes.len() {
            let at_end = i == bytes.len();
            if at_end || (bytes[i] == b',' && !in_quote) {
                let piece = line[start..i].trim();
                let value = piece
                    .strip_prefix('"')
                    .and_then(|p| p.strip_suffix('"'))
                    .unwrap_or(piece);
                out.push(value.to_string());
                start = i + 1;
            } else if bytes[i] == b'"' {
                in_quote = !in_quote;
            }
        }
        return;
    }
    let mut cur = String::new();
    let mut in_quote = false;
    let mut started = false; // a token has begun, including an empty quoted one
    for c in line.chars() {
        match c {
            '"' => {
                in_quote = !in_quote;
                started = true;
            }
            c if c.is_whitespace() && !in_quote => {
                if started {
                    out.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            c => {
                cur.push(c);
                started = true;
            }
        }
    }
    if started {
        out.push(cur);
    }
}

// ---- Canonical writer -------------------------------------------------------

/// Serialize an [`AuxFile`] in canonical form: legacy headers, space delimited
/// values, one row per line, two space indentation. Idempotent under
/// `parse_aux`.
#[must_use]
pub fn write_aux(file: &AuxFile) -> String {
    let mut s = String::new();
    for section in &file.sections {
        match section {
            AuxSection::Data(d) => write_object(&mut s, d),
            AuxSection::Script(sc) => {
                match &sc.name {
                    Some(name) => {
                        let _ = writeln!(s, "SCRIPT {name}");
                    }
                    None => s.push_str("SCRIPT\n"),
                }
                s.push_str("{\n");
                for line in &sc.lines {
                    s.push_str(line);
                    s.push('\n');
                }
                s.push_str("}\n\n");
            }
        }
    }
    s
}

fn write_object(s: &mut String, d: &AuxObject) {
    // Legacy syntax puts the optional section name between DATA and `(`.
    match &d.data_name {
        Some(name) => {
            let _ = write!(s, "DATA {name}");
        }
        None => s.push_str("DATA "),
    }
    let _ = write!(s, "({}, [{}]", d.object_type, d.fields.join(", "));
    if let Some(create) = &d.create_if_not_found {
        let _ = write!(s, ", AUXDEF, {create}");
    }
    s.push_str(")\n{\n");
    for row in &d.rows {
        s.push_str("  ");
        for (i, v) in row.values.iter().enumerate() {
            if i > 0 {
                s.push(' ');
            }
            push_value(s, v);
        }
        s.push('\n');
        for sub in &row.subdata {
            let _ = writeln!(s, "  <SUBDATA {}>", sub.name);
            for line in &sub.lines {
                s.push_str(line);
                s.push('\n');
            }
            s.push_str("  </SUBDATA>\n");
        }
    }
    s.push_str("}\n\n");
}

/// Write one value, quoting when the bare token would not survive a re-read:
/// empty, embedded whitespace or comma, or a `//` that would read as a comment.
fn push_value(s: &mut String, v: &str) {
    let needs_quotes =
        v.is_empty() || v.contains(char::is_whitespace) || v.contains(',') || v.contains("//");
    if needs_quotes {
        s.push('"');
        s.push_str(v);
        s.push('"');
    } else {
        s.push_str(v);
    }
}
