//! The `.json` classifier is the first code an untrusted transmission file
//! reaches through `parse`, and a file picker dispatches on its answer, so it
//! must never panic, never overflow the stack on a nested value, and never
//! answer outside the closed family set. The text and byte entry points ride
//! the same input and must agree.
#![no_main]

use libfuzzer_sys::fuzz_target;
use powerio_tx::format::routing::{JSON_CLASSES, classify_json_bytes, classify_json_text};

fuzz_target!(|data: &[u8]| {
    let from_bytes = classify_json_bytes(data);
    assert!(
        JSON_CLASSES.contains(&from_bytes.family()),
        "byte classifier answered outside the closed set: {from_bytes:?}"
    );
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let from_text = classify_json_text(text);
    assert!(
        JSON_CLASSES.contains(&from_text.family()),
        "text classifier answered outside the closed set: {from_text:?}"
    );
    assert_eq!(
        from_text.family(),
        from_bytes.family(),
        "the text and byte classifiers disagree"
    );
});
