//! Matrix builder throughput. Run with `cargo bench -p powerio-matrix --bench matrix`.
//!
//! These benches time derived matrix construction from an already parsed and
//! indexed network. Parser throughput lives in `powerio/benches/parse.rs`; this
//! file answers whether the sparse builders themselves changed.

use criterion::{Criterion, criterion_group, criterion_main};
use powerio_matrix::matrix::{
    BuildOptions, DcConvention, Units, build_adjacency, build_bdoubleprime, build_bprime,
    build_flow_map, build_incidence, build_lacpf, build_opf_instance, build_ptdf_lodf,
    build_weighted_laplacian, build_ybus, ground_at_each,
};
use powerio_matrix::pipeline::{MatrixKind, Pipeline, RhsKind};
use powerio_matrix::{IndexedNetwork, parse_matpower};
use std::hint::black_box;

fn fixture(name: &str) -> &'static str {
    match name {
        "case118" => include_str!("../../tests/data/case118.m"),
        "case2869pegase" => include_str!("../../tests/data/case2869pegase.m"),
        _ => unreachable!("unknown fixture"),
    }
}

fn network(name: &str) -> powerio_matrix::Network {
    parse_matpower(fixture(name)).unwrap_or_else(|e| panic!("parse {name}: {e}"))
}

fn bench_matrix_builders(c: &mut Criterion) {
    for case in ["case118", "case2869pegase"] {
        let net = network(case);
        let view = IndexedNetwork::new(&net);
        let opts = BuildOptions::default();

        c.bench_function(&format!("matrix_bprime_{case}"), |b| {
            b.iter(|| build_bprime(black_box(&view), black_box(&opts)).unwrap());
        });
        c.bench_function(&format!("matrix_bdoubleprime_{case}"), |b| {
            b.iter(|| build_bdoubleprime(black_box(&view), black_box(&opts)).unwrap());
        });
        c.bench_function(&format!("matrix_ybus_{case}"), |b| {
            b.iter(|| build_ybus(black_box(&view), black_box(&opts)).unwrap());
        });
        c.bench_function(&format!("matrix_lacpf_{case}"), |b| {
            b.iter(|| build_lacpf(black_box(&view), black_box(&opts)).unwrap());
        });
        c.bench_function(&format!("matrix_adjacency_{case}"), |b| {
            b.iter(|| build_adjacency(black_box(&view)).unwrap());
        });
    }
}

fn bench_dcopf_parts(c: &mut Criterion) {
    let net = network("case118");
    let view = IndexedNetwork::new(&net);

    c.bench_function("dcopf_incidence_case118", |b| {
        b.iter(|| {
            build_incidence(
                black_box(&view),
                black_box(DcConvention::PaperPure),
                black_box(&BuildOptions::default()),
            )
            .unwrap()
        });
    });

    let incidence =
        build_incidence(&view, DcConvention::PaperPure, &BuildOptions::default()).unwrap();
    c.bench_function("dcopf_laplacian_case118", |b| {
        b.iter(|| build_weighted_laplacian(black_box(&incidence.a), black_box(&incidence.b)));
    });
    let refs = view.reference_bus_indices();
    c.bench_function("dcopf_grounded_laplacian_case118", |b| {
        b.iter(|| {
            let l = build_weighted_laplacian(&incidence.a, &incidence.b);
            ground_at_each(black_box(&l), black_box(&refs))
        });
    });
    c.bench_function("dcopf_flow_map_case118", |b| {
        b.iter(|| build_flow_map(black_box(&incidence.a), black_box(&incidence.b)));
    });
    c.bench_function("dcopf_instance_case118", |b| {
        b.iter(|| {
            build_opf_instance(
                black_box(&view),
                black_box(&incidence),
                black_box(Units::PerUnit),
            )
            .unwrap()
        });
    });
}

fn bench_dense_sensitivities(c: &mut Criterion) {
    let net = network("case118");
    let view = IndexedNetwork::new(&net);
    c.bench_function("sensitivity_ptdf_lodf_case118", |b| {
        b.iter(|| build_ptdf_lodf(black_box(&view), black_box(DcConvention::PaperPure)).unwrap());
    });
}

fn bench_pipeline_paths(c: &mut Criterion) {
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
            let outputs = pipeline
                .run(black_box(&net), black_box(out.path()))
                .unwrap();
            black_box(outputs.files.len())
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
