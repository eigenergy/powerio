//! The Schneider Electric ADMS extension of CIM16
//! (`http://www.schneider-electric-dms.com/CIM16v33/2017/extension`, read as
//! `sedms:` whatever prefix a document declares).
//!
//! An LV network exported with it connects its customers without an
//! `EnergyConsumer`: a `ServiceLocation` names the terminal it is served from
//! (`sedms:ServiceLocation.Terminal`) and its phases
//! (`sedms:ServiceLocation.phases`), each `UsagePoint` at the location
//! states its demand, and a `FlexibleLoad` of a usage point is the
//! generation behind that meter. The export leaves out the substation that
//! supplies the network and marks the nodes where it joins that supply
//! (`sedms:ConnectivityNode.stitchingInfo`).
//!
//! The reader calls into this module only when a document uses the
//! extension namespace; every value it fills in is a `READ.CIM_DIST.*`
//! finding.

use std::collections::{BTreeSet, HashMap};

use super::super::xml::CimDocument;
use super::{
    Configuration, DistBus, DistCoordsKind, DistIbr, DistLoad, DistLocation, DistSwitch,
    IbrPrimeMover, IbrTopology, Mapper, Shapes, VoltageSource, expand_phase_code, three_phases,
    with_neutral, wye_source,
};
use crate::diagnostics::{Diagnostic, codes as C};

/// What the extension adds to the network: loads from usage points,
/// inverter based resources from flexible loads, and the supply the export
/// leaves out.
#[derive(Default)]
pub(super) struct Extension {
    pub(super) loads: Vec<DistLoad>,
    pub(super) ibrs: Vec<DistIbr>,
    pub(super) supply: Supply,
}

/// Where a `ServiceLocation` connects: the bus of the terminal it is served
/// from, its phase conductors, and whether it brings out the neutral.
#[derive(Clone)]
pub(super) struct ServicePoint {
    bus: String,
    phases: Vec<String>,
    neutral: bool,
}

/// The supply the reader places for a network exported without its source:
/// the busbars, the switches that join them to the stitched nodes, and the
/// sources on them.
#[derive(Default)]
pub(super) struct Supply {
    pub(super) buses: Vec<String>,
    pub(super) switches: Vec<DistSwitch>,
    pub(super) sources: Vec<VoltageSource>,
}

/// Whether a document uses the extension: the XML reader gives its
/// properties the `sedms:` prefix only when they are in its namespace.
pub(super) fn uses_extension(doc: &CimDocument) -> bool {
    doc.objects
        .iter()
        .flat_map(|object| &object.props)
        .any(|(key, _)| key.starts_with("sedms:"))
}

