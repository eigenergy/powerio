//! The .pwb reader against its same vintage aux siblings: the decode is
//! accepted only if counts are exact and values match the aux within storage
//! precision (the binary stores most quantities as f32, the aux prints the
//! f64 widening of them).

use std::collections::{BTreeMap, BTreeSet};
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
    let aux = parse_file(vendored("ACTIVSg200.aux"), None)
        .unwrap()
        .network;

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
    let mut aux_by_id: BTreeMap<(usize, usize, String), &powerio::Branch> = BTreeMap::default();
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
            (p.rate_b - a.rate_b).abs() < 1e-4 * a.rate_b.abs().max(1.0),
            "{key:?} rate_b {} vs {}",
            p.rate_b,
            a.rate_b
        );
        assert!(
            (p.effective_tap() - a.effective_tap()).abs() < 1e-6,
            "{key:?} tap {} vs {}",
            p.effective_tap(),
            a.effective_tap()
        );
        assert_eq!(p.is_transformer(), a.is_transformer(), "{key:?} kind");
        assert_eq!(
            p.extras.get("BranchDeviceType"),
            a.extras.get("BranchDeviceType"),
            "{key:?} device type"
        );
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
    let raw = parse_file(vendored("ACTIVSg200.RAW"), None)
        .unwrap()
        .network;
    let raw_by_pair: BTreeMap<(usize, usize), &powerio::Branch> = raw
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

/// The June 2016 ACTIVSg2000 export (Simulator 19 era record family, bus
/// flag words 0x06/0x07, three inline rating slots) against its same day aux
/// sibling: exact counts, every decoded value at the print precision of the
/// lower precision side (this aux prints solved voltages at 6 decimals,
/// powers and ratings at 3). Fetched fixtures; skipped when absent.
#[test]
#[allow(clippy::too_many_lines)]
fn texas2000_june2016_pwb_matches_its_aux_sibling() {
    let (Some(pwb_path), Some(aux_path)) = (
        fetched("Texas2000_June2016.pwb"),
        fetched("Texas2000_June2016.AUX"),
    ) else {
        eprintln!("skipped: run benchmarks/fetch_powerworld.sh");
        return;
    };
    let pwb = read_pwb(&pwb_path);
    let aux = parse_file(aux_path, None).unwrap().network;

    assert_eq!(pwb.buses.len(), 2007);
    assert_eq!(pwb.loads.len(), 1417);
    assert_eq!(pwb.generators.len(), 282);
    assert_eq!(pwb.shunts.len(), 41);
    assert_eq!(pwb.branches.len(), 3043);
    assert_eq!(aux.buses.len(), 2007);
    assert_eq!(aux.branches.len(), 3043);

    for (p, a) in pwb.buses.iter().zip(&aux.buses) {
        assert_eq!(p.id, a.id);
        assert_eq!(p.name, a.name);
        assert!((p.base_kv - a.base_kv).abs() < 1e-4, "bus {} kV", p.id);
        assert_eq!((p.area, p.zone), (a.area, a.zone), "bus {}", p.id);
        assert!(
            (p.vm - a.vm).abs() <= 5e-7,
            "bus {} vm {} vs {}",
            p.id,
            p.vm,
            a.vm
        );
        assert!(
            (p.va - a.va).abs() <= 5e-5,
            "bus {} va {} vs {}",
            p.id,
            p.va,
            a.va
        );
    }

    for (p, a) in pwb.loads.iter().zip(&aux.loads) {
        assert_eq!(p.bus, a.bus);
        assert!(
            (p.p - a.p).abs() <= 1e-3,
            "load at {}: {} vs {}",
            p.bus,
            p.p,
            a.p
        );
        assert!(
            (p.q - a.q).abs() <= 1e-3,
            "load q at {}: {} vs {}",
            p.bus,
            p.q,
            a.q
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
                (x - y).abs() <= 1e-3 + 1e-6 * y.abs(),
                "gen at {} {what}: {x} vs {y}",
                p.bus
            );
        }
    }
    for (p, a) in pwb.shunts.iter().zip(&aux.shunts) {
        assert_eq!(p.bus, a.bus);
        assert!(
            (p.b - a.b).abs() <= 1e-3,
            "shunt at {}: {} vs {}",
            p.bus,
            p.b,
            a.b
        );
    }

    let mut aux_by_id: BTreeMap<(usize, usize, String), &powerio::Branch> = BTreeMap::default();
    for b in &aux.branches {
        aux_by_id.insert((b.from.0, b.to.0, ckt(b)), b);
    }
    let mut transformers = 0;
    for p in &pwb.branches {
        let key = (p.from.0, p.to.0, ckt(p));
        let a = aux_by_id
            .remove(&key)
            .unwrap_or_else(|| panic!("{key:?} not in aux"));
        assert!((p.r - a.r).abs() <= 5e-7, "{key:?} R {} vs {}", p.r, a.r);
        assert!((p.x - a.x).abs() <= 5e-7, "{key:?} X {} vs {}", p.x, a.x);
        assert!((p.b - a.b).abs() <= 5e-7, "{key:?} B {} vs {}", p.b, a.b);
        for (x, y, what) in [
            (p.rate_a, a.rate_a, "rate_a"),
            (p.rate_b, a.rate_b, "rate_b"),
            (p.rate_c, a.rate_c, "rate_c"),
        ] {
            assert!(
                (x - y).abs() <= 1e-3 + 1e-6 * y.abs(),
                "{key:?} {what} {x} vs {y}"
            );
        }
        assert!(
            (p.effective_tap() - a.effective_tap()).abs() < 1e-6,
            "{key:?} tap {} vs {}",
            p.effective_tap(),
            a.effective_tap()
        );
        assert_eq!(p.is_transformer(), a.is_transformer(), "{key:?} kind");
        assert_eq!(
            p.extras.get("BranchDeviceType"),
            a.extras.get("BranchDeviceType"),
            "{key:?} device type"
        );
        transformers += usize::from(p.is_transformer());
    }
    assert!(
        aux_by_id.is_empty(),
        "aux branches missing from pwb: {aux_by_id:?}"
    );
    assert_eq!(transformers, 562);
}

