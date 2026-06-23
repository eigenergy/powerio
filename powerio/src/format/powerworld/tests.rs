//! Unit tests for the generic aux grammar, one per construct of the format
//! guide.

use super::auxiliary::{AuxSection, parse_aux, write_aux};
use crate::Error;

fn one_object(text: &str) -> super::AuxObject {
    let file = parse_aux(text).unwrap();
    assert_eq!(file.sections.len(), 1, "expected one section");
    match file.sections.into_iter().next().unwrap() {
        AuxSection::Data(d) => d,
        AuxSection::Script(_) => panic!("expected a DATA section"),
    }
}

#[test]
fn legacy_single_line_header() {
    let d = one_object("DATA (Bus, [BusNum, BusName])\n{\n1 \"Alpha\"\n2 \"Beta\"\n}\n");
    assert_eq!(d.object_type, "Bus");
    assert_eq!(d.fields, ["BusNum", "BusName"]);
    assert_eq!(d.rows.len(), 2);
    assert_eq!(d.rows[0].values, ["1", "Alpha"]);
    assert_eq!(d.data_name, None);
}

#[test]
fn multiline_field_list_with_comments_and_blanks() {
    let d = one_object(
        "DATA (Bus, [BusNum, // trailing comment\n\
         \n\
         // a whole-line comment inside the field list\n\
         BusName, BusPUVolt,\n\
         BusAngle])\n{\n1 \"A\" 1.01 -3.5\n}\n",
    );
    assert_eq!(d.fields, ["BusNum", "BusName", "BusPUVolt", "BusAngle"]);
    assert_eq!(d.rows[0].values, ["1", "A", "1.01", "-3.5"]);
}

#[test]
fn location_suffixes_are_preserved() {
    let d = one_object("DATA (Branch, [BusNum, BusNum:1, LineR:1])\n{\n1 2 0.01\n}\n");
    assert_eq!(d.fields, ["BusNum", "BusNum:1", "LineR:1"]);
    assert_eq!(d.field_index("busnum:1"), Some(1));
}

#[test]
fn concise_header() {
    let d = one_object("Bus (BusNum, BusName)\n{\n1 \"A\"\n}\n");
    assert_eq!(d.object_type, "Bus");
    assert_eq!(d.fields, ["BusNum", "BusName"]);
    assert_eq!(d.rows.len(), 1);
}

#[test]
fn concise_header_with_data_name() {
    let d = one_object("Bus MySection(BusNum)\n{\n1\n}\n");
    assert_eq!(d.object_type, "Bus");
    assert_eq!(d.data_name.as_deref(), Some("MySection"));
}

#[test]
fn legacy_header_with_data_name_specifier_and_create() {
    let d = one_object("DATA Named(Bus, [BusNum, BusName], CSVAUX, NO)\n{\n1, \"A B\"\n}\n");
    assert_eq!(d.data_name.as_deref(), Some("Named"));
    assert_eq!(d.create_if_not_found.as_deref(), Some("NO"));
    assert_eq!(d.rows[0].values, ["1", "A B"]);
}

#[test]
fn csv_values_keep_empties_and_quoted_commas() {
    let d = one_object("DATA (Bus, [A, B, C], CSV)\n{\n1,,\"x, y\"\n}\n");
    assert_eq!(d.rows[0].values, ["1", "", "x, y"]);
}

#[test]
fn multiline_value_rows_complete_by_field_count() {
    let d = one_object(
        "DATA (Gen, [BusNum, GenID, GenMW, GenStatus])\n{\n1 \"1\"\n100.0 Closed\n2 \"1\" 50.0 Closed\n}\n",
    );
    assert_eq!(d.rows.len(), 2);
    assert_eq!(d.rows[0].values, ["1", "1", "100.0", "Closed"]);
    assert_eq!(d.rows[1].values, ["2", "1", "50.0", "Closed"]);
}

#[test]
fn quoted_strings_with_spaces_and_empty_quotes() {
    let d = one_object("DATA (Sub, [Name, City])\n{\n\"CREVE COEUR\" \"\"\n}\n");
    assert_eq!(d.rows[0].values, ["CREVE COEUR", ""]);
}

