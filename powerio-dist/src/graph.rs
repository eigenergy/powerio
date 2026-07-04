//! Render ready bus and terminal graph projection.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::{
    DistBus, DistIbr, DistNetwork, DistTransformer, VoltageSource, Winding, pair_keys,
};

/// A collapsed bus graph with terminal level attachments and conductor maps.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DistGraph {
    pub buses: Vec<DistGraphBus>,
    pub edges: Vec<DistGraphEdge>,
}

/// One bus node in the collapsed graph.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DistGraphBus {
    pub id: String,
    pub terminals: Vec<String>,
    pub grounded: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xy: Option<[f64; 2]>,
    pub load_kw: f64,
    pub gen_kw: f64,
    pub has_source: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub terminal_attachments: BTreeMap<String, Vec<DistGraphAttachment>>,
}

/// An element connected to a bus terminal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DistGraphAttachment {
    pub kind: DistGraphAttachmentKind,
    pub id: String,
}

/// Element family for terminal attachments.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DistGraphAttachmentKind {
    Load,
    Generator,
    Ibr,
    Shunt,
    Source,
}

/// One edge in the collapsed graph.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DistGraphEdge {
    pub kind: DistGraphEdgeKind,
    pub id: String,
    pub from: String,
    pub to: String,
    /// `(from_terminal, to_terminal)` pairs in conductor order.
    pub conductors: Vec<(String, String)>,
    pub closed: bool,
    pub n_phases: usize,
}

/// Edge family in the collapsed graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DistGraphEdgeKind {
    Line,
    Switch,
    Transformer,
}

impl DistNetwork {
    /// Project this network into a render ready bus and terminal graph.
    #[must_use]
    pub fn graph(&self) -> DistGraph {
        DistGraph::from_network(self)
    }
}

impl DistGraph {
    /// Build the graph projection for one distribution network.
    #[must_use]
    pub fn from_network(net: &DistNetwork) -> Self {
        let mut builder = GraphBuilder::new(&net.buses);

        for line in &net.lines {
            let from = builder.canonical_bus_id(&line.bus_from);
            let to = builder.canonical_bus_id(&line.bus_to);
            builder.edges.push(DistGraphEdge {
                kind: DistGraphEdgeKind::Line,
                id: line.name.clone(),
                from,
                to,
                conductors: conductor_pairs(&line.terminal_map_from, &line.terminal_map_to),
                closed: true,
                n_phases: line.terminal_map_from.len().min(line.terminal_map_to.len()),
            });
        }

        for switch in &net.switches {
            let from = builder.canonical_bus_id(&switch.bus_from);
            let to = builder.canonical_bus_id(&switch.bus_to);
            builder.edges.push(DistGraphEdge {
                kind: DistGraphEdgeKind::Switch,
                id: switch.name.clone(),
                from,
                to,
                conductors: conductor_pairs(&switch.terminal_map_from, &switch.terminal_map_to),
                closed: !switch.open,
                n_phases: switch
                    .terminal_map_from
                    .len()
                    .min(switch.terminal_map_to.len()),
            });
        }

        for transformer in &net.transformers {
            builder.add_transformer_edges(transformer);
        }

        for load in &net.loads {
            builder.add_load(
                &load.bus,
                &load.terminal_map,
                &load.name,
                watts_to_kw(&load.p_nom),
            );
        }
        for generator in &net.generators {
            builder.add_generator(
                &generator.bus,
                &generator.terminal_map,
                &generator.name,
                watts_to_kw(&generator.p_nom),
            );
        }
        for ibr in &net.ibrs {
            builder.add_ibr(ibr);
        }
        for shunt in &net.shunts {
            builder.add_attachment(
                &shunt.bus,
                &shunt.terminal_map,
                DistGraphAttachmentKind::Shunt,
                &shunt.name,
            );
        }
        for source in &net.sources {
            builder.add_source(source);
        }

        DistGraph {
            buses: builder.buses,
            edges: builder.edges,
        }
    }
}

struct GraphBuilder {
    buses: Vec<DistGraphBus>,
    bus_index: BTreeMap<String, usize>,
    edges: Vec<DistGraphEdge>,
}

impl GraphBuilder {
    fn new(buses: &[DistBus]) -> Self {
        let mut builder = Self {
            buses: Vec::new(),
            bus_index: BTreeMap::new(),
            edges: Vec::new(),
        };
        for bus in buses {
            builder.push_bus(bus);
        }
        builder
    }

    fn push_bus(&mut self, bus: &DistBus) {
        let index = self.buses.len();
        self.bus_index.insert(bus_key(&bus.id), index);
        self.buses.push(DistGraphBus {
            id: bus.id.clone(),
            terminals: bus.terminals.clone(),
            grounded: bus.grounded.clone(),
            xy: bus_xy(bus),
            load_kw: 0.0,
            gen_kw: 0.0,
            has_source: false,
            terminal_attachments: bus
                .terminals
                .iter()
                .map(|terminal| (terminal.clone(), Vec::new()))
                .collect(),
        });
    }

