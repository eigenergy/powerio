use std::ops::Range;

use powerio_core::{Error, Source, SourceBuffer, SourceSpan};

use crate::diagnostics::codes;

const MAX_BYTES: usize = 64 << 20;
const MAX_TOKENS: usize = 1 << 20;
const MAX_TOKEN_BYTES: usize = 1 << 20;
const MAX_NESTING: usize = 64;

/// Lexical category. Words keep punctuation used in units and complex literals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TokenKind {
    Word,
    Quoted,
    OpenBrace,
    CloseBrace,
    Semicolon,
    Directive,
}

/// A token borrows its bytes through a shared source-buffer owner.
#[derive(Clone, Debug)]
pub struct Token {
    kind: TokenKind,
    buffer: SourceBuffer,
    range: Range<usize>,
}

impl Token {
    #[must_use]
    pub const fn kind(&self) -> TokenKind {
        self.kind
    }

    /// Exact spelling, including quotes, escapes, or a directive's leading `#`.
    #[must_use]
    #[expect(
        clippy::missing_panics_doc,
        reason = "private token fields are created only from validated UTF-8 ranges"
    )]
    pub fn text(&self) -> &str {
        // Only the UTF-8-validated lexer constructs tokens; all token
        // boundaries are ASCII delimiters or complete UTF-8 character edges.
        std::str::from_utf8(&self.buffer.bytes()[self.range.clone()])
            .expect("GLM token boundaries remain valid UTF-8")
    }

    #[must_use]
    pub fn span(&self) -> SourceSpan {
        span(&self.buffer, self.range.clone())
    }
}

/// Source syntax retains duplicate properties and unknown declarations in order.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Statement {
    /// A statement ending in a semicolon, such as a property or module import.
    Simple {
        tokens: Vec<Token>,
        span: SourceSpan,
    },
    /// A brace-delimited declaration, including nested and inline objects.
    Block {
        header: Vec<Token>,
        body: Vec<Statement>,
        span: SourceSpan,
    },
    /// A preprocessing line, retained without executing or evaluating it.
    Directive(Token),
}

/// Parsed syntax with its source owner. Source bytes, comments and whitespace
/// remain available exactly as supplied; the tree does not replace those bytes.
#[derive(Clone, Debug)]
pub struct RawGlm {
    pub source: Source,
    pub statements: Vec<Statement>,
}

/// Parse one primary GLM buffer without preprocessing or electrical projection.
/// Includes and conditionals remain directive statements in this syntax API.
/// Input is bounded to 64 MiB, one million tokens, 1 MiB per token and 64
/// nested blocks. All spans address the original bytes, including a UTF-8 BOM.
pub fn parse_raw_source(source: Source) -> Result<RawGlm, Error> {
    let result = (|| {
        let buffer = source.primary_buffer()?;
        let tokens = lex(&buffer)?;
        let mut parser = Parser {
            buffer: &buffer,
            tokens: tokens.into_iter().peekable(),
        };
        parser.statements(0, None)
    })();
    match result {
        Ok(statements) => Ok(RawGlm { source, statements }),
        Err(error) => Err(error.with_source(source)),
    }
}

fn span(buffer: &SourceBuffer, range: Range<usize>) -> SourceSpan {
    SourceSpan::new(buffer.id().clone(), range.start as u64, range.end as u64)
        .expect("lexer ranges are ordered")
}

fn malformed(buffer: &SourceBuffer, range: Range<usize>, message: &str) -> Error {
    Error::new(&codes::PARSE_GLM_SYNTAX, message).with_span(span(buffer, range))
}

fn limited(buffer: &SourceBuffer, range: Range<usize>, message: &str) -> Error {
    Error::new(&codes::PARSE_GLM_LIMIT_EXCEEDED, message).with_span(span(buffer, range))
}

fn lex(buffer: &SourceBuffer) -> Result<Vec<Token>, Error> {
    let bytes = buffer.bytes();
    if bytes.len() > MAX_BYTES {
        return Err(limited(buffer, 0..bytes.len(), "GLM input exceeds 64 MiB"));
    }
    std::str::from_utf8(bytes).map_err(|error| {
        let start = error.valid_up_to();
        malformed(
            buffer,
            start..start + error.error_len().unwrap_or(1),
            "GLM source is not UTF-8",
        )
    })?;
    if let Some(offset) = bytes.iter().position(|byte| *byte == 0) {
        return Err(malformed(
            buffer,
            offset..offset + 1,
            "GLM source contains a NUL byte",
        ));
    }
    let mut tokens = Vec::new();
    let mut cursor = if buffer.has_utf8_bom() { 3 } else { 0 };
    let mut line_start = true;
    while cursor < bytes.len() {
        let start = cursor;
        if bytes[cursor].is_ascii_whitespace() {
            if matches!(bytes[cursor], b'\n' | b'\r') {
                line_start = true;
            }
            cursor += 1;
            continue;
        }
        if bytes[cursor..].starts_with(b"//") {
            cursor += 2;
            while cursor < bytes.len() && !matches!(bytes[cursor], b'\n' | b'\r') {
                cursor += 1;
            }
            continue;
        }
        if bytes[cursor..].starts_with(b"/*") {
            cursor += 2;
            while cursor < bytes.len() && !bytes[cursor..].starts_with(b"*/") {
                if matches!(bytes[cursor], b'\n' | b'\r') {
                    line_start = true;
                }
                cursor += 1;
            }
            if cursor == bytes.len() {
                return Err(malformed(
                    buffer,
                    start..cursor,
                    "unterminated GLM block comment",
                ));
            }
            cursor += 2;
            continue;
        }
        let kind = match bytes[cursor] {
            b'{' => {
                cursor += 1;
                TokenKind::OpenBrace
            }
            b'}' => {
                cursor += 1;
                TokenKind::CloseBrace
            }
            b';' => {
                cursor += 1;
                TokenKind::Semicolon
            }
            b'#' if line_start => {
                while cursor < bytes.len() && !matches!(bytes[cursor], b'\n' | b'\r') {
                    cursor += 1;
                }
                TokenKind::Directive
            }
            b'\'' | b'"' => {
                cursor = quoted_end(buffer, start)?;
                TokenKind::Quoted
            }
            _ => {
                while cursor < bytes.len() {
                    if bytes[cursor..].starts_with(b"${") {
                        cursor += 2;
                        while cursor < bytes.len() && bytes[cursor] != b'}' {
                            cursor += 1;
                        }
                        if cursor == bytes.len() {
                            return Err(malformed(
                                buffer,
                                start..cursor,
                                "unterminated GLM variable reference",
                            ));
                        }
                        cursor += 1;
                    } else if bytes[cursor].is_ascii_whitespace()
                        || matches!(bytes[cursor], b'{' | b'}' | b';' | b'\'' | b'"')
                        || bytes[cursor..].starts_with(b"//")
                        || bytes[cursor..].starts_with(b"/*")
                    {
                        break;
                    } else {
                        cursor += 1;
                    }
                }
                TokenKind::Word
            }
        };
        line_start = false;
        if cursor - start > MAX_TOKEN_BYTES || tokens.len() == MAX_TOKENS {
            return Err(limited(
                buffer,
                start..cursor,
                "GLM token size or count exceeds its limit",
            ));
        }
        tokens.push(Token {
            kind,
            buffer: buffer.clone(),
            range: start..cursor,
        });
    }
    Ok(tokens)
}

