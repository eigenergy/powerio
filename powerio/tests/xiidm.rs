use powerio::{Destination, EmittedOutput, FormatId, Source, deserialize, emit, parse, serialize};

const UNMAPPED_FIELDS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<iidm:network xmlns:iidm="http://www.powsybl.org/schema/iidm/1_17" id="unmapped" caseDate="2026-01-01T00:00:00Z" forecastDistance="0" sourceFormat="test" minimumValidationLevel="STEADY_STATE_HYPOTHESIS">
  <iidm:substation id="S">
    <iidm:voltageLevel id="VL" nominalV="20" topologyKind="BUS_BREAKER">
      <iidm:busBreakerTopology><iidm:bus id="B" v="20" angle="0" fictitiousP0="1" fictitiousQ0="2"/></iidm:busBreakerTopology>
      <iidm:load id="L" loadType="FICTITIOUS" p0="5" q0="1" bus="B" connectableBus="B"/>
      <iidm:generator id="G" energySource="OTHER" minP="0" maxP="10" ratedS="10" voltageRegulatorOn="true" targetP="5" targetQ="0" targetV="20" isCondenser="true" equivalentLocalTargetV="20" bus="B" connectableBus="B">
        <iidm:minMaxReactiveLimits minQ="-5" maxQ="5"/>
      </iidm:generator>
      <iidm:shuntCompensator id="SH" sectionCount="1" solvedSectionCount="1" voltageRegulatorOn="false" bus="B" connectableBus="B">
        <iidm:shuntLinearModel bPerSection="0.001" maximumSectionCount="1"/>
      </iidm:shuntCompensator>
    </iidm:voltageLevel>
    <iidm:voltageLevel id="VL2" nominalV="10" topologyKind="BUS_BREAKER">
      <iidm:busBreakerTopology><iidm:bus id="B2"/></iidm:busBreakerTopology>
    </iidm:voltageLevel>
    <iidm:voltageLevel id="VL3" nominalV="5" topologyKind="BUS_BREAKER">
      <iidm:busBreakerTopology><iidm:bus id="B3"/></iidm:busBreakerTopology>
    </iidm:voltageLevel>
    <iidm:twoWindingsTransformer id="T2" r="0.1" x="1" ratedU1="20" ratedU2="10" ratedS="8" bus1="B" connectableBus1="B" voltageLevelId1="VL" bus2="B2" connectableBus2="B2" voltageLevelId2="VL2"/>
    <iidm:threeWindingsTransformer id="T3" ratedU0="20" r1="0.1" x1="1" ratedU1="20" ratedS1="9" r2="0.1" x2="1" ratedU2="10" ratedS2="8" r3="0.1" x3="1" ratedU3="5" ratedS3="7" bus1="B" connectableBus1="B" voltageLevelId1="VL" bus2="B2" connectableBus2="B2" voltageLevelId2="VL2" bus3="B3" connectableBus3="B3" voltageLevelId3="VL3"/>
  </iidm:substation>
</iidm:network>"#;

fn memory_text(result: powerio::EmitResult) -> String {
    let EmittedOutput::Memory { mut artifacts } = result.into_output() else {
        panic!("memory destination returned a path output");
    };
    String::from_utf8(artifacts.pop().unwrap().into_bytes()).unwrap()
}

#[test]
fn unmapped_official_fields_remain_diagnosed_after_ir_and_fresh_emission() {
    let source = Source::from_memory("unmapped.xiidm", UNMAPPED_FIELDS.as_bytes().to_vec())
        .unwrap()
        .with_format(FormatId::new("xiidm").unwrap());
    let module = parse(source).unwrap();
    let field_diagnostics = module
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code() == "READ.XIIDM.FIELD_UNMAPPED")
        .map(powerio::Diagnostic::message)
        .collect::<Vec<_>>();
    assert!(
        field_diagnostics
            .iter()
            .any(|message| message.contains("`isCondenser=true`"))
    );
    assert!(
        field_diagnostics
            .iter()
            .any(|message| message.contains("`equivalentLocalTargetV=20`"))
    );
    assert!(
        field_diagnostics
            .iter()
            .any(|message| message.contains("`loadType=FICTITIOUS`"))
    );
    for expected in [
        "`fictitiousP0=1`",
        "`fictitiousQ0=2`",
        "`solvedSectionCount=1`",
        "`ratedS=8`",
        "`ratedS1=9`",
        "`ratedS2=8`",
        "`ratedS3=7`",
    ] {
        assert!(
            field_diagnostics
                .iter()
                .any(|message| message.contains(expected)),
            "missing diagnostic for {expected}"
        );
    }

    let stored = serialize(&module, Destination::memory("unmapped.pio.json").unwrap()).unwrap();
    let restored = deserialize(
        Source::from_memory("unmapped.pio.json", memory_text(stored).into_bytes()).unwrap(),
    )
    .unwrap();
    assert_eq!(
        restored
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code() == "READ.XIIDM.FIELD_UNMAPPED")
            .count(),
        field_diagnostics.len()
    );

    let emitted = emit(
        &restored,
        "xiidm",
        Destination::memory("fresh.xiidm").unwrap(),
    )
    .unwrap();
    let text = memory_text(emitted);
    assert!(!text.contains("isCondenser="));
    assert!(!text.contains("equivalentLocalTargetV="));
    assert!(text.contains("loadType=\"UNDEFINED\""));
    assert!(!text.contains("fictitiousP0="));
    assert!(!text.contains("fictitiousQ0="));
    assert!(!text.contains("solvedSectionCount="));
    assert!(text.contains("ratedS=\"10\""));
    assert!(!text.contains("ratedS1="));
    assert!(!text.contains("ratedS2="));
    assert!(!text.contains("ratedS3="));
}