    fn bus_index(&mut self, id: &str) -> usize {
        let key = bus_key(id);
        if let Some(index) = self.bus_index.get(&key) {
            return *index;
        }
        let index = self.buses.len();
        self.bus_index.insert(key, index);
        self.buses.push(DistGraphBus {
            id: id.to_owned(),
            terminals: Vec::new(),
            grounded: Vec::new(),
            xy: None,
            load_kw: 0.0,
            gen_kw: 0.0,
            has_source: false,
            terminal_attachments: BTreeMap::new(),
        });
        index
    }

    fn canonical_bus_id(&mut self, id: &str) -> String {
        let index = self.bus_index(id);
        self.buses[index].id.clone()
    }

    fn add_transformer_edges(&mut self, transformer: &DistTransformer) {
        for (from_idx, to_idx) in pair_keys(transformer.windings.len()) {
            let Some(from_winding) = transformer.windings.get(from_idx) else {
                continue;
            };
            let Some(to_winding) = transformer.windings.get(to_idx) else {
                continue;
            };
            let from = self.canonical_bus_id(&from_winding.bus);
            let to = self.canonical_bus_id(&to_winding.bus);
            self.edges.push(transformer_edge(
                transformer,
                from,
                to,
                from_winding,
                to_winding,
            ));
        }
    }

    fn add_load(&mut self, bus: &str, terminals: &[String], id: &str, load_kw: f64) {
        let index = self.bus_index(bus);
        self.buses[index].load_kw += load_kw;
        self.add_attachment(bus, terminals, DistGraphAttachmentKind::Load, id);
    }

    fn add_generator(&mut self, bus: &str, terminals: &[String], id: &str, gen_kw: f64) {
        let index = self.bus_index(bus);
        self.buses[index].gen_kw += gen_kw;
        self.add_attachment(bus, terminals, DistGraphAttachmentKind::Generator, id);
    }

    fn add_ibr(&mut self, ibr: &DistIbr) {
        let index = self.bus_index(&ibr.bus);
        self.buses[index].gen_kw += ibr_kw(ibr);
        self.add_attachment(
            &ibr.bus,
            &ibr.terminal_map,
            DistGraphAttachmentKind::Ibr,
            &ibr.name,
        );
    }

    fn add_source(&mut self, source: &VoltageSource) {
        let index = self.bus_index(&source.bus);
        self.buses[index].has_source = true;
        self.add_attachment(
            &source.bus,
            &source.terminal_map,
            DistGraphAttachmentKind::Source,
            &source.name,
        );
    }

    fn add_attachment(
        &mut self,
        bus: &str,
        terminals: &[String],
        kind: DistGraphAttachmentKind,
        id: &str,
    ) {
        let index = self.bus_index(bus);
        let attachment = DistGraphAttachment {
            kind,
            id: id.to_owned(),
        };
        if terminals.is_empty() {
            self.buses[index]
                .terminal_attachments
                .entry(String::new())
                .or_default()
                .push(attachment);
            return;
        }
        for terminal in terminals {
            self.buses[index]
                .terminal_attachments
                .entry(terminal.clone())
                .or_default()
                .push(attachment.clone());
        }
    }
}

fn transformer_edge(
    transformer: &DistTransformer,
    from: String,
    to: String,
    from_winding: &Winding,
    to_winding: &Winding,
) -> DistGraphEdge {
    DistGraphEdge {
        kind: DistGraphEdgeKind::Transformer,
        id: transformer.name.clone(),
        from,
        to,
        conductors: conductor_pairs(&from_winding.terminal_map, &to_winding.terminal_map),
        closed: true,
        n_phases: transformer.phases,
    }
}

fn conductor_pairs(from: &[String], to: &[String]) -> Vec<(String, String)> {
    from.iter().cloned().zip(to.iter().cloned()).collect()
}

fn watts_to_kw(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / 1000.0
}

fn ibr_kw(ibr: &DistIbr) -> f64 {
    ibr.p_avail
        .or_else(|| ibr.p_max.as_ref().map(|p| p.iter().sum()))
        .unwrap_or(0.0)
        / 1000.0
}

fn bus_key(id: &str) -> String {
    id.to_ascii_lowercase()
}

fn bus_xy(bus: &DistBus) -> Option<[f64; 2]> {
    let x = number_extra(&bus.extras, &["x", "lon", "lng", "longitude"])?;
    let y = number_extra(&bus.extras, &["y", "lat", "latitude"])?;
    Some([x, y])
}

