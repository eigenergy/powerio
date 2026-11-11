//! MG-RAVENS and distribution CIM are both CIM100 derived: the same classes,
//! the same `IdentifiedObject.mRID`, the same `PhaseCode` vocabulary. A
//! feeder moving between them keeps its electrical values and its
//! identifiers, except for the losses each writer names in a diagnostic.

#[path = "helpers/electrical_projection.rs"]
mod electrical_projection;
mod helpers;
#[path = "helpers/source_equivalents.rs"]
mod source_equivalents;

use helpers::{Conv, Parsed};
use powerio_dist::{DistTargetFormat, MulticonductorNetwork};
use std::path::PathBuf;

fn case(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data/dist")
        .join(rel)
}

fn has(out: &Conv, code: &str, prefix: &str) -> bool {
    out.diagnostics
        .iter()
        .any(|d| d.code() == code && d.message().starts_with(prefix))
}

/// Apply the named, diagnosed RAVENS losses to the expected network: the
/// pairs beyond the first of a three winding tank, and regulator taps.
/// Each one must carry its own finding; nothing else is waived.
fn ravens_expected(net: &MulticonductorNetwork, out: &Conv) -> MulticonductorNetwork {
    let (expected, problems) = electrical_projection::ravens_projection(net, &out.diagnostics);
    assert!(problems.is_empty(), "{}", problems.join("; "));
    let (expected, problems) =
        electrical_projection::ravens_transformer_projection(&expected, &out.diagnostics);
    assert!(problems.is_empty(), "{}", problems.join("; "));
    expected
}

/// A `PhaseCode` is a set: a winding whose terminals are not in A, B, C, N
/// order (a center tap's second half, `[4, 2]`) is written in that order,
/// with a finding.
fn cim_expected(net: &MulticonductorNetwork, out: &Conv) -> MulticonductorNetwork {
    let mut expected = net.clone();
    for transformer in expected.transformers_mut() {
        let name = transformer.name.clone();
        for (index, winding) in transformer.windings.iter_mut().enumerate() {
            let mut ordered = winding.terminal_map.clone();
            ordered.sort();
            if ordered != winding.terminal_map {
                let prefix = format!("transformer {name} winding {}: terminal order", index + 1);
                assert!(
                    has(out, "EMIT.CIM_DIST.VALUE_COLLAPSED", &prefix),
                    "{prefix}: no finding; {:?}",
                    out.warnings
                );
                winding.terminal_map = ordered;
            }
        }
    }
    expected
}

fn assert_same_circuit(label: &str, before: &MulticonductorNetwork, after: &MulticonductorNetwork) {
    let before = source_equivalents::contract_source_pairs(before);
    let after = source_equivalents::contract_source_pairs(after);
    source_equivalents::assert_source_constraints(&before, &after, label);
    let differences = electrical_projection::differences(&before, &after);
    assert!(
        differences.is_empty(),
        "{label}: {}",
        differences.join("; ")
    );
}

fn to_ravens(net: &Parsed) -> (Conv, Parsed) {
    let out = net.emit_value(DistTargetFormat::RavensJson);
    let back = helpers::parse_str(&out.text, "ravens-json").unwrap();
    (out, back)
}

fn to_cim(net: &MulticonductorNetwork) -> (Conv, Parsed) {
    let out = helpers::emit_value(net, DistTargetFormat::CimDist);
    let back = helpers::parse_str(&out.text, "cim-dist").unwrap();
    (out, back)
}

#[test]
fn cim_feeders_keep_their_circuit_through_ravens() {
    for rel in ["cim/micro_feeder.xml", "cim/tank_feeder.xml"] {
        let net = helpers::parse_file(case(rel), Some("cim-dist")).unwrap();
        let (out, back) = to_ravens(&net);
        assert_same_circuit(rel, &ravens_expected(&net, &out), &back);
        // And on into CIM again: the second hop loses nothing further
        // than the PhaseCode orders the CIM writer names.
        let (out, again) = to_cim(&back);
        assert_same_circuit(
            &format!("{rel} -> ravens -> cim"),
            &cim_expected(&back, &out),
            &again,
        );
    }
}

/// An open delta bank's tanks have one phase to phase secondary (`AB`,
/// `BC`). Counting its two conductors as two phases halves the per phase
/// rating and moves the primary to a line to neutral base.
#[test]
fn open_delta_tank_stays_single_phase_through_ravens() {
    let net = helpers::parse_file(case("cim/tank_feeder.xml"), Some("cim-dist")).unwrap();
    let (_, back) = to_ravens(&net);
    for name in ["od_t1", "od_t2"] {
        let before = net.transformers().iter().find(|t| t.name == name).unwrap();
        let after = back.transformers().iter().find(|t| t.name == name).unwrap();
        assert_eq!(before.phases, 1, "{name}");
        assert_eq!(after.phases, 1, "{name}");
        assert_eq!(
            after.windings[1].terminal_map,
            before.windings[1].terminal_map
        );
    }
}

