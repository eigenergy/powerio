//! CGMES transformer projection preserves its circuit and active terminal limits.
use std::fmt::Write;

use powerio_core::{Destination, EmittedOutput, Fidelity, FormatId, PioModule, Source};
use powerio_tx::{BalancedNetwork, TargetFormat};

// Small synthetic transformer: terminal sequence deliberately opposes winding order.
fn transformer_document(version: u8, limits: &str) -> String {
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
    format!(
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
    )
}

fn parse_document(text: String) -> PioModule<BalancedNetwork> {
    powerio_tx::parse(
        Source::from_memory("limits.xml", text.into_bytes())
            .unwrap()
            .with_format(FormatId::new("cgmes").unwrap()),
    )
    .unwrap()
}

fn transformer(version: u8, limits: &str) -> PioModule<BalancedNetwork> {
    parse_document(transformer_document(version, limits))
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

fn ratio_tap(end: &str, ratio: f64) -> String {
    let increment = (ratio - 1.0) * 100.0;
    format!(
        r##"<cim:RatioTapChanger rdf:ID="tap-{end}">
 <cim:RatioTapChanger.TransformerEnd rdf:resource="#{end}"/>
 <cim:TapChanger.lowStep>0</cim:TapChanger.lowStep>
 <cim:TapChanger.highStep>1</cim:TapChanger.highStep>
 <cim:TapChanger.neutralStep>0</cim:TapChanger.neutralStep>
 <cim:TapChanger.normalStep>0</cim:TapChanger.normalStep>
 <cim:TapChanger.step>1</cim:TapChanger.step>
 <cim:RatioTapChanger.stepVoltageIncrement>{increment}</cim:RatioTapChanger.stepVoltageIncrement>
</cim:RatioTapChanger>"##
    )
}

fn inverse(r: f64, x: f64) -> (f64, f64) {
    let denominator = r * r + x * x;
    (r / denominator, -x / denominator)
}

fn linear_phase_tap(end: &str, shift: f64) -> String {
    format!(
        r##"<cim:PhaseTapChangerLinear rdf:ID="phase-{end}">
 <cim:PhaseTapChanger.TransformerEnd rdf:resource="#{end}"/>
 <cim:TapChanger.lowStep>0</cim:TapChanger.lowStep>
 <cim:TapChanger.highStep>1</cim:TapChanger.highStep>
 <cim:TapChanger.neutralStep>0</cim:TapChanger.neutralStep>
 <cim:TapChanger.step>1</cim:TapChanger.step>
 <cim:PhaseTapChangerLinear.stepPhaseShiftIncrement>{shift}</cim:PhaseTapChangerLinear.stepPhaseShiftIncrement>
</cim:PhaseTapChangerLinear>"##
    )
}

fn terminal_admittances(branch: &powerio_tx::Branch) -> [(f64, f64); 4] {
    let (g, b) = inverse(branch.r, branch.x);
    let charging = branch.calc_terminal_charging();
    let tap = branch.calc_effective_tap();
    let (sin, cos) = branch.shift.to_radians().sin_cos();
    [
        (
            (g + charging.g_fr) / (tap * tap),
            (b + charging.b_fr) / (tap * tap),
        ),
        ((-g * cos + b * sin) / tap, (-b * cos - g * sin) / tap),
        ((-g * cos - b * sin) / tap, (-b * cos + g * sin) / tap),
        (g + charging.g_to, b + charging.b_to),
    ]
}

// Reproduction from Mohamed Numair in #829, extended to signed shifts,
// unequal voltage levels, and retained ratio changers on either winding.
#[test]
fn generated_phase_taps_use_the_end_one_reactance_base() {
    for version in [2, 3] {
        for tap in [0.9, 1.0, 1.05, 1.2] {
            for shift in [-10.0, 10.0, 25.0] {
                for (rho1, rho2) in [(1.0, 1.0), (1.05, 1.0), (1.0, 1.1), (1.05, 1.1)] {
                    let taps = ratio_tap("end-hv", rho1) + &ratio_tap("end-lv", rho2);
                    let text =
                        transformer_document(version, &taps).replace("End.x>0.1<", "End.x>0<");
                    let mut source = parse_document(text).sever_source();
                    let branch = &mut source.value_mut().branches_mut()[0];
                    branch.r = 0.01;
                    branch.x = 0.1;
                    branch.tap = tap;
                    branch.shift = shift;
                    let expected = branch.clone();
                    let output = tempfile::tempdir().unwrap();
                    let path = output.path().join("cgmes");
                    powerio_tx::emit(&source, TargetFormat::Cgmes, Destination::path(&path))
                        .unwrap();
                    let equipment = std::fs::read_dir(&path)
                        .unwrap()
                        .map(|entry| entry.unwrap().path())
                        .find(|path| path.to_string_lossy().ends_with("_EQ.xml"))
                        .unwrap();
                    let xml = std::fs::read_to_string(equipment).unwrap();
                    let first = xml
                        .split("<cim:PowerTransformerEnd ")
                        .skip(1)
                        .map(|end| end.split("</cim:PowerTransformerEnd>").next().unwrap())
                        .find(|end| end.contains("<cim:TransformerEnd.endNumber>1<"))
                        .unwrap();
                    let number = |text: &str, field: &str| -> f64 {
                        text.split(&format!("<cim:{field}>"))
                            .nth(1)
                            .unwrap()
                            .split('<')
                            .next()
                            .unwrap()
                            .parse()
                            .unwrap()
                    };
                    let end_x = number(first, "PowerTransformerEnd.x");
                    let u1 = 220.0 * tap * rho2 / rho1;
                    assert_close(end_x, 0.1 * u1 * u1 / (100.0 * rho2 * rho2));
                    assert_close(number(&xml, "PhaseTapChangerLinear.xMin"), end_x);
                    assert_close(number(&xml, "PhaseTapChangerLinear.xMax"), end_x);
                    let reread = powerio_tx::parse(Source::open(&path).unwrap()).unwrap();
                    let stored = ir_round_trip(&reread);
                    let second = canonical_round_trip(stored.clone());
                    for module in [&reread, &stored, &second] {
                        let branch = &module.value().branches()[0];
                        for (actual, expected) in [
                            (branch.r, expected.r),
                            (branch.x, expected.x),
                            (branch.tap, expected.tap),
                            (branch.shift, expected.shift),
                        ] {
                            assert_close(actual, expected);
                        }
                        for (actual, expected) in terminal_admittances(branch)
                            .into_iter()
                            .zip(terminal_admittances(&expected))
                        {
                            assert_close(actual.0, expected.0);
                            assert_close(actual.1, expected.1);
                        }
                        let phase = module
                            .value()
                            .detailed_connectivity()
                            .as_ref()
                            .unwrap()
                            .tap_changers
                            .iter()
                            .find(|tap| tap.kind == powerio_tx::TapChangerKind::Phase)
                            .unwrap();
                        assert_eq!(phase.steps.len(), 3);
                        for step in &phase.steps {
                            assert_close(step.reactance_deviation_percent, 0.0);
                        }
                        assert!(
                            !module
                                .diagnostics()
                                .iter()
                                .any(|d| d.message().contains("percentage deviations"))
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn transformer_voltage_bases_preserve_the_original_two_port_circuit() {
    for version in [2, 3] {
        for (u1, u2) in [
            (220.0_f64, 20.0_f64),
            (242.0, 20.0),
            (220.0, 24.0),
            (242.0, 24.0),
        ] {
            for (rho1, rho2) in [(1.0, 1.0), (1.05, 1.0), (1.0, 1.1), (1.05, 1.1)] {
                let taps = ratio_tap("end-hv", rho1)
                    + &ratio_tap("end-lv", rho2)
                    + &linear_phase_tap("end-hv", 11.0)
                    + &linear_phase_tap("end-lv", -4.0);
                let text = transformer_document(version, &taps)
                    .replace("ratedU>220<", &format!("ratedU>{u1}<"))
                    .replace("ratedU>20<", &format!("ratedU>{u2}<"))
                    .replace(
                        "<cim:PowerTransformerEnd.x>10",
                        concat!(
                            "<cim:PowerTransformerEnd.r>2</cim:PowerTransformerEnd.r>",
                            "<cim:PowerTransformerEnd.g>0.0001</cim:PowerTransformerEnd.g>",
                            "<cim:PowerTransformerEnd.b>0.0002</cim:PowerTransformerEnd.b>",
                            "<cim:PowerTransformerEnd.x>10"
                        ),
                    )
                    .replace("End.x>0.1<", "End.x>0<");
                // The normative L circuit has one series impedance and
                // one from-side shunt, both referred to rated voltage 1.
                // Compare all four entries without using reader helpers.
                let (g, b) = inverse(2.0 / u1.powi(2) * 100.0, 10.0 / u1.powi(2) * 100.0);
                let t1 = u1 / 220.0 * rho1;
                let t2 = u2 / 20.0 * rho2;
                let (sin, cos) = (11.0_f64 - -4.0).to_radians().sin_cos();
                let expected = [
                    (
                        (g + 0.0001 * u1.powi(2) / 100.0) / t1.powi(2),
                        (b + 0.0002 * u1.powi(2) / 100.0) / t1.powi(2),
                    ),
                    (
                        (-g * cos + b * sin) / (t1 * t2),
                        (-b * cos - g * sin) / (t1 * t2),
                    ),
                    (
                        (-g * cos - b * sin) / (t1 * t2),
                        (-b * cos + g * sin) / (t1 * t2),
                    ),
                    (g / t2.powi(2), b / t2.powi(2)),
                ];
                let parsed = parse_document(text);
                assert!(
                    !parsed
                        .diagnostics()
                        .iter()
                        .any(|diagnostic| { diagnostic.message().contains("L-model convention") })
                );
                let fresh = canonical_round_trip(ir_round_trip(&parsed));
                for network in [parsed.value(), fresh.value()] {
                    for (actual, expected) in terminal_admittances(&network.branches()[0])
                        .into_iter()
                        .zip(expected)
                    {
                        assert_close(actual.0, expected.0);
                        assert_close(actual.1, expected.1);
                    }
                }
            }
        }
    }
}

#[test]
fn charging_loss_is_reported_for_a_far_transformer_shunt_or_unequal_line_ends() {
    use powerio_tx::{Branch, BranchCharging, Bus, BusId, BusType};
    let asymmetric = BranchCharging::new(0.001, 0.002, 0.003, 0.004);
    let symmetric = BranchCharging::new(0.001, 0.002, 0.001, 0.002);
    let l_model = BranchCharging::new(0.001, 0.002, 0.0, 0.0);
    for (transformer, charging, collapsed) in [
        (true, asymmetric, true),
        (true, symmetric, true),
        (true, BranchCharging::new(0.0, 0.0, 1e-20, 0.0), true),
        (true, l_model, false),
        (false, asymmetric, true),
        (false, symmetric, false),
    ] {
        let mut network = BalancedNetwork::new("terminal charging", 100.0);
        network.buses_mut().extend([
            Bus::new(BusId(1), BusType::Ref, 220.0),
            Bus::new(BusId(2), BusType::Pq, 220.0),
        ]);
        let mut branch = Branch::new(BusId(1), BusId(2), 0.01, 0.1);
        branch.tap = if transformer { 1.05 } else { 0.0 };
        branch.b = charging.calc_total_b();
        branch.charging = Some(charging);
        network.branches_mut().push(branch);
        network.assign_missing_component_ids();
        let output = tempfile::tempdir().unwrap();
        let path = output.path().join("cgmes");
        let result = powerio_tx::emit(
            &PioModule::new(network),
            TargetFormat::Cgmes,
            Destination::path(&path),
        )
        .unwrap();
        assert_eq!(
            result
                .diagnostics()
                .iter()
                .any(|d| d.code() == "EMIT.CGMES.VALUE_COLLAPSED"),
            collapsed
        );
        if transformer {
            assert_zero_secondary_parameters(&path);
        }
        let reread = powerio_tx::parse(Source::open(path).unwrap()).unwrap();
        let actual = reread.value().branches()[0].calc_terminal_charging();
        let expected = if transformer {
            BranchCharging::new(charging.calc_total_g(), charging.calc_total_b(), 0.0, 0.0)
        } else if collapsed {
            BranchCharging::new(
                charging.calc_total_g() / 2.0,
                charging.calc_total_b() / 2.0,
                charging.calc_total_g() / 2.0,
                charging.calc_total_b() / 2.0,
            )
        } else {
            charging
        };
        for (actual, expected) in [
            (actual.g_fr, expected.g_fr),
            (actual.b_fr, expected.b_fr),
            (actual.g_to, expected.g_to),
            (actual.b_to, expected.b_to),
        ] {
            assert_close(actual, expected);
        }
    }
}

fn assert_zero_secondary_parameters(path: &std::path::Path) {
    let equipment = std::fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.to_string_lossy().ends_with("_EQ.xml"))
        .unwrap();
    let xml = std::fs::read_to_string(equipment).unwrap();
    let second = xml
        .split("<cim:PowerTransformerEnd ")
        .skip(1)
        .map(|end| end.split("</cim:PowerTransformerEnd>").next().unwrap())
        .find(|end| end.contains("<cim:TransformerEnd.endNumber>2<"))
        .unwrap();
    for field in ["r", "x", "g", "b"] {
        assert!(second.contains(&format!("<cim:PowerTransformerEnd.{field}>0<")));
    }
}

#[test]
fn transformer_base_conversion_rejects_overflowing_impedance_and_charging() {
    use powerio_tx::{Branch, BranchCharging, Bus, BusId, BusType};
    let text = transformer_document(3, "");
    let large_impedance = text
        .replace("ratedU>20<", "ratedU>2000<")
        .replace("End.x>10<", "End.x>1e308<");
    let large_charging = text
        .replace("ratedU>20<", "ratedU>2e-155<")
        .replace("End.x>0.1<", "End.x>0<")
        .replace(
            "<cim:PowerTransformerEnd.x>10",
            concat!(
                "<cim:PowerTransformerEnd.g>1</cim:PowerTransformerEnd.g>",
                "<cim:PowerTransformerEnd.x>10"
            ),
        );
    let large_ratio = transformer_document(3, &ratio_tap("end-hv", 1e306))
        .replace("ratedU>220<", "ratedU>1e154<");
    for text in [large_impedance, large_charging, large_ratio] {
        let result = powerio_tx::parse(
            Source::from_memory("overflow.xml", text.into_bytes())
                .unwrap()
                .with_format(FormatId::new("cgmes").unwrap()),
        );
        assert!(result.unwrap_err().to_string().contains("overflows"));
    }
    for (kv, r, g) in [(220.0, 1e308, 0.001), (1e-100, 0.01, 1e308)] {
        let mut network = BalancedNetwork::new("winding overflow", 100.0);
        network.buses_mut().extend([
            Bus::new(BusId(1), BusType::Ref, kv),
            Bus::new(BusId(2), BusType::Pq, kv),
        ]);
        let mut branch = Branch::new(BusId(1), BusId(2), r, 0.1);
        branch.tap = 1.0;
        branch.charging = Some(BranchCharging::new(g, 0.0, 0.0, 0.0));
        network.branches_mut().push(branch);
        network.assign_missing_component_ids();
        let result = powerio_tx::emit(
            &PioModule::new(network),
            TargetFormat::Cgmes,
            Destination::memory("overflow").unwrap(),
        );
        assert!(result.unwrap_err().to_string().contains("overflows"));
    }
}

#[test]
fn only_the_active_phase_tap_ratio_warns_about_the_calculation_projection() {
    const TAP_ID: &str = "20000000-0000-4000-8000-000000000002";
    for version in [2, 3] {
        for class in ["PhaseTapChangerTabular", "PhaseTapChangerAsymmetrical"] {
            for active in [0, 1] {
                let fields = if class == "PhaseTapChangerTabular" {
                    r##"<cim:PhaseTapChangerTabular.PhaseTapChangerTable rdf:resource="#phase-table"/>"##
                } else {
                    concat!(
                        "<cim:PhaseTapChangerNonLinear.voltageStepIncrement>20</cim:PhaseTapChangerNonLinear.voltageStepIncrement>",
                        "<cim:PhaseTapChangerAsymmetrical.windingConnectionAngle>60</cim:PhaseTapChangerAsymmetrical.windingConnectionAngle>"
                    )
                };
                let mut tap = format!(
                    r##"<cim:{class} rdf:ID="{TAP_ID}">
 <cim:PhaseTapChanger.TransformerEnd rdf:resource="#end-hv"/>
 <cim:TapChanger.lowStep>0</cim:TapChanger.lowStep>
 <cim:TapChanger.highStep>1</cim:TapChanger.highStep>
 <cim:TapChanger.neutralStep>0</cim:TapChanger.neutralStep>
 <cim:TapChanger.normalStep>0</cim:TapChanger.normalStep>
 <cim:TapChanger.step>{active}</cim:TapChanger.step>
 {fields}
</cim:{class}>"##
                );
                if class == "PhaseTapChangerTabular" {
                    tap.push_str(r#"<cim:PhaseTapChangerTable rdf:ID="phase-table"/>"#);
                    for (step, rho, angle) in [(0, 1.0, 0.0), (1, 1.1, 5.0)] {
                        write!(
                            tap,
                            r##"<cim:PhaseTapChangerTablePoint rdf:ID="point-{step}">
 <cim:PhaseTapChangerTablePoint.PhaseTapChangerTable rdf:resource="#phase-table"/>
 <cim:TapChangerTablePoint.step>{step}</cim:TapChangerTablePoint.step>
 <cim:TapChangerTablePoint.ratio>{rho}</cim:TapChangerTablePoint.ratio>
 <cim:PhaseTapChangerTablePoint.angle>{angle}</cim:PhaseTapChangerTablePoint.angle>
</cim:PhaseTapChangerTablePoint>"##
                        )
                        .unwrap();
                    }
                }
                let parsed = transformer(version, &tap);
                let stored = ir_round_trip(&parsed);
                let fresh = canonical_round_trip(stored.clone());
                for module in [&parsed, &stored, &fresh] {
                    let warning = module.diagnostics().iter().find(|diagnostic| {
                        diagnostic.code() == "READ.CGMES.VALUE_APPROXIMATED"
                            && diagnostic.message().contains(TAP_ID)
                            && diagnostic.message().contains("voltage ratio")
                    });
                    assert_eq!(warning.is_some(), active == 1);
                    if let Some(warning) = warning {
                        assert!(warning.message().contains("active step 1"));
                    }
                    let detail = module.value().detailed_connectivity().as_ref().unwrap();
                    let step = detail.tap_changers[0]
                        .steps
                        .iter()
                        .find(|step| step.position == active)
                        .unwrap();
                    assert_close(module.value().branches()[0].shift, step.alpha_degrees);
                    assert_close(module.value().branches()[0].tap, 1.0);
                    assert_eq!(step.rho > 1.0, active == 1);
                }
            }
        }
    }
}

#[test]
fn active_ratio_tap_magnitudes_must_be_positive() {
    for end in ["end-hv", "end-lv"] {
        for rho in [0.0, -1.0] {
            for tabular in [false, true] {
                let mut tap = ratio_tap(end, rho);
                if tabular {
                    tap = tap.replace(
                        "</cim:RatioTapChanger>",
                        concat!(
                            "<cim:RatioTapChanger.RatioTapChangerTable rdf:resource=\"#ratio-table\"/>",
                            "</cim:RatioTapChanger>"
                        ),
                    );
                    write!(
                        tap,
                        r##"<cim:RatioTapChangerTable rdf:ID="ratio-table"/>
<cim:RatioTapChangerTablePoint rdf:ID="ratio-point">
 <cim:RatioTapChangerTablePoint.RatioTapChangerTable rdf:resource="#ratio-table"/>
 <cim:TapChangerTablePoint.step>1</cim:TapChangerTablePoint.step>
 <cim:TapChangerTablePoint.ratio>{rho}</cim:TapChangerTablePoint.ratio>
</cim:RatioTapChangerTablePoint>"##
                    )
                    .unwrap();
                }
                let result = powerio_tx::parse(
                    Source::from_memory("ratio.xml", transformer_document(3, &tap).into_bytes())
                        .unwrap()
                        .with_format(FormatId::new("cgmes").unwrap()),
                );
                assert!(
                    result
                        .unwrap_err()
                        .to_string()
                        .contains("positive voltage ratio")
                );
            }
            let mut module = transformer(3, &ratio_tap(end, 1.0));
            let detail = std::sync::Arc::make_mut(
                module
                    .value_mut()
                    .detailed_connectivity_mut()
                    .as_mut()
                    .unwrap(),
            );
            detail.tap_changers[0]
                .steps
                .iter_mut()
                .find(|step| step.position == 1)
                .unwrap()
                .rho = rho;
            let result = powerio_tx::emit(
                &module.sever_source(),
                TargetFormat::Cgmes,
                Destination::memory("invalid-ratio").unwrap(),
            );
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("positive voltage ratios")
            );
        }
    }
}

#[test]
fn nonconforming_transformer_ends_report_the_assumed_common_parameter_base() {
    for version in [2, 3] {
        for (x1, x2, reference_kv, assumption) in [
            (0.0, 0.1, 20.0, "all series data is on end 2"),
            (10.0, 0.1, 220.0, "assuming end 1 rated voltage"),
        ] {
            let text = transformer_document(version, "")
                .replace("End.x>10<", &format!("End.x>{x1}<"))
                .replace("End.x>0.1<", &format!("End.x>{x2}<"))
                .replace(
                    "<cim:PowerTransformerEnd.ratedU>220",
                    concat!(
                        "<cim:PowerTransformerEnd.g>0.001</cim:PowerTransformerEnd.g>",
                        "<cim:PowerTransformerEnd.b>0.002</cim:PowerTransformerEnd.b>",
                        "<cim:PowerTransformerEnd.ratedU>220"
                    ),
                )
                .replace(
                    "<cim:PowerTransformerEnd.ratedU>20",
                    concat!(
                        "<cim:PowerTransformerEnd.g>0.003</cim:PowerTransformerEnd.g>",
                        "<cim:PowerTransformerEnd.b>0.004</cim:PowerTransformerEnd.b>",
                        "<cim:PowerTransformerEnd.ratedU>20"
                    ),
                );
            let parsed = parse_document(text);
            assert!(parsed.diagnostics().iter().any(|diagnostic| {
                diagnostic.code() == "READ.CGMES.VALUE_APPROXIMATED"
                    && diagnostic.message().contains("L-model convention")
                    && diagnostic.message().contains(assumption)
            }));
            let stored = ir_round_trip(&parsed);
            let fresh = canonical_round_trip(stored.clone());
            for network in [parsed.value(), stored.value(), fresh.value()] {
                let branch = &network.branches()[0];
                let z_base = reference_kv * reference_kv / 100.0;
                assert_close(branch.x, (x1 + x2) / z_base);
                let shunt = branch.calc_terminal_charging();
                assert_close(shunt.g_fr, 0.004 * z_base);
                assert_close(shunt.b_fr, 0.006 * z_base);
                assert_close(shunt.g_to, 0.0);
                assert_close(shunt.b_to, 0.0);
            }
        }
    }
}

fn table_tap_with_deviation(class: &str, kind: &str, active: i32, field: &str) -> String {
    let mut tap = format!(
        r##"<cim:{class} rdf:ID="20000000-0000-4000-8000-000000000003">
 <cim:{kind}.TransformerEnd rdf:resource="#end-hv"/>
 <cim:TapChanger.lowStep>0</cim:TapChanger.lowStep>
 <cim:TapChanger.highStep>1</cim:TapChanger.highStep>
 <cim:TapChanger.neutralStep>0</cim:TapChanger.neutralStep>
 <cim:TapChanger.step>{active}</cim:TapChanger.step>
 <cim:{class}.{kind}Table rdf:resource="#table"/>
</cim:{class}>
<cim:{kind}Table rdf:ID="table"/>"##
    );
    for step in [0, 1] {
        let deviation = if step == 1 { 12.5 } else { 0.0 };
        write!(
            tap,
            r##"<cim:{kind}TablePoint rdf:ID="point-{step}">
 <cim:{kind}TablePoint.{kind}Table rdf:resource="#table"/>
 <cim:TapChangerTablePoint.step>{step}</cim:TapChangerTablePoint.step>
 <cim:TapChangerTablePoint.ratio>1</cim:TapChangerTablePoint.ratio>
 <cim:TapChangerTablePoint.{field}>{deviation}</cim:TapChangerTablePoint.{field}>
</cim:{kind}TablePoint>"##
        )
        .unwrap();
    }
    tap
}

#[test]
fn active_tap_parameter_deviations_warn_even_when_the_voltage_ratio_is_one() {
    for version in [2, 3] {
        for (class, kind) in [
            ("RatioTapChanger", "RatioTapChanger"),
            ("PhaseTapChangerTabular", "PhaseTapChanger"),
        ] {
            for field in ["r", "x", "g", "b"] {
                for active in [0, 1] {
                    let parsed = transformer(
                        version,
                        &table_tap_with_deviation(class, kind, active, field),
                    );
                    let stored = ir_round_trip(&parsed);
                    let fresh = canonical_round_trip(stored.clone());
                    for module in [&parsed, &stored, &fresh] {
                        let warning = module.diagnostics().iter().find(|diagnostic| {
                            diagnostic.code() == "READ.CGMES.VALUE_APPROXIMATED"
                                && diagnostic.message().contains("percentage deviations")
                        });
                        assert_eq!(warning.is_some(), active == 1);
                        if let Some(warning) = warning {
                            assert!(warning.message().contains("active step 1"));
                            assert!(warning.message().contains("12.5"));
                        }
                        let detail = module.value().detailed_connectivity().as_ref().unwrap();
                        let step = detail.tap_changers[0]
                            .steps
                            .iter()
                            .find(|step| step.position == 1)
                            .unwrap();
                        assert_close(step.rho, 1.0);
                        let value = match field {
                            "r" => step.resistance_deviation_percent,
                            "x" => step.reactance_deviation_percent,
                            "g" => step.conductance_deviation_percent,
                            "b" => step.susceptance_deviation_percent,
                            _ => unreachable!(),
                        };
                        assert_close(value, 12.5);
                    }
                }
            }
        }
    }
}
