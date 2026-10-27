//! Distribution CIM (CIM100 RDF/XML, GridAPPS-D style) import and export,
//! pinned on a hand computed micro feeder, and the payoff: a CIM feeder
//! converts to OpenDSS, PMD, and BMOPF through the shared distribution
//! machinery, and every distribution source writes CIM.

use std::fmt::Write as _;
use std::path::PathBuf;

use powerio_dist::{Configuration, DiagnosticSeverity, DistTargetFormat, DistWindingConn};

mod helpers;
use helpers::{emit_value, parse_file, parse_str};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/data/dist/cim/micro_feeder.xml")
}

fn micro(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data/dist/micro")
        .join(name)
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
}

/// The rendered findings with a code from `codes`.
fn findings<'a>(warnings: &'a [String], code: &str) -> Vec<&'a String> {
    warnings.iter().filter(|w| w.starts_with(code)).collect()
}

const RDF_OPEN: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:cim="http://iec.ch/TC57/CIM100#">
"#;

/// A two bus feeder: a source, a three phase segment with a phase impedance
/// matrix, and whatever `extra` adds.
fn feeder(extra: &str) -> String {
    format!(
        r##"{RDF_OPEN}
  <cim:BaseFrequency rdf:ID="_f"><cim:BaseFrequency.frequency>60</cim:BaseFrequency.frequency></cim:BaseFrequency>
  <cim:ConnectivityNode rdf:ID="_n1"><cim:IdentifiedObject.name>n1</cim:IdentifiedObject.name></cim:ConnectivityNode>
  <cim:ConnectivityNode rdf:ID="_n2"><cim:IdentifiedObject.name>n2</cim:IdentifiedObject.name></cim:ConnectivityNode>
  <cim:EnergySource rdf:ID="_src">
    <cim:IdentifiedObject.name>src</cim:IdentifiedObject.name>
    <cim:EnergySource.voltageMagnitude>12470</cim:EnergySource.voltageMagnitude>
  </cim:EnergySource>
  <cim:Terminal rdf:ID="_src_t1">
    <cim:Terminal.ConductingEquipment rdf:resource="#_src"/>
    <cim:Terminal.ConnectivityNode rdf:resource="#_n1"/>
    <cim:Terminal.phases rdf:resource="http://iec.ch/TC57/CIM100#PhaseCode.ABC"/>
    <cim:ACDCTerminal.sequenceNumber>1</cim:ACDCTerminal.sequenceNumber>
  </cim:Terminal>
  <cim:ACLineSegment rdf:ID="_l1">
    <cim:IdentifiedObject.name>l1</cim:IdentifiedObject.name>
    <cim:Conductor.length>100</cim:Conductor.length>
    <cim:ACLineSegment.r>0.1</cim:ACLineSegment.r>
    <cim:ACLineSegment.x>0.2</cim:ACLineSegment.x>
  </cim:ACLineSegment>
  <cim:Terminal rdf:ID="_l1_t1">
    <cim:Terminal.ConductingEquipment rdf:resource="#_l1"/>
    <cim:Terminal.ConnectivityNode rdf:resource="#_n1"/>
    <cim:Terminal.phases rdf:resource="http://iec.ch/TC57/CIM100#PhaseCode.ABC"/>
    <cim:ACDCTerminal.sequenceNumber>1</cim:ACDCTerminal.sequenceNumber>
  </cim:Terminal>
  <cim:Terminal rdf:ID="_l1_t2">
    <cim:Terminal.ConductingEquipment rdf:resource="#_l1"/>
    <cim:Terminal.ConnectivityNode rdf:resource="#_n2"/>
    <cim:Terminal.phases rdf:resource="http://iec.ch/TC57/CIM100#PhaseCode.ABC"/>
    <cim:ACDCTerminal.sequenceNumber>2</cim:ACDCTerminal.sequenceNumber>
  </cim:Terminal>
{extra}
</rdf:RDF>
"##
    )
}

#[test]
#[allow(clippy::float_cmp)] // exact hand computed fixture literals are the assertion
fn micro_feeder_maps_exactly() {
    let net = parse_file(fixture(), None).unwrap();
    let declared = net.module.source().and_then(powerio_core::Source::format);
    assert_eq!(
        declared.map(powerio_core::FormatId::as_str),
        Some("cim-dist")
    );
    assert_eq!(net.base_frequency(), 60.0);

    // Four connectivity nodes, four buses; the wye secondary node carries a
    // grounded neutral terminal.
    assert_eq!(net.buses().len(), 4);
    let n4 = net.bus("n4").expect("n4 present");
    assert!(n4.terminals.contains(&"4".to_string()));
    assert_eq!(n4.grounded, vec!["4".to_string()]);

    // A line and its own line code; impedance stays in ohm/m, length in m.
    assert_eq!(net.lines().len(), 1);
    let line = &net.lines()[0];
    assert_eq!(line.bus_from, "sourcebus");
    assert_eq!(line.bus_to, "n2");
    assert_eq!(line.terminal_map_from, ["1", "2", "3"]);
    assert!(close(line.length, 100.0));
    let code = net.linecode(&line.linecode).expect("line code present");
    assert_eq!(code.n_conductors, 3);
    assert!(close(code.r_series[0][0], 0.001), "self r/m");
    assert!(close(code.r_series[1][0], 0.0004), "mutual r/m");
    assert!(close(code.x_series[0][0], 0.002));
    // The shunt susceptance halves at each end.
    assert!(close(code.b_from[0][0], 0.5e-8));
    assert!(close(code.b_to[0][0], 0.5e-8));

    // A wye load: per phase watts and vars straight off the phase objects.
    assert_eq!(net.loads().len(), 1);
    let load = &net.loads()[0];
    assert_eq!(load.bus, "n2");
    assert_eq!(load.configuration, Configuration::Wye);
    assert_eq!(load.terminal_map, ["1", "2", "3", "4"]);
    assert_eq!(load.p_nom, vec![1000.0, 1000.0, 1000.0]);
    assert_eq!(load.q_nom, vec![500.0, 500.0, 500.0]);

    // A shunt: per phase siemens on the diagonal.
    assert_eq!(net.shunts().len(), 1);
    assert!(close(net.shunts()[0].b[0][0], 0.0001));
    assert!(close(net.shunts()[0].b[1][1], 0.0001));
    assert!(close(net.shunts()[0].b[0][1], 0.0));

    // A closed switch from n2 to n3.
    assert_eq!(net.switches().len(), 1);
    assert!(!net.switches()[0].open);
    assert_eq!(net.switches()[0].bus_from, "n2");
    assert_eq!(net.switches()[0].bus_to, "n3");

    // A delta/wye transformer, impedances in percent of the winding base.
    assert_eq!(net.transformers().len(), 1);
    let xf = &net.transformers()[0];
    assert_eq!(xf.windings.len(), 2);
    assert_eq!(xf.windings[0].conn, DistWindingConn::Delta);
    assert_eq!(xf.windings[1].conn, DistWindingConn::Wye);
    assert!(close(xf.windings[0].v_ref, 4160.0));
    assert!(close(xf.windings[1].v_ref, 480.0));
    let z_base = 4160.0_f64.powi(2) / 500_000.0;
    assert!(close(xf.windings[0].r_pct, 0.5 / z_base * 100.0), "r_pct");
    assert!(close(xf.xsc_pct[0], 2.0 / z_base * 100.0), "xsc_pct");

    // The source: line to neutral magnitude at 120 degree spacing, and the
    // grounded neutral at zero.
    assert_eq!(net.sources().len(), 1);
    let src = &net.sources()[0];
    assert_eq!(src.bus, "sourcebus");
    assert_eq!(src.terminal_map, ["1", "2", "3", "4"]);
    assert!(close(src.v_magnitude[0], 4160.0 / 3.0_f64.sqrt()));
    assert_eq!(src.v_magnitude[3], 0.0);
    assert!(close(src.v_angle[1], -2.0 * std::f64::consts::PI / 3.0));
    assert!(close(src.v_angle[2], 2.0 * std::f64::consts::PI / 3.0));

    // The hand written feeder maps completely. Its findings are the two
    // values the compensator leaves to defaults and the base voltage
    // context the model has no field for.
    assert_eq!(
        net.warnings,
        [
            "READ.CIM_DIST.VALUE_DEFAULTED: LinearShuntCompensator cap1: no \
             ShuntCompensator.phaseConnection; wye is assumed",
            "READ.CIM_DIST.VALUE_DEFAULTED: LinearShuntCompensator cap1: no sections or \
             normalSections; one section is assumed",
            "READ.CIM_DIST.CONTEXT_UNMAPPED: 2 BaseVoltage object(s) have no multiconductor \
             mapping (context) and are not carried",
        ]
    );
}

