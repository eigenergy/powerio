//! PSS/E `.raw` revision coverage: a case written at v34 and v35 reads back to
//! the same electrical core as the MATPOWER source it came from.
//!
//! `case14_v34.raw` / `case14_v35.raw` were produced from `case14.m` with
//! `powerio convert … --to psse34/psse35`, so they carry the modern deltas (the
//! system-wide header marker, the named 12-rating branch record, and the load
//! distributed-generation / load-type trailing columns). The reader takes the
//! revision from the file header and must recover the same network from each.
//!
//! Revision 32 is read only: `ExampleVersion32_exported.raw` and
//! `IEEE_30_bus.raw` come from PowSybl Core (see `tests/data/psse/README.md`)
//! and `case7_v32.raw` states every record type the reader maps in the
//! revision 32 layout. Fresh output of a revision 32 source uses revision 33.

// The base frequency is an exact decimal (60.0) read from the header; bit
// equality is the intended assertion.
#![allow(clippy::float_cmp)]
mod helpers;
#[allow(unused_imports)]
use helpers::*;

use std::path::{Path, PathBuf};

use powerio_core::Source;
use powerio_tx::{
    BalancedNetwork, BusId, BusType, LoadVoltageModel, SwitchedShuntMode, TargetFormat,
    TransformerControl, TransformerControlMode,
};

fn data(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data")
        .join(name)
}

fn read_psse(name: &str) -> BalancedNetwork {
    parse_psse(&std::fs::read_to_string(data(name)).unwrap()).unwrap()
}

#[derive(Debug, PartialEq)]
struct Core {
    buses: usize,
    branches: usize,
    gens: usize,
    loads: usize,
    load_p: i64,
    load_q: i64,
    gen_p: i64,
}

fn core(net: &BalancedNetwork) -> Core {
    let r = |x: f64| (x * 1e3).round() as i64;
    Core {
        buses: net.buses().len(),
        branches: net.branches().len(),
        gens: net.generators().len(),
        loads: net.loads().len(),
        load_p: r(net.loads().iter().map(|l| l.p).sum()),
        load_q: r(net.loads().iter().map(|l| l.q).sum()),
        gen_p: r(net.generators().iter().map(|g| g.pg).sum()),
    }
}

#[test]
fn v34_and_v35_fixtures_match_the_matpower_source() {
    let source = core(&parse_matpower_file(data("case14.m")).unwrap());
    let v34 = read_psse("psse/case14_v34.raw");
    let v35 = read_psse("psse/case14_v35.raw");

    assert_eq!(core(&v34), source, "v34 fixture lost or gained elements");
    assert_eq!(core(&v35), source, "v35 fixture lost or gained elements");
    // Frequency rides the header at every revision.
    assert_eq!(v34.base_frequency(), 60.0);
    assert_eq!(v35.base_frequency(), 60.0);
}

#[test]
fn transformer_control_round_trips_at_v34_and_v35() {
    // The count/sum checks above cannot see the winding line control columns:
    // v34/35 widen the line to twelve ratings and insert NODE after CONT, so
    // COD sits at 15 and RMA..NTP at 18..22. A regulating control must survive
    // a write/read cycle at both revisions.
    let mut net = parse_matpower_file(data("case14.m")).unwrap();
    let idx = net
        .branches()
        .iter()
        .position(powerio_tx::Branch::is_transformer)
        .expect("case14 has a transformer");
    let (from, to) = (net.branches()[idx].from, net.branches()[idx].to);
    let mut ctl = TransformerControl::new(TransformerControlMode::Voltage);
    ctl.controlled_bus = Some(to);
    ctl.tap_max = 1.08;
    ctl.tap_min = 0.92;
    ctl.band_max = 1.05;
    ctl.band_min = 0.98;
    ctl.ntp = 17;
    ctl.mva_base = 100.0;
    net.branches_mut()[idx].control = Some(ctl);

    for rev in [34u32, 35] {
        let text = emit_psse_rev(&net, rev).text;
        let back = parse_psse(&text).unwrap();
        let br = back
            .branches()
            .iter()
            .find(|b| b.from == from && b.to == to)
            .unwrap();
        let c = br
            .control
            .as_ref()
            .unwrap_or_else(|| panic!("rev {rev} lost the transformer control"));
        assert_eq!(c.mode, TransformerControlMode::Voltage, "rev {rev} COD");
        assert_eq!(c.controlled_bus, Some(to), "rev {rev} CONT");
        assert!((c.tap_max - 1.08).abs() < 1e-12, "rev {rev} RMA");
        assert!((c.tap_min - 0.92).abs() < 1e-12, "rev {rev} RMI");
        assert!((c.band_max - 1.05).abs() < 1e-12, "rev {rev} VMA");
        assert!((c.band_min - 0.98).abs() < 1e-12, "rev {rev} VMI");
        assert_eq!(c.ntp, 17, "rev {rev} NTP");
    }
}

