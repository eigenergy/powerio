//! UCTE-DEF reader and writer: the synthetic fixture parses to the expected
//! counts and values, the vendored PowSybl fixtures parse and convert, same
//! format emission returns the source text, and fresh output reads back to
//! the same network.
//!
//! The fixture values are exact decimal literals, so the value assertions
//! compare them exactly.
#![allow(clippy::float_cmp, clippy::too_many_lines)]

mod helpers;
#[allow(unused_imports)]
use helpers::*;

use std::path::{Path, PathBuf};

use powerio_core::{FormatId, Source};
use powerio_tx::network::{BalancedNetwork, BusId, BusType, GeneratorEnergySource};
use powerio_tx::{TargetFormat, TransformerControlMode, parse_target_format};

fn data(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data/ucte")
        .join(name)
}

fn parse_fixture(name: &str) -> powerio_core::PioModule<BalancedNetwork> {
    powerio_tx::parse(Source::open(data(name)).unwrap()).unwrap()
}

fn parse_ucte_text(name: &str, text: &str) -> powerio_core::PioModule<BalancedNetwork> {
    let source = Source::from_memory(name, text.as_bytes().to_vec())
        .unwrap()
        .with_format(FormatId::new("ucte").unwrap());
    powerio_tx::parse(source).unwrap()
}

fn messages(module: &powerio_core::PioModule<BalancedNetwork>) -> Vec<String> {
    powerio_tx::diagnostics::render_diagnostics(&module.diagnostics)
}

fn bus_by_name<'a>(net: &'a BalancedNetwork, name: &str) -> &'a powerio_tx::network::Bus {
    net.buses()
        .iter()
        .find(|bus| bus.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("bus {name:?}"))
}

fn approx(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * b.abs().max(1.0)
}

#[test]
fn aliases_and_extensions_route_to_ucte() {
    for alias in ["ucte", "UCTE", "uct", "ucte-def", "UCTE_DEF"] {
        assert_eq!(
            parse_target_format(alias),
            Some(TargetFormat::Ucte),
            "{alias}"
        );
    }
    assert_eq!(TargetFormat::Ucte.extension(), "uct");
    assert_eq!(TargetFormat::Ucte.token(), "ucte");
    let module = parse_fixture("synthetic_all_blocks.uct");
    assert_eq!(
        module.value().source_format(),
        powerio_tx::network::SourceFormat::Ucte
    );
    assert_eq!(
        module.source().unwrap().format().map(FormatId::as_str),
        Some("ucte")
    );
}