#[test]
fn comments_inside_body_are_stripped() {
    let d = one_object("DATA (Bus, [BusNum])\n{\n// comment row\n1 // trailing\n}\n");
    assert_eq!(d.rows.len(), 1);
    assert_eq!(d.rows[0].values, ["1"]);
}

#[test]
fn comment_marker_inside_quotes_is_data() {
    let d = one_object("DATA (Owner, [Name])\n{\n\"http://example\"\n}\n");
    assert_eq!(d.rows[0].values, ["http://example"]);
}

#[test]
fn subdata_attaches_to_the_row_above() {
    let d = one_object(
        "DATA (Contingency, [CTGLabel])\n{\n\"L_1\"\n<SUBDATA CTGElement>\n  BRANCH 1 2 1\n</SUBDATA>\n\"L_2\"\n}\n",
    );
    assert_eq!(d.rows.len(), 2);
    assert_eq!(d.rows[0].subdata.len(), 1);
    assert_eq!(d.rows[0].subdata[0].name, "CTGElement");
    assert_eq!(d.rows[0].subdata[0].lines, ["  BRANCH 1 2 1"]);
    assert!(d.rows[1].subdata.is_empty());
}

#[test]
fn subdata_interior_kept_verbatim_including_comments() {
    let d = one_object(
        "DATA (PWCaseInformation, [Selected])\n{\n\"NO \"\n<SUBDATA PWCaseHeader>\n//Case Description\n  free text, with commas\n</SUBDATA>\n}\n",
    );
    assert_eq!(
        d.rows[0].subdata[0].lines,
        ["//Case Description", "  free text, with commas"]
    );
}

#[test]
fn script_section_is_retained_verbatim() {
    let text = "SCRIPT MyActions\n{\nSolvePowerFlow;\nEnterMode(RUN);\n}\n";
    let file = parse_aux(text).unwrap();
    let AuxSection::Script(sc) = &file.sections[0] else {
        panic!("expected SCRIPT");
    };
    assert_eq!(sc.name.as_deref(), Some("MyActions"));
    assert_eq!(sc.lines, ["SolvePowerFlow;", "EnterMode(RUN);"]);
    // And it survives the canonical write.
    let out = write_aux(&file);
    assert!(out.contains("SCRIPT MyActions"));
    assert!(out.contains("SolvePowerFlow;"));
}

#[test]
fn brace_on_header_line_is_accepted() {
    let d = one_object("Bus (BusNum) {\n1\n}\n");
    assert_eq!(d.rows.len(), 1);
}

#[test]
fn canonical_write_is_idempotent() {
    let text = "DATA Named(Bus, [BusNum, BusName], AUXDEF, NO)\n{\n1 \"A B\"\n2 \"\"\n<SUBDATA Memo>\nnote line\n</SUBDATA>\n}\n\nSCRIPT\n{\nSolvePowerFlow;\n}\n";
    let first = write_aux(&parse_aux(text).unwrap());
    let second = write_aux(&parse_aux(&first).unwrap());
    assert_eq!(first, second, "canonical aux output must be idempotent");
}

// ---- Network mapping --------------------------------------------------------

use super::{parse_powerworld, write_powerworld};

#[test]
fn unmodeled_data_blocks_warn_on_parse() {
    let parsed = crate::parse_str(
        "DATA (Bus, [BusNum])\n{\n1\n}\n\
         DATA (Owner, [Name])\n{\n\"Utility\"\n}\n\
         DATA (Area, [Number, Name])\n{\n1 \"North\"\n}\n",
        "powerworld",
    )
    .unwrap();

    assert_eq!(parsed.network.buses.len(), 1);
    assert!(
        parsed
            .warnings
            .iter()
            .any(|w| w.contains("DATA Owner") && w.contains("not modeled")),
        "missing Owner warning: {:?}",
        parsed.warnings
    );
    assert!(
        parsed
            .warnings
            .iter()
            .any(|w| w.contains("DATA Area") && w.contains("not modeled")),
        "missing Area warning: {:?}",
        parsed.warnings
    );
}