fn close(actual: f64, expected: f64, what: &str) {
    assert!(
        (actual - expected).abs() < 1e-9,
        "{what}: {actual} != {expected}"
    );
}

/// The PowSybl revision 32 export example reads to the records it states,
/// with the bus voltage limits at the revision 33 defaults the record does not
/// carry.
#[test]
fn revision32_example_fixture_reads_the_stated_records() {
    let example = read_psse("psse/ExampleVersion32_exported.raw");
    assert_eq!(example.buses().len(), 8);
    assert_eq!(example.branches().len(), 8);
    assert_eq!(
        example
            .branches()
            .iter()
            .filter(|branch| branch.is_transformer())
            .count(),
        4
    );
    assert_eq!(example.generators().len(), 1);
    assert_eq!(example.loads().len(), 2);
    assert_eq!(example.shunts().len(), 2);
    assert_eq!(example.areas().len(), 1);
    assert_eq!(example.base_frequency(), 60.0);
    let swing = &example.buses()[0];
    assert_eq!(swing.kind, BusType::Ref);
    close(swing.base_kv, 138.0, "swing base kV");
    close(swing.vm, 1.02, "swing vm");
    close(
        swing.vmax,
        1.1,
        "revision 32 has no NVHI; the default applies",
    );
    close(
        swing.vmin,
        0.9,
        "revision 32 has no NVLO; the default applies",
    );
    assert_eq!(swing.evhi, None);
    let generator = &example.generators()[0];
    assert_eq!(generator.bus, BusId(6));
    close(generator.pg, 30.2, "PG");
    close(generator.qmax, 2.7, "QT");
    close(generator.qmin, -1.8, "QB");
    close(generator.vg, 1.025, "VS");
    close(generator.mbase, 6.2, "MBASE");
    close(generator.pmax, 200.6, "PT");
    close(generator.pmin, -200.0, "PB");
    let load = example
        .loads()
        .iter()
        .find(|load| load.bus == BusId(8))
        .unwrap();
    close(load.p, 8.631, "PL");
    close(load.q, 2.314, "QL");
    let transformer = example
        .branches()
        .iter()
        .find(|branch| branch.from == BusId(4) && branch.to == BusId(5))
        .unwrap();
    close(transformer.x, 0.20912, "X1-2");
    close(transformer.calc_effective_tap(), 0.978, "WINDV1");
    let control = transformer.control.as_ref().unwrap();
    assert_eq!(control.mode, TransformerControlMode::Fixed);
    assert_eq!(control.ntp, 10);
    close(control.tap_max, 1.5, "RMA");
    close(control.tap_min, 0.51, "RMI");
    assert!(
        example
            .branches()
            .iter()
            .any(|branch| branch.from == BusId(3) && branch.to == BusId(3)),
        "the self loop on bus 3 is kept"
    );
    let fixed = example
        .shunts()
        .iter()
        .find(|shunt| shunt.control.is_none())
        .unwrap();
    assert_eq!(fixed.bus, BusId(7));
    close(fixed.b, 19.0, "BL");
    let switched = example
        .shunts()
        .iter()
        .find(|shunt| shunt.control.is_some())
        .unwrap();
    assert_eq!(switched.bus, BusId(7));
    close(switched.b, 0.0, "BINIT");
    let control = switched.control.as_ref().unwrap();
    assert_eq!(control.mode, SwitchedShuntMode::Continuous);
    close(control.vhigh, 1.051, "VSWHI");
    close(control.vlow, 1.0, "VSWLO");
    assert_eq!(control.blocks.len(), 1);
    assert_eq!(control.blocks[0].steps, 1);
    close(control.blocks[0].b, 14.95, "B1");
}

