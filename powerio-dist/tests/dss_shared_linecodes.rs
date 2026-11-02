//! OpenDSS Lines that carry an unchanged LineCode share one per-meter
//! linecode named after it; Lines whose primitive differs keep their own.

mod helpers;
use helpers::{emit_value, parse_bmopf_str, parse_dss_str, parse_pmd_str};
use powerio_dist::{DistLineCode, DistTargetFormat, MulticonductorNetwork};
use std::f64::consts::TAU;

/// `lc` is edited between `other` and `d`, and a declared `lc_1` takes the
/// first suffix the edited prefix would otherwise use.
const EDITED: &str = "New Circuit.c
New Linecode.lc nphases=1 rmatrix=[1] xmatrix=[2] cmatrix=[3] units=km
New Linecode.lc_1 nphases=1 rmatrix=[5] xmatrix=[6] cmatrix=[0] units=km
New Line.a bus1=sourcebus.1 bus2=a.1 linecode=lc length=10 units=m
New Line.b bus1=a.1 bus2=b.1 linecode=lc length=0.25 units=km
New Line.c bus1=b.1 bus2=c.1 linecode=lc length=3
New Line.other bus1=c.1 bus2=o.1 linecode=lc_1 length=7 units=m
Edit Linecode.lc rmatrix=[7]
New Line.d bus1=c.1 bus2=d.1 linecode=lc length=5 units=m
New Line.e bus1=d.1 bus2=e.1 linecode=lc length=6 units=m
New Line.open bus1=e.1 bus2=f.1 linecode=lc length=8 units=m
Open Line.open 2 1
";

fn code<'a>(net: &'a MulticonductorNetwork, line: &str) -> &'a DistLineCode {
    let line = net
        .lines()
        .iter()
        .find(|l| l.name.eq_ignore_ascii_case(line))
        .unwrap_or_else(|| panic!("line {line}"));
    net.linecode(&line.linecode).unwrap()
}

/// The codes named after a source LineCode, without generated ones.
fn line_codes(net: &MulticonductorNetwork) -> Vec<&str> {
    net.line_codes()
        .iter()
        .map(|c| c.name.as_str())
        .filter(|name| !name.starts_with("_pio_"))
        .collect()
}

#[test]
fn lines_share_one_code_per_linecode_prefix() {
    let net = parse_dss_str(EDITED);
    for line in ["a", "b", "c"] {
        assert_eq!(code(&net, line).name, "lc", "line {line}");
    }
    assert_eq!(code(&net, "other").name, "lc_1");
    for line in ["d", "e"] {
        assert_eq!(code(&net, line).name, "lc_2", "line {line}");
    }
    // The open Line's ports reduce through the code its prefix shares.
    assert!(net.lines().iter().all(|l| l.name != "open"));
    assert_eq!(line_codes(&net), ["lc", "lc_1", "lc_2"]);

    // Per-km values over 1000 m, each divided once.
    let lc = code(&net, "a");
    assert_eq!(lc.r_series[0][0].to_bits(), (1.0_f64 / 1000.0).to_bits());
    assert_eq!(lc.x_series[0][0].to_bits(), (2.0_f64 / 1000.0).to_bits());
    let b = 3.0 * (TAU * 60.0 * 1e-9) / 1000.0 * 0.5;
    assert_eq!(lc.b_from[0][0].to_bits(), b.to_bits());
    assert_eq!(lc.b_to, lc.b_from);
    assert!(lc.source.as_deref().unwrap().contains("LineCode.lc "));
    assert_eq!(
        code(&net, "d").r_series[0][0].to_bits(),
        (7.0_f64 / 1000.0).to_bits()
    );

    // Each Line keeps its native total impedance to within an ulp or two.
    for (line, ohms) in [("a", 0.01), ("b", 0.25), ("c", 3.0), ("d", 0.035)] {
        let length = net.lines().iter().find(|l| l.name == line).unwrap().length;
        let total = code(&net, line).r_series[0][0] * length;
        assert!(
            (total - ohms).abs() <= 2.0 * f64::EPSILON * ohms,
            "line {line}: {total} != {ohms}"
        );
    }
}

