//! Malformed-input fuzzing of the balanced model JSON reader (serde
//! deserialization plus the reference validation pass), which is what
//! `pio_from_json` exposes to untrusted input.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = powerio::BalancedNetwork::from_json(text);
    }
});
