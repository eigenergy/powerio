use super::*;

fn sample() -> Geometry {
    Geometry {
        wires: vec![Wire {
            x: 0.0,
            height: 10.0,
            radius: 0.01,
            capacitance_radius: 0.01,
            gmr: 0.007_788,
            rdc: 0.0001,
            rac: 0.000_102,
        }],
        phases: 1,
        reduce: false,
        earth_model: EarthModel::Carson,
        rho: 100.0,
    }
}
#[test]
fn single_wire_matches_closed_form_and_rejects_invalid_geometry() {
    let mut input = sample();
    let result = input.calculate(60.0).unwrap();
    let omega = TWO_PI * 60.0;
    assert!((result.r[0][0] - (0.000_102 + omega * MU_0 / 8.0)).abs() < 1e-15);
    assert!((result.b[0][0] - TWO_PI * EPSILON_0 * omega / (2000.0_f64.ln())).abs() < 1e-20);
    input.wires.push(input.wires[0]);
    assert!(input.calculate(60.0).unwrap_err().contains("same space"));
    input.wires.pop();
    input.wires[0].height = 0.0;
    assert!(input.calculate(60.0).is_err());
    assert!(sample().calculate(f64::NAN).is_err());
}
#[test]
#[ignore = "requires pinned native geometry matrix observations"]
fn pinned_overhead_matrices() {
    #[derive(Deserialize)]
    struct Case {
        geometry: Geometry,
        frequency: f64,
        r: Vec<Vec<f64>>,
        x: Vec<Vec<f64>>,
        b: Vec<Vec<f64>>,
    }
    let path = std::env::var("POWERIO_GEOMETRY_ORACLE").unwrap();
    let cases: std::collections::BTreeMap<String, Case> =
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert!(!cases.is_empty());
    for (name, case) in cases {
        let result = case
            .geometry
            .calculate(case.frequency)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        for (label, got, expected, absolute) in [
            ("r", result.r, case.r, 1e-12),
            ("x", result.x, case.x, 1e-12),
            ("b", result.b, case.b, 1e-18),
        ] {
            assert_eq!(got.len(), expected.len(), "{name}/{label}");
            for (row, reference) in got.iter().zip(&expected) {
                assert_eq!(row.len(), reference.len());
                for (a, b) in row.iter().zip(reference) {
                    assert!(
                        (a - b).abs() <= absolute + 1e-10 * b.abs(),
                        "{name}/{label}: {a} != {b}; error {}",
                        (a - b).abs()
                    );
                }
            }
        }
    }
}
