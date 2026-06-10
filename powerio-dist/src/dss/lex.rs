//! Tokenizer matching OpenDSS's TParser (Parser/ParserDel.cpp).
//!
//! A command line is a sequence of parameters, positional or `name=value`.
//! Delimiters are `,` and `=` plus space and tab; a token opening with one of
//! `( " ' [ {` runs to the matching closer and keeps delimiters inside;
//! `!` and `//` start a comment that eats the rest of the line. A token
//! beginning with `@` is replaced by the named parser variable, keeping any
//! `.node` suffix. Quoted tokens parse as RPN when read as numbers; vector
//! values re-tokenize their content with `|` terminating a matrix row.

use std::collections::BTreeMap;

use super::rpn::{self, RpnCalc};

/// Parser variables (`var @x=...`), looked up case insensitively with the
/// leading `@` included in the key.
pub type VarMap = BTreeMap<String, String>;

const BEGIN_QUOTE: &[u8] = b"(\"'[{";
const END_QUOTE: &[u8] = b")\"']}";

/// What ended the last token.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Delim {
    Whitespace,
    Char(u8),
    Comment,
}

/// One parameter from a command line.
#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    /// Property name to the left of `=`; `None` for a positional value.
    pub name: Option<String>,
    pub value: Value,
}

/// A raw value token. `quoted` records that the token came from a quote pair,
/// which switches numeric interpretation to RPN.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Value {
    pub text: String,
    pub quoted: bool,
}

/// A `bus1=name.1.2.0` bus reference: name plus ordered node numbers.
#[derive(Clone, Debug, PartialEq)]
pub struct BusSpec {
    pub name: String,
    /// Node numbers as written; `0` is ground. Unparseable nodes become -1,
    /// matching the reference parser's error marker.
    pub nodes: Vec<i32>,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ValueError {
    #[error("`{0}` is not a number")]
    NotANumber(String),
    #[error("bad RPN token `{token}` in `{expr}`")]
    BadRpn { expr: String, token: String },
}

pub struct Scanner<'a> {
    buf: &'a [u8],
    pos: usize,
    last_delim: Delim,
    /// Extra delimiter, the matrix row terminator `|` during vector parsing.
    row_term: bool,
    vars: Option<&'a VarMap>,
}

impl<'a> Scanner<'a> {
    pub fn new(line: &'a str, vars: Option<&'a VarMap>) -> Self {
        let mut s = Scanner {
            buf: line.as_bytes(),
            pos: 0,
            last_delim: Delim::Whitespace,
            row_term: false,
            vars,
        };
        s.skip_whitespace();
        s
    }

    fn skip_whitespace(&mut self) {
        while self.pos < self.buf.len() && matches!(self.buf[self.pos], b' ' | b'\t') {
            self.pos += 1;
        }
    }

    fn is_delim_char(&self, b: u8) -> bool {
        b == b',' || b == b'=' || (self.row_term && b == b'|')
    }

    fn at_comment(&self) -> bool {
        match self.buf.get(self.pos) {
            Some(b'!') => true,
            Some(b'/') => self.buf.get(self.pos + 1) == Some(&b'/'),
            _ => false,
        }
    }

