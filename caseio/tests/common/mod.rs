//! Shared helpers for the converter integration tests.

use serde_json::Value;

/// Structural + numeric (tolerant) equality of two JSON values: same shape and
/// keys, numbers within a small relative tolerance. The per-unit PowerModels
/// round-trip (÷base on write, ×base on read) is not bit-exact in f64, so the
/// JSON comparisons use this rather than `==`.
pub fn json_approx_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => match (x.as_f64(), y.as_f64()) {
            (Some(xf), Some(yf)) => (xf - yf).abs() <= 1e-9 * xf.abs().max(yf.abs()).max(1.0),
            _ => x == y,
        },
        (Value::Array(xs), Value::Array(ys)) => {
            xs.len() == ys.len() && xs.iter().zip(ys).all(|(p, q)| json_approx_eq(p, q))
        }
        (Value::Object(xs), Value::Object(ys)) => {
            xs.len() == ys.len()
                && xs.iter().all(|(k, p)| ys.get(k).is_some_and(|q| json_approx_eq(p, q)))
        }
        _ => a == b,
    }
}