#[test]
fn the_synthetic_case_parses_to_the_expected_counts_and_values() {
    let module = parse_fixture("synthetic_all_blocks.uct");
    let net = &module.value();
    assert_eq!(net.base_mva(), 100.0);
    assert_eq!(net.base_frequency(), 50.0);
    assert_eq!(net.buses().len(), 6);
    assert_eq!(net.loads().len(), 3);
    assert_eq!(net.generators().len(), 2);
    assert_eq!(net.branches().len(), 7);
    assert_eq!(net.switches().len(), 1);
    assert_eq!(net.areas().len(), 3);

    // Nodes: file order ids, the code as the name, the level as the base.
    let generator_bus = bus_by_name(net, "FGEN__11");
    assert_eq!(generator_bus.id, BusId(1));
    assert_eq!(generator_bus.kind, BusType::Ref);
    assert_eq!(generator_bus.base_kv, 380.0);
    assert!(approx(generator_bus.vm, 410.0 / 380.0));
    assert_eq!(generator_bus.area, 1);
    assert_eq!(
        generator_bus.extras["ucte_geographical_name"],
        serde_json::json!("GEN 400")
    );
    assert_eq!(
        generator_bus.extras["ucte_power_plant_type"],
        serde_json::json!("N")
    );
    assert_eq!(
        generator_bus.extras["ucte_primary_control_static"],
        serde_json::json!(5.0)
    );
    assert_eq!(
        generator_bus.extras["ucte_short_circuit_power"],
        serde_json::json!(10000.0)
    );
    let medium_voltage = bus_by_name(net, "FMV___21");
    assert_eq!(medium_voltage.kind, BusType::Pv);
    assert_eq!(medium_voltage.base_kv, 220.0);
    let cross_border = bus_by_name(net, "XFRBE_11");
    assert_eq!(cross_border.area, 3);
    assert_eq!(
        cross_border.extras["ucte_node_status"],
        serde_json::json!(1)
    );
    let areas: Vec<(&str, &str)> = net
        .areas()
        .iter()
        .map(|a| (a.name.as_deref().unwrap(), a.area_type.as_deref().unwrap()))
        .collect();
    assert_eq!(
        areas,
        [
            ("FR", "ControlArea"),
            ("BE", "ControlArea"),
            ("XX", "CrossBorder")
        ]
    );

    // Generation: UCTE signs negated, limits swapped into min/max order.
    let slack = net.generators().iter().find(|g| g.bus == BusId(1)).unwrap();
    assert_eq!((slack.pg, slack.qg), (300.0, 50.0));
    assert_eq!((slack.pmin, slack.pmax), (100.0, 400.0));
    assert_eq!((slack.qmin, slack.qmax), (-150.0, 150.0));
    assert!(approx(slack.vg, 410.0 / 380.0));
    assert!(slack.voltage_regulation_on);
    assert_eq!(slack.energy_source, GeneratorEnergySource::Nuclear);
    let hydro = net
        .generators()
        .iter()
        .find(|g| g.bus == medium_voltage.id)
        .unwrap();
    assert_eq!(hydro.energy_source, GeneratorEnergySource::Hydro);
    assert_eq!((hydro.pg, hydro.pmin, hydro.pmax), (80.0, 20.0, 120.0));
    assert_eq!((hydro.qmin, hydro.qmax), (-60.0, 60.0));
    let load = net.loads().iter().find(|l| l.bus == BusId(2)).unwrap();
    assert_eq!((load.p, load.q), (250.0, 80.0));

    // Lines: ohm and microsiemens on the level base, ampere to MVA.
    let zbase = 380.0 * 380.0 / 100.0;
    let line = &net.branches()[0];
    assert_eq!((line.from, line.to), (BusId(1), BusId(2)));
    assert!(approx(line.r, 2.5 / zbase));
    assert!(approx(line.x, 25.0 / zbase));
    assert!(approx(line.b, 300.0e-6 * zbase));
    assert!(approx(line.rate_a, 3f64.sqrt() * 380.0 * 1200.0 / 1000.0));
    assert_eq!(line.current_ratings.unwrap().c_rating_a, 1200.0);
    assert_eq!(line.name.as_deref(), Some("GEN-LOAD 1"));
    assert_eq!(line.tap, 0.0);
    assert!(line.in_service);
    assert!(!net.branches()[1].in_service);
    let equivalent = &net.branches()[4];
    assert_eq!(
        equivalent.extras["ucte_equivalent"],
        serde_json::json!(true)
    );
    assert!(approx(equivalent.x, 0.05 / zbase));
    let coupler = &net.switches()[0];
    assert_eq!(
        (coupler.from, coupler.to, coupler.closed),
        (BusId(2), BusId(3), true)
    );
    assert_eq!(coupler.current_rating, Some(3000.0));

    // Transformers: from the regulated winding, impedance on the node 1 base.
    let ratio_transformer = &net.branches()[5];
    assert_eq!(
        (ratio_transformer.from, ratio_transformer.to),
        (BusId(2), BusId(4))
    );
    let zbase_220 = 220.0 * 220.0 / 100.0;
    assert!(approx(ratio_transformer.r, 0.5 / zbase_220));
    assert!(approx(ratio_transformer.x, 20.0 / zbase_220));
    let nominal_tap = (400.0 / 380.0) / (231.0 / 220.0);
    assert!(approx(ratio_transformer.tap, nominal_tap * 1.045));
    assert_eq!(ratio_transformer.shift, 0.0);
    let charging = ratio_transformer.charging.unwrap();
    assert!(approx(charging.b_fr, -3.0e-6 * zbase_220));
    assert!(approx(charging.g_fr, 1.0e-6 * zbase_220));
    assert!(approx(
        ratio_transformer.rate_a,
        3f64.sqrt() * 220.0 * 500.0 / 1000.0
    ));
    let control = ratio_transformer.control.as_ref().unwrap();
    assert_eq!(control.mode, TransformerControlMode::Voltage);
    assert!(control.enabled);
    assert_eq!(control.controlled_bus, Some(BusId(2)));
    assert!(approx(control.band_min, 405.0 / 380.0));
    assert_eq!(control.ntp, 25);
    assert_eq!(control.mva_base, 300.0);
    assert_eq!(
        ratio_transformer.extras["ucte_phase_regulation"],
        serde_json::json!({"du": 1.5, "n": 12, "np": 3, "u": 405.0})
    );
    assert!(ratio_transformer.extras["ucte_special_description"].is_array());
    // A 400 kV rated winding on the 380 kV level is an off nominal ratio.
    let phase_shifter = &net.branches()[6];
    assert_eq!((phase_shifter.from, phase_shifter.to), (BusId(4), BusId(3)));
    assert!(approx(phase_shifter.tap, (220.0 / 220.0) / (400.0 / 380.0)));
    let expected_shift = 2.0 * (-0.02f64).atan2(1.0).to_degrees();
    assert!(approx(phase_shifter.shift, expected_shift));
    let control = phase_shifter.control.as_ref().unwrap();
    assert_eq!(control.mode, TransformerControlMode::ActiveFlow);
    assert!(!control.enabled);
    assert_eq!(control.band_min, 120.0);

    // The retained blocks and the reactance floor are reported, with spans.
    let rendered = messages(&module);
    assert!(
        rendered
            .iter()
            .any(|m| m.starts_with("READ.UCTE.RETAINED_SOURCE_ONLY")
                && m.contains("##TT block: 1 special transformer description")),
        "{rendered:#?}"
    );
    assert!(
        rendered
            .iter()
            .any(|m| m.starts_with("READ.UCTE.RETAINED_SOURCE_ONLY")
                && m.contains("##E block: 1 scheduled exchange record(s) (FR-BE 150 MW)")),
        "{rendered:#?}"
    );
    assert!(
        rendered
            .iter()
            .any(|m| m.starts_with("READ.UCTE.VALUE_SUBSTITUTED: line 20:")
                && m.contains("0.05 ohm floor")),
        "{rendered:#?}"
    );
    let floor = module
        .diagnostics
        .iter()
        .find(|d| d.message().contains("0.05 ohm floor"))
        .unwrap();
    let span = &floor.spans()[0];
    let text = std::fs::read_to_string(data("synthetic_all_blocks.uct")).unwrap();
    let start = usize::try_from(span.byte_start()).unwrap();
    let end = usize::try_from(span.byte_end()).unwrap();
    assert!(
        text[start..end].starts_with("FLOAD_12 FGEN__11 1 1"),
        "{:?}",
        &text[start..end]
    );
    assert!(
        rendered
            .iter()
            .any(|m| m.starts_with("READ.UCTE.VALUE_DEFAULTED") && m.contains("100 MVA")),
        "{rendered:#?}"
    );
}

