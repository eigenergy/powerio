//! PowSybl exchange formats (XIIDM, JIIDM) against the PSS/E formats (RAW,
//! RAWX): what one states and the other reads back. Each test runs the
//! smallest synthetic case that shows the rule and, where a vendored PowSybl
//! or PSS/E fixture exercises it, that fixture as well. The rules follow
//! PowSybl's own psse-converter, cited at each writer and reader.
mod helpers;
#[allow(unused_imports)]
use helpers::*;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use powerio_tx::{BalancedNetwork, TargetFormat};

fn data(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data")
        .join(name)
}

fn xiidm(text: &str) -> BalancedNetwork {
    parse_str(text, "xiidm").unwrap().network
}

fn rawx(network: &BalancedNetwork) -> TextEmission {
    emit_value(network, TargetFormat::PsseRawx).unwrap()
}

fn read_rawx(emitted: &TextEmission) -> BalancedNetwork {
    parse_str(&emitted.text, "psse-rawx").unwrap().network
}

fn codes_and_messages(emitted: &TextEmission) -> Vec<String> {
    emitted.render_diagnostics()
}

fn node_numbers_by_substation(network: &BalancedNetwork) -> Vec<(String, i32)> {
    let detailed = network.detailed_connectivity().as_ref().unwrap();
    let mut numbers = detailed
        .connectivity_nodes
        .iter()
        .map(|node| {
            let level = detailed
                .voltage_levels
                .iter()
                .find(|level| level.component == node.voltage_level)
                .unwrap();
            (
                level.substation.as_ref().unwrap().to_string(),
                node.node_number.unwrap(),
            )
        })
        .collect::<Vec<_>>();
    numbers.sort();
    numbers
}

/// One substation with two node breaker voltage levels joined by a
/// transformer. IIDM numbers nodes within a voltage level, so both levels
/// state nodes 0, 1, and 2.
const TWO_LEVEL_SUBSTATION: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<iidm:network xmlns:iidm="http://www.powsybl.org/schema/iidm/1_17" id="two-levels" caseDate="2025-01-01T00:00:00Z" forecastDistance="0" sourceFormat="test" minimumValidationLevel="STEADY_STATE_HYPOTHESIS">
  <iidm:substation id="S">
    <iidm:voltageLevel id="VL1" nominalV="110" topologyKind="NODE_BREAKER">
      <iidm:nodeBreakerTopology>
        <iidm:busbarSection id="BBS1" node="0"/>
        <iidm:switch id="SW11" kind="BREAKER" open="false" node1="0" node2="1"/>
        <iidm:switch id="SW12" kind="BREAKER" open="false" node1="0" node2="2"/>
      </iidm:nodeBreakerTopology>
      <iidm:generator id="G" energySource="OTHER" minP="0" maxP="50" voltageRegulatorOn="true" targetP="10" targetV="110" node="2">
        <iidm:minMaxReactiveLimits minQ="-20" maxQ="20"/>
      </iidm:generator>
    </iidm:voltageLevel>
    <iidm:voltageLevel id="VL2" nominalV="20" topologyKind="NODE_BREAKER">
      <iidm:nodeBreakerTopology>
        <iidm:busbarSection id="BBS2" node="0"/>
        <iidm:switch id="SW21" kind="BREAKER" open="false" node1="0" node2="1"/>
        <iidm:switch id="SW22" kind="BREAKER" open="false" node1="0" node2="2"/>
      </iidm:nodeBreakerTopology>
      <iidm:load id="L" loadType="UNDEFINED" p0="10" q0="2" node="2"/>
    </iidm:voltageLevel>
    <iidm:twoWindingsTransformer id="T" r="0.1" x="2.0" g="0" b="0" ratedU1="110" ratedU2="20" node1="1" voltageLevelId1="VL1" node2="1" voltageLevelId2="VL2"/>
  </iidm:substation>
</iidm:network>"#;

