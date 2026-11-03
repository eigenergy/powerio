#![no_main]

use libfuzzer_sys::fuzz_target;
use powerio_tx::dynamic::parse_dyr_bytes;

fuzz_target!(|input: &[u8]| {
    if let Ok(case) = parse_dyr_bytes(input) {
        assert_eq!(case.to_dyr_string().as_bytes(), input);
        for record in case.records() {
            assert!(!record.model_name().is_empty());
            let _ = record.parameters();
        }
    }
});
