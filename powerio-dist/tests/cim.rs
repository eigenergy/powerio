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

/// A directory case reads through the shared `Source` acquisition, so the
/// path checks every directory format gets apply: a top level `.xml` that is
/// a symbolic link to a document outside the directory is refused, not
/// followed.
#[cfg(unix)]
#[test]
fn a_directory_refuses_a_symlinked_document() {
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("feeder.xml"), feeder("")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a_eq.xml"), feeder("")).unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("feeder.xml"),
        dir.path().join("b_link.xml"),
    )
    .unwrap();
    let error = parse_file(dir.path(), Some("cim-dist")).unwrap_err();
    assert!(error.to_string().contains("symbolic link"), "{error}");
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
    // A tank without catalog data and the unmapped classes are reported,
    // not dropped silently.
    assert!(
        findings(w, "READ.CIM_DIST.RECORD_DROPPED")
            .iter()
            .any(|f| f.contains("TransformerTank") && f.contains("no TransformerTankInfo")),
        "{w:#?}"
    );
    let classes = findings(w, "READ.CIM_DIST.CLASS_UNMAPPED");
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

/// The content the profile defines and the 0.11 mapping does not type
/// (regulator and tap changer controls, distributed energy resources) is
/// reported with a code on read and on write, and a bus branch document with
/// no `ConnectivityNode` is refused rather than read as an empty feeder.
#[test]
fn controls_der_and_bus_branch_input_are_reported_not_mapped() {
    let extra = r#"
  <cim:RegulatingControl rdf:ID="_rc">
    <cim:IdentifiedObject.name>rc</cim:IdentifiedObject.name>
    <cim:RegulatingControl.targetValue>122</cim:RegulatingControl.targetValue>
  </cim:RegulatingControl>
  <cim:TapChangerControl rdf:ID="_tcc">
    <cim:IdentifiedObject.name>tcc</cim:IdentifiedObject.name>
  </cim:TapChangerControl>
  <cim:PowerElectronicsConnection rdf:ID="_pec">
    <cim:IdentifiedObject.name>pv</cim:IdentifiedObject.name>
    <cim:PowerElectronicsConnection.p>1000</cim:PowerElectronicsConnection.p>
  </cim:PowerElectronicsConnection>
"#;
    let net = parse_str(&feeder(extra), "cim-dist").unwrap();
    let classes = findings(&net.warnings, "READ.CIM_DIST.CLASS_UNMAPPED");
    for class in [
        "RegulatingControl",
        "TapChangerControl",
        "PowerElectronicsConnection",
    ] {
        assert!(
            classes.iter().any(|f| f.contains(class)),
            "{class}: {:#?}",
            net.warnings
        );
    }

    // An inverter and its control written to CIM are reported as dropped.
    let net = parse_file(micro("ibr_pv_control.dss"), None).unwrap();
    let out = emit_value(&net, DistTargetFormat::CimDist);
    let dropped = findings(&out.warnings, "EMIT.CIM_DIST.RECORD_DROPPED");
    assert!(
        dropped
            .iter()
            .any(|f| f.contains("inverter based resource")),
        "{:#?}",
        out.warnings
    );

    let bus_branch = format!(
        r#"{RDF_OPEN}
  <cim:TopologicalNode rdf:ID="_tn1"><cim:IdentifiedObject.name>tn1</cim:IdentifiedObject.name></cim:TopologicalNode>
  <cim:ACLineSegmentPhase rdf:ID="_p"/>
</rdf:RDF>
"#
    );
    let error = parse_str(&bus_branch, "cim-dist").unwrap_err();
    assert!(
        error.to_string().contains("TopologicalNode only"),
        "{error}"
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
        DistTargetFormat::Gridlabd,
        DistTargetFormat::GridlabdArras,
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

#[test]
fn dss_source_impedance_is_written_once_in_cim() {
    for phases in [1, 3] {
        let net = parse_str(
            &format!(
                "New Circuit.c phases={phases} bus1=s basekv=12.47 r1=0.1 x1=0.2 r0=0.3 x0=0.4"
            ),
            "dss",
        )
        .unwrap();
        assert_eq!(net.lines().len(), 1);
        let mut network = net.module.value().clone();
        // Retained native metadata cannot change the typed ideal source.
        network.sources_mut()[0]
            .extras
            .insert("basekv".into(), serde_json::json!(12.47));
        let out = emit_value(&network, DistTargetFormat::CimDist);
        assert!(!out.text.contains("<cim:EnergySource.r>"));
        assert!(!out.text.contains("<cim:EnergySource.x>"));
        let back = parse_str(&out.text, "cim-dist").unwrap();
        assert_eq!(back.lines().len(), 1);
        let original = network.linecode(&network.lines()[0].linecode).unwrap();
        let restored = back.linecode(&back.lines()[0].linecode).unwrap();
        for (a, b) in original
            .r_series
            .iter()
            .flatten()
            .zip(restored.r_series.iter().flatten())
        {
            assert!(close(*a, *b));
        }
        for (a, b) in original
            .x_series
            .iter()
            .flatten()
            .zip(restored.x_series.iter().flatten())
        {
            assert!(close(*a, *b));
        }
    }
}

#[test]
fn both_glm_profiles_write_cim_loads_and_sources() {
    let text = "module powerflow { solver_method FBS; }\nobject node { name 1; phases AN; bustype SWING; nominal_voltage 240; }\nobject load { name 2; parent 1; phases AN; nominal_voltage 240; constant_power_A 100+20j; }";
    for format in ["gridlabd", "gridlabd-arras"] {
        let net = parse_str(text, format).unwrap();
        let out = emit_value(&net, DistTargetFormat::CimDist);
        let back = parse_str(&out.text, "cim-dist").unwrap();
        assert_eq!(back.sources().len(), net.sources().len());
        assert_eq!(back.loads().len(), net.loads().len());
        assert!(close(back.loads()[0].p_nom[0], 100.0));
        assert!(close(back.loads()[0].q_nom[0], 20.0));
        assert!(close(back.sources()[0].v_magnitude[0], 240.0));
    }
}

#[test]
fn cim_reports_bounded_load_law_loss() {
    let net = parse_str(
        "New Circuit.c bus1=s basekv=12.47\nNew Load.l bus1=s phases=3 kv=12.47 kw=10 model=8 zipv=[0.2 0.3 0.5 0.2 0.3 0.5 0.7]",
        "dss",
    ).unwrap();
    let out = emit_value(&net, DistTargetFormat::CimDist);
    assert!(
        out.warnings
            .iter()
            .any(|finding| finding.starts_with("EMIT.CIM_DIST.FIELD_DROPPED")
                && finding.contains("voltage dependence model"))
    );
}

// ---------------------------------------------------------------------------
// Transformer tanks
// ---------------------------------------------------------------------------

fn tank_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/data/dist/cim/tank_feeder.xml")
}

fn transformer<'a>(
    net: &'a powerio_dist::MulticonductorNetwork,
    name: &str,
) -> &'a powerio_dist::DistTransformer {
    net.transformers()
        .iter()
        .find(|t| t.name == name)
        .unwrap_or_else(|| panic!("transformer {name} missing"))
}