/// The v19 ACTIVSg2000 export (April 2017, Simulator 20 era records with
/// count prefixed list tails, bus flags 0x36/0x37 and branch flags
/// 0xFE/0xFF) against the published case in MATPOWER format. The v19 file
/// has no same day sibling, so the bar is structural identity plus values
/// that are stable across snapshots (loads, impedances), with every
/// difference pinned exactly. Buses match by order: the .m renumbers
/// 1..2000 to 1001..8160 but keeps the order and the names (apostrophes
/// printed as spaces). Fetched fixtures; skipped when absent.
#[test]
#[allow(clippy::too_many_lines)]
fn activsg2000_v19_pwb_matches_the_published_case() {
    let (Some(pwb_path), Some(m_path)) = (
        fetched("ACTIV_SG_2000_v19.pwb"),
        fetched("case_ACTIVSg2000.m"),
    ) else {
        eprintln!("skipped: run benchmarks/fetch_powerworld.sh");
        return;
    };
    let pwb = read_pwb(&pwb_path);
    let m = parse_file(m_path, None).unwrap().network;

    assert_eq!(pwb.buses.len(), 2000);
    assert_eq!(pwb.loads.len(), 1350);
    assert_eq!(pwb.generators.len(), 545);
    assert_eq!(pwb.shunts.len(), 154);
    assert_eq!(pwb.branches.len(), 3202);
    assert_eq!(m.buses.len(), 2000);

    // Bus identity by name: the published case renumbered and reordered the
    // buses, so order does not map them, but names are unique in both files
    // (the .m flattens apostrophes to spaces, and its "May-00" is this
    // file's "MAY 0" mangled by a spreadsheet export). Two buses were
    // re-leveled after the v19 snapshot, pinned below.
    let m_by_name: BTreeMap<String, &powerio::Bus> = m
        .buses
        .iter()
        .map(|b| {
            let n = b
                .name
                .as_deref()
                .unwrap_or("")
                .trim_matches('\'')
                .to_string();
            (if n == "May-00" { "MAY 0".into() } else { n }, b)
        })
        .collect();
    assert_eq!(m_by_name.len(), 2000, "duplicate .m bus names");
    let mut m_id_by_pwb_id = BTreeMap::new();
    let mut kv_deltas = Vec::new();
    for p in &pwb.buses {
        let pn = p.name.as_deref().unwrap_or("").replace('\'', " ");
        let a = m_by_name
            .get(&pn)
            .unwrap_or_else(|| panic!("bus {} {pn:?} not in the .m", p.id));
        if (p.base_kv - a.base_kv).abs() >= 1e-4 {
            kv_deltas.push((a.id.0, p.base_kv, a.base_kv));
        }
        m_id_by_pwb_id.insert(p.id.0, a.id.0);
    }
    assert_eq!(kv_deltas, [(1079, 18.0, 500.0), (5052, 22.0, 115.0)]);

    // Loads are unchanged between the snapshots: per bus totals match the
    // .m bus table at its print precision (2 decimals).
    let mut pwb_load: BTreeMap<usize, (f64, f64)> = BTreeMap::default();
    for l in &pwb.loads {
        let e = pwb_load
            .entry(m_id_by_pwb_id[&l.bus.0])
            .or_insert((0.0, 0.0));
        e.0 += l.p;
        e.1 += l.q;
    }
    let mut m_load: BTreeMap<usize, (f64, f64)> = BTreeMap::default();
    for l in &m.loads {
        let e = m_load.entry(l.bus.0).or_insert((0.0, 0.0));
        e.0 += l.p;
        e.1 += l.q;
    }
    assert_eq!(pwb_load.len(), m_load.len());
    for (bus, (p, q)) in &pwb_load {
        let (mp, mq) = m_load[bus];
        assert!(
            (p - mp).abs() <= 5e-3 + 1e-6 * mp.abs(),
            "load at m bus {bus}: {p} vs {mp}"
        );
        assert!(
            (q - mq).abs() <= 5e-3 + 1e-6 * mq.abs(),
            "load q at m bus {bus}: {q} vs {mq}"
        );
    }

    // Branch identity: endpoints mapped through the bus names. The .m
    // carries no circuit IDs, so parallel branches pair up within an
    // endpoint group sorted by impedance. Snapshot deltas are pinned.
    let pair = |a: usize, b: usize| (a.min(b), a.max(b));
    let mut m_by_pair: BTreeMap<(usize, usize), Vec<&powerio::Branch>> = BTreeMap::default();
    for b in &m.branches {
        m_by_pair.entry(pair(b.from.0, b.to.0)).or_default().push(b);
    }
    let mut p_by_pair: BTreeMap<(usize, usize), Vec<&powerio::Branch>> = BTreeMap::default();
    for p in &pwb.branches {
        p_by_pair
            .entry(pair(m_id_by_pwb_id[&p.from.0], m_id_by_pwb_id[&p.to.0]))
            .or_default()
            .push(p);
    }
    let by_imp = |a: &&powerio::Branch, b: &&powerio::Branch| {
        a.x.total_cmp(&b.x)
            .then(a.r.total_cmp(&b.r))
            .then(a.b.total_cmp(&b.b))
            // Parallel units can share impedances; break the tie by kind.
            .then(a.is_transformer().cmp(&b.is_transformer()))
    };
    let mut count_deltas = Vec::new();
    let mut imp_deltas = Vec::new();
    let mut kind_deltas = Vec::new();
    let mut matched = 0;
    for (k, mut pv) in p_by_pair {
        let mut mv = m_by_pair.remove(&k).unwrap_or_default();
        if pv.len() != mv.len() {
            count_deltas.push((k.0, k.1, pv.len(), mv.len()));
        }
        pv.sort_by(by_imp);
        mv.sort_by(by_imp);
        for (p, a) in pv.iter().zip(&mv) {
            matched += 1;
            // The published .m prints impedances at 5 decimals.
            for (x, y, what) in [(p.r, a.r, "R"), (p.x, a.x, "X"), (p.b, a.b, "B")] {
                if (x - y).abs() > 5.1e-6 + 1.5e-7 * y.abs() {
                    imp_deltas.push((k.0, k.1, what, x, y));
                }
            }
            if p.is_transformer() != a.is_transformer() {
                kind_deltas.push((k.0, k.1, p.tap));
            }
        }
    }
    // Endpoint pairs only in the .m: branches added after the v19 snapshot.
    let added_later: Vec<_> = m_by_pair.keys().copied().collect();
    // The published revision added two parallel circuits and dropped three
    // v19 branches (per pair counts: pwb vs .m).
    assert_eq!(
        count_deltas,
        [
            (3048, 5045, 1, 2),
            (5018, 5236, 1, 2),
            (5050, 8038, 1, 0),
            (5258, 8108, 1, 0),
            (5454, 8124, 1, 0),
        ],
        "per pair count deltas"
    );
    // The new endpoint pairs rewire the same buses the revision re-leveled
    // (1079, 5052): 3206 published = 3199 matched + 2 extra parallels + 5
    // branches at these four new pairs.
    assert_eq!(
        added_later,
        [(1079, 3048), (5052, 8038), (5052, 8124), (8108, 8153)],
        "pairs only in the .m"
    );
    assert_eq!(matched, 3199);
    // Impedance revisions in the same two regions the revision rewired.
    let imp_keys: Vec<_> = imp_deltas.iter().map(|&(a, b, w, ..)| (a, b, w)).collect();
    assert_eq!(
        imp_keys,
        [
            (1071, 1079, "R"),
            (1071, 1079, "X"),
            (5049, 5050, "R"),
            (5049, 5050, "X"),
        ],
        "{imp_deltas:?}"
    );
    assert_eq!(kind_deltas, Vec::<(usize, usize, f64)>::new());

    // Dispatch and shunt schedules moved between the snapshots; the
    // generator placement still has to line up. The one extra v19 machine
    // sits at bus 5052, the bus the revision re-leveled and rewired.
    let m_gen_buses: BTreeSet<usize> = m.generators.iter().map(|g| g.bus.0).collect();
    let pwb_gen_buses: BTreeSet<usize> = pwb
        .generators
        .iter()
        .map(|g| m_id_by_pwb_id[&g.bus.0])
        .collect();
    let pwb_only: Vec<_> = pwb_gen_buses.difference(&m_gen_buses).copied().collect();
    assert_eq!(pwb_only, [5052], "gen buses only in the pwb");
    assert!(
        m_gen_buses.is_subset(&pwb_gen_buses),
        "gen buses only in the .m"
    );
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

    // An undecoded writer format constant (the 2017 era 118 bus sample
    // carries 338 at offset 0x08) is a vintage rejection naming the
    // constant, never a generic magic mismatch.
    let mut newer = Vec::new();
    newer.extend_from_slice(&15000u64.to_le_bytes());
    newer.extend_from_slice(&338u64.to_le_bytes());
    newer.extend_from_slice(&20u64.to_le_bytes());
    newer.extend_from_slice(&[0u8; 4096]);
    let err = parse_pwb(&newer, None).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("unsupported PowerWorld .pwb vintage") && msg.contains("338"),
        "{err}"
    );

    // A decoded constant over a garbage body dies at the bus layout gate,
    // through the same loud vintage path; pinned for each constant the
    // header gate admits.
    for v in [425u64, 483, 508, 537, 550, 551] {
        let mut garbage = Vec::new();
        garbage.extend_from_slice(&15000u64.to_le_bytes());
        garbage.extend_from_slice(&v.to_le_bytes());
        garbage.extend_from_slice(&20u64.to_le_bytes());
        garbage.extend_from_slice(&[0u8; 4096]);
        let err = parse_pwb(&garbage, None).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("unsupported PowerWorld .pwb vintage")
                && msg.contains("no recognized bus record layout"),
            "constant {v}: {err}"
        );
    }
}

