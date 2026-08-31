//! Malformed-input fuzzing of the stored module reader: header dispatch,
//! exact typed DTO decode, reference validation, and the one way 0.9 package
//! upgrade — the whole surface `pio_module_read_json` exposes to untrusted
//! input. A read that succeeds must also write, and the rewritten document
//! must read back: a reader accepting a document its own writer refuses is a
//! validation hole.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        if let Ok(module) = powerio::stored::read_module(text) {
            let emitted =
                powerio::stored::emit_module(&module).expect("an accepted module emits");
            powerio::stored::read_module(&emitted).expect("an emitted document reads back");
        }
    }
});
