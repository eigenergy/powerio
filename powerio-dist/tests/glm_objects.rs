use powerio_core::Source;
use powerio_dist::glm::{GlmPropertyValue, PreprocessOptions, parse_project};

fn memory(text: &str) -> Source {
    Source::from_memory("case.glm", text.as_bytes()).unwrap()
}

#[test]
fn nested_parent_and_inline_equipment_links_have_different_semantics() {
    let project = parse_project(
        memory(
            r#"
        object node:17 { name source; }
        object load:901 {
            name "sink";
            object recorder { name nested_recorder; };
        }
        object overhead_line {
            name line;
            from node:17; to id:901;
            configuration object line_configuration { name cfg; z11 1+0.5j; };
        }
    "#,
        ),
        PreprocessOptions::default(),
    )
    .unwrap();
    let index = project.index_objects().unwrap();
    let source = index.resolve_reference("source").unwrap().unwrap();
    let sink = index.resolve_reference("sink").unwrap().unwrap();
    let line = index.resolve_reference("line").unwrap().unwrap();
    let recorder = index.resolve_reference("nested_recorder").unwrap().unwrap();
    let cfg = index.resolve_reference("cfg").unwrap().unwrap();
    assert_eq!(index.parent(recorder), Some(sink));
    assert_eq!(index.parent(cfg), None);
    assert_eq!(
        index.resolve_property(line, "configuration").unwrap(),
        Some(cfg)
    );
    assert_eq!(index.resolve_property(line, "from").unwrap(), Some(source));
    assert_eq!(index.resolve_property(line, "to").unwrap(), Some(sink));
    assert_eq!(index.get(source).unwrap().explicit_id(), Some(17));
    assert_ne!(source.ordinal(), 17);
    assert!(index.resolve_reference("node:0").is_err());
    assert!(index.resolve_reference("load:17").is_err());
}

#[test]
fn explicit_parent_overrides_nesting_and_forward_names_resolve() {
    let project = parse_project(
        memory(
            r"
        object node { name outer;
            object load { name child; parent later; };
            object load { name unparented; parent root; };
        }
        object node { name later; }
    ",
        ),
        PreprocessOptions::default(),
    )
    .unwrap();
    let index = project.index_objects().unwrap();
    let later = index.resolve_reference("later").unwrap();
    let child = index.resolve_reference("child").unwrap().unwrap();
    let unparented = index.resolve_reference("unparented").unwrap().unwrap();
    assert_eq!(index.parent(child), later);
    assert_eq!(index.parent(unparented), None);
}

#[test]
fn ranges_and_anonymous_counts_keep_explicit_ids_separate() {
    let project = parse_project(
        memory("object node:3..5 {} object load:..2 {}"),
        PreprocessOptions::default(),
    )
    .unwrap();
    let index = project.index_objects().unwrap();
    assert_eq!(index.objects().len(), 5);
    for (id, object) in (3..=5).zip(index.objects()) {
        assert_eq!(object.explicit_id(), Some(id));
        assert_eq!(
            index.resolve_reference(&format!("id:{id}")).unwrap(),
            Some(object.key())
        );
    }
    assert_eq!(index.objects()[3].explicit_id(), None);
    assert!(index.resolve_reference("load:4").is_err());
    assert!(index.resolve_reference("load:id+1").is_err());
}

#[test]
fn duplicate_names_global_ids_cycles_and_expansion_limits_fail() {
    for text in [
        "object node { name a; } object load { name a; }",
        "object node:7 {} object load:7 {}",
        "object node { name a; parent b; } object node { name b; parent a; }",
        "object node { name a; parent a; }",
        "object node { name a; parent missing; }",
        "object node { parent; }",
        "object node { name; }",
        "object node:..1048577 {}",
        "object node:8..2 {}",
        "object node:4294967296 {}",
        "namespace feeder { object node {} }",
    ] {
        let project = parse_project(memory(text), PreprocessOptions::default()).unwrap();
        let failure = project.index_objects().unwrap_err();
        assert_eq!(
            failure.diagnostics()[0].code(),
            "PARSE.GLM.OBJECT",
            "{text}"
        );
        assert!(failure.retained_source().is_some());
    }
}

#[test]
fn repeated_scalar_properties_and_uninterpreted_syntax_remain_visible() {
    let project = parse_project(
        memory("object custom { name a; value 1; value 2; arbitrary block { x 1; } }"),
        PreprocessOptions::default(),
    )
    .unwrap();
    let index = project.index_objects().unwrap();
    let object = &index.objects()[0];
    assert_eq!(object.class_name(), "custom");
    assert_eq!(
        object
            .properties()
            .iter()
            .filter(|p| p.name == "value")
            .count(),
        2
    );
    let GlmPropertyValue::Tokens(tokens) = &object.properties()[2].value else {
        panic!()
    };
    assert_eq!(tokens[0].text(), "2");
    assert_eq!(object.other_statements().len(), 1);
}

#[test]
fn reference_failures_point_into_included_original_buffers() {
    let text = "object overhead_line { name line; from missing; }";
    let source = memory("#include \"line.glm\"\n")
        .with_named_buffer("line.glm", text.as_bytes())
        .unwrap();
    let project = parse_project(source, PreprocessOptions::default()).unwrap();
    let index = project.index_objects().unwrap();
    let failure = index
        .resolve_property(index.objects()[0].key(), "from")
        .unwrap_err();
    let span = &failure.diagnostics()[0].spans()[0];
    assert_eq!(span.source().as_str(), "line.glm");
    assert_eq!(
        &text[span.byte_start() as usize..span.byte_end() as usize],
        "from missing;"
    );
    assert_eq!(project.dependencies().len(), 2);
}

#[test]
fn syntax_errors_from_substitutions_point_to_the_original_reference() {
    let text = "#define END=}\nobject node {} ${END}\n";
    let failure = parse_project(memory(text), PreprocessOptions::default()).unwrap_err();
    assert_eq!(failure.diagnostics()[0].code(), "PARSE.GLM.SYNTAX");
    let span = &failure.diagnostics()[0].spans()[0];
    assert_eq!(
        &text[span.byte_start() as usize..span.byte_end() as usize],
        "${END}"
    );
    assert_eq!(
        failure
            .retained_source()
            .unwrap()
            .primary_buffer()
            .unwrap()
            .bytes(),
        text.as_bytes()
    );
}
