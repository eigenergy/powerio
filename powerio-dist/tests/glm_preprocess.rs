use std::collections::BTreeMap;

use powerio_core::Source;
use powerio_dist::glm::{GlmDialect, PreprocessOptions, parse_raw_source, preprocess_source};

fn memory(text: &str) -> Source {
    Source::from_memory("case.glm", text.as_bytes()).unwrap()
}

#[test]
fn included_declarations_use_primary_directory_and_keep_exact_project_bytes() {
    let main = "\u{feff}#define LABEL=café\r\n#define VN=240\r\n#include \"parts/properties.glm\"\r\n#include \"parts/load.glm\"\r\n";
    let properties = "object node { name \"${LABEL}\";\nnominal_voltage ${VN}; }\n";
    let load = "#include \"shared.glm\"\nobject load { nominal_voltage ${VN}; }";
    let source = memory(main)
        .with_named_buffer("parts/properties.glm", properties.as_bytes())
        .unwrap()
        .with_named_buffer("parts/load.glm", load.as_bytes())
        .unwrap()
        .with_named_buffer("shared.glm", &b"#define VN=120\n"[..])
        .unwrap()
        .with_named_buffer("parts/shared.glm", &b"#define VN=999\n"[..])
        .unwrap()
        .with_named_buffer("unused.glm", &b"#system must-not-execute\n"[..])
        .unwrap();
    let prepared = preprocess_source(source, PreprocessOptions::default()).unwrap();
    assert!(
        prepared
            .text()
            .contains("name \"café\";\nnominal_voltage 240;")
    );
    assert!(
        prepared
            .text()
            .contains("object load { nominal_voltage 120; }")
    );
    assert_eq!(prepared.dependencies().len(), 4);
    assert!(
        !prepared
            .dependencies()
            .iter()
            .any(|id| id.as_str() == "unused.glm")
    );
    assert_eq!(
        prepared.source.primary_buffer().unwrap().bytes(),
        main.as_bytes()
    );
    let part = prepared
        .source
        .referenced_buffer(
            &prepared.source.primary_buffer().unwrap(),
            "parts/properties.glm",
        )
        .unwrap();
    assert_eq!(part.bytes(), properties.as_bytes());
    let label = prepared.text().find("café").unwrap();
    let spans = prepared.original_spans(label..label + "café".len());
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].source().as_str(), "parts/properties.glm");
    assert_eq!(
        &properties[spans[0].byte_start() as usize..spans[0].byte_end() as usize],
        "${LABEL}"
    );
    let nominal = prepared.text().find("nominal_voltage").unwrap();
    let spans = prepared.original_spans(nominal..nominal + 7);
    assert_eq!(
        &properties[spans[0].byte_start() as usize..spans[0].byte_end() as usize],
        "nominal"
    );
    let syntax = parse_raw_source(memory(prepared.text())).unwrap();
    assert_eq!(syntax.statements.len(), 2);
}

#[test]
fn includes_do_not_silently_create_nested_objects_from_native_global_declarations() {
    let source = memory("object node {\n#include \"child.glm\"\n}")
        .with_named_buffer("child.glm", &b"object load {}"[..])
        .unwrap();
    let failure = preprocess_source(source, PreprocessOptions::default()).unwrap_err();
    assert_eq!(failure.diagnostics()[0].code(), "PARSE.GLM.PREPROCESSING");
    let source = memory("#include \"partial.glm\"\n}")
        .with_named_buffer("partial.glm", &b"object load {"[..])
        .unwrap();
    assert!(preprocess_source(source, PreprocessOptions::default()).is_err());
}

#[test]
fn comments_and_quoted_braces_do_not_change_include_boundaries() {
    let source = memory("// ${UNDEFINED} ignored\nobject node { name \"{literal}\"; }; /* { */\n#include \"other.glm\"\n")
        .with_named_buffer("other.glm", &b"object load { name second; }\n"[..]).unwrap();
    let prepared = preprocess_source(source, PreprocessOptions::default()).unwrap();
    assert_eq!(
        parse_raw_source(memory(prepared.text()))
            .unwrap()
            .statements
            .len(),
        2
    );
}

#[test]
fn conditionals_precede_syntax_and_compare_strings_like_gridlabd() {
    let text = "#define X=100\n#if X<20\nobject node { name selected; }\n#else\ninvalid { syntax \"\n#endif\n#ifndef X\n#include \"missing.glm\"\n#ifdef UNKNOWN\n#system ignored\n#endif\n#else\n#ifdef EMPTY\nobject load { name empty_is_defined; }\n#endif\n#endif\n";
    let options = PreprocessOptions {
        defines: BTreeMap::from([("EMPTY".into(), String::new())]),
        ..Default::default()
    };
    let prepared = preprocess_source(memory(text), options).unwrap();
    assert_eq!(prepared.dependencies().len(), 1);
    assert_eq!(
        parse_raw_source(memory(prepared.text()))
            .unwrap()
            .statements
            .len(),
        2
    );
    assert!(!prepared.text().contains("invalid"));
    assert!(prepared.text().contains("selected"));
}

