//! Cross format parity: the same case exported by PowerWorld in different
//! formats must agree when read through powerio.
//!
//! Two fixture sets, two kinds of agreement:
//!
//! - ACTIVSg200 (vendored): the `.aux` is a 2018 revision of the case while
//!   `case_ACTIVSg200.m` (2017) and `ACTIVSg200.RAW` (2017) are earlier
//!   snapshots, so the parity here is *structural*: bus identities, voltage
//!   levels, branch identities, impedances and taps. Solved state (vm/va),
//!   load and dispatch values changed between revisions and are not compared.
//!   Known structural delta, asserted exactly: the aux carries one branch
//!   (82-64) absent from the earlier exports.
//! - ACTIVSg2000 June 2016 (fetched, skipped when absent): every sibling was
//!   exported the same day from one case, so values must agree too: ZIP load
//!   totals against MATPOWER bus Pd/Qd, dispatch, impedances, taps, ratings.
//!
//! Tolerances are print precision of the lower precision side: the `.m`
//! writes 6 decimals for impedances, 2 for MW.

use std::collections::{BTreeMap, BTreeSet};

use powerio::network::{BusId, Network};
use powerio::parse_file;

mod common;
use common::{activsg2000_fetched as fetched, ckt, powerworld_vendored as vendored};

/// Branch identity: from, to, trimmed circuit ID.
fn branch_ids(net: &Network) -> BTreeSet<(usize, usize, String)> {
    net.branches
        .iter()
        .map(|b| (b.from.0, b.to.0, ckt(b)))
        .collect()
}

#[test]
fn activsg200_aux_vs_matpower_structural() {
    let aux = parse_file(vendored("ACTIVSg200.aux"), None).unwrap();
    let m = parse_file(vendored("case_ACTIVSg200.m"), None).unwrap();

    // Bus identity and voltage level agree bus for bus. The aux prints f32
    // noise in nominal kV (13.800000190734863), so the compare is approximate.
    assert_eq!(aux.buses.len(), 200);
    assert_eq!(m.buses.len(), 200);
    let aux_kv: BTreeMap<usize, f64> = aux.buses.iter().map(|b| (b.id.0, b.base_kv)).collect();
    for b in &m.buses {
        let kv = aux_kv[&b.id.0];
        assert!(
            (kv - b.base_kv).abs() < 1e-4,
            "bus {} kV {kv} vs {}",
            b.id,
            b.base_kv
        );
    }

    assert_eq!(aux.generators.len(), m.generators.len());
    assert_eq!(aux.shunts.len(), 4);

    // Branch identity: everything in the 2017 .m exists in the 2018 aux; the
    // aux adds exactly one line, 82-64 circuit 1.
    let aux_br = branch_ids(&aux);
    let m_br = branch_ids(&m);
    assert_eq!(aux_br.len(), 246);
    assert_eq!(m_br.len(), 245);
    let extra: Vec<_> = aux_br.difference(&m_br).collect();
    assert_eq!(extra, [&(82, 64, "1".to_string())]);
    assert!(m_br.is_subset(&aux_br), "every .m branch is in the aux");

    // Impedance, charging, and tap agree on every matched branch to the .m
    // file's print precision.
    let by_id: BTreeMap<(usize, usize, String), &powerio::Branch> = aux
        .branches
        .iter()
        .map(|b| ((b.from.0, b.to.0, ckt(b)), b))
        .collect();
    for mb in &m.branches {
        let key = (mb.from.0, mb.to.0, "1".to_string());
        let ab = by_id[&key];
        assert!((ab.r - mb.r).abs() < 1e-5, "{key:?} R {} vs {}", ab.r, mb.r);
        assert!((ab.x - mb.x).abs() < 1e-5, "{key:?} X {} vs {}", ab.x, mb.x);
        assert!((ab.b - mb.b).abs() < 1e-5, "{key:?} B {} vs {}", ab.b, mb.b);
        assert!(
            (ab.effective_tap() - mb.effective_tap()).abs() < 1e-5,
            "{key:?} tap {} vs {}",
            ab.effective_tap(),
            mb.effective_tap()
        );
    }
}

