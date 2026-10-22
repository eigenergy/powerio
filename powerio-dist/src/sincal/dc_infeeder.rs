//! Static, directly connected converter generators. Input Data (April 2014),
//! printed pp. 82–83 and 87: total scaled P/Q, phase/phase-pair/wye ports,
//! solid earth reference and Z0=Z1. Fault/inverter-control models are separate.

use super::{
    format_error,
    schema::{NativeDatabase, require_table},
    semantics::{Connection, ElectricalTerminal, State, require_input_categories},
    transformer::{integer, number, reference},
};
use crate::{Configuration, DistBus, DistGenerator, DistSwitch, Result};

pub(super) struct DcInfeederInput {
    pub element: i64,
    pub terminal: ElectricalTerminal,
    pub state: State,
    pub kind: i64,
    pub watts: f64,
    pub vars: f64,
}

pub(super) struct DcInfeederCircuit {
    pub bus: DistBus,
    pub generator: DistGenerator,
    pub switch: DistSwitch,
}

impl NativeDatabase {
    pub fn dc_infeeder_input(&self, element: i64) -> Result<DcInfeederInput> {
        if self.elements.get(&element).map(String::as_str) != Some("DCInfeeder") {
            return Err(format_error(format!(
                "Element {element} is not a DCInfeeder"
            )));
        }
        let mut ports = self.electrical_terminals(element)?;
        if ports.len() != 1 || ports[0].position != 1 || ports[0].connection == Connection::Neutral
        {
            return Err(format_error(
                "DC infeeder requires one phase or phase-pair port",
            ));
        }
        require_table(
            &self.connection,
            "DCInfeeder",
            &["Element_ID", "Variant_ID"],
        )?;
        require_table(
            &self.connection,
            "VoltageLevel",
            &["VoltLevel_ID", "Variant_ID", "Flag_DCInfeeder"],
        )?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT e.Flag_Input AS ElementInput, v.Flag_DCInfeeder AS IgnoreConverters, d.* FROM DCInfeeder d JOIN Element e
             ON e.Element_ID=d.Element_ID AND e.Variant_ID=d.Variant_ID
             LEFT JOIN VoltageLevel v ON v.VoltLevel_ID=e.VoltLevel_ID AND v.Variant_ID=e.Variant_ID
             WHERE d.Element_ID=?1 AND d.Variant_ID=?2",
            )
            .map_err(format_error)?;
        let mut rows = statement
            .query([element, self.variant])
            .map_err(format_error)?;
        let row = rows
            .next()
            .map_err(format_error)?
            .ok_or_else(|| format_error("missing DCInfeeder row"))?;
        let kind = require_static_mode(row)?;
        let watts = scaled_power(number(row, "P")?, number(row, "fP")?)?;
        let vars = scaled_power(number(row, "Q")?, number(row, "fQ")?)?;
        if rows.next().map_err(format_error)?.is_some() {
            return Err(format_error("ambiguous DCInfeeder row"));
        }
        Ok(DcInfeederInput {
            element,
            terminal: ports.remove(0),
            state: self.element_state(element)?,
            kind,
            watts,
            vars,
        })
    }
}

