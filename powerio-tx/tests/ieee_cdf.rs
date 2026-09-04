//! The vendored IEEE 14 and 30 bus CDF cases against the MATPOWER cases that
//! derive from the same IEEE data.
#![allow(clippy::float_cmp, clippy::too_many_lines)]

mod helpers;
#[allow(unused_imports)]
use helpers::*;

use std::path::PathBuf;

use powerio_tx::{BalancedNetwork, BusId, BusType, SourceFormat, TargetFormat};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data")
        .join(name)
}

fn temp_path(name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "powerio-ieee-cdf-test-{}-{}-{name}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    path
}

fn cdf(name: &str) -> Parsed {
    parse_file(fixture(&format!("ieee-cdf/{name}")), None).unwrap()
}

fn matpower(name: &str) -> BalancedNetwork {
    parse_file(fixture(name), None).unwrap().network
}

fn branch_keys(net: &BalancedNetwork) -> Vec<(BusId, BusId)> {
    net.branches().iter().map(|b| (b.from, b.to)).collect()
}

fn loads_by_bus(net: &BalancedNetwork) -> Vec<(BusId, f64, f64)> {
    let mut loads: Vec<_> = net.loads().iter().map(|l| (l.bus, l.p, l.q)).collect();
    loads.sort_by_key(|(bus, _, _)| *bus);
    loads
}

fn generators_by_bus(net: &BalancedNetwork) -> Vec<(BusId, f64, f64)> {
    let mut generators: Vec<_> = net
        .generators()
        .iter()
        .map(|g| (g.bus, g.pg, g.qg))
        .collect();
    generators.sort_by_key(|(bus, _, _)| *bus);
    generators
}

/// `case14.m` is `ieee14cdf.txt` converted by MATPOWER's `cdf2matp`, so the
/// two agree on every value the CDF states. The converter adds what the CDF
/// lacks: voltage limits of 1.06/0.94, `Pmax = Pg + baseMVA`, `Pmin = 0`,
/// `Qmax = Qmin + 10` at bus 1 where the CDF states 0/0, and a synthesized
/// cost table. Those are the only differences.
#[test]
fn ieee14_matches_case14_where_the_cdf_states_a_value() {
    let parsed = cdf("ieee14cdf.txt");
    let cdf = &parsed.network;
    let reference = matpower("case14.m");

    assert_eq!(cdf.source_format(), SourceFormat::IeeeCdf);
    assert_eq!(cdf.name(), "ieee14cdf");
    assert_eq!(cdf.base_mva(), reference.base_mva());
    assert_eq!(cdf.case_metadata().case_date.as_deref(), Some("1993-08-19"));

    assert_eq!(cdf.buses().len(), 14);
    assert_eq!(cdf.branches().len(), 20);
    assert_eq!(cdf.generators().len(), 5);
    assert_eq!(cdf.buses().len(), reference.buses().len());
    assert_eq!(cdf.branches().len(), reference.branches().len());
    assert_eq!(cdf.generators().len(), reference.generators().len());
    assert_eq!(cdf.loads().len(), reference.loads().len());
    assert_eq!(cdf.shunts().len(), reference.shunts().len());

    for (bus, expected) in cdf.buses().iter().zip(reference.buses()) {
        assert_eq!(bus.id, expected.id);
        assert_eq!(bus.kind, expected.kind, "bus {}", bus.id);
        assert_eq!(
            (bus.vm, bus.va),
            (expected.vm, expected.va),
            "bus {}",
            bus.id
        );
        assert_eq!(bus.base_kv, expected.base_kv, "bus {}", bus.id);
        assert_eq!((bus.area, bus.zone), (expected.area, expected.zone));
        assert_eq!(bus.name, expected.name, "bus {}", bus.id);
        // cdf2matp writes 1.06/0.94; the CDF states no voltage limits and
        // the reader keeps the model defaults.
        assert_eq!((bus.vmax, bus.vmin), (1.1, 0.9));
        assert_eq!((expected.vmax, expected.vmin), (1.06, 0.94));
    }
    assert_eq!(loads_by_bus(cdf), loads_by_bus(&reference));
    assert_eq!(
        cdf.shunts()
            .iter()
            .map(|s| (s.bus, s.g, s.b))
            .collect::<Vec<_>>(),
        reference
            .shunts()
            .iter()
            .map(|s| (s.bus, s.g, s.b))
            .collect::<Vec<_>>()
    );

    assert_eq!(branch_keys(cdf), branch_keys(&reference));
    for (branch, expected) in cdf.branches().iter().zip(reference.branches()) {
        let key = format!("branch {}-{}", branch.from, branch.to);
        assert_eq!(
            (branch.r, branch.x, branch.b),
            (expected.r, expected.x, expected.b),
            "{key}"
        );
        assert_eq!(branch.tap, expected.tap, "{key}");
        assert_eq!(branch.shift, expected.shift, "{key}");
        assert_eq!(
            (branch.rate_a, branch.rate_b, branch.rate_c),
            (expected.rate_a, expected.rate_b, expected.rate_c),
            "{key}"
        );
        assert!(branch.in_service && expected.in_service, "{key}");
        assert_eq!(
            (branch.angmin, branch.angmax),
            (expected.angmin, expected.angmax)
        );
    }
    assert_eq!(
        cdf.branches().iter().filter(|b| b.is_transformer()).count(),
        3
    );

    assert_eq!(generators_by_bus(cdf), generators_by_bus(&reference));
    for (generator, expected) in cdf.generators().iter().zip(reference.generators()) {
        let key = format!("generator at bus {}", generator.bus);
        assert_eq!(generator.vg, expected.vg, "{key}");
        assert_eq!(generator.mbase, expected.mbase, "{key}");
        assert!(generator.in_service, "{key}");
        assert_eq!(generator.pmin, expected.pmin, "{key}");
        // cdf2matp: Pmax = Pg + baseMVA.
        assert_eq!(generator.pmax, 9999.0, "{key}");
        assert_eq!(expected.pmax, expected.pg + 100.0, "{key}");
        assert_eq!(generator.qmin, expected.qmin, "{key}");
        if generator.bus == BusId(1) {
            // cdf2matp: Qmax = Qmin at bus 1, so Qmax was set to Qmin + 10.
            assert_eq!((generator.qmax, expected.qmax), (0.0, 10.0));
        } else {
            assert_eq!(generator.qmax, expected.qmax, "{key}");
        }
        assert!(generator.cost.is_none(), "{key}");
        assert!(expected.cost.is_some(), "{key}");
    }
    assert_eq!(cdf.areas().len(), 1);
    assert_eq!(cdf.areas()[0].slack_bus, Some(BusId(2)));
    assert_eq!(
        cdf.areas()[0].name.as_deref(),
        Some("IEEE 14 Bus Test Case")
    );

    let rendered = parsed.render_diagnostics();
    assert!(
        rendered
            .iter()
            .all(|line| !line.contains("SOURCE_MALFORMED") && !line.contains("TRUNCATED")),
        "{rendered:?}"
    );
}

