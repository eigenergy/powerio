//! Fixed reactor/capacitor banks; Input Data (April 2014), pp.125–136.
//! S is the total installed nameplate rating, including reduced phase ports.
//! Device earth/star references never ground a native bus neutral.
use num_complex::Complex64;
use rusqlite::Row;

use super::{
    format_error,
    schema::NativeDatabase,
    semantics::{Connection, ElectricalTerminal, State, require_input_categories},
    shunt_impedance::{ShuntImpedanceCircuit, reciprocal},
    transformer::{integer, number, reference},
};
use crate::{DistBus, DistShunt, DistSwitch, Result};

pub(super) struct RatedShuntInput {
    element: i64,
    pub terminal: ElectricalTerminal,
    state: State,
    positive: Complex64,
    zero: Complex64,
    grounded: bool,
    pub defaulted: Vec<&'static str>,
}

impl NativeDatabase {
    fn rated_shunt_controls(
        &self,
        row: &Row<'_>,
        legacy: bool,
        defaulted: &mut Vec<&'static str>,
    ) -> Result<i64> {
        if default_flag(row, "Flag_Macro", legacy, defaulted)? != 0 {
            return Err(format_error("rated shunt macro requires separate mapping"));
        }
        for field in ["Macro_ID", "Node_ID", "Terminal_ID"] {
            if reference(row, field)?.is_some() {
                return Err(format_error(format!(
                    "rated shunt requires resolution of {field}"
                )));
            }
        }
        for field in ["Ctrl_OpSer_ID", "Ctrl_OpPnt_ID"] {
            if self.newer_reference(row, field)?.is_some() {
                return Err(format_error(
                    "rated shunt controller/profile requires operating-state resolution",
                ));
            }
        }
        if !(legacy && row.as_ref().column_index("Flag_Step").is_err())
            && integer(row, "Flag_Step")? != 0
        {
            return Err(format_error(
                "rated shunt discrete step curve requires separate mapping",
            ));
        }
        let regulator = default_flag(row, "Flag_roh", legacy, defaulted)?;
        if !matches!(regulator, 0 | 1) {
            return Err(format_error(
                "rated shunt regulator requires operating-state resolution",
            ));
        }
        Ok(regulator)
    }
    pub fn rated_shunt_input(&self, element: i64) -> Result<RatedShuntInput> {
        let table = match self.elements.get(&element).map(String::as_str) {
            Some("ShuntReactor") => "ShuntReactor",
            Some("ShuntCondensator") => "ShuntCondensator",
            _ => return Err(format_error("element is not a rated shunt bank")),
        };
        let state = self.element_state(element)?;
        let mut ports = self.electrical_terminals(element)?;
        if ports.len() != 1 || ports[0].position != 1 || ports[0].connection == Connection::Neutral
        {
            return Err(format_error(
                "rated shunt requires one phase or phase-pair port",
            ));
        }
        // SQL identifier is selected exclusively from the literals above.
        let mut statement = self
            .connection
            .prepare(&format!(
                "SELECT e.Flag_Input AS ElementInput,s.* FROM {table} s JOIN Element e
             ON e.Element_ID=s.Element_ID AND e.Variant_ID=s.Variant_ID
             WHERE s.Element_ID=?1 AND s.Variant_ID=?2"
            ))
            .map_err(format_error)?;
        let mut rows = statement
            .query([element, self.variant])
            .map_err(format_error)?;
        let row = rows
            .next()
            .map_err(format_error)?
            .ok_or_else(|| format_error("missing rated shunt row"))?;
        let categories = integer(row, "ElementInput")?;
        require_input_categories(categories, 2)?;
        Self::materialized_type(row)?;
        if self.newer_integer(row, "Flag_Lf", 1)? != 1 {
            return Err(format_error(
                "rated shunt requires the fixed impedance input mode",
            ));
        }
        let legacy = self.version.to_bits() == 11.5_f64.to_bits();
        let mut defaulted = Vec::new();
        let regulator = self.rated_shunt_controls(row, legacy, &mut defaulted)?;
        let (positive, at_nominal) =
            positive_admittance(row, table, regulator, legacy, &mut defaulted)?;
        let connection = ports[0].connection;
        let count = connection.phases().unwrap().len();
        let ground = default_flag(row, "Flag_Z0", legacy, &mut defaulted)?;
        if !matches!(ground, 0 | 1) || reference(row, "Stp_ID")?.is_some() {
            return Err(format_error(
                "rated shunt neutral impedance requires separate mapping",
            ));
        }
        // Single-phase and phase-pair port definitions specify the two physical
        // endpoints. Grounding/sequence selection applies to a three-phase star.
        let grounded = count == 1 || (count == 3 && ground == 1);
        let zero = if grounded && ground == 1 {
            require_input_categories(categories, 6)?;
            zero_admittance(row, table, positive, at_nominal)?
        } else {
            Complex64::new(0.0, 0.0)
        };
        if !zero.re.is_finite() || !zero.im.is_finite() {
            return Err(format_error(
                "nonfinite rated shunt zero-sequence admittance",
            ));
        }
        if count == 1
            && ground == 1
            && (zero - positive).norm() > 16.0 * f64::EPSILON * positive.norm()
        {
            return Err(format_error(
                "single-phase rated shunt with distinct zero sequence requires verified mapping",
            ));
        }
        if rows.next().map_err(format_error)?.is_some() {
            return Err(format_error("ambiguous rated shunt row"));
        }
        Ok(RatedShuntInput {
            element,
            terminal: ports.remove(0),
            state,
            positive,
            zero,
            grounded,
            defaulted,
        })
    }
}

