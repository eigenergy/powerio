use powerio_core::Source;

#[test]
fn direct_spacing_without_any_geometry_objects_reaches_typed_lines() {
    let source = Source::from_memory("direct.dss", "new circuit.c\nnew wiredata.w rac=.0002 radius=.01 radunits=m runits=m\nnew linespacing.s nconds=1 nphases=1 units=m x=[0] h=[10]\nnew line.a bus1=sourcebus.1 bus2=end.1 spacing=s wires=[w] length=100 units=m\n".as_bytes()).unwrap();
    let module = powerio_dist::parse(source).unwrap();
    assert!(module.value().lines().iter().any(|line| line.name == "a"));
    assert!(powerio_dist::audit_electrical_readiness(module.value()).is_ready());
}

#[test]
fn missing_spacing_and_conductor_bindings_remain_blocked_after_late_definitions() {
    for text in [
        "new circuit.c\nnew line.a bus1=sourcebus.1 bus2=end.1 spacing=later\nnew linespacing.later nconds=1 nphases=1 x=[0] h=[10]\n",
        "new circuit.c\nnew linespacing.s nconds=1 nphases=1 x=[0] h=[10]\nnew line.a bus1=sourcebus.1 bus2=end.1 spacing=s wires=[later]\nnew wiredata.later rac=.0002 radius=.01\n",
    ] {
        let module =
            powerio_dist::parse(Source::from_memory("missing.dss", text.as_bytes()).unwrap())
                .unwrap();
        assert!(!powerio_dist::audit_electrical_readiness(module.value()).is_ready());
        assert!(module.value().lines().iter().all(|line| line.name != "a"));
    }
}
