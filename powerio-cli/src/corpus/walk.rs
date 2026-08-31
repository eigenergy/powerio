//! Random conversion walks: chain a case through a cycle of formats and hold
//! the chain to properties a single leg cannot express.
//!
//! [`super::compare`] runs every leg from the pristine case. That is the right
//! shape for a gate, and it is blind to three things:
//!
//! - **Path dependence.** Reaching `egret` through `psse` and `surge` may give
//!   a different network than converting to `egret` directly. Every leg on the
//!   way can be individually clean and the composition still wrong.
//! - **Drift.** Conversion should settle: once a case has been through a
//!   format, going through it again should be a no-op. A pair that never
//!   settles loses or invents something on every pass, and one leg cannot see
//!   it.
//! - **Resurrection.** A count that reached zero and came back is a writer
//!   stating data no reader gave it. Only a chain reaches the empty state that
//!   makes this visible.
//!
//! A walk also reaches inputs the matrix never builds: a leg fed a network
//! that has already crossed three formats is a leg the gate has never run.
//!
//! # Learning from the walks
//!
//! Random search over a ten format graph wastes most of its budget re-walking
//! what it already knows. The run keeps a [`Ledger`] in the work directory,
//! keyed by directed edge, holding every signature that edge has ever produced
//! — warning templates, changed model paths, finding codes. Two things read
//! it:
//!
//! - **Sampling.** An edge's weight is its novelty rate, `(1 + novel) / (1 +
//!   visits)`. An edge nobody has walked scores 1; one that has stopped
//!   teaching anything decays toward 0 without ever reaching it, so nothing is
//!   permanently excluded.
//! - **Stopping.** A walk that adds no signature is dry. `settle` consecutive
//!   dry walks end the run: the sampler has stopped finding, rather than the
//!   caller having guessed a walk count.
//!
//! The ledger persists, so a second `powerio corpus walk` against the same work
//! directory resumes with everything the first learned. Delete it to start over.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::Result;
use powerio_dist::MulticonductorNetwork;
use powerio_matrix::BalancedNetwork;
use serde::{Deserialize, Serialize};

use super::anonymize;
use super::{Bucket, Domain, Ingest, balanced, catch_panic, core_delta, multiconductor};
use crate::invariants;

pub(super) const WALK_FILE: &str = "walks.json";
pub(super) const LEDGER_FILE: &str = "ledger.json";

/// The formats a walk may step through: every transmission format that both
/// writes text and reads it back.
///
/// `pypsa-csv` and `gridfm` are directories and `pwb` is read only, so none of
/// them can be a step. `goc3` and `opfdata` write only by echoing a retained
/// source, which a walk clears at the first hop, so they would write an empty
/// document. model JSON is the model transport rather than a case format
/// and is lossless by construction, so it would dilute the sample with steps
/// that cannot fail.
pub const ALPHABET: [&str; 10] = [
    "matpower",
    "psse",
    "psse34",
    "psse35",
    "powerworld",
    "pandapower-json",
    "powermodels-json",
    "egret-json",
    "pslf",
    "surge-json",
];

/// The distribution alphabet: every multiconductor format, each of which both
/// writes text and reads it back. Three formats gives two choices per step,
/// which is still enough to reach every composition the pairwise compare
/// cannot: `dss → pmd → dss` asks whether OpenDSS settles, and `dss → pmd →
/// bmopf` asks whether the route matters.
pub const DIST_ALPHABET: [&str; 3] = ["dss", "pmd-json", "bmopf-json"];

/// What a hop changed, in the vocabulary [`super::Comparison`] already uses so
/// the reporter sanitizes both the same way.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HopDelta {
    pub core_changed: Option<String>,
    pub ybus: Option<String>,
    pub injection: Option<String>,
    pub dc_terminal: Option<String>,
    pub model_diffs: Vec<String>,
}

impl HopDelta {
    fn of(before: &BalancedNetwork, after: &BalancedNetwork) -> Self {
        let mut out = Self::default();
        let (core_before, core_after) = (
            invariants::transmission_core(before),
            invariants::transmission_core(after),
        );
        if core_before != core_after {
            out.core_changed = Some(core_delta(&core_before, &core_after));
        }
        if let Ok(Some(change)) = invariants::ybus_change(before, after) {
            out.ybus = Some(change.to_string());
        }
        if let Some(change) = invariants::injection_change(before, after) {
            out.injection = Some(change.to_string());
        }
        if let Some(change) = invariants::dc_terminal_change(before, after) {
            out.dc_terminal = Some(change.to_string());
        }
        out.finish_diffs(&invariants::model_diffs(
            &invariants::transmission_value(before),
            &invariants::transmission_value(after),
        ));
        out
    }

