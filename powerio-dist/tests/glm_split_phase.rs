use powerio_core::{Destination, EmittedOutput, Source};
use powerio_dist::glm::{GlmDialect, PreprocessOptions, emit_canonical, parse};
use powerio_dist::{Configuration, DistLoadVoltageModel, DistLocationTarget, DistTargetFormat};

fn deck() -> &'static str {
    "module powerflow { solver_method NR; }
object node { name source; phases CN; bustype SWING; nominal_voltage 7200; }
object triplex_load { name sink; phases CS; nominal_voltage 120;
constant_power_1 1945+487.5j; constant_power_2 800+200j;
constant_current_12 2-.5j; }
object transformer_configuration { name cfg; connect_type SINGLE_PHASE_CENTER_TAPPED;
primary_voltage 7200; secondary_voltage 120; power_rating 15; powerC_rating 15;
impedance .006+.0136j; impedance1 .012+.0068j; impedance2 .018+.009j;
shunt_impedance 1728000+691200j; }
object transformer { name x; nominal_voltage 7199.558; phases CS; from source; to sink; configuration cfg;
latitude -27; longitude 153; }"
}

fn read(text: &str) -> powerio_core::PioModule<powerio_dist::MulticonductorNetwork> {
    parse(
        Source::from_memory("case.glm", text.as_bytes()).unwrap(),
        PreprocessOptions::default(),
    )
    .unwrap()
}

fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-12, "{a} != {b}");
}

#[test]
fn center_tapped_service_keeps_active_phase_base_polarity_and_internal_core() {
    let module = read(deck());
    let n = module.value();
    let t = &n.transformers()[0];
    assert_eq!(t.phases, 1);
    assert_eq!(t.windings.len(), 4);
    assert_eq!(t.windings[0].terminal_map, ["3", "n"]);
    assert_eq!(t.windings[1].terminal_map, ["1", "n"]);
    assert_eq!(t.windings[2].terminal_map, ["n", "2"]);
    assert_eq!(t.windings[1].bus, t.windings[2].bus);
    for winding in &t.windings {
        close(winding.s_rating, 15_000.0);
    }
    for (actual, expected) in t.xsc_pct.iter().zip([2.04, 2.26, 1.36, 1.58, 0.68, 0.9]) {
        close(*actual, expected);
    }
    close(t.windings[0].r_pct, 0.6);
    close(t.windings[1].r_pct, 1.2);
    close(t.windings[2].r_pct, 1.8);
    let core = n
        .loads()
        .iter()
        .find(|l| l.bus == t.windings[3].bus)
        .unwrap();
    close(core.p_nom[0], 15_000.0 / 1_728_000.0);
    close(core.q_nom[0], 15_000.0 / 691_200.0);
    assert_eq!(
        core.voltage_model,
        DistLoadVoltageModel::ConstantImpedance {
            v_nom: vec![7200.0]
        }
    );
    assert_eq!(n.component_locations().len(), 1);
    assert_eq!(
        n.component_locations()[0].target,
        DistLocationTarget::Transformer
    );
    assert_eq!(n.component_locations()[0].name, "x");
    assert!(
        module
            .diagnostics()
            .iter()
            .any(|d| d.code() == "READ.GLM.TRANSFORMER_EQUIVALENT")
    );
}

#[test]
fn triplex_current_uses_local_angle_and_line_line_voltage() {
    let n = read(deck()).into_value();
    let load = n
        .loads()
        .iter()
        .find(|l| l.name.contains("current_12"))
        .unwrap();
    assert_eq!(load.terminal_map, ["1", "2"]);
    assert_eq!(load.configuration, Configuration::SinglePhase);
    close(load.p_nom[0], 480.0);
    close(load.q_nom[0], 120.0);
    assert_eq!(
        load.voltage_model,
        DistLoadVoltageModel::ConstantCurrent { v_nom: vec![240.0] }
    );
    let leg2 = n
        .loads()
        .iter()
        .find(|l| l.name.ends_with("power_2"))
        .unwrap();
    assert_eq!(leg2.terminal_map, ["n", "2"]);
    assert_eq!(leg2.p_nom, [800.0]);
}

#[test]
fn native_secondary_and_rating_defaults_are_applied_on_the_phase_base() {
    let text = deck()
        .replace(" powerC_rating 15;", "")
        .replace("impedance1 .012+.0068j; impedance2 .018+.009j;", "");
    let n = read(&text).into_value();
    let t = &n.transformers()[0];
    for winding in &t.windings {
        close(winding.s_rating, 5_000.0);
    }
    close(t.windings[0].r_pct, 0.3);
    close(t.windings[1].r_pct, 0.6);
    close(t.windings[2].r_pct, 0.6);
    close(t.xsc_pct[0], 1.632);
    let n = read(&deck().replace("impedance1 .012+.0068j;", "")).into_value();
    close(n.transformers()[0].windings[1].r_pct, 1.8);
    close(n.transformers()[0].xsc_pct[0], 2.26);
}

#[test]
fn aliases_keep_source_order_and_untouched_core_default() {
    let text = deck()
        .replace(
            "impedance1 .012+.0068j;",
            "resistance1 .1; reactance1 .2; impedance1 .012+.0068j; resistance1 .02;",
        )
        .replace("shunt_impedance 1728000+691200j;", "shunt_resistance 50;")
        .replace(
            "constant_power_1 1945+487.5j;",
            "constant_power_1_real 100; constant_power_1 1945+487.5j; constant_power_1_reac 300;",
        );
    let n = read(&text).into_value();
    close(n.transformers()[0].windings[1].r_pct, 2.0);
    let core = n
        .loads()
        .iter()
        .find(|l| l.name.contains("core_loss"))
        .unwrap();
    close(core.p_nom[0], 300.0);
    close(core.q_nom[0], 15_000.0 / 999_999_999.0);
    let load = n
        .loads()
        .iter()
        .find(|l| l.name.ends_with("power_1"))
        .unwrap();
    assert_eq!(load.p_nom, [1945.0]);
    assert_eq!(load.q_nom, [300.0]);
}

