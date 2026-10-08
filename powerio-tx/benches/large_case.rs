//! Large-case throughput. Run with `cargo bench --bench large_case`.
//!
//! A synthetic PSS/E revision 35 case of about 100,000 buses is generated in
//! memory, sized like a continental planning model: a meshed 230 kV ring with
//! chords, 25,000 two winding and 4,000 three winding transformers (a tenth
//! of them out of service), 13,000 generators that each state PSS/E fields
//! the neutral model keeps as source metadata, and 60,000 loads. Two
//! benchmarks time the PSS/E reader on it and the indexed analysis view
//! built from the result.
//!
//! Set `POWERIO_PGLIB_CASE78484` to the path of PGLib's
//! `pglib_opf_case78484_epigrids.m` to time the same two steps on that case;
//! it is not vendored. Set `POWERIO_LARGE_CASE_WRITE` to a path to also write
//! the synthetic case there, for profiling outside the harness.

use std::fmt::Write as _;
use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};

const BUSES: usize = 100_000;
const TWO_WINDING: usize = 25_000;
const THREE_WINDING: usize = 4_000;
const GENERATORS: usize = 13_000;
const LOADS: usize = 60_000;

/// The synthetic case as PSS/E revision 35 text. Deterministic: every count
/// and value is a function of the element index.
fn synthetic_v35() -> String {
    let mut s = String::with_capacity(48 << 20);
    s.push_str("0, 100.0, 35, 0, 1, 60.0   / synthetic large case\nLARGE\n\n");
    s.push_str("0 / END OF SYSTEM-WIDE DATA, BEGIN BUS DATA\n");
    for i in 1..=BUSES {
        let kind = if i == 1 {
            3
        } else if i % 8 == 0 {
            2
        } else {
            1
        };
        let va = -((i % 360) as f64) * 0.1;
        let _ = writeln!(
            s,
            "{i}, 'B{i:<10}', 230.0, {kind}, {}, 1, 1, 1.01, {va:.2}, 1.1, 0.9, 1.1, 0.9",
            1 + i % 150
        );
    }
    s.push_str("0 / END OF BUS DATA, BEGIN LOAD DATA\n");
    for k in 0..LOADS {
        let bus = 2 + (k * 7919) % (BUSES - 1);
        let _ = writeln!(
            s,
            "{bus}, '{}', 1, 1, 1, {:.1}, {:.1}, 0.0, 0.0, 0.0, 0.0, 1, 1, 0, 0.0, 0.0, 0, ''",
            1 + k % 3,
            5.0 + (k % 40) as f64,
            1.0 + (k % 9) as f64
        );
    }
    s.push_str("0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA\n");
    s.push_str("0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA\n");
    for k in 0..GENERATORS {
        // A machine id other than the one the writer would allocate and a
        // non-default subtransient reactance: both ride as source metadata.
        let bus = 1 + (k * 13) % BUSES;
        let _ = writeln!(
            s,
            "{bus}, 'G{}', {:.1}, 0.0, 100.0, -100.0, 1.02, 0, 0, 100.0, 0.0, 0.25, 0.0, 0.0, 1.0, 1, 100.0, 300.0, 0.0, 0, 1, 1.0",
            k % 4,
            50.0 + (k % 200) as f64
        );
    }
    s.push_str("0 / END OF GENERATOR DATA, BEGIN BRANCH DATA\n");
    let ratings = ", 0.0".repeat(12);
    let branch = |s: &mut String, from: usize, to: usize, x: f64| {
        let _ = writeln!(
            s,
            "{from}, {to}, '1', 0.002, {x:.4}, 0.01, '            '{ratings}, 0.0, 0.0, 0.0, 0.0, 1, 1, 0.0, 1, 1.0"
        );
    };
    for i in 1..=BUSES {
        branch(&mut s, i, i % BUSES + 1, 0.01 + (i % 17) as f64 * 0.001);
    }
    for i in (1..=BUSES).step_by(7) {
        branch(&mut s, i, (i + 997) % BUSES + 1, 0.05);
    }
    s.push_str("0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA\n");
    push_transformers(&mut s);
    s.push_str("0 / END OF TRANSFORMER DATA, BEGIN AREA DATA\n");
    for section in [
        "AREA DATA, BEGIN TWO-TERMINAL DC",
        "TWO-TERMINAL DC DATA, BEGIN VSC DC LINE",
        "VSC DC LINE DATA, BEGIN IMPEDANCE CORRECTION",
        "IMPEDANCE CORRECTION DATA, BEGIN MULTI-TERMINAL DC",
        "MULTI-TERMINAL DC DATA, BEGIN MULTI-SECTION LINE",
        "MULTI-SECTION LINE DATA, BEGIN ZONE",
        "ZONE DATA, BEGIN INTER-AREA TRANSFER",
        "INTER-AREA TRANSFER DATA, BEGIN OWNER",
        "OWNER DATA, BEGIN FACTS DEVICE",
        "FACTS DEVICE DATA, BEGIN SWITCHED SHUNT",
        "SWITCHED SHUNT DATA, BEGIN GNE DEVICE",
        "GNE DEVICE DATA, BEGIN INDUCTION MACHINE",
    ] {
        let _ = writeln!(s, "0 / END OF {section} DATA");
    }
    s.push_str("0 / END OF INDUCTION MACHINE DATA\nQ\n");
    s
}