#[test]
fn the_2003_revision_reads_with_the_same_layout() {
    let text = std::fs::read_to_string(data("synthetic_all_blocks.uct")).unwrap();
    let older = text.replace("##C 2007.05.01", "##C 2003.09.01");
    let module = parse_ucte_text("older.uct", &older);
    assert_eq!(module.value().buses().len(), 6);
    assert_eq!(module.value().branches().len(), 7);
    assert!(
        messages(&module)
            .iter()
            .any(|m| m.contains("UCTE-DEF 2003.09.01"))
    );
}

#[test]
fn malformed_input_is_refused_with_the_line() {
    let refuse = |text: &str| {
        let source = Source::from_memory("bad.uct", text.as_bytes().to_vec())
            .unwrap()
            .with_format(FormatId::new("ucte").unwrap());
        powerio_tx::parse(source).unwrap_err().to_string()
    };
    assert!(refuse("##C 2010.01.01\n##N\n").contains("revision \"2010.01.01\" is not supported"));
    assert!(refuse("##N\n##ZFR\n").contains("first block must be a ##C comment block"));
    let no_zone =
        "##C 2007.05.01\n##N\nFFNHV111 FNHV1__ HV1- 0 0        0.00000 0.00000 0.00000 0.00000\n";
    assert!(refuse(no_zone).contains("line 3: a node must be defined under a ##Z country line"));
    let unknown_node = "##C 2007.05.01\n##N\n##ZFR\nFFNHV111 FNHV1__ HV1- 0 0        0.00000 0.00000 0.00000 0.00000\n##L\nFFNHV111 FFNHV211 1 0 3.0035 32.995 385.9970   1519\n";
    assert!(refuse(unknown_node).contains("line 6: node \"FFNHV211\" is not declared"));
    let two_levels = "##C 2007.05.01\n##N\n##ZFR\nFFNHV111 FNHV1__ HV1- 0 0        0.00000 0.00000 0.00000 0.00000\nFFNHV221 FNHV2__ HV2- 0 0        0.00000 0.00000 0.00000 0.00000\n##L\nFFNHV111 FFNHV221 1 0 3.0035 32.995 385.9970   1519\n";
    assert!(refuse(two_levels).contains("joins two different voltage levels"));
}

