//! Exercise both RAVENS profiles and PMD MATHEMATICAL with explicit formats,
//! including fresh emission so retained source bytes cannot hide writer bugs.
#![no_main]

use libfuzzer_sys::fuzz_target;
use powerio::{Destination, FormatId, Source};

fuzz_target!(|data: &[u8]| {
    for format in ["ravens-json", "pmd-math-json", "pmd-json"] {
        let Ok(source) = Source::from_memory("fuzz.json", data.to_vec()) else {
            continue;
        };
        let source = source.with_format(FormatId::new(format).unwrap());
        let Ok(module) = powerio::parse(source) else {
            continue;
        };
        let module = module.sever_source();
        for target in ["ravens-json", "pmd-math-json"] {
            let destination = Destination::memory("fuzz").unwrap();
            let _ = powerio::emit(&module, target, destination);
        }
    }
});
