//! Explicit positive-sequence SINCAL adapter. Three-phase terminal codes are
//! validated *after* the caller selects this profile; they never select it.
//! Electrical meanings belong here, not in the shared acquisition crate.

#[cfg(test)]
mod access_tests;
mod equipment;
mod profile;
mod rows;
mod settings;
pub(super) mod source;
#[cfg(test)]
mod tests;

use crate::Result;
use crate::network::{BalancedNetwork, Bus, BusId, BusType, SourceFormat, Switch};
use powerio_sincal::DatabaseSnapshot;
use rows::{NativeRow, get, table};
use settings::StaticProfiles;
use std::collections::BTreeMap;

fn error(message: impl std::fmt::Display) -> crate::Error {
    crate::Error::FormatRead {
        format: "sincal",
        message: message.to_string(),
    }
}

fn bus_id(id: i64) -> Result<BusId> {
    usize::try_from(id)
        .map(BusId)
        .map_err(|_| error(format!("invalid native Node_ID {id}")))
}

/// Internal explicit-family entry point. Acquisition has already validated
/// native identities. This is not automatic format dispatch, and accepting
/// a schema structurally does not authorize its electrical interpretation.
///
/// # Errors
/// Unsupported active input, inconsistent topology, or invalid electrical data.
pub fn read_balanced_snapshot(db: &DatabaseSnapshot, name: &str) -> Result<BalancedNetwork> {
    read_balanced_snapshot_at(db, name, None)
}

/// Read a selected daily snapshot; no implicit profile timestamp is chosen.
///
/// # Errors
/// Unsupported electrical/profile semantics or invalid source input.
pub fn read_balanced_snapshot_at(
    db: &DatabaseSnapshot,
    name: &str,
    hours: Option<f64>,
) -> Result<BalancedNetwork> {
    profile::validate_time(hours)?;
    if hours.is_some() && db.version.to_bits() != 11.5_f64.to_bits() {
        return Err(error(
            "selected daily snapshots currently require schema 11.5",
        ));
    }
    if ![11.5_f64, 14.8, 15.5, 16.0]
        .iter()
        .any(|v| v.to_bits() == db.version.to_bits())
    {
        return Err(error(format!(
            "balanced electrical adapter does not yet support schema {}",
            db.version
        )));
    }
    let settings = table(db, "CalcParameter", "CalcParameter_ID")?;
    if settings.len() != 1 {
        return Err(error("expected one CalcParameter record"));
    }
    let settings = settings
        .values()
        .next()
        .ok_or_else(|| error("missing calculation settings"))?;
    let profiles = settings::validate(db, settings, hours)?;
    let frequency = settings.positive("f")?;
    let levels = table(db, "VoltageLevel", "VoltLevel_ID")?;
    let nodes = table(db, "Node", "Node_ID")?;
    // SINCAL's physical units do not supply a system MVA base. 100 MVA is
    // an internal conversion basis; it is not an inferred native datum.
    let mut network = BalancedNetwork::new(name, 100.0);
    *network.source_format_mut() = SourceFormat::Sincal;
    *network.base_frequency_mut() = frequency;
    read_buses(&nodes, &levels, settings, frequency, &mut network)?;
    read_elements(db, &levels, &profiles, &mut network)?;
    network.check_references("sincal")?;
    Ok(network)
}

fn read_buses(
    nodes: &BTreeMap<i64, NativeRow>,
    levels: &BTreeMap<i64, NativeRow>,
    settings: &NativeRow,
    frequency: f64,
    network: &mut BalancedNetwork,
) -> Result<()> {
    for (&id, node) in nodes {
        node.equals("Flag_Volt", 0)?;
        node.inactive(&["RefNode_ID", "Stp_ID"])?;
        let level = get(levels, node.integer("VoltLevel_ID")?, "VoltageLevel")?;
        let default_voltage = level.legacy() && level.is_null("Flag_Volt");
        if !default_voltage {
            level.equals("Flag_Volt", 1)?;
        }
        if level.positive("f")?.to_bits() != frequency.to_bits() {
            return Err(level.bad("f", "mixed frequencies"));
        }
        let mut bus = Bus::new(bus_id(id)?, BusType::Pq, level.positive("Un")?);
        if default_voltage {
            bus.extras.insert(
                "sincal_voltage_basis".into(),
                serde_json::json!({
                    "voltage_level": node.integer("VoltLevel_ID")?,
                    "defaulted": "Flag_Volt", "value": 1, "schema": level.schema
                }),
            );
        }
        bus.name = Some(node.text("Name")?);
        bus.uid = Some(format!("sincal:node:{id}"));
        bus.vmin = settings.positive("ull")? / 100.0;
        bus.vmax = settings.positive("uul")? / 100.0;
        if bus.vmin > bus.vmax {
            return Err(settings.bad("ull/uul", "inverted limits"));
        }
        // Native nonzero start voltage is absolute kV, not the nominal base.
        // Respect Flat Start; values in the inactive fields remain in source.
        let start = node.nonnegative("Un")?;
        let angle = node.number("Phi")?;
        if !settings.state("Flag_ABW")? && start > 0.0 {
            bus.vm = start / bus.base_kv;
            if !bus.vm.is_finite() || bus.vm <= 0.0 {
                return Err(node.bad("Un", "initial per-unit voltage overflows or underflows"));
            }
            bus.va = angle;
        }
        node.inactive(&["Uul", "Ull"])?;
        network.buses_mut().push(bus);
    }
    Ok(())
}