    /// The distribution delta: element counts and the typed-model diff, the
    /// same two properties [`super::compare`] grades for this domain.
    fn of_dist(before: &MulticonductorNetwork, after: &MulticonductorNetwork) -> Self {
        let mut out = Self::default();
        let (core_before, core_after) = (
            invariants::distribution_core(before),
            invariants::distribution_core(after),
        );
        if core_before != core_after {
            out.core_changed = Some(super::dist_core_delta(&core_before, &core_after));
        }
        out.finish_diffs(&invariants::model_diffs(
            &invariants::distribution_value(before),
            &invariants::distribution_value(after),
        ));
        out
    }

    fn finish_diffs(&mut self, diffs: &[invariants::ModelDiff]) {
        self.model_diffs = diffs
            .iter()
            .map(|d| anonymize::collapse_path(&d.path))
            .collect();
        self.model_diffs.sort();
        self.model_diffs.dedup();
    }

    fn is_empty(&self) -> bool {
        !self.electrical() && self.model_diffs.is_empty()
    }

    /// Whether an electrical property moved, as opposed to retained extras or
    /// other model fields alone.
    ///
    /// A counts-only core change stays out: splitting one unbalanced load into
    /// its phases or restating transformer charging as bus shunts regroups
    /// elements the way the target format demands, and the pairwise compare
    /// grades the same move `core.regrouped`. Power moving is the line, there
    /// as here.
    pub(super) fn electrical(&self) -> bool {
        self.core_changed.as_deref().is_some_and(super::power_moved)
            || self.ybus.is_some()
            || self.injection.is_some()
            || self.dc_terminal.is_some()
    }

    /// The signatures this delta teaches its edge. Values are left out: the
    /// ledger decides what to explore next and two runs of one defect on
    /// different cases must count as one thing learned.
    fn signatures(&self, into: &mut BTreeSet<String>) {
        for (key, present) in [
            ("core", self.core_changed.is_some()),
            ("ybus", self.ybus.is_some()),
            ("injection", self.injection.is_some()),
            ("dc", self.dc_terminal.is_some()),
        ] {
            if present {
                into.insert(key.to_string());
            }
        }
        for path in &self.model_diffs {
            into.insert(format!("path:{path}"));
        }
    }
}

/// One step of a walk: write the running network in a format, read it back,
/// and say what moved.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hop {
    /// The format written at this step.
    pub to: String,
    pub warnings: Vec<String>,
    pub failure: Option<String>,
    /// Against the network this hop started from: the leg's own loss, the same
    /// quantity [`super::compare`] measures, but from a walked input rather
    /// than the pristine case.
    pub leg: HopDelta,
    /// Set when converting the origin straight to this format gives a
    /// different network than arriving here along the walk. The chain's route
    /// changed the destination, which no single leg can state.
    pub path_dependent: Option<String>,
    /// Whether that difference reaches an electrical property (counts, power,
    /// `Y_bus`, DC terminals) rather than retained extras alone. Retention
    /// honestly thins along a chain, so only the electrical kind is graded a
    /// defect.
    #[serde(default)]
    pub path_dependent_electrical: bool,
    /// Counts the origin had that are zero here. Not a defect on its own: it
    /// says a later clean hop may be clean only because the table is empty.
    pub emptied: Vec<String>,
    /// Counts that were zero at the previous hop and are nonzero here.
    pub resurrected: Vec<String>,
    /// The earlier hop this format was last written at, when the walk revisits
    /// one.
    pub revisit_of: Option<usize>,
    /// Set when that revisit produced a different network than the first
    /// visit: conversion has not settled.
    pub drift: Option<String>,
    /// As [`path_dependent_electrical`](Self::path_dependent_electrical), for
    /// the drift.
    #[serde(default)]
    pub drift_electrical: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Walk {
    pub bucket: String,
    /// The seed this walk was drawn with. Replays it exactly.
    pub seed: u64,
    pub origin: String,
    pub hops: Vec<Hop>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Walks {
    pub walks: Vec<Walk>,
    /// Buckets whose walks were cut short by the settle rule: the sampler
    /// stopped finding there before the walk budget ran out.
    pub settled_buckets: usize,
}

/// What one directed edge has taught the harness.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EdgeMemory {
    pub visits: usize,
    /// Visits that added at least one signature.
    pub novel: usize,
    pub signatures: BTreeSet<String>,
}