/// RTS-GMLC (NREL/GMLC Reliability Test System): the first cross format
/// oracle outside the TAMU cases and outside aux exports entirely. The .PWB
/// (Simulator 19 era record family) checks against the .m and .RAW siblings
/// from the same repository commit. Fetched fixtures; skipped when absent.
#[test]
fn rts_gmlc_pwb_matches_its_matpower_and_raw_siblings() {
    use common::rts_gmlc_fetched as rts;
    let (Some(pwb_path), Some(m_path), Some(raw_path)) =
        (rts("RTS-GMLC.PWB"), rts("RTS_GMLC.m"), rts("RTS-GMLC.RAW"))
    else {
        eprintln!("skipped: run benchmarks/fetch_powerworld.sh");
        return;
    };
    let pwb = read_pwb(&pwb_path);
    let m = parse_file(m_path, None).unwrap().network;
    let raw = parse_file(raw_path, None).unwrap().network;

    assert_eq!(pwb.buses.len(), 73);
    assert_eq!(m.buses.len(), 73);
    assert_eq!(raw.buses.len(), 73);
    assert_eq!(pwb.branches.len(), 120);
    assert_eq!(m.branches.len(), 120);

    // Bus identity by number (RTS-96 numbering, no renumbering between
    // formats), voltage level, and the solved state against the .RAW.
    let m_bus: BTreeMap<usize, &powerio::Bus> = m.buses.iter().map(|b| (b.id.0, b)).collect();
    let raw_bus: BTreeMap<usize, &powerio::Bus> = raw.buses.iter().map(|b| (b.id.0, b)).collect();
    for p in &pwb.buses {
        let a = m_bus[&p.id.0];
        assert!((p.base_kv - a.base_kv).abs() < 1e-4, "bus {} kV", p.id);
        assert_eq!((p.area, p.zone), (a.area, a.zone), "bus {}", p.id);
        let r = raw_bus[&p.id.0];
        assert!(
            (p.vm - r.vm).abs() <= 5e-6,
            "bus {} vm {} vs {}",
            p.id,
            p.vm,
            r.vm
        );
    }

    // Branches grouped by endpoint pair against the .m (which carries no
    // circuit IDs); parallel units zip within a pair sorted by impedance.
    let pair = |a: usize, b: usize| (a.min(b), a.max(b));
    let mut m_by_pair: BTreeMap<(usize, usize), Vec<&powerio::Branch>> = BTreeMap::default();
    for b in &m.branches {
        m_by_pair.entry(pair(b.from.0, b.to.0)).or_default().push(b);
    }
    let mut p_by_pair: BTreeMap<(usize, usize), Vec<&powerio::Branch>> = BTreeMap::default();
    for p in &pwb.branches {
        p_by_pair.entry(pair(p.from.0, p.to.0)).or_default().push(p);
    }
    assert_eq!(p_by_pair.len(), m_by_pair.len());
    let by_imp = |a: &&powerio::Branch, b: &&powerio::Branch| {
        a.x.total_cmp(&b.x)
            .then(a.r.total_cmp(&b.r))
            .then(a.b.total_cmp(&b.b))
            .then(a.is_transformer().cmp(&b.is_transformer()))
    };
    let mut transformers = 0;
    let mut kind_deltas = Vec::new();
    for (k, mut pv) in p_by_pair {
        let mut mv = m_by_pair
            .remove(&k)
            .unwrap_or_else(|| panic!("{k:?} not in the .m"));
        assert_eq!(pv.len(), mv.len(), "{k:?} parallel count");
        pv.sort_by(by_imp);
        mv.sort_by(by_imp);
        for (p, a) in pv.iter().zip(&mv) {
            assert!(
                (p.r - a.r).abs() <= 5.1e-6 + 1.5e-7 * a.r.abs(),
                "{k:?} R {} vs {}",
                p.r,
                a.r
            );
            assert!(
                (p.x - a.x).abs() <= 5.1e-6 + 1.5e-7 * a.x.abs(),
                "{k:?} X {} vs {}",
                p.x,
                a.x
            );
            assert!(
                (p.b - a.b).abs() <= 5.1e-6 + 1.5e-7 * a.b.abs(),
                "{k:?} B {} vs {}",
                p.b,
                a.b
            );
            assert!(
                (p.rate_a - a.rate_a).abs() <= 1e-3 + 1e-6 * a.rate_a.abs(),
                "{k:?} rate_a {} vs {}",
                p.rate_a,
                a.rate_a
            );
            if p.is_transformer() != a.is_transformer() {
                kind_deltas.push((k.0, k.1, p.tap, a.tap));
            }
            transformers += usize::from(p.is_transformer());
        }
    }
    assert!(
        m_by_pair.is_empty(),
        "pairs only in the .m: {:?}",
        m_by_pair.keys()
    );
    // The unit tap ambiguity, the other way around: the .PWB stores 323-325
    // as a line device where the .m writes ratio 1.0.
    assert_eq!(kind_deltas, [(323, 325, 0.0, 1.0)]);
    assert_eq!(transformers, 15);

    // Generator placement against the .m.
    let m_gen_buses: BTreeSet<usize> = m.generators.iter().map(|g| g.bus.0).collect();
    let p_gen_buses: BTreeSet<usize> = pwb.generators.iter().map(|g| g.bus.0).collect();
    assert_eq!(p_gen_buses, m_gen_buses);
}