#[test]
fn node_breaker_iidm_is_numbered_as_psse_numbers_a_substation() {
    let network = xiidm(TWO_LEVEL_SUBSTATION);
    let emitted = rawx(&network);
    let messages = codes_and_messages(&emitted);
    assert!(
        messages.iter().any(|message| message.starts_with(
            "EMIT.PSSE.VALUE_SUBSTITUTED: 6 connectivity node(s) in 1 substation(s) renumbered from 1"
        )),
        "{messages:#?}"
    );

    let back = read_rawx(&emitted);
    let numbers = node_numbers_by_substation(&back)
        .into_iter()
        .map(|(_, number)| number)
        .collect::<Vec<_>>();
    assert_eq!(numbers, [1, 2, 3, 4, 5, 6]);
    let detailed = back.detailed_connectivity().as_ref().unwrap();
    assert_eq!(detailed.switches.len(), 4);
    assert_eq!(back.buses().len(), 2);

    // Revision 35 RAW nests the same records; revision 33 has no block for
    // them and says so without a writer error behind it.
    let raw35 = emit_psse_rev(&network, 35);
    let back35 = parse_psse(&raw35.text).unwrap();
    assert_eq!(node_numbers_by_substation(&back35).len(), 6);
    let raw33 = emit_psse_rev(&network, 33);
    let messages = codes_and_messages(&raw33);
    assert!(
        messages.iter().any(|message| message
            == "EMIT.PSSE.RECORD_DROPPED: detailed substation connectivity dropped: PSS/E revision 33 has no substation data block"),
        "{messages:#?}"
    );
    assert!(
        messages
            .iter()
            .all(|message| !message.contains("read error") && !message.contains("renumbered")),
        "{messages:#?}"
    );
}

#[test]
fn powsybl_fictitious_switch_case_writes_rawx() {
    let network = parse_file(data("xiidm/powsybl/V1_17/fictitiousSwitchRef.xml"), None)
        .unwrap()
        .network;
    let emitted = rawx(&network);
    let back = read_rawx(&emitted);
    assert_eq!(back.buses().len(), network.buses().len());
    let numbers = node_numbers_by_substation(&back);
    let distinct = numbers.iter().collect::<BTreeSet<_>>();
    assert_eq!(distinct.len(), numbers.len());
    assert!(numbers.iter().all(|(_, number)| *number > 0));
    assert_eq!(
        back.detailed_connectivity()
            .as_ref()
            .unwrap()
            .switches
            .len(),
        network
            .detailed_connectivity()
            .as_ref()
            .unwrap()
            .switches
            .len()
    );
}

fn close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() <= 1e-9 * expected.abs().max(1.0),
        "{actual} != {expected}"
    );
}

/// Two 380 kV buses joined by a line that states current limits only.
const LINE_CURRENT_LIMITS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<iidm:network xmlns:iidm="http://www.powsybl.org/schema/iidm/1_17" id="limits" caseDate="2025-01-01T00:00:00Z" forecastDistance="0" sourceFormat="test" minimumValidationLevel="STEADY_STATE_HYPOTHESIS">
  <iidm:substation id="S">
    <iidm:voltageLevel id="VL1" nominalV="380" topologyKind="BUS_BREAKER">
      <iidm:busBreakerTopology><iidm:bus id="B1" v="380" angle="0"/></iidm:busBreakerTopology>
      <iidm:generator id="G1" energySource="OTHER" minP="0" maxP="200" voltageRegulatorOn="true" targetP="100" targetQ="10" targetV="380" bus="B1" connectableBus="B1">
        <iidm:minMaxReactiveLimits minQ="-50" maxQ="50"/>
      </iidm:generator>
    </iidm:voltageLevel>
    <iidm:voltageLevel id="VL2" nominalV="380" topologyKind="BUS_BREAKER">
      <iidm:busBreakerTopology><iidm:bus id="B2" v="378" angle="-2"/></iidm:busBreakerTopology>
      <iidm:load id="L1" loadType="UNDEFINED" p0="90" q0="30" bus="B2" connectableBus="B2"/>
    </iidm:voltageLevel>
  </iidm:substation>
  <iidm:line id="LINE" r="2" x="20" g1="0" b1="0" g2="0" b2="0" bus1="B1" connectableBus1="B1" voltageLevelId1="VL1" bus2="B2" connectableBus2="B2" voltageLevelId2="VL2" selectedOperationalLimitsGroupIds1="DEFAULT">
    <iidm:operationalLimitsGroup1 id="DEFAULT">
      <iidm:currentLimits permanentLimit="931.0">
        <iidm:temporaryLimit name="IT20" acceptableDuration="1200" value="1100.0"/>
        <iidm:temporaryLimit name="IT1" acceptableDuration="60" value="1500.0"/>
      </iidm:currentLimits>
    </iidm:operationalLimitsGroup1>
  </iidm:line>
