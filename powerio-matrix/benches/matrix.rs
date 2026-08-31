//! Matrix builder throughput. Run with `cargo bench -p powerio-matrix --bench matrix`.
//!
//! These benches time derived matrix construction from an already parsed and
//! indexed network. Parser throughput lives in `powerio-tx/benches/parse.rs`; this
//! file answers whether the sparse builders themselves changed.
//!
//! The benches hold `b = 1/x` so a timing series stays comparable across the
//! 0.9.0 branch susceptance formula change.

use criterion::{Criterion, criterion_group, criterion_main};
use powerio_matrix::matrix::{
    BranchSusceptanceFormula, BuildOptions, calc_adjacency_matrix, calc_admittance_matrix,
    calc_bdoubleprime_matrix, calc_bprime_matrix, calc_branch_flow_matrix, calc_lacpf_matrix,
    calc_ptdf_lodf, calc_weighted_laplacian, ground_at_each,
};
use powerio_matrix::pipeline::{MatrixKind, Pipeline, RhsKind};
use powerio_matrix::{BalancedNetwork, DcOperators, IndexedNetwork};
use powerio_prob::DcPfInstance;

fn parse_matpower(text: &str) -> Result<BalancedNetwork, powerio_core::Error> {
    let source = powerio_core::Source::from_bytes("case.m", text.as_bytes().to_vec())?
        .with_format(powerio_core::FormatId::new("matpower")?);
    powerio_tx::parse(source).map(powerio_core::PioModule::into_value)
}
use std::hint::black_box;
use std::path::Path;

fn fixture(name: &str) -> String {
    assert!(matches!(name, "case118" | "case2869pegase"));
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data")
        .join(format!("{name}.m"));
    std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "read checkout-only benchmark fixture {}: {error}",
            path.display()
        )
    })
}

fn network(name: &str) -> powerio_matrix::BalancedNetwork {
    parse_matpower(&fixture(name)).unwrap_or_else(|e| panic!("parse {name}: {e}"))
}

fn bench_matrix_builders(c: &mut Criterion) {
    for case in ["case118", "case2869pegase"] {
        let net = network(case);
        let view = IndexedNetwork::new(&net);
        let opts = BuildOptions::default();

        c.bench_function(&format!("matrix_bprime_{case}"), |b| {
            b.iter(|| calc_bprime_matrix(black_box(&view), black_box(&opts)).unwrap());
        });
        c.bench_function(&format!("matrix_bdoubleprime_{case}"), |b| {
            b.iter(|| calc_bdoubleprime_matrix(black_box(&view), black_box(&opts)).unwrap());
        });
        c.bench_function(&format!("matrix_ybus_{case}"), |b| {
            b.iter(|| calc_admittance_matrix(black_box(&view), black_box(&opts)).unwrap());
        });
        c.bench_function(&format!("matrix_lacpf_{case}"), |b| {
            b.iter(|| calc_lacpf_matrix(black_box(&view), black_box(&opts)).unwrap());
        });
        c.bench_function(&format!("matrix_adjacency_{case}"), |b| {
            b.iter(|| calc_adjacency_matrix(black_box(&view)).unwrap());
        });
    }
}

fn bench_dcopf_parts(c: &mut Criterion) {
    let net = network("case118");
    let view = IndexedNetwork::new(&net);
    let instance = DcPfInstance::from_network(net.clone())
        .unwrap()
        .with_branch_susceptance_formula(BranchSusceptanceFormula::ReactanceOnly);
    let operators = DcOperators::build(&instance).unwrap();

    c.bench_function("dcopf_incidence_case118", |b| {
        b.iter(|| black_box(&operators).calc_incidence_matrix());
    });

    let incidence = operators.calc_incidence_matrix().transpose_view().to_csr();
    let weights = operators
        .branch_susceptances()
        .iter()
        .map(|value| -*value)
        .collect::<Vec<_>>();
    c.bench_function("dcopf_laplacian_case118", |b| {
        b.iter(|| calc_weighted_laplacian(black_box(&incidence), black_box(&weights)));
    });
    let refs = view.reference_bus_indices();
    c.bench_function("dcopf_grounded_laplacian_case118", |b| {
        b.iter(|| {
            let l = calc_weighted_laplacian(&incidence, &weights);
            ground_at_each(black_box(&l), black_box(&refs))
        });
    });
    c.bench_function("dcopf_branch_flow_matrix_case118", |b| {
        b.iter(|| calc_branch_flow_matrix(black_box(&incidence), black_box(&weights)));
    });
}

fn bench_dense_sensitivities(c: &mut Criterion) {
    let net = network("case118");
    let view = IndexedNetwork::new(&net);
    c.bench_function("sensitivity_ptdf_lodf_case118", |b| {
        b.iter(|| {
            calc_ptdf_lodf(
                black_box(&view),
                black_box(BranchSusceptanceFormula::ReactanceOnly),
            )
            .unwrap()
        });
    });
}

fn bench_pipeline_paths(c: &mut Criterion) {
    // The writer reserves its destination exclusively, and criterion invokes
    // the routine repeatedly across warmup and measurement, so every run
    // writes into a fresh, process-uniquely named subdirectory.
    static PIPELINE_RUN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let net = network("case2869pegase");
    let out = tempfile::tempdir().expect("create benchmark output directory");
    let pipeline = Pipeline {
        matrices: vec![MatrixKind::YbusG, MatrixKind::YbusB],
        options: BuildOptions::default(),
        rhs: RhsKind::None,
        rng_seed: 0,
        source_file: None,
    };

    c.bench_function("pipeline_ybus_pair_case2869pegase", |b| {
        b.iter(|| {
            let n = PIPELINE_RUN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dir = out.path().join(n.to_string());
            std::fs::create_dir(&dir).unwrap();
            let outputs = pipeline.run(black_box(&net), black_box(&dir)).unwrap();
            black_box(outputs.files.len());
        });
    });
}

criterion_group!(
    benches,
    bench_matrix_builders,
    bench_dcopf_parts,
    bench_dense_sensitivities,
    bench_pipeline_paths
);
criterion_main!(benches);
