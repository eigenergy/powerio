//! The 3x3 conversion harness: diagonal byte identity via the retained
//! source, canonical writer idempotence, and off diagonal round trips with
//! the lossy transforms named per cell. `cargo test --test matrix --
//! --ignored write_conversion_matrix` regenerates docs/conversion-matrix.md.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use powerio_dist::{DistLoadVoltageModel, DistTargetFormat, MulticonductorNetwork, Result};

mod helpers;
use helpers::{Conv as Emission, Sidecar, parse_bmopf_str, parse_dss_file, parse_pmd_str};

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data/dist")
        .join(rel)
}

#[derive(Clone, Copy, PartialEq)]
enum Fmt {
    Dss,
    Bmopf,
    Pmd,
}

impl Fmt {
    fn target(self) -> DistTargetFormat {
        match self {
            Fmt::Dss => DistTargetFormat::Dss,
            Fmt::Bmopf => DistTargetFormat::BmopfJson,
            Fmt::Pmd => DistTargetFormat::PmdJson,
        }
    }

    fn parse_emission(self, conv: &Emission) -> Result<helpers::Parsed> {
        self.parse_text_and_sidecars(&conv.text, &conv.sidecars)
    }

    fn parse_text_and_sidecars(self, text: &str, sidecars: &[Sidecar]) -> Result<helpers::Parsed> {
        match self {
            Fmt::Dss => {
                // Unique path per call: the harness tests run in parallel
                // threads and must not race on a shared temp file.
                use std::sync::atomic::{AtomicU64, Ordering};
                static COUNTER: AtomicU64 = AtomicU64::new(0);
                let dir = std::env::temp_dir()
                    .join("powerio-dist-matrix")
                    .join(format!("{}", COUNTER.fetch_add(1, Ordering::Relaxed)));
                std::fs::create_dir_all(&dir).unwrap();
                let path = dir.join("roundtrip.dss");
                std::fs::write(&path, text).unwrap();
                for sidecar in sidecars {
                    let sidecar_path = dir.join(&sidecar.path);
                    if let Some(parent) = sidecar_path.parent() {
                        std::fs::create_dir_all(parent).unwrap();
                    }
                    std::fs::write(sidecar_path, &sidecar.text).unwrap();
                }
                let parsed = helpers::parse_dss_file(&path);
                let _ = std::fs::remove_dir_all(&dir);
                parsed
            }
            Fmt::Bmopf => parse_bmopf_str(text),
            Fmt::Pmd => parse_pmd_str(text),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Fmt::Dss => "dss",
            Fmt::Bmopf => "BMOPF",
            Fmt::Pmd => "PMD",
        }
    }
}

struct Case {
    label: &'static str,
    rel: &'static str,
    fmt: Fmt,
    /// Transformer shapes BMOPF restates (wye-wye decomposition, center tap
    /// collapse), making the D→B→D transformer list structurally different.
    bmopf_restates_transformers: bool,
    /// dss expresses perfect grounding as node 0, so a grounded terminal's
    /// name does not survive a trip through dss. Only the public BMOPF
    /// IEEE 13 example grounds phase terminals (its three wire buses mark
    /// the highest terminal grounded); everywhere else the grounded
    /// terminal is the materialized neutral, which dss regenerates as the
    /// same name.
    dss_renames_grounded: bool,
}

