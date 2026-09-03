//! The conversion invariants, shared by the matrix test and the corpus
//! harness.
//!
//! Every property here answers one question about a conversion: did the
//! electrical problem survive, did the element counts survive, and which
//! typed-model fields moved. The matrix test holds seven vendored cases to
//! them; [`crate::corpus`] holds an arbitrary corpus to the same ones.
//!
//! Results are structured rather than preformatted. The matrix test renders
//! bus ids and values into its report; the corpus renders class ordinals and
//! magnitudes into a findings file that may leave the machine. A shared
//! `format!` would have forced one of those two to be wrong.

use std::collections::BTreeMap;

use powerio_dist::MulticonductorNetwork;
use powerio_matrix::BalancedNetwork;

/// Relative tolerance for the electrical invariants. Conversions print
/// shortest round-trip floats, so anything past this is a real change.
const ELECTRICAL_TOL: f64 = 1e-8;

/// Relative tolerance for the typed-model diff, one decade looser than the
/// electrical one because it compares values that crossed a text format.
const MODEL_TOL: f64 = 1e-9;

/// The number of diff paths kept per comparison; enough to name the disease
/// without drowning the report.
pub const MAX_MODEL_DIFFS: usize = 24;

/// A `Y_bus` entry that changed across a conversion, addressed by external bus
/// id pair so a renumbering target cannot hide behind its own dense index.
#[derive(Debug, Clone, PartialEq)]
pub struct YbusChange {
    pub from_bus: usize,
    pub to_bus: usize,
    pub before: (f64, f64),
    pub after: (f64, f64),
}

impl std::fmt::Display for YbusChange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Y[{}, {}] was {}+j{}, now {}+j{}",
            self.from_bus, self.to_bus, self.before.0, self.before.1, self.after.0, self.after.1
        )
    }
}

/// Which per-bus injection moved.
///
/// The DC entries are per-bus too: a two-terminal DC line contributes at the
/// bus each of its ends sits on. They name the end by its converter role
/// rather than by the line's `from`/`to`, which are branch words for a
/// quantity tallied at a bus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Injection {
    LoadP,
    LoadQ,
    GenP,
    GenQ,
    /// Real power a DC line takes out of its rectifier bus.
    DcRectifierP,
    /// Real power a DC line delivers to its inverter bus.
    DcInverterP,
    DcRectifierQ,
    DcInverterQ,
}

impl Injection {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::LoadP => "load p",
            Self::LoadQ => "load q",
            Self::GenP => "gen p",
            Self::GenQ => "gen q",
            Self::DcRectifierP => "dc rectifier p",
            Self::DcInverterP => "dc inverter p",
            Self::DcRectifierQ => "dc rectifier q",
            Self::DcInverterQ => "dc inverter q",
        }
    }
}

/// A per-bus injection that moved across a conversion.
#[derive(Debug, Clone, PartialEq)]
pub struct InjectionChange {
    pub bus: usize,
    pub quantity: Injection,
    pub before: f64,
    pub after: f64,
    /// Whether the per-bus totals over *every* element agree while the
    /// in-service totals do not.
    ///
    /// That is weaker than "only the statuses differ": the tallies are per bus,
    /// so two changes at one bus that cancel — an idle load switched on while a
    /// live one of the same size is switched off — also satisfy it. Read it as
    /// "the numbers in the file are unchanged", which is what it checks, and
    /// not as a promise about which elements moved.
    pub status_only: bool,
}

impl std::fmt::Display for InjectionChange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "bus {} {}: {} -> {}",
            self.bus,
            self.quantity.label(),
            self.before,
            self.after
        )
    }
}

/// One bus row of the relabeling-free electrical digest: the bus's own
/// admittance and its injections.
#[derive(Debug, Clone, PartialEq)]
struct BusRow {
    diagonal: (f64, f64),
    injections: [f64; 4],
}