/// The harness's memory of its own walks, persisted in the work directory.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Ledger {
    /// Keyed `"<from>><to>"`.
    pub edges: BTreeMap<String, EdgeMemory>,
    pub walks: usize,
}

impl Ledger {
    fn key(from: &str, to: &str) -> String {
        format!("{from}>{to}")
    }

    /// The sampler's preference for stepping `from → to`.
    ///
    /// Laplace smoothed so an unvisited edge scores 1 and a well worn one
    /// decays toward, but never reaches, 0: an edge that stopped teaching
    /// under one case may still have something to say under the next.
    fn weight(&self, from: &str, to: &str) -> f64 {
        self.edges
            .get(&Self::key(from, to))
            .map_or(1.0, |e| (1.0 + e.novel as f64) / (1.0 + e.visits as f64))
    }

    /// Record a visit. Returns whether it taught the edge anything.
    fn learn(&mut self, from: &str, to: &str, signatures: &BTreeSet<String>) -> bool {
        let entry = self.edges.entry(Self::key(from, to)).or_default();
        entry.visits += 1;
        let before = entry.signatures.len();
        entry.signatures.extend(signatures.iter().cloned());
        let novel = entry.signatures.len() > before;
        if novel {
            entry.novel += 1;
        }
        novel
    }
}

/// xorshift64*, so a walk is reproducible from its seed with no dependency.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        // A zero state is a fixed point of the shift.
        Self(seed | 1)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// An index into `weights`, drawn proportionally. Falls back to the last
    /// entry when every weight is zero, which the smoothing prevents anyway.
    fn weighted(&mut self, weights: &[f64]) -> usize {
        let total: f64 = weights.iter().sum();
        if total <= 0.0 {
            return weights.len().saturating_sub(1);
        }
        // 53 bits is the whole f64 mantissa, so the draw is uniform on [0, 1).
        let unit = (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
        let mut cursor = unit * total;
        for (i, w) in weights.iter().enumerate() {
            cursor -= w;
            if cursor < 0.0 {
                return i;
            }
        }
        weights.len() - 1
    }
}

/// The running network of a walk, in whichever model the bucket holds.
#[derive(Clone)]
enum Net {
    Balanced(BalancedNetwork),
    Dist(MulticonductorNetwork),
}

impl Net {
    fn delta(&self, after: &Net) -> HopDelta {
        match (self, after) {
            (Self::Balanced(a), Self::Balanced(b)) => HopDelta::of(a, b),
            (Self::Dist(a), Self::Dist(b)) => HopDelta::of_dist(a, b),
            // A walk never crosses domains; the alphabets are disjoint.
            _ => HopDelta::default(),
        }
    }

    /// Element tables `self` has that `after` does not.
    fn emptied(&self, after: &Net) -> Vec<String> {
        match (self, after) {
            (Self::Balanced(a), Self::Balanced(b)) => {
                let (a, b) = (
                    invariants::transmission_core(a),
                    invariants::transmission_core(b),
                );
                zeroed(&[
                    ("branches", a.branches, b.branches),
                    ("generators", a.generators, b.generators),
                    ("loads", a.loads, b.loads),
                    ("shunts", a.shunts, b.shunts),
                ])
            }
            (Self::Dist(a), Self::Dist(b)) => {
                let (a, b) = (
                    invariants::distribution_core(a),
                    invariants::distribution_core(b),
                );
                zeroed(&[
                    ("buses", a.buses, b.buses),
                    ("generators", a.generators, b.generators),
                    ("loads", a.loads, b.loads),
                    ("shunts", a.shunts, b.shunts),
                ])
            }
            _ => Vec::new(),
        }
    }

