//! How the PSS/E RAW writer states a MATPOWER case's branches: one circuit id
//! per branch on each bus pair (#939), and a unit ratio branch with line
//! charging as a BRANCH record (#940).

mod helpers;

use std::collections::BTreeMap;
use std::path::Path;

use helpers::{emit_psse_rev, parse_matpower_file, parse_psse};

fn case(name: &str) -> powerio_tx::BalancedNetwork {
    parse_matpower_file(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/data")
            .join(name),
    )
    .unwrap_or_else(|error| panic!("parse {name}: {error}"))
}

/// The records of one RAW section, from its BEGIN line to its END line.
fn section<'a>(raw: &'a str, name: &str) -> Vec<&'a str> {
    raw.lines()
        .skip_while(|line| !line.contains(&format!("BEGIN {name} DATA")))
        .skip(1)
        .take_while(|line| !line.starts_with("0 /"))
        .collect()
}

fn field(line: &str, index: usize) -> String {
    line.split(',')
        .nth(index)
        .unwrap_or_else(|| panic!("field {index} of {line:?}"))
        .trim()
        .trim_matches('\'')
        .trim()
        .to_owned()
}

/// Every BRANCH record and two winding TRANSFORMER record, keyed by its two
/// buses in ascending order and its circuit id, with how many records state
/// that key. A transformer record's first line is the one whose third field,
/// `K`, is zero; its other three lines have fewer fields.
fn circuit_keys(raw: &str) -> BTreeMap<(u64, u64, String), usize> {
    let mut keys = BTreeMap::new();
    let mut add = |i: String, j: String, ckt: String| {
        let (i, j): (u64, u64) = (i.parse().unwrap(), j.parse().unwrap());
        *keys.entry((i.min(j), i.max(j), ckt)).or_insert(0) += 1;
    };
    for line in section(raw, "BRANCH") {
        add(field(line, 0), field(line, 1), field(line, 2));
    }
    for line in section(raw, "TRANSFORMER") {
        if line.split(',').count() > 12 && field(line, 2) == "0" {
            add(field(line, 0), field(line, 1), field(line, 3));
        }
    }
    keys
}

#[test]
fn case2869pegase_states_each_circuit_once() {
    // case2869pegase lists 3113-5289 and 5289-3113, and 4614-7011 and
    // 7011-4614, each pair once in each direction. PSS/E names a branch by
    // its two buses in either order, so the second of each pair takes '2'.
    let net = case("case2869pegase.m");
    for rev in [33, 34, 35] {
        let raw = emit_psse_rev(&net, rev).text;
        let keys = circuit_keys(&raw);
        let records: usize = keys.values().sum();
        assert_eq!(records, net.branches().len(), "rev {rev}");
        let repeated: Vec<_> = keys.iter().filter(|(_, count)| **count > 1).collect();
        assert!(repeated.is_empty(), "rev {rev}: {repeated:?}");
        for (i, j) in [(3113, 5289), (4614, 7011)] {
            for ckt in ["1", "2"] {
                assert!(
                    keys.contains_key(&(i, j, ckt.to_owned())),
                    "rev {rev}: {i}-{j} circuit {ckt}"
                );
            }
        }

        // The ids survive a round trip through the file.
        let again = emit_psse_rev(&parse_psse(&raw).unwrap(), rev).text;
        assert_eq!(circuit_keys(&again), keys, "rev {rev}");
    }
}

