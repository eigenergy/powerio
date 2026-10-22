use std::collections::BTreeSet;
use std::sync::Arc;

use powerio_tx::{
    Area, BalancedNetwork, Branch, Bus, BusId, BusType, DetailedConnectivity, Generator, Hvdc,
    Impedance, IslandReferencePolicy, Load, NormalizeSettings, Selector, Shunt,
    StaticVarCompensator, Storage, Switch, Transformer3W, Winding,
};
use serde_json::{Value, json};

fn id(kind: &str, name: &str) -> Value {
    json!({"component_type": kind, "local_id": name})
}

fn case() -> BalancedNetwork {
    let buses = [90, 10, 20, 30].map(|id| Bus::new(BusId(id), BusType::Pq, 230.0));
    let mut net = BalancedNetwork::in_memory("cut", 100.0, buses.to_vec(), vec![]);
    for bus in [90, 10, 30, 20] {
        net.loads_mut().push(Load::new(BusId(bus), 2.0, 1.0));
        net.shunts_mut().push(Shunt::new(BusId(bus), 0.0, 1.0));
        net.generators_mut().push(Generator::new(BusId(bus)));
        net.storage_mut().push(Storage::new(BusId(bus)));
        net.static_var_compensators_mut()
            .push(StaticVarCompensator::new(BusId(bus), -1.0, 1.0));
    }
    for (from, to) in [(90, 10), (10, 20), (20, 30)] {
        net.branches_mut()
            .push(Branch::new(BusId(from), BusId(to), 0.0, 0.1));
        net.switches_mut()
            .push(Switch::new(BusId(from), BusId(to), false));
        net.hvdc_mut().push(Hvdc::new(BusId(from), BusId(to)));
    }
    // Duplicate endpoint rows must not collapse or acquire one another's index.
    let mut inactive = net.branches()[1].clone();
    inactive.in_service = false;
    net.branches_mut().push(inactive);
    for ends in [[90, 10, 20], [10, 20, 30]] {
        let mut transformer = Transformer3W::new(
            ends.map(|bus| Winding::new(BusId(bus))),
            [Impedance::new(0.0, 0.1, 100.0); 3],
        );
        transformer.set_winding_in_service([true, true, false]);
        net.transformers_3w_mut().push(transformer);
    }
    net.areas_mut().extend([Area::new(2), Area::new(1)]);
    net.assign_missing_component_ids();
    net
}

#[test]
fn row_maps_follow_filters_and_dead_winding_stubs() {
    let net = case();
    let before = net.clone();
    let cut = net.subset_buses(&[BusId(10), BusId(20), BusId(999)].into_iter().collect());
    assert_eq!(cut.bus_rows, [None, Some(0), Some(1), Some(2)]);
    assert_eq!(cut.branch_rows, [None, Some(0), Some(1), Some(2)]);
    assert_eq!(cut.switch_rows, [None, Some(0), Some(1)]);
    assert_eq!(cut.hvdc_rows, [None, Some(0), Some(1)]);
    assert_eq!(cut.transformer_3w_rows, [None, Some(0)]);
    for rows in [
        &cut.load_rows,
        &cut.shunt_rows,
        &cut.generator_rows,
        &cut.storage_rows,
        &cut.static_var_compensator_rows,
    ] {
        assert_eq!(rows, &[None, Some(0), None, Some(1)]);
    }
    assert_eq!(cut.area_rows, [None, Some(0)]);
    let stub = &cut.network.buses()[2];
    assert_eq!(stub.id, BusId(30));
    assert_eq!(stub.kind, BusType::Isolated);
    assert_eq!(stub.extras["tie_bus"], json!(true));
    assert_eq!(
        cut.network.transformers_3w()[0].winding_in_service(),
        [true, true, false]
    );
    assert!(!cut.network.branches()[2].in_service);
    assert_eq!(
        serde_json::to_value(&net).unwrap(),
        serde_json::to_value(&before).unwrap(),
        "the cut must not mutate the source"
    );
    cut.network.validate().unwrap();
    macro_rules! check {
        ($map:ident, $table:ident) => {
            for (source, target) in cut.$map.iter().enumerate() {
                if let Some(row) = target {
                    assert_eq!(net.$table()[source].uid, cut.network.$table()[*row].uid);
                }
            }
        };
    }
    check!(bus_rows, buses);
    check!(branch_rows, branches);
    check!(switch_rows, switches);
    check!(transformer_3w_rows, transformers_3w);
    check!(generator_rows, generators);
    check!(load_rows, loads);
    check!(shunt_rows, shunts);
    check!(storage_rows, storage);
    check!(hvdc_rows, hvdc);
    check!(static_var_compensator_rows, static_var_compensators);
}