const CASES: &[Case] = &[
    Case {
        label: "IEEE 13",
        rel: "opendss/ieee13/IEEE13Nodeckt.dss",
        fmt: Fmt::Dss,
        bmopf_restates_transformers: true,
        dss_renames_grounded: false,
    },
    Case {
        label: "IEEE 34",
        rel: "opendss/ieee34/ieee34Mod1.dss",
        fmt: Fmt::Dss,
        bmopf_restates_transformers: true,
        dss_renames_grounded: false,
    },
    Case {
        label: "IEEE 123",
        rel: "opendss/ieee123/IEEE123Master.dss",
        fmt: Fmt::Dss,
        bmopf_restates_transformers: true,
        dss_renames_grounded: false,
    },
    Case {
        label: "single phase transformer",
        rel: "micro/xfmr_single_phase.dss",
        fmt: Fmt::Dss,
        bmopf_restates_transformers: false,
        dss_renames_grounded: false,
    },
    Case {
        label: "center tap transformer",
        rel: "micro/xfmr_center_tap.dss",
        fmt: Fmt::Dss,
        bmopf_restates_transformers: true,
        dss_renames_grounded: false,
    },
    Case {
        label: "wye delta transformer",
        rel: "micro/xfmr_wye_delta.dss",
        fmt: Fmt::Dss,
        bmopf_restates_transformers: false,
        dss_renames_grounded: false,
    },
    Case {
        label: "delta wye transformer",
        rel: "micro/xfmr_delta_wye.dss",
        fmt: Fmt::Dss,
        bmopf_restates_transformers: false,
        dss_renames_grounded: false,
    },
    Case {
        // Open wye / open delta bank: the single phase wye/delta path. BMOPF
        // single_phase carries no wye/delta label, so the delta secondary
        // reads back as wye (terminals preserved), restating the transformer.
        label: "open wye open delta transformer",
        rel: "micro/xfmr_open_wye_open_delta.dss",
        fmt: Fmt::Dss,
        bmopf_restates_transformers: true,
        dss_renames_grounded: false,
    },
    Case {
        // Single phase delta-wye (phase to phase primary, grounded wye
        // secondary): the other single phase wye/delta orientation. Same
        // BMOPF conn-label restatement as above.
        label: "single phase delta wye transformer",
        rel: "micro/xfmr_1ph_delta_wye.dss",
        fmt: Fmt::Dss,
        bmopf_restates_transformers: true,
        dss_renames_grounded: false,
    },
    Case {
        label: "switch states",
        rel: "micro/switch.dss",
        fmt: Fmt::Dss,
        bmopf_restates_transformers: false,
        dss_renames_grounded: false,
    },
    Case {
        label: "four wire linecode",
        rel: "micro/fourwire_linecode.dss",
        fmt: Fmt::Dss,
        bmopf_restates_transformers: false,
        dss_renames_grounded: false,
    },
    Case {
        label: "constructor defaults",
        rel: "micro/defaults_degenerate.dss",
        fmt: Fmt::Dss,
        bmopf_restates_transformers: true,
        dss_renames_grounded: false,
    },
    Case {
        label: "ten conductor linecode",
        rel: "micro/linecode_10x10.dss",
        fmt: Fmt::Dss,
        bmopf_restates_transformers: false,
        dss_renames_grounded: false,
    },
    Case {
        label: "BMOPF IEEE 13 example",
        rel: "bmopf/example_ieee13.json",
        fmt: Fmt::Bmopf,
        bmopf_restates_transformers: false,
        dss_renames_grounded: true,
    },
    Case {
        label: "BMOPF ENWL example",
        rel: "bmopf/example_enwl_n1_f2.json",
        fmt: Fmt::Bmopf,
        bmopf_restates_transformers: false,
        dss_renames_grounded: false,
    },
    Case {
        label: "PMD IEEE 13",
        rel: "pmd/ieee13.json",
        fmt: Fmt::Pmd,
        bmopf_restates_transformers: true,
        dss_renames_grounded: false,
    },
    Case {
        label: "PMD four wire",
        rel: "pmd/fourwire_linecode.json",
        fmt: Fmt::Pmd,
        bmopf_restates_transformers: false,
        dss_renames_grounded: false,
    },
];

fn parse_case(case: &Case) -> helpers::Parsed {
    let path = fixture(case.rel);
    match case.fmt {
        Fmt::Dss => parse_dss_file(&path).unwrap(),
        Fmt::Bmopf => helpers::parse_bmopf_file(&path).unwrap(),
        Fmt::Pmd => helpers::parse_pmd_file(&path).unwrap(),
    }
}

fn by_name<'a, T>(items: &'a [T], name: impl Fn(&'a T) -> &'a str) -> Vec<(&'a str, &'a T)> {
    let mut v: Vec<(&str, &T)> = items.iter().map(|t| (name(t), t)).collect();
    v.sort_by_key(|(n, _)| n.to_ascii_lowercase());
    v
}

fn same_v_nom(a: &[f64], b: &[f64], allow_derived: bool) -> bool {
    (a.len() == b.len() && a.iter().zip(b).all(|(x, y)| close_power(*x, *y)))
        || (a.len() == 1 && b.iter().all(|v| close_power(*v, a[0])))
        || (b.len() == 1 && a.iter().all(|v| close_power(*v, b[0])))
        || (allow_derived && a.is_empty() && !b.is_empty())
}

