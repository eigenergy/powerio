use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use powerio_core::{Error, Source, SourceBuffer, SourceId, SourceSpan};

use crate::diagnostics::codes;

const MAX_BYTES: usize = 64 << 20;
const MAX_LINE: usize = 1 << 20;
const MAX_ORIGINS: usize = 1 << 20;
const MAX_FILES: usize = 4096;
const MAX_DEPTH: usize = 64;
const MAX_DEFINES: usize = 4096;

/// Loader conventions qualified against the pinned engine versions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum GlmDialect {
    #[default]
    Pnnl5_3,
    Arras4_3,
}

/// Explicit preprocessing inputs. No process environment or simulator globals
/// are consulted. Callers must supply the globals required by their profile.
#[derive(Clone, Debug, Default)]
pub struct PreprocessOptions {
    pub dialect: GlmDialect,
    pub defines: BTreeMap<String, String>,
    /// Permit repeated includes, but never an include cycle.
    pub reinclude: bool,
}

/// A contiguous expansion segment and the source bytes responsible for it.
#[derive(Clone, Debug)]
pub struct ExpansionOrigin {
    pub expanded: Range<usize>,
    pub original: SourceSpan,
    /// True for a byte-for-byte copy. False for a variable substitution or a
    /// separator inserted at an include boundary.
    pub exact: bool,
}

/// Preprocessed text, distinct from the retained original project. This is not
/// an electrically interpreted network and must never replace source retention.
#[derive(Clone, Debug)]
pub struct PreparedGlm {
    pub source: Source,
    text: String,
    origins: Vec<ExpansionOrigin>,
    /// Buffers visited in first-use order; excludes unused supplied buffers.
    dependencies: Vec<SourceId>,
    absolute_include: bool,
}

impl PreparedGlm {
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub fn origins(&self) -> &[ExpansionOrigin] {
        &self.origins
    }

    #[must_use]
    pub fn dependencies(&self) -> &[SourceId] {
        &self.dependencies
    }

    pub(super) const fn has_absolute_include(&self) -> bool {
        self.absolute_include
    }

    /// Translate an expanded byte range to its original source segments.
    /// Substitution segments point to the complete `${name}` reference.
    /// Empty or out-of-bounds ranges return an empty vector.
    #[must_use]
    pub fn original_spans(&self, range: Range<usize>) -> Vec<SourceSpan> {
        self.original_span_iter(range).collect()
    }

    pub(crate) fn original_span_iter(
        &self,
        mut range: Range<usize>,
    ) -> impl Iterator<Item = SourceSpan> + '_ {
        if range.start >= range.end || range.end > self.text.len() {
            range = 0..0;
        }
        let first = self
            .origins
            .partition_point(|o| o.expanded.end <= range.start);
        self.origins[first..]
            .iter()
            .take_while(move |o| o.expanded.start < range.end)
            .map(move |o| {
                if o.exact {
                    let start = o.original.byte_start()
                        + (range.start.max(o.expanded.start) - o.expanded.start) as u64;
                    let end = o.original.byte_start()
                        + (range.end.min(o.expanded.end) - o.expanded.start) as u64;
                    SourceSpan::new(o.original.source().clone(), start, end)
                        .expect("intersected expansion ranges are ordered")
                } else {
                    o.original.clone()
                }
            })
    }
}

