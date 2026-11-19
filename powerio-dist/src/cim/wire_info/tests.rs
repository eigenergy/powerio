use super::*;

fn bare(label: &str) -> Catalog<'static> {
    Catalog {
        label: label.into(),
        class: "OverheadWireInfo".into(),
        value: Box::new(|key| match key {
            "WireInfo.radius" => Some(0.01),
            "WireInfo.gmr" => Some(0.0078),
            "WireInfo.rDC20" => Some(1e-4),
            _ => None,
        }),
    }
}

fn at(sequence: f64, x: f64, y: f64) -> Position {
    Position {
        sequence: Some(sequence),
        x: Some(x),
        y: Some(y),
    }
}

fn conductor(label: &str, sequence: f64, reduced: bool) -> Conductor<'static> {
    Conductor {
        label: label.into(),
        sequence: Some(sequence),
        catalog: Some(bare(label)),
        reduced,
    }
}

#[test]
fn repeated_positions_are_one_and_conflicting_ones_are_refused() {
    let records = [at(1.0, 0.0, 9.0), at(1.0, 0.0, 9.0), at(2.0, 1.0, 9.0)];
    let conductors = [conductor("a", 1.0, false), conductor("n", 2.0, true)];
    let impedance = segment_impedance("s", &records, &conductors, 60.0).unwrap();
    assert_eq!(impedance.r.len(), 1);
    // rAC is the dc value times 1.02, and says so.
    assert!(impedance.notes[0].contains("1.02"));
    let conflicting = [at(1.0, 0.0, 9.0), at(1.0, 0.5, 9.0)];
    let error = segment_impedance("s", &conflicting, &conductors[..1], 60.0).unwrap_err();
    assert!(error.contains("WirePosition 1 is stated at"), "{error}");
}

#[test]
fn a_position_no_phase_names_leaves_the_impedance_unknown() {
    let records = [at(1.0, 0.0, 9.0), at(2.0, 1.0, 9.0)];
    let error =
        segment_impedance("601", &records, &[conductor("a", 1.0, false)], 60.0).unwrap_err();
    assert!(error.contains("position(s) 2"), "{error}");
    let error =
        segment_impedance("601", &records, &[conductor("a", 3.0, false)], 60.0).unwrap_err();
    assert!(error.contains("names WirePosition 3"), "{error}");
}

#[test]
fn an_explicit_neutral_stays_and_a_reduced_one_leaves() {
    let records = [at(1.0, 0.0, 9.0), at(2.0, 1.0, 9.0)];
    let explicit = segment_impedance(
        "s",
        &records,
        &[conductor("a", 1.0, false), conductor("n", 2.0, false)],
        60.0,
    )
    .unwrap();
    let reduced = segment_impedance(
        "s",
        &records,
        &[conductor("a", 1.0, false), conductor("n", 2.0, true)],
        60.0,
    )
    .unwrap();
    assert_eq!(explicit.r.len(), 2);
    // Kron reduction of the explicit pair is the reduced result.
    let z = |i: usize, j: usize| num_complex::Complex64::new(explicit.r[i][j], explicit.x[i][j]);
    let kron = z(0, 0) - z(0, 1) * z(1, 0) / z(1, 1);
    assert!((kron.re - reduced.r[0][0]).abs() < 1e-15);
    assert!((kron.im - reduced.x[0][0]).abs() < 1e-15);
}
