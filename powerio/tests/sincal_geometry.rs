#![allow(clippy::float_cmp)] // Exact native coordinates and synthetic integer midpoints.
mod helpers;

use powerio::geo::{CoordinateSpace, GeoLayer};
use powerio::{Destination, EmittedOutput, ParseOptions, PioValue, Source};

const GRAPHICS: &str = "
CREATE TABLE GraphicNode(GraphicNode_ID,Variant_ID,Flag_Variant,Node_ID,GraphicArea_ID,NodeStartX,NodeStartY,NodeEndX,NodeEndY);
INSERT INTO GraphicNode VALUES(1,1,1,10,1,0,0,0,0),(2,1,1,20,1,10,4,10,8);
CREATE TABLE GraphicElement(GraphicElement_ID,Variant_ID,Flag_Variant,Element_ID,GraphicArea_ID);
INSERT INTO GraphicElement VALUES(5,1,1,30,1);
CREATE TABLE GraphicTerminal(GraphicTerminal_ID,Variant_ID,Flag_Variant,GraphicElement_ID,Terminal_ID,GraphicArea_ID,PosX,PosY);
INSERT INTO GraphicTerminal VALUES(11,1,1,5,40,1,0,0),(22,1,1,5,50,1,10,4);
CREATE TABLE GraphicBucklePoint(GraphicPoint_ID,Variant_ID,Flag_Variant,GraphicTerminal_ID,NoPoint,PosX,PosY);
INSERT INTO GraphicBucklePoint VALUES(1,1,1,22,1,8,2);
CREATE TABLE GraphicAreaTile(GraphicArea_ID,Variant_ID,Flag,VectorX,VectorY,ScalePaper,ScaleReal);
INSERT INTO GraphicAreaTile VALUES(1,1,2,100,200,100,1);
";

fn dist(edit: &str) -> powerio::PioModule<PioValue> {
    let db = rusqlite::Connection::open_in_memory().unwrap();
    db.execute_batch(include_str!(
        "../../tests/data/sincal/synthetic-multiconductor.sql"
    ))
    .unwrap();
    db.execute_batch(edit).unwrap();
    powerio::parse_with_options(
        Source::from_memory("synthetic.db", db.serialize("main").unwrap().to_vec()).unwrap(),
        &ParseOptions::default()
            .format("sincal-multiconductor")
            .unwrap(),
    )
    .unwrap()
}

fn check_roundtrips(module: &powerio::PioModule<PioValue>, layer: &GeoLayer) {
    let restored =
        helpers::deserialize_module_text(&helpers::serialize_module_text(module).unwrap()).unwrap();
    assert_eq!(module.extensions(), restored.extensions());
    match (module.value(), restored.value()) {
        (PioValue::BalancedNetwork(a), PioValue::BalancedNetwork(b)) => assert_eq!(
            serde_json::to_value(a).unwrap(),
            serde_json::to_value(b).unwrap()
        ),
        (PioValue::MulticonductorNetwork(a), PioValue::MulticonductorNetwork(b)) => assert_eq!(
            serde_json::to_value(a).unwrap(),
            serde_json::to_value(b).unwrap()
        ),
        _ => panic!("wrong family"),
    }
    assert_eq!(
        GeoLayer::parse(&layer.to_geojson(), None).unwrap().layer,
        *layer
    );
    let EmittedOutput::Memory { artifacts } =
        powerio::emit(module, "sincal", Destination::memory("echo").unwrap())
            .unwrap()
            .into_output()
    else {
        panic!("memory")
    };
    assert_eq!(
        artifacts[0].bytes(),
        module.source().unwrap().primary_buffer().unwrap().bytes()
    );
}