fn require_static_mode(row: &rusqlite::Row<'_>) -> Result<i64> {
    require_input_categories(integer(row, "ElementInput")?, 2)?;
    if integer(row, "IgnoreConverters")? != 0 {
        return Err(format_error(
            "voltage level ignores DC infeeders; inactive-equipment retention required",
        ));
    }
    if integer(row, "Flag_Lf")? != 1 || integer(row, "Flag_Connect")? != 1 {
        return Err(format_error(
            "DC infeeder requires direct connection and P/Q input mode",
        ));
    }
    let kind = integer(row, "Flag_DCtyp")?;
    if !(1..=7).contains(&kind) {
        return Err(format_error("unknown DC infeeder type"));
    }
    for field in [
        "Typ_ID",
        "Mpl_ID",
        "Macro_ID",
        "EnergyStorage_ID",
        "DayOpSer_ID",
        "WeekOpSer_ID",
        "YearOpSer_ID",
        "PowerLimit_ID",
        "TransformerTap_ID",
        "Qctrl_U_P_ID",
        "Qctrl_PF_U_ID",
        "Qctrl_PF_P_ID",
        "Qctrl_P_Q_ID",
        "Qctrl_U_Q_ID",
        "Node_ID",
        "MasterElm_ID",
        "HarCur_ID",
        "HarVolt_ID",
    ] {
        if reference(row, field)?.is_some() {
            return Err(format_error(format!(
                "DC infeeder requires resolution of {field}"
            )));
        }
    }
    for field in [
        "Flag_Typ_ID",
        "IslandOp",
        "Flag_Macro",
        "Flag_Converter",
        "Flag_LfLimit",
        "Flag_LimitType",
        "Flag_Pctrl",
        "Flag_Qctrl",
        "Flag_CtrlPrior",
        "Flag_ShdU",
        "Flag_ShdP",
    ] {
        if integer(row, field)? != 0 {
            return Err(format_error(format!("unsupported DC infeeder {field}")));
        }
    }
    // These newer modes are admitted only at the values observed in the
    // schema-14.8 fixture. Their other meanings are not inferred by name.
    if integer(row, "Flag_ChkType")? != 1 || integer(row, "Flag_I")? != 1 {
        return Err(format_error("unsupported DC infeeder check/current mode"));
    }
    for field in ["Rlf", "Xlf", "Kr", "Ireg", "pk", "Sn_Inverter"] {
        if number(row, field)? != 0.0 {
            return Err(format_error(format!("unresolved DC infeeder {field}")));
        }
    }
    for field in ["Flag_LA", "Boost_Idc"] {
        let value: Option<f64> = row.get(field).map_err(format_error)?;
        if value.is_some_and(|v| v != 0.0) {
            return Err(format_error(format!("unresolved DC infeeder {field}")));
        }
    }
    Ok(kind)
}

fn scaled_power(value: f64, factor: f64) -> Result<f64> {
    let scaled = value * factor * 1e6;
    if !scaled.is_finite() || (value != 0.0 && factor != 0.0 && scaled == 0.0) {
        return Err(format_error(
            "DC infeeder power overflows or underflows SI units",
        ));
    }
    Ok(scaled)
}

impl DcInfeederInput {
    pub fn circuit(&self, native: &DistBus) -> Result<DcInfeederCircuit> {
        if self.state != State::On {
            return Err(format_error(
                "inactive DC infeeder requires service-state mapping",
            ));
        }
        let connection = self.terminal.connection;
        let indices = connection
            .phases()
            .ok_or_else(|| format_error("neutral-only DC infeeder"))?;
        let phases: Vec<String> = indices.iter().map(|p| (p + 1).to_string()).collect();
        if native.id != self.terminal.node.to_string()
            || phases.iter().any(|p| !native.terminals.contains(p))
        {
            return Err(format_error(
                "DC infeeder bus identity or phase map mismatch",
            ));
        }
        let wye = phases.len() == 3;
        let earth = phases.len() != 2;
        let (branches, divisor) = if wye { (3, 3.0) } else { (1, 1.0) };
        for power in [self.watts, self.vars] {
            if !power.is_finite() || (power != 0.0 && power / divisor == 0.0) {
                return Err(format_error("invalid DC infeeder branch power"));
            }
        }
        let mut terminals = phases.clone();
        if earth {
            terminals.push("0".into());
        }
        let internal = format!("sincal:dc-infeeder:{}", self.element);
        let mut bus = DistBus::new(internal.clone(), terminals.clone());
        if earth {
            bus.grounded.push("0".into());
        }
        let mut generator = DistGenerator::new(
            self.element.to_string(),
            internal.clone(),
            terminals,
            if wye {
                Configuration::Wye
            } else {
                Configuration::SinglePhase
            },
            vec![self.watts / divisor; branches],
            vec![self.vars / divisor; branches],
        );
        generator
            .extras
            .insert("sincal_dc_type".into(), self.kind.into());
        let switch = DistSwitch::new(
            format!("sincal:terminal:{}", self.terminal.id),
            native.id.clone(),
            internal,
            phases.clone(),
            phases,
            self.terminal.state == State::Off,
        );
        Ok(DcInfeederCircuit {
            bus,
            generator,
            switch,
        })
    }
}