/// The transformer section: two winding units between buses fifty apart and
/// three winding units over three consecutive buses, a tenth of each out of
/// service.
fn push_transformers(s: &mut String) {
    let winding = |tap: f64| {
        format!(
            "{tap}, 0.0, 0.0{}, 0, 0, 0, 1.1, 0.9, 1.1, 0.9, 33, 0, 0.0, 0.0, 0.0",
            ", 100.0".repeat(12)
        )
    };
    for k in 0..TWO_WINDING {
        let from = 1 + (k * 4) % BUSES;
        let to = (from + 50) % BUSES + 1;
        let status = u8::from(k % 10 != 0);
        let _ = writeln!(
            s,
            "{from}, {to}, 0, '1', 1, 1, 1, 0.0, 0.0, 2, '            ', {status}, 1, 1.0, 0, 1.0, 0, 1.0, 0, 1.0, '            ', 0"
        );
        let _ = writeln!(s, "0.0005, 0.08, 100.0");
        let _ = writeln!(s, "{}", winding(1.0 + (k % 5) as f64 * 0.0125));
        let _ = writeln!(s, "1.0, 0.0");
    }
    for k in 0..THREE_WINDING {
        let a = 3 + (k * 23) % (BUSES - 4);
        let status = u8::from(k % 10 != 0);
        let _ = writeln!(
            s,
            "{a}, {}, {}, '1', 1, 1, 1, 0.0, 0.0, 2, '            ', {status}, 1, 1.0, 0, 1.0, 0, 1.0, 0, 1.0, '            ', 0",
            a + 1,
            a + 2
        );
        let _ = writeln!(
            s,
            "0.001, 0.06, 100.0, 0.001, 0.08, 100.0, 0.001, 0.07, 100.0, 1.0, -1.5"
        );
        for _ in 0..3 {
            let _ = writeln!(s, "{}", winding(1.0));
        }
    }
}

fn parse(name: &str, text: &str, format: &str) -> powerio_tx::BalancedNetwork {
    let source = powerio_core::Source::from_memory(name, text.as_bytes().to_vec())
        .unwrap()
        .with_format(powerio_core::FormatId::new(format).unwrap());
    powerio_tx::parse(source).unwrap().into_value()
}

fn bench_large(c: &mut Criterion) {
    let text = synthetic_v35();
    if let Ok(path) = std::env::var("POWERIO_LARGE_CASE_WRITE") {
        std::fs::write(path, &text).unwrap();
    }
    let net = parse("large.raw", &text, "psse");
    assert_eq!(net.buses().len(), BUSES);
    assert_eq!(net.generators().len(), GENERATORS);

    let mut group = c.benchmark_group("large_case");
    group.sample_size(10);
    group.bench_function("parse_psse_v35_100k", |b| {
        b.iter(|| parse("large.raw", black_box(&text), "psse"));
    });
    group.bench_function("index_psse_v35_100k", |b| {
        b.iter(|| powerio_tx::IndexedNetwork::new(black_box(&net)).n());
    });
    if let Ok(path) = std::env::var("POWERIO_PGLIB_CASE78484") {
        let pglib = std::fs::read_to_string(&path).unwrap();
        let net = parse("case78484.m", &pglib, "matpower");
        group.bench_function("parse_pglib_case78484", |b| {
            b.iter(|| parse("case78484.m", black_box(&pglib), "matpower"));
        });
        group.bench_function("index_pglib_case78484", |b| {
            b.iter(|| powerio_tx::IndexedNetwork::new(black_box(&net)).n());
        });
    }
    group.finish();
}

criterion_group!(benches, bench_large);
criterion_main!(benches);
