//! The verified three-phase sequence-line profile.
//!
//! This representation assumes equal positive/negative sequence impedances.
//! It permits unbalanced phase currents, but does not describe an explicit
//! neutral conductor. No four-wire primitive can be recovered from it.

use std::f64::consts::TAU;

use super::{
    format_error,
    schema::{NativeDatabase, require_table},
    semantics::require_input_categories,
};
use crate::{ConductorMatrix, DistLineCode, Result};

#[derive(Clone, Copy, Debug)]
pub(super) struct SequenceParameters {
    /// Native units: ohm/km and nF/km.
    pub r1: f64,
    pub x1: f64,
    pub c1: f64,
    pub r0: f64,
    pub x0: f64,
    pub c0: f64,
    pub frequency_hz: f64,
}

impl SequenceParameters {
    pub fn to_linecode(self, name: String) -> Result<DistLineCode> {
        for (field, value) in [
            ("r", self.r1),
            ("x", self.x1),
            ("c", self.c1),
            ("r0", self.r0),
            ("x0", self.x0),
            ("c0", self.c0),
        ] {
            if !value.is_finite() || value < 0.0 {
                return Err(format_error(format!(
                    "line {name}: invalid {field}={value}"
                )));
            }
        }
        if !self.frequency_hz.is_finite() || self.frequency_hz <= 0.0 {
            return Err(format_error("line frequency must be finite and positive"));
        }
        let mut code = DistLineCode::new(
            name,
            phase_matrix(self.r1 / 1000.0, self.r0 / 1000.0),
            phase_matrix(self.x1 / 1000.0, self.x0 / 1000.0),
        );
        // nF/km -> F/m -> S/m; each end gets half the shunt admittance.
        let factor = TAU * self.frequency_hz * 1e-12 / 2.0;
        code.b_from = phase_matrix(self.c1 * factor, self.c0 * factor);
        code.b_to.clone_from(&code.b_from);
        for (positive, zero, matrix) in [
            (self.r1, self.r0, &code.r_series),
            (self.x1, self.x0, &code.x_series),
            (self.c1, self.c0, &code.b_from),
        ] {
            if (positive != 0.0 || zero != 0.0)
                && matrix.iter().flatten().all(|value| *value == 0.0)
            {
                return Err(format_error(
                    "nonzero line parameter underflows SI phase matrix",
                ));
            }
        }
        if code.b_from.iter().flatten().any(|value| !value.is_finite()) {
            return Err(format_error("line charging overflows SI units"));
        }
        Ok(code)
    }
}

fn phase_matrix(positive: f64, zero: f64) -> ConductorMatrix {
    // Divide before adding to avoid overflowing large finite input values.
    let mutual = zero / 3.0 - positive / 3.0;
    let diagonal = 2.0 * (positive / 3.0) + zero / 3.0;
    let mut matrix = vec![vec![mutual; 3]; 3];
    for (i, row) in matrix.iter_mut().enumerate() {
        row[i] = diagonal;
    }
    matrix
}

pub(super) struct SequenceLine {
    pub element_id: i64,
    pub length_m: f64,
    pub code: DistLineCode,
    pub frequency_hz: f64,
    pub defaulted: Vec<&'static str>,
}

/// Corrections evaluated from variant-local native operating data.
pub(super) struct LineOperatingPoint {
    pub frequency_hz: f64,
    pub resistance_factor: f64,
    /// Full positive/negative-sequence shunt conductance, including parallel
    /// count, in S/m. The documented zero-sequence shunt has no conductance.
    pub conductance_s_per_m: f64,
}

impl SequenceLine {
    pub fn at_operating_point(self, point: &LineOperatingPoint) -> Result<Self> {
        let mut line = self.at_frequency(point.frequency_hz)?;
        scale_matrix(&mut line.code.r_series, point.resistance_factor)?;
        let g = point.conductance_s_per_m;
        if !g.is_finite() || g < 0.0 {
            return Err(format_error("invalid line dielectric conductance"));
        }
        // Input Data (2014), printed pp. 153 and 156: G1=G2, G0=0.
        // Transform to phases before splitting the pi shunt between ends.
        line.code.g_from = phase_matrix(g / 2.0, 0.0);
        line.code.g_to.clone_from(&line.code.g_from);
        if g > 0.0 && line.code.g_from[0][0] == 0.0 {
            return Err(format_error("line dielectric conductance underflows"));
        }
        Ok(line)
    }