/// The electrical problem compared up to bus relabeling, for a target format
/// that states no bus number.
///
/// Every bus contributes its diagonal admittance and its four injection
/// columns, every branch its off-diagonal admittance, and the two collections
/// are compared as sorted multisets. That is the power flow problem itself: an
/// admittance that changed, an injection that moved to a bus with different
/// neighbours, or an element that vanished all show up, while a renumbering
/// alone does not. Two buses carrying identical rows are interchangeable, and
/// exchanging them is the same power flow problem.
///
/// Returns the first row that disagrees, in the digest's own order.
#[must_use]
pub fn electrical_change_up_to_relabeling(
    before: &BalancedNetwork,
    after: &BalancedNetwork,
    tolerance: f64,
) -> Option<String> {
    // Where every bus states a nominal voltage, the digest holds physical
    // admittance: a format that admits only a fixed set of voltage levels
    // writes a 345 kV case under the 330 kV level, and the per-unit values move
    // with the base while the network does not. Where a bus states none, the
    // writer divided by the same substituted nominal voltage the reader
    // multiplies back, so the per-unit values are the comparable ones.
    let physical = [before, after].iter().all(|net| {
        net.buses()
            .iter()
            .all(|bus| bus.base_kv.is_finite() && bus.base_kv > 0.0)
    });
    let digest = |net: &BalancedNetwork| -> Option<(Vec<BusRow>, Vec<(f64, f64)>)> {
        let nominal = net
            .buses()
            .iter()
            .map(|bus| (bus.id.0, bus.base_kv))
            .collect::<BTreeMap<_, _>>();
        let scale = |from: usize, to: usize| -> f64 {
            if !physical {
                return 1.0;
            }
            let voltage = |bus: usize| nominal.get(&bus).copied().unwrap_or(1.0);
            net.base_mva() / (voltage(from) * voltage(to))
        };
        let view = powerio_matrix::IndexedNetwork::new(net);
        let parts =
            powerio_matrix::calc_admittance_matrix(&view, &powerio_matrix::BuildOptions::default())
                .ok()?;
        let mut entries = BTreeMap::<(usize, usize), (f64, f64)>::new();
        for (value, (i, j)) in &parts.g {
            entries
                .entry((view.bus_id(i).0, view.bus_id(j).0))
                .or_insert((0.0, 0.0))
                .0 = *value;
        }
        for (value, (i, j)) in &parts.b {
            entries
                .entry((view.bus_id(i).0, view.bus_id(j).0))
                .or_insert((0.0, 0.0))
                .1 = *value;
        }
        let injections = per_bus(net, true);
        let mut rows = Vec::new();
        let mut off_diagonal = Vec::new();
        for ((from, to), value) in entries {
            let value = (value.0 * scale(from, to), value.1 * scale(from, to));
            if from == to {
                rows.push(BusRow {
                    diagonal: value,
                    injections: injections.get(&from).copied().unwrap_or_default(),
                });
            } else {
                off_diagonal.push(value);
            }
        }
        let key = |value: &(f64, f64)| (value.0.to_bits(), value.1.to_bits());
        rows.sort_by_key(|row| (key(&row.diagonal), row.injections.map(f64::to_bits)));
        off_diagonal.sort_by_key(key);
        Some((rows, off_diagonal))
    };
    let (before_rows, before_off) = digest(before)?;
    let (after_rows, after_off) = digest(after)?;
    if before_rows.len() != after_rows.len() {
        return Some(format!(
            "{} bus admittance row(s) became {}",
            before_rows.len(),
            after_rows.len()
        ));
    }
    if before_off.len() != after_off.len() {
        return Some(format!(
            "{} off-diagonal admittance entr(ies) became {}",
            before_off.len(),
            after_off.len()
        ));
    }
    for (before, after) in before_rows.iter().zip(&after_rows) {
        let moved = beyond_tol(before.diagonal.0, after.diagonal.0, tolerance)
            || beyond_tol(before.diagonal.1, after.diagonal.1, tolerance)
            || before
                .injections
                .iter()
                .zip(&after.injections)
                .any(|(before, after)| beyond_tol(*before, *after, tolerance));
        if moved {
            return Some(format!("bus row {before:?} became {after:?}"));
        }
    }
    for (before, after) in before_off.iter().zip(&after_off) {
        if beyond_tol(before.0, after.0, tolerance) || beyond_tol(before.1, after.1, tolerance) {
            return Some(format!(
                "off-diagonal admittance {before:?} became {after:?}"
            ));
        }
    }
    None
}

/// Why a `Y_bus` comparison produced no answer./// Why a `Y_bus` comparison produced no answer.
///
/// Distinguished from "the matrices agree" on purpose: a network the matrix
/// builder refuses is a conversion failure, and reporting it as agreement is
/// how an invariant passes vacuously.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum YbusUnavailable {
    Before,
    After,
}