#[test]
fn changed_primitives_keep_their_own_code() {
    let net = parse_dss_str(
        "New Circuit.c
New Linecode.lc nphases=1 rmatrix=[1] xmatrix=[2] cmatrix=[0] units=km
New Line.plain bus1=sourcebus.1 bus2=p.1 linecode=lc length=1 units=m
New Line.matrix bus1=p.1 bus2=m.1 linecode=lc rmatrix=[3] length=1 units=m
New Line.sequence bus1=p.1 bus2=s.1 r1=0.5 linecode=lc length=1 units=m
New Line.frequency bus1=p.1 bus2=f.1 linecode=lc basefreq=50 length=1 units=m
New Line.earth bus1=p.1 bus2=g.1 linecode=lc rg=0.5 length=1 units=m
New Line.copy bus1=p.1 bus2=k.1 like=plain length=1 units=m
New Line.later bus1=p.1 bus2=l.1 phases=1 linecode=lc length=1 units=m
Edit Line.later x1=4
New Line.before bus1=p.1 bus2=r.1 phases=1 rmatrix=[3] linecode=lc length=1 units=m
",
    );
    for line in ["plain", "before"] {
        assert_eq!(code(&net, line).name, "lc", "line {line}");
    }
    for line in ["matrix", "sequence", "frequency", "earth", "copy", "later"] {
        assert_eq!(code(&net, line).name, format!("_pio_line_{line}"));
    }
    let plain = code(&net, "plain");
    assert_ne!(code(&net, "matrix").r_series, plain.r_series);
    assert_ne!(code(&net, "later").x_series, plain.x_series);
}

#[test]
fn a_unitless_linecode_shares_per_line_unit() {
    let net = parse_dss_str(
        "New Circuit.c
New Linecode.free nphases=1 rmatrix=[1] xmatrix=[2] cmatrix=[0]
New Line.m bus1=sourcebus.1 bus2=m.1 linecode=free length=10 units=m
New Line.ft bus1=m.1 bus2=f.1 linecode=free length=10 units=ft
New Line.m2 bus1=f.1 bus2=n.1 linecode=free length=3 units=m
New Line.none bus1=n.1 bus2=z.1 linecode=free length=2
",
    );
    assert_eq!(code(&net, "m").name, "free");
    assert_eq!(code(&net, "m2").name, "free");
    assert_eq!(code(&net, "m").r_series[0][0].to_bits(), 1.0_f64.to_bits());
    let ft = code(&net, "ft");
    assert_eq!(ft.name, "free_1");
    assert_eq!(ft.r_series[0][0].to_bits(), (1.0_f64 / 0.3048).to_bits());
    // No declared distance: the one-meter equivalent stays per Line.
    assert_eq!(code(&net, "none").name, "_pio_line_none");
}

#[test]
fn linecode_lines_share_beside_spacing_lines() {
    let net = parse_dss_str(
        "new circuit.c
new wiredata.w rac=.0002 radius=.01 radunits=m runits=m
new linespacing.s nconds=1 nphases=1 units=m x=[0] h=[10]
new linecode.lc nphases=1 rmatrix=[1] xmatrix=[2] cmatrix=[0] units=km
new line.g bus1=sourcebus.1 bus2=g.1 spacing=s wires=[w] length=100 units=m
new line.a bus1=sourcebus.1 bus2=a.1 linecode=lc length=10 units=m
new line.b bus1=a.1 bus2=b.1 linecode=lc length=20 units=m
new line.later bus1=b.1 bus2=c.1 linecode=lc spacing=s wires=[w] length=5 units=m
",
    );
    assert_eq!(code(&net, "a").name, "lc");
    assert_eq!(code(&net, "b").name, "lc");
    assert_eq!(code(&net, "g").name, "_pio_line_g");
    assert_eq!(code(&net, "later").name, "_pio_line_later");
}

type Reparse = fn(&str) -> MulticonductorNetwork;

#[test]
fn shared_codes_survive_dss_bmopf_and_pmd_round_trips() {
    let net = parse_dss_str(EDITED);
    let reparse: [(DistTargetFormat, Reparse); 3] = [
        (DistTargetFormat::Dss, |text| parse_dss_str(text).clone()),
        (DistTargetFormat::BmopfJson, |text| {
            parse_bmopf_str(text).unwrap().clone()
        }),
        (DistTargetFormat::PmdJson, |text| {
            parse_pmd_str(text).unwrap().clone()
        }),
    ];
    for (target, reparse) in reparse {
        let back = reparse(&emit_value(&net, target).text);
        assert_eq!(line_codes(&back), ["lc", "lc_1", "lc_2"], "{target:?}");
        for line in ["a", "b", "c", "other", "d", "e"] {
            let (before, after) = (code(&net, line), code(&back, line));
            assert_eq!(before.name, after.name, "{target:?} line {line}");
            for (x, y) in [
                (&before.r_series, &after.r_series),
                (&before.x_series, &after.x_series),
                (&before.b_from, &after.b_from),
            ] {
                let (x, y) = (x[0][0], y[0][0]);
                assert!(
                    (x - y).abs() <= 1e-12 * x.abs(),
                    "{target:?} line {line}: {x} != {y}"
                );
            }
        }
        // Written again, a reread case keeps the same names.
        let again = reparse(&emit_value(&back, target).text);
        assert_eq!(line_codes(&again), ["lc", "lc_1", "lc_2"], "{target:?}");
    }
}