#[test]
fn cim_feeder_converts_to_every_distribution_format() {
    // The point of landing in the multiconductor model: a CIM feeder writes
    // as OpenDSS, PMD, and BMOPF with no extra bridge.
    let net = parse_file(fixture(), None).unwrap();
    for target in [
        DistTargetFormat::Dss,
        DistTargetFormat::PmdJson,
        DistTargetFormat::BmopfJson,
    ] {
        let out = emit_value(&net, target);
        assert!(!out.text.is_empty(), "{}: empty output", target.name());
    }

    // OpenDSS output names the feeder's elements.
    let dss = emit_value(&net, DistTargetFormat::Dss).text.to_lowercase();
    assert!(dss.contains("line.line1"), "dss: {dss}");
    assert!(dss.contains("load.load1"));
    assert!(dss.contains("transformer.xfm1"));

    // PMD and BMOPF reparse: the conversion is structurally valid, not just
    // nonempty text.
    let pmd = emit_value(&net, DistTargetFormat::PmdJson).text;
    let reparsed = parse_str(&pmd, "pmd-json").unwrap();
    assert_eq!(reparsed.lines().len(), net.lines().len());
    assert_eq!(reparsed.loads().len(), net.loads().len());

    let bmopf = emit_value(&net, DistTargetFormat::BmopfJson).text;
    let reparsed = parse_str(&bmopf, "bmopf-json").unwrap();
    assert_eq!(reparsed.lines().len(), net.lines().len());
}

#[test]
fn xml_extension_and_cim_dist_token_both_route() {
    // Recognized from the extension and the per phase classes.
    let net = parse_file(fixture(), None).unwrap();
    assert_eq!(net.buses().len(), 4);
    // The explicit token on in memory text.
    let text = std::fs::read_to_string(fixture()).unwrap();
    let net = parse_str(&text, "cim-dist").unwrap();
    assert_eq!(net.buses().len(), 4);
    // `cim-dist` is a writable distribution target.
    assert_eq!(
        powerio_dist::parse_dist_target_format("cim-dist"),
        Some(DistTargetFormat::CimDist)
    );
    assert_eq!(DistTargetFormat::CimDist.name(), "cim-dist");
    assert!(powerio_dist::looks_like_cim_dist(text.as_bytes()));
}

/// The writer is the reader's inverse: parse, write, parse keeps the
/// projection, imported mRIDs pass through, a second write is byte
/// identical, and a parse writes its own bytes back.
#[test]
fn cim_writer_round_trips() {
    let net = parse_file(fixture(), None).unwrap();
    let first = emit_value(&net, DistTargetFormat::CimDist);
    assert!(first.warnings.is_empty(), "{:#?}", first.warnings);
    let back = parse_str(&first.text, "cim-dist").unwrap();

    assert_eq!(back.buses().len(), net.buses().len());
    assert_eq!(back.lines().len(), net.lines().len());
    assert_eq!(back.loads().len(), net.loads().len());
    assert_eq!(back.shunts().len(), net.shunts().len());
    assert_eq!(back.switches().len(), net.switches().len());
    assert_eq!(back.transformers().len(), net.transformers().len());
    assert_eq!(back.sources().len(), net.sources().len());
    assert_eq!(back.name(), net.name());

    let code_a = net.linecode(&net.lines()[0].linecode).unwrap();
    let code_b = back.linecode(&back.lines()[0].linecode).unwrap();
    for i in 0..code_a.n_conductors {
        for j in 0..code_a.n_conductors {
            assert!(
                close(code_a.r_series[i][j], code_b.r_series[i][j]),
                "r {i}{j}"
            );
            assert!(
                close(code_a.x_series[i][j], code_b.x_series[i][j]),
                "x {i}{j}"
            );
            assert!(
                close(
                    code_a.b_from[i][j] + code_a.b_to[i][j],
                    code_b.b_from[i][j] + code_b.b_to[i][j]
                ),
                "b {i}{j}"
            );
        }
    }
    assert_eq!(back.loads()[0].p_nom, net.loads()[0].p_nom);
    assert_eq!(back.loads()[0].configuration, net.loads()[0].configuration);
    assert!(close(back.shunts()[0].b[0][0], net.shunts()[0].b[0][0]));
    let (a, b) = (&net.transformers()[0], &back.transformers()[0]);
    assert_eq!(a.windings[0].conn, b.windings[0].conn);
    assert!(close(a.windings[0].v_ref, b.windings[0].v_ref));
    assert!(close(a.windings[0].r_pct, b.windings[0].r_pct));
    assert!(close(a.xsc_pct[0], b.xsc_pct[0]));
    assert!(close(
        net.sources()[0].v_magnitude[0],
        back.sources()[0].v_magnitude[0]
    ));

    // Deterministic ids: the reparse carries the same mRIDs, so a second
    // write is byte identical.
    let second = emit_value(&back, DistTargetFormat::CimDist);
    assert_eq!(first.text, second.text);

    // Same format echo: a single document parse writes its source back.
    assert_eq!(back.emit(DistTargetFormat::CimDist).text, first.text);
    let original = std::fs::read_to_string(fixture()).unwrap();
    assert_eq!(net.emit(DistTargetFormat::CimDist).text, original);
}