fn same_load_voltage_model(
    a: &DistLoadVoltageModel,
    b: &DistLoadVoltageModel,
    allow_derived_v_nom: bool,
) -> bool {
    match (a, b) {
        (
            DistLoadVoltageModel::ConstantPower { v_nom: a },
            DistLoadVoltageModel::ConstantPower { v_nom: b },
        )
        | (
            DistLoadVoltageModel::ConstantCurrent { v_nom: a },
            DistLoadVoltageModel::ConstantCurrent { v_nom: b },
        )
        | (
            DistLoadVoltageModel::ConstantImpedance { v_nom: a },
            DistLoadVoltageModel::ConstantImpedance { v_nom: b },
        ) => same_v_nom(a, b, allow_derived_v_nom),
        (
            DistLoadVoltageModel::Zip {
                v_nom: av,
                alpha_z: aaz,
                alpha_i: aai,
                alpha_p: aap,
                beta_z: abz,
                beta_i: abi,
                beta_p: abp,
            },
            DistLoadVoltageModel::Zip {
                v_nom: bv,
                alpha_z: b_alpha_z,
                alpha_i: bai,
                alpha_p: bap,
                beta_z: bbz,
                beta_i: bbi,
                beta_p: bbp,
            },
        ) => {
            same_v_nom(av, bv, allow_derived_v_nom)
                && aaz == b_alpha_z
                && aai == bai
                && aap == bap
                && abz == bbz
                && abi == bbi
                && abp == bbp
        }
        (
            DistLoadVoltageModel::Exponential {
                v_nom: av,
                gamma_p: ap,
                gamma_q: aq,
            },
            DistLoadVoltageModel::Exponential {
                v_nom: bv,
                gamma_p: bp,
                gamma_q: bq,
            },
        ) => same_v_nom(av, bv, allow_derived_v_nom) && ap == bp && aq == bq,
        _ => false,
    }
}

fn close_power(x: f64, y: f64) -> bool {
    (x - y).abs() <= 4.0 * f64::EPSILON * x.abs().max(y.abs())
}

fn assert_loads_eq(
    a: &MulticonductorNetwork,
    b: &MulticonductorNetwork,
    what: &str,
    allow_derived_v_nom: bool,
) {
    if target_is_dss_leg(what) {
        assert_dss_loads_eq(a, b, what);
        return;
    }
    assert_eq!(a.loads().len(), b.loads().len(), "{what}: loads");
    for ((_, x), (_, y)) in by_name(a.loads(), |l| &l.name)
        .iter()
        .zip(&by_name(b.loads(), |l| &l.name))
    {
        for (p, q) in x.p_nom.iter().zip(&y.p_nom) {
            assert!(close_power(*p, *q), "{what}: load {} p {p} vs {q}", x.name);
        }
        for (p, q) in x.q_nom.iter().zip(&y.q_nom) {
            assert!(close_power(*p, *q), "{what}: load {} q {p} vs {q}", x.name);
        }
        assert_maps_eq(
            &x.terminal_map,
            &y.terminal_map,
            what,
            &format!("load {} map", x.name),
        );
        assert!(
            same_load_voltage_model(&x.voltage_model, &y.voltage_model, allow_derived_v_nom),
            "{what}: load {} voltage model {:?} vs {:?}",
            x.name,
            x.voltage_model,
            y.voltage_model
        );
    }
}

/// Loads over a dss leg (#266 item 2).
///
/// A load whose phases carry different power has no single `Load` expression,
/// so the writer splits it into one single phase `Load` per terminal, named
/// `<load>_<terminal>`. The count therefore grows, and what has to survive is
/// the per phase profile: each source load is matched by its own name when it
/// came back whole, or by its parts, and the two must state the same power.
fn assert_dss_loads_eq(a: &MulticonductorNetwork, b: &MulticonductorNetwork, what: &str) {
    let mut matched = 0usize;
    for x in a.loads() {
        let parts: Vec<&powerio_dist::DistLoad> = b
            .loads()
            .iter()
            .filter(|y| {
                y.name == x.name
                    || y.name
                        .strip_prefix(&format!("{}_", x.name))
                        .is_some_and(|rest| x.terminal_map.iter().any(|t| t == rest))
            })
            .collect();
        assert!(
            !parts.is_empty(),
            "{what}: load {} has no counterpart",
            x.name
        );
        matched += parts.len();

        let mut want: Vec<(f64, f64)> = x
            .p_nom
            .iter()
            .copied()
            .zip(x.q_nom.iter().copied())
            .collect();
        let mut got: Vec<(f64, f64)> = parts
            .iter()
            .flat_map(|y| y.p_nom.iter().copied().zip(y.q_nom.iter().copied()))
            .collect();
        if parts.len() == 1 && got.len() != want.len() {
            // Came back whole: one balanced object states the total, which is
            // all a balanced source load ever said.
            let sum = |v: &[(f64, f64)]| {
                v.iter()
                    .fold((0.0, 0.0), |(p, q), (dp, dq)| (p + dp, q + dq))
            };
            let (wp, wq) = sum(&want);
            let (gp, gq) = sum(&got);
            assert!(
                close_power(wp, gp) && close_power(wq, gq),
                "{what}: load {} totals ({wp}, {wq}) vs ({gp}, {gq})",
                x.name
            );
            continue;
        }
        let by_power = |v: &mut Vec<(f64, f64)>| {
            v.sort_by(|l, r| l.partial_cmp(r).expect("no NaN power in a fixture"));
        };
        by_power(&mut want);
        by_power(&mut got);
        assert_eq!(
            want.len(),
            got.len(),
            "{what}: load {} phase count {want:?} vs {got:?}",
            x.name
        );
        for ((wp, wq), (gp, gq)) in want.iter().zip(&got) {
            assert!(
                close_power(*wp, *gp) && close_power(*wq, *gq),
                "{what}: load {} phase ({wp}, {wq}) vs ({gp}, {gq})",
                x.name
            );
        }
    }
    assert_eq!(
        matched,
        b.loads().len(),
        "{what}: {} loads came back unmatched",
        b.loads().len() - matched
    );
}