    /// Convert rated-frequency reactance and charging to the operating
    /// frequency. Resistance and thermal current limits do not scale here.
    pub fn at_frequency(mut self, frequency_hz: f64) -> Result<Self> {
        if !frequency_hz.is_finite() || frequency_hz <= 0.0 {
            return Err(format_error("invalid line operating frequency"));
        }
        let factor = frequency_hz / self.frequency_hz;
        scale_matrix(&mut self.code.x_series, factor)?;
        scale_matrix(&mut self.code.b_from, factor)?;
        scale_matrix(&mut self.code.b_to, factor)?;
        self.frequency_hz = frequency_hz;
        Ok(self)
    }
}

fn scale_matrix(matrix: &mut ConductorMatrix, factor: f64) -> Result<()> {
    if !factor.is_finite() || factor <= 0.0 {
        return Err(format_error("line scaling factor overflows or underflows"));
    }
    for value in matrix.iter_mut().flatten() {
        let scaled = *value * factor;
        if !scaled.is_finite() || (*value != 0.0 && scaled == 0.0) {
            return Err(format_error(
                "line parameter overflows or underflows during scaling",
            ));
        }
        *value = scaled;
    }
    Ok(())
}

fn apply_parallel_rating(
    code: &mut DistLineCode,
    parallel: f64,
    rating: f64,
    amperes: f64,
) -> Result<()> {
    if !parallel.is_finite() || parallel <= 0.0 || !rating.is_finite() || rating <= 0.0 {
        return Err(format_error(
            "line parallel and rating factors must be finite and positive",
        ));
    }
    scale_matrix(&mut code.r_series, 1.0 / parallel)?;
    scale_matrix(&mut code.x_series, 1.0 / parallel)?;
    for matrix in [
        &mut code.g_from,
        &mut code.g_to,
        &mut code.b_from,
        &mut code.b_to,
    ] {
        scale_matrix(matrix, parallel)?;
    }
    // Database Description (April 2014), p.19: Ith defaults to zero.
    // Keep an unspecified thermal rating absent in the generic model; do
    // not invent a positive limit or a zero-current electrical constraint.
    if amperes == 0.0 {
        code.i_max = None;
        return Ok(());
    }
    let limit = amperes * rating * parallel;
    if !limit.is_finite() || limit <= 0.0 {
        return Err(format_error(
            "line ampacity overflows or underflows during scaling",
        ));
    }
    code.i_max = Some(vec![limit; 3]);
    Ok(())
}