fn read_elements(
    db: &DatabaseSnapshot,
    levels: &BTreeMap<i64, NativeRow>,
    profiles: &StaticProfiles,
    network: &mut BalancedNetwork,
) -> Result<()> {
    let elements = table(db, "Element", "Element_ID")?;
    let terminals = table(db, "Terminal", "Terminal_ID")?;
    let kinds = [
        "Line",
        "Load",
        "Infeeder",
        "DCInfeeder",
        "TwoWindingTransformer",
        "ShuntCondensator",
    ];
    let mut data = BTreeMap::new();
    for kind in kinds {
        if db.elements.values().any(|k| k == kind) {
            let records = table(db, kind, "Element_ID")?;
            for &id in records.keys() {
                if db.elements.get(&id).map(String::as_str) != Some(kind) {
                    return Err(error(format!("orphan/mistyped {kind}[{id}]")));
                }
            }
            data.insert(kind, records);
        }
    }
    for (&id, kind) in &db.elements {
        let element = get(&elements, id, "Element")?;
        if element.integer("Flag_Input")? & 2 == 0 {
            return Err(element.bad("Flag_Input", "load-flow inputs absent"));
        }
        let in_service = element.state("Flag_State")?;
        let records = data
            .get(kind.as_str())
            .ok_or_else(|| element.bad("Type", format!("unsupported {kind}")))?;
        let input = get(records, id, kind)?;
        let count = if matches!(kind.as_str(), "Line" | "TwoWindingTransformer") {
            2
        } else {
            1
        };
        let ports = ports(db, &terminals, id, count, network)?;
        let uid = format!("sincal:element:{id}");
        match kind.as_str() {
            "Line" => {
                let branch = equipment::line(
                    input,
                    &ports,
                    network,
                    &uid,
                    get(levels, element.integer("VoltLevel_ID")?, "VoltageLevel")?,
                )?;
                network.branches_mut().push(branch);
            }
            "TwoWindingTransformer" => {
                let branch = equipment::transformer(input, &ports, network, &uid)?;
                network.branches_mut().push(branch);
            }
            "Load" => {
                let base_kv = network
                    .buses()
                    .iter()
                    .find(|b| b.id == ports[0])
                    .unwrap()
                    .base_kv;
                network.loads_mut().push(equipment::load(
                    db, input, ports[0], base_kv, &uid, profiles,
                )?);
            }
            "Infeeder" | "DCInfeeder" => {
                if kind == "DCInfeeder" {
                    get(levels, element.integer("VoltLevel_ID")?, "VoltageLevel")?
                        .equals("Flag_DCInfeeder", 0)?;
                }
                profiles.check(input, kind == "Infeeder")?;
                let generator = equipment::generator(
                    input,
                    ports[0],
                    kind == "Infeeder",
                    &uid,
                    network,
                    in_service,
                )?;
                network.generators_mut().push(generator);
            }
            "ShuntCondensator" => {
                let mut shunt = equipment::capacitor(input, ports[0], network, &uid)?;
                shunt.in_service = in_service;
                network.shunts_mut().push(shunt);
            }
            _ => unreachable!(),
        }
        match kind.as_str() {
            "Line" | "TwoWindingTransformer" => {
                let branch = network.branches_mut().last_mut().unwrap();
                branch.in_service = in_service;
                branch.name = Some(element.text("Name")?);
            }
            "Load" => network.loads_mut().last_mut().unwrap().in_service = in_service,
            _ => (),
        }
    }
    Ok(())
}

/// An open terminal leaves the device itself present, connected through an
/// explicit open switch. In particular, opening one end must not remove the
/// energized end's line charging. Native node IDs are never renumbered.
fn ports(
    db: &DatabaseSnapshot,
    rows: &BTreeMap<i64, NativeRow>,
    element: i64,
    count: usize,
    network: &mut BalancedNetwork,
) -> Result<Vec<BusId>> {
    let mut native = db
        .terminals
        .iter()
        .filter(|(_, t)| t.element == element)
        .collect::<Vec<_>>();
    native.sort_by_key(|(_, t)| t.position);
    if native.len() != count {
        return Err(error(format!(
            "Element[{element}]: expected {count} terminals"
        )));
    }
    let mut result = Vec::new();
    for (index, (&id, terminal)) in native.into_iter().enumerate() {
        if terminal.position != i64::try_from(index).map_err(error)? + 1 {
            return Err(error(format!("Terminal[{id}]: wrong position")));
        }
        let row = get(rows, id, "Terminal")?;
        row.equals("Flag_Terminal", 7)?;
        let closed = row.state("Flag_State")?;
        let has_switch = row.state("Flag_Switch")?;
        let node = bus_id(terminal.node)?;
        if closed && !has_switch {
            result.push(node);
            continue;
        }
        let aux_id = network
            .buses()
            .iter()
            .map(|b| b.id.0)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| error("auxiliary node ID overflow"))?;
        let mut bus = network
            .buses()
            .iter()
            .find(|b| b.id == node)
            .unwrap()
            .clone();
        bus.id = BusId(aux_id);
        // The auxiliary terminal has no source merely because its original
        // bus does. A source adapter may explicitly prescribe it afterward.
        bus.kind = BusType::Pq;
        bus.uid = Some(format!("sincal:terminal:{id}:bus"));
        bus.name = Some(format!("SINCAL terminal {id}"));
        let mut switch = Switch::new(node, bus.id, closed);
        switch.uid = Some(format!("sincal:terminal:{id}:switch"));
        result.push(bus.id);
        network.buses_mut().push(bus);
        network.switches_mut().push(switch);
    }
    Ok(result)
}
