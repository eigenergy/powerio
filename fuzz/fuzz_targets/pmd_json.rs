//! Malformed-input fuzzing of the PMD ENGINEERING and BMOPF JSON readers.
//! Both ride serde_json for tokenizing, so the target is what happens after:
//! the readers project a free form document onto the model, and the writers
//! and graph builder size allocations from what landed there. A missing cap
//! surfaces as a panic or a runaway allocation on a small input.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let _ = text;
    for format in ["pmd-json", "bmopf-json"] {
        let Ok(source) = powerio_core::Source::from_bytes("<fuzz>", data.to_vec()) else {
            continue;
        };
        let Ok(id) = powerio_core::FormatId::new(format) else {
            continue;
        };
        let Ok(module) = powerio_dist::parse(source.with_format(id)) else {
            continue;
        };
        let module = module.sever_source();
        let _ = module.value().to_graph();
        for target in [
            powerio_dist::DistTargetFormat::PmdJson,
            powerio_dist::DistTargetFormat::BmopfJson,
            powerio_dist::DistTargetFormat::Dss,
        ] {
            let Ok(destination) = powerio_core::Destination::memory("fuzz") else {
                return;
            };
            let _ = powerio_dist::emit(&module, target, destination);
        }
    }
});