fn zero_admittance(
    row: &Row<'_>,
    table: &str,
    positive: Complex64,
    at_nominal: bool,
) -> Result<Complex64> {
    let sign = if table == "ShuntReactor" { -1.0 } else { 1.0 };
    Ok(match integer(row, "Flag_Z0_Input")? {
        3 if table == "ShuntReactor" => positive,
        1 => {
            let ratio = number(row, "Z0_Z1")?;
            let rx = number(row, "R0_X0")?;
            if ratio <= 0.0 || -sign * rx < 0.0 {
                return Err(format_error("invalid rated shunt zero-sequence ratio"));
            }
            let magnitude = positive.norm() / ratio;
            if positive.norm() > 0.0 && magnitude == 0.0 {
                return Err(format_error(
                    "rated shunt zero-sequence admittance underflows",
                ));
            }
            let denominator = rx.hypot(1.0);
            Complex64::new(
                -sign * magnitude * (rx / denominator),
                sign * magnitude / denominator,
            )
        }
        2 if at_nominal => reciprocal(number(row, "R0")?, number(row, "X0")?)?,
        _ => {
            return Err(format_error(
                "rated shunt zero-sequence mode or stepped direct impedance requires separate mapping",
            ));
        }
    })
}

fn positive_admittance(
    row: &Row<'_>,
    table: &str,
    regulator: i64,
    legacy: bool,
    defaulted: &mut Vec<&'static str>,
) -> Result<(Complex64, bool)> {
    let rated = number(row, "Sn")?;
    let volts = number(row, "Un")?;
    if rated <= 0.0 || volts <= 0.0 {
        return Err(format_error("rated shunt requires positive Sn and Un"));
    }
    let current = if regulator == 1 {
        let position = number(row, "roh")?;
        let low = number(row, "rohl")?;
        let mid = number(row, "rohm")?;
        let high = number(row, "rohu")?;
        if !(low <= mid && mid <= high && low <= position && position <= high) {
            return Err(format_error(
                "rated shunt fixed step is outside its declared range",
            ));
        }
        rated + number(row, "deltaS")? * (position - mid)
    } else {
        rated
    };
    if !current.is_finite() || current < 0.0 {
        return Err(format_error("invalid rated shunt operating apparent power"));
    }
    let loss = if table == "ShuntReactor" {
        default_number(row, "Vcu", legacy, defaulted)?
            + default_number(row, "Vfe", legacy, defaulted)?
    } else {
        default_number(row, "Vdi", legacy, defaulted)?
    } / 1000.0;
    if !loss.is_finite() || loss > rated {
        return Err(format_error("rated shunt losses exceed apparent power"));
    }
    let power_factor = loss / rated;
    let reactive_factor = ((1.0 - power_factor) * (1.0 + power_factor)).sqrt();
    // MVA / kV² is already siemens. Divide in stages to avoid squaring Un.
    let scale = current / volts / volts;
    let sign = if table == "ShuntReactor" { -1.0 } else { 1.0 };
    let positive = Complex64::new(scale * power_factor, sign * scale * reactive_factor);
    if !scale.is_finite()
        || (current > 0.0 && scale == 0.0)
        || (loss > 0.0 && current > 0.0 && positive.re == 0.0)
        || (reactive_factor > 0.0 && current > 0.0 && positive.im == 0.0)
    {
        return Err(format_error(
            "rated shunt admittance overflows or underflows siemens",
        ));
    }
    Ok((positive, current.to_bits() == rated.to_bits()))
}

