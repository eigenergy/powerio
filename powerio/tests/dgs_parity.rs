//! Parity for the DIgSILENT DGS reader against the MATPOWER companion of the
//! same standard case.
//!
//! DGS is the plaintext format PowerFactory writes for interoperability (the
//! `.pfd` project export is encrypted and has no public decoder). These fixtures
//! are real PowerFactory exports of the IEEE 39 and IEEE 118 systems, so the
//! topology must match the canonical MATPOWER case.
//!
//! Topology (bus set, branch endpoints, generator count) is asserted exactly.
//! For IEEE 39 the VeraGrid export also carries the same per-unit data as
//! case39, so the reader's per-unit conversion and transformer Z/tap math are
//! checked numerically (45/46 branches; one transformer differs by a single
//! source value). The IEEE 118 export is a different 118-bus variant, so only
//! its provenance-independent invariants (bus set, generator count) are checked.
//!
//! Fixtures are vendored under `tests/data/dgs/` (see its README). Each test
//! skips when its fixture is absent, so the suite stays green without them.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use powerio::network::Network;
use powerio::parse_file;

fn data(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data")
        .join(rel)
}

fn load(rel: &str) -> Option<Network> {
    let path = data(rel);
    if !path.exists() {
        eprintln!("skip: fixture {} not present", path.display());
        return None;
    }
    Some(
        parse_file(&path, None)
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
            .network,
    )
}

fn bus_ids(net: &Network) -> BTreeSet<usize> {
    net.buses.iter().map(|b| b.id.0).collect()
}

fn branch_endpoints(net: &Network) -> Vec<(usize, usize)> {
    let mut v: Vec<(usize, usize)> = net
        .branches
        .iter()
        .map(|b| (b.from.0.min(b.to.0), b.from.0.max(b.to.0)))
        .collect();
    v.sort_unstable();
    v
}

#[test]
fn ieee39_dgs_matches_case39_matpower() {
    let (Some(dgs), Some(mp)) = (load("dgs/IEEE_39.dgs"), load("case39.m")) else {
        return;
    };

    assert_eq!(dgs.buses.len(), 39, "IEEE 39 DGS bus count");
    assert_eq!(bus_ids(&dgs), bus_ids(&mp), "IEEE 39 bus id set");
    assert_eq!(
        branch_endpoints(&dgs),
        branch_endpoints(&mp),
        "IEEE 39 branch endpoint multiset (lines + transformers)"
    );
    assert_eq!(
        dgs.generators.len(),
        mp.generators.len(),
        "IEEE 39 generator count"
    );
    // 34 lines + 12 transformers in the canonical case.
    assert_eq!(
        dgs.branches.iter().filter(|b| b.tap != 0.0).count(),
        12,
        "IEEE 39 transformer count"
    );

    // Numerical parity: this DGS export carries the same per-unit data as
    // case39, so the reader's per-unit conversion (rline*dline / zbase) and the
    // transformer Z/tap math must reproduce it. Index by endpoint (the case has
    // no parallel branches) and compare r, x, and effective tap. One transformer
    // (20-34) has 2x the impedance in the VeraGrid export, a single-element
    // source difference, so the gate is 45/46 rather than exact.
    let by_endpoint = |net: &Network| -> HashMap<(usize, usize), (f64, f64, f64)> {
        net.branches
            .iter()
            .map(|b| {
                (
                    (b.from.0.min(b.to.0), b.from.0.max(b.to.0)),
                    (b.r, b.x, b.effective_tap()),
                )
            })
            .collect()
    };
    let d = by_endpoint(&dgs);
    let m = by_endpoint(&mp);
    assert_eq!(
        d.len(),
        46,
        "IEEE 39 DGS endpoints collide (resolution bug)"
    );
    assert_eq!(m.len(), 46, "case39 endpoints collide");
    let close = |a: f64, b: f64| (a - b).abs() < 1e-3;
    let mut matched = 0;
    for (k, &(dr, dx, dt)) in &d {
        if let Some(&(mr, mx, mt)) = m.get(k) {
            if close(dr, mr) && close(dx, mx) && close(dt, mt) {
                matched += 1;
            }
        }
    }
    assert!(
        matched >= 45,
        "IEEE 39 branch r/x/tap parity vs case39: {matched}/46 within 1e-3"
    );
}

#[test]
fn ieee118_dgs_matches_case118_buses_and_generators() {
    let (Some(dgs), Some(mp)) = (load("dgs/IEEE118_v2_test.dgs"), load("case118.m")) else {
        return;
    };

    assert_eq!(dgs.buses.len(), 118, "IEEE 118 DGS bus count");
    assert_eq!(bus_ids(&dgs), bus_ids(&mp), "IEEE 118 bus id set");
    assert_eq!(dgs.generators.len(), 54, "IEEE 118 DGS generator count");
    assert_eq!(
        dgs.generators.len(),
        mp.generators.len(),
        "IEEE 118 generator count vs MATPOWER"
    );
    // This public DGS export is a 118-bus variant with 170 lines + 9
    // transformers = 179 branches; MATPOWER case118 has 186. The difference is
    // the export's line set, a provenance difference, not a parser defect.
    assert_eq!(dgs.branches.len(), 179, "IEEE 118 DGS branch count");
    for b in &dgs.branches {
        assert!(
            b.r.is_finite() && b.x.is_finite(),
            "IEEE 118 branch {}-{} impedance not finite",
            b.from.0,
            b.to.0
        );
    }
}
