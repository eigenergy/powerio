use powerio_core::{Destination, PioModule, Source};
use powerio_dist::{DistTargetFormat, audit_electrical_readiness};

const BASE: &str = "New Circuit.c basekv=12.47\nNew Transformer.t phases=3 windings=2 buses=[sourcebus.1.2.3.0 secondary.1.2.3.0] conns=[delta wye] kvs=[12.47 .48] kvas=[1000 1000] xhl=5\n";

#[test]
fn whole_terminal_opens_phase_conductors_and_preserves_the_neutral() {
    let text = format!("{BASE}Open Transformer.t 2\n");
    let module =
        powerio_dist::parse(Source::from_memory("open.dss", text.as_bytes()).unwrap()).unwrap();
    let net = module.value();
    assert!(audit_electrical_readiness(net).is_ready());
    assert_eq!(net.transformers().len(), 1);
    assert_eq!(net.shunts().len(), 2);
    assert_eq!(net.switches().as_slice(), []);
    let transformer = &net.transformers()[0];
    assert_eq!(transformer.windings[0].bus, "sourcebus");
    assert_ne!(transformer.windings[1].bus, "secondary");
    assert_eq!(
        transformer.extras["dss_open_transformer_ports"][1]["closed"],
        serde_json::json!([false, false, false, true])
    );
    let ground = net
        .buses()
        .iter()
        .find(|b| b.id == transformer.windings[1].bus)
        .unwrap();
    assert!(
        ground
            .grounded
            .contains(transformer.windings[1].terminal_map.last().unwrap())
    );
    for shunt in net.shunts() {
        assert_eq!(shunt.g.len(), 4);
        assert_eq!(shunt.g[0][0].to_bits(), 1e-12_f64.to_bits());
    }
    powerio_dist::emit(
        &PioModule::new(net.clone()),
        DistTargetFormat::Dss,
        Destination::memory("out").unwrap(),
    )
    .unwrap();
}

#[test]
fn reclosed_ports_restore_the_original_transformer() {
    let text = format!("{BASE}Open Transformer.t 2\nClose Transformer.t 2\n");
    let net = powerio_dist::parse(Source::from_memory("closed.dss", text.as_bytes()).unwrap())
        .unwrap()
        .into_value();
    assert_eq!(net.transformers()[0].windings[1].bus, "secondary");
    assert_eq!(net.shunts().as_slice(), []);
    assert!(
        !net.transformers()[0]
            .extras
            .contains_key("dss_open_transformer_ports")
    );
}

#[test]
fn terminal_layout_changes_and_invalid_indices_fail_explicitly() {
    for commands in [
        "Open Transformer.t 3",
        "Open Transformer.t 2 5",
        "Open Transformer.t 2\nEdit Transformer.t phases=1",
        "Open Transformer.t 2\nEdit Transformer.t windings=3",
    ] {
        let text = format!("{BASE}{commands}\n");
        assert!(
            powerio_dist::parse(Source::from_memory("bad.dss", text.as_bytes()).unwrap()).is_err(),
            "{commands}"
        );
    }
}

#[test]
fn unconverted_enabled_faults_block_electrical_emission() {
    for enabled in ["yes", "no"] {
        let text = format!(
            "New Circuit.c\nNew Fault.f bus1=sourcebus phases=3 r=.0001 enabled={enabled}\n"
        );
        let net = powerio_dist::parse(Source::from_memory("fault.dss", text.as_bytes()).unwrap())
            .unwrap()
            .into_value();
        assert_eq!(audit_electrical_readiness(&net).is_ready(), enabled == "no");
        if enabled == "yes" {
            assert!(
                powerio_dist::emit(
                    &PioModule::new(net),
                    DistTargetFormat::Dss,
                    Destination::memory("out").unwrap()
                )
                .is_err()
            );
        }
    }
}

#[test]
#[ignore = "external native open-transformer cases; exports actual typed winding edits"]
fn export_edited_open_transformer_windings() {
    let root = std::path::PathBuf::from(std::env::var("POWERIO_OPEN_TRANSFORMER_CASES").unwrap());
    let cases: Vec<String> =
        serde_json::from_slice(&std::fs::read(root.join("cases.json")).unwrap()).unwrap();
    for case in cases {
        let folder = root.join(case);
        let mut net = powerio_dist::parse(Source::open(folder.join("native.dss")).unwrap())
            .unwrap()
            .into_value();
        assert_eq!(net.transformers().len(), 1);
        let transformer = &mut net.transformers_mut()[0];
        for winding in &mut transformer.windings {
            winding.r_pct *= 1.17;
        }
        transformer.windings[0].tap *= 1.03;
        powerio_dist::emit(
            &PioModule::new(net),
            DistTargetFormat::Dss,
            Destination::path(folder.join("typed-edited")),
        )
        .unwrap();
    }
}

#[test]
fn two_phase_open_delta_keeps_its_third_hot_terminal() {
    let text = "New Circuit.c\nNew Transformer.t phases=2 windings=2 buses=[sourcebus.1.2.3 secondary.1.2.3] conns=[delta delta] kvs=[12.47 .48] kvas=[1000 1000] xhl=5\nOpen Transformer.t 2 1\n";
    let net = powerio_dist::parse(Source::from_memory("two-phase.dss", text.as_bytes()).unwrap())
        .unwrap()
        .into_value();
    let transformer = &net.transformers()[0];
    assert_eq!(transformer.windings[0].terminal_map, ["1", "2", "3"]);
    assert_eq!(transformer.windings[1].terminal_map.len(), 3);
    assert_eq!(net.switches().len(), 2);
    assert!(net.switches().iter().any(|s| s.terminal_map_from == ["3"]));
}

#[test]
fn enabled_fault_state_commands_cannot_hide_behind_disabled_properties() {
    for target in ["Fault.f", "@target"] {
        let text = format!(
            "New Circuit.c\nNew Fault.f bus1=sourcebus enabled=no\nVar @target=Fault.f\nEnable {target}\n"
        );
        let error =
            powerio_dist::parse(Source::from_memory("fault-state.dss", text.as_bytes()).unwrap())
                .unwrap_err();
        assert!(error.to_string().contains("static electrical snapshot"));
    }
}
