//! Malformed input fuzzing of the sole PowerIO 1.0 IR reader through the
//! public operations: exact header and typed DTO decoding plus reference
//! validation. A document that deserializes must also serialize, and the
//! serialized document must deserialize again: a reader accepting a document
//! its own writer refuses is a validation hole.
#![no_main]

use libfuzzer_sys::fuzz_target;
use powerio::{Destination, EmittedOutput, Source};

fuzz_target!(|data: &[u8]| {
    let Ok(source) = Source::from_memory("fuzz.pio.json", data.to_vec()) else {
        return;
    };
    let Ok(module) = powerio::deserialize(source) else {
        return;
    };
    let destination = Destination::memory("fuzz").expect("a memory destination");
    let result = powerio::serialize(&module, destination).expect("an accepted module serializes");
    let EmittedOutput::Memory { artifacts } = result.into_output() else {
        panic!("a memory destination yields memory artifacts");
    };
    let artifact = artifacts
        .into_iter()
        .next()
        .expect("serialize writes one artifact");
    let source = Source::from_memory("fuzz.pio.json", artifact.into_bytes())
        .expect("serialized bytes form a source");
    powerio::deserialize(source).expect("a serialized document deserializes again");
});
