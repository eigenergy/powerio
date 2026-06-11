//! Unit tests for the generic aux grammar, one per construct of the format
//! guide.

use super::aux::{AuxSection, parse_aux, write_aux};
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
