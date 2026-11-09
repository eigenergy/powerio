//! CGMES operational limits describe the active state and the terminal they constrain.
use powerio_core::{Destination, EmittedOutput, Fidelity, FormatId, PioModule, Source};
use powerio_tx::{BalancedNetwork, TargetFormat};

// Small synthetic transformer: terminal sequence deliberately opposes winding order.
fn transformer(version: u8, limits: &str) -> PioModule<BalancedNetwork> {
    let (namespace, profile, kind) = if version == 3 {
        (
            "http://iec.ch/TC57/CIM100#",
            "http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0",
            "eu:OperationalLimitType.kind",
        )
    } else {
        (
            "http://iec.ch/TC57/2013/CIM-schema-cim16#",
            "http://entsoe.eu/CIM/EquipmentCore/3/1",
            "entsoe:OperationalLimitType.limitType",
        )
    };
    let text = format!(
        r##"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
 xmlns:cim="{namespace}" xmlns:eu="http://iec.ch/TC57/CIM100-European#"
 xmlns:entsoe="http://entsoe.eu/CIM/SchemaExtension/3/1#"
 xmlns:md="http://iec.ch/TC57/61970-552/ModelDescription/1#">
 <md:FullModel rdf:about="urn:uuid:limits-eq">
  <md:Model.profile>{profile}</md:Model.profile>
 </md:FullModel>
 <cim:BaseVoltage rdf:ID="hv"><cim:BaseVoltage.nominalVoltage>220</cim:BaseVoltage.nominalVoltage></cim:BaseVoltage>
 <cim:BaseVoltage rdf:ID="lv"><cim:BaseVoltage.nominalVoltage>20</cim:BaseVoltage.nominalVoltage></cim:BaseVoltage>
 <cim:Substation rdf:ID="substation"/>
 <cim:VoltageLevel rdf:ID="level-hv">
  <cim:VoltageLevel.Substation rdf:resource="#substation"/>
  <cim:VoltageLevel.BaseVoltage rdf:resource="#hv"/>
 </cim:VoltageLevel>
 <cim:VoltageLevel rdf:ID="level-lv">
  <cim:VoltageLevel.Substation rdf:resource="#substation"/>
  <cim:VoltageLevel.BaseVoltage rdf:resource="#lv"/>
 </cim:VoltageLevel>
 <cim:TopologicalNode rdf:ID="node-hv">
  <cim:TopologicalNode.BaseVoltage rdf:resource="#hv"/>
  <cim:TopologicalNode.ConnectivityNodeContainer rdf:resource="#level-hv"/>
 </cim:TopologicalNode>
 <cim:TopologicalNode rdf:ID="node-lv">
  <cim:TopologicalNode.BaseVoltage rdf:resource="#lv"/>
  <cim:TopologicalNode.ConnectivityNodeContainer rdf:resource="#level-lv"/>
 </cim:TopologicalNode>
 <cim:PowerTransformer rdf:ID="transformer">
  <cim:Equipment.EquipmentContainer rdf:resource="#substation"/>
 </cim:PowerTransformer>
 <cim:Terminal rdf:ID="terminal-hv">
  <cim:Terminal.ConductingEquipment rdf:resource="#transformer"/>
  <cim:Terminal.TopologicalNode rdf:resource="#node-hv"/>
  <cim:ACDCTerminal.sequenceNumber>2</cim:ACDCTerminal.sequenceNumber>
 </cim:Terminal>
 <cim:Terminal rdf:ID="terminal-lv">
  <cim:Terminal.ConductingEquipment rdf:resource="#transformer"/>
  <cim:Terminal.TopologicalNode rdf:resource="#node-lv"/>
  <cim:ACDCTerminal.sequenceNumber>1</cim:ACDCTerminal.sequenceNumber>
 </cim:Terminal>
 <cim:PowerTransformerEnd rdf:ID="end-hv">
  <cim:PowerTransformerEnd.PowerTransformer rdf:resource="#transformer"/>
  <cim:TransformerEnd.Terminal rdf:resource="#terminal-hv"/>
  <cim:TransformerEnd.endNumber>1</cim:TransformerEnd.endNumber>
  <cim:PowerTransformerEnd.ratedU>220</cim:PowerTransformerEnd.ratedU>
  <cim:PowerTransformerEnd.x>10</cim:PowerTransformerEnd.x>
 </cim:PowerTransformerEnd>
 <cim:PowerTransformerEnd rdf:ID="end-lv">
  <cim:PowerTransformerEnd.PowerTransformer rdf:resource="#transformer"/>
  <cim:TransformerEnd.Terminal rdf:resource="#terminal-lv"/>
  <cim:TransformerEnd.endNumber>2</cim:TransformerEnd.endNumber>
  <cim:PowerTransformerEnd.ratedU>20</cim:PowerTransformerEnd.ratedU>
  <cim:PowerTransformerEnd.x>0.1</cim:PowerTransformerEnd.x>
 </cim:PowerTransformerEnd>
 <cim:OperationalLimitType rdf:ID="permanent">
  <{kind} rdf:resource="{namespace}LimitKind.patl"/>
  <cim:OperationalLimitType.isInfiniteDuration>true</cim:OperationalLimitType.isInfiniteDuration>
 </cim:OperationalLimitType>
 <cim:OperationalLimitType rdf:ID="temporary">
  <{kind} rdf:resource="{namespace}LimitKind.tatl"/>
  <cim:OperationalLimitType.isInfiniteDuration>false</cim:OperationalLimitType.isInfiniteDuration>
  <cim:OperationalLimitType.acceptableDuration>60</cim:OperationalLimitType.acceptableDuration>
 </cim:OperationalLimitType>
 {limits}
</rdf:RDF>"##
    );
    powerio_tx::parse(
        Source::from_memory("limits.xml", text.into_bytes())
            .unwrap()
            .with_format(FormatId::new("cgmes").unwrap()),
    )
    .unwrap()
}

