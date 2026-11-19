use super::{Writer, complex_literal, flags, phases, unsupported};
use crate::glm::connections::{EXPLICIT, reference};
use crate::{Configuration, DistLoad, DistLoadVoltageModel, DistLocationTarget};
use num_complex::Complex64;
use powerio_core::Error;
use std::fmt::Write as _;

impl Writer<'_> {
    pub(super) fn pair_load(&mut self, load: &DistLoad) -> Result<(), Error> {
        self.pair_load_with_target(load, DistLocationTarget::Load)
    }

    pub(super) fn pair_load_with_target(
        &mut self,
        load: &DistLoad,
        target: DistLocationTarget,
    ) -> Result<(), Error> {
        let connections = connections(load, self.neutral_last(&load.bus, &load.terminal_map))?;
        let active = phases(&self.phase_terminals(&load.bus, &load.terminal_map))?;
        let available = self
            .phases
            .get(load.bus.as_str())
            .ok_or_else(|| unsupported(format!("load {} has no exact bus identity", load.name)))?;
        if active.iter().any(|p| !available.contains(p))
            || !load.extras.is_empty()
            || load.p_nom.len() != connections.len()
            || load.q_nom.len() != connections.len()
        {
            return Err(unsupported(format!(
                "load {} has incomplete terminal/power vectors or uninterpreted extras",
                load.name
            )));
        }
        let (law, nominal) = match &load.voltage_model {
            DistLoadVoltageModel::ConstantPower { v_nom } => ("power", v_nom),
            DistLoadVoltageModel::ConstantCurrent { v_nom } => ("current", v_nom),
            DistLoadVoltageModel::ConstantImpedance { v_nom } => ("impedance", v_nom),
            _ => {
                return Err(unsupported(
                    "phase-pair load needs another voltage-law mapping",
                ));
            }
        };
        if law != "power"
            && (nominal.len() != connections.len()
                || nominal.iter().any(|v| !v.is_finite() || *v <= 0.0))
        {
            return Err(unsupported(
                "phase-pair load needs positive finite per-connection nominal voltages",
            ));
        }
        let _ = writeln!(
            self.out,
            "object load {{ name \"{}\"; parent \"{}\"; phases {}N; nominal_voltage {};",
            load.name,
            load.bus,
            flags(&active),
            self.nominal[load.bus.as_str()]
        );
        self.coordinates(target, &load.name);
        for (i, (a, b)) in connections.iter().enumerate() {
            let (suffix, from, to) = EXPLICIT
                .into_iter()
                .find(|(_, from, to)| {
                    let x = (from + 1).to_string();
                    let y = to.map_or_else(|| "n".to_owned(), |p| (p + 1).to_string());
                    (a.as_str() == x && b.as_str() == y) || (a.as_str() == y && b.as_str() == x)
                })
                .ok_or_else(|| unsupported("phase-pair load has no native explicit connection"))?;
            if a.as_str() != (from + 1).to_string() {
                self.diagnostics.push(&crate::diagnostics::codes::EMIT_GLM_VALUE_REBASED,
                    format!("load {} channel {i}: terminal orientation normalized to {suffix}; its two-terminal power law is unchanged", load.name));
            }
            let power = Complex64::new(load.p_nom[i], load.q_nom[i]);
            let base = reference(1.0, from, to);
            let value = crate::load_voltage::calc_zip_coefficient(
                crate::glm::read::zip_law(law),
                power,
                base / base.norm(),
                // A constant power law states no nominal voltage.
                nominal.get(i).copied().unwrap_or(f64::NAN),
            );
            if power != Complex64::default() && value == Complex64::default() {
                return Err(unsupported(
                    "phase-pair load coefficient underflows to zero",
                ));
            }
            let _ = writeln!(
                self.out,
                "    constant_{law}_{suffix} {};",
                complex_literal(value)?
            );
        }
        self.out.push_str("}\n");
        if connections.len() > 1 {
            self.diagnostics.push(&crate::diagnostics::codes::EMIT_GLM_VALUE_REBASED,
                format!("load {}: delta channels emitted as explicit phase-pair properties; reading separates their component identities", load.name));
        }
        Ok(())
    }
}

/// The terminal pairs of a load's connections; `neutral_last` states
/// whether its WYE neutral is its last terminal, where GridLAB-D states it.
fn connections(load: &DistLoad, neutral_last: bool) -> Result<Vec<(&String, &String)>, Error> {
    let map = load.terminal_map.as_slice();
    let neutral_last = map.len() >= 2 && neutral_last;
    let channels = match load.configuration {
        Configuration::Wye if !neutral_last => None,
        Configuration::Wye => {
            crate::calc_connection_channels(load.configuration, map.len(), map.len() - 1, None).ok()
        }
        configuration => {
            crate::calc_connection_channels(configuration, map.len(), load.p_nom.len(), None).ok()
        }
    };
    let result = channels
        .and_then(|channels| {
            channels
                .iter()
                .map(|channel| channel.to.map(|to| (&map[channel.from], &map[to])))
                .collect::<Option<Vec<_>>>()
        })
        .ok_or_else(|| {
            unsupported(format!(
                "load {} needs an additional connection model",
                load.name
            ))
        })?;
    if load.configuration == Configuration::Wye
        && result
            .iter()
            .map(|(p, _)| *p)
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != result.len()
    {
        return Err(unsupported("wye injection has repeated phase connections"));
    }
    Ok(result)
}
