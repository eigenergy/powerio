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
        #[serde(default)]
        ratings: BTreeMap<String, [f64; 2]>,
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
                if let Some(expected) = observation.ratings.get(&line) {
                    for (actual, expected) in [state.normamps, state.emergamps].iter().zip(expected)
                    {
                        assert!(
                            (actual - expected).abs() < 1e-10,
                            "{name}/{line}/{}/rating: {actual} != {expected}",
                            observation.command
                        );
                    }
                }
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

#[test]
fn direct_spacing_uses_previous_active_earth_model_and_first_wire_rating() {
    let script = "clear\nnew circuit.c basekv=12.47\nnew wiredata.w rac=.0002 radius=.01 radunits=m runits=m normamps=100\nnew wiredata.n rac=.0004 radius=.005 radunits=m runits=m normamps=70\nnew linespacing.s nconds=2 nphases=1 units=m x=[0 1] h=[10 9]\nnew line.a bus1=sourcebus.1 bus2=a.1 spacing=s wires=[w n] length=100 units=m earthmodel=carson\nnew line.b bus1=sourcebus.1 bus2=b.1 spacing=s wires=[w n] length=100 units=m earthmodel=deri\nset controlmode=off\nsolve\n";
    let raw =
        crate::dss::raw::__parse_raw_with(script, "direct.dss", &mut |_: &std::path::Path| {
            Err(std::io::Error::other("no includes"))
        });
    let mut states = resolve(&raw);
    // Pinned native DSS C-API87d85c2 observations. The first line computes with
    // Deri, then sets Carson, which the second line uses for its calculation.
    for (name, expected) in [
        (
            "a",
            Complex::new(5.867_598_074_966_158, -12.584_981_620_131_156),
        ),
        (
            "b",
            Complex::new(5.931_475_775_215_832, -12.543_154_948_714_399),
        ),
    ] {
        let index = raw
            .objects
            .iter()
            .position(|o| o.class == "line" && o.name == name)
            .unwrap();
        let state = states.get_mut(&index).unwrap().as_mut().unwrap();
        let matrix = state.primitive(60.0).unwrap();
        let y = Complex::new(1.0, 0.0) / Complex::new(matrix.r[0][0], matrix.x[0][0]);
        assert!((y - expected).norm() < 1e-10);
        assert!((state.normamps - 100.0).abs() < 1e-10);
    }
}