#[test]
fn case118_states_its_charged_unit_ratio_branches_as_lines() {
    // case118 states 68-116 (b = 0.164) and 86-87 (b = 0.0445) with a ratio of
    // exactly 1 and no shift; every other ratio branch is a transformer
    // without charging.
    let net = case("case118.m");
    let emission = emit_psse_rev(&net, 33);
    let raw = &emission.text;
    let branch = |i: usize, j: usize| {
        section(raw, "BRANCH")
            .into_iter()
            .find(|line| field(line, 0) == i.to_string() && field(line, 1) == j.to_string())
    };
    let transformers = section(raw, "TRANSFORMER");
    for (i, j, b) in [(68, 116, 0.164), (86, 87, 0.0445)] {
        let line = branch(i, j).unwrap_or_else(|| panic!("BRANCH record {i}-{j}:\n{raw}"));
        assert!(
            (field(line, 5).parse::<f64>().unwrap() - b).abs() < 1e-12,
            "{line}"
        );
        assert!(
            !transformers
                .iter()
                .any(|line| line.starts_with(&format!("{i}, {j}, 0,"))),
            "{i}-{j} is not a transformer record"
        );
    }
    let ratio_branches = net.branches().iter().filter(|b| b.is_transformer()).count();
    assert_eq!(transformers.len(), 4 * (ratio_branches - 2));

    let rendered = emission.render_diagnostics();
    assert!(
        rendered.iter().any(
            |note| note.starts_with("EMIT.PSSE.ELEMENT_RELABELED: 2 branch(es)")
                && note.contains("(86-87, 68-116)")
        ),
        "{rendered:?}"
    );
    assert!(
        !rendered
            .iter()
            .any(|note| note.contains("winding 1's bus, so")),
        "{rendered:?}"
    );

    // Read back, both branches are lines with half their charging at each end.
    let back = parse_psse(raw).unwrap();
    for (i, j, b) in [(68, 116, 0.164), (86, 87, 0.0445)] {
        let branch = back
            .branches()
            .iter()
            .find(|branch| branch.from.0 == i && branch.to.0 == j)
            .unwrap();
        assert!(!branch.is_transformer());
        let charging = branch.calc_terminal_charging();
        assert!((charging.b_fr - b / 2.0).abs() < 1e-12);
        assert!((charging.b_to - b / 2.0).abs() < 1e-12);
    }
}

/// The `(ibus, jbus, ckt)` of every row of one RAWX table, keeping only the
/// two winding rows (`kbus` 0) of the transformer table.
fn rawx_rows(rawx: &serde_json::Value, table: &str) -> Vec<(u64, u64, String)> {
    let table = &rawx["network"][table];
    let Some(fields) = table["fields"].as_array() else {
        return Vec::new();
    };
    let column = |name: &str| {
        fields
            .iter()
            .position(|field| field == name)
            .unwrap_or_else(|| panic!("RAWX field {name}"))
    };
    let (ibus, jbus, ckt) = (column("ibus"), column("jbus"), column("ckt"));
    let kbus = fields.iter().position(|field| field == "kbus");
    table["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| kbus.is_none_or(|k| row[k].as_u64() == Some(0)))
        .map(|row| {
            (
                row[ibus].as_u64().unwrap(),
                row[jbus].as_u64().unwrap(),
                row[ckt].as_str().unwrap().trim().to_owned(),
            )
        })
        .collect()
}

#[test]
fn rawx_output_states_the_same_branch_records() {
    for (name, restated) in [("case2869pegase.m", None), ("case118.m", Some((68, 116)))] {
        let net = case(name);
        let text = helpers::emit_value(&net, powerio_tx::TargetFormat::PsseRawx)
            .unwrap()
            .text;
        let rawx: serde_json::Value = serde_json::from_str(&text).unwrap();
        let lines = rawx_rows(&rawx, "acline");
        let transformers = rawx_rows(&rawx, "transformer");
        assert_eq!(
            lines.len() + transformers.len(),
            net.branches().len(),
            "{name}"
        );

        let mut keys = BTreeMap::new();
        for (i, j, ckt) in lines.iter().chain(&transformers) {
            *keys
                .entry(((*i).min(*j), (*i).max(*j), ckt.clone()))
                .or_insert(0) += 1;
        }
        let repeated: Vec<_> = keys.iter().filter(|(_, count)| **count > 1).collect();
        assert!(repeated.is_empty(), "{name}: {repeated:?}");

        if let Some((i, j)) = restated {
            assert!(lines.iter().any(|row| (row.0, row.1) == (i, j)), "{name}");
            assert!(
                !transformers.iter().any(|row| (row.0, row.1) == (i, j)),
                "{name}"
            );
        }
    }
}
