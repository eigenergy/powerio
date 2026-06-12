//! Malformed-input fuzzing of the PowerWorld `.aux` reader — the one
//! hand-written text tokenizer `parse_str` reaches (the JSON dialects ride
//! serde), so it carries the same byte-indexing hazards as the binary
//! decoders.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = powerio::parse_str(text, "powerworld");
    }
});