#[test]
fn activsg200_aux_vs_psse_structural() {
    let aux = parse_file(vendored("ACTIVSg200.aux"), None).unwrap();
    let raw = parse_file(vendored("ACTIVSg200.RAW"), None).unwrap();

    assert_eq!(raw.buses.len(), 200);
    let aux_kv: BTreeMap<usize, f64> = aux.buses.iter().map(|b| (b.id.0, b.base_kv)).collect();
    for b in &raw.buses {
        let kv = aux_kv[&b.id.0];
        assert!(
            (kv - b.base_kv).abs() < 1e-4,
            "bus {} kV {kv} vs {}",
            b.id,
            b.base_kv
        );
    }
    assert_eq!(aux.generators.len(), raw.generators.len());
    assert_eq!(aux.loads.len(), raw.loads.len());

    // The RAW reader does not carry circuit IDs yet, so branch parity here is
    // the multiset of bus pairs. Same one branch delta as the .m.
    let pairs = |net: &Network| -> BTreeMap<(usize, usize), usize> {
        let mut out = BTreeMap::new();
        for b in &net.branches {
            *out.entry((b.from.0, b.to.0)).or_default() += 1;
        }
        out
    };
    let mut aux_pairs = pairs(&aux);
    let raw_pairs = pairs(&raw);
    assert_eq!(aux_pairs.remove(&(82, 64)), Some(1));
    assert_eq!(aux_pairs, raw_pairs);
}

