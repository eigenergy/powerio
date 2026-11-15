//! Pinned OpenDSS overhead, concentric-neutral and tape-shield geometry
//! calculations in SI units.
//!
//! Ordered conductor definitions and shared native calculation caches feed
//! conductor-resolved Line snapshots. Unqualified cable paths retain
//! explicit unresolved-geometry diagnostics.
mod cable;
pub(super) mod cache;
pub(super) mod definition;
pub(super) mod direct;
mod spacing;
pub(super) mod timeline;
mod wire;

use super::linecode::LineMatrices;
use num_complex::Complex64 as Complex;
use serde::Deserialize;
use std::f64::consts::PI;

// These deliberately match LineConstants.pas in the pinned engine. Replacing
// them with higher-precision physical constants changes the source model.
#[expect(
    clippy::approx_constant,
    reason = "match the pinned OpenDSS engine constant"
)]
const TWO_PI: f64 = 6.283_185_307;
const MU_0: f64 = 12.566_37e-7;
const EPSILON_0: f64 = 8.854e-12;

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(in crate::dss) enum EarthModel {
    Carson,
    FullCarson,
    Deri,
}

impl EarthModel {
    pub fn parse(text: &str) -> Result<Self, String> {
        match text.to_ascii_lowercase().as_str() {
            "carson" => Ok(Self::Carson),
            "fullcarson" => Ok(Self::FullCarson),
            "deri" => Ok(Self::Deri),
            _ => Err(format!("unqualified earth model {text:?}")),
        }
    }
}

/// One conductor in SI units: position, radii, resistances per meter, and
/// the cable screen around it when it is a shielded cable.
#[derive(Clone, Copy, Debug, Deserialize)]
pub(crate) struct Wire {
    #[serde(default)]
    pub cable: Option<cable::Data>,
    pub x: f64,
    pub height: f64,
    pub radius: f64,
    pub capacitance_radius: f64,
    pub gmr: f64,
    pub rdc: f64,
    pub rac: f64,
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct Geometry {
    pub wires: Vec<Wire>,
    pub phases: usize,
    pub reduce: bool,
    #[serde(default)]
    pub reduction_order: Option<usize>,
    pub earth_model: EarthModel,
    pub rho: f64,
}

fn bessel_ratio(alpha: Complex) -> Complex {
    if alpha.norm() > 35.0 {
        return Complex::new(1.0, 0.0);
    }
    // Same bounded series and absolute term criterion as native mathutil.
    let z = alpha * alpha * 0.25;
    let mut term = z;
    let mut zero = Complex::new(1.0, 0.0) + term;
    for i in 2..=1001 {
        term = z * term / f64::from(i * i);
        zero += term;
        if term.norm_sqr() < 1e-20 {
            break;
        }
    }
    let mut term = alpha / 2.0;
    let mut one = term;
    let mut increment = term;
    for i in (4..=1000).step_by(2) {
        let next = alpha / f64::from(i);
        term *= increment * next;
        one += term;
        increment = next;
        if term.norm_sqr() < 1e-20 {
            break;
        }
    }
    zero / one
}

impl Geometry {
    fn validate(&self, frequency: f64) -> Result<(), String> {
        let n = self.wires.len();
        if n == 0
            || n > 32
            || self.phases == 0
            || self.phases > n
            || !frequency.is_finite()
            || frequency <= 0.0
            || !self.rho.is_finite()
            || self.rho <= 0.0
        {
            return Err("overhead geometry needs 1..32 conductors, a valid phase count, positive frequency and earth resistivity".into());
        }
        for (i, wire) in self.wires.iter().enumerate() {
            if !wire.x.is_finite()
                || [
                    wire.height,
                    wire.radius,
                    wire.capacitance_radius,
                    wire.gmr,
                    wire.rdc,
                    wire.rac,
                ]
                .iter()
                .any(|v| !v.is_finite() || *v <= 0.0)
            {
                return Err(format!("overhead conductor {} has invalid SI data", i + 1));
            }
            for other in &self.wires[..i] {
                if (wire.x - other.x).hypot(wire.height - other.height) < wire.radius + other.radius
                {
                    return Err("overhead conductors occupy the same space".into());
                }
            }
        }
        Ok(())
    }

    /// The conductor pair's loop impedance with earth return, without the
    /// internal impedance: the simplified Carson equation whole, or the
    /// free space term `j ω μ₀ / 2π · ln(1 / D)` plus the earth model's
    /// correction.
    pub(super) fn conductor_loop(
        &self,
        i: usize,
        j: usize,
        distance: f64,
        frequency: f64,
    ) -> Complex {
        let omega_mu = TWO_PI * frequency * MU_0;
        if matches!(self.earth_model, EarthModel::Carson) {
            return crate::overhead::calc_carson_impedance(
                distance,
                crate::overhead::CarsonEarth::new(
                    omega_mu / 8.0,
                    omega_mu / TWO_PI,
                    (658.5 * (self.rho / frequency).sqrt()).ln(),
                ),
            );
        }
        Complex::new(0.0, omega_mu / TWO_PI * (1.0 / distance).ln()) + self.earth(i, j, frequency)
    }

