use powerio_core::Source;
use powerio_dist::{
    IbrPrimeMover,
    glm::{GlmDialect, PreprocessOptions, parse},
};
fn deck() -> String {
    "module powerflow { solver_method NR; } module generators;
object meter { name source; phases ABCN; bustype SWING; nominal_voltage 240; }
object inverter { name inv; parent source; phases ABCN; inverter_type FOUR_QUADRANT;
four_quadrant_control_mode CONSTANT_PF; P_Out -2500; power_factor -1; inverter_efficiency .975;
sense_object source; charge_on_threshold -5000; rated_power 83333.333;
object battery { name bat; use_internal_battery_model true; battery_type LI_ION;
nominal_voltage 48; battery_capacity 500000; state_of_charge 1; round_trip_efficiency .86;
rated_power 250000; latitude 0; longitude 150; } }"
        .into()
}
#[test]
fn internal_battery_constant_pf_does_not_use_scheduled_power() {
    for dialect in [GlmDialect::Pnnl5_3, GlmDialect::Arras4_3] {
        let input = deck();
        let module = parse(
            Source::from_memory("battery.glm", input.as_bytes()).unwrap(),
            PreprocessOptions {
                dialect,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(module.value().ibrs().len(), 3);
        for ibr in module.value().ibrs() {
            assert_eq!(ibr.prime_mover, IbrPrimeMover::Battery);
            assert_eq!(ibr.p_min, Some(vec![0.0]));
            assert_eq!(ibr.p_max, ibr.p_min);
            assert_eq!(ibr.q_min, Some(vec![0.0]));
            assert_eq!(ibr.q_max, ibr.q_min);
            assert_eq!(ibr.p_avail, None);
        }
        assert_eq!(module.value().component_locations().len(), 3);
        assert!(
            module
                .diagnostics()
                .iter()
                .any(|d| d.code() == "READ.GLM.BATTERY_IDLE")
        );
    }
}
#[test]
fn unqualified_battery_states_and_active_dispatch_are_refused() {
    for input in [
        deck().replace("CONSTANT_PF", "CONSTANT_PQ"),
        deck().replace(
            "internal_battery_model true",
            "internal_battery_model false",
        ),
        deck().replace("state_of_charge 1", "state_of_charge -1"),
        deck().replace("sense_object source", "sense_object missing"),
        deck().replace("LI_ION", "LEAD_ACID"),
    ] {
        assert!(
            parse(
                Source::from_memory("battery.glm", input.as_bytes()).unwrap(),
                PreprocessOptions::default()
            )
            .is_err()
        );
    }
}
