//! Native underground cable primitives and their zero-voltage return reduction.
//!
//! Constants and units follow the pinned PNNL/Arras underground_line profiles.
//! The selected profile determines which return paths participate in reduction;
//! the resulting phase matrices do not add independently solved neutral terminals.

use super::{METERS_PER_MILE, ObjectKey, Reader, phase_name};
use crate::DistLineCode;
use crate::diagnostics::codes;
use crate::glm::GlmDialect;
use crate::shielded_cable::{
    calc_cable_capacitance, calc_concentric_neutral_gmr, calc_concentric_neutral_radius,
    calc_tape_shield_radius,
};
use num_complex::Complex64;
use powerio_core::{Diagnostic, Error};

pub(super) fn unit(class: &str, name: &str) -> Option<&'static str> {
    if class != "underground_line_conductor" {
        return None;
    }
    Some(match name {
        "outer_diameter" | "conductor_diameter" | "neutral_diameter" | "shield_diameter"
        | "shield_thickness" => "in",
        "conductor_gmr" | "neutral_gmr" | "shield_gmr" => "ft",
        "conductor_resistance" | "neutral_resistance" | "shield_resistance" => "Ohm/mile",
        "neutral_strands" | "insulation_relative_permitivitty" => "unit",
        _ => return None,
    })
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Concentric,
    Shield,
    Bare,
}

// Phase label, GMR, resistance, and optional own-ring separation.
type PrimitiveWire = (char, f64, f64, Option<f64>);

struct Cable {
    wire: ObjectKey,
    label: char,
    kind: Kind,
    // Equivalent return GMR, resistance and phase-to-return separation.
    return_path: Option<(f64, f64, f64)>,
    capacitance: f64, // farads per mile, whole pi line
}