#[test]
fn empty_and_whole_cuts_have_total_maps() {
    for net in [case(), case().to_normalized().unwrap()] {
        let empty = net.subset_buses(&BTreeSet::new());
        let whole = net.subset_buses(&net.buses().iter().map(|b| b.id).collect());
        macro_rules! check {
            ($map:ident, $table:ident) => {
                assert_eq!(empty.$map, vec![None; net.$table().len()]);
                assert_eq!(
                    whole.$map,
                    (0..net.$table().len()).map(Some).collect::<Vec<_>>()
                );
            };
        }
        check!(bus_rows, buses);
        check!(branch_rows, branches);
        check!(switch_rows, switches);
        check!(transformer_3w_rows, transformers_3w);
        check!(generator_rows, generators);
        check!(load_rows, loads);
        check!(shunt_rows, shunts);
        check!(storage_rows, storage);
        check!(hvdc_rows, hvdc);
        check!(static_var_compensator_rows, static_var_compensators);
        assert_eq!(empty.area_rows, vec![None; net.areas().len()]);
        assert_eq!(
            whole.area_rows,
            if net.is_normalized() {
                vec![]
            } else {
                vec![None, Some(0)]
            }
        );
        empty.network.validate().unwrap();
        whole.network.validate().unwrap();
    }
}

fn topology_case() -> BalancedNetwork {
    let mut net = case();
    // Node 30 belongs to the dropped calculated bus only through its node list.
    let mut nodes: Vec<Value> = [10, 20, 30]
        .map(|bus| {
            json!({
                "component": id("connectivity_node", &format!("N{bus}")),
                "voltage_level": id("voltage_level", "VL"),
                "calculated_bus": if bus == 30 { None } else { Some(bus) }
            })
        })
        .to_vec();
    nodes.push(json!({"component":id("connectivity_node", "unassigned"), "voltage_level":id("voltage_level","VL")}));
    let load = net.loads()[1].uid.as_ref().unwrap();
    let removed_load = net.loads()[2].uid.as_ref().unwrap();
    let branch = net.branches()[1].uid.as_ref().unwrap();
    let details = json!({
        "voltage_levels": [{"component":id("voltage_level","VL"), "nominal_kv":230.0, "topology_kind":"node_breaker", "buses":[10,20,30]}],
        "connectivity_nodes": nodes,
        "calculated_buses": ([10,20,30].map(|bus| json!({"voltage_level":id("voltage_level","VL"), "calculated_bus":bus, "nodes":[id("connectivity_node", &format!("N{bus}"))]}))),
        "component_metadata": [
            {"component": id("load", load)}, {"component":id("load", removed_load)},
            {"component": id("junction","J")}
        ],
        "junctions":[{"component":id("junction","J")}],
        "dc_nodes":[{"component":id("dc_node","DC")}],
        "dc_grounds":[{"component":id("dc_ground","ground"), "equipment_container":id("load",removed_load), "dc_terminal":{"dc_node":id("dc_node","DC")}}],
        "busbar_sections":[{"component":id("busbar_section","BB"), "voltage_level":id("voltage_level","VL"), "node":id("connectivity_node","N30")}],
        "terminals":[
            {"component":id("terminal","T1"), "equipment":id("load",load), "terminal":1,"voltage_level":id("voltage_level","VL"),"node":id("connectivity_node","N10"),"connected":true},
            {"component":id("terminal","T2"), "equipment":id("load",removed_load), "terminal":1,"voltage_level":id("voltage_level","VL"),"node":id("connectivity_node","N30"),"connected":true},
            {"equipment":id("junction","J"), "terminal":1,"voltage_level":id("voltage_level","VL"),"node":id("connectivity_node","N30"),"connected":true}
        ],
        "switches":[{"component":id("switch","S"), "voltage_level":id("voltage_level","VL"), "kind":"breaker", "endpoint1":{"kind":"node", "component":id("connectivity_node","N10")},"endpoint2":{"kind":"node", "component":id("connectivity_node","N30")}, "open":true}],
        "internal_connections":[{"voltage_level":id("voltage_level","VL"), "node1":id("connectivity_node","N10"), "node2":id("connectivity_node","N30")}],
        "tap_changers":[{"transformer":id("branch",branch), "winding":1,"kind":"ratio", "low_tap_position":0,"load_tap_changing_capabilities":true,"regulating":false,"steps":[], "regulation_terminal":{"equipment":id("load",removed_load),"terminal":1}}]
    });
    *net.detailed_connectivity_mut() = Some(Arc::new(serde_json::from_value(details).unwrap()));
    // No transformer can pull bus 30 into this test as a stub.
    net.transformers_3w_mut().clear();
    net.validate().unwrap();
    net
}

