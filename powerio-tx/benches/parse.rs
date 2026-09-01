//! Parse + round-trip throughput. Run with `cargo bench --bench parse`.
//!
//! Three groups, all in-process micro-benchmarks over the vendored fixtures:
//! - `parse_*` / `emit_*` / `roundtrip_*`: the MATPOWER hot path. Parse time
//!   is dominated by the field-finding scan over the source text; `emit`
//!   echoes the retained source. The large pegase case is the headline number
//!   for the "fastest parser" claim.
//! - `parse_<format>_*`: the non-MATPOWER readers (PowerModels JSON, PSS/E,
//!   PowerWorld). One case is converted to each format once, then timed on the
//!   way back in. This is regression coverage for the readers the owned-source
//!   refactor touched.
//! - `parse_sniffed_*`: the same PowerModels JSON text with no declared
//!   format, routed by [`powerio_tx::format::routing::classify_json_text`]
//!   instead of a stated token. Regression coverage for #440's double
//!   classification.
//!
//! This is the micro-benchmark half. The cross-tool comparison against
//! PowerModels.jl, ExaPowerIO.jl, and pandapower is a separate set of scripts
//! under `benchmarks/` (see `benchmarks/RESULTS.md`); the two don't overlap.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use powerio_tx::TargetFormat;

const CASES: &[&str] = &["case57", "case118", "case2869pegase"];

fn src(case: &str) -> String {
    std::fs::read_to_string(format!("../tests/data/{case}.m")).unwrap()
}

fn parse_named(text: &str, from: &str) -> powerio_tx::network::BalancedNetwork {
    let source = powerio_core::Source::from_memory("case", text.as_bytes().to_vec())
        .unwrap()
        .with_format(powerio_core::FormatId::new(from).unwrap());
    powerio_tx::parse(source).unwrap().into_value()
}

/// Parse with no declared format: a `.json` name, routed by the content
/// classifier rather than a stated token.
fn parse_sniffed(text: &str) -> powerio_tx::network::BalancedNetwork {
    let source = powerio_core::Source::from_memory("case.json", text.as_bytes().to_vec()).unwrap();
    powerio_tx::parse(source).unwrap().into_value()
}

fn parse_case(text: &str) -> powerio_tx::network::BalancedNetwork {
    parse_named(text, "matpower")
}

fn emit_text(network: &powerio_tx::network::BalancedNetwork, target: TargetFormat) -> String {
    let module = powerio_core::PioModule::new(network.clone());
    let result = powerio_tx::emit(
        &module,
        target,
        powerio_core::Destination::memory("case").unwrap(),
    )
    .unwrap();
    let powerio_core::EmittedOutput::Memory { mut artifacts } = result.into_output() else {
        unreachable!("memory destination returns memory output");
    };
    String::from_utf8(
        artifacts
            .pop()
            .expect("text emission has one artifact")
            .into_bytes(),
    )
    .expect("case text is UTF-8")
}

fn bench_parse(c: &mut Criterion) {
    for case in CASES {
        let s = src(case);
        c.bench_function(&format!("parse_{case}"), |b| {
            b.iter(|| parse_case(black_box(&s)));
        });
    }
}

fn bench_roundtrip(c: &mut Criterion) {
    for case in CASES {
        let s = src(case);
        let parsed = parse_case(&s);
        c.bench_function(&format!("emit_{case}"), |b| {
            b.iter(|| emit_text(black_box(&parsed), TargetFormat::Matpower));
        });
        c.bench_function(&format!("roundtrip_{case}"), |b| {
            b.iter(|| emit_text(&parse_case(black_box(&s)), TargetFormat::Matpower));
        });
    }
}

// The readable non-MATPOWER formats, paired with the writer that produces a
// fixture for them.
const FORMATS: &[(&str, TargetFormat)] = &[
    ("powermodels-json", TargetFormat::PowerModelsJson),
    ("psse", TargetFormat::Psse { rev: 33 }),
    ("powerworld", TargetFormat::PowerWorld),
    ("egret-json", TargetFormat::EgretJson),
];

fn bench_parse_formats(c: &mut Criterion) {
    let case = "case118";
    let net = parse_case(&src(case));
    for (name, fmt) in FORMATS {
        // Convert once outside the timed loop; `parse_str` runs the same
        // owned-source reader the file path does.
        let text = emit_text(&net, *fmt);
        // A reader that can't re-read its own writer would make the timing
        // meaningless, so fail loudly here rather than benchmark an error path.
        let _ = parse_named(&text, name);
        c.bench_function(&format!("parse_{name}_{case}"), |b| {
            b.iter(|| parse_named(black_box(&text), name));
        });
    }
}

/// The sniffed JSON path (no declared format, routed by content
/// classification): regression coverage for #440, which found the
/// classifier materializing the whole document twice over before the
/// reader's own parse ever ran.
fn bench_parse_sniffed_json(c: &mut Criterion) {
    let case = "case118";
    let net = parse_case(&src(case));
    let text = emit_text(&net, TargetFormat::PowerModelsJson);
    let _ = parse_sniffed(&text);
    c.bench_function(&format!("parse_sniffed_powermodels-json_{case}"), |b| {
        b.iter(|| parse_sniffed(black_box(&text)));
    });
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
            b.iter(|| parse_named(black_box(text), "aux"));
        });
    }
    for (name, bytes) in &pwb_jobs {
        c.bench_function(name, |b| {
            b.iter(|| powerio_tx::format::powerworld::__parse_pwb(black_box(bytes), None).unwrap());
        });
    }
}

/// The `.pwd` display decoder: a byte-offset scan over the whole file, the one
/// reader whose hot loop runs per byte rather than per record: regression
/// coverage for the total (Option-returning) byte accessors.
fn bench_powerworld_pwd(c: &mut Criterion) {
    let Ok(bytes) = std::fs::read("../tests/data/powerworld/ACTIVSg200.pwd") else {
        return;
    };
    c.bench_function("parse_pwd_activsg200", |b| {
        b.iter(|| powerio_tx::format::powerworld::__parse_pwd(black_box(&bytes)).unwrap());
    });
}

criterion_group!(
    benches,
    bench_parse,
    bench_roundtrip,
    bench_parse_formats,
    bench_parse_sniffed_json,
    bench_powerworld_pwb,
    bench_powerworld_pwd
);
criterion_main!(benches);