#[test]
// Exact kV values parsed from the fixture; bit equality is the assertion.
#[allow(clippy::float_cmp)]
fn writer_sanitizes_bus_names_that_would_corrupt_a_value() {
    // A bus name carrying a double quote would close the quoted BusName field
    // early on re-read and shift every later column; the writer replaces it and
    // warns, so the second bus's nominal kV survives the round trip.
    let mut net = parse_powerworld(
        "DATA (Bus, [BusNum, BusName, BusNomVolt])\n{\n1 \"A\" 230\n2 \"B\" 138\n}\n",
    )
    .unwrap();
    net.buses[0].name = Some("O\"Brien".to_string());
    let conv = write_powerworld(&net);
    let reparsed = parse_powerworld(&conv.text).unwrap();
    assert_eq!(reparsed.buses.len(), 2);
    assert_eq!(reparsed.buses[0].base_kv, 230.0);
    assert_eq!(reparsed.buses[1].base_kv, 138.0);
    assert!(!reparsed.buses[0].name.as_deref().unwrap().contains('"'));
    assert!(
        conv.warnings.iter().any(|w| w.contains("bus name")),
        "expected a sanitization warning, got {:?}",
        conv.warnings
    );
}

#[test]
// Exact decimal fractions parsed from the fixture; bit equality is the assertion.
#[allow(clippy::float_cmp)]
fn zip_load_components_sum_and_survive_in_extras() {
    let net = parse_powerworld(
        "DATA (Bus, [BusNum, BusNomVolt])\n{\n1 115\n}\n\
         DATA (Load, [BusNum, LoadID, LoadStatus, LoadSMW, LoadSMVR, LoadIMW, LoadIMVR, LoadZMW, LoadZMVR])\n\
         {\n1 \"1 \" \"Closed\" 10.0 2.0 4.0 1.0 6.0 3.0\n}\n",
    )
    .unwrap();
    let l = &net.loads[0];
    assert_eq!(l.p, 20.0, "S + I + Z MW at nominal voltage");
    assert_eq!(l.q, 6.0);
    assert_eq!(
        l.extras.get("LoadIMW").and_then(|v| v.as_str()),
        Some("4.0"),
        "voltage dependent components kept in extras"
    );
    assert_eq!(l.extras.get("LoadID").and_then(|v| v.as_str()), Some("1"));
}

#[test]
// Exact decimal fractions parsed from the fixture; bit equality is the assertion.
#[allow(clippy::float_cmp)]
fn pure_constant_power_zip_load_keeps_no_component_extras() {
    let net = parse_powerworld(
        "DATA (Bus, [BusNum])\n{\n1\n}\n\
         DATA (Load, [BusNum, LoadSMW, LoadSMVR, LoadIMW, LoadIMVR, LoadZMW, LoadZMVR])\n\
         {\n1 10.0 2.0 0 0 0 0\n}\n",
    )
    .unwrap();
    let l = &net.loads[0];
    assert_eq!((l.p, l.q), (10.0, 2.0));
    assert!(!l.extras.contains_key("LoadSMW"));
}

#[test]
fn bus_kinds_derive_from_slack_flag_and_generators() {
    use crate::network::BusType;
    let net = parse_powerworld(
        "DATA (Bus, [BusNum, BusSlack])\n{\n1 \"YES \"\n2 \"NO \"\n3 \"NO \"\n}\n\
         DATA (Gen, [BusNum, GenID, GenStatus, GenMWSetPoint])\n\
         {\n1 \"1\" \"Closed\" 100\n2 \"1\" \"Closed\" 50\n3 \"1\" \"Open\" 0\n}\n",
    )
    .unwrap();
    let kinds: Vec<BusType> = net.buses.iter().map(|b| b.kind).collect();
    assert_eq!(
        kinds,
        [BusType::Ref, BusType::Pv, BusType::Pq],
        "slack flag wins, in-service gen promotes to PV, open gen does not"
    );
}

