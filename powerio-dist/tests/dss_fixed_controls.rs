use powerio_core::Source;

#[test]
fn a_retained_control_has_a_distinct_diagnostic_and_keeps_written_taps() {
    let source = "new circuit.test basekv=11\nnew transformer.t phases=3 windings=2 buses=[sourcebus load] conns=[wye wye] kvs=[11 .4] kvas=[100 100] taps=[1 1.025] xhl=4\nnew regcontrol.r transformer=t winding=2 vreg=120 band=2 ptratio=1\n";
    let module =
        powerio_dist::parse(Source::from_memory("test.dss", source.as_bytes()).unwrap()).unwrap();
    assert!(
        module
            .diagnostics()
            .iter()
            .any(|d| d.code() == "READ.DSS.CONTROL_NOT_EXECUTED")
    );
    assert!(
        !module
            .diagnostics()
            .iter()
            .any(|d| d.code() == "READ.DSS.VALUE_UNSUPPORTED")
    );
    assert_eq!(module.value().untyped_objects()[0].class, "regcontrol");
    assert!((module.value().transformers()[0].windings[1].tap - 1.025).abs() < 1e-12);
    let broken = source.replace(
        "regcontrol.r transformer=t",
        "regcontrol.r transformer=missing",
    );
    let module =
        powerio_dist::parse(Source::from_memory("test.dss", broken.as_bytes()).unwrap()).unwrap();
    assert!(
        module
            .diagnostics()
            .iter()
            .any(|d| d.code() == "READ.DSS.REFERENCE_DROPPED")
    );
    assert!(
        !module
            .diagnostics()
            .iter()
            .any(|d| d.code() == "READ.DSS.CONTROL_NOT_EXECUTED")
    );
}
