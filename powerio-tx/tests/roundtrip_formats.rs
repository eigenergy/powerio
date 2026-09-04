//! All-pairs converter harness. For every format with a reader, this checks the
//! two-tier fidelity behavior against the vendored cases:
//!
//! - **core preservation** — MATPOWER → format → `BalancedNetwork` keeps the electrical
//!   core (bus/branch/gen/load/shunt counts, total demand, total generation,
//!   base);
//! - **reader∘writer idempotence** — serialize → read → serialize is stable;
//! - **same-format byte-exact echo** — reading a format then writing it back
//!   reproduces the bytes.
//!
//! All five formats (MATPOWER, PowerModels JSON, PSS/E, PowerWorld, egret) have a
//! reader and a writer, so each runs the full set. PowerModels' and egret's
//! value-for-value checks against the reference tools live in
//! `benchmarks/validate_powermodels.jl` and `benchmarks/validate_egret.py`.
mod helpers;
#[allow(unused_imports)]
use helpers::*;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use powerio_tx::{BalancedNetwork, BusType, TargetFormat};

mod common;
use common::json_approx_eq;

fn write_psse34(n: &BalancedNetwork) -> String {
    emit_psse_rev(n, 34).text
}
fn write_psse35(n: &BalancedNetwork) -> String {
    emit_psse_rev(n, 35).text
}

