//! Wire catalog line impedance from every reader that states conductors:
//! an OpenDSS geometry (overhead, concentric-neutral and tape-shielded
//! cable), and the same conductors as CIM and MG-RAVENS wire catalog lines
//! (`WireSpacingInfo`, `WirePosition`, `OverheadWireInfo`,
//! `ConcentricNeutralCableInfo`, `TapeShieldCableInfo`). All three go
//! through the one DSS line constants kernel.
//!
//! The fixtures are written for these tests from the conductor data of the
//! MG-RAVENS `IEEE13_Assets` example; no file of that repository is
//! vendored. The frozen numbers are OpenDSS's (DSS C-API 0.14.5 through
//! OpenDSSDirect.py 0.9.4) for [`DSS`], per foot divided by 0.3048.

use std::fmt::Write as _;

use powerio_dist::{DistTargetFormat, MulticonductorNetwork};
use serde_json::{Value, json};

mod helpers;
use helpers::{emit_value, parse_dss_str, parse_str};

const IN: f64 = 0.0254;
const FT: f64 = 0.3048;
const MI: f64 = 1609.344;

const DSS: &str = "Clear
New Circuit.wires basekv=4.16 pu=1.0 phases=3 bus1=src MVAsc3=2000 MVAsc1=2100
New WireData.ACSR_556_5 NormAmps=730 DIAM=0.927 GMRac=0.37320 Rdc=0.035227273 Runits=kft Radunits=in gmrunits=in
New WireData.ACSR_4/0 NormAmps=340 DIAM=0.563 GMRac=0.09768 Rdc=0.112121212 Runits=kft Radunits=in gmrunits=in
New WireData.CU_1/0 NormAmps=100 DIAM=0.368 GMRac=0.13356 Rac=0.607 Runits=mi Radunits=in gmrunits=in
New TSData.TS_1/0 NormAmps=165 DIAM=0.368 GMRac=0.13320 Rac=0.97 Runits=mi Radunits=in gmrunits=in
~ EpsR=2.3 InsLayer=0.220 DiaIns=0.82 DiaCable=1.06 DiaShield=0.88 TapeLayer=0.005 TapeLap=20
New CNData.CN_250 NormAmps=260 DIAM=0.567 GMRac=0.20520 Rac=0.41 Runits=mi Radunits=in gmrunits=in
~ EpsR=2.3 InsLayer=0.220 DiaIns=1.06 DiaCable=1.29 k=13 DiaStrand=0.0641 GmrStrand=0.02496 Rstrand=14.8722
New LineSpacing.500 nconds=4 nphases=3 units=ft x=[-4 -1 3 0] h=[28 28 28 24]
New LineGeometry.601 nconds=4 nphases=3 reduce=y spacing=500 wires=[ACSR_556_5 ACSR_556_5 ACSR_556_5 ACSR_4/0]
New LineGeometry.606 nconds=3 nphases=3 reduce=y
~ cond=1 cncable=CN_250 x=-0.5 h=-4 units=ft
~ cond=2 cncable=CN_250 x=0.0 h=-4 units=ft
~ cond=3 cncable=CN_250 x=0.5 h=-4 units=ft
New LineGeometry.607 nconds=2 nphases=1 reduce=y
~ cond=1 tscable=TS_1/0 x=0.0 h=-4 units=ft
~ cond=2 wire=CU_1/0 x=0.25 h=-4 units=ft
New Line.oh Phases=3 Bus1=src.1.2.3 Bus2=a.1.2.3 Geometry=601 Length=1000 units=ft
New Line.cn Phases=3 Bus1=a.1.2.3 Bus2=b.1.2.3 Geometry=606 Length=500 units=ft
New Line.ts Phases=1 Bus1=b.1 Bus2=c.1 Geometry=607 Length=800 units=ft
New Load.c Bus1=c.1 Phases=1 kV=2.4 kW=100 kvar=50
Set VoltageBases=[4.16]
Calcvoltagebases
Solve
";

/// OpenDSS's per meter series resistance and reactance (ohm/m) and shunt
/// capacitance (nF/m), row major.
struct Reference {
    r: &'static [f64],
    x: &'static [f64],
    c_nf: &'static [f64],
}