/// The IEEE 30 bus revision 32 case shares its element counts with MATPOWER
/// `case30.m`. That MATPOWER case carries the Alsac and Stott data rather than
/// the University of Washington archive values, so the electrical values are
/// checked against the RAW text itself.
#[test]
fn revision32_ieee30_fixture_matches_case30_counts_and_its_records() {
    let ieee30 = read_psse("psse/IEEE_30_bus.raw");
    let matpower = parse_matpower_file(data("case30.m")).unwrap();
    assert_eq!(ieee30.buses().len(), matpower.buses().len());
    assert_eq!(ieee30.branches().len(), matpower.branches().len());
    assert_eq!(ieee30.generators().len(), matpower.generators().len());
    assert_eq!(ieee30.loads().len(), 21);
    assert_eq!(ieee30.shunts().len(), 2);
    assert_eq!(
        ieee30
            .branches()
            .iter()
            .filter(|branch| branch.is_transformer())
            .count(),
        4
    );
    close(
        ieee30.loads().iter().map(|load| load.p).sum(),
        283.4,
        "IEEE 30 total MW demand",
    );
    close(
        ieee30.loads().iter().map(|load| load.q).sum(),
        126.2,
        "IEEE 30 total MVAr demand",
    );
    let glen_lyn = &ieee30.buses()[0];
    assert_eq!(glen_lyn.name.as_deref(), Some("Glen Lyn"));
    assert_eq!(glen_lyn.kind, BusType::Ref);
    close(glen_lyn.base_kv, 132.0, "Glen Lyn base kV");
    close(glen_lyn.vm, 1.06, "Glen Lyn vm");
    let line = &ieee30.branches()[0];
    assert_eq!((line.from, line.to), (BusId(1), BusId(2)));
    close(line.r, 0.0192, "R");
    close(line.x, 0.0575, "X");
    close(line.b, 0.0528, "B");
    close(line.rate_a, 130.0, "RATEA");
    let transformer = ieee30
        .branches()
        .iter()
        .find(|branch| branch.from == BusId(4) && branch.to == BusId(12))
        .unwrap();
    close(transformer.x, 0.256, "X1-2");
    close(transformer.calc_effective_tap(), 0.932, "WINDV1");
    close(transformer.rate_a, 65.0, "RATA1");
    let generator = &ieee30.generators()[0];
    close(generator.pg, 260.948, "PG");
    close(generator.qg, -16.787, "QG");
    close(generator.pmax, 10000.0, "PT");
}

/// `case7_v32.raw` maps every record type the reader handles; the bus, load,
/// shunt, and generator records come first.
#[test]
fn hand_written_revision32_case_maps_bus_load_shunt_and_generator_records() {
    let net = read_psse("psse/case7_v32.raw");
    assert_eq!(net.name(), "case7_v32");
    assert_eq!(net.buses().len(), 7);
    assert_eq!(net.loads().len(), 3);
    assert_eq!(net.shunts().len(), 2);
    assert_eq!(net.generators().len(), 2);
    assert_eq!(net.branches().len(), 5);
    assert_eq!(net.transformers_3w().len(), 1);
    assert_eq!(net.hvdc().len(), 1);
    assert_eq!(net.areas().len(), 2);

    let rectifier_bus = &net.buses()[5];
    assert_eq!((rectifier_bus.area, rectifier_bus.zone), (2, 2));
    let zip = net
        .loads()
        .iter()
        .find(|load| load.bus == BusId(4))
        .unwrap();
    close(zip.p, 48.0, "PL + IP + YP");
    close(zip.q, 13.0, "QL + IQ + YQ");
    let Some(LoadVoltageModel::Zip {
        p_constant_current,
        q_constant_impedance,
        ..
    }) = &zip.voltage_model
    else {
        panic!("the ZIP load is typed");
    };
    close(*p_constant_current, 5.0, "IP");
    close(*q_constant_impedance, 1.0, "YQ");
    let fixed = net
        .shunts()
        .iter()
        .find(|shunt| shunt.control.is_none())
        .unwrap();
    close(fixed.b, 15.0, "BL");
    let switched = net
        .shunts()
        .iter()
        .find(|shunt| shunt.control.is_some())
        .unwrap();
    close(switched.b, 10.0, "BINIT");
    let control = switched.control.as_ref().unwrap();
    assert_eq!(control.mode, SwitchedShuntMode::Discrete);
    close(control.vhigh, 1.05, "VSWHI");
    close(control.vlow, 0.95, "VSWLO");
    assert_eq!(
        control
            .blocks
            .iter()
            .map(|block| (block.steps, block.b))
            .collect::<Vec<_>>(),
        vec![(2, 5.0), (1, 10.0)]
    );
    let generator = &net.generators()[0];
    close(generator.pg, 120.0, "PG");
    close(generator.mbase, 200.0, "MBASE");
    close(generator.pmax, 250.0, "PT");
    close(generator.pmin, 10.0, "PB");
}

