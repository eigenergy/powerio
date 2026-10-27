//! Immutable references to electrical definition state at an assignment.
//!
//! The property and edit vectors only append. Prefix lengths therefore retain
//! past state without cloning matrix literals or retaining another object tree.

use std::collections::BTreeMap;

// The staged line resolver consumes these records. Raw acquisition already
// records them so later reader integration does not change source execution.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct Snapshot {
    pub object: usize,
    pub properties: usize,
    pub edits: usize,
    // Empty MakeLike operations can leave the property count unchanged.
    pub copies: usize,
    pub events: usize,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy, Debug)]
pub(super) struct Event {
    pub position: Snapshot,
    pub frequency: f64,
    pub rebuild_only: bool,
    pub terminal_state: Option<(usize, usize, bool)>,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Debug)]
pub(super) struct CopyPoint {
    pub start: usize,
    pub end: usize,
    // Copied legacy edit checkpoints are not native EndEdit calls on the
    // destination. Keep their exact span, including zero-property copies.
    pub edit_start: usize,
    pub edit_end: usize,
    pub source: Snapshot,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug)]
pub(super) struct LineHistory {
    pub constructor_frequency: f64,
    pub copies: Vec<CopyPoint>,
    pub events: Vec<Event>,
    // Property offset -> referenced linecode prefix. None records a missing
    // reference at assignment, even if that name is declared later.
    pub linecodes: BTreeMap<usize, Option<Snapshot>>,
    pub xfmrcodes: BTreeMap<usize, Option<Snapshot>>,
    pub geometries: BTreeMap<usize, Option<Snapshot>>,
    pub spacings: BTreeMap<usize, Option<Snapshot>>,
    // Geometry conductor assignments bind identities immediately; their
    // snapshots also preserve the ratings copied at that assignment.
    pub wires: BTreeMap<usize, Vec<Option<Snapshot>>>,
}

impl LineHistory {
    pub fn new(constructor_frequency: f64) -> Self {
        Self {
            constructor_frequency,
            copies: Vec::new(),
            events: Vec::new(),
            linecodes: BTreeMap::new(),
            geometries: BTreeMap::new(),
            spacings: BTreeMap::new(),
            xfmrcodes: BTreeMap::new(),
            wires: BTreeMap::new(),
        }
    }
}

#[cfg(test)]
pub(super) fn base_frequency(raw: &super::raw::RawDss, mut point: Snapshot) -> Result<f64, String> {
    // Follow the last assignment affecting BaseFrequency. MakeLike and
    // linecode assignment copy the referenced value, not a live reference.
    // Iteration avoids stack growth for long, valid chains of empty copies.
    let mut visited = std::collections::HashSet::new();
    'history: loop {
        if !visited.insert(point) {
            return Err("cyclic line frequency history".into());
        }
        let object = raw.objects.get(point.object).ok_or("missing line object")?;
        let history = raw
            .line_history
            .get(&point.object)
            .ok_or("missing line history")?;
        let properties = object
            .props
            .get(..point.properties)
            .ok_or("invalid property prefix")?;
        object
            .edits
            .get(..point.edits)
            .ok_or("invalid edit prefix")?;
        let copy = history
            .copies
            .get(..point.copies)
            .ok_or("invalid copy prefix")?
            .last();
        let start = copy.map_or(0, |copy| copy.end);
        if start > properties.len() || copy.is_some_and(|copy| copy.start > copy.end) {
            return Err("invalid copy span".into());
        }
        for (offset, property) in properties.iter().enumerate().skip(start).rev() {
            match property.name.as_deref() {
                Some("basefreq") => {
                    return positive_frequency(
                        property
                            .value
                            .to_f64(Some(&raw.vars))
                            .map_err(|e| e.to_string())?,
                    );
                }
                Some("linecode") if object.class == "line" => {
                    point = history
                        .linecodes
                        .get(&offset)
                        .ok_or("missing linecode binding")?
                        .ok_or("linecode did not exist at assignment")?;
                    continue 'history;
                }
                _ => {}
            }
        }
        if let Some(copy) = copy {
            point = copy.source;
        } else {
            return positive_frequency(history.constructor_frequency);
        }
    }
}