impl NativeDatabase {
    /// Decode a sequence line, applying the selected missing-data policy.
    /// This does not resolve switching, service state, end connections or
    /// the network as a whole.
    /// Unsupported modes are refused before numerical data is constructed.
    #[allow(clippy::too_many_lines)] // Keep selected-row validation and circuit conversion together.
    pub fn sequence_line(&self, element: i64) -> Result<SequenceLine> {
        if self.elements.get(&element).map(String::as_str) != Some("Line") {
            return Err(format_error(format!("Element {element} is not a Line")));
        }
        let zero_mode = self.zero_sequence_mode()?;
        require_table(&self.connection, "Line", &["Element_ID", "Variant_ID"])?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT e.Flag_Input, l.Flag_Z0_Input, l.Typ_ID, l.CoupData_ID,
                    l.ParSys, l.fr, l.l, l.Ith, l.r, l.x, l.c, l.r0, l.x0, l.c0, l.fn,
                    l.R0_R1, l.X0_X1, l.*
             FROM Line l JOIN Element e ON e.Element_ID=l.Element_ID AND e.Variant_ID=l.Variant_ID
             WHERE l.Element_ID=?1 AND l.Variant_ID=?2",
            )
            .map_err(format_error)?;
        let mut rows = statement
            .query([element, self.variant])
            .map_err(format_error)?;
        let row = rows
            .next()
            .map_err(format_error)?
            .ok_or_else(|| format_error(format!("missing Line row for Element {element}")))?;
        let input: i64 = row.get(0).map_err(format_error)?;
        let zero_input: i64 = if input & 4 != 0 {
            row.get(1).map_err(format_error)?
        } else {
            0
        };
        let supplied = input & 4 != 0;
        let synthesize = !supplied && zero_mode == 2;
        if require_input_categories(input, 2).is_err()
            || (!synthesize && (!supplied || !matches!(zero_input, 1 | 2)))
        {
            return Err(format_error(format!(
                "Line {element}: explicit zero-sequence data required (load-flow/zero-sequence input bits, Flag_Z0_Input=1 or 2)"
            )));
        }
        Self::materialized_type(row)?;
        let coupling: Option<i64> = row.get(3).map_err(format_error)?;
        if !matches!(coupling, None | Some(0)) {
            return Err(format_error(format!(
                "Line {element}: referenced type/coupling data must be resolved"
            )));
        }
        let mut defaulted = Vec::new();
        let parallel = self.line_optional_number(row, "ParSys", &mut defaulted)?;
        let rating_factor = self.line_optional_number(row, "fr", &mut defaulted)?;
        let length: f64 = row.get(6).map_err(format_error)?;
        let current: f64 = row.get(7).map_err(format_error)?;
        if !length.is_finite()
            || length <= 0.0
            || !(length * 1000.0).is_finite()
            || !current.is_finite()
            || current < 0.0
            || !(current * 1000.0).is_finite()
        {
            return Err(format_error(format!(
                "Line {element}: positive finite length and nonnegative finite ampacity required"
            )));
        }
        let mut parameters = SequenceParameters {
            r1: row.get(8).map_err(format_error)?,
            x1: row.get(9).map_err(format_error)?,
            c1: row.get(10).map_err(format_error)?,
            r0: 0.0,
            x0: 0.0,
            c0: if synthesize {
                0.0
            } else {
                row.get(13).map_err(format_error)?
            },
            frequency_hz: self.line_optional_number(row, "fn", &mut defaulted)?,
        };
        (parameters.r0, parameters.x0) = if synthesize {
            // Input Data (April 2014), p.219: mode 2 supplements missing
            // zero-sequence data with the positive-sequence data. Explicit
            // component inputs retain precedence over this calculation mode.
            parameters.c0 = parameters.c1;
            defaulted.push("zero_sequence_from_positive");
            (parameters.r1, parameters.x1)
        } else if zero_input == 1 {
            let r_ratio: f64 = row.get(15).map_err(format_error)?;
            let x_ratio: f64 = row.get(16).map_err(format_error)?;
            if !r_ratio.is_finite() || r_ratio < 0.0 || !x_ratio.is_finite() || x_ratio < 0.0 {
                return Err(format_error("invalid line zero-sequence ratios"));
            }
            (parameters.r1 * r_ratio, parameters.x1 * x_ratio)
        } else {
            (
                row.get(11).map_err(format_error)?,
                row.get(12).map_err(format_error)?,
            )
        };
        if rows.next().map_err(format_error)?.is_some() {
            return Err(format_error(format!(
                "duplicate Line row for Element {element}"
            )));
        }
        let mut code = parameters.to_linecode(format!("sincal-line-{element}"))?;
        apply_parallel_rating(&mut code, parallel, rating_factor, current * 1000.0)?;
        Ok(SequenceLine {
            element_id: element,
            length_m: length * 1000.0,
            code,
            frequency_hz: parameters.frequency_hz,
            defaulted,
        })
    }

    pub(super) fn require_input_zero_sequence(&self) -> Result<()> {
        if self.zero_sequence_mode()? != 1 {
            return Err(format_error(
                "component requires explicit input zero-sequence mode",
            ));
        }
        Ok(())
    }

    pub(super) fn zero_sequence_mode(&self) -> Result<i64> {
        require_table(
            &self.connection,
            "CalcParameter",
            &["Variant_ID", "Flag_LFZ0"],
        )?;
        let mut statement = self
            .connection
            .prepare("SELECT Flag_LFZ0 FROM CalcParameter WHERE Variant_ID=?1")
            .map_err(format_error)?;
        let mut rows = statement.query([self.variant]).map_err(format_error)?;
        let row = rows
            .next()
            .map_err(format_error)?
            .ok_or_else(|| format_error("missing calculation settings for selected variant"))?;
        let mode: i64 = row.get(0).map_err(format_error)?;
        if !matches!(mode, 1 | 2) || rows.next().map_err(format_error)?.is_some() {
            return Err(format_error(
                "requires one calculation setting with input zero-sequence data",
            ));
        }
        Ok(mode)
    }
}
