//! QSTS shaped data at the dynamic boundary: a parsed feeder under an
//! operating point per time step, the network owned once. Parsing a `.dss`
//! script never claims a solve occurred; the state series is supplied at
//! this boundary by whatever produced it.

use powerio_prob::state::MulticonductorStateBuilder;

#[test]
fn qsts_shaped_states_share_one_network() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tests/data/dist/opendss/ieee13/IEEE13Nodeckt.dss"
    );
    let source = powerio_core::Source::open(path).unwrap();
    let network = powerio_dist::parse(source).unwrap().into_value();

    // 24 hourly steps in the official QSTS shape: hourly points with stated
    // durations, one quantity column varying (the tie switch cycling).
    let time_points: Vec<powerio_core::TimePoint> = (0..24)
        .map(|hour| {
            powerio_core::TimePoint::new(
                format!("hour {hour}"),
                Some(std::time::Duration::from_secs(3600)),
            )
            .unwrap()
        })
        .collect();
    let closed: Vec<f64> = (0..24)
        .map(|hour| f64::from(u8::from(hour % 2 == 0)))
        .collect();
    let states = MulticonductorStateBuilder::new(network, time_points)
        .switch_closed(closed)
        .build()
        .unwrap();

    assert_eq!(states.len(), 24);
    let first = states.values()[0].network();
    for point in states.values() {
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
    assert_eq!(states.values()[0].switch_closed("671692"), Some(true));
    assert_eq!(states.values()[23].switch_closed("671692"), Some(false));
    assert_eq!(
        states.time_points()[23].duration(),
        Some(std::time::Duration::from_secs(3600))
    );
}
