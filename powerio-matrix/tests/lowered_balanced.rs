use powerio_matrix::{BuildOptions, IndexedNetwork, build_bprime, build_ybus};
use powerio_pkg::{MulticonductorToBalancedOptions, lower_multiconductor_to_balanced};

#[test]
fn lowered_multiconductor_balanced_model_builds_matrices() {
    let text = include_str!("../../tests/data/dist/micro/fourwire_linecode.dss");
    let net = powerio_dist::parse_str(text, "dss").expect("parse distribution fixture");
    let lowered =
        lower_multiconductor_to_balanced(&net, MulticonductorToBalancedOptions::default())
            .expect("lower to balanced");

    let view = IndexedNetwork::new(&lowered.network);
    let bprime = build_bprime(&view, &BuildOptions::default()).expect("build B prime");
    let ybus = build_ybus(&view, &BuildOptions::default()).expect("build Y bus");

    assert_eq!(bprime.rows(), view.n());
    assert_eq!(bprime.cols(), view.n());
    assert_eq!(ybus.g.rows(), view.n());
    assert_eq!(ybus.b.cols(), view.n());
    assert!(bprime.nnz() > 0);
}