const OVERHEAD: Reference = Reference {
    r: &[
        0.000_211_484_727_272_054_68,
        9.800_311_579_299_487e-5,
        9.610_610_749_020_618e-5,
        9.800_311_579_299_487e-5,
        0.000_217_923_626_545_882_28,
        9.930_189_247_374_397e-5,
        9.610_610_749_020_618e-5,
        9.930_189_247_374_397e-5,
        0.000_213_946_076_085_350_06,
    ],
    x: &[
        0.000_653_881_672_860_368_6,
        0.000_299_058_211_142_351_8,
        0.000_241_463_928_970_384_2,
        0.000_299_058_211_142_351_8,
        0.000_632_755_747_134_637_6,
        0.000_273_148_982_449_674_37,
        0.000_241_463_928_970_384_2,
        0.000_273_148_982_449_674_37,
        0.000_645_753_755_723_088_3,
    ],
    c_nf: &[
        0.009_610_821_774_472_898,
        -0.002_913_430_699_649_592,
        -0.001_230_581_242_926_842_4,
        -0.002_913_430_699_649_592,
        0.010_300_154_653_657_473,
        -0.002_300_962_209_490_132,
        -0.001_230_581_242_926_842_4,
        -0.002_300_962_209_490_132,
        0.009_398_738_800_783_178,
    ],
};

const CONCENTRIC_NEUTRAL: Reference = Reference {
    r: &[
        0.000_491_885_386_646_890_9,
        0.000_198_742_965_389_581_44,
        0.000_177_491_168_363_642_71,
        0.000_198_742_965_389_581_44,
        0.000_486_187_197_311_355_63,
        0.000_198_742_965_389_581_44,
        0.000_177_491_168_363_642_71,
        0.000_198_742_965_389_581_44,
        0.000_491_885_386_646_890_7,
    ],
    x: &[
        0.000_276_950_152_987_070_8,
        1.999_546_441_801_915_2e-5,
        -9.248_512_756_198_087e-6,
        1.999_546_441_801_915_2e-5,
        0.000_250_697_859_421_504,
        1.999_546_441_801_912_8e-5,
        -9.248_512_756_198_087e-6,
        1.999_546_441_801_912_8e-5,
        0.000_276_950_152_987_070_9,
    ],
    c_nf: &[
        0.238_580_866_552_065_2,
        0.0,
        0.0,
        0.0,
        0.238_580_866_552_065_2,
        0.0,
        0.0,
        0.0,
        0.238_580_866_552_065_2,
    ],
};

const TAPE_SHIELD: Reference = Reference {
    r: &[0.000_802_906_788_157_298_7],
    x: &[0.000_429_278_946_174_020_5],
    c_nf: &[0.166_358_780_162_155_88],
};

/// The line's per meter matrices match OpenDSS's: series impedance to
/// 1e-9 relative of its largest entry, shunt susceptance likewise.
fn assert_matches(net: &MulticonductorNetwork, line: &str, reference: &Reference) {
    let line = net
        .lines()
        .iter()
        .find(|l| l.name.eq_ignore_ascii_case(line))
        .unwrap_or_else(|| panic!("line {line} is read"));
    let code = net.linecode(&line.linecode).expect("its line code");
    let n = code.n_conductors;
    assert_eq!(n * n, reference.r.len(), "{}: conductor count", line.name);
    let omega = 2.0 * std::f64::consts::PI * 60.0;
    let scale = |values: &[f64]| values.iter().fold(0.0_f64, |m, v| m.max(v.abs()));
    let z_scale = scale(reference.r).max(scale(reference.x));
    let b_scale = scale(reference.c_nf) * 1e-9 * omega;
    for i in 0..n {
        for j in 0..n {
            let k = i * n + j;
            let b = code.b_from[i][j] + code.b_to[i][j];
            for (what, got, want, tolerance) in [
                ("r", code.r_series[i][j], reference.r[k], 1e-9 * z_scale),
                ("x", code.x_series[i][j], reference.x[k], 1e-9 * z_scale),
                ("b", b, reference.c_nf[k] * 1e-9 * omega, 1e-9 * b_scale),
            ] {
                assert!(
                    (got - want).abs() <= tolerance,
                    "{} {what}[{i}][{j}]: {got} != OpenDSS {want}",
                    line.name
                );
            }
        }
    }
}

