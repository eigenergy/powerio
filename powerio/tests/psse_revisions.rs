//! PSS/E `.raw` revision coverage: a case written at v34 and v35 reads back to
//! the same electrical core as the MATPOWER source it came from.
//!
//! `case14_v34.raw` / `case14_v35.raw` were produced from `case14.m` with
//! `powerio convert … --to psse34/psse35`, so they carry the modern deltas (the
//! system-wide header marker, the named 12-rating branch record, and the load
//! distributed-generation / load-type trailing columns). The reader takes the
//! revision from the file header and must recover the same network from each.

// The base frequency is an exact decimal (60.0) read from the header; bit
// equality is the intended assertion.
#![allow(clippy::float_cmp)]

use std::path::{Path, PathBuf};

use powerio::{
    Network, TransformerControl, TransformerControlMode, parse_matpower_file, parse_psse,
    write_psse_rev,
};

fn data(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data")
        .join(name)
}

fn read_psse(name: &str) -> Network {
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

fn core(net: &Network) -> Core {
    let r = |x: f64| (x * 1e3).round() as i64;
    Core {
        buses: net.buses.len(),
        branches: net.branches.len(),
        gens: net.generators.len(),
        loads: net.loads.len(),
        load_p: r(net.loads.iter().map(|l| l.p).sum()),
        load_q: r(net.loads.iter().map(|l| l.q).sum()),
        gen_p: r(net.generators.iter().map(|g| g.pg).sum()),
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
    assert_eq!(v34.base_frequency, 60.0);
    assert_eq!(v35.base_frequency, 60.0);
}

#[test]
fn transformer_control_round_trips_at_v34_and_v35() {
    // The count/sum checks above cannot see the winding line control columns:
    // v34/35 widen the line to twelve ratings and insert NODE after CONT, so
    // COD sits at 15 and RMA..NTP at 18..22. A regulating control must survive
    // a write/read cycle at both revisions.
    let mut net = parse_matpower_file(data("case14.m")).unwrap();
    let idx = net
        .branches
        .iter()
        .position(powerio::Branch::is_transformer)
        .expect("case14 has a transformer");
    let (from, to) = (net.branches[idx].from, net.branches[idx].to);
    let mut ctl = TransformerControl::new(TransformerControlMode::Voltage);
    ctl.controlled_bus = Some(to);
    ctl.tap_max = 1.08;
    ctl.tap_min = 0.92;
    ctl.band_max = 1.05;
    ctl.band_min = 0.98;
    ctl.ntp = 17;
    ctl.mva_base = 100.0;
    net.branches[idx].control = Some(ctl);

    for rev in [34u32, 35] {
        let text = write_psse_rev(&net, rev).text;
        let back = parse_psse(&text).unwrap();
        let br = back
            .branches
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