fn limit(class: &str, id: &str, target: &str, kind: &str, values: &str) -> String {
    let association = if target.starts_with("terminal-") {
        "Terminal"
    } else {
        "Equipment"
    };
    format!(
        r##"<cim:OperationalLimitSet rdf:ID="set-{id}">
 <cim:OperationalLimitSet.{association} rdf:resource="#{target}"/>
</cim:OperationalLimitSet>
<cim:{class} rdf:ID="{id}">
 <cim:OperationalLimit.OperationalLimitSet rdf:resource="#set-{id}"/>
 <cim:OperationalLimit.OperationalLimitType rdf:resource="#{kind}"/>
 {values}
</cim:{class}>"##
    )
}

fn canonical_round_trip(module: PioModule<BalancedNetwork>) -> PioModule<BalancedNetwork> {
    let output = tempfile::tempdir().unwrap();
    let path = output.path().join("cgmes");
    let emitted = powerio_tx::emit(
        &module.sever_source(),
        TargetFormat::Cgmes,
        Destination::path(&path),
    )
    .unwrap();
    assert_eq!(emitted.fidelity(), Fidelity::Canonical);
    powerio_tx::parse(Source::open(path).unwrap()).unwrap()
}

fn ir_round_trip(module: &PioModule<BalancedNetwork>) -> PioModule<BalancedNetwork> {
    let stored = powerio::serialize(module, Destination::memory("module").unwrap()).unwrap();
    let EmittedOutput::Memory { mut artifacts } = stored.into_output() else {
        panic!("expected memory output");
    };
    let restored = powerio::deserialize(
        Source::from_memory("module.pio.json", artifacts.pop().unwrap().into_bytes()).unwrap(),
    )
    .unwrap();
    // Deserialization retains the IR document, never the original CGMES bytes.
    assert_eq!(restored.source().unwrap().name(), "module.pio.json");
    for diagnostic in module.diagnostics() {
        assert!(restored.diagnostics().iter().any(|restored| {
            restored.code() == diagnostic.code() && restored.message() == diagnostic.message()
        }));
    }
    restored.map_value(|value| match value {
        powerio::PioValue::BalancedNetwork(network) => network,
        _ => panic!("expected balanced network"),
    })
}

fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-10,
        "expected {expected}, got {actual}"
    );
}

#[test]
fn active_limits_survive_fresh_emission_and_report_the_distinct_default() {
    for class in ["CurrentLimit", "ActivePowerLimit", "ApparentPowerLimit"] {
        for (kind, temporary) in [("permanent", false), ("temporary", true)] {
            let values = format!(
                "<cim:{class}.normalValue>100</cim:{class}.normalValue>\
                 <cim:{class}.value>150</cim:{class}.value>"
            );
            let module = transformer(3, &limit(class, "limit", "terminal-lv", kind, &values));
            let expected = if class == "CurrentLimit" {
                3f64.sqrt() * 20.0 * 150.0 / 1000.0
            } else {
                150.0
            };
            assert!(module.diagnostics().iter().any(|diagnostic| {
                diagnostic.code() == "READ.CGMES.FIELD_UNMAPPED"
                    && diagnostic
                        .message()
                        .contains(&format!("{class}.normalValue=100"))
                    && diagnostic.message().contains(&format!("{class}.value=150"))
            }));
            let stored = ir_round_trip(&module);
            let fresh = canonical_round_trip(stored.clone());
            for parsed in [&module, &stored, &fresh] {
                let branch = &parsed.value().branches()[0];
                assert_close(
                    if temporary {
                        branch.rate_c
                    } else {
                        branch.rate_a
                    },
                    expected,
                );
                let detailed = parsed.value().detailed_connectivity().as_ref().unwrap();
                let group = &detailed.operational_limit_groups[0];
                let loading = match class {
                    "CurrentLimit" => &group.current_limits,
                    "ActivePowerLimit" => &group.active_power_limits,
                    _ => &group.apparent_power_limits,
                }
                .as_ref()
                .unwrap();
                assert_close(
                    if temporary {
                        loading.temporary_limits[0].value
                    } else {
                        loading.permanent_limit.unwrap()
                    },
                    150.0,
                );
            }
        }
    }
}

#[test]
fn default_only_and_identical_active_limits_need_no_distinct_default_diagnostic() {
    for values in [
        "<cim:CurrentLimit.normalValue>100</cim:CurrentLimit.normalValue>",
        "<cim:CurrentLimit.value>100</cim:CurrentLimit.value>",
        "<cim:CurrentLimit.normalValue>100</cim:CurrentLimit.normalValue><cim:CurrentLimit.value>100</cim:CurrentLimit.value>",
    ] {
        let module = transformer(
            3,
            &limit("CurrentLimit", "limit", "terminal-hv", "permanent", values),
        );
        assert_close(
            module.value().branches()[0].rate_a,
            3f64.sqrt() * 220.0 * 100.0 / 1000.0,
        );
        assert!(
            module
                .diagnostics()
                .iter()
                .all(|diagnostic| { !diagnostic.message().contains("distinct normal limit") })
        );
        let group = &module
            .value()
            .detailed_connectivity()
            .as_ref()
            .unwrap()
            .operational_limit_groups[0];
        assert_close(
            group
                .current_limits
                .as_ref()
                .unwrap()
                .permanent_limit
                .unwrap(),
            100.0,
        );
    }
}

