//! Concentric-neutral cable constants, matching the pinned DSS CNLineConstants.
use super::{EPSILON_0, EarthModel, Geometry, MU_0, TWO_PI, bessel_ratio, wire::meters};
use crate::dss::linecode::LineMatrices;
use num_complex::Complex64 as Complex;
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

/// Native definition values; `si` converts the lengths and resistance once.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub(in crate::dss) struct Data {
    pub k: usize,
    pub diastrand: f64,
    pub gmrstrand: f64,
    pub rstrand: f64,
    pub epsr: f64,
    pub inslayer: f64,
    pub diains: f64,
    pub diacable: f64,
}

impl Default for Data {
    fn default() -> Self {
        Self {
            k: 2,
            diastrand: -1.0,
            gmrstrand: -1.0,
            rstrand: -1.0,
            epsr: 2.3,
            inslayer: -1.0,
            diains: -1.0,
            diacable: -1.0,
        }
    }
}

impl Data {
    pub fn apply(&mut self, key: &str, value: f64) -> Result<(), String> {
        match key {
            "k" => {
                if value.fract() != 0.0 || !(2.0..=f64::from(i32::MAX)).contains(&value) {
                    return Err("CNData needs at least two neutral strands".into());
                }
                self.k = value as usize;
            }
            "diastrand" => {
                self.diastrand = value;
                // This default remains in GMRUnits, even when RadiusUnits differ.
                if self.gmrstrand <= 0.0 {
                    self.gmrstrand = 0.7788 * 0.5 * value;
                }
            }
            "gmrstrand" => self.gmrstrand = value,
            "rstrand" => self.rstrand = value,
            "epsr" => self.epsr = value,
            "inslayer" => self.inslayer = value,
            "diains" => self.diains = value,
            "diacable" => self.diacable = value,
            _ => return Err(format!("unqualified CNData property {key}")),
        }
        if value <= 0.0 || (key == "epsr" && value < 1.0) {
            return Err(format!("invalid CNData {key}"));
        }
        Ok(())
    }

    pub fn si(mut self, radius_units: usize, gmr_units: usize, resistance_units: usize) -> Self {
        let length = meters(radius_units);
        self.diastrand *= length;
        self.inslayer *= length;
        self.diains *= length;
        self.diacable *= length;
        self.gmrstrand *= meters(gmr_units);
        self.rstrand /= meters(resistance_units);
        self
    }

    fn validate(self) -> Result<(), String> {
        if self.k < 2
            || self.epsr < 1.0
            || [
                self.diastrand,
                self.gmrstrand,
                self.rstrand,
                self.epsr,
                self.inslayer,
                self.diains,
                self.diacable,
            ]
            .iter()
            .any(|v| !v.is_finite() || *v <= 0.0)
            || self.diains <= 2.0 * self.inslayer
            || self.diacable <= self.diastrand
        {
            return Err("invalid or incomplete concentric-neutral cable data".into());
        }
        Ok(())
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
        return Err(
            "invalid concentric-neutral geometry dimensions, frequency or resistivity".into(),
        );
    }
    for (i, wire) in input.wires.iter().enumerate() {
        if !wire.x.is_finite()
            || !wire.height.is_finite()
            || [wire.radius, wire.gmr, wire.rdc, wire.rac]
                .iter()
                .any(|v| !v.is_finite() || *v <= 0.0)
        {
            return Err("invalid concentric-neutral geometry conductor data".into());
        }
        // The native CN model adds one neutral ring for every phase position.
        // Bare conductors are supported only in the trailing neutral positions.
        if (i < input.phases) != wire.cable.is_some() {
            return Err(
                "CN geometry requires cables in all phase positions and bare trailing neutrals"
                    .into(),
            );
        }
        if let Some(cable) = wire.cable {
            cable.validate()?;
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
    let factor = Complex::new(0.0, TWO_PI * frequency * MU_0 / TWO_PI);
    let mut z = vec![vec![Complex::default(); order]; order];
    for (i, wire) in input.wires.iter().enumerate() {
        let mut zi = internal(input, i, frequency);
        let radius = if frequency > 40.0 && frequency < 1000.0 {
            zi.im = 0.0;
            wire.gmr
        } else {
            wire.radius
        };
        z[i][i] = zi + factor * (1.0 / radius).ln() + input.earth(i, i, frequency);
        for (j, other) in input.wires.iter().enumerate().take(i) {
            let distance = (wire.x - other.x).hypot(wire.height - other.height);
            z[i][j] = factor * (1.0 / distance).ln() + input.earth(i, j, frequency);
            z[j][i] = z[i][j];
        }
    }
    for i in 0..phases {
        let wire = &input.wires[i];
        let cable = wire.cable.unwrap();
        let k = cable.k as f64;
        let radius = 0.5 * (cable.diacable - cable.diastrand);
        let gmr = (cable.gmrstrand * k * radius.powf(k - 1.0)).powf(1.0 / k);
        let index = n + i;
        z[index][index] =
            cable.rstrand / k + factor * (1.0 / gmr).ln() + input.earth(i, i, frequency);
        for (j, other) in input.wires.iter().enumerate().take(i) {
            let distance = (wire.x - other.x).hypot(wire.height - other.height);
            z[index][n + j] = factor * (1.0 / distance).ln() + input.earth(i, j, frequency);
            z[n + j][index] = z[index][n + j];
        }
        for (j, other) in input.wires.iter().enumerate() {
            let distance = if i == j {
                radius
            } else {
                let d = (wire.x - other.x).hypot(wire.height - other.height);
                (d.powf(k) - radius.powf(k)).powf(1.0 / k)
            };
            z[index][j] = factor * (1.0 / distance).ln() + input.earth(i, j, frequency);
            z[j][index] = z[index][j];
        }
    }
    let keep = input
        .reduction_order
        .unwrap_or(if input.reduce { phases } else { n });
    if keep == 0 || keep > n {
        return Err("invalid cable reduction order".into());
    }
    // Concentric neutrals are always eliminated, before optional bare neutrals.
    for k in (keep..order).rev() {
        if z[k][k] == Complex::default() {
            return Err("cable reduction has a zero pivot".into());
        }
        for i in 0..k {
            for j in 0..k {
                let correction = z[i][k] * z[k][j] / z[k][k];
                z[i][j] -= correction;
            }
        }
    }
    let mut b = vec![vec![0.0; keep]; keep];
    for (i, row) in b.iter_mut().enumerate().take(phases) {
        let cable = input.wires[i].cable.unwrap();
        let outer = 0.5 * cable.diains;
        let inner = outer - cable.inslayer;
        row[i] = TWO_PI * EPSILON_0 * cable.epsr * (TWO_PI * frequency) / (outer / inner).ln();
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