/// The admittance matrix, entry for entry by bus id pair, on one voltage base.
///
/// Warnings account for *dropped* data, never for corrupted electrics, so this
/// holds on yellow cells too. A format may relocate admittance (pandapower
/// rides MATPOWER transformer charging as bus shunts) or restate it in other
/// units, but `Y_bus` is where all of those spellings meet.
///
/// A format that states impedances in ohms rather than per unit carries its own
/// bus nominal voltages, and some of those formats admit only a fixed set: a
/// UCTE-DEF node code names one of ten voltage levels, so a 345 kV case is
/// written under the 330 kV level and reads back per unit on 330 kV. The
/// physical network is the same; only the per-unit reference moved. Each entry
/// is therefore compared after the standard change of base, scaling the result
/// entry by `V_before(i) * V_before(j) / (V_after(i) * V_after(j))`, which is
/// exactly 1 whenever both sides state the same nominal voltage. A bus with no
/// positive nominal voltage on either side contributes a unit ratio, because a
/// writer that substitutes one nominal voltage divides by the same value the
/// reader multiplies back.
///
/// A silent base change is still a loss: `base_kv` is a typed field, so the
/// model comparison reports it and a cell claiming zero warnings fails on it.
///
/// # Errors
///
/// Returns [`YbusUnavailable`] when either side has no buildable admittance
/// matrix.
pub fn ybus_change(
    before: &BalancedNetwork,
    after: &BalancedNetwork,
) -> Result<Option<YbusChange>, YbusUnavailable> {
    ybus_change_within(before, after, ELECTRICAL_TOL)
}

/// [`ybus_change`] under a stated relative tolerance, for a format that states
/// each value in a fixed width field and so cannot return more digits than the
/// field holds.
///
/// # Errors
///
/// Returns [`YbusUnavailable`] when either side has no buildable admittance
/// matrix.
pub fn ybus_change_within(
    before: &BalancedNetwork,
    after: &BalancedNetwork,
    tolerance: f64,
) -> Result<Option<YbusChange>, YbusUnavailable> {
    let nominal = |net: &BalancedNetwork| -> BTreeMap<usize, f64> {
        net.buses()
            .iter()
            .filter(|bus| bus.base_kv.is_finite() && bus.base_kv > 0.0)
            .map(|bus| (bus.id.0, bus.base_kv))
            .collect()
    };
    let nominal_before = nominal(before);
    let nominal_after = nominal(after);
    let base_ratio = |bus: usize| -> f64 {
        match (nominal_before.get(&bus), nominal_after.get(&bus)) {
            (Some(before), Some(after)) => before / after,
            _ => 1.0,
        }
    };
    let entries = |net: &BalancedNetwork| -> Option<BTreeMap<(usize, usize), (f64, f64)>> {
        let view = powerio_matrix::IndexedNetwork::new(net);
        let parts =
            powerio_matrix::calc_admittance_matrix(&view, &powerio_matrix::BuildOptions::default())
                .ok()?;
        let mut map = BTreeMap::new();
        for (value, (i, j)) in &parts.g {
            map.entry((view.bus_id(i).0, view.bus_id(j).0))
                .or_insert((0.0, 0.0))
                .0 = *value;
        }
        for (value, (i, j)) in &parts.b {
            map.entry((view.bus_id(i).0, view.bus_id(j).0))
                .or_insert((0.0, 0.0))
                .1 = *value;
        }
        Some(map)
    };
    let a = entries(before).ok_or(YbusUnavailable::Before)?;
    let b = entries(after).ok_or(YbusUnavailable::After)?;
    for key in a.keys().chain(b.keys().filter(|k| !a.contains_key(*k))) {
        let (ga, ba) = a.get(key).copied().unwrap_or((0.0, 0.0));
        let (gb, bb) = b.get(key).copied().unwrap_or((0.0, 0.0));
        let scale = base_ratio(key.0) * base_ratio(key.1);
        let (gb, bb) = (gb * scale, bb * scale);
        if beyond_tol(ga, gb, tolerance) || beyond_tol(ba, bb, tolerance) {
            return Ok(Some(YbusChange {
                from_bus: key.0,
                to_bus: key.1,
                before: (ga, ba),
                after: (gb, bb),
            }));
        }
    }
    Ok(None)
}

