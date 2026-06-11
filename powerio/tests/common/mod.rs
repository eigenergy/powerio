//! Shared helpers for the converter integration tests. Each test binary
//! compiles this module and uses its own subset, hence the `allow(dead_code)`.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// Structural + numeric (tolerant) equality of two JSON values: same shape and
/// keys, numbers within a small relative tolerance. The per-unit PowerModels
/// round-trip (÷base on write, ×base on read) is not bit-exact in f64, so the
/// JSON comparisons use this rather than `==`.
#[allow(dead_code)]
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
                && xs
                    .iter()
                    .all(|(k, p)| ys.get(k).is_some_and(|q| json_approx_eq(p, q)))
        }
        _ => a == b,
    }
}

/// A vendored PowerWorld fixture under `tests/data/powerworld/`.
#[allow(dead_code)]
pub fn powerworld_vendored(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data/powerworld")
        .join(name)
}

/// A fetched ACTIVSg2000 fixture (`benchmarks/fetch_powerworld.sh`); `None`
/// when the fetch has not run, so tests skip instead of fail.
#[allow(dead_code)]
pub fn activsg2000_fetched(name: &str) -> Option<PathBuf> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data/large/ACTIVSg2000")
        .join(name);
    p.exists().then_some(p)
}

/// Branch circuit identity: the trimmed `LineCircuit` extra, `"1"` (the
/// PowerWorld default) when absent.
#[allow(dead_code)]
pub fn ckt(b: &powerio::Branch) -> String {
    b.extras
        .get("LineCircuit")
        .and_then(|v| v.as_str())
        .unwrap_or("1")
        .trim()
        .to_string()
}