/// dss to CIM: any distribution source writes CIM, and the reparse keeps the
/// elements.
#[test]
fn dss_converts_to_cim() {
    for case in [
        "xfmr_single_phase.dss",
        "xfmr_delta_wye.dss",
        "fourwire_linecode.dss",
        "switch.dss",
    ] {
        let net = parse_file(micro(case), None).unwrap();
        let out = emit_value(&net, DistTargetFormat::CimDist);
        let back = parse_str(&out.text, "cim-dist").unwrap();
        assert_eq!(
            back.transformers().len(),
            net.transformers().len(),
            "{case}"
        );
        assert_eq!(back.loads().len(), net.loads().len(), "{case}");
        assert_eq!(back.lines().len(), net.lines().len(), "{case}");
        assert_eq!(back.switches().len(), net.switches().len(), "{case}");
        // The canonical CIM output is idempotent.
        assert_eq!(
            emit_value(&back, DistTargetFormat::CimDist).text,
            out.text,
            "{case}"
        );
    }
}

/// A four wire segment keeps its neutral conductor and the full 4×4 matrix.
#[test]
fn a_neutral_conductor_survives_cim() {
    let net = parse_file(micro("fourwire_linecode.dss"), None).unwrap();
    let line = net
        .lines()
        .iter()
        .find(|l| l.terminal_map_from.len() == 4)
        .expect("a four wire line");
    let code = net.linecode(&line.linecode).unwrap();
    let back = parse_str(
        &emit_value(&net, DistTargetFormat::CimDist).text,
        "cim-dist",
    )
    .unwrap();
    let back_line = back.lines().iter().find(|l| l.name == line.name).unwrap();
    assert_eq!(back_line.terminal_map_from, line.terminal_map_from);
    let back_code = back.linecode(&back_line.linecode).unwrap();
    assert_eq!(back_code.n_conductors, code.n_conductors);
    for i in 0..code.n_conductors {
        for j in 0..code.n_conductors {
            assert!(
                close(code.r_series[i][j], back_code.r_series[i][j]),
                "r {i}{j}"
            );
            assert!(
                close(code.x_series[i][j], back_code.x_series[i][j]),
                "x {i}{j}"
            );
        }
    }
}

/// More than four conductors have no `SinglePhaseKind`; the writer says so.
#[test]
fn conductors_beyond_four_are_reported_on_output() {
    let net = parse_file(micro("linecode_10x10.dss"), None).unwrap();
    let out = emit_value(&net, DistTargetFormat::CimDist);
    assert!(
        !findings(&out.warnings, "EMIT.CIM_DIST.FIELD_DROPPED").is_empty(),
        "{:#?}",
        out.warnings
    );
}

/// A three winding (center tap) transformer writes its pair reactances as
/// mesh impedances and reads them back, and a tap rides a RatioTapChanger.
#[test]
fn three_windings_and_taps_round_trip() {
    let mut net = parse_file(micro("xfmr_center_tap.dss"), None).unwrap();
    let xf = net
        .transformers()
        .iter()
        .find(|t| t.windings.len() == 3)
        .expect("a three winding transformer")
        .clone();
    net.transformers_mut()[0].windings[0].tap = 1.0125;
    let tap = net.transformers()[0].windings[0].tap;
    let out = emit_value(&net, DistTargetFormat::CimDist);
    assert!(out.text.contains("TransformerMeshImpedance"));
    assert!(out.text.contains("RatioTapChanger"));
    let back = parse_str(&out.text, "cim-dist").unwrap();
    let back_xf = back
        .transformers()
        .iter()
        .find(|t| t.name == xf.name)
        .unwrap();
    assert_eq!(back_xf.xsc_pct.len(), xf.xsc_pct.len());
    for (a, b) in xf.xsc_pct.iter().zip(&back_xf.xsc_pct) {
        assert!(close(*a, *b), "xsc {a} vs {b}");
    }
    assert!(close(back.transformers()[0].windings[0].tap, tap));
    assert_eq!(emit_value(&back, DistTargetFormat::CimDist).text, out.text);
}

/// A directory of documents reads as one case: `rdf:about` objects extend
/// the ones another document defines, and a disagreement keeps the first
/// statement with a finding.
#[test]
fn a_directory_reads_as_one_case() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a_eq.xml"), feeder("")).unwrap();
    std::fs::write(
        dir.path().join("b_ssh.xml"),
        format!(
            r##"{RDF_OPEN}
  <cim:EnergySource rdf:about="#_src">
    <cim:EnergySource.voltageMagnitude>12000</cim:EnergySource.voltageMagnitude>
    <cim:EnergySource.voltageAngle>0.1</cim:EnergySource.voltageAngle>
  </cim:EnergySource>
</rdf:RDF>
"##
        ),
    )
    .unwrap();
    std::fs::write(dir.path().join("notes.txt"), "not a document").unwrap();
    let net = parse_file(dir.path(), Some("cim-dist")).unwrap();
    assert_eq!(net.lines().len(), 1);
    let src = &net.sources()[0];
    // The angle comes from the second document; the magnitude conflict
    // keeps the first.
    assert!(close(src.v_angle[0], 0.1));
    assert!(close(src.v_magnitude[0], 12470.0 / 3.0_f64.sqrt()));
    assert_eq!(
        findings(&net.warnings, "READ.CIM_DIST.VALUE_CONFLICT").len(),
        1,
        "{:#?}",
        net.warnings
    );
    // A directory has no single source text to echo.
    assert!(net.source.is_none());
}

/// An out-of-service CIM switch must not connect the buses in the imported model.
#[test]
fn an_out_of_service_switch_is_preserved_as_open() {
    let text = feeder(
        r##"<cim:Breaker rdf:ID="_sw1">
    <cim:IdentifiedObject.name>sw1</cim:IdentifiedObject.name>
    <cim:Switch.open>false</cim:Switch.open>
    <cim:Equipment.inService>false</cim:Equipment.inService>
  </cim:Breaker>
  <cim:Terminal rdf:ID="_sw_t1">
    <cim:Terminal.ConductingEquipment rdf:resource="#_sw1"/>
    <cim:Terminal.ConnectivityNode rdf:resource="#_n1"/>
    <cim:Terminal.phases rdf:resource="http://iec.ch/TC57/CIM100#PhaseCode.ABC"/>
    <cim:ACDCTerminal.sequenceNumber>1</cim:ACDCTerminal.sequenceNumber>
  </cim:Terminal>
  <cim:Terminal rdf:ID="_sw_t2">
    <cim:Terminal.ConductingEquipment rdf:resource="#_sw1"/>
    <cim:Terminal.ConnectivityNode rdf:resource="#_n2"/>
    <cim:Terminal.phases rdf:resource="http://iec.ch/TC57/CIM100#PhaseCode.ABC"/>
    <cim:ACDCTerminal.sequenceNumber>2</cim:ACDCTerminal.sequenceNumber>
  </cim:Terminal>"##,
    );
    let net = parse_str(&text, "cim-dist").unwrap();
    assert_eq!(net.switches().len(), 1);
    assert!(net.switches()[0].open, "{:#?}", net.switches()[0]);
    assert!(
        findings(&net.warnings, "READ.CIM_DIST.VALUE_UNSUPPORTED")
            .iter()
            .any(|finding| finding.contains("sw1") && finding.contains("open switch")),
        "{:#?}",
        net.warnings
    );
}

