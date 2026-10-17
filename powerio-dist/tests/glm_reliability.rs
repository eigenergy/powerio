use powerio_core::Source;
use powerio_dist::glm::{PreprocessOptions, parse};
fn deck() -> String {
    "clock { timezone UTC0; starttime '2000-01-01 0:00:00'; stoptime '2000-01-01 0:00:01'; }
module powerflow { solver_method NR; } module reliability { report_event_log false; }
object node { name source; phases ABN; bustype SWING; nominal_voltage 240; }
object node { name sink; phases ABN; nominal_voltage 240; }
object series_reactor { name line; from source; to sink; phases ABN; phase_A_impedance .1+.2j; phase_B_impedance .1+.2j; }
object fault_check { name check; check_mode ONCHANGE; strictly_radial false; grid_association true; eventgen_object events; }
object eventgen { name events; fault_type DLG-X; manual_outages \"line,2100-01-01 00:00:05,2100-01-01 00:00:30\"; }".into()
}
#[test]
fn future_outages_preserve_static_network_with_explicit_diagnostic() {
    let text = deck();
    let module = parse(
        Source::from_memory("future.glm", text.as_bytes()).unwrap(),
        PreprocessOptions::default(),
    )
    .unwrap();
    assert_eq!(module.value().lines().len(), 1);
    assert_eq!(module.value().buses().len(), 2);
    assert!(
        module
            .diagnostics()
            .iter()
            .any(|d| d.code() == "READ.GLM.CONTROL_FROZEN")
    );
}
#[test]
fn active_events_invalid_dates_and_unsupported_phases_are_not_ignored() {
    for text in [
        deck().replace("2100-01-01", "2000-01-01"),
        deck().replace("2100-01-01", "2100-02-29"),
        deck().replace("manual_outages", "target_group"),
        deck().replace("line,2100", "missing,2100"),
        deck().replace(
            "phases ABN; phase_A_impedance",
            "phases AN; phase_A_impedance",
        ),
        deck().replace("report_event_log false", "enable_subsecond_models true"),
        deck().replace(
            "stoptime '2000-01-01 0:00:01'",
            "stoptime '2100-01-01 0:00:06'",
        ),
    ] {
        assert!(
            parse(
                Source::from_memory("invalid.glm", text.as_bytes()).unwrap(),
                PreprocessOptions::default()
            )
            .is_err(),
            "{text}"
        );
    }
}

#[test]
fn swing_pq_is_only_pq_when_each_phase_reaches_the_fixed_source() {
    let text = deck().replace(
        "name sink; phases ABN;",
        "name sink; phases ABN; bustype SWING_PQ;",
    );
    let m = parse(
        Source::from_memory("connected.glm", text.as_bytes()).unwrap(),
        PreprocessOptions::default(),
    )
    .unwrap();
    assert_eq!(m.value().sources().len(), 1);
    let isolated = text.replace(
        "phases ABN; phase_A_impedance",
        "phases AN; phase_A_impedance",
    );
    assert!(
        parse(
            Source::from_memory("isolated.glm", isolated.as_bytes()).unwrap(),
            PreprocessOptions::default()
        )
        .is_err()
    );
}