#[cfg(test)]
fn positive_frequency(value: f64) -> Result<f64, String> {
    if value.is_finite() && value > 0.0 {
        Ok(value)
    } else {
        Err("line base frequency must be positive finite".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dss::raw::{__parse_raw_with, RawDss};
    use std::fmt::Write as _;

    fn read(text: &str) -> RawDss {
        __parse_raw_with(text, "history.dss", &mut |_: &std::path::Path| {
            Err(std::io::Error::other("no includes"))
        })
    }

    fn index(raw: &RawDss, class: &str, name: &str) -> usize {
        raw.objects
            .iter()
            .position(|o| o.class == class && o.name == name)
            .unwrap()
    }

    #[test]
    fn linecode_binding_keeps_the_assigned_prefix_and_constructor() {
        let raw = read(
            "Set defaultbasefrequency=50\nNew Circuit.c\nNew Linecode.lc r1=0.2\nNew Line.first linecode=lc\nSet basefrequency=70\nEdit Linecode.lc basefreq=60 r1=0.7\nNew Line.second linecode=lc",
        );
        let first = index(&raw, "line", "first");
        let second = index(&raw, "line", "second");
        let historical = raw.line_history[&first]
            .linecodes
            .values()
            .next()
            .unwrap()
            .unwrap();
        let code = &raw.objects[historical.object];
        assert_eq!(
            code.props[..historical.properties]
                .iter()
                .find(|p| p.name.as_deref() == Some("r1"))
                .unwrap()
                .value
                .text,
            "0.2"
        );
        assert_eq!(code.get("r1").unwrap().text, "0.7");
        assert_eq!(
            base_frequency(&raw, raw.line_snapshot(first))
                .unwrap()
                .to_bits(),
            50.0_f64.to_bits()
        );
        assert_eq!(
            base_frequency(&raw, raw.line_snapshot(second))
                .unwrap()
                .to_bits(),
            60.0_f64.to_bits()
        );
        assert_eq!(
            raw.line_history[&second].constructor_frequency.to_bits(),
            70.0_f64.to_bits()
        );
    }

    #[test]
    fn a_forward_reference_stays_missing_after_the_name_is_declared() {
        let raw = read(
            "New Circuit.c\nNew Line.target linecode=later\nNew Linecode.later\nEdit Line.target linecode=later",
        );
        let target = index(&raw, "line", "target");
        let values: Vec<_> = raw.line_history[&target].linecodes.values().collect();
        assert_eq!(values[0], &None);
        assert!(values[1].is_some());
    }

    #[test]
    fn long_empty_copy_chains_use_iterative_frequency_resolution() {
        let mut text =
            "Set defaultbasefrequency=50\nNew Circuit.c\nNew Linecode.c0\nSet basefrequency=70\n"
                .to_owned();
        for i in 1..=512 {
            writeln!(text, "New Linecode.c{i} like=c{}", i - 1).unwrap();
        }
        let raw = read(&text);
        let target = index(&raw, "linecode", "c512");
        assert_eq!(
            base_frequency(&raw, raw.line_snapshot(target))
                .unwrap()
                .to_bits(),
            50.0_f64.to_bits()
        );
    }

    #[test]
    #[ignore = "requires pinned native line history observations"]
    fn pinned_line_history() {
        #[derive(serde::Deserialize)]
        struct Case {
            script: String,
            frequencies: BTreeMap<String, f64>,
        }
        #[derive(serde::Deserialize)]
        struct Report {
            cases: BTreeMap<String, Case>,
            failures: Vec<String>,
        }
        let path = std::env::var("POWERIO_LINE_HISTORY_ORACLE").expect("native observations");
        let report: Report = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(report.failures, [] as [std::string::String; 0]);
        assert!(!report.cases.is_empty());
        let mut findings = BTreeMap::new();
        for (name, case) in report.cases {
            let raw = read(&case.script);
            let mut compared = 0;
            for (key, expected) in case.frequencies {
                let (class, object) = key.split_once('.').unwrap();
                let point = raw.line_snapshot(index(&raw, class, object));
                let actual = base_frequency(&raw, point).unwrap();
                assert!(
                    (actual - expected).abs() <= 1e-12,
                    "{name}/{key}: {actual} != {expected}"
                );
                compared += 1;
            }
            assert!(compared > 0);
            findings.insert(name, compared);
        }
        let output = std::env::var("POWERIO_LINE_HISTORY_RESULT").expect("comparison path");
        std::fs::write(output, serde_json::to_vec_pretty(&findings).unwrap()).unwrap();
    }
}