// Database Description (April 2014), pp.29–32: these optional fields default
// to zero. Only the acquired 11.5 layout admits NULL as that recorded default;
// missing columns, required ratings, modern NULLs and active step inputs reject.
fn default_flag(
    row: &Row<'_>,
    field: &'static str,
    legacy: bool,
    defaults: &mut Vec<&'static str>,
) -> Result<i64> {
    match row.get::<_, Option<i64>>(field).map_err(format_error)? {
        Some(v) => Ok(v),
        None if legacy => {
            defaults.push(field);
            Ok(0)
        }
        None => Err(format_error(format!(
            "NULL rated shunt {field} outside legacy profile"
        ))),
    }
}
fn default_number(
    row: &Row<'_>,
    field: &'static str,
    legacy: bool,
    defaults: &mut Vec<&'static str>,
) -> Result<f64> {
    match row.get::<_, Option<f64>>(field).map_err(format_error)? {
        Some(v) if v.is_finite() && v >= 0.0 => Ok(v),
        None if legacy => {
            defaults.push(field);
            Ok(0.0)
        }
        _ => Err(format_error(format!("invalid rated shunt {field}"))),
    }
}

impl RatedShuntInput {
    pub fn circuit(&self, native: &DistBus) -> Result<ShuntImpedanceCircuit> {
        let phases = self
            .terminal
            .connection
            .phases()
            .unwrap()
            .iter()
            .map(|p| (p + 1).to_string())
            .collect::<Vec<_>>();
        if native.id != self.terminal.node.to_string()
            || phases.iter().any(|p| !native.terminals.contains(p))
        {
            return Err(format_error(
                "rated shunt bus identity or phase map mismatch",
            ));
        }
        let count = phases.len();
        let mut terminals = phases.clone();
        if self.grounded {
            terminals.push("earth".into());
        }
        let n = terminals.len();
        let mut y = vec![vec![Complex64::new(0.0, 0.0); n]; n];
        if count == 2 {
            y[0][0] = self.positive;
            y[1][1] = self.positive;
            y[0][1] = -self.positive;
            y[1][0] = -self.positive;
        } else if count == 1 {
            let branch = 3.0 * self.positive;
            y[0][0] = branch;
            y[1][1] = branch;
            y[0][1] = -branch;
            y[1][0] = -branch;
        } else {
            for i in 0..3 {
                for j in 0..3 {
                    let value = (self.zero - self.positive) / 3.0
                        + if i == j {
                            self.positive
                        } else {
                            Complex64::new(0.0, 0.0)
                        };
                    y[i][j] = value;
                    if self.grounded {
                        y[i][3] -= value;
                        y[3][j] -= value;
                        y[3][3] += value;
                    }
                }
            }
        }
        if y.iter()
            .flatten()
            .any(|v| !v.re.is_finite() || !v.im.is_finite())
        {
            return Err(format_error("rated shunt conductor primitive overflows"));
        }
        let internal = format!("sincal:shunt:{}", self.element);
        let mut bus = DistBus::new(&internal, terminals.clone());
        if self.grounded {
            bus.grounded.push("earth".into());
        }
        let shunt = DistShunt::new(
            self.element.to_string(),
            &internal,
            terminals,
            y.iter().map(|r| r.iter().map(|v| v.re).collect()).collect(),
            y.iter().map(|r| r.iter().map(|v| v.im).collect()).collect(),
        );
        let switch = DistSwitch::new(
            format!("sincal:terminal:{}", self.terminal.id),
            &native.id,
            &internal,
            phases.clone(),
            phases,
            self.state == State::Off || self.terminal.state == State::Off,
        );
        Ok(ShuntImpedanceCircuit { bus, shunt, switch })
    }
}