/// `case7_v32.raw` branch, two winding, and three winding transformer records
/// in the revision 32 layout: no VECGRP on the transformer record and no CNXA
/// on the winding lines.
#[test]
fn hand_written_revision32_case_maps_branch_and_transformer_records() {
    let net = read_psse("psse/case7_v32.raw");
    let line = &net.branches()[0];
    close(line.rate_b, 320.0, "RATEB");
    close(line.rate_c, 350.0, "RATEC");
    assert_eq!(
        line.extras
            .get("psse_len")
            .and_then(serde_json::Value::as_f64),
        Some(50.0)
    );
    let transformer = net
        .branches()
        .iter()
        .find(|branch| branch.is_transformer())
        .unwrap();
    assert_eq!((transformer.from, transformer.to), (BusId(2), BusId(3)));
    close(transformer.calc_effective_tap(), 1.025, "WINDV1 / WINDV2");
    close(transformer.x, 0.1, "X1-2");
    let charging = transformer.calc_terminal_charging();
    close(charging.g_fr, 0.001, "MAG1");
    close(charging.b_fr, -0.02, "MAG2");
    let control = transformer.control.as_ref().unwrap();
    assert_eq!(control.mode, TransformerControlMode::Voltage);
    assert!(control.enabled);
    assert_eq!(control.controlled_bus, Some(BusId(3)));
    close(control.band_max, 1.05, "VMA");
    close(control.band_min, 0.95, "VMI");
    assert_eq!(control.winding_connection_angle, None);
    let three_winding = &net.transformers_3w()[0];
    assert_eq!(
        three_winding
            .windings
            .iter()
            .map(|winding| winding.bus)
            .collect::<Vec<_>>(),
        vec![BusId(1), BusId(4), BusId(5)]
    );
    close(three_winding.z[1].x, 0.2, "X2-3");
    close(three_winding.star_vm, 0.98, "VMSTAR");
    close(three_winding.star_va, -1.5, "ANSTAR");
    close(three_winding.windings[2].shift, 30.0, "ANG3");
    close(three_winding.windings[2].tap, 0.95, "WINDV3");
}

/// `case7_v32.raw` area and two-terminal DC records, and the zone, owner,
/// inter-area transfer, and impedance correction records the reader skips
/// and reports. No record is short, so nothing is reported as defaulted.
#[test]
fn hand_written_revision32_case_maps_area_dc_and_skipped_records() {
    let parsed = parse_file(data("psse/case7_v32.raw"), Some("psse")).unwrap();
    let net = &parsed.network;
    let areas = net.areas();
    assert_eq!(areas[0].slack_bus, Some(BusId(1)));
    close(areas[0].tolerance, 10.0, "PTOL");
    assert_eq!(areas[0].name.as_deref(), Some("AREA ONE"));
    assert_eq!(areas[1].slack_bus, None);
    close(areas[1].net_interchange, -50.0, "PDES");
    let dc = &net.hvdc()[0];
    assert_eq!((dc.from, dc.to), (BusId(6), BusId(7)));
    assert!(dc.in_service);
    close(dc.pf, 100.0, "SETVL at the rectifier");
    close(dc.pt, 99.8, "SETVL minus the I squared RDC drop");
    assert_eq!(
        dc.extras
            .get("psse_dc_rdc")
            .and_then(serde_json::Value::as_f64),
        Some(5.0)
    );
    assert!(
        dc.extras.contains_key("psse_dc_rectifier_tail"),
        "a converter line that differs from the writer's default is retained"
    );

    let lines = parsed.render_diagnostics();
    for section in [
        "ZONE section (2 record line(s))",
        "OWNER section (1 record line(s))",
        "INTER-AREA TRANSFER section (1 record line(s))",
        "IMPEDANCE CORRECTION section (1 record line(s))",
    ] {
        assert!(
            lines.iter().any(|line| {
                line.starts_with("READ.PSSE.SECTION_UNSUPPORTED") && line.contains(section)
            }),
            "{section}: {lines:?}"
        );
    }
    assert!(
        !lines
            .iter()
            .any(|line| line.starts_with("READ.PSSE.VALUE_DEFAULTED")),
        "{lines:?}"
    );
}

