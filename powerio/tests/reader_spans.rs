//! The MATPOWER and PSS/E readers attach the byte range of the record a
//! finding is about, into the retained source, for failures and warnings
//! alike.

use powerio::{PioModule, PioValue, Source};
use powerio_core::{Error, SourceSpan};

fn memory_source(name: &str, text: &str) -> Source {
    Source::from_memory(name, text.as_bytes().to_vec()).unwrap()
}

fn parse_failure(name: &str, text: &str, format: &str) -> Error {
    powerio::parse_with_options(
        memory_source(name, text),
        &powerio::ParseOptions::default().format(format).unwrap(),
    )
    .expect_err("the case is malformed")
}

/// The one span of the diagnostic that ended the operation.
fn failure_span(error: &Error) -> &SourceSpan {
    let diagnostic = &error.diagnostics()[0];
    assert_eq!(diagnostic.spans().len(), 1, "{diagnostic:?}");
    &diagnostic.spans()[0]
}

/// The retained bytes a span of a failed parse names.
fn failure_bytes(error: &Error, span: &SourceSpan) -> Vec<u8> {
    let buffer = error.retained_source().unwrap().primary_buffer().unwrap();
    assert_eq!(span.source(), buffer.id());
    buffer.bytes()[span.byte_start() as usize..span.byte_end() as usize].to_vec()
}

/// One based line and column of a byte offset in `text`.
fn line_and_column(text: &[u8], offset: usize) -> (usize, usize) {
    let before = &text[..offset];
    // Splitting on newlines yields one piece per line, so the piece count is
    // the one based line number.
    let line = before.split(|byte| *byte == b'\n').count();
    let column = before
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(offset, |newline| offset - newline - 1)
        + 1;
    (line, column)
}

const SHORT_ROW: &str = "\t2  1  0 0;";

fn matpower_with_a_short_bus_row() -> String {
    format!(
        "function mpc = short\nmpc.baseMVA = 100;\nmpc.bus = [\n\t1  3  0 0 0 0 1 1.0 0 345 1 1.1 0.9;\n{SHORT_ROW}\n];\nmpc.branch = [];\n"
    )
}

#[test]
fn a_short_matpower_row_names_its_bytes_line_and_column() {
    let text = matpower_with_a_short_bus_row();
    let error = parse_failure("short.m", &text, "matpower");
    assert_eq!(error.diagnostics()[0].code(), "PARSE.MATPOWER.MALFORMED");
    let span = failure_span(&error);
    assert_eq!(failure_bytes(&error, span), b"2  1  0 0");
    assert_eq!(
        line_and_column(text.as_bytes(), span.byte_start() as usize),
        (5, 2)
    );
    assert_eq!(
        line_and_column(text.as_bytes(), span.byte_end() as usize),
        (5, 11)
    );
}

#[test]
fn a_byte_order_mark_keeps_the_span_on_the_retained_bytes() {
    let text = format!("\u{feff}{}", matpower_with_a_short_bus_row());
    let error = parse_failure("short.m", &text, "matpower");
    let span = failure_span(&error);
    assert_eq!(failure_bytes(&error, span), b"2  1  0 0");
    // `text` starts with the three mark bytes the reader never sees, and so
    // does the retained buffer, so the span names the same offset.
    assert_eq!(span.byte_start() as usize, text.find("2  1  0 0").unwrap());
}

#[test]
fn a_malformed_matpower_token_names_its_row() {
    let text = "mpc.baseMVA = 100;\nmpc.bus = [\n\t1  3  0 0 0 0 1 1.0 0 345 1 1.1 0.9;\n\t2  1  0 x 0 0 1 1.0 0 345 1 1.1 0.9;\n];\nmpc.branch = [];\n";
    let error = parse_failure("token.m", text, "matpower");
    let span = failure_span(&error);
    assert_eq!(
        failure_bytes(&error, span),
        b"2  1  0 x 0 0 1 1.0 0 345 1 1.1 0.9;"
    );
}

#[test]
fn a_matpower_failure_without_a_record_carries_no_span() {
    let error = parse_failure("nobus.m", "mpc.baseMVA = 100;\n", "matpower");
    assert!(error.diagnostics()[0].spans().is_empty());
}

const RAW_HEADER: &str = "0, 100.00, 33, 0, 0, 60.00\nSPANS\nCOMMENT\n";

