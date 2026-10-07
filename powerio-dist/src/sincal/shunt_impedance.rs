//! Constant ohmic shunts: April 2014 Input Data, printed pp. 121–122.
//! A star point belongs to the device, never to an unrelated bus neutral.

use num_complex::Complex64;

use super::{
    format_error,
    schema::NativeDatabase,
    semantics::{Connection, ElectricalTerminal, State, require_input_categories},
    transformer::{integer, number, reference},
};
use crate::{DistBus, DistShunt, DistSwitch, Result};

pub(super) struct ShuntImpedanceInput {
    element: i64,
    pub terminal: ElectricalTerminal,
    grounded: bool,
    admittance: Complex64,
}

pub(super) struct ShuntImpedanceCircuit {
    pub bus: DistBus,
    pub shunt: DistShunt,
    pub switch: DistSwitch,
}

impl NativeDatabase {
    pub fn shunt_impedance_input(&self, element: i64) -> Result<ShuntImpedanceInput> {
        if self.elements.get(&element).map(String::as_str) != Some("ShuntImpedance") {
            return Err(format_error("element is not a ShuntImpedance"));
        }
        if self.element_state(element)? != State::On {
            return Err(format_error(
                "inactive shunt requires service-state mapping",
            ));
        }
        let mut ports = self.electrical_terminals(element)?;
        if ports.len() != 1 || ports[0].position != 1 || ports[0].connection == Connection::Neutral
        {
            return Err(format_error("shunt requires one phase or phase-pair port"));
        }
        let mut statement = self
            .connection
            .prepare(
                "SELECT e.Flag_Input AS ElementInput, s.* FROM ShuntImpedance s JOIN Element e
             ON e.Element_ID=s.Element_ID AND e.Variant_ID=s.Variant_ID
             WHERE s.Element_ID=?1 AND s.Variant_ID=?2",
            )
            .map_err(format_error)?;
        let mut rows = statement
            .query([element, self.variant])
            .map_err(format_error)?;
        let row = rows
            .next()
            .map_err(format_error)?
            .ok_or_else(|| format_error("missing ShuntImpedance row"))?;
        require_input_categories(integer(row, "ElementInput")?, 6)?;
        // The authentic schema-14.8 table adds a selector absent from the
        // April 2014 documentation. Neither its values nor a NULL default
        // establish the old R/X mode. Keep that layout explicitly gated.
        if row.as_ref().column_index("Flag_Lf").is_ok() {
            return Err(format_error(
                "shunt Flag_Lf layout requires documented input-mode semantics",
            ));
        }
        if row.as_ref().column_index("Typ_ID").is_ok() && reference(row, "Typ_ID")?.is_some() {
            return Err(format_error("shunt requires type-reference resolution"));
        }
        if row.as_ref().column_index("Flag_Typ_ID").is_ok() && integer(row, "Flag_Typ_ID")? != 0 {
            return Err(format_error("shunt requires type-reference resolution"));
        }
        if !matches!(integer(row, "Flag_I")?, 0 | 1) || number(row, "Ireg")? != 0.0 {
            return Err(format_error(
                "shunt current-control mode requires separate mapping",
            ));
        }
        if integer(row, "Flag_Macro")? != 0 {
            return Err(format_error("shunt macro requires separate mapping"));
        }
        for field in ["Stp_ID", "Macro_ID"] {
            if reference(row, field)?.is_some() {
                return Err(format_error(format!(
                    "shunt requires resolution of {field}"
                )));
            }
        }
        // Harmonic and transient inputs do not alter this fundamental-frequency
        // constant-impedance profile. Native source retention preserves them.
        let grounded = match integer(row, "Flag_Z0")? {
            0 => false,
            1 if integer(row, "Flag_Z0_Input")? == 3 => true,
            _ => {
                return Err(format_error(
                    "shunt requires ungrounded or solid Z0=Z1 mapping",
                ));
            }
        };
        let phases = ports[0]
            .connection
            .phases()
            .expect("phase port checked")
            .len();
        if (phases == 1 && !grounded) || (phases == 2 && grounded) {
            return Err(format_error(
                "unsupported shunt connection/grounding combination",
            ));
        }
        let admittance = reciprocal(number(row, "R")?, number(row, "X")?)?;
        if rows.next().map_err(format_error)?.is_some() {
            return Err(format_error("ambiguous ShuntImpedance row"));
        }
        Ok(ShuntImpedanceInput {
            element,
            terminal: ports.remove(0),
            grounded,
            admittance,
        })
    }
}

fn reciprocal(r: f64, x: f64) -> Result<Complex64> {
    let scale = r.abs().max(x.abs());
    if r < 0.0 || scale == 0.0 {
        return Err(format_error(
            "shunt requires nonnegative resistance and nonzero impedance",
        ));
    }
    // Scaling avoids squaring large or tiny ohmic inputs.
    let scaled_r = r / scale;
    let scaled_x = x / scale;
    let denominator = scaled_r * scaled_r + scaled_x * scaled_x;
    let y = Complex64::new(
        (scaled_r / denominator) / scale,
        (-scaled_x / denominator) / scale,
    );
    if !y.re.is_finite()
        || !y.im.is_finite()
        || (r != 0.0 && y.re == 0.0)
        || (x != 0.0 && y.im == 0.0)
    {
        return Err(format_error(
            "shunt admittance overflows or underflows siemens",
        ));
    }
    Ok(y)
}

impl ShuntImpedanceInput {
    pub fn circuit(&self, native: &DistBus) -> Result<ShuntImpedanceCircuit> {
        let phases: Vec<String> = self
            .terminal
            .connection
            .phases()
            .ok_or_else(|| format_error("neutral-only shunt"))?
            .iter()
            .map(|p| (p + 1).to_string())
            .collect();
        if native.id != self.terminal.node.to_string()
            || phases.iter().any(|p| !native.terminals.contains(p))
        {
            return Err(format_error("shunt bus identity or phase map mismatch"));
        }
        let pair = phases.len() == 2;
        let mut terminals = phases.clone();
        if !pair {
            terminals.push("star".into());
        }
        let internal = format!("sincal:shunt:{}", self.element);
        let mut bus = DistBus::new(internal.clone(), terminals.clone());
        if self.grounded {
            bus.grounded.push("star".into());
        }
        let n = terminals.len();
        let mut g = vec![vec![0.0; n]; n];
        let mut b = g.clone();
        let branches = if pair { 1 } else { phases.len() };
        for i in 0..branches {
            let j = n - 1;
            for (matrix, y) in [(&mut g, self.admittance.re), (&mut b, self.admittance.im)] {
                matrix[i][i] += y;
                matrix[j][j] += y;
                matrix[i][j] -= y;
                matrix[j][i] -= y;
            }
        }
        if g.iter().chain(&b).flatten().any(|v| !v.is_finite()) {
            return Err(format_error("shunt primitive overflows siemens"));
        }
        let shunt = DistShunt::new(self.element.to_string(), internal.clone(), terminals, g, b);
        let switch = DistSwitch::new(
            format!("sincal:terminal:{}", self.terminal.id),
            native.id.clone(),
            internal,
            phases.clone(),
            phases,
            self.terminal.state == State::Off,
        );
        Ok(ShuntImpedanceCircuit { bus, shunt, switch })
    }
}
