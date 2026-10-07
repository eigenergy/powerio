//! Active load-input decoding, separate from electrical network lowering.
//!
//! Values use SI units, but total versus per-phase input and relative versus
//! absolute voltage remain explicit. This prevents premature phase allocation
//! or a guessed neutral/voltage base. Profile and grounding references must be
//! resolved by the mapper before creating a numerical DistLoad.

use num_complex::Complex64;
use rusqlite::Row;

use super::{
    format_error,
    schema::{NativeDatabase, require_table},
    semantics::{Connection, ElectricalTerminal, State, require_input_categories},
};
use crate::Result;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LoadModel {
    Impedance,
    Power,
    Current,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum VoltageInput {
    /// Fraction of the native nominal voltage; not yet phase-voltage based.
    Relative(f64),
    /// Native absolute voltage in volts; connection/base resolution is later.
    Absolute(f64),
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum PowerInput {
    Total {
        p: f64,
        q: f64,
    },
    /// Aggregate power explicitly connected in delta (native input mode 15).
    DeltaTotal {
        p: f64,
        q: f64,
    },
    /// A/B/C order, not terminal-row order.
    Wye {
        p: [f64; 3],
        q: [f64; 3],
    },
    /// AB/BC/CA branch order, not an A/B/C conductor-power vector.
    Delta {
        p: [f64; 3],
        q: [f64; 3],
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum LoadZeroSequence {
    NotDeclared,
    SameAsPositive,
    MagnitudeRatio { z0_over_z1: f64, r0_over_x0: f64 },
    DirectOhms(Complex64),
}

pub(super) struct LoadInput {
    pub element: i64,
    pub terminal: ElectricalTerminal,
    pub state: State,
    pub model: LoadModel,
    pub voltage: VoltageInput,
    pub power: PowerInput,
    pub neutral_point: Option<i64>,
    pub zero_sequence: LoadZeroSequence,
    /// Native compatibility inputs; a nonzero value needs its own mapping.
    pub negative_sequence_power: Option<Complex64>,
    /// Daily, weekly, yearly operating series; resolve the selected state.
    pub operating_series: [Option<i64>; 3],
    pub profile_selection: Option<super::load_profile::LoadProfileSelection>,
}

impl NativeDatabase {
    pub fn load_input(&self, element: i64) -> Result<LoadInput> {
        if self.elements.get(&element).map(String::as_str) != Some("Load") {
            return Err(format_error(format!("Element {element} is not a Load")));
        }
        let mut terminals = self.electrical_terminals(element)?;
        if terminals.len() != 1 || terminals[0].position != 1 {
            return Err(format_error(format!(
                "Load {element} requires exactly one port"
            )));
        }
        if terminals[0].connection == Connection::Neutral {
            return Err(format_error(
                "a load requires a phase or phase-pair connection",
            ));
        }
        require_table(&self.connection, "Load", &["Element_ID", "Variant_ID"])?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT e.Flag_Input AS ElementInput, l.* FROM Load l JOIN Element e
             ON e.Element_ID=l.Element_ID AND e.Variant_ID=l.Variant_ID
             WHERE l.Element_ID=?1 AND l.Variant_ID=?2",
            )
            .map_err(format_error)?;
        let mut rows = statement
            .query([element, self.variant])
            .map_err(format_error)?;
        let row = rows
            .next()
            .map_err(format_error)?
            .ok_or_else(|| format_error(format!("missing Load row for Element {element}")))?;
        require_input_categories(integer(row, "ElementInput")?, 2)?;
        check_static_input(row)?;
        let model = match integer(row, "Flag_LoadType")? {
            1 => LoadModel::Impedance,
            2 => LoadModel::Power,
            3 => LoadModel::Current,
            code => return Err(format_error(format!("unsupported load model {code}"))),
        };
        let mode = integer(row, "Flag_Lf")?;
        let voltage = match mode {
            1 | 3 | 11 | 13 | 14 | 15 => VoltageInput::Relative(positive(row, "u")? / 100.0),
            2 | 4 | 12 => VoltageInput::Absolute(scaled(row, "Ul", 1000.0)?),
            code => return Err(format_error(format!("unsupported load input mode {code}"))),
        };
        if matches!(voltage, VoltageInput::Absolute(v) | VoltageInput::Relative(v) if v <= 0.0) {
            return Err(format_error("load voltage must be positive"));
        }
        let power = power_input(row, mode)?;
        let neutral_point = reference(row, "Stp_ID")?;
        let categories = integer(row, "ElementInput")?;
        let zero_sequence = zero_sequence(row, categories)?;
        let negative_sequence_power = if categories & 8 != 0 {
            Some(Complex64::new(
                scaled(row, "Pneg", 1e6)?,
                scaled(row, "Qneg", 1e6)?,
            ))
        } else {
            None
        };
        let operating_series = [
            reference(row, "DayOpSer_ID")?,
            reference(row, "WeekOpSer_ID")?,
            reference(row, "YearOpSer_ID")?,
        ];
        if rows.next().map_err(format_error)?.is_some() {
            return Err(format_error(format!(
                "duplicate Load row for Element {element}"
            )));
        }
        Ok(LoadInput {
            element,
            terminal: terminals.remove(0),
            state: self.element_state(element)?,
            model,
            voltage,
            power,
            neutral_point,
            zero_sequence,
            negative_sequence_power,
            operating_series,
            profile_selection: None,
        })
    }
}

fn check_static_input(row: &Row<'_>) -> Result<()> {
    if integer(row, "Flag_Load")? != 1 {
        return Err(format_error("house-connection load requires customer data"));
    }
    for field in [
        "Typ_ID",
        "Mpl_ID",
        "Gang_ID",
        "Load_ID",
        "IncrSer_ID",
        "Macro_ID",
    ] {
        if reference(row, field)?.is_some() {
            return Err(format_error(format!("load requires resolution of {field}")));
        }
    }
    if number(row, "Ireg")? != 0.0 || integer(row, "Flag_Typified")? != 0 {
        return Err(format_error(
            "controlled or typified load requires additional mapping",
        ));
    }
    Ok(())
}

fn power_input(row: &Row<'_>, mode: i64) -> Result<PowerInput> {
    match mode {
        1 | 2 => Ok(PowerInput::Total {
            p: factored(row, "P", "fP")?,
            q: factored(row, "Q", "fQ")?,
        }),
        15 => Ok(PowerInput::DeltaTotal {
            p: factored(row, "P", "fP")?,
            q: factored(row, "Q", "fQ")?,
        }),
        3 | 4 | 11 | 12 => {
            let pf = number(row, "cosphi")?;
            // Sign conventions for negative power factors require additional
            // evidence. Do not silently turn those into inductive loads.
            if !(0.0..=1.0).contains(&pf) || (matches!(mode, 11 | 12) && pf == 0.0) {
                return Err(format_error("unsupported or invalid load power factor"));
            }
            let apparent = if matches!(mode, 3 | 4) {
                factored(row, "S", "fS")?
            } else {
                factored(row, "P", "fP")? / pf
            };
            let p = finite(apparent * pf, "load active power")?;
            let q = finite(apparent * (1.0 - pf * pf).sqrt(), "load reactive power")?;
            Ok(PowerInput::Total { p, q })
        }
        13 => Ok(PowerInput::Wye {
            p: [
                factored(row, "P1", "fP")?,
                factored(row, "P2", "fP")?,
                factored(row, "P3", "fP")?,
            ],
            q: [
                factored(row, "Q1", "fQ")?,
                factored(row, "Q2", "fQ")?,
                factored(row, "Q3", "fQ")?,
            ],
        }),
        14 => Ok(PowerInput::Delta {
            p: [
                factored(row, "P12", "fP")?,
                factored(row, "P23", "fP")?,
                factored(row, "P31", "fP")?,
            ],
            q: [
                factored(row, "Q12", "fQ")?,
                factored(row, "Q23", "fQ")?,
                factored(row, "Q31", "fQ")?,
            ],
        }),
        _ => Err(format_error(format!("unsupported load input mode {mode}"))),
    }
}

fn zero_sequence(row: &Row<'_>, categories: i64) -> Result<LoadZeroSequence> {
    if categories & 4 == 0 {
        return Ok(LoadZeroSequence::NotDeclared);
    }
    match integer(row, "Flag_Z0_Input")? {
        1 => {
            let z0_over_z1 = number(row, "Z0_Z1")?;
            let r0_over_x0 = number(row, "R0_X0")?;
            if z0_over_z1 < 0.0 || r0_over_x0 < 0.0 {
                return Err(format_error("negative load zero-sequence ratio"));
            }
            Ok(LoadZeroSequence::MagnitudeRatio {
                z0_over_z1,
                r0_over_x0,
            })
        }
        2 => {
            let r = number(row, "R0")?;
            if r < 0.0 {
                return Err(format_error("negative load zero-sequence resistance"));
            }
            Ok(LoadZeroSequence::DirectOhms(Complex64::new(
                r,
                number(row, "X0")?,
            )))
        }
        3 => Ok(LoadZeroSequence::SameAsPositive),
        code => Err(format_error(format!(
            "unsupported load zero-sequence mode {code}"
        ))),
    }
}

fn integer(row: &Row<'_>, field: &str) -> Result<i64> {
    row.get(field).map_err(format_error)
}

fn reference(row: &Row<'_>, field: &str) -> Result<Option<i64>> {
    let id: Option<i64> = row.get(field).map_err(format_error)?;
    match id {
        None | Some(0) => Ok(None),
        Some(id) if id > 0 => Ok(Some(id)),
        _ => Err(format_error(format!("invalid {field} reference"))),
    }
}

fn finite(value: f64, field: &str) -> Result<f64> {
    if !value.is_finite() {
        return Err(format_error(format!("nonfinite {field}")));
    }
    Ok(value)
}

fn number(row: &Row<'_>, field: &str) -> Result<f64> {
    finite(row.get(field).map_err(format_error)?, field)
}

fn positive(row: &Row<'_>, field: &str) -> Result<f64> {
    let value = number(row, field)?;
    if value <= 0.0 {
        return Err(format_error(format!("nonpositive {field}")));
    }
    Ok(value)
}

fn scaled(row: &Row<'_>, field: &str, factor: f64) -> Result<f64> {
    finite(number(row, field)? * factor, field)
}

fn factored(row: &Row<'_>, value: &str, factor: &str) -> Result<f64> {
    finite(number(row, value)? * number(row, factor)? * 1e6, value)
}