    /// Emit as `to` and read back, collecting both sides' warnings. A panic
    /// on either side comes back as a message rather than unwinding.
    fn convert(&self, to: &str, warnings: &mut Vec<String>) -> std::result::Result<Net, String> {
        match self {
            Self::Balanced(net) => {
                let Some(target) = powerio_tx::format::parse_target_format(to) else {
                    return Err(format!("no writer for {to}"));
                };
                let emission = match catch_panic(|| crate::compat::emit_tx_value(net, target)) {
                    Ok(Ok(emission)) => emission,
                    Ok(Err(err)) => return Err(format!("emit: {err}")),
                    Err(message) => return Err(format!("emit panicked: {message}")),
                };
                warnings.extend(emission.render_diagnostics());
                match catch_panic(|| crate::compat::parse_str(&emission.text, to)) {
                    Ok(Ok(parsed)) => {
                        warnings.extend(parsed.render_diagnostics());
                        Ok(Self::Balanced(parsed.network))
                    }
                    Ok(Err(err)) => Err(format!("readback: {err}")),
                    Err(message) => Err(format!("readback panicked: {message}")),
                }
            }
            Self::Dist(net) => {
                let Some(target) = powerio_dist::parse_dist_target_format(to) else {
                    return Err(format!("no writer for {to}"));
                };
                let emission = match catch_panic(|| crate::compat::emit_dist_value(net, target)) {
                    Ok(Ok(emission)) => emission,
                    Ok(Err(err)) => return Err(format!("emit: {err}")),
                    Err(message) => return Err(format!("emit panicked: {message}")),
                };
                warnings.extend(emission.render_diagnostics());
                // A deck that pulls in other files cannot be read back from a
                // string; see the same test in the pairwise compare.
                if super::has_include(&emission.text) {
                    return Err("emitted deck redirects to include files".to_string());
                }
                match catch_panic(|| crate::compat::dist_parse_str(&emission.text, to)) {
                    Ok(Ok(parsed)) => {
                        warnings.extend(parsed.warnings.iter().cloned());
                        Ok(Self::Dist(parsed.network))
                    }
                    Ok(Err(err)) => Err(format!("readback: {err}")),
                    Err(message) => Err(format!("readback panicked: {message}")),
                }
            }
        }
    }
}

fn zeroed(counts: &[(&str, usize, usize)]) -> Vec<String> {
    counts
        .iter()
        .filter(|&&(_, had, has)| had > 0 && has == 0)
        .map(|&(label, ..)| label.to_string())
        .collect()
}

/// Walk every bucket, learning as it goes.
///
/// Each walk starts from a bucket member, draws `hops` formats by the ledger's
/// novelty weights, and converts through them in turn. The run stops early
/// when `settle` consecutive walks add nothing to the ledger.
///
/// # Errors
///
/// Fails when the work directory has no ingest output or cannot be written.
pub fn walk(work: &Path, walks: usize, hops: usize, seed: u64, settle: usize) -> Result<Walks> {
    let ingest: Ingest = super::read_json(&work.join(super::INGEST_FILE))?;
    let ledger_path = work.join(LEDGER_FILE);
    let mut ledger: Ledger = super::read_json(&ledger_path).unwrap_or_default();

    let mut out = Walks::default();
    let mut rng = Rng::new(seed);

    for bucket in &ingest.buckets {
        let Some((origin_format, origin)) = first_readable(bucket) else {
            continue;
        };
        let alphabet: &[&str] = match bucket.domain {
            Domain::Transmission => &ALPHABET,
            Domain::Distribution => &DIST_ALPHABET,
        };
        // Direct conversions of this bucket's origin, computed at most once
        // per format across all its walks.
        let mut direct: BTreeMap<String, Option<Net>> = BTreeMap::new();
        // The settle rule is per bucket: each bucket is a different case, and
        // one going quiet says nothing about what the next can teach. A global
        // streak would let a few quiet buckets starve every one after them.
        let mut dry = 0usize;
        for _ in 0..walks {
            if dry >= settle {
                break;
            }
            let walk_seed = rng.next_u64();
            let path = draw_path(
                &ledger,
                alphabet,
                &origin_format,
                hops,
                &mut Rng::new(walk_seed),
            );
            let (walk, learned) = run_walk(
                bucket.id.clone(),
                walk_seed,
                &origin_format,
                &origin,
                &path,
                &mut ledger,
                &mut direct,
            );
            dry = if learned { 0 } else { dry + 1 };
            ledger.walks += 1;
            out.walks.push(walk);
        }
        if dry >= settle {
            out.settled_buckets += 1;
        }
    }

    super::write_json(&ledger_path, &ledger)?;
    super::write_json(&work.join(WALK_FILE), &out)?;
    Ok(out)
}

/// The first bucket member that reads in the bucket's own model, with its
/// format.
///
/// The retained source text is cleared. A walk measures conversions, and a
/// network that kept its source byte-echoes any write back to its own format
/// — which would make the direct baseline for that format the identity and
/// flag every route to it as path dependent over nothing but retention.
fn first_readable(bucket: &Bucket) -> Option<(String, Net)> {
    bucket.members.iter().find_map(|m| {
        let net = match bucket.domain {
            Domain::Transmission => {
                let net = balanced(&m.path)?;
                Net::Balanced(net)
            }
            Domain::Distribution => {
                let mut net = multiconductor(&m.path)?;
                *net.source_format_mut() = None;
                Net::Dist(net)
            }
        };
        Some((m.format.clone(), net))
    })
}

