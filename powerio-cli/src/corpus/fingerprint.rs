//! Electrical fingerprints, which is how the harness finds siblings.
//!
//! Two files hold the same case when their networks agree electrically, and
//! nothing else counts: not the filename, not the directory, not the element
//! names, not the format. A utility archive states one case as `feeder_a.raw`
//! and `FeederA_2019_final_v3.m`; a fingerprint built from bus count, base
//! MVA, the bus graph's degree sequence and the quantized impedance multiset
//! groups them anyway, and groups nothing else with them.
//!
//! Every component is invariant under the things a conversion is allowed to
//! change (renumbering, renaming, reordering, merging devices at one bus) and
//! sensitive to the things it is not (topology, impedance, demand).
//!
//! Nothing here reads `in_service`, and that is the point. A fingerprint built
//! from the operating point splits exactly the siblings whose disagreement
//! about the operating point is the finding: two exports of one case that
//! differ only in which machines are open would land in separate buckets and
//! never be compared. The key describes what a file holds; what a reader made
//! of it belongs in the findings.
//!
//! Generator capability stays out for the same reason at one remove — two
//! honest exports of one case routinely state different limits, so a key that
//! includes them splits real siblings.

use std::collections::BTreeMap;

use powerio_matrix::BalancedNetwork;

/// Quantum for per-unit impedance magnitudes. Coarser than any writer's
/// printed precision, so a value that survived a text format lands on the same
/// step as its source.
const IMPEDANCE_QUANTUM: f64 = 1e-5;

/// Quantum for MW/MVAr totals.
const POWER_QUANTUM: f64 = 1e-2;

/// Quantum for base MVA.
const BASE_MVA_QUANTUM: f64 = 1e-3;

fn quantize(x: f64, q: f64) -> i64 {
    if x.is_finite() {
        (x / q).round() as i64
    } else {
        i64::MIN
    }
}

/// What makes two files the same case.
///
/// The whole struct is the bucketing key. It deliberately leaves out the
/// load, generator and shunt counts: formats merge and split devices at a bus
/// (PSS/E states three loads where MATPOWER states one bus demand), so those
/// counts differ between honest siblings while the demand they sum to does
/// not.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct Fingerprint {
    pub buses: usize,
    pub base_mva: i64,
    /// Sorted bus degrees over in-service branches. Invariant under bus
    /// renumbering, sensitive to a lost or added branch.
    pub degrees: Vec<usize>,
    /// Sorted quantized `|z|` of every branch.
    pub impedances: Vec<i64>,
    /// Number of DC lines. A case with an HVDC link is a different case from
    /// the AC network underneath it, however alike the two look.
    pub hvdc: usize,
    pub load_p: i64,
    pub load_q: i64,
}

impl Fingerprint {
    #[must_use]
    pub fn of(net: &BalancedNetwork) -> Self {
        let mut degree: BTreeMap<usize, usize> = net.buses().iter().map(|b| (b.id.0, 0)).collect();
        let mut impedances = Vec::new();
        for br in net.branches() {
            *degree.entry(br.from.0).or_default() += 1;
            *degree.entry(br.to.0).or_default() += 1;
            impedances.push(quantize(br.r.hypot(br.x), IMPEDANCE_QUANTUM));
        }
        impedances.sort_unstable();
        let mut degrees: Vec<usize> = degree.into_values().collect();
        degrees.sort_unstable();
        Self {
            buses: net.buses().len(),
            base_mva: quantize(net.base_mva(), BASE_MVA_QUANTUM),
            degrees,
            impedances,
            hvdc: net.hvdc().len(),
            load_p: quantize(net.loads().iter().map(|l| l.p).sum(), POWER_QUANTUM),
            load_q: quantize(net.loads().iter().map(|l| l.q).sum(), POWER_QUANTUM),
        }
    }

    /// A multiconductor network's fingerprint.
    ///
    /// Coarser than the balanced one: a distribution line carries an impedance
    /// matrix rather than a scalar, so there is no `|z|` multiset to compare,
    /// and the degree sequence runs over lines, switches and transformers
    /// together because formats disagree about which of the three a given
    /// device is.
    #[must_use]
    pub fn of_distribution(net: &powerio_dist::MulticonductorNetwork) -> Self {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for (a, b) in net
            .lines()
            .iter()
            .map(|l| (l.bus_from.as_str(), l.bus_to.as_str()))
            .chain(
                net.switches()
                    .iter()
                    .map(|s| (s.bus_from.as_str(), s.bus_to.as_str())),
            )
        {
            *counts.entry(a.to_string()).or_default() += 1;
            *counts.entry(b.to_string()).or_default() += 1;
        }
        let mut degrees: Vec<usize> = counts.into_values().collect();
        degrees.sort_unstable();
        Self {
            buses: net.buses().len(),
            // A multiconductor network states no system base; the domain in
            // the primary key is what keeps these apart from balanced cases.
            base_mva: 0,
            degrees,
            impedances: Vec::new(),
            hvdc: 0,
            load_p: quantize(
                net.loads().iter().flat_map(|l| l.p_nom.iter()).sum(),
                POWER_QUANTUM,
            ),
            load_q: quantize(
                net.loads().iter().flat_map(|l| l.q_nom.iter()).sum(),
                POWER_QUANTUM,
            ),
        }
    }