#[test]
// Exact decimal fractions parsed from the fixture; bit equality is the assertion.
#[allow(clippy::float_cmp)]
fn real_export_field_names_map_gen_shunt_and_transformer() {
    let net = parse_powerworld(
        "DATA (Bus, [BusNum])\n{\n1\n2\n}\n\
         DATA (Gen, [BusNum, GenID, GenStatus, GenMWSetPoint, GenMvrSetPoint, GenMWMax, GenMWMin, GenMVRMax, GenMVRMin, GenVoltSet, GenMVABase])\n\
         {\n1 \"1\" \"Closed\" 80.5 12.25 100 20 50 -50 1.04 120\n}\n\
         DATA (Shunt, [BusNum, ShuntID, SSStatus, SSNMW, SSNMVR])\n{\n2 \"1\" \"Closed\" 0 25.0\n}\n\
         DATA (Branch, [BusNum, BusNum:1, LineCircuit, BranchDeviceType, LineStatus, LineR:1, LineX:1, LineC:1, LineTap:1, LinePhase, LineAMVA, LineAMVA:1, LineAMVA:2])\n\
         {\n1 2 \" 1\" \"Transformer\" \"Closed\" 0.001 0.05 0.002 0.9875 -2.5 250 260 270\n}\n",
    )
    .unwrap();
    let g = &net.generators[0];
    assert_eq!((g.pg, g.qg, g.vg, g.mbase), (80.5, 12.25, 1.04, 120.0));
    let sh = &net.shunts[0];
    assert_eq!((sh.g, sh.b), (0.0, 25.0));
    assert!(sh.in_service);
    let br = &net.branches[0];
    assert_eq!((br.r, br.x, br.b), (0.001, 0.05, 0.002));
    assert_eq!((br.tap, br.shift), (0.9875, -2.5));
    assert_eq!((br.rate_a, br.rate_b, br.rate_c), (250.0, 260.0, 270.0));
    assert!(br.is_transformer());
    assert_eq!(
        br.extras.get("LineCircuit").and_then(|v| v.as_str()),
        Some(" 1"),
        "circuit ID kept verbatim, padding included"
    );
}

#[test]
// Exact decimal fractions parsed from the fixture; bit equality is the assertion.
#[allow(clippy::float_cmp)]
fn name_keyed_core_rows_resolve_bus_labels() {
    let net = parse_powerworld(
        "DATA (Bus, [BusName_NomVolt, BusNum, BusName, BusNomVolt, BusSlack])\n\
         {\n\"ALPHA_230.00\" 10 \"ALPHA\" 230 \"YES \"\n\"BETA_230.00\" 20 \"BETA\" 230 \"NO \"\n}\n\
         DATA (Load, [BusName_NomVolt, LoadID, LoadStatus, LoadSMW, LoadSMVR])\n\
         {\n\"BETA_230.00\" \"1\" \"Closed\" 12.0 4.0\n}\n\
         DATA (Gen, [BusName_NomVolt, GenID, GenStatus, GenMW, GenMVR, GenMWMax, GenMWMin])\n\
         {\n\"ALPHA_230.00\" \"1\" \"Closed\" 50.0 5.0 80.0 10.0\n}\n\
         DATA (Shunt, [BusName_NomVolt, ShuntID, SSStatus, SSNMW, SSNMVR])\n\
         {\n\"BETA_230.00\" \"1\" \"Closed\" 0.0 3.0\n}\n\
         DATA (Branch, [BusName_NomVolt, BusName_NomVolt:1, LineCircuit, LineStatus, LineR, LineX, LineC])\n\
         {\n\"ALPHA_230.00\" \"BETA_230.00\" \"1\" \"Closed\" 0.01 0.05 0.002\n}\n",
    )
    .unwrap();

    assert_eq!(net.buses[0].id, crate::network::BusId(10));
    assert_eq!(net.loads[0].bus, crate::network::BusId(20));
    assert_eq!(net.generators[0].bus, crate::network::BusId(10));
    assert_eq!(net.shunts[0].bus, crate::network::BusId(20));
    assert_eq!(net.branches[0].from, crate::network::BusId(10));
    assert_eq!(net.branches[0].to, crate::network::BusId(20));
    assert_eq!((net.branches[0].r, net.branches[0].x), (0.01, 0.05));
}

