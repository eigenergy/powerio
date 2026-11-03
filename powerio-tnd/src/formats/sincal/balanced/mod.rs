//! Explicit positive-sequence SINCAL adapter. Three-phase terminal codes are
//! validated *after* the caller selects this profile; they never select it.
//! Electrical meanings belong here, not in the shared acquisition crate.

#[cfg(test)]
mod access_tests;
mod equipment;
mod geometry;
mod profile;
mod rows;
mod settings;
mod write;
pub use write::{ExperimentalBalancedOutput, write_experimental_balanced};
pub(super) mod source;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod write_tests;

use crate::formats::sincal::storage::{DatabaseSnapshot, TerminalIdentity};
use powerio_tx::Result;
use powerio_tx::network::{BalancedNetwork, Bus, BusId, BusType, SourceFormat, Switch};
use rows::{NativeRow, get, table};
use settings::StaticProfiles;
use std::collections::BTreeMap;

/// Refuse a line with `LineSeg` sections in the selected variant. A section
/// carries its own impedance, so the `Line` row alone does not describe the
/// line, as the multiconductor reader also refuses. A project without the
/// table has no sections; a view in its place is not native storage.
fn refuse_segmented_lines(db: &DatabaseSnapshot) -> Result<()> {
    let kind: Option<String> = db
        .connection
        .query_row(
            "SELECT type FROM pragma_table_list('LineSeg') WHERE schema='main' AND name='LineSeg'",
            [],
            |row| row.get(0),
        )
        .map(Some)
        .or_else(|cause| match cause {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            cause => Err(cause),
        })
        .map_err(error)?;
    if kind.is_none() {
        return Ok(());
    }
    crate::formats::sincal::storage::require_table(
        &db.connection,
        "LineSeg",
        &["Line_ID", "Variant_ID"],
    )
    .map_err(error)?;
    let mut statement = db
        .connection
        .prepare("SELECT DISTINCT Line_ID FROM LineSeg WHERE Variant_ID=?1")
        .map_err(error)?;
    let segmented: std::collections::BTreeSet<i64> = statement
        .query_map([db.variant], |row| row.get(0))
        .map_err(error)?
        .collect::<rusqlite::Result<_>>()
        .map_err(error)?;
    match db
        .elements
        .iter()
        .find(|(id, kind)| kind.as_str() == "Line" && segmented.contains(id))
    {
        Some((id, _)) => Err(error(format!(
            "Line[{id}]: line segments require explicit assembly"
        ))),
        None => Ok(()),
    }
}

fn error<T: std::fmt::Display + 'static>(message: T) -> powerio_tx::Error {
    powerio_tx::Error::FormatRead {
        format: "sincal",
        message: crate::formats::sincal::storage::failure_message(&message),
    }
}

/// Position of a bus in `network.buses()`. The reader pushes native nodes in
/// ascending `Node_ID` order and gives each auxiliary bus the next larger ID,
/// so the table is sorted by ID and a binary search finds the bus without
/// scanning every bus per lookup. The scan remains only as a fallback.
fn bus_position(network: &BalancedNetwork, id: BusId) -> Option<usize> {
    let buses = network.buses();
    match buses.binary_search_by_key(&id, |b| b.id) {
        Ok(position) => Some(position),
        Err(_) => buses.iter().position(|b| b.id == id),
    }
}

fn find_bus(network: &BalancedNetwork, id: BusId) -> Option<&Bus> {
    bus_position(network, id).map(|position| &network.buses()[position])
}

fn find_bus_mut(network: &mut BalancedNetwork, id: BusId) -> Option<&mut Bus> {
    bus_position(network, id).map(|position| &mut network.buses_mut()[position])
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
    network.validate()?;
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
    refuse_segmented_lines(db)?;
    let by_element = terminals_by_element(db);
    // Auxiliary buses take the next ID above every bus so far.
    let mut largest_bus = network.buses().iter().map(|b| b.id.0).max().unwrap_or(0);
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
        let native = by_element.get(&id).map_or(&[][..], Vec::as_slice);
        let ports = ports(native, &terminals, id, count, &mut largest_bus, network)?;
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
                let base_kv = find_bus(network, ports[0]).unwrap().base_kv;
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

/// Group terminals by element once, in `Terminal_ID` order, instead of
/// scanning every terminal for each element.
fn terminals_by_element(db: &DatabaseSnapshot) -> BTreeMap<i64, Vec<(&i64, &TerminalIdentity)>> {
    let mut by_element = BTreeMap::<i64, Vec<_>>::new();
    for (id, terminal) in &db.terminals {
        by_element
            .entry(terminal.element)
            .or_default()
            .push((id, terminal));
    }
    by_element
}

/// An open terminal leaves the device itself present, connected through an
/// explicit open switch. In particular, opening one end must not remove the
/// energized end's line charging. Native node IDs are never renumbered.
/// `native` holds the element's terminals in `Terminal_ID` order;
/// `largest_bus` is the largest bus ID in the network so far.
fn ports(
    native: &[(&i64, &TerminalIdentity)],
    rows: &BTreeMap<i64, NativeRow>,
    element: i64,
    count: usize,
    largest_bus: &mut usize,
    network: &mut BalancedNetwork,
) -> Result<Vec<BusId>> {
    let mut native = native.to_vec();
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
        let aux_id = largest_bus
            .checked_add(1)
            .ok_or_else(|| error("auxiliary node ID overflow"))?;
        let mut bus = find_bus(network, node).unwrap().clone();
        bus.id = BusId(aux_id);
        // The auxiliary terminal has no source merely because its original
        // bus does. A source adapter may explicitly prescribe it afterward.
        bus.kind = BusType::Pq;
        bus.uid = Some(format!("sincal:terminal:{id}:bus"));
        bus.name = Some(format!("SINCAL terminal {id}"));
        let mut switch = Switch::new(node, bus.id, closed);
        switch.uid = Some(format!("sincal:terminal:{id}:switch"));
        result.push(bus.id);
        *largest_bus = aux_id;
        network.buses_mut().push(bus);
        network.switches_mut().push(switch);
    }
    Ok(result)
}
