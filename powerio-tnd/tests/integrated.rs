use num_complex::Complex64;
use powerio_core::{PioModule, Source};
use powerio_dist::{DistBus, DistSwitch, MulticonductorNetwork, VoltageSource};
use powerio_tnd::{IntegratedNetwork, Interface, TransmissionDemand, boundary_power};
use powerio_tx::{BalancedNetwork, Bus, BusId, BusType, Load};
use std::{collections::BTreeMap, f64::consts::PI};

fn parts() -> (
    PioModule<BalancedNetwork>,
    BTreeMap<String, PioModule<MulticonductorNetwork>>,
    Interface,
) {
    let mut tx = BalancedNetwork::in_memory(
        "transmission",
        100.0,
        vec![Bus::new(BusId(1), BusType::Ref, 110.0)],
        vec![],
    );
    let mut load = Load::new(BusId(1), 10.0, 3.0);
    load.uid = Some("feeder-demand".into());
    tx.loads_mut().push(load);
    let mut dist = MulticonductorNetwork::named("feeder");
    dist.buses_mut().push(DistBus::new(
        "head",
        vec!["1".into(), "2".into(), "3".into()],
    ));
    dist.sources_mut().push(VoltageSource::new(
        "grid",
        "head",
        vec!["1".into(), "2".into(), "3".into()],
        vec![11000.0 / 3.0_f64.sqrt(); 3],
        vec![0.0, -2.0 * PI / 3.0, 2.0 * PI / 3.0],
    ));
    let link = Interface::new(
        "substation",
        BusId(1),
        "feeder",
        "grid",
        ["1".into(), "2".into(), "3".into()],
        11.0 / 110.0,
        TransmissionDemand::ReplaceLoads(vec!["feeder-demand".into()]),
    );
    (
        PioModule::new(tx),
        BTreeMap::from([("feeder".into(), PioModule::new(dist))]),
        link,
    )
}
fn close(a: Complex64, b: Complex64) {
    assert!((a - b).norm() < 1e-8, "{a} != {b}");
}
#[test]
fn physical_units_phase_order_and_power_direction() {
    let (tx, feeders, mut link) = parts();
    link.voltage_ratio = 11.0 / 110.0;
    link.angle_shift_radians = PI / 6.0;
    let model = IntegratedNetwork::new(tx.clone(), feeders.clone(), vec![link.clone()]).unwrap();
    let voltage = model
        .boundary_voltages("substation", Complex64::new(1.0, 0.0))
        .unwrap();
    close(
        voltage[0],
        Complex64::from_polar(11000.0 / 3.0_f64.sqrt(), PI / 6.0),
    );
    close(
        voltage[1] / voltage[0],
        Complex64::from_polar(1.0, -2.0 * PI / 3.0),
    );
    let current = voltage.map(|v| v / 100.0);
    close(
        boundary_power(voltage, current).unwrap(),
        Complex64::new(1.21, 0.0),
    );
    close(
        boundary_power(voltage, current.map(|i| -i)).unwrap(),
        Complex64::new(-1.21, 0.0),
    );
    let mut network = tx.value().clone();
    *network.base_mva_mut() = 17.0;
    let other = IntegratedNetwork::new(PioModule::new(network), feeders, vec![link]).unwrap();
    assert_eq!(
        voltage,
        other
            .boundary_voltages("substation", Complex64::new(1.0, 0.0))
            .unwrap()
    );
    assert_eq!(model.transmission().value().loads().len(), 1);
}
#[test]
fn unbalanced_reactive_power_and_nonfinite_inputs() {
    let v = [
        Complex64::new(1000.0, 0.0),
        Complex64::new(0.0, 1000.0),
        Complex64::new(-1000.0, 0.0),
    ];
    let i = [
        Complex64::new(1.0, -2.0),
        Complex64::new(3.0, 4.0),
        Complex64::new(-5.0, 6.0),
    ];
    close(boundary_power(v, i).unwrap(), Complex64::new(0.01, 0.011));
    assert!(boundary_power(v, [Complex64::new(f64::NAN, 0.0); 3]).is_err());
    assert!(
        boundary_power(
            [Complex64::new(f64::MAX, 0.0); 3],
            [Complex64::new(f64::MAX, 0.0); 3]
        )
        .is_err()
    );
    let (tx, feeders, link) = parts();
    let model = IntegratedNetwork::new(tx, feeders, vec![link]).unwrap();
    assert!(
        model
            .boundary_voltages("missing", Complex64::new(1.0, 0.0))
            .is_err()
    );
    assert!(
        model
            .boundary_voltages("substation", Complex64::new(f64::MAX, 0.0))
            .is_err()
    );
}
#[test]
fn rejects_inconsistent_interfaces() {
    let (tx, feeders, link) = parts();
    let reject =
        |link| assert!(IntegratedNetwork::new(tx.clone(), feeders.clone(), vec![link]).is_err());
    let mut bad = link.clone();
    bad.transmission_bus = BusId(99);
    reject(bad);
    let mut bad = link.clone();
    bad.feeder = "missing".into();
    reject(bad);
    let mut bad = link.clone();
    bad.voltage_source = "missing".into();
    reject(bad);
    let mut bad = link.clone();
    bad.phase_terminals[1] = "1".into();
    reject(bad);
    let mut bad = link.clone();
    bad.voltage_ratio = 0.0;
    reject(bad);
    let mut bad = link.clone();
    bad.angle_shift_radians = f64::NAN;
    reject(bad);
    let mut bad = link.clone();
    bad.demand = TransmissionDemand::ReplaceLoads(vec![]);
    reject(bad);
    let mut bad = link.clone();
    bad.demand = TransmissionDemand::ReplaceLoads(vec!["missing".into()]);
    reject(bad);
    let mut bad = link.clone();
    bad.demand = TransmissionDemand::ReplaceLoads(vec!["feeder-demand".into(); 2]);
    reject(bad);
    assert!(
        IntegratedNetwork::new(
            tx.clone(),
            feeders.clone(),
            vec![link.clone(), link.clone()]
        )
        .is_err()
    );
    let mut bad = feeders.clone();
    bad.insert("unconnected".into(), feeders["feeder"].clone());
    assert!(IntegratedNetwork::new(tx.clone(), bad, vec![link.clone()]).is_err());
    let mut net = feeders["feeder"].value().clone();
    *net.base_frequency_mut() = 50.0;
    assert!(
        IntegratedNetwork::new(
            tx,
            BTreeMap::from([("feeder".into(), PioModule::new(net))]),
            vec![link]
        )
        .is_err()
    );
}
#[test]
fn stated_source_voltages_must_match_the_ratio_and_rotation() {
    let (tx, feeders, link) = parts();
    let build =
        |link: &Interface| IntegratedNetwork::new(tx.clone(), feeders.clone(), vec![link.clone()]);
    // An unset ratio would impose 63.5 kV on a 6.35 kV feeder.
    let mut unit = link.clone();
    unit.voltage_ratio = 1.0;
    let error = build(&unit).unwrap_err().to_string();
    assert!(error.contains("voltage ratio"), "{error}");
    // Swapping B and C supplies negative-sequence voltage.
    let mut swapped = link.clone();
    swapped.phase_terminals = ["1".into(), "3".into(), "2".into()];
    let error = build(&swapped).unwrap_err().to_string();
    assert!(error.contains("rotation"), "{error}");
    // An off-nominal setpoint and a small angle offset are ordinary.
    let mut net = feeders["feeder"].value().clone();
    let source = &mut net.sources_mut()[0];
    source.v_magnitude = source.v_magnitude.iter().map(|v| v * 1.05).collect();
    source.v_angle[1] += 0.05;
    let stated = BTreeMap::from([("feeder".into(), PioModule::new(net))]);
    assert!(IntegratedNetwork::new(tx.clone(), stated, vec![link]).is_ok());
}

