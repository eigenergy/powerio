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
    for format in ["pmd-json", "bmopf-json"] {
        let Ok(net) = powerio_dist::parse_str(text, format) else {
            continue;
        };
        let _ = powerio_dist::write_pmd_json(&net);
        let _ = powerio_dist::write_bmopf_json(&net);
        let _ = powerio_dist::write_dss(&net);
        let _ = net.graph();
    }
});
