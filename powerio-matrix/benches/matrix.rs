//! Matrix builder throughput. Run with `cargo bench -p powerio-matrix --bench matrix`.
//!
//! These benches time derived matrix construction from an already parsed and
//! indexed network. Parser throughput lives in `powerio/benches/parse.rs`; this
//! file answers whether the sparse builders themselves changed.
//!
//! The benches hold `b = 1/x` so a timing series stays comparable across the
//! 0.9.0 convention change.

use criterion::{Criterion, criterion_group, criterion_main};
use powerio_matrix::matrix::{
    BuildOptions, DcConvention, SensitivityOptions, SensitivitySolver, build_adjacency,
    build_bdoubleprime, build_bprime, build_flow_map, build_incidence, build_lacpf,
    build_ptdf_lodf, build_ptdf_lodf_with_options, build_weighted_laplacian, build_ybus,
    ground_at_each,
};
use powerio_matrix::pipeline::{MatrixKind, Pipeline, RhsKind};
use powerio_matrix::{IndexedNetwork, parse_matpower};
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
                black_box(DcConvention::ReactanceOnly),
                black_box(&BuildOptions::default()),
            )
            .unwrap()
        });
    });

    let incidence =
        build_incidence(&view, DcConvention::ReactanceOnly, &BuildOptions::default()).unwrap();
    c.bench_function("dcopf_laplacian_case118", |b| {
        b.iter(|| {
            build_weighted_laplacian(black_box(&incidence.a), black_box(&incidence.branch_weight))
        });
    });
    let refs = view.reference_bus_indices();
    c.bench_function("dcopf_grounded_laplacian_case118", |b| {
        b.iter(|| {
            let l = build_weighted_laplacian(&incidence.a, &incidence.branch_weight);
            ground_at_each(black_box(&l), black_box(&refs))
        });
    });
    c.bench_function("dcopf_flow_map_case118", |b| {
        b.iter(|| build_flow_map(black_box(&incidence.a), black_box(&incidence.branch_weight)));
    });
}

fn bench_sensitivities(c: &mut Criterion) {
    let net = network("case118");
    let view = IndexedNetwork::new(&net);
    c.bench_function("sensitivity_ptdf_lodf_case118", |b| {
        b.iter(|| {
            build_ptdf_lodf(black_box(&view), black_box(DcConvention::ReactanceOnly)).unwrap()
        });
    });
    let sparse = SensitivityOptions {
        convention: DcConvention::ReactanceOnly,
        solver: SensitivitySolver::Sparse,
        ..Default::default()
    };
    c.bench_function("sensitivity_ptdf_lodf_sparse_case118", |b| {
        b.iter(|| build_ptdf_lodf_with_options(black_box(&view), black_box(&sparse)).unwrap());
    });

    let net = network("case2869pegase");
    let view = IndexedNetwork::new(&net);
    let sparse_solve = SensitivityOptions {
        convention: DcConvention::ReactanceOnly,
        solver: SensitivitySolver::Sparse,
        // A normal PTDF/LODF is dense-like at this size. Dropping all
        // non-structural entries keeps this benchmark focused on factorization,
        // batched solves, and flow evaluation instead of output allocation.
        drop_tolerance: f64::MAX,
        ..Default::default()
    };
    c.bench_function("sensitivity_solve_sparse_case2869pegase", |b| {
        b.iter(|| {
            build_ptdf_lodf_with_options(black_box(&view), black_box(&sparse_solve)).unwrap()
        });
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
    bench_sensitivities,
    bench_pipeline_paths
);
criterion_main!(benches);