#[test]
fn dss_geometries_match_opendss_including_tape_shielded_cable() {
    let net = parse_dss_str(DSS);
    assert!(
        !net.warnings
            .iter()
            .any(|w| w.contains("GEOMETRY_UNRESOLVED")),
        "{:#?}",
        net.warnings
    );
    assert_matches(&net, "oh", &OVERHEAD);
    assert_matches(&net, "cn", &CONCENTRIC_NEUTRAL);
    assert_matches(&net, "ts", &TAPE_SHIELD);
    // The computed lines convert; before tape-shield support this refused.
    emit_value(&net, DistTargetFormat::RavensJson);
}

// ----- the same conductors as CIM wire catalog entries ----------------------

/// One catalog entry: CIM class, id, and its properties in SI units.
struct Entry {
    class: &'static str,
    id: &'static str,
    props: Vec<(&'static str, f64)>,
}

fn wire(id: &'static str, diameter_in: f64, gmr_in: f64, rdc: f64, rac: f64) -> Entry {
    Entry {
        class: "OverheadWireInfo",
        id,
        props: vec![
            ("WireInfo.radius", diameter_in / 2.0 * IN),
            ("WireInfo.gmr", gmr_in * IN),
            ("WireInfo.rDC20", rdc),
            ("WireInfo.rAC25", rac),
        ],
    }
}

fn entries() -> Vec<Entry> {
    let acsr556 = 0.035_227_273 / (1000.0 * FT);
    let acsr40 = 0.112_121_212 / (1000.0 * FT);
    let mut ts = wire("ts_1_0", 0.368, 0.13320, 0.97 / MI / 1.02, 0.97 / MI);
    ts.class = "TapeShieldCableInfo";
    ts.props.extend([
        ("WireInfo.insulationThickness", 0.220 * IN),
        ("CableInfo.diameterOverInsulation", 0.82 * IN),
        // The inside diameter of the tape: 0.88 in over the tape.
        ("CableInfo.diameterOverScreen", (0.88 - 2.0 * 0.005) * IN),
        ("CableInfo.diameterOverJacket", 1.06 * IN),
        ("CableInfo.relativePermittivity", 2.3),
        ("TapeShieldCableInfo.tapeThickness", 0.005 * IN),
        ("TapeShieldCableInfo.tapeLap", 20.0),
    ]);
    let mut cn = wire("cn_250", 0.567, 0.20520, 0.41 / MI / 1.02, 0.41 / MI);
    cn.class = "ConcentricNeutralCableInfo";
    cn.props.extend([
        ("WireInfo.insulationThickness", 0.220 * IN),
        ("CableInfo.diameterOverInsulation", 1.06 * IN),
        ("CableInfo.relativePermittivity", 2.3),
        ("ConcentricNeutralCableInfo.diameterOverNeutral", 1.29 * IN),
        ("ConcentricNeutralCableInfo.neutralStrandCount", 13.0),
        (
            "ConcentricNeutralCableInfo.neutralStrandRadius",
            0.0641 / 2.0 * IN,
        ),
        ("ConcentricNeutralCableInfo.neutralStrandGmr", 0.02496 * IN),
        (
            "ConcentricNeutralCableInfo.neutralStrandRDC20",
            14.8722 / MI,
        ),
    ]);
    vec![
        wire("acsr_556_5", 0.927, 0.37320, acsr556, acsr556 * 1.02),
        wire("acsr_4_0", 0.563, 0.09768, acsr40, acsr40 * 1.02),
        wire("cu_1_0", 0.368, 0.13356, 0.607 / MI / 1.02, 0.607 / MI),
        ts,
        cn,
    ]
}

/// A segment on a spacing: its conductors as (phase letter, wire id), in
/// position order, and the spacing's positions in feet.
struct Segment {
    name: &'static str,
    phases: &'static str,
    from: &'static str,
    to: &'static str,
    length_ft: f64,
    spacing: &'static str,
    positions: &'static [(f64, f64)],
    conductors: &'static [(&'static str, &'static str)],
}

const SEGMENTS: &[Segment] = &[
    Segment {
        name: "oh",
        phases: "ABC",
        from: "src",
        to: "a",
        length_ft: 1000.0,
        spacing: "s601",
        positions: &[(-4.0, 28.0), (-1.0, 28.0), (3.0, 28.0), (0.0, 24.0)],
        conductors: &[
            ("A", "acsr_556_5"),
            ("B", "acsr_556_5"),
            ("C", "acsr_556_5"),
            ("N", "acsr_4_0"),
        ],
    },
    Segment {
        name: "cn",
        phases: "ABC",
        from: "a",
        to: "b",
        length_ft: 500.0,
        spacing: "s606",
        positions: &[(-0.5, -4.0), (0.0, -4.0), (0.5, -4.0)],
        conductors: &[("A", "cn_250"), ("B", "cn_250"), ("C", "cn_250")],
    },
    Segment {
        name: "ts",
        phases: "A",
        from: "b",
        to: "c",
        length_ft: 800.0,
        spacing: "s607",
        positions: &[(0.0, -4.0), (0.25, -4.0)],
        conductors: &[("A", "ts_1_0"), ("N", "cu_1_0")],
    },
];

/// The CIM document; `omit_neutrals` drops the `N` phase records, as the
/// MG-RAVENS exporter does.
fn cim(omit_neutrals: bool) -> String {
    let mut x = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<rdf:RDF \
         xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" \
         xmlns:cim=\"http://iec.ch/TC57/CIM100#\">\n<cim:BaseFrequency \
         rdf:ID=\"_f\"><cim:BaseFrequency.frequency>60</cim:BaseFrequency.frequency>\
         </cim:BaseFrequency>\n",
    );
    for node in ["src", "a", "b", "c"] {
        writeln!(
            x,
            "<cim:ConnectivityNode rdf:ID=\"_{node}\"><cim:IdentifiedObject.name>{node}\
             </cim:IdentifiedObject.name></cim:ConnectivityNode>"
        )
        .unwrap();
    }
    for entry in entries() {
        write!(x, "<cim:{} rdf:ID=\"_{}\">", entry.class, entry.id).unwrap();
        for (key, value) in &entry.props {
            write!(x, "<cim:{key}>{value}</cim:{key}>").unwrap();
        }
        writeln!(x, "</cim:{}>", entry.class).unwrap();
    }
    for s in SEGMENTS {
        writeln!(
            x,
            "<cim:WireSpacingInfo rdf:ID=\"_{0}\"><cim:IdentifiedObject.name>{0}\
             </cim:IdentifiedObject.name></cim:WireSpacingInfo>",
            s.spacing
        )
        .unwrap();
        for (k, (px, py)) in s.positions.iter().enumerate() {
            writeln!(
                x,
                "<cim:WirePosition rdf:ID=\"_{0}_{1}\"><cim:WirePosition.WireSpacingInfo \
                 rdf:resource=\"#_{0}\"/><cim:WirePosition.sequenceNumber>{1}\
                 </cim:WirePosition.sequenceNumber><cim:WirePosition.xCoord>{2}\
                 </cim:WirePosition.xCoord><cim:WirePosition.yCoord>{3}\
                 </cim:WirePosition.yCoord></cim:WirePosition>",
                s.spacing,
                k + 1,
                px * FT,
                py * FT
            )
            .unwrap();
        }
        writeln!(
            x,
            "<cim:ACLineSegment rdf:ID=\"_{0}\"><cim:IdentifiedObject.name>{0}\
             </cim:IdentifiedObject.name><cim:Conductor.length>{1}</cim:Conductor.length>\
             <cim:ACLineSegment.WireSpacingInfo rdf:resource=\"#_{2}\"/></cim:ACLineSegment>",
            s.name,
            s.length_ft * FT,
            s.spacing
        )
        .unwrap();
        for (k, node) in [s.from, s.to].iter().enumerate() {
            writeln!(
                x,
                "<cim:Terminal rdf:ID=\"_{0}_t{1}\"><cim:Terminal.ConductingEquipment \
                 rdf:resource=\"#_{0}\"/><cim:Terminal.ConnectivityNode rdf:resource=\"#_{2}\"/>\
                 <cim:Terminal.phases rdf:resource=\"http://iec.ch/TC57/CIM100#PhaseCode.{3}\"/>\
                 <cim:ACDCTerminal.sequenceNumber>{1}</cim:ACDCTerminal.sequenceNumber>\
                 </cim:Terminal>",
                s.name,
                k + 1,
                node,
                s.phases
            )
            .unwrap();
        }
        for (k, (phase, wire)) in s.conductors.iter().enumerate() {
            if omit_neutrals && *phase == "N" {
                continue;
            }
            writeln!(
                x,
                "<cim:ACLineSegmentPhase rdf:ID=\"_{0}_{1}\"><cim:IdentifiedObject.name>{0}_{1}\
                 </cim:IdentifiedObject.name><cim:ACLineSegmentPhase.phase \
                 rdf:resource=\"http://iec.ch/TC57/CIM100#SinglePhaseKind.{1}\"/>\
                 <cim:ACLineSegmentPhase.sequenceNumber>{2}</cim:ACLineSegmentPhase.sequenceNumber>\
                 <cim:ACLineSegmentPhase.ACLineSegment rdf:resource=\"#_{0}\"/>\
                 <cim:PowerSystemResource.AssetDatasheet rdf:resource=\"#_{3}\"/>\
                 </cim:ACLineSegmentPhase>",
                s.name,
                phase,
                k + 1,
                wire
            )
            .unwrap();
        }
    }
    x.push_str("</rdf:RDF>\n");
    x
}

