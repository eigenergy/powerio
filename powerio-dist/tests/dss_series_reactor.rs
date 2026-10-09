use powerio_core::Source;

fn parse(text: &str) -> powerio_dist::MulticonductorNetwork {
    powerio_dist::parse(Source::from_memory("reactor.dss", text.as_bytes()).unwrap())
        .unwrap()
        .into_value()
}

#[test]
fn scalar_series_reactor_connects_original_ports_without_name_collision() {
    let net = parse(
        "new circuit.c\nnew line._pio_reactor_r bus1=sourcebus bus2=other\nnew reactor.r phases=2 bus1=sourcebus.3.1 bus2=end.1.2 r=.3 x=12 basefreq=50 normamps=400 emergamps=500\n",
    );
    let line = net
        .lines()
        .iter()
        .find(|l| l.name == "_pio_reactor_r_1")
        .unwrap();
    assert_eq!(line.bus_from, "sourcebus");
    assert_eq!(line.bus_to, "end");
    assert_eq!(line.i_max, Some(vec![500.0; 2]));
    assert_eq!(line.extras["dss_original_element"], "reactor.r");
    let code = net
        .line_codes()
        .iter()
        .find(|c| c.name == line.linecode)
        .unwrap();
    assert_eq!(code.r_series, vec![vec![0.3, 0.0], vec![0.0, 0.3]]);
    assert!((code.x_series[0][0] - 14.4).abs() < 1e-12);
    assert!(powerio_dist::audit_electrical_readiness(&net).is_ready());
}

#[test]
fn unresolved_reactor_physics_is_blocked_instead_of_simplified() {
    for property in [
        "rp=20",
        "kvar=100",
        "rmatrix=[1 | 0 1 | 0 0 1]",
        "conn=delta",
        "rcurve=curve",
        "x=0 r=0",
    ] {
        let net = parse(&format!(
            "new circuit.c\nnew reactor.r bus1=sourcebus bus2=end x=12 {property}\n"
        ));
        assert!(
            !powerio_dist::audit_electrical_readiness(&net).is_ready(),
            "{property}"
        );
        assert!(net.lines().iter().all(|l| l.name != "_pio_reactor_r"));
    }
}

#[test]
fn disabled_series_reactor_does_not_create_an_energized_branch() {
    let net = parse("new circuit.c\nnew reactor.r bus1=sourcebus bus2=end x=12 enabled=no\n");
    assert!(net.lines().iter().all(|l| l.name != "_pio_reactor_r"));
    assert!(powerio_dist::audit_electrical_readiness(&net).is_ready());
}

#[test]
fn disabled_property_cannot_hide_an_unresolved_enable_command() {
    let net = parse(
        "new circuit.c\nnew reactor.r bus1=sourcebus bus2=end x=12 enabled=no\nenable reactor.r\n",
    );
    assert!(!powerio_dist::audit_electrical_readiness(&net).is_ready());
}

#[test]
fn storage_readiness_requires_a_typed_static_equivalent() {
    let idle = parse("new circuit.c\nnew storage.b bus1=sourcebus kwrated=250 state=idling\n");
    assert_eq!(idle.loads().len(), 1);
    assert!(powerio_dist::audit_electrical_readiness(&idle).is_ready());
    let net = parse(
        "new circuit.c\nnew storage.b bus1=sourcebus kwrated=250 state=charging effcurve=c\n",
    );
    let audit = powerio_dist::audit_electrical_readiness(&net);
    assert!(!audit.is_ready());
    assert!(
        audit
            .findings
            .iter()
            .any(|f| f.code == "READINESS.DSS.STORAGE_UNRESOLVED")
    );
}
