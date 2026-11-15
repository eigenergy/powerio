//! Shared native primitive validity check for staged source and Line lowering.

use num_complex::Complex64;

use crate::dense::{DensePivot, DenseRefusal};

pub(super) fn validate(impedance: &[Vec<Complex64>], kind: &str, code: u32) -> Result<(), String> {
    // The reference engine's refusal: the one dense elimination in
    // TcMatrix.Invert's diagonal pivot order. It refuses a zero self
    // impedance with nonzero mutuals, which row pivoting inverts, and
    // accepts a pivot left at rounding noise: an exact zero pivot is the
    // native inversion error, with no epsilon test. The model's own inverse
    // pivots by rows, so this inverse is discarded.
    match crate::dense::eliminate_dense_matrix(impedance, DensePivot::OpenDssDiagonal, 0.0, 0.0) {
        Ok(_) => Ok(()),
        Err(DenseRefusal::Singular) => Err(format!(
            "native {kind} primitive inversion has a zero diagonal pivot (error {code})"
        )),
        Err(DenseRefusal::NonFinite) => {
            Err(format!("{kind} primitive elimination became nonfinite"))
        }
    }
}