#[test]
fn licensed_simbench_geometry_uses_existing_balanced_geo_and_roundtrip_surfaces() {
    let module = powerio::parse_with_options(
        Source::from_memory(
            "case.sinx",
            include_bytes!("../../tests/data/sincal/1-LV-rural1--0-sw.sinx").to_vec(),
        )
        .unwrap(),
        &ParseOptions::default().format("sincal-balanced").unwrap(),
    )
    .unwrap();
    let PioValue::BalancedNetwork(net) = module.value() else {
        panic!("balanced")
    };
    assert_eq!(
        net.buses().iter().filter(|b| b.location.is_some()).count(),
        15
    );
    assert_eq!(net.geo().as_ref().unwrap().space, CoordinateSpace::Unknown);
    assert!(net.branches().iter().any(|b| b.route.is_some()));
    assert_eq!(
        module.extensions()["powerio.sincal.graphic_view"]["native_fields"]["Flag"],
        1
    );
    check_roundtrips(&module, &net.to_geo_layer());
    let projected =
        powerio::emit(&module, "matpower", Destination::memory("case.m").unwrap()).unwrap();
    assert!(
        projected
            .diagnostics()
            .iter()
            .any(|d| d.message().contains("bus location(s)")
                && d.message().contains("branch route(s) dropped"))
    );

    let report = module
        .diagnostics()
        .iter()
        .find(|d| d.code() == "READ.SINCAL.GEOMETRY")
        .unwrap();
    assert_eq!(report.details()["mapped_points"], 15);
    let retained = module
        .diagnostics()
        .iter()
        .find(|d| d.details().get("table").is_some_and(|v| v == "GraphicNode"))
        .unwrap();
    assert_eq!(retained.details()["whole_table"], false);
}

#[test]
fn distribution_geometry_is_optional_and_does_not_change_electrical_values() {
    let original = dist("");
    let module = dist(GRAPHICS);
    let PioValue::MulticonductorNetwork(net) = module.value() else {
        panic!("dist")
    };
    let PioValue::MulticonductorNetwork(baseline) = original.value() else {
        unreachable!()
    };
    let bus = net.buses().iter().find(|b| b.id == "10").unwrap();
    assert_eq!(bus.location.as_ref().unwrap().x, 0.0);
    let bus = net.buses().iter().find(|b| b.id == "20").unwrap();
    assert_eq!(bus.location.as_ref().unwrap().y, 6.0);
    assert_eq!(
        bus.location.as_ref().unwrap().kind,
        Some(powerio_dist::geo::DistCoordsKind::Derived)
    );
    assert!(
        net.buses()
            .iter()
            .filter(|b| b.id != "10" && b.id != "20")
            .all(|b| b.location.is_none())
    );
    assert_eq!(
        net.lines()[0]
            .route
            .as_ref()
            .unwrap()
            .iter()
            .map(|p| [p.x, p.y])
            .collect::<Vec<_>>(),
        [[0., 0.], [8., 2.], [10., 4.]]
    );
    assert_eq!(
        net.geo().as_ref().unwrap().space,
        powerio_dist::CoordinateSpace::Diagram { canvas: None }
    );
    check_roundtrips(&module, &powerio::dist_geo::to_dist_geo_layer(net));
    let mut electrical = net.clone();
    for b in electrical.buses_mut() {
        b.location = None;
        b.extras.remove("sincal_geometry");
    }
    for l in electrical.lines_mut() {
        l.route = None;
    }
    *electrical.geo_mut() = None;
    assert_eq!(
        serde_json::to_value(electrical).unwrap(),
        serde_json::to_value(baseline).unwrap()
    );
    powerio::to_mc_ac_pf_instance(&module).unwrap();
    let matrix =
        powerio_matrix::matrix::multiconductor::calc_multiconductor_admittance_matrix(net).unwrap();
    assert_eq!(matrix.diagnostics(), []);
}

#[test]
fn malformed_optional_geometry_leaves_electrical_reader_usable() {
    let module = dist(&format!(
        "{GRAPHICS} UPDATE GraphicNode SET NodeStartX=NULL; UPDATE GraphicTerminal SET PosX=NULL;"
    ));
    let PioValue::MulticonductorNetwork(net) = module.value() else {
        panic!("dist")
    };
    assert!(net.buses().iter().all(|b| b.location.is_none()));
    assert!(net.lines().iter().all(|l| l.route.is_none()));
    assert!(
        module
            .diagnostics()
            .iter()
            .any(|d| d.code() == "READ.DIST.SINCAL_GEOMETRY")
    );
    powerio::to_mc_ac_pf_instance(&module).unwrap();
}