#[test]
fn unknown_variables_and_active_execution_are_explicit_failures() {
    for text in [
        "name ${HOME};",
        "#system echo unexpected\n",
        "#include [https://example.com/a]\n",
        "#include <header.h>\n",
        "#ifexist outside.glm\n#endif\n",
        "#define X value\n",
        "#define EXEC=#system bad\n${EXEC}\n",
    ] {
        let failure = preprocess_source(memory(text), PreprocessOptions::default()).unwrap_err();
        assert_eq!(
            failure.diagnostics()[0].code(),
            "PARSE.GLM.PREPROCESSING",
            "{text}"
        );
        assert!(failure.retained_source().is_some());
        assert_ne!(failure.diagnostics()[0].spans(), []);
    }
}

#[test]
fn include_cycles_duplicates_and_escape_paths_fail_without_external_reads() {
    for path in ["../outside.glm", "/outside.glm", "https://example.com/a"] {
        assert!(
            preprocess_source(
                memory(&format!("#include \"{path}\"\n")),
                PreprocessOptions::default()
            )
            .is_err()
        );
    }
    let source = memory("#include \"a.glm\"\n")
        .with_named_buffer("a.glm", &b"#include \"a.glm\"\n"[..])
        .unwrap();
    assert!(
        preprocess_source(
            source,
            PreprocessOptions {
                reinclude: true,
                ..Default::default()
            }
        )
        .is_err()
    );
    let source = memory("#include \"a.glm\"\n#include \"a.glm\"\n")
        .with_named_buffer("a.glm", &b"object node {}"[..])
        .unwrap();
    assert!(preprocess_source(source.clone(), PreprocessOptions::default()).is_err());
    let prepared = preprocess_source(
        source,
        PreprocessOptions {
            reinclude: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(prepared.dependencies().len(), 2);
    assert_eq!(
        parse_raw_source(memory(prepared.text()))
            .unwrap()
            .statements
            .len(),
        2
    );
}

#[test]
fn missing_include_error_points_to_the_referrer_and_retains_source() {
    let failure = preprocess_source(
        memory("#include \"missing.glm\"\n"),
        PreprocessOptions::default(),
    )
    .unwrap_err();
    assert!(
        failure.diagnostics()[0]
            .code()
            .starts_with("REQUEST.SOURCE.")
    );
    let source = failure.retained_source().unwrap();
    let span = failure.diagnostics()[0].spans().last().unwrap();
    assert_eq!(span.source(), source.primary_buffer().unwrap().id());
}

#[test]
fn conditional_errors_and_resource_bounds_are_not_silently_repaired() {
    for text in [
        "#else\n",
        "#endif\n",
        "#ifdef X\n",
        "#ifdef X\n#else\n#else\n#endif\n",
        "#if X==1\n#endif\n",
        "#ifdef X\n#endif extra\n",
    ] {
        assert!(
            preprocess_source(memory(text), PreprocessOptions::default()).is_err(),
            "{text}"
        );
    }
    for text in [
        " ".repeat((1 << 20) + 1),
        format!("{}{}", "#ifdef X\n".repeat(65), "#endif\n".repeat(65)),
    ] {
        let failure = preprocess_source(memory(&text), PreprocessOptions::default()).unwrap_err();
        assert_eq!(failure.diagnostics()[0].code(), "PARSE.GLM.LIMIT_EXCEEDED");
    }
}

#[test]
fn settings_remain_inert_and_do_not_guess_typed_engine_globals() {
    let prepared = preprocess_source(
        memory("#set powerflow::solver_method=NR\n#warning review this model\nobject node {}"),
        PreprocessOptions::default(),
    )
    .unwrap();
    assert!(
        prepared
            .text()
            .starts_with("#set powerflow::solver_method=NR\n")
    );
    assert!(
        preprocess_source(
            memory("#set randomseed=1\nname ${randomseed};"),
            PreprocessOptions::default()
        )
        .is_err()
    );
}

#[test]
fn dialect_changes_if_left_operand_and_arras_settings_are_applied() {
    let text = "#define SELECT=100\n#if SELECT<20\nobject node { name variable; }\n#else\nobject node { name literal; }\n#endif\n";
    let pnnl = preprocess_source(memory(text), PreprocessOptions::default()).unwrap();
    let arras_options = PreprocessOptions {
        dialect: GlmDialect::Arras4_3,
        ..Default::default()
    };
    let arras = preprocess_source(memory(text), arras_options.clone()).unwrap();
    assert!(pnnl.text().contains("name variable;"));
    assert!(arras.text().contains("name literal;"));
    let explicitly_variable = preprocess_source(
        memory(&format!("#set literal_if=FALSE\n{text}")),
        arras_options.clone(),
    )
    .unwrap();
    assert!(explicitly_variable.text().contains("name variable;"));
    let literal = preprocess_source(
        memory("#if \"phase A\" in \"A\"\nobject node {}\n#endif\n"),
        arras_options,
    )
    .unwrap();
    assert!(literal.text().contains("object node"));
}