    /// The coarse bucketing key: the part of the fingerprint that is exact
    /// even across a lossy text format. Sorting by it gives the bucket
    /// ordinals, so a bucket id derives from electrical content and from
    /// nothing in the source.
    #[must_use]
    pub fn primary(&self) -> PrimaryKey {
        PrimaryKey {
            buses: self.buses,
            base_mva: self.base_mva,
            hvdc: self.hvdc,
            degrees: self.degrees.clone(),
        }
    }

    /// Whether two fingerprints sharing a [`PrimaryKey`] really are the same
    /// case.
    ///
    /// Compared with a tolerance rather than by quantized equality: two
    /// siblings whose impedance lands either side of a quantum boundary are
    /// still the same case, and exact equality on a rounded value would split
    /// them. The quantized fields exist to make the fingerprint serializable
    /// and sortable; agreement is decided here.
    #[must_use]
    pub fn agrees_with(&self, other: &Self) -> bool {
        let close = |a: i64, b: i64, tolerance: i64| (a - b).abs() <= tolerance;
        self.impedances.len() == other.impedances.len()
            && std::iter::zip(&self.impedances, &other.impedances)
                .all(|(a, b)| close(*a, *b, slack(*a)))
            && close(self.load_p, other.load_p, slack(self.load_p))
            && close(self.load_q, other.load_q, slack(self.load_q))
    }
}

/// One part in `1e4` of the value, and never less than one quantum: enough to
/// absorb a fixed-width column's truncation, too little to merge two cases.
///
/// One rule for impedances and for power. They were two identically-bodied
/// functions, which is how a later change to one tolerance silently leaves the
/// other behind.
fn slack(quantized: i64) -> i64 {
    1.max(quantized.abs() / 10_000)
}

/// Whether two networks state the same electrical data, ignoring which
/// elements are in service.
///
/// Bucketing is deliberately loose — topology and demand — so that a case and
/// its variants land together and get compared. That looseness is wrong for
/// the sibling leg, which asks whether two *readers* disagree: a base case and
/// its pglib derivative differ in generator limits for honest reasons, and
/// reporting that as a reader disagreement buries the real findings.
///
/// Status is excluded on purpose. Two exports of one case that differ only in
/// which machines are open are the same data, and the disagreement about
/// status is exactly the finding the sibling leg exists to raise.
#[must_use]
pub fn same_data(a: &BalancedNetwork, b: &BalancedNetwork) -> bool {
    let branches = |net: &BalancedNetwork| {
        let mut v: Vec<[i64; 5]> = net
            .branches()
            .iter()
            .map(|br| {
                [
                    quantize(br.r, IMPEDANCE_QUANTUM),
                    quantize(br.x, IMPEDANCE_QUANTUM),
                    quantize(br.b, IMPEDANCE_QUANTUM),
                    quantize(br.effective_tap(), IMPEDANCE_QUANTUM),
                    quantize(br.shift, IMPEDANCE_QUANTUM),
                ]
            })
            .collect();
        v.sort_unstable();
        v
    };
    let generators = |net: &BalancedNetwork| {
        let mut v: Vec<[i64; 4]> = net
            .generators()
            .iter()
            .map(|g| {
                [
                    quantize(g.pmax, POWER_QUANTUM),
                    quantize(g.pmin, POWER_QUANTUM),
                    quantize(g.qmax, POWER_QUANTUM),
                    quantize(g.qmin, POWER_QUANTUM),
                ]
            })
            .collect();
        v.sort_unstable();
        v
    };
    let loads = |net: &BalancedNetwork| {
        let mut v: Vec<[i64; 2]> = net
            .loads()
            .iter()
            .map(|l| [quantize(l.p, POWER_QUANTUM), quantize(l.q, POWER_QUANTUM)])
            .collect();
        v.sort_unstable();
        v
    };
    branches(a) == branches(b) && generators(a) == generators(b) && loads(a) == loads(b)
}

/// The exact half of a fingerprint. Buckets group by this, then split by
/// [`Fingerprint::agrees_with`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PrimaryKey {
    pub buses: usize,
    pub base_mva: i64,
    pub hvdc: usize,
    pub degrees: Vec<usize>,
}
