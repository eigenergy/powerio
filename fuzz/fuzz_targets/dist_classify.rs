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
    let Ok(net) = powerio_dist::parse_str(text, format.name()) else {
        return;
    };
    let _ = powerio_dist::write_bmopf_json(&net);
    let _ = powerio_dist::write_pmd_json(&net);
});