/// A directory case is its top level `.xml` documents: a copy under a
/// subdirectory is not merged in, and a directory whose documents sit only
/// in a subdirectory is not a case.
#[test]
fn a_directory_reads_only_its_top_level_documents() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("feeder.xml"), feeder("")).unwrap();
    std::fs::create_dir(dir.path().join("archive")).unwrap();
    std::fs::write(dir.path().join("archive/feeder.xml"), feeder("")).unwrap();
    std::fs::write(dir.path().join("archive/schema.xml"), "<schema/>").unwrap();
    let net = parse_file(dir.path(), Some("cim-dist")).unwrap();
    assert_eq!(net.lines().len(), 1);

    let nested = tempfile::tempdir().unwrap();
    std::fs::create_dir(nested.path().join("archive")).unwrap();
    std::fs::copy(fixture(), nested.path().join("archive/feeder.xml")).unwrap();
    let source = powerio_core::Source::open(nested.path()).unwrap();
    assert!(!powerio_dist::source_looks_like_cim_dist(&source));
    let error = parse_file(nested.path(), Some("cim-dist")).unwrap_err();
    assert!(error.to_string().contains("top level"), "{error}");
}

/// A property holding nested elements has no value: its inner text is not
/// read as a number, and the property is reported.
#[test]
fn a_property_with_nested_elements_is_reported_not_read() {
    let text = feeder(
        r##"<cim:EnergySource rdf:about="#_src">
    <cim:EnergySource.voltageAngle><v:A xmlns:v="http://vendor.example/ext#">1</v:A><v:B xmlns:v="http://vendor.example/ext#">2</v:B></cim:EnergySource.voltageAngle>
  </cim:EnergySource>"##,
    );
    let net = parse_str(&text, "cim-dist").unwrap();
    assert!(
        close(net.sources()[0].v_angle[0], 0.0),
        "{:?}",
        net.sources()[0].v_angle
    );
    assert!(
        findings(&net.warnings, "READ.CIM_DIST.FIELD_UNMAPPED")
            .iter()
            .any(|f| f.contains("EnergySource.voltageAngle") && f.contains("nested")),
        "{:#?}",
        net.warnings
    );
}

/// What the July reader dropped silently is mapped or reported.
#[test]
fn unmapped_and_partial_content_is_reported() {
    let extra = r##"
  <cim:ConnectivityNode rdf:ID="_n3"><cim:IdentifiedObject.name>n2</cim:IdentifiedObject.name></cim:ConnectivityNode>
  <cim:LinearShuntCompensator rdf:ID="_cap">
    <cim:IdentifiedObject.name>cap</cim:IdentifiedObject.name>
    <cim:LinearShuntCompensator.bPerSection>0.001</cim:LinearShuntCompensator.bPerSection>
    <cim:ShuntCompensator.sections>2</cim:ShuntCompensator.sections>
    <cim:ShuntCompensator.phaseConnection rdf:resource="http://iec.ch/TC57/CIM100#PhaseShuntConnectionKind.Y"/>
    <cim:ShuntCompensator.nomU>12470</cim:ShuntCompensator.nomU>
  </cim:LinearShuntCompensator>
  <cim:Terminal rdf:ID="_cap_t1">
    <cim:Terminal.ConductingEquipment rdf:resource="#_cap"/>
    <cim:Terminal.ConnectivityNode rdf:resource="#_n2"/>
    <cim:Terminal.phases rdf:resource="http://iec.ch/TC57/CIM100#PhaseCode.ABC"/>
  </cim:Terminal>
  <cim:ACLineSegment rdf:ID="_geo">
    <cim:IdentifiedObject.name>geo</cim:IdentifiedObject.name>
    <cim:Conductor.length>50</cim:Conductor.length>
  </cim:ACLineSegment>
  <cim:Terminal rdf:ID="_geo_t1">
    <cim:Terminal.ConductingEquipment rdf:resource="#_geo"/>
    <cim:Terminal.ConnectivityNode rdf:resource="#_n2"/>
    <cim:ACDCTerminal.sequenceNumber>1</cim:ACDCTerminal.sequenceNumber>
  </cim:Terminal>
  <cim:Terminal rdf:ID="_geo_t2">
    <cim:Terminal.ConductingEquipment rdf:resource="#_geo"/>
    <cim:Terminal.ConnectivityNode rdf:resource="#_missing"/>
    <cim:ACDCTerminal.sequenceNumber>2</cim:ACDCTerminal.sequenceNumber>
  </cim:Terminal>
  <cim:PowerTransformer rdf:ID="_tank_xf"><cim:IdentifiedObject.name>reg</cim:IdentifiedObject.name></cim:PowerTransformer>
  <cim:TransformerTank rdf:ID="_tank"><cim:TransformerTank.PowerTransformer rdf:resource="#_tank_xf"/></cim:TransformerTank>
  <cim:WireSpacingInfo rdf:ID="_spacing"/>
  <cim:BaseVoltage rdf:ID="_bv"/>
"##;
    let mut text = feeder(extra);
    // The source states its Thevenin impedance.
    text = text.replace(
        "<cim:EnergySource.voltageMagnitude>12470</cim:EnergySource.voltageMagnitude>",
        "<cim:EnergySource.voltageMagnitude>12470</cim:EnergySource.voltageMagnitude>\
         <cim:EnergySource.r>0.1</cim:EnergySource.r><cim:EnergySource.x>0.5</cim:EnergySource.x>\
         <cim:EnergySource.r0>0.3</cim:EnergySource.r0><cim:EnergySource.x0>1.4</cim:EnergySource.x0>",
    );
    let net = parse_str(&text, "cim-dist").unwrap();
    let w = &net.warnings;

    // Sections multiply the per section admittance.
    let cap = net.shunts().iter().find(|s| s.name == "cap").unwrap();
    assert!(close(cap.b[0][0], 0.002));
    // The source impedance rides the rs/xs matrices the dss and PMD writers
    // read: self (z0 + 2 z1)/3, mutual (z0 − z1)/3.
    let src = &net.sources()[0];
    let rs = src.extras["rs"].as_array().unwrap();
    assert!(close(rs[0][0].as_f64().unwrap(), (0.3 + 0.2) / 3.0));
    assert!(close(rs[0][1].as_f64().unwrap(), (0.3 - 0.1) / 3.0));

    // A repeated node name is replaced by the mRID.
    assert!(net.bus("n3").is_some(), "{w:#?}");
    assert_eq!(
        findings(w, "READ.CIM_DIST.IDENTIFIER_SUBSTITUTED").len(),
        1,
        "{w:#?}"
    );
    // A terminal on an undefined node drops the segment; the reference and
    // the record are both reported.
    assert!(
        !findings(w, "READ.CIM_DIST.REFERENCE_DROPPED").is_empty(),
        "{w:#?}"
    );
    assert!(net.lines().iter().all(|l| l.name != "geo"));
    // The tank form and the unmapped classes are reported, not dropped
    // silently.
    assert!(
        findings(w, "READ.CIM_DIST.RECORD_DROPPED")
            .iter()
            .any(|f| f.contains("TransformerTank form")),
        "{w:#?}"
    );
    let classes = findings(w, "READ.CIM_DIST.CLASS_UNMAPPED");
    assert!(
        classes.iter().any(|f| f.contains("TransformerTank")),
        "{w:#?}"
    );
    assert!(
        classes.iter().any(|f| f.contains("WireSpacingInfo")),
        "{w:#?}"
    );
    assert!(
        findings(w, "READ.CIM_DIST.CONTEXT_UNMAPPED")
            .iter()
            .any(|f| f.contains("BaseVoltage")),
        "{w:#?}"
    );
    // A property the mapping does not read is counted per class.
    assert!(
        findings(w, "READ.CIM_DIST.FIELD_UNMAPPED")
            .iter()
            .any(|f| f.contains("ShuntCompensator.nomU")),
        "{w:#?}"
    );
}