/// The other half of the AC operating point: per-bus demand and generation.
///
/// [`ybus_change`] pins the network matrices; this pins the injections at each
/// bus, so together a conversion that passes both leaves the power flow
/// problem unchanged — whatever it dropped in costs, limits, names, or extras.
/// No conversion may move power between buses; formats only merge or split
/// devices at one bus.
///
/// The injection compared is the *connected* one — in-service elements only,
/// which is what a solver sees. Comparing every element regardless of status
/// would let a conversion promote an out-of-service load to a live one without
/// moving the total, which changes the power flow problem while leaving the
/// stated numbers alone. `status_only` marks that case: connected power moved,
/// stated power did not.
///
/// Data loss on out-of-service elements is a loss, not corrupted electrics, so
/// it belongs to the model diff and the warning accounting rather than here.
#[must_use]
pub fn injection_change(
    before: &BalancedNetwork,
    after: &BalancedNetwork,
) -> Option<InjectionChange> {
    injection_change_within(before, after, ELECTRICAL_TOL)
}

/// [`injection_change`] under a stated relative tolerance, for a format that
/// states each value in a fixed width field and so cannot return more digits
/// than the field holds.
#[must_use]
pub fn injection_change_within(
    before: &BalancedNetwork,
    after: &BalancedNetwork,
    tolerance: f64,
) -> Option<InjectionChange> {
    let change = first_moved(
        &per_bus(before, true),
        &per_bus(after, true),
        AC_INJECTIONS,
        tolerance,
    )?;
    let status_only = first_moved(
        &per_bus(before, false),
        &per_bus(after, false),
        AC_INJECTIONS,
        tolerance,
    )
    .is_none();
    Some(InjectionChange {
        status_only,
        ..change
    })
}

/// What each slot of [`per_bus`] holds.
const AC_INJECTIONS: [Injection; 4] = [
    Injection::LoadP,
    Injection::LoadQ,
    Injection::GenP,
    Injection::GenQ,
];

/// What each slot of [`per_bus_dc`] holds.
const DC_TERMINALS: [Injection; 4] = [
    Injection::DcRectifierP,
    Injection::DcInverterP,
    Injection::DcRectifierQ,
    Injection::DcInverterQ,
];

fn per_bus(net: &BalancedNetwork, in_service_only: bool) -> BTreeMap<usize, [f64; 4]> {
    let mut map: BTreeMap<usize, [f64; 4]> = BTreeMap::new();
    for l in net
        .loads()
        .iter()
        .filter(|l| l.in_service || !in_service_only)
    {
        let e = map.entry(l.bus.0).or_default();
        e[0] += l.p;
        e[1] += l.q;
    }
    for g in net
        .generators()
        .iter()
        .filter(|g| g.in_service || !in_service_only)
    {
        let e = map.entry(g.bus.0).or_default();
        e[2] += g.pg;
        e[3] += g.qg;
    }
    map
}

/// Per-bus DC terminal power, the same tally for HVDC that [`per_bus`] is for
/// loads and generators.
///
/// The two converter roles keep separate slots rather than netting, so a bus
/// hosting one line's rectifier and another's inverter states both. Netting
/// them would let a conversion swap the ends of a pair and still compare
/// equal.
fn per_bus_dc(net: &BalancedNetwork, in_service_only: bool) -> BTreeMap<usize, [f64; 4]> {
    let mut map: BTreeMap<usize, [f64; 4]> = BTreeMap::new();
    for d in net
        .hvdc()
        .iter()
        .filter(|d| d.in_service || !in_service_only)
    {
        let from = map.entry(d.from.0).or_default();
        from[0] += d.pf;
        from[2] += d.qf;
        let to = map.entry(d.to.0).or_default();
        to[1] += d.pt;
        to[3] += d.qt;
    }
    map
}

