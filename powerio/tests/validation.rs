//! Input-validation and reader-robustness guarantees: malformed input must fail
//! loudly, never silently default into a structurally valid but wrong network.

use std::path::Path;

use powerio::network::{Branch, Bus, BusId, BusType, Extras, Network};
use powerio::{Error, parse_psse};

fn bus(id: usize, kind: BusType) -> Bus {
    Bus {
        id: BusId(id),
        kind,
        vm: 1.0,
        va: 0.0,
        base_kv: 1.0,
        vmax: 1.1,
        vmin: 0.9,
        area: 1,
        zone: 1,
        name: None,
        extras: Extras::new(),
    }
}

fn branch(from: usize, to: usize) -> Branch {
    Branch {
        from: BusId(from),
        to: BusId(to),
        r: 0.0,
        x: 0.1,
        b: 0.0,
        rate_a: 0.0,
        rate_b: 0.0,
        rate_c: 0.0,
        tap: 0.0,
        shift: 0.0,
        in_service: true,
        angmin: -360.0,
        angmax: 360.0,
        extras: Extras::new(),
    }
}

#[test]
fn validate_rejects_duplicate_bus_id() {
    // Two buses share id 1: dense indexing would collapse them onto one index
    // and silently corrupt every nodal aggregate, so validate() must reject it.
    let net = Network::in_memory(
        "dup",
        100.0,
        vec![bus(1, BusType::Ref), bus(1, BusType::Pq)],
        Vec::new(),
    );
    assert!(matches!(net.validate(), Err(Error::FormatRead { .. })));
}

#[test]
fn validate_rejects_dangling_branch_endpoint() {
    let net = Network::in_memory(
        "dangling",
        100.0,
        vec![bus(1, BusType::Ref), bus(2, BusType::Pq)],
        vec![branch(1, 99)],
    );
    assert!(matches!(net.validate(), Err(Error::FormatRead { .. })));
}

#[test]
fn from_json_rejects_dangling_reference() {
    // to_json does not validate, so a hand-built (or hand-edited) invalid network
    // serializes fine; from_json must reject it on the way back in, since the
    // C ABI and Julia bridge ride on this transport.
    let bad = Network::in_memory(
        "bad",
        100.0,
        vec![bus(1, BusType::Ref), bus(2, BusType::Pq)],
        vec![branch(1, 99)],
    );
    let json = bad.to_json().unwrap();
    assert!(matches!(
        Network::from_json(&json),
        Err(Error::FormatRead { .. })
    ));
}

#[test]
fn psse_rejects_malformed_numeric_field() {
    // The pristine fixture parses; corrupting one numeric field (a bus voltage
    // magnitude) must error rather than silently default it — a present-but-
    // garbage number that becomes 0.0 would corrupt the matrices downstream.
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data/psse/case14.raw");
    let good = std::fs::read_to_string(&path).unwrap();
    assert!(parse_psse(&good).is_ok(), "pristine fixture should parse");

    let bad = good.replacen("1.05999994", "1.0xx99994", 1);
    assert_ne!(good, bad, "corruption target not found in fixture");
    assert!(matches!(parse_psse(&bad), Err(Error::FormatRead { .. })));
}