/// The dss round trip leg of the harness.
fn target_is_dss_leg(what: &str) -> bool {
    what.contains("→ dss → back")
}

/// dss flattens per-load voltage profiles, so v_nom can materialize on the
/// way back.
fn target_may_materialize_v_nom(what: &str) -> bool {
    target_is_dss_leg(what)
}

/// dss spells terminals as numeric node positions and PMD requires integer
/// connections, so non numeric terminal names do not survive those legs.
fn target_renumbers_terminals(what: &str) -> bool {
    what.contains("→ dss → back") || what.contains("→ PMD → back")
}

/// Strict name equality. A renumbering leg (dss node positions, PMD integer
/// connections) falls back to arity, but only when a non numeric name is
/// involved: purely numeric maps survive those legs verbatim, so a silent
/// permutation cannot hide behind the fallback. The renumbering scheme
/// itself still needs its dedicated pin (#266).
#[track_caller]
fn assert_maps_eq(x: &[String], y: &[String], what: &str, ctx: &str) {
    if x == y {
        return;
    }
    let numeric = |m: &[String]| m.iter().all(|t| t.parse::<u32>().is_ok());
    assert!(
        target_renumbers_terminals(what) && x.len() == y.len() && !(numeric(x) && numeric(y)),
        "{what}: {ctx} {x:?} vs {y:?}"
    );
}

/// The model fields every format carries; the per cell comparisons run on
/// this projection, with transformer carve outs where BMOPF restates them.
fn assert_projection_eq(
    a: &MulticonductorNetwork,
    b: &MulticonductorNetwork,
    what: &str,
    transformers: bool,
) {
    // JSON formats key elements by name, so order is not preserved across
    // a round trip; compare per name.
    assert_eq!(a.buses().len(), b.buses().len(), "{what}: bus count");
    let buses_a = by_name(a.buses(), |b| &b.id);
    let buses_b = by_name(b.buses(), |b| &b.id);
    for ((_, x), (_, y)) in buses_a.iter().zip(&buses_b) {
        assert!(x.id.eq_ignore_ascii_case(&y.id), "{what}: bus set");
        assert_maps_eq(
            &x.terminals,
            &y.terminals,
            what,
            &format!("bus {} terminals", x.id),
        );
        // dss has no standalone grounding statement (grounding is a node 0
        // reference) and its reader materializes source bus grounding, so
        // grounding equality only holds off the dss legs.
        if !target_is_dss_leg(what) {
            assert_maps_eq(
                &x.grounded,
                &y.grounded,
                what,
                &format!("bus {} grounding", x.id),
            );
        }
    }
    assert_eq!(a.switches().len(), b.switches().len(), "{what}: switches");
    for ((_, x), (_, y)) in by_name(a.switches(), |s| &s.name)
        .iter()
        .zip(&by_name(b.switches(), |s| &s.name))
    {
        assert_eq!(x.open, y.open, "{what}: switch {}", x.name);
    }
    // Scale changes (kW to W and back) cost at most one rounding per
    // direction; powers compare to 2 ULP relative, everything structural
    // exactly.
    let allow_derived_v_nom = target_may_materialize_v_nom(what);
    assert_loads_eq(a, b, what, allow_derived_v_nom);
    assert_eq!(a.lines().len(), b.lines().len(), "{what}: lines");
    for ((_, x), (_, y)) in by_name(a.lines(), |l| &l.name)
        .iter()
        .zip(&by_name(b.lines(), |l| &l.name))
    {
        assert!(
            x.name.eq_ignore_ascii_case(&y.name),
            "{what}: line set ({} vs {})",
            x.name,
            y.name
        );
        assert!(
            x.bus_from.eq_ignore_ascii_case(&y.bus_from)
                && x.bus_to.eq_ignore_ascii_case(&y.bus_to),
            "{what}: line {} endpoints",
            x.name
        );
        assert_eq!(
            x.length.to_bits(),
            y.length.to_bits(),
            "{what}: line {} length",
            x.name
        );
        assert_maps_eq(
            &x.terminal_map_from,
            &y.terminal_map_from,
            what,
            &format!("line {} from map", x.name),
        );
        assert_maps_eq(
            &x.terminal_map_to,
            &y.terminal_map_to,
            what,
            &format!("line {} to map", x.name),
        );
    }
    if transformers {
        assert_eq!(
            a.transformers().len(),
            b.transformers().len(),
            "{what}: transformers"
        );
        for ((_, x), (_, y)) in by_name(a.transformers(), |t| &t.name)
            .iter()
            .zip(&by_name(b.transformers(), |t| &t.name))
        {
            assert_eq!(
                x.windings.len(),
                y.windings.len(),
                "{what}: xfmr {}",
                x.name
            );
            for (wx, wy) in x.windings.iter().zip(&y.windings) {
                assert_eq!(wx.conn, wy.conn, "{what}: xfmr {} conn", x.name);
                assert!(
                    (wx.v_ref - wy.v_ref).abs() <= 1e-9 * wx.v_ref.abs().max(1.0),
                    "{what}: xfmr {} v_ref {} vs {}",
                    x.name,
                    wx.v_ref,
                    wy.v_ref
                );
            }
        }
    }
}