/// Every value the fixture README pins: catalog ratings, resistances from
/// the end infos, reactances from the short circuit tests, terminal maps
/// from the tank end phases, taps on tank ends, and no-load data from both
/// forms.
#[test]
fn tank_feeder_maps_exactly() {
    let net = parse_file(tank_fixture(), None).unwrap();
    let unexpected: Vec<&String> = net
        .warnings
        .iter()
        .filter(|w| !w.starts_with("READ.CIM_DIST.CONTEXT_UNMAPPED"))
        .collect();
    assert!(unexpected.is_empty(), "{unexpected:#?}");
    assert_eq!(net.transformers().len(), 9);

    // Center tap: s1 is 1, 4 and s2 is 4, 2, so the halves stack.
    let ct = transformer(&net, "ct_tank");
    assert_eq!(ct.phases, 1);
    let maps: Vec<&[String]> = ct.windings.iter().map(|w| &w.terminal_map[..]).collect();
    assert_eq!(maps, [&["1", "4"][..], &["1", "4"], &["4", "2"]]);
    for (w, (v, r)) in ct
        .windings
        .iter()
        .zip([(7200.0, 0.6), (120.0, 1.2), (120.0, 1.2)])
    {
        assert!(close(w.v_ref, v));
        assert!(close(w.s_rating, 25_000.0));
        assert!(close(w.r_pct, r), "r {} vs {r}", w.r_pct);
    }
    for (x, expected) in ct.xsc_pct.iter().zip([2.04, 2.04, 1.36]) {
        assert!(close(*x, expected), "xsc {x} vs {expected}");
    }
    assert_eq!(ct.extras["cim_form"], "tank");
    assert_eq!(ct.extras["cim_power_transformer"], "ct");
    assert_eq!(ct.extras["cim_tank_info"], "ct_info");
    assert_eq!(
        ct.extras["cim_phase_angle_clock"],
        serde_json::json!([0.0, 0.0, 6.0])
    );
    assert_eq!(
        ct.extras["cim_end_mrids"],
        serde_json::json!(["ct_tank_e1", "ct_tank_e2", "ct_tank_e3"])
    );

    // Open wye / open delta: line to line secondaries are delta legs.
    let t1 = transformer(&net, "od_t1");
    let t2 = transformer(&net, "od_t2");
    assert_eq!(t1.windings[1].terminal_map, ["1", "2"]);
    assert_eq!(t1.windings[1].conn, DistWindingConn::Delta);
    assert_eq!(t2.windings[0].terminal_map, ["2", "4"]);
    assert_eq!(t2.windings[1].terminal_map, ["2", "3"]);
    assert_eq!(t1.extras["cim_power_transformer"], "od");
    assert_eq!(t2.extras["cim_power_transformer"], "od");
    assert!(close(t1.xsc_pct[0], 5.75));

    // Single phase with a no-load test: 0.05 kW and 0.538516% on 25 kVA.
    let sp = transformer(&net, "sp_tank");
    assert!(close(sp.extras["%noloadloss"].as_f64().unwrap(), 0.2));
    assert!(close(sp.extras["%imag"].as_f64().unwrap(), 0.5));
    assert!(close(sp.xsc_pct[0], 2.04));

    // Three phase delta/wye.
    let dy = transformer(&net, "dy_tank");
    assert_eq!(dy.phases, 3);
    assert_eq!(dy.windings[0].terminal_map, ["1", "2", "3"]);
    assert_eq!(dy.windings[0].conn, DistWindingConn::Delta);
    assert_eq!(dy.windings[1].terminal_map, ["1", "2", "3", "4"]);
    assert_eq!(
        dy.extras["cim_phase_angle_clock"],
        serde_json::json!([0.0, 1.0])
    );

    // The regulator bank: taps from the tap changers on the tank ends,
    // 1 + step · 0.625%.
    for (name, phase, tap) in [
        ("rega", "1", 1.0625),
        ("regb", "2", 1.05),
        ("regc", "3", 1.06875),
    ] {
        let reg = transformer(&net, name);
        assert_eq!(reg.windings[0].terminal_map, [phase, "4"]);
        assert!(close(reg.windings[0].tap, 1.0));
        assert!(close(reg.windings[1].tap, tap), "{name}");
        assert!(close(reg.xsc_pct[0], 0.01));
        assert!(close(reg.windings[0].r_pct, 0.005));
        assert_eq!(reg.windings[0].r_neutral, Some(0.0));
        assert_eq!(reg.extras["cim_power_transformer"], "reg");
        assert_eq!(reg.extras["cim_tank_info"], "reg_info");
    }
}

/// The tank form and the dss micro fixtures land on the same
/// `DistTransformer` values.
#[test]
fn tank_and_dss_fixtures_agree() {
    let cim = parse_file(tank_fixture(), None).unwrap();
    for (dss, pairs) in [
        ("xfmr_center_tap.dss", &[("t1", "ct_tank")][..]),
        (
            "xfmr_open_wye_open_delta.dss",
            &[("t1", "od_t1"), ("t2", "od_t2")],
        ),
        ("xfmr_single_phase.dss", &[("t1", "sp_tank")]),
        ("xfmr_delta_wye.dss", &[("t1", "dy_tank")]),
        ("xfmr_wye_delta.dss", &[("t1", "wd")]),
    ] {
        let net = parse_file(micro(dss), None).unwrap();
        for (dss_name, cim_name) in pairs {
            let a = transformer(&net, dss_name);
            let b = transformer(&cim, cim_name);
            let what = format!("{dss} {dss_name} vs {cim_name}");
            assert_eq!(a.phases, b.phases, "{what}");
            assert_eq!(a.windings.len(), b.windings.len(), "{what}");
            for (wa, wb) in a.windings.iter().zip(&b.windings) {
                assert_eq!(wa.terminal_map, wb.terminal_map, "{what}");
                assert_eq!(wa.conn, wb.conn, "{what}");
                assert!(close(wa.v_ref, wb.v_ref), "{what}");
                assert!(close(wa.s_rating, wb.s_rating), "{what}");
                assert!(close(wa.r_pct, wb.r_pct), "{what}: r");
                assert!(close(wa.tap, wb.tap), "{what}: tap");
            }
            assert_eq!(a.xsc_pct.len(), b.xsc_pct.len(), "{what}");
            for (xa, xb) in a.xsc_pct.iter().zip(&b.xsc_pct) {
                assert!(close(*xa, *xb), "{what}: xsc {xa} vs {xb}");
            }
        }
    }
}

/// A center tap tank `tank` under PowerTransformer `pt` on the two bus
/// feeder, after IEEE 13's `ct5`: phases B, s1 and s2, without catalog
/// data.
fn center_tap_tank(tank: &str, pt: &str) -> String {
    let mut out = format!(
        r##"<cim:PowerTransformer rdf:ID="_{pt}"><cim:IdentifiedObject.name>{pt}</cim:IdentifiedObject.name></cim:PowerTransformer>
  <cim:TransformerTank rdf:ID="_{tank}"><cim:IdentifiedObject.name>{tank}</cim:IdentifiedObject.name><cim:TransformerTank.PowerTransformer rdf:resource="#_{pt}"/></cim:TransformerTank>
"##
    );
    for (n, (phase, bus)) in [("B", "n1"), ("s1", "n2"), ("s2", "n2")]
        .into_iter()
        .enumerate()
    {
        let n = n + 1;
        let _ = write!(
            out,
            r##"  <cim:Terminal rdf:ID="_{tank}_t{n}"><cim:Terminal.ConductingEquipment rdf:resource="#_{pt}"/><cim:Terminal.ConnectivityNode rdf:resource="#_{bus}"/></cim:Terminal>
  <cim:TransformerTankEnd rdf:ID="_{tank}_e{n}"><cim:TransformerTankEnd.TransformerTank rdf:resource="#_{tank}"/><cim:TransformerTankEnd.phases rdf:resource="http://iec.ch/TC57/CIM100#PhaseCode.{phase}"/><cim:TransformerEnd.endNumber>{n}</cim:TransformerEnd.endNumber><cim:TransformerEnd.Terminal rdf:resource="#_{tank}_t{n}"/></cim:TransformerTankEnd>
"##
        );
    }
    out
}

