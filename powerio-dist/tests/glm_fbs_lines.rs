use num_complex::Complex64 as C;
use powerio_core::Source;
use powerio_dist::DistLocationTarget;
use powerio_dist::glm::{PreprocessOptions, parse};

#[test]
fn native_hybrid_constraints_and_separate_coordinates_survive_import() {
    for phases in ["AB", "AC", "ABC"] {
        let labels = phases
            .chars()
            .map(|p| (p as u8 - b'A' + 1) as usize)
            .collect::<Vec<_>>();
        let n = labels.len();
        let z = (0..n)
            .map(|i| {
                (0..n)
                    .map(|j| C::new(if i == j { 1.0 + i as f64 } else { 0.07 }, 0.5))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let props = properties(&labels, &z);
        let text=format!("module powerflow {{ solver_method FBS; line_capacitance TRUE; }}
object node {{ name a; phases {phases}N; bustype SWING; nominal_voltage 240; }}
object node {{ name b; phases {phases}N; nominal_voltage 240; }}
object line_configuration {{ name cfg; {props} latitude 1; longitude 2; }}
object overhead_line {{ name line; phases {phases}N; from a; to b; configuration cfg; length 1 mile; latitude 3; longitude 4; }}");
        let module = parse(
            Source::from_memory("case.glm", text.as_bytes()).unwrap(),
            PreprocessOptions::default(),
        )
        .unwrap();
        let net = module.value();
        let sh = &net.shunts()[0];
        assert_eq!(sh.terminal_map.len(), 3 * n);
        assert_eq!(net.line_codes().len(), 1);
        assert!(
            net.component_locations()
                .iter()
                .any(|l| l.target == DistLocationTarget::LineCode)
        );
        assert!(
            net.component_locations()
                .iter()
                .any(|l| l.target == DistLocationTarget::Shunt)
        );
        let y = (0..n)
            .map(|i| {
                (0..n)
                    .map(|j| C::new(0.0, std::f64::consts::PI * 60.0 * cap(i, j) * 1e-9))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let times = |m: &[Vec<C>], v: &[C]| {
            m.iter()
                .map(|r| r.iter().zip(v).map(|(a, b)| a * b).sum::<C>())
                .collect::<Vec<_>>()
        };
        let vt = (0..n)
            .map(|i| C::new(230.0 + i as f64, -20.0 * i as f64))
            .collect::<Vec<_>>();
        let it = (0..n)
            .map(|i| C::new(2.0 + i as f64, -1.0))
            .collect::<Vec<_>>();
        let zyv = times(&z, &times(&y, &vt));
        let zit = times(&z, &it);
        let vf = (0..n).map(|i| vt[i] + zyv[i] + zit[i]).collect::<Vec<_>>();
        let yv = times(&y, &vt);
        let yzyv = times(&y, &zyv);
        let zyit = times(&z, &times(&y, &it));
        let incoming = (0..n)
            .map(|i| 2.0 * yv[i] + yzyv[i] + it[i] + zyit[i])
            .collect::<Vec<_>>();
        let scale = z.iter().flatten().map(|v| v.norm()).fold(0.0_f64, f64::max);
        let voltages = vf
            .into_iter()
            .chain(vt)
            .chain(it.iter().map(|v| v * scale))
            .collect::<Vec<_>>();
        for (i, (g, b)) in sh.g.iter().zip(&sh.b).enumerate() {
            let got = g
                .iter()
                .zip(b)
                .zip(&voltages)
                .map(|((&g, &b), v)| C::new(g, b) * v)
                .sum::<C>();
            let expected = if i < n {
                incoming[i]
            } else if i < 2 * n {
                -it[i - n]
            } else {
                C::default()
            };
            assert!(
                (got - expected).norm() < 1e-12,
                "{phases} row {i}: {got} != {expected}"
            );
        }
    }
}

fn cap(i: usize, j: usize) -> f64 {
    if i == j {
        100.0 + 20.0 * i as f64
    } else {
        -10.0
    }
}
fn properties(labels: &[usize], z: &[Vec<C>]) -> String {
    use std::fmt::Write;
    let mut props = String::new();
    for (i, &a) in labels.iter().enumerate() {
        for (j, &b) in labels.iter().enumerate() {
            write!(
                props,
                "z{a}{b} {}+{}j; c{a}{b} {}; ",
                z[i][j].re,
                z[i][j].im,
                cap(i, j)
            )
            .unwrap();
        }
    }
    props
}