/// The MG-RAVENS document of the same segments.
fn ravens(omit_neutrals: bool) -> String {
    let terminal = |name: &str, k: usize, node: &str, phases: &str| {
        json!({
            "Ravens.cimObjectType": "Terminal",
            "IdentifiedObject.name": format!("{name}_T{k}"),
            "ACDCTerminal.sequenceNumber": k,
            "Terminal.phases": format!("PhaseCode.{phases}"),
            "Terminal.ConnectivityNode": format!("ConnectivityNode::'{node}'"),
        })
    };
    let mut nodes = serde_json::Map::new();
    for node in ["src", "a", "b", "c"] {
        nodes.insert(
            node.into(),
            json!({"Ravens.cimObjectType": "ConnectivityNode", "IdentifiedObject.name": node}),
        );
    }
    let mut infos = serde_json::Map::new();
    for entry in entries() {
        let mut object = serde_json::Map::new();
        object.insert("Ravens.cimObjectType".into(), entry.class.into());
        for (key, value) in entry.props {
            object.insert(key.into(), value.into());
        }
        infos.insert(entry.id.into(), Value::Object(object));
    }
    let mut spacings = serde_json::Map::new();
    let mut segments = serde_json::Map::new();
    for s in SEGMENTS {
        let positions: Vec<Value> = s
            .positions
            .iter()
            .enumerate()
            .map(|(k, (px, py))| {
                json!({
                    "Ravens.cimObjectType": "WirePosition",
                    "WirePosition.sequenceNumber": k + 1,
                    "WirePosition.xCoord": px * FT,
                    "WirePosition.yCoord": py * FT,
                })
            })
            .collect();
        spacings.insert(
            s.spacing.into(),
            json!({
                "Ravens.cimObjectType": "WireSpacingInfo",
                "IdentifiedObject.name": s.spacing,
                "WireSpacingInfo.WirePositions": positions,
            }),
        );
        let phases: Vec<Value> = s
            .conductors
            .iter()
            .enumerate()
            .filter(|(_, (phase, _))| !(omit_neutrals && *phase == "N"))
            .map(|(k, (phase, wire))| {
                json!({
                    "Ravens.cimObjectType": "ACLineSegmentPhase",
                    "IdentifiedObject.name": format!("{}_{phase}", s.name),
                    "ACLineSegmentPhase.phase": format!("SinglePhaseKind.{phase}"),
                    "ACLineSegmentPhase.sequenceNumber": k + 1,
                    "PowerSystemResource.AssetDatasheet": format!("WireInfo::'{wire}'"),
                })
            })
            .collect();
        segments.insert(
            s.name.into(),
            json!({
                "Ravens.cimObjectType": "ACLineSegment",
                "IdentifiedObject.name": s.name,
                "Conductor.length": s.length_ft * FT,
                "ACLineSegment.WireSpacingInfo": format!("WireSpacingInfo::'{}'", s.spacing),
                "ConductingEquipment.Terminals": [
                    terminal(s.name, 1, s.from, s.phases),
                    terminal(s.name, 2, s.to, s.phases),
                ],
                "ACLineSegment.ACLineSegmentPhase": phases,
            }),
        );
    }
    json!({
        "ConnectivityNode": nodes,
        "AssetInfo": {"WireSpacingInfo": spacings, "WireInfo": infos},
        "PowerSystemResource": {"Equipment": {"ConductingEquipment": {"Conductor": {
            "ACLineSegment": segments
        }}}},
    })
    .to_string()
}

