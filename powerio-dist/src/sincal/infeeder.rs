//! Native AC infeeder inputs. Load-flow impedance is distinct from fault data.
//!
//! Setpoints retain their regulation location, native line-line voltage basis,
//! and aggregate powers. These are not yet grounded conductor-domain sources.

use num_complex::Complex64;
use rusqlite::Row;

use super::{
    format_error,
    load::VoltageInput,
    schema::{NativeDatabase, require_table},
    semantics::{Connection, ElectricalTerminal, State, require_input_categories},
    transformer::{integer, number, reference},
};
use crate::Result;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RegulationPoint {
    /// Voltage behind the load-flow internal impedance.
    Internal,
    /// Voltage at the electrical port, with internal drop reported separately.
    Terminal,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum InfeederSetpoint {
    Current {
        amperes: f64,
        angle_rad: f64,
    },
    Power {
        watts: f64,
        vars: f64,
    },
    Voltage {
        voltage: VoltageInput,
        angle_rad: f64,
        at: RegulationPoint,
    },
    ActivePowerVoltage {
        watts: f64,
        voltage: VoltageInput,
        at: RegulationPoint,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum InternalImpedance {
    /// Zero positive-sequence load-flow impedance. This alone does not ground
    /// the source or establish its zero-sequence circuit.
    Ideal,
    /// April 2014 Input Data pp. 45–49: xi is a percent of the reactance
    /// base Unn²/Sk2, not a percent of the short-circuit impedance magnitude.
    ReactancePercent {
        fraction: f64,
        short_circuit_va: f64,
        r_over_x: f64,
    },
}

impl InternalImpedance {
    /// Positive-sequence load-flow impedance; zero sequence/neutral topology
    /// must still be resolved before constructing any conductor matrix.
    pub fn positive_sequence_ohms(self, nominal_ll_volts: f64) -> Result<Complex64> {
        if !nominal_ll_volts.is_finite() || nominal_ll_volts <= 0.0 {
            return Err(format_error("invalid infeeder nominal voltage"));
        }
        match self {
            Self::Ideal => Ok(Complex64::new(0.0, 0.0)),
            Self::ReactancePercent {
                fraction,
                short_circuit_va,
                r_over_x,
            } => {
                let x =
                    finite(fraction * (nominal_ll_volts / short_circuit_va) * nominal_ll_volts)?;
                if x <= 0.0 {
                    return Err(format_error("infeeder internal reactance underflow"));
                }
                Ok(Complex64::new(finite(r_over_x * x)?, x))
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
/// Native zero-sequence inputs, before calculation-mode and neutral resolution.
/// They must not be mistaken for an already assembled load-flow primitive.
pub(super) enum SourceZeroSequence {
    MagnitudeRatio { z0_over_z1: f64, r0_over_x0: f64 },
    DirectOhms(Complex64),
    SameAsPositive,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum SourceGrounding {
    Ungrounded,
    Solid(SourceZeroSequence),
    Impedance {
        sequence: SourceZeroSequence,
        neutral_point: Option<i64>,
    },
}

pub(super) struct InfeederInput {
    pub element: i64,
    pub terminal: ElectricalTerminal,
    pub state: State,
    pub setpoint: InfeederSetpoint,
    pub internal_impedance: InternalImpedance,
    pub grounding: SourceGrounding,
    pub defaulted: Vec<&'static str>,
    /// Daily, weekly and yearly series. No state is implicitly selected.
    pub operating_series: [Option<i64>; 3],
}

impl NativeDatabase {
    fn assume_inactive_control(&self, row: &Row<'_>, field: &str) -> bool {
        self.assume_inactive_source_controls
            && self.version.to_bits() == 11.5_f64.to_bits()
            && matches!(row.get_ref(field), Ok(rusqlite::types::ValueRef::Null))
    }

    pub fn infeeder_input(&self, element: i64) -> Result<InfeederInput> {
        if self.elements.get(&element).map(String::as_str) != Some("Infeeder") {
            return Err(format_error(format!(
                "Element {element} is not an Infeeder"
            )));
        }
        let mut terminals = self.electrical_terminals(element)?;
        if terminals.len() != 1
            || terminals[0].position != 1
            || terminals[0].connection == Connection::Neutral
        {
            return Err(format_error(
                "infeeder requires one phase or phase-pair port",
            ));
        }
        require_table(&self.connection, "Infeeder", &["Element_ID", "Variant_ID"])?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT e.Flag_Input AS ElementInput, i.* FROM Infeeder i JOIN Element e
             ON e.Element_ID=i.Element_ID AND e.Variant_ID=i.Variant_ID
             WHERE i.Element_ID=?1 AND i.Variant_ID=?2",
            )
            .map_err(format_error)?;
        let mut rows = statement
            .query([element, self.variant])
            .map_err(format_error)?;
        let row = rows
            .next()
            .map_err(format_error)?
            .ok_or_else(|| format_error("missing Infeeder row"))?;
        require_input_categories(integer(row, "ElementInput")?, 2)?;
        Self::materialized_type(row)?;
        for field in [
            "Mpl_ID",
            "IncrSer_ID",
            "Macro_ID",
            "MasterElm_ID",
            "Node_ID",
            "PowerLimit_ID",
        ] {
            if reference(row, field)?.is_some() {
                return Err(format_error(format!(
                    "infeeder requires resolution of {field}"
                )));
            }
        }
        let mut defaulted = Vec::new();
        for field in [
            "Flag_LfLimit",
            "Flag_LfCtrl",
            "Flag_Pctrl",
            "Flag_Qctrl",
            "Flag_Macro",
        ] {
            // Opt-in schema-11.5 compatibility: SQL NULL is an assumption,
            // not proof of the application default. Never accept active values,
            // missing columns, or NULLs in unrelated/newer controls.
            if field != "Flag_Pctrl" && self.assume_inactive_control(row, field) {
                defaulted.push(field);
            } else if self.newer_integer(row, field, 0)? != 0 {
                return Err(format_error(format!(
                    "unsupported infeeder control {field}"
                )));
            }
        }
        if self.assume_inactive_control(row, "Kr") {
            defaulted.push("Kr");
        } else if number(row, "Kr")? != 0.0 {
            return Err(format_error(
                "infeeder frequency control requires additional mapping",
            ));
        }
        // These newer fields are absent from the 2014 definition. Do not guess
        // that they are interchangeable with xi or silently discard them.
        if self.newer_number(row, "Rlf")? != 0.0 || self.newer_number(row, "Xlf")? != 0.0 {
            return Err(format_error(
                "unverified explicit infeeder load-flow impedance",
            ));
        }
        let setpoint = setpoint(row)?;
        let internal_impedance = internal_impedance(row)?;
        let grounding = grounding(row)?;
        let operating_series = [
            reference(row, "DayOpSer_ID")?,
            reference(row, "WeekOpSer_ID")?,
            reference(row, "YearOpSer_ID")?,
        ];
        if rows.next().map_err(format_error)?.is_some() {
            return Err(format_error("duplicate Infeeder row"));
        }
        Ok(InfeederInput {
            element,
            terminal: terminals.remove(0),
            state: self.element_state(element)?,
            setpoint,
            internal_impedance,
            grounding,
            defaulted,
            operating_series,
        })
    }
}

fn setpoint(row: &Row<'_>) -> Result<InfeederSetpoint> {
    let mode = integer(row, "Flag_Lf")?;
    match mode {
        1 => Ok(InfeederSetpoint::Current {
            amperes: nonnegative(factored(row, "I", "fI", 1000.0)?)?,
            angle_rad: number(row, "phi")?.to_radians(),
        }),
        2 => Ok(InfeederSetpoint::Power {
            watts: factored(row, "P", "fP", 1e6)?,
            vars: factored(row, "Q", "fQ", 1e6)?,
        }),
        4 | 10 => {
            let pf = number(row, "cosphi")?;
            if !(0.0..=1.0).contains(&pf) || (mode == 10 && pf == 0.0) {
                return Err(format_error("unsupported infeeder power-factor convention"));
            }
            let s = if mode == 4 {
                nonnegative(factored(row, "S", "fS", 1e6)?)?
            } else {
                finite(factored(row, "P", "fP", 1e6)? / pf)?
            };
            Ok(InfeederSetpoint::Power {
                watts: finite(s * pf)?,
                vars: finite(s * (1.0 - pf * pf).sqrt())?,
            })
        }
        3 | 6 | 8 | 9 => Ok(InfeederSetpoint::Voltage {
            voltage: voltage(row, matches!(mode, 3 | 8))?,
            angle_rad: number(row, "delta")?.to_radians(),
            at: if matches!(mode, 3 | 6) {
                RegulationPoint::Internal
            } else {
                RegulationPoint::Terminal
            },
        }),
        5 | 7 | 11 | 12 => Ok(InfeederSetpoint::ActivePowerVoltage {
            watts: factored(row, "P", "fP", 1e6)?,
            voltage: voltage(row, matches!(mode, 5 | 11))?,
            at: if matches!(mode, 11 | 12) {
                RegulationPoint::Internal
            } else {
                RegulationPoint::Terminal
            },
        }),
        _ => Err(format_error(format!(
            "unsupported infeeder input mode {mode}"
        ))),
    }
}

fn voltage(row: &Row<'_>, relative: bool) -> Result<VoltageInput> {
    let value = if relative {
        number(row, "u")? / 100.0
    } else {
        finite(number(row, "Ug")? * 1000.0)?
    };
    if value <= 0.0 {
        return Err(format_error("nonpositive infeeder voltage"));
    }
    Ok(if relative {
        VoltageInput::Relative(value)
    } else {
        VoltageInput::Absolute(value)
    })
}

fn internal_impedance(row: &Row<'_>) -> Result<InternalImpedance> {
    let percent = nonnegative(number(row, "xi")?)?;
    if percent == 0.0 {
        return Ok(InternalImpedance::Ideal);
    }
    let fraction = percent / 100.0;
    if fraction == 0.0 {
        return Err(format_error("infeeder internal reactance underflow"));
    }
    require_input_categories(integer(row, "ElementInput")?, 1)?;
    if integer(row, "Flag_Typ")? != 2 {
        return Err(format_error(
            "nonzero xi with direct fault R/X requires a verified impedance-base conversion",
        ));
    }
    let short_circuit_va = finite(number(row, "Sk2")? * 1e6)?;
    if short_circuit_va <= 0.0 {
        return Err(format_error("nonpositive infeeder short-circuit base"));
    }
    Ok(InternalImpedance::ReactancePercent {
        fraction,
        short_circuit_va,
        r_over_x: nonnegative(number(row, "R_X")?)?,
    })
}

fn grounding(row: &Row<'_>) -> Result<SourceGrounding> {
    let flag = integer(row, "Flag_Z0")?;
    if flag == 0 {
        return Ok(SourceGrounding::Ungrounded);
    }
    if !matches!(flag, 1 | 2) {
        return Err(format_error("unknown infeeder grounding"));
    }
    require_input_categories(integer(row, "ElementInput")?, 4)?;
    let sequence = match integer(row, "Flag_Z0_Input")? {
        1 => SourceZeroSequence::MagnitudeRatio {
            z0_over_z1: nonnegative(number(row, "Z0_Z1")?)?,
            r0_over_x0: nonnegative(number(row, "R0_X0")?)?,
        },
        2 => SourceZeroSequence::DirectOhms(Complex64::new(
            nonnegative(number(row, "R0")?)?,
            number(row, "X0")?,
        )),
        3 => SourceZeroSequence::SameAsPositive,
        _ => return Err(format_error("unknown infeeder zero-sequence input")),
    };
    Ok(if flag == 1 {
        SourceGrounding::Solid(sequence)
    } else {
        SourceGrounding::Impedance {
            sequence,
            neutral_point: reference(row, "Stp_ID")?,
        }
    })
}

fn finite(value: f64) -> Result<f64> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(format_error("infeeder value overflows SI units"))
    }
}

fn nonnegative(value: f64) -> Result<f64> {
    if value >= 0.0 {
        Ok(value)
    } else {
        Err(format_error("negative infeeder magnitude"))
    }
}

fn factored(row: &Row<'_>, value: &str, factor: &str, scale: f64) -> Result<f64> {
    finite(number(row, value)? * number(row, factor)? * scale)
}