fn quoted_end(buffer: &SourceBuffer, start: usize) -> Result<usize, Error> {
    let bytes = buffer.bytes();
    let quote = bytes[start];
    let mut cursor = start + 1;
    while cursor < bytes.len() {
        let byte = bytes[cursor];
        cursor += 1;
        if byte == quote {
            return Ok(cursor);
        }
        if byte == b'\\' && cursor < bytes.len() {
            cursor += 1;
        }
    }
    Err(malformed(
        buffer,
        start..bytes.len(),
        "unterminated GLM quoted token",
    ))
}

struct Parser<'a> {
    buffer: &'a SourceBuffer,
    tokens: std::iter::Peekable<std::vec::IntoIter<Token>>,
}

impl Parser<'_> {
    fn statements(
        &mut self,
        depth: usize,
        opening: Option<usize>,
    ) -> Result<Vec<Statement>, Error> {
        let mut output = Vec::new();
        let mut header = Vec::<Token>::new();
        if opening.is_some()
            && self
                .tokens
                .peek()
                .is_some_and(|t| t.kind == TokenKind::CloseBrace)
        {
            return Ok(output);
        }
        while let Some(token) = self.tokens.next() {
            match token.kind {
                TokenKind::Directive => {
                    if !header.is_empty() {
                        return Err(malformed(
                            self.buffer,
                            token.range,
                            "directive interrupts a GLM statement",
                        ));
                    }
                    output.push(Statement::Directive(token));
                }
                TokenKind::Word | TokenKind::Quoted => header.push(token),
                TokenKind::Semicolon => {
                    if let Some(first) = header.first() {
                        output.push(Statement::Simple {
                            span: span(self.buffer, first.range.start..token.range.end),
                            tokens: std::mem::take(&mut header),
                        });
                    }
                }
                TokenKind::OpenBrace => {
                    output.push(self.block(std::mem::take(&mut header), token, depth)?);
                }
                TokenKind::CloseBrace => {
                    return Err(malformed(
                        self.buffer,
                        token.range,
                        "unexpected GLM closing brace",
                    ));
                }
            }
            if opening.is_some()
                && self
                    .tokens
                    .peek()
                    .is_some_and(|t| t.kind == TokenKind::CloseBrace)
            {
                if let Some(first) = header.first() {
                    return Err(malformed(
                        self.buffer,
                        first.range.clone(),
                        "GLM property is missing its semicolon",
                    ));
                }
                return Ok(output);
            }
        }
        if let Some(start) = opening {
            return Err(malformed(
                self.buffer,
                start..self.buffer.bytes().len(),
                "unterminated GLM block",
            ));
        }
        if let Some(first) = header.first() {
            return Err(malformed(
                self.buffer,
                first.range.clone(),
                "GLM statement is missing its semicolon or block",
            ));
        }
        Ok(output)
    }

    fn block(
        &mut self,
        header: Vec<Token>,
        opening: Token,
        depth: usize,
    ) -> Result<Statement, Error> {
        let Some(first) = header.first() else {
            return Err(malformed(
                self.buffer,
                opening.range,
                "GLM block has no declaration",
            ));
        };
        if depth == MAX_NESTING {
            return Err(limited(
                self.buffer,
                opening.range,
                "GLM blocks exceed 64 nesting levels",
            ));
        }
        let start = first.range.start;
        let body = self.statements(depth + 1, Some(opening.range.start))?;
        // The recursive call leaves the matching closing brace.
        let closing = self
            .tokens
            .next()
            .expect("a successful nested parse leaves a brace");
        let end = if self
            .tokens
            .peek()
            .is_some_and(|t| t.kind == TokenKind::Semicolon)
        {
            self.tokens.next().expect("peeked token exists").range.end
        } else {
            closing.range.end
        };
        Ok(Statement::Block {
            header,
            body,
            span: span(self.buffer, start..end),
        })
    }
}