impl Reader<'_> {
    fn cable(&self, wire: ObjectKey, label: char) -> Result<Cable, Error> {
        let positive = |name| self.positive_geometry(wire, name);
        let outer = self.real(wire, "outer_diameter").unwrap_or(0.0);
        if ["conductor_diameter", "neutral_diameter"]
            .iter()
            .any(|name| self.real(wire, name).unwrap_or(0.0) > outer)
        {
            return Err(self.error(
                wire,
                "cable outer diameter is smaller than a conductor or strand diameter",
            ));
        }
        let neutral_gmr = self.real(wire, "neutral_gmr").unwrap_or(0.0);
        let shield_gmr = self.real(wire, "shield_gmr").unwrap_or(0.0);
        if neutral_gmr < 0.0
            || shield_gmr < 0.0
            || (self.project.dialect() == GlmDialect::Pnnl5_3
                && neutral_gmr > 0.0
                && shield_gmr > 0.0)
        {
            return Err(self.error(
                wire,
                "cable must select one valid concentric-neutral or tape-shield return",
            ));
        }
        let kind = if shield_gmr > 0.0 {
            Kind::Shield
        } else if neutral_gmr > 0.0 {
            Kind::Concentric
        } else if self.project.dialect() == GlmDialect::Arras4_3 {
            Kind::Bare
        } else {
            return Err(self.error(
                wire,
                "PNNL requires a concentric neutral or tape shield on each phase cable",
            ));
        };
        // The screen's radius and, for a concentric neutral, its strands,
        // in feet: the capacitance's outer radius and Kersting's strand term.
        let (return_path, screen) = match kind {
            Kind::Concentric => {
                let strands = positive("neutral_strands")?;
                if strands.fract() != 0.0 || strands > f64::from(i16::MAX) {
                    return Err(
                        self.error(wire, "neutral_strands must be a positive 16-bit integer")
                    );
                }
                let diameter = positive("neutral_diameter")?;
                let radius =
                    calc_concentric_neutral_radius(positive("outer_diameter")?, diameter) / 12.0;
                if radius <= 0.0 {
                    return Err(self.error(wire, "concentric-neutral ring radius must be positive"));
                }
                let gmr = calc_concentric_neutral_gmr(neutral_gmr, strands, radius);
                let resistance = positive("neutral_resistance")? / strands;
                (
                    Some((gmr, resistance, radius)),
                    Some((radius, Some((strands, diameter / 24.0)))),
                )
            }
            Kind::Shield => {
                let diameter = positive("shield_diameter")?;
                let thickness = positive("shield_thickness")?;
                if diameter <= thickness {
                    return Err(self.error(wire, "tape-shield diameter must exceed its thickness"));
                }
                // GridLAB-D states the shield's GMR; its capacitance takes
                // the tape's mean radius.
                (
                    Some((shield_gmr, positive("shield_resistance")?, shield_gmr)),
                    Some((calc_tape_shield_radius(diameter, thickness) / 12.0, None)),
                )
            }
            Kind::Bare => (None, None),
        };
        let capacitance = match screen {
            Some((outer, strands)) if self.line_capacitance => {
                let permittivity = self
                    .real(wire, "insulation_relative_permitivitty")
                    .unwrap_or(1.0);
                // GridLAB-D's rounded permittivity of free space, F/mile.
                let capacitance = calc_cable_capacitance(
                    2.0 * std::f64::consts::PI * 0.01420e-6 * permittivity,
                    outer,
                    positive("conductor_diameter")? / 24.0,
                    strands,
                );
                if permittivity <= 0.0 || !capacitance.is_finite() || capacitance <= 0.0 {
                    return Err(self.error(wire, "cable capacitance requires positive finite permittivity and geometric denominator"));
                }
                capacitance
            }
            _ => 0.0,
        };
        Ok(Cable {
            wire,
            label,
            kind,
            return_path,
            capacitance,
        })
    }

    fn cable_neutral(
        &self,
        config: ObjectKey,
        cables: &[Cable],
        neutral: bool,
    ) -> Result<Option<PrimitiveWire>, Error> {
        let arras = self.project.dialect() == GlmDialect::Arras4_3;
        let kind = cables[0].kind;
        if !neutral {
            if arras && kind == Kind::Shield {
                return Err(self.error(config, "Arras tape-shield reduction without N is singular"));
            }
            return Ok(None);
        }
        let wire = self.geometry_reference(config, "conductor_N", "underground_line_conductor")?;
        let gmr = self.positive_geometry(wire, "conductor_gmr")?;
        let resistance = self.positive_geometry(wire, "conductor_resistance")?;
        if !arras {
            return Ok(Some(('N', gmr, resistance, None)));
        }
        // Arras CN uses only its three ring rows. Its bare cable path also
        // ignores N: the neutral_gmr of the first phase is zero by definition.
        if kind != Kind::Shield {
            return Ok(None);
        }
        // The pinned Arras TS path takes N's self term from the first phase's
        // neutral fields, despite taking mutual spacings from the N geometry.
        let first = cables[0].wire;
        Ok(Some((
            'N',
            self.real(first, "neutral_gmr").unwrap_or(0.0),
            self.real(first, "neutral_resistance").unwrap_or(0.0),
            None,
        )))
    }

    pub(super) fn cable_line_code(
        &mut self,
        config: ObjectKey,
        phases: &[usize],
        neutral: bool,
        name: &str,
    ) -> Result<DistLineCode, Error> {
        let spacing = self.geometry_reference(config, "spacing", "line_spacing")?;
        let mut cables = Vec::new();
        for &phase in phases {
            let label = phase_name(phase);
            let wire = self.geometry_reference(
                config,
                &format!("conductor_{label}"),
                "underground_line_conductor",
            )?;
            cables.push(self.cable(wire, label)?);
        }
        if cables.iter().any(|c| c.kind != cables[0].kind) {
            return Err(self.error(
                config,
                "mixed underground cable return types are not a native qualified circuit",
            ));
        }
        // Each primitive carries its owning phase label. Native mutual
        // distances between a cable and another cable's return equal their
        // centre spacing; its own concentric return uses the ring radius.
        let mut wires: Vec<_> = cables
            .iter()
            .map(|c| {
                Ok((
                    c.label,
                    self.positive_geometry(c.wire, "conductor_gmr")?,
                    self.positive_geometry(c.wire, "conductor_resistance")?,
                    None,
                ))
            })
            .collect::<Result<_, Error>>()?;
        for c in &cables {
            if let Some((gmr, resistance, distance)) = c.return_path {
                wires.push((c.label, gmr, resistance, Some(distance)));
            }
        }
        if let Some(wire) = self.cable_neutral(config, &cables, neutral)? {
            wires.push(wire);
        }
        let frequency = self.net.base_frequency();
        let earth = crate::glm::line_physics::carson_earth(frequency);
        let mut z = vec![vec![Complex64::default(); wires.len()]; wires.len()];
        for (i, &(a, gmr, resistance, _)) in wires.iter().enumerate() {
            for (j, &(b, _, _, ring)) in wires.iter().enumerate().skip(i) {
                if i == j && gmr == 0.0 {
                    continue;
                }
                let distance = if i == j {
                    gmr
                } else if a == b {
                    ring.ok_or_else(|| self.error(config, "cable return has no separation"))?
                } else {
                    let (a, b) = if a < b { (a, b) } else { (b, a) };
                    self.positive_geometry(spacing, &format!("distance_{a}{b}"))?
                };
                if !distance.is_finite() || distance <= 0.0 || !(1.0 / distance).is_finite() {
                    return Err(self.error(config, "cable primitive distance or GMR is invalid"));
                }
                let value = crate::overhead::calc_carson_impedance(distance, earth)
                    + if i == j { resistance } else { 0.0 };
                z[i][j] = value;
                z[j][i] = value;
            }
        }
        let phase_count = phases.len();
        // Eliminates the zero-voltage returns (rings, shields and N) one at a
        // time, which keeps the matrix exactly symmetric. Each step takes the
        // remaining return of largest self impedance, so Arras TS's zero N
        // diagonal is eliminated last, after the shields have filled it.
        while z.len() > phase_count {
            let pivot = (phase_count..z.len())
                .max_by(|&a, &b| z[a][a].norm().total_cmp(&z[b][b].norm()))
                .expect("a return remains");
            let keep: Vec<usize> = (0..z.len()).filter(|&i| i != pivot).collect();
            z = crate::kron::calc_kron_reduced_impedance(&z, &keep, &[pivot], 0.0).ok_or_else(
                || self.error(config, "cable return reduction is singular or nonfinite"),
            )?;
        }
        let mut resistance = vec![vec![0.0; phase_count]; phase_count];
        let mut reactance = resistance.clone();
        for i in 0..phase_count {
            for j in 0..phase_count {
                let value = z[i][j] / METERS_PER_MILE;
                if !value.re.is_finite() || !value.im.is_finite() {
                    return Err(self.error(config, "cable impedance reduction overflows"));
                }
                resistance[i][j] = value.re;
                reactance[i][j] = value.im;
            }
        }
        let mut code = DistLineCode::new(name, resistance, reactance);
        for (i, cable) in cables.iter().enumerate() {
            let susceptance =
                std::f64::consts::PI * frequency * cable.capacitance / METERS_PER_MILE;
            if !susceptance.is_finite() {
                return Err(self.error(config, "cable capacitance overflows"));
            }
            code.b_from[i][i] = susceptance;
            code.b_to[i][i] = susceptance;
        }
        code.source = Some(format!(
            "GridLAB-D {:?} underground modified Carson; native cable return reduction; earth resistivity 100 ohm-m",
            self.project.dialect()
        ));
        self.diagnostics.push(Diagnostic::of(&codes::READ_GLM_GEOMETRY_MATERIALIZED,
            format!("configuration {} becomes explicit underground phase matrices for {:?}; PNNL includes separate N, Arras CN/bare omits N and Arras shield uses first-phase neutral self parameters", self.names[&config], self.project.dialect())));
        Ok(code)
    }
}