/// The `ct5` catalog entry `_info`: 5 kVA, 2400/120/120 V, leakage
/// impedances 2.04%, 2.04% and 1.36% of the energised end's base, end
/// resistances `r` (ohm, `None` leaves one unstated) and test losses
/// `losses` (kW).
fn center_tap_catalog(r: [Option<f64>; 3], losses: [f64; 3]) -> String {
    let mut out = String::from(
        r#"  <cim:TransformerTankInfo rdf:ID="_info"/>
"#,
    );
    for (n, u) in [2400, 120, 120].into_iter().enumerate() {
        let n = n + 1;
        let resistance = r[n - 1].map_or_else(String::new, |r| {
            format!("<cim:TransformerEndInfo.r>{r}</cim:TransformerEndInfo.r>")
        });
        let _ = writeln!(
            out,
            r##"  <cim:TransformerEndInfo rdf:ID="_info_e{n}"><cim:TransformerEndInfo.TransformerTankInfo rdf:resource="#_info"/><cim:TransformerEndInfo.endNumber>{n}</cim:TransformerEndInfo.endNumber><cim:TransformerEndInfo.connectionKind rdf:resource="http://iec.ch/TC57/CIM100#WindingConnection.I"/><cim:TransformerEndInfo.ratedU>{u}</cim:TransformerEndInfo.ratedU><cim:TransformerEndInfo.ratedS>5000</cim:TransformerEndInfo.ratedS>{resistance}</cim:TransformerEndInfo>"##
        );
    }
    for ((i, j, z), loss) in [
        (1, 2, 23.5008_f64.hypot(20.736)),
        (1, 3, 23.5008_f64.hypot(20.736)),
        (2, 3, 0.039_168_f64.hypot(0.069_12)),
    ]
    .into_iter()
    .zip(losses)
    {
        let _ = writeln!(
            out,
            r##"  <cim:ShortCircuitTest rdf:ID="_sc{i}{j}"><cim:ShortCircuitTest.EnergisedEnd rdf:resource="#_info_e{i}"/><cim:ShortCircuitTest.GroundedEnds rdf:resource="#_info_e{j}"/><cim:ShortCircuitTest.leakageImpedance>{z}</cim:ShortCircuitTest.leakageImpedance><cim:ShortCircuitTest.loss>{loss}</cim:ShortCircuitTest.loss><cim:TransformerTest.basePower>5000</cim:TransformerTest.basePower></cim:ShortCircuitTest>"##
        );
    }
    out
}

/// Tank `ct` with its catalog entry, linked by `_asset`.
fn center_tap(r: [Option<f64>; 3], losses: [f64; 3]) -> String {
    format!(
        r##"{}  <cim:Asset rdf:ID="_asset"><cim:Asset.AssetInfo rdf:resource="#_info"/><cim:Asset.PowerSystemResources rdf:resource="#_ct"/></cim:Asset>
{}"##,
        center_tap_tank("ct", "pt"),
        center_tap_catalog(r, losses)
    )
}

/// End resistances of 0.6%, 1.2% and 1.2% (6.912 ohm on 1152 ohm, 0.03456
/// ohm on 2.88 ohm twice).
const CT5_R: [Option<f64>; 3] = [Some(6.912), Some(0.034_56), Some(0.034_56)];

/// `ct5` states a loss on the 1 to 2 test only (0.09 kW, 1.8%) and 0 on the
/// others. A loss of 0 states nothing, so the end resistances stand, and the
/// stated loss agrees with them (0.6 + 1.2).
#[test]
fn zero_test_losses_defer_to_end_resistance() {
    let net = parse_str(&feeder(&center_tap(CT5_R, [0.09, 0.0, 0.0])), "cim-dist").unwrap();
    let ct = transformer(&net, "ct");
    let r: Vec<f64> = ct.windings.iter().map(|w| w.r_pct).collect();
    assert!(
        close(r[0], 0.6) && close(r[1], 1.2) && close(r[2], 1.2),
        "{r:?}"
    );
    assert!(
        findings(&net.warnings, "READ.CIM_DIST.VALUE_CONFLICT").is_empty(),
        "{:#?}",
        net.warnings
    );
    // Standard magnitudes include the resolved end resistances, even on
    // pairs whose loss is zero/unstated.
    assert!(close(ct.xsc_pct[0], 2.04));
    assert!(close(ct.xsc_pct[1], 2.04));
    assert!(close(ct.xsc_pct[2], 1.36));
}

/// A nonzero test loss that disagrees with the end resistances is reported,
/// and the end resistances are kept.
#[test]
fn test_losses_and_end_resistance_conflict_is_reported() {
    let net = parse_str(&feeder(&center_tap(CT5_R, [0.2, 0.0, 0.0])), "cim-dist").unwrap();
    let ct = transformer(&net, "ct");
    assert!(close(ct.windings[0].r_pct, 0.6));
    assert!(close(ct.windings[1].r_pct, 1.2));
    let conflicts = findings(&net.warnings, "READ.CIM_DIST.VALUE_CONFLICT");
    assert_eq!(conflicts.len(), 1, "{:#?}", net.warnings);
    assert!(conflicts[0].contains("end resistances are kept"));
}

/// With no end resistance stated, the losses split as CIMHub's
/// `SetWindingResistances` does: the star split for three windings.
#[test]
fn losses_split_when_no_end_states_a_resistance() {
    let net = parse_str(
        &feeder(&center_tap([None; 3], [0.09, 0.09, 0.12])),
        "cim-dist",
    )
    .unwrap();
    let ct = transformer(&net, "ct");
    // r12 = r13 = 1.8%, r23 = 2.4%: r1 = 0.6, r2 = r3 = 1.2.
    let r: Vec<f64> = ct.windings.iter().map(|w| w.r_pct).collect();
    assert!(
        close(r[0], 0.6) && close(r[1], 1.2) && close(r[2], 1.2),
        "{r:?}"
    );
}

#[test]
fn a_tank_without_catalog_data_is_dropped() {
    let text = center_tap(CT5_R, [0.09, 0.0, 0.0]).replace(
        r##"<cim:Asset.PowerSystemResources rdf:resource="#_ct"/>"##,
        "",
    );
    let net = parse_str(&feeder(&text), "cim-dist").unwrap();
    assert_eq!(net.transformers().as_slice(), []);
    let dropped = findings(&net.warnings, "READ.CIM_DIST.RECORD_DROPPED");
    assert!(
        dropped
            .iter()
            .any(|f| f.contains("TransformerTank ct") && f.contains("no TransformerTankInfo")),
        "{:#?}",
        net.warnings
    );
    // The tank ends go with it rather than being reported field by field.
    assert!(
        findings(&net.warnings, "READ.CIM_DIST.FIELD_UNMAPPED")
            .iter()
            .all(|f| !f.contains("TransformerTankEnd")),
        "{:#?}",
        net.warnings
    );
}

/// One `Asset` lists many tanks, in one document or extended from another:
/// every listed tank resolves its catalog entry, and the repeated role is
/// not a conflict.
#[test]
fn an_asset_lists_many_tanks() {
    let net = parse_file(tank_fixture(), None).unwrap();
    for name in ["od_t1", "od_t2"] {
        assert_eq!(transformer(&net, name).extras["cim_tank_info"], "od_info");
    }

    // A second tank on the same catalog entry, listed by an extension of
    // the Asset in another document.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("a.xml"),
        feeder(&format!(
            "{}{}",
            center_tap(CT5_R, [0.09, 0.0, 0.0]),
            center_tap_tank("ct2", "pt2")
        )),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("b.xml"),
        format!(
            r##"{RDF_OPEN}
  <cim:Asset rdf:about="#_asset"><cim:Asset.PowerSystemResources rdf:resource="#_ct2"/></cim:Asset>
</rdf:RDF>
"##
        ),
    )
    .unwrap();
    let net = parse_file(dir.path(), Some("cim-dist")).unwrap();
    assert_eq!(net.transformers().len(), 2, "{:#?}", net.warnings);
    assert_eq!(transformer(&net, "ct2").extras["cim_tank_info"], "info");
    assert!(
        findings(&net.warnings, "READ.CIM_DIST.VALUE_CONFLICT").is_empty(),
        "{:#?}",
        net.warnings
    );
}

