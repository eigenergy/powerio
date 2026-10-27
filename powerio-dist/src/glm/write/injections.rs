//! Fixed injections projected to signed loads using current typed values.
use super::{Writer, unsupported};
use crate::{Configuration, DistLoad, DistLocationTarget, IbrTopology};
use powerio_core::Error;

impl Writer<'_> {
    pub(super) fn static_injections(&mut self) -> Result<(), Error> {
        for generator in self.net.generators() {
            if !generator.extras.is_empty() {
                return Err(unsupported(format!(
                    "generator {} has an unresolved voltage/current law or electrical extras",
                    generator.name
                )));
            }
            let load = DistLoad::new(
                &generator.name,
                &generator.bus,
                generator.terminal_map.clone(),
                generator.configuration,
                generator.p_nom.iter().map(|p| -p).collect(),
                generator.q_nom.iter().map(|q| -q).collect(),
            );
            self.injection_load(&load, DistLocationTarget::Generator)?;
            if generator.p_min.is_some()
                || generator.p_max.is_some()
                || generator.q_min.is_some()
                || generator.q_max.is_some()
                || generator.s_max.is_some()
                || generator.i_max.is_some()
                || generator.cost.is_some()
            {
                self.dropped(format!("generator {}: optimization bounds, ratings and costs have no fields on the signed constant-power load equivalent",generator.name));
            }
        }
        for inverter in self.net.ibrs() {
            if !inverter.extras.is_empty()
                || inverter.control_profile.is_some()
                || inverter.voltage_aggregation.is_some()
            {
                return Err(unsupported(format!(
                    "inverter {} has an unresolved voltage/current law or control profile",
                    inverter.name
                )));
            }
            let (Some(p), Some(pmax), Some(q), Some(qmax)) = (
                &inverter.p_min,
                &inverter.p_max,
                &inverter.q_min,
                &inverter.q_max,
            ) else {
                return Err(unsupported(format!(
                    "inverter {} needs explicit fixed P/Q bounds; available power does not specify dispatch",
                    inverter.name
                )));
            };
            if p != pmax || q != qmax || p.is_empty() || p.len() != q.len() {
                return Err(unsupported(format!(
                    "inverter {} needs explicit fixed per-connection P/Q values",
                    inverter.name
                )));
            }
            let configuration = match inverter.topology {
                IbrTopology::SinglePhase if inverter.terminal_map.len() == 2 && p.len() == 1 => {
                    Configuration::SinglePhase
                }
                IbrTopology::FourLeg if inverter.terminal_map.last().is_some_and(|n| n == "n") => {
                    Configuration::Wye
                }
                _ => {
                    return Err(unsupported(format!(
                        "inverter {} needs another conductor/topology mapping",
                        inverter.name
                    )));
                }
            };
            let load = DistLoad::new(
                &inverter.name,
                &inverter.bus,
                inverter.terminal_map.clone(),
                configuration,
                p.iter().map(|p| -p).collect(),
                q.iter().map(|q| -q).collect(),
            );
            self.injection_load(&load, DistLocationTarget::Ibr)?;
            self.dropped(format!("inverter {}: prime mover, available power, ratings and optimization limits have no fields on the signed constant-power load equivalent; current fixed dispatch is unchanged",inverter.name));
        }
        Ok(())
    }

    fn injection_load(&mut self, load: &DistLoad, target: DistLocationTarget) -> Result<(), Error> {
        if self.services.buses.contains_key(load.bus.as_str()) {
            self.service_load_with_target(load, target)?;
        } else {
            self.pair_load_with_target(load, target)?;
        }
        self.diagnostics.push(&crate::diagnostics::codes::EMIT_GLM_VALUE_REBASED,format!("{target:?} {}: current fixed P/Q injection becomes a signed constant-power load; reimport retains its electrical law and point under the load component identity",load.name));
        Ok(())
    }
}