/// Same day exports: aux values must match the MATPOWER sibling.
#[test]
// One sweep per quantity over the same fixture pair; splitting it would
// scatter the parity contract.
#[allow(clippy::too_many_lines)]
fn activsg2000_june2016_aux_vs_matpower_values() {
    let (Some(aux_path), Some(m_path)) = (
        fetched("Texas2000_June2016.AUX"),
        fetched("Texas2000_June2016.m"),
    ) else {
        eprintln!("skipped: run benchmarks/fetch_powerworld.sh to fetch ACTIVSg2000");
        return;
    };
    let aux = parse_file(&aux_path, Some("powerworld")).unwrap();
    let m = parse_file(&m_path, None).unwrap();

    assert_eq!(aux.buses.len(), m.buses.len(), "bus count");
    assert_eq!(aux.generators.len(), m.generators.len(), "gen count");
    // MATPOWER carries no circuit IDs, so branch identity parity is the
    // multiset of bus pairs.
    let pairs = |net: &Network| -> BTreeMap<(usize, usize), usize> {
        let mut out = BTreeMap::new();
        for b in &net.branches {
            *out.entry((b.from.0, b.to.0)).or_default() += 1;
        }
        out
    };
    assert_eq!(pairs(&aux), pairs(&m), "branch bus pair multisets");

    // Solved state, bus for bus, to the .m print precision.
    let m_bus: BTreeMap<usize, (f64, f64)> =
        m.buses.iter().map(|b| (b.id.0, (b.vm, b.va))).collect();
    for b in &aux.buses {
        let (vm, va) = m_bus[&b.id.0];
        assert!(
            (b.vm - vm).abs() < 1e-6,
            "bus {} vm {} vs {}",
            b.id,
            b.vm,
            vm
        );
        assert!(
            (b.va - va).abs() < 1e-4,
            "bus {} va {} vs {}",
            b.id,
            b.va,
            va
        );
    }

    // ZIP load totals per bus against the folded MATPOWER Pd/Qd.
    let mut aux_load: BTreeMap<BusId, (f64, f64)> = BTreeMap::new();
    for l in aux.loads.iter().filter(|l| l.in_service) {
        let e = aux_load.entry(l.bus).or_default();
        e.0 += l.p;
        e.1 += l.q;
    }
    let mut m_load: BTreeMap<BusId, (f64, f64)> = BTreeMap::new();
    for l in m.loads.iter().filter(|l| l.in_service) {
        let e = m_load.entry(l.bus).or_default();
        e.0 += l.p;
        e.1 += l.q;
    }
    // Union of load buses: the .m prints 2 decimals and rounds sub 0.005 MW
    // loads (four of them in this case) to zero, dropping them entirely.
    let load_buses: BTreeSet<BusId> = aux_load.keys().chain(m_load.keys()).copied().collect();
    for bus in load_buses {
        let (p, q) = aux_load.get(&bus).copied().unwrap_or_default();
        let (mp, mq) = m_load.get(&bus).copied().unwrap_or_default();
        assert!((p - mp).abs() < 5.01e-3, "bus {bus} Pd {p} vs {mp}");
        assert!((q - mq).abs() < 5.01e-3, "bus {bus} Qd {q} vs {mq}");
    }

    // Dispatch and limits per generator (bus + position among the bus's gens).
    let mut m_gens: BTreeMap<usize, Vec<&powerio::Generator>> = BTreeMap::new();
    for g in &m.generators {
        m_gens.entry(g.bus.0).or_default().push(g);
    }
    let mut aux_gens: BTreeMap<usize, Vec<&powerio::Generator>> = BTreeMap::new();
    for g in &aux.generators {
        aux_gens.entry(g.bus.0).or_default().push(g);
    }
    for (bus, gens) in &aux_gens {
        let mg = &m_gens[bus];
        assert_eq!(gens.len(), mg.len(), "gen count at bus {bus}");
        for (a, b) in gens.iter().zip(mg) {
            assert!(
                (a.pg - b.pg).abs() < 5.01e-3,
                "bus {bus} pg {} vs {}",
                a.pg,
                b.pg
            );
            assert!((a.pmax - b.pmax).abs() < 5.01e-3, "bus {bus} pmax");
            assert!((a.qmax - b.qmax).abs() < 5.01e-3, "bus {bus} qmax");
        }
    }

    // Branch values on every branch. Parallel circuits have no shared ID
    // between the formats, so branches are grouped per bus pair in file order
    // (both exporters write parallels adjacently, circuit order preserved).
    let group = |net: &Network| -> BTreeMap<(usize, usize), Vec<powerio::Branch>> {
        let mut out: BTreeMap<(usize, usize), Vec<powerio::Branch>> = BTreeMap::new();
        for b in &net.branches {
            out.entry((b.from.0, b.to.0)).or_default().push(b.clone());
        }
        out
    };
    let mut compared = 0usize;
    let m_groups = group(&m);
    for (pair, aux_group) in group(&aux) {
        let m_group = &m_groups[&pair];
        assert_eq!(aux_group.len(), m_group.len(), "parallel count at {pair:?}");
        for (ab, mb) in aux_group.iter().zip(m_group) {
            assert!(
                (ab.r - mb.r).abs() < 1e-5,
                "{pair:?} R {} vs {}",
                ab.r,
                mb.r
            );
            assert!(
                (ab.x - mb.x).abs() < 1e-5,
                "{pair:?} X {} vs {}",
                ab.x,
                mb.x
            );
            assert!(
                (ab.effective_tap() - mb.effective_tap()).abs() < 1e-5,
                "{pair:?} tap {} vs {}",
                ab.effective_tap(),
                mb.effective_tap()
            );
            assert!(
                (ab.rate_a - mb.rate_a).abs() < 5e-2,
                "{pair:?} rate_a {} vs {}",
                ab.rate_a,
                mb.rate_a
            );
            compared += 1;
        }
    }
    assert_eq!(compared, aux.branches.len(), "every aux branch compared");
}

/// The contingency payload of the vendored aux, through the typed accessor.
#[test]
fn activsg200_contingencies_are_reachable() {
    use powerio::format::powerworld::{aux_sections, contingencies};
    let net = parse_file(vendored("ACTIVSg200.aux"), None).unwrap();
    let aux = aux_sections(&net).expect("powerworld source").unwrap();
    let ctgs = contingencies(&aux);
    assert_eq!(ctgs.len(), 245);
    assert_eq!(ctgs[0].label, "L_000002CREVECOEUR1-000001CREVECOEUR0C1");
    assert!(
        ctgs.iter().all(|c| !c.actions.is_empty()),
        "every contingency carries at least one action"
    );
    assert_eq!(ctgs[0].actions[0], "BRANCH 2 1 1 OPEN");
}