</iidm:network>"#;

#[test]
fn iidm_current_limits_become_psse_mva_ratings() {
    let network = xiidm(LINE_CURRENT_LIMITS);
    let mva = |amperes: f64| 3f64.sqrt() * 380.0 * amperes / 1000.0;
    for rev in [33, 35] {
        let emitted = emit_psse_rev(&network, rev);
        let messages = codes_and_messages(&emitted);
        assert!(
            messages.iter().any(|message| message.starts_with(
                "EMIT.PSSE.VALUE_SUBSTITUTED: 1 branch current limit set(s) written as MVA ratings"
            )),
            "{messages:#?}"
        );
        assert!(
            messages
                .iter()
                .all(|message| !message.contains("current rating record(s) dropped")),
            "{messages:#?}"
        );
        let back = parse_psse(&emitted.text).unwrap();
        let line = &back.branches()[0];
        close(line.rate_a, mva(931.0));
        close(line.rate_b, mva(1100.0));
        close(line.rate_c, mva(1500.0));
    }
    let back = read_rawx(&rawx(&network));
    close(back.branches()[0].rate_a, mva(931.0));
}

#[test]
fn powsybl_line_current_limit_becomes_rate_a() {
    let network = parse_file(data("xiidm/powsybl/V1_17/fictitiousSwitchRef.xml"), None)
        .unwrap()
        .network;
    let back = parse_psse(&emit_psse_rev(&network, 35).text).unwrap();
    // 931 A on each side of the 225 kV line `CJ`.
    let line = back
        .branches()
        .iter()
        .find(|branch| !branch.is_transformer())
        .unwrap();
    close(line.rate_a, 3f64.sqrt() * 225.0 * 0.931);
}

#[test]
fn three_winding_leg_current_limits_rate_each_winding() {
    let network = parse_file(
        data("xiidm/powsybl/V1_0/threeWindingsTransformerRoundTripRef.xml"),
        None,
    )
    .unwrap()
    .network;
    let windings = &network.transformers_3w()[0].windings;
    // Each leg states 1000/1200/1400 A scaled by its voltage level: 132, 33,
    // and 11 kV legs carry 1000, 100, and 10 A permanently.
    for (winding, (kv, amperes)) in
        windings
            .iter()
            .zip([(132.0, 1000.0), (33.0, 100.0), (11.0, 10.0)])
    {
        let mva = |factor: f64| 3f64.sqrt() * kv * amperes * factor / 1000.0;
        close(winding.rate_a, mva(1.0));
        close(winding.rate_b, mva(1.2));
        close(winding.rate_c, mva(1.4));
    }
    let back = parse_psse(&emit_psse_rev(&network, 33).text).unwrap();
    close(
        back.transformers_3w()[0].windings[0].rate_a,
        3f64.sqrt() * 132.0,
    );
    // XIIDM output restates the amperes from the retained limit groups.
    let xml = emit_value(&network, TargetFormat::Xiidm).unwrap().text;
    assert!(xml.contains("permanentLimit=\"1000\""), "{xml}");
    assert!(!xml.contains("apparentPowerLimits"), "{xml}");
}

/// Buses 1 and 10, the PSS/E numbering a dense renumbering would lose.
fn two_numbered_buses() -> BalancedNetwork {
    use powerio_tx::network::{Branch, Bus, BusId, BusType, Generator, Load};
    let mut first = Bus::new(BusId::new(1), BusType::Ref, 138.0);
    first.name = Some("Alpha".to_owned());
    let mut tenth = Bus::new(BusId::new(10), BusType::Pq, 138.0);
    tenth.name = Some("Bus 10".to_owned());
    let mut network = BalancedNetwork::in_memory(
        "numbered",
        100.0,
        vec![first, tenth],
        vec![Branch::new(BusId::new(1), BusId::new(10), 0.01, 0.1)],
    );
    network.generators_mut().push(Generator::new(BusId::new(1)));
    network
        .loads_mut()
        .push(Load::new(BusId::new(10), 10.0, 2.0));
    network
}