/// A segment with no impedance at all is an error severity finding.
#[test]
fn a_segment_without_impedance_is_flagged() {
    let text = feeder("").replace(
        "<cim:ACLineSegment.r>0.1</cim:ACLineSegment.r>\n    <cim:ACLineSegment.x>0.2</cim:ACLineSegment.x>",
        "",
    );
    let net = parse_str(&text, "cim-dist").unwrap();
    let errors: Vec<_> = net
        .diagnostics
        .iter()
        .filter(|d| d.code() == "READ.CIM_DIST.IMPEDANCE_UNRESOLVED")
        .collect();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].severity(), DiagnosticSeverity::Error);
}

/// Conversion refuses a segment with no impedance, whose zero placeholder
/// would be a short circuit. Reading it and echoing the source still work.
#[test]
fn a_segment_without_impedance_is_not_converted() {
    let text = feeder("").replace(
        "<cim:ACLineSegment.r>0.1</cim:ACLineSegment.r>\n    <cim:ACLineSegment.x>0.2</cim:ACLineSegment.x>",
        "",
    );
    let net = parse_str(&text, "cim-dist").unwrap();
    assert!(
        powerio_dist::audit_electrical_readiness(&net)
            .blockers()
            .any(|f| f.code == "READINESS.CIM_DIST.IMPEDANCE_UNRESOLVED")
    );
    for target in [
        DistTargetFormat::Dss,
        DistTargetFormat::PmdJson,
        DistTargetFormat::BmopfJson,
        DistTargetFormat::CimDist,
    ] {
        let error = powerio_dist::emit_with_options(
            &powerio_core::PioModule::new((*net).clone()),
            target,
            &powerio_dist::EmitOptions::default(),
            powerio_core::Destination::memory("out").unwrap(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("l1"), "{error}");
    }
    assert_eq!(net.emit(DistTargetFormat::CimDist).text, text);
}

/// `feeder` with segment `l1` described by impedance object `_pli` instead
/// of its own series totals, plus `extra`.
fn feeder_on_pli(extra: &str) -> String {
    feeder(extra).replace(
        "<cim:ACLineSegment.r>0.1</cim:ACLineSegment.r>\n    <cim:ACLineSegment.x>0.2</cim:ACLineSegment.x>",
        r##"<cim:ACLineSegment.PerLengthImpedance rdf:resource="#_pli"/>"##,
    )
}

/// One `PhaseImpedanceData` entry of `_pli`.
fn impedance_entry(row: usize, col: usize, r: f64, x: f64) -> String {
    format!(
        r##"<cim:PhaseImpedanceData rdf:ID="_d{row}{col}">
    <cim:PhaseImpedanceData.PhaseImpedance rdf:resource="#_pli"/>
    <cim:PhaseImpedanceData.row>{row}</cim:PhaseImpedanceData.row>
    <cim:PhaseImpedanceData.column>{col}</cim:PhaseImpedanceData.column>
    <cim:PhaseImpedanceData.r>{r}</cim:PhaseImpedanceData.r>
    <cim:PhaseImpedanceData.x>{x}</cim:PhaseImpedanceData.x>
  </cim:PhaseImpedanceData>"##
    )
}

/// A three conductor `_pli` whose entry (i, j) has r = i + j/10 and
/// x = 10 r, so every entry is distinct.
fn three_conductor_pli() -> String {
    let mut out = String::from(
        r#"<cim:PerLengthPhaseImpedance rdf:ID="_pli">
    <cim:IdentifiedObject.name>pli</cim:IdentifiedObject.name>
    <cim:PerLengthPhaseImpedance.conductorCount>3</cim:PerLengthPhaseImpedance.conductorCount>
  </cim:PerLengthPhaseImpedance>"#,
    );
    for row in 1..=3 {
        for col in 1..=row {
            let r = row as f64 + col as f64 / 10.0;
            out.push_str(&impedance_entry(row, col, r, 10.0 * r));
        }
    }
    out
}

/// Segment `l2` from n2 to a new node n3 on `_pli`, with one
/// `ACLineSegmentPhase` per `(phase, sequenceNumber)`.
fn segment_on_pli(phases: &[(&str, usize)]) -> String {
    let code: String = phases.iter().map(|(p, _)| *p).collect();
    let mut out = format!(
        r##"<cim:ConnectivityNode rdf:ID="_n3"><cim:IdentifiedObject.name>n3</cim:IdentifiedObject.name></cim:ConnectivityNode>
  <cim:ACLineSegment rdf:ID="_l2">
    <cim:IdentifiedObject.name>l2</cim:IdentifiedObject.name>
    <cim:Conductor.length>50</cim:Conductor.length>
    <cim:ACLineSegment.PerLengthImpedance rdf:resource="#_pli"/>
  </cim:ACLineSegment>
  <cim:Terminal rdf:ID="_l2_t1">
    <cim:Terminal.ConductingEquipment rdf:resource="#_l2"/>
    <cim:Terminal.ConnectivityNode rdf:resource="#_n2"/>
    <cim:Terminal.phases rdf:resource="http://iec.ch/TC57/CIM100#PhaseCode.{code}"/>
    <cim:ACDCTerminal.sequenceNumber>1</cim:ACDCTerminal.sequenceNumber>
  </cim:Terminal>
  <cim:Terminal rdf:ID="_l2_t2">
    <cim:Terminal.ConductingEquipment rdf:resource="#_l2"/>
    <cim:Terminal.ConnectivityNode rdf:resource="#_n3"/>
    <cim:Terminal.phases rdf:resource="http://iec.ch/TC57/CIM100#PhaseCode.{code}"/>
    <cim:ACDCTerminal.sequenceNumber>2</cim:ACDCTerminal.sequenceNumber>
  </cim:Terminal>"##
    );
    for (phase, seq) in phases {
        let _ = write!(
            out,
            r##"<cim:ACLineSegmentPhase rdf:ID="_l2p{phase}">
    <cim:ACLineSegmentPhase.ACLineSegment rdf:resource="#_l2"/>
    <cim:ACLineSegmentPhase.phase rdf:resource="http://iec.ch/TC57/CIM100#SinglePhaseKind.{phase}"/>
    <cim:ACLineSegmentPhase.sequenceNumber>{seq}</cim:ACLineSegmentPhase.sequenceNumber>
  </cim:ACLineSegmentPhase>"##
        );
    }
    out
}

/// A conductor count far beyond any real line drops the impedance object
/// with a finding instead of sizing matrices from it.
#[test]
fn an_oversized_conductor_count_is_dropped() {
    for pli in [
        r#"<cim:PerLengthPhaseImpedance rdf:ID="_pli">
    <cim:PerLengthPhaseImpedance.conductorCount>1e9</cim:PerLengthPhaseImpedance.conductorCount>
  </cim:PerLengthPhaseImpedance>"#
            .to_owned()
            + &impedance_entry(1, 1, 0.1, 0.2),
        r#"<cim:PerLengthPhaseImpedance rdf:ID="_pli"/>"#.to_owned()
            + &impedance_entry(1_000_000, 1, 0.1, 0.2),
    ] {
        let net = parse_str(&feeder_on_pli(&pli), "cim-dist").unwrap();
        assert!(
            findings(&net.warnings, "READ.CIM_DIST.RECORD_DROPPED")
                .iter()
                .any(|f| f.contains("supported maximum of 64")),
            "{:#?}",
            net.warnings
        );
        assert_eq!(
            findings(&net.warnings, "READ.CIM_DIST.IMPEDANCE_UNRESOLVED").len(),
            1,
            "{:#?}",
            net.warnings
        );
    }
}

/// A segment phase takes the matrix row its sequence number names, so a
/// phase A and C segment on a three conductor matrix gets rows 1 and 3.
#[test]
fn a_segment_phase_takes_its_numbered_impedance_row() {
    let text = feeder(&format!(
        "{}\n{}",
        three_conductor_pli(),
        segment_on_pli(&[("A", 1), ("C", 3)])
    ));
    let net = parse_str(&text, "cim-dist").unwrap();
    let line = net.lines().iter().find(|l| l.name == "l2").unwrap();
    let code = net
        .line_codes()
        .iter()
        .find(|c| c.name == line.linecode)
        .unwrap();
    let r = |i: usize, j: usize| {
        let (row, col) = (i.max(j), i.min(j));
        row as f64 + col as f64 / 10.0
    };
    assert_eq!(code.n_conductors, 2);
    assert!(close(code.r_series[0][0], r(1, 1)), "{:?}", code.r_series);
    assert!(close(code.r_series[0][1], r(1, 3)), "{:?}", code.r_series);
    assert!(close(code.r_series[1][1], r(3, 3)), "{:?}", code.r_series);
    assert!(
        close(code.x_series[1][1], 10.0 * r(3, 3)),
        "{:?}",
        code.x_series
    );
}

/// Sequence numbers that cannot name distinct rows fall back to phase
/// order, with a finding.
#[test]
fn unusable_sequence_numbers_fall_back_to_phase_order() {
    let text = feeder(&format!(
        "{}\n{}",
        three_conductor_pli(),
        segment_on_pli(&[("A", 2), ("C", 2)])
    ));
    let net = parse_str(&text, "cim-dist").unwrap();
    assert!(
        findings(&net.warnings, "READ.CIM_DIST.VALUE_DEFAULTED")
            .iter()
            .any(|f| f.contains("sequence numbers")),
        "{:#?}",
        net.warnings
    );
    let line = net.lines().iter().find(|l| l.name == "l2").unwrap();
    let code = net
        .line_codes()
        .iter()
        .find(|c| c.name == line.linecode)
        .unwrap();
    assert!(close(code.r_series[1][1], 2.2), "{:?}", code.r_series);
}

/// Single phase segments on different phases share one sequence impedance
/// code: sequence values expand the same way whichever phase they take.
#[test]
fn single_phase_segments_share_a_sequence_impedance_code() {
    let psi = r#"<cim:PerLengthSequenceImpedance rdf:ID="_pli">
    <cim:PerLengthSequenceImpedance.r>0.001</cim:PerLengthSequenceImpedance.r>
    <cim:PerLengthSequenceImpedance.x>0.002</cim:PerLengthSequenceImpedance.x>
    <cim:PerLengthSequenceImpedance.r0>0.003</cim:PerLengthSequenceImpedance.r0>
    <cim:PerLengthSequenceImpedance.x0>0.006</cim:PerLengthSequenceImpedance.x0>
  </cim:PerLengthSequenceImpedance>"#;
    let second = segment_on_pli(&[("C", 3)])
        .replace("_l2", "_l3")
        .replace("l2<", "l3<")
        .replace("_n3", "_n5")
        .replace(">n3<", ">n5<");
    let text = feeder(&format!("{psi}\n{}\n{second}", segment_on_pli(&[("B", 2)])));
    let net = parse_str(&text, "cim-dist").unwrap();
    let code = |name: &str| {
        &net.lines()
            .iter()
            .find(|l| l.name == name)
            .unwrap()
            .linecode
    };
    assert_eq!(code("l2"), code("l3"));
}

/// A segment using part of a shared impedance matrix writes as its own
/// impedance object: the output defines every id once and reads back.
#[test]
fn a_projected_line_code_writes_its_own_mrid() {
    let text = feeder_on_pli(&format!(
        "{}\n{}",
        three_conductor_pli(),
        segment_on_pli(&[("A", 1), ("C", 3)])
    ));
    let net = parse_str(&text, "cim-dist").unwrap();
    assert_eq!(net.line_codes().len(), 2, "{:#?}", net.line_codes());
    let out = emit_value(&net, DistTargetFormat::CimDist);
    let mut ids: Vec<&str> = out
        .text
        .split("rdf:ID=\"")
        .skip(1)
        .map(|rest| rest.split('"').next().unwrap())
        .collect();
    let count = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), count, "an rdf:ID repeats");
    let back = parse_str(&out.text, "cim-dist").unwrap();
    assert_eq!(back.lines().len(), net.lines().len());
}