/// The `PowerTransformerEnd` form's core admittance becomes the same
/// no-load percentages: `g` and `b` times end 1's impedance base.
#[test]
fn core_admittance_becomes_no_load_percentages() {
    let net = parse_file(tank_fixture(), None).unwrap();
    let wd = transformer(&net, "wd");
    assert!(!wd.extras.contains_key("cim_form"));
    assert!(close(wd.extras["%noloadloss"].as_f64().unwrap(), 0.1));
    assert!(close(wd.extras["%imag"].as_f64().unwrap(), 0.4));
    // And back: the writer states them as a core admittance on end 1.
    let out = emit_value(&net, DistTargetFormat::CimDist);
    assert!(
        out.text
            .contains("TransformerCoreAdmittance.TransformerEnd")
    );
    let back = parse_str(&out.text, "cim-dist").unwrap();
    let again = transformer(&back, "wd");
    assert_eq!(again.extras["%noloadloss"], wd.extras["%noloadloss"]);
    assert_eq!(again.extras["%imag"], wd.extras["%imag"]);
}

/// Write then read keeps the tank grouping, the imported mRIDs, catalog
/// sharing, and the form, and every value comes back bit for bit.
#[test]
fn tank_banks_round_trip() {
    let net = parse_file(tank_fixture(), None).unwrap();
    let out = emit_value(&net, DistTargetFormat::CimDist);
    assert!(out.warnings.is_empty(), "{:#?}", out.warnings);
    let count = |class: &str| out.text.matches(&format!("<cim:{class} ")).count();
    // Six PowerTransformers (the two banks group their tanks), eight tanks,
    // and five catalog entries: the regulators share one.
    assert_eq!(count("PowerTransformer"), 6);
    assert_eq!(count("TransformerTank"), 8);
    assert_eq!(count("TransformerTankInfo"), 5);
    assert_eq!(count("Asset"), 5);
    assert_eq!(count("NoLoadTest"), 1);
    assert_eq!(count("TransformerCoreAdmittance"), 1);
    let back = parse_str(&out.text, "cim-dist").unwrap();
    assert_eq!(back.transformers().len(), net.transformers().len());
    for (a, b) in net.transformers().iter().zip(back.transformers()) {
        assert_eq!(a, b, "{}", a.name);
    }
}

/// A dss center tap writes CIM (the `PowerTransformerEnd` form, decision D2)
/// and reads back as the transformer the dss reader produced, except the
/// reversed second half: a terminal `PhaseCode` names a set, not an order,
/// so `4, 2` comes back as `2, 4`, with a finding on output. The tank form
/// (`s2`) carries it.
#[test]
fn dss_center_tap_writes_and_rereads() {
    let net = parse_file(micro("xfmr_center_tap.dss"), None).unwrap();
    let out = emit_value(&net, DistTargetFormat::CimDist);
    assert!(!out.text.contains("TransformerTank"));
    assert!(
        findings(&out.warnings, "EMIT.CIM_DIST.VALUE_COLLAPSED")
            .iter()
            .any(|f| f.contains("winding 3") && f.contains("[4, 2]")),
        "{:#?}",
        out.warnings
    );
    let back = parse_str(&out.text, "cim-dist").unwrap();
    let (a, b) = (transformer(&net, "t1"), transformer(&back, "t1"));
    assert_eq!(b.windings[2].terminal_map, ["2", "4"]);
    for (wa, wb) in a.windings.iter().zip(&b.windings).take(2) {
        assert_eq!(wa.terminal_map, wb.terminal_map);
    }
    for (wa, wb) in a.windings.iter().zip(&b.windings) {
        assert_eq!(wa.conn, wb.conn);
        assert!(close(wa.v_ref, wb.v_ref));
        assert!(close(wa.r_pct, wb.r_pct));
    }
    for (xa, xb) in a.xsc_pct.iter().zip(&b.xsc_pct) {
        assert!(close(*xa, *xb));
    }
}

// ---------------------------------------------------------------------------
// CIM16 LV feeders
// ---------------------------------------------------------------------------
//
// A synthetic CIM16 feeder in the form some distribution exports take:
// sequence impedances, enumerations written as text, single phase fuses
// grouped by a composite switch, customer classes, and a Diagram Layout.

const CIM16_OPEN: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<rdf:RDF xmlns:cim="http://iec.ch/TC57/2013/CIM-schema-cim16#" xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
"#;

/// A terminal with its phase code written as text.
fn cim16_terminal(id: &str, equipment: &str, node: &str, phases: &str, sequence: u8) -> String {
    format!(
        r##"  <cim:Terminal rdf:ID="_{id}">
    <cim:Terminal.ConductingEquipment rdf:resource="#_{equipment}" />
    <cim:Terminal.ConnectivityNode rdf:resource="#_{node}" />
    <cim:Terminal.phases>{phases}</cim:Terminal.phases>
    <cim:ACDCTerminal.sequenceNumber>{sequence}</cim:ACDCTerminal.sequenceNumber>
  </cim:Terminal>
"##
    )
}

/// A single phase fuse between two nodes, optionally in a composite switch.
fn cim16_fuse(id: &str, from: &str, to: &str, phase: &str, composite: Option<&str>) -> String {
    let composite = composite.map_or_else(String::new, |cs| {
        format!("    <cim:Switch.CompositeSwitch rdf:resource=\"#_{cs}\" />\n")
    });
    format!(
        r#"  <cim:Fuse rdf:ID="_{id}">
    <cim:Switch.ratedCurrent>25.0</cim:Switch.ratedCurrent>
    <cim:Switch.normalOpen>false</cim:Switch.normalOpen>
{composite}    <cim:IdentifiedObject.name>{id}</cim:IdentifiedObject.name>
  </cim:Fuse>
{}{}"#,
        cim16_terminal(&format!("{id}_t1"), id, from, phase, 1),
        cim16_terminal(&format!("{id}_t2"), id, to, phase, 2),
    )
}

/// An energy consumer at `node` with its connection and powers as text.
fn cim16_consumer(id: &str, node: &str, phases: &str, connection: &str, p: f64, q: f64) -> String {
    format!(
        r#"  <cim:EnergyConsumer rdf:ID="_{id}">
    <cim:EnergyConsumer.phaseConnection>{connection}</cim:EnergyConsumer.phaseConnection>
    <cim:EnergyConsumer.p>{p}</cim:EnergyConsumer.p>
    <cim:EnergyConsumer.q>{q}</cim:EnergyConsumer.q>
    <cim:IdentifiedObject.name>{id}</cim:IdentifiedObject.name>
  </cim:EnergyConsumer>
{}"#,
        cim16_terminal(&format!("{id}_t1"), id, node, phases, 1),
    )
}

/// A diagram object of the GIS diagram drawing `element`, with its points as
/// (sequence number, x, y).
fn cim16_drawing(element: &str, points: &[(u32, f64, f64)]) -> String {
    let mut text = format!(
        r##"  <cim:DiagramObject rdf:ID="_DO_{element}">
    <cim:DiagramObject.rotation>0</cim:DiagramObject.rotation>
    <cim:DiagramObject.IdentifiedObject rdf:resource="#_{element}" />
    <cim:DiagramObject.Diagram rdf:resource="#_GIS" />
  </cim:DiagramObject>
"##
    );
    for (sequence, x, y) in points {
        let _ = write!(
            text,
            r##"  <cim:DiagramObjectPoint rdf:ID="_DOP_{element}_{sequence}">
    <cim:DiagramObjectPoint.DiagramObject rdf:resource="#_DO_{element}" />
    <cim:DiagramObjectPoint.sequenceNumber>{sequence}</cim:DiagramObjectPoint.sequenceNumber>
    <cim:DiagramObjectPoint.xPosition>{x}</cim:DiagramObjectPoint.xPosition>
    <cim:DiagramObjectPoint.yPosition>{y}</cim:DiagramObjectPoint.yPosition>
  </cim:DiagramObjectPoint>
"##
        );
    }
    text
}

