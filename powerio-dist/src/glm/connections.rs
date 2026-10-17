//! Explicit native delta/wye channels, independent of the legacy D phase flag.

use num_complex::Complex64;

pub(super) const EXPLICIT: [(&str, usize, Option<usize>); 6] = [
    ("AB", 0, Some(1)),
    ("BC", 1, Some(2)),
    ("CA", 2, Some(0)),
    ("AN", 0, None),
    ("BN", 1, None),
    ("CN", 2, None),
];

pub(super) fn reference(nominal: f64, from: usize, to: Option<usize>) -> Complex64 {
    super::read::nominal_phasor(nominal, from)
        - to.map_or(Complex64::default(), |p| {
            super::read::nominal_phasor(nominal, p)
        })
}
