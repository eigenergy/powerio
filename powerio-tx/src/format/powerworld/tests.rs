//! Unit tests for the generic aux grammar, one per construct of the format
//! guide.

use super::auxiliary::{AuxSection, emit_aux, parse_aux};
use crate::Error;

fn one_object(text: &str) -> super::AuxObject<'_> {
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
    assert_eq!(d.rows[1].subdata.as_slice(), []);
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
    // And it survives canonical emission.
    let out = emit_aux(&file);
    assert!(out.contains("SCRIPT MyActions"));
    assert!(out.contains("SolvePowerFlow;"));
}

#[test]
fn brace_on_header_line_is_accepted() {
    let d = one_object("Bus (BusNum) {\n1\n}\n");
    assert_eq!(d.rows.len(), 1);
}

#[test]
fn canonical_emission_is_idempotent() {
    let text = "DATA Named(Bus, [BusNum, BusName], AUXDEF, NO)\n{\n1 \"A B\"\n2 \"\"\n<SUBDATA Memo>\nnote line\n</SUBDATA>\n}\n\nSCRIPT\n{\nSolvePowerFlow;\n}\n";
    let first = emit_aux(&parse_aux(text).unwrap());
    let second = emit_aux(&parse_aux(&first).unwrap());
    assert_eq!(first, second, "canonical aux output must be idempotent");
}

// ---- BalancedNetwork mapping --------------------------------------------------------

use super::write_powerworld;

fn parse_powerworld(content: &str) -> crate::Result<crate::network::BalancedNetwork> {
    let mut warnings = crate::diagnostics::Diagnostics::new();
    super::map::parse_powerworld_source(content, None, &mut warnings)
}

#[test]
fn unmodeled_data_blocks_warn_on_parse() {
    let parsed = crate::parse_str(
        "DATA (Bus, [BusNum])\n{\n1\n}\n\
         DATA (Owner, [Name])\n{\n\"Utility\"\n}\n\
         DATA (Area, [Number, Name])\n{\n1 \"North\"\n}\n",
        "powerworld",
    )
    .unwrap();

    assert_eq!(parsed.network.buses().len(), 1);
    assert!(
        parsed
            .render_diagnostics()
            .iter()
            .any(|w| w.contains("DATA Owner") && w.contains("not modeled")),
        "missing Owner warning: {:?}",
        parsed.render_diagnostics()
    );
    assert!(
        parsed
            .render_diagnostics()
            .iter()
            .any(|w| w.contains("DATA Area") && w.contains("not modeled")),
        "missing Area warning: {:?}",
        parsed.render_diagnostics()
    );
}

#[test]
// Exact decimal fractions parsed from the fixture; bit equality is the assertion.
#[allow(clippy::float_cmp)]
fn bare_bus_latitude_and_longitude_promote_into_the_location() {
    // Older complete case exports write the pair on the bus row itself,
    // without the `:1` substation suffix.
    let net = parse_powerworld(
        "DATA (Bus, [BusNum, Latitude, Longitude])\n{\n1 34.2 -80.05\n2 34.3 \"\"\n}\n",
    )
    .unwrap();
    let location = net.buses()[0].location.expect("bus 1 location");
    assert_eq!((location.x, location.y), (-80.05, 34.2));
    assert!(!net.buses()[0].extras.contains_key("Latitude"));
    assert!(!net.buses()[0].extras.contains_key("Longitude"));
    // Half a pair promotes nothing, so the column stays in extras.
    assert!(net.buses()[1].location.is_none());
    assert_eq!(
        net.buses()[1]
            .extras
            .get("Latitude")
            .and_then(|v| v.as_str()),
        Some("34.3")
    );
}