fn data(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data")
        .join(name)
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

fn fingerprint(net: &BalancedNetwork) -> Fingerprint {
    let r = |x: f64| (x * 1e3).round() as i64;
    Fingerprint {
        buses: net.buses().len(),
        branches: net.branches().len(),
        gens: net.generators().len(),
        loads: net.loads().len(),
        shunts: net.shunts().len(),
        load_p: r(net.loads().iter().map(|l| l.p).sum()),
        load_q: r(net.loads().iter().map(|l| l.q).sum()),
        gen_p: r(net.generators().iter().map(|g| g.pg).sum()),
        base_mva: r(net.base_mva()),
    }
}

#[derive(Debug, PartialEq)]
struct ValueFingerprint {
    base_mva: i64,
    buses: Vec<BusValue>,
    loads_by_bus: Vec<BusInjection>,
    shunts_by_bus: Vec<BusInjection>,
    branches: Vec<BranchValue>,
    generators: Vec<GeneratorValue>,
}

#[derive(Debug, PartialEq)]
struct BusValue {
    id: usize,
    kind: BusType,
    vm: i64,
    va: i64,
    base_kv: i64,
    vmax: i64,
    vmin: i64,
    area: usize,
    zone: usize,
}

#[derive(Debug, PartialEq)]
struct BusInjection {
    bus: usize,
    p_or_g: i64,
    q_or_b: i64,
}

#[derive(Debug, PartialEq)]
struct BranchValue {
    from: usize,
    to: usize,
    occurrence: usize,
    r: i64,
    x: i64,
    b: Option<i64>,
    rate_a: i64,
    rate_b: i64,
    rate_c: i64,
    tap: i64,
    shift: i64,
    in_service: bool,
    angmin: i64,
    angmax: i64,
}

#[derive(Debug, PartialEq)]
struct GeneratorValue {
    bus: usize,
    occurrence: usize,
    pg: i64,
    qg: i64,
    pmax: i64,
    pmin: i64,
    qmax: i64,
    qmin: i64,
    vg: Option<i64>,
    mbase: i64,
    in_service: bool,
}

fn round_value(x: f64) -> i64 {
    (x * 1e5).round() as i64
}

fn value_fingerprint(net: &BalancedNetwork, target: TargetFormat) -> ValueFingerprint {
    ValueFingerprint {
        base_mva: round_value(net.base_mva()),
        buses: bus_values(net),
        // Sum only in-service injections: the by-bus aggregation cannot carry a
        // per-element service flag, so counting out-of-service p/q would both
        // mask a writer that flips in_service and fail a writer that correctly
        // drops out-of-service injections.
        loads_by_bus: bus_injections(
            net.loads()
                .iter()
                .filter(|load| load.in_service)
                .map(|load| (load.bus.0, load.p, load.q)),
        ),
        shunts_by_bus: bus_injections(
            net.shunts()
                .iter()
                .filter(|shunt| shunt.in_service)
                .map(|shunt| (shunt.bus.0, shunt.g, shunt.b)),
        ),
        branches: branch_values(net, target),
        generators: generator_values(net, target),
    }
}

fn bus_values(net: &BalancedNetwork) -> Vec<BusValue> {
    let mut buses: Vec<_> = net
        .buses()
        .iter()
        .map(|bus| BusValue {
            id: bus.id.0,
            kind: bus.kind,
            vm: round_value(bus.vm),
            va: round_value(bus.va),
            base_kv: round_value(bus.base_kv),
            vmax: round_value(bus.vmax),
            vmin: round_value(bus.vmin),
            area: bus.area,
            zone: bus.zone,
        })
        .collect();
    buses.sort_by_key(|b| b.id);
    buses
}

fn bus_injections<I>(values: I) -> Vec<BusInjection>
where
    I: IntoIterator<Item = (usize, f64, f64)>,
{
    let mut by_bus = BTreeMap::<usize, (f64, f64)>::new();
    for (bus, p_or_g, q_or_b) in values {
        let entry = by_bus.entry(bus).or_default();
        entry.0 += p_or_g;
        entry.1 += q_or_b;
    }
    by_bus
        .into_iter()
        .map(|(bus, (p_or_g, q_or_b))| BusInjection {
            bus,
            p_or_g: round_value(p_or_g),
            q_or_b: round_value(q_or_b),
        })
        .collect()
}

fn branch_values(net: &BalancedNetwork, target: TargetFormat) -> Vec<BranchValue> {
    let mut branch_counts = BTreeMap::<(usize, usize), usize>::new();
    let mut branches: Vec<_> = net
        .branches()
        .iter()
        .map(|branch| {
            let key = (branch.from.0, branch.to.0);
            let occurrence = *branch_counts
                .entry(key)
                .and_modify(|n| *n += 1)
                .or_insert(0);
            BranchValue {
                from: branch.from.0,
                to: branch.to.0,
                occurrence,
                r: round_value(branch.r),
                x: round_value(branch.x),
                b: (!(target == TargetFormat::Pslf && branch.is_transformer()))
                    .then_some(round_value(branch.calc_total_charging_b())),
                rate_a: round_value(branch.rate_a),
                rate_b: round_value(branch.rate_b),
                rate_c: round_value(branch.rate_c),
                tap: round_value(branch.tap),
                shift: round_value(branch.shift),
                in_service: branch.in_service,
                angmin: round_value(branch.angmin),
                angmax: round_value(branch.angmax),
            }
        })
        .collect();
    branches.sort_by_key(|b| (b.from, b.to, b.occurrence));
    branches
}

fn generator_values(net: &BalancedNetwork, target: TargetFormat) -> Vec<GeneratorValue> {
    let mut generator_counts = BTreeMap::<usize, usize>::new();
    let mut generators: Vec<_> = net
        .generators()
        .iter()
        .map(|generator| {
            let occurrence = *generator_counts
                .entry(generator.bus.0)
                .and_modify(|n| *n += 1)
                .or_insert(0);
            GeneratorValue {
                bus: generator.bus.0,
                occurrence,
                pg: round_value(generator.pg),
                qg: round_value(generator.qg),
                pmax: round_value(generator.pmax),
                pmin: round_value(generator.pmin),
                qmax: round_value(generator.qmax),
                qmin: round_value(generator.qmin),
                vg: (target != TargetFormat::Pslf).then_some(round_value(generator.vg)),
                mbase: round_value(generator.mbase),
                in_service: generator.in_service,
            }
        })
        .collect();
    generators.sort_by_key(|g| (g.bus, g.occurrence));
    generators
}

fn assert_value_fingerprint_eq(got: &ValueFingerprint, expected: &ValueFingerprint, context: &str) {
    assert_eq!(got.base_mva, expected.base_mva, "{context}: base_mva");
    assert_eq!(got.buses, expected.buses, "{context}: buses");
    assert_eq!(
        got.loads_by_bus, expected.loads_by_bus,
        "{context}: loads by bus"
    );
    assert_eq!(
        got.shunts_by_bus, expected.shunts_by_bus,
        "{context}: shunts by bus"
    );
    assert_eq!(
        got.branches.len(),
        expected.branches.len(),
        "{context}: branch count"
    );
    for (i, (got, expected)) in got.branches.iter().zip(&expected.branches).enumerate() {
        assert_eq!(got, expected, "{context}: branch {i}");
    }
    assert_eq!(
        got.generators.len(),
        expected.generators.len(),
        "{context}: generator count"
    );
    for (i, (got, expected)) in got.generators.iter().zip(&expected.generators).enumerate() {
        assert_eq!(got, expected, "{context}: generator {i}");
    }
}

/// One format with a reader and a direct serializer (the serializer ignores the
/// same-format echo so we exercise the real writer).
struct Roundtrippable {
    name: &'static str,
    format: TargetFormat,
    write: fn(&BalancedNetwork) -> String,
    /// The declared format token the module read uses.
    token: &'static str,
}

fn read_module(text: &str, token: &str) -> powerio_core::PioModule<BalancedNetwork> {
    let source = powerio_core::Source::from_memory("case", text.as_bytes().to_vec())
        .unwrap()
        .with_format(powerio_tx::parse_format_id(token).unwrap());
    powerio_tx::parse(source).unwrap()
}

fn roundtrippable() -> Vec<Roundtrippable> {
    vec![
        Roundtrippable {
            name: "PowerModels JSON",
            format: TargetFormat::PowerModelsJson,
            write: |n| emit_powermodels_json(n).text,
            token: "powermodels-json",
        },
        Roundtrippable {
            name: "PSS/E .raw",
            format: TargetFormat::Psse { rev: 33 },
            write: |n| emit_psse(n).text,
            token: "psse",
        },
        Roundtrippable {
            name: "PSS/E .raw v34",
            format: TargetFormat::Psse { rev: 34 },
            write: write_psse34,
            token: "psse",
        },
        Roundtrippable {
            name: "PSS/E .raw v35",
            format: TargetFormat::Psse { rev: 35 },
            write: write_psse35,
            token: "psse",
        },
        Roundtrippable {
            name: "PowerWorld .aux",
            format: TargetFormat::PowerWorld,
            write: |n| emit_powerworld(n).text,
            token: "powerworld",
        },
        Roundtrippable {
            name: "egret JSON",
            format: TargetFormat::EgretJson,
            write: |n| emit_egret_json(n).text,
            token: "egret-json",
        },
        Roundtrippable {
            name: "PSLF .epc",
            format: TargetFormat::Pslf,
            write: |n| emit_pslf(n).text,
            token: "pslf",
        },
    ]
}

#[test]
fn core_preserved_through_each_format() {
    for case in CASES {
        let net0 = parse_matpower_file(data(case)).unwrap();
        let fp0 = fingerprint(&net0);
        for fmt in roundtrippable() {
            let net1 = read_module(&(fmt.write)(&net0), fmt.token).into_value();
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
fn stable_element_values_preserved_through_each_format() {
    for case in CASES {
        let net0 = parse_matpower_file(data(case)).unwrap();
        for fmt in roundtrippable() {
            let fp0 = value_fingerprint(&net0, fmt.format);
            let net1 = read_module(&(fmt.write)(&net0), fmt.token).into_value();
            assert_value_fingerprint_eq(
                &value_fingerprint(&net1, fmt.format),
                &fp0,
                &format!("{case} via {}", fmt.name),
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
            let t1 = (fmt.write)(&read_module(&t0, fmt.token).into_value());
            if fmt.format == TargetFormat::PowerModelsJson {
                // PowerModels JSON is per-unit; the ÷base / ×base round-trip is not
                // bit-exact in f64, so compare structure and values with a tolerance.
                let v0: serde_json::Value = serde_json::from_str(&t0).unwrap();
                let v1: serde_json::Value = serde_json::from_str(&t1).unwrap();
                assert!(
                    json_approx_eq(&v0, &v1),
                    "{case} via {}: serialize→read→serialize not stable",
                    fmt.name
                );
            } else {
                assert_eq!(
                    t0, t1,
                    "{case} via {}: serialize→read→serialize not stable",
                    fmt.name
                );
            }
        }
    }
}

#[test]
fn same_format_round_trip_is_byte_exact() {
    for case in CASES {
        let net0 = parse_matpower_file(data(case)).unwrap();
        for fmt in roundtrippable() {
            let text = (fmt.write)(&net0);
            let module = read_module(&text, fmt.token);
            assert_eq!(
                emit_module(&module, fmt.format).unwrap().text,
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
    let pm = emit_powermodels_json(&net0).text;
    let from_pm = parse_powermodels_json(&pm).unwrap();
    let fp = fingerprint(&net0);
    assert_eq!(
        fingerprint(&parse_psse(&emit_psse(&from_pm).text).unwrap()),
        fp
    );
    assert_eq!(
        fingerprint(&parse_powerworld(&emit_powerworld(&from_pm).text).unwrap()),
        fp
    );
}

#[test]
fn egret_fixtures_round_trip_byte_exact() {
    // egret ModelData files (case9/14/30 from egret's own serializer, dcline3
    // hand-authored) read and echo back byte for byte; dcline3 exercises the
    // dc_branch path.
    for f in [
        "egret/case9.json",
        "egret/case14.json",
        "egret/case30.json",
        "egret/dcline3.json",
    ] {
        let text = std::fs::read_to_string(data(f)).unwrap();
        let parsed = parse_str(&text, "egret-json").unwrap();
        assert_eq!(
            parsed.emit(TargetFormat::EgretJson).unwrap().text,
            text,
            "{f}: egret same-format write is not a byte-exact echo"
        );
    }
    // dc_branch maps to an hvdc line on read.
    let hv =
        parse_egret_json(&std::fs::read_to_string(data("egret/dcline3.json")).unwrap()).unwrap();
    assert_eq!(hv.hvdc().len(), 1, "dc_branch should map to one hvdc line");
    assert_eq!(hv.buses().len(), 3);
}
