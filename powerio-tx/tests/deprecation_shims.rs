//! The 0.8 names still compile against 0.9 behind deprecation warnings, and
//! the retired `powerio-json` token gets guidance instead of a bare unknown.
//! Everything here goes away at 1.0.0 along with the aliases it pins.
#![allow(deprecated)]
mod helpers;
#[allow(unused_imports)]
use helpers::*;

use powerio_tx::{DcConvention, Network};

#[test]
fn the_08_type_name_still_compiles() {
    let net: Network = powerio_tx::BalancedNetwork::new("shim", 100.0);
    let same: &powerio_tx::BalancedNetwork = &net;
    assert_eq!(same.name(), "shim");
}

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

#[test]
#[allow(clippy::float_cmp)]
fn the_08_branch_method_still_answers() {
    let net = parse_str(
        "function mpc = s\nmpc.version = '2';\nmpc.baseMVA = 100;\nmpc.bus = [\n  1 3 0 0 0 0 1 1 0 230 1 1.1 0.9;\n  2 1 0 0 0 0 1 1 0 230 1 1.1 0.9;\n];\nmpc.branch = [\n  1 2 0.01 0.1 0.04 0 0 0 0 0 1 -360 360;\n];\n",
        "matpower",
    )
    .unwrap()
    .network;
    let b = &net.branches()[0];
    assert_eq!(b.legacy_total_charging_b(), b.total_charging_b());
}
