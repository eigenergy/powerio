//! The branch pi model: the one place its nodal admittance block and its
//! self-loop fold are formed.

use num_complex::Complex64;

/// The primitives of a branch pi model, per unit: series admittance, the
/// terminal charging at each end, the tap magnitude, and the phase shift in
/// radians. The matrix builders, the preparations and the bus merge all
/// evaluate a branch through it.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct PiModel {
    /// Series admittance `y = 1/(r + jx)`.
    pub series: Complex64,
    /// Charging admittance at the from end.
    pub from_charging: Complex64,
    /// Charging admittance at the to end.
    pub to_charging: Complex64,
    /// Tap magnitude.
    pub tap: f64,
    /// Phase shift, radians.
    pub shift: f64,
}

impl PiModel {
    /// A pi model from its primitives.
    #[must_use]
    pub const fn new(
        series: Complex64,
        from_charging: Complex64,
        to_charging: Complex64,
        tap: f64,
        shift: f64,
    ) -> Self {
        Self {
            series,
            from_charging,
            to_charging,
            tap,
            shift,
        }
    }

    /// The nodal admittance block `[Yff, Yft, Ytf, Ytt]` in `makeYbus` form:
    ///
    /// ```text
    /// Yff = (y + y_fr) / tap²    Yft = -y / conj(t)
    /// Ytf = -y / t               Ytt = y + y_to
    /// ```
    ///
    /// with `t = tap e^{j shift}`.
    #[must_use]
    pub fn calc_block(&self) -> [Complex64; 4] {
        let a = Complex64::from_polar(self.tap, self.shift);
        let a_norm_sqr = self.tap * self.tap;
        let y = self.series;
        let y_ff = (y + self.from_charging) / a_norm_sqr;
        let y_tt = y + self.to_charging;
        let y_ft = -y / a.conj();
        let y_tf = -y / a;
        [y_ff, y_ft, y_tf, y_tt]
    }

    /// The admittance the branch adds to its one bus when both its ends are
    /// on that bus: `Yff + Yft + Ytf + Ytt` of [`calc_block`](Self::calc_block),
    /// formed as
    ///
    /// ```text
    /// y |1/tap - e^{j shift}|² + y_fr / tap² + y_to
    /// ```
    ///
    /// The squared norm avoids the cancellation between the four block
    /// entries for a near unity tap, and it is exactly zero for a nominal
    /// jumper (unity tap, no shift). The one self-loop fold: `Y_bus`, the AC
    /// preparation and the bus merge read it.
    #[must_use]
    pub fn calc_folded(&self) -> Complex64 {
        let (sin, cos) = self.shift.sin_cos();
        let inverse_tap = self.tap.recip();
        let factor = (inverse_tap - cos).powi(2) + sin.powi(2);
        self.series * factor + self.from_charging * inverse_tap.powi(2) + self.to_charging
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fold equals the block sum up to rounding, and is exactly zero
    /// where the block sum only cancels to zero.
    #[test]
    fn the_fold_is_the_block_sum() {
        let model = PiModel::new(
            Complex64::new(1.2, -8.5),
            Complex64::new(0.001, 0.02),
            Complex64::new(0.0, 0.03),
            0.97,
            0.1,
        );
        let [a, b, c, d] = model.calc_block();
        let sum = a + b + c + d;
        let folded = model.calc_folded();
        assert!((sum - folded).norm() <= 1e-12 * sum.norm().max(1.0));

        let jumper = PiModel::new(
            Complex64::new(3.0, -1e6),
            Complex64::new(0.0, 0.0),
            Complex64::new(0.0, 0.0),
            1.0,
            0.0,
        );
        assert_eq!(jumper.calc_folded(), Complex64::new(0.0, 0.0));
    }
}