#[test]
fn cim_wire_catalog_lines_match_opendss() {
    let net = parse_str(&cim(false), "cim-dist").unwrap();
    assert!(
        !net.warnings
            .iter()
            .any(|w| w.contains("IMPEDANCE_UNRESOLVED")),
        "{:#?}",
        net.warnings
    );
    assert!(net.warnings.iter().any(|w| w.contains("Deri, 100 ohm-m")));
    assert_matches(&net, "oh", &OVERHEAD);
    assert_matches(&net, "cn", &CONCENTRIC_NEUTRAL);
    assert_matches(&net, "ts", &TAPE_SHIELD);
    // The terminals state no neutral, so the neutral conductor is reduced
    // and leaves the segment's terminal map.
    let oh = net.lines().iter().find(|l| l.name == "oh").unwrap();
    assert_eq!(oh.terminal_map_from, ["1", "2", "3"]);
}

#[test]
fn ravens_wire_catalog_lines_match_opendss() {
    let net = parse_str(&ravens(false), "ravens-json").unwrap();
    assert!(
        !net.warnings
            .iter()
            .any(|w| w.contains("ACLineSegment") && w.contains("RETAINED_SOURCE_ONLY")),
        "{:#?}",
        net.warnings
    );
    assert_matches(&net, "oh", &OVERHEAD);
    assert_matches(&net, "cn", &CONCENTRIC_NEUTRAL);
    assert_matches(&net, "ts", &TAPE_SHIELD);
}