/// `case30.m` restates the 30 bus system from Alsac and Stott rather than the
/// archive file: series impedances are rounded to the nearest 0.01 (two
/// values come from a different table: the 1-3 reactance is 0.19 against the
/// archive's 0.1652 and the 16-17 resistance 0.08 against 0.0524), the
/// charging column holds the half line charging B/2 rounded the same way
/// where the archive states the total B, the shunt susceptances are divided
/// by 100 and the bus 10 shunt is moved to bus 5, the load at bus 5 is
/// zeroed, generator locations, dispatch, and limits come from Ferrero et
/// al., every base kV is 135, the transformer taps are left at 0, and line
/// ratings are added. The topology, the element counts, and the remaining
/// loads agree.
#[test]
fn ieee30_matches_case30_up_to_its_documented_edits() {
    let parsed = cdf("ieee30cdf.txt");
    let cdf = &parsed.network;
    let reference = matpower("case30.m");

    assert_eq!(cdf.buses().len(), 30);
    assert_eq!(cdf.branches().len(), 41);
    assert_eq!(cdf.generators().len(), 6);
    assert_eq!(cdf.buses().len(), reference.buses().len());
    assert_eq!(cdf.branches().len(), reference.branches().len());
    assert_eq!(cdf.generators().len(), reference.generators().len());
    assert_eq!(cdf.base_mva(), reference.base_mva());

    assert_eq!(branch_keys(cdf), branch_keys(&reference));
    let restated = [
        ((BusId(1), BusId(3)), "x", 0.1652, 0.19),
        ((BusId(16), BusId(17)), "r", 0.0524, 0.08),
    ];
    for (branch, expected) in cdf.branches().iter().zip(reference.branches()) {
        let key = format!("branch {}-{}", branch.from, branch.to);
        for (value, rounded, what) in [
            (branch.r, expected.r, "r"),
            (branch.x, expected.x, "x"),
            (branch.b / 2.0, expected.b, "b/2"),
        ] {
            if let Some((_, _, archive, case30)) = restated
                .iter()
                .find(|(pair, field, _, _)| *pair == (branch.from, branch.to) && *field == what)
            {
                assert_eq!((value, rounded), (*archive, *case30), "{key} {what}");
                continue;
            }
            assert!(
                (value - rounded).abs() <= 0.005 + 1e-9,
                "{key} {what}: CDF {value} versus case30 {rounded}"
            );
        }
        assert_eq!(expected.tap, 0.0, "{key}");
        assert_eq!(branch.rate_a, 0.0, "{key}");
        assert!(expected.rate_a > 0.0, "{key}");
    }
    let cdf_taps: Vec<_> = cdf
        .branches()
        .iter()
        .filter(|b| b.is_transformer())
        .map(|b| ((b.from, b.to), b.tap))
        .collect();
    assert_eq!(
        cdf_taps,
        [
            ((BusId(6), BusId(9)), 0.978),
            ((BusId(6), BusId(10)), 0.969),
            ((BusId(4), BusId(12)), 0.932),
            ((BusId(28), BusId(27)), 0.968),
        ]
    );

    let cdf_loads = loads_by_bus(cdf);
    let reference_loads = loads_by_bus(&reference);
    assert_eq!(cdf_loads.len(), reference_loads.len() + 1);
    assert!(cdf_loads.contains(&(BusId(5), 94.2, 19.0)));
    assert!(reference_loads.iter().all(|(bus, _, _)| *bus != BusId(5)));
    assert_eq!(
        cdf_loads
            .iter()
            .filter(|(bus, _, _)| *bus != BusId(5))
            .copied()
            .collect::<Vec<_>>(),
        reference_loads
    );

    assert_eq!(
        cdf.shunts()
            .iter()
            .map(|s| (s.bus, s.b))
            .collect::<Vec<_>>(),
        [(BusId(10), 19.0), (BusId(24), 4.3)]
    );
    assert_eq!(
        reference
            .shunts()
            .iter()
            .map(|s| (s.bus, s.b))
            .collect::<Vec<_>>(),
        [(BusId(5), 0.19), (BusId(24), 0.04)]
    );

    let cdf_generators: Vec<_> = generators_by_bus(cdf)
        .into_iter()
        .map(|(bus, _, _)| bus)
        .collect();
    let reference_generators: Vec<_> = generators_by_bus(&reference)
        .into_iter()
        .map(|(bus, _, _)| bus)
        .collect();
    assert_eq!(cdf_generators, [1, 2, 5, 8, 11, 13].map(BusId));
    assert_eq!(reference_generators, [1, 2, 13, 22, 23, 27].map(BusId));
    assert_eq!(
        generators_by_bus(cdf)[0],
        (BusId(1), 260.2, -16.1),
        "the archive dispatch at the slack bus"
    );
    assert_eq!(cdf.buses()[0].kind, BusType::Ref);
    assert_eq!(cdf.buses()[0].base_kv, 132.0);
    assert!(reference.buses().iter().all(|b| b.base_kv == 135.0));

    // The archive copy places its one interchange record after the `-9`
    // terminator, so it is reported and no area is read.
    assert!(cdf.areas().is_empty());
    let malformed: Vec<_> = parsed
        .diagnostics
        .iter()
        .filter(|d| d.code() == "READ.IEEE_CDF.SOURCE_MALFORMED")
        .collect();
    assert_eq!(malformed.len(), 2, "{:?}", parsed.render_diagnostics());
    assert!(malformed[1].message().contains("record outside a section"));
    assert_eq!(malformed[1].spans().len(), 1);
}