#[test]
fn ravens_case_keeps_its_circuit_through_cim() {
    let net = helpers::parse_file(case("ravens/case3_balanced.json"), Some("ravens-json")).unwrap();
    let (out, back) = to_cim(&net);
    assert_same_circuit("ravens case3 -> cim", &net, &back);
    // The distribution CIM writer has no operational limit mapping yet; the
    // segment ampacity is a named loss, not a silent one.
    for line in ["ohline", "quad"] {
        let prefix = format!("line {line}: ampacity and apparent power ratings are not written");
        assert!(
            has(&out, "EMIT.CIM_DIST.FIELD_DROPPED", &prefix),
            "{prefix}: {:?}",
            out.warnings
        );
    }
    let (_, again) = to_ravens(&back);
    assert_same_circuit("ravens case3 -> cim -> ravens", &back, &again);
}

/// Every element but a transformer keeps its mRID in both directions, and
/// neither writer reports the other's id as an unmapped extra.
#[test]
fn mrids_survive_between_cim_and_ravens() {
    fn ids(net: &MulticonductorNetwork, key: &str) -> Vec<(String, Option<String>)> {
        let get = |extras: &powerio_dist::Extras| {
            extras.get(key).and_then(|v| v.as_str()).map(str::to_owned)
        };
        let mut out = Vec::new();
        out.extend(
            net.buses()
                .iter()
                .map(|b| (format!("bus {}", b.id), get(&b.extras))),
        );
        out.extend(
            net.line_codes()
                .iter()
                .map(|c| (format!("code {}", c.name), get(&c.extras))),
        );
        out.extend(
            net.lines()
                .iter()
                .map(|l| (format!("line {}", l.name), get(&l.extras))),
        );
        out.extend(
            net.switches()
                .iter()
                .map(|s| (format!("switch {}", s.name), get(&s.extras))),
        );
        out.extend(
            net.loads()
                .iter()
                .map(|l| (format!("load {}", l.name), get(&l.extras))),
        );
        out.extend(
            net.shunts()
                .iter()
                .map(|s| (format!("shunt {}", s.name), get(&s.extras))),
        );
        out.extend(
            net.sources()
                .iter()
                .map(|s| (format!("source {}", s.name), get(&s.extras))),
        );
        out.sort();
        out
    }
    let net = helpers::parse_file(case("cim/micro_feeder.xml"), Some("cim-dist")).unwrap();
    let original = ids(&net, "cim_mrid");
    assert!(original.iter().all(|(_, id)| id.is_some()), "{original:?}");

    let (out, back) = to_ravens(&net);
    assert_eq!(ids(&back, "ravens_mrid"), original, "cim -> ravens");
    let (out_cim, again) = to_cim(&back);
    assert_eq!(ids(&again, "cim_mrid"), original, "cim -> ravens -> cim");

    for (conv, key) in [(&out, "cim_mrid"), (&out_cim, "ravens_mrid")] {
        let leaked: Vec<_> = conv
            .warnings
            .iter()
            .filter(|w| w.contains("EXTRAS_DROPPED") && w.contains(key))
            .filter(|w| !w.contains("transformer "))
            .collect();
        assert!(leaked.is_empty(), "{leaked:?}");
    }

    // The other direction: a RAVENS document's ids carry into CIM.
    let net = helpers::parse_file(case("ravens/case3_balanced.json"), Some("ravens-json")).unwrap();
    let original = ids(&net, "ravens_mrid");
    let (_, back) = to_cim(&net);
    let carried: Vec<_> = original.iter().filter(|(_, id)| id.is_some()).collect();
    assert_ne!(carried, Vec::<&(String, Option<String>)>::new());
    let after = ids(&back, "cim_mrid");
    for (what, id) in carried {
        assert!(
            after.contains(&(what.clone(), id.clone())),
            "{what}: {id:?} not kept; {after:?}"
        );
    }
}

/// Canonical output of a converted feeder is a fixed point in each format.
#[test]
fn converted_feeders_are_canonical_fixed_points() {
    for rel in ["cim/micro_feeder.xml", "cim/tank_feeder.xml"] {
        let net = helpers::parse_file(case(rel), Some("cim-dist")).unwrap();
        let (first, back) = to_ravens(&net);
        let second = back.emit_value(DistTargetFormat::RavensJson);
        assert!(
            first.text == second.text,
            "{rel}: RAVENS write is not stable"
        );
    }
    // A parse from memory names the network, so the CIM fixed point starts
    // from the first CIM readback.
    let net = helpers::parse_file(case("ravens/case3_balanced.json"), Some("ravens-json")).unwrap();
    let (_, back) = to_cim(&net);
    let (first, back) = to_cim(&back);
    let second = back.emit_value(DistTargetFormat::CimDist);
    assert!(first.text == second.text, "case3: CIM write is not stable");
}

