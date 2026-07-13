//! Normalized dense tables for solver and compiler front ends.
//!
//! `Network::to_normalized` keeps source bus ids because it is still a network
//! model. Solver inputs want dense row ids, stable row order, and enough
//! provenance to map lowered data back to the source case. This module provides
//! that table layout without changing the lossless `Network` representation.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::network::{
    BranchCurrentRatings, BranchRatingSet, BusId, BusType, GenCaps, GenCost, Hvdc,
    LoadVoltageModel, Network,
};
use crate::{Error, IndexedNetwork, Result};

/// Stable pass name for the balanced normalized solver table lowering.
pub const NORMALIZED_SOLVER_TABLES_PASS: &str = "balanced-to-normalized-solver-tables";

/// A row oriented, dense indexed, per unit/radian view of a balanced network.
///
/// The source `Network` is first normalized with [`Network::to_normalized`], then
/// lowered through [`IndexedNetwork`] so 3-winding transformers appear as star
/// buses and branches. Source ids are preserved as metadata; every reference used
/// for computation is dense and zero based.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct NormalizedSolverTables {
    pub pass: String,
    pub network_name: String,
    pub base_mva: f64,
    pub base_frequency: f64,
    pub units: SolverTableUnits,
    pub index: SolverTableIndex,
    pub buses: Vec<SolverBusRow>,
    pub loads: Vec<SolverLoadRow>,
    pub shunts: Vec<SolverShuntRow>,
    pub branches: Vec<SolverBranchRow>,
    pub switches: Vec<SolverSwitchRow>,
    pub arcs: Vec<SolverArcRow>,
    pub generators: Vec<SolverGeneratorRow>,
    pub storage: Vec<SolverStorageRow>,
    pub hvdc: Vec<SolverHvdcRow>,
}

/// Units carried by [`NormalizedSolverTables`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct SolverTableUnits {
    pub power: String,
    pub voltage: String,
    pub angle: String,
    pub impedance: String,
    pub admittance: String,
    pub dense_index_base: String,
}

impl Default for SolverTableUnits {
    fn default() -> Self {
        Self {
            power: "per_unit".to_string(),
            voltage: "per_unit".to_string(),
            angle: "radian".to_string(),
            impedance: "per_unit".to_string(),
            admittance: "per_unit".to_string(),
            dense_index_base: "zero".to_string(),
        }
    }
}

