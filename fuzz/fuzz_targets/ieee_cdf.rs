//! Malformed-input fuzzing of the IEEE Common Data Format reader.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // CDF is a fixed-column text format, but arbitrary bytes are intentional:
    // invalid encodings and truncated records must fail as structured errors,
    // not panic while the reader walks field boundaries.
    let Ok(source) = powerio_core::Source::from_memory("input.cdf", data.to_vec()) else {
        return;
    };
    // This is a compile-time harness invariant, not an input-dependent failure.
    // Fail loudly rather than silently skipping every fuzz case if the ID changes.
    let format = powerio_core::FormatId::new("ieee-cdf")
        .expect("the canonical IEEE CDF format ID must be valid");
    let _ = powerio::parse(source.with_format(format));
});
