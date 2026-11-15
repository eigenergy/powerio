//! Shielded cable constants: concentric-neutral cables matching the pinned DSS
//! CNLineConstants and tape-shielded cables matching TSLineConstants.
//!
//! Both build the same impedance matrix: phase cores and bare conductors
//! first, then one screen (neutral ring or tape shield) per phase cable, which
//! Kron reduction always eliminates. Only the screen's resistance, its own
//! GMR and its distance to the cores differ between the two.
use super::{EPSILON_0, EarthModel, Geometry, MU_0, TWO_PI, bessel_ratio, wire::meters};
use crate::dss::linecode::LineMatrices;
use crate::shielded_cable::{
    calc_cable_capacitance, calc_concentric_neutral_distance, calc_concentric_neutral_gmr,
    calc_concentric_neutral_radius, calc_tape_shield_radius,
};
use num_complex::Complex64 as Complex;
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

/// The screen around a phase core, in native units until `si`.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Screen {
    /// `k` round neutral strands on a circle (CNData).
    ConcentricNeutral {
        k: usize,
        diastrand: f64,
        gmrstrand: f64,
        rstrand: f64,
    },
    /// A helically lapped copper tape (TSData): outer diameter over the
    /// tape, tape thickness, and lap in percent.
    TapeShield {
        diashield: f64,
        tapelayer: f64,
        tapelap: f64,
    },
}

/// Native cable definition values; `si` converts the lengths and resistance once.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub(crate) struct Data {
    pub screen: Screen,
    pub epsr: f64,
    pub inslayer: f64,
    pub diains: f64,
    /// The diameter over the neutral strands (CNData) or over the jacket
    /// (TSData, which the tape model does not read); `None` when unstated.
    pub diacable: Option<f64>,
}

/// Copper resistivity, ohm-m, and 1/pi, as TSLineConstants writes them.
const TAPE_RESISTIVITY: f64 = 2.3718e-8;
#[expect(
    clippy::approx_constant,
    reason = "match the pinned OpenDSS engine constant"
)]
const TAPE_ONE_OVER_PI: f64 = 0.3183;

impl Data {
    fn cable(screen: Screen) -> Self {
        Self {
            screen,
            epsr: 2.3,
            inslayer: -1.0,
            diains: -1.0,
            diacable: None,
        }
    }

    /// CNData defaults.
    pub fn concentric_neutral() -> Self {
        Self::cable(Screen::ConcentricNeutral {
            k: 2,
            diastrand: -1.0,
            gmrstrand: -1.0,
            rstrand: -1.0,
        })
    }

    /// TSData defaults: a 20 percent lap.
    pub fn tape_shield() -> Self {
        Self::cable(Screen::TapeShield {
            diashield: -1.0,
            tapelayer: -1.0,
            tapelap: 20.0,
        })
    }