fn raw_after_buses(sections: &str) -> String {
    format!(
        "{RAW_HEADER}1, 'B1', 138.0, 3, 1, 1, 1, 1.0, 0.0\n2, 'B2', 0.0, 1, 1, 1, 1, 1.0, 0.0\n0 / END OF BUS DATA, BEGIN LOAD DATA\n0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA\n0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA\n0 / END OF GENERATOR DATA, BEGIN BRANCH DATA\n0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA\n{sections}0 / END OF TRANSFORMER DATA, BEGIN AREA DATA\n0 / END OF AREA DATA, BEGIN TWO-TERMINAL DC DATA\n0 / END OF TWO-TERMINAL DC DATA, BEGIN VSC DC LINE DATA\n0 / END OF VSC DC LINE DATA, BEGIN IMPEDANCE CORRECTION DATA\n0 / END OF IMPEDANCE CORRECTION DATA, BEGIN MULTI-TERMINAL DC DATA\n0 / END OF MULTI-TERMINAL DC DATA, BEGIN MULTI-SECTION LINE DATA\n0 / END OF MULTI-SECTION LINE DATA, BEGIN ZONE DATA\n0 / END OF ZONE DATA, BEGIN INTER-AREA TRANSFER DATA\n0 / END OF INTER-AREA TRANSFER DATA, BEGIN OWNER DATA\n0 / END OF OWNER DATA, BEGIN FACTS DEVICE DATA\n0 / END OF FACTS DEVICE DATA, BEGIN SWITCHED SHUNT DATA\n0 / END OF SWITCHED SHUNT DATA, BEGIN GNE DEVICE DATA\n0 / END OF GNE DEVICE DATA, BEGIN INDUCTION MACHINE DATA\n0 / END OF INDUCTION MACHINE DATA\nQ\n"
    )
}

const TRANSFORMER_LINE_1: &str = "1, 2, 0, '1', 2, 1, 1, 0.0, 0.0, 2, 'T1', 1, 1, 1.0";
const TRANSFORMER_LINE_2: &str = "0.01, 0.1, 100.0";
const TRANSFORMER_LINE_3: &str =
    "1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0, 0, 1.1, 0.9, 1.1, 0.9, 33, 0, 0.0, 0.0";
const TRANSFORMER_LINE_4: &str = "1.0, 0.0";

#[test]
fn a_malformed_psse_record_names_its_line() {
    let bad_bus = "abc, 'B1', 138.0, 3, 1, 1, 1, 1.0, 0.0";
    let text = format!("{RAW_HEADER}  {bad_bus}\n0 / END OF BUS DATA\nQ\n");
    let error = parse_failure("bad.raw", &text, "psse");
    let span = failure_span(&error);
    assert_eq!(failure_bytes(&error, span), bad_bus.as_bytes());
    assert_eq!(
        line_and_column(text.as_bytes(), span.byte_start() as usize),
        (4, 3)
    );
}

#[test]
fn a_psse_record_cut_short_by_a_terminator_names_the_lines_read() {
    let text = raw_after_buses(&format!("{TRANSFORMER_LINE_1}\n{TRANSFORMER_LINE_2}\n"));
    let error = parse_failure("cut.raw", &text, "psse");
    let span = failure_span(&error);
    assert_eq!(
        failure_bytes(&error, span),
        format!("{TRANSFORMER_LINE_1}\n{TRANSFORMER_LINE_2}").as_bytes()
    );
}

#[test]
fn a_psse_warning_covers_the_multi_line_record_it_came_from() {
    let record = format!(
        "{TRANSFORMER_LINE_1}\n{TRANSFORMER_LINE_2}\n{TRANSFORMER_LINE_3}\n{TRANSFORMER_LINE_4}"
    );
    let text = raw_after_buses(&format!("{record}\n"));
    let module: PioModule<PioValue> = powerio::parse_with_options(
        memory_source("warn.raw", &text),
        &powerio::ParseOptions::default().format("psse").unwrap(),
    )
    .unwrap();
    let substituted: Vec<_> = module
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code() == "READ.PSSE.VALUE_SUBSTITUTED")
        .collect();
    assert!(!substituted.is_empty(), "{:?}", module.diagnostics);
    let buffer = module.source().unwrap().primary_buffer().unwrap();
    for diagnostic in substituted {
        assert_eq!(diagnostic.spans().len(), 1, "{diagnostic:?}");
        let span = &diagnostic.spans()[0];
        assert_eq!(span.source(), module.sources()[0].id());
        assert_eq!(
            &buffer.bytes()[span.byte_start() as usize..span.byte_end() as usize],
            record.as_bytes()
        );
    }
    // Findings about the whole document carry no record span.
    assert!(
        module
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code() != "READ.PSSE.VALUE_SUBSTITUTED")
            .all(|diagnostic| diagnostic.spans().is_empty()),
        "{:?}",
        module.diagnostics
    );
}

#[test]
fn a_psse_header_failure_names_the_header_line() {
    let header = "0, abc, 33, 0, 0, 60.00";
    let text = format!("{header}\nSPANS\nCOMMENT\nQ\n");
    let error = parse_failure("header.raw", &text, "psse");
    let span = failure_span(&error);
    assert_eq!(failure_bytes(&error, span), header.as_bytes());
}
