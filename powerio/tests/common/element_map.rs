//! The shared element map harness (eigenergy/powerio#916).
//!
//! Each target has its own `powerio/tests/element_map_<format>.rs`, which
//! calls [`check_format`] with a reader for its written artifact. The reader
//! looks up one mapped row in the bytes the writer produced and returns what
//! that row states ([`ReadBack`]); the harness compares it with the IR
//! element and checks the map's rules:
//!
//! 1. every bus, branch, and generator `uid` appears exactly once, in module
//!    order, and a `null` entry has an `EMIT.ELEMENT_MAP.UNMAPPED` diagnostic
//!    naming it;
//! 3. no two `uid`s share a `(table, index)` or a `(table, key)`;
//! 4. every branch row has a `flip`, and its endpoints after the flip are the
//!    element's `from` and `to` buses; each generator row states the
//!    element's bus, and each bus row the element's number;
//! 6. the same module gives the same map and the same bytes twice.
//!
//! A new target adds one file:
//!
//! ```ignore
//! mod common;
//! use common::element_map::{ReadBack, Written, check_format};
//!
//! #[test]
//! fn my_format_rows_read_back_as_their_elements() {
//!     check_format("my-format", |written: &Written, kind, row| {
//!         // parse written.text() (or written.artifact("...")) and return
//!         // ReadBack::Bus / ::Branch / ::Generator for `row`
//!         todo!()
//!     });
//! }
//! ```

use std::collections::BTreeSet;
use std::path::PathBuf;

use powerio::{
    BalancedNetwork, ElementMap, EmitResult, EmittedOutput, PioModule, PioValue, RowRef,
};

/// The element map test case: sparse bus numbers, parallel lines 10-20 and a
/// reversed parallel 20-10, two generators on bus 10, a tap transformer, a
/// phase shifter, an out of service branch, and an out of service generator.
pub fn case_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/data/case4par.m")
}

/// `case4par.m` parsed, with its retained MATPOWER source severed so every
/// target (MATPOWER included) runs its writer.
pub fn case() -> PioModule<PioValue> {
    powerio::parse(case_path())
        .expect("case4par.m parses")
        .sever_source()
}

/// The balanced network a module holds.
pub fn network(module: &PioModule<PioValue>) -> &BalancedNetwork {
    match module.value() {
        PioValue::BalancedNetwork(network) => network,
        other => panic!("expected a balanced network, got {}", other.type_name()),
    }
}

/// Which section of the map a row belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Bus,
    Branch,
    Generator,
}

/// What a written row states, as the target's reader sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadBack {
    /// A bus row's bus number.
    Bus(usize),
    /// A branch row's first and second end buses, in the order written.
    Branch(usize, usize),
    /// A generator row's bus.
    Generator(usize),
}

/// One emission in memory: its artifacts by name, relative to the output root.
pub struct Written {
    pub result: EmitResult,
    pub artifacts: Vec<(String, Vec<u8>)>,
}

impl Written {
    /// The only artifact of a one-file output, as UTF-8 text.
    pub fn text(&self) -> &str {
        assert_eq!(self.artifacts.len(), 1, "expected a one-file output");
        std::str::from_utf8(&self.artifacts[0].1).expect("the artifact is UTF-8")
    }

    /// The artifact of a directory output whose name ends with `suffix`.
    pub fn artifact(&self, suffix: &str) -> &[u8] {
        self.artifacts
            .iter()
            .find(|(name, _)| name.ends_with(suffix))
            .map_or_else(
                || panic!("no artifact ending in {suffix}"),
                |(_, bytes)| bytes.as_slice(),
            )
    }

    pub fn map(&self) -> &ElementMap {
        self.result
            .element_map()
            .expect("the writer records an element map")
    }
}

/// Emit `module` to `format` in memory.
pub fn emit(module: &PioModule<PioValue>, format: &str) -> Written {
    let destination = powerio::Destination::memory("out").unwrap();
    let result = powerio::emit(module, format, destination)
        .unwrap_or_else(|error| panic!("emitting {format} failed: {error}"));
    artifacts(result)
}

/// Serialize `module` as PowerIO IR in memory.
pub fn serialize(module: &PioModule<PioValue>) -> Written {
    let destination = powerio::Destination::memory("module.pio.json").unwrap();
    artifacts(powerio::serialize(module, destination).expect("serialize"))
}

fn artifacts(result: EmitResult) -> Written {
    let EmittedOutput::Memory { artifacts } = result.output() else {
        panic!("a memory destination returns memory artifacts");
    };
    let artifacts = artifacts
        .iter()
        .map(|artifact| {
            let name = artifact.name().as_str();
            let name = name.strip_prefix("out/").unwrap_or(name).to_owned();
            (name, artifact.bytes().to_vec())
        })
        .collect();
    Written { result, artifacts }
}

/// Emit the case to `format` twice and check the map against the written
/// rows `read` reports, plus rules 1, 3, 4, and 6. Returns the first
/// emission for format specific checks.
pub fn check_format(format: &str, read: impl Fn(&Written, Kind, &RowRef) -> ReadBack) -> Written {
    let module = case();
    let written = emit(&module, format);
    let again = emit(&module, format);
    assert_eq!(
        written.artifacts, again.artifacts,
        "{format}: the same module writes the same bytes"
    );
    assert_eq!(
        written.map().to_json(),
        again.map().to_json(),
        "{format}: the same module gives the same map (rule 6)"
    );
    check_map(network(&module), &written, format, read);
    written
}