impl Mapper<'_> {
    /// Everything the extension maps.
    pub(super) fn sedms_extension(&mut self) -> Extension {
        Extension {
            loads: self.usage_point_loads(),
            ibrs: self.flexible_loads(),
            supply: self.stitched_supply(),
        }
    }

    /// A model name no element of table `table` holds yet: `base`, else
    /// `base_2`, `base_3`, …, for an element the reader places itself.
    fn fresh_name(&mut self, table: &'static str, base: &str) -> String {
        let taken = self.taken.entry(table).or_default();
        let mut candidate = base.to_owned();
        let mut n = 1;
        while !taken.insert(candidate.to_ascii_lowercase()) {
            n += 1;
            candidate = format!("{base}_{n}");
        }
        candidate
    }

    // -- service locations ---------------------------------------------------

    /// The service location a usage point connects through, when the
    /// location names the terminal it is served from.
    fn served_location(&mut self, usage_point: &str) -> Option<String> {
        let location = self.target("UsagePoint", usage_point, "UsagePoint.ServiceLocation")?;
        self.store
            .refv(&location, "sedms:ServiceLocation.Terminal")
            .is_some()
            .then_some(location)
    }

    /// Where a service location connects: the bus of the terminal it is
    /// served from, and the phases it states, else its terminal's.
    fn service_point(&mut self, location: &str) -> Option<ServicePoint> {
        if let Some(point) = self.service_points.get(location) {
            return point.clone();
        }
        let point = self.resolve_service_point(location);
        self.consumed.insert(location.to_owned());
        if point.is_none() {
            // The usage points and resources there are dropped with a
            // finding that names the location.
            self.dropped.insert(location.to_owned());
        }
        self.service_points
            .insert(location.to_owned(), point.clone());
        point
    }

    fn resolve_service_point(&mut self, location: &str) -> Option<ServicePoint> {
        let terminal = self.target(
            "ServiceLocation",
            location,
            "sedms:ServiceLocation.Terminal",
        )?;
        let bus = self.bus_of_terminal(&terminal)?;
        let stated = self
            .store
            .enum_value(location, "sedms:ServiceLocation.phases")
            .map(str::to_owned);
        let map = match stated.as_deref().and_then(expand_phase_code) {
            Some(map) if map.iter().any(|t| t != "4") => map,
            _ => {
                let label = self.label(location);
                let why = match &stated {
                    Some(code) => format!("phase code `{code}` names no phase conductor"),
                    None => "no sedms:ServiceLocation.phases".to_owned(),
                };
                self.note(
                    &C::READ_CIM_DIST_VALUE_DEFAULTED,
                    "ServiceLocation",
                    location,
                    format!("ServiceLocation {label}: {why}; the phases of its terminal are used"),
                );
                self.terminal_phases(&terminal).unwrap_or_else(three_phases)
            }
        };
        let neutral = map.iter().any(|t| t == "4");
        let phases: Vec<String> = map.into_iter().filter(|t| t != "4").collect();
        Some(ServicePoint {
            bus,
            phases,
            neutral,
        })
    }

    /// A stated power of a usage point or resource, or 0 with a finding.
    fn stated_power(&mut self, class: &str, id: &str, key: &str) -> f64 {
        self.store.f(id, key).unwrap_or_else(|| {
            let label = self.label(id);
            self.note(
                &C::READ_CIM_DIST_VALUE_DEFAULTED,
                class,
                id,
                format!("{class} {label}: no {key}; 0 is used"),
            );
            0.0
        })
    }

    /// One load per `UsagePoint` (one per meter) at its service location,
    /// with the stated `sedms:UsagePoint.p` and `.q` (W and var) shared
    /// evenly over the location's phases. A location with the neutral is a
    /// wye or single phase load; one without it is delta on three phases and
    /// phase to phase on two.
    fn usage_point_loads(&mut self) -> Vec<DistLoad> {
        let mut loads = Vec::new();
        for id in self.store.ids_of("UsagePoint") {
            let Some(location) = self.served_location(&id) else {
                continue;
            };
            self.consumed.insert(id.clone());
            let Some(point) = self.service_point(&location) else {
                let location = self.label(&location);
                self.drop_record(
                    "UsagePoint",
                    &id,
                    &format!(
                        "its ServiceLocation {location} is not served from a connectivity node"
                    ),
                );
                continue;
            };
            let p = self.stated_power("UsagePoint", &id, "sedms:UsagePoint.p");
            let q = self.stated_power("UsagePoint", &id, "sedms:UsagePoint.q");
            #[allow(clippy::cast_precision_loss)] // at most three phases
            let n = point.phases.len() as f64;
            let even = |total: f64| vec![total / n; point.phases.len()];
            let (configuration, map, p_nom, q_nom) = match (point.neutral, point.phases.len()) {
                (_, 1) => (
                    Configuration::SinglePhase,
                    with_neutral(point.phases.clone(), Configuration::SinglePhase),
                    vec![p],
                    vec![q],
                ),
                (false, 2) => (
                    Configuration::SinglePhase,
                    point.phases.clone(),
                    vec![p],
                    vec![q],
                ),
                (false, _) => (Configuration::Delta, point.phases.clone(), even(p), even(q)),
                (true, _) => (
                    Configuration::Wye,
                    with_neutral(point.phases.clone(), Configuration::Wye),
                    even(p),
                    even(q),
                ),
            };
            let name = self.unique_name("load", "UsagePoint", &id);
            let mut load = DistLoad::new(name, point.bus, map, configuration, p_nom, q_nom);
            load.extras.insert("cim_mrid".into(), id.clone().into());
            loads.push(load);
        }
        loads
    }

    /// One inverter based resource per `FlexibleLoad`, at the service
    /// location of the usage point it serves (`sedms:FlexibleLoad.Customer`).
    /// `sedms:FlexibleLoad.pMax` (W) is the nameplate. A resource that is not
    /// dispatchable holds its average output `pAvg` (W) at unity power
    /// factor, so its bounds are pinned there; a dispatchable one ranges from
    /// 0 to the nameplate.
    #[allow(clippy::too_many_lines)] // one resource shape per branch
    fn flexible_loads(&mut self) -> Vec<DistIbr> {
        let mut ibrs = Vec::new();
        for id in self.store.ids_of("FlexibleLoad") {
            let Some(customer) = self.target("FlexibleLoad", &id, "sedms:FlexibleLoad.Customer")
            else {
                continue;
            };
            self.consumed.insert(id.clone());
            let point = match self.served_location(&customer) {
                Some(location) => self.service_point(&location),
                None => None,
            };
            let Some(point) = point else {
                let customer = self.label(&customer);
                self.drop_record(
                    "FlexibleLoad",
                    &id,
                    &format!("its customer {customer} is not served from a connectivity node"),
                );
                continue;
            };
            let n = point.phases.len();
            let (topology, map) = match (n, point.neutral) {
                (1, _) => (
                    IbrTopology::SinglePhase,
                    with_neutral(point.phases.clone(), Configuration::SinglePhase),
                ),
                (3, false) => (IbrTopology::ThreeLeg, point.phases.clone()),
                (3, true) => (
                    IbrTopology::FourLeg,
                    with_neutral(point.phases.clone(), Configuration::Wye),
                ),
                _ => {
                    self.drop_record(
                        "FlexibleLoad",
                        &id,
                        &format!("an inverter on {n} phases has no topology in the model"),
                    );
                    continue;
                }
            };
            let label = self.label(&id);
            let prime_mover = match self
                .store
                .enum_value(&id, "sedms:FlexibleLoad.resourceType")
                .map(str::to_owned)
            {
                Some(kind) if kind == "SolarPanel" => IbrPrimeMover::Pv,
                Some(kind) if kind == "Battery" => IbrPrimeMover::Battery,
                Some(kind) => {
                    self.note(
                        &C::READ_CIM_DIST_VALUE_UNSUPPORTED,
                        "FlexibleLoad",
                        &id,
                        format!(
                            "FlexibleLoad {label}: resource type `{kind}` is read as a generic \
                             inverter based resource"
                        ),
                    );
                    IbrPrimeMover::Generic
                }
                None => {
                    self.note(
                        &C::READ_CIM_DIST_VALUE_DEFAULTED,
                        "FlexibleLoad",
                        &id,
                        format!(
                            "FlexibleLoad {label}: no resourceType; a generic inverter based \
                             resource is assumed"
                        ),
                    );
                    IbrPrimeMover::Generic
                }
            };
            let rated = self.stated_power("FlexibleLoad", &id, "sedms:FlexibleLoad.pMax");
            let average = self.stated_power("FlexibleLoad", &id, "sedms:FlexibleLoad.pAvg");
            let dispatchable = self
                .store
                .boolean(&id, "sedms:FlexibleLoad.dispatchable")
                .unwrap_or_else(|| {
                    self.note(
                        &C::READ_CIM_DIST_VALUE_DEFAULTED,
                        "FlexibleLoad",
                        &id,
                        format!(
                            "FlexibleLoad {label}: no dispatchable flag; it holds its average \
                             output"
                        ),
                    );
                    false
                });
            #[allow(clippy::cast_precision_loss)] // at most three phases
            let per_phase = |total: f64| vec![total / n as f64; n];
            let name = self.unique_name("ibr", "FlexibleLoad", &id);
            let mut ibr = DistIbr::new(
                name,
                point.bus,
                map,
                topology,
                prime_mover,
                per_phase(rated),
            );
            if dispatchable {
                ibr.p_avail = Some(rated);
                ibr.p_min = Some(vec![0.0; n]);
                ibr.p_max = Some(per_phase(rated));
            } else {
                ibr.p_avail = Some(average);
                ibr.p_min = Some(per_phase(average));
                ibr.p_max = Some(per_phase(average));
                ibr.q_min = Some(vec![0.0; n]);
                ibr.q_max = Some(vec![0.0; n]);
            }
            ibr.extras.insert("cim_mrid".into(), id.clone().into());
            ibrs.push(ibr);
        }
        ibrs
    }

    // -- supply ------------------------------------------------------------

    /// The supply of a network exported without its source. A Schneider
    /// Electric ADMS export cuts an LV network out at the nodes where it
    /// joins its supply, marks each with `sedms:ConnectivityNode.stitchingInfo`,
    /// and names the circuits the node joins
    /// (`sedms:ConnectivityNode.connectedCircuits`, the upstream circuit
    /// first). When the documents define no `EnergySource`, the nodes that
    /// join the same circuits hang off one busbar the export leaves out: the
    /// reader adds that bus, a closed switch from it to each node named after
    /// the node's container (the feeder), and an ideal three phase source on
    /// it at the rated voltage of the nodes' equipment
    /// (`sedms:ConductingEquipment.ratedVoltage`), 1 pu and 0°.
    fn stitched_supply(&mut self) -> Supply {
        let mut supply = Supply::default();
        if self.store.count_of("EnergySource") > 0 {
            return supply;
        }
        // Stitched nodes grouped by the circuits they join, in document order.
        let mut groups: Vec<(String, String, Vec<String>)> = Vec::new();
        for node in self.store.ids_of("ConnectivityNode") {
            let Some(info) = self
                .store
                .text(&node, "sedms:ConnectivityNode.stitchingInfo")
                .map(str::trim)
                .filter(|info| !info.is_empty())
                .map(str::to_owned)
            else {
                continue;
            };
            let circuits = self
                .store
                .text(&node, "sedms:ConnectivityNode.connectedCircuits")
                .map_or_else(String::new, |c| c.trim().to_owned());
            match groups.iter_mut().find(|(c, _, _)| *c == circuits) {
                Some((_, _, nodes)) => nodes.push(node),
                None => groups.push((circuits, info, vec![node])),
            }
        }
        if groups.is_empty() {
            return supply;
        }
        let mut terminals_at: HashMap<String, Vec<String>> = HashMap::new();
        for terminal in self.store.ids_of("Terminal") {
            if let Some(node) = self.wiring.node_of.get(&terminal) {
                terminals_at.entry(node.clone()).or_default().push(terminal);
            }
        }
        for (circuits, info, nodes) in groups {
            self.supply_busbar(&mut supply, &terminals_at, &circuits, &info, &nodes);
        }
        supply
    }

    /// One busbar of [`Self::stitched_supply`] for the `nodes` stitched to
    /// `circuits`: each node's switch takes the phases of the terminals
    /// there, and the source the first rated voltage their equipment states.
    fn supply_busbar(
        &mut self,
        supply: &mut Supply,
        terminals_at: &HashMap<String, Vec<String>>,
        circuits: &str,
        info: &str,
        nodes: &[String],
    ) {
        let mut rated: Option<f64> = None;
        let mut disagree = false;
        let mut feeders = Vec::new();
        for node in nodes {
            let mut phases: BTreeSet<String> = BTreeSet::new();
            for terminal in terminals_at.get(node).map_or(&[][..], Vec::as_slice) {
                if let Some(stated) = self.terminal_phases(terminal) {
                    phases.extend(stated);
                }
                let stated = self
                    .store
                    .refv(terminal, "Terminal.ConductingEquipment")
                    .map(str::to_owned)
                    .and_then(|equipment| {
                        self.store
                            .f(&equipment, "sedms:ConductingEquipment.ratedVoltage")
                    })
                    .filter(|v| *v > 0.0);
                match (rated, stated) {
                    (None, Some(v)) => rated = Some(v),
                    (Some(first), Some(v)) => {
                        disagree |= (v - first).abs() > 1e-6 * first;
                    }
                    _ => {}
                }
            }
            let mut map: Vec<String> = phases.into_iter().collect();
            if map.is_empty() {
                map = three_phases();
            }
            let container = self
                .store
                .refv(node, "ConnectivityNode.ConnectivityNodeContainer")
                .map(str::to_owned);
            feeders.push((self.bus_of_node[node].clone(), map, container));
        }
        let labels: Vec<String> = nodes.iter().map(|n| self.label(n)).collect();
        let Some(rated) = rated else {
            self.diags.record(Diagnostic::of(
                &C::READ_CIM_DIST_VALUE_UNSUPPORTED,
                format!(
                    "the documents define no EnergySource, and the equipment at the \
                     ConnectivityNodes stitched to `{circuits}` ({}) states no \
                     sedms:ConductingEquipment.ratedVoltage; no supply is placed",
                    labels.join(", ")
                ),
            ));
            return;
        };
        if disagree {
            self.diags.record(Diagnostic::of(
                &C::READ_CIM_DIST_VALUE_CONFLICT,
                format!(
                    "the equipment at the ConnectivityNodes stitched to `{circuits}` ({}) states \
                     different sedms:ConductingEquipment.ratedVoltage values; the source takes \
                     the first, {rated} V",
                    labels.join(", ")
                ),
            ));
        }
        // The export states voltages as single precision kilovolts (0.4 kV
        // reads 400.0000059604645 V); the source takes the rated voltage at
        // that precision.
        #[allow(clippy::cast_possible_truncation)]
        let v_ll = f64::from(rated as f32);
        let busbar = self.fresh_name("bus", "sourcebus");
        for (bus, map, container) in feeders {
            let name = match container {
                Some(container) => self.fresh_name("switch", &container),
                None => self.fresh_name("switch", &format!("{bus}_supply")),
            };
            let mut switch = DistSwitch::new(name, busbar.clone(), bus, map.clone(), map, false);
            // The export does not say which device the bay has.
            switch.extras.insert("cim_class".into(), "Switch".into());
            supply.switches.push(switch);
        }
        let source = self.fresh_name("source", "source");
        supply.sources.push(wye_source(
            source.clone(),
            busbar.clone(),
            three_phases(),
            v_ll,
            0.0,
        ));
        let mut diagnostic = Diagnostic::of(
            &C::READ_CIM_DIST_VALUE_DEFAULTED,
            format!(
                "the documents define no EnergySource; {} ConnectivityNode(s) are stitched \
                 (sedms:ConnectivityNode.stitchingInfo `{info}`) to `{circuits}`: {}. The reader \
                 joins them at a new bus `{busbar}` through closed switches and places the ideal \
                 source `{source}` there at their rated {v_ll} V, 1 pu and 0°",
                nodes.len(),
                labels.join(", ")
            ),
        );
        let _ = diagnostic.insert_detail("class", serde_json::Value::from("ConnectivityNode"));
        let _ = diagnostic.insert_detail("count", serde_json::Value::from(nodes.len()));
        self.diags.record(diagnostic);
        supply.buses.push(busbar);
    }

    // -- layout ------------------------------------------------------------

    /// Whether a diagram is the export's GIS layout (`sedms:Diagram.type`),
    /// which draws in projected coordinates even when it names no CRS.
    pub(super) fn sedms_gis_diagram(&self, diagram: &str) -> bool {
        self.store
            .enum_value(diagram, "sedms:Diagram.type")
            .is_some_and(|kind| kind.eq_ignore_ascii_case("GIS"))
    }

    /// Place a node the equipment layout left unplaced at the point of a
    /// service location served from it.
    pub(super) fn place_service_locations(
        &self,
        shape_of: &Shapes,
        at_node: &mut HashMap<String, DistLocation>,
    ) {
        for location in self.store.ids_of("ServiceLocation") {
            let node = self
                .store
                .refv(&location, "sedms:ServiceLocation.Terminal")
                .and_then(|terminal| self.wiring.node_of.get(terminal));
            if let (Some(&(x, y)), Some(node)) = (
                shape_of.get(&location).and_then(|shape| shape.first()),
                node,
            ) {
                at_node.entry(node.clone()).or_insert(DistLocation {
                    x,
                    y,
                    kind: Some(DistCoordsKind::Derived),
                });
            }
        }
    }
}

/// Each supply busbar at the centroid of the buses its switches feed.
pub(super) fn locate_busbars(buses: &mut [DistBus], switches: &[DistSwitch], busbars: &[String]) {
    let located: HashMap<String, DistLocation> = buses
        .iter()
        .filter_map(|bus| bus.location.map(|location| (bus.id.clone(), location)))
        .collect();
    for busbar in busbars {
        let fed: Vec<DistLocation> = switches
            .iter()
            .filter(|switch| switch.bus_from == *busbar)
            .filter_map(|switch| located.get(&switch.bus_to).copied())
            .collect();
        if fed.is_empty() {
            continue;
        }
        #[allow(clippy::cast_precision_loss)] // a handful of feeders
        let n = fed.len() as f64;
        let (x, y) = fed.iter().fold((0.0, 0.0), |(x, y), p| (x + p.x, y + p.y));
        if let Some(bus) = buses.iter_mut().find(|bus| bus.id == *busbar) {
            bus.location = Some(DistLocation {
                x: x / n,
                y: y / n,
                kind: Some(DistCoordsKind::Derived),
            });
        }
    }
}
