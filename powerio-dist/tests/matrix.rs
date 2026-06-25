//! The 3x3 conversion harness: diagonal byte identity via the retained
//! source, canonical writer idempotence, and off diagonal round trips with
//! the lossy transforms named per cell. `cargo test --test matrix --
//! --ignored write_conversion_matrix` regenerates docs/conversion-matrix.md.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use powerio_dist::{
    DistLoadVoltageModel, DistNetwork, DistTargetFormat, Result, parse_bmopf_str, parse_dss_file,
    parse_pmd_str,
};

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

    fn parse(self, text: &str) -> Result<DistNetwork> {
        match self {
            Fmt::Dss => {
                // Unique path per call: the harness tests run in parallel
                // threads and must not race on a shared temp file.
                use std::sync::atomic::{AtomicU64, Ordering};
                static COUNTER: AtomicU64 = AtomicU64::new(0);
                let dir = std::env::temp_dir().join("powerio-dist-matrix");
                std::fs::create_dir_all(&dir).unwrap();
                let path = dir.join(format!(
                    "roundtrip-{}.dss",
                    COUNTER.fetch_add(1, Ordering::Relaxed)
                ));
                std::fs::write(&path, text).unwrap();
                let parsed = powerio_dist::dss::parse_dss_file(&path);
                let _ = std::fs::remove_file(&path);
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

fn parse_case(case: &Case) -> DistNetwork {
    let path = fixture(case.rel);
    match case.fmt {
        Fmt::Dss => parse_dss_file(&path).unwrap(),
        Fmt::Bmopf => powerio_dist::parse_bmopf_file(&path).unwrap(),
        Fmt::Pmd => powerio_dist::parse_pmd_file(&path).unwrap(),
    }
}

fn by_name<'a, T>(items: &'a [T], name: impl Fn(&'a T) -> &'a str) -> Vec<(&'a str, &'a T)> {
    let mut v: Vec<(&str, &T)> = items.iter().map(|t| (name(t), t)).collect();
    v.sort_by_key(|(n, _)| n.to_ascii_lowercase());
    v
}

fn same_v_nom(a: &[f64], b: &[f64], allow_missing: bool) -> bool {
    a == b || (allow_missing && (a.is_empty() || b.is_empty()))
}

fn same_load_voltage_model(
    a: &DistLoadVoltageModel,
    b: &DistLoadVoltageModel,
    allow_missing_v_nom: bool,
) -> bool {
    match (a, b) {
        (DistLoadVoltageModel::ConstantPower, DistLoadVoltageModel::ConstantPower) => true,
        (
            DistLoadVoltageModel::ConstantCurrent { v_nom: a },
            DistLoadVoltageModel::ConstantCurrent { v_nom: b },
        )
        | (
            DistLoadVoltageModel::ConstantImpedance { v_nom: a },
            DistLoadVoltageModel::ConstantImpedance { v_nom: b },
        ) => same_v_nom(a, b, allow_missing_v_nom),
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
            same_v_nom(av, bv, allow_missing_v_nom)
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
        ) => same_v_nom(av, bv, allow_missing_v_nom) && ap == bp && aq == bq,
        _ => false,
    }
}

fn close_power(x: f64, y: f64) -> bool {
    (x - y).abs() <= 4.0 * f64::EPSILON * x.abs().max(y.abs())
}

fn assert_loads_eq(a: &DistNetwork, b: &DistNetwork, what: &str, allow_missing_v_nom: bool) {
    assert_eq!(a.loads.len(), b.loads.len(), "{what}: loads");
    for ((_, x), (_, y)) in by_name(&a.loads, |l| &l.name)
        .iter()
        .zip(&by_name(&b.loads, |l| &l.name))
    {
        for (p, q) in x.p_nom.iter().zip(&y.p_nom) {
            assert!(close_power(*p, *q), "{what}: load {} p {p} vs {q}", x.name);
        }
        for (p, q) in x.q_nom.iter().zip(&y.q_nom) {
            assert!(close_power(*p, *q), "{what}: load {} q {p} vs {q}", x.name);
        }
        assert_eq!(
            x.terminal_map, y.terminal_map,
            "{what}: load {} map",
            x.name
        );
        assert!(
            same_load_voltage_model(&x.voltage_model, &y.voltage_model, allow_missing_v_nom),
            "{what}: load {} voltage model {:?} vs {:?}",
            x.name,
            x.voltage_model,
            y.voltage_model
        );
    }
}

fn legacy_dss_pmd_v_nom_loss(what: &str) -> bool {
    matches!(
        what,
        "IEEE 13 → PMD → back"
            | "IEEE 34 → PMD → back"
            | "IEEE 123 → PMD → back"
            | "PMD IEEE 13 → dss → back"
    )
}

/// The model fields every format carries; the per cell comparisons run on
/// this projection, with transformer carve outs where BMOPF restates them.
fn assert_projection_eq(a: &DistNetwork, b: &DistNetwork, what: &str, transformers: bool) {
    // JSON formats key elements by name, so order is not preserved across
    // a round trip; compare per name.
    assert_eq!(a.buses.len(), b.buses.len(), "{what}: bus count");
    let buses_a = by_name(&a.buses, |b| &b.id);
    let buses_b = by_name(&b.buses, |b| &b.id);
    for ((_, x), (_, y)) in buses_a.iter().zip(&buses_b) {
        assert!(x.id.eq_ignore_ascii_case(&y.id), "{what}: bus set");
        assert_eq!(x.terminals, y.terminals, "{what}: bus {} terminals", x.id);
        assert_eq!(x.grounded, y.grounded, "{what}: bus {} grounding", x.id);
    }
    assert_eq!(a.switches.len(), b.switches.len(), "{what}: switches");
    for ((_, x), (_, y)) in by_name(&a.switches, |s| &s.name)
        .iter()
        .zip(&by_name(&b.switches, |s| &s.name))
    {
        assert_eq!(x.open, y.open, "{what}: switch {}", x.name);
    }
    // Scale changes (kW to W and back) cost at most one rounding per
    // direction; powers compare to 2 ULP relative, everything structural
    // exactly.
    let allow_missing_v_nom = legacy_dss_pmd_v_nom_loss(what);
    assert_loads_eq(a, b, what, allow_missing_v_nom);
    assert_eq!(a.lines.len(), b.lines.len(), "{what}: lines");
    for ((_, x), (_, y)) in by_name(&a.lines, |l| &l.name)
        .iter()
        .zip(&by_name(&b.lines, |l| &l.name))
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
        assert_eq!(
            x.terminal_map_from, y.terminal_map_from,
            "{what}: line {} from map",
            x.name
        );
        assert_eq!(
            x.terminal_map_to, y.terminal_map_to,
            "{what}: line {} to map",
            x.name
        );
    }
    if transformers {
        assert_eq!(
            a.transformers.len(),
            b.transformers.len(),
            "{what}: transformers"
        );
        for ((_, x), (_, y)) in by_name(&a.transformers, |t| &t.name)
            .iter()
            .zip(&by_name(&b.transformers, |t| &t.name))
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
fn assert_linecodes_close(a: &DistNetwork, b: &DistNetwork, what: &str) {
    assert_eq!(a.linecodes.len(), b.linecodes.len(), "{what}: linecodes");
    let close = |x: f64, y: f64| (x - y).abs() <= 1e-12 * x.abs().max(y.abs()).max(1e-300);
    let mut xs: Vec<_> = a.linecodes.iter().collect();
    let mut ys: Vec<_> = b.linecodes.iter().collect();
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
fn normalize_grounded(net: &DistNetwork) -> DistNetwork {
    let mut net = net.clone();
    let grounded: BTreeMap<String, Vec<String>> = net
        .buses
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
    for b in &mut net.buses {
        let g = b.grounded.clone();
        for t in b.terminals.iter_mut().chain(b.grounded.iter_mut()) {
            if g.contains(t) {
                *t = "G".to_string();
            }
        }
    }
    for l in &mut net.lines {
        fix(&l.bus_from.clone(), &mut l.terminal_map_from);
        fix(&l.bus_to.clone(), &mut l.terminal_map_to);
    }
    for s in &mut net.switches {
        fix(&s.bus_from.clone(), &mut s.terminal_map_from);
        fix(&s.bus_to.clone(), &mut s.terminal_map_to);
    }
    for l in &mut net.loads {
        fix(&l.bus.clone(), &mut l.terminal_map);
    }
    for t in &mut net.transformers {
        for w in &mut t.windings {
            fix(&w.bus.clone(), &mut w.terminal_map);
        }
    }
    net
}

fn normalize_bmopf_bus_metadata(net: &DistNetwork, usage_net: &DistNetwork) -> DistNetwork {
    let mut net = net.clone();
    let mut usage: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut add = |bus: &str, terms: &[String]| {
        usage
            .entry(bus.to_string())
            .or_default()
            .extend(terms.iter().cloned());
    };
    for l in &usage_net.lines {
        add(&l.bus_from, &l.terminal_map_from);
        add(&l.bus_to, &l.terminal_map_to);
    }
    for s in &usage_net.switches {
        add(&s.bus_from, &s.terminal_map_from);
        add(&s.bus_to, &s.terminal_map_to);
    }
    for l in &usage_net.loads {
        add(&l.bus, &l.terminal_map);
    }
    for g in &usage_net.generators {
        add(&g.bus, &g.terminal_map);
    }
    for s in &usage_net.shunts {
        add(&s.bus, &s.terminal_map);
    }
    for s in &usage_net.sources {
        add(&s.bus, &s.terminal_map);
    }
    for t in &usage_net.transformers {
        for w in &t.windings {
            add(&w.bus, &w.terminal_map);
        }
    }

    net.buses.retain(|b| usage.contains_key(&b.id));
    for b in &mut net.buses {
        let Some(used) = usage.get(&b.id) else {
            continue;
        };
        b.terminals.retain(|term| used.contains(term));
        b.grounded.retain(|term| used.contains(term));
    }
    net
}

#[test]
fn diagonal_byte_identity() {
    for case in CASES {
        let net = parse_case(case);
        let original = std::fs::read_to_string(fixture(case.rel)).unwrap();
        let echoed = net.to_format(case.fmt.target());
        assert_eq!(echoed.text, original, "{}: diagonal echo", case.label);
        assert!(echoed.warnings.is_empty(), "{}: echo warns", case.label);
    }
}

#[test]
fn canonical_writers_are_idempotent() {
    for case in CASES {
        let net = parse_case(case);
        for target in [Fmt::Dss, Fmt::Bmopf, Fmt::Pmd] {
            let first = match target {
                Fmt::Dss => powerio_dist::write_dss(&net),
                Fmt::Bmopf => powerio_dist::write_bmopf_json(&net),
                Fmt::Pmd => powerio_dist::write_pmd_json(&net),
            };
            let reparsed = match target.parse(&first.text) {
                Ok(n) => n,
                Err(e) => panic!("{} → {}: reparse failed: {e}", case.label, target.name()),
            };
            let second = match target {
                Fmt::Dss => powerio_dist::write_dss(&reparsed),
                Fmt::Bmopf => powerio_dist::write_bmopf_json(&reparsed),
                Fmt::Pmd => powerio_dist::write_pmd_json(&reparsed),
            };
            assert_eq!(
                first.text,
                second.text,
                "{} → {}: canonical output is not idempotent",
                case.label,
                target.name()
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
            let out = net.to_format(target.target());
            let back = target
                .parse(&out.text)
                .unwrap_or_else(|e| panic!("{what}: {e}"));
            let transformers = !(target == Fmt::Bmopf && case.bmopf_restates_transformers);
            let (expected, actual) = if target == Fmt::Bmopf {
                (
                    normalize_bmopf_bus_metadata(&net, &back),
                    normalize_bmopf_bus_metadata(&back, &back),
                )
            } else {
                (net.clone(), back)
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
            let out = net.to_format(target.target());
            match target.parse(&out.text) {
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
        let dss = powerio_dist::write_dss(&net);
        std::fs::write(dir.join(format!("{stem}.canonical.dss")), &dss.text).unwrap();
        if case.fmt == Fmt::Dss {
            // Through each JSON format and back to dss.
            for (suffix, text) in [
                ("via_bmopf", powerio_dist::write_bmopf_json(&net).text),
                ("via_pmd", powerio_dist::write_pmd_json(&net).text),
            ] {
                let mid: DistNetwork = if suffix == "via_bmopf" {
                    parse_bmopf_str(&text).unwrap()
                } else {
                    parse_pmd_str(&text).unwrap()
                };
                let out = powerio_dist::write_dss(&mid);
                std::fs::write(dir.join(format!("{stem}.{suffix}.dss")), &out.text).unwrap();
            }
        }
    }
    let _ = Arc::new(());
}