#[test]
fn node_breaker_cut_filters_hierarchy_and_equipment_references() {
    let net = topology_case();
    let before = net.detailed_connectivity().clone();
    let cut = net.subset_buses(&[BusId(10), BusId(20)].into_iter().collect());
    cut.network.validate().unwrap();
    let detail = cut.network.detailed_connectivity().as_ref().unwrap();
    assert_eq!(detail.voltage_levels[0].buses, [BusId(10), BusId(20)]);
    assert_eq!(detail.calculated_buses.len(), 2);
    assert_eq!(detail.connectivity_nodes.len(), 3); // includes unassigned source topology
    assert!(
        detail
            .connectivity_nodes
            .iter()
            .all(|n| n.component.local_id() != "N30")
    );
    assert_eq!(detail.busbar_sections.as_slice(), []);
    assert_eq!(detail.switches.as_slice(), []);
    assert_eq!(detail.internal_connections.as_slice(), []);
    assert_eq!(detail.junctions.as_slice(), []);
    assert_eq!(detail.terminals.len(), 1);
    assert_eq!(detail.component_metadata.len(), 1);
    assert!(detail.dc_grounds[0].equipment_container.is_none());
    assert_eq!(detail.tap_changers.len(), 1);
    assert!(detail.tap_changers[0].regulation_terminal.is_none());
    assert_eq!(net.detailed_connectivity(), &before);
    let whole = net.subset_buses(&net.buses().iter().map(|b| b.id).collect());
    assert_eq!(whole.network.detailed_connectivity(), &before);
}

#[test]
fn bus_breaker_cut_and_selector_share_the_connectivity_fix() {
    let mut net = case();
    net.transformers_3w_mut().clear();
    let detail: DetailedConnectivity = serde_json::from_value(json!({
        "voltage_levels":[{"component":id("voltage_level","VL"), "nominal_kv":230.0,"topology_kind":"bus_breaker","buses":[10,20]}],
        "bus_breaker_buses": ([10,20].map(|bus|json!({"component":id("bus", &format!("B{bus}")),"voltage_level":id("voltage_level","VL"),"calculated_bus":bus})))
    })).unwrap();
    *net.detailed_connectivity_mut() = Some(Arc::new(detail));
    net.validate().unwrap();
    let selector = Selector {
        bus: Some((10, 10)),
        ..Selector::default()
    };
    let cut = net.subset(&selector, false);
    cut.validate().unwrap();
    let detail = cut.detailed_connectivity().as_ref().unwrap();
    assert_eq!(detail.voltage_levels[0].buses, [BusId(10)]);
    assert_eq!(detail.bus_breaker_buses.len(), 1);
    let boundary = net.subset(&selector, true);
    boundary.validate().unwrap();
    assert_eq!(
        boundary
            .detailed_connectivity()
            .as_ref()
            .unwrap()
            .bus_breaker_buses
            .len(),
        2
    );
}

mod helpers;