#[test]
fn a_busbar_coupler_with_the_same_node_at_both_ends_is_ignored() {
    let text = "##C 2007.05.01\n\
##N\n\
##ZBE\n\
BBBBBB11              0 0        3.96708 0.00000 0.00000 0.00000\n\
##L\n\
BBBBBB11 BBBBBB11 1 7 0.0000 0.0000 0.000000   2000 BABAA\n\
##T\n\
##R\n";
    let module = parse_ucte_text("self-coupler.uct", text);

    assert!(module.value().switches().is_empty());
    assert!(
        messages(&module).iter().any(|message| message
            .starts_with("READ.UCTE.RECORD_IGNORED: line 6:")
            && message.contains("PowSybl UcteImporter ignores it")),
        "{:#?}",
        messages(&module)
    );
}

#[test]
fn the_powsybl_fixtures_parse_and_convert_to_matpower() {
    let module = parse_fixture("20170322_1844_SN3_FR2.uct");
    let net = &module.value();
    assert_eq!(net.buses().len(), 5);
    assert_eq!(net.branches().len(), 5);
    assert_eq!(net.generators().len(), 2);
    assert_eq!(net.loads().len(), 1);
    assert_eq!(
        net.case_metadata().case_date.as_deref(),
        Some("2017-03-22T18:44")
    );
    let generator = net.generators().iter().find(|g| g.bus == BusId(1)).unwrap();
    assert_eq!(generator.pg, 800.0);
    assert_eq!((generator.pmin, generator.pmax), (-9999.0, 9999.0));
    assert_eq!(bus_by_name(net, "FFNGEN71").base_kv, 27.0);
    assert_eq!(bus_by_name(net, "FFNHV311").kind, BusType::Ref);
    let matpower = emit_module(&module, TargetFormat::Matpower).unwrap();
    assert!(matpower.text.contains("mpc.bus = ["));
    let reread = parse_str(&matpower.text, "matpower").unwrap();
    assert_eq!(reread.network.buses().len(), 5);
    assert_eq!(reread.network.branches().len(), 5);

    let module = parse_fixture("elementName.uct");
    let net = &module.value();
    assert_eq!(net.buses().len(), 11);
    assert_eq!(net.branches().len(), 8);
    assert_eq!(net.switches().len(), 1);
    assert_eq!(net.areas().len(), 4);
    assert_eq!(bus_by_name(net, "XB__F_11").area, 4);
    let tie = net
        .branches()
        .iter()
        .find(|b| b.name.as_deref() == Some("Test TL 1/1"))
        .unwrap();
    assert_eq!(tie.from, bus_by_name(net, "XB__F_11").id);
    assert!(
        messages(&module)
            .iter()
            .any(|m| m.starts_with("READ.UCTE.REFERENCE_DROPPED")
                && m.contains("HDDDDD12 HCCCCC11 1")),
        "{:#?}",
        messages(&module)
    );
    let matpower = emit_module(&module, TargetFormat::Matpower).unwrap();
    assert!(parse_str(&matpower.text, "matpower").is_ok());
}

