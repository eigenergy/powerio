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
