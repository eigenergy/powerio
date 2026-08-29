//! Strips MATLAB `%` line comments. Respects single and double quoted
//! string literals. Preserves line numbering so subsequent regex field
//! extraction stays simple.

pub(crate) fn strip_comments(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for line in input.lines() {
        out.push_str(strip_line_comment(line));
        out.push('\n');
    }
    out
}

fn strip_line_comment(line: &str) -> &str {
    comment_split(line).0
}

/// Split a line into `(code, comment)` at the first `%` that starts a comment,
/// respecting single/double quoted strings. The comment half keeps the leading
/// `%`; when there is no comment it is empty. Same FSM as [`strip_line_comment`]
/// but returns both halves so the document builder can round-trip comments.
pub(crate) fn comment_split(line: &str) -> (&str, &str) {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum State {
        Code,
        InString(u8),
    }
    let bytes = line.as_bytes();
    let mut state = State::Code;
    for (i, &b) in bytes.iter().enumerate() {
        match (state, b) {
            (State::Code, b'%') => return (&line[..i], &line[i..]),
            (State::Code, b'\'') => state = State::InString(b'\''),
            (State::Code, b'"') => state = State::InString(b'"'),
            (State::InString(q), c) if c == q => state = State::Code,
            _ => {}
        }
    }
    (line, "")
}

#[cfg(test)]
mod tests {
    use super::strip_line_comment;

    #[test]
    fn strips_trailing_comment() {
        assert_eq!(strip_line_comment("foo = 1; % bar"), "foo = 1; ");
    }

    #[test]
    fn keeps_percent_in_string() {
        assert_eq!(
            strip_line_comment("name = '50% load';"),
            "name = '50% load';"
        );
    }

    #[test]
    fn entire_line_comment() {
        assert_eq!(strip_line_comment("% all of it"), "");
    }

    #[test]
    fn no_comment() {
        assert_eq!(strip_line_comment("a = b + c;"), "a = b + c;");
    }
}
