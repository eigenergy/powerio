//! Multiconductor MG-RAVENS import and export, pinned on the upstream
//! `case3_balanced` example, plus the payoff: a RAVENS feeder converts to
//! OpenDSS, PMD, BMOPF, and distribution CIM, and every distribution source
//! converts to RAVENS.

mod helpers;

use powerio_core::cim::ravens::{Profile, detect_profile};
use std::path::PathBuf;

use helpers::{Parsed, parse_file, parse_str};
use powerio_dist::{Configuration, DistSourceFormat, DistTargetFormat, Error};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/data/dist/ravens/case3_balanced.json")
}

fn parse_ravens_file(path: impl AsRef<std::path::Path>) -> Result<Parsed, Error> {
    parse_file(path, Some("ravens-json"))
}

fn parse_ravens_str(text: &str) -> Result<Parsed, Error> {
    parse_str(text, "ravens-json")
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
}

#[test]
#[allow(clippy::float_cmp)] // exact hand-checked fixture literals are the assertion
fn case3_balanced_maps_exactly() {
    let net = parse_ravens_file(fixture()).unwrap();
    assert_eq!(*net.source_format(), Some(DistSourceFormat::RavensJson));
    // MG-RAVENS carries no frequency; the reader assumes the OpenDSS default
    // and says so.
    assert_eq!(net.base_frequency(), 60.0);
    assert!(
        net.warnings
            .iter()
            .any(|w| w.contains("READ.RAVENS_DIST.VALUE_DEFAULTED") && w.contains("frequency")),
        "warnings: {:?}",
        net.warnings
    );

    // Three external nodes plus the ideal EMF behind the source impedance;
    // grounded neutral terminal, the transit node does not.
    assert_eq!(net.buses().len(), 4);
    let loadbus = net.bus("loadbus").expect("loadbus present");
    assert!(loadbus.terminals.contains(&"4".to_string()));
    assert_eq!(loadbus.grounded, vec!["4".to_string()]);
    assert_eq!(net.bus("primary").unwrap().grounded, Vec::<String>::new());

    // Two per-length impedance matrices become two linecodes, ohm/m, the
    // shunt susceptance split in half at each end.
    assert_eq!(net.line_codes().len(), 3);
    let lc = net.linecode("556mcm").expect("556mcm present");
    assert_eq!(lc.n_conductors, 3);
    assert!(close(lc.r_series[0][0], 0.1));
    assert!(close(lc.r_series[1][0], 0.04), "mutual r");
    assert!(close(lc.x_series[0][0], 0.0583));
    assert!(close(lc.b_from[0][0], 1.6e-5 / 2.0));
    assert!(close(lc.b_to[0][0], 1.6e-5 / 2.0));
    // The emergency current limit becomes the linecode ampacity.
    assert_eq!(lc.i_max, Some(vec![600.0, 600.0, 600.0]));

    // Two three-phase lines; impedance stays in ohm/m, length in meters.
    assert_eq!(net.lines().len(), 3);
    let ohline = net.lines().iter().find(|l| l.name == "ohline").unwrap();
    assert_eq!(ohline.bus_from, "sourcebus");
    assert_eq!(ohline.bus_to, "primary");
    assert_eq!(ohline.terminal_map_from, ["1", "2", "3"]);
    assert!(close(ohline.length, 1.0));
    assert_eq!(ohline.linecode, "556mcm");
    // The continuous limit rides in extras as the dss `normamps`.
    assert!(close(
        ohline
            .extras
            .get("normamps")
            .and_then(serde_json::Value::as_f64)
            .unwrap(),
        400.0
    ));

    // Three single-phase wye loads, one per phase, powers straight off the
    // phase objects, line-to-neutral nominal voltage off the 400 V base.
    assert_eq!(net.loads().len(), 3);
    let l1 = net.loads().iter().find(|l| l.name == "l1").unwrap();
    assert_eq!(l1.bus, "loadbus");
    assert_eq!(l1.configuration, Configuration::SinglePhase);
    assert_eq!(l1.terminal_map, ["1", "4"]);
    assert_eq!(l1.p_nom, vec![6000.0]);
    assert_eq!(l1.q_nom, vec![3000.0]);
    assert!(close(l1.voltage_model.v_nom()[0], 400.0 / 3f64.sqrt()));

    // The Thevenin source: earth referenced on its hot terminals, line to
    // neutral magnitude at 120 degree spacing, the base kV and per unit
    // recovered into extras.
    assert_eq!(net.sources().len(), 1);
    let src = &net.sources()[0];
    assert_ne!(src.bus, "sourcebus");
    let source_branch = net
        .lines()
        .iter()
        .find(|line| line.bus_from == src.bus)
        .unwrap();
    assert_eq!(source_branch.bus_to, "sourcebus");
    let source_z = net.linecode(&source_branch.linecode).unwrap();
    assert!(close(
        source_z.r_series[0][0],
        (2.0 * 3.880_570_000_581_328e-8 + 5.069_596_039_676_399e-8) / 3.0
    ));
    assert_eq!(src.terminal_map, ["1", "2", "3"]);
    assert!(src.reference_terminal.is_none());
    assert!(close(src.v_magnitude[0], 398.36 / 3f64.sqrt()));
    assert!(close(src.v_angle[1], -2.0 * std::f64::consts::PI / 3.0));
    assert!(close(
        src.extras
            .get("basekv")
            .and_then(serde_json::Value::as_f64)
            .unwrap(),
        0.4
    ));
    assert!(close(
        src.extras
            .get("pu")
            .and_then(serde_json::Value::as_f64)
            .unwrap(),
        0.9959
    ));

    // The load-shape schedule has no typed model and is preserved untyped so
    // it round-trips to MG-RAVENS; the finding names the class.
    assert!(
        net.untyped_objects()
            .iter()
            .any(|u| u.class == "EnergyConsumerSchedule" && u.name == "ls1")
    );
    assert!(
        net.warnings
            .iter()
            .any(|w| w.contains("READ.RAVENS_DIST.CLASS_UNMAPPED")
                && w.contains("EnergyConsumerSchedule")),
        "warnings: {:?}",
        net.warnings
    );
}