/// Upstream documents (the IEEE 13 example at the pinned MG-RAVENS
/// revision) state a transformer's terminals once, on the enclosing
/// `PowerTransformer`, rather than on each `TransformerTankEnd`. Moving every
/// end's terminal onto its parent reads back the same transformers.
#[test]
fn transformer_terminals_on_the_power_transformer_read_the_same() {
    use serde_json::Value;

    fn hoist(value: &mut Value) {
        match value {
            Value::Object(map) => {
                if map.get("Ravens.cimObjectType").and_then(Value::as_str)
                    == Some("PowerTransformer")
                {
                    let mut terminals = Vec::new();
                    for tank in map
                        .get_mut("PowerTransformer.TransformerTank")
                        .and_then(Value::as_array_mut)
                        .into_iter()
                        .flatten()
                    {
                        for end in tank
                            .get_mut("TransformerTank.TransformerTankEnd")
                            .and_then(Value::as_array_mut)
                            .into_iter()
                            .flatten()
                        {
                            let number = end["TransformerEnd.endNumber"].clone();
                            let end = end.as_object_mut().unwrap();
                            let Some(Value::Array(mut own)) =
                                end.remove("ConductingEquipment.Terminals")
                            else {
                                continue;
                            };
                            let mut terminal = own.remove(0);
                            terminal["ACDCTerminal.sequenceNumber"] = number;
                            terminals.push(terminal);
                        }
                    }
                    map.insert(
                        "ConductingEquipment.Terminals".into(),
                        Value::Array(terminals),
                    );
                }
                map.values_mut().for_each(hoist);
            }
            Value::Array(items) => items.iter_mut().for_each(hoist),
            _ => {}
        }
    }

    let net = helpers::parse_file(case("cim/tank_feeder.xml"), Some("cim-dist")).unwrap();
    let (out, direct) = to_ravens(&net);
    let mut doc: Value = serde_json::from_str(&out.text).unwrap();
    hoist(&mut doc);
    let text = serde_json::to_string(&doc).unwrap();
    assert!(!text.contains("\"TransformerTankEnd.Terminals\""));
    let hoisted = helpers::parse_str(&text, "ravens-json").unwrap();
    assert_eq!(
        hoisted.transformers().len(),
        direct.transformers().len(),
        "{:?}",
        hoisted.warnings
    );
    assert_same_circuit("terminals on the PowerTransformer", &direct, &hoisted);
}

/// `opendss2xml` names the regulated end with a `TransformerEnd.RatioTapChanger`
/// pointer. The tap lands on that winding, not on the first winding whose
/// rated voltage matches `neutralU` (a 1:1 regulator matches both).
#[test]
fn ratio_tap_changer_pointer_picks_the_regulated_end() {
    use serde_json::{Value, json};

    fn point(value: &mut Value) {
        match value {
            Value::Object(map) => {
                if map.get("Ravens.cimObjectType").and_then(Value::as_str)
                    == Some("TransformerTank")
                    && map.get("IdentifiedObject.name").and_then(Value::as_str) == Some("rega")
                {
                    let ends = map["TransformerTank.TransformerTankEnd"]
                        .as_array_mut()
                        .unwrap();
                    let end = ends
                        .iter_mut()
                        .find(|e| e["TransformerEnd.endNumber"] == json!(2))
                        .unwrap();
                    end["TransformerEnd.RatioTapChanger"] = json!("RatioTapChanger::'rega_rtc'");
                }
                map.values_mut().for_each(point);
            }
            Value::Array(items) => items.iter_mut().for_each(point),
            _ => {}
        }
    }

    let net = helpers::parse_file(case("cim/tank_feeder.xml"), Some("cim-dist")).unwrap();
    let (out, _) = to_ravens(&net);
    let mut doc: Value = serde_json::from_str(&out.text).unwrap();
    point(&mut doc);
    // As upstream states it: the tap ratio in `step`, neutralU at the 1:1
    // regulator's common rated voltage.
    doc["PowerSystemResource"]["TapChanger"] = json!({"RatioTapChanger": {"rega_rtc": {
        "Ravens.cimObjectType": "RatioTapChanger",
        "IdentifiedObject.name": "rega_rtc",
        "RatioTapChanger.stepVoltageIncrement": 106.25,
        "TapChanger.neutralStep": 0,
        "TapChanger.neutralU": 2400.0,
        "TapChanger.step": 1.0625
    }}});
    let back = helpers::parse_str(&serde_json::to_string(&doc).unwrap(), "ravens-json").unwrap();
    let rega = back
        .transformers()
        .iter()
        .find(|t| t.name == "rega")
        .unwrap();
    let taps: Vec<f64> = rega.windings.iter().map(|w| w.tap).collect();
    assert_eq!(taps, [1.0, 1.0625], "{:?}", back.warnings);
}