/// The hub surface: `parse_file` dispatches `.pwb` by extension and by the
/// explicit `pwb` source name, and the network converts onward (the CLI's
/// `powerio convert ACTIVSg200.pwb out.m` path).
#[test]
fn parse_file_dispatches_pwb_and_converts() {
    let net = parse_file(vendored("ACTIVSg200.pwb"), None)
        .unwrap()
        .network;
    assert_eq!(net.buses.len(), 200);
    assert_eq!(net.branches.len(), 246);
    let by_name = parse_file(vendored("ACTIVSg200.pwb"), Some("pwb"))
        .unwrap()
        .network;
    assert_eq!(by_name.buses.len(), 200);

    let conv = powerio::write_as(&net, powerio::TargetFormat::Matpower);
    let back = powerio::parse_str(&conv.text, "matpower").unwrap().network;
    assert_eq!(back.buses.len(), 200);
    assert_eq!(back.branches.len(), 246);
}

/// The published ACTIVSg2000 set's current era export (bus flag bits 6/8)
/// against the aux from the same set: exact counts, values at the print
/// precision of the lower precision side, the same bar as the other decoded
/// vintages. Machine specific corpus file; skipped unless the local manifest
/// lists it next to its aux sibling.
#[test]
#[allow(clippy::too_many_lines)]
fn activsg2000_current_era_pwb_matches_its_aux_sibling() {
    let Some(pwb_path) = common::local_corpus_path("ACTIVSg2000 published set export (local only)")
    else {
        eprintln!("skipped: not in the local corpus manifest");
        return;
    };
    let aux_path = pwb_path.with_file_name("ACTIVSg2000.aux");
    if !aux_path.exists() {
        eprintln!("skipped: no aux sibling next to the export");
        return;
    }
    let pwb = read_pwb(&pwb_path);
    let aux = parse_file(aux_path, None).unwrap().network;

    assert_eq!(pwb.buses.len(), 2000);
    assert_eq!(pwb.branches.len(), 3206);
    assert_eq!(pwb.buses.len(), aux.buses.len());
    assert_eq!(pwb.loads.len(), aux.loads.len());
    assert_eq!(pwb.generators.len(), aux.generators.len());
    assert_eq!(pwb.shunts.len(), aux.shunts.len());
    assert_eq!(pwb.branches.len(), aux.branches.len());

    for (p, a) in pwb.buses.iter().zip(&aux.buses) {
        assert_eq!(p.id, a.id);
        assert_eq!(p.name, a.name);
        assert!((p.base_kv - a.base_kv).abs() < 1e-4, "bus {} kV", p.id);
        assert_eq!((p.area, p.zone), (a.area, a.zone), "bus {}", p.id);
        assert!(
            (p.vm - a.vm).abs() <= 5e-7,
            "bus {} vm {} vs {}",
            p.id,
            p.vm,
            a.vm
        );
        assert!(
            (p.va - a.va).abs() <= 5e-5,
            "bus {} va {} vs {}",
            p.id,
            p.va,
            a.va
        );
    }
    for (p, a) in pwb.loads.iter().zip(&aux.loads) {
        assert_eq!(p.bus, a.bus);
        assert!(
            (p.p - a.p).abs() <= 1e-3,
            "load at {}: {} vs {}",
            p.bus,
            p.p,
            a.p
        );
        assert!(
            (p.q - a.q).abs() <= 1e-3,
            "load q at {}: {} vs {}",
            p.bus,
            p.q,
            a.q
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
                (x - y).abs() <= 1e-3 + 1e-6 * y.abs(),
                "gen at {} {what}: {x} vs {y}",
                p.bus
            );
        }
    }
    for (p, a) in pwb.shunts.iter().zip(&aux.shunts) {
        assert_eq!(p.bus, a.bus);
        assert!(
            (p.b - a.b).abs() <= 1e-3,
            "shunt at {}: {} vs {}",
            p.bus,
            p.b,
            a.b
        );
    }
    let mut aux_by_id: BTreeMap<(usize, usize, String), &powerio::Branch> = BTreeMap::default();
    for b in &aux.branches {
        aux_by_id.insert((b.from.0, b.to.0, ckt(b)), b);
    }
    for p in &pwb.branches {
        let key = (p.from.0, p.to.0, ckt(p));
        let a = aux_by_id
            .remove(&key)
            .unwrap_or_else(|| panic!("{key:?} not in aux"));
        assert!((p.r - a.r).abs() <= 5e-7, "{key:?} R {} vs {}", p.r, a.r);
        assert!((p.x - a.x).abs() <= 5e-7, "{key:?} X {} vs {}", p.x, a.x);
        assert!((p.b - a.b).abs() <= 5e-7, "{key:?} B {} vs {}", p.b, a.b);
        assert!(
            (p.rate_a - a.rate_a).abs() <= 1e-3 + 1e-6 * a.rate_a.abs(),
            "{key:?} rate_a {} vs {}",
            p.rate_a,
            a.rate_a
        );
        assert!(
            (p.rate_b - a.rate_b).abs() <= 1e-3 + 1e-6 * a.rate_b.abs(),
            "{key:?} rate_b {} vs {}",
            p.rate_b,
            a.rate_b
        );
        assert!(
            (p.effective_tap() - a.effective_tap()).abs() < 1e-6,
            "{key:?} tap {} vs {}",
            p.effective_tap(),
            a.effective_tap()
        );
        assert_eq!(p.is_transformer(), a.is_transformer(), "{key:?} kind");
    }
    assert!(aux_by_id.is_empty(), "aux branches missing: {aux_by_id:?}");
}