#[test]
fn blank_numeric_bus_keys_fall_back_to_labels_before_merging() {
    let net = parse_powerworld(
        "DATA (Bus, [BusName_NomVolt, BusNum, BusName, BusNomVolt, BusSlack])\n\
         {\n\"ALPHA_230.00\" 10 \"ALPHA\" 230 \"YES \"\n\"BETA_230.00\" 20 \"BETA\" 230 \"NO \"\n}\n\
         DATA (Load, [BusNum, BusName_NomVolt, LoadID, LoadStatus, LoadSMW])\n\
         {\n\"\" \"ALPHA_230.00\" \"1\" \"Closed\" 12.0\n\"\" \"BETA_230.00\" \"1\" \"Closed\" 34.0\n}\n",
    )
    .unwrap();

    assert_eq!(net.loads.len(), 2);
    assert_eq!(net.loads[0].bus, crate::network::BusId(10));
    assert_eq!(net.loads[1].bus, crate::network::BusId(20));
    assert_eq!((net.loads[0].p, net.loads[1].p), (12.0, 34.0));
}

#[test]
fn branch_identity_survives_aux_to_aux_through_the_typed_model() {
    let src = "DATA (Bus, [BusNum])\n{\n1\n2\n}\n\
         DATA (Branch, [BusNum, BusNum:1, LineCircuit, BranchDeviceType, LineStatus, LineR, LineX])\n\
         {\n1 2 \" 2\" \"Breaker\" \"Open\" 0 0.0001\n}\n";
    let net = parse_powerworld(src).unwrap();
    // Strip the retained source to force the canonical writer.
    let mut net = net;
    net.source = None;
    let out = super::write_powerworld(&net);
    let again = parse_powerworld(&out.text).unwrap();
    let br = &again.branches[0];
    assert_eq!(
        br.extras.get("LineCircuit").and_then(|v| v.as_str()),
        Some(" 2")
    );
    assert_eq!(
        br.extras.get("BranchDeviceType").and_then(|v| v.as_str()),
        Some("Breaker")
    );
    assert!(!br.in_service);
}

// ---- Loud errors ------------------------------------------------------------

fn read_err(text: &str) -> String {
    match parse_aux(text) {
        Err(Error::FormatRead { message, .. }) => message,
        other => panic!("expected FormatRead, got {other:?}"),
    }
}

#[test]
fn too_many_values_on_a_row_is_an_error() {
    let m = read_err("DATA (Bus, [BusNum])\n{\n1 2\n}\n");
    assert!(m.contains("2 values for 1"), "got: {m}");
}

#[test]
fn partial_row_at_closing_brace_is_an_error() {
    let m = read_err("DATA (Bus, [BusNum, BusName])\n{\n1\n}\n");
    assert!(m.contains("1 of 2"), "got: {m}");
}

#[test]
fn unterminated_body_is_an_error() {
    let m = read_err("DATA (Bus, [BusNum])\n{\n1\n");
    assert!(m.contains("unterminated"), "got: {m}");
}

#[test]
fn unterminated_subdata_is_an_error() {
    let m = read_err("DATA (C, [X])\n{\n1\n<SUBDATA Memo>\nnote\n}\n");
    assert!(m.contains("unterminated SUBDATA"), "got: {m}");
}

#[test]
fn subdata_before_any_row_is_an_error() {
    let m = read_err("DATA (C, [X])\n{\n<SUBDATA Memo>\nnote\n</SUBDATA>\n1\n}\n");
    assert!(m.contains("SUBDATA before any value row"), "got: {m}");
}

#[test]
fn unknown_header_argument_is_an_error() {
    let m = read_err("DATA (Bus, [BusNum], WHATEVER)\n{\n1\n}\n");
    assert!(m.contains("unknown DATA header argument"), "got: {m}");
}

#[test]
fn unterminated_header_is_an_error() {
    let m = read_err("DATA (Bus, [BusNum\n");
    assert!(m.contains("unterminated section header"), "got: {m}");
}
