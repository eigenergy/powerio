#![no_main]

use libfuzzer_sys::fuzz_target;
use powerio_core::Source;
use powerio_dist::glm::{GlmDialect, PreprocessOptions, parse_project};

fuzz_target!(|bytes: &[u8]| {
    if bytes.len() > 8192 { return; }
    let source = Source::from_memory("case.glm", bytes).unwrap();
    let dialect = if bytes.first().is_some_and(|b| b & 1 == 1) { GlmDialect::Arras4_3 } else { GlmDialect::Pnnl5_3 };
    if let Ok(project) = parse_project(source, PreprocessOptions { dialect, ..Default::default() }) {
        if let Ok(index) = project.index_objects() {
            if let Ok(points) = index.declared_coordinates() {
                assert_eq!(points.len(), index.objects().len());
                for point in points {
                    assert!(point.latitude.is_none_or(|v| v.is_finite() && v.abs() <= 90.0));
                    assert!(point.longitude.is_none_or(|v| v.is_finite() && v.abs() <= 180.0));
                }
            }
            for (ordinal, object) in index.objects().iter().enumerate().take(1024) {
                assert_eq!(object.key().ordinal(), ordinal);
                if let Some(parent) = index.parent(object.key()) { assert!(index.get(parent).is_some()); }
                for field in ["from", "to", "configuration"] {
                    if let Ok(Some(key)) = index.resolve_property(object.key(), field) { assert!(index.get(key).is_some()); }
                }
                for span in project.original_spans(object.span()) { assert!(span.byte_end() <= bytes.len() as u64); }
            }
        }
    }
});
