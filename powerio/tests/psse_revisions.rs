//! PSS/E `.raw` revision coverage: a case written at v34 and v35 reads back to
//! the same electrical core as the MATPOWER source it came from.
//!
//! `case14_v34.raw` / `case14_v35.raw` were produced from `case14.m` with
//! `powerio convert … --to psse34/psse35`, so they carry the modern deltas (the
//! system-wide header marker, the named 12-rating branch record, and the load
//! distributed-generation / load-type trailing columns). The reader takes the
//! revision from the file header and must recover the same network from each.

// The base frequency is an exact decimal (60.0) read from the header; bit
// equality is the intended assertion.
#![allow(clippy::float_cmp)]

use std::path::{Path, PathBuf};

use powerio::{Network, parse_matpower_file, parse_psse};

fn data(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data")
        .join(name)
}

fn read_psse(name: &str) -> Network {
    parse_psse(&std::fs::read_to_string(data(name)).unwrap()).unwrap()
}

#[derive(Debug, PartialEq)]
struct Core {
    buses: usize,
    branches: usize,
    gens: usize,
    loads: usize,
    load_p: i64,
    load_q: i64,
    gen_p: i64,
}

fn core(net: &Network) -> Core {
    let r = |x: f64| (x * 1e3).round() as i64;
    Core {
        buses: net.buses.len(),
        branches: net.branches.len(),
        gens: net.generators.len(),
        loads: net.loads.len(),
        load_p: r(net.loads.iter().map(|l| l.p).sum()),
        load_q: r(net.loads.iter().map(|l| l.q).sum()),
        gen_p: r(net.generators.iter().map(|g| g.pg).sum()),
    }
}

#[test]
fn v34_and_v35_fixtures_match_the_matpower_source() {
    let source = core(&parse_matpower_file(data("case14.m")).unwrap());
    let v34 = read_psse("psse/case14_v34.raw");
    let v35 = read_psse("psse/case14_v35.raw");

    assert_eq!(core(&v34), source, "v34 fixture lost or gained elements");
    assert_eq!(core(&v35), source, "v35 fixture lost or gained elements");
    // Frequency rides the header at every revision.
    assert_eq!(v34.base_frequency, 60.0);
    assert_eq!(v35.base_frequency, 60.0);
}
