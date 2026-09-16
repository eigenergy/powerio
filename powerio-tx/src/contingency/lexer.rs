//! The line tokenizer the PSS/E contingency analysis files share.
//!
//! The three files PSS/E's contingency solvers read (`.con`, `.sub`,
//! `.mon`) are line oriented free format text with one tokenizer between them:
//! whitespace runs separate tokens, a quoted token may hold spaces, and a line
//! or a line tail may be a comment. [`lex`] applies that tokenizer once and
//! keeps each line's original text and 1-based number, so a reader that cannot
//! interpret a line can keep the line as written and name its position.

/// One token of a statement line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Token<'a> {
    /// The token text with any surrounding quotes removed.
    pub text: &'a str,
    /// Whether the source quoted this token.
    pub quoted: bool,
}

/// What a source line carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LineKind {
    /// Nothing but whitespace.
    Blank,
    /// A full line comment: the first non-blank character is `/`, `!`, or `#`,
    /// or the first token is `COM`.
    Comment,
    /// At least one statement token.
    Statement,
}

/// One lexed source line.
#[derive(Debug, Clone)]
pub(crate) struct LexedLine<'a> {
    /// 1-based position in the source text.
    pub number: usize,
    /// The line as written, without its terminator.
    pub text: &'a str,
    pub kind: LineKind,
    /// Statement tokens, empty on a blank or comment line.
    pub tokens: Vec<Token<'a>>,
}

impl<'a> LexedLine<'a> {
    /// Token texts as written, quotes removed.
    pub fn words(&self) -> Vec<&'a str> {
        self.tokens.iter().map(|token| token.text).collect()
    }

    /// Token texts upper cased, for keyword matching.
    pub fn keywords(&self) -> Vec<String> {
        self.tokens
            .iter()
            .map(|token| token.text.to_ascii_uppercase())
            .collect()
    }

    /// The line with leading and trailing whitespace removed.
    pub fn trimmed(&self) -> &'a str {
        self.text.trim()
    }
}

/// Lex every line of `text`.
pub(crate) fn lex(text: &str) -> Vec<LexedLine<'_>> {
    text.lines()
        .enumerate()
        .map(|(index, line)| lex_line(index + 1, line))
        .collect()
}

fn lex_line(number: usize, text: &str) -> LexedLine<'_> {
    let trimmed = text.trim_start();
    if trimmed.is_empty() {
        return LexedLine {
            number,
            text,
            kind: LineKind::Blank,
            tokens: Vec::new(),
        };
    }
    if trimmed.starts_with(['/', '!', '#']) {
        return LexedLine {
            number,
            text,
            kind: LineKind::Comment,
            tokens: Vec::new(),
        };
    }
    let tokens = tokenize(text);
    let comment = tokens
        .first()
        .is_some_and(|token| !token.quoted && token.text.eq_ignore_ascii_case("COM"));
    let kind = if comment || tokens.is_empty() {
        LineKind::Comment
    } else {
        LineKind::Statement
    };
    LexedLine {
        number,
        text,
        kind,
        tokens: if comment { Vec::new() } else { tokens },
    }
}

/// Split one line into tokens. A token that opens with `'` or `"` runs to its
/// closing quote and may hold spaces; the quotes are removed and the inner
/// text kept. A token whose quote never closes runs to the end of the line and
/// drops the line's trailing whitespace, which is not part of the name. An
/// unquoted token whose first character is `/` after at least one statement
/// token ends the statement: the rest of the line is a comment.
fn tokenize(line: &str) -> Vec<Token<'_>> {
    let bytes = line.as_bytes();
    let mut tokens = Vec::new();
    let mut at = 0;
    while at < line.len() {
        while at < line.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        if at >= line.len() {
            break;
        }
        let quote = bytes[at];
        if quote == b'\'' || quote == b'"' {
            let start = at + 1;
            let Some(end) = closing_quote(line, start, quote) else {
                tokens.push(Token {
                    text: line[start..].trim_end(),
                    quoted: true,
                });
                break;
            };
            tokens.push(Token {
                text: &line[start..end],
                quoted: true,
            });
            at = end + 1;
            continue;
        }
        let start = at;
        while at < line.len() && !bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        let text = &line[start..at];
        if !tokens.is_empty() && text.starts_with('/') {
            break;
        }
        tokens.push(Token {
            text,
            quoted: false,
        });
    }
    tokens
}