/// Expand local quoted includes containing complete declarations,
/// `#define NAME=value`, `#ifdef`, `#ifndef`,
/// string-comparison `#if`, `#else`, `#endif`, and explicit `${NAME}` variables.
///
/// Substitution is one pass, including within quoted values. Defines become
/// visible in source order, also across includes. Inactive branches are removed
/// before syntax parsing. `#set` and `#warning` remain inert directive lines for
/// subsequent interpretation; they do not populate the explicit define table.
/// Preprocessing switches `reinclude`, and Arras `literal_if` and
/// `relax_undefined_if`, are also applied as explicit boolean settings.
/// Other active directives fail explicitly. In particular, this function never
/// executes code, accesses the process environment, or fetches remote includes.
/// Include paths resolve relative to the primary file, matching an engine
/// launched in that directory; implicit `GLPATH` searches are not performed.
///
/// Bounds: 64 MiB processed input and expanded output, 1 MiB per line/definition,
/// 4096 files/defines, one million source-map segments, and 64 include or
/// conditional levels. Duplicate includes require `reinclude`; cycles fail.
pub fn preprocess_source(source: Source, options: PreprocessOptions) -> Result<PreparedGlm, Error> {
    let result: Result<_, Error> = (|| {
        let primary = source.primary_buffer()?;
        let define_bytes = options.defines.iter().fold(0usize, |n, (k, v)| {
            n.saturating_add(k.len()).saturating_add(v.len())
        });
        let mut context = Context {
            source: &source,
            include_base: primary.clone(),
            literal_if: options.dialect == GlmDialect::Arras4_3,
            relax_undefined_if: false,
            options,
            output: String::new(),
            origins: Vec::new(),
            dependencies: Vec::new(),
            absolute_include: false,
            visited: BTreeSet::new(),
            active: BTreeSet::new(),
            input_bytes: 0,
            define_bytes,
            boundary: Boundary::default(),
        };
        context.check_defines(&primary, 0..0)?;
        context.file(&primary, 0)?;
        Ok((
            context.output,
            context.origins,
            context.dependencies,
            context.absolute_include,
        ))
    })();
    match result {
        Ok((text, origins, dependencies, absolute_include)) => Ok(PreparedGlm {
            source,
            text,
            origins,
            dependencies,
            absolute_include,
        }),
        Err(error) => Err(error.with_source(source)),
    }
}

struct Context<'a> {
    source: &'a Source,
    include_base: SourceBuffer,
    options: PreprocessOptions,
    output: String,
    origins: Vec<ExpansionOrigin>,
    dependencies: Vec<SourceId>,
    absolute_include: bool,
    visited: BTreeSet<SourceId>,
    active: BTreeSet<SourceId>,
    input_bytes: usize,
    define_bytes: usize,
    boundary: Boundary,
    literal_if: bool,
    relax_undefined_if: bool,
}

/// Incremental declaration boundary check, independent of expansion segments.
/// The syntax parser still validates the actual declarations after expansion.
#[derive(Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "incremental lexical boundary flags span independent expansion chunks"
)]
struct Boundary {
    depth: usize,
    quote: Option<u8>,
    escaped: bool,
    slash: bool,
    block_comment: bool,
    star: bool,
    line_comment: bool,
    pending: bool,
    invalid: bool,
}

impl Boundary {
    fn ready(&self) -> bool {
        self.depth == 0
            && self.quote.is_none()
            && !self.block_comment
            && !self.slash
            && !self.pending
            && !self.invalid
    }

    fn observe(&mut self, text: &str) {
        for byte in text.bytes() {
            if self.line_comment {
                if byte == b'\n' {
                    self.line_comment = false;
                }
                continue;
            }
            if self.block_comment {
                if self.star && byte == b'/' {
                    self.block_comment = false;
                    self.star = false;
                } else {
                    self.star = byte == b'*';
                }
                continue;
            }
            if let Some(quote) = self.quote {
                if self.escaped {
                    self.escaped = false;
                } else if byte == b'\\' {
                    self.escaped = true;
                } else if byte == quote {
                    self.quote = None;
                }
                continue;
            }
            if self.slash {
                self.slash = false;
                match byte {
                    b'/' => {
                        self.line_comment = true;
                        continue;
                    }
                    b'*' => {
                        self.block_comment = true;
                        continue;
                    }
                    _ => self.pending = true,
                }
            }
            match byte {
                b'/' => self.slash = true,
                b'#' => self.line_comment = true,
                b'\'' | b'"' => {
                    self.quote = Some(byte);
                    self.pending = true;
                }
                b'{' => {
                    self.depth += 1;
                    self.pending = true;
                }
                b'}' => {
                    if self.depth == 0 {
                        self.invalid = true;
                    } else {
                        self.depth -= 1;
                        self.pending = self.depth != 0;
                    }
                }
                b';' if self.depth == 0 => self.pending = false,
                b if b.is_ascii_whitespace() => {}
                _ => self.pending = true,
            }
        }
    }
}