#[test]
fn transformer_current_limits_use_the_target_winding_voltage() {
    for version in [2, 3] {
        for (hv, lv) in [("terminal-hv", "terminal-lv"), ("end-hv", "end-lv")] {
            let limits = limit(
                "CurrentLimit",
                "hv-limit",
                hv,
                "permanent",
                "<cim:CurrentLimit.value>1000</cim:CurrentLimit.value>",
            ) + &limit(
                "CurrentLimit",
                "lv-limit",
                lv,
                "permanent",
                "<cim:CurrentLimit.value>500</cim:CurrentLimit.value>",
            );
            let module = transformer(version, &limits);
            // S = sqrt(3) * U * I. The 20 kV terminal binds at 17.32 MVA,
            // independent of terminal sequence and transformer end ordering.
            assert_close(
                module.value().branches()[0].rate_a,
                3f64.sqrt() * 20.0 * 500.0 / 1000.0,
            );
        }
    }
}

#[test]
fn equipment_power_limits_still_constrain_a_transformer() {
    let limits = limit(
        "CurrentLimit",
        "lv-limit",
        "terminal-lv",
        "permanent",
        "<cim:CurrentLimit.value>500</cim:CurrentLimit.value>",
    ) + &limit(
        "ApparentPowerLimit",
        "equipment-limit",
        "transformer",
        "permanent",
        "<cim:ApparentPowerLimit.value>10</cim:ApparentPowerLimit.value>",
    );
    let module = transformer(3, &limits);
    assert_close(module.value().branches()[0].rate_a, 10.0);
}

#[test]
fn imported_tap_changer_identity_survives_ir_and_fresh_emission() {
    const TAP_ID: &str = "20000000-0000-4000-8000-000000000001";
    for (class, end_property, increment) in [
        (
            "RatioTapChanger",
            "RatioTapChanger",
            "RatioTapChanger.stepVoltageIncrement",
        ),
        (
            "PhaseTapChangerLinear",
            "PhaseTapChanger",
            "PhaseTapChangerLinear.stepPhaseShiftIncrement",
        ),
    ] {
        let tap = format!(
            r##"<cim:{class} rdf:ID="{TAP_ID}">
 <cim:IdentifiedObject.name>Imported tap</cim:IdentifiedObject.name>
 <cim:{end_property}.TransformerEnd rdf:resource="#end-hv"/>
 <cim:TapChanger.lowStep>-1</cim:TapChanger.lowStep>
 <cim:TapChanger.highStep>1</cim:TapChanger.highStep>
 <cim:TapChanger.neutralStep>0</cim:TapChanger.neutralStep>
 <cim:TapChanger.normalStep>0</cim:TapChanger.normalStep>
 <cim:TapChanger.step>1</cim:TapChanger.step>
 <cim:{increment}>1.25</cim:{increment}>
</cim:{class}>"##
        );
        for version in [2, 3] {
            for legacy_metadata in [false, true] {
                let mut module = transformer(version, &tap);
                if legacy_metadata {
                    // Older IR has the right CGMES identity in generic metadata.
                    let detailed = std::sync::Arc::make_mut(
                        module
                            .value_mut()
                            .detailed_connectivity_mut()
                            .as_mut()
                            .unwrap(),
                    );
                    for record in &mut detailed.component_metadata {
                        if record.component.local_id() == TAP_ID {
                            record.component =
                                powerio_core::ComponentId::new("cgmes_object", TAP_ID).unwrap();
                        }
                    }
                }
                let fresh = canonical_round_trip(ir_round_trip(&module));
                let detailed = fresh.value().detailed_connectivity().as_ref().unwrap();
                assert_eq!(detailed.tap_changers.len(), 1);
                let component = detailed.tap_changers[0].component.as_ref().unwrap();
                assert_eq!(component.local_id(), TAP_ID);
                assert!(detailed.component_metadata.iter().any(|record| {
                    record.component == *component && record.name.as_deref() == Some("Imported tap")
                }));
                assert_close(
                    fresh.value().branches()[0].tap,
                    module.value().branches()[0].tap,
                );
                assert_close(
                    fresh.value().branches()[0].shift,
                    module.value().branches()[0].shift,
                );
            }
        }
    }
}
