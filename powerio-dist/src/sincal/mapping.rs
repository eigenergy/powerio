//! Complete assembly of the currently verified component profiles.
//!
//! This is private and atomic: any unsupported component rejects the whole
//! mapping. A partial or empty replacement network is never returned as a
//! successful interpretation of an unsupported native project.
//! Isolated nodes retain their empty conductor sets. As with other parsers,
//! the separate readiness audit decides whether a consumer can use the value.

use std::collections::BTreeMap;

use super::{
    format_error, schema::NativeDatabase, source_mapping::IdealVoltageBoundary,
    topology::TopologyDraft, transformer::number,
};
use crate::{DistBus, MulticonductorNetwork, Result};

/// Internal assembly result. It cannot be handed to a numerical consumer as
/// a complete network: source constraints are still separate compiler data.
/// Keep the partial network private so a floating boundary cannot be dropped
/// by accidentally returning only its buses and passive components.
pub(super) struct CircuitDraft {
    network_without_sources: MulticonductorNetwork,
    source_boundaries: Vec<IdealVoltageBoundary>,
}

pub(super) struct MappingContext {
    pub frequency: f64,
    topology: TopologyDraft,
    buses: BTreeMap<i64, DistBus>,
}

impl CircuitDraft {
    pub fn new(frequency: f64) -> Self {
        let mut network_without_sources = MulticonductorNetwork::new();
        *network_without_sources.base_frequency_mut() = frequency;
        Self {
            network_without_sources,
            source_boundaries: Vec::new(),
        }
    }

    pub fn source_boundaries(&self) -> &[IdealVoltageBoundary] {
        &self.source_boundaries
    }

    fn into_network(mut self) -> MulticonductorNetwork {
        for boundary in self.source_boundaries {
            let source = boundary.into_source();
            self.network_without_sources.sources_mut().push(source);
        }
        self.network_without_sources
    }
}

impl NativeDatabase {
    pub fn network(&self) -> Result<MulticonductorNetwork> {
        Ok(self.circuit_draft()?.into_network())
    }

    pub fn circuit_draft(&self) -> Result<CircuitDraft> {
        let context = self.mapping_context()?;
        let mut draft = CircuitDraft::new(context.frequency);
        for &element in self.elements.keys() {
            self.map_component(element, &context, &mut draft)?;
        }
        draft
            .network_without_sources
            .buses_mut()
            .extend(context.buses.into_values());
        Ok(draft)
    }

    pub(super) fn mapping_context(&self) -> Result<MappingContext> {
        self.require_input_zero_sequence()?;
        self.require_element_voltage_bases()?;
        let frequency = self.mapping_frequency()?;
        let topology = self.topology_draft()?;
        for node in topology.nodes.values() {
            if node.neutral_point.is_some() {
                return Err(format_error(format!(
                    "Node {} requires neutral-point circuit assembly",
                    node.id
                )));
            }
            if node.level_frequency_hz.to_bits() != frequency.to_bits() {
                return Err(format_error(format!(
                    "Node {} has inconsistent network frequency",
                    node.id
                )));
            }
        }
        let buses: BTreeMap<i64, DistBus> = topology
            .nodes
            .keys()
            .copied()
            .zip(topology.buses())
            .collect();
        Ok(MappingContext {
            frequency,
            topology,
            buses,
        })
    }

