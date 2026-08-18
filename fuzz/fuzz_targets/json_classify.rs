//! The `.json` classifier is the first code an untrusted transmission file
//! reaches through `parse_file`, and a file picker dispatches on its answer,
//! so it must never panic, never overflow the stack on a nested value, and
//! never answer outside the closed family set. The C entry point rides the
//! same input, since a binding reaches the classifier only through it.
#![no_main]

use std::ffi::CString;

use libfuzzer_sys::fuzz_target;
use powerio::format::routing::{JSON_CLASSES, classify_json_text};

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let class = classify_json_text(text);
    assert!(
        JSON_CLASSES.contains(&class.family()),
        "classifier answered outside the closed set: {class:?}"
    );

    // The C label is the family, optionally with a `:<format>` tail; an
    // interior NUL cannot reach the entry point, so skip those inputs.
    let Ok(c_text) = CString::new(text) else {
        return;
    };
    let mut buf = [0i8; 128];
    let n = unsafe { powerio_capi::pio_classify_str(c_text.as_ptr(), buf.as_mut_ptr(), buf.len()) };
    let label = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }
        .to_str()
        .expect("the label is ASCII");
    assert!(n >= label.len(), "the returned length must not undercount");
    let family = label.split(':').next().unwrap_or("");
    assert!(
        JSON_CLASSES.contains(&family),
        "pio_classify_str answered outside the closed set: {label}"
    );
});
