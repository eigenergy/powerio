//! Validate the typed boundary before indexing conductor vectors. Readiness
//! covers line primitives, but does not yet validate every load/switch/source.
use super::write::{Result, error};
use crate::{Configuration, MulticonductorNetwork};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn validate(net: &MulticonductorNetwork) -> Result<()> {
    if !net.base_frequency().is_finite() || net.base_frequency() <= 0.0 {
        return Err(error("invalid network frequency"));
    }
    let buses = net
        .buses()
        .iter()
        .map(|b| (b.id.as_str(), b))
        .collect::<BTreeMap<_, _>>();
    for bus in net.buses() {
        if bus.terminals.iter().collect::<BTreeSet<_>>().len() != bus.terminals.len()
            || bus.grounded.iter().any(|g| !bus.terminals.contains(g))
        {
            return Err(error(format!(
                "bus {}: duplicate terminals or undeclared ground",
                bus.id
            )));
        }
    }
    let port = |bus: &str, terminals: &[String]| -> Result<()> {
        let declared = buses
            .get(bus)
            .ok_or_else(|| error(format!("unresolved exact bus reference {bus}")))?;
        if terminals.is_empty()
            || terminals.iter().collect::<BTreeSet<_>>().len() != terminals.len()
            || terminals.iter().any(|t| !declared.terminals.contains(t))
        {
            return Err(error(format!(
                "bus {bus}: duplicate, empty or undeclared port terminals"
            )));
        }
        Ok(())
    };
    for source in net.sources() {
        port(&source.bus, &source.terminal_map)?;
        let n = source.terminal_map.len();
        if source.v_magnitude.len() != n
            || source.v_angle.len() != n
            || source
                .v_magnitude
                .iter()
                .chain(&source.v_angle)
                .any(|v| !v.is_finite())
        {
            return Err(error("source phasor vectors must be complete and finite"));
        }
        if let Some(reference) = &source.reference_terminal
            && (!buses[source.bus.as_str()].terminals.contains(reference)
                || source.terminal_map.contains(reference))
        {
            return Err(error("source reference is missing or overlaps a phase"));
        }
    }
    for load in net.loads() {
        port(&load.bus, &load.terminal_map)?;
        validate_load(load)?;
    }
    for transformer in net.transformers() {
        for winding in &transformer.windings {
            port(&winding.bus, &winding.terminal_map)?;
        }
    }
    for switch in net.switches() {
        port(&switch.bus_from, &switch.terminal_map_from)?;
        port(&switch.bus_to, &switch.terminal_map_to)?;
        if switch.terminal_map_from.len() != switch.terminal_map_to.len() {
            return Err(error("switch port lengths differ"));
        }
    }
    for line in net.lines() {
        port(&line.bus_from, &line.terminal_map_from)?;
        port(&line.bus_to, &line.terminal_map_to)?;
        let code = net
            .line_codes()
            .iter()
            .find(|c| c.name == line.linecode)
            .ok_or_else(|| error("unresolved exact linecode reference"))?;
        for limits in [line.i_max.as_ref(), code.i_max.as_ref()]
            .into_iter()
            .flatten()
        {
            if limits.len() != code.n_conductors {
                return Err(error(
                    "line ampacity vector does not match its conductor count",
                ));
            }
        }
    }
    Ok(())
}

fn validate_load(load: &crate::DistLoad) -> Result<()> {
    let n = load.p_nom.len();
    let count = match load.configuration {
        Configuration::Wye => load.terminal_map.len().checked_sub(1),
        Configuration::SinglePhase => (load.terminal_map.len() == 2).then_some(1),
        Configuration::Delta => match load.terminal_map.len() {
            2 => Some(1),
            3 => Some(3),
            _ => None,
        },
    };
    if !(1..=3).contains(&n)
        || count != Some(n)
        || load.q_nom.len() != n
        || load.p_nom.iter().chain(&load.q_nom).any(|v| !v.is_finite())
    {
        return Err(error("load branch vectors do not match its connection"));
    }
    let voltages = load.voltage_model.v_nom();
    if !voltages.is_empty()
        && (voltages.len() != n || voltages.iter().any(|v| !v.is_finite() || *v <= 0.0))
    {
        return Err(error("invalid load nominal voltage vector"));
    }
    Ok(())
}