#[test]
fn balanced_transmission_profile_is_rejected() {
    // An AlgorithmSettings/ProducerCostFunction document is the balanced
    // profile the transmission hub reads; the multiconductor reader refuses
    // it rather than inventing per-phase detail.
    let balanced = r#"{
      "ConnectivityNode": {"b": {"Ravens.cimObjectType": "ConnectivityNode",
        "IdentifiedObject.name": "b"}},
      "ApplicationSettings": {"s": {"Ravens.cimObjectType": "AlgorithmSettings",
        "IdentifiedObject.name": "s"}}
    }"#;
    let err = parse_ravens_str(balanced).unwrap_err();
    assert!(
        err.to_string().contains("balanced"),
        "expected a balanced-profile rejection, got: {err}"
    );
    assert_eq!(detect_profile(balanced), Profile::Balanced);
    assert_eq!(
        detect_profile(&std::fs::read_to_string(fixture()).unwrap()),
        Profile::Multiconductor
    );
}

#[test]
fn ravens_feeder_converts_to_every_distribution_format() {
    // The whole point of landing in MulticonductorNetwork: a RAVENS feeder
    // writes as OpenDSS, PMD, BMOPF, and distribution CIM with no extra
    // bridge.
    let net = parse_ravens_file(fixture()).unwrap();
    for target in [
        DistTargetFormat::Dss,
        DistTargetFormat::PmdJson,
        DistTargetFormat::BmopfJson,
        DistTargetFormat::CimDist,
    ] {
        let conv = net.emit_value(target);
        assert!(!conv.text.is_empty(), "{}: empty output", target.name());
    }

    // OpenDSS output names the feeder's elements.
    let dss = net.emit_value(DistTargetFormat::Dss).text.to_lowercase();
    assert!(dss.contains("line.ohline"), "dss: {dss}");
    assert!(dss.contains("load.l1"));
    assert!(dss.contains("linecode.556mcm"));

    // PMD and BMOPF reparse: proof the conversion is structurally valid.
    let pmd = net.emit_value(DistTargetFormat::PmdJson).text;
    let reparsed = parse_str(&pmd, "pmd-json").unwrap();
    assert_eq!(reparsed.lines().len(), net.lines().len());
    assert_eq!(reparsed.loads().len(), net.loads().len());

    let bmopf = net.emit_value(DistTargetFormat::BmopfJson).text;
    let reparsed = parse_str(&bmopf, "bmopf-json").unwrap();
    assert_eq!(reparsed.lines().len(), net.lines().len());
}

#[test]
fn json_extension_and_ravens_token_both_route() {
    // Auto-detected by the top-level RAVENS markers.
    let net = parse_file(fixture(), None).unwrap();
    assert_eq!(net.loads().len(), 3);
    assert_eq!(*net.source_format(), Some(DistSourceFormat::RavensJson));
    // Explicit token via in-memory text.
    let text = std::fs::read_to_string(fixture()).unwrap();
    let net = parse_str(&text, "ravens").unwrap();
    assert_eq!(net.loads().len(), 3);
    // `ravens` is a full writable distribution target under every spelling.
    for alias in [
        "ravens",
        "ravens-json",
        "mgravens",
        "mg-ravens",
        "MG_RAVENS_JSON",
    ] {
        assert_eq!(
            powerio_dist::parse_dist_target_format(alias),
            Some(DistTargetFormat::RavensJson),
            "{alias}"
        );
    }
    assert_eq!(DistTargetFormat::RavensJson.name(), "ravens-json");
}