/// A `.txt` name is inferred from the title card; the declared token and
/// its aliases route any name.
#[test]
fn detection_by_extension_and_declared_format() {
    let parsed = parse_file(fixture("ieee-cdf/ieee14cdf.txt"), None).unwrap();
    assert_eq!(parsed.network.source_format(), SourceFormat::IeeeCdf);

    let text = std::fs::read_to_string(fixture("ieee-cdf/ieee14cdf.txt")).unwrap();
    for token in ["ieee-cdf", "IEEE_CDF", "cdf"] {
        let parsed = parse_str(&text, token).unwrap();
        assert_eq!(
            parsed.network.source_format(),
            SourceFormat::IeeeCdf,
            "{token}"
        );
        assert_eq!(parsed.network.buses().len(), 14, "{token}");
    }

    let renamed = temp_path("ieee14.cdf");
    std::fs::write(&renamed, &text).unwrap();
    let parsed = parse_file(&renamed, None).unwrap();
    assert_eq!(parsed.network.source_format(), SourceFormat::IeeeCdf);

    // A `.txt` that is not a CDF title card is refused, not misread.
    let other = temp_path("notes.txt");
    std::fs::write(&other, "release notes\nnothing here\n").unwrap();
    let error = parse_file(&other, None).unwrap_err();
    assert!(
        error.to_string().contains("declare a source format"),
        "{error}"
    );
}

/// Cross-format emission from a CDF source writes canonical MATPOWER that
/// reads back to the same network; a same-format target is refused because
/// the format has no writer.
#[test]
fn fresh_matpower_from_a_cdf_source_reads_back() {
    let parsed = cdf("ieee14cdf.txt");
    let emission = parsed.emit(TargetFormat::Matpower).unwrap();
    let back = parse_str(&emission.text, "matpower").unwrap().network;
    assert_eq!(back.buses().len(), 14);
    assert_eq!(back.branches().len(), 20);
    assert_eq!(back.generators().len(), 5);
    assert_eq!(loads_by_bus(&back), loads_by_bus(&parsed.network));
    assert_eq!(generators_by_bus(&back), generators_by_bus(&parsed.network));
    assert_eq!(branch_keys(&back), branch_keys(&parsed.network));
    for (branch, expected) in back.branches().iter().zip(parsed.network.branches()) {
        assert_eq!(
            (branch.r, branch.x, branch.b, branch.tap),
            (expected.r, expected.x, expected.b, expected.tap)
        );
    }
    assert_eq!(back.areas().len(), 1);

    assert_eq!("ieee-cdf".parse::<TargetFormat>().ok(), None);
}
