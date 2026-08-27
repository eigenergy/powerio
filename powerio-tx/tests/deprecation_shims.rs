//! The remaining 0.9 compatibility spellings still answer, and the retired
//! `powerio-json` token gets guidance instead of a bare unknown. The alias
//! tests go away with their aliases before 1.0.0.
#![allow(deprecated)]
mod helpers;
#[allow(unused_imports)]
use helpers::*;

use powerio_tx::DcConvention;

#[test]
fn paper_pure_works_in_expression_and_pattern_position() {
    let convention = DcConvention::PaperPure;
    assert_eq!(convention, DcConvention::ReactanceOnly);
    match convention {
        DcConvention::PaperPure => {}
        other => panic!("PaperPure must match its successor, got {other:?}"),
    }
}

#[test]
fn the_retired_powerio_json_token_gets_guidance() {
    let err = parse_str("x", "powerio-json").unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("retired in 0.9.0"), "{msg}");
    assert!(msg.contains("model-json"), "{msg}");
}
