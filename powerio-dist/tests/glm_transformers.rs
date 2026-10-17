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

#[test]
fn nr_delta_declaration_preserves_native_grounded_wye_equations() {
    for connection in ["DELTA_DELTA", "2"] {
        let m = read(&deck().replace("WYE_WYE", connection));
        let t = &m.value().transformers()[0];
        assert!(
            t.windings
                .iter()
                .all(|w| w.conn == powerio_dist::DistWindingConn::Wye)
        );
        assert_eq!(t.windings[0].terminal_map, ["1", "2", "3", "n"]);
        assert_eq!(t.extras["gridlabd_declared_connection"], "DELTA_DELTA");
        assert!(
            m.diagnostics()
                .iter()
                .any(|d| d.code() == "READ.GLM.TRANSFORMER_EQUIVALENT")
        );
        let output = fresh(m.value());
        assert!(output.contains("connect_type WYE_WYE;"));
        let again = read(&output);
        assert_eq!(again.value().transformers()[0].windings, t.windings);
        assert_eq!(again.value().transformers()[0].xsc_pct, t.xsc_pct);
    }
}

#[test]
fn physical_delta_windings_are_not_silently_written_as_native_nr_delta() {
    let mut n = read(deck()).into_value();
    for w in &mut n.transformers_mut()[0].windings {
        w.conn = powerio_dist::DistWindingConn::Delta;
        w.terminal_map.pop();
    }
    assert!(emit_canonical(&n, Destination::memory("bad.glm").unwrap()).is_err());
}

#[test]
fn profiler_is_retained_without_changing_electrical_values() {
    let baseline = read(deck());
    let profiled = read(&format!("#set profiler=1\n{}", deck()));
    assert_eq!(
        serde_json::to_value(profiled.value()).unwrap(),
        serde_json::to_value(baseline.value()).unwrap()
    );
    assert!(
        profiled
            .diagnostics()
            .iter()
            .any(|d| d.code() == "READ.GLM.SOURCE_ONLY")
    );
    assert!(
        parse(
            Source::from_memory(
                "bad.glm",
                format!("#set profiler=garbage\n{}", deck()).as_bytes()
            )
            .unwrap(),
            PreprocessOptions::default()
        )
        .is_err()
    );
}

#[test]
fn fbs_core_and_phase_rating_sum_are_preserved() {
    let text = deck()
        .replace("solver_method NR", "solver_method FBS")
        .replace("WYE_WYE", "1")
        .replace(
            "power_rating .5 MVA",
            "powerA_rating 100; powerB_rating 150; powerC_rating .25 MVA",
        );
    let m = read(&text);
    let t = &m.value().transformers()[0];
    assert!((t.windings[0].s_rating - 500_000.0).abs() < 1e-8);
    let y = &t.extras["no_load_shunt"];
    assert_eq!(y["winding"], 2);
    let expected = 500_000.0 / 480f64.powi(2) / 999_999_999.0;
    assert!((y["g"].as_f64().unwrap() - expected).abs() < 1e-20);
    assert!((y["b"].as_f64().unwrap() + expected).abs() < 1e-20);
    let output = fresh(m.value());
    assert!(output.contains("x__glm_shunt_to"));
    let again = read(&output);
    assert_eq!(again.value().loads().len(), m.value().loads().len() + 3);
    let mut edited = m.into_value();
    edited.transformers_mut()[0].extras.insert(
        "no_load_shunt".into(),
        serde_json::json!({"winding": 2, "g": 0.01, "b": -0.02}),
    );
    assert_ne!(fresh(&edited), output);
}

#[test]
fn inconsistent_phase_ratings_and_fbs_delta_are_rejected() {
    for text in [
        deck().replace("power_rating .5 MVA", "power_rating 500; powerA_rating 300"),
        deck().replace(
            "power_rating .5 MVA",
            "powerA_rating -100; powerB_rating 600",
        ),
        deck()
            .replace("solver_method NR", "solver_method FBS")
            .replace("WYE_WYE", "DELTA_DELTA"),
    ] {
        assert!(
            parse(
                Source::from_memory("bad.glm", text.as_bytes()).unwrap(),
                PreprocessOptions::default()
            )
            .is_err()
        );
    }
}

#[test]
fn implicit_fbs_source_is_the_unique_upstream_root() {
    let explicit = deck().replace("solver_method NR", "solver_method FBS");
    let implicit = explicit.replace("bustype SWING;", "");
    let a = read(&explicit);
    let b = read(&implicit);
    assert_eq!(a.value().sources(), b.value().sources());
    assert!(
        b.diagnostics()
            .iter()
            .any(|d| d.code() == "READ.GLM.IMPLICIT_SOURCE")
    );
    // Reversing declarations must not turn the downstream bus into the source.
    let reordered = implicit
        .lines()
        .filter(|l| !l.starts_with("object node"))
        .chain(implicit.lines().filter(|l| l.starts_with("object node")))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(read(&reordered).value().sources(), a.value().sources());
    for extra in [
        "object node { name island; phases ABCN; nominal_voltage 2400; }",
        "object transformer { name cycle; phases ABCN; from sink; to source; configuration cfg; }",
        "object transformer { name parallel; phases ABCN; from source; to sink; configuration cfg; }",
    ] {
        assert!(
            parse(
                Source::from_memory("bad.glm", format!("{implicit} {extra}").as_bytes()).unwrap(),
                PreprocessOptions::default()
            )
            .is_err()
        );
    }
}