/// Draw a path of `hops` formats, never stepping to the format just written
/// (a same format step is the round trip [`super::compare`] already runs).
fn draw_path(
    ledger: &Ledger,
    alphabet: &[&str],
    origin: &str,
    hops: usize,
    rng: &mut Rng,
) -> Vec<String> {
    let mut path = Vec::with_capacity(hops);
    let mut here = origin.to_string();
    for _ in 0..hops {
        let choices: Vec<&str> = alphabet
            .iter()
            .copied()
            .filter(|f| *f != here.as_str())
            .collect();
        let weights: Vec<f64> = choices.iter().map(|to| ledger.weight(&here, to)).collect();
        let pick = choices[rng.weighted(&weights)].to_string();
        here.clone_from(&pick);
        path.push(pick);
    }
    path
}

/// Convert `origin` along `path`, recording every hop. Returns the walk and
/// whether it taught the ledger anything.
fn run_walk(
    bucket: String,
    seed: u64,
    origin_format: &str,
    origin: &Net,
    path: &[String],
    ledger: &mut Ledger,
    direct: &mut BTreeMap<String, Option<Net>>,
) -> (Walk, bool) {
    let mut walk = Walk {
        bucket,
        seed,
        origin: origin_format.to_string(),
        hops: Vec::with_capacity(path.len()),
    };
    let mut learned = false;
    let mut current = origin.clone();
    let mut here = origin_format.to_string();
    // Every network the walk has produced, by the format that produced it, so
    // a revisit can be compared against the first pass.
    let mut seen: Vec<(String, Net)> = Vec::new();

    for to in path {
        let mut hop = Hop {
            to: to.clone(),
            warnings: Vec::new(),
            failure: None,
            leg: HopDelta::default(),
            path_dependent: None,
            path_dependent_electrical: false,
            emptied: Vec::new(),
            resurrected: Vec::new(),
            revisit_of: None,
            drift: None,
            drift_electrical: false,
        };
        let next = match current.convert(to, &mut hop.warnings) {
            Ok(net) => net,
            Err(message) => {
                // A failed hop still teaches its edge, or a crash found once
                // would be re-found on every later run.
                let mut signatures = BTreeSet::new();
                signatures.insert(format!("fail:{}", warning_shape(&message)));
                learned |= ledger.learn(&here, to, &signatures);
                hop.failure = Some(message);
                walk.hops.push(hop);
                // Nothing to carry forward.
                break;
            }
        };

        hop.leg = current.delta(&next);
        hop.emptied = origin.emptied(&next);
        hop.resurrected = next.emptied(&current);

        if let Some(index) = seen.iter().position(|(f, _)| f == to) {
            hop.revisit_of = Some(index);
            let delta = seen[index].1.delta(&next);
            if !delta.is_empty() {
                hop.drift_electrical = delta.electrical();
                hop.drift = Some(describe(&delta));
            }
        }

        // Would converting the origin straight here have landed in the same
        // place? Only asked past the first hop, where the walk and the direct
        // conversion are the same thing. The direct result depends only on the
        // origin and the format, so it is computed once per bucket however
        // many walks and hops ask.
        if !seen.is_empty() {
            let baseline = direct
                .entry(to.clone())
                .or_insert_with(|| origin.convert(to, &mut Vec::new()).ok());
            if let Some(direct_net) = baseline {
                let delta = direct_net.delta(&next);
                if !delta.is_empty() {
                    hop.path_dependent_electrical = delta.electrical();
                    hop.path_dependent = Some(describe(&delta));
                }
            }
        }

        let mut signatures = BTreeSet::new();
        hop.leg.signatures(&mut signatures);
        for warning in &hop.warnings {
            signatures.insert(format!("warn:{}", warning_shape(warning)));
        }
        for (code, present) in [
            ("drift", hop.drift.is_some()),
            ("path-dependent", hop.path_dependent.is_some()),
            ("resurrection", !hop.resurrected.is_empty()),
        ] {
            if present {
                signatures.insert(code.to_string());
            }
        }
        learned |= ledger.learn(&here, to, &signatures);

        seen.push((to.clone(), next.clone()));
        here.clone_from(to);
        current = next;
        walk.hops.push(hop);
    }

    (walk, learned)
}

