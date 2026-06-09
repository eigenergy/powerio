//! Parse + round-trip throughput. Run with `cargo bench --bench parse`.
//!
//! Three groups, all in-process micro-benchmarks over the vendored fixtures:
//! - `parse_*` / `write_*` / `roundtrip_*`: the MATPOWER hot path. Parse time
//!   is dominated by the field-finding scan over the source text; `write`
//!   echoes the retained source. The large pegase case is the headline number
//!   for the "fastest parser" claim.
//! - `parse_<format>_*`: the non-MATPOWER readers (PowerModels JSON, PSS/E,
//!   PowerWorld). One case is converted to each format once, then timed on the
//!   way back in. This is regression coverage for the readers the owned-source
//!   refactor touched.
//!
//! This is the micro-benchmark half. The cross-tool comparison against
//! PowerModels.jl, ExaPowerIO.jl, and pandapower is a separate set of scripts
//! under `benchmarks/` (see `benchmarks/RESULTS.md`); the two don't overlap.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use powerio::{TargetFormat, parse_matpower, parse_str, write_as, write_matpower};

const CASES: &[&str] = &["case57", "case118", "case2869pegase"];

fn src(case: &str) -> String {
    std::fs::read_to_string(format!("../tests/data/{case}.m")).unwrap()
}

fn bench_parse(c: &mut Criterion) {
    for case in CASES {
        let s = src(case);
        c.bench_function(&format!("parse_{case}"), |b| {
            b.iter(|| parse_matpower(black_box(&s)).unwrap());
        });
    }
}

fn bench_roundtrip(c: &mut Criterion) {
    for case in CASES {
        let s = src(case);
        let parsed = parse_matpower(&s).unwrap();
        c.bench_function(&format!("write_{case}"), |b| {
            b.iter(|| write_matpower(black_box(&parsed)));
        });
        c.bench_function(&format!("roundtrip_{case}"), |b| {
            b.iter(|| write_matpower(&parse_matpower(black_box(&s)).unwrap()));
        });
    }
}

// The readable non-MATPOWER formats, paired with the writer that produces a
// fixture for them. egret JSON is write-only, so it isn't here.
const FORMATS: &[(&str, TargetFormat)] = &[
    ("powermodels-json", TargetFormat::PowerModelsJson),
    ("psse", TargetFormat::Psse),
    ("powerworld", TargetFormat::PowerWorld),
];

fn bench_parse_formats(c: &mut Criterion) {
    let case = "case118";
    let net = parse_matpower(&src(case)).unwrap();
    for (name, fmt) in FORMATS {
        // Convert once outside the timed loop; `parse_str` runs the same
        // owned-source reader the file path does.
        let text = write_as(&net, *fmt).text;
        // A reader that can't re-read its own writer would make the timing
        // meaningless, so fail loudly here rather than benchmark an error path.
        parse_str(&text, name)
            .unwrap_or_else(|e| panic!("{name} writer output did not reparse: {e}"));
        c.bench_function(&format!("parse_{name}_{case}"), |b| {
            b.iter(|| parse_str(black_box(&text), name).unwrap());
        });
    }
}

criterion_group!(benches, bench_parse, bench_roundtrip, bench_parse_formats);
criterion_main!(benches);
