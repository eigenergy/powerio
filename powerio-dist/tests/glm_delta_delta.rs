use powerio_core::{Destination, EmittedOutput, Source};
use powerio_dist::glm::{GlmDialect, PreprocessOptions, parse};
use powerio_dist::{DistLocationTarget, DistTargetFormat};

fn deck() -> &'static str {
    "module powerflow { solver_method FBS; }
object node { name source; phases ABCN; bustype SWING; nominal_voltage 2400; }
object node { name sink; phases ABCN; nominal_voltage 240; }
object transformer_configuration { name cfg; connect_type DELTA_DELTA; primary_voltage 4160; secondary_voltage 480; power_rating 500; impedance .011+.02j; }
object transformer { name xf; phases ABCN; from source; to sink; configuration cfg; latitude -27; longitude 153; }"
}
#[test]
fn native_fbs_delta_law_keeps_independent_voltage_and_current_maps() {
    for dialect in [GlmDialect::Pnnl5_3, GlmDialect::Arras4_3] {
        let module = parse(
            Source::from_memory("case.glm", deck().as_bytes()).unwrap(),
            PreprocessOptions {
                dialect,
                ..Default::default()
            },
        )
        .unwrap();
        let net = module.value();
        assert_eq!(net.transformers().as_slice(), []);
        assert_eq!(net.switches().len(), 2);
        assert_eq!(net.shunts().len(), 1);
        let shunt = &net.shunts()[0];
        assert_eq!(shunt.terminal_map.len(), 9);
        // Common-mode primary voltage has no term in the latent constraint.
        for row in 6..9 {
            assert!(shunt.g[row][..3].iter().sum::<f64>().abs() < 1e-15);
        }
        // Unit common-mode output current still reaches every primary phase.
        let scale = 480.0 * 480.0 / 500_000.0;
        let ratio = 4160.0 / 480.0;
        for row in 0..3 {
            assert!((shunt.g[row][6..].iter().sum::<f64>() * scale - 1.0 / ratio).abs() < 1e-15);
        }
        assert!(
            net.component_locations()
                .iter()
                .any(|p| p.target == DistLocationTarget::Shunt && p.name == "xf")
        );
        assert!(
            module
                .diagnostics()
                .iter()
                .any(|d| d.code() == "READ.GLM.TRANSFORMER_EQUIVALENT")
        );
        let output = powerio_dist::emit(
            &module,
            DistTargetFormat::Dss,
            Destination::memory("case.dss").unwrap(),
        )
        .unwrap();
        assert!(
            output
                .diagnostics()
                .iter()
                .any(|d| d.code() == "EMIT.DSS.IMPEDANCE_EQUIVALENT")
        );
        let EmittedOutput::Memory { artifacts } = output.output() else {
            panic!()
        };
        let text = std::str::from_utf8(artifacts[0].bytes()).unwrap();
        assert!(text.contains("New Vsource._pio_coupled__pio_shunt_0_pair_"));
        assert!(text.contains("New Reactor._pio_shunt_0_diagonal_"));
        let source = powerio_dist::emit(
            &module,
            if dialect == GlmDialect::Pnnl5_3 {
                DistTargetFormat::Gridlabd
            } else {
                DistTargetFormat::GridlabdArras
            },
            Destination::memory("case.glm").unwrap(),
        )
        .unwrap();
        let EmittedOutput::Memory { artifacts } = source.output() else {
            panic!()
        };
        assert_eq!(artifacts[0].bytes(), deck().as_bytes());
        let mut invalid = module.clone();
        invalid.value_mut().shunts_mut()[0].g[0][6] = f64::MAX;
        assert!(
            powerio_dist::emit(
                &invalid,
                DistTargetFormat::Dss,
                Destination::memory("bad.dss").unwrap()
            )
            .is_err()
        );
    }
}
#[test]
fn incomplete_fbs_delta_data_still_fails_before_returning_a_network() {
    for (old, new) in [
        ("impedance .011+.02j", "impedance 0+.02j"),
        ("phases ABCN; from", "phases ABN; from"),
        ("secondary_voltage 480", "secondary_voltage 1e-200"),
    ] {
        assert!(
            parse(
                Source::from_memory("bad.glm", deck().replace(old, new).as_bytes()).unwrap(),
                PreprocessOptions::default()
            )
            .is_err()
        );
    }
}