/// The MG-RAVENS exporter names no wire for a spacing's neutral position.
/// That neutral's effect on the phase impedance is unknown, so the segment
/// is refused with the position named, never computed without it.
#[test]
fn an_unstated_neutral_wire_is_refused_with_its_position() {
    let net = parse_str(&cim(true), "cim-dist").unwrap();
    let refused: Vec<&String> = net
        .warnings
        .iter()
        .filter(|w| w.starts_with("READ.CIM_DIST.IMPEDANCE_UNRESOLVED"))
        .collect();
    assert_eq!(refused.len(), 2, "{refused:#?}");
    assert!(
        refused
            .iter()
            .any(|w| w.contains("oh") && w.contains("position(s) 4"))
    );
    assert!(
        refused
            .iter()
            .any(|w| w.contains("ts") && w.contains("position(s) 2"))
    );
    // The concentric-neutral segment states every conductor.
    assert_matches(&net, "cn", &CONCENTRIC_NEUTRAL);

    let net = parse_str(&ravens(true), "ravens-json").unwrap();
    assert!(net.lines().iter().all(|l| l.name == "cn"));
    assert!(net.warnings.iter().any(|w| w.contains("ACLineSegment ts")
        && w.contains("position(s) 2")
        && w.contains("RETAINED_SOURCE_ONLY")));
}
