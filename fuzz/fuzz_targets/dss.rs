//! Malformed-input fuzzing of the OpenDSS reader, the distribution family's
//! hand-written tokenizer. `parse_dss_str` disables includes, so the harness
//! exercises the scanner and the element readers without touching the
//! filesystem. Writing the parsed network back is part of the target: the
//! writers size matrices from model arrays the reader filled, so a reader
//! cap that does not hold shows up as a panic or a runaway allocation here
//! rather than in a consumer.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let net = powerio_dist::dss::parse_dss_str(text);
    let _ = powerio_dist::write_pmd_json(&net);
    let _ = powerio_dist::write_bmopf_json(&net);
    let _ = net.graph();
});