/// Linecode matrices compare to within one ULP scale relative error: a
/// basis change (the PMD capacitance form, the dss per length form) costs
/// at most one rounding per direction.
fn assert_linecodes_close(a: &MulticonductorNetwork, b: &MulticonductorNetwork, what: &str) {
    assert_eq!(
        a.linecodes().len(),
        b.linecodes().len(),
        "{what}: linecodes"
    );
    let close = |x: f64, y: f64| (x - y).abs() <= 1e-12 * x.abs().max(y.abs()).max(1e-300);
    let mut xs: Vec<_> = a.linecodes().iter().collect();
    let mut ys: Vec<_> = b.linecodes().iter().collect();
    xs.sort_by_key(|c| c.name.to_ascii_lowercase());
    ys.sort_by_key(|c| c.name.to_ascii_lowercase());
    for (x, y) in xs.iter().zip(&ys) {
        assert!(
            x.name.eq_ignore_ascii_case(&y.name),
            "{what}: linecode set ({} vs {})",
            x.name,
            y.name
        );
        assert_eq!(
            x.n_conductors, y.n_conductors,
            "{what}: linecode {} size",
            x.name
        );
        let mats = [
            ("r", &x.r_series, &y.r_series),
            ("x", &x.x_series, &y.x_series),
            ("b", &x.b_from, &y.b_from),
        ];
        for (label, mx, my) in mats {
            assert_eq!(mx.len(), my.len(), "{what}: linecode {} {label}", x.name);
            for (rx, ry) in mx.iter().zip(my) {
                assert_eq!(rx.len(), ry.len(), "{what}: linecode {} {label}", x.name);
                for (vx, vy) in rx.iter().zip(ry) {
                    assert!(
                        close(*vx, *vy),
                        "{what}: linecode {} {label} {vx} vs {vy}",
                        x.name
                    );
                }
            }
        }
    }
}

/// Replaces every grounded terminal name with "G", on buses and in the
/// terminal maps of the elements referencing them.
fn normalize_grounded(net: &MulticonductorNetwork) -> MulticonductorNetwork {
    let mut net = net.clone();
    let grounded: BTreeMap<String, Vec<String>> = net
        .buses()
        .iter()
        .map(|b| (b.id.to_ascii_lowercase(), b.grounded.clone()))
        .collect();
    let fix = |bus: &str, map: &mut Vec<String>| {
        if let Some(g) = grounded.get(&bus.to_ascii_lowercase()) {
            for t in map.iter_mut() {
                if g.contains(t) {
                    *t = "G".to_string();
                }
            }
        }
    };
    for b in net.buses_mut() {
        let g = b.grounded.clone();
        for t in b.terminals.iter_mut().chain(b.grounded.iter_mut()) {
            if g.contains(t) {
                *t = "G".to_string();
            }
        }
    }
    for l in net.lines_mut() {
        fix(&l.bus_from.clone(), &mut l.terminal_map_from);
        fix(&l.bus_to.clone(), &mut l.terminal_map_to);
    }
    for s in net.switches_mut() {
        fix(&s.bus_from.clone(), &mut s.terminal_map_from);
        fix(&s.bus_to.clone(), &mut s.terminal_map_to);
    }
    for l in net.loads_mut() {
        fix(&l.bus.clone(), &mut l.terminal_map);
    }
    for t in net.transformers_mut() {
        for w in &mut t.windings {
            fix(&w.bus.clone(), &mut w.terminal_map);
        }
    }
    net
}

