//! All-pairs converter harness. For every format with a reader, this checks the
//! two-tier fidelity contract against the vendored cases:
//!
//! - **core preservation** — MATPOWER → format → `Network` keeps the electrical
//!   core (bus/branch/gen/load/shunt counts, total demand, total generation,
//!   base);
//! - **reader∘writer idempotence** — serialize → read → serialize is stable;
//! - **same-format byte-exact echo** — reading a format then writing it back
//!   reproduces the bytes.
//!
//! EGRET has no reader yet, so it gets the write-side checks only. PowerModels'
//! value-for-value check against PowerModels.jl lives in
//! `benchmarks/validate_powermodels.jl`.

use std::path::{Path, PathBuf};

use caseio::{
    parse_matpower_file, parse_powermodels_json, parse_powerworld, parse_psse, write_as,
    write_egret_json, write_powermodels_json, write_powerworld, write_psse, Network, TargetFormat,
};

fn data(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data").join(name)
}

const CASES: [&str; 5] = ["case9.m", "case14.m", "case30.m", "case57.m", "case118.m"];

/// Electrical core of a network, compared across conversions (rounded so the
/// MW/p.u. round-trips don't trip exact float equality).
#[derive(Debug, PartialEq)]
struct Fingerprint {
    buses: usize,
    branches: usize,
    gens: usize,
    loads: usize,
    shunts: usize,
    load_p: i64,
    load_q: i64,
    gen_p: i64,
    base_mva: i64,
}

fn fingerprint(net: &Network) -> Fingerprint {
    let r = |x: f64| (x * 1e3).round() as i64;
    Fingerprint {
        buses: net.buses.len(),
        branches: net.branches.len(),
        gens: net.generators.len(),
        loads: net.loads.len(),
        shunts: net.shunts.len(),
        load_p: r(net.loads.iter().map(|l| l.p).sum()),
        load_q: r(net.loads.iter().map(|l| l.q).sum()),
        gen_p: r(net.generators.iter().map(|g| g.pg).sum()),
        base_mva: r(net.base_mva),
    }
}

/// One format with a reader and a direct serializer (the serializer ignores the
/// same-format echo so we exercise the real writer).
struct Roundtrippable {
    name: &'static str,
    format: TargetFormat,
    write: fn(&Network) -> String,
    read: fn(&str) -> Network,
}

fn roundtrippable() -> Vec<Roundtrippable> {
    vec![
        Roundtrippable {
            name: "PowerModels JSON",
            format: TargetFormat::PowerModelsJson,
            write: |n| write_powermodels_json(n).text,
            read: |s| parse_powermodels_json(s).unwrap(),
        },
        Roundtrippable {
            name: "PSS/E .raw",
            format: TargetFormat::Psse,
            write: |n| write_psse(n).text,
            read: |s| parse_psse(s).unwrap(),
        },
        Roundtrippable {
            name: "PowerWorld .aux",
            format: TargetFormat::PowerWorld,
            write: |n| write_powerworld(n).text,
            read: |s| parse_powerworld(s).unwrap(),
        },
    ]
}

#[test]
fn core_preserved_through_each_format() {
    for case in CASES {
        let net0 = parse_matpower_file(data(case)).unwrap();
        let fp0 = fingerprint(&net0);
        for fmt in roundtrippable() {
            let net1 = (fmt.read)(&(fmt.write)(&net0));
            assert_eq!(
                fingerprint(&net1),
                fp0,
                "{case} via {}: electrical core changed",
                fmt.name
            );
        }
    }
}

#[test]
fn reader_writer_is_idempotent() {
    for case in CASES {
        let net0 = parse_matpower_file(data(case)).unwrap();
        for fmt in roundtrippable() {
            let t0 = (fmt.write)(&net0);
            let t1 = (fmt.write)(&(fmt.read)(&t0));
            assert_eq!(t0, t1, "{case} via {}: serialize→read→serialize not stable", fmt.name);
        }
    }
}

#[test]
fn same_format_round_trip_is_byte_exact() {
    for case in CASES {
        let net0 = parse_matpower_file(data(case)).unwrap();
        for fmt in roundtrippable() {
            let text = (fmt.write)(&net0);
            let net_from_text = (fmt.read)(&text); // carries source = text, format = fmt
            assert_eq!(
                write_as(&net_from_text, fmt.format).text,
                text,
                "{case} {}: same-format write is not a byte-exact echo",
                fmt.name
            );
        }
    }
}

#[test]
fn cross_format_powermodels_to_psse_and_powerworld() {
    // A non-MATPOWER source through the hub to two other formats, core preserved.
    let net0 = parse_matpower_file(data("case30.m")).unwrap();
    let pm = write_powermodels_json(&net0).text;
    let from_pm = parse_powermodels_json(&pm).unwrap();
    let fp = fingerprint(&net0);
    assert_eq!(fingerprint(&parse_psse(&write_psse(&from_pm).text).unwrap()), fp);
    assert_eq!(fingerprint(&parse_powerworld(&write_powerworld(&from_pm).text).unwrap()), fp);
}

#[test]
fn egret_write_side_only() {
    // No EGRET reader yet: confirm valid JSON and clean warnings (case30 has no
    // dcline/storage to drop).
    let net0 = parse_matpower_file(data("case30.m")).unwrap();
    let conv = write_egret_json(&net0);
    let v: serde_json::Value = serde_json::from_str(&conv.text).unwrap();
    assert!(v["elements"]["bus"].as_object().unwrap().len() == net0.buses.len());
    assert!(conv.warnings.is_empty(), "unexpected warnings: {:?}", conv.warnings);
}
