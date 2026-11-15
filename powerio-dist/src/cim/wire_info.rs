//! Wire catalog line impedance: a segment's `WireSpacingInfo`, its
//! `WirePosition`s and the `WireInfo` classes its phases name, mapped onto the
//! one DSS line constants kernel ([`crate::dss::geometry::catalog_matrices`]).
//!
//! The CIM and MG-RAVENS readers both call [`segment_impedance`]; neither
//! carries line constants of its own. CIM states every quantity in SI units:
//! coordinates, radii, GMRs and diameters in meters, resistances in ohm/m.
//! CIM states no earth model, so the kernel's OpenDSS defaults apply (Deri's
//! model, 100 ohm-m), which is how OpenDSS computes the same conductors.

use crate::dss::geometry::{CableData, Screen, Wire, catalog_matrices};
use crate::model::ConductorMatrix;
use std::collections::BTreeMap;

/// The wire catalog classes [`segment_impedance`] reads.
pub(crate) const CATALOG_CLASSES: &[&str] = &[
    "WireInfo",
    "OverheadWireInfo",
    "ConcentricNeutralCableInfo",
    "TapeShieldCableInfo",
];

/// One `WirePosition` of a spacing, as its record states it.
pub(crate) struct Position {
    pub sequence: Option<f64>,
    pub x: Option<f64>,
    pub y: Option<f64>,
}

/// A catalog entry's numeric property by CIM name.
pub(crate) type Lookup<'a> = Box<dyn Fn(&str) -> Option<f64> + 'a>;

/// A wire catalog entry: its label and class, and its numeric properties.
pub(crate) struct Catalog<'a> {
    pub label: String,
    pub class: String,
    pub value: Lookup<'a>,
}

/// One conductor of a segment, in the segment's conductor order: the
/// `ACLineSegmentPhase.sequenceNumber` that names its position, its catalog
/// entry, and whether it is Kron reduced: a neutral the segment's terminals
/// do not connect is grounded at both ends, as a DSS line on a spacing with
/// more conductors than phases reduces its neutrals.
pub(crate) struct Conductor<'a> {
    pub label: String,
    pub sequence: Option<f64>,
    pub catalog: Option<Catalog<'a>>,
    pub reduced: bool,
}

/// Series impedance (ohm/m) and whole shunt susceptance (S/m) per pair of
/// unreduced conductors, in the segment's conductor order, plus findings
/// about values the catalog left to a default.
#[derive(Debug)]
pub(crate) struct Impedance {
    pub r: ConductorMatrix,
    pub x: ConductorMatrix,
    pub b: ConductorMatrix,
    pub notes: Vec<String>,
}

/// A position number: a whole number from 1.
fn sequence(value: Option<f64>) -> Option<u32> {
    let v = value?;
    (v.fract() == 0.0 && (1.0..=f64::from(u32::MAX)).contains(&v)).then_some(v as u32)
}

/// The positions of a spacing by number. A record repeated with the same
/// number and coordinates is one position; the same number at two places
/// is refused.
fn positions(spacing: &str, records: &[Position]) -> Result<BTreeMap<u32, (f64, f64)>, String> {
    let mut out = BTreeMap::new();
    for record in records {
        let Some(number) = sequence(record.sequence) else {
            return Err(format!(
                "WireSpacingInfo {spacing}: a WirePosition states no whole sequenceNumber"
            ));
        };
        let (Some(x), Some(y)) = (record.x, record.y) else {
            return Err(format!(
                "WireSpacingInfo {spacing}: WirePosition {number} states no xCoord and yCoord"
            ));
        };
        match out.insert(number, (x, y)) {
            Some(previous) if previous != (x, y) => {
                return Err(format!(
                    "WireSpacingInfo {spacing}: WirePosition {number} is stated at ({}, {}) m \
                     and at ({x}, {y}) m",
                    previous.0, previous.1
                ));
            }
            _ => {}
        }
    }
    if out.is_empty() {
        return Err(format!("WireSpacingInfo {spacing} states no WirePosition"));
    }
    Ok(out)
}

