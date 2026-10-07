//! Static load projection through existing typed distribution records.
//!
//! The caller supplies an already materialized bus and, for phase-to-earth
//! branches, its explicit grounded terminal. Nothing silently grounds a native
//! neutral. Circuit assembly keeps native terminal switching explicit.

use super::{
    format_error,
    load::{LoadInput, LoadModel, LoadZeroSequence, PowerInput, VoltageInput},
    semantics::{Connection, State},
};
use crate::{Configuration, DistBus, DistLoad, DistLoadVoltageModel, DistSwitch, Result};

pub(super) struct LoadCircuit {
    pub bus: DistBus,
    pub load: DistLoad,
    pub switch: DistSwitch,
}

impl LoadInput {
    pub fn requires_earth(&self) -> Result<bool> {
        branch_powers(&self.power, self.terminal.connection).map(|(delta, _, _)| !delta)
    }

    pub fn connection_resolves_absent_sequence(&self) -> Result<bool> {
        // Input Data (April 2014), pp. 97–98 explicitly connects L1/L2/L3
        // to earth and L12/L23/L31 between phases. These two-terminal
        // connections need no inferred star when sequence input is absent.
        // Three-phase Wye still needs its separate star/sequence resolution;
        // active nontrivial sequence impedances must never be discarded.
        Ok(!self.requires_earth()?
            || matches!(
                self.terminal.connection,
                Connection::L1 | Connection::L2 | Connection::L3
            ))
    }

    pub fn lower_static(
        &self,
        bus: &DistBus,
        nominal_ll_volts: f64,
        earth_terminal: Option<&str>,
    ) -> Result<DistLoad> {
        if self.state != State::On || self.terminal.state != State::On {
            return Err(format_error(
                "load requires service/switch circuit lowering",
            ));
        }
        if bus.id != self.terminal.node.to_string() {
            return Err(format_error("load bus does not match native terminal node"));
        }
        self.lower_on_bus(bus, nominal_ll_volts, earth_terminal)
    }

    pub fn circuit(&self, native: &DistBus, nominal_ll_volts: f64) -> Result<LoadCircuit> {
        let phases: Vec<String> = self
            .terminal
            .connection
            .phases()
            .ok_or_else(|| format_error("neutral-only load"))?
            .iter()
            .map(|p| (p + 1).to_string())
            .collect();
        if native.id != self.terminal.node.to_string()
            || phases.iter().any(|p| !native.terminals.contains(p))
        {
            return Err(format_error("load bus identity or phase map mismatch"));
        }
        let internal = format!("sincal:load:{}", self.element);
        let mut bus = DistBus::new(internal.clone(), phases.clone());
        let earth = if self.requires_earth()? {
            bus.terminals.push("0".into());
            bus.grounded.push("0".into());
            Some("0")
        } else {
            None
        };
        let load = self.lower_on_bus(&bus, nominal_ll_volts, earth)?;
        let switch = DistSwitch::new(
            format!("sincal:terminal:{}", self.terminal.id),
            native.id.clone(),
            internal,
            phases.clone(),
            phases,
            self.terminal.state == State::Off,
        );
        Ok(LoadCircuit { bus, load, switch })
    }

