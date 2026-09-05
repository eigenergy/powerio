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
fn parsed_geometry_is_blocked_before_analysis() {
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
    assert!(!report.is_ready());
    assert!(
        report
            .blockers()
            .any(|finding| finding.code == "READINESS.DSS.GEOMETRY_DEFERRED")
    );
}

#[test]
fn parsed_swer_geometry_is_also_blocked() {
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
    assert!(!report.is_ready());
    assert!(
        report
            .blockers()
            .any(|finding| finding.code == "READINESS.DSS.GEOMETRY_DEFERRED")
    );
}
