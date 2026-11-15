//! Pinned OpenDSS overhead, concentric-neutral and tape-shield geometry
//! calculations in SI units.
//!
//! Ordered conductor definitions and shared native calculation caches feed
//! conductor-resolved Line snapshots. Unqualified cable paths retain
//! explicit unresolved-geometry diagnostics.
mod cable;
pub(super) mod cache;
pub(crate) use cable::{Data as CableData, Screen};
pub(super) mod definition;
pub(super) mod direct;
mod spacing;
pub(super) mod timeline;
mod wire;
pub(crate) use wire::{AC_TO_DC_RESISTANCE, calc_conductor_resistances};

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
                    crate::overhead::calc_earth_return_log_depth(
                        self.rho,
                        frequency,
                        crate::overhead::EarthDepthForm::OpenDss,
                    ),
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

    /// A conductor's self impedance: its internal impedance
    /// ([`calc_internal_impedance`]) plus its loop with earth return at its
    /// GMR. Outside power frequencies (40 to 1000 Hz) the loop is taken at
    /// the radius and the internal reactance kept, as `LineConstants` does.
    fn conductor_self_impedance(&self, i: usize, frequency: f64) -> Complex {
        let wire = &self.wires[i];
        let mut internal = calc_internal_impedance(wire, self.earth_model, frequency);
        let radius = if frequency > 40.0 && frequency < 1000.0 {
            internal.im = 0.0;
            wire.gmr
        } else {
            wire.radius
        };
        internal + self.conductor_loop(i, i, radius, frequency)
    }

    /// The conductors' series impedance block, self and mutual, in wire
    /// order: what the overhead and the cable models both start from.
    fn conductor_block(&self, frequency: f64) -> Vec<Vec<Complex>> {
        let n = self.wires.len();
        let mut z = vec![vec![Complex::default(); n]; n];
        for (i, wire) in self.wires.iter().enumerate() {
            z[i][i] = self.conductor_self_impedance(i, frequency);
            for (j, other) in self.wires.iter().enumerate().take(i) {
                let distance = (wire.x - other.x).hypot(wire.height - other.height);
                z[i][j] = self.conductor_loop(i, j, distance, frequency);
                z[j][i] = z[i][j];
            }
        }
        z
    }

    /// The conductors this geometry keeps: its Kron order, else its phases
    /// when it reduces, else every conductor.
    fn kept_conductors(&self) -> usize {
        self.reduction_order.unwrap_or(if self.reduce {
            self.phases
        } else {
            self.wires.len()
        })
    }

    pub fn calculate(&self, frequency: f64) -> Result<LineMatrices, String> {
        if self.wires.iter().any(|w| w.cable.is_some()) {
            return cable::calculate(self, frequency);
        }
        self.validate(frequency)?;
        let n = self.wires.len();
        let omega = TWO_PI * frequency;
        let z = self.conductor_block(frequency);
        let mut potential = vec![vec![0.0; n]; n];
        for (i, wire) in self.wires.iter().enumerate() {
            potential[i][i] = crate::overhead::calc_potential_coefficient(
                2.0 * wire.height,
                wire.capacitance_radius,
            );
            for (j, other) in self.wires.iter().enumerate().take(i) {
                potential[i][j] = crate::overhead::calc_potential_coefficient(
                    (wire.x - other.x).hypot(wire.height + other.height),
                    (wire.x - other.x).hypot(wire.height - other.height),
                );
                potential[j][i] = potential[i][j];
            }
        }
        let inverse = crate::dense::invert_dense_matrix(
            &potential,
            crate::dense::DensePivot::Partial,
            0.0,
            0.0,
        )
        .ok_or("geometry potential matrix is singular")?;
        let keep = self.kept_conductors();
        // Native reduction retains the phase block of the inverted full
        // potential matrix; it does not Kron-reduce that admittance.
        let b = inverse
            .iter()
            .take(keep)
            .map(|row| {
                row.iter()
                    .take(keep)
                    .map(|v| v * TWO_PI * EPSILON_0 * omega)
                    .collect()
            })
            .collect();
        reduce_to_line_matrices(z, keep, b, "geometry")
    }
}

/// A conductor's internal impedance per meter: its AC resistance and the
/// solid round conductor's `ω μ₀ / 8π` under Carson's earth, or the Bessel
/// skin effect from its DC resistance under Deri's, as `LineConstants`
/// forms it. The one internal impedance of the overhead and cable models.
fn calc_internal_impedance(wire: &Wire, earth_model: EarthModel, frequency: f64) -> Complex {
    match earth_model {
        EarthModel::Carson | EarthModel::FullCarson => {
            Complex::new(wire.rac, TWO_PI * frequency * MU_0 / (8.0 * PI))
        }
        EarthModel::Deri => {
            let alpha = Complex::new(1.0, 1.0) * (frequency * MU_0 / wire.rdc).sqrt();
            Complex::new(1.0, 1.0) * bessel_ratio(alpha) * (wire.rdc * frequency * MU_0).sqrt()
                / 2.0
        }
    }
}

/// Kron reduce `z` to its first `keep` conductors, one at a time and the
/// last first as OpenDSS does, and return it with the shunt block `b`,
/// refusing a zero pivot or a nonfinite entry. `what` names the model in
/// the refusal.
fn reduce_to_line_matrices(
    mut z: Vec<Vec<Complex>>,
    keep: usize,
    b: Vec<Vec<f64>>,
    what: &str,
) -> Result<LineMatrices, String> {
    if keep == 0 || keep > z.len() {
        return Err(format!("invalid {what} reduction order"));
    }
    for k in (keep..z.len()).rev() {
        z = crate::kron::calc_kron_reduced_impedance(&z, &(0..k).collect::<Vec<_>>(), &[k], 0.0)
            .ok_or_else(|| format!("{what} reduction has a zero pivot"))?;
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
        b,
    };
    if [&result.r, &result.x, &result.b]
        .into_iter()
        .flatten()
        .flatten()
        .any(|v| !v.is_finite())
    {
        return Err(format!("{what} calculation became nonfinite"));
    }
    Ok(result)
}

/// Line constants for a reader whose source states its conductors and their
/// positions instead of a DSS geometry (a CIM wire catalog line), under
/// OpenDSS's default earth: Deri's model at 100 ohm-m. The first `keep`
/// conductors stay in the result; the rest are Kron reduced, as a DSS
/// geometry with `reduce=y` reduces its neutrals, and cable screens always
/// are. Cables lead and bare conductors trail, as the cable models require.
pub(crate) fn catalog_matrices(
    wires: Vec<Wire>,
    keep: usize,
    frequency: f64,
) -> Result<LineMatrices, String> {
    let n = wires.len();
    let cables = wires.iter().take_while(|w| w.cable.is_some()).count();
    if keep == 0 || keep > n {
        return Err("a wire catalog line needs at least one unreduced conductor".into());
    }
    Geometry {
        wires,
        phases: if cables > 0 { cables } else { n },
        reduce: false,
        reduction_order: Some(keep),
        earth_model: EarthModel::Deri,
        rho: 100.0,
    }
    .calculate(frequency)
}

#[cfg(test)]
mod tests;