/// ACTIVSg500's current era export against its same set aux. This is the
/// file whose bit 4 branch records carry the huge tail blobs (per bus f64
/// vectors, contingency label text), so it pins the unbounded tail resync
/// as well as the values. Machine specific corpus file; skipped unless the
/// local manifest lists it next to its aux sibling.
#[test]
#[allow(clippy::too_many_lines)]
fn activsg500_pwb_matches_its_aux_sibling() {
    let Some(pwb_path) = common::local_corpus_path("ACTIVSg500 export (local only)") else {
        eprintln!("skipped: not in the local corpus manifest");
        return;
    };
    let aux_path = pwb_path.with_file_name("ACTIVSg500.aux");
    if !aux_path.exists() {
        eprintln!("skipped: no aux sibling next to the export");
        return;
    }
    let pwb = read_pwb(&pwb_path);
    let aux = parse_file(aux_path, None).unwrap().network;

    assert_eq!(pwb.buses.len(), 500);
    assert_eq!(pwb.branches.len(), 599);
    assert_eq!(pwb.loads.len(), aux.loads.len());
    assert_eq!(pwb.generators.len(), aux.generators.len());
    assert_eq!(pwb.shunts.len(), aux.shunts.len());
    assert_eq!(pwb.branches.len(), aux.branches.len());

    for (p, a) in pwb.buses.iter().zip(&aux.buses) {
        assert_eq!(p.id, a.id);
        assert_eq!(p.name, a.name);
        assert_eq!((p.area, p.zone), (a.area, a.zone), "bus {}", p.id);
        assert!((p.base_kv - a.base_kv).abs() < 1e-4, "bus {} kV", p.id);
        assert!(
            (p.vm - a.vm).abs() <= 5e-7,
            "bus {} vm {} vs {}",
            p.id,
            p.vm,
            a.vm
        );
        assert!(
            (p.va - a.va).abs() <= 5e-5,
            "bus {} va {} vs {}",
            p.id,
            p.va,
            a.va
        );
    }
    for (p, a) in pwb.loads.iter().zip(&aux.loads) {
        assert_eq!(p.bus, a.bus);
        assert!(
            (p.p - a.p).abs() <= 1e-3,
            "load at {}: {} vs {}",
            p.bus,
            p.p,
            a.p
        );
        assert!(
            (p.q - a.q).abs() <= 1e-3,
            "load q at {}: {} vs {}",
            p.bus,
            p.q,
            a.q
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
                (x - y).abs() <= 1e-3 + 1e-6 * y.abs(),
                "gen at {} {what}: {x} vs {y}",
                p.bus
            );
        }
    }
    for (p, a) in pwb.shunts.iter().zip(&aux.shunts) {
        assert_eq!(p.bus, a.bus);
        assert!(
            (p.b - a.b).abs() <= 1e-3,
            "shunt at {}: {} vs {}",
            p.bus,
            p.b,
            a.b
        );
    }
    let mut aux_by_id: BTreeMap<(usize, usize, String), &powerio::Branch> = BTreeMap::default();
    for b in &aux.branches {
        aux_by_id.insert((b.from.0, b.to.0, ckt(b)), b);
    }
    for p in &pwb.branches {
        let key = (p.from.0, p.to.0, ckt(p));
        let a = aux_by_id
            .remove(&key)
            .unwrap_or_else(|| panic!("{key:?} not in aux"));
        assert!((p.r - a.r).abs() <= 5e-7, "{key:?} R {} vs {}", p.r, a.r);
        assert!((p.x - a.x).abs() <= 5e-7, "{key:?} X {} vs {}", p.x, a.x);
        assert!((p.b - a.b).abs() <= 5e-7, "{key:?} B {} vs {}", p.b, a.b);
        assert!(
            (p.rate_a - a.rate_a).abs() <= 1e-3 + 1e-6 * a.rate_a.abs(),
            "{key:?} rate_a {} vs {}",
            p.rate_a,
            a.rate_a
        );
        assert!(
            (p.rate_b - a.rate_b).abs() <= 1e-3 + 1e-6 * a.rate_b.abs(),
            "{key:?} rate_b {} vs {}",
            p.rate_b,
            a.rate_b
        );
        assert!(
            (p.effective_tap() - a.effective_tap()).abs() < 1e-6,
            "{key:?} tap {} vs {}",
            p.effective_tap(),
            a.effective_tap()
        );
        assert_eq!(p.is_transformer(), a.is_transformer(), "{key:?} kind");
    }
    assert!(aux_by_id.is_empty(), "aux branches missing: {aux_by_id:?}");
}