fn normalize_bmopf_bus_metadata(
    net: &MulticonductorNetwork,
    usage_net: &MulticonductorNetwork,
) -> MulticonductorNetwork {
    let mut net = net.clone();
    let mut usage: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut add = |bus: &str, terms: &[String]| {
        usage
            .entry(bus.to_string())
            .or_default()
            .extend(terms.iter().cloned());
    };
    for l in usage_net.lines() {
        add(&l.bus_from, &l.terminal_map_from);
        add(&l.bus_to, &l.terminal_map_to);
    }
    for s in usage_net.switches() {
        add(&s.bus_from, &s.terminal_map_from);
        add(&s.bus_to, &s.terminal_map_to);
    }
    for l in usage_net.loads() {
        add(&l.bus, &l.terminal_map);
    }
    for g in usage_net.generators() {
        add(&g.bus, &g.terminal_map);
    }
    for s in usage_net.shunts() {
        add(&s.bus, &s.terminal_map);
    }
    for s in usage_net.sources() {
        add(&s.bus, &s.terminal_map);
    }
    for t in usage_net.transformers() {
        for w in &t.windings {
            add(&w.bus, &w.terminal_map);
        }
    }

    net.buses_mut().retain(|b| usage.contains_key(&b.id));
    for b in net.buses_mut() {
        let Some(used) = usage.get(&b.id) else {
            continue;
        };
        // Grounded terminals count as used: the writers keep them (ground
        // references them), so the projection must too.
        let used: BTreeSet<&String> = used.iter().chain(&b.grounded).collect();
        b.terminals.retain(|term| used.contains(term));
    }
    net
}

#[test]
fn diagonal_byte_identity() {
    for case in CASES {
        let net = parse_case(case);
        let original = std::fs::read_to_string(fixture(case.rel)).unwrap();
        let echoed = net.emit(case.fmt.target());
        assert_eq!(echoed.text, original, "{}: diagonal echo", case.label);
        assert!(echoed.warnings.is_empty(), "{}: echo warns", case.label);
    }
}

/// The (case, target) pairs whose first canonical write is not yet a fixed
/// point. Each entry needs a source construct that explains it, because the
/// general rule is that one write reaches the canonical form. The second
/// write must then be a fixed point, which the test still checks.
const LATE_CONVERGENCE: &[(&str, &str)] = &[
    // The BMOPF ieee13 example declares a single_phase transformer with
    // three phase terminal maps; dss narrows it to its real phase count.
    ("BMOPF IEEE 13 example", "dss"),
];

fn converges_on_the_second_write(label: &str, target: &str) -> bool {
    LATE_CONVERGENCE.contains(&(label, target))
}

#[test]
fn canonical_writers_are_idempotent() {
    for case in CASES {
        let net = parse_case(case);
        for target in [Fmt::Dss, Fmt::Bmopf, Fmt::Pmd] {
            let first = match target {
                Fmt::Dss => helpers::emit_dss(&net),
                Fmt::Bmopf => helpers::emit_bmopf_json(&net),
                Fmt::Pmd => helpers::emit_pmd_json(&net),
            };
            let reparsed = match target.parse_emission(&first) {
                Ok(n) => n,
                Err(e) => panic!("{} → {}: reparse failed: {e}", case.label, target.name()),
            };
            let second = match target {
                Fmt::Dss => helpers::emit_dss(&reparsed),
                Fmt::Bmopf => helpers::emit_bmopf_json(&reparsed),
                Fmt::Pmd => helpers::emit_pmd_json(&reparsed),
            };
            if first.text != second.text {
                assert!(
                    converges_on_the_second_write(case.label, target.name()),
                    "{} → {}: the first canonical write is not a fixed point, and this \
                     pair is not a known one-step canonicalization; add it to \
                     LATE_CONVERGENCE only with the construct that explains it",
                    case.label,
                    target.name()
                );
                // A degenerate source construct can canonicalize once through
                // the target (the BMOPF ieee13 example carries a single_phase
                // transformer with three phase terminal maps, which dss
                // narrows to its actual phase count); the canonical form must
                // then be a fixed point.
                let reparsed2 = target.parse_emission(&second).unwrap_or_else(|e| {
                    panic!("{} → {}: reparse failed: {e}", case.label, target.name())
                });
                let third = match target {
                    Fmt::Dss => helpers::emit_dss(&reparsed2),
                    Fmt::Bmopf => helpers::emit_bmopf_json(&reparsed2),
                    Fmt::Pmd => helpers::emit_pmd_json(&reparsed2),
                };
                assert_eq!(
                    second.text,
                    third.text,
                    "{} → {}: canonical output does not converge",
                    case.label,
                    target.name()
                );
            }
        }
    }
}