    /// Shared by atomic whole-network assembly and the diagnostic audit.
    /// A failed attempt may have populated its private draft; never return
    /// that draft as a successfully parsed network.
    pub(super) fn map_component(
        &self,
        element: i64,
        context: &MappingContext,
        draft: &mut CircuitDraft,
    ) -> Result<()> {
        let kind = self
            .elements
            .get(&element)
            .ok_or_else(|| format_error(format!("unknown Element {element}")))?;
        let buses = &context.buses;
        let topology = &context.topology;
        let net = &mut draft.network_without_sources;
        let source_boundaries = &mut draft.source_boundaries;
        (|| -> Result<()> {
            match kind.as_str() {
                "Line" => {
                    let circuit = self.line_circuit(element, buses)?;
                    net.lines_mut().push(circuit.line);
                    net.line_codes_mut().push(circuit.code);
                    net.buses_mut().extend(circuit.auxiliary_buses);
                    net.switches_mut().extend(circuit.terminal_switches);
                }
                "Load" => {
                    let input = self.load_input(element)?;
                    let node = input.terminal.node;
                    let circuit =
                        input.circuit(&buses[&node], topology.nodes[&node].nominal_ll_volts)?;
                    net.buses_mut().push(circuit.bus);
                    net.loads_mut().push(circuit.load);
                    net.switches_mut().push(circuit.switch);
                }
                "Infeeder" => {
                    let input = self.infeeder_input(element)?;
                    let node = input.terminal.node;
                    let circuit = input.ideal_boundary_circuit(
                        &buses[&node],
                        topology.nodes[&node].nominal_ll_volts,
                    )?;
                    net.buses_mut().push(circuit.bus);
                    source_boundaries.push(circuit.boundary);
                    net.switches_mut().push(circuit.switch);
                }
                "DCInfeeder" => {
                    let input = self.dc_infeeder_input(element)?;
                    let circuit = input.circuit(&buses[&input.terminal.node])?;
                    net.buses_mut().push(circuit.bus);
                    net.generators_mut().push(circuit.generator);
                    net.switches_mut().push(circuit.switch);
                }
                "ShuntImpedance" => {
                    let input = self.shunt_impedance_input(element)?;
                    let circuit = input.circuit(&buses[&input.terminal.node])?;
                    net.buses_mut().push(circuit.bus);
                    net.shunts_mut().push(circuit.shunt);
                    net.switches_mut().push(circuit.switch);
                }
                "TwoWindingTransformer" => {
                    let circuit = self.transformer_circuit(element, buses)?;
                    net.buses_mut().push(circuit.auxiliary_bus);
                    net.shunts_mut().push(circuit.shunt);
                    net.switches_mut().extend(circuit.terminal_switches);
                }
                _ => return Err(format_error(format!("unmapped element type {kind}"))),
            }
            Ok(())
        })()
        .map_err(|error| format_error(format!("Element {element} ({kind}): {error}")))
    }

    fn require_element_voltage_bases(&self) -> Result<()> {
        // Equipment and nodes can refer to different voltage levels. Checking
        // only the node leaves absolute component voltage inputs ambiguous.
        let mut statement = self
            .connection
            .prepare(
                "SELECT e.Element_ID, v.Flag_Volt FROM Element e LEFT JOIN VoltageLevel v
             ON v.VoltLevel_ID=e.VoltLevel_ID AND v.Variant_ID=e.Variant_ID
             WHERE e.Variant_ID=?1 ORDER BY e.Element_ID",
            )
            .map_err(format_error)?;
        let mut rows = statement.query([self.variant]).map_err(format_error)?;
        let mut previous = None;
        while let Some(row) = rows.next().map_err(format_error)? {
            let id: i64 = row.get(0).map_err(format_error)?;
            let kind: Option<i64> = row.get(1).map_err(format_error)?;
            if previous == Some(id) || kind != Some(1) {
                return Err(format_error(format!(
                    "Element {id}: requires an unambiguous line-line voltage level (Flag_Volt=1)"
                )));
            }
            previous = Some(id);
        }
        Ok(())
    }

    fn mapping_frequency(&self) -> Result<f64> {
        let mut statement = self
            .connection
            .prepare("SELECT f FROM CalcParameter WHERE Variant_ID=?1")
            .map_err(format_error)?;
        let mut rows = statement.query([self.variant]).map_err(format_error)?;
        let row = rows
            .next()
            .map_err(format_error)?
            .ok_or_else(|| format_error("missing calculation frequency"))?;
        let value = number(row, "f")?;
        if value <= 0.0 || rows.next().map_err(format_error)?.is_some() {
            return Err(format_error("invalid or ambiguous calculation frequency"));
        }
        Ok(value)
    }
}