/// A delta as one line, for the fields a walk finding names.
fn describe(delta: &HopDelta) -> String {
    let mut parts = Vec::new();
    for value in [
        delta.core_changed.as_ref(),
        delta.ybus.as_ref(),
        delta.injection.as_ref(),
        delta.dc_terminal.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        parts.push(value.clone());
    }
    if !delta.model_diffs.is_empty() {
        parts.push(delta.model_diffs.join(", "));
    }
    parts.join("; ")
}

/// A warning reduced to its shape: the first eight tokens, each masked to `_`
/// unless it is plain lowercase prose. The ledger counts kinds of warning, and
/// an element name or a count in the text would make every case look like a
/// new lesson — the first corpus run minted 2551 "lessons" on one edge from
/// nineteen spellings of one warning about line names.
///
/// A token survives when every character is a lowercase letter or the
/// punctuation warning prose uses around it. Anything else — digits, an
/// element name's capitals, a quoted value — masks. Uppercase powerio
/// vocabulary (MATPOWER, PSS/E) masks with them, which costs nothing: the
/// masked token is in a fixed position, so the shape stays distinct.
fn warning_shape(warning: &str) -> String {
    warning
        .split_whitespace()
        .take(8)
        .map(|w| {
            let prose = w
                .chars()
                .all(|c| c.is_ascii_lowercase() || "()[]{}.,:;`-_/".contains(c));
            if prose { w } else { "_" }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seed_replays_its_path() {
        let ledger = Ledger::default();
        let a = draw_path(&ledger, &ALPHABET, "matpower", 6, &mut Rng::new(7));
        let b = draw_path(&ledger, &ALPHABET, "matpower", 6, &mut Rng::new(7));
        assert_eq!(a, b);
        assert_eq!(a.len(), 6);
        assert_ne!(
            a,
            draw_path(&ledger, &ALPHABET, "matpower", 6, &mut Rng::new(8))
        );
    }

    #[test]
    fn a_path_never_steps_to_the_format_it_is_already_in() {
        let ledger = Ledger::default();
        for alphabet in [&ALPHABET[..], &DIST_ALPHABET[..]] {
            let path = draw_path(&ledger, alphabet, alphabet[0], 40, &mut Rng::new(3));
            assert_ne!(path[0], alphabet[0]);
            assert!(
                path.windows(2).all(|w| w[0] != w[1]),
                "a same-format step is the round trip compare already runs: {path:?}"
            );
        }
    }

    #[test]
    fn the_ledger_prefers_an_edge_it_has_not_walked() {
        let mut ledger = Ledger::default();
        let nothing = BTreeSet::new();
        // Ten visits that taught nothing drive the weight down; an untouched
        // edge stays at 1.
        for _ in 0..10 {
            assert!(!ledger.learn("matpower", "psse", &nothing));
        }
        assert!(ledger.weight("matpower", "psse") < 0.1);
        assert!((ledger.weight("matpower", "egret-json") - 1.0).abs() < f64::EPSILON);
        // Never zero: an edge that taught nothing under one case may still
        // have something to say under the next.
        assert!(ledger.weight("matpower", "psse") > 0.0);
    }

    #[test]
    fn a_visit_that_teaches_is_novel_once() {
        let mut ledger = Ledger::default();
        let lesson: BTreeSet<String> = ["path:.loads[#].p".to_string()].into_iter().collect();
        assert!(ledger.learn("psse", "matpower", &lesson));
        assert!(
            !ledger.learn("psse", "matpower", &lesson),
            "the same lesson twice is not a second lesson"
        );
        let edge = &ledger.edges["psse>matpower"];
        assert_eq!((edge.visits, edge.novel), (2, 1));
    }

    #[test]
    fn a_warning_shape_drops_what_makes_one_warning_look_like_many() {
        // Counts and element names mask positionally; the prose around them
        // stays, so the shape is stable across cases and still distinct
        // between warnings.
        assert_eq!(
            warning_shape("3 switch(es) dropped: MATPOWER has no switch table"),
            "_ switch(es) dropped: _ has no switch table"
        );
        for name in ["L100", "L101", "SourceBus"] {
            assert_eq!(
                warning_shape(&format!("line {name}: `units` has no place")),
                "line _ `units` has no place"
            );
        }
        assert_ne!(
            warning_shape("bus RG60: location has no place"),
            warning_shape("line L100: `units` has no place")
        );
    }
}