/// Hawaii40 (2022, header format constant 508) against its same set aux:
/// the parity evidence that admits the 508 header era. Machine specific
/// corpus file; skipped unless the local manifest lists it.
#[test]
#[allow(clippy::too_many_lines)]
fn hawaii40_pwb_matches_its_aux_sibling() {
    let Some(pwb_path) = common::local_corpus_path("Hawaii40 2022 export (local only)") else {
        eprintln!("skipped: not in the local corpus manifest");
        return;
    };
    let aux_path = pwb_path.with_file_name("Hawaii40_220906.AUX");
    if !aux_path.exists() {
        eprintln!("skipped: no aux sibling next to the export");
        return;
    }
    let pwb = read_pwb(&pwb_path);
    let aux = parse_file(aux_path, None).unwrap().network;

    assert_eq!(pwb.buses.len(), aux.buses.len());
    assert_eq!(pwb.loads.len(), aux.loads.len());
    assert_eq!(pwb.generators.len(), aux.generators.len());
    assert_eq!(pwb.shunts.len(), aux.shunts.len());
    assert_eq!(pwb.branches.len(), aux.branches.len());

    for (p, a) in pwb.buses.iter().zip(&aux.buses) {
        assert_eq!(p.id, a.id);
        assert_eq!(p.name, a.name);
        assert_eq!((p.area, p.zone), (a.area, a.zone), "bus {}", p.id);
        assert!((p.base_kv - a.base_kv).abs() < 1e-4, "bus {} kV", p.id);
        assert!(
            (p.vm - a.vm).abs() <= 5e-7,
            "bus {} vm {} vs {}",
            p.id,
            p.vm,
            a.vm
        );
        assert!(
            (p.va - a.va).abs() <= 5e-5,
            "bus {} va {} vs {}",
            p.id,
            p.va,
            a.va
        );
    }
    for (p, a) in pwb.loads.iter().zip(&aux.loads) {
        assert_eq!(p.bus, a.bus);
        assert!(
            (p.p - a.p).abs() <= 1e-3,
            "load at {}: {} vs {}",
            p.bus,
            p.p,
            a.p
        );
        assert!(
            (p.q - a.q).abs() <= 1e-3,
            "load q at {}: {} vs {}",
            p.bus,
            p.q,
            a.q
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
                (x - y).abs() <= 1e-3 + 1e-6 * y.abs(),
                "gen at {} {what}: {x} vs {y}",
                p.bus
            );
        }
    }
    for (p, a) in pwb.shunts.iter().zip(&aux.shunts) {
        assert_eq!(p.bus, a.bus);
        assert!(
            (p.b - a.b).abs() <= 1e-3,
            "shunt at {}: {} vs {}",
            p.bus,
            p.b,
            a.b
        );
    }
    let mut aux_by_id: BTreeMap<(usize, usize, String), &powerio::Branch> = BTreeMap::default();
    for b in &aux.branches {
        aux_by_id.insert((b.from.0, b.to.0, ckt(b)), b);
    }
    for p in &pwb.branches {
        let key = (p.from.0, p.to.0, ckt(p));
        let a = aux_by_id
            .remove(&key)
            .unwrap_or_else(|| panic!("{key:?} not in aux"));
        assert!((p.r - a.r).abs() <= 5e-7, "{key:?} R {} vs {}", p.r, a.r);
        assert!((p.x - a.x).abs() <= 5e-7, "{key:?} X {} vs {}", p.x, a.x);
        assert!((p.b - a.b).abs() <= 5e-7, "{key:?} B {} vs {}", p.b, a.b);
        assert!(
            (p.rate_a - a.rate_a).abs() <= 1e-3 + 1e-6 * a.rate_a.abs(),
            "{key:?} rate_a {} vs {}",
            p.rate_a,
            a.rate_a
        );
        assert!(
            (p.rate_b - a.rate_b).abs() <= 1e-3 + 1e-6 * a.rate_b.abs(),
            "{key:?} rate_b {} vs {}",
            p.rate_b,
            a.rate_b
        );
        assert!(
            (p.effective_tap() - a.effective_tap()).abs() < 1e-6,
            "{key:?} tap {} vs {}",
            p.effective_tap(),
            a.effective_tap()
        );
        assert_eq!(p.is_transformer(), a.is_transformer(), "{key:?} kind");
    }
    assert!(aux_by_id.is_empty(), "aux branches missing: {aux_by_id:?}");
}