    fn class(&self) -> &'static str {
        match self.screen {
            Screen::ConcentricNeutral { .. } => "CNData",
            Screen::TapeShield { .. } => "TSData",
        }
    }

    pub fn apply(&mut self, key: &str, value: f64) -> Result<(), String> {
        let class = self.class();
        match (&mut self.screen, key) {
            (Screen::ConcentricNeutral { k, .. }, "k") => {
                if value.fract() != 0.0 || !(2.0..=f64::from(i32::MAX)).contains(&value) {
                    return Err("CNData needs at least two neutral strands".into());
                }
                *k = value as usize;
            }
            (
                Screen::ConcentricNeutral {
                    diastrand,
                    gmrstrand,
                    ..
                },
                "diastrand",
            ) => {
                *diastrand = value;
                // This default remains in GMRUnits, even when RadiusUnits differ.
                if *gmrstrand <= 0.0 {
                    *gmrstrand = 0.7788 * 0.5 * value;
                }
            }
            (Screen::ConcentricNeutral { gmrstrand, .. }, "gmrstrand") => *gmrstrand = value,
            (Screen::ConcentricNeutral { rstrand, .. }, "rstrand") => *rstrand = value,
            (Screen::TapeShield { diashield, .. }, "diashield") => *diashield = value,
            (Screen::TapeShield { tapelayer, .. }, "tapelayer") => *tapelayer = value,
            (Screen::TapeShield { tapelap, .. }, "tapelap") => {
                if !(0.0..100.0).contains(&value) {
                    return Err("TSData tape lap must be a percentage below 100".into());
                }
                *tapelap = value;
            }
            (_, "epsr") => self.epsr = value,
            (_, "inslayer") => self.inslayer = value,
            (_, "diains") => self.diains = value,
            (_, "diacable") => self.diacable = Some(value),
            _ => return Err(format!("unqualified {class} property {key}")),
        }
        if (value <= 0.0 && key != "tapelap") || (key == "epsr" && value < 1.0) {
            return Err(format!("invalid {class} {key}"));
        }
        Ok(())
    }

    pub fn si(mut self, radius_units: usize, gmr_units: usize, resistance_units: usize) -> Self {
        let length = meters(radius_units);
        self.inslayer *= length;
        self.diains *= length;
        self.diacable = self.diacable.map(|d| d * length);
        match &mut self.screen {
            Screen::ConcentricNeutral {
                diastrand,
                gmrstrand,
                rstrand,
                ..
            } => {
                *diastrand *= length;
                *gmrstrand *= meters(gmr_units);
                *rstrand /= meters(resistance_units);
            }
            Screen::TapeShield {
                diashield,
                tapelayer,
                ..
            } => {
                *diashield *= length;
                *tapelayer *= length;
            }
        }
        self
    }

    fn validate(self) -> Result<(), String> {
        let insulation = self.epsr >= 1.0
            && [self.epsr, self.inslayer, self.diains]
                .iter()
                .all(|v| v.is_finite() && *v > 0.0)
            && self.diains > 2.0 * self.inslayer;
        let screen = match self.screen {
            Screen::ConcentricNeutral {
                k,
                diastrand,
                gmrstrand,
                rstrand,
            } => {
                k >= 2
                    && [diastrand, gmrstrand, rstrand]
                        .iter()
                        .all(|v| v.is_finite() && *v > 0.0)
                    && self
                        .diacable
                        .is_some_and(|d| d.is_finite() && d > diastrand)
            }
            Screen::TapeShield {
                diashield,
                tapelayer,
                tapelap,
            } => {
                [diashield, tapelayer]
                    .iter()
                    .all(|v| v.is_finite() && *v > 0.0)
                    && (0.0..100.0).contains(&tapelap)
                    && diashield > tapelayer
            }
        };
        if !(insulation && screen) {
            return Err(match self.screen {
                Screen::ConcentricNeutral { .. } => {
                    "invalid or incomplete concentric-neutral cable data".into()
                }
                Screen::TapeShield { .. } => "invalid or incomplete tape-shield cable data".into(),
            });
        }
        Ok(())
    }

    /// The screen's resistance, ohm/m.
    fn screen_resistance(&self) -> f64 {
        match self.screen {
            Screen::ConcentricNeutral { k, rstrand, .. } => rstrand / k as f64,
            Screen::TapeShield {
                diashield,
                tapelayer,
                tapelap,
            } => {
                TAPE_ONE_OVER_PI * TAPE_RESISTIVITY
                    / (diashield * tapelayer * (50.0 / (100.0 - tapelap)).sqrt())
            }
        }
    }

    /// The concentric neutral's ring radius. `validate` requires the
    /// diameter over the neutral; were it missing, the NaN would reach the
    /// calculation's nonfinite refusal, never a number.
    fn neutral_ring_radius(&self, diastrand: f64) -> f64 {
        self.diacable.map_or(f64::NAN, |diacable| {
            calc_concentric_neutral_radius(diacable, diastrand)
        })
    }

    /// The screen's own GMR.
    fn screen_gmr(&self) -> f64 {
        match self.screen {
            Screen::ConcentricNeutral {
                k,
                diastrand,
                gmrstrand,
                ..
            } => calc_concentric_neutral_gmr(
                gmrstrand,
                k as f64,
                self.neutral_ring_radius(diastrand),
            ),
            Screen::TapeShield {
                diashield,
                tapelayer,
                ..
            } => calc_tape_shield_radius(diashield, tapelayer),
        }
    }

    /// The screen's distance to a conductor `distance` from its core
    /// (`None`: its own core).
    fn screen_distance(&self, distance: Option<f64>) -> f64 {
        match self.screen {
            Screen::ConcentricNeutral { k, diastrand, .. } => {
                let radius = self.neutral_ring_radius(diastrand);
                distance.map_or(radius, |d| {
                    calc_concentric_neutral_distance(d, k as f64, radius)
                })
            }
            Screen::TapeShield { .. } => distance.unwrap_or_else(|| self.screen_gmr()),
        }
    }
}