/// Every revision 32 source writes fresh at revision 33, reads back to the same
/// electrical core, and converts to MATPOWER.
#[test]
fn revision32_sources_write_at_33_and_convert_to_matpower() {
    for name in [
        "psse/ExampleVersion32_exported.raw",
        "psse/IEEE_30_bus.raw",
        "psse/case7_v32.raw",
    ] {
        let net = read_psse(name);
        let written = emit_psse_rev(&net, 33);
        let header = written.text.lines().next().unwrap();
        assert!(header.contains(", 33, "), "{name}: {header}");
        let back = parse_psse(&written.text)
            .unwrap_or_else(|error| panic!("{name}: fresh revision 33 text: {error}"));
        assert_eq!(core(&back), core(&net), "{name}");
        assert_eq!(back.shunts().len(), net.shunts().len(), "{name}");
        assert_eq!(
            back.transformers_3w().len(),
            net.transformers_3w().len(),
            "{name}"
        );
        assert_eq!(back.hvdc().len(), net.hvdc().len(), "{name}");
        assert_eq!(back.areas().len(), net.areas().len(), "{name}");
        for (before, after) in net.buses().iter().zip(back.buses()) {
            assert_eq!(before.id, after.id, "{name}");
            close(after.vm, before.vm, name);
            close(after.vmax, before.vmax, name);
        }

        let matpower = emit_matpower(&net);
        let converted = parse_str(&matpower, "matpower")
            .unwrap_or_else(|error| panic!("{name}: MATPOWER conversion: {error}"))
            .network;
        assert_eq!(converted.buses().len(), net.buses().len(), "{name}");
        assert_eq!(
            converted.generators().len(),
            net.generators().len(),
            "{name}"
        );
    }
}

/// A parsed revision 32 module retains its source text like every other
/// revision. No emission target names revision 32, so a same format write
/// serializes fresh revision 33 text rather than returning the retained text,
/// and it is not reported as a downgrade.
#[test]
fn revision32_module_retains_its_source_and_writes_fresh_33() {
    let path = data("psse/ExampleVersion32_exported.raw");
    let module = powerio_tx::parse(Source::open(&path).unwrap()).unwrap();
    let retained = module.source().unwrap().primary_buffer().unwrap();
    assert_eq!(retained.bytes(), std::fs::read(&path).unwrap().as_slice());

    let emitted = emit_module(&module, TargetFormat::Psse { rev: 33 }).unwrap();
    assert!(emitted.text.lines().next().unwrap().contains(", 33, "));
    assert_ne!(emitted.text.as_bytes(), retained.bytes());
    assert!(
        !emitted
            .render_diagnostics()
            .iter()
            .any(|line| line.starts_with("EMIT.PSSE.DOWNGRADED")),
        "{:?}",
        emitted.render_diagnostics()
    );
    let back = parse_psse(&emitted.text).unwrap();
    assert_eq!(core(&back), core(module.value()));
}

/// A revision 32 record that ends before its last typed field is reported
/// with the byte range of that record in the retained source, offset by the
/// byte order mark when the source carries one.
#[test]
fn short_revision32_record_is_reported_with_its_span() {
    let body = "0, 100.00, 32, 0, 0, 60.00\nCASE\nCOMMENT\n\
                1,'B1          ',230.0,3,1,1,1\n\
                2,'B2          ',230.0,1,1,1,1,1.0,0.0\n\
                0 / END OF BUS DATA, BEGIN LOAD DATA\n\
                2,'1 ',1,1,1,10.0\n\
                0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA\nQ\n";
    for prefix in ["", "\u{feff}"] {
        let text = format!("{prefix}{body}");
        let source = Source::from_memory("short.raw", text.as_bytes().to_vec()).unwrap();
        let module = powerio_tx::parse(source).unwrap();
        let reported: Vec<_> = module
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code() == "READ.PSSE.VALUE_DEFAULTED")
            .collect();
        assert_eq!(reported.len(), 2, "{:?}", module.diagnostics);
        for (diagnostic, record) in reported
            .iter()
            .zip(["1,'B1          ',230.0,3,1,1,1", "2,'1 ',1,1,1,10.0"])
        {
            assert!(
                diagnostic.message().contains("revision 32"),
                "{diagnostic:?}"
            );
            let spans = diagnostic.spans();
            assert_eq!(spans.len(), 1, "{diagnostic:?}");
            let start = text.find(record).unwrap() as u64;
            assert_eq!(spans[0].byte_start(), start, "{record}");
            assert_eq!(spans[0].byte_end(), start + record.len() as u64, "{record}");
            assert_eq!(
                spans[0].source(),
                module.sources()[0].id(),
                "the span names the module source"
            );
        }
        let bus = &module.value().buses()[0];
        close(bus.vm, 1.0, "VM defaulted");
        close(bus.va, 0.0, "VA defaulted");
    }
}