/// Texas7k (2021, header format constant 483) against its same day aux and
/// MATPOWER siblings: the parity evidence that admits the 483 header era,
/// at 6717 bus scale. The case carries the corpus's first out of service
/// devices (94 open machines), so the generator in service bit is asserted
/// here against both siblings, not just read as a default. The .m exporter
/// prints at fixed decimals and writes the solved regulated bus voltage
/// into VG, so voltage setpoints are asserted against the aux and the .m
/// carries the topology, dispatch, and status parity. Machine specific
/// corpus file; skipped unless the local manifest lists it.
#[test]
#[allow(clippy::too_many_lines)]
fn texas7k_pwb_matches_its_aux_and_matpower_siblings() {
    let Some(pwb_path) = common::local_corpus_path("Texas7k 2021 export (local only)") else {
        eprintln!("skipped: not in the local corpus manifest");
        return;
    };
    let aux_path = pwb_path.with_file_name("Texas7k_20210804.AUX");
    let m_path = pwb_path.with_file_name("Texas7k_20210804.m");
    if !aux_path.exists() || !m_path.exists() {
        eprintln!("skipped: no aux/.m siblings next to the export");
        return;
    }
    let pwb = read_pwb(&pwb_path);
    let aux = parse_file(aux_path, None).unwrap().network;
    let m = parse_file(m_path, None).unwrap().network;

    assert_eq!(pwb.buses.len(), 6717);
    assert_eq!(pwb.loads.len(), 5095);
    assert_eq!(pwb.generators.len(), 731);
    assert_eq!(pwb.shunts.len(), 634);
    assert_eq!(pwb.branches.len(), 9140);
    assert_eq!(aux.buses.len(), pwb.buses.len());
    assert_eq!(aux.loads.len(), pwb.loads.len());
    assert_eq!(aux.generators.len(), pwb.generators.len());
    assert_eq!(aux.shunts.len(), pwb.shunts.len());
    assert_eq!(aux.branches.len(), pwb.branches.len());
    assert_eq!(m.buses.len(), pwb.buses.len());
    assert_eq!(m.generators.len(), pwb.generators.len());
    assert_eq!(m.branches.len(), pwb.branches.len());

    // Buses: identity and the f64 solved state against the aux; the .m
    // prints vm at 7 and va at 6 decimals.
    let m_bus: BTreeMap<usize, &powerio::Bus> = m.buses.iter().map(|b| (b.id.0, b)).collect();
    for (p, a) in pwb.buses.iter().zip(&aux.buses) {
        assert_eq!(p.id, a.id);
        assert_eq!(p.name, a.name);
        assert!((p.base_kv - a.base_kv).abs() < 1e-4, "bus {} kV", p.id);
        assert_eq!((p.area, p.zone), (a.area, a.zone), "bus {}", p.id);
        assert!((p.vm - a.vm).abs() < 1e-9, "bus {} vm", p.id);
        assert!((p.va - a.va).abs() < 1e-7, "bus {} va", p.id);
        let mb = m_bus[&p.id.0];
        assert!(
            (p.base_kv - mb.base_kv).abs() < 1e-4,
            "bus {} kV vs .m",
            p.id
        );
        assert!((p.vm - mb.vm).abs() <= 1e-6, "bus {} vm vs .m", p.id);
        assert!((p.va - mb.va).abs() <= 1e-5, "bus {} va vs .m", p.id);
    }
    // The binary stores no slack flag and no bus type; the .m types every
    // generator bus PV whether or not its machines are in service, while
    // the derived pwb kinds mark only buses with an in service machine.
    // The electrical values above are the bus parity contract.
    assert!(pwb.buses.iter().all(|b| b.kind != powerio::BusType::Ref));

    // Loads against the aux (f32 storage); per bus totals against the .m
    // bus rows (printed at 2 decimals).
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
        assert!(p.in_service, "load at {} in service", p.bus);
    }
    let mut pwb_pd: BTreeMap<usize, (f64, f64)> = BTreeMap::default();
    for l in &pwb.loads {
        let e = pwb_pd.entry(l.bus.0).or_default();
        e.0 += l.p;
        e.1 += l.q;
    }
    let mut m_pd: BTreeMap<usize, (f64, f64)> = BTreeMap::default();
    for l in &m.loads {
        let e = m_pd.entry(l.bus.0).or_default();
        e.0 += l.p;
        e.1 += l.q;
    }
    for (bus, (p, q)) in &pwb_pd {
        let (mp, mq) = m_pd.remove(bus).unwrap_or((0.0, 0.0));
        assert!(
            (p - mp).abs() <= 5.1e-3 + 1e-5 * mp.abs(),
            "bus {bus} Pd {p} vs .m {mp}"
        );
        assert!(
            (q - mq).abs() <= 5.1e-3 + 1e-5 * mq.abs(),
            "bus {bus} Qd {q} vs .m {mq}"
        );
    }
    assert!(
        m_pd.values().all(|(p, q)| *p == 0.0 && *q == 0.0),
        "loads only in the .m: {m_pd:?}"
    );

    // Generators against the aux, including the in service bit: this case
    // is the first with open machines, and the binary's status byte is
    // validated here on all 637 + 94 of them.
    for (p, a) in pwb.generators.iter().zip(&aux.generators) {
        assert_eq!(p.bus, a.bus);
        assert_eq!(p.in_service, a.in_service, "gen at {} status", p.bus);
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
    assert_eq!(pwb.generators.iter().filter(|g| !g.in_service).count(), 94);
    // Placement, per bus dispatch, and statuses against the .m (printed at
    // 2 decimals; machine order within a bus is not preserved, so per bus
    // aggregates are the contract).
    #[derive(Default)]
    #[allow(clippy::items_after_statements)]
    struct BusGens {
        n: usize,
        on: usize,
        pg: f64,
        pmax: f64,
    }
    let aggregate = |gens: &[powerio::Generator]| {
        let mut by: BTreeMap<usize, BusGens> = BTreeMap::default();
        for g in gens {
            let e = by.entry(g.bus.0).or_default();
            e.n += 1;
            e.on += usize::from(g.in_service);
            e.pg += g.pg;
            e.pmax += g.pmax;
        }
        by
    };
    let pwb_gen = aggregate(&pwb.generators);
    let m_gen = aggregate(&m.generators);
    assert_eq!(
        pwb_gen.keys().collect::<Vec<_>>(),
        m_gen.keys().collect::<Vec<_>>()
    );
    for (bus, p) in &pwb_gen {
        let a = &m_gen[bus];
        assert_eq!((p.n, p.on), (a.n, a.on), "gen counts at {bus}");
        assert!(
            (p.pg - a.pg).abs() <= 5.1e-3 * p.n as f64 + 1e-5 * a.pg.abs(),
            "bus {bus} pg {} vs .m {}",
            p.pg,
            a.pg
        );
        assert!(
            (p.pmax - a.pmax).abs() <= 5.1e-3 * p.n as f64 + 1e-5 * a.pmax.abs(),
            "bus {bus} pmax {} vs .m {}",
            p.pmax,
            a.pmax
        );
    }

    // Shunts against the aux; per bus totals against the .m.
    for (p, a) in pwb.shunts.iter().zip(&aux.shunts) {
        assert_eq!(p.bus, a.bus);
        assert!(
            (p.b - a.b).abs() < 1e-4 * a.b.abs().max(1.0),
            "shunt at {}",
            p.bus
        );
    }
    let mut pwb_bs: BTreeMap<usize, f64> = BTreeMap::default();
    for s in &pwb.shunts {
        *pwb_bs.entry(s.bus.0).or_default() += s.b;
    }
    let mut m_bs: BTreeMap<usize, f64> = BTreeMap::default();
    for s in &m.shunts {
        *m_bs.entry(s.bus.0).or_default() += s.b;
    }
    for (bus, b) in &pwb_bs {
        let mb = m_bs.remove(bus).unwrap_or(0.0);
        assert!(
            (b - mb).abs() <= 5.1e-3 + 1e-5 * mb.abs(),
            "bus {bus} Bs {b} vs .m {mb}"
        );
    }
    assert!(
        m_bs.values().all(|b| *b == 0.0),
        "shunts only in the .m: {m_bs:?}"
    );

    // Branches against the aux by full identity (endpoints + circuit).
    let mut aux_by_id: BTreeMap<(usize, usize, String), &powerio::Branch> = BTreeMap::default();
    for b in &aux.branches {
        aux_by_id.insert((b.from.0, b.to.0, ckt(b)), b);
    }
    for p in &pwb.branches {
        let key = (p.from.0, p.to.0, ckt(p));
        let a = aux_by_id
            .remove(&key)
            .unwrap_or_else(|| panic!("{key:?} not in aux"));
        let tol = |v: f64| 5e-7 + 1e-6 * v.abs();
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
            (p.rate_a - a.rate_a).abs() <= 1e-3 + 1e-6 * a.rate_a.abs(),
            "{key:?} rate_a {} vs {}",
            p.rate_a,
            a.rate_a
        );
        assert!(
            (p.tap - a.tap).abs() <= 5e-7 + 1e-6 * a.tap.abs(),
            "{key:?} tap {} vs {}",
            p.tap,
            a.tap
        );
        assert_eq!(p.is_transformer(), a.is_transformer(), "{key:?} kind");
        assert!(p.in_service, "{key:?} in service");
    }
    assert!(aux_by_id.is_empty(), "aux branches missing: {aux_by_id:?}");

    // Topology against the .m: endpoint pairs with parallel counts (the .m
    // carries no circuit IDs; parallel units zip within a pair sorted by
    // impedance), every branch in service on both sides.
    let pair = |a: usize, b: usize| (a.min(b), a.max(b));
    let mut m_by_pair: BTreeMap<(usize, usize), Vec<&powerio::Branch>> = BTreeMap::default();
    for b in &m.branches {
        assert!(b.in_service);
        m_by_pair.entry(pair(b.from.0, b.to.0)).or_default().push(b);
    }
    let mut p_by_pair: BTreeMap<(usize, usize), Vec<&powerio::Branch>> = BTreeMap::default();
    for p in &pwb.branches {
        p_by_pair.entry(pair(p.from.0, p.to.0)).or_default().push(p);
    }
    assert_eq!(p_by_pair.len(), m_by_pair.len());
    let by_imp = |a: &&powerio::Branch, b: &&powerio::Branch| {
        a.x.total_cmp(&b.x)
            .then(a.r.total_cmp(&b.r))
            .then(a.b.total_cmp(&b.b))
    };
    for (k, mut pv) in p_by_pair {
        let mut mv = m_by_pair
            .remove(&k)
            .unwrap_or_else(|| panic!("{k:?} not in the .m"));
        assert_eq!(pv.len(), mv.len(), "{k:?} parallel count");
        pv.sort_by(by_imp);
        mv.sort_by(by_imp);
        for (p, a) in pv.iter().zip(&mv) {
            assert!(
                (p.r - a.r).abs() <= 5.1e-7 + 1.5e-6 * a.r.abs(),
                "{k:?} R {} vs .m {}",
                p.r,
                a.r
            );
            assert!(
                (p.x - a.x).abs() <= 5.1e-7 + 1.5e-6 * a.x.abs(),
                "{k:?} X {} vs .m {}",
                p.x,
                a.x
            );
            assert!(
                (p.b - a.b).abs() <= 5.1e-6 + 1.5e-6 * a.b.abs(),
                "{k:?} B {} vs .m {}",
                p.b,
                a.b
            );
            assert!(
                (p.rate_a - a.rate_a).abs() <= 5.1e-3 + 1e-5 * a.rate_a.abs(),
                "{k:?} rate_a {} vs .m {}",
                p.rate_a,
                a.rate_a
            );
        }
    }
    assert!(
        m_by_pair.is_empty(),
        "pairs only in the .m: {:?}",
        m_by_pair.keys()
    );
}

