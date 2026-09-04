//! Malformed-input fuzzing of the PSS/E `.raw` reader.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = text;
        let Ok(source) = powerio_core::Source::from_memory("<fuzz>", data.to_vec()) else {
            return;
        };
        let Ok(id) = powerio_core::FormatId::new("psse") else {
            return;
        };
        let _ = powerio::parse(source.with_format(id));
    }
});