/// Whether a DC line's terminal power moved across a conversion.
///
/// Separate from [`injection_change`] because the two carry different weight.
/// No format may move AC power between buses — they only merge and split
/// devices at one bus — so that is an outright failure. DC terminal power is
/// different: PowerWorld's aux vocabulary states no HVDC at all, PSS/E carries
/// the two ends only through `SETVL`/`RDC`/`VSCHD`, and those losses are real
/// and declared. A caller that wants the strict reading treats any result here
/// as a failure; the conversion matrix instead requires the loss to be
/// declared, which is what its warning parity and typed-model gates already
/// do.
///
/// A DC line never enters `Y_bus`, so without this property the two-terminal
/// DC surface sits outside every electrical check in this module.
#[must_use]
pub fn dc_terminal_change(
    before: &BalancedNetwork,
    after: &BalancedNetwork,
) -> Option<InjectionChange> {
    let change = first_moved(
        &per_bus_dc(before, true),
        &per_bus_dc(after, true),
        DC_TERMINALS,
        ELECTRICAL_TOL,
    )?;
    let status_only = first_moved(
        &per_bus_dc(before, false),
        &per_bus_dc(after, false),
        DC_TERMINALS,
        ELECTRICAL_TOL,
    )
    .is_none();
    Some(InjectionChange {
        status_only,
        ..change
    })
}

/// The first slot whose tally moved, named by `quantities`, which states what
/// each slot holds. One tally per domain rather than one wide one shared
/// between them: a shared array lets a caller write an AC total into a DC slot
/// with nothing to catch it, and makes every comparison scan the other
/// domain's always-zero half.
fn first_moved<const N: usize>(
    a: &BTreeMap<usize, [f64; N]>,
    b: &BTreeMap<usize, [f64; N]>,
    quantities: [Injection; N],
    tolerance: f64,
) -> Option<InjectionChange> {
    for key in a.keys().chain(b.keys().filter(|k| !a.contains_key(*k))) {
        let x = a.get(key).copied().unwrap_or([0.0; N]);
        let y = b.get(key).copied().unwrap_or([0.0; N]);
        for (i, quantity) in quantities.into_iter().enumerate() {
            if beyond_tol(x[i], y[i], tolerance) {
                return Some(InjectionChange {
                    bus: *key,
                    quantity,
                    before: x[i],
                    after: y[i],
                    status_only: false,
                });
            }
        }
    }
    None
}

/// Whether two values differ by more than `tol` relative.
///
/// A non-finite value on either side counts as a difference unless both sides
/// are the same kind of non-finite. Every comparison against NaN is false, so
/// the plain arithmetic would report a setpoint that turned into NaN as "no
/// change" — the one answer that must never be given about a value that stopped
/// being a number.
fn beyond_tol(x: f64, y: f64, tol: f64) -> bool {
    if !x.is_finite() || !y.is_finite() {
        // Two NaNs are the same absence of a number; every other pairing
        // involving a non-finite value is a change. Compared bitwise on
        // purpose: there is no margin of error to speak of once a value has
        // left the reals.
        #[allow(clippy::float_cmp)]
        return !(x.is_nan() && y.is_nan()) && x != y;
    }
    (x - y).abs() > tol * x.abs().max(y.abs()).max(1.0)
}

/// How many elements the two networks disagree about the service status of.
///
/// Counted by class rather than matched element by element, because two
/// formats number and order their tables differently and the count is what
/// makes the disagreement actionable. An out of service machine normally
/// states zero output, so this difference does not show up in the injections:
/// the two files agree about today's dispatch and disagree about what exists
/// to dispatch.
#[must_use]
pub fn status_disagreements(a: &BalancedNetwork, b: &BalancedNetwork) -> usize {
    let off = |net: &BalancedNetwork| {
        [
            net.generators().iter().filter(|g| !g.in_service).count(),
            net.branches().iter().filter(|x| !x.in_service).count(),
            net.loads().iter().filter(|l| !l.in_service).count(),
            net.shunts().iter().filter(|s| !s.in_service).count(),
            net.switches().iter().filter(|x| !x.closed).count(),
            net.storage().iter().filter(|x| !x.in_service).count(),
            net.hvdc().iter().filter(|d| !d.in_service).count(),
            net.transformers_3w()
                .iter()
                .filter(|t| !t.in_service)
                .count(),
        ]
    };
    off(a)
        .iter()
        .zip(off(b).iter())
        .map(|(x, y)| x.abs_diff(*y))
        .sum()
}