/// The layout of [`cim16_feeder`] in made up projected coordinates, the
/// segment's points listed out of sequence.
fn cim16_layout() -> String {
    let mut text = r#"  <cim:Diagram rdf:ID="_GIS">
    <cim:IdentifiedObject.name>EPSG:3857</cim:IdentifiedObject.name>
  </cim:Diagram>
"#
    .to_owned();
    text.push_str(&cim16_drawing(
        "AC1",
        &[
            (2, 1_010.0, 2_005.0),
            (1, 1_000.0, 2_000.0),
            (3, 1_020.0, 2_000.0),
        ],
    ));
    text.push_str(&cim16_drawing("CS1", &[(1, 1_021.0, 2_000.5)]));
    text.push_str(&cim16_drawing("FU4", &[(1, 1_022.0, 1_999.0)]));
    text
}

/// A source at the head, a cable to a cabinet, three single phase fuses in a
/// composite switch to a house, one fuse on phase B to a flat, a consumer at
/// each, and the customer classes for the house.
fn cim16_feeder() -> String {
    let mut text = CIM16_OPEN.to_owned();
    text.push_str(
        r##"  <cim:Circuit rdf:ID="_FEEDER_1"><cim:IdentifiedObject.name>Feeder 1</cim:IdentifiedObject.name></cim:Circuit>
  <cim:PSRType rdf:ID="_PSR"><cim:IdentifiedObject.name>cable</cim:IdentifiedObject.name></cim:PSRType>
  <cim:ConnectivityNode rdf:ID="_CN_HEAD"><cim:IdentifiedObject.name>CN_HEAD</cim:IdentifiedObject.name></cim:ConnectivityNode>
  <cim:ConnectivityNode rdf:ID="_CN_CABINET"><cim:IdentifiedObject.name>CN_CABINET</cim:IdentifiedObject.name></cim:ConnectivityNode>
  <cim:ConnectivityNode rdf:ID="_CN_HOUSE"><cim:IdentifiedObject.name>CN_HOUSE</cim:IdentifiedObject.name></cim:ConnectivityNode>
  <cim:ConnectivityNode rdf:ID="_CN_FLAT"><cim:IdentifiedObject.name>CN_FLAT</cim:IdentifiedObject.name></cim:ConnectivityNode>
  <cim:EnergySource rdf:ID="_SRC">
    <cim:EnergySource.nominalVoltage>400.0</cim:EnergySource.nominalVoltage>
    <cim:IdentifiedObject.name>grid</cim:IdentifiedObject.name>
  </cim:EnergySource>
  <cim:WireInfo rdf:ID="_WI">
    <cim:WireInfo.ratedCurrent>140.0</cim:WireInfo.ratedCurrent>
    <cim:IdentifiedObject.name>Cable type A</cim:IdentifiedObject.name>
  </cim:WireInfo>
  <cim:PerLengthSequenceImpedance rdf:ID="_PLI">
    <cim:PerLengthSequenceImpedance.r>0.0004</cim:PerLengthSequenceImpedance.r>
    <cim:PerLengthSequenceImpedance.x>0.00008</cim:PerLengthSequenceImpedance.x>
    <cim:PerLengthSequenceImpedance.r0>0.0012</cim:PerLengthSequenceImpedance.r0>
    <cim:PerLengthSequenceImpedance.x0>0.0003</cim:PerLengthSequenceImpedance.x0>
    <cim:IdentifiedObject.name>LV_cable_4x70</cim:IdentifiedObject.name>
  </cim:PerLengthSequenceImpedance>
  <cim:ACLineSegment rdf:ID="_AC1">
    <cim:ACLineSegment.PerLengthImpedance rdf:resource="#_PLI" />
    <cim:Conductor.length>50.0</cim:Conductor.length>
    <cim:PowerSystemResource.AssetDatasheet rdf:resource="#_WI" />
    <cim:PowerSystemResource.PSRType rdf:resource="#_PSR" />
    <cim:IdentifiedObject.name>AC1</cim:IdentifiedObject.name>
  </cim:ACLineSegment>
  <cim:CompositeSwitch rdf:ID="_CS1"><cim:IdentifiedObject.name>CS1</cim:IdentifiedObject.name></cim:CompositeSwitch>
  <cim:ServiceLocation rdf:ID="_SL_HOUSE"><cim:IdentifiedObject.name>SL_HOUSE</cim:IdentifiedObject.name></cim:ServiceLocation>
  <cim:UsagePoint rdf:ID="_UP_HOUSE">
    <cim:UsagePoint.ServiceLocation rdf:resource="#_SL_HOUSE" />
    <cim:IdentifiedObject.name>UP_HOUSE</cim:IdentifiedObject.name>
  </cim:UsagePoint>
"##,
    );
    text.push_str(&cim16_terminal("SRC_t1", "SRC", "CN_HEAD", "ABC", 1));
    text.push_str(&cim16_terminal("AC1_t1", "AC1", "CN_HEAD", "ABC", 1));
    text.push_str(&cim16_terminal("AC1_t2", "AC1", "CN_CABINET", "ABC", 2));
    for (fuse, phase) in [("FU1", "A"), ("FU2", "B"), ("FU3", "C")] {
        text.push_str(&cim16_fuse(
            fuse,
            "CN_CABINET",
            "CN_HOUSE",
            phase,
            Some("CS1"),
        ));
    }
    text.push_str(&cim16_fuse("FU4", "CN_CABINET", "CN_FLAT", "B", None));
    text.push_str(&cim16_consumer(
        "HOUSE", "CN_HOUSE", "ABCN", "Y", 3000.0, 300.0,
    ));
    text.push_str(&cim16_consumer("FLAT", "CN_FLAT", "BN", "Y", 1000.0, 100.0));
    text.push_str(&cim16_layout());
    text.push_str("</rdf:RDF>\n");
    text
}

fn strings(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| (*n).to_owned()).collect()
}

