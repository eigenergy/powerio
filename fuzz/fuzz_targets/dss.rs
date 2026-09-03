//! Malformed-input fuzzing of the OpenDSS reader, the distribution family's
//! hand-written tokenizer. An in-memory source has no named buffers, so the
//! harness exercises the scanner and the element readers without touching
//! the filesystem. Writing the parsed network back is part of the target:
//! the writers size matrices from model arrays the reader filled, so a
//! reader cap that does not hold shows up as a panic or a runaway allocation
//! here rather than in a consumer.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if std::str::from_utf8(data).is_err() {
        return;
    }
    let Ok(source) = powerio_core::Source::from_memory("<fuzz>", data.to_vec()) else {
        return;
    };
    let Ok(id) = powerio_core::FormatId::new("dss") else {
        return;
    };
    let Ok(module) = powerio_dist::parse(source.with_format(id)) else {
        return;
    };
    let net = module.value();
    let _ = net.to_graph();
    for format in [
        powerio_dist::DistTargetFormat::PmdJson,
        powerio_dist::DistTargetFormat::BmopfJson,
    ] {
        let Ok(destination) = powerio_core::Destination::memory("fuzz") else {
            return;
        };
        let _ = powerio_dist::emit(&module, format, destination);
    }
});