fn numbers_and_names(network: &BalancedNetwork) -> Vec<(usize, Option<String>)> {
    network
        .buses()
        .iter()
        .map(|bus| (bus.id.0, bus.name.clone()))
        .collect()
}

#[test]
fn psse_bus_numbers_and_names_survive_iidm() {
    let network = parse_psse(&emit_psse(&two_numbered_buses()).text).unwrap();
    let xml = emit_value(&network, TargetFormat::Xiidm).unwrap().text;
    assert!(xml.contains(r#"<iidm:bus id="B10" name="Bus 10""#), "{xml}");
    let back = parse_psse(&emit_psse(&xiidm(&xml)).text).unwrap();
    assert_eq!(numbers_and_names(&back), numbers_and_names(&network));
    // JIIDM states the same ids.
    let json = emit_value(&network, TargetFormat::Jiidm).unwrap().text;
    let back = parse_str(&json, "jiidm").unwrap().network;
    assert_eq!(numbers_and_names(&back), numbers_and_names(&network));

    let source = parse_file(data("psse/ExampleVersion32_exported.raw"), None)
        .unwrap()
        .network;
    let xml = emit_value(&source, TargetFormat::Xiidm).unwrap().text;
    let back = parse_psse(&emit_psse(&xiidm(&xml)).text).unwrap();
    assert_eq!(numbers_and_names(&back), numbers_and_names(&source));
}

#[test]
fn an_iidm_bus_id_with_no_number_is_numbered_after_the_recovered_ones() {
    let xml = LINE_CURRENT_LIMITS
        .replace("\"B1\"", "\"B7\"")
        .replace("\"B2\"", "\"X\"");
    let parsed = parse_str(&xml, "xiidm").unwrap();
    let ids = parsed
        .network
        .buses()
        .iter()
        .map(|bus| bus.id.0)
        .collect::<Vec<_>>();
    assert_eq!(ids, [7, 1]);
    let messages = powerio_tx::diagnostics::render_diagnostics(&parsed.diagnostics);
    assert!(
        messages.iter().any(|message| message
            == "READ.XIIDM.VALUE_DEFAULTED: 1 bus(es) state no PSS/E bus number in their IIDM id (`B<n>`), or repeat one; they were numbered after the 1 recovered number(s)"),
        "{messages:#?}"
    );
}

fn reference_buses(network: &BalancedNetwork) -> Vec<usize> {
    network
        .buses()
        .iter()
        .filter(|bus| bus.kind == powerio_tx::BusType::Ref)
        .map(|bus| bus.id.0)
        .collect()
}

#[test]
fn a_reference_bus_with_only_branches_stays_the_reference() {
    use powerio_tx::network::{Branch, Bus, BusId, BusType, Generator};
    // Bus 1 is the swing bus and hosts only the line; the generator is at 2.
    let mut network = BalancedNetwork::in_memory(
        "branch reference",
        100.0,
        vec![
            Bus::new(BusId::new(1), BusType::Ref, 138.0),
            Bus::new(BusId::new(2), BusType::Pv, 138.0),
        ],
        vec![Branch::new(BusId::new(1), BusId::new(2), 0.01, 0.1)],
    );
    network.generators_mut().push(Generator::new(BusId::new(2)));
    let network = parse_psse(&emit_psse(&network).text).unwrap();
    let parsed = parse_str(
        &emit_value(&network, TargetFormat::Xiidm).unwrap().text,
        "xiidm",
    )
    .unwrap();
    assert_eq!(reference_buses(&parsed.network), [1]);
    let messages = powerio_tx::diagnostics::render_diagnostics(&parsed.diagnostics);
    assert!(
        messages
            .iter()
            .all(|message| !message.contains("slack terminal")),
        "{messages:#?}"
    );

    // PowSybl's eight bus export: the swing bus 1 hosts no injection.
    let source = parse_file(data("psse/ExampleVersion32_exported.raw"), None)
        .unwrap()
        .network;
    assert_eq!(reference_buses(&source), [1]);
    let xml = emit_value(&source, TargetFormat::Xiidm).unwrap().text;
    assert_eq!(reference_buses(&xiidm(&xml)), [1]);
}

/// A 138/138 kV transformer from bus 1 to bus 2 under the given control.
fn controlled_transformer(
    control: powerio_tx::network::TransformerControl,
    tap: f64,
    shift: f64,
) -> BalancedNetwork {
    use powerio_tx::network::{Branch, Bus, BusId, BusType, Generator, Load};
    let mut branch = Branch::new(BusId::new(1), BusId::new(2), 0.0, 0.1);
    branch.tap = tap;
    branch.shift = shift;
    branch.control = Some(control);
    let mut network = BalancedNetwork::in_memory(
        "controlled",
        100.0,
        vec![
            Bus::new(BusId::new(1), BusType::Ref, 138.0),
            Bus::new(BusId::new(2), BusType::Pq, 138.0),
        ],
        vec![branch],
    );
    network.generators_mut().push(Generator::new(BusId::new(1)));
    network
        .loads_mut()
        .push(Load::new(BusId::new(2), 10.0, 2.0));
    // Read it as PSS/E states it.
    parse_psse(&emit_psse(&network).text).unwrap()
}

fn voltage_control() -> powerio_tx::network::TransformerControl {
    use powerio_tx::network::{BusId, TransformerControl, TransformerControlMode};
    let mut control = TransformerControl::new(TransformerControlMode::Voltage);
    control.controlled_bus = Some(BusId::new(2));
    control.tap_min = 0.9;
    control.tap_max = 1.1;
    control.band_min = 0.95;
    control.band_max = 1.05;
    control.ntp = 5;
    control
}

fn control_of(network: &BalancedNetwork) -> powerio_tx::network::TransformerControl {
    network.branches()[0].control.clone().unwrap()
}

fn assert_same_control(
    actual: &powerio_tx::network::TransformerControl,
    expected: &powerio_tx::network::TransformerControl,
) {
    assert_eq!(actual.mode, expected.mode);
    assert_eq!(actual.enabled, expected.enabled);
    assert_eq!(actual.controlled_bus, expected.controlled_bus);
    assert_eq!(actual.ntp, expected.ntp);
    for (actual, expected) in [
        (actual.tap_min, expected.tap_min),
        (actual.tap_max, expected.tap_max),
        (actual.band_min, expected.band_min),
        (actual.band_max, expected.band_max),
    ] {
        assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
    }
}

#[test]
fn psse_voltage_control_crosses_iidm_as_a_regulating_ratio_tap_changer() {
    let network = controlled_transformer(voltage_control(), 1.0, 0.0);
    let emitted = emit_value(&network, TargetFormat::Xiidm).unwrap();
    let xml = &emitted.text;
    // Five steps from 0.9 to 1.1, regulating bus 2 to 138 kV within 13.8 kV.
    assert!(xml.contains("regulationMode=\"VOLTAGE\""), "{xml}");
    assert!(xml.contains("regulating=\"true\""), "{xml}");
    assert!(xml.contains("regulationValue=\"138\""), "{xml}");
    assert_eq!(xml.matches("<iidm:step ").count(), 5, "{xml}");
    let messages = codes_and_messages(&emitted);
    assert!(
        messages
            .iter()
            .all(|message| !message.contains("data has no XIIDM transformer field")),
        "{messages:#?}"
    );

    let iidm = xiidm(xml);
    assert_same_control(&control_of(&iidm), &control_of(&network));
    let back = parse_psse(&emit_psse(&iidm).text).unwrap();
    assert_same_control(&control_of(&back), &control_of(&network));
    assert!((back.branches()[0].tap - 1.0).abs() < 1e-12);
}

#[test]
fn psse_active_power_control_crosses_iidm_as_a_phase_tap_changer() {
    use powerio_tx::network::{BusId, TransformerControl, TransformerControlMode};
    let mut control = TransformerControl::new(TransformerControlMode::ActiveFlow);
    control.controlled_bus = Some(BusId::new(2));
    control.tap_min = -30.0;
    control.tap_max = 30.0;
    control.band_min = 40.0;
    control.band_max = 60.0;
    control.ntp = 7;
    let network = controlled_transformer(control, 1.0, 10.0);
    let xml = emit_value(&network, TargetFormat::Xiidm).unwrap().text;
    assert!(
        xml.contains("regulationMode=\"ACTIVE_POWER_CONTROL\""),
        "{xml}"
    );
    assert!(xml.contains("regulationValue=\"50\""), "{xml}");
    assert!(xml.contains("targetDeadband=\"20\""), "{xml}");
    let back = parse_psse(&emit_psse(&xiidm(&xml)).text).unwrap();
    assert_same_control(&control_of(&back), &control_of(&network));
    assert!((back.branches()[0].shift - 10.0).abs() < 1e-9);
}

#[test]
fn a_present_ratio_between_steps_becomes_a_step_of_its_own() {
    let network = controlled_transformer(voltage_control(), 1.01, 0.0);
    let emitted = emit_value(&network, TargetFormat::Xiidm).unwrap();
    let messages = codes_and_messages(&emitted);
    assert!(
        messages.iter().any(|message| message
            .starts_with("EMIT.XIIDM.VALUE_SUBSTITUTED: transformer")
            && message.contains("so it has 6 steps")),
        "{messages:#?}"
    );
    let back = xiidm(&emitted.text);
    assert!((back.branches()[0].tap - 1.01).abs() < 1e-12);
    assert_eq!(control_of(&back).ntp, 6);
}

#[test]
fn powsybl_three_winding_tap_regulation_reaches_psse() {
    let network = parse_file(
        data("xiidm/powsybl/V1_0/threeWindingsTransformerRoundTripRef.xml"),
        None,
    )
    .unwrap()
    .network;
    let back = parse_psse(&emit_psse(&network).text).unwrap();
    let windings = &back.transformers_3w()[0].windings;
    // `ratioTapChanger2` regulates LOAD_33 at bus 2 to 33 kV over three steps.
    let control = windings[1].control.as_ref().unwrap();
    assert_eq!(
        control.mode,
        powerio_tx::network::TransformerControlMode::Voltage
    );
    assert!(control.enabled);
    assert_eq!(control.controlled_bus.map(|bus| bus.0), Some(2));
    assert_eq!(control.ntp, 3);
    assert!((control.tap_min - 0.9).abs() < 1e-9 && (control.tap_max - 1.1).abs() < 1e-9);
    assert!((control.band_min - 1.0).abs() < 1e-9 && (control.band_max - 1.0).abs() < 1e-9);
    // `ratioTapChanger3` states a target but does not regulate.
    let control = windings[2].control.as_ref().unwrap();
    assert!(!control.enabled);
    assert_eq!(control.controlled_bus.map(|bus| bus.0), Some(3));
}

#[test]
fn psse_bus_zone_and_owner_survive_iidm_as_properties() {
    let mut network = two_numbered_buses();
    network.buses_mut()[1].zone = 2;
    network.buses_mut()[1]
        .extras
        .insert("psse_owner".to_owned(), serde_json::Value::from(3));
    let network = parse_psse(&emit_psse(&network).text).unwrap();
    let xml = emit_value(&network, TargetFormat::Xiidm).unwrap().text;
    assert!(
        xml.contains(r#"<iidm:property name="psse_zone" value="2"/>"#),
        "{xml}"
    );
    assert!(
        xml.contains(r#"<iidm:property name="psse_owner" value="3"/>"#),
        "{xml}"
    );
    let back = parse_psse(&emit_psse(&xiidm(&xml)).text).unwrap();
    let bus = &back.buses()[1];
    assert_eq!(bus.zone, 2);
    assert_eq!(bus.extras["psse_owner"], serde_json::Value::from(3));
    // Bus 1 states PSS/E's defaults and gains no property.
    assert_eq!(back.buses()[0].zone, 1);
    assert_eq!(xml.matches("psse_zone").count(), 1, "{xml}");
}