struct Conditional {
    parent: bool,
    condition: bool,
    has_else: bool,
    opening: Range<usize>,
}

fn span(buffer: &SourceBuffer, range: Range<usize>) -> SourceSpan {
    SourceSpan::new(buffer.id().clone(), range.start as u64, range.end as u64)
        .expect("preprocessor ranges are ordered")
}

fn error(buffer: &SourceBuffer, range: Range<usize>, message: impl Into<String>) -> Error {
    Error::new(&codes::PARSE_GLM_PREPROCESSING, message).with_span(span(buffer, range))
}

fn limit(buffer: &SourceBuffer, range: Range<usize>) -> Error {
    Error::new(
        &codes::PARSE_GLM_LIMIT_EXCEEDED,
        "GLM preprocessing resource limit exceeded",
    )
    .with_span(span(buffer, range))
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

impl Context<'_> {
    fn check_defines(&self, buffer: &SourceBuffer, range: Range<usize>) -> Result<(), Error> {
        if self.options.defines.len() > MAX_DEFINES
            || self.options.defines.values().any(|v| v.len() > MAX_LINE)
            || self.define_bytes > MAX_BYTES
        {
            return Err(limit(buffer, range));
        }
        if self.options.defines.keys().any(|k| !valid_name(k)) {
            return Err(error(
                buffer,
                range,
                "GLM defines require alphanumeric/underscore names",
            ));
        }
        if self
            .options
            .defines
            .values()
            .any(|v| v.contains(['\r', '\n', '\0']))
        {
            return Err(error(
                buffer,
                range,
                "GLM explicit variables must be single-line non-NUL text",
            ));
        }
        Ok(())
    }

    fn file(&mut self, buffer: &SourceBuffer, depth: usize) -> Result<(), Error> {
        let whole = 0..buffer.bytes().len();
        if depth == MAX_DEPTH || self.active.contains(buffer.id()) {
            return Err(error(buffer, whole, "GLM include cycle or depth limit"));
        }
        if self.visited.contains(buffer.id()) && !self.options.reinclude {
            return Err(error(
                buffer,
                whole,
                "GLM file included more than once without reinclude",
            ));
        }
        self.input_bytes = self.input_bytes.saturating_add(buffer.bytes().len());
        if self.input_bytes > MAX_BYTES {
            return Err(limit(buffer, whole));
        }
        if self.visited.insert(buffer.id().clone()) {
            if self.visited.len() > MAX_FILES || self.input_bytes > MAX_BYTES {
                return Err(limit(buffer, whole));
            }
            self.dependencies.push(buffer.id().clone());
        }
        let text = std::str::from_utf8(buffer.bytes())
            .map_err(|_| error(buffer, whole.clone(), "GLM source is not UTF-8"))?;
        if text.contains('\0') {
            return Err(error(buffer, whole, "GLM source contains a NUL byte"));
        }
        self.active.insert(buffer.id().clone());
        let mut stack = Vec::<Conditional>::new();
        let mut enabled = true;
        let mut offset = if buffer.has_utf8_bom() { 3 } else { 0 };
        for line in text[offset..].split_inclusive('\n') {
            let range = offset..offset + line.len();
            if line.len() > MAX_LINE {
                return Err(limit(buffer, range));
            }
            let trimmed = line.trim_start();
            if let Some(directive) = trimmed.strip_prefix('#') {
                // Comments on macro lines must follow whitespace in native GLM.
                let directive = directive.split_once(" //").map_or(directive, |(a, _)| a);
                let directive = directive
                    .split_once("\t//")
                    .map_or(directive, |(a, _)| a)
                    .trim_end();
                let (name, term) = directive
                    .split_once(char::is_whitespace)
                    .unwrap_or((directive, ""));
                self.directive(
                    buffer,
                    range.clone(),
                    name,
                    term.trim_start(),
                    &mut stack,
                    &mut enabled,
                    depth,
                )?;
            } else if enabled {
                self.expand_to_output(buffer, range.clone())?;
            }
            offset = range.end;
        }
        if let Some(top) = stack.last() {
            return Err(error(
                buffer,
                top.opening.clone(),
                "GLM conditional has no #endif in its file",
            ));
        }
        self.active.remove(buffer.id());
        Ok(())
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn directive(
        &mut self,
        buffer: &SourceBuffer,
        range: Range<usize>,
        name: &str,
        term: &str,
        stack: &mut Vec<Conditional>,
        enabled: &mut bool,
        depth: usize,
    ) -> Result<(), Error> {
        match name {
            "ifdef" | "ifndef" | "if" => {
                if stack.len() == MAX_DEPTH {
                    return Err(limit(buffer, range));
                }
                let condition = if *enabled {
                    let term = self.expand_value(buffer, range.clone(), term)?;
                    if name == "if" {
                        self.condition(buffer, range.clone(), &term)?
                    } else {
                        if !valid_name(&term) {
                            return Err(error(buffer, range, "invalid GLM conditional variable"));
                        }
                        self.options.defines.contains_key(&term) == (name == "ifdef")
                    }
                } else {
                    false
                };
                stack.push(Conditional {
                    parent: *enabled,
                    condition,
                    has_else: false,
                    opening: range,
                });
                *enabled &= condition;
            }
            "else" => {
                let top = stack
                    .last_mut()
                    .ok_or_else(|| error(buffer, range.clone(), "GLM #else without #if"))?;
                if top.has_else || !term.is_empty() {
                    return Err(error(buffer, range, "duplicate or malformed GLM #else"));
                }
                top.has_else = true;
                *enabled = top.parent && !top.condition;
            }
            "endif" => {
                let top = stack
                    .pop()
                    .ok_or_else(|| error(buffer, range.clone(), "GLM #endif without #if"))?;
                if !term.is_empty() {
                    return Err(error(buffer, range, "GLM #endif has unexpected text"));
                }
                *enabled = top.parent;
            }
            _ if !*enabled => {}
            "define" => {
                let value = self.expand_value(buffer, range.clone(), term)?;
                let (key, value) = value.split_once('=').unwrap_or((&value, ""));
                if !valid_name(key) {
                    return Err(error(
                        buffer,
                        range,
                        "expected #define NAME=value or #define NAME",
                    ));
                }
                if let Some(old) = self.options.defines.get(key) {
                    self.define_bytes -= old.len();
                } else {
                    self.define_bytes += key.len();
                }
                self.define_bytes += value.len();
                if self.define_bytes > MAX_BYTES
                    || (self.options.defines.len() == MAX_DEFINES
                        && !self.options.defines.contains_key(key))
                {
                    return Err(limit(buffer, range));
                }
                self.options
                    .defines
                    .insert(key.to_owned(), value.to_owned());
            }
            "include" => {
                if !self.boundary.ready() {
                    return Err(error(
                        buffer,
                        range,
                        "GLM includes require a complete top-level declaration boundary",
                    ));
                }
                let value = self.expand_value(buffer, range.clone(), term)?;
                let value = value.strip_suffix(';').unwrap_or(&value).trim_end();
                let path = value
                    .strip_prefix('"')
                    .and_then(|s| s.strip_suffix('"'))
                    .filter(|s| !s.is_empty() && !s.contains(['"', '\\', '\n', '\r', ':']));
                let path = path.ok_or_else(|| {
                    error(
                        buffer,
                        range.clone(),
                        "only local quoted GLM includes are supported",
                    )
                })?;
                let included = self
                    .source
                    .referenced_buffer(&self.include_base, path)
                    .map_err(|e| e.with_span(span(buffer, range.clone())))?;
                self.absolute_include |= path.starts_with('/');
                self.file(&included, depth + 1)?;
                if !self.boundary.ready() {
                    return Err(error(
                        buffer,
                        range,
                        "included GLM file ends inside a declaration",
                    ));
                }
                self.append("\n", span(buffer, range), false)?;
            }
            "set" | "warning" => {
                // These remain syntax, not execution. Their later interpretation
                // must decide static readiness and typed global conversions.
                if name == "set" {
                    let term = self.expand_value(buffer, range.clone(), term)?;
                    if let Some((key, value)) = term.split_once('=')
                        && (key == "reinclude"
                            || (self.options.dialect == GlmDialect::Arras4_3
                                && matches!(key, "literal_if" | "relax_undefined_if")))
                    {
                        let value = match value {
                            "TRUE" | "true" | "1" => true,
                            "FALSE" | "false" | "0" => false,
                            _ => {
                                return Err(error(
                                    buffer,
                                    range,
                                    "GLM preprocessing switch requires an explicit boolean",
                                ));
                            }
                        };
                        match key {
                            "reinclude" => self.options.reinclude = value,
                            "literal_if" => self.literal_if = value,
                            _ => self.relax_undefined_if = value,
                        }
                    }
                }
                self.expand_to_output(buffer, range)?;
            }
            _ => {
                return Err(error(
                    buffer,
                    range,
                    format!("unsupported active GLM directive #{name}"),
                ));
            }
        }
        Ok(())
    }

    fn condition(
        &self,
        buffer: &SourceBuffer,
        range: Range<usize>,
        term: &str,
    ) -> Result<bool, Error> {
        if self.options.dialect == GlmDialect::Arras4_3 {
            return self.arras_condition(buffer, range, term);
        }
        let at = term
            .find(['!', '<', '>', '='])
            .ok_or_else(|| error(buffer, range.clone(), "GLM #if requires NAME<op>value"))?;
        let (key, rest) = term.split_at(at);
        let op_end = rest
            .find(|c| !matches!(c, '!' | '<' | '>' | '='))
            .unwrap_or(rest.len());
        let (op, rhs) = rest.split_at(op_end);
        let lhs = self.options.defines.get(key).ok_or_else(|| {
            error(
                buffer,
                range.clone(),
                "GLM #if variable is not explicitly defined",
            )
        })?;
        match op {
            "<" => Ok(lhs.as_str() < rhs),
            ">" => Ok(lhs.as_str() > rhs),
            "<=" => Ok(lhs.as_str() <= rhs),
            ">=" => Ok(lhs.as_str() >= rhs),
            "==" => Ok(lhs == rhs),
            "!=" => Ok(lhs != rhs),
            _ => Err(error(buffer, range, "invalid GLM #if comparison operator")),
        }
    }

    fn arras_condition(
        &self,
        buffer: &SourceBuffer,
        range: Range<usize>,
        term: &str,
    ) -> Result<bool, Error> {
        let (left, rest) = if let Some(quoted) = term.strip_prefix('"') {
            quoted.split_once('"').ok_or_else(|| {
                error(buffer, range.clone(), "unterminated Arras #if left operand")
            })?
        } else {
            let end = term
                .find(|c: char| c.is_ascii_whitespace() || matches!(c, '!' | '<' | '>' | '='))
                .ok_or_else(|| error(buffer, range.clone(), "missing Arras #if comparison"))?;
            term.split_at(end)
        };
        let rest = rest.trim_start();
        let op = ["not_in", "in", "<=", ">=", "==", "!=", "<", ">"]
            .into_iter()
            .find(|op| rest.starts_with(op))
            .ok_or_else(|| error(buffer, range.clone(), "invalid Arras #if comparison"))?;
        let rhs = rest[op.len()..].trim_start();
        let rhs = if let Some(quoted) = rhs.strip_prefix('"') {
            quoted
                .strip_suffix('"')
                .filter(|s| !s.contains('"'))
                .ok_or_else(|| {
                    error(
                        buffer,
                        range.clone(),
                        "unterminated Arras #if right operand",
                    )
                })?
        } else {
            if rhs.chars().any(char::is_whitespace) {
                return Err(error(
                    buffer,
                    range,
                    "Arras #if operands containing spaces must be quoted",
                ));
            }
            rhs
        };
        let lhs = if self.literal_if {
            left
        } else {
            match self.options.defines.get(left) {
                Some(value) => value,
                None if self.relax_undefined_if => "",
                None => {
                    return Err(error(
                        buffer,
                        range,
                        "Arras #if variable is not explicitly defined",
                    ));
                }
            }
        };
        match op {
            "<" => Ok(lhs < rhs),
            ">" => Ok(lhs > rhs),
            "<=" => Ok(lhs <= rhs),
            ">=" => Ok(lhs >= rhs),
            "==" => Ok(lhs == rhs),
            "!=" => Ok(lhs != rhs),
            "in" => Ok(lhs.contains(rhs)),
            "not_in" => Ok(!lhs.contains(rhs)),
            _ => unreachable!("operator was selected from a fixed set"),
        }
    }

    fn expand_value(
        &self,
        buffer: &SourceBuffer,
        range: Range<usize>,
        text: &str,
    ) -> Result<String, Error> {
        let mut output = String::new();
        for (literal, variable) in
            substitutions(text).map_err(|m| error(buffer, range.clone(), m))?
        {
            let value = if let Some(key) = variable {
                self.options.defines.get(key).ok_or_else(|| {
                    error(
                        buffer,
                        range.clone(),
                        format!("GLM variable {key} is not explicitly defined"),
                    )
                })?
            } else {
                literal
            };
            if output.len().saturating_add(value.len()) > MAX_LINE {
                return Err(limit(buffer, range));
            }
            output.push_str(value);
        }
        Ok(output)
    }

    fn expand_to_output(
        &mut self,
        buffer: &SourceBuffer,
        range: Range<usize>,
    ) -> Result<(), Error> {
        let text = std::str::from_utf8(&buffer.bytes()[range.clone()])
            .expect("file validates UTF-8 before splitting lines");
        let comment = if text.trim_start().starts_with('#') {
            text.find(" //").into_iter().chain(text.find("\t//")).min()
        } else {
            text.find("//")
        };
        let body = comment.map_or(text, |end| &text[..end]);
        let initial_size = self.output.len();
        let mut offset = range.start;
        for (literal, variable) in
            substitutions(body).map_err(|m| error(buffer, range.clone(), m))?
        {
            let original = span(buffer, offset..offset + literal.len());
            if let Some(key) = variable {
                let value = self
                    .options
                    .defines
                    .get(key)
                    .ok_or_else(|| {
                        error(
                            buffer,
                            offset..offset + literal.len(),
                            format!("GLM variable {key} is not explicitly defined"),
                        )
                    })?
                    .clone();
                self.append(&value, original, false)?;
            } else {
                self.append(literal, original, true)?;
            }
            offset += literal.len();
            if self.output.len() - initial_size > MAX_LINE {
                return Err(limit(buffer, range));
            }
        }
        if comment.is_some() && text.ends_with('\n') {
            self.append("\n", span(buffer, range.end - 1..range.end), true)?;
        }
        if !text.trim_start().starts_with('#')
            && self.output[initial_size..].trim_start().starts_with('#')
        {
            return Err(error(
                buffer,
                range,
                "GLM variable expansion generated a directive; explicit directive syntax is required",
            ));
        }
        Ok(())
    }

    fn append(&mut self, text: &str, original: SourceSpan, exact: bool) -> Result<(), Error> {
        if text.is_empty() {
            return Ok(());
        }
        if self.output.len().saturating_add(text.len()) > MAX_BYTES
            || self.origins.len() == MAX_ORIGINS
        {
            return Err(Error::new(
                &codes::PARSE_GLM_LIMIT_EXCEEDED,
                "GLM expansion exceeds its byte or source-map budget",
            )
            .with_span(original));
        }
        let start = self.output.len();
        self.output.push_str(text);
        self.boundary.observe(text);
        self.origins.push(ExpansionOrigin {
            expanded: start..self.output.len(),
            original,
            exact,
        });
        Ok(())
    }
}

/// Segments retain their original lengths, including the complete reference.
fn substitutions(mut text: &str) -> Result<Vec<(&str, Option<&str>)>, &'static str> {
    let mut result = Vec::new();
    while let Some(start) = text.find("${") {
        result.push((&text[..start], None));
        text = &text[start..];
        let end = text
            .find('}')
            .ok_or("unterminated GLM variable reference")?;
        let key = &text[2..end];
        if !valid_name(key) {
            return Err("GLM variable reference requires a simple explicit name");
        }
        result.push((&text[..=end], Some(key)));
        text = &text[end + 1..];
    }
    result.push((text, None));
    Ok(result)
}