#[test]
fn unsupported_connections_and_singular_leakage_fail_explicitly() {
    for (old, new) in [
        ("phases CS", "phases ABS"),
        ("powerC_rating 15", "powerA_rating 15"),
        ("impedance2 .018+.009j", "impedance2 0+.009j"),
        ("shunt_impedance 1728000+691200j", "shunt_impedance 0+10j"),
        ("object triplex_load", "object load"),
        (
            "name sink; phases CS",
            "name sink; phases CS; bustype SWING",
        ),
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
    let text = format!(
        "{}\nobject overhead_line {{ name bad; phases CN; from source; to sink; length 1; configuration z; }}\nobject line_configuration {{ name z; z33 1+1j; }}",
        deck()
    );
    assert!(
        parse(
            Source::from_memory("bad.glm", text.as_bytes()).unwrap(),
            PreprocessOptions::default()
        )
        .is_err()
    );
}

#[test]
fn pnnl_legacy_triplex_power_is_not_misrepresented_as_arras_input() {
    let text = deck()
        .replace("object triplex_load", "object triplex_node")
        .replace("constant_power_", "power_")
        .replace("constant_current_12 2-.5j;", "");
    let n = read(&text).into_value();
    assert_eq!(n.loads().len(), 3);
    let error = parse(
        Source::from_memory("case.glm", text.as_bytes()).unwrap(),
        PreprocessOptions {
            dialect: GlmDialect::Arras4_3,
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("hidden"));
}

#[test]
fn parented_triplex_loads_keep_anchor_and_collision_free_core_ids() {
    let text = deck()
        .replace(
            "object triplex_load { name sink;",
            "object triplex_node { name sink;",
        )
        .replace(
            "constant_power_1 1945+487.5j; constant_power_2 800+200j;\nconstant_current_12 2-.5j;",
            "",
        )
        + "\nobject triplex_load { name x__glm_core; parent sink; phases CS; nominal_voltage 120; constant_power_1 100+20j; }";
    let n = read(&text).into_value();
    assert_eq!(n.buses().len(), 3);
    assert_ne!(n.transformers()[0].windings[3].bus, "x__glm_core");
    assert_eq!(n.loads()[0].bus, "sink");
    let bad = text.replace("parent sink; phases CS", "parent sink; phases BS");
    assert!(
        parse(
            Source::from_memory("bad.glm", bad.as_bytes()).unwrap(),
            PreprocessOptions::default()
        )
        .is_err()
    );
}

#[test]
fn source_echo_and_fresh_center_tapped_emission_preserve_the_service() {
    let module = read(deck());
    let result = powerio_dist::emit(
        &module,
        DistTargetFormat::Gridlabd,
        Destination::memory("case.glm").unwrap(),
    )
    .unwrap();
    let EmittedOutput::Memory { artifacts } = result.output() else {
        panic!()
    };
    assert_eq!(artifacts[0].bytes(), deck().as_bytes());
    let result = emit_canonical(module.value(), Destination::memory("case.glm").unwrap()).unwrap();
    let EmittedOutput::Memory { artifacts } = result.output() else {
        panic!()
    };
    let fresh = parse(
        Source::from_memory("fresh.glm", artifacts[0].bytes()).unwrap(),
        PreprocessOptions::default(),
    )
    .unwrap()
    .into_value();
    let a = &module.value().transformers()[0];
    let b = &fresh.transformers()[0];
    assert_eq!(a.windings.len(), b.windings.len());
    for (a, b) in a.windings.iter().zip(&b.windings) {
        close(a.v_ref, b.v_ref);
        close(a.r_pct, b.r_pct);
        close(a.s_rating, b.s_rating);
        assert_eq!(a.terminal_map, b.terminal_map);
    }
    for (a, b) in a.xsc_pct.iter().zip(&b.xsc_pct) {
        close(*a, *b);
    }
    assert_eq!(
        fresh.component_locations(),
        module.value().component_locations()
    );
}

#[test]
fn a_core_with_an_extra_device_cannot_be_folded_into_the_native_transformer() {
    let mut net = read(deck()).into_value();
    let mut extra = net
        .loads()
        .iter()
        .find(|l| l.bus == net.transformers()[0].windings[3].bus)
        .unwrap()
        .clone();
    extra.name = "extra-core-device".into();
    net.loads_mut().push(extra);
    assert!(emit_canonical(&net, Destination::memory("case.glm").unwrap()).is_err());
}

#[test]
fn native_center_tapped_output_rejects_unequal_effective_leg_ratios() {
    let mut net = read(deck()).into_value();
    net.transformers_mut()[0].windings[1].tap = 1.1;
    assert!(emit_canonical(&net, Destination::memory("case.glm").unwrap()).is_err());
}

#[test]
fn current_loads_with_conflicting_points_cannot_be_grouped_silently() {
    let text = format!(
        "{}\nobject triplex_load {{ name first; parent sink; phases CS; nominal_voltage 120; constant_current_1 1-.1j; latitude -27; }}\nobject triplex_load {{ name second; parent sink; phases CS; nominal_voltage 120; constant_current_1 2-.2j; latitude -28; }}",
        deck()
    );
    let net = read(&text).into_value();
    assert!(emit_canonical(&net, Destination::memory("case.glm").unwrap()).is_err());
}
