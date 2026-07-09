//! Index based DC-OPF instance built from vendored MATPOWER cases. The matrix
//! form of the instance and the export bundle live in
//! `powerio-matrix/tests/dcopf.rs`; these exercise the index based
//! `DcOpfInstance` and `build_dc_opf_instance`.

use powerio::{IndexedNetwork, Network, parse_matpower_file};
use powerio_opf::{DcOpfInstance, Units, build_dc_opf_instance, project_gen_to_bus};

fn case9() -> Network {
    parse_matpower_file("../tests/data/case9.m").expect("parse case9")
}

#[test]
fn shapes_and_maps() {
    let net = case9();
    let view = IndexedNetwork::new(&net);
    let opf = build_dc_opf_instance(&view, Units::PerUnit).expect("build");

    assert_eq!(opf.n, 9);
    assert_eq!(opf.n_gen(), 3);
    assert_eq!(opf.gen_costs.gen_of_col.len(), opf.n_gen());
    assert_eq!(opf.bus_of_col.len(), opf.n_gen());
    assert_eq!(opf.m, opf.f_max.len());
    assert_eq!(opf.m, opf.branch_of_col.len());
    for v in [
        &opf.bus.q,
        &opf.bus.c,
        &opf.bus.pmax,
        &opf.bus.pmin,
        &opf.bus.p_d,
    ] {
        assert_eq!(v.len(), opf.n);
    }
    for &bus in &opf.bus_of_col {
        assert!(bus < opf.n, "generator maps to a bus in range");
    }
    for &k in &opf.branch_of_col {
        assert!(k < view.branches().len(), "branch index in range");
    }
}

#[test]
fn bus_vectors_are_the_scatter_of_gen_vectors() {
    let net = case9();
    let view = IndexedNetwork::new(&net);
    let opf = build_dc_opf_instance(&view, Units::Native).expect("build");
    assert_eq!(
        project_gen_to_bus(&opf.bus_of_col, &opf.gen_costs.pmax, opf.n),
        opf.bus.pmax
    );
    assert_eq!(
        project_gen_to_bus(&opf.bus_of_col, &opf.gen_costs.q, opf.n),
        opf.bus.q
    );
}

#[test]
fn per_unit_scales_native_by_base() {
    let net = case9();
    let view = IndexedNetwork::new(&net);
    let base = view.per_unit_base();
    let native = build_dc_opf_instance(&view, Units::Native).expect("native");
    let pu = build_dc_opf_instance(&view, Units::PerUnit).expect("pu");
    for (nat, per) in native.gen_costs.pmax.iter().zip(&pu.gen_costs.pmax) {
        assert!((*per - *nat / base).abs() < 1e-9, "pmax scales by 1/base");
    }
    for (nat, per) in native.gen_costs.c.iter().zip(&pu.gen_costs.c) {
        assert!(
            (*per - *nat * base).abs() < 1e-9,
            "linear cost scales by base"
        );
    }
}

#[test]
fn serde_round_trip() {
    let net = case9();
    let view = IndexedNetwork::new(&net);
    let opf = build_dc_opf_instance(&view, Units::PerUnit).expect("build");
    let json = serde_json::to_string(&opf).expect("serialize");
    let back: DcOpfInstance = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(opf, back);
}
