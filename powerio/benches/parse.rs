//! MATPOWER parse + round-trip throughput. Run with `cargo bench --bench parse`.
//!
//! Parse time is dominated by the field-finding scan over the source text;
//! `write` echoes the retained source. The large pegase case is the headline
//! number for the "fast parser" claim — see `benchmarks/RESULTS.md` for the
//! cross-tool comparison against PowerModels.jl and ExaPowerIO.jl.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use powerio::{parse_matpower, write_matpower};

const CASES: &[&str] = &["case57", "case118", "case2869pegase"];

fn bench_parse(c: &mut Criterion) {
    for case in CASES {
        let src = std::fs::read_to_string(format!("../tests/data/{case}.m")).unwrap();
        c.bench_function(&format!("parse_{case}"), |b| {
            b.iter(|| parse_matpower(black_box(&src)).unwrap());
        });
    }
}

fn bench_roundtrip(c: &mut Criterion) {
    for case in CASES {
        let src = std::fs::read_to_string(format!("../tests/data/{case}.m")).unwrap();
        let parsed = parse_matpower(&src).unwrap();
        c.bench_function(&format!("write_{case}"), |b| {
            b.iter(|| write_matpower(black_box(&parsed)));
        });
        c.bench_function(&format!("roundtrip_{case}"), |b| {
            b.iter(|| write_matpower(&parse_matpower(black_box(&src)).unwrap()));
        });
    }
}

criterion_group!(benches, bench_parse, bench_roundtrip);
criterion_main!(benches);
