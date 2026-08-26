//! Malformed-input fuzzing of the PowerWorld `.aux` reader: the one
//! hand-written text tokenizer the parse entry reaches (the JSON dialects ride
//! serde), so it carries the same byte-indexing hazards as the binary
//! decoders.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = text;
        let Ok(source) = powerio_core::Source::from_bytes("<fuzz>", data.to_vec()) else {
            return;
        };
        let Ok(id) = powerio_core::FormatId::new("powerworld") else {
            return;
        };
        let _ = powerio::parse(source.with_format(id));
    }
});
