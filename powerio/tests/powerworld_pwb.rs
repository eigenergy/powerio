//! The .pwb reader against its same vintage aux siblings: the decode is
//! accepted only if counts are exact and values match the aux within storage
//! precision (the binary stores most quantities as f32, the aux prints the
//! f64 widening of them).

use std::path::Path;

use powerio::format::powerworld::parse_pwb;
use powerio::network::Network;
use powerio::parse_file;

mod common;
use common::{activsg2000_fetched as fetched, ckt, powerworld_vendored as vendored};

fn read_pwb(path: &Path) -> Network {
    let bytes = std::fs::read(path).unwrap();
    parse_pwb(&bytes, path.file_stem().and_then(|s| s.to_str())).unwrap()
}

/// Every decoded quantity of the vendored 200 bus binary against the same
/// vintage aux export, element by element.
#[test]
#[allow(clippy::too_many_lines)]
fn activsg200_pwb_matches_its_aux_sibling() {
    let pwb = read_pwb(&vendored("ACTIVSg200.pwb"));
    let aux = parse_file(vendored("ACTIVSg200.aux"), None).unwrap();

    assert_eq!(pwb.buses.len(), 200);
    assert_eq!(pwb.generators.len(), 49);
    assert_eq!(pwb.loads.len(), 160);
    assert_eq!(pwb.shunts.len(), 4);
    assert_eq!(pwb.branches.len(), 246);

    // Buses: identity, name, kV, area/zone, and the f64 solved state.
    for (p, a) in pwb.buses.iter().zip(&aux.buses) {
        assert_eq!(p.id, a.id);
        assert_eq!(p.name, a.name);
        assert!((p.base_kv - a.base_kv).abs() < 1e-4, "bus {} kV", p.id);
        assert_eq!((p.area, p.zone), (a.area, a.zone), "bus {}", p.id);
        assert!((p.vm - a.vm).abs() < 1e-12, "bus {} vm", p.id);
        assert!((p.va - a.va).abs() < 1e-9, "bus {} va", p.id);
    }
    // The binary carries no slack flag, so no bus reads as Ref; bus type is
    // derived from the generators (PV where an in-service machine sits). The
    // aux marks exactly one Ref bus (189). The pwb bus types are therefore a
    // best effort, not asserted against the aux here; the electrical values
    // above are the parity contract.
    assert!(pwb.buses.iter().all(|b| b.kind != powerio::BusType::Ref));
    assert_eq!(
        aux.buses
            .iter()
            .filter(|b| b.kind == powerio::BusType::Ref)
            .count(),
        1
    );

    // Loads and generators in per unit storage: f32 precision. Device
    // in-service status comes from a single byte whose meaning is only
    // partly validated (every device in this case is in service), so the
    // electrical values are the parity contract, not the status flag.
    for (p, a) in pwb.loads.iter().zip(&aux.loads) {
        assert_eq!(p.bus, a.bus);
        assert!(
            (p.p - a.p).abs() < 1e-4 * a.p.abs().max(1.0),
            "load at {}",
            p.bus
        );
        assert!(
            (p.q - a.q).abs() < 1e-4 * a.q.abs().max(1.0),
            "load q at {}",
            p.bus
        );
    }
    for (p, a) in pwb.generators.iter().zip(&aux.generators) {
        assert_eq!(p.bus, a.bus);
        for (x, y, what) in [
            (p.pg, a.pg, "pg"),
            (p.qg, a.qg, "qg"),
            (p.pmax, a.pmax, "pmax"),
            (p.pmin, a.pmin, "pmin"),
            (p.qmax, a.qmax, "qmax"),
            (p.qmin, a.qmin, "qmin"),
            (p.vg, a.vg, "vg"),
            (p.mbase, a.mbase, "mbase"),
        ] {
            assert!(
                (x - y).abs() < 1e-4 * y.abs().max(1.0),
                "gen at {} {what}: {x} vs {y}",
                p.bus
            );
        }
    }
    for (p, a) in pwb.shunts.iter().zip(&aux.shunts) {
        assert_eq!(p.bus, a.bus);
        assert!(
            (p.b - a.b).abs() < 1e-4 * a.b.abs().max(1.0),
            "shunt at {}",
            p.bus
        );
    }

    // Branches: identity (including the default circuit on the one record
    // that omits it), impedances, ratings, taps, device kind.
    let mut aux_by_id: std::collections::BTreeMap<(usize, usize, String), &powerio::Branch> =
        std::collections::BTreeMap::default();
    for b in &aux.branches {
        aux_by_id.insert((b.from.0, b.to.0, ckt(b)), b);
    }
    let mut transformers = 0;
    for p in &pwb.branches {
        let key = (p.from.0, p.to.0, ckt(p));
        let a = aux_by_id
            .remove(&key)
            .unwrap_or_else(|| panic!("{key:?} not in aux"));
        // Print precision of the lower precision side: the aux line section
        // prints the f64 widening of the stored f32 (20 decimals, near
        // exact), the transformer section prints 6 decimals. The RAW sweep
        // below pins the transformers at full f32 precision.
        let tol = |v: f64| {
            if p.is_transformer() {
                5e-7
            } else {
                1e-9 * v.abs().max(1e-3)
            }
        };
        assert!(
            (p.r - a.r).abs() <= tol(a.r),
            "{key:?} R {} vs {}",
            p.r,
            a.r
        );
        assert!(
            (p.x - a.x).abs() <= tol(a.x),
            "{key:?} X {} vs {}",
            p.x,
            a.x
        );
        assert!(
            (p.b - a.b).abs() <= tol(a.b),
            "{key:?} B {} vs {}",
            p.b,
            a.b
        );
        assert!(
            (p.rate_a - a.rate_a).abs() < 1e-4 * a.rate_a.abs().max(1.0),
            "{key:?} rate_a {} vs {}",
            p.rate_a,
            a.rate_a
        );
        assert!(
            (p.effective_tap() - a.effective_tap()).abs() < 1e-6,
            "{key:?} tap {} vs {}",
            p.effective_tap(),
            a.effective_tap()
        );
        assert_eq!(p.is_transformer(), a.is_transformer(), "{key:?} kind");
        transformers += usize::from(p.is_transformer());
    }
    assert!(
        aux_by_id.is_empty(),
        "aux branches missing from pwb: {aux_by_id:?}"
    );
    assert_eq!(transformers, 66);

    // The RAW sibling prints impedances at 6 significant digits, tighter
    // than the aux transformer section for per unit values below one, and
    // confirms the binary stores the full f32: transformer (15,14) R is
    // 6.37329E-4 in the RAW, 0.000637329 decoded, 0.000637 in the aux. The
    // RAW is a 2017 snapshot of the same case and carries no circuit IDs, so
    // transformers are matched on endpoints alone (no parallel pairs among
    // them); the two values TAMU revised between snapshots are pinned below.
    let raw = parse_file(vendored("ACTIVSg200.RAW"), None).unwrap();
    let raw_by_pair: std::collections::BTreeMap<(usize, usize), &powerio::Branch> = raw
        .branches
        .iter()
        .map(|b| ((b.from.0, b.to.0), b))
        .collect();
    let mut snapshot_deltas = Vec::new();
    for p in pwb.branches.iter().filter(|p| p.is_transformer()) {
        let pair = (p.from.0, p.to.0);
        let a = raw_by_pair
            .get(&pair)
            .unwrap_or_else(|| panic!("{pair:?} not in RAW"));
        for (x, y, what) in [(p.r, a.r, "R"), (p.x, a.x, "X")] {
            if (x - y).abs() > 5e-6 * y.abs() + 1e-12 {
                snapshot_deltas.push((pair.0, pair.1, what, x, y));
            }
        }
    }
    // TAMU rounded two transformer impedances between the 2017 and 2018
    // revisions (R 0.000495087 -> 0.000495, X 0.0078147 -> 0.007815); the pwb
    // agrees with its 2018 aux sibling on both. The delta set is asserted
    // exactly, like the (82,64) branch in powerworld_parity.rs.
    let deltas: Vec<_> = snapshot_deltas
        .iter()
        .map(|&(f, t, what, ..)| (f, t, what))
        .collect();
    assert_eq!(
        deltas,
        [(179, 178, "R"), (189, 187, "X")],
        "snapshot deltas changed: {snapshot_deltas:?}"
    );
}