    /// TParser::GetToken. Returns `None` at end of line; an empty token can
    /// occur mid stream (e.g. between consecutive commas), as in the
    /// reference.
    fn get_token(&mut self) -> Option<(String, bool)> {
        if self.pos >= self.buf.len() {
            return None;
        }
        self.last_delim = Delim::Whitespace;
        let mut quoted = false;
        let text;

        let open = self.buf[self.pos];
        if let Some(qi) = BEGIN_QUOTE.iter().position(|&q| q == open) {
            let close = END_QUOTE[qi];
            self.pos += 1;
            let start = self.pos;
            while self.pos < self.buf.len() && self.buf[self.pos] != close {
                self.pos += 1;
            }
            text = String::from_utf8_lossy(&self.buf[start..self.pos]).into_owned();
            if self.pos < self.buf.len() {
                self.pos += 1; // past the closer
            }
            quoted = true;
        } else {
            let start = self.pos;
            while self.pos < self.buf.len() {
                if self.at_comment() {
                    self.last_delim = Delim::Comment;
                    break;
                }
                let b = self.buf[self.pos];
                if self.is_delim_char(b) {
                    self.last_delim = Delim::Char(b);
                    break;
                }
                if matches!(b, b' ' | b'\t') {
                    self.last_delim = Delim::Whitespace;
                    break;
                }
                self.pos += 1;
            }
            text = String::from_utf8_lossy(&self.buf[start..self.pos]).into_owned();
        }

        if self.last_delim == Delim::Comment {
            self.pos = self.buf.len();
            return Some((text, quoted));
        }

        // Move past one terminating delimiter, eating whitespace around it,
        // so `a = b` and `a=b` scan identically.
        if self.last_delim == Delim::Whitespace {
            self.skip_whitespace();
        }
        if self.pos < self.buf.len() {
            if self.at_comment() {
                self.pos = self.buf.len();
                return Some((text, quoted));
            }
            let b = self.buf[self.pos];
            if self.is_delim_char(b) {
                self.last_delim = Delim::Char(b);
                self.pos += 1;
            }
        }
        self.skip_whitespace();
        Some((text, quoted))
    }

    /// TParser::CheckforVar: a token starting with `@` is replaced by its
    /// variable value, keeping a `.node.node` suffix (`^` also cuts the
    /// name). A value stored as `{...}` unwraps and becomes a quoted token.
    fn substitute(&self, token: String, quoted: bool) -> (String, bool) {
        if token.len() < 2 || !token.starts_with('@') {
            return (token, quoted);
        }
        let Some(vars) = self.vars else {
            return (token, quoted);
        };
        let cut = token.find(['.', '^']).unwrap_or(token.len());
        let (name, suffix) = token.split_at(cut);
        let key = name.to_ascii_lowercase();
        let Some(value) = vars.get(&key) else {
            return (token, quoted);
        };
        if let Some(inner) = value.strip_prefix('{').and_then(|v| v.strip_suffix('}')) {
            (format!("{inner}{suffix}"), true)
        } else {
            (format!("{value}{suffix}"), quoted)
        }
    }

    /// TParser::GetNextParam: one positional or `name=value` parameter.
    /// Variable substitution applies to the value, never the name.
    pub fn next_param(&mut self) -> Option<Param> {
        let (tok, quoted) = self.get_token()?;
        let (name, raw) = if self.last_delim == Delim::Char(b'=') {
            (Some(tok), self.get_token().unwrap_or_default())
        } else {
            (None, (tok, quoted))
        };
        let (text, quoted) = self.substitute(raw.0, raw.1);
        Some(Param {
            name,
            value: Value { text, quoted },
        })
    }

    /// Remaining unscanned text, trimmed; the argument tail for commands that
    /// take free text.
    pub fn remainder(&self) -> &str {
        std::str::from_utf8(&self.buf[self.pos.min(self.buf.len())..])
            .unwrap_or_default()
            .trim()
    }
}

impl Value {
    pub fn new(text: impl Into<String>) -> Self {
        Value {
            text: text.into(),
            quoted: false,
        }
    }

    /// TParser::MakeDouble_: quoted tokens evaluate as RPN, bare tokens must
    /// be plain numbers. An empty value is 0, as in the reference.
    pub fn to_f64(&self, vars: Option<&VarMap>) -> Result<f64, ValueError> {
        if self.text.is_empty() {
            return Ok(0.0);
        }
        if self.quoted {
            return self.eval_rpn(vars);
        }
        rpn::parse_number(&self.text).ok_or_else(|| ValueError::NotANumber(self.text.clone()))
    }

    /// TParser::MakeInteger_: parse as a double and round.
    pub fn to_i64(&self, vars: Option<&VarMap>) -> Result<i64, ValueError> {
        self.to_f64(vars).map(|v| v.round() as i64)
    }