#[test]
fn same_format_emission_returns_the_source_text() {
    for name in [
        "synthetic_all_blocks.uct",
        "20170322_1844_SN3_FR2.uct",
        "elementName.uct",
    ] {
        let module = parse_fixture(name);
        let text = std::fs::read_to_string(data(name)).unwrap();
        let emission = emit_module(&module, TargetFormat::Ucte).unwrap();
        assert_eq!(emission.text, text, "{name}");
        assert!(emission.diagnostics.is_empty(), "{name}");
    }
}

#[test]
fn fresh_output_reads_back_to_the_same_network() {
    let module = parse_fixture("synthetic_all_blocks.uct");
    let fresh = emit_value(module.value(), TargetFormat::Ucte).unwrap();
    let rendered = fresh.render_diagnostics();
    assert!(
        rendered
            .iter()
            .all(|m| !m.contains("VALUE_SUBSTITUTED") || m.contains("system base")),
        "{rendered:#?}"
    );
    assert!(fresh.text.starts_with("##C 2007.05.01\n"));
    assert!(fresh.text.contains("##ZXX\nXFRBE_11 XNODE FR-BE  1 0"));
    assert!(
        fresh
            .text
            .contains("\n##TT\nFMV___21 FLOAD_11 1     3  0.5000 20.000"),
        "{}",
        fresh.text
    );
    let reread = parse_ucte_text("fresh.uct", &fresh.text);
    let (a, b) = (&module.value(), &reread.value());
    assert_eq!(a.buses().len(), b.buses().len());
    for (x, y) in a.buses().iter().zip(b.buses()) {
        assert_eq!(
            (x.id, &x.name, x.kind, x.base_kv, x.area),
            (y.id, &y.name, y.kind, y.base_kv, y.area)
        );
        assert!(approx(x.vm, y.vm), "bus {} vm {} vs {}", x.id, x.vm, y.vm);
    }
    assert_eq!(a.loads().len(), b.loads().len());
    for (x, y) in a.loads().iter().zip(b.loads()) {
        assert_eq!((x.bus, x.p, x.q), (y.bus, y.p, y.q));
    }
    assert_eq!(a.generators().len(), b.generators().len());
    for (x, y) in a.generators().iter().zip(b.generators()) {
        assert_eq!(
            (x.bus, x.pg, x.qg, x.pmin, x.pmax, x.qmin, x.qmax),
            (y.bus, y.pg, y.qg, y.pmin, y.pmax, y.qmin, y.qmax)
        );
        assert!(approx(x.vg, y.vg));
        assert_eq!(x.energy_source, y.energy_source);
    }
    assert_eq!(a.branches().len(), b.branches().len());
    for (x, y) in a.branches().iter().zip(b.branches()) {
        assert_eq!(
            (x.from, x.to, x.in_service, &x.name),
            (y.from, y.to, y.in_service, &y.name)
        );
        for (label, p, q) in [
            ("r", x.r, y.r),
            ("x", x.x, y.x),
            ("b", x.b, y.b),
            ("tap", x.tap, y.tap),
            ("shift", x.shift, y.shift),
            ("rate_a", x.rate_a, y.rate_a),
        ] {
            assert!(
                (p - q).abs() <= 1e-6 * q.abs().max(1.0),
                "branch {}-{} {label}: {p} vs {q}",
                x.from,
                x.to
            );
        }
        assert_eq!(
            x.control.as_ref().map(|c| c.mode),
            y.control.as_ref().map(|c| c.mode)
        );
        assert_eq!(
            x.extras.get("ucte_phase_regulation"),
            y.extras.get("ucte_phase_regulation")
        );
        assert_eq!(
            x.extras.get("ucte_angle_regulation"),
            y.extras.get("ucte_angle_regulation")
        );
    }
    assert_eq!(a.switches().len(), b.switches().len());
    assert_eq!(a.areas().len(), b.areas().len());
}

