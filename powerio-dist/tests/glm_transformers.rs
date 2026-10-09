use powerio_core::{Destination, EmittedOutput, Source};
use powerio_dist::glm::{PreprocessOptions, emit_canonical, parse};

fn deck() -> &'static str {
    "module powerflow { solver_method NR; }
object node { name source; phases ABCN; bustype SWING; nominal_voltage 2401.7771198288433; }
object load { name sink; phases ABCN; nominal_voltage 277.1281292110204; constant_power_A 160000+110000j; }
object transformer_configuration { name cfg; connect_type WYE_WYE; install_type PADMOUNT;
primary_voltage 4160 V; secondary_voltage .48 kV; power_rating .5 MVA;
impedance .03+.04j; resistance .011; reactance .02; }
object transformer { name x; phases ABCN; from source; to sink; configuration cfg; latitude 0; longitude 151; }"
}

fn read(s: &str) -> powerio_core::PioModule<powerio_dist::MulticonductorNetwork> {
    parse(
        Source::from_memory("case.glm", s.as_bytes()).unwrap(),
        PreprocessOptions::default(),
    )
    .unwrap()
}

fn fresh(n: &powerio_dist::MulticonductorNetwork) -> String {
    let e = emit_canonical(n, Destination::memory("case.glm").unwrap()).unwrap();
    let EmittedOutput::Memory { artifacts } = e.output() else {
        panic!()
    };
    String::from_utf8(artifacts[0].bytes().to_vec()).unwrap()
}

#[test]
fn ieee13_transformer_uses_si_winding_bases_and_edited_value() {
    let m = read(deck());
    let t = &m.value().transformers()[0];
    assert_eq!(t.phases, 3);
    assert_eq!(t.windings[0].terminal_map, ["1", "2", "3", "n"]);
    assert!((t.windings[0].s_rating - 500_000.0).abs() < 1e-8);
    assert!((t.windings[1].v_ref - 480.0).abs() < 1e-12);
    assert!((t.windings[1].r_pct - 1.1).abs() < 1e-12);
    assert!((t.xsc_pct[0] - 2.0).abs() < 1e-12);
    let again = read(&fresh(m.value()));
    assert_eq!(again.value().transformers(), m.value().transformers());
    assert_eq!(
        again.value().component_locations(),
        m.value().component_locations()
    );
    let mut edited = m.into_value();
    edited.transformers_mut()[0].windings[1].v_ref = 600.0;
    edited.transformers_mut()[0].xsc_pct[0] = 3.0;
    let again = read(&fresh(&edited));
    assert_eq!(again.value().transformers(), edited.transformers());
}

#[test]
fn transformer_alias_assignments_obey_source_order() {
    let m = read(&deck().replace(
        "impedance .03+.04j; resistance .011; reactance .02;",
        "resistance .011; reactance .02; impedance .03+.04j;",
    ));
    let t = &m.value().transformers()[0];
    assert!((t.windings[1].r_pct - 3.0).abs() < 1e-12);
    assert!((t.xsc_pct[0] - 4.0).abs() < 1e-12);
}

#[test]
fn unqualified_transformer_physics_fails_instead_of_projecting() {
    for (old, new) in [
        ("solver_method NR", "solver_method FBS"),
        ("WYE_WYE", "DELTA_GWYE"),
        ("resistance .011", "resistance 0"),
        ("reactance .02", "reactance -1"),
        ("power_rating .5 MVA", "power_rating 0"),
        ("install_type PADMOUNT", "shunt_resistance 100"),
    ] {
        let text = deck().replace(old, new);
        assert!(
            parse(
                Source::from_memory("bad.glm", text.as_bytes()).unwrap(),
                PreprocessOptions::default()
            )
            .is_err(),
            "accepted {new}"
        );
    }
    let mut n = read(deck()).into_value();
    n.transformers_mut()[0].windings[1].tap = 1.05;
    assert!(emit_canonical(&n, Destination::memory("bad.glm").unwrap()).is_err());
}
