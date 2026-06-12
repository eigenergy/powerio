//! Malformed-input fuzzing of the PowerWorld `.pwd` display decoder — raw
//! attacker-controlled bytes drive every offset and length it reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = powerio::format::powerworld::parse_pwd(data);
});