#[test]
#[allow(clippy::float_cmp)] // exact literals and exact even splits are the assertion
fn a_cim16_feeder_maps_text_enumerations_ratings_and_defaults() {
    let text = cim16_feeder();
    // Routed from the content: the customer classes mark distribution CIM
    // in a document with no per phase class.
    assert!(powerio_dist::looks_like_cim_dist(text.as_bytes()));
    let source = powerio_core::Source::from_memory("feeder.xml", text.as_bytes().to_vec()).unwrap();
    assert!(powerio_dist::source_looks_like_cim_dist(&source));
    let net = parse_str(&text, "cim-dist").unwrap();
    assert_eq!(net.base_frequency(), 50.0, "{:#?}", net.warnings);

    // The phase codes stated as text are read: no element defaults to ABC.
    assert!(
        !net.warnings
            .iter()
            .any(|w| w.contains("no phases are stated")),
        "{:#?}",
        net.warnings
    );
    let switch = |name: &str| net.switches().iter().find(|s| s.name == name).unwrap();
    for (fuse, phase) in [("FU1", "1"), ("FU2", "2"), ("FU3", "3"), ("FU4", "2")] {
        let fuse = switch(fuse);
        assert_eq!(fuse.terminal_map_from, strings(&[phase]));
        assert_eq!(fuse.terminal_map_to, strings(&[phase]));
        assert!(!fuse.open);
    }

    // The segment: its sequence impedance as a line code, and the ampacity
    // of its wire catalog entry.
    let line = &net.lines()[0];
    assert_eq!(line.linecode, "LV_cable_4x70");
    assert_eq!(line.i_max, Some(vec![140.0; 3]));

    // A connection stated as text: the house is a wye load, the flat a
    // single phase load on phase B.
    let load = |name: &str| net.loads().iter().find(|l| l.name == name).unwrap();
    let house = load("HOUSE");
    assert_eq!(house.configuration, Configuration::Wye);
    assert_eq!(house.terminal_map, strings(&["1", "2", "3", "4"]));
    assert_eq!(house.p_nom, vec![1000.0; 3]);
    let flat = load("FLAT");
    assert_eq!(flat.configuration, Configuration::SinglePhase);
    assert_eq!(flat.terminal_map, strings(&["2", "4"]));
    let [grid] = net.sources().as_slice() else {
        panic!("{:?}", net.sources())
    };
    assert!(close(grid.v_magnitude[0], 400.0 / 3f64.sqrt()));

    // The default is a finding; the groupings are context, and the customer
    // classes the model has no table for are reported.
    assert!(
        net.warnings
            .iter()
            .any(|w| w.contains("50 Hz is assumed for the CIM16 namespace")),
        "{:#?}",
        net.warnings
    );
    for class in ["CompositeSwitch", "Circuit", "PSRType"] {
        assert!(
            net.warnings
                .iter()
                .any(|w| w.starts_with("READ.CIM_DIST.CONTEXT_UNMAPPED") && w.contains(class)),
            "{class}: {:#?}",
            net.warnings
        );
    }
    let unmapped = findings(&net.warnings, "READ.CIM_DIST.CLASS_UNMAPPED");
    for class in ["ServiceLocation", "UsagePoint"] {
        assert!(
            unmapped.iter().any(|w| w.contains(class)),
            "{class}: {unmapped:#?}"
        );
    }
    assert!(
        !unmapped.iter().any(|w| w.contains("WireInfo")),
        "{unmapped:#?}"
    );
}

#[test]
fn a_cim16_feeder_converts_to_every_distribution_format() {
    // Once in the multiconductor model, the feeder writes as every
    // distribution format through the shared writers.
    let net = parse_str(&cim16_feeder(), "cim-dist").unwrap();
    for (target, token) in [
        (DistTargetFormat::Dss, "dss"),
        (DistTargetFormat::PmdJson, "pmd-json"),
        (DistTargetFormat::BmopfJson, "bmopf-json"),
        (DistTargetFormat::CimDist, "cim-dist"),
    ] {
        let out = emit_value(&net, target);
        let back = parse_str(&out.text, token)
            .unwrap_or_else(|e| panic!("{}: {e}\n{}", target.name(), out.text));
        // OpenDSS states an ideal source per phase, so only its presence
        // is format neutral.
        assert_ne!(back.sources().as_slice(), [], "{}", target.name());
        assert_eq!(back.loads().len(), net.loads().len(), "{}", target.name());
        assert!(
            close(back.base_frequency(), 50.0),
            "{}: {} Hz",
            target.name(),
            back.base_frequency()
        );
    }

    // The canonical CIM100 rewrite resolves every reference, and a second
    // write is byte identical.
    let first = emit_value(&net, DistTargetFormat::CimDist);
    assert_references_resolve(&first.text, std::path::Path::new("cim16 feeder"));
    let back = parse_str(&first.text, "cim-dist").unwrap();
    assert_eq!(
        emit_value(&back, DistTargetFormat::CimDist).text,
        first.text
    );
}

#[test]
#[allow(clippy::float_cmp)] // the source coordinates are the assertion
fn a_cim16_layout_locates_buses_and_routes_segments() {
    use powerio_dist::{CoordinateSpace, DistCoordsKind};

    let net = parse_str(&cim16_feeder(), "cim-dist").unwrap();
    let geo = net.geo().clone().expect("the diagram gives coordinates");
    assert_eq!(
        geo.space,
        CoordinateSpace::Projected {
            crs: Some("EPSG:3857".into())
        }
    );
    // The segment's route in sequence order, terminal 1 first.
    let route: Vec<(f64, f64)> = net.lines()[0]
        .route
        .as_ref()
        .unwrap()
        .iter()
        .map(|p| (p.x, p.y))
        .collect();
    assert_eq!(
        route,
        [(1_000.0, 2_000.0), (1_010.0, 2_005.0), (1_020.0, 2_000.0)]
    );
    let at = |id: &str| {
        let bus = net.buses().iter().find(|b| b.id == id).unwrap();
        let location = bus
            .location
            .unwrap_or_else(|| panic!("{id} has no location"));
        assert_eq!(location.kind, Some(DistCoordsKind::Derived), "{id}");
        (location.x, location.y)
    };
    // Segment ends place the nodes they reach; the customer side of the
    // cabinet takes its composite switch, and the flat its own fuse.
    assert_eq!(at("CN_HEAD"), (1_000.0, 2_000.0));
    assert_eq!(at("CN_CABINET"), (1_020.0, 2_000.0));
    assert_eq!(at("CN_HOUSE"), (1_021.0, 2_000.5));
    assert_eq!(at("CN_FLAT"), (1_022.0, 1_999.0));
    for class in ["Diagram", "DiagramObject", "DiagramObjectPoint"] {
        assert!(
            !net.warnings
                .iter()
                .any(|w| w.starts_with("READ.CIM_DIST.CLASS_UNMAPPED")
                    && w.contains(&format!(" {class} object"))),
            "{class}: {:#?}",
            net.warnings
        );
    }
}

// ---------------------------------------------------------------------------
// CIM16 LV exports with the Schneider Electric ADMS extension
// ---------------------------------------------------------------------------
//
// The feeder below is synthetic: hand written in the shape of the export,
// with invented names, values, and coordinates. It reuses the CIM16 helpers
// above.

const ADMS_OPEN: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<rdf:RDF xmlns:cim="http://iec.ch/TC57/2013/CIM-schema-cim16#" xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:sedms="http://www.schneider-electric-dms.com/CIM16v33/2017/extension">
"#;

/// A service location served from a fuse's downstream terminal, and the
/// usage points (meters) there with their demand.
fn adms_customer(location: &str, terminal: &str, phases: &str, meters: &[(&str, &str)]) -> String {
    let mut text = format!(
        r##"  <cim:ServiceLocation rdf:ID="_{location}">
    <sedms:ServiceLocation.Terminal rdf:resource="#_{terminal}" />
    <sedms:ServiceLocation.phases>{phases}</sedms:ServiceLocation.phases>
    <cim:IdentifiedObject.name>{location}</cim:IdentifiedObject.name>
  </cim:ServiceLocation>
"##
    );
    for (meter, powers) in meters {
        let _ = write!(
            text,
            r##"  <cim:UsagePoint rdf:ID="{meter}">
    <cim:UsagePoint.ServiceLocation rdf:resource="#_{location}" />
{powers}    <cim:IdentifiedObject.name>{meter}</cim:IdentifiedObject.name>
  </cim:UsagePoint>
"##
        );
    }
    text
}

/// The GIS layout of [`adms_feeder`], with made up projected coordinates and
/// the segment's points listed out of sequence.
fn adms_layout() -> String {
    let mut text = r#"  <cim:Diagram rdf:ID="_GIS">
    <sedms:Diagram.type>GIS</sedms:Diagram.type>
    <cim:IdentifiedObject.name>EPSG:3857</cim:IdentifiedObject.name>
  </cim:Diagram>
"#
    .to_owned();
    text.push_str(&cim16_drawing(
        "AC1",
        &[
            (2, 1_010.0, 2_005.0),
            (1, 1_000.0, 2_000.0),
            (3, 1_020.0, 2_000.0),
        ],
    ));
    text.push_str(&cim16_drawing("CS1", &[(1, 1_021.0, 2_000.5)]));
    text.push_str(&cim16_drawing("FU4", &[(1, 1_022.0, 1_999.0)]));
    text.push_str(&cim16_drawing("SL_HOUSE", &[(1, 1_021.0, 2_000.5)]));
    text
}

