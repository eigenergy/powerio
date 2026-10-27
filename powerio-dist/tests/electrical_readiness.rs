use std::fs;

use powerio_core::Source;
use powerio_dist::{audit_electrical_readiness, parse};

fn parse_text(
    name: &str,
    text: &str,
) -> powerio_core::PioModule<powerio_dist::MulticonductorNetwork> {
    let dir = std::env::temp_dir().join(format!(
        "powerio-dss-readiness-{name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("master.dss");
    fs::write(&path, text).unwrap();
    parse(Source::open(&path).unwrap()).unwrap()
}

#[test]
fn parsed_overhead_geometry_is_ready_for_analysis() {
    let module = parse_text(
        "geometry",
        r"clear
new circuit.t basekv=4.16 phases=3 bus1=sourcebus
new wiredata.w rac=0.1859 gmr=0.0313 radius=0.4635 runits=mi gmrunits=ft radunits=in
new linegeometry.g nconds=4 nphases=3 reduce=no
~ cond=1 wire=w x=2.5 h=29 units=ft
~ cond=2 wire=w x=0 h=29 units=ft
~ cond=3 wire=w x=7 h=29 units=ft
~ cond=4 wire=w x=4 h=25 units=ft
new line.l bus1=sourcebus bus2=b geometry=g length=1 units=m
",
    );

    let report = audit_electrical_readiness(module.value());
    assert!(report.is_ready(), "{:?}", report.findings);
    assert!(report.blockers().next().is_none());
}

#[test]
fn parsed_single_conductor_geometry_is_ready() {
    let module = parse_text(
        "swer",
        r"clear
new circuit.swer basekv=19.1 phases=1 bus1=sourcebus
new wiredata.w rac=1.093 gmr=0.00296 radius=0.00318 runits=km gmrunits=m radunits=m
new linegeometry.g nconds=1 nphases=1 reduce=no
~ cond=1 wire=w x=0 h=8.5 units=m
new line.l bus1=sourcebus.1 bus2=b.1 geometry=g length=1 units=m
",
    );

    let report = audit_electrical_readiness(module.value());
    assert!(report.is_ready(), "{:?}", report.findings);
    assert!(report.blockers().next().is_none());
}

// Resolved geometry retains definitions, without substituting factory matrices.
// The Line factory self resistance is (R0 + 2 R1) / 3 in the phase domain.

const OPENDSS_FACTORY_R11_PER_M: f64 = 0.098_133_333_333_333_34;

#[test]
fn geometry_line_materializes_without_factory_defaults() {
    let module = parse_text(
        "geometry-defaults",
        r"clear
new circuit.t basekv=4.16 phases=3 bus1=sourcebus
new wiredata.w rac=0.1859 gmr=0.0313 radius=0.4635 runits=mi gmrunits=ft radunits=in
new linegeometry.g nconds=4 nphases=3 reduce=no
~ cond=1 wire=w x=2.5 h=29 units=ft
~ cond=2 wire=w x=0 h=29 units=ft
~ cond=3 wire=w x=7 h=29 units=ft
~ cond=4 wire=w x=4 h=25 units=ft
new line.l bus1=sourcebus bus2=b geometry=g length=1 units=m
",
    );

    assert!(
        !module
            .diagnostics()
            .iter()
            .any(|finding| finding.code() == "READ.DSS.GEOMETRY_UNRESOLVED"),
        "the geometry was electrically resolved"
    );
    let net = module.value();
    assert!(
        net.lines()
            .iter()
            .any(|line| line.name == "l" && line.terminal_map_from.len() == 4),
        "all four declared conductors become a typed line"
    );
    assert!(
        net.line_codes().iter().all(|code| {
            code.r_series
                .first()
                .and_then(|row| row.first())
                .is_none_or(|r| (*r - OPENDSS_FACTORY_R11_PER_M).abs() > 1e-12)
        }),
        "no linecode carries the OpenDSS factory R1"
    );
    assert!(
        net.untyped_objects()
            .iter()
            .any(|object| object.class.eq_ignore_ascii_case("linegeometry") && object.name == "g"),
        "the source geometry definition is retained"
    );
}

#[test]
fn single_conductor_geometry_never_becomes_three_conductors() {
    let module = parse_text(
        "swer-defaults",
        r"clear
new circuit.swer basekv=19.1 phases=1 bus1=sourcebus
new wiredata.w rac=1.093 gmr=0.00296 radius=0.00318 runits=km gmrunits=m radunits=m
new linegeometry.g nconds=1 nphases=1 reduce=no
~ cond=1 wire=w x=0 h=8.5 units=m
new line.l bus1=sourcebus.1 bus2=b.1 geometry=g length=1 units=m
",
    );

    assert!(
        !module
            .diagnostics()
            .iter()
            .any(|finding| finding.code() == "READ.DSS.GEOMETRY_UNRESOLVED")
    );
    let net = module.value();
    assert!(
        net.lines()
            .iter()
            .all(|line| line.terminal_map_from.len() == 1 && line.terminal_map_to.len() == 1),
        "no typed line gained a fabricated conductor count"
    );
    assert!(
        net.line_codes().iter().all(|code| code.n_conductors != 3),
        "no three conductor linecode was synthesized for a one conductor geometry"
    );
}

#[test]
fn missing_geometry_still_blocks_without_a_factory_line() {
    let module = parse_text(
        "missing-geometry",
        "new circuit.c\nnew line.l bus1=sourcebus bus2=end geometry=missing\n",
    );
    assert!(
        audit_electrical_readiness(module.value())
            .blockers()
            .any(|f| f.code == "READINESS.DSS.GEOMETRY_DEFERRED")
    );
    assert!(!module.value().lines().iter().any(|line| line.name == "l"));
}