fn validate(input: &Geometry, frequency: f64) -> Result<(), String> {
    let n = input.wires.len();
    if n == 0
        || n > 32
        || input.phases == 0
        || input.phases > n
        || !frequency.is_finite()
        || frequency <= 0.0
        || !input.rho.is_finite()
        || input.rho <= 0.0
    {
        return Err("invalid cable geometry dimensions, frequency or resistivity".into());
    }
    for (i, wire) in input.wires.iter().enumerate() {
        if !wire.x.is_finite()
            || !wire.height.is_finite()
            || [wire.radius, wire.gmr, wire.rdc, wire.rac]
                .iter()
                .any(|v| !v.is_finite() || *v <= 0.0)
        {
            return Err("invalid cable geometry conductor data".into());
        }
        // The native cable models add one screen for every phase position.
        // Bare conductors are supported only in the trailing neutral positions.
        if (i < input.phases) != wire.cable.is_some() {
            return Err(
                "cable geometry requires cables in all phase positions and bare trailing neutrals"
                    .into(),
            );
        }
        if let Some(cable) = wire.cable {
            cable.validate()?;
            // One line constants object serves the geometry: CN or TS, not both.
            if let Some(first) = input.wires[0].cable
                && std::mem::discriminant(&first.screen) != std::mem::discriminant(&cable.screen)
            {
                return Err(
                    "a geometry mixing concentric-neutral and tape-shield cables is unqualified"
                        .into(),
                );
            }
        }
        for (j, other) in input.wires[..i].iter().enumerate() {
            let radius = if i < input.phases { wire.radius } else { 0.0 };
            let other_radius = if j < input.phases { other.radius } else { 0.0 };
            if (wire.x - other.x).hypot(wire.height - other.height) < radius + other_radius {
                return Err("cable conductors occupy the same space".into());
            }
        }
    }
    Ok(())
}

fn internal(input: &Geometry, i: usize, frequency: f64) -> Complex {
    let wire = &input.wires[i];
    match input.earth_model {
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

pub(super) fn calculate(input: &Geometry, frequency: f64) -> Result<LineMatrices, String> {
    validate(input, frequency)?;
    let n = input.wires.len();
    let phases = input.phases;
    let order = n + phases;
    let mut z = vec![vec![Complex::default(); order]; order];
    for (i, wire) in input.wires.iter().enumerate() {
        let mut zi = internal(input, i, frequency);
        let radius = if frequency > 40.0 && frequency < 1000.0 {
            zi.im = 0.0;
            wire.gmr
        } else {
            wire.radius
        };
        z[i][i] = zi + input.conductor_loop(i, i, radius, frequency);
        for (j, other) in input.wires.iter().enumerate().take(i) {
            let distance = (wire.x - other.x).hypot(wire.height - other.height);
            z[i][j] = input.conductor_loop(i, j, distance, frequency);
            z[j][i] = z[i][j];
        }
    }
    for i in 0..phases {
        let wire = &input.wires[i];
        let cable = wire.cable.unwrap();
        let index = n + i;
        z[index][index] =
            cable.screen_resistance() + input.conductor_loop(i, i, cable.screen_gmr(), frequency);
        for (j, other) in input.wires.iter().enumerate().take(i) {
            let distance = (wire.x - other.x).hypot(wire.height - other.height);
            z[index][n + j] = input.conductor_loop(i, j, distance, frequency);
            z[n + j][index] = z[index][n + j];
        }
        for (j, other) in input.wires.iter().enumerate() {
            let distance = cable.screen_distance(
                (i != j).then(|| (wire.x - other.x).hypot(wire.height - other.height)),
            );
            z[index][j] = input.conductor_loop(i, j, distance, frequency);
            z[j][index] = z[index][j];
        }
    }
    let keep = input
        .reduction_order
        .unwrap_or(if input.reduce { phases } else { n });
    if keep == 0 || keep > n {
        return Err("invalid cable reduction order".into());
    }
    // Cable screens are always eliminated, before optional bare neutrals,
    // one conductor at a time and the last first, as OpenDSS does.
    for k in (keep..order).rev() {
        z = crate::kron::calc_kron_reduced_impedance(&z, &(0..k).collect::<Vec<_>>(), &[k], 0.0)
            .ok_or("cable reduction has a zero pivot")?;
    }
    let mut b = vec![vec![0.0; keep]; keep];
    for (i, row) in b.iter_mut().enumerate().take(phases) {
        let cable = input.wires[i].cable.unwrap();
        let outer = 0.5 * cable.diains;
        let inner = outer - cable.inslayer;
        // OpenDSS takes the coaxial capacitance over the insulation for both
        // cable kinds, without Kersting's strand term.
        row[i] = TWO_PI
            * frequency
            * calc_cable_capacitance(TWO_PI * EPSILON_0 * cable.epsr, outer, inner, None);
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
        return Err("cable calculation became nonfinite".into());
    }
    Ok(result)
}

#[cfg(test)]
mod tests;
