//! The 3x3 conversion harness: diagonal byte identity via the retained
//! source, canonical JSON stability, and electrical round trips with
//! the lossy transforms named per cell. `cargo test --test matrix --
//! --ignored write_conversion_matrix` regenerates docs/conversion-matrix.md.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use powerio_dist::{DistLoadVoltageModel, DistTargetFormat, MulticonductorNetwork, Result};

#[path = "helpers/electrical_projection.rs"]
mod electrical_projection;
mod helpers;
#[path = "helpers/source_equivalents.rs"]
mod source_equivalents;
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
    Ravens,
}

impl Fmt {
    fn target(self) -> DistTargetFormat {
        match self {
            Fmt::Dss => DistTargetFormat::Dss,
            Fmt::Bmopf => DistTargetFormat::BmopfJson,
            Fmt::Pmd => DistTargetFormat::PmdJson,
            Fmt::Ravens => DistTargetFormat::RavensJson,
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
            Fmt::Ravens => helpers::parse_str(text, "ravens-json"),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Fmt::Dss => "dss",
            Fmt::Bmopf => "BMOPF",
            Fmt::Pmd => "PMD",
            Fmt::Ravens => "RAVENS",
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
        Fmt::Ravens => helpers::parse_file(&path, Some("ravens-json")).unwrap(),
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

#[allow(clippy::float_cmp)] // Exact ZIP coefficients are checked before acknowledging a diagnosed projection.
fn same_load_voltage_model(
    a: &DistLoadVoltageModel,
    b: &DistLoadVoltageModel,
    allow_derived_v_nom: bool,
    allow_exponential_projection: bool,
) -> bool {
    if let DistLoadVoltageModel::BoundedExponential {
        v_nom,
        gamma_p,
        gamma_q,
        ..
    } = a
    {
        // JSON destinations diagnose the missing voltage band; only BMOPF
        // can also retain the nominal exponents. PMD diagnoses their loss.
        return same_v_nom(v_nom, b.v_nom(), allow_derived_v_nom)
            && match b {
                DistLoadVoltageModel::Exponential {
                    gamma_p: p,
                    gamma_q: q,
                    ..
                } => gamma_p == p && gamma_q == q,
                DistLoadVoltageModel::ConstantPower { .. } => allow_exponential_projection,
                _ => false,
            };
    }
    if let DistLoadVoltageModel::BoundedZip {
        v_nom,
        alpha_z,
        alpha_i,
        alpha_p,
        beta_z,
        beta_i,
        beta_p,
        ..
    } = a
    {
        // Off-diagonal JSON projection reports the lost voltage band below.
        // These corpus loads are pure P/I; verify every nominal coefficient.
        let (want_z, want_i, want_p) = match b {
            DistLoadVoltageModel::ConstantPower { .. } => (0.0, 0.0, 1.0),
            DistLoadVoltageModel::ConstantCurrent { .. } => (0.0, 1.0, 0.0),
            _ => return false,
        };
        return same_v_nom(v_nom, b.v_nom(), allow_derived_v_nom)
            && [alpha_z, beta_z]
                .iter()
                .all(|v| v.iter().all(|x| *x == want_z))
            && [alpha_i, beta_i]
                .iter()
                .all(|v| v.iter().all(|x| *x == want_i))
            && [alpha_p, beta_p]
                .iter()
                .all(|v| v.iter().all(|x| *x == want_p));
    }
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
            same_load_voltage_model(
                &x.voltage_model,
                &y.voltage_model,
                allow_derived_v_nom,
                what.contains("→ PMD → back")
            ),
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
        // DSS node zero is global earth. A grounded placeholder can be
        // created or omitted when no equipment references it; compare the
        // non-ground conductors here and the actual device ports below.
        let live = |bus: &powerio_dist::DistBus| {
            bus.terminals
                .iter()
                .filter(|t| !target_is_dss_leg(what) || !bus.grounded.contains(t))
                .cloned()
                .collect::<Vec<_>>()
        };
        assert_maps_eq(&live(x), &live(y), what, &format!("bus {} terminals", x.id));
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
        if !target_is_dss_leg(what) {
            assert_eq!(
                x.length.to_bits(),
                y.length.to_bits(),
                "{what}: line {} length",
                x.name
            );
        }
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
    let close = |x: f64, y: f64| (x - y).abs() <= 1e-12 * x.abs().max(y.abs()).max(1e-300);
    // Canonical DSS allocates a private effective code per line. Compare
    // physical matrices at each named branch, including both shunt halves.
    for line in a.lines() {
        let other = b
            .lines()
            .iter()
            .find(|l| l.name.eq_ignore_ascii_case(&line.name))
            .unwrap();
        let x = a.linecode(&line.linecode).unwrap();
        let y = b.linecode(&other.linecode).unwrap();
        assert_eq!(
            x.n_conductors, y.n_conductors,
            "{what}: line {} conductors",
            line.name
        );
        let mats = [
            ("r", &x.r_series, &y.r_series),
            ("x", &x.x_series, &y.x_series),
            ("b_from", &x.b_from, &y.b_from),
            ("b_to", &x.b_to, &y.b_to),
            ("g_from", &x.g_from, &y.g_from),
            ("g_to", &x.g_to, &y.g_to),
        ];
        for (label, mx, my) in mats {
            assert_eq!(mx.len(), my.len(), "{what}: linecode {} {label}", x.name);
            for (rx, ry) in mx.iter().zip(my) {
                assert_eq!(rx.len(), ry.len(), "{what}: linecode {} {label}", x.name);
                for (vx, vy) in rx.iter().zip(ry) {
                    assert!(
                        close(*vx * line.length, *vy * other.length),
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
        let retained = if matches!(case.fmt, Fmt::Dss) && !echoed.sidecars.is_empty() {
            let entry = format!(
                "source/{}",
                std::path::Path::new(case.rel)
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
            );
            &echoed
                .sidecars
                .iter()
                .find(|file| file.path == entry)
                .unwrap()
                .text
        } else {
            &echoed.text
        };
        assert_eq!(*retained, original, "{}: diagonal echo", case.label);
        assert!(echoed.warnings.is_empty(), "{}: echo warns", case.label);
    }
}

// The legacy PMD reader retains finite switch matrices in extras. Check
// those explicit values against the emitted line instead of treating that
// record as an ideal contact. Moving these matrices into typed physics is
// still a separate reader gap; no default epsilon is allowed here.
fn with_pmd_switch_impedances(net: &MulticonductorNetwork) -> MulticonductorNetwork {
    let mut result = net.clone();
    for switch in net.switches().iter().filter(|s| !s.open) {
        let (Some(r), Some(x)) = (switch.extras.get("pmd_rs"), switch.extras.get("pmd_xs")) else {
            continue;
        };
        let code_name = format!("test_switch_{}", switch.name);
        assert!(result.linecode(&code_name).is_none());
        result
            .line_codes_mut()
            .push(powerio_dist::DistLineCode::new(
                &code_name,
                serde_json::from_value(r.clone()).unwrap(),
                serde_json::from_value(x.clone()).unwrap(),
            ));
        result.lines_mut().push(powerio_dist::DistLine::new(
            &switch.name,
            &switch.bus_from,
            &switch.bus_to,
            switch.terminal_map_from.clone(),
            switch.terminal_map_to.clone(),
            code_name,
            1.0,
        ));
        result.switches_mut().retain(|s| s.name != switch.name);
    }
    result
}

fn assert_dss_equivalent(
    a: &MulticonductorNetwork,
    b: &MulticonductorNetwork,
    case: &Case,
    stage: &str,
) {
    let what = format!("{} → dss → back ({stage})", case.label);
    let a = source_equivalents::contract_source_pairs(&with_pmd_switch_impedances(a));
    let b = source_equivalents::contract_source_pairs(b);
    let a = source_equivalents::contract_transformer_pairs(&a);
    let b = source_equivalents::contract_transformer_pairs(&b);
    source_equivalents::assert_source_constraints(&a, &b, &what);
    let a = normalize_bmopf_bus_metadata(&a, &a);
    let (a, b) = if case.dss_renames_grounded {
        (normalize_grounded(&a), normalize_grounded(&b))
    } else {
        (a, b)
    };
    assert_projection_eq(&a, &b, &what, true);
    assert_linecodes_close(&a, &b, &what);
}

#[test]
fn canonical_writers_preserve_electrical_equivalents() {
    for case in CASES {
        let net = parse_case(case);
        for target in [Fmt::Dss, Fmt::Bmopf, Fmt::Pmd] {
            let first = match target {
                Fmt::Dss => helpers::emit_dss(&net),
                Fmt::Bmopf => helpers::emit_bmopf_json(&net),
                Fmt::Pmd => helpers::emit_pmd_json(&net),
                Fmt::Ravens => helpers::emit_value(&net, DistTargetFormat::RavensJson),
            };
            let reparsed = match target.parse_emission(&first) {
                Ok(n) => n,
                Err(e) => panic!("{} → {}: reparse failed: {e}", case.label, target.name()),
            };
            let second = match target {
                Fmt::Dss => helpers::emit_dss(&reparsed),
                Fmt::Bmopf => helpers::emit_bmopf_json(&reparsed),
                Fmt::Pmd => helpers::emit_pmd_json(&reparsed),
                Fmt::Ravens => helpers::emit_value(&reparsed, DistTargetFormat::RavensJson),
            };
            if target == Fmt::Dss {
                let third = target.parse_emission(&second).unwrap();
                assert_dss_equivalent(&net, &reparsed, case, "first canonical write");
                assert_dss_equivalent(&reparsed, &third, case, "second canonical write");
                continue;
            }
            assert_eq!(
                first.text,
                second.text,
                "{} → {}: canonical JSON is not stable",
                case.label,
                target.name()
            );
        }
    }
}

/// Every fixture converts to MG-RAVENS and the canonical write is byte
/// idempotent (write, parse, write). RAVENS is not in the strict
/// projection grid above: its transformer-tank and per-phase restatement
/// makes the structural comparison a separate concern (see `tests/ravens.rs`
/// for the projection round trips), but byte idempotence is the writer's
/// core guarantee and holds across the whole corpus.
#[test]
fn ravens_writer_idempotent_across_fixtures() {
    for case in CASES {
        let net = parse_case(case);
        let first = helpers::emit_value(&net, Fmt::Ravens.target());
        let (expected, projection_problems) =
            electrical_projection::ravens_projection(&net, &first.diagnostics);
        assert!(
            projection_problems.is_empty(),
            "{}: {}",
            case.label,
            projection_problems.join("; ")
        );
        let reparsed = Fmt::Ravens.parse_emission(&first).unwrap_or_else(|e| {
            panic!(
                "{} -> {}: reparse failed: {e}",
                case.label,
                Fmt::Ravens.name()
            )
        });
        // Unsupported-conductor cases still check the surviving circuit and
        // canonical stability; a diagnostic is not a reason to skip a case.
        if expected.lines().len() != net.lines().len()
            || expected.loads().len() != net.loads().len()
        {
            let before = source_equivalents::contract_source_pairs(&expected);
            let after = source_equivalents::contract_source_pairs(&reparsed);
            let differences = electrical_projection::differences(&before, &after);
            assert!(
                differences.is_empty(),
                "{}: {}",
                case.label,
                differences.join("; ")
            );
        }
        let second = helpers::emit_value(&reparsed, Fmt::Ravens.target());
        if first.text != second.text {
            let (a, b): (Vec<&str>, Vec<&str>) =
                (first.text.lines().collect(), second.text.lines().collect());
            let at = a
                .iter()
                .zip(&b)
                .position(|(x, y)| x != y)
                .unwrap_or(a.len().min(b.len()));
            let window =
                |lines: &[&str]| lines[at.saturating_sub(3)..(at + 4).min(lines.len())].join("\n");
            panic!(
                "{} -> {}: canonical output is not idempotent at line {}:\n--- first\n{}\n--- second\n{}",
                case.label,
                Fmt::Ravens.name(),
                at + 1,
                window(&a),
                window(&b)
            );
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
            if target == Fmt::Dss {
                assert_dss_equivalent(&net, &back, case, "cross-format write");
                continue;
            }
            for load in net.loads().iter().filter(|l| {
                matches!(
                    l.voltage_model,
                    DistLoadVoltageModel::BoundedZip { .. }
                        | DistLoadVoltageModel::BoundedExponential { .. }
                )
            }) {
                assert!(
                    out.warnings
                        .iter()
                        .any(|w| w.contains(&format!("load {}:", load.name))
                            && w.contains("voltage-band transitions")),
                    "{what}: lost voltage band without disclosure"
                );
                if target == Fmt::Pmd
                    && matches!(
                        load.voltage_model,
                        DistLoadVoltageModel::BoundedExponential { .. }
                    )
                {
                    assert!(
                        out.warnings
                            .iter()
                            .any(|w| w.contains(&format!("load {}:", load.name))
                                && w.contains("exponential load model")),
                        "{what}: lost exponents without disclosure"
                    );
                }
            }
            let transformers = !(target == Fmt::Bmopf && case.bmopf_restates_transformers);
            let (expected, actual) = if target == Fmt::Bmopf {
                (
                    normalize_bmopf_bus_metadata(&net, &back),
                    normalize_bmopf_bus_metadata(&back, &back),
                )
            } else {
                ((*net).clone(), (*back).clone())
            };
            source_equivalents::assert_source_constraints(&expected, &actual, &what);
            assert_projection_eq(&expected, &actual, &what, transformers);
            assert_linecodes_close(&expected, &actual, &what);
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

/// Supplied conductor positions form a bijection per bus across OpenDSS and
/// PMD renumbering. OpenDSS can append a grounded neutral to a WYE generator
/// whose map lists only its phases; that addition must carry a diagnostic.
#[test]
fn renumbering_legs_are_position_stable_bijections() {
    for case in CASES {
        let net = parse_case(case);
        let before = terminal_maps(&net);
        for target in [Fmt::Dss, Fmt::Pmd] {
            let conv = match target {
                Fmt::Dss => helpers::emit_dss(&net),
                Fmt::Pmd => helpers::emit_pmd_json(&net),
                Fmt::Bmopf | Fmt::Ravens => unreachable!("not a renumbering leg"),
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
                let grounded_generator_return = matches!(target, Fmt::Dss)
                    && key.starts_with("generator ")
                    && target_map.len() == source_map.len() + 1
                    && round_tripped
                        .bus(bus)
                        .is_some_and(|b| target_map.last().is_some_and(|t| b.grounded.contains(t)))
                    && conv
                        .warnings
                        .iter()
                        .any(|d| d.starts_with("EMIT.DSS.VALUE_COLLAPSED:") && d.contains(key));
                assert!(
                    source_map.len() == target_map.len() || grounded_generator_return,
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

#[test]
fn source_equivalence_checks_reject_corrupted_circuits() {
    let original = helpers::parse_dss_str(
        "New Circuit.c phases=1 basekv=0.24 bus1=feed.1 r1=3 x1=4 r0=3 x0=4\n",
    );
    let emitted = helpers::emit_dss(&original);
    let round_trip = Fmt::Dss.parse_emission(&emitted).unwrap();
    let restored = source_equivalents::contract_source_pairs(&round_trip);
    source_equivalents::assert_source_constraints(&original, &restored, "uncorrupted");
    for fault in ["impedance", "shunt", "ground", "angle", "connection"] {
        let mut changed = (*round_trip).clone();
        let source_bus = changed.sources()[0].bus.clone();
        let line = changed
            .lines()
            .iter()
            .find(|l| l.bus_from == source_bus)
            .unwrap()
            .clone();
        match fault {
            "impedance" | "shunt" => {
                let code = changed
                    .line_codes_mut()
                    .iter_mut()
                    .find(|c| c.name == line.linecode)
                    .unwrap();
                if fault == "impedance" {
                    code.r_series[0][0] += 0.01;
                } else {
                    code.b_from[0][0] += 0.01;
                }
            }
            "ground" => changed
                .buses_mut()
                .iter_mut()
                .find(|b| b.id == line.bus_to)
                .unwrap()
                .grounded
                .push(line.terminal_map_to[0].clone()),
            "angle" => changed.sources_mut()[0].v_angle[0] += 0.1,
            "connection" => changed.loads_mut().push(powerio_dist::DistLoad::new(
                "extra",
                &line.bus_to,
                line.terminal_map_to.clone(),
                powerio_dist::Configuration::SinglePhase,
                vec![1.0],
                vec![0.0],
            )),
            _ => unreachable!(),
        }
        let changed = source_equivalents::contract_source_pairs(&changed);
        assert!(
            std::panic::catch_unwind(|| {
                source_equivalents::assert_source_constraints(&original, &changed, fault);
            })
            .is_err(),
            "accepted {fault} corruption"
        );
    }
}

#[test]
fn ideal_transformer_contraction_requires_electrical_cancellation() {
    let source = "New Circuit.c phases=1 bus1=source.1.0 basekv=2.4\n\
        New Transformer.x phases=1 windings=2 buses=(source.1.0 internal.1.2) \
        conns=(wye wye) kvs=(2.4 .24) kvas=(10 10) \
        %rs=(0 17.36111111111111) xscarray=(0) ppm_antifloat=0\n\
        New Line.cancel phases=2 bus1=internal.1.2 bus2=load.1.2 length=1 units=m \
        rmatrix=(-.5 | 0 -.5) xmatrix=(0 | 0 0) cmatrix=(0 | 0 0)\n";
    let parse = |text: &str| Fmt::Dss.parse_text_and_sidecars(text, &[]).unwrap();
    let original = parse(source);
    let contracted = source_equivalents::contract_transformer_pairs(&original);
    assert_eq!(contracted.lines().len() + 1, original.lines().len());
    let winding = &contracted.transformers()[0].windings[1];
    assert_eq!(winding.bus, "load");
    assert_eq!(winding.terminal_map, ["1", "2"]);
    assert_eq!(winding.r_pct.to_bits(), 0.0_f64.to_bits());
    assert!(contracted.bus("internal").is_none());
    for invalid in [
        source.replace("(-.5 | 0 -.5)", "(-.4 | 0 -.5)"),
        source.replace("cmatrix=(0 | 0 0)", "cmatrix=(1 | 0 1)"),
        format!("{source}New Load.extra phases=1 bus1=internal.1.2 kv=.24 kw=1\n"),
    ] {
        let original = parse(&invalid);
        let contracted = source_equivalents::contract_transformer_pairs(&original);
        assert_eq!(contracted.lines().len(), original.lines().len());
        assert!(contracted.bus("internal").is_some());
    }
}
