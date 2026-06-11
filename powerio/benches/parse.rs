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
// fixture for them.
const FORMATS: &[(&str, TargetFormat)] = &[
    ("powermodels-json", TargetFormat::PowerModelsJson),
    ("psse", TargetFormat::Psse),
    ("powerworld", TargetFormat::PowerWorld),
    ("egret-json", TargetFormat::EgretJson),
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

/// PowerWorld aux against pwb on the same case at each scale the fixtures
/// provide: the vendored 200 bus pair, the fetched 2000 bus pair and the
/// RTS-GMLC binary when present (benchmarks/fetch_powerworld.sh; absent
/// fixtures skip silently). `POWERIO_BENCH_AUX`/`POWERIO_BENCH_PWB` add one
/// more file each, for cases that cannot be fetched (the 7k bus TAMU aux);
/// those are explicit requests, so a missing path fails loudly.
fn bench_powerworld_pwb(c: &mut Criterion) {
    let pairs: &[(&str, &str, &str)] = &[
        (
            "activsg200",
            "../tests/data/powerworld/ACTIVSg200.aux",
            "../tests/data/powerworld/ACTIVSg200.pwb",
        ),
        (
            "activsg2000",
            "../tests/data/large/ACTIVSg2000/Texas2000_June2016.AUX",
            "../tests/data/large/ACTIVSg2000/Texas2000_June2016.pwb",
        ),
        ("rts_gmlc", "", "../tests/data/large/RTS-GMLC/RTS-GMLC.PWB"),
    ];
    let mut aux_jobs: Vec<(String, String)> = Vec::new();
    let mut pwb_jobs: Vec<(String, Vec<u8>)> = Vec::new();
    for (label, aux, pwb) in pairs {
        if let Ok(text) = std::fs::read_to_string(aux) {
            aux_jobs.push((format!("parse_aux_{label}"), text));
        }
        if let Ok(bytes) = std::fs::read(pwb) {
            pwb_jobs.push((format!("parse_pwb_{label}"), bytes));
        }
    }
    if let Ok(path) = std::env::var("POWERIO_BENCH_AUX") {
        aux_jobs.push((
            "parse_aux_extra".into(),
            std::fs::read_to_string(path).unwrap(),
        ));
    }
    if let Ok(path) = std::env::var("POWERIO_BENCH_PWB") {
        pwb_jobs.push(("parse_pwb_extra".into(), std::fs::read(path).unwrap()));
    }
    for (name, text) in &aux_jobs {
        c.bench_function(name, |b| {
            b.iter(|| parse_str(black_box(text), "aux").unwrap());
        });
    }
    for (name, bytes) in &pwb_jobs {
        c.bench_function(name, |b| {
            b.iter(|| powerio::format::powerworld::parse_pwb(black_box(bytes), None).unwrap());
        });
    }
}

criterion_group!(
    benches,
    bench_parse,
    bench_roundtrip,
    bench_parse_formats,
    bench_powerworld_pwb
);
criterion_main!(benches);