/// Check one emission's map against the network it came from.
pub fn check_map(
    network: &BalancedNetwork,
    written: &Written,
    format: &str,
    read: impl Fn(&Written, Kind, &RowRef) -> ReadBack,
) {
    let map = written.map();
    assert_eq!(map.format(), format);
    assert_eq!(map.powerio_version(), env!("CARGO_PKG_VERSION"));
    let unmapped: BTreeSet<String> = written
        .result
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.code() == "EMIT.ELEMENT_MAP.UNMAPPED")
        .map(|diagnostic| diagnostic.message().to_owned())
        .collect();

    // A record without a uid is keyed on the uid `serialize` would assign,
    // which this harness does not recompute; only stated uids are compared.
    let uids = |uids: Vec<Option<&String>>| {
        uids.into_iter()
            .map(Option::<&String>::cloned)
            .collect::<Vec<_>>()
    };
    let sections = [
        (
            Kind::Bus,
            uids(network.buses().iter().map(|b| b.uid.as_ref()).collect()),
            map.bus().collect::<Vec<_>>(),
        ),
        (
            Kind::Branch,
            uids(network.branches().iter().map(|b| b.uid.as_ref()).collect()),
            map.branch().collect::<Vec<_>>(),
        ),
        (
            Kind::Generator,
            uids(
                network
                    .generators()
                    .iter()
                    .map(|g| g.uid.as_ref())
                    .collect(),
            ),
            map.generator().collect::<Vec<_>>(),
        ),
    ];

    let mut rows = BTreeSet::new();
    let mut keys = BTreeSet::new();
    for (kind, expected, entries) in sections {
        // Rule 1: every uid once, in module order.
        assert_eq!(
            entries.len(),
            expected.len(),
            "{format} {kind:?}: one entry per element"
        );
        for ((uid, _), stated) in entries.iter().zip(&expected) {
            if let Some(stated) = stated {
                assert_eq!(uid, stated, "{format} {kind:?}: uids in module order");
            }
        }
        let distinct: BTreeSet<&str> = entries.iter().map(|(uid, _)| *uid).collect();
        assert_eq!(
            distinct.len(),
            entries.len(),
            "{format} {kind:?}: each uid once"
        );
        for (position, (uid, row)) in entries.iter().enumerate() {
            let Some(row) = row else {
                assert!(
                    unmapped
                        .iter()
                        .any(|message| message.contains(&format!("'{uid}'"))),
                    "{format}: unmapped {kind:?} '{uid}' has no diagnostic naming it"
                );
                continue;
            };
            // Rule 3: distinct rows and keys.
            assert!(
                rows.insert((row.table.clone(), row.index)),
                "{format}: two elements on {} row {}",
                row.table,
                row.index
            );
            if let Some(key) = &row.key {
                assert!(
                    keys.insert((row.table.clone(), key.clone())),
                    "{format}: two elements with {} key {key}",
                    row.table
                );
            }
            // Rule 4 and the read back.
            let read_back = read(written, kind, row);
            check_read_back(network, format, kind, position, uid, row, read_back);
        }
    }
}

/// Compare one read back row with the IR element at `position`.
fn check_read_back(
    network: &BalancedNetwork,
    format: &str,
    kind: Kind,
    position: usize,
    uid: &str,
    row: &RowRef,
    read_back: ReadBack,
) {
    let place = format!(
        "{format}: {kind:?} '{uid}' from {} row {}",
        row.table, row.index
    );
    match kind {
        Kind::Bus => {
            assert_eq!(row.flip, None, "{place}: a bus row has no flip");
            assert_eq!(
                read_back,
                ReadBack::Bus(network.buses()[position].id.0),
                "{place}"
            );
        }
        Kind::Branch => {
            let branch = &network.branches()[position];
            let flip = row.flip.expect("every branch row states flip (rule 4)");
            let ReadBack::Branch(first, second) = read_back else {
                panic!("{place}: read back as {read_back:?}");
            };
            let ends = if flip {
                (second, first)
            } else {
                (first, second)
            };
            assert_eq!(
                ends,
                (branch.from.0, branch.to.0),
                "{place}: endpoints after flip"
            );
        }
        Kind::Generator => {
            assert_eq!(row.flip, None, "{place}: a generator row has no flip");
            assert_eq!(
                read_back,
                ReadBack::Generator(network.generators()[position].bus.0),
                "{place}"
            );
        }
    }
}

/// Re-emitting a target's own output to the same target echoes the retained
/// bytes, runs no writer, and returns no map.
pub fn check_retained_echo_has_no_map(written: &Written, format: &str) {
    let source = powerio::Source::from_memory(
        written.artifacts[0].0.clone(),
        written.artifacts[0].1.clone(),
    )
    .unwrap()
    .with_format(powerio::FormatId::new(format).unwrap());
    let reparsed = powerio::parse(source).expect("the written artifact parses");
    let echo = emit(&reparsed, format);
    assert_eq!(echo.result.fidelity(), powerio::Fidelity::ExactSameFormat);
    assert!(echo.result.element_map().is_none());
}