#[test]
fn a_matpower_case_writes_valid_node_codes_and_reads_back() {
    fn line(network: &BalancedNetwork) -> &powerio_tx::network::Branch {
        network
            .branches()
            .iter()
            .find(|branch| branch.from == BusId(4) && branch.to == BusId(5))
            .unwrap()
    }

    let module = powerio_tx::parse(
        Source::open(Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data/case9.m")).unwrap(),
    )
    .unwrap();
    let fresh = emit_value(module.value(), TargetFormat::Ucte).unwrap();
    let rendered = fresh.render_diagnostics();
    // case9 has no bus names and sits at 345 kV, which is not a UCTE level.
    assert!(
        rendered.iter().any(|m| m
            .starts_with("EMIT.UCTE.VALUE_SUBSTITUTED: 9 bus name(s) are not UCTE node codes")
            && m.contains("bus 1 -> \"A0000181\"")),
        "{rendered:#?}"
    );
    assert!(
        rendered
            .iter()
            .any(|m| m.contains("not one of the ten UCTE voltage levels")
                && m.contains("345 kV under level 8 = 330 kV")),
        "{rendered:#?}"
    );
    assert!(
        rendered
            .iter()
            .any(|m| m.contains("generator cost curves dropped"))
    );
    assert!(rendered.iter().any(|m| m.contains("60 Hz dropped")));
    let reread = parse_ucte_text("case9.uct", &fresh.text);
    let net = &reread.value();
    assert_eq!(net.buses().len(), 9);
    assert_eq!(net.branches().len(), 9);
    assert_eq!(net.generators().len(), 3);
    assert_eq!(net.loads().len(), 3);
    assert_eq!(
        net.buses()
            .iter()
            .filter(|b| b.kind == BusType::Ref)
            .count(),
        1
    );
    assert_eq!(net.branches().iter().filter(|b| b.tap != 0.0).count(), 0);
    for bus in net.buses() {
        let name = bus.name.as_deref().unwrap();
        assert_eq!(name.len(), 8);
        assert_eq!(&name[..1], "A");
    }
    // Physical values survive the level substitution: the slack generator
    // set point in kV and the line ohms are what the source stated.
    let source = &module.value();
    let source_slack = source
        .generators()
        .iter()
        .find(|g| g.bus == BusId(1))
        .unwrap();
    let source_bus = source.buses().iter().find(|b| b.id == BusId(1)).unwrap();
    let fresh_slack = net.generators().iter().find(|g| g.bus == BusId(1)).unwrap();
    let fresh_bus = net.buses().iter().find(|b| b.id == BusId(1)).unwrap();
    assert!(
        (source_slack.vg * source_bus.base_kv - fresh_slack.vg * fresh_bus.base_kv).abs() < 1e-3
    );
    let kv = |n: &BalancedNetwork| n.buses().iter().find(|b| b.id == BusId(5)).unwrap().base_kv;
    // The six character reactance field holds two decimals at this magnitude.
    let ohm = |n: &BalancedNetwork| line(n).x * kv(n) * kv(n) / n.base_mva();
    assert!(
        (ohm(source) - ohm(net)).abs() < 1e-2,
        "the source and emitted line impedances differ"
    );
}

#[test]
fn a_case_without_base_kv_is_written_at_the_level_nominal_voltage() {
    let module = powerio_tx::parse(
        Source::open(Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data/case14.m")).unwrap(),
    )
    .unwrap();
    let fresh = emit_value(module.value(), TargetFormat::Ucte).unwrap();
    let rendered = fresh.render_diagnostics();
    assert!(
        rendered
            .iter()
            .any(|m| m
                .starts_with("EMIT.UCTE.VALUE_DEFAULTED: 14 bus(es) state no positive base kV")),
        "{rendered:#?}"
    );
    assert!(
        rendered
            .iter()
            .any(|m| m.starts_with("EMIT.UCTE.RECORD_DROPPED: 1 shunt(s) dropped")),
        "{rendered:#?}"
    );
    let reread = parse_ucte_text("case14.uct", &fresh.text);
    let net = &reread.value();
    assert_eq!(net.buses().len(), 14);
    assert_eq!(net.branches().len(), 20);
    assert_eq!(net.branches().iter().filter(|b| b.tap != 0.0).count(), 3);
    // Per unit impedances survive: the level nominal voltage is the base on
    // both sides of the conversion.
    let source_line = &module.value().branches()[0];
    let fresh_line = &net.branches()[0];
    assert!(
        (source_line.x - fresh_line.x).abs() < 1e-6,
        "the source and emitted line reactances differ"
    );
    let source_tap = module
        .value()
        .branches()
        .iter()
        .find(|b| b.tap != 0.0)
        .unwrap()
        .tap;
    let fresh_tap = net.branches().iter().find(|b| b.tap != 0.0).unwrap().tap;
    assert!(
        (source_tap - fresh_tap).abs() < 1e-3,
        "{source_tap} vs {fresh_tap}"
    );
}

#[test]
fn a_phase_shift_and_a_voltage_control_write_regulation_records() {
    use powerio_tx::network::{Branch, Bus, Generator, TransformerControl};
    let mut buses = vec![
        Bus::new(BusId(1), BusType::Ref, 380.0),
        Bus::new(BusId(2), BusType::Pq, 380.0),
        Bus::new(BusId(3), BusType::Pq, 220.0),
    ];
    buses[0].name = Some("FSLACK11".into());
    buses[1].name = Some("FSHIFT11".into());
    buses[2].name = Some("FTAPS_21".into());
    let mut shifter = Branch::new(BusId(1), BusId(2), 0.001, 0.02);
    shifter.tap = 1.0;
    shifter.shift = 5.0;
    let mut ltc = Branch::new(BusId(2), BusId(3), 0.001, 0.05);
    ltc.tap = 1.05;
    let mut control = TransformerControl::new(TransformerControlMode::Voltage);
    control.tap_min = 0.9;
    control.tap_max = 1.1;
    control.ntp = 21;
    control.band_min = 1.02;
    control.band_max = 1.04;
    control.controlled_bus = Some(BusId(2));
    ltc.control = Some(control);
    let mut net = BalancedNetwork::in_memory("regulated", 100.0, buses, vec![shifter, ltc]);
    let mut generator = Generator::new(BusId(1));
    generator.pg = 10.0;
    generator.pmax = 20.0;
    net.generators_mut().push(generator);
    let fresh = emit_value(&net, TargetFormat::Ucte).unwrap();
    assert!(fresh.text.contains("FSHIFT11 FSLACK11 1"), "{}", fresh.text);
    assert!(fresh.text.contains("SYMM"), "{}", fresh.text);
    let reread = parse_ucte_text("regulated.uct", &fresh.text);
    let net = &reread.value();
    // The five character voltage step field carries four significant digits.
    let shifter = net.branches().iter().find(|b| b.shift != 0.0).unwrap();
    assert!((shifter.shift - 5.0).abs() < 1e-3, "{}", shifter.shift);
    assert!((shifter.tap - 1.0).abs() < 1e-9);
    let ltc = net
        .branches()
        .iter()
        .find(|b| {
            b.control
                .as_ref()
                .is_some_and(|c| c.mode == TransformerControlMode::Voltage)
        })
        .unwrap();
    assert!((ltc.tap - 1.05).abs() < 1e-6, "{}", ltc.tap);
    let control = ltc.control.as_ref().unwrap();
    assert_eq!(control.ntp, 21);
    assert!((control.band_min - 1.03).abs() < 1e-9);
}