/// An impedance object with no data is dropped, and a segment that names
/// it has no impedance: an error finding, not a silent zero matrix.
#[test]
fn an_impedance_object_without_data_leaves_the_segment_unresolved() {
    let text = feeder_on_pli(
        r#"<cim:PerLengthPhaseImpedance rdf:ID="_pli">
    <cim:PerLengthPhaseImpedance.conductorCount>3</cim:PerLengthPhaseImpedance.conductorCount>
  </cim:PerLengthPhaseImpedance>"#,
    );
    let net = parse_str(&text, "cim-dist").unwrap();
    assert!(
        findings(&net.warnings, "READ.CIM_DIST.RECORD_DROPPED")
            .iter()
            .any(|f| f.contains("no PhaseImpedanceData")),
        "{:#?}",
        net.warnings
    );
    let unresolved = findings(&net.warnings, "READ.CIM_DIST.IMPEDANCE_UNRESOLVED");
    assert_eq!(unresolved.len(), 1, "{:#?}", net.warnings);
    assert!(
        unresolved[0].contains("no usable impedance"),
        "{unresolved:?}"
    );
}

/// A per-length impedance needs the segment's length. Without one the 1 m
/// placeholder would fabricate a near short, so the segment is unresolved;
/// series totals do not depend on the length and stay resolved.
#[test]
fn a_per_length_impedance_without_a_length_leaves_the_segment_unresolved() {
    let unlengthed =
        |text: String| text.replace("<cim:Conductor.length>100</cim:Conductor.length>", "");
    let net = parse_str(
        &unlengthed(feeder_on_pli(&three_conductor_pli())),
        "cim-dist",
    )
    .unwrap();
    let unresolved = findings(&net.warnings, "READ.CIM_DIST.IMPEDANCE_UNRESOLVED");
    assert_eq!(unresolved.len(), 1, "{:#?}", net.warnings);
    assert!(
        unresolved[0].contains("needs a positive Conductor.length"),
        "{unresolved:?}"
    );
    assert!(
        powerio_dist::audit_electrical_readiness(&net)
            .blockers()
            .any(|f| f.code == "READINESS.CIM_DIST.IMPEDANCE_UNRESOLVED")
    );

    let net = parse_str(&unlengthed(feeder("")), "cim-dist").unwrap();
    let unresolved = findings(&net.warnings, "READ.CIM_DIST.IMPEDANCE_UNRESOLVED");
    assert_eq!(unresolved.len(), 0, "{unresolved:?}");
    assert_eq!(
        findings(&net.warnings, "READ.CIM_DIST.VALUE_DEFAULTED").len(),
        1
    );
}