/// Identity and provenance vectors that apply across the tables.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct SolverTableIndex {
    /// Source bus id for each dense bus row. Synthetic 3-winding star buses also
    /// receive a stable id in this vector, but have no source row.
    pub bus_ids: Vec<BusId>,
    pub reference_bus_indices: Vec<usize>,
    pub component_labels: Vec<usize>,
    pub branch_from_arc_indices: Vec<usize>,
    pub branch_to_arc_indices: Vec<usize>,
    pub bus_source_rows: Vec<Option<usize>>,
    pub load_source_rows: Vec<Option<usize>>,
    pub shunt_source_rows: Vec<Option<usize>>,
    pub branch_source_rows: Vec<Option<usize>>,
    pub switch_source_rows: Vec<Option<usize>>,
    pub generator_source_rows: Vec<Option<usize>>,
    pub storage_source_rows: Vec<Option<usize>>,
    pub hvdc_source_rows: Vec<Option<usize>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct SolverBusRow {
    pub index: usize,
    pub bus_id: BusId,
    pub source_row: Option<usize>,
    pub kind: BusType,
    pub vm: f64,
    pub va: f64,
    pub base_kv: f64,
    pub vmax: f64,
    pub vmin: f64,
    pub evhi: Option<f64>,
    pub evlo: Option<f64>,
    pub area: usize,
    pub zone: usize,
    pub pd: f64,
    pub qd: f64,
    pub gs: f64,
    pub bs: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct SolverLoadRow {
    pub index: usize,
    pub source_row: Option<usize>,
    pub bus_index: usize,
    pub p: f64,
    pub q: f64,
    pub voltage_model: Option<LoadVoltageModel>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct SolverShuntRow {
    pub index: usize,
    pub source_row: Option<usize>,
    pub bus_index: usize,
    pub g: f64,
    pub b: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct SolverBranchRow {
    pub index: usize,
    pub source_row: Option<usize>,
    pub from_bus_index: usize,
    pub to_bus_index: usize,
    pub r: f64,
    pub x: f64,
    pub b: f64,
    pub g_fr: f64,
    pub b_fr: f64,
    pub g_to: f64,
    pub b_to: f64,
    pub rate_a: f64,
    pub rate_b: f64,
    pub rate_c: f64,
    pub rating_sets: Vec<BranchRatingSet>,
    pub current_ratings: Option<BranchCurrentRatings>,
    pub tap: f64,
    pub shift: f64,
    pub angmin: f64,
    pub angmax: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct SolverSwitchRow {
    pub index: usize,
    pub source_row: Option<usize>,
    pub from_bus_index: usize,
    pub to_bus_index: usize,
    pub closed: bool,
    pub thermal_rating: Option<f64>,
    pub current_rating: Option<f64>,
    pub pf: Option<f64>,
    pub qf: Option<f64>,
    pub pt: Option<f64>,
    pub qt: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SolverArcTerminal {
    From,
    To,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct SolverArcRow {
    pub index: usize,
    pub branch_index: usize,
    pub terminal: SolverArcTerminal,
    pub from_bus_index: usize,
    pub to_bus_index: usize,
    pub tap: f64,
    pub shift: f64,
    pub g_shunt: f64,
    pub b_shunt: f64,
    pub rate_a: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct SolverGeneratorRow {
    pub index: usize,
    pub source_row: Option<usize>,
    pub bus_index: usize,
    pub pg: f64,
    pub qg: f64,
    pub pmax: f64,
    pub pmin: f64,
    pub qmax: f64,
    pub qmin: f64,
    pub vg: f64,
    pub mbase: f64,
    pub cost: Option<SolverCostRow>,
    pub caps: GenCaps,
    pub regulated_bus_index: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct SolverStorageRow {
    pub index: usize,
    pub source_row: Option<usize>,
    pub bus_index: usize,
    pub ps: f64,
    pub qs: f64,
    pub energy: f64,
    pub energy_rating: f64,
    pub charge_rating: f64,
    pub discharge_rating: f64,
    pub charge_efficiency: f64,
    pub discharge_efficiency: f64,
    pub thermal_rating: f64,
    pub current_rating: Option<f64>,
    pub qmin: f64,
    pub qmax: f64,
    pub r: f64,
    pub x: f64,
    pub p_loss: f64,
    pub q_loss: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct SolverHvdcRow {
    pub index: usize,
    pub source_row: Option<usize>,
    pub from_bus_index: usize,
    pub to_bus_index: usize,
    pub pf: f64,
    pub pt: f64,
    pub qf: f64,
    pub qt: f64,
    pub vf: f64,
    pub vt: f64,
    pub pmin: f64,
    pub pmax: f64,
    pub qminf: f64,
    pub qmaxf: f64,
    pub qmint: f64,
    pub qmaxt: f64,
    pub loss0: f64,
    pub loss1: f64,
    pub cost: Option<SolverCostRow>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct SolverCostRow {
    pub model: u8,
    pub startup: f64,
    pub shutdown: f64,
    pub ncost: usize,
    pub coeffs: Vec<f64>,
}

impl From<&GenCost> for SolverCostRow {
    fn from(cost: &GenCost) -> Self {
        Self {
            model: cost.model,
            startup: cost.startup,
            shutdown: cost.shutdown,
            ncost: cost.ncost,
            coeffs: cost.coeffs.clone(),
        }
    }
}

impl Network {
    /// Lower this balanced network into normalized dense solver tables.
    ///
    /// # Errors
    /// Propagates [`Network::to_normalized`] errors and reports
    /// [`Error::UnknownBus`] if the derived normalized network contains an
    /// internal dangling bus reference.
    pub fn to_normalized_solver_tables(&self) -> Result<NormalizedSolverTables> {
        NormalizedSolverTables::from_network(self)
    }
}

impl NormalizedSolverTables {
    pub fn from_network(source: &Network) -> Result<Self> {
        let normalized = normalized_for_solver(source)?;
        let view = IndexedNetwork::new(&normalized);
        let net = view.network();
        let provenance = SourceRows::new(source, net);

        let branch_arcs = branch_and_arc_rows(&view, &provenance)?;
        let buses = bus_rows(&view, &provenance);
        let loads = load_rows(&view, &provenance)?;
        let shunts = shunt_rows(&view, &provenance)?;
        let switches = switch_rows(&view, &provenance)?;
        let generators = generator_rows(&view, &provenance)?;
        let storage = storage_rows(&view, &provenance)?;
        let hvdc = hvdc_rows(&view, &provenance)?;

        Ok(Self {
            pass: NORMALIZED_SOLVER_TABLES_PASS.to_string(),
            network_name: net.name.clone(),
            base_mva: net.base_mva,
            base_frequency: net.base_frequency,
            units: SolverTableUnits::default(),
            index: SolverTableIndex {
                bus_ids: net.buses.iter().map(|b| b.id).collect(),
                reference_bus_indices: view.reference_bus_indices(),
                component_labels: view.connected_component_labels(),
                branch_from_arc_indices: branch_arcs.branch_from_arc_indices,
                branch_to_arc_indices: branch_arcs.branch_to_arc_indices,
                bus_source_rows: provenance.bus,
                load_source_rows: provenance.load,
                shunt_source_rows: provenance.shunt,
                branch_source_rows: provenance.branch,
                switch_source_rows: provenance.switch,
                generator_source_rows: provenance.generator,
                storage_source_rows: provenance.storage,
                hvdc_source_rows: provenance.hvdc,
            },
            buses,
            loads,
            shunts,
            branches: branch_arcs.branches,
            switches,
            arcs: branch_arcs.arcs,
            generators,
            storage,
            hvdc,
        })
    }
}

fn normalized_for_solver(source: &Network) -> Result<Network> {
    if source.is_normalized() {
        Ok(source.clone())
    } else {
        source.to_normalized()
    }
}

fn bus_rows(view: &IndexedNetwork<'_>, provenance: &SourceRows) -> Vec<SolverBusRow> {
    view.network()
        .buses
        .iter()
        .enumerate()
        .map(|(i, bus)| SolverBusRow {
            index: i,
            bus_id: bus.id,
            source_row: provenance.bus[i],
            kind: bus.kind,
            vm: bus.vm,
            va: bus.va,
            base_kv: bus.base_kv,
            vmax: bus.vmax,
            vmin: bus.vmin,
            evhi: bus.evhi,
            evlo: bus.evlo,
            area: bus.area,
            zone: bus.zone,
            pd: view.pd()[i],
            qd: view.qd()[i],
            gs: view.gs()[i],
            bs: view.bs()[i],
        })
        .collect()
}

fn load_rows(view: &IndexedNetwork<'_>, provenance: &SourceRows) -> Result<Vec<SolverLoadRow>> {
    view.network()
        .loads
        .iter()
        .enumerate()
        .map(|(i, load)| {
            Ok(SolverLoadRow {
                index: i,
                source_row: provenance.load[i],
                bus_index: dense_bus(view, load.bus, i)?,
                p: load.p,
                q: load.q,
                voltage_model: load.voltage_model.clone(),
            })
        })
        .collect()
}

fn shunt_rows(view: &IndexedNetwork<'_>, provenance: &SourceRows) -> Result<Vec<SolverShuntRow>> {
    view.network()
        .shunts
        .iter()
        .enumerate()
        .map(|(i, shunt)| {
            Ok(SolverShuntRow {
                index: i,
                source_row: provenance.shunt[i],
                bus_index: dense_bus(view, shunt.bus, i)?,
                g: shunt.g,
                b: shunt.b,
            })
        })
        .collect()
}

struct BranchArcRows {
    branches: Vec<SolverBranchRow>,
    arcs: Vec<SolverArcRow>,
    branch_from_arc_indices: Vec<usize>,
    branch_to_arc_indices: Vec<usize>,
}

fn branch_and_arc_rows(
    view: &IndexedNetwork<'_>,
    provenance: &SourceRows,
) -> Result<BranchArcRows> {
    let net = view.network();
    let mut branch_from_arc_indices = Vec::with_capacity(net.branches.len());
    let mut branch_to_arc_indices = Vec::with_capacity(net.branches.len());
    let mut arcs = Vec::with_capacity(net.branches.len() * 2);
    let branches = net
        .branches
        .iter()
        .enumerate()
        .map(|(i, branch)| {
            let from_bus_index = dense_bus(view, branch.from, i)?;
            let to_bus_index = dense_bus(view, branch.to, i)?;
            let charging = branch.terminal_charging();
            let from_arc = arcs.len();
            arcs.push(SolverArcRow {
                index: from_arc,
                branch_index: i,
                terminal: SolverArcTerminal::From,
                from_bus_index,
                to_bus_index,
                tap: branch.tap,
                shift: branch.shift,
                g_shunt: charging.g_fr,
                b_shunt: charging.b_fr,
                rate_a: branch.rate_a,
            });
            let to_arc = arcs.len();
            arcs.push(SolverArcRow {
                index: to_arc,
                branch_index: i,
                terminal: SolverArcTerminal::To,
                from_bus_index: to_bus_index,
                to_bus_index: from_bus_index,
                tap: 1.0,
                shift: 0.0,
                g_shunt: charging.g_to,
                b_shunt: charging.b_to,
                rate_a: branch.rate_a,
            });
            branch_from_arc_indices.push(from_arc);
            branch_to_arc_indices.push(to_arc);

            Ok(SolverBranchRow {
                index: i,
                source_row: provenance.branch[i],
                from_bus_index,
                to_bus_index,
                r: branch.r,
                x: branch.x,
                b: branch.b,
                g_fr: charging.g_fr,
                b_fr: charging.b_fr,
                g_to: charging.g_to,
                b_to: charging.b_to,
                rate_a: branch.rate_a,
                rate_b: branch.rate_b,
                rate_c: branch.rate_c,
                rating_sets: branch.rating_sets.clone(),
                current_ratings: branch.current_ratings,
                tap: branch.tap,
                shift: branch.shift,
                angmin: branch.angmin,
                angmax: branch.angmax,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(BranchArcRows {
        branches,
        arcs,
        branch_from_arc_indices,
        branch_to_arc_indices,
    })
}

fn switch_rows(view: &IndexedNetwork<'_>, provenance: &SourceRows) -> Result<Vec<SolverSwitchRow>> {
    view.network()
        .switches
        .iter()
        .enumerate()
        .map(|(i, switch)| {
            Ok(SolverSwitchRow {
                index: i,
                source_row: provenance.switch[i],
                from_bus_index: dense_bus(view, switch.from, i)?,
                to_bus_index: dense_bus(view, switch.to, i)?,
                closed: switch.closed,
                thermal_rating: switch.thermal_rating,
                current_rating: switch.current_rating,
                pf: switch.pf,
                qf: switch.qf,
                pt: switch.pt,
                qt: switch.qt,
            })
        })
        .collect()
}

fn generator_rows(
    view: &IndexedNetwork<'_>,
    provenance: &SourceRows,
) -> Result<Vec<SolverGeneratorRow>> {
    view.network()
        .generators
        .iter()
        .enumerate()
        .map(|(i, generator)| {
            Ok(SolverGeneratorRow {
                index: i,
                source_row: provenance.generator[i],
                bus_index: dense_bus(view, generator.bus, i)?,
                pg: generator.pg,
                qg: generator.qg,
                pmax: generator.pmax,
                pmin: generator.pmin,
                qmax: generator.qmax,
                qmin: generator.qmin,
                vg: generator.vg,
                mbase: generator.mbase,
                cost: generator.cost.as_ref().map(SolverCostRow::from),
                caps: generator.caps,
                regulated_bus_index: generator
                    .regulated_bus
                    .map(|bus| dense_bus(view, bus, i))
                    .transpose()?,
            })
        })
        .collect()
}

fn storage_rows(
    view: &IndexedNetwork<'_>,
    provenance: &SourceRows,
) -> Result<Vec<SolverStorageRow>> {
    let base_mva = view.network().base_mva;
    view.network()
        .storage
        .iter()
        .enumerate()
        .map(|(i, storage)| {
            Ok(SolverStorageRow {
                index: i,
                source_row: provenance.storage[i],
                bus_index: dense_bus(view, storage.bus, i)?,
                ps: storage.ps / base_mva,
                qs: storage.qs / base_mva,
                energy: storage.energy,
                energy_rating: storage.energy_rating,
                charge_rating: storage.charge_rating,
                discharge_rating: storage.discharge_rating,
                charge_efficiency: storage.charge_efficiency,
                discharge_efficiency: storage.discharge_efficiency,
                thermal_rating: storage.thermal_rating,
                current_rating: storage.current_rating,
                qmin: storage.qmin,
                qmax: storage.qmax,
                r: storage.r,
                x: storage.x,
                p_loss: storage.p_loss,
                q_loss: storage.q_loss,
            })
        })
        .collect()
}

fn hvdc_rows(view: &IndexedNetwork<'_>, provenance: &SourceRows) -> Result<Vec<SolverHvdcRow>> {
    view.network()
        .hvdc
        .iter()
        .enumerate()
        .map(|(i, hvdc)| hvdc_row(view, provenance, i, hvdc))
        .collect()
}

fn hvdc_row(
    view: &IndexedNetwork<'_>,
    provenance: &SourceRows,
    i: usize,
    hvdc: &Hvdc,
) -> Result<SolverHvdcRow> {
    let base_mva = view.network().base_mva;
    Ok(SolverHvdcRow {
        index: i,
        source_row: provenance.hvdc[i],
        from_bus_index: dense_bus(view, hvdc.from, i)?,
        to_bus_index: dense_bus(view, hvdc.to, i)?,
        pf: hvdc.pf,
        pt: hvdc.pt,
        qf: hvdc.qf,
        qt: hvdc.qt,
        vf: hvdc.vf,
        vt: hvdc.vt,
        pmin: hvdc.pmin / base_mva,
        pmax: hvdc.pmax / base_mva,
        qminf: hvdc.qminf,
        qmaxf: hvdc.qmaxf,
        qmint: hvdc.qmint,
        qmaxt: hvdc.qmaxt,
        loss0: hvdc.loss0,
        loss1: hvdc.loss1,
        cost: hvdc.cost.as_ref().map(SolverCostRow::from),
    })
}

fn dense_bus(view: &IndexedNetwork<'_>, bus_id: BusId, element_index: usize) -> Result<usize> {
    view.bus_index(bus_id).ok_or(Error::UnknownBus {
        bus_id,
        element_index,
    })
}

#[derive(Debug)]
struct SourceRows {
    bus: Vec<Option<usize>>,
    load: Vec<Option<usize>>,
    shunt: Vec<Option<usize>>,
    branch: Vec<Option<usize>>,
    switch: Vec<Option<usize>>,
    generator: Vec<Option<usize>>,
    storage: Vec<Option<usize>>,
    hvdc: Vec<Option<usize>>,
}

impl SourceRows {
    fn new(source: &Network, lowered: &Network) -> Self {
        let kept_buses: HashSet<BusId> = source
            .buses
            .iter()
            .filter(|b| b.kind != BusType::Isolated)
            .map(|b| b.id)
            .collect();
        let bus_source: HashMap<BusId, usize> = source
            .buses
            .iter()
            .enumerate()
            .filter(|(_, b)| b.kind != BusType::Isolated)
            .map(|(i, b)| (b.id, i))
            .collect();
        let bus = lowered
            .buses
            .iter()
            .map(|b| bus_source.get(&b.id).copied())
            .collect();

        Self {
            bus,
            load: resize_sources(
                lowered.loads.len(),
                source.loads.iter().enumerate().filter_map(|(i, load)| {
                    (load.in_service && kept_buses.contains(&load.bus)).then_some(i)
                }),
            ),
            shunt: resize_sources(
                lowered.shunts.len(),
                source.shunts.iter().enumerate().filter_map(|(i, shunt)| {
                    (shunt.in_service && kept_buses.contains(&shunt.bus)).then_some(i)
                }),
            ),
            branch: resize_sources(
                lowered.branches.len(),
                source
                    .branches
                    .iter()
                    .enumerate()
                    .filter_map(|(i, branch)| {
                        (branch.in_service
                            && kept_buses.contains(&branch.from)
                            && kept_buses.contains(&branch.to))
                        .then_some(i)
                    }),
            ),
            switch: resize_sources(
                lowered.switches.len(),
                source
                    .switches
                    .iter()
                    .enumerate()
                    .filter_map(|(i, switch)| {
                        (kept_buses.contains(&switch.from) && kept_buses.contains(&switch.to))
                            .then_some(i)
                    }),
            ),
            generator: resize_sources(
                lowered.generators.len(),
                source
                    .generators
                    .iter()
                    .enumerate()
                    .filter_map(|(i, generator)| {
                        (generator.in_service && kept_buses.contains(&generator.bus)).then_some(i)
                    }),
            ),
            storage: resize_sources(
                lowered.storage.len(),
                source
                    .storage
                    .iter()
                    .enumerate()
                    .filter_map(|(i, storage)| {
                        (storage.in_service && kept_buses.contains(&storage.bus)).then_some(i)
                    }),
            ),
            hvdc: resize_sources(
                lowered.hvdc.len(),
                source.hvdc.iter().enumerate().filter_map(|(i, hvdc)| {
                    (hvdc.in_service
                        && kept_buses.contains(&hvdc.from)
                        && kept_buses.contains(&hvdc.to))
                    .then_some(i)
                }),
            ),
        }
    }
}

fn resize_sources(len: usize, rows: impl Iterator<Item = usize>) -> Vec<Option<usize>> {
    let mut out: Vec<Option<usize>> = rows.map(Some).collect();
    out.resize(len, None);
    out.truncate(len);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::{Branch, Bus, Extras, Generator, Hvdc, Load, SourceFormat, Storage};
    use crate::parse_file;

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-12
    }

    fn bus(id: usize, kind: BusType) -> Bus {
        Bus {
            id: BusId(id),
            kind,
            vm: 1.0,
            va: 0.0,
            base_kv: 230.0,
            vmax: 1.1,
            vmin: 0.9,
            evhi: None,
            evlo: None,
            area: 1,
            zone: 1,
            name: None,
            uid: None,
            location: None,
            extras: Extras::new(),
        }
    }

    fn branch(from: usize, to: usize, in_service: bool) -> Branch {
        Branch {
            from: BusId(from),
            to: BusId(to),
            r: 0.01,
            x: 0.1,
            b: 0.02,
            charging: None,
            rate_a: 100.0,
            rate_b: 110.0,
            rate_c: 120.0,
            rating_sets: Vec::new(),
            current_ratings: None,
            tap: 0.0,
            shift: 30.0,
            in_service,
            angmin: -360.0,
            angmax: 360.0,
            control: None,
            solution: None,
            uid: None,
            route: None,
            extras: Extras::new(),
        }
    }

    fn generator(bus: usize, in_service: bool) -> Generator {
        Generator {
            bus: BusId(bus),
            pg: 50.0,
            qg: 5.0,
            pmax: 80.0,
            pmin: 0.0,
            qmax: 40.0,
            qmin: -40.0,
            vg: 1.0,
            mbase: 100.0,
            in_service,
            cost: None,
            caps: [None; crate::network::GEN_EXTRA_KEYS.len()],
            regulated_bus: None,
            uid: None,
        }
    }

    #[test]
    fn solver_tables_are_dense_normalized_and_traceable() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/case14.m");
        let net = parse_file(path, None).unwrap().network;

        let tables = net.to_normalized_solver_tables().unwrap();

        assert_eq!(tables.pass, NORMALIZED_SOLVER_TABLES_PASS);
        assert_eq!(tables.units.power, "per_unit");
        assert_eq!(tables.units.angle, "radian");
        assert_eq!(tables.buses.len(), 14);
        assert_eq!(tables.branches.len(), 20);
        assert_eq!(tables.arcs.len(), 40);
        assert_eq!(tables.index.reference_bus_indices, vec![0]);
        assert_eq!(tables.index.branch_from_arc_indices[0], 0);
        assert_eq!(tables.index.branch_to_arc_indices[0], 1);
        assert_eq!(tables.arcs[0].terminal, SolverArcTerminal::From);
        assert_eq!(tables.arcs[1].terminal, SolverArcTerminal::To);
        assert!(tables.index.bus_source_rows.iter().all(Option::is_some));
        assert!(tables.index.branch_source_rows.iter().all(Option::is_some));

        let bus_2 = &tables.buses[1];
        assert_eq!(bus_2.bus_id, BusId(2));
        assert!(approx(bus_2.pd, 21.7 / 100.0));
        assert!(approx(bus_2.qd, 12.7 / 100.0));
    }

    #[test]
    fn solver_tables_filter_out_of_service_rows_and_keep_source_rows() {
        let mut net = Network::in_memory(
            "filtered",
            100.0,
            vec![
                bus(1, BusType::Ref),
                bus(2, BusType::Pq),
                bus(3, BusType::Isolated),
            ],
            vec![branch(1, 2, true), branch(1, 3, true), branch(1, 2, false)],
        );
        net.loads.push(Load {
            bus: BusId(2),
            p: 10.0,
            q: 5.0,
            voltage_model: None,
            in_service: true,
            uid: None,
            extras: Extras::new(),
        });
        net.loads.push(Load {
            bus: BusId(3),
            p: 99.0,
            q: 99.0,
            voltage_model: None,
            in_service: true,
            uid: None,
            extras: Extras::new(),
        });
        net.generators.push(generator(1, true));
        net.generators.push(generator(2, false));
        net.source_format = SourceFormat::Matpower;

        let tables = net.to_normalized_solver_tables().unwrap();

        assert_eq!(tables.index.bus_ids, vec![BusId(1), BusId(2)]);
        assert_eq!(tables.branches.len(), 1);
        assert_eq!(tables.loads.len(), 1);
        assert_eq!(tables.generators.len(), 1);
        assert_eq!(tables.index.branch_source_rows, vec![Some(0)]);
        assert_eq!(tables.index.load_source_rows, vec![Some(0)]);
        assert_eq!(tables.index.generator_source_rows, vec![Some(0)]);
        assert!(approx(tables.loads[0].p, 0.1));
        assert!(approx(tables.branches[0].rate_a, 1.0));
        assert!(approx(tables.branches[0].tap, 1.0));
        assert!(approx(tables.branches[0].shift, 30.0_f64.to_radians()));
    }

    #[test]
    fn solver_tables_do_not_scale_an_already_normalized_network_twice() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/case14.m");
        let net = parse_file(path, None).unwrap().network;
        let normalized = net.to_normalized().unwrap();

        let tables = normalized.to_normalized_solver_tables().unwrap();

        let bus_2 = &tables.buses[1];
        assert!(approx(bus_2.pd, 21.7 / 100.0));
        assert!(approx(bus_2.qd, 12.7 / 100.0));
    }

    #[test]
    fn solver_tables_scale_storage_and_hvdc_power_fields_to_per_unit() {
        let mut net = Network::in_memory(
            "storage-hvdc",
            100.0,
            vec![bus(1, BusType::Ref), bus(2, BusType::Pq)],
            Vec::new(),
        );
        net.generators.push(generator(1, true));
        net.storage.push(Storage {
            bus: BusId(2),
            ps: 30.0,
            qs: -10.0,
            energy: 50.0,
            energy_rating: 100.0,
            charge_rating: 20.0,
            discharge_rating: 25.0,
            charge_efficiency: 0.9,
            discharge_efficiency: 0.85,
            thermal_rating: 40.0,
            current_rating: None,
            qmin: -15.0,
            qmax: 15.0,
            r: 0.01,
            x: 0.02,
            p_loss: 2.0,
            q_loss: 1.0,
            in_service: true,
            uid: None,
            extras: Extras::new(),
        });
        net.hvdc.push(Hvdc {
            from: BusId(1),
            to: BusId(2),
            in_service: true,
            pf: 20.0,
            pt: -19.0,
            qf: 5.0,
            qt: -4.0,
            vf: 1.0,
            vt: 1.0,
            pmin: -40.0,
            pmax: 75.0,
            qminf: -25.0,
            qmaxf: 30.0,
            qmint: -20.0,
            qmaxt: 22.0,
            loss0: 1.5,
            loss1: 0.02,
            cost: None,
            uid: None,
            extras: Extras::new(),
        });

        let tables = net.to_normalized_solver_tables().unwrap();

        let storage = &tables.storage[0];
        assert!(approx(storage.ps, 0.3));
        assert!(approx(storage.qs, -0.1));
        assert!(approx(storage.energy, 0.5));
        assert!(approx(storage.thermal_rating, 0.4));
        assert!(approx(storage.p_loss, 0.02));

        let hvdc = &tables.hvdc[0];
        assert!(approx(hvdc.pf, 0.2));
        assert!(approx(hvdc.pt, -0.19));
        assert!(approx(hvdc.pmin, -0.4));
        assert!(approx(hvdc.pmax, 0.75));
        assert!(approx(hvdc.qminf, -0.25));
        assert!(approx(hvdc.loss0, 0.015));
    }
}
