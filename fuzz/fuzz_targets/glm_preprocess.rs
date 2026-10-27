#![no_main]

use libfuzzer_sys::fuzz_target;
use powerio_core::Source;
use powerio_dist::glm::{GlmDialect, PreprocessOptions, preprocess_source};

fuzz_target!(|bytes: &[u8]| {
    if bytes.len() > 1 << 20 { return; }
    let source = Source::from_memory("case.glm", bytes).unwrap();
    let dialect = if bytes.first().is_some_and(|b| b & 1 == 1) { GlmDialect::Arras4_3 } else { GlmDialect::Pnnl5_3 };
    if let Ok(prepared) = preprocess_source(source, PreprocessOptions { dialect, ..Default::default() }) {
        for origin in prepared.origins() {
            let original = origin.original.byte_start() as usize..origin.original.byte_end() as usize;
            assert!(original.end <= bytes.len());
            if origin.exact {
                assert_eq!(&prepared.text().as_bytes()[origin.expanded.clone()], &bytes[original]);
            }
            assert!(!prepared.original_spans(origin.expanded.clone()).is_empty());
        }
    }
});