    fn eval_rpn(&self, vars: Option<&VarMap>) -> Result<f64, ValueError> {
        let mut calc = RpnCalc::new();
        let mut scan = Scanner::new(&self.text, vars);
        while let Some((tok, _)) = scan.get_token() {
            if tok.is_empty() {
                continue;
            }
            let (tok, _) = scan.substitute(tok, false);
            if !calc.apply(&tok) {
                return Err(ValueError::BadRpn {
                    expr: self.text.clone(),
                    token: tok,
                });
            }
        }
        Ok(calc.x())
    }

    /// TParser::ParseAsVector over the whole value: numbers separated by
    /// whitespace or commas. `|` row terminators split a matrix value into
    /// rows; a plain vector is one row.
    pub fn to_rows(&self, vars: Option<&VarMap>) -> Result<Vec<Vec<f64>>, ValueError> {
        let mut rows = Vec::new();
        let mut row = Vec::new();
        let mut scan = Scanner::new(&self.text, vars);
        scan.row_term = true;
        while let Some((tok, quoted)) = scan.get_token() {
            if !tok.is_empty() {
                let (text, quoted) = scan.substitute(tok, quoted);
                row.push(Value { text, quoted }.to_f64(vars)?);
            }
            if scan.last_delim == Delim::Char(b'|') {
                rows.push(std::mem::take(&mut row));
            }
        }
        if !row.is_empty() || rows.is_empty() {
            rows.push(row);
        }
        Ok(rows)
    }

    /// A flat numeric vector (kVs, taps, ZIPV, ...).
    pub fn to_vector(&self, vars: Option<&VarMap>) -> Result<Vec<f64>, ValueError> {
        Ok(self.to_rows(vars)?.into_iter().flatten().collect())
    }

    /// A list of string items (`buses=(b1, b2)`, `conns=(wye delta)`).
    pub fn to_string_list(&self, vars: Option<&VarMap>) -> Vec<String> {
        let mut out = Vec::new();
        let mut scan = Scanner::new(&self.text, vars);
        while let Some((tok, quoted)) = scan.get_token() {
            if !tok.is_empty() {
                out.push(scan.substitute(tok, quoted).0);
            }
        }
        out
    }

    /// TParser::ParseAsBusName: `name.1.2.0` into name and node list.
    pub fn to_bus_spec(&self) -> BusSpec {
        let text = self.text.trim();
        match text.split_once('.') {
            None => BusSpec {
                name: text.to_string(),
                nodes: Vec::new(),
            },
            Some((name, rest)) => BusSpec {
                name: name.trim().to_string(),
                nodes: rest
                    .split('.')
                    .map(|n| n.trim().parse::<i32>().unwrap_or(-1))
                    .collect(),
            },
        }
    }

    /// OpenDSS boolean: leading `y`/`t`/`1` is true, anything else false.
    pub fn to_bool(&self) -> bool {
        matches!(
            self.text.bytes().next().map(|b| b.to_ascii_lowercase()),
            Some(b'y' | b't' | b'1')
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(line: &str) -> Vec<(Option<String>, String, bool)> {
        let mut scan = Scanner::new(line, None);
        let mut out = Vec::new();
        while let Some(p) = scan.next_param() {
            out.push((p.name, p.value.text, p.value.quoted));
        }
        out
    }

    #[test]
    fn positional_and_named() {
        let p = params("Line.l1 bus1=a bus2=b 0.3");
        assert_eq!(p[0], (None, "Line.l1".into(), false));
        assert_eq!(p[1], (Some("bus1".into()), "a".into(), false));
        assert_eq!(p[2], (Some("bus2".into()), "b".into(), false));
        assert_eq!(p[3], (None, "0.3".into(), false));
    }

    #[test]
    fn spaces_around_equals() {
        assert_eq!(params("a = b"), params("a=b"));
        assert_eq!(params("a =b"), params("a= b"));
    }

    #[test]
    fn comma_separates() {
        let p = params("conns=(wye, delta)");
        assert_eq!(p[0], (Some("conns".into()), "wye, delta".into(), true));
    }

    #[test]
    fn quote_pairs() {
        for (open, close) in [('(', ')'), ('"', '"'), ('\'', '\''), ('[', ']'), ('{', '}')] {
            let line = format!("x={open}1 2 3{close}");
            let p = params(&line);
            assert_eq!(p[0], (Some("x".into()), "1 2 3".into(), true), "{open}");
        }
    }

    #[test]
    fn comments_stop_the_line() {
        assert_eq!(params("a=1 ! trailing").len(), 1);
        assert_eq!(params("a=1 // trailing").len(), 1);
        assert_eq!(params("a=1!glued").len(), 1);
        assert!(params("! whole line").first().unwrap().1.is_empty());
    }

    #[test]
    fn slash_alone_is_not_a_comment() {
        let p = params("x=a/b");
        assert_eq!(p[0], (Some("x".into()), "a/b".into(), false));
    }

    #[test]
    fn rpn_value() {
        let v = Value {
            text: "8 1000 /".into(),
            quoted: true,
        };
        assert_eq!(v.to_f64(None), Ok(0.008));
        let bare = Value::new("3.5");
        assert_eq!(bare.to_f64(None), Ok(3.5));
        let bad = Value::new("abc");
        assert!(bad.to_f64(None).is_err());
    }

    #[test]
    fn quoted_single_number_is_rpn() {
        let v = Value {
            text: "42".into(),
            quoted: true,
        };
        assert_eq!(v.to_f64(None), Ok(42.0));
    }

    #[test]
    fn matrix_rows() {
        let v = Value {
            text: "0.088 | 0.031 0.090 | 0.030 0.031 0.088".into(),
            quoted: true,
        };
        let rows = v.to_rows(None).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0], vec![0.088]);
        assert_eq!(rows[2], vec![0.030, 0.031, 0.088]);
    }

