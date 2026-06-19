//! Malformed-input fuzzing of the canonical `powerio-json` snapshot reader
//! (serde deserialization plus the reference validation pass).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = powerio::parse_str(text, "powerio-json");
    }
});
