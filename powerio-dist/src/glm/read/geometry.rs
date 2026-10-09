//! The pinned overhead-line modified-Carson profile, in native units.
//! The output is the source's phase circuit, including its explicit neutral
//! reduction. It does not invent an independently solved neutral conductor.

use super::{METERS_PER_MILE, Reader, class, phase_name};
use crate::DistLineCode;
use crate::diagnostics::codes;
use crate::glm::ObjectKey;
use num_complex::Complex64;
use powerio_core::{Diagnostic, Error};

pub(super) fn spacing_property(name: &str) -> bool {
    matches!(
        name,
        "distance_AB"
            | "distance_AC"
            | "distance_BC"
            | "distance_AN"
            | "distance_BN"
            | "distance_CN"
            | "distance_AE"
            | "distance_BE"
            | "distance_CE"
            | "distance_NE"
    )
}

pub(super) fn allowed(class: &str, name: &str) -> bool {
    match class {
        "line_configuration" => matches!(
            name,
            "spacing" | "conductor_A" | "conductor_B" | "conductor_C" | "conductor_N"
        ),
        "line_spacing" => spacing_property(name),
        "overhead_line_conductor" => {
            matches!(name, "resistance" | "geometric_mean_radius" | "diameter")
        }
        _ => false,
    }
}

pub(super) fn unit(class: &str, name: &str) -> Option<&'static str> {
    if rating_property(class, name) {
        return Some("A");
    }
    match (class, name) {
        ("line_spacing", name) if spacing_property(name) => Some("ft"),
        ("overhead_line_conductor", "resistance") => Some("Ohm/mile"),
        ("overhead_line_conductor", "geometric_mean_radius") => Some("ft"),
        ("overhead_line_conductor", "diameter") => Some("in"),
        _ => None,
    }
}

impl Reader<'_> {
    pub(super) fn geometry_reference(
        &self,
        key: ObjectKey,
        name: &str,
        expected: &str,
    ) -> Result<ObjectKey, Error> {
        let target = self
            .index
            .resolve_property(key, name)?
            .ok_or_else(|| self.error(key, format!("geometry requires {name}")))?;
        if class(&self.index.objects()[target.ordinal()]) != expected {
            return Err(self.error(key, format!("{name} must reference {expected}")));
        }
        Ok(target)
    }

    pub(super) fn positive_geometry(&self, key: ObjectKey, name: &str) -> Result<f64, Error> {
        self.real(key, name).filter(|v| v.is_finite() && *v > 0.0)
            .ok_or_else(|| self.error(key, format!("geometry requires finite positive {name}; native missing/invalid-value fallbacks are not qualified")))
    }

    pub(super) fn geometry_line_code(
        &mut self,
        config: ObjectKey,
        phases: &[usize],
        neutral: bool,
        name: &str,
    ) -> Result<DistLineCode, Error> {
        let spacing = self.geometry_reference(config, "spacing", "line_spacing")?;
        let mut labels: Vec<_> = phases.iter().map(|&p| phase_name(p)).collect();
        if neutral {
            labels.push('N');
        }
        let mut conductors = Vec::new();
        for label in &labels {
            conductors.push(self.geometry_reference(
                config,
                &format!("conductor_{label}"),
                "overhead_line_conductor",
            )?);
        }
        let z = self.geometry_impedance(spacing, &labels, &conductors)?;
        let n = phases.len();
        let mut resistance = vec![vec![0.0; n]; n];
        let mut reactance = resistance.clone();
        for i in 0..n {
            for j in 0..n {
                let value = if neutral {
                    z[i][j] - z[i][n] * z[n][j] / z[n][n]
                } else {
                    z[i][j]
                } / METERS_PER_MILE;
                if !value.re.is_finite() || !value.im.is_finite() {
                    return Err(self.error(config, "geometry impedance reduction is nonfinite"));
                }
                resistance[i][j] = value.re;
                reactance[i][j] = value.im;
            }
        }
        let mut code = DistLineCode::new(name, resistance, reactance);
        if self.line_capacitance {
            let potential = self.geometry_potential(spacing, &labels, &conductors)?;
            let capacitance = inverse(&potential).ok_or_else(|| {
                self.error(config, "geometry potential matrix is singular or nonfinite")
            })?;
            let factor =
                2.0 * std::f64::consts::PI.powi(2) * self.net.base_frequency() * 0.014_240e-6
                    / METERS_PER_MILE;
            for (i, row) in capacitance.iter().take(n).enumerate() {
                for (j, &value) in row.iter().take(n).enumerate().skip(i) {
                    let b = value * factor;
                    if !b.is_finite() {
                        return Err(self.error(config, "geometry capacitance overflows"));
                    }
                    code.b_from[i][j] = b;
                    code.b_from[j][i] = b;
                }
            }
            // The image-potential inverse is mathematically symmetric.
            // Copy one triangle so arithmetic roundoff cannot create a
            // nonreciprocal shunt in canonical output.
            code.b_to.clone_from(&code.b_from);
        }
        let reduction = if neutral {
            "neutral reduced at zero neutral voltage"
        } else {
            "no neutral conductor"
        };
        code.source = Some(format!(
            "GridLAB-D modified Carson; earth resistivity 100 ohm-m; {reduction}"
        ));
        self.diagnostics.push(Diagnostic::of(&codes::READ_GLM_GEOMETRY_MATERIALIZED,
            format!("configuration {} becomes explicit phase matrices: {reduction}; conductor GMR/resistance, cross-section spacing and enabled image capacitance are materialized at {} Hz; source geometry is retained in the module, not inferred from map coordinates", self.names[&config], self.net.base_frequency())));
        Ok(code)
    }

    fn geometry_impedance(
        &mut self,
        spacing: ObjectKey,
        labels: &[char],
        conductors: &[ObjectKey],
    ) -> Result<Vec<Vec<Complex64>>, Error> {
        let f = self.net.base_frequency();
        let earth = (100.0 / f).ln() / 2.0 + 7.6786;
        let mut z = vec![vec![Complex64::default(); labels.len()]; labels.len()];
        for (i, &wire) in conductors.iter().enumerate() {
            let gmr = self.positive_geometry(wire, "geometric_mean_radius")?;
            let resistance = self.positive_geometry(wire, "resistance")?;
            for j in i..labels.len() {
                let distance = if i == j {
                    gmr
                } else {
                    let field = format!("distance_{}{}", labels[i], labels[j]);
                    let distance = self.real(spacing, &field).unwrap_or(0.0);
                    if distance < 0.0 {
                        return Err(
                            self.error(spacing, "negative conductor spacing is not qualified")
                        );
                    }
                    if distance == 0.0 {
                        self.diagnostics.push(Diagnostic::of(&codes::READ_GLM_GEOMETRY_MATERIALIZED,
                            format!("{}: missing/zero {field} gives zero native mutual impedance; no geometric distance is inferred", self.names[&spacing])));
                        continue;
                    }
                    distance
                };
                // Native log(1/d) must also be representable: algebraically
                // replacing it with -log(d) must not repair native overflow.
                if !(1.0 / distance).is_finite() || 1.0 / distance == 0.0 {
                    return Err(self.error(
                        spacing,
                        "native geometry reciprocal overflows or underflows",
                    ));
                }
                let value = Complex64::new(
                    if i == j { resistance } else { 0.0 } + 0.001_588_36 * f,
                    0.002_022_37 * f * (earth - distance.ln()),
                );
                if !value.re.is_finite() || !value.im.is_finite() {
                    return Err(self.error(spacing, "geometry primitive impedance overflows"));
                }
                z[i][j] = value;
                z[j][i] = value;
            }
        }
        Ok(z)
    }

    fn geometry_potential(
        &self,
        spacing: ObjectKey,
        labels: &[char],
        conductors: &[ObjectKey],
    ) -> Result<Vec<Vec<f64>>, Error> {
        let heights = labels
            .iter()
            .map(|p| self.positive_geometry(spacing, &format!("distance_{p}E")))
            .collect::<Result<Vec<_>, _>>()?;
        let mut potential = vec![vec![0.0; labels.len()]; labels.len()];
        for (i, &wire) in conductors.iter().enumerate() {
            let radius = self.positive_geometry(wire, "diameter")? / 24.0;
            if heights[i] <= radius {
                return Err(self.error(wire, "conductor intersects its earth image"));
            }
            potential[i][i] = (2.0 * heights[i] / radius).ln();
            for j in i + 1..labels.len() {
                let distance = self
                    .positive_geometry(spacing, &format!("distance_{}{}", labels[i], labels[j]))?;
                let image = distance.hypot(2.0 * (heights[i] * heights[j]).sqrt());
                potential[i][j] = (image / distance).ln();
                potential[j][i] = potential[i][j];
            }
        }
        Ok(potential)
    }
}