#[test]
// Exact decimal fractions parsed from the fixture; bit equality is the assertion.
#[allow(clippy::float_cmp)]
fn a_bus_row_promotes_one_coordinate_pair_and_keeps_the_other() {
    // A complete case export carries both pairs. The substation pair wins,
    // and the bus's own pair stays in extras as unmodeled data.
    let net = parse_powerworld(
        "DATA (Bus, [BusNum, Latitude:1, Longitude:1, Latitude, Longitude])\n\
         {\n1 34.2 -80.05 30.1 -90.5\n2 \"\" -80.10 34.4 -80.20\n}\n",
    )
    .unwrap();
    let location = net.buses()[0].location.expect("bus 1 location");
    assert_eq!((location.x, location.y), (-80.05, 34.2));
    assert_eq!(
        net.buses()[0]
            .extras
            .get("Latitude")
            .and_then(|v| v.as_str()),
        Some("30.1")
    );
    assert!(!net.buses()[0].extras.contains_key("Latitude:1"));

    // Half of the substation pair promotes nothing from that pair, so the
    // bus's own pair promotes whole. Mixing the two would place the bus at
    // the substation's longitude.
    let location = net.buses()[1].location.expect("bus 2 location");
    assert_eq!((location.x, location.y), (-80.20, 34.4));
    assert_eq!(
        net.buses()[1]
            .extras
            .get("Longitude:1")
            .and_then(|v| v.as_str()),
        Some("-80.10")
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
    net.buses_mut()[0].name = Some("O\"Brien".to_string());
    let conv = write_powerworld(&net);
    let reparsed = parse_powerworld(&conv.text).unwrap();
    assert_eq!(reparsed.buses().len(), 2);
    assert_eq!(reparsed.buses()[0].base_kv, 230.0);
    assert_eq!(reparsed.buses()[1].base_kv, 138.0);
    assert!(!reparsed.buses()[0].name.as_deref().unwrap().contains('"'));
    assert!(
        conv.render_diagnostics()
            .iter()
            .any(|w| w.contains("bus name")),
        "expected a sanitization warning, got {:?}",
        conv.render_diagnostics()
    );
}

/// Simulator resolves object names case insensitively, and real exports use
/// that: the UIUC 150 bus aux spells every section `BUS`/`GEN`/`LOAD`. An
/// exact match read the file as a case with no buses.
#[test]
fn uppercase_object_sections_read_like_their_canonical_spelling() {
    let net = parse_powerworld(
        "DATA (BUS, [BusNum, BusNomVolt])\n{\n1 115\n2 115\n}\n\
         DATA (GEN, [BusNum, GenMW, GenStatus])\n{\n1 50 \"Closed\"\n}\n\
         DATA (LOAD, [BusNum, LoadMW, LoadMVR, LoadStatus])\n{\n2 40 10 \"Closed\"\n}\n\
         DATA (BRANCH, [BusNum, BusNum:1, LineCircuit, LineStatus, LineR, LineX])\n\
         {\n1 2 \" 1\" \"Closed\" 0.01 0.1\n}\n",
    )
    .unwrap();
    assert_eq!(net.buses().len(), 2);
    assert_eq!(net.generators().len(), 1);
    assert_eq!(net.loads().len(), 1);
    assert_eq!(net.branches().len(), 1);
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
    let l = &net.loads()[0];
    assert_eq!(l.p, 20.0, "S + I + Z MW at nominal voltage");
    assert_eq!(l.q, 6.0);
    assert_eq!(
        l.extras.get("LoadIMW").and_then(|v| v.as_str()),
        Some("4.0"),
        "voltage dependent components kept in extras"
    );
    // LoadID `1` is the writer's own positional fallback for the first load,
    // so it is not retained: a rewrite re-derives it.
    assert_eq!(l.extras.get("LoadID"), None);
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
    let l = &net.loads()[0];
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
    let kinds: Vec<BusType> = net.buses().iter().map(|b| b.kind).collect();
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
    let g = &net.generators()[0];
    assert_eq!((g.pg, g.qg, g.vg, g.mbase), (80.5, 12.25, 1.04, 120.0));
    let sh = &net.shunts()[0];
    assert_eq!((sh.g, sh.b), (0.0, 25.0));
    assert!(sh.in_service);
    let br = &net.branches()[0];
    assert_eq!((br.r, br.x, br.b), (0.001, 0.05, 0.002));
    assert_eq!((br.tap, br.shift), (0.9875, -2.5));
    assert_eq!((br.rate_a, br.rate_b, br.rate_c), (250.0, 260.0, 270.0));
    assert!(br.is_transformer());
    // A padded " 1" is the allocator's default with PowerWorld's padding on
    // it; both PowerWorld readers drop it (trim-insensitively) so the pwb and
    // aux reads of one case agree. An explicit non-default circuit is kept
    // verbatim, padding included.
    assert_eq!(br.extras.get("LineCircuit"), None);
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

    assert_eq!(net.buses()[0].id, crate::network::BusId(10));
    assert_eq!(net.loads()[0].bus, crate::network::BusId(20));
    assert_eq!(net.generators()[0].bus, crate::network::BusId(10));
    assert_eq!(net.shunts()[0].bus, crate::network::BusId(20));
    assert_eq!(net.branches()[0].from, crate::network::BusId(10));
    assert_eq!(net.branches()[0].to, crate::network::BusId(20));
    assert_eq!((net.branches()[0].r, net.branches()[0].x), (0.01, 0.05));
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

    assert_eq!(net.loads().len(), 2);
    assert_eq!(net.loads()[0].bus, crate::network::BusId(10));
    assert_eq!(net.loads()[1].bus, crate::network::BusId(20));
    assert_eq!((net.loads()[0].p, net.loads()[1].p), (12.0, 34.0));
}

#[test]
fn branch_identity_survives_aux_to_aux_through_the_typed_model() {
    let src = "DATA (Bus, [BusNum])\n{\n1\n2\n}\n\
         DATA (Branch, [BusNum, BusNum:1, LineCircuit, BranchDeviceType, LineStatus, LineR, LineX])\n\
         {\n1 2 \" 2\" \"Breaker\" \"Open\" 0 0.0001\n}\n";
    let net = parse_powerworld(src).unwrap();
    let out = super::write_powerworld(&net);
    let again = parse_powerworld(&out.text).unwrap();
    let br = &again.branches()[0];
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

// ---- Writing what Simulator loads --------------------------------------------

/// A case in the vocabulary of Simulator's own exports: a slack, a PV bus, an
/// unnamed PQ bus, and one of each core device, with a phase shifting
/// transformer.
const SIMULATOR_VOCABULARY_CASE: &str = "\
DATA (Bus, [BusNum, BusName, BusNomVolt, AreaNum, ZoneNum, BusSlack, BusVoltLimHigh:1, BusVoltLimLow:1])
{
1 \"One\" 230 1 1 \"YES\" 1.05 0.95
2 \"Two\" 230 1 2 \"NO\" 1.06 0.94
3 \"\" 115 2 2 \"NO\" 1.1 0.9
}
DATA (Load, [BusNum, LoadID, LoadStatus, LoadSMW, LoadSMVR])
{
3 \"1\" \"Closed\" 40 10
}
DATA (Shunt, [BusNum, ShuntID, SSStatus, SSCMode, SSNMW, SSNMVR])
{
3 \"1\" \"Closed\" \"Fixed\" 1.5 20
}
DATA (Gen, [BusNum, GenID, GenStatus, GenMW, GenMWMax, GenVoltSet])
{
1 \"1\" \"Closed\" 50 80 1.02
2 \"1\" \"Closed\" 10 30 1.01
}
DATA (Branch, [BusNum, BusNum:1, LineCircuit, BranchDeviceType, LineStatus, LineR, LineX, LineC, LineAMVA, LineAMVA:1, LineAMVA:2, LineTap, LinePhase])
{
1 2 \"1\" \"Line\" \"Closed\" 0.01 0.1 0.02 100 110 120 1 0
2 3 \"1\" \"Transformer\" \"Open\" 0.002 0.05 0 200 210 220 0.95 3
}
";

/// The declared fields of the `object` section of `text`.
fn written_fields(text: &str, object: &str) -> Vec<String> {
    parse_aux(text)
        .unwrap()
        .data()
        .find(|d| d.object_type == object)
        .unwrap_or_else(|| panic!("no {object} section in\n{text}"))
        .fields
        .clone()
}

/// The `field` value of each row of the `object` section of `text`.
fn written_column(text: &str, object: &str, field: &str) -> Vec<String> {
    let file = parse_aux(text).unwrap();
    let d = file.data().find(|d| d.object_type == object).unwrap();
    let at = d.field_index(field).unwrap();
    d.rows.iter().map(|r| r.values[at].to_string()).collect()
}

/// Simulator 24 skips a field it does not define and refuses to create an
/// object whose required fields are missing. Loading the 0.11.12 writer's
/// case14 created the buses and branches but no load, shunt, or generator,
/// and dropped every tap, bus voltage limit, and the slack. These are the
/// names Simulator's `GetFieldList` defines and its own exports write.
#[test]
fn writer_states_the_fields_simulator_defines_and_requires() {
    let net = parse_powerworld(SIMULATOR_VOCABULARY_CASE).unwrap();
    let text = write_powerworld(&net).text;
    assert_eq!(written_fields(&text, "Sim_Solution_Options"), ["SBase"]);
    assert_eq!(
        written_fields(&text, "Bus"),
        [
            "BusNum",
            "BusName",
            "BusNomVolt",
            "BusPUVolt",
            "BusAngle",
            "AreaNum",
            "ZoneNum",
            "BusSlack",
            "BusVoltLim",
            "BusVoltLimHigh:1",
            "BusVoltLimLow:1",
            "BusCat",
        ]
    );
    assert_eq!(
        written_fields(&text, "Load"),
        ["BusNum", "LoadID", "LoadStatus", "LoadSMW", "LoadSMVR"]
    );
    assert_eq!(
        written_fields(&text, "Shunt"),
        [
            "BusNum", "ShuntID", "SSStatus", "SSCMode", "SSNMW", "SSNMVR"
        ]
    );
    assert_eq!(
        written_fields(&text, "Gen"),
        [
            "BusNum",
            "GenID",
            "GenStatus",
            "GenAGCAble",
            "GenAVRAble",
            "GenMW",
            "GenMVR",
            "GenMWMax",
            "GenMWMin",
            "GenMVRMax",
            "GenMVRMin",
            "GenVoltSet",
            "GenMVABase",
        ]
    );
    // LineTap is enterable only on a transformer, and Simulator reads a row's
    // fields in order, so the device type comes first.
    assert_eq!(
        written_fields(&text, "Branch"),
        [
            "BusNum",
            "BusNum:1",
            "LineCircuit",
            "BranchDeviceType",
            "LineStatus",
            "LineR",
            "LineX",
            "LineC",
            "LineAMVA",
            "LineAMVA:1",
            "LineAMVA:2",
            "LineTap",
            "LinePhase",
        ]
    );
    assert_eq!(
        written_column(&text, "Bus", "BusSlack"),
        ["YES", "NO", "NO"]
    );
    assert_eq!(written_column(&text, "Bus", "BusVoltLim"), ["YES"; 3]);
    assert_eq!(written_column(&text, "Shunt", "SSCMode"), ["Fixed"]);
}

/// The new vocabulary reads back to the same typed network.
#[test]
// Exact decimal fractions written and parsed back; bit equality is the assertion.
#[allow(clippy::float_cmp)]
fn simulator_vocabulary_round_trips_through_the_writer() {
    let net = parse_powerworld(SIMULATOR_VOCABULARY_CASE).unwrap();
    let again = parse_powerworld(&write_powerworld(&net).text).unwrap();
    assert_eq!(again.buses(), net.buses());
    assert_eq!(again.loads(), net.loads());
    assert_eq!(again.shunts(), net.shunts());
    assert_eq!(again.generators(), net.generators());
    assert_eq!(again.branches(), net.branches());
    let xf = &again.branches()[1];
    assert_eq!((xf.tap, xf.shift, xf.rate_c), (0.95, 3.0, 220.0));
    assert!(!xf.in_service);
    assert_eq!(again.shunts()[0].g, 1.5);
    // Fixed is the mode the writer states for every shunt, so it is no extra;
    // any other mode is kept for the writer to report.
    assert!(!net.shunts()[0].extras.contains_key("SSCMode"));
    let discrete =
        parse_powerworld(&SIMULATOR_VOCABULARY_CASE.replace("\"Fixed\"", "\"Discrete\"")).unwrap();
    assert_eq!(
        discrete.shunts()[0]
            .extras
            .get("SSCMode")
            .and_then(|v| v.as_str()),
        Some("Discrete")
    );
}

/// Simulator refuses to create a bus whose BusName is blank, so an unnamed
/// bus is written under its number, and a name equal to the bus number reads
/// back as no name.
#[test]
fn an_unnamed_bus_is_written_under_its_number_and_reads_back_unnamed() {
    let net = parse_powerworld(SIMULATOR_VOCABULARY_CASE).unwrap();
    assert_eq!(net.buses()[2].name, None);
    let text = write_powerworld(&net).text;
    assert_eq!(written_column(&text, "Bus", "BusName"), ["One", "Two", "3"]);
    let again = parse_powerworld(&text).unwrap();
    assert_eq!(again.buses()[2].name, None);
    assert_eq!(again.buses()[0].name.as_deref(), Some("One"));
}

/// AVR is what makes a bus PV in Simulator. A machine on a PQ bus holds its
/// Mvar output, as in a MATPOWER power flow, and so does one whose
/// regulation is off.
#[test]
fn generator_avr_follows_the_bus_type_and_the_regulation_flag() {
    let mut net = parse_powerworld(SIMULATOR_VOCABULARY_CASE).unwrap();
    let text = write_powerworld(&net).text;
    assert_eq!(written_column(&text, "Gen", "GenAVRAble"), ["YES", "YES"]);
    assert_eq!(written_column(&text, "Gen", "GenAGCAble"), ["YES", "YES"]);

    net.buses_mut()[1].kind = crate::network::BusType::Pq;
    let text = write_powerworld(&net).text;
    assert_eq!(written_column(&text, "Gen", "GenAVRAble"), ["YES", "NO"]);

    net.buses_mut()[1].kind = crate::network::BusType::Pv;
    net.generators_mut()[0].voltage_regulation_on = false;
    let text = write_powerworld(&net).text;
    assert_eq!(written_column(&text, "Gen", "GenAVRAble"), ["NO", "YES"]);
}

/// Simulator identifies a branch by its two buses in either order plus its
/// circuit. pegase's 5289 to 3113 and 3113 to 5289 lines, both written as
/// circuit 1, loaded as one branch, and Simulator logged nothing. A reversed
/// parallel gets its own circuit, and the positional circuits still read back
/// as defaults.
#[test]
// Exact decimal fractions written and parsed back; bit equality is the assertion.
#[allow(clippy::float_cmp)]
fn a_reversed_parallel_branch_gets_its_own_circuit() {
    let src = "DATA (Bus, [BusNum])\n{\n1\n2\n3\n}\n\
         DATA (Branch, [BusNum, BusNum:1, LineCircuit, LineR, LineX])\n\
         {\n2 1 \"1\" 0.01 0.1\n1 2 \"2\" 0.02 0.2\n2 3 \"1\" 0.03 0.3\n}\n";
    let net = parse_powerworld(src).unwrap();
    assert!(
        net.branches()
            .iter()
            .all(|b| !b.extras.contains_key("LineCircuit")),
        "positional circuits are defaults"
    );
    let text = write_powerworld(&net).text;
    assert_eq!(
        written_column(&text, "Branch", "LineCircuit"),
        ["1", "2", "1"]
    );
    let again = parse_powerworld(&text).unwrap();
    assert_eq!(again.branches(), net.branches());
    assert_eq!(again.branches()[1].r, 0.02);
}

/// Simulator's load, shunt, and generator ids are two characters, and it
/// truncates a longer one: numbered over the whole table, the 100th load read
/// as "10" and merged with another load at its bus (ACTIVSg200 from PSS/E lost
/// 21 of 160 loads). The writer numbers devices per bus, and an id equal to
/// its per bus position reads back as the default.
#[test]
fn device_ids_count_per_bus() {
    use std::fmt::Write as _;

    let mut src = String::from("DATA (Bus, [BusNum])\n{\n1\n2\n}\n");
    src.push_str("DATA (Load, [BusNum, LoadID, LoadSMW])\n{\n");
    for i in 0..120 {
        // Load ids 1 to 60 at each bus, interleaved.
        writeln!(src, "{} \"{}\" {i}", 1 + i % 2, i / 2 + 1).unwrap();
    }
    src.push_str(
        "}\nDATA (Gen, [BusNum, GenID, GenMW])\n{\n1 \"1\" 10\n2 \"1\" 20\n1 \"2\" 30\n}\n",
    );
    let net = parse_powerworld(&src).unwrap();
    assert_eq!(net.loads().len(), 120);
    assert!(
        net.loads().iter().all(|l| !l.extras.contains_key("LoadID")),
        "per bus positions are defaults"
    );
    let text = write_powerworld(&net).text;
    let ids = written_column(&text, "Load", "LoadID");
    assert!(ids.iter().all(|id| id.len() <= 2), "{ids:?}");
    assert_eq!(ids[118..], ["60", "60"]);
    assert_eq!(written_column(&text, "Gen", "GenID"), ["1", "1", "2"]);
    let again = parse_powerworld(&text).unwrap();
    assert_eq!(again.loads(), net.loads());
}

/// A bus without a positive nominal voltage, or in area or zone 0, loads in
/// Simulator with a substituted value, and the writer says so.
#[test]
fn values_simulator_substitutes_are_reported() {
    let net = parse_powerworld(
        "DATA (Bus, [BusNum, BusNomVolt, AreaNum, ZoneNum])\n{\n1 0 1 1\n2 230 0 1\n3 230 1 1\n}\n",
    )
    .unwrap();
    let diagnostics = write_powerworld(&net).render_diagnostics();
    assert!(
        diagnostics
            .iter()
            .any(|d| d.contains("1 bus(es) state no positive nominal voltage")),
        "{diagnostics:?}"
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.contains("1 bus(es) in area or zone 0")),
        "{diagnostics:?}"
    );
}

/// Branch impedances are per unit on the system base, so a case on a base
/// other than 100 MVA states it where Simulator reads it.
#[test]
// Exact decimal fractions written and parsed back; bit equality is the assertion.
#[allow(clippy::float_cmp)]
fn the_system_base_is_stated_as_sbase_and_read_back() {
    let mut net = parse_powerworld(SIMULATOR_VOCABULARY_CASE).unwrap();
    assert_eq!(net.base_mva(), 100.0);
    *net.base_mva_mut() = 10.0;
    let text = write_powerworld(&net).text;
    assert_eq!(
        written_column(&text, "Sim_Solution_Options", "SBase"),
        ["10"]
    );
    assert_eq!(parse_powerworld(&text).unwrap().base_mva(), 10.0);

    // The block alone states the base; the comment marker is not needed.
    let block_only = "DATA (Sim_Solution_Options, [SBase])\n{\n  25\n}\n\
                      DATA (Bus, [BusNum])\n{\n1\n}\n";
    let mut warnings = crate::diagnostics::Diagnostics::new();
    let read = super::map::parse_powerworld_source(block_only, None, &mut warnings).unwrap();
    assert_eq!(read.base_mva(), 25.0);
    assert_eq!(
        warnings,
        crate::diagnostics::Diagnostics::new(),
        "a block stating only SBase is modeled"
    );
}

/// Simulator regulates buses joined by a branch at or below its zero
/// impedance threshold (0.0002 pu by default) as one group and overrides all
/// but one set point: pegase's 1043 and 3513, joined by x = 0.0002, solved
/// 5.6e-5 pu away from MATPOWER. Such a case states the threshold at
/// Simulator's floor, which reads back without a warning; a branch at or
/// below the floor is reported.
#[test]
fn a_near_zero_impedance_branch_lowers_the_zbr_threshold() {
    let case = |x: &str| {
        format!(
            "DATA (Bus, [BusNum])\n{{\n1\n2\n3\n}}\n\
             DATA (Branch, [BusNum, BusNum:1, LineCircuit, LineR, LineX])\n\
             {{\n1 2 \"1\" 0.001 0.01\n2 3 \"1\" 0.00001 {x}\n}}\n"
        )
    };
    let plain = write_powerworld(&parse_powerworld(&case("0.01")).unwrap());
    assert_eq!(
        written_fields(&plain.text, "Sim_Solution_Options"),
        ["SBase"]
    );

    let small = write_powerworld(&parse_powerworld(&case("0.0002")).unwrap());
    assert_eq!(
        written_fields(&small.text, "Sim_Solution_Options"),
        ["SBase", "ZBRThreshold"]
    );
    assert_eq!(
        written_column(&small.text, "Sim_Solution_Options", "ZBRThreshold"),
        ["0.00005"]
    );
    assert!(
        !small
            .render_diagnostics()
            .iter()
            .any(|d| d.contains("zero impedance"))
    );
    let mut warnings = crate::diagnostics::Diagnostics::new();
    super::map::parse_powerworld_source(&small.text, None, &mut warnings).unwrap();
    assert_eq!(warnings, crate::diagnostics::Diagnostics::new());

    let tiny = write_powerworld(&parse_powerworld(&case("0.00001")).unwrap());
    assert!(
        tiny.render_diagnostics()
            .iter()
            .any(|d| d
                .contains("1 branch(es) at or below Simulator's lowest zero impedance threshold")),
        "{:?}",
        tiny.render_diagnostics()
    );

    // A third party threshold is a Simulator setting the model does not hold.
    let mut warnings = crate::diagnostics::Diagnostics::new();
    super::map::parse_powerworld_source(
        "DATA (Sim_Solution_Options, [SBase, ZBRThreshold])\n{\n100 0.001\n}\n\
         DATA (Bus, [BusNum])\n{\n1\n}\n",
        None,
        &mut warnings,
    )
    .unwrap();
    assert_ne!(warnings, crate::diagnostics::Diagnostics::new());
}

#[test]
fn a_nonpositive_or_non_numeric_sbase_is_a_read_error() {
    for bad in ["0", "-100", "abc", "inf"] {
        let text = format!(
            "DATA (Sim_Solution_Options, [SBase])\n{{\n  \"{bad}\"\n}}\n\
             DATA (Bus, [BusNum])\n{{\n1\n}}\n"
        );
        match parse_powerworld(&text) {
            Err(Error::FormatRead { message, .. }) => {
                assert!(message.contains("SBase"), "{message}");
            }
            other => panic!("SBase {bad:?} should be refused, got {other:?}"),
        }
    }
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

/// The device type is dropped only when the writer derives the same value
/// back. It derives from `tap != 0 || shift != 0`, so a record stating `Line`
/// while stating a phase shift must keep the extra: dropping it rewrites the
/// branch as a `Transformer`, and the typed model is identical either way, so
/// nothing downstream would notice.
#[test]
fn a_line_that_states_a_phase_shift_stays_a_line() {
    let src = "DATA (Bus, [BusNum])\n{\n1\n2\n}\n\
         DATA (Branch, [BusNum, BusNum:1, LineCircuit, BranchDeviceType, LineStatus, LineR, LineX, LinePhase])\n\
         {\n1 2 \" 1\" \"Line\" \"Closed\" 0.01 0.1 3.5\n}\n";
    let net = parse_powerworld(src).unwrap();
    assert_eq!(
        net.branches()[0]
            .extras
            .get("BranchDeviceType")
            .and_then(|v| v.as_str()),
        Some("Line"),
        "a Line with a phase shift is not what the writer would derive"
    );
    let out = super::write_powerworld(&net);
    let row = out
        .text
        .lines()
        .find(|l| l.trim_start().starts_with("1 2 "))
        .unwrap();
    assert!(
        row.contains("\"Line\""),
        "the branch must rewrite as a Line: {row}"
    );

    // A plain line states what the writer derives, so the extra is dropped and
    // the round trip still lands on `Line`.
    let plain = src.replace(" 3.5\n", " 0\n");
    let net = parse_powerworld(&plain).unwrap();
    assert!(!net.branches()[0].extras.contains_key("BranchDeviceType"));
    let out = super::write_powerworld(&net);
    let row = out
        .text
        .lines()
        .find(|l| l.trim_start().starts_with("1 2 "))
        .unwrap();
    assert!(row.contains("\"Line\""), "{row}");
}