fn number_extra(extras: &BTreeMap<String, serde_json::Value>, names: &[&str]) -> Option<f64> {
    names.iter().find_map(|name| {
        extras
            .get(*name)
            .and_then(serde_json::Value::as_f64)
            .filter(|value| value.is_finite())
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::model::{Configuration, DistGenerator, DistLoad};

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    fn assert_close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-12, "{actual} != {expected}");
    }

    fn fixture(path: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/data/dist")
            .join(path)
    }

    fn bus<'a>(graph: &'a DistGraph, id: &str) -> &'a DistGraphBus {
        graph
            .buses
            .iter()
            .find(|bus| bus.id.eq_ignore_ascii_case(id))
            .expect("graph bus exists")
    }

    fn edge<'a>(graph: &'a DistGraph, kind: DistGraphEdgeKind, id: &str) -> &'a DistGraphEdge {
        graph
            .edges
            .iter()
            .find(|edge| edge.kind == kind && edge.id.eq_ignore_ascii_case(id))
            .expect("graph edge exists")
    }

    #[test]
    fn graph_projects_open_switch_fixture() {
        let net = crate::parse_file(fixture("micro/switch.dss"), None).expect("parse switch");
        let graph = net.graph();

        let open = edge(&graph, DistGraphEdgeKind::Switch, "sw_open");
        assert!(!open.closed);
        assert_eq!(open.from, "mid");
        assert_eq!(open.to, "stub");
        assert_eq!(
            open.conductors,
            vec![
                ("1".to_owned(), "1".to_owned()),
                ("2".to_owned(), "2".to_owned()),
                ("3".to_owned(), "3".to_owned())
            ]
        );
        assert_eq!(open.n_phases, 3);

        let closed = edge(&graph, DistGraphEdgeKind::Switch, "sw_closed");
        assert!(closed.closed);

        let sourcebus = bus(&graph, "sourcebus");
        assert!(sourcebus.has_source);
        let loadbus = bus(&graph, "loadbus");
        assert_close(loadbus.load_kw, 500.0);
        assert!(
            loadbus
                .terminal_attachments
                .get("1")
                .expect("terminal attachment")
                .iter()
                .any(
                    |attachment| attachment.kind == DistGraphAttachmentKind::Load
                        && attachment.id == "l1"
                )
        );
    }

    #[test]
    fn graph_projects_one_edge_per_transformer_winding_pair() {
        let net =
            crate::parse_file(fixture("micro/xfmr_center_tap.dss"), None).expect("parse xfmr");
        let graph = net.graph();
        let transformer_edges: Vec<_> = graph
            .edges
            .iter()
            .filter(|edge| edge.kind == DistGraphEdgeKind::Transformer && edge.id == "t1")
            .collect();

        assert_eq!(transformer_edges.len(), 3);
        assert!(
            transformer_edges
                .iter()
                .any(|edge| edge.from == "sourcebus" && edge.to == "secondary")
        );
        assert!(
            transformer_edges
                .iter()
                .any(|edge| edge.from == "secondary" && edge.to == "secondary")
        );
        assert!(
            transformer_edges
                .iter()
                .all(|edge| edge.closed && edge.n_phases == 1)
        );

        let secondary = bus(&graph, "secondary");
        assert_close(secondary.load_kw, 15.0);
    }

    #[test]
    fn graph_projects_bmopf_fixture() {
        let net =
            crate::parse_file(fixture("bmopf/example_ieee13.json"), None).expect("parse bmopf");
        let graph = net.graph();

        assert!(graph.buses.len() >= net.buses.len());
        assert!(
            graph
                .edges
                .iter()
                .any(|edge| edge.kind == DistGraphEdgeKind::Line)
        );
        assert!(
            graph
                .edges
                .iter()
                .any(|edge| edge.kind == DistGraphEdgeKind::Transformer)
        );
    }

    #[test]
    fn graph_accumulates_terminal_attachments_and_generation() {
        let mut net = DistNetwork::new();
        net.buses
            .push(DistBus::new("b1", strings(&["a", "b", "n"])));
        net.loads.push(DistLoad::new(
            "load",
            "b1",
            strings(&["a", "n"]),
            Configuration::Wye,
            vec![1000.0],
            vec![0.0],
        ));
        net.generators.push(DistGenerator::new(
            "gen",
            "b1",
            strings(&["b", "n"]),
            Configuration::Wye,
            vec![2000.0],
            vec![0.0],
        ));
        net.sources.push(VoltageSource::new(
            "source",
            "b1",
            strings(&["a", "b", "n"]),
            vec![1.0, 1.0, 0.0],
            vec![0.0, 0.0, 0.0],
        ));

        let graph = net.graph();
        let b1 = bus(&graph, "b1");

        assert_close(b1.load_kw, 1.0);
        assert_close(b1.gen_kw, 2.0);
        assert!(b1.has_source);
        assert_eq!(
            b1.terminal_attachments
                .get("n")
                .expect("neutral attachments")
                .len(),
            3
        );
    }

    #[test]
    fn graph_uses_extra_coordinates_when_present() {
        let mut bus = DistBus::new("b1", strings(&["1"]));
        bus.extras
            .insert("longitude".into(), serde_json::json!(-80.0));
        bus.extras
            .insert("latitude".into(), serde_json::json!(35.0));
        let net = DistNetwork {
            buses: vec![bus],
            ..DistNetwork::new()
        };

        assert_eq!(net.graph().buses[0].xy, Some([-80.0, 35.0]));
    }
}