// At most four conductors. Partial pivoting avoids phase-order-dependent zero
// pivots; singular/nonfinite inputs fail instead of fabricating capacitance.
fn inverse(matrix: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = matrix.len();
    let mut a = matrix.to_vec();
    let mut result = vec![vec![0.0; n]; n];
    for (i, row) in result.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    for i in 0..n {
        let pivot = (i..n).max_by(|&j, &k| a[j][i].abs().total_cmp(&a[k][i].abs()))?;
        a.swap(i, pivot);
        result.swap(i, pivot);
        let divisor = a[i][i];
        if divisor == 0.0 || !divisor.is_finite() {
            return None;
        }
        for j in 0..n {
            a[i][j] /= divisor;
            result[i][j] /= divisor;
        }
        for j in 0..n {
            if i == j {
                continue;
            }
            let factor = a[j][i];
            for k in 0..n {
                a[j][k] -= factor * a[i][k];
                result[j][k] -= factor * result[i][k];
            }
        }
    }
    result
        .iter()
        .flatten()
        .all(|v| v.is_finite())
        .then_some(result)
}

pub(super) fn rating_property(class: &str, name: &str) -> bool {
    matches!(
        class,
        "overhead_line_conductor"
            | "underground_line_conductor"
            | "triplex_line_conductor"
            | "line_configuration"
            | "triplex_line_configuration"
    ) && matches!(
        name,
        "rating.summer.continuous"
            | "rating.summer.emergency"
            | "rating.winter.continuous"
            | "rating.winter.emergency"
    )
}

impl Reader<'_> {
    pub(super) fn rating_metadata(&self, key: ObjectKey) -> Result<Option<Diagnostic>, Error> {
        let object = &self.index.objects()[key.ordinal()];
        let fields: Vec<_> = object
            .properties()
            .iter()
            .filter(|p| rating_property(class(object), p.name))
            .map(|p| p.name)
            .collect();
        if fields.is_empty() {
            return Ok(None);
        }
        if fields
            .iter()
            .any(|f| self.real(key, f).is_none_or(|v| v <= 0.0))
        {
            return Err(self.error(key, "seasonal ampacity must be positive"));
        }
        Ok(Some(Diagnostic::of(
            &codes::READ_GLM_SOURCE_ONLY,
            format!(
                "{}: seasonal conductor/configuration ampacities remain in source; the static power-flow circuit is preserved but typed ampacity projection is unfinished",
                self.names[&key]
            ),
        )))
    }
}
