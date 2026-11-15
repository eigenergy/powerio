//! Overhead line conductor equations shared by the OpenDSS and GridLAB-D
//! geometry readers: the simplified Carson earth return impedance and
//! Maxwell's potential coefficients.

use num_complex::Complex64;

/// The earth return of the simplified Carson equations, in the unit system
/// the caller works in (ohms per unit length and the length unit of its
/// distances).
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct CarsonEarth {
    /// `ω μ₀ / 8`, the earth return resistance per unit length.
    pub resistance: f64,
    /// `ω μ₀ / 2π`, the reactance per unit length per neper of distance.
    pub reactance: f64,
    /// `ln D_e`, the natural log of the equivalent earth return depth
    /// (`658.5 √(ρ / f)` metres), in the caller's distance unit.
    pub log_depth: f64,
}

impl CarsonEarth {
    /// The earth return from its three coefficients.
    #[must_use]
    pub const fn new(resistance: f64, reactance: f64, log_depth: f64) -> Self {
        Self {
            resistance,
            reactance,
            log_depth,
        }
    }
}

/// The simplified Carson impedance of a conductor pair with earth return,
/// per unit length, without the conductor's own internal impedance:
///
/// ```text
/// z = ω μ₀ / 8 + j (ω μ₀ / 2π · ln(1 / D) + ω μ₀ / 2π · ln D_e)
/// ```
///
/// with `D` the pair's distance, or a conductor's geometric mean radius for
/// its self term. The one simplified Carson equation: the OpenDSS geometry
/// (`EarthModel::Carson`, SI units) and the GridLAB-D overhead configuration
/// (feet and miles, its own rounded coefficients) read it.
#[must_use]
pub fn calc_carson_impedance(distance: f64, earth: CarsonEarth) -> Complex64 {
    Complex64::new(
        earth.resistance,
        earth.reactance * (1.0 / distance).ln() + earth.reactance * earth.log_depth,
    )
}

/// Maxwell's potential coefficient between two conductors (without the
/// `1 / 2πε` factor): `ln(S / D)`, with `S` the distance to the other
/// conductor's earth image and `D` the direct distance. A conductor's self
/// coefficient takes `S = 2h` and `D` its radius. The one potential
/// coefficient: the OpenDSS and GridLAB-D geometry capacitances read it.
#[must_use]
pub fn calc_potential_coefficient(image_distance: f64, distance: f64) -> f64 {
    (image_distance / distance).ln()
}