#[test]
fn off_diagonal_round_trips() {
    for case in CASES {
        let net = parse_case(case);
        for target in [Fmt::Dss, Fmt::Bmopf, Fmt::Pmd] {
            if target == case.fmt {
                continue;
            }
            let what = format!("{} → {} → back", case.label, target.name());
            let out = net.emit(target.target());
            let back = target
                .parse_emission(&out)
                .unwrap_or_else(|e| panic!("{what}: {e}"));
            let transformers = !(target == Fmt::Bmopf && case.bmopf_restates_transformers);
            let (expected, actual) = match target {
                Fmt::Bmopf => (
                    normalize_bmopf_bus_metadata(&net, &back),
                    normalize_bmopf_bus_metadata(&back, &back),
                ),
                // A dss leg only materializes referenced or grounded
                // terminals, so project the source the same way.
                Fmt::Dss => (normalize_bmopf_bus_metadata(&net, &net), (*back).clone()),
                Fmt::Pmd => ((*net).clone(), (*back).clone()),
            };
            if target == Fmt::Dss && case.dss_renames_grounded {
                // Grounded phase terminals fold into node 0 on the way
                // through dss; compare the networks with each bus's grounded
                // terminals normalized to one token.
                let (a, b) = (normalize_grounded(&expected), normalize_grounded(&actual));
                assert_projection_eq(&a, &b, &what, transformers);
                assert_linecodes_close(&a, &b, &what);
            } else {
                assert_projection_eq(&expected, &actual, &what, transformers);
                assert_linecodes_close(&expected, &actual, &what);
            }
        }
    }
}

/// Regenerates docs/conversion-matrix.md; the table records every cell of
/// the matrix with its outcome.
#[test]
#[ignore = "writes docs/conversion-matrix.md; run on demand"]
fn write_conversion_matrix() {
    let mut md = String::new();
    md.push_str("# Conversion matrix\n\n");
    md.push_str(
        "Generated by `cargo test -p powerio-dist --test matrix -- --ignored \
         write_conversion_matrix`. Rows are fixtures (tests/data/dist, provenance in its \
         README); columns are conversion targets. `echo` is the byte exact diagonal; `ok` is \
         a canonical write that reparses to the common projection of the model; `ok (n warn)` \
         names the count of fidelity losses the conversion reports, each one listed in the \
         conversion's warnings.\n\n",
    );
    md.push_str("| fixture | source | → dss | → BMOPF | → PMD |\n");
    md.push_str("|---|---|---|---|---|\n");
    for case in CASES {
        let net = parse_case(case);
        let mut cells = Vec::new();
        for target in [Fmt::Dss, Fmt::Bmopf, Fmt::Pmd] {
            if target == case.fmt {
                cells.push("echo".to_string());
                continue;
            }
            let out = net.emit(target.target());
            match target.parse_emission(&out) {
                Ok(_) => {
                    if out.warnings.is_empty() {
                        cells.push("ok".to_string());
                    } else {
                        cells.push(format!("ok ({} warn)", out.warnings.len()));
                    }
                }
                Err(e) => cells.push(format!("FAIL: {e}")),
            }
        }
        let _ = writeln!(
            md,
            "| {} | {} | {} | {} | {} |",
            case.label,
            case.fmt.name(),
            cells[0],
            cells[1],
            cells[2]
        );
    }
    md.push('\n');
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/conversion-matrix.md");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, md).unwrap();
}

/// Writes every fixture's canonical dss output under target/physics so
/// tools/physics_check.py can re-solve them against the originals.
#[test]
#[ignore = "writes target/physics; run before tools/physics_check.py"]
fn emit_for_physics_check() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/physics");
    std::fs::create_dir_all(&dir).unwrap();
    for case in CASES {
        let net = parse_case(case);
        let stem = case
            .rel
            .replace('/', "_")
            .replace(".dss", "")
            .replace(".json", "");
        // The canonical dss regeneration (echo bypassed on purpose).
        let dss = helpers::emit_dss(&net);
        std::fs::write(dir.join(format!("{stem}.canonical.dss")), &dss.text).unwrap();
        if case.fmt == Fmt::Dss {
            // Through each JSON format and back to dss.
            for (suffix, text) in [
                ("via_bmopf", helpers::emit_bmopf_json(&net).text),
                ("via_pmd", helpers::emit_pmd_json(&net).text),
            ] {
                let mid = if suffix == "via_bmopf" {
                    parse_bmopf_str(&text).unwrap()
                } else {
                    parse_pmd_str(&text).unwrap()
                };
                let out = helpers::emit_dss(&mid);
                std::fs::write(dir.join(format!("{stem}.{suffix}.dss")), &out.text).unwrap();
            }
        }
    }
    let _ = Arc::new(());
}

