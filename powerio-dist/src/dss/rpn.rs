//! RPN expression evaluator matching OpenDSS's TRPNCalc (Parser/RPN.cpp).
//!
//! OpenDSS evaluates any quoted token that is not a plain number as an RPN
//! expression: `(8 1000 /)` is 8/1000, `(1 2 +)` is 3. The calculator is a
//! ten register HP style stack; entering a number rolls the stack up, binary
//! operators combine X and Y and roll down. Trig works in degrees. The roll
//! operations shift rather than rotate, mirroring the reference exactly.

const STACK: usize = 10;

pub(crate) struct RpnCalc {
    /// s[0] is the X register, s[1] Y, s[2] Z.
    s: [f64; STACK],
}

impl RpnCalc {
    pub(crate) fn new() -> Self {
        RpnCalc { s: [0.0; STACK] }
    }

    pub(crate) fn x(&self) -> f64 {
        self.s[0]
    }

    fn roll_up(&mut self) {
        for i in (1..STACK).rev() {
            self.s[i] = self.s[i - 1];
        }
    }

    fn roll_dn(&mut self) {
        for i in 1..STACK {
            self.s[i - 1] = self.s[i];
        }
    }

    fn enter(&mut self, v: f64) {
        self.roll_up();
        self.s[0] = v;
    }

    fn binary(&mut self, f: impl Fn(f64, f64) -> f64) {
        // Matches the reference: result lands in Y, then the stack rolls down.
        self.s[1] = f(self.s[1], self.s[0]);
        self.roll_dn();
    }

    /// Applies one RPN token. Returns false for an unrecognized op.
    pub(crate) fn apply(&mut self, token: &str) -> bool {
        if let Some(v) = parse_number(token) {
            self.enter(v);
            return true;
        }
        let d = std::f64::consts::PI / 180.0;
        match token.to_ascii_lowercase().as_str() {
            "+" => self.binary(|y, x| y + x),
            "-" => self.binary(|y, x| y - x),
            "*" => self.binary(|y, x| y * x),
            "/" => self.binary(|y, x| y / x),
            "^" => self.binary(f64::powf),
            "atan2" => self.binary(move |y, x| y.atan2(x) / d),
            "sqrt" => self.s[0] = self.s[0].sqrt(),
            "sqr" => self.s[0] = self.s[0] * self.s[0],
            "sin" => self.s[0] = (self.s[0] * d).sin(),
            "cos" => self.s[0] = (self.s[0] * d).cos(),
            "tan" => self.s[0] = (self.s[0] * d).tan(),
            "asin" => self.s[0] = self.s[0].asin() / d,
            "acos" => self.s[0] = self.s[0].acos() / d,
            "atan" => self.s[0] = self.s[0].atan() / d,
            "ln" => self.s[0] = self.s[0].ln(),
            "exp" => self.s[0] = self.s[0].exp(),
            "log10" => self.s[0] = self.s[0].log10(),
            "inv" => self.s[0] = 1.0 / self.s[0],
            "pi" => self.enter(std::f64::consts::PI),
            "swap" => self.s.swap(0, 1),
            "rollup" => self.roll_up(),
            "rolldn" => self.roll_dn(),
            _ => return false,
        }
        true
    }
}

/// Number parsing with Pascal `val` semantics: the whole token must be a
/// decimal or scientific float; `inf`/`nan` spellings are not numbers.
pub(crate) fn parse_number(token: &str) -> Option<f64> {
    if token.is_empty()
        || !token
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'+' | b'-' | b'e' | b'E'))
    {
        return None;
    }
    token.parse::<f64>().ok().filter(|v| v.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(tokens: &[&str]) -> f64 {
        let mut c = RpnCalc::new();
        for t in tokens {
            assert!(c.apply(t), "bad RPN token {t}");
        }
        c.x()
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn arithmetic() {
        assert_eq!(eval(&["8", "1000", "/"]), 0.008);
        assert_eq!(eval(&["1", "2", "+"]), 3.0);
        assert_eq!(eval(&["10", "4", "-"]), 6.0);
        assert_eq!(eval(&["2", "3", "^"]), 8.0);
    }

    #[test]
    fn degrees_trig() {
        assert!((eval(&["30", "sin"]) - 0.5).abs() < 1e-12);
        assert!((eval(&["60", "cos"]) - 0.5).abs() < 1e-12);
        assert!((eval(&["1", "1", "atan2"]) - 45.0).abs() < 1e-12);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn stack_ops() {
        assert_eq!(eval(&["2", "5", "swap", "-"]), 3.0);
        assert!((eval(&["pi"]) - std::f64::consts::PI).abs() < 1e-15);
        assert_eq!(eval(&["9", "sqrt"]), 3.0);
        assert_eq!(eval(&["4", "inv"]), 0.25);
    }

    #[test]
    fn unknown_op() {
        let mut c = RpnCalc::new();
        assert!(!c.apply("bogus"));
    }

    #[test]
    fn number_syntax() {
        assert_eq!(parse_number("1.5e3"), Some(1500.0));
        assert_eq!(parse_number(".5"), Some(0.5));
        assert_eq!(parse_number("-2"), Some(-2.0));
        assert_eq!(parse_number("inf"), None);
        assert_eq!(parse_number("nan"), None);
        assert_eq!(parse_number("1.2.3"), None);
        assert_eq!(parse_number(""), None);
    }
}