/// The June 2016 ACTIVSg2000 export uses the Simulator 19 era record family
/// (bus flag words 0x06/0x07); the v19 file shares the Simulator 20 era head
/// layout but carries count prefixed lists in some bus record tails (flag
/// bit 4). Neither tail layout is decoded, so both must die at the vintage
/// gate with the evidence named rather than return a partial network.
/// Fetched fixtures; skipped when absent. When those tails are decoded, this
/// test becomes a parity test like the 200 bus one above.
#[test]
fn simulator19_vintage_is_rejected_loudly() {
    for name in ["Texas2000_June2016.pwb", "ACTIV_SG_2000_v19.pwb"] {
        let Some(path) = fetched(name) else {
            eprintln!("skipped {name}: run benchmarks/fetch_powerworld.sh");
            continue;
        };
        let bytes = std::fs::read(&path).unwrap();
        let err = parse_pwb(&bytes, None).unwrap_err();
        assert!(
            err.to_string()
                .contains("unsupported PowerWorld .pwb vintage"),
            "{name}: expected a loud vintage rejection, got: {err}"
        );
    }
}

/// Loud rejection of files that are not the validated layout.
#[test]
fn rejects_unrecognized_binaries() {
    let err = parse_pwb(b"not a pwb at all", None).unwrap_err();
    assert!(err.to_string().contains("header magic mismatch"), "{err}");

    // Right magic, garbage body.
    let mut fake = Vec::new();
    fake.extend_from_slice(&15000u64.to_le_bytes());
    fake.extend_from_slice(&425u64.to_le_bytes());
    fake.extend_from_slice(&20u64.to_le_bytes());
    fake.extend_from_slice(&[0u8; 4096]);
    let err = parse_pwb(&fake, None).unwrap_err();
    // All-zero body: no bus record run, so the vintage gate turns it away.
    assert!(
        err.to_string()
            .contains("unsupported PowerWorld .pwb vintage"),
        "{err}"
    );

    // A newer writer format constant (2021/2022 era exports carry 483, 508,
    // 537, 550, or 551 at offset 0x08) is a vintage rejection naming the
    // constant, never a generic magic mismatch.
    let mut newer = Vec::new();
    newer.extend_from_slice(&15000u64.to_le_bytes());
    newer.extend_from_slice(&483u64.to_le_bytes());
    newer.extend_from_slice(&20u64.to_le_bytes());
    newer.extend_from_slice(&[0u8; 4096]);
    let err = parse_pwb(&newer, None).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("unsupported PowerWorld .pwb vintage") && msg.contains("483"),
        "{err}"
    );
}
