use super::*;
use num_complex::Complex64 as Complex;
use std::fmt::Write as _;

#[test]
#[ignore = "requires pinned native geometry cache trace"]
fn pinned_calculation_history() {
    #[derive(serde::Deserialize)]
    struct Observation {
        command: String,
        primitives: BTreeMap<String, Vec<f64>>,
    }
    #[derive(serde::Deserialize)]
    struct Case {
        base: String,
        observations: Vec<Observation>,
    }
    let path = std::env::var("POWERIO_GEOMETRY_CACHE_ORACLE").expect("native cache trace");
    let cases: BTreeMap<String, Case> =
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert!(!cases.is_empty());
    for (name, case) in cases {
        let mut script = case.base;
        for observation in case.observations {
            if observation.command != "initial" {
                writeln!(script, "{}\nsolve", observation.command).unwrap();
            }
            let raw = crate::dss::raw::__parse_raw_with(
                &script,
                "geometry-cache.dss",
                &mut |_: &std::path::Path| Err(std::io::Error::other("no includes")),
            );
            let mut states = resolve(&raw);
            for (line, expected) in observation.primitives {
                let object = raw
                    .objects
                    .iter()
                    .position(|o| o.class == "line" && o.name == line)
                    .unwrap();
                let state = states
                    .get_mut(&object)
                    .unwrap()
                    .as_mut()
                    .unwrap_or_else(|e| panic!("{name}/{line}: {e}"));
                let m = state.primitive(raw.snapshot_frequency()).unwrap();
                assert_eq!(m.r.len(), 1);
                let y = Complex::new(1.0, 0.0) / Complex::new(m.r[0][0], m.x[0][0]);
                let diagonal = y + Complex::new(0.0, m.b[0][0] / 2.0);
                let actual = [diagonal, -y, -y, diagonal];
                assert_eq!(expected.len(), 8);
                for (actual, expected) in actual.iter().zip(expected.chunks_exact(2)) {
                    let expected = Complex::new(expected[0], expected[1]);
                    assert!(
                        (*actual - expected).norm() <= 1e-10 + 1e-10 * expected.norm(),
                        "{name}/{line}/{}: {actual} != {expected}",
                        observation.command
                    );
                }
            }
        }
    }
}
