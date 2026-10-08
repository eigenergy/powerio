#![allow(clippy::float_cmp)] // Exact native coordinates and synthetic integer midpoints.
use crate::DatabaseSnapshot;
use rusqlite::Connection;

fn database(edit: &str) -> DatabaseSnapshot {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch("CREATE TABLE Version(Version_ID,Version_No,Calc_Type); INSERT INTO Version VALUES(1,14.8,1);
        CREATE TABLE Variant(Variant_ID,ParentVariant_ID,Flag_Variant); INSERT INTO Variant VALUES(1,0,1),(2,0,1);
        CREATE TABLE Node(Node_ID,Variant_ID); INSERT INTO Node VALUES(100,1),(200,1);
        CREATE TABLE Element(Element_ID,Variant_ID,Type); INSERT INTO Element VALUES(50,1,'Line');
        CREATE TABLE Terminal(Terminal_ID,Variant_ID,Element_ID,Node_ID,TerminalNo); INSERT INTO Terminal VALUES(10,1,50,100,1),(20,1,50,200,2);
        CREATE TABLE GraphicNode(GraphicNode_ID,Variant_ID,Flag_Variant,Node_ID,GraphicArea_ID,NodeStartX,NodeStartY,NodeEndX,NodeEndY);
        INSERT INTO GraphicNode VALUES(1,1,1,100,1,0,0,0,0),(2,1,1,200,1,10,4,10,8);
        CREATE TABLE GraphicElement(GraphicElement_ID,Variant_ID,Flag_Variant,Element_ID,GraphicArea_ID); INSERT INTO GraphicElement VALUES(5,1,1,50,1);
        CREATE TABLE GraphicTerminal(GraphicTerminal_ID,Variant_ID,Flag_Variant,GraphicElement_ID,Terminal_ID,GraphicArea_ID,PosX,PosY);
        INSERT INTO GraphicTerminal VALUES(11,1,1,5,10,1,0,0),(22,1,1,5,20,1,10,4);
        CREATE TABLE GraphicBucklePoint(GraphicPoint_ID,Variant_ID,Flag_Variant,GraphicTerminal_ID,NoPoint,PosX,PosY);
        INSERT INTO GraphicBucklePoint VALUES(1,1,1,11,1,0,2),(2,1,1,22,1,10,2);").unwrap();
    c.execute_batch(edit).unwrap();
    DatabaseSnapshot::decode(&c.serialize("main").unwrap(), Some(1)).unwrap()
}
#[test]
fn drawing_points_keep_zero_and_derive_extended_busbar_midpoints() {
    let g = database("").drawing_geometry().unwrap();
    assert_eq!(g.points[&100].point(), [0.0, 0.0]);
    assert!(!g.points[&100].derived());
    let tiny = f64::from_bits(1);
    let p = crate::DrawingPoint {
        start: [tiny, -0.0],
        end: [tiny, -0.0],
    };
    assert_eq!(p.point()[0].to_bits(), tiny.to_bits());
    assert_eq!(g.points[&200].point(), [10.0, 6.0]);
    assert!(g.points[&200].derived());
    assert_eq!(
        g.routes[&50],
        [[0.0, 0.0], [0.0, 2.0], [10.0, 2.0], [10.0, 4.0]]
    );
}
#[test]
fn variant_and_inactive_graphic_records_do_not_contaminate_selected_geometry() {
    let g = database(
        "INSERT INTO GraphicNode VALUES(3,2,1,100,2,999,999,999,999),(4,1,0,100,3,999,999,999,999)",
    )
    .drawing_geometry()
    .unwrap();
    assert_eq!(g.points.len(), 2);
    assert_eq!(g.area, Some(1));
}
#[test]
fn ambiguous_areas_and_conflicting_duplicates_are_not_first_row_wins() {
    let g = database("INSERT INTO GraphicNode VALUES(3,1,1,100,2,0,0,0,0)")
        .drawing_geometry()
        .unwrap();
    assert_eq!(g.points.len(), 0);
    assert_eq!(g.routes.len(), 0);
    let g = database("INSERT INTO GraphicNode VALUES(3,1,1,100,1,1,1,1,1)")
        .drawing_geometry()
        .unwrap();
    assert!(!g.points.contains_key(&100));
    assert!(g.findings.contains_key("GraphicNode.conflicting_positions"));
}
#[test]
fn invalid_optional_geometry_is_diagnosed_without_breaking_snapshot() {
    for edit in [
        "UPDATE GraphicNode SET NodeStartX=NULL WHERE Node_ID=100",
        "UPDATE GraphicNode SET NodeStartX=1e999 WHERE Node_ID=100",
        "UPDATE GraphicNode SET Node_ID=999 WHERE Node_ID=100",
    ] {
        let g = database(edit).drawing_geometry().unwrap();
        assert!(!g.points.contains_key(&100));
        assert!(!g.findings.is_empty());
    }
    let g = database("DROP TABLE GraphicNode")
        .drawing_geometry()
        .unwrap();
    assert_eq!(g.points.len(), 0);
}
#[test]
fn unverified_multiple_bends_invalid_order_and_cross_element_ports_refuse_route() {
    for edit in [
        "INSERT INTO GraphicBucklePoint VALUES(3,1,1,11,2,2,2)",
        "INSERT INTO GraphicBucklePoint VALUES(3,1,1,11,1,2,2)",
        "UPDATE GraphicBucklePoint SET PosX=NULL",
        "UPDATE GraphicTerminal SET Terminal_ID=999 WHERE Terminal_ID=10",
        "UPDATE GraphicTerminal SET GraphicArea_ID=2 WHERE Terminal_ID=10",
        "UPDATE GraphicTerminal SET Terminal_ID=20 WHERE Terminal_ID=10",
    ] {
        let g = database(edit).drawing_geometry().unwrap();
        assert_eq!(g.routes.len(), 0, "{edit}");
        assert!(!g.findings.is_empty());
    }
}
#[test]
fn missing_route_schema_keeps_valid_bus_points() {
    let g = database("DROP TABLE GraphicBucklePoint")
        .drawing_geometry()
        .unwrap();
    assert_eq!(g.points.len(), 2);
    assert_eq!(g.routes.len(), 0);
}

#[test]
fn duplicate_graphic_ids_invalidate_both_lines_instead_of_reassigning_geometry() {
    let second = "INSERT INTO Element VALUES(60,1,'Line');
        INSERT INTO Terminal VALUES(30,1,60,100,1),(40,1,60,200,2);
        INSERT INTO GraphicElement VALUES(6,1,1,60,1);
        INSERT INTO GraphicTerminal VALUES(33,1,1,6,30,1,0,0),(44,1,1,6,40,1,10,4);";
    let valid = database(second).drawing_geometry().unwrap();
    assert_eq!(valid.routes.len(), 2, "parallel lines join by native ID");
    assert_eq!(valid.routes[&60].len(), 2);
    assert_eq!(valid.routes[&50].len(), 4);
    for edit in [
        "UPDATE GraphicElement SET GraphicElement_ID=5 WHERE Element_ID=60",
        "UPDATE GraphicTerminal SET GraphicTerminal_ID=11 WHERE Terminal_ID=30",
        "INSERT INTO GraphicTerminal VALUES(11,1,1,5,10,1,NULL,0)",
    ] {
        let g = database(&format!("{second}{edit}"))
            .drawing_geometry()
            .unwrap();
        assert!(!g.routes.contains_key(&50), "{edit}");
        assert!(!g.findings.is_empty());
    }
}

#[test]
fn invalid_duplicate_node_record_cannot_leave_a_plausible_partial_location() {
    for edit in [
        "INSERT INTO GraphicNode VALUES(3,1,1,100,1,NULL,0,0,0)",
        "UPDATE GraphicNode SET GraphicNode_ID=1 WHERE Node_ID=200",
    ] {
        let g = database(edit).drawing_geometry().unwrap();
        assert!(!g.points.contains_key(&100));
        assert!(!g.findings.is_empty());
    }
}
