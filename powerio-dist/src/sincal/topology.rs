//! Bus construction from native topology and voltage-level references.
//!
//! Node.Un is a load-flow initial guess; Node.Flag_Phase selects fault phases.
//! Neither supplies the nominal voltage or a bus's electrical conductor set.
//! Collected terminals are declared equipment connections, not a claim that
//! every conductor is energized. Service and switching are lowered later.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    format_error,
    schema::{NativeDatabase, require_table},
    semantics::{Connection, ElectricalTerminal},
};
use crate::{DistBus, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Conductor {
    L1,
    L2,
    L3,
    Neutral,
}

impl Conductor {
    fn phase(index: usize) -> Result<Self> {
        [Self::L1, Self::L2, Self::L3]
            .get(index)
            .copied()
            .ok_or_else(|| format_error("invalid transformer phase position"))
    }

    fn name(self) -> &'static str {
        match self {
            Self::L1 => "1",
            Self::L2 => "2",
            Self::L3 => "3",
            Self::Neutral => "n",
        }
    }
}

pub(super) struct NodeInput {
    pub id: i64,
    pub name: Option<String>,
    pub voltage_level: i64,
    pub nominal_ll_volts: f64,
    pub level_frequency_hz: f64,
    pub legacy_voltage_basis: bool,
    pub neutral_point: Option<i64>,
}

/// Deliberately not a MulticonductorNetwork: an incomplete equipment mapping
/// must not be accepted by a numerical consumer as an empty physical network.
pub(super) struct TopologyDraft {
    pub nodes: BTreeMap<i64, NodeInput>,
    pub conductors: BTreeMap<i64, BTreeSet<Conductor>>,
}

impl TopologyDraft {
    pub fn buses(&self) -> Vec<DistBus> {
        self.nodes
            .values()
            .map(|node| {
                let terminals = self.conductors[&node.id]
                    .iter()
                    .map(|c| c.name().to_owned())
                    .collect();
                let mut bus = DistBus::new(node.id.to_string(), terminals);
                bus.extras.insert(
                    "sincal".into(),
                    serde_json::json!({
                        "name": node.name, "voltage_level": node.voltage_level,
                        "nominal_ll_volts": node.nominal_ll_volts,
                        "legacy_voltage_basis": node.legacy_voltage_basis,
                    }),
                );
                // A neutral conductor is not implicitly grounded. Neutral-point
                // references belong to the subsequent explicit circuit mapping.
                bus
            })
            .collect()
    }
}

impl NativeDatabase {
    pub fn node_inputs(&self) -> Result<BTreeMap<i64, NodeInput>> {
        require_table(
            &self.connection,
            "VoltageLevel",
            &["VoltLevel_ID", "Variant_ID", "Un", "f", "Flag_Volt"],
        )?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT n.Node_ID, n.Name, n.VoltLevel_ID, n.Stp_ID, v.Un, v.f, v.Flag_Volt, v.VoltLevel_ID
             FROM Node n LEFT JOIN VoltageLevel v
             ON v.VoltLevel_ID=n.VoltLevel_ID AND v.Variant_ID=n.Variant_ID
             WHERE n.Variant_ID=?1 ORDER BY n.Node_ID",
            )
            .map_err(format_error)?;
        let mut rows = statement.query([self.variant]).map_err(format_error)?;
        let mut nodes = BTreeMap::new();
        while let Some(row) = rows.next().map_err(format_error)? {
            let id = row.get(0).map_err(format_error)?;
            if nodes.len() >= self.nodes.len() || nodes.contains_key(&id) {
                return Err(format_error("ambiguous node/voltage-level join"));
            }
            // Database Interface and Automation (April 2015), p. 90:
            // 1 is line-line, 2 is line-earth. Do not silently apply the
            // existing line-line component formulas to another voltage basis.
            let kind: Option<i64> = row.get(6).map_err(format_error)?;
            let level: Option<i64> = row.get(7).map_err(format_error)?;
            if level.is_none() {
                return Err(format_error(format!(
                    "Node {id}: missing referenced voltage level"
                )));
            }
            let legacy_voltage_basis = self
                .line_line_voltage_basis(kind)
                .map_err(|e| format_error(format!("Node {id}: {e}")))?;
            let voltage: Option<f64> = row.get(4).map_err(format_error)?;
            let frequency: Option<f64> = row.get(5).map_err(format_error)?;
            let voltage = positive(voltage, "nominal voltage", id)? * 1000.0;
            if !voltage.is_finite() {
                return Err(format_error("node nominal voltage overflows SI units"));
            }
            let neutral_point = match row.get(3).map_err(format_error)? {
                None | Some(0) => None,
                Some(value) if value > 0 => Some(value),
                _ => return Err(format_error("invalid node neutral-point reference")),
            };
            nodes.insert(
                id,
                NodeInput {
                    id,
                    name: row.get(1).map_err(format_error)?,
                    voltage_level: row.get(2).map_err(format_error)?,
                    nominal_ll_volts: voltage,
                    legacy_voltage_basis,
                    level_frequency_hz: positive(frequency, "level frequency", id)?,
                    neutral_point,
                },
            );
        }
        if nodes.len() != self.nodes.len() {
            return Err(format_error("incomplete node/voltage-level join"));
        }
        Ok(nodes)
    }

    pub fn topology_draft(&self) -> Result<TopologyDraft> {
        let nodes = self.node_inputs()?;
        let mut conductors: BTreeMap<_, BTreeSet<_>> =
            nodes.keys().map(|&id| (id, BTreeSet::new())).collect();
        for (&id, kind) in &self.elements {
            if kind == "TwoWindingTransformer" {
                let (ports, coils) = self.transformer_topology(id)?;
                for coil in &coils {
                    for (port, incidence) in ports.iter().zip([coil.primary, coil.secondary]) {
                        for (phase, coefficient) in incidence[..3].iter().enumerate() {
                            if *coefficient != 0 {
                                conductors
                                    .get_mut(&port.node)
                                    .ok_or_else(|| {
                                        format_error("transformer terminal references missing node")
                                    })?
                                    .insert(Conductor::phase(phase)?);
                            }
                        }
                    }
                }
                continue;
            }
            if !matches!(
                kind.as_str(),
                "Line" | "Load" | "Infeeder" | "DCInfeeder" | "ShuntImpedance"
            ) {
                return Err(format_error(format!(
                    "Element {id}: no conductor mapping for element type {kind}"
                )));
            }
            for port in self.electrical_terminals(id)? {
                add_port(&mut conductors, &port)?;
            }
        }
        Ok(TopologyDraft { nodes, conductors })
    }
}

fn add_port(
    conductors: &mut BTreeMap<i64, BTreeSet<Conductor>>,
    port: &ElectricalTerminal,
) -> Result<()> {
    let at_node = conductors
        .get_mut(&port.node)
        .ok_or_else(|| format_error("terminal references missing node"))?;
    if port.connection == Connection::Neutral {
        at_node.insert(Conductor::Neutral);
    } else if let Some(phases) = port.connection.phases() {
        for &phase in phases {
            at_node.insert(Conductor::phase(phase)?);
        }
    }
    Ok(())
}

fn positive(value: Option<f64>, field: &str, node: i64) -> Result<f64> {
    value.filter(|v| v.is_finite() && *v > 0.0).ok_or_else(|| {
        format_error(format!(
            "Node {node}: missing or invalid {field} from voltage level"
        ))
    })
}
