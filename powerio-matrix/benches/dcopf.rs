//! MATPOWER parse and DC OPF preparation throughput. Run with
//! `cargo bench -p powerio-matrix --bench dcopf`.
//!
//! The two halves of what a consumer pays before a solver sees anything: the
//! reader that turns source text into a [`BalancedNetwork`], and the assembly
//! that turns an instance into the contiguous arrays a solver formulates over.
//! `matrix.rs` times the sparse builders on top of an already prepared case;
//! this file times the path to it.
//!
//! The vendored `case2869pegase` always runs. Point `POWERIO_BENCH_PGLIB` at a
//! pglib-opf checkout to add `pglib_opf_case13659_pegase.m`, the large case
//! the preparation is tuned against.

use std::hint::black_box;
use std::path::PathBuf;

use criterion::{Criterion, criterion_group, criterion_main};
use powerio_matrix::{BalancedNetwork, DcOpfAssemblyOptions, build_dc_opf_preparation};
use powerio_prob::DcOpfInstance;

fn parse_matpower(text: &str) -> BalancedNetwork {
    let source = powerio_core::Source::from_memory("case.m", text.as_bytes().to_vec())
        .expect("memory source")
        .with_format(powerio_core::FormatId::new("matpower").expect("matpower"));
    powerio_tx::parse(source)
        .map(powerio_core::PioModule::into_value)
        .expect("parse")
}

/// `(name, source text)` for each case this run can reach.
fn cases() -> Vec<(String, String)> {
    let vendored = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/data/case2869pegase.m");
    let mut cases = vec![(
        "case2869pegase".to_owned(),
        std::fs::read_to_string(&vendored)
            .unwrap_or_else(|error| panic!("read {}: {error}", vendored.display())),
    )];
    if let Some(root) = std::env::var_os("POWERIO_BENCH_PGLIB") {
        let large = PathBuf::from(root).join("pglib_opf_case13659_pegase.m");
        match std::fs::read_to_string(&large) {
            Ok(text) => cases.push(("case13659pegase".to_owned(), text)),
            Err(error) => eprintln!("skipping {}: {error}", large.display()),
        }
    }
    cases
}

fn bench_parse_and_prepare(c: &mut Criterion) {
    let options = DcOpfAssemblyOptions::default();
    for (name, text) in cases() {
        c.bench_function(&format!("dcopf_parse_{name}"), |b| {
            b.iter(|| parse_matpower(black_box(&text)));
        });

        let network = parse_matpower(&text);
        c.bench_function(&format!("dcopf_instance_{name}"), |b| {
            b.iter(|| DcOpfInstance::from_network(black_box(network.clone())).expect("instance"));
        });

        let instance = DcOpfInstance::from_network(network).expect("instance");
        c.bench_function(&format!("dcopf_preparation_{name}"), |b| {
            b.iter(|| {
                build_dc_opf_preparation(black_box(&instance), black_box(&options))
                    .expect("preparation")
            });
        });
    }
}

criterion_group!(benches, bench_parse_and_prepare);
criterion_main!(benches);
