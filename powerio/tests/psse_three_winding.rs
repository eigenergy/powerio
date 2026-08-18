//! A PSS/E `.raw` carrying an in-service 3-winding transformer, read from the
//! corpus and taken through the star-lowered view every per-bus extractor
//! reports over.
//!
//! `case3_3w_v33.raw` is the only corpus fixture whose transformer records have
//! a nonzero third bus. The same bytes are the PowerIO.jl fixture, so the two
//! suites cannot drift.

use std::collections::HashSet;
use std::path::Path;

use powerio::indexed::IndexedNetwork;
use powerio::network::BusId;
use powerio::parse_psse;

const FIXTURE: &str = "../tests/data/psse/case3_3w_v33.raw";

fn read() -> powerio::BalancedNetwork {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE);
    parse_psse(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn the_reader_keeps_the_three_winding_record_out_of_the_branch_table() {
    let net = read();
    assert_eq!(net.buses.len(), 3);
    assert_eq!(net.transformers_3w.len(), 1);
    assert!(net.transformers_3w[0].in_service);
    assert!(
        net.branches.is_empty(),
        "a 3-winding record is one transformer, not three branches"
    );
}

#[test]
fn the_star_lowered_space_is_one_bus_wider_per_in_service_transformer() {
    let net = read();
    let n_star = net.transformers_3w.iter().filter(|t| t.in_service).count();
    let view = IndexedNetwork::new(&net);

    assert_eq!(view.n(), net.buses.len() + n_star);
    assert_eq!(view.n(), 4);
    assert_eq!(view.branches().len(), 3 * n_star);

    // The per-bus extractors are sized off the same space, which is the v5
    // change: through v4 the bus count reported the unexpanded table.
    assert_eq!(view.pd().len(), view.n());
    assert_eq!(view.qd().len(), view.n());
    assert_eq!(view.gs().len(), view.n());
    assert_eq!(view.bs().len(), view.n());
}

#[test]
fn the_reported_bus_ids_are_distinct_and_cover_the_file_ids() {
    let net = read();
    let view = IndexedNetwork::new(&net);

    let ids: Vec<BusId> = (0..view.n()).map(|i| view.bus_id(i)).collect();
    assert_eq!(ids.len(), view.n());
    assert_eq!(
        ids.iter().collect::<HashSet<_>>().len(),
        view.n(),
        "the synthetic star point must not reuse a file bus id"
    );
    for bus in &net.buses {
        assert!(ids.contains(&bus.id), "file bus {:?} dropped", bus.id);
    }
}

#[test]
fn every_branch_endpoint_lands_inside_the_reported_bus_space() {
    let net = read();
    let view = IndexedNetwork::new(&net);

    for branch in view.branches() {
        for end in [branch.from, branch.to] {
            let idx = view
                .bus_index(end)
                .unwrap_or_else(|| panic!("endpoint {end:?} is not a reported bus"));
            assert!(idx < view.n());
        }
    }

    // The star branches ground buses 2 and 3 through the reference bus instead
    // of leaving them as ungrounded islands.
    assert_eq!(view.n_connected_components(), 1);
    view.check_reference_coverage().unwrap();
}