/// Element counts and the totals that must survive any conversion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransmissionCore {
    pub buses: usize,
    pub branches: usize,
    pub generators: usize,
    pub loads: usize,
    pub shunts: usize,
    pub load_p: i64,
    pub load_q: i64,
    pub gen_p: i64,
    pub base_mva: i64,
}

fn milli(x: f64) -> i64 {
    (x * 1e3).round() as i64
}

impl TransmissionCore {
    /// Whether two cores state the same case under a relative tolerance on the
    /// power totals.
    ///
    /// Element counts are compared exactly. A format that states each value in
    /// a fixed width field returns fewer digits than the total carries, so the
    /// totals are compared within `tolerance`.
    #[must_use]
    pub fn agrees_within(&self, other: &Self, tolerance: f64) -> bool {
        let totals = |core: &Self| [core.load_p, core.load_q, core.gen_p, core.base_mva];
        self.buses == other.buses
            && self.branches == other.branches
            && self.generators == other.generators
            && self.loads == other.loads
            && self.shunts == other.shunts
            && totals(self)
                .into_iter()
                .zip(totals(other))
                .all(|(before, after)| !beyond_tol(before as f64, after as f64, tolerance))
    }
}

#[must_use]
pub fn transmission_core(net: &BalancedNetwork) -> TransmissionCore {
    TransmissionCore {
        buses: net.buses().len(),
        branches: net.branches().len(),
        generators: net.generators().len(),
        loads: net.loads().len(),
        shunts: net.shunts().len(),
        load_p: milli(net.loads().iter().map(|load| load.p).sum()),
        load_q: milli(net.loads().iter().map(|load| load.q).sum()),
        gen_p: milli(net.generators().iter().map(|generator| generator.pg).sum()),
        base_mva: milli(net.base_mva()),
    }
}

/// Element counts and totals for a multiconductor network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DistributionCore {
    pub buses: usize,
    pub loads: usize,
    pub generators: usize,
    pub shunts: usize,
    pub load_p: i64,
    pub load_q: i64,
}

#[must_use]
pub fn distribution_core(net: &MulticonductorNetwork) -> DistributionCore {
    DistributionCore {
        buses: net.buses().len(),
        loads: net.loads().len(),
        generators: net.generators().len(),
        shunts: net.shunts().len(),
        load_p: milli(net.loads().iter().flat_map(|load| load.p_nom.iter()).sum()),
        load_q: milli(net.loads().iter().flat_map(|load| load.q_nom.iter()).sum()),
    }
}

/// A [`BalancedNetwork`] as a JSON value with the conversion-neutral identity
/// fields cleared and two representation choices canonicalized, so the model
/// diff compares data, not provenance or spelling:
///
/// - `charging: None` and the symmetric split it abbreviates are one fact;
///   both sides expand to the explicit terminal form.
/// - Element order is a table-layout artifact (pandapower re-groups lines and
///   trafos; several formats sort by id); everything sorts by identity.
///
/// # Panics
///
/// Panics if the network does not serialize, which would mean a model field
/// whose `Serialize` fails — a bug in the model rather than in the case.
#[must_use]
pub fn transmission_value(net: &BalancedNetwork) -> serde_json::Value {
    let mut net = net.clone();
    *net.name_mut() = String::new();
    *net.source_format_mut() = powerio_matrix::SourceFormat::Matpower;
    for br in net.branches_mut() {
        br.charging = Some(br.calc_terminal_charging());
    }
    net.buses_mut().sort_by_key(|b| b.id);
    net.branches_mut().sort_by_key(|a| (a.from, a.to));
    net.loads_mut().sort_by_key(|l| l.bus);
    net.shunts_mut().sort_by_key(|s| s.bus);
    net.generators_mut().sort_by_key(|g| g.bus);
    net.storage_mut().sort_by_key(|s| s.bus);
    net.hvdc_mut().sort_by_key(|d| (d.from, d.to));
    serde_json::to_value(&net).unwrap()
}

/// A [`MulticonductorNetwork`] as a JSON value with identity fields cleared.
///
/// # Panics
///
/// Panics if the network does not serialize; see [`transmission_value`].
#[must_use]
pub fn distribution_value(net: &MulticonductorNetwork) -> serde_json::Value {
    let mut net = net.clone();
    *net.name_mut() = None;
    *net.source_format_mut() = None;
    serde_json::to_value(&net).unwrap()
}

