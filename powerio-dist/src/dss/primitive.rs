//! Shared native primitive validity check for staged source and Line lowering.

use num_complex::Complex64;

pub(super) fn validate(impedance: &[Vec<Complex64>], kind: &str, code: u32) -> Result<(), String> {
    // Check the successive Schur complements in the pinned engine's
    // diagonal-pivot order (TcMatrix.Invert). Only unused rows/columns affect
    // later pivots, so no inverse is constructed or substituted into the model.
    // An exact zero pivot triggers a native inversion error; there is no epsilon test.
    let mut reduced = impedance.to_vec();
    let mut remaining: Vec<_> = (0..reduced.len()).collect();
    while !remaining.is_empty() {
        let mut selected = None;
        let mut largest = 0.0;
        for &i in &remaining {
            // Native Cabs uses sqrt(re²+im²), including its range behavior.
            let magnitude = reduced[i][i].norm_sqr().sqrt();
            if magnitude > largest {
                largest = magnitude;
                selected = Some(i);
            }
        }
        let pivot = selected.ok_or_else(|| {
            format!("native {kind} primitive inversion has a zero diagonal pivot (error {code})")
        })?;
        remaining.retain(|&i| i != pivot);
        for &i in &remaining {
            for &j in &remaining {
                let correction =
                    scaled_division(reduced[i][pivot] * reduced[pivot][j], reduced[pivot][pivot]);
                reduced[i][j] -= correction;
                if !reduced[i][j].re.is_finite() || !reduced[i][j].im.is_finite() {
                    return Err(format!("{kind} primitive elimination became nonfinite"));
                }
            }
        }
    }
    Ok(())
}

fn scaled_division(numerator: Complex64, denominator: Complex64) -> Complex64 {
    // Ratio-based complex division matches the native runtime's arithmetic
    // order. A squared-denominator formula can round a zero pivot differently.
    if denominator.re.abs() > denominator.im.abs() {
        let ratio = denominator.im / denominator.re;
        let divisor = denominator.re + denominator.im * ratio;
        Complex64::new(
            (numerator.re + numerator.im * ratio) / divisor,
            (numerator.im - numerator.re * ratio) / divisor,
        )
    } else {
        let ratio = denominator.re / denominator.im;
        let divisor = denominator.im + denominator.re * ratio;
        Complex64::new(
            (numerator.im + numerator.re * ratio) / divisor,
            (-numerator.re + numerator.im * ratio) / divisor,
        )
    }
}