/// A wye consumer `id` on n2 stating `total`, with phase records A, B and C
/// that each state `phase_p`.
fn consumer(id: &str, total: &str, phase_p: &str) -> String {
    let mut out = format!(
        r##"<cim:EnergyConsumer rdf:ID="_{id}">
    <cim:IdentifiedObject.name>{id}</cim:IdentifiedObject.name>
    <cim:EnergyConsumer.phaseConnection rdf:resource="http://iec.ch/TC57/CIM100#PhaseShuntConnectionKind.Y"/>
    {total}
  </cim:EnergyConsumer>
  <cim:Terminal rdf:ID="_{id}_t">
    <cim:Terminal.ConductingEquipment rdf:resource="#_{id}"/>
    <cim:Terminal.ConnectivityNode rdf:resource="#_n2"/>
    <cim:ACDCTerminal.sequenceNumber>1</cim:ACDCTerminal.sequenceNumber>
  </cim:Terminal>"##
    );
    for phase in ["A", "B", "C"] {
        let _ = write!(
            out,
            r##"<cim:EnergyConsumerPhase rdf:ID="_{id}_{phase}">
    <cim:EnergyConsumerPhase.EnergyConsumer rdf:resource="#_{id}"/>
    <cim:EnergyConsumerPhase.phase rdf:resource="http://iec.ch/TC57/CIM100#SinglePhaseKind.{phase}"/>
    {phase_p}
  </cim:EnergyConsumerPhase>"##
        );
    }
    out
}

/// A consumer whose phase records state only their phase shares its stated
/// total, and a total that disagrees with the phases is reported.
#[test]
fn consumer_totals_and_phase_powers_are_reconciled() {
    let text = feeder(&format!(
        "{}\n{}",
        consumer(
            "shared",
            "<cim:EnergyConsumer.p>30000</cim:EnergyConsumer.p><cim:EnergyConsumer.q>9000</cim:EnergyConsumer.q>",
            "",
        ),
        consumer(
            "conflict",
            "<cim:EnergyConsumer.p>5000</cim:EnergyConsumer.p><cim:EnergyConsumer.q>0</cim:EnergyConsumer.q>",
            "<cim:EnergyConsumerPhase.p>1000</cim:EnergyConsumerPhase.p><cim:EnergyConsumerPhase.q>0</cim:EnergyConsumerPhase.q>",
        ),
    ));
    let net = parse_str(&text, "cim-dist").unwrap();
    let shared = net.loads().iter().find(|l| l.name == "shared").unwrap();
    assert_eq!(shared.p_nom, vec![10_000.0; 3]);
    assert_eq!(shared.q_nom, vec![3_000.0; 3]);
    assert!(
        findings(&net.warnings, "READ.CIM_DIST.VALUE_DEFAULTED")
            .iter()
            .any(|f| f.contains("shared") && f.contains("split evenly")),
        "{:#?}",
        net.warnings
    );
    let conflict = net.loads().iter().find(|l| l.name == "conflict").unwrap();
    assert_eq!(conflict.p_nom, vec![1_000.0; 3]);
    assert!(
        findings(&net.warnings, "READ.CIM_DIST.VALUE_CONFLICT")
            .iter()
            .any(|f| f.contains("conflict") && f.contains("sum to 3000")),
        "{:#?}",
        net.warnings
    );
}

/// A total that differs from its phases' sum only by rounding in the
/// seventh significant figure, as the GridAPPS-D feeders state, is not a
/// conflict; a difference beyond one part in a million still is.
#[test]
fn consumer_totals_within_tolerance_are_not_conflicts() {
    let phase_p = "<cim:EnergyConsumerPhase.p>487.880653333</cim:EnergyConsumerPhase.p><cim:EnergyConsumerPhase.q>0</cim:EnergyConsumerPhase.q>";
    let text = feeder(&format!(
        "{}\n{}",
        consumer(
            "near",
            "<cim:EnergyConsumer.p>1463.642</cim:EnergyConsumer.p><cim:EnergyConsumer.q>0</cim:EnergyConsumer.q>",
            phase_p,
        ),
        consumer(
            "far",
            "<cim:EnergyConsumer.p>1463.65</cim:EnergyConsumer.p><cim:EnergyConsumer.q>0</cim:EnergyConsumer.q>",
            phase_p,
        ),
    ));
    let net = parse_str(&text, "cim-dist").unwrap();
    let conflicts = findings(&net.warnings, "READ.CIM_DIST.VALUE_CONFLICT");
    assert_eq!(conflicts.len(), 1, "{conflicts:#?}");
    assert!(conflicts[0].contains("far"), "{conflicts:#?}");
}

/// Written phase impedance data names its matrix through the standard
/// `PhaseImpedanceData.PhaseImpedance` association.
#[test]
fn written_phase_impedance_uses_the_standard_role() {
    let net = parse_file(fixture(), None).unwrap();
    let text = emit_value(&net, DistTargetFormat::CimDist).text;
    let entries = text.matches("<cim:PhaseImpedanceData rdf:").count();
    assert!(entries > 0, "{text}");
    assert_eq!(
        text.matches("<cim:PhaseImpedanceData.PhaseImpedance rdf:resource=")
            .count(),
        entries,
        "{text}"
    );
    assert!(!text.contains("PhaseImpedanceData.PerLengthPhaseImpedance"));
}

