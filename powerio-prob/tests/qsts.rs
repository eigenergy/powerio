//! QSTS shaped data at the dynamic boundary: a parsed feeder under an
//! operating point per time step, the network owned once. Parsing a `.dss`
//! script never claims a solve occurred; the operating point series is supplied at
//! this boundary by whatever produced it.

use powerio_dist::{DistBus, DistSwitch};
use powerio_prob::operating::MulticonductorOperatingPointBuilder;

#[test]
fn qsts_shaped_operating_points_share_one_network() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tests/data/dist/opendss/ieee13/IEEE13Nodeckt.dss"
    );
    let source = powerio_core::Source::open(path).unwrap();
    let mut network = powerio_dist::parse(source).unwrap().into_value();

    // The native switch-like Line has finite impedance, not ideal-contact
    // semantics. Keep that branch and add an application-defined series
    // contact for the supplied operating-point states. No native switching
    // sequence is executed by parsing this case.
    let tie = network
        .lines()
        .iter()
        .find(|line| line.name == "671692")
        .unwrap();
    assert_eq!(tie.extras["dss_switch"], serde_json::json!(true));
    let from = tie.bus_from.clone();
    let terminals = tie.terminal_map_from.clone();
    let code_name = tie.linecode.clone();
    let resistance = network.linecode(&code_name).unwrap().r_series.clone();
    assert!(resistance[0][0] * tie.length > 0.0);
    network
        .buses_mut()
        .push(DistBus::new("qsts_tie_port", terminals.clone()));
    network
        .lines_mut()
        .iter_mut()
        .find(|line| line.name == "671692")
        .unwrap()
        .bus_from = "qsts_tie_port".into();
    network.switches_mut().push(DistSwitch::new(
        "qsts_tie_contact",
        from,
        "qsts_tie_port",
        terminals.clone(),
        terminals,
        false,
    ));
    assert_eq!(network.linecode(&code_name).unwrap().r_series, resistance);

    // 24 supplied hourly states, with durations and the ideal contact cycling.
    let time_points: Vec<powerio_core::TimePoint> = (0..24)
        .map(|hour| {
            powerio_core::TimePoint::new(
                format!("hour {hour}"),
                Some(std::time::Duration::from_secs(3600)),
            )
            .unwrap()
        })
        .collect();
    let closed: Vec<bool> = (0..24).map(|hour| hour % 2 == 0).collect();
    let operating_points = MulticonductorOperatingPointBuilder::new(network, time_points)
        .switch_closed(closed)
        .build()
        .unwrap();

    assert_eq!(operating_points.len(), 24);
    let first = operating_points.values()[0].network();
    for point in operating_points.values() {
        // One shared network under every point: the same table allocation,
        // never a copy per step.
        assert!(std::ptr::eq(
            first.buses().as_ptr(),
            point.network().buses().as_ptr()
        ));
        assert!(std::ptr::eq(
            first.lines().as_ptr(),
            point.network().lines().as_ptr()
        ));
    }
    assert_eq!(
        operating_points.values()[0].switch_closed("qsts_tie_contact"),
        Some(true)
    );
    assert_eq!(
        operating_points.values()[23].switch_closed("qsts_tie_contact"),
        Some(false)
    );
    assert_eq!(
        operating_points.time_points()[23].duration(),
        Some(std::time::Duration::from_secs(3600))
    );
}