#[test]
fn xiidm_cuts_validate_and_emit_without_restoring_removed_equipment() {
    for fixture in ["eurostag-tutorial1-lf.xml", "fictitiousSwitchRef.xml"] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/data/xiidm/powsybl/V1_17")
            .join(fixture);
        let net = helpers::parse_file(path, Some("xiidm")).unwrap().network;
        let whole = net.subset_buses(&net.buses().iter().map(|b| b.id).collect());
        assert_eq!(
            whole.network.detailed_connectivity(),
            net.detailed_connectivity()
        );
        for bus in net.buses() {
            let cut = net.subset_buses(&[bus.id].into_iter().collect()).network;
            cut.validate().unwrap();
            let emitted = helpers::emit_value(&cut, powerio_tx::TargetFormat::Xiidm).unwrap();
            let restored = helpers::parse_str(&emitted.text, "xiidm").unwrap().network;
            restored.validate().unwrap();
            assert_eq!(
                restored.buses().len(),
                cut.buses().len(),
                "{fixture}: bus {}",
                bus.id
            );
            assert_eq!(restored.loads().len(), cut.loads().len(), "{fixture}");
            assert_eq!(
                restored.generators().len(),
                cut.generators().len(),
                "{fixture}"
            );
            assert_eq!(restored.branches().len(), cut.branches().len(), "{fixture}");
        }
    }
}

#[test]
fn cgmes_node_breaker_cuts_survive_fresh_profile_emission() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data/cgmes/node-breaker");
    let net = helpers::parse_file(path, Some("cgmes")).unwrap().network;
    for bus in net.buses() {
        let cut = net.subset_buses(&[bus.id].into_iter().collect()).network;
        cut.validate().unwrap();
        let directory = tempfile::tempdir().unwrap();
        powerio_tx::emit(
            &powerio_core::PioModule::new(cut.clone()),
            powerio_tx::TargetFormat::Cgmes,
            powerio_core::Destination::path(directory.path().join("profiles")),
        )
        .unwrap();
        let restored = helpers::parse_file(directory.path().join("profiles"), Some("cgmes"))
            .unwrap()
            .network;
        restored.validate().unwrap();
        assert_eq!(restored.buses().len(), cut.buses().len());
        assert_eq!(restored.loads().len(), cut.loads().len());
        assert_eq!(restored.generators().len(), cut.generators().len());
        assert_eq!(restored.branches().len(), cut.branches().len());
    }
}

#[test]
fn island_cut_and_normalization_keep_all_twelve_winding_ratings() {
    use powerio_tx::{BranchRatingSet, IslandReferencePolicy, NormalizeSettings};
    let mut net = case();
    for winding in 0..3 {
        let transformer = &mut net.transformers_3w_mut()[1];
        transformer.windings[winding].rate_a = 100.0;
        transformer.windings[winding].rate_b = 200.0;
        transformer.windings[winding].rate_c = 300.0;
        transformer.set_winding_rating_sets(
            winding,
            (4..=12)
                .map(|number| {
                    BranchRatingSet::new(format!("RATE{number}"), f64::from(number) * 100.0)
                })
                .collect(),
        );
    }
    *net.detailed_connectivity_mut() = Some(Arc::new(serde_json::from_value(json!({
        "voltage_levels":[{"component":id("voltage_level","VL"),"nominal_kv":230.0,"topology_kind":"bus_breaker","buses":[90,10,20,30]}],
        "bus_breaker_buses":([90,10,20,30].map(|bus|json!({"component":id("bus",&format!("BB{bus}")),"voltage_level":id("voltage_level","VL"),"calculated_bus":bus})))
    })).unwrap()));
    net.validate().unwrap();
    let cut = net.subset_buses(&[BusId(10), BusId(20)].into_iter().collect());
    assert_eq!(cut.transformer_3w_rows, [None, Some(0)]);
    let transformer = &cut.network.transformers_3w()[0];
    for winding in 0..3 {
        assert_eq!(
            transformer.winding_rating_sets(winding),
            net.transformers_3w()[1].winding_rating_sets(winding)
        );
    }
    let settings =
        NormalizeSettings::default().with_island_references(IslandReferencePolicy::PerIsland);
    let (normalized, _) = cut.network.to_normalized_with(&settings).unwrap();
    assert_eq!(normalized.network.calc_islands().islands.len(), 1);
    let transformer = &normalized.network.transformers_3w()[0];
    assert_eq!(transformer.winding_in_service(), [true, true, false]);
    for winding in 0..3 {
        let w = &transformer.windings[winding];
        assert!((w.rate_a - 1.0).abs() < 1e-12);
        assert!((w.rate_b - 2.0).abs() < 1e-12);
        assert!((w.rate_c - 3.0).abs() < 1e-12);
        assert_eq!(
            transformer.winding_rating_sets(winding),
            (4..=12)
                .map(|number| { BranchRatingSet::new(format!("RATE{number}"), f64::from(number)) })
                .collect::<Vec<_>>()
        );
    }
    normalized.network.validate().unwrap();
    let detail = normalized.network.detailed_connectivity().as_ref().unwrap();
    assert_eq!(detail.voltage_levels[0].buses, [BusId(10), BusId(20)]);
    assert_eq!(detail.bus_breaker_buses.len(), 2);
}

