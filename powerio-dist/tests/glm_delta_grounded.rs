use powerio_core::{Destination, Source};
use powerio_dist::glm::{GlmDialect, PreprocessOptions, emit_canonical, parse};
use powerio_dist::{DistLocationTarget, DistWindingConn, MulticonductorNetwork};

fn deck() -> &'static str {
    "module powerflow { solver_method FBS; }
object node { name source; phases ABCN; bustype SWING; nominal_voltage 66395; }
object load { name sink; phases ABCN; nominal_voltage 7200; constant_power_A 1000+200j; }
object transformer_configuration { name cfg; connect_type DELTA_GWYE; primary_voltage 115 kV; secondary_voltage 12.47 kV; power_rating 27.5 MVA; impedance .01344+.1551j; }
object transformer { name substation; phases ABCN; from source; to sink; configuration cfg; latitude -27; longitude 153; }"
}

fn read(text: &str, dialect: GlmDialect) -> powerio_core::PioModule<MulticonductorNetwork> {
    parse(
        Source::from_memory("case.glm", text.as_bytes()).unwrap(),
        PreprocessOptions {
            dialect,
            ..Default::default()
        },
    )
    .unwrap()
}

#[test]
fn delta_grounded_uses_phase_pairs_without_an_artificial_primary_ground() {
    for dialect in [GlmDialect::Pnnl5_3, GlmDialect::Arras4_3] {
        let m = read(deck(), dialect);
        assert_eq!(m.value().transformers().len(), 3);
        for (phase, t) in m.value().transformers().iter().enumerate() {
            assert_eq!(t.phases, 1);
            assert_eq!(t.windings.len(), 2);
            assert_eq!(
                t.windings[0].terminal_map,
                [(phase + 1).to_string(), ["3", "1", "2"][phase].into()]
            );
            assert_eq!(
                t.windings[1].terminal_map,
                [(phase + 1).to_string(), "n".into()]
            );
            assert!(t.windings.iter().all(|w| w.conn == DistWindingConn::Wye));
            assert!((t.windings[0].v_ref - 115_000.0).abs() < 1e-10);
            assert!((t.windings[1].v_ref - 12470.0 / 3.0_f64.sqrt()).abs() < 1e-10);
            assert!((t.windings[1].s_rating - 27_500_000.0 / 3.0).abs() < 1e-8);
            assert!((t.windings[1].r_pct - 1.344).abs() < 1e-12);
            assert!((t.xsc_pct[0] - 15.51).abs() < 1e-12);
            assert!(!t.extras.contains_key("no_load_shunt"));
            assert_eq!(t.extras["ppm_antifloat"], 0.0);
            assert!(
                m.value()
                    .component_locations()
                    .iter()
                    .any(|p| p.target == DistLocationTarget::Transformer && p.name == t.name)
            );
        }
        assert!(
            m.diagnostics()
                .iter()
                .any(|d| d.code() == "READ.GLM.TRANSFORMER_EQUIVALENT")
        );
        // Canonical writing must not silently turn these phase-pair coils into
        // phase-to-ground coils. Its corresponding GLM projection is unfinished.
        assert!(emit_canonical(m.value(), Destination::memory("case.glm").unwrap()).is_err());
    }
}

#[test]
fn step_up_and_equal_line_voltage_ratings_use_the_native_opposite_orientation() {
    for primary in ["1 kV", "12.47 kV"] {
        let text = deck().replace("115 kV", primary).replace("DELTA_GWYE", "3");
        let m = read(&text, GlmDialect::Pnnl5_3);
        for (phase, t) in m.value().transformers().iter().enumerate() {
            assert_eq!(
                t.windings[0].terminal_map,
                [(phase + 1).to_string(), ["2", "3", "1"][phase].into()]
            );
        }
    }
    let nr = read(
        &deck().replace("solver_method FBS", "solver_method NR"),
        GlmDialect::Pnnl5_3,
    );
    assert_eq!(
        nr.value().transformers(),
        read(deck(), GlmDialect::Pnnl5_3).value().transformers()
    );
}

#[test]
fn incomplete_delta_grounded_circuits_are_rejected() {
    for (old, new) in [
        (
            "name substation; phases ABCN",
            "name substation; phases ABN",
        ),
        ("name sink; phases ABCN", "name sink; phases ABN"),
        ("impedance .01344+.1551j", "impedance 0+.1551j"),
        ("power_rating 27.5 MVA", "power_rating 0"),
        ("name cfg;", "name cfg; shunt_resistance 50;"),
    ] {
        assert!(
            parse(
                Source::from_memory("bad.glm", deck().replace(old, new).as_bytes()).unwrap(),
                PreprocessOptions::default()
            )
            .is_err(),
            "accepted {new}"
        );
    }
}