/// One typed-model field that changed across a conversion.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelDiff {
    /// Serde path, e.g. `.loads[3].p`. Element *names* never appear here: the
    /// typed model holds elements in arrays, so a path carries indices only.
    pub path: String,
    pub before: DiffSide,
    pub after: DiffSide,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DiffSide {
    Number(f64),
    /// A JSON value rendered compactly. May hold case data, so the corpus
    /// reporter never emits it verbatim.
    Value(String),
    /// The array length, when two arrays differ in length.
    Len(usize),
    Absent,
}

impl std::fmt::Display for DiffSide {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Number(x) => write!(f, "{x}"),
            Self::Value(v) => write!(f, "{v}"),
            Self::Len(n) => write!(f, "{n} entries"),
            Self::Absent => write!(f, "absent"),
        }
    }
}

impl std::fmt::Display for ModelDiff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Matches the rendering the matrix report has always used, so its
        // committed baselines and details tables do not move.
        match (&self.before, &self.after) {
            (DiffSide::Len(x), DiffSide::Len(y)) => {
                write!(f, "{}: {x} entries -> {y}", self.path)
            }
            (before, after) => write!(f, "{}: {before} -> {after}", self.path),
        }
    }
}

/// Every typed-model field the conversion changed, capped at
/// [`MAX_MODEL_DIFFS`].
///
/// Numbers compare within a last-ulp relative tolerance — the writers print
/// shortest round-trip floats, so anything beyond a text wobble is a real
/// change — and everything else compares exactly.
#[must_use]
pub fn model_diffs(before: &serde_json::Value, after: &serde_json::Value) -> Vec<ModelDiff> {
    let mut out = Vec::new();
    diff_values(before, after, "", &mut out);
    out
}

fn diff_values(a: &serde_json::Value, b: &serde_json::Value, path: &str, out: &mut Vec<ModelDiff>) {
    use serde_json::Value;
    if out.len() >= MAX_MODEL_DIFFS {
        return;
    }
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (
                x.as_f64().unwrap_or(f64::NAN),
                y.as_f64().unwrap_or(f64::NAN),
            );
            // Written as "not within" rather than "beyond": a NaN compares
            // false to everything, so it must fall through to a diff unless
            // both sides are NaN.
            let within = (x - y).abs() <= MODEL_TOL * x.abs().max(y.abs()).max(1.0);
            if !(within || (x.is_nan() && y.is_nan())) {
                out.push(ModelDiff {
                    path: path.to_string(),
                    before: DiffSide::Number(x),
                    after: DiffSide::Number(y),
                });
            }
        }
        (Value::Array(xs), Value::Array(ys)) => {
            if xs.len() != ys.len() {
                out.push(ModelDiff {
                    path: path.to_string(),
                    before: DiffSide::Len(xs.len()),
                    after: DiffSide::Len(ys.len()),
                });
                return;
            }
            for (i, (x, y)) in xs.iter().zip(ys).enumerate() {
                diff_values(x, y, &format!("{path}[{i}]"), out);
            }
        }
        (Value::Object(xs), Value::Object(ys)) => {
            for key in xs.keys().chain(ys.keys().filter(|k| !xs.contains_key(*k))) {
                // Rechecked per key: the recursive call guards itself, but the
                // two direct pushes below do not, and an object of many
                // one-sided keys would otherwise carry the list past the cap.
                if out.len() >= MAX_MODEL_DIFFS {
                    return;
                }
                let sub = format!("{path}.{key}");
                match (xs.get(key), ys.get(key)) {
                    (Some(x), Some(y)) => diff_values(x, y, &sub, out),
                    (Some(x), None) => out.push(ModelDiff {
                        path: sub,
                        before: DiffSide::Value(x.to_string()),
                        after: DiffSide::Absent,
                    }),
                    (None, Some(y)) => out.push(ModelDiff {
                        path: sub,
                        before: DiffSide::Absent,
                        after: DiffSide::Value(y.to_string()),
                    }),
                    (None, None) => unreachable!(),
                }
            }
        }
        (x, y) if x == y => {}
        (x, y) => out.push(ModelDiff {
            path: path.to_string(),
            before: DiffSide::Value(x.to_string()),
            after: DiffSide::Value(y.to_string()),
        }),
    }
}