    fn lower_on_bus(
        &self,
        bus: &DistBus,
        nominal_ll_volts: f64,
        earth_terminal: Option<&str>,
    ) -> Result<DistLoad> {
        if self.state != State::On {
            return Err(format_error(
                "inactive load requires service-state retention",
            ));
        }
        if self.operating_series.iter().any(Option::is_some) {
            return Err(format_error(
                "load requires an explicitly resolved profile state",
            ));
        }
        if self.neutral_point.is_some() {
            return Err(format_error(
                "load requires neutral-point circuit resolution",
            ));
        }
        let (delta, p_nom, q_nom) = branch_powers(&self.power, self.terminal.connection)?;
        if self.zero_sequence != LoadZeroSequence::SameAsPositive
            && !(self.connection_resolves_absent_sequence()?
                && self.zero_sequence == LoadZeroSequence::NotDeclared)
        {
            return Err(format_error(
                "load requires zero-sequence circuit resolution",
            ));
        }
        if self
            .negative_sequence_power
            .is_some_and(|s| s.re != 0.0 || s.im != 0.0)
        {
            return Err(format_error(
                "load requires negative-sequence power mapping",
            ));
        }
        let voltage = resolve_voltage(self.voltage, nominal_ll_volts)?;
        let connection = self.terminal.connection;
        let phases = connection
            .phases()
            .ok_or_else(|| format_error("neutral-only load"))?;
        let mut terminal_map: Vec<String> = phases.iter().map(|i| (i + 1).to_string()).collect();
        for terminal in &terminal_map {
            if !bus.terminals.contains(terminal) {
                return Err(format_error(format!(
                    "load references unavailable conductor {terminal}"
                )));
            }
        }
        if !p_nom.iter().chain(&q_nom).all(|v| v.is_finite()) {
            return Err(format_error("nonfinite load branch power"));
        }
        let branch_voltage = if delta {
            voltage
        } else {
            voltage / 3.0_f64.sqrt()
        };
        if branch_voltage <= 0.0 {
            return Err(format_error("load branch voltage underflow"));
        }
        if !delta {
            let earth = earth_terminal
                .ok_or_else(|| format_error("load requires an explicit earth terminal"))?;
            if !bus.terminals.iter().any(|t| t == earth)
                || !bus.grounded.iter().any(|t| t == earth)
                || terminal_map.iter().any(|t| t == earth)
            {
                return Err(format_error(
                    "load earth terminal is absent, floating, or an active phase",
                ));
            }
            terminal_map.push(earth.to_owned());
        }
        let configuration = if p_nom.len() == 1 {
            Configuration::SinglePhase
        } else if delta {
            Configuration::Delta
        } else {
            Configuration::Wye
        };
        let v_nom = vec![branch_voltage; p_nom.len()];
        let voltage_model = match self.model {
            LoadModel::Power => DistLoadVoltageModel::ConstantPower { v_nom },
            LoadModel::Current => DistLoadVoltageModel::ConstantCurrent { v_nom },
            LoadModel::Impedance => DistLoadVoltageModel::ConstantImpedance { v_nom },
        };
        let mut load = DistLoad::new(
            self.element.to_string(),
            bus.id.clone(),
            terminal_map,
            configuration,
            p_nom,
            q_nom,
        );
        load.voltage_model = voltage_model;
        if let Some(selection) = &self.profile_selection {
            load.extras.insert(
                "sincal_profile".into(),
                serde_json::to_value(selection).map_err(format_error)?,
            );
        }
        Ok(load)
    }
}

pub(super) fn resolve_voltage(input: VoltageInput, nominal_ll_volts: f64) -> Result<f64> {
    if !nominal_ll_volts.is_finite() || nominal_ll_volts <= 0.0 {
        return Err(format_error("invalid load nominal bus voltage"));
    }
    let volts = match input {
        VoltageInput::Relative(fraction) => fraction * nominal_ll_volts,
        VoltageInput::Absolute(volts) => volts,
    };
    if volts.is_finite() && volts > 0.0 {
        Ok(volts)
    } else {
        Err(format_error("invalid load resolved voltage"))
    }
}

fn branch_powers(power: &PowerInput, connection: Connection) -> Result<(bool, Vec<f64>, Vec<f64>)> {
    // Explicit per-phase arrays with reduced terminal selections need an
    // independently verified native precedence rule. Never discard powers.
    Ok(match power {
        PowerInput::Total { p, q } => {
            let count: u8 = if connection == Connection::L123 { 3 } else { 1 };
            (
                connection.phases().map_or(0, <[usize]>::len) == 2,
                vec![p / f64::from(count); usize::from(count)],
                vec![q / f64::from(count); usize::from(count)],
            )
        }
        PowerInput::DeltaTotal { p, q } => {
            let count: u8 = match connection.phases().map_or(0, <[usize]>::len) {
                3 => 3,
                2 => 1,
                _ => {
                    return Err(format_error(
                        "delta load requires a phase-pair or three phases",
                    ));
                }
            };
            (
                true,
                vec![p / f64::from(count); usize::from(count)],
                vec![q / f64::from(count); usize::from(count)],
            )
        }
        PowerInput::Wye { p, q } if connection == Connection::L123 => {
            (false, p.to_vec(), q.to_vec())
        }
        PowerInput::Delta { p, q } if connection == Connection::L123 => {
            (true, p.to_vec(), q.to_vec())
        }
        _ => {
            return Err(format_error(
                "per-phase load inputs with reduced terminal selection require verified mapping",
            ));
        }
    })
}