    fn earth(&self, i: usize, j: usize, frequency: f64) -> Complex {
        let first = &self.wires[i];
        let second = &self.wires[j];
        let omega_mu = TWO_PI * frequency * MU_0;
        match self.earth_model {
            EarthModel::Carson => {
                unreachable!("the simplified Carson self and mutual terms are formed whole")
            }
            EarthModel::Deri => {
                let depth = 1.0 / Complex::new(0.0, omega_mu / self.rho).sqrt();
                let height = first.height.abs() + second.height.abs() + 2.0 * depth;
                let argument = if i == j {
                    height
                } else {
                    (height * height + (first.x - second.x).powi(2)).sqrt()
                };
                Complex::new(0.0, omega_mu / TWO_PI) * argument.ln()
            }
            EarthModel::FullCarson => {
                let distance = (first.height.abs() + second.height.abs()).hypot(first.x - second.x);
                let theta = if i == j {
                    0.0
                } else {
                    ((first.height.abs() + second.height.abs()) / distance).acos()
                };
                let scaled_distance = 2.8099e-3 * distance * (frequency / self.rho).sqrt();
                let b1 = 1.0 / (3.0 * 2.0_f64.sqrt());
                let b2 = 1.0 / 16.0;
                let b3 = b1 / 15.0;
                let b4 = b2 / 24.0;
                let c2 = 1.365_931_5_f64;
                let c4 = c2 + 1.0 / 4.0 + 1.0 / 6.0;
                let real = PI / 8.0 - b1 * scaled_distance * theta.cos()
                    + b2 * scaled_distance
                        * scaled_distance
                        * ((c2.exp() / scaled_distance).ln() * (2.0 * theta).cos()
                            + theta * (2.0 * theta).sin())
                    + b3 * scaled_distance.powi(3) * (3.0 * theta).cos()
                    - b4 * PI / 4.0 * scaled_distance.powi(4) * (4.0 * theta).cos();
                let imaginary = 0.5 * (1.85138 / scaled_distance).ln()
                    + b1 * scaled_distance * theta.cos()
                    - b2 * PI / 4.0 * scaled_distance * scaled_distance * (2.0 * theta).cos()
                    + b3 * scaled_distance.powi(3) * (3.0 * theta).cos()
                    - b4 * scaled_distance.powi(4)
                        * ((c4.exp() / scaled_distance).ln() * (4.0 * theta).cos()
                            + theta * (4.0 * theta).sin())
                    + 0.5 * distance.ln();
                Complex::new(real, imaginary) * (omega_mu / PI)
            }
        }
    }

    pub fn calculate(&self, frequency: f64) -> Result<LineMatrices, String> {
        if self.wires.iter().any(|w| w.cable.is_some()) {
            return cable::calculate(self, frequency);
        }
        self.validate(frequency)?;
        let n = self.wires.len();
        let omega = TWO_PI * frequency;
        let mut z = vec![vec![Complex::default(); n]; n];
        let mut potential = vec![vec![0.0; n]; n];
        for i in 0..n {
            let wire = &self.wires[i];
            let mut internal = match self.earth_model {
                EarthModel::Carson | EarthModel::FullCarson => {
                    Complex::new(wire.rac, omega * MU_0 / (8.0 * PI))
                }
                EarthModel::Deri => {
                    let alpha = Complex::new(1.0, 1.0) * (frequency * MU_0 / wire.rdc).sqrt();
                    Complex::new(1.0, 1.0)
                        * bessel_ratio(alpha)
                        * (wire.rdc * frequency * MU_0).sqrt()
                        / 2.0
                }
            };
            let radius = if frequency > 40.0 && frequency < 1000.0 {
                internal.im = 0.0;
                wire.gmr
            } else {
                wire.radius
            };
            z[i][i] = internal + self.conductor_loop(i, i, radius, frequency);
            potential[i][i] = crate::overhead::calc_potential_coefficient(
                2.0 * wire.height,
                wire.capacitance_radius,
            );
            for j in 0..i {
                let other = &self.wires[j];
                let distance = (wire.x - other.x).hypot(wire.height - other.height);
                z[i][j] = self.conductor_loop(i, j, distance, frequency);
                z[j][i] = z[i][j];
                potential[i][j] = crate::overhead::calc_potential_coefficient(
                    (wire.x - other.x).hypot(wire.height + other.height),
                    distance,
                );
                potential[j][i] = potential[i][j];
            }
        }
        let inverse = crate::dense::invert_dense_matrix(&potential, 0.0)
            .ok_or("geometry potential matrix is singular")?;
        let keep = self
            .reduction_order
            .unwrap_or(if self.reduce { self.phases } else { n });
        // OpenDSS eliminates one conductor at a time, the last first.
        for k in (keep..n).rev() {
            z = crate::kron::calc_kron_reduced_impedance(
                &z,
                &(0..k).collect::<Vec<_>>(),
                &[k],
                0.0,
            )
            .ok_or("geometry neutral reduction has a zero pivot")?;
        }
        let result = LineMatrices {
            r: z.iter()
                .take(keep)
                .map(|row| row.iter().take(keep).map(|v| v.re).collect())
                .collect(),
            x: z.iter()
                .take(keep)
                .map(|row| row.iter().take(keep).map(|v| v.im).collect())
                .collect(),
            // Native reduction retains the phase block of the inverted full
            // potential matrix; it does not Kron-reduce that admittance.
            b: inverse
                .iter()
                .take(keep)
                .map(|row| {
                    row.iter()
                        .take(keep)
                        .map(|v| v * TWO_PI * EPSILON_0 * omega)
                        .collect()
                })
                .collect(),
        };
        if [&result.r, &result.x, &result.b]
            .into_iter()
            .flatten()
            .flatten()
            .any(|v| !v.is_finite())
        {
            return Err("geometry calculation became nonfinite".into());
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests;