/// Every terminal map of one network as `(element key, bus id, map)`, in a
/// stable order. The bus id matters because the rename is per bus: dss
/// spells a terminal as its node position within its own bus.
fn terminal_maps(net: &MulticonductorNetwork) -> Vec<(String, String, Vec<String>)> {
    let key = |s: &str| s.to_lowercase();
    // Element maps only. A bus's own terminal list is not a rename: dss
    // spells perfect grounding as node 0, so a grounded terminal legitimately
    // leaves the list on that leg (the `dss_renames_grounded` carve-out).
    let mut out = Vec::new();
    for line in net.lines() {
        let name = key(&line.name);
        out.push((
            format!("line {name} from"),
            key(&line.bus_from),
            line.terminal_map_from.clone(),
        ));
        out.push((
            format!("line {name} to"),
            key(&line.bus_to),
            line.terminal_map_to.clone(),
        ));
    }
    for switch in net.switches() {
        let name = key(&switch.name);
        out.push((
            format!("switch {name} from"),
            key(&switch.bus_from),
            switch.terminal_map_from.clone(),
        ));
        out.push((
            format!("switch {name} to"),
            key(&switch.bus_to),
            switch.terminal_map_to.clone(),
        ));
    }
    for load in net.loads() {
        out.push((
            format!("load {}", key(&load.name)),
            key(&load.bus),
            load.terminal_map.clone(),
        ));
    }
    for generator in net.generators() {
        out.push((
            format!("generator {}", key(&generator.name)),
            key(&generator.bus),
            generator.terminal_map.clone(),
        ));
    }
    for shunt in net.shunts() {
        out.push((
            format!("shunt {}", key(&shunt.name)),
            key(&shunt.bus),
            shunt.terminal_map.clone(),
        ));
    }
    out.sort();
    out
}

/// The renumbering scheme itself, which the arity fallback in
/// `assert_maps_eq` cannot check (issue #266 item 3).
///
/// dss spells terminals as node positions and PMD requires integer
/// connections, so a named terminal takes a new name on those legs. The
/// rename must be a position-stable bijection per bus:
///
/// - every element keeps its arity, and position `k` before the leg is
///   position `k` after it, so no phase order permutation can hide;
/// - within one bus, a source name always takes the same new name, so two
///   elements that shared a conductor still share it;
/// - within one bus, two source names never take one new name, so two
///   conductors do not merge.
///
/// The scope is one bus because dss numbers a terminal by its position in
/// its own bus, so the same name at two buses can legitimately differ.
/// A permutation, a merge, or an inconsistent rename each fails here even
/// though all of them keep the arity.
#[test]
fn renumbering_legs_are_position_stable_bijections() {
    for case in CASES {
        let net = parse_case(case);
        let before = terminal_maps(&net);
        for target in [Fmt::Dss, Fmt::Pmd] {
            let conv = match target {
                Fmt::Dss => helpers::emit_dss(&net),
                Fmt::Pmd => helpers::emit_pmd_json(&net),
                Fmt::Bmopf => unreachable!("BMOPF keeps terminal names"),
            };
            let round_tripped = target
                .parse_emission(&conv)
                .unwrap_or_else(|e| panic!("{} → {}: {e}", case.label, target.name()));
            let after = terminal_maps(&round_tripped);
            let what = format!("{} → {} → back", case.label, target.name());

            // Keyed by `(bus, name)`: the rename is per bus.
            let mut forward: BTreeMap<(String, String), String> = BTreeMap::new();
            let mut backward: BTreeMap<(String, String), String> = BTreeMap::new();
            let after_by_key: BTreeMap<&str, &Vec<String>> =
                after.iter().map(|(k, _, v)| (k.as_str(), v)).collect();

            for (key, bus, source_map) in &before {
                let Some(target_map) = after_by_key.get(key.as_str()) else {
                    // Element identity across the leg is `assert_projection_eq`'s
                    // job; this test only pins the rename.
                    continue;
                };
                assert_eq!(
                    source_map.len(),
                    target_map.len(),
                    "{what}: {key} changed arity: {source_map:?} vs {target_map:?}"
                );
                for (position, (source, renamed)) in
                    source_map.iter().zip(target_map.iter()).enumerate()
                {
                    let previous = forward.insert((bus.clone(), source.clone()), renamed.clone());
                    assert!(
                        previous.as_ref().is_none_or(|p| p == renamed),
                        "{what}: {key} position {position}: at bus `{bus}` terminal \
                         `{source}` became `{renamed}` here and `{}` elsewhere",
                        previous.unwrap_or_default()
                    );
                    let previous = backward.insert((bus.clone(), renamed.clone()), source.clone());
                    assert!(
                        previous.as_ref().is_none_or(|p| p == source),
                        "{what}: {key} position {position}: at bus `{bus}` `{renamed}` \
                         stands for both `{source}` and `{}`",
                        previous.unwrap_or_default()
                    );
                }
            }
        }
    }
}