/// The writer is the reader's inverse: parse, write, parse preserves the
/// projection, imported mRIDs pass through, and a second write is byte
/// identical.
#[test]
fn ravens_writer_round_trips() {
    let net = parse_ravens_file(fixture()).unwrap();
    let first = net.emit_value(DistTargetFormat::RavensJson);
    let back = parse_ravens_str(&first.text).unwrap();

    assert_eq!(back.buses().len(), net.buses().len());
    assert_eq!(back.line_codes().len(), net.line_codes().len());
    assert_eq!(back.lines().len(), net.lines().len());
    assert_eq!(back.loads().len(), net.loads().len());
    assert_eq!(back.sources().len(), net.sources().len());

    let lc_a = net.linecode("556mcm").unwrap();
    let lc_b = back.linecode("556mcm").unwrap();
    for i in 0..lc_a.n_conductors {
        for j in 0..lc_a.n_conductors {
            assert!(close(lc_a.r_series[i][j], lc_b.r_series[i][j]), "r {i}{j}");
            assert!(close(lc_a.x_series[i][j], lc_b.x_series[i][j]), "x {i}{j}");
            assert!(
                close(
                    lc_a.b_from[i][j] + lc_a.b_to[i][j],
                    lc_b.b_from[i][j] + lc_b.b_to[i][j]
                ),
                "b {i}{j}"
            );
        }
    }
    let la = net.loads().iter().find(|l| l.name == "l1").unwrap();
    let lb = back.loads().iter().find(|l| l.name == "l1").unwrap();
    assert_eq!(la.p_nom, lb.p_nom);
    assert_eq!(la.configuration, lb.configuration);
    assert!(close(
        net.sources()[0].v_magnitude[0],
        back.sources()[0].v_magnitude[0]
    ));
    assert!(close(
        net.sources()[0].v_angle[1],
        back.sources()[0].v_angle[1]
    ));

    // Imported mRIDs pass through, so the second write is byte identical.
    let second = back.emit_value(DistTargetFormat::RavensJson);
    assert_eq!(first.text, second.text);

    // Same-format echo tier: a parse retains its source on the module, so
    // the original module echoes the original fixture bytes, and the reparse
    // of the canonical output echoes that canonical output.
    let original = std::fs::read_to_string(fixture()).unwrap();
    assert_eq!(net.emit(DistTargetFormat::RavensJson).text, original);
    assert_eq!(back.emit(DistTargetFormat::RavensJson).text, first.text);
}

/// Machines and inverter-based resources carry the CIM injection-negative
/// sign in the document (the upstream `opendss2xml` writes `-gen.kW`), so a
/// positive generation setpoint emits as negative and reads back positive.
#[test]
fn generation_uses_injection_negative_sign() {
    let dss = "clear\nNew Circuit.c basekv=12.47 bus1=src\n\
               New Generator.g1 bus1=b2 phases=3 kv=12.47 kw=500 kvar=100 conn=wye\n\
               New Line.l1 bus1=src bus2=b2 length=1 r1=0.1 x1=0.1\n";
    let net = parse_str(dss, "dss").unwrap();
    let p_gen: f64 = net.generators()[0].p_nom.iter().sum();
    assert!(p_gen > 0.0, "dss generation is positive: {p_gen}");

    let text = net.emit_value(DistTargetFormat::RavensJson).text;
    let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
    let mut ps = Vec::new();
    collect_key(&doc, "RotatingMachine.p", &mut ps);
    assert_eq!(ps.len(), 1);
    assert!(
        ps[0] < 0.0,
        "RotatingMachine.p is injection-negative: {}",
        ps[0]
    );

    let back = parse_ravens_str(&text).unwrap();
    let p_back: f64 = back.generators()[0].p_nom.iter().sum();
    assert!(
        close(p_back, p_gen),
        "generation round-trips positive: {p_back} vs {p_gen}"
    );

    // A PV inverter takes the same convention and round-trips.
    let pv = parse_file(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/data/dist/micro/ibr_pv_control.dss"),
        None,
    )
    .unwrap();
    if let Some(avail) = pv.ibrs().first().and_then(|i| i.p_avail) {
        let text = pv.emit_value(DistTargetFormat::RavensJson).text;
        let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
        let mut pe = Vec::new();
        collect_key(&doc, "PowerElectronicsConnection.p", &mut pe);
        assert!(
            pe.iter().all(|&p| p <= 0.0),
            "PV p is injection-negative: {pe:?}"
        );
        let back = parse_ravens_str(&text).unwrap();
        assert!(
            close(back.ibrs()[0].p_avail.unwrap(), avail),
            "PV p_avail round-trips positive"
        );
    }
}