/// A catalog entry at `(x, y)` as one kernel conductor.
fn wire(catalog: &Catalog<'_>, x: f64, y: f64, notes: &mut Vec<String>) -> Result<Wire, String> {
    let (label, class) = (&catalog.label, catalog.class.as_str());
    let value = |key: &str| (catalog.value)(key);
    let positive = |key: &str| -> Result<f64, String> {
        value(key)
            .filter(|v| *v > 0.0)
            .ok_or_else(|| format!("{class} {label} states no positive {key}"))
    };
    let radius = positive("WireInfo.radius")?;
    let gmr = positive("WireInfo.gmr")?;
    let r_ac = ["WireInfo.rAC25", "WireInfo.rAC50", "WireInfo.rAC75"]
        .into_iter()
        .find_map(|key| value(key).filter(|v| *v > 0.0));
    let r_dc = value("WireInfo.rDC20").filter(|v| *v > 0.0);
    // OpenDSS's ConductorData ratio between the two, which its CIM export
    // also writes.
    let (r_dc, r_ac) = match (r_dc, r_ac) {
        (Some(dc), Some(ac)) => (dc, ac),
        (Some(dc), None) => {
            notes.push(format!(
                "{class} {label} states rDC20 but no rAC25, rAC50 or rAC75; the ac resistance \
                 is 1.02 times rDC20"
            ));
            (dc, dc * 1.02)
        }
        (None, Some(ac)) => {
            notes.push(format!(
                "{class} {label} states an ac resistance but no rDC20; the dc resistance is the \
                 ac one over 1.02"
            ));
            (ac / 1.02, ac)
        }
        (None, None) => {
            return Err(format!(
                "{class} {label} states no positive rDC20, rAC25, rAC50 or rAC75"
            ));
        }
    };
    let cable = match class {
        "WireInfo" | "OverheadWireInfo" => None,
        "ConcentricNeutralCableInfo" | "TapeShieldCableInfo" => {
            let screen = if class == "ConcentricNeutralCableInfo" {
                let strands = positive("ConcentricNeutralCableInfo.neutralStrandCount")?;
                if strands.fract() != 0.0 || strands < 2.0 {
                    return Err(format!(
                        "{class} {label} needs at least two whole neutral strands"
                    ));
                }
                Screen::ConcentricNeutral {
                    k: strands as usize,
                    diastrand: 2.0 * positive("ConcentricNeutralCableInfo.neutralStrandRadius")?,
                    gmrstrand: positive("ConcentricNeutralCableInfo.neutralStrandGmr")?,
                    rstrand: positive("ConcentricNeutralCableInfo.neutralStrandRDC20")?,
                }
            } else {
                // CIM's diameter over the screen is the tape's inside
                // diameter; the tape model takes the diameter over the tape.
                let thickness = positive("TapeShieldCableInfo.tapeThickness")?;
                Screen::TapeShield {
                    diashield: positive("CableInfo.diameterOverScreen")? + 2.0 * thickness,
                    tapelayer: thickness,
                    tapelap: value("TapeShieldCableInfo.tapeLap")
                        .ok_or_else(|| format!("{class} {label} states no tapeLap"))?,
                }
            };
            Some(CableData {
                screen,
                epsr: positive("CableInfo.relativePermittivity")?,
                inslayer: positive("WireInfo.insulationThickness")?,
                diains: positive("CableInfo.diameterOverInsulation")?,
                diacable: if class == "ConcentricNeutralCableInfo" {
                    positive("ConcentricNeutralCableInfo.diameterOverNeutral")?
                } else {
                    value("CableInfo.diameterOverJacket").unwrap_or(f64::NAN)
                },
            })
        }
        _ => {
            return Err(format!(
                "{label} is a {class}, not a wire or cable catalog entry"
            ));
        }
    };
    Ok(Wire {
        cable,
        x,
        height: y,
        radius,
        capacitance_radius: radius,
        gmr,
        rdc: r_dc,
        rac: r_ac,
    })
}

/// The per-meter matrices of a segment whose phases name catalog entries on
/// the positions of `spacing`. Every position must hold exactly one of the
/// segment's conductors: a position no phase names (an unstated neutral)
/// leaves the impedance unknown, and the error says which.
pub(crate) fn segment_impedance(
    spacing: &str,
    records: &[Position],
    conductors: &[Conductor<'_>],
    frequency: f64,
) -> Result<Impedance, String> {
    let positions = positions(spacing, records)?;
    let mut notes = Vec::new();
    let mut taken = BTreeMap::new();
    let mut wires = Vec::with_capacity(conductors.len());
    let mut reduced = Vec::new();
    for conductor in conductors {
        let label = &conductor.label;
        let Some(number) = sequence(conductor.sequence) else {
            return Err(format!(
                "ACLineSegmentPhase {label} states no whole sequenceNumber naming a WirePosition"
            ));
        };
        let Some(&(x, y)) = positions.get(&number) else {
            return Err(format!(
                "ACLineSegmentPhase {label} names WirePosition {number}, which WireSpacingInfo \
                 {spacing} does not state"
            ));
        };
        if let Some(other) = taken.insert(number, label) {
            return Err(format!(
                "ACLineSegmentPhases {other} and {label} both name WirePosition {number}"
            ));
        }
        let Some(catalog) = &conductor.catalog else {
            return Err(format!(
                "ACLineSegmentPhase {label} names no wire or cable catalog entry"
            ));
        };
        let wire = wire(catalog, x, y, &mut notes)?;
        if conductor.reduced {
            reduced.push(wire);
        } else {
            wires.push(wire);
        }
    }
    let keep = wires.len();
    wires.extend(reduced);
    let unnamed: Vec<String> = positions
        .keys()
        .filter(|n| !taken.contains_key(n))
        .map(u32::to_string)
        .collect();
    if !unnamed.is_empty() {
        return Err(format!(
            "WireSpacingInfo {spacing} places {} conductors, but no ACLineSegmentPhase names a \
             wire for position(s) {}; that conductor's wire (typically the neutral) is not \
             stated, so its effect on the phase impedance is unknown",
            positions.len(),
            unnamed.join(", ")
        ));
    }
    let matrices = catalog_matrices(wires, keep, frequency).map_err(|e| {
        format!("the wire catalog conductors on WireSpacingInfo {spacing} are refused: {e}")
    })?;
    Ok(Impedance {
        r: matrices.r,
        x: matrices.x,
        b: matrices.b,
        notes,
    })
}

#[cfg(test)]
mod tests;
