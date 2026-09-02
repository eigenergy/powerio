//! The PowerFactory DGS reader against the PowSybl reference fixtures and the
//! MATPOWER case14 that ieee14.dgs was exported from.

mod helpers;

use std::collections::BTreeMap;

use helpers::{Parsed, parse_file};
use powerio_tx::network::{BusId, BusType, SourceFormat, TransformerControlMode};

fn fixture(name: &str) -> String {
    format!("{}/../tests/data/powerfactory/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn parse_fixture(name: &str) -> Parsed {
    parse_file(fixture(name), None).unwrap()
}

/// The MATPOWER bus number a PowerFactory terminal label such as `1 Bus 1`
/// opens with.
fn label_number(name: &str) -> usize {
    name.split_whitespace()
        .next()
        .and_then(|token| token.parse().ok())
        .unwrap()
}

fn codes(parsed: &Parsed) -> Vec<String> {
    parsed
        .diagnostics
        .iter()
        .map(|d| d.code().to_owned())
        .collect()
}

#[test]
fn ieee14_matches_the_matpower_case_where_the_export_carries_the_data() {
    let parsed = parse_fixture("ieee14.dgs");
    let net = &parsed.network;
    let reference = parse_file(
        format!("{}/../tests/data/case14.m", env!("CARGO_MANIFEST_DIR")),
        None,
    )
    .unwrap()
    .network;
    assert_eq!(net.source_format(), SourceFormat::Dgs);
    assert_eq!(net.name(), "1 IEEE14");
    assert_eq!(net.base_mva(), 100.0);
    assert_eq!(net.base_frequency(), 60.0);
    assert_eq!(net.buses().len(), 14);
    assert_eq!(net.branches().len(), 20);
    assert_eq!(net.generators().len(), 5);
    assert_eq!(net.loads().len(), 11);
    assert_eq!(net.shunts().len(), 1);

    // Bus ids are the ElmTerm object ids; the label carries the case number.
    let number: BTreeMap<BusId, usize> = net
        .buses()
        .iter()
        .map(|bus| (bus.id, label_number(bus.name.as_deref().unwrap())))
        .collect();
    let mut numbers = number.values().copied().collect::<Vec<_>>();
    numbers.sort_unstable();
    assert_eq!(numbers, (1..=14).collect::<Vec<_>>());
    for bus in net.buses() {
        assert_eq!(bus.base_kv, 138.0);
        assert_eq!(bus.vmax, 1.1);
        assert_eq!(bus.vmin, 0.9);
    }
    let kind = |n: usize| {
        net.buses()
            .iter()
            .find(|bus| number[&bus.id] == n)
            .unwrap()
            .kind
    };
    assert_eq!(kind(1), BusType::Ref);
    for pv in [2, 3, 6, 8] {
        assert_eq!(kind(pv), BusType::Pv, "bus {pv}");
    }
    assert_eq!(kind(4), BusType::Pq);

    // Every branch of case14 appears with its impedance and charging.
    let by_ends = |from: usize, to: usize| {
        net.branches()
            .iter()
            .find(|b| number[&b.from] == from && number[&b.to] == to)
            .unwrap()
    };
    for branch in reference.branches() {
        let mine = by_ends(branch.from.0, branch.to.0);
        assert!(
            (mine.r - branch.r).abs() < 2e-6,
            "r {}-{}: {} vs {}",
            branch.from,
            branch.to,
            mine.r,
            branch.r
        );
        assert!(
            (mine.x - branch.x).abs() < 2e-6,
            "x {}-{}: {} vs {}",
            branch.from,
            branch.to,
            mine.x,
            branch.x
        );
        assert!(
            (mine.b - branch.b).abs() < 2e-6,
            "b {}-{}: {} vs {}",
            branch.from,
            branch.to,
            mine.b,
            branch.b
        );
        assert!(
            (mine.calc_effective_tap() - branch.calc_effective_tap()).abs() < 2e-4,
            "tap {}-{}: {} vs {}",
            branch.from,
            branch.to,
            mine.tap,
            branch.tap
        );
        assert_eq!(mine.shift, 0.0);
        assert!(mine.in_service);
    }
    // The three transformers carry the 100 MVA rating; the lines carry the
    // 0.41837 kA rating at 138 kV.
    assert_eq!(by_ends(4, 7).rate_a, 100.0);
    assert!((by_ends(1, 2).rate_a - 3f64.sqrt() * 138.0 * 0.41837).abs() < 1e-6);

    // Loads and the capacitor bank agree with the case; the machines carry
    // the export's dispatch, which differs from the solved case state.
    for load in reference.loads() {
        let mine = net
            .loads()
            .iter()
            .find(|l| number[&l.bus] == load.bus.0)
            .unwrap();
        assert!((mine.p - load.p).abs() < 1e-9, "p at {}", load.bus);
        assert!((mine.q - load.q).abs() < 1e-9, "q at {}", load.bus);
        assert!(mine.in_service);
    }
    let shunt = &net.shunts()[0];
    assert_eq!(number[&shunt.bus], 9);
    assert!((shunt.b - 19.0).abs() < 1e-3, "{}", shunt.b);
    assert_eq!(shunt.g, 0.0);
    assert_eq!(shunt.section_count, Some(1));
    for generator in reference.generators() {
        let mine = net
            .generators()
            .iter()
            .find(|g| number[&g.bus] == generator.bus.0)
            .unwrap();
        assert!((mine.vg - generator.vg).abs() < 1e-9, "vg at {}", generator.bus);
        // The export states no reactive range for the slack machine.
        if generator.bus.0 == 1 {
            assert_eq!((mine.qmin, mine.qmax), (0.0, 0.0));
        } else {
            assert!((mine.qmin - generator.qmin).abs() < 1e-3, "qmin at {}", generator.bus);
            assert!((mine.qmax - generator.qmax).abs() < 1e-3, "qmax at {}", generator.bus);
        }
        assert!(mine.voltage_regulation_on);
        assert!(mine.in_service);
    }
    let bus2 = net
        .generators()
        .iter()
        .find(|g| number[&g.bus] == 2)
        .unwrap();
    assert_eq!(bus2.pg, 40.0);
    assert_eq!(bus2.mbase, 60.0);
    assert_eq!(bus2.pmax, 10000.0);
    assert_eq!(bus2.uid.as_deref(), Some("sym_2_1"));
    assert!(
        codes(&parsed)
            .iter()
            .all(|code| code.starts_with("READ.DGS.")),
        "{:?}",
        parsed.render_diagnostics()
    );
}

#[test]
fn findings_carry_the_row_they_come_from() {
    let parsed = parse_fixture("Tower.dgs");
    let coupling = parsed
        .diagnostics
        .iter()
        .find(|d| d.code() == "READ.DGS.VALUE_COLLAPSED" && d.message().contains("couples"))
        .expect("the tower reports its dropped circuit coupling");
    let span = &coupling.spans()[0];
    let text = std::fs::read_to_string(fixture("Tower.dgs")).unwrap();
    let row = &text[span.byte_start() as usize..span.byte_end() as usize];
    assert!(row.starts_with("12;tow_2_3;"), "{row}");
}

#[test]
fn tower_circuits_take_their_diagonal_sequence_impedance() {
    let parsed = parse_fixture("Tower.dgs");
    let net = &parsed.network;
    assert_eq!(net.buses().len(), 3);
    assert_eq!(net.branches().len(), 4);
    let tower_lines: Vec<_> = net
        .branches()
        .iter()
        .filter(|b| b.extras.contains_key("dgs.tower"))
        .collect();
    assert_eq!(tower_lines.len(), 2);
    // R_c1 diagonal 0.05419216181657 ohm per km over 52 km at 400 kV.
    let zbase = 400.0 * 400.0 / 100.0;
    for line in &tower_lines {
        assert!((line.r - 0.054_192_161_816_57 * 52.0 / zbase).abs() < 1e-9);
        assert!((line.x - 0.415_558_072_434_22 * 52.0 / zbase).abs() < 1e-9);
    }
    // Decimal commas in the export read as decimal points.
    let load = &net.loads()[0];
    assert_eq!(load.p, 50.0);
    assert_eq!(load.q, 25.0);
}

#[test]
fn hvdc_converters_become_one_link_and_no_dc_buses() {
    let parsed = parse_fixture("Hvdc.dgs");
    let net = &parsed.network;
    assert_eq!(net.buses().len(), 3, "the DC terminals are not buses");
    assert_eq!(net.branches().len(), 2, "the DC lines are not branches");
    assert_eq!(net.hvdc().len(), 1);
    let link = &net.hvdc()[0];
    assert_eq!(link.pf, 600.0);
    assert!((link.loss0 - 20.0).abs() < 1e-9);
    assert_eq!(link.pmax, 900.0);
    assert_eq!(link.nominal_voltage_kv, Some(320.0));
    // Two parallel 50 km DC lines at 0.101465 ohm per km.
    assert!((link.resistance_ohm.unwrap() - 0.101_465 * 50.0 / 2.0).abs() < 1e-6);
    assert_ne!(link.from, link.to);
}

#[test]
fn closed_couplers_join_terminals_and_solved_voltages_are_kept() {
    let parsed = parse_fixture("Switches.dgs");
    let net = &parsed.network;
    assert_eq!(net.buses().len(), 3);
    let joined = net
        .buses()
        .iter()
        .find(|bus| bus.extras.contains_key("dgs.terminals"))
        .unwrap();
    assert_eq!(joined.base_kv, 20.0);
    assert!((joined.vm - 1.040_265_086_407_43).abs() < 1e-12);
    assert!((joined.va + 2.093_833_991_100_34).abs() < 1e-12);
    assert!(net.switches().is_empty(), "a closed coupler is not a switch record");
    let load = &net.loads()[0];
    assert_eq!(load.bus, joined.id);
}

#[test]
fn external_grid_is_the_slack_generator() {
    let parsed = parse_fixture("ExternalGrid.dgs");
    let net = &parsed.network;
    let grid = net
        .generators()
        .iter()
        .find(|g| g.uid.as_deref() == Some("external_grid_3_1"))
        .unwrap();
    assert_eq!(grid.pg, 10.0);
    assert_eq!(grid.qg, 5.0);
    assert_eq!(grid.vg, 1.1);
    assert!(grid.voltage_regulation_on);
    let bus = net.buses().iter().find(|bus| bus.id == grid.bus).unwrap();
    assert_eq!(bus.kind, BusType::Ref);
}

#[test]
fn capability_curve_collapses_to_its_widest_range() {
    let parsed = parse_fixture("CapabilityCurve.dgs");
    let generator = parsed
        .network
        .generators()
        .iter()
        .find(|g| g.qmin < -100.0)
        .expect("the curved machine");
    assert_eq!(generator.qmin, -250.0);
    assert!((generator.qmax - 353.2732).abs() < 1e-9);
    assert!(codes(&parsed).contains(&"READ.DGS.VALUE_COLLAPSED".to_owned()));
}

#[test]
fn three_winding_transformer_keeps_its_windings_and_control() {
    let parsed = parse_fixture("ThreeWindingsTransformerVoltageControl.dgs");
    let net = &parsed.network;
    assert_eq!(net.transformers_3w().len(), 8);
    let by_name = |name: &str| {
        net.transformers_3w()
            .iter()
            .find(|t| t.name.as_deref() == Some(name))
            .unwrap()
    };
    let local = by_name("T3w_2_2M_2L_1");
    assert_eq!(local.windings[0].rate_a, 101.0);
    assert_eq!(local.windings[1].rate_a, 201.0);
    assert_eq!(local.windings[2].rate_a, 301.0);
    assert!(local.in_service);
    let controlled = local
        .windings
        .iter()
        .filter_map(|w| w.control.as_ref())
        .collect::<Vec<_>>();
    assert_eq!(controlled.len(), 1, "one winding regulates");
    assert_eq!(controlled[0].mode, TransformerControlMode::Voltage);
    assert!(controlled[0].enabled);
    assert_eq!(controlled[0].band_min, 1.005);
    assert_eq!(controlled[0].band_max, 1.015);
    assert_eq!(controlled[0].controlled_bus, None);
    // Remote regulation names terminal 53's bus.
    let remote = by_name("T3w_5_5M_5L_1");
    let control = remote.windings[0].control.as_ref().unwrap();
    assert_eq!(control.controlled_bus, Some(BusId(53)));
    // A stated tap position beyond the type's range is clamped.
    assert!(codes(&parsed).contains(&"READ.DGS.VALUE_SUBSTITUTED".to_owned()));
}

#[test]
fn explicit_tap_tables_supply_the_phase_shift() {
    let parsed = parse_fixture("Transformer-Phase-with-mTaps.dgs");
    let transformer = parsed
        .network
        .branches()
        .iter()
        .find(|b| b.name.as_deref() == Some("trf_2_3_1"))
        .unwrap();
    // Tap position 0 with `ntpmn` -1 reads the second table row: angle -0.88.
    assert_eq!(transformer.shift, -0.88);
    assert_eq!(transformer.extras["dgs.tap_position"], 0);
    assert!((transformer.tap - 1.0).abs() < 1e-12);
}

#[test]
fn medium_voltage_loads_state_every_input_mode() {
    let parsed = parse_fixture("MediumVoltageLoad.dgs");
    let net = &parsed.network;
    let load = |name: &str| {
        net.loads()
            .iter()
            .find(|l| l.uid.as_deref() == Some(name))
            .unwrap()
    };
    assert_eq!((load("lod_3_1").p, load("lod_3_1").q), (10.0, 5.0));
    let from_p_s = load("lod_31_1");
    assert_eq!(from_p_s.p, 10.0);
    assert!((from_p_s.q - (15f64.powi(2) - 10f64.powi(2)).sqrt()).abs() < 1e-9);
    let from_q_s = load("lod_32_1");
    assert_eq!(from_q_s.q, 5.0);
    assert!((from_q_s.p - (15f64.powi(2) - 5f64.powi(2)).sqrt()).abs() < 1e-9);
    let from_p_cos = load("lod_33_1");
    assert!((from_p_cos.q - 10.0 * (1.0 - 0.98f64 * 0.98).sqrt() / 0.98).abs() < 1e-9);
    let from_s_cos = load("lod_35_1");
    assert!((from_s_cos.p - 15.0 * 0.98).abs() < 1e-9);
    let generation = net
        .generators()
        .iter()
        .find(|g| g.uid.as_deref() == Some("lod_33_1-G"))
        .expect("the load's generation");
    assert!((generation.pg - 0.02).abs() < 1e-9);
    assert!(!generation.voltage_regulation_on);
}

#[test]
fn robustness_export_reads_with_reported_gaps() {
    let parsed = parse_fixture("robustness.dgs");
    let net = &parsed.network;
    assert_eq!(net.buses().len(), 2);
    // Eight loads are declared; one has no cubicle and is dropped.
    assert_eq!(net.loads().len(), 7);
    assert!(
        codes(&parsed).contains(&"READ.DGS.RECORD_UNMAPPED".to_owned()),
        "{:?}",
        parsed.render_diagnostics()
    );
    let dropped = parsed
        .diagnostics
        .iter()
        .filter(|d| d.code() == "READ.DGS.RECORD_UNMAPPED")
        .count();
    assert_eq!(dropped, 2, "{:?}", parsed.render_diagnostics());
    assert!(
        codes(&parsed).contains(&"READ.DGS.VALUE_DEFAULTED".to_owned()),
        "{:?}",
        parsed.render_diagnostics()
    );
    // No machine declares the slack; the one regulating machine's bus is it.
    assert_eq!(
        net.buses().iter().filter(|b| b.kind == BusType::Ref).count(),
        1
    );
}

#[test]
fn the_same_format_write_returns_the_retained_source() {
    let path = fixture("ieee14.dgs");
    let source = powerio_core::Source::open(&path).unwrap();
    let module = powerio_tx::parse(source).unwrap();
    let result = powerio_tx::emit(
        &module,
        powerio_tx::TargetFormat::Dgs,
        powerio_core::Destination::memory("copy.dgs").unwrap(),
    )
    .unwrap();
    let powerio_core::EmittedOutput::Memory { mut artifacts } = result.into_output() else {
        panic!("memory destination")
    };
    assert_eq!(
        artifacts.pop().unwrap().into_bytes(),
        std::fs::read(&path).unwrap()
    );
}

#[test]
fn a_project_file_is_refused_with_dgs_guidance() {
    let source =
        powerio_core::Source::from_memory("project.pfd", vec![0x47, 0x50, 0x0c, 0x2a]).unwrap();
    let error = powerio_tx::parse(source).unwrap_err().to_string();
    assert!(error.contains("encrypted"), "{error}");
    assert!(error.contains("DGS"), "{error}");
}