/// A synthetic LV feeder in the form the ADMS exports: CIM16 with the
/// `sedms` extension, enumerations as text, single phase fuses grouped by a
/// composite switch, customers as usage points at service locations, a
/// rooftop PV unit as a flexible load, a GIS layout, and no source: the head
/// node is stitched to the substation the export leaves out.
fn adms_feeder() -> String {
    let mut text = ADMS_OPEN.to_owned();
    text.push_str(
        r##"  <cim:Circuit rdf:ID="_LV_NET">
    <sedms:Circuit.isMaster>true</sedms:Circuit.isMaster>
    <cim:IdentifiedObject.name>LV network 1</cim:IdentifiedObject.name>
  </cim:Circuit>
  <cim:Circuit rdf:ID="_FEEDER_1">
    <sedms:Circuit.circuitType>LVFeeder</sedms:Circuit.circuitType>
  </cim:Circuit>
  <cim:ConnectivityNode rdf:ID="_CN_HEAD">
    <sedms:ConnectivityNode.stitchingInfo>LV</sedms:ConnectivityNode.stitchingInfo>
    <sedms:ConnectivityNode.connectedCircuits>_MV_FEEDER#_LV_NET</sedms:ConnectivityNode.connectedCircuits>
    <cim:ConnectivityNode.ConnectivityNodeContainer rdf:resource="#_FEEDER_1" />
    <cim:IdentifiedObject.name>CN_HEAD</cim:IdentifiedObject.name>
  </cim:ConnectivityNode>
  <cim:ConnectivityNode rdf:ID="_CN_CABINET"><cim:IdentifiedObject.name>CN_CABINET</cim:IdentifiedObject.name></cim:ConnectivityNode>
  <cim:ConnectivityNode rdf:ID="_CN_HOUSE"><cim:IdentifiedObject.name>CN_HOUSE</cim:IdentifiedObject.name></cim:ConnectivityNode>
  <cim:ConnectivityNode rdf:ID="_CN_FLAT"><cim:IdentifiedObject.name>CN_FLAT</cim:IdentifiedObject.name></cim:ConnectivityNode>
  <cim:WireInfo rdf:ID="_WI">
    <cim:WireInfo.ratedCurrent>140.0</cim:WireInfo.ratedCurrent>
    <cim:IdentifiedObject.name>Cable type A</cim:IdentifiedObject.name>
  </cim:WireInfo>
  <cim:PerLengthSequenceImpedance rdf:ID="_PLI">
    <cim:PerLengthSequenceImpedance.r>0.0004</cim:PerLengthSequenceImpedance.r>
    <cim:PerLengthSequenceImpedance.x>0.00008</cim:PerLengthSequenceImpedance.x>
    <cim:PerLengthSequenceImpedance.r0>0.0012</cim:PerLengthSequenceImpedance.r0>
    <cim:PerLengthSequenceImpedance.x0>0.0003</cim:PerLengthSequenceImpedance.x0>
    <cim:IdentifiedObject.name>LV_cable_4x70</cim:IdentifiedObject.name>
  </cim:PerLengthSequenceImpedance>
  <cim:ACLineSegment rdf:ID="_AC1">
    <cim:ACLineSegment.PerLengthImpedance rdf:resource="#_PLI" />
    <cim:Conductor.length>50.0</cim:Conductor.length>
    <sedms:ConductingEquipment.ratedVoltage>400.0000059604645</sedms:ConductingEquipment.ratedVoltage>
    <cim:PowerSystemResource.AssetDatasheet rdf:resource="#_WI" />
    <cim:IdentifiedObject.name>AC1</cim:IdentifiedObject.name>
  </cim:ACLineSegment>
  <cim:CompositeSwitch rdf:ID="_CS1"><cim:IdentifiedObject.name>CS1</cim:IdentifiedObject.name></cim:CompositeSwitch>
"##,
    );
    text.push_str(&cim16_terminal("AC1_t1", "AC1", "CN_HEAD", "ABC", 1));
    text.push_str(&cim16_terminal("AC1_t2", "AC1", "CN_CABINET", "ABC", 2));
    for (fuse, phase) in [("FU1", "A"), ("FU2", "B"), ("FU3", "C")] {
        text.push_str(&cim16_fuse(
            fuse,
            "CN_CABINET",
            "CN_HOUSE",
            phase,
            Some("CS1"),
        ));
    }
    text.push_str(&cim16_fuse("FU4", "CN_CABINET", "CN_FLAT", "B", None));
    let p = |w: &str, var: &str| {
        format!(
            "    <sedms:UsagePoint.p>{w}</sedms:UsagePoint.p>\n    <sedms:UsagePoint.q>{var}</sedms:UsagePoint.q>\n"
        )
    };
    let (house_1, house_2, flat) = (
        p("3000.0", "300.0"),
        p("600.0", "0.0"),
        p("1000.0", "100.0"),
    );
    let pv_meter = "    <sedms:UsagePoint.p>150.0</sedms:UsagePoint.p>\n".to_owned();
    text.push_str(&adms_customer(
        "SL_HOUSE",
        "FU1_t2",
        "ABCN",
        &[("1", &house_1), ("2", &house_2)],
    ));
    text.push_str(&adms_customer("SL_FLAT", "FU4_t2", "BN", &[("3", &flat)]));
    text.push_str(&adms_customer(
        "SL_PV",
        "FU2_t2",
        "ABC",
        &[("4", &pv_meter)],
    ));
    text.push_str(&adms_layout());
    text.push_str(
        r##"  <cim:FlexibleLoad rdf:ID="_FL1">
    <sedms:FlexibleLoad.pMax>9000.0</sedms:FlexibleLoad.pMax>
    <sedms:FlexibleLoad.pAvg>1500.0</sedms:FlexibleLoad.pAvg>
    <sedms:FlexibleLoad.Customer rdf:resource="#4" />
    <sedms:FlexibleLoad.dispatchable>false</sedms:FlexibleLoad.dispatchable>
    <sedms:FlexibleLoad.resourceType>SolarPanel</sedms:FlexibleLoad.resourceType>
    <cim:IdentifiedObject.name>PV1</cim:IdentifiedObject.name>
  </cim:FlexibleLoad>
</rdf:RDF>
"##,
    );
    text
}

