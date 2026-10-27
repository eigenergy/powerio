use powerio_core::Source;
use powerio_dist::glm::{Statement, Token, TokenKind, parse_raw_source};

fn memory(text: &str) -> Source {
    Source::from_memory("case.glm", text.as_bytes()).unwrap()
}

#[test]
fn source_bytes_and_unicode_spans_survive_syntax_parsing() {
    let text = "\u{feff}// untouched\r\nobject load { name \"café; { feeder }\"; voltage_A 240-120d V; }\r\n";
    let raw = parse_raw_source(memory(text)).unwrap();
    assert_eq!(
        raw.source.primary_buffer().unwrap().bytes(),
        text.as_bytes()
    );
    let Statement::Block { header, body, span } = &raw.statements[0] else {
        panic!()
    };
    assert_eq!(header[0].text(), "object");
    assert_eq!(
        &text[span.byte_start() as usize..span.byte_end() as usize],
        "object load { name \"café; { feeder }\"; voltage_A 240-120d V; }"
    );
    let Statement::Simple { tokens, .. } = &body[0] else {
        panic!()
    };
    assert_eq!(tokens[1].kind(), TokenKind::Quoted);
    assert_eq!(tokens[1].text(), "\"café; { feeder }\"");
    let position = tokens[1].span();
    assert_eq!(
        &text[position.byte_start() as usize..position.byte_end() as usize],
        tokens[1].text()
    );
}

#[test]
fn nested_and_inline_objects_remain_ordered_with_duplicate_properties() {
    let raw = parse_raw_source(memory(
        r"
        module powerflow;
        object load:17 {
            name first; name last;
            object recorder {};
            configuration object line_configuration { z11 1e-3+2e-4j Ohm/mile; };
        }
    ",
    ))
    .unwrap();
    let Statement::Block { header, body, .. } = &raw.statements[1] else {
        panic!()
    };
    assert_eq!(header[1].text(), "load:17");
    assert_eq!(body.len(), 4);
    let Statement::Simple { tokens, .. } = &body[0] else {
        panic!()
    };
    assert_eq!(tokens[1].text(), "first");
    let Statement::Simple { tokens, .. } = &body[1] else {
        panic!()
    };
    assert_eq!(tokens[1].text(), "last");
    let Statement::Block { body: nested, .. } = &body[2] else {
        panic!()
    };
    assert!(nested.is_empty());
    let Statement::Block { header, .. } = &body[3] else {
        panic!()
    };
    assert_eq!(
        header.iter().map(Token::text).collect::<Vec<_>>(),
        ["configuration", "object", "line_configuration"]
    );
}

#[test]
fn directive_lines_and_variables_are_retained_without_execution() {
    let source = "#define VN=240\n#include \"missing.glm\";\n#system touch /must-not-be-created\nobject load { nominal_voltage ${VN}; name \"${LABEL}\"; }";
    let raw = parse_raw_source(memory(source)).unwrap();
    assert_eq!(raw.statements.len(), 4);
    assert_eq!(raw.source.acquired_buffers().len(), 1);
    let Statement::Directive(token) = &raw.statements[1] else {
        panic!()
    };
    assert_eq!(token.text(), "#include \"missing.glm\";");
    let Statement::Block { body, .. } = &raw.statements[3] else {
        panic!()
    };
    let Statement::Simple { tokens, .. } = &body[0] else {
        panic!()
    };
    assert_eq!(tokens[1].text(), "${VN}");
}

#[test]
fn comments_do_not_terminate_quoted_values_or_create_objects() {
    let raw = parse_raw_source(memory(
        r#"
        /* object bogus { } */
        object node { name "https://a/*b*/\"quoted\""; } // object ignored {}
    "#,
    ))
    .unwrap();
    assert_eq!(raw.statements.len(), 1);
    let Statement::Block { body, .. } = &raw.statements[0] else {
        panic!()
    };
    assert_eq!(body.len(), 1);
}

#[test]
fn malformed_input_returns_registered_span_and_source() {
    for text in [
        "object node {",
        "object node { name missing }",
        "object node { name \"open; }",
        "/* open",
        "}",
        "module powerflow",
        "object load { x ${missing; }",
        "object node { name a\0b; }",
    ] {
        let error = parse_raw_source(memory(text)).unwrap_err();
        assert_eq!(error.info().unwrap().code, "PARSE.GLM.SYNTAX", "{text}");
        assert!(error.retained_source().is_some());
        assert_ne!(error.diagnostics()[0].spans(), []);
    }
    let source = Source::from_memory("bad.glm", vec![0xff, b';']).unwrap();
    assert!(parse_raw_source(source).is_err());
}

#[test]
fn nesting_and_token_size_are_bounded_before_unbounded_work() {
    let text = format!("{}{}", "object node {".repeat(65), "}".repeat(65));
    let error = parse_raw_source(memory(&text)).unwrap_err();
    assert_eq!(error.info().unwrap().code, "PARSE.GLM.LIMIT_EXCEEDED");
    let text = format!("object node {{ name {}; }}", "x".repeat((1 << 20) + 1));
    let error = parse_raw_source(memory(&text)).unwrap_err();
    assert_eq!(error.info().unwrap().code, "PARSE.GLM.LIMIT_EXCEEDED");
}

#[test]
fn valid_boundary_nesting_and_empty_blocks_parse() {
    let text = format!("{}{}", "object node {".repeat(64), "};".repeat(64));
    assert!(parse_raw_source(memory(&text)).is_ok());
    assert!(parse_raw_source(memory("clock {}; module tape {}; ;")).is_ok());
}
