//! The `.json` classifier is the first code an untrusted distribution file
//! reaches through `parse_file`, and it runs before any reader cap applies.
//! It must never panic and never overflow the stack on a nested value. The
//! target drives the classifier and, when it names a reader, that reader
//! and both writers as well.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(format) = powerio_dist::classify_distribution_json(text) else {
        return;
    };
    let Ok(source) = powerio_core::Source::from_bytes("<fuzz>", data.to_vec()) else {
        return;
    };
    let Ok(id) = powerio_core::FormatId::new(format.name()) else {
        return;
    };
    let Ok(module) = powerio_dist::parse(source.with_format(id)) else {
        return;
    };
    let module = module.sever_source();
    for target in [
        powerio_dist::DistTargetFormat::BmopfJson,
        powerio_dist::DistTargetFormat::PmdJson,
    ] {
        let Ok(destination) = powerio_core::Destination::memory("fuzz") else {
            return;
        };
        let _ = powerio_dist::emit(&module, target, destination);
    }
});