    #[test]
    fn vector_with_commas() {
        let v = Value {
            text: "7.2, 0.24".into(),
            quoted: true,
        };
        assert_eq!(v.to_vector(None).unwrap(), vec![7.2, 0.24]);
    }

    #[test]
    fn rpn_inside_vector() {
        let v = Value {
            text: "1 \"8 1000 /\"".into(),
            quoted: true,
        };
        assert_eq!(v.to_vector(None).unwrap(), vec![1.0, 0.008]);
    }

    #[test]
    fn bus_dotting() {
        let b = Value::new("632.1.2.3.0").to_bus_spec();
        assert_eq!(b.name, "632");
        assert_eq!(b.nodes, vec![1, 2, 3, 0]);
        let plain = Value::new("sourcebus").to_bus_spec();
        assert_eq!(plain.name, "sourcebus");
        assert!(plain.nodes.is_empty());
        let bad = Value::new("b.1.x").to_bus_spec();
        assert_eq!(bad.nodes, vec![1, -1]);
    }

    #[test]
    fn var_substitution() {
        let mut vars = VarMap::new();
        vars.insert("@kv".into(), "12.47".into());
        vars.insert("@bus".into(), "632".into());
        vars.insert("@expr".into(), "{2 3 *}".into());
        let mut scan = Scanner::new("kv=@kv bus1=@bus.1.2 x=@expr y=@undef", Some(&vars));
        let p1 = scan.next_param().unwrap();
        assert_eq!(p1.value.text, "12.47");
        let p2 = scan.next_param().unwrap();
        assert_eq!(p2.value.text, "632.1.2");
        let p3 = scan.next_param().unwrap();
        assert_eq!(p3.value.text, "2 3 *");
        assert!(p3.value.quoted);
        assert_eq!(p3.value.to_f64(Some(&vars)), Ok(6.0));
        let p4 = scan.next_param().unwrap();
        assert_eq!(p4.value.text, "@undef");
    }

    #[test]
    fn string_list() {
        let v = Value {
            text: "b1, b2".into(),
            quoted: true,
        };
        assert_eq!(v.to_string_list(None), vec!["b1", "b2"]);
    }

    #[test]
    fn booleans() {
        assert!(Value::new("yes").to_bool());
        assert!(Value::new("Y").to_bool());
        assert!(Value::new("true").to_bool());
        assert!(Value::new("1").to_bool());
        assert!(!Value::new("no").to_bool());
        assert!(!Value::new("false").to_bool());
        assert!(!Value::new("").to_bool());
    }
}
