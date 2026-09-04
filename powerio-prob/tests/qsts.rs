//! QSTS shaped data at the dynamic boundary: a parsed feeder under an
//! operating point per time step, the network owned once. Parsing a `.dss`
//! script never claims a solve occurred; the operating point series is supplied at
//! this boundary by whatever produced it.

use powerio_prob::operating::MulticonductorOperatingPointBuilder;

#[test]
fn qsts_shaped_operating_points_share_one_network() {
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
        operating_points.values()[0].switch_closed("671692"),
        Some(true)
    );
    assert_eq!(
        operating_points.values()[23].switch_closed("671692"),
        Some(false)
    );
    assert_eq!(
        operating_points.time_points()[23].duration(),
        Some(std::time::Duration::from_secs(3600))
    );
}
