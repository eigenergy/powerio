//! Shared native primitive validity check for staged source and Line lowering.

use num_complex::Complex64;

pub(super) fn validate(impedance: &[Vec<Complex64>], kind: &str, code: u32) -> Result<(), String> {
    // The native engine refuses a primitive it cannot invert (error `code`).
    // Invertibility is the one dense inverse's verdict
    // (`crate::dense::invert_dense_matrix`); no inverse is substituted into
    // the model here.
    let inverse = crate::dense::invert_dense_matrix(impedance, 0.0).ok_or_else(|| {
        format!("native {kind} primitive inversion has a zero pivot (error {code})")
    })?;
    if inverse
        .iter()
        .flatten()
        .any(|value| !value.re.is_finite() || !value.im.is_finite())
    {
        return Err(format!("{kind} primitive elimination became nonfinite"));
    }
    Ok(())
}
