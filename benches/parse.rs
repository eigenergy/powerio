//! MATPOWER parse throughput. Run with `cargo bench --bench parse`.
//!
//! The interesting number is wall time per parse on the larger vendored cases,
//! which is dominated by the field-finding scan over the source text.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use netmat::parse_matpower;

fn bench_parse(c: &mut Criterion) {
    for case in ["case57", "case118"] {
        let src = std::fs::read_to_string(format!("tests/data/{case}.m")).unwrap();
        c.bench_function(&format!("parse_{case}"), |b| {
            b.iter(|| parse_matpower(black_box(&src)).unwrap());
        });
    }
}

criterion_group!(benches, bench_parse);
criterion_main!(benches);
