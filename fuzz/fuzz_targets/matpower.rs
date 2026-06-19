//! Malformed-input fuzzing of the MATPOWER text reader: any input must come
//! back as `Ok` or a structured `Err`, never a panic (the gencost NCOST
//! overflow this crate exists to keep caught was found exactly this way).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = powerio::parse_str(text, "matpower");
    }
});