#[test]
fn per_island_normalization_projects_node_breaker_connectivity() {
    let mut net = topology_case();
    net.branches_mut()
        .retain(|b| b.from == BusId(10) && b.to == BusId(20));
    net.generators_mut().retain(|g| g.bus == BusId(10));
    net.validate().unwrap();
    let before = net.detailed_connectivity().clone();
    let settings =
        NormalizeSettings::default().with_island_references(IslandReferencePolicy::PerIsland);
    let (normalized, _) = net.to_normalized_with(&settings).unwrap();
    normalized.network.validate().unwrap();
    let detail = normalized.network.detailed_connectivity().as_ref().unwrap();
    assert_eq!(detail.voltage_levels[0].buses, [BusId(10), BusId(20)]);
    assert_eq!(detail.calculated_buses.len(), 2);
    assert_eq!(detail.connectivity_nodes.len(), 3); // unassigned source node survives
    assert_eq!(detail.terminals.len(), 1);
    assert_eq!(detail.internal_connections.as_slice(), []);
    assert_eq!(detail.switches.as_slice(), []);
    assert_eq!(detail.busbar_sections.as_slice(), []);
    assert_eq!(detail.junctions.as_slice(), []);
    assert_eq!(detail.component_metadata.len(), 1);
    assert!(detail.tap_changers[0].regulation_terminal.is_none());
    assert_eq!(net.detailed_connectivity(), &before);
}

#[test]
fn per_island_normalization_projects_bus_breaker_connectivity() {
    let mut net = case();
    net.branches_mut().clear();
    net.transformers_3w_mut().clear();
    net.generators_mut().retain(|g| g.bus == BusId(10));
    *net.detailed_connectivity_mut() = Some(Arc::new(serde_json::from_value(json!({
        "voltage_levels":[{"component":id("voltage_level","VL"),"nominal_kv":230.0,"topology_kind":"bus_breaker","buses":[10,20]}],
        "bus_breaker_buses":([10,20].map(|bus|json!({"component":id("bus",&format!("BB{bus}")),"voltage_level":id("voltage_level","VL"),"calculated_bus":bus})))
    })).unwrap()));
    net.validate().unwrap();
    let settings =
        NormalizeSettings::default().with_island_references(IslandReferencePolicy::PerIsland);
    let (normalized, _) = net.to_normalized_with(&settings).unwrap();
    normalized.network.validate().unwrap();
    let detail = normalized.network.detailed_connectivity().as_ref().unwrap();
    assert_eq!(detail.voltage_levels[0].buses, [BusId(10)]);
    assert_eq!(detail.bus_breaker_buses.len(), 1);
}

#[test]
fn normalization_keeps_orphaned_node_angles_in_degrees() {
    let mut net = topology_case();
    net.buses_mut()[2].va = 30.0; // bus 20 has no terminal or busbar anchor
    let detail = Arc::make_mut(net.detailed_connectivity_mut().as_mut().unwrap());
    detail
        .calculated_buses
        .retain(|b| b.calculated_bus != BusId(20));
    // Removing inactive equipment invokes the orphan-preservation path even
    // though every bus remains. The synthesized record must use the result units.
    let normalized = net.to_normalized().unwrap();
    normalized.validate().unwrap();
    let detail = normalized.detailed_connectivity().as_ref().unwrap();
    let bus = detail
        .calculated_buses
        .iter()
        .find(|b| b.calculated_bus == BusId(20))
        .unwrap();
    assert!((bus.angle_degrees.unwrap() - 30.0).abs() < 1e-12);
}