/// The Texas7k v21 and v22 resaves (header format constants 508 and 551)
/// against the same day 2022 aux: the parity evidence that admits those
/// constants for the node level case family. The v21 save is also the
/// evidence that bus flag bit 6 is a per record presence bit (the slack
/// bus record clears it) rather than a file constant. Machine specific
/// corpus files; skipped unless the local manifest lists them.
#[test]
fn texas7k_resaves_match_the_2022_aux() {
    let labels = [
        "Texas7k saved as v21 (local only)",
        "Texas7k saved as v22 (local only)",
    ];
    for label in labels {
        let Some(pwb_path) = common::local_corpus_path(label) else {
            eprintln!("skipped {label}: not in the local corpus manifest");
            continue;
        };
        let aux_path = pwb_path.with_file_name("Texas7k_20220923.AUX");
        if !aux_path.exists() {
            eprintln!("skipped {label}: no aux sibling next to the export");
            continue;
        }
        let pwb = read_pwb(&pwb_path);
        let aux = parse_file(aux_path, None).unwrap().network;

        assert_eq!(pwb.buses.len(), 6717, "{label}");
        assert_eq!(pwb.loads.len(), aux.loads.len(), "{label}");
        assert_eq!(pwb.generators.len(), aux.generators.len(), "{label}");
        assert_eq!(pwb.shunts.len(), aux.shunts.len(), "{label}");
        assert_eq!(pwb.branches.len(), 9140, "{label}");
        assert_eq!(aux.branches.len(), 9140, "{label}");

        for (p, a) in pwb.buses.iter().zip(&aux.buses) {
            assert_eq!(p.id, a.id, "{label}");
            assert!((p.vm - a.vm).abs() < 1e-6, "{label} bus {} vm", p.id);
            assert!((p.va - a.va).abs() < 1e-4, "{label} bus {} va", p.id);
        }
        for (p, a) in pwb.loads.iter().zip(&aux.loads) {
            assert_eq!(p.bus, a.bus, "{label}");
            assert!(
                (p.p - a.p).abs() < 1e-3 * a.p.abs().max(1.0),
                "{label} load at {}",
                p.bus
            );
        }
        for (p, a) in pwb.generators.iter().zip(&aux.generators) {
            assert_eq!(p.bus, a.bus, "{label}");
            assert_eq!(p.in_service, a.in_service, "{label} gen at {}", p.bus);
            assert!(
                (p.pg - a.pg).abs() < 1e-3 * a.pg.abs().max(1.0),
                "{label} gen at {} pg",
                p.bus
            );
            assert!(
                (p.mbase - a.mbase).abs() < 1e-3 * a.mbase.abs().max(1.0),
                "{label} gen at {} mbase",
                p.bus
            );
        }
        assert_eq!(
            pwb.generators.iter().filter(|g| !g.in_service).count(),
            94,
            "{label}"
        );
        let mut aux_by_id: BTreeMap<(usize, usize, String), &powerio::Branch> = BTreeMap::default();
        for b in &aux.branches {
            aux_by_id.insert((b.from.0, b.to.0, ckt(b)), b);
        }
        for p in &pwb.branches {
            let key = (p.from.0, p.to.0, ckt(p));
            let a = aux_by_id
                .remove(&key)
                .unwrap_or_else(|| panic!("{label}: {key:?} not in aux"));
            assert!(
                (p.x - a.x).abs() <= 5e-7 + 1e-6 * a.x.abs(),
                "{label} {key:?} X {} vs {}",
                p.x,
                a.x
            );
            assert_eq!(p.is_transformer(), a.is_transformer(), "{label} {key:?}");
        }
        assert!(
            aux_by_id.is_empty(),
            "{label}: aux branches missing: {aux_by_id:?}"
        );
    }
}