/// Each transformer takes the mesh impedance between its own ends, and a
/// tap changer position sets the winding tap.
#[test]
fn transformer_reactance_and_taps_come_from_their_own_records() {
    let transformer = |id: &str, x: f64, step: Option<i32>| {
        let tap = step.map_or_else(String::new, |step| {
            format!(
                r##"<cim:RatioTapChanger rdf:ID="_{id}_tap">
    <cim:RatioTapChanger.TransformerEnd rdf:resource="#_{id}_e1"/>
    <cim:TapChanger.step>{step}</cim:TapChanger.step>
    <cim:TapChanger.neutralStep>0</cim:TapChanger.neutralStep>
    <cim:RatioTapChanger.stepVoltageIncrement>0.625</cim:RatioTapChanger.stepVoltageIncrement>
  </cim:RatioTapChanger>"##
            )
        });
        let end = |n: u8, bus: &str| {
            format!(
                r##"<cim:PowerTransformerEnd rdf:ID="_{id}_e{n}">
    <cim:PowerTransformerEnd.PowerTransformer rdf:resource="#_{id}"/>
    <cim:TransformerEnd.endNumber>{n}</cim:TransformerEnd.endNumber>
    <cim:TransformerEnd.Terminal rdf:resource="#_{id}_t{n}"/>
    <cim:PowerTransformerEnd.ratedU>12470</cim:PowerTransformerEnd.ratedU>
    <cim:PowerTransformerEnd.ratedS>1000000</cim:PowerTransformerEnd.ratedS>
    <cim:PowerTransformerEnd.connectionKind rdf:resource="http://iec.ch/TC57/CIM100#WindingConnection.Yn"/>
  </cim:PowerTransformerEnd>
  <cim:Terminal rdf:ID="_{id}_t{n}">
    <cim:Terminal.ConductingEquipment rdf:resource="#_{id}"/>
    <cim:Terminal.ConnectivityNode rdf:resource="#_{bus}"/>
    <cim:Terminal.phases rdf:resource="http://iec.ch/TC57/CIM100#PhaseCode.ABCN"/>
    <cim:ACDCTerminal.sequenceNumber>{n}</cim:ACDCTerminal.sequenceNumber>
  </cim:Terminal>"##
            )
        };
        format!(
            r##"<cim:PowerTransformer rdf:ID="_{id}"><cim:IdentifiedObject.name>{id}</cim:IdentifiedObject.name></cim:PowerTransformer>
  {}
  {}
  <cim:TransformerMeshImpedance rdf:ID="_{id}_z">
    <cim:TransformerMeshImpedance.FromTransformerEnd rdf:resource="#_{id}_e1"/>
    <cim:TransformerMeshImpedance.ToTransformerEnd rdf:resource="#_{id}_e2"/>
    <cim:TransformerMeshImpedance.x>{x}</cim:TransformerMeshImpedance.x>
  </cim:TransformerMeshImpedance>
  {tap}"##,
            end(1, "n1"),
            end(2, "n2"),
        )
    };
    let text = feeder(&format!(
        "{}\n{}",
        transformer("ta", 7.775_209, Some(4)),
        transformer("tb", 15.550_418, None)
    ));
    let net = parse_str(&text, "cim-dist").unwrap();
    let z_base = 12470.0_f64.powi(2) / 1e6;
    let ta = net.transformers().iter().find(|t| t.name == "ta").unwrap();
    let tb = net.transformers().iter().find(|t| t.name == "tb").unwrap();
    assert!(close(ta.xsc_pct[0], 7.775_209 / z_base * 100.0));
    assert!(close(tb.xsc_pct[0], 15.550_418 / z_base * 100.0));
    assert!(close(ta.windings[0].tap, 1.025));
    assert!(close(tb.windings[0].tap, 1.0));
}

#[test]
fn malformed_sets_are_refused() {
    // One id defined twice.
    let twice = feeder(r#"<cim:ConnectivityNode rdf:ID="_n1"/>"#);
    let error = parse_str(&twice, "cim-dist").unwrap_err();
    assert!(error.to_string().contains("defined twice"), "{error}");
    // A DTD.
    let dtd = format!(
        "<!DOCTYPE rdf:RDF>{}",
        feeder("")
            .trim_start_matches(|c| c != '<')
            .split_once("?>")
            .unwrap()
            .1
    );
    assert!(parse_str(&dtd, "cim-dist").is_err());
    // Bus branch documents name what is missing.
    let topological = format!(r#"{RDF_OPEN}<cim:TopologicalNode rdf:ID="_tn"/></rdf:RDF>"#);
    let error = parse_str(&topological, "cim-dist").unwrap_err();
    assert!(error.to_string().contains("TopologicalNode"), "{error}");
}

/// Bundled conductors (several on one terminal) collapse to one phase each,
/// with a finding, and the output still reads back.
#[test]
fn bundled_conductors_write_each_phase_once() {
    let net = parse_file(micro("linecode_10x10.dss"), None).unwrap();
    let out = emit_value(&net, DistTargetFormat::CimDist);
    assert!(
        findings(&out.warnings, "EMIT.CIM_DIST.VALUE_COLLAPSED")
            .iter()
            .any(|f| f.contains("more than one conductor")),
        "{:#?}",
        out.warnings
    );
    let back = parse_str(&out.text, "cim-dist").unwrap();
    assert_eq!(back.lines().len(), net.lines().len());
}

/// Every `rdf:ID` in written CIM is defined once, and every `rdf:resource`
/// naming a document object (`#_id`) names one of them.
fn assert_references_resolve(text: &str, path: &std::path::Path) {
    let attribute = |name: &str| -> Vec<&str> {
        text.split(name)
            .skip(1)
            .map(|rest| rest.split('"').next().unwrap())
            .collect()
    };
    let ids = attribute("rdf:ID=\"");
    let defined: std::collections::BTreeSet<&str> = ids.iter().copied().collect();
    assert_eq!(
        defined.len(),
        ids.len(),
        "{}: an rdf:ID repeats",
        path.display()
    );
    for target in attribute("rdf:resource=\"") {
        if let Some(id) = target.strip_prefix('#') {
            assert!(
                defined.contains(id),
                "{}: rdf:resource `{target}` names no written object",
                path.display()
            );
        }
    }
}

/// Every distribution fixture the repository carries writes CIM that the
/// reader accepts, and the canonical output is idempotent: write, read,
/// write is byte stable.
#[test]
fn every_distribution_fixture_writes_stable_cim() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/data/dist");
    let mut checked = 0;
    for dir in ["micro", "pmd", "bmopf", "cim"] {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(root.join(dir))
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| matches!(e, "dss" | "json" | "xml"))
            })
            .collect();
        paths.sort();
        for path in paths {
            // Fixtures that are not standalone cases (schemas, fragments,
            // solution files) are another suite's business.
            let Ok(net) = parse_file(&path, None) else {
                continue;
            };
            let first = emit_value(&net, DistTargetFormat::CimDist);
            assert_references_resolve(&first.text, &path);
            let back = parse_str(&first.text, "cim-dist").unwrap_or_else(|e| {
                panic!("{}: CIM output does not read back: {e}", path.display())
            });
            let second = emit_value(&back, DistTargetFormat::CimDist);
            assert_eq!(
                first.text,
                second.text,
                "{}: rewrite differs",
                path.display()
            );
            checked += 1;
        }
    }
    assert!(checked >= 10, "only {checked} fixtures checked");
}
