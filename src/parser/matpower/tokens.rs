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
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum State {
        Code,
        InString(u8),
    }
    let bytes = line.as_bytes();
    let mut state = State::Code;
    for (i, &b) in bytes.iter().enumerate() {
        match (state, b) {
            (State::Code, b'%') => return &line[..i],
            (State::Code, b'\'') => state = State::InString(b'\''),
            (State::Code, b'"') => state = State::InString(b'"'),
            (State::InString(q), c) if c == q => state = State::Code,
            _ => {}
        }
    }
    line
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
