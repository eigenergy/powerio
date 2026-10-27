use powerio_core::Source;
use powerio_dist::glm::{GlmDialect, PreprocessOptions, parse};

const DECK: &str = "module powerflow { solver_method NR; }
object node { name source; phases ABCN; bustype SWING; nominal_voltage 240; voltage_A 300+0j; }
object node { name sink; phases ABCN; nominal_voltage 120; }
object transformer_configuration { name cfg; connect_type DELTA_GWYE; primary_voltage 480; secondary_voltage 240; power_rating 100 kVA; impedance .01+.04j; }
object transformer { name bank; phases ABCN; from source; to sink; configuration cfg; }";

#[test]
fn nr_delta_grounded_refuses_unqualified_source_zero_sequence() {
    for dialect in [GlmDialect::Pnnl5_3, GlmDialect::Arras4_3] {
        let options = PreprocessOptions {
            dialect,
            ..Default::default()
        };
        let error = parse(
            Source::from_memory("case.glm", DECK.as_bytes()).unwrap(),
            options,
        )
        .unwrap_err();
        assert!(error.to_string().contains("zero-sequence"), "{error}");
        for supported in [
            DECK.replace(" voltage_A 300+0j;", ""),
            DECK.replace("solver_method NR", "solver_method FBS"),
        ] {
            parse(
                Source::from_memory("case.glm", supported.as_bytes()).unwrap(),
                PreprocessOptions {
                    dialect,
                    ..Default::default()
                },
            )
            .unwrap();
        }
    }
}
