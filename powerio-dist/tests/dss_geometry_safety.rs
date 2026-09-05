//! Regression harness for OpenDSS geometry-defined lines.
//!
//! Geometry-family classes (`LineGeometry`, `LineSpacing`, `WireData`,
//! `CNData`, `TSData`) are currently deferred by the typed distribution model.
//! Until typed geometry lowering exists, a geometry-backed line must not be
//! normalized with OpenDSS `Line` factory impedance or a fabricated conductor
//! count. These tests are intentionally ignored while the normalization
//! contract is being changed; they document the expected safety invariant.

use std::fs;

use powerio_core::Source;
use powerio_dist::parse;

fn parse_text(name: &str, text: &str) -> powerio_core::PioModule<powerio_dist::MulticonductorNetwork> {
    let dir = std::env::temp_dir().join(format!("powerio-dss-geometry-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("master.dss");
    fs::write(&path, text).unwrap();
    parse(Source::open(&path).unwrap()).unwrap()
}

#[test]
#[ignore = "geometry lowering contract is being hardened; see eigenergy/powerio#479"]
fn geometry_line_is_not_replaced_by_factory_defaults() {
    let module = parse_text(
        "geometry-defaults",
        r#"clear
new circuit.t basekv=4.16 phases=3 bus1=sourcebus
new wiredata.w rac=0.1859 gmr=0.0313 radius=0.4635 runits=mi gmrunits=ft radunits=in
new linegeometry.g nconds=4 nphases=3 reduce=no
~ cond=1 wire=w x=2.5 h=29 units=ft
~ cond=2 wire=w x=0 h=29 units=ft
~ cond=3 wire=w x=7 h=29 units=ft
~ cond=4 wire=w x=4 h=25 units=ft
new line.l bus1=sourcebus bus2=b geometry=g length=1 units=m
"#,
    );

    // Safety invariant: unsupported geometry must produce a diagnostic and
    // must not create a typed linecode containing the OpenDSS factory R/X.
    let rendered = powerio_dist::diagnostics::render_diagnostics(module.diagnostics());
    assert!(
        rendered.iter().any(|d| d.to_ascii_lowercase().contains("geometry")),
        "expected a parse-time geometry diagnostic, got {rendered:?}"
    );
    let net = module.value();
    assert!(
        net.linecodes().iter().all(|c| {
            c.r_series
                .first()
                .and_then(|row| row.first())
                .is_none_or(|r| (*r - 0.09813333333333334).abs() > 1e-12)
        }),
        "geometry line must not inherit OpenDSS factory R1"
    );
}

#[test]
#[ignore = "geometry lowering contract is being hardened; see eigenergy/powerio#479"]
fn single_conductor_geometry_does_not_become_three_conductors() {
    let module = parse_text(
        "swer",
        r#"clear
new circuit.swer basekv=19.1 phases=1 bus1=sourcebus
new wiredata.w rac=1.093 gmr=0.00296 radius=0.00318 runits=km gmrunits=m radunits=m
new linegeometry.g nconds=1 nphases=1 reduce=no
~ cond=1 wire=w x=0 h=8.5 units=m
new line.l bus1=sourcebus.1 bus2=b.1 geometry=g length=1 units=m
"#,
    );

    let rendered = powerio_dist::diagnostics::render_diagnostics(module.diagnostics());
    assert!(rendered.iter().any(|d| d.to_ascii_lowercase().contains("geometry")));
    let net = module.value();
    assert!(net.lines().is_empty() || net.lines().iter().all(|l| {
        l.terminal_map_from.len() == 1 && l.terminal_map_to.len() == 1
    }));
}
