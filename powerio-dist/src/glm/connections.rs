//! Explicit native delta/wye channels, independent of the legacy D phase flag.

use num_complex::Complex64;

pub(super) const EXPLICIT: [(&str, usize, Option<usize>); 6] = [
    ("AB", 0, Some(1)),
    ("BC", 1, Some(2)),
    ("CA", 2, Some(0)),
    ("AN", 0, None),
    ("BN", 1, None),
    ("CN", 2, None),
];

pub(super) fn reference(nominal: f64, from: usize, to: Option<usize>) -> Complex64 {
    super::read::nominal_phasor(nominal, from)
        - to.map_or(Complex64::default(), |p| {
            super::read::nominal_phasor(nominal, p)
        })
}

/// The NR DELTA_GWYE equivalent is unqualified with source zero sequence.
/// Group partial source rows by bus and phase; ignore only summation roundoff.
pub(super) fn source_with_zero_sequence(sources: &[crate::VoltageSource]) -> Option<&str> {
    let mut buses = std::collections::BTreeMap::new();
    for source in sources {
        let voltages = buses.entry(source.bus.as_str()).or_insert([None; 3]);
        for (terminal, (&magnitude, &angle)) in source
            .terminal_map
            .iter()
            .zip(source.v_magnitude.iter().zip(&source.v_angle))
        {
            let phase = match terminal.as_str() {
                "1" => 0,
                "2" => 1,
                "3" => 2,
                _ => continue,
            };
            voltages[phase] = Some((magnitude, angle));
        }
    }
    buses.into_iter().find_map(|(bus, voltages)| {
        let [Some(a), Some(b), Some(c)] = voltages else {
            return None;
        };
        let scale = a.0.max(b.0).max(c.0);
        let sum: Complex64 = [a, b, c]
            .into_iter()
            .map(|(magnitude, angle)| Complex64::from_polar(magnitude / scale, angle))
            .sum();
        (sum.norm() > 128.0 * f64::EPSILON).then_some(bus)
    })
}