/// The MG-RAVENS tools state `ShortCircuitTest.leakageImpedance` as the
/// pair's percent impedance magnitude times the end's base ohms:
/// `opendss2xml` writes sqrt((%r1 + %r2)^2 + %x^2) * Zbase, and
/// PowerModelsDistribution's RAVENS reader recovers %x as
/// sqrt((leak / Zbase)^2 - (%r1 + %r2)^2). The writer matches both.
#[test]
fn leakage_impedance_follows_the_mg_ravens_tools() {
    use serde_json::Value;

    fn find<'v>(value: &'v Value, kind: &str, name: &str) -> Option<&'v Value> {
        match value {
            Value::Object(map) => {
                if map.get("Ravens.cimObjectType").and_then(Value::as_str) == Some(kind)
                    && map.get("IdentifiedObject.name").and_then(Value::as_str) == Some(name)
                {
                    return Some(value);
                }
                map.values().find_map(|v| find(v, kind, name))
            }
            Value::Array(items) => items.iter().find_map(|v| find(v, kind, name)),
            _ => None,
        }
    }

    let net = helpers::parse_file(case("cim/tank_feeder.xml"), Some("cim-dist")).unwrap();
    let sp = net
        .transformers()
        .iter()
        .find(|t| t.name == "sp_tank")
        .unwrap();
    let (v, s) = (sp.windings[0].v_ref, sp.windings[0].s_rating);
    let r_pair = sp.windings[0].r_pct + sp.windings[1].r_pct;
    let z_base = v * v / s;
    let (out, back) = to_ravens(&net);
    let doc: Value = serde_json::from_str(&out.text).unwrap();
    let test = find(&doc, "ShortCircuitTest", "sp_tank_info_1_shortcircuit").unwrap();
    let leak = test["ShortCircuitTest.leakageImpedance"].as_f64().unwrap();
    let expected = sp.xsc_pct[0].hypot(r_pair) * z_base;
    assert!(
        (leak - expected).abs() <= 1e-9 * expected,
        "{leak} vs {expected}"
    );
    let pmd_x = ((leak / z_base).powi(2) - r_pair.powi(2)).sqrt();
    assert!((pmd_x - sp.xsc_pct[0]).abs() < 1e-9, "{pmd_x}");
    let read = back
        .transformers()
        .iter()
        .find(|t| t.name == "sp_tank")
        .unwrap();
    assert!((read.xsc_pct[0] - sp.xsc_pct[0]).abs() < 1e-9);
}

/// The `New Line` statements whose two ends name different node lists.
/// PMD's Kron reduction (`transform_data_model`) refuses a line whose
/// `f_connections` differ from its `t_connections`.
fn lines_with_mismatched_nodes(dss: &str) -> Vec<&str> {
    fn nodes<'a>(statement: &'a str, key: &str) -> Option<&'a str> {
        statement
            .split_whitespace()
            .find_map(|token| token.strip_prefix(key))
            .map(|bus| bus.split_once('.').map_or("", |(_, nodes)| nodes))
    }
    dss.lines()
        .filter(|statement| statement.starts_with("New Line."))
        .filter(|statement| nodes(statement, "bus1=") != nodes(statement, "bus2="))
        .collect()
}

#[test]
fn dss_helper_lines_name_the_same_nodes_at_both_ends() {
    // #765: a RAVENS or CIM source has a grounded neutral, so the OpenDSS
    // writer states it as one single phase source per conductor behind a
    // helper line. That line joined node 1 of its private bus to phase B or
    // C of the source bus.
    let ravens =
        helpers::parse_file(case("ravens/case3_balanced.json"), Some("ravens-json")).unwrap();
    let (_, cim) = to_cim(&ravens);
    for (what, net) in [("ravens", &ravens), ("cim", &cim)] {
        let dss = net.emit_value(DistTargetFormat::Dss).text;
        assert!(dss.contains("_pio_ideal_"), "{what}: {dss}");
        let mismatched = lines_with_mismatched_nodes(&dss);
        assert!(mismatched.is_empty(), "{what}: {mismatched:#?}");
    }
}