#[test]
#[allow(clippy::float_cmp)] // exact literals and exact even splits are the assertion
fn an_adms_feeder_maps_customers_generation_and_supply() {
    let text = adms_feeder();
    // Routed from the name and the content: usage points mark distribution
    // CIM in a document with no per phase class.
    assert!(powerio_dist::looks_like_cim_dist(text.as_bytes()));
    let source = powerio_core::Source::from_memory("adms.xml", text.as_bytes().to_vec()).unwrap();
    assert!(powerio_dist::source_looks_like_cim_dist(&source));
    let net = parse_str(&text, "cim-dist").unwrap();
    assert_eq!(net.base_frequency(), 50.0, "{:#?}", net.warnings);

    // The phase codes stated as text are read: no element defaults to ABC.
    assert!(
        !net.warnings
            .iter()
            .any(|w| w.contains("no phases are stated")),
        "{:#?}",
        net.warnings
    );
    let switch = |name: &str| net.switches().iter().find(|s| s.name == name).unwrap();
    for (fuse, phase) in [("FU1", "1"), ("FU2", "2"), ("FU3", "3"), ("FU4", "2")] {
        let fuse = switch(fuse);
        assert_eq!(fuse.terminal_map_from, strings(&[phase]));
        assert_eq!(fuse.terminal_map_to, strings(&[phase]));
        assert!(!fuse.open);
    }

    // The segment: its sequence impedance under the catalog name, and the
    // ampacity of its wire catalog entry.
    let line = &net.lines()[0];
    assert_eq!(line.linecode, "LV_cable_4x70");
    assert_eq!(line.i_max, Some(vec![140.0; 3]));

    // One load per usage point, the stated powers shared over the phases of
    // the service location: wye with the neutral, delta without it.
    let load = |name: &str| net.loads().iter().find(|l| l.name == name).unwrap();
    let house = load("1");
    assert_eq!(house.bus, "CN_HOUSE");
    assert_eq!(house.terminal_map, strings(&["1", "2", "3", "4"]));
    assert_eq!(house.configuration, Configuration::Wye);
    assert_eq!(house.p_nom, vec![1000.0; 3]);
    assert_eq!(house.q_nom, vec![100.0; 3]);
    assert_eq!(load("2").p_nom, vec![200.0; 3]);
    let flat = load("3");
    assert_eq!(flat.bus, "CN_FLAT");
    assert_eq!(flat.terminal_map, strings(&["2", "4"]));
    assert_eq!(flat.configuration, Configuration::SinglePhase);
    assert_eq!(
        (flat.p_nom.clone(), flat.q_nom.clone()),
        (vec![1000.0], vec![100.0])
    );
    let pv_meter = load("4");
    assert_eq!(pv_meter.configuration, Configuration::Delta);
    assert_eq!(pv_meter.terminal_map, strings(&["1", "2", "3"]));
    assert_eq!(pv_meter.q_nom, vec![0.0; 3]);

    // The rooftop PV: a three leg inverter (the location has no neutral)
    // that holds its average output, not dispatchable.
    let [pv] = net.ibrs().as_slice() else {
        panic!("{:?}", net.ibrs())
    };
    assert_eq!(pv.name, "PV1");
    assert_eq!(pv.bus, "CN_HOUSE");
    assert_eq!(pv.topology, powerio_dist::IbrTopology::ThreeLeg);
    assert_eq!(pv.prime_mover, powerio_dist::IbrPrimeMover::Pv);
    assert_eq!(pv.terminal_map, strings(&["1", "2", "3"]));
    assert_eq!(pv.s_max, vec![3000.0; 3]);
    assert_eq!(pv.p_min, Some(vec![500.0; 3]));
    assert_eq!(pv.p_max, pv.p_min);
    assert_eq!(pv.q_min, Some(vec![0.0; 3]));

    // The supply the export leaves out: a busbar, a closed switch to the
    // stitched head named after its feeder, and an ideal source at the
    // rated 400 V (stated through single precision kilovolts).
    let busbar = net.buses().iter().find(|b| b.id == "sourcebus").unwrap();
    assert_eq!(busbar.terminals, strings(&["1", "2", "3", "4"]));
    assert_eq!(busbar.grounded, strings(&["4"]));
    let feeder = switch("FEEDER_1");
    assert_eq!(
        (feeder.bus_from.as_str(), feeder.bus_to.as_str()),
        ("sourcebus", "CN_HEAD")
    );
    assert_eq!(feeder.terminal_map_from, strings(&["1", "2", "3"]));
    let [source] = net.sources().as_slice() else {
        panic!("{:?}", net.sources())
    };
    assert_eq!(
        (source.name.as_str(), source.bus.as_str()),
        ("source", "sourcebus")
    );
    assert!(close(source.v_magnitude[0], 400.0 / 3f64.sqrt()));
    assert_eq!(source.v_magnitude[3], 0.0);

    // Every assumption is a finding, and the groupings are context.
    for expected in [
        "50 Hz is assumed for the CIM16 namespace",
        "places the ideal source `source` there at their rated 400 V",
        "no sedms:UsagePoint.q; 0 is used",
    ] {
        assert!(
            net.warnings.iter().any(|w| w.contains(expected)),
            "missing `{expected}` in {:#?}",
            net.warnings
        );
    }
    let unmapped = findings(&net.warnings, "READ.CIM_DIST.CLASS_UNMAPPED");
    assert_eq!(unmapped, Vec::<&String>::new());
    for class in ["CompositeSwitch", "Circuit"] {
        assert!(
            net.warnings
                .iter()
                .any(|w| w.starts_with("READ.CIM_DIST.CONTEXT_UNMAPPED") && w.contains(class)),
            "{class}: {:#?}",
            net.warnings
        );
    }
}

#[test]
fn an_adms_feeder_converts_to_every_distribution_format() {
    // Once in the multiconductor model, the export writes as every
    // distribution format through the shared writers: no target needs a
    // bridge of its own.
    let net = parse_str(&adms_feeder(), "cim-dist").unwrap();
    for (target, token) in [
        (DistTargetFormat::Dss, "dss"),
        (DistTargetFormat::PmdJson, "pmd-json"),
        (DistTargetFormat::BmopfJson, "bmopf-json"),
        (DistTargetFormat::CimDist, "cim-dist"),
    ] {
        let out = emit_value(&net, target);
        let back = parse_str(&out.text, token)
            .unwrap_or_else(|e| panic!("{}: {e}\n{}", target.name(), out.text));
        // OpenDSS states an ideal source per phase, so only its presence
        // is format neutral.
        assert_ne!(back.sources().as_slice(), [], "{}", target.name());
        assert_eq!(back.loads().len(), net.loads().len(), "{}", target.name());
        assert!(
            close(back.base_frequency(), 50.0),
            "{}: {} Hz",
            target.name(),
            back.base_frequency()
        );
    }

    // The canonical CIM100 rewrite resolves every reference, and a second
    // write is byte identical.
    let first = emit_value(&net, DistTargetFormat::CimDist);
    assert_references_resolve(&first.text, std::path::Path::new("adms feeder"));
    let back = parse_str(&first.text, "cim-dist").unwrap();
    assert_eq!(
        emit_value(&back, DistTargetFormat::CimDist).text,
        first.text
    );
}

#[test]
#[allow(clippy::float_cmp)] // the source coordinates are the assertion
fn an_adms_layout_locates_buses_and_routes_segments() {
    use powerio_dist::{CoordinateSpace, DistCoordsKind};

    let net = parse_str(&adms_feeder(), "cim-dist").unwrap();
    let geo = net
        .geo()
        .clone()
        .expect("the GIS diagram gives coordinates");
    assert_eq!(
        geo.space,
        CoordinateSpace::Projected {
            crs: Some("EPSG:3857".into())
        }
    );
    // The segment's route in sequence order, terminal 1 first.
    let route: Vec<(f64, f64)> = net.lines()[0]
        .route
        .as_ref()
        .unwrap()
        .iter()
        .map(|p| (p.x, p.y))
        .collect();
    assert_eq!(
        route,
        [(1_000.0, 2_000.0), (1_010.0, 2_005.0), (1_020.0, 2_000.0)]
    );
    let at = |id: &str| {
        let bus = net.buses().iter().find(|b| b.id == id).unwrap();
        let location = bus
            .location
            .unwrap_or_else(|| panic!("{id} has no location"));
        assert_eq!(location.kind, Some(DistCoordsKind::Derived), "{id}");
        (location.x, location.y)
    };
    // Segment ends place the nodes they reach; the customer side of the
    // cabinet takes its composite switch, the flat its own fuse, and the
    // supply busbar the stitched node it feeds.
    assert_eq!(at("CN_HEAD"), (1_000.0, 2_000.0));
    assert_eq!(at("CN_CABINET"), (1_020.0, 2_000.0));
    assert_eq!(at("CN_HOUSE"), (1_021.0, 2_000.5));
    assert_eq!(at("CN_FLAT"), (1_022.0, 1_999.0));
    assert_eq!(at("sourcebus"), (1_000.0, 2_000.0));
    for class in ["Diagram", "DiagramObject", "DiagramObjectPoint"] {
        assert!(
            !net.warnings
                .iter()
                .any(|w| w.starts_with("READ.CIM_DIST.CLASS_UNMAPPED")
                    && w.contains(&format!(" {class} object"))),
            "{class}: {:#?}",
            net.warnings
        );
    }
}