#[test]
fn preserves_earth_clamps_and_refuses_conflicting_sources() {
    let (tx, mut feeders, link) = parts();
    let mut net = feeders["feeder"].value().clone();
    net.buses_mut()[0].terminals.push("4".into());
    net.sources_mut()[0].terminal_map.push("4".into());
    net.sources_mut()[0].v_magnitude.push(0.0);
    net.sources_mut()[0].v_angle.push(0.0);
    feeders.insert("feeder".into(), PioModule::new(net.clone()));
    let value = IntegratedNetwork::new(tx.clone(), feeders.clone(), vec![link.clone()]).unwrap();
    assert_eq!(
        value.feeders()["feeder"].value().sources()[0]
            .terminal_map
            .len(),
        4
    );
    net.sources_mut()[0].v_magnitude[3] = 1.0;
    feeders.insert("feeder".into(), PioModule::new(net.clone()));
    assert!(IntegratedNetwork::new(tx.clone(), feeders.clone(), vec![link.clone()]).is_err());
    net.sources_mut()[0].v_magnitude[3] = 0.0;
    net.sources_mut()[0].reference_terminal = Some("4".into());
    feeders.insert("feeder".into(), PioModule::new(net.clone()));
    assert!(IntegratedNetwork::new(tx.clone(), feeders.clone(), vec![link.clone()]).is_err());
    net.sources_mut()[0].reference_terminal = None;
    let mut second = net.sources()[0].clone();
    second.name = "other".into();
    net.sources_mut().push(second);
    feeders.insert("feeder".into(), PioModule::new(net));
    assert!(IntegratedNetwork::new(tx, feeders, vec![link]).is_err());
}
fn terminals(names: &[&str]) -> Vec<String> {
    names.iter().map(|&name| name.to_owned()).collect()
}
fn with_feeder(
    tx: &PioModule<BalancedNetwork>,
    link: &Interface,
    net: &MulticonductorNetwork,
) -> Result<IntegratedNetwork, powerio_core::Error> {
    IntegratedNetwork::new(
        tx.clone(),
        BTreeMap::from([("feeder".into(), PioModule::new(net.clone()))]),
        vec![link.clone()],
    )
}
#[test]
fn refuses_sources_reached_through_closed_switches() {
    let (tx, feeders, link) = parts();
    let mut net = feeders["feeder"].value().clone();
    let abc = terminals(&["1", "2", "3"]);
    net.buses_mut().push(DistBus::new("other", abc.clone()));
    let mut second = net.sources()[0].clone();
    second.name = "backfeed".into();
    second.bus = "other".into();
    net.sources_mut().push(second);
    net.switches_mut().push(DistSwitch::new(
        "tie",
        "head",
        "other",
        abc.clone(),
        abc,
        true,
    ));
    assert!(with_feeder(&tx, &link, &net).is_ok());
    net.switches_mut()[0].open = false;
    assert!(with_feeder(&tx, &link, &net).is_err());
}
#[test]
fn closed_switches_join_terminals_by_their_maps() {
    let (tx, feeders, link) = parts();
    let mut net = feeders["feeder"].value().clone();
    net.buses_mut()
        .push(DistBus::new("other", terminals(&["1", "2"])));
    net.sources_mut().push(VoltageSource::new(
        "aux",
        "other",
        terminals(&["2"]),
        vec![230.0],
        vec![0.0],
    ));
    // Bus connectivity alone is not terminal equivalence: head 1 meets the
    // unsourced other 1, while aux prescribes only other 2.
    net.switches_mut().push(DistSwitch::new(
        "tie",
        "head",
        "other",
        terminals(&["1"]),
        terminals(&["1"]),
        false,
    ));
    assert!(with_feeder(&tx, &link, &net).is_ok());
    net.switches_mut()[0].terminal_map_to = terminals(&["2"]);
    assert!(with_feeder(&tx, &link, &net).is_err());
}
#[test]
fn closed_switch_chains_and_grounds_reach_the_interface() {
    let (tx, feeders, link) = parts();
    let base = feeders["feeder"].value().clone();
    let abc = terminals(&["1", "2", "3"]);
    let mut net = base.clone();
    for bus in ["mid", "other"] {
        net.buses_mut().push(DistBus::new(bus, abc.clone()));
    }
    let mut second = net.sources()[0].clone();
    second.name = "backfeed".into();
    second.bus = "OTHER".into();
    net.sources_mut().push(second);
    for (name, from, to) in [("a", "head", "mid"), ("b", "Mid", "other")] {
        net.switches_mut().push(DistSwitch::new(
            name,
            from,
            to,
            abc.clone(),
            abc.clone(),
            false,
        ));
    }
    assert!(with_feeder(&tx, &link, &net).is_err());
    net.switches_mut()[1].open = true;
    assert!(with_feeder(&tx, &link, &net).is_ok());

    let mut net = base;
    let mut grounded = DistBus::new("other", abc.clone());
    grounded.grounded = terminals(&["1"]);
    net.buses_mut().push(grounded);
    net.switches_mut().push(DistSwitch::new(
        "tie",
        "head",
        "other",
        abc.clone(),
        abc,
        true,
    ));
    assert!(with_feeder(&tx, &link, &net).is_ok());
    net.switches_mut()[0].open = false;
    assert!(with_feeder(&tx, &link, &net).is_err());
}
#[test]
fn refuses_closed_switches_without_one_to_one_terminal_maps() {
    let (tx, feeders, link) = parts();
    let mut net = feeders["feeder"].value().clone();
    net.buses_mut()
        .push(DistBus::new("other", terminals(&["1", "2", "3"])));
    net.switches_mut().push(DistSwitch::new(
        "tie",
        "head",
        "other",
        terminals(&["1", "2"]),
        terminals(&["1"]),
        true,
    ));
    assert!(with_feeder(&tx, &link, &net).is_ok());
    net.switches_mut()[0].open = false;
    let error = with_feeder(&tx, &link, &net).unwrap_err().to_string();
    assert!(error.contains("one to one"), "{error}");
}
#[test]
fn independently_parsed_formats_preserve_modules() {
    let source = Source::from_memory(
        "case.m",
        include_bytes!("../../tests/data/case9.m").to_vec(),
    )
    .unwrap();
    let tx = powerio_tx::parse(source).unwrap();
    let source = Source::from_memory(
        "feeder.dss",
        b"clear\nnew circuit.feeder basekv=11 pu=1 phases=3 bus1=head\n".to_vec(),
    )
    .unwrap();
    let feeder = powerio_dist::parse(source).unwrap();
    let source_name = feeder.value().sources()[0].name.clone();
    let link = Interface::new(
        "head",
        tx.value().buses()[0].id,
        "feeder",
        source_name,
        ["1".into(), "2".into(), "3".into()],
        // An 11 kV feeder below case9's 345 kV bus 1.
        11.0 / 345.0,
        TransmissionDemand::Additional,
    );
    let model = IntegratedNetwork::new(tx, BTreeMap::from([("feeder".into(), feeder)]), vec![link])
        .unwrap();
    assert!(model.transmission().source().is_some());
    assert!(model.feeders()["feeder"].source().is_some());
}