/// The index of the quote that closes a token opened at `start`.
///
/// A quote followed by whitespace or by the end of the line closes the token.
/// A quote sitting inside a word is part of the name, which is how PSS/E
/// writes a case name holding an apostrophe (`'L_000022O'~1'`); the search
/// then continues to the last quote of the same character on the line.
fn closing_quote(line: &str, start: usize, quote: u8) -> Option<usize> {
    let quote_char = quote as char;
    let last = line.rfind(quote_char)?;
    let mut search = start;
    loop {
        let at = search + line[search..].find(quote_char)?;
        if at >= last {
            return Some(at);
        }
        match line[at + 1..].chars().next() {
            None => return Some(at),
            Some(next) if next.is_whitespace() => return Some(at),
            Some(_) => search = at + 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{LineKind, lex};

    fn words(line: &str) -> Vec<&str> {
        let lexed = lex(line);
        lexed[0].words()
    }

    #[test]
    fn whitespace_runs_collapse() {
        assert_eq!(
            words("OPEN LINE FROM BUS   1001 TO BUS   1064 CIRCUIT 1"),
            vec![
                "OPEN", "LINE", "FROM", "BUS", "1001", "TO", "BUS", "1064", "CIRCUIT", "1"
            ]
        );
        assert_eq!(
            words("\tREMOVE\tMACHINE\t1"),
            vec!["REMOVE", "MACHINE", "1"]
        );
    }

    #[test]
    fn a_quoted_token_keeps_its_spaces_and_loses_its_quotes() {
        assert_eq!(words("CKT '1 '"), vec!["CKT", "1 "]);
        assert_eq!(
            words("FROM BUS '02CHAMBR 345' TO BUS '03ABC 138'"),
            vec!["FROM", "BUS", "02CHAMBR 345", "TO", "BUS", "03ABC 138"]
        );
        assert_eq!(
            words("SUBSYSTEM \"North West\""),
            vec!["SUBSYSTEM", "North West"]
        );
    }

    #[test]
    fn a_quote_inside_a_word_belongs_to_the_name() {
        assert_eq!(
            words("CONTINGENCY 'L_000022O'~1'"),
            vec!["CONTINGENCY", "L_000022O'~1"]
        );
        assert_eq!(
            words("CONTINGENCY 'T_000004O'DO'"),
            vec!["CONTINGENCY", "T_000004O'DO"]
        );
    }

    #[test]
    fn an_unclosed_quote_runs_to_the_end_of_the_line() {
        assert_eq!(words("CONTINGENCY 'OPEN"), vec!["CONTINGENCY", "OPEN"]);
        // The line's trailing whitespace is not part of the name.
        assert_eq!(words("CONTINGENCY 'OPEN   "), vec!["CONTINGENCY", "OPEN"]);
        assert_eq!(words("CKT 'A B   "), vec!["CKT", "A B"]);
    }

    #[test]
    fn comment_lines_carry_no_tokens() {
        let lines = lex("/PSS(R)E 35\n! note\n# note\n// note\nCOM banner\n\nEND\n");
        let kinds: Vec<LineKind> = lines.iter().map(|line| line.kind).collect();
        assert_eq!(
            kinds,
            vec![
                LineKind::Comment,
                LineKind::Comment,
                LineKind::Comment,
                LineKind::Comment,
                LineKind::Comment,
                LineKind::Blank,
                LineKind::Statement,
            ]
        );
        assert!(lines[4].tokens.is_empty());
        assert_eq!(lines[4].text, "COM banner");
    }

    #[test]
    fn a_slash_token_after_a_statement_token_ends_the_statement() {
        assert_eq!(
            words("DISCONNECT BUS 5 / the load bus"),
            vec!["DISCONNECT", "BUS", "5"]
        );
        assert_eq!(
            words("DISCONNECT BUS 5 /the load bus"),
            vec!["DISCONNECT", "BUS", "5"]
        );
    }

    #[test]
    fn every_line_keeps_its_text_and_number() {
        let lines = lex("  A B\n\nC\n");
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].number, 1);
        assert_eq!(lines[0].text, "  A B");
        assert_eq!(lines[0].trimmed(), "A B");
        assert_eq!(lines[2].number, 3);
        assert_eq!(lines[2].keywords(), vec!["C".to_owned()]);
    }
}