/// A multi-phase wye capacitor's `ShuntCompensator.nomU` is the line-to-line
/// voltage (`cap.kV * 1000`, the upstream opendss2xml convention), not the
/// per-phase value, and the per-section susceptance carries the total reactive
/// power without a spurious per-phase factor.
#[test]
fn shunt_nom_voltage_is_line_to_line() {
    let dss = "clear\nNew Circuit.c basekv=4.16 bus1=src\n\
               New Line.l1 bus1=src bus2=b2 length=1 r1=0.1 x1=0.1\n\
               New Capacitor.c1 bus1=b2 phases=3 kv=4.16 kvar=300 conn=wye\n";
    let net = parse_str(dss, "dss").unwrap();
    let text = net.emit_value(DistTargetFormat::RavensJson).text;
    let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
    let mut nom = Vec::new();
    collect_key(&doc, "ShuntCompensator.nomU", &mut nom);
    assert_eq!(nom.len(), 1);
    assert!(
        close(nom[0], 4160.0),
        "nomU is line-to-line 4160, not per-phase 2402: {}",
        nom[0]
    );
    // The total reactive power recovers on reparse (no phase count inflation).
    let back = parse_ravens_str(&text).unwrap();
    let b: f64 = back.shunts()[0].b[0][0];
    let q_total = b * 4160.0 * 4160.0; // vars
    assert!(
        close(q_total, 300_000.0),
        "recovered total kvar is 300: {}",
        q_total / 1000.0
    );
}

/// Recursively collect every value stored under `key` anywhere in the JSON.
fn collect_key(v: &serde_json::Value, key: &str, out: &mut Vec<f64>) {
    match v {
        serde_json::Value::Object(m) => {
            if let Some(x) = m.get(key).and_then(serde_json::Value::as_f64) {
                out.push(x);
            }
            for c in m.values() {
                collect_key(c, key, out);
            }
        }
        serde_json::Value::Array(a) => {
            for c in a {
                collect_key(c, key, out);
            }
        }
        _ => {}
    }
}

/// dss to RAVENS: any distribution source now writes RAVENS, including a
/// transformer, which round-trips through the catalog `TransformerEndInfo`
/// records.
#[test]
fn dss_converts_to_ravens_with_transformer() {
    let dss = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data/dist/micro/xfmr_single_phase.dss");
    let net = parse_file(dss, None).unwrap();
    let conv = net.emit_value(DistTargetFormat::RavensJson);
    let back = parse_ravens_str(&conv.text).unwrap();
    assert_eq!(back.transformers().len(), net.transformers().len());
    assert_eq!(back.loads().len(), net.loads().len());
    assert_eq!(back.sources().len(), net.sources().len());

    // The transformer winding voltages and connections survive the trip
    // through the RAVENS catalog records.
    if let (Some(a), Some(b)) = (net.transformers().first(), back.transformers().first()) {
        assert_eq!(a.windings.len(), b.windings.len());
        for (wa, wb) in a.windings.iter().zip(&b.windings) {
            assert_eq!(wa.conn, wb.conn, "winding connection");
            assert!(
                close(wa.v_ref, wb.v_ref),
                "winding voltage {} vs {}",
                wa.v_ref,
                wb.v_ref
            );
        }
    }
}

/// A dss feeder with a ZIP load converts to RAVENS and reparses, exercising
/// the ZIP / exponential load-response path, and its findings carry codes.
#[test]
fn dss_zip_load_round_trips_through_ravens() {
    let dss = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data/dist/micro/onephase_zip_load.dss");
    let net = parse_file(dss, None).unwrap();
    let conv = net.emit_value(DistTargetFormat::RavensJson);
    assert!(
        conv.warnings
            .iter()
            .all(|w| w.contains("EMIT.RAVENS_DIST.")),
        "every writer finding carries an EMIT.RAVENS code: {:?}",
        conv.warnings
    );
    let back = parse_ravens_str(&conv.text).unwrap();
    assert_eq!(back.loads().len(), net.loads().len());
    // Writing the reparse again is byte identical (deterministic mRIDs).
    let second = back.emit_value(DistTargetFormat::RavensJson);
    assert_eq!(conv.text, second.text);
}
