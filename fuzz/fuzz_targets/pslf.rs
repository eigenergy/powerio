//! Malformed-input fuzzing of the PSLF `.epc` reader (a hand-written tokenizer).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = text;
        let Ok(source) = powerio_core::Source::from_bytes("<fuzz>", data.to_vec()) else {
            return;
        };
        let Ok(id) = powerio_core::FormatId::new("pslf") else {
            return;
        };
        let _ = powerio::parse(source.with_format(id));
    }
});