#[cfg(feature = "sincal")]
#[test]
fn sincal_is_an_adapter_into_the_same_composition() {
    use powerio_tnd::formats::sincal::{
        __write_sincal_multiconductor_experimental, ExperimentalMulticonductorOptions,
        SincalReadOptions, parse_sincal,
    };
    let (tx, feeders, _) = parts();
    let options = ExperimentalMulticonductorOptions {
        nominal_ll_volts: BTreeMap::from([("head".into(), 11000.0)]),
    };
    let output =
        __write_sincal_multiconductor_experimental(feeders["feeder"].value(), &options).unwrap();
    let feeder = parse_sincal(
        Source::from_memory("project.db", output.database.clone()).unwrap(),
        &SincalReadOptions::default(),
    )
    .unwrap();
    let source = &feeder.value().sources()[0];
    let link = Interface::new(
        "substation",
        BusId(1),
        "sincal-feeder",
        &source.name,
        ["1".into(), "2".into(), "3".into()],
        11.0 / 110.0,
        TransmissionDemand::Additional,
    );
    let model = IntegratedNetwork::new(
        tx,
        BTreeMap::from([("sincal-feeder".into(), feeder)]),
        vec![link],
    )
    .unwrap();
    assert_eq!(
        model.feeders()["sincal-feeder"]
            .source()
            .unwrap()
            .primary_buffer()
            .unwrap()
            .bytes(),
        output.database
    );
}
