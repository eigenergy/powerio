use powerio::{BalancedNetwork, Bus, BusId, BusType, DynamicStudy, parse_dyr, parse_dyr_bytes};

#[test]
fn facade_pairing_keeps_sparse_source_ids_and_opaque_keys() {
    let text = "! retained comment\r\n1000 'USERMODEL' 'NOT-A-GENERATOR' 1.00 /\r\n77 'GENCLS' '1' 13.0 0.0 /\r\n10 'OTHER' 'LOAD-A' 'a,b' /\r\n";
    let network = BalancedNetwork::in_memory(
        "static",
        100.0,
        vec![
            Bus::new(BusId(1000), BusType::Ref, 230.0),
            Bus::new(BusId(10), BusType::Pq, 115.0),
        ],
        Vec::new(),
    );
    let dynamics = parse_dyr(text).unwrap();
    let study = DynamicStudy::new(network, dynamics);
    let missing = study.unresolved_buses();
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0].bus_id, 77);
    assert_eq!(missing[0].line, 3);
    assert_eq!(missing[0].model_name, "GENCLS");
    assert_eq!(
        study.dynamics().records()[0].device_id(),
        Some("NOT-A-GENERATOR")
    );
    assert_eq!(study.dynamics().to_dyr_string(), text);
    assert_eq!(study.network().buses()[0].id, BusId(1000));
    let (network, dynamics) = study.into_parts();
    assert_eq!(network.buses()[1].id, BusId(10));
    assert_eq!(dynamics.to_dyr_string(), text);
    assert_eq!(dynamics.unresolved_buses_in(&network), missing);
}

#[test]
fn public_byte_parser_preserves_source_and_rejects_invalid_utf8() {
    let text = "! no generic model schema\n10 'CUSTOM' 'KEY' 'x/y' 1e-3 /\n";
    let case = parse_dyr_bytes(text.as_bytes()).unwrap();
    assert_eq!(case.to_dyr_string(), text);
    assert_eq!(case.records()[0].parameters(), &["x/y", "1e-3"]);
    assert!(matches!(
        parse_dyr_bytes(&[0xff]),
        Err(powerio::DyrError::InvalidUtf8)
    ));
}

/// Generators and DYR records link by `(bus, machine id)`, with ids the PSS/E
/// reader keeps only when they differ from the positional default.
#[test]
fn dyr_records_link_to_generators_by_machine_id() {
    let raw = "0,   100.00, 33, 0, 0, 60.00     / test
first line
second line
   101,'BUS 1', 230.0000,3,   1,   1,   1,1.05000,   0.0000
   102,'BUS 2', 230.0000,2,   1,   1,   1,1.04000,   1.3118
0 /End of Bus data, Begin Load data
0 /End of Load data, Begin Fixed shunt data
0 /End of Fixed shunt data, Begin Generator data
   101,'1 ',   -50.000,    21.572,   100.000,  -100.000,1.05000,     0,   100.000, 0.00000E+0, 1.00000E-5, 0.00000E+0, 0.00000E+0,1.00000,1,  100.0,   100.000,     0.000,   1,1.0000
   102,'G2',    30.000,   -10.000,   100.000,  -100.000,1.04000,     0,   200.000, 1.00000E-3, 2.99500E-1, 0.00000E+0, 0.00000E+0,1.00000,1,  100.0,   100.000,     0.000,   1,1.0000
   102,'1 ',    20.000,   -10.228,   100.000,  -100.000,1.04000,     0,   100.000, 0.00000E+0, 2.00000E-1, 0.00000E+0, 0.00000E+0,1.00000,1,  100.0,   100.000,     0.000,   1,1.0000
   102,'OS',     0.000,     0.000,   100.000,  -100.000,1.04000,     0,   100.000, 0.00000E+0, 2.00000E-1, 0.00000E+0, 0.00000E+0,1.00000,0,  100.0,   100.000,     0.000,   1,1.0000
0 /End of Generator data, Begin Branch data
   101,    102,'1 ', 0.00000E+0, 1.00000E-1,   0.00000,  100.00,  100.00,  100.00,  0.00000,  0.00000,  0.00000,  0.00000,1,1,   0.00,   1,1.0000
   101,    102,'2 ', 0.00000E+0, 1.00000E-1,   0.00000,  100.00,  100.00,  100.00,  0.00000,  0.00000,  0.00000,  0.00000,1,1,   0.00,   1,1.0000
0 /End of Branch data, Begin Transformer data
0 /End of Transformer data
Q
";
    let module =
        powerio::parse(powerio::Source::from_memory("case.raw", raw.as_bytes().to_vec()).unwrap())
            .unwrap();
    let powerio::PioValue::BalancedNetwork(network) = module.value() else {
        panic!("expected a balanced network");
    };
    assert_eq!(powerio::psse_machine_ids(network), ["1", "G2", "1", "OS"]);
    assert_eq!(
        powerio::generator_source_impedance(network, 1),
        Some((1e-3, 0.2995))
    );
    assert_eq!(powerio::generator_source_impedance(network, 9), None);

    let dyr = "101 'GENCLS' 1 0.0 0.0 /
102 'GENCLS' 'G2' 3.0 0.0 /
102 'GENCLS' 1 5.0 1.0 /
102 'SEXS' 1 0.1 10 100 0.1 0 4 /
103 'GENCLS' 1 5.0 1.0 /
102 'XYZ' 1 1 /
102 'GENCLS' 'OS' 4.0 0.0 /
102 'SEXS' 1 0.2 10 100 0.1 0 4 /
";
    let study = DynamicStudy::new(network.clone(), parse_dyr(dyr).unwrap());
    let linking = study.machines();
    assert_eq!(linking.machines.len(), 3);
    let g2 = &linking.machines[1];
    assert_eq!(
        (g2.bus, g2.machine_id.as_str(), g2.mbase),
        (102, "G2", 200.0)
    );
    assert!(matches!(
        g2.machine.as_ref().map(|m| &m.model),
        Some(powerio::DynamicModel::Gencls(g)) if (g.h - 3.0).abs() < 1e-12
    ));
    assert!(g2.exciter.is_none());
    let g3 = &linking.machines[2];
    assert_eq!(g3.machine_id, "1");
    assert!(matches!(
        g3.exciter.as_ref().map(|m| &m.model),
        Some(powerio::DynamicModel::Sexs(_))
    ));
    assert_eq!(linking.issues.len(), 4);
    assert!(matches!(
        linking.issues[0],
        powerio::LinkIssue::NoGenerator { bus: 103, .. }
    ));
    assert!(matches!(
        &linking.issues[1],
        powerio::LinkIssue::Untyped { line: 6, bus: Some(102), machine_id: Some(id), .. } if id == "1"
    ));
    assert!(matches!(
        &linking.issues[2],
        powerio::LinkIssue::OutOfService { bus: 102, machine_id, .. } if machine_id == "OS"
    ));
    // The first exciter of a unit wins; the second is reported, not linked.
    assert!(matches!(
        &linking.issues[3],
        powerio::LinkIssue::Duplicate {
            line: 8,
            role: powerio::ModelRole::Exciter,
            ..
        }
    ));
    assert!(matches!(
        g3.exciter.as_ref().map(|m| &m.model),
        Some(powerio::DynamicModel::Sexs(s)) if (s.ta_tb - 0.1).abs() < 1e-12
    ));
}
