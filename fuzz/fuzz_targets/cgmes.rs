#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(source) = powerio_core::Source::from_memory("input.xml", data.to_vec()) else {
        return;
    };
    let format = powerio_core::FormatId::new("cgmes").unwrap();
    let _ = powerio::parse(source.with_format(format));
});
