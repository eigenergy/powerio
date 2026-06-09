//! GridFM interchange: a parsed case as the gridfm-datakit Parquet schema.
//!
//! [`gridfm-datakit`](https://github.com/gridfm) writes per-scenario Parquet
//! tables that [`gridfm-graphkit`](https://github.com/gridfm)'s
//! `HeteroGridDatasetDisk` trains a GNN on. This module emits the same four
//! tables — `bus_data`, `gen_data`, `branch_data`, `y_bus_data` — from one
//! parsed [`Network`], so graphkit can train on powerio output directly and the
//! scenario-batch path (issue #14) has its on-disk format.
//!
//! # Snapshots and scenarios
//!
//! powerio has no power flow solver. One parsed case is one snapshot
//! (`scenario = 0`): voltages and generator dispatch are the case's stored
//! values, and branch flows `pf/qf/pt/qt` are computed from those voltages and
//! the branch admittances (`branch_flows`). For a solved MATPOWER case the
//! stored voltages are the converged operating point, so the flows match what a
//! solver would report to float tolerance; for an unsolved/flat start case they
//! are the flows at the stored voltages, not a re-solved dispatch.
//!
//! A scenario batch ([`write_gridfm_batch`] / [`gridfm_record_batches_batch`])
//! row-stacks many snapshots into the four tables, keyed by the `scenario`
//! column. The snapshots share a base element set — the same bus/branch/gen
//! counts and bus-id ordering, so the dense bus index means the same bus across
//! scenarios — enforced by the shape check ([`Error::ScenarioShapeMismatch`]).
//! Within that, load, dispatch, voltages, branch status, bus type, and costs may
//! all differ per snapshot. This matches datakit, whose topology variants (N-K,
//! random component drop) toggle `BR_STATUS`/`GEN_STATUS` on a fixed element set,
//! and graphkit's `HeteroGridDatasetDisk`, which groups by `scenario` and
//! rebuilds the graph independently for each one. powerio doesn't generate the
//! perturbations; a caller (e.g. a scenario generator) supplies the snapshots.
//!
//! # Units
//!
//! - `Pd, Qd, Pg, Qg, p_mw, q_mvar` are MW/MVAr, passed through from the case
//!   (loads and generator setpoints are already MW/MVAr). The branch flows
//!   `pf, qf, pt, qt` are MW/MVAr too, computed in per-unit and scaled by
//!   `base_mva`.
//! - `Vm` per-unit, `Va` degrees; `r, x, b` and the `Y**` admittances per-unit.
//! - `GS, BS` are the MATPOWER shunt values (MW/MVAr at V = 1) divided by
//!   `base_mva`, matching datakit's normalization.
//! - Costs are the raw MATPOWER coefficients: `cp2 = c2`, `cp1 = c1`,
//!   `cp0 = c0`. A cost row gridfm can't represent (piecewise, missing,
//!   malformed, or cubic and higher) emits zeros — graphkit ignores the cost
//!   columns — and is counted in the manifest. The `_eur` suffixes are
//!   datakit's column names, not a unit powerio converts to.
//! - `bus`, `from_bus`, `to_bus` are dense `[0, n)` indices; `idx` is the
//!   0-based generator/branch row. An out-of-service branch keeps its physical
//!   `Y**` admittances but carries zero flows (its `br_status` is 0).

// Bus/branch indices and Y_bus nnz counts cast to `i64` for the Arrow columns;
// they are bounded far below `i64::MAX`, so the wrap clippy warns about can't
// happen.
#![allow(clippy::cast_possible_wrap)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::array::{ArrayRef, Float64Array, Int64Array};
use arrow::datatypes::{Field, Schema};
use arrow::record_batch::RecordBatch;
use num_complex::Complex64;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use serde::Serialize;

use crate::indexed::IndexedNetwork;
use crate::matrix::{BuildOptions, YbusFlags, branch_admittance, branch_flows, build_ybus};
use crate::network::{BusId, BusType};
use crate::{ElementCounts, Error, GenCost, Network, Result, ScenarioMismatch};

/// Options for the gridfm export — the batch-wide knobs. The scenario id is a
/// per-snapshot property (set via [`GridfmSnapshot::new`] / [`numbered_snapshots`],
/// or the explicit argument to the single-case [`write_gridfm_dataset`] /
/// [`gridfm_record_batches`]), not an option here.
#[derive(Debug, Clone)]
pub struct GridfmOptions {
    /// Also write `y_bus_data.parquet`. graphkit reconstructs admittances from
    /// the branch table and ignores it, but datakit emits it, so the default is
    /// `true` for parity.
    pub include_y_bus: bool,
    /// Apply transformer tap ratios to the admittances. Default `true` (the
    /// physical admittances datakit stores).
    pub include_taps: bool,
    /// Apply phase shifts to the admittances. Default `true`.
    pub include_shifts: bool,
}

impl Default for GridfmOptions {
    fn default() -> Self {
        Self {
            include_y_bus: true,
            include_taps: true,
            include_shifts: true,
        }
    }
}

impl GridfmOptions {
    /// The Y_bus build flags these options select (only taps and shifts matter
    /// for the gridfm admittances; the B'/B'' scheme and zero-impedance policy
    /// don't apply).
    fn build_options(&self) -> BuildOptions {
        BuildOptions {
            include_taps: self.include_taps,
            include_shifts: self.include_shifts,
            ..Default::default()
        }
    }
}

/// One snapshot in a gridfm scenario batch: a parsed [`Network`] and the scenario
/// id stamped into its rows.
///
/// powerio has no solver, so each snapshot is an operating point a caller (e.g. a
/// scenario generator) has already produced. Snapshots in one batch share a base
/// element set — the same bus/branch/gen counts and bus-id ordering — so the
/// dense bus index means the same bus across snapshots and the tables stay
/// schema-consistent. The builders enforce that and otherwise return
/// [`Error::ScenarioShapeMismatch`]. Within that, load, dispatch, voltages,
/// branch status, bus type, and costs may all vary per snapshot — this mirrors
/// gridfm-datakit, whose topology variants (N-K, random component drop) toggle
/// `BR_STATUS`/`GEN_STATUS` on a fixed element set, and gridfm-graphkit, which
/// rebuilds the graph independently for every scenario.
#[derive(Debug, Clone, Copy)]
pub struct GridfmSnapshot<'a> {
    /// The parsed case for this scenario.
    net: &'a Network,
    /// The scenario id stamped into the `scenario`/`load_scenario_idx` columns.
    scenario: i64,
}

impl<'a> GridfmSnapshot<'a> {
    /// A snapshot pairing a network with the scenario id stamped into its rows.
    /// For the common "k-th input is `base + k`" numbering, prefer
    /// [`numbered_snapshots`], which assigns ids with checked arithmetic.
    #[must_use]
    pub fn new(net: &'a Network, scenario: i64) -> Self {
        Self { net, scenario }
    }
}

/// The gridfm-datakit tables as Arrow record batches. The Parquet writer builds
/// from these; a deferred gridfm-schema Arrow C Data Interface export (issue #38)
/// would reuse them. (The raw network Arrow export that ships in powerio-capi is
/// a different, lighter schema.)
///
/// For a scenario batch the tables are row-stacked: each table holds the rows of
/// every snapshot back-to-back, keyed by the `scenario` column (0-based dense bus
/// indices and generator/branch `idx` reset per scenario).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct GridfmTables {
    pub bus: RecordBatch,
    pub generator: RecordBatch,
    pub branch: RecordBatch,
    /// `None` when [`GridfmOptions::include_y_bus`] is off — the table isn't
    /// built (graphkit reconstructs admittances from the branch table anyway).
    pub y_bus: Option<RecordBatch>,
}

/// What [`write_gridfm_dataset`] wrote, plus the counts of columns it had to zero
/// (see the manifest) so a caller can surface them.
#[derive(Debug, Clone)]
pub struct GridfmOutputs {
    pub dir: PathBuf,
    pub files: Vec<PathBuf>,
    /// Branches with `r² + x² = 0`, whose admittance/flow columns were zeroed.
    pub dropped_zero_impedance: usize,
    /// Generators whose cost row gridfm couldn't represent, whose `cp*` columns
    /// were zeroed.
    pub degenerate_cost_gens: usize,
}

#[derive(Serialize)]
struct GridfmMeta {
    case_name: String,
    base_mva: f64,
    /// The first snapshot's scenario id (the base for a batch).
    scenario: i64,
    /// Number of stacked scenarios (1 for a single case).
    n_scenarios: usize,
    schema: &'static str,
    /// Shared base element set (equal across all snapshots by the shape check).
    n_buses: usize,
    n_branches: usize,
    /// In-service branch count of the **first** snapshot; branch status may
    /// differ per scenario, so this describes scenario 0, not the whole batch.
    n_branches_in_service: usize,
    n_gens: usize,
    /// Reference (slack) bus of the **first** snapshot. Each snapshot resolves
    /// its own reference and carries it in the bus `REF` / gen `is_slack_gen`
    /// columns; this records scenario 0's.
    reference_bus: usize,
    /// Branches with `r² + x² = 0` (admittance/flow columns zeroed), summed over
    /// every snapshot in the batch.
    dropped_zero_impedance: usize,
    /// Generators whose cost row gridfm can't represent (piecewise, missing,
    /// malformed, or cubic and higher; `cp*` columns zeroed), summed over every
    /// snapshot in the batch.
    degenerate_cost_gens: usize,
    files: Vec<String>,
    powerio_version: String,
}

/// Build the four gridfm tables for one network, stamping `scenario` into the id
/// columns. Pure (no I/O). A thin wrapper over [`gridfm_record_batches_batch`]
/// for one snapshot.
///
/// # Errors
/// [`Error::ReferenceBusCount`] unless the case has exactly one reference bus
/// (graphkit needs a slack), [`Error::NormalizedGridfmSnapshot`] for a normalized
/// input, [`Error::NonFiniteGridfmValue`] for a NaN/Inf field that would reach
/// Parquet, [`Error::NonFiniteSusceptance`] if a finite branch impedance still
/// yields a non-finite admittance, and [`Error::UnknownBus`] if a generator or
/// branch references a bus the network doesn't define.
pub fn gridfm_record_batches(
    net: &Network,
    scenario: i64,
    opts: &GridfmOptions,
) -> Result<GridfmTables> {
    let snap = GridfmSnapshot::new(net, scenario);
    gridfm_record_batches_batch(std::slice::from_ref(&snap), opts)
}

/// Build the four gridfm tables for a batch of scenarios, row-stacked and keyed
/// by the `scenario` column. Pure (no I/O). Each snapshot carries its own
/// scenario id; the `include_y_bus`/taps/shifts flags apply to every snapshot.
///
/// # Errors
/// [`Error::EmptyScenarioBatch`] for an empty batch,
/// [`Error::ScenarioShapeMismatch`] if the snapshots don't share one base element
/// set (counts + bus-id order), plus everything [`gridfm_record_batches`] can
/// return.
pub fn gridfm_record_batches_batch(
    snapshots: &[GridfmSnapshot],
    opts: &GridfmOptions,
) -> Result<GridfmTables> {
    let views = snapshot_views(snapshots)?;
    tables_from_views(&views, opts)
}

/// The four tables from already-built, shape-checked snapshot views. The Y_bus
/// table is built only when [`GridfmOptions::include_y_bus`] is set — otherwise
/// it's `None` and the per-snapshot `build_ybus` is skipped entirely.
fn tables_from_views(views: &[SnapshotView], opts: &GridfmOptions) -> Result<GridfmTables> {
    Ok(GridfmTables {
        bus: bus_batch(views)?,
        generator: gen_batch(views)?,
        branch: branch_batch(views, opts)?,
        y_bus: if opts.include_y_bus {
            Some(y_bus_batch(views, opts)?)
        } else {
            None
        },
    })
}

/// A resolved snapshot: its indexed view, scenario id, and reference bus.
struct SnapshotView<'a> {
    view: IndexedNetwork<'a>,
    scenario: i64,
    ref_bus: usize,
}

/// Build and shape-check the views for a scenario batch. Every snapshot must
/// resolve to exactly one reference bus and share the first snapshot's base
/// element set (bus / branch / generator counts and bus-id ordering), so the
/// row-stacked tables stay schema-consistent.
fn snapshot_views<'a>(snapshots: &'a [GridfmSnapshot<'a>]) -> Result<Vec<SnapshotView<'a>>> {
    let first = snapshots.first().ok_or(Error::EmptyScenarioBatch)?;
    let expected = shape_of(first.net);
    let expected_ids: Vec<BusId> = first.net.buses.iter().map(|b| b.id).collect();

    let mut views = Vec::with_capacity(snapshots.len());
    for (k, snap) in snapshots.iter().enumerate() {
        let got = shape_of(snap.net);
        if got != expected {
            return Err(Error::ScenarioShapeMismatch {
                index: k,
                reason: ScenarioMismatch::Counts { expected, got },
            });
        }
        let ids_match = snap
            .net
            .buses
            .iter()
            .map(|b| b.id)
            .eq(expected_ids.iter().copied());
        if !ids_match {
            return Err(Error::ScenarioShapeMismatch {
                index: k,
                reason: ScenarioMismatch::BusOrder,
            });
        }
        validate_snapshot_inputs(snap.net, snap.scenario)?;
        let view = IndexedNetwork::new(snap.net);
        let ref_bus = view.reference_bus_index()?;
        views.push(SnapshotView {
            view,
            scenario: snap.scenario,
            ref_bus,
        });
    }
    Ok(views)
}

fn validate_snapshot_inputs(net: &Network, scenario: i64) -> Result<()> {
    if net.is_normalized() {
        return Err(Error::NormalizedGridfmSnapshot { scenario });
    }
    if !net.base_mva.is_finite() || net.base_mva <= 0.0 {
        return Err(Error::InvalidBaseMva { base: net.base_mva });
    }

    for (row, b) in net.buses.iter().enumerate() {
        finite(scenario, "bus", row, "vm", b.vm)?;
        finite(scenario, "bus", row, "va", b.va)?;
        finite(scenario, "bus", row, "base_kv", b.base_kv)?;
        finite(scenario, "bus", row, "vmax", b.vmax)?;
        finite(scenario, "bus", row, "vmin", b.vmin)?;
    }
    for (row, l) in net.loads.iter().enumerate() {
        finite(scenario, "load", row, "p", l.p)?;
        finite(scenario, "load", row, "q", l.q)?;
    }
    for (row, s) in net.shunts.iter().enumerate() {
        finite(scenario, "shunt", row, "g", s.g)?;
        finite(scenario, "shunt", row, "b", s.b)?;
    }
    for (row, br) in net.branches.iter().enumerate() {
        finite(scenario, "branch", row, "r", br.r)?;
        finite(scenario, "branch", row, "x", br.x)?;
        finite(scenario, "branch", row, "b", br.b)?;
        finite(scenario, "branch", row, "tap", br.tap)?;
        finite(scenario, "branch", row, "shift", br.shift)?;
        finite(scenario, "branch", row, "angmin", br.angmin)?;
        finite(scenario, "branch", row, "angmax", br.angmax)?;
        finite(scenario, "branch", row, "rate_a", br.rate_a)?;
    }
    for (row, g) in net.generators.iter().enumerate() {
        finite(scenario, "generator", row, "pg", g.pg)?;
        finite(scenario, "generator", row, "qg", g.qg)?;
        finite(scenario, "generator", row, "pmax", g.pmax)?;
        finite(scenario, "generator", row, "pmin", g.pmin)?;
        finite(scenario, "generator", row, "qmax", g.qmax)?;
        finite(scenario, "generator", row, "qmin", g.qmin)?;
        if let Some(cost) = &g.cost {
            if cost_representable(Some(cost)) {
                for (k, &coeff) in cost.coeffs.iter().take(cost.ncost).enumerate() {
                    finite(scenario, "gencost", row, coeff_field(k), coeff)?;
                }
            }
        }
    }
    Ok(())
}

fn finite(
    scenario: i64,
    element: &'static str,
    row: usize,
    field: &'static str,
    value: f64,
) -> Result<()> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(Error::NonFiniteGridfmValue {
            scenario,
            element,
            row,
            field,
            value,
        })
    }
}

fn coeff_field(index: usize) -> &'static str {
    match index {
        0 => "coeffs[0]",
        1 => "coeffs[1]",
        2 => "coeffs[2]",
        _ => "coeffs",
    }
}

/// The base element shape a scenario batch shares.
fn shape_of(net: &Network) -> ElementCounts {
    ElementCounts {
        buses: net.buses.len(),
        branches: net.branches.len(),
        gens: net.generators.len(),
    }
}

/// Number a list of networks into snapshots, stamping the k-th `base + k` — the
/// one place the "k-th input is scenario `base + k`" rule lives, so the CLI and
/// the Python binding can't drift. Checked: returns [`Error::ScenarioIdOverflow`]
/// rather than wrapping or panicking if a scenario id exceeds `i64`.
pub fn numbered_snapshots<'a>(nets: &[&'a Network], base: i64) -> Result<Vec<GridfmSnapshot<'a>>> {
    nets.iter()
        .enumerate()
        .map(|(k, &net)| {
            let scenario = i64::try_from(k)
                .ok()
                .and_then(|offset| base.checked_add(offset))
                .ok_or(Error::ScenarioIdOverflow { base, index: k })?;
            Ok(GridfmSnapshot::new(net, scenario))
        })
        .collect()
}

/// Write the gridfm-datakit Parquet dataset for one case under
/// `out_dir/<network_name>/raw/`, matching datakit's directory layout. Stamps
/// `scenario` into the id columns. Writes `bus_data.parquet`, `gen_data.parquet`,
/// `branch_data.parquet`, optionally `y_bus_data.parquet`, and a
/// `gridfm_meta.json` manifest.
///
/// Expects a raw snapshot (powers in MW, angles in degrees); pass the parsed
/// `Network`, not a [`to_normalized`](powerio::Network::to_normalized) per-unit
/// product, whose fields would be mislabeled.
///
/// # Errors
/// Propagates [`gridfm_record_batches`] and any filesystem/Parquet error.
pub fn write_gridfm_dataset(
    net: &Network,
    scenario: i64,
    out_dir: impl AsRef<Path>,
    opts: &GridfmOptions,
) -> Result<GridfmOutputs> {
    let snap = GridfmSnapshot::new(net, scenario);
    write_gridfm_batch(std::slice::from_ref(&snap), out_dir, opts)
}

/// Write a batch of scenarios as one gridfm-datakit dataset under
/// `out_dir/<network_name>/raw/`, row-stacking every snapshot's tables and keying
/// them by the `scenario` column. The dataset name and the base element counts
/// come from the first snapshot (shared across the batch by the shape check); the
/// dropped/degenerate counts are summed over every snapshot, while `reference_bus`
/// / `n_branches_in_service` record the first snapshot only (they can differ per
/// scenario, so the manifest documents them as scenario 0's).
///
/// # Errors
/// Propagates [`gridfm_record_batches_batch`] and any filesystem/Parquet error.
pub fn write_gridfm_batch(
    snapshots: &[GridfmSnapshot],
    out_dir: impl AsRef<Path>,
    opts: &GridfmOptions,
) -> Result<GridfmOutputs> {
    let views = snapshot_views(snapshots)?;
    let tables = tables_from_views(&views, opts)?;

    // The shape check guarantees every snapshot shares the base element set, so
    // the name and structural counts come from the first.
    let net = views[0].view.network();
    let dir = out_dir.as_ref().join(&net.name).join("raw");
    std::fs::create_dir_all(&dir)?;

    let mut files = Vec::new();
    put_parquet(&dir, "bus_data.parquet", &tables.bus, &mut files)?;
    put_parquet(&dir, "gen_data.parquet", &tables.generator, &mut files)?;
    put_parquet(&dir, "branch_data.parquet", &tables.branch, &mut files)?;
    if let Some(y_bus) = &tables.y_bus {
        put_parquet(&dir, "y_bus_data.parquet", y_bus, &mut files)?;
    }

    // Branch status and costs may differ per scenario, so count the zeroed rows
    // across every snapshot — the totals describe the whole stacked dataset.
    let dropped_zero_impedance: usize = views
        .iter()
        .flat_map(|v| v.view.network().branches.iter())
        .filter(|br| br.r * br.r + br.x * br.x == 0.0)
        .count();
    let degenerate_cost_gens: usize = views
        .iter()
        .flat_map(|v| v.view.network().generators.iter())
        .filter(|g| !cost_representable(g.cost.as_ref()))
        .count();

    let meta = GridfmMeta {
        case_name: net.name.clone(),
        base_mva: net.base_mva,
        scenario: views[0].scenario,
        n_scenarios: views.len(),
        schema: "gridfm-datakit",
        n_buses: net.buses.len(),
        n_branches: net.branches.len(),
        n_branches_in_service: net.branches.iter().filter(|b| b.in_service).count(),
        n_gens: net.generators.len(),
        reference_bus: views[0].ref_bus,
        dropped_zero_impedance,
        degenerate_cost_gens,
        files: files
            .iter()
            .filter_map(|p| p.file_name().and_then(|s| s.to_str()).map(str::to_string))
            .collect(),
        powerio_version: env!("CARGO_PKG_VERSION").to_string(),
    };
    let meta_path = dir.join("gridfm_meta.json");
    let json = serde_json::to_string_pretty(&meta).map_err(|e| Error::Parquet(e.to_string()))?;
    std::fs::write(&meta_path, json)?;
    files.push(meta_path);

    Ok(GridfmOutputs {
        dir,
        files,
        dropped_zero_impedance,
        degenerate_cost_gens,
    })
}

// --- table builders --------------------------------------------------------

fn bus_batch(snaps: &[SnapshotView]) -> Result<RecordBatch> {
    let total: usize = snaps.iter().map(|s| s.view.n()).sum();
    let mut scenario = Vec::with_capacity(total);
    let mut bus_idx = Vec::with_capacity(total);
    let (mut pd, mut qd) = (Vec::with_capacity(total), Vec::with_capacity(total));
    let (mut pg_col, mut qg_col) = (Vec::with_capacity(total), Vec::with_capacity(total));
    let (mut vm, mut va) = (Vec::with_capacity(total), Vec::with_capacity(total));
    let (mut pq, mut pv, mut refc) = (
        Vec::with_capacity(total),
        Vec::with_capacity(total),
        Vec::with_capacity(total),
    );
    let mut vn_kv = Vec::with_capacity(total);
    let (mut min_vm, mut max_vm) = (Vec::with_capacity(total), Vec::with_capacity(total));
    let (mut gs, mut bs) = (Vec::with_capacity(total), Vec::with_capacity(total));

    for s in snaps {
        let view = &s.view;
        let n = view.n();
        let base = view.base_mva();
        let buses = &view.network().buses;

        // Per-bus generation, summed over in-service generators (dense order).
        let mut pg = vec![0.0; n];
        let mut qg = vec![0.0; n];
        for (_, g) in view.in_service_gens() {
            if let Some(i) = view.bus_index(g.bus) {
                pg[i] += g.pg;
                qg[i] += g.qg;
            }
        }

        scenario.resize(scenario.len() + n, s.scenario);
        bus_idx.extend(0..n as i64);
        pd.extend_from_slice(view.pd());
        qd.extend_from_slice(view.qd());
        pg_col.extend(pg);
        qg_col.extend(qg);
        vm.extend(buses.iter().map(|b| b.vm));
        va.extend(buses.iter().map(|b| b.va));
        pq.extend(buses.iter().map(|b| i64::from(b.kind == BusType::Pq)));
        pv.extend(buses.iter().map(|b| i64::from(b.kind == BusType::Pv)));
        refc.extend(buses.iter().map(|b| i64::from(b.kind == BusType::Ref)));
        vn_kv.extend(buses.iter().map(|b| b.base_kv));
        min_vm.extend(buses.iter().map(|b| b.vmin));
        max_vm.extend(buses.iter().map(|b| b.vmax));
        gs.extend(view.gs().iter().map(|g| g / base));
        bs.extend(view.bs().iter().map(|b| b / base));
    }

    batch(with_scenario_pair(
        scenario,
        vec![
            ("bus", i64s(bus_idx)),
            ("Pd", f64s(pd)),
            ("Qd", f64s(qd)),
            ("Pg", f64s(pg_col)),
            ("Qg", f64s(qg_col)),
            ("Vm", f64s(vm)),
            ("Va", f64s(va)),
            ("PQ", i64s(pq)),
            ("PV", i64s(pv)),
            ("REF", i64s(refc)),
            ("vn_kv", f64s(vn_kv)),
            ("min_vm_pu", f64s(min_vm)),
            ("max_vm_pu", f64s(max_vm)),
            ("GS", f64s(gs)),
            ("BS", f64s(bs)),
        ],
    ))
}

fn gen_batch(snaps: &[SnapshotView]) -> Result<RecordBatch> {
    let total: usize = snaps.iter().map(|s| s.view.generators().len()).sum();
    let mut scenario = Vec::with_capacity(total);
    let mut idx = Vec::with_capacity(total);
    let mut bus = Vec::with_capacity(total);
    let (mut p_mw, mut q_mvar) = (Vec::with_capacity(total), Vec::with_capacity(total));
    let (mut min_p, mut max_p) = (Vec::with_capacity(total), Vec::with_capacity(total));
    let (mut min_q, mut max_q) = (Vec::with_capacity(total), Vec::with_capacity(total));
    let (mut cp0, mut cp1, mut cp2) = (
        Vec::with_capacity(total),
        Vec::with_capacity(total),
        Vec::with_capacity(total),
    );
    let mut in_service = Vec::with_capacity(total);
    let mut is_slack = Vec::with_capacity(total);

    for s in snaps {
        let view = &s.view;
        // One pass over the snapshot's generators: every column gets one push per
        // generator, in dense source order.
        for (row, g) in view.generators().iter().enumerate() {
            let i = view.bus_index(g.bus).ok_or(Error::UnknownBus {
                bus_id: g.bus,
                element_index: row,
            })?;
            scenario.push(s.scenario);
            idx.push(row as i64);
            bus.push(i as i64);
            is_slack.push(i64::from(i == s.ref_bus));
            let (c0, c1, c2) = gridfm_cost(g.cost.as_ref());
            cp0.push(c0);
            cp1.push(c1);
            cp2.push(c2);
            p_mw.push(g.pg);
            q_mvar.push(g.qg);
            min_p.push(g.pmin);
            max_p.push(g.pmax);
            min_q.push(g.qmin);
            max_q.push(g.qmax);
            in_service.push(i64::from(g.in_service));
        }
    }

    batch(with_scenario_pair(
        scenario,
        vec![
            ("idx", i64s(idx)),
            ("bus", i64s(bus)),
            ("p_mw", f64s(p_mw)),
            ("q_mvar", f64s(q_mvar)),
            ("min_p_mw", f64s(min_p)),
            ("max_p_mw", f64s(max_p)),
            ("min_q_mvar", f64s(min_q)),
            ("max_q_mvar", f64s(max_q)),
            ("cp0_eur", f64s(cp0)),
            ("cp1_eur_per_mw", f64s(cp1)),
            ("cp2_eur_per_mw2", f64s(cp2)),
            ("in_service", i64s(in_service)),
            ("is_slack_gen", i64s(is_slack)),
        ],
    ))
}

#[allow(clippy::too_many_lines, clippy::many_single_char_names)]
fn branch_batch(snaps: &[SnapshotView], opts: &GridfmOptions) -> Result<RecordBatch> {
    let total: usize = snaps.iter().map(|s| s.view.branches().len()).sum();

    // Same flags the Y_bus builder derives, so the branch admittance columns and
    // y_bus_data come from one kernel. The taps/shifts flags are batch-wide (from
    // `opts`); the admittances themselves are recomputed per snapshot.
    let flags = YbusFlags {
        unity_taps: !opts.include_taps,
        zero_shifts: !opts.include_shifts,
        ..Default::default()
    };

    let mut scenario = Vec::with_capacity(total);
    let mut idx = Vec::with_capacity(total);
    let (mut from_bus, mut to_bus) = (Vec::with_capacity(total), Vec::with_capacity(total));
    let (mut pf, mut qf, mut pt, mut qt) = (
        Vec::with_capacity(total),
        Vec::with_capacity(total),
        Vec::with_capacity(total),
        Vec::with_capacity(total),
    );
    let (mut yff_r, mut yff_i) = (Vec::with_capacity(total), Vec::with_capacity(total));
    let (mut yft_r, mut yft_i) = (Vec::with_capacity(total), Vec::with_capacity(total));
    let (mut ytf_r, mut ytf_i) = (Vec::with_capacity(total), Vec::with_capacity(total));
    let (mut ytt_r, mut ytt_i) = (Vec::with_capacity(total), Vec::with_capacity(total));
    let (mut r_col, mut x_col, mut b_col) = (
        Vec::with_capacity(total),
        Vec::with_capacity(total),
        Vec::with_capacity(total),
    );
    let (mut tap, mut shift) = (Vec::with_capacity(total), Vec::with_capacity(total));
    let (mut ang_min, mut ang_max) = (Vec::with_capacity(total), Vec::with_capacity(total));
    let mut rate_a = Vec::with_capacity(total);
    let mut br_status = Vec::with_capacity(total);

    for s in snaps {
        let view = &s.view;
        let base = view.base_mva();
        let branches = view.branches();
        let buses = &view.network().buses;
        // Complex bus voltages `vm·e^{jθ}`, dense order, for the flow evaluation.
        let v: Vec<Complex64> = buses
            .iter()
            .map(|b| Complex64::from_polar(b.vm, b.va.to_radians()))
            .collect();

        scenario.resize(scenario.len() + branches.len(), s.scenario);
        idx.extend(0..branches.len() as i64);

        for (row, br) in branches.iter().enumerate() {
            let i = view.bus_index(br.from).ok_or(Error::UnknownBus {
                bus_id: br.from,
                element_index: row,
            })?;
            let j = view.bus_index(br.to).ok_or(Error::UnknownBus {
                bus_id: br.to,
                element_index: row,
            })?;
            from_bus.push(i as i64);
            to_bus.push(j as i64);

            // Zero-impedance branch → `None` → zeroed admittance/flow columns (never NaN).
            let shift_rad = if flags.zero_shifts {
                0.0
            } else {
                view.angle_radians(br.shift)
            };
            let block = branch_admittance(br, flags, shift_rad, row)?;
            let [y_ff, y_ft, y_tf, y_tt] = block.unwrap_or([Complex64::new(0.0, 0.0); 4]);
            yff_r.push(y_ff.re);
            yff_i.push(y_ff.im);
            yft_r.push(y_ft.re);
            yft_i.push(y_ft.im);
            ytf_r.push(y_tf.re);
            ytf_i.push(y_tf.im);
            ytt_r.push(y_tt.re);
            ytt_i.push(y_tt.im);

            let (sf, st) = if br.in_service && block.is_some() {
                branch_flows(&[y_ff, y_ft, y_tf, y_tt], v[i], v[j])
            } else {
                (Complex64::new(0.0, 0.0), Complex64::new(0.0, 0.0))
            };
            pf.push(sf.re * base);
            qf.push(sf.im * base);
            pt.push(st.re * base);
            qt.push(st.im * base);

            r_col.push(br.r);
            x_col.push(br.x);
            b_col.push(br.b);
            tap.push(br.effective_tap());
            shift.push(br.shift);
            ang_min.push(br.angmin);
            ang_max.push(br.angmax);
            rate_a.push(br.rate_a);
            br_status.push(i64::from(br.in_service));
        }
    }

    batch(with_scenario_pair(
        scenario,
        vec![
            ("idx", i64s(idx)),
            ("from_bus", i64s(from_bus)),
            ("to_bus", i64s(to_bus)),
            ("pf", f64s(pf)),
            ("qf", f64s(qf)),
            ("pt", f64s(pt)),
            ("qt", f64s(qt)),
            ("r", f64s(r_col)),
            ("x", f64s(x_col)),
            ("b", f64s(b_col)),
            ("Yff_r", f64s(yff_r)),
            ("Yff_i", f64s(yff_i)),
            ("Yft_r", f64s(yft_r)),
            ("Yft_i", f64s(yft_i)),
            ("Ytf_r", f64s(ytf_r)),
            ("Ytf_i", f64s(ytf_i)),
            ("Ytt_r", f64s(ytt_r)),
            ("Ytt_i", f64s(ytt_i)),
            ("tap", f64s(tap)),
            ("shift", f64s(shift)),
            ("ang_min", f64s(ang_min)),
            ("ang_max", f64s(ang_max)),
            ("rate_a", f64s(rate_a)),
            ("br_status", i64s(br_status)),
        ],
    ))
}

fn y_bus_batch(snaps: &[SnapshotView], opts: &GridfmOptions) -> Result<RecordBatch> {
    // Upper bound on stacked nnz: each snapshot's Y_bus has at most 4 entries per
    // branch plus a diagonal. The exact count varies (lossless branches, dropped
    // zeros, per-scenario branch status), so this only sizes the allocation.
    let est: usize = snaps
        .iter()
        .map(|s| 4 * s.view.branches().len() + s.view.n())
        .sum();
    let mut scenario = Vec::with_capacity(est);
    let mut index1 = Vec::with_capacity(est);
    let mut index2 = Vec::with_capacity(est);
    let mut g_vals = Vec::with_capacity(est);
    let mut b_vals = Vec::with_capacity(est);

    for s in snaps {
        let parts = build_ybus(&s.view, &opts.build_options())?;
        // G and B don't share a sparsity pattern: a lossless branch (r = 0) is a
        // pure reactance, so its G entries are structurally zero where B's aren't.
        // datakit keys y_bus rows on the complex value being nonzero, i.e. the
        // union of the G and B positions. Merge into a sorted (row, col) map so the
        // output is row-major like `np.nonzero`, then drop any all-zero position.
        let mut entries: std::collections::BTreeMap<(usize, usize), (f64, f64)> =
            std::collections::BTreeMap::new();
        for (row, g_row) in parts.g.outer_iterator().enumerate() {
            for (col, &gv) in g_row.iter() {
                entries.entry((row, col)).or_default().0 = gv;
            }
        }
        for (row, b_row) in parts.b.outer_iterator().enumerate() {
            for (col, &bv) in b_row.iter() {
                entries.entry((row, col)).or_default().1 = bv;
            }
        }

        for ((row, col), (gv, bv)) in entries {
            if gv == 0.0 && bv == 0.0 {
                continue;
            }
            scenario.push(s.scenario);
            index1.push(row as i64);
            index2.push(col as i64);
            g_vals.push(gv);
            b_vals.push(bv);
        }
    }

    batch(with_scenario_pair(
        scenario,
        vec![
            ("index1", i64s(index1)),
            ("index2", i64s(index2)),
            ("G", f64s(g_vals)),
            ("B", f64s(b_vals)),
        ],
    ))
}

// --- small helpers ---------------------------------------------------------

/// `(cp0, cp1, cp2)` = raw MATPOWER `(c0, c1, c2)` from a polynomial cost row.
/// Coefficients are highest-order first, so `ncost == 3` is `[c2, c1, c0]`.
/// Piecewise, missing, or malformed rows give zeros.
fn gridfm_cost(cost: Option<&GenCost>) -> (f64, f64, f64) {
    match cost {
        Some(c) if c.model == 2 && c.coeffs.len() >= c.ncost => match c.ncost {
            3 => (c.coeffs[2], c.coeffs[1], c.coeffs[0]),
            2 => (c.coeffs[1], c.coeffs[0], 0.0),
            1 => (c.coeffs[0], 0.0, 0.0),
            _ => (0.0, 0.0, 0.0),
        },
        _ => (0.0, 0.0, 0.0),
    }
}

/// Whether [`gridfm_cost`] can represent this cost row (used for the manifest's
/// degenerate-cost count).
fn cost_representable(cost: Option<&GenCost>) -> bool {
    matches!(cost, Some(c) if c.model == 2 && c.coeffs.len() >= c.ncost && (1..=3).contains(&c.ncost))
}

fn put_parquet(
    dir: &Path,
    name: &str,
    batch: &RecordBatch,
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    let path = dir.join(name);
    let file = std::fs::File::create(&path)?;
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .build();
    let mut writer = ArrowWriter::try_new(file, batch.schema(), Some(props))
        .map_err(|e| Error::Parquet(e.to_string()))?;
    writer
        .write(batch)
        .map_err(|e| Error::Parquet(e.to_string()))?;
    writer.close().map_err(|e| Error::Parquet(e.to_string()))?;
    files.push(path);
    Ok(())
}

/// Assemble a [`RecordBatch`] from named columns, in order; field types are read
/// off the arrays and all columns are non-null.
fn batch(columns: Vec<(&str, ArrayRef)>) -> Result<RecordBatch> {
    let fields: Vec<Field> = columns
        .iter()
        .map(|(name, arr)| Field::new(*name, arr.data_type().clone(), false))
        .collect();
    let arrays: Vec<ArrayRef> = columns.into_iter().map(|(_, arr)| arr).collect();
    RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays)
        .map_err(|e| Error::Parquet(e.to_string()))
}

fn i64s(v: Vec<i64>) -> ArrayRef {
    Arc::new(Int64Array::from(v))
}

fn f64s(v: Vec<f64>) -> ArrayRef {
    Arc::new(Float64Array::from(v))
}

/// Prepend the `scenario` / `load_scenario_idx` id columns (which hold identical
/// values) to a table's other columns. The Int64 array is built once and the Arc
/// shared, so the duplicate column costs a pointer, not a second `Vec`.
fn with_scenario_pair(
    scenario: Vec<i64>,
    rest: Vec<(&'static str, ArrayRef)>,
) -> Vec<(&'static str, ArrayRef)> {
    let scenario = i64s(scenario);
    let mut cols = Vec::with_capacity(rest.len() + 2);
    cols.push(("scenario", scenario.clone()));
    cols.push(("load_scenario_idx", scenario));
    cols.extend(rest);
    cols
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::{Branch, Bus, BusId, Extras, Generator};
    use arrow::array::{Float64Array, Int64Array};
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

    const BUS_COLS: &[&str] = &[
        "scenario",
        "load_scenario_idx",
        "bus",
        "Pd",
        "Qd",
        "Pg",
        "Qg",
        "Vm",
        "Va",
        "PQ",
        "PV",
        "REF",
        "vn_kv",
        "min_vm_pu",
        "max_vm_pu",
        "GS",
        "BS",
    ];
    const GEN_COLS: &[&str] = &[
        "scenario",
        "load_scenario_idx",
        "idx",
        "bus",
        "p_mw",
        "q_mvar",
        "min_p_mw",
        "max_p_mw",
        "min_q_mvar",
        "max_q_mvar",
        "cp0_eur",
        "cp1_eur_per_mw",
        "cp2_eur_per_mw2",
        "in_service",
        "is_slack_gen",
    ];
    const BRANCH_COLS: &[&str] = &[
        "scenario",
        "load_scenario_idx",
        "idx",
        "from_bus",
        "to_bus",
        "pf",
        "qf",
        "pt",
        "qt",
        "r",
        "x",
        "b",
        "Yff_r",
        "Yff_i",
        "Yft_r",
        "Yft_i",
        "Ytf_r",
        "Ytf_i",
        "Ytt_r",
        "Ytt_i",
        "tap",
        "shift",
        "ang_min",
        "ang_max",
        "rate_a",
        "br_status",
    ];
    const YBUS_COLS: &[&str] = &[
        "scenario",
        "load_scenario_idx",
        "index1",
        "index2",
        "G",
        "B",
    ];

    fn case14() -> Network {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/case14.m");
        crate::parse_matpower_file(path).unwrap()
    }

    fn names(b: &RecordBatch) -> Vec<String> {
        b.schema()
            .fields()
            .iter()
            .map(|f| f.name().clone())
            .collect()
    }

    fn col_i64<'a>(b: &'a RecordBatch, name: &str) -> &'a Int64Array {
        b.column_by_name(name)
            .unwrap()
            .as_any()
            .downcast_ref()
            .unwrap()
    }

    fn col_f64<'a>(b: &'a RecordBatch, name: &str) -> &'a Float64Array {
        b.column_by_name(name)
            .unwrap()
            .as_any()
            .downcast_ref()
            .unwrap()
    }

    fn read(path: &Path) -> RecordBatch {
        let file = std::fs::File::open(path).unwrap();
        let mut reader = ParquetRecordBatchReaderBuilder::try_new(file)
            .unwrap()
            .build()
            .unwrap();
        // case14 fits in one row group / batch.
        reader.next().unwrap().unwrap()
    }

    #[test]
    fn schema_and_row_counts_match_case14() {
        let net = case14();
        let tables = gridfm_record_batches(&net, 0, &GridfmOptions::default()).unwrap();

        assert_eq!(names(&tables.bus), BUS_COLS);
        assert_eq!(names(&tables.generator), GEN_COLS);
        assert_eq!(names(&tables.branch), BRANCH_COLS);
        assert_eq!(names(tables.y_bus.as_ref().unwrap()), YBUS_COLS);

        assert_eq!(tables.bus.num_rows(), net.buses.len()); // 14
        assert_eq!(tables.generator.num_rows(), net.generators.len()); // 5
        assert_eq!(tables.branch.num_rows(), net.branches.len()); // 20
    }

    #[test]
    fn parquet_round_trips_through_reader() {
        let net = case14();
        let dir = tempfile::tempdir().unwrap();
        let out = write_gridfm_dataset(&net, 0, dir.path(), &GridfmOptions::default()).unwrap();

        let raw = dir.path().join("case14").join("raw");
        assert_eq!(out.dir, raw);
        for f in ["bus_data", "gen_data", "branch_data", "y_bus_data"] {
            assert!(raw.join(format!("{f}.parquet")).is_file(), "missing {f}");
        }
        assert!(raw.join("gridfm_meta.json").is_file());

        let bus = read(&raw.join("bus_data.parquet"));
        assert_eq!(names(&bus), BUS_COLS);
        assert_eq!(bus.num_rows(), net.buses.len());
        assert_eq!(names(&read(&raw.join("gen_data.parquet"))), GEN_COLS);
        assert_eq!(names(&read(&raw.join("branch_data.parquet"))), BRANCH_COLS);
        assert_eq!(names(&read(&raw.join("y_bus_data.parquet"))), YBUS_COLS);
    }

    #[test]
    fn bus_table_values_are_consistent() {
        let net = case14();
        let view = IndexedNetwork::new(&net);
        let tables = gridfm_record_batches(&net, 0, &GridfmOptions::default()).unwrap();
        let bus = &tables.bus;

        // Exactly one reference bus; PQ/PV/REF partition every bus.
        let (pq, pv, r) = (col_i64(bus, "PQ"), col_i64(bus, "PV"), col_i64(bus, "REF"));
        assert_eq!(r.values().iter().sum::<i64>(), 1);
        for i in 0..bus.num_rows() {
            assert_eq!(pq.value(i) + pv.value(i) + r.value(i), 1);
        }

        // GS/BS are the per-bus shunt aggregate divided by base_mva.
        let base = net.base_mva;
        let gs = col_f64(bus, "GS");
        for i in 0..bus.num_rows() {
            assert!((gs.value(i) - view.gs()[i] / base).abs() < 1e-12);
        }

        // `bus` column is the dense 0..n range.
        let bus_idx = col_i64(bus, "bus");
        for i in 0..bus.num_rows() {
            assert_eq!(bus_idx.value(i), i as i64);
        }
    }

    #[test]
    fn branch_admittance_columns_match_build_ybus() {
        // The branch table's Y** columns are the same kernel build_ybus scatters,
        // so a known in-service branch's block must equal branch_admittance.
        let net = case14();
        let tables = gridfm_record_batches(&net, 0, &GridfmOptions::default()).unwrap();
        let br = &tables.branch;

        let yff_r = col_f64(br, "Yff_r");
        let yff_i = col_f64(br, "Yff_i");
        for (row, branch) in net.branches.iter().enumerate() {
            // Raw fixture, so the shift is in degrees — convert as build_ybus does.
            let shift_rad = branch.shift.to_radians();
            if let Some(block) =
                branch_admittance(branch, YbusFlags::default(), shift_rad, row).unwrap()
            {
                assert!((yff_r.value(row) - block[0].re).abs() < 1e-12);
                assert!((yff_i.value(row) - block[0].im).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn is_slack_gen_marks_the_reference_bus() {
        let net = case14();
        let view = IndexedNetwork::new(&net);
        let ref_bus = view.reference_bus_index().unwrap();
        let tables = gridfm_record_batches(&net, 0, &GridfmOptions::default()).unwrap();
        let g = &tables.generator;

        let bus = col_i64(g, "bus");
        let slack = col_i64(g, "is_slack_gen");
        for i in 0..g.num_rows() {
            assert_eq!(slack.value(i) == 1, bus.value(i) as usize == ref_bus);
        }
        assert!(slack.values().contains(&1), "no slack generator");
    }

    #[test]
    fn branch_flows_close_the_power_balance_on_a_solved_case() {
        // case14 ships a converged operating point, so the active flows must obey
        // KCL: total branch loss = generation - demand - shunt draw. This is the
        // value-level guard on branch_flows (a missing ×base, a sign flip, or a
        // wrong conj would break it). Every branch's real loss is also ≥ 0.
        let net = case14();
        let view = IndexedNetwork::new(&net);
        let tables = gridfm_record_batches(&net, 0, &GridfmOptions::default()).unwrap();
        let br = &tables.branch;
        let (pf, pt, status) = (
            col_f64(br, "pf"),
            col_f64(br, "pt"),
            col_i64(br, "br_status"),
        );

        let mut loss = 0.0;
        for i in 0..br.num_rows() {
            if status.value(i) == 1 {
                let l = pf.value(i) + pt.value(i);
                assert!(l >= -1e-6, "branch {i} has negative real loss {l}");
                loss += l;
            }
        }
        assert!(loss > 1.0, "case14 has ~13 MW of real loss, got {loss}");

        let gen_p: f64 = net
            .generators
            .iter()
            .filter(|g| g.in_service)
            .map(|g| g.pg)
            .sum();
        let load_p: f64 = net.loads.iter().map(|l| l.p).sum();
        // Real shunt draw at the stored voltages: gs() is MW at V = 1.
        let shunt_p: f64 = (0..view.n())
            .map(|i| view.gs()[i] * net.buses[i].vm.powi(2))
            .sum();
        assert!(
            (loss - (gen_p - load_p - shunt_p)).abs() < 0.5,
            "power balance off: loss {loss} vs gen-load-shunt {}",
            gen_p - load_p - shunt_p
        );
    }

    #[test]
    fn zero_impedance_branch_zeros_columns_and_is_counted() {
        // No vendored fixture has r = x = 0, so build one: branch 0 is a zero-
        // impedance tie, branch 1 is normal. The tie's admittance and flow columns
        // must be zero (never NaN), and the manifest must count it.
        let net = Network::in_memory(
            "zeroimp",
            100.0,
            vec![
                bus(1, BusType::Ref),
                bus(2, BusType::Pq),
                bus(3, BusType::Pq),
            ],
            vec![branch(1, 2, 0.0, 0.0), branch(2, 3, 0.01, 0.1)],
        );
        let tables = gridfm_record_batches(&net, 0, &GridfmOptions::default()).unwrap();
        let br = &tables.branch;
        for col in [
            "Yff_r", "Yff_i", "Yft_r", "Yft_i", "Ytf_r", "Ytf_i", "Ytt_r", "Ytt_i", "pf", "qf",
            "pt", "qt",
        ] {
            let v = col_f64(br, col).value(0);
            assert!(
                v == 0.0,
                "{col} should be 0 for the zero-impedance branch, got {v}"
            );
        }

        let dir = tempfile::tempdir().unwrap();
        let out = write_gridfm_dataset(&net, 0, dir.path(), &GridfmOptions::default()).unwrap();
        assert_eq!(out.dropped_zero_impedance, 1);
        let meta: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(out.dir.join("gridfm_meta.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(meta["dropped_zero_impedance"], 1);
    }

    #[test]
    fn gridfm_cost_maps_every_arm_to_raw_coefficients() {
        // Polynomial coeffs are highest-order first: [c2, c1, c0] -> (cp0, cp1, cp2).
        assert_eq!(
            gridfm_cost(Some(&gencost(2, 3, vec![2.0, 3.0, 4.0]))),
            (4.0, 3.0, 2.0)
        );
        assert_eq!(
            gridfm_cost(Some(&gencost(2, 2, vec![3.0, 4.0]))),
            (4.0, 3.0, 0.0)
        );
        assert_eq!(
            gridfm_cost(Some(&gencost(2, 1, vec![4.0]))),
            (4.0, 0.0, 0.0)
        );
        // Unrepresentable rows collapse to zeros and report as not representable.
        let piecewise = gencost(1, 2, vec![0.0, 0.0, 1.0, 1.0]);
        let malformed = gencost(2, 3, vec![1.0]); // fewer coeffs than ncost claims
        assert_eq!(gridfm_cost(Some(&piecewise)), (0.0, 0.0, 0.0));
        assert_eq!(gridfm_cost(Some(&malformed)), (0.0, 0.0, 0.0));
        assert_eq!(gridfm_cost(None), (0.0, 0.0, 0.0));
        assert!(!cost_representable(Some(&piecewise)));
        assert!(!cost_representable(Some(&malformed)));
        assert!(!cost_representable(None));
        assert!(cost_representable(Some(&gencost(
            2,
            3,
            vec![1.0, 2.0, 3.0]
        ))));
    }

    #[test]
    fn missing_reference_bus_errors() {
        // gridfm_record_batches' documented precondition: exactly one ref bus.
        let net = Network::in_memory(
            "noref",
            100.0,
            vec![bus(1, BusType::Pq), bus(2, BusType::Pq)],
            vec![branch(1, 2, 0.01, 0.1)],
        );
        let err = gridfm_record_batches(&net, 0, &GridfmOptions::default()).unwrap_err();
        assert!(
            matches!(err, Error::ReferenceBusCount { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn non_finite_bus_voltage_errors_before_parquet() {
        let mut net = case14();
        net.buses[0].vm = f64::NAN;
        let err = gridfm_record_batches(&net, 7, &GridfmOptions::default()).unwrap_err();
        match err {
            Error::NonFiniteGridfmValue {
                scenario,
                element,
                row,
                field,
                value,
            } => {
                assert_eq!(scenario, 7);
                assert_eq!(element, "bus");
                assert_eq!(row, 0);
                assert_eq!(field, "vm");
                assert!(value.is_nan());
            }
            other => panic!("expected NonFiniteGridfmValue, got {other:?}"),
        }
    }

    #[test]
    fn non_finite_tap_errors_even_without_y_bus_table() {
        let mut net = case14();
        net.branches[0].tap = f64::NAN;
        let opts = GridfmOptions {
            include_y_bus: false,
            ..Default::default()
        };
        let err = gridfm_record_batches(&net, 0, &opts).unwrap_err();
        assert!(
            matches!(
                err,
                Error::NonFiniteGridfmValue {
                    element: "branch",
                    row: 0,
                    field: "tap",
                    ..
                }
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn normalized_snapshot_is_rejected_in_release_builds() {
        let net = case14().to_normalized().unwrap();
        let err = gridfm_record_batches(&net, 3, &GridfmOptions::default()).unwrap_err();
        assert!(
            matches!(err, Error::NormalizedGridfmSnapshot { scenario: 3 }),
            "got {err:?}"
        );
    }

    #[test]
    fn non_finite_representable_cost_errors() {
        let mut net = Network::in_memory(
            "badcost",
            100.0,
            vec![bus(1, BusType::Ref), bus(2, BusType::Pq)],
            vec![branch(1, 2, 0.01, 0.1)],
        );
        net.generators
            .push(gen_at(1, gencost(2, 3, vec![f64::NAN, 1.0, 0.0])));

        let err = gridfm_record_batches(&net, 0, &GridfmOptions::default()).unwrap_err();
        assert!(
            matches!(
                err,
                Error::NonFiniteGridfmValue {
                    element: "gencost",
                    row: 0,
                    field: "coeffs[0]",
                    ..
                }
            ),
            "got {err:?}"
        );
    }

    /// case14 with every load and generator setpoint scaled — a perturbed
    /// operating point on the same topology, the scenario-batch contract.
    fn scaled(net: &Network, factor: f64) -> Network {
        let mut s = net.clone();
        for l in &mut s.loads {
            l.p *= factor;
            l.q *= factor;
        }
        for g in &mut s.generators {
            g.pg *= factor;
            g.qg *= factor;
        }
        s
    }

    #[test]
    fn batch_stacks_scenarios_keyed_by_scenario_column() {
        let base = case14();
        let up = scaled(&base, 1.1);
        let down = scaled(&base, 0.9);
        let snaps = [
            GridfmSnapshot {
                net: &base,
                scenario: 0,
            },
            GridfmSnapshot {
                net: &up,
                scenario: 1,
            },
            GridfmSnapshot {
                net: &down,
                scenario: 2,
            },
        ];
        let tables = gridfm_record_batches_batch(&snaps, &GridfmOptions::default()).unwrap();

        // Schema is unchanged; rows are 3× the single-snapshot counts.
        assert_eq!(names(&tables.bus), BUS_COLS);
        assert_eq!(names(&tables.branch), BRANCH_COLS);
        assert_eq!(tables.bus.num_rows(), 3 * base.buses.len());
        assert_eq!(tables.generator.num_rows(), 3 * base.generators.len());
        assert_eq!(tables.branch.num_rows(), 3 * base.branches.len());

        // The scenario column is blocked 0.., and the dense bus index resets to
        // 0..n within each scenario.
        let n = base.buses.len();
        let scen = col_i64(&tables.bus, "scenario");
        let lsi = col_i64(&tables.bus, "load_scenario_idx");
        let bus_idx = col_i64(&tables.bus, "bus");
        for k in 0..3 {
            for i in 0..n {
                let row = k * n + i;
                assert_eq!(scen.value(row), k as i64);
                assert_eq!(lsi.value(row), k as i64);
                assert_eq!(bus_idx.value(row), i as i64);
            }
        }

        // The first scenario's rows match the standalone single-case tables, so
        // batching is a pure row-stack over the established single-snapshot path.
        // Compare every column bit-exactly (not just one), so a per-column offset
        // or ordering regression in the row-stack can't slip through.
        let single = gridfm_record_batches(&base, 0, &GridfmOptions::default()).unwrap();
        let bit_exact = |b: &RecordBatch, s: &RecordBatch, col: &str, rows: usize| {
            let (bb, ss) = (col_f64(b, col), col_f64(s, col));
            for i in 0..rows {
                assert_eq!(
                    bb.value(i).to_bits(),
                    ss.value(i).to_bits(),
                    "scenario-0 {col}[{i}] differs from the single-case path"
                );
            }
        };
        for col in ["Pd", "Qd", "Pg", "Qg", "Vm", "Va", "GS", "BS"] {
            bit_exact(&tables.bus, &single.bus, col, n);
        }
        bit_exact(
            &tables.generator,
            &single.generator,
            "p_mw",
            base.generators.len(),
        );
        bit_exact(&tables.branch, &single.branch, "pf", base.branches.len());

        // The perturbed scenario's load really differs (guards against stamping
        // the same network three times).
        let pd_batch = col_f64(&tables.bus, "Pd");
        let pd_single = col_f64(&single.bus, "Pd");
        assert!((pd_batch.value(n) - 1.1 * pd_single.value(0)).abs() < 1e-9);
    }

    #[test]
    fn batch_dataset_writes_stacked_parquet_with_scenario_count() {
        let base = case14();
        let up = scaled(&base, 1.25);
        let snaps = [
            GridfmSnapshot {
                net: &base,
                scenario: 0,
            },
            GridfmSnapshot {
                net: &up,
                scenario: 1,
            },
        ];
        let dir = tempfile::tempdir().unwrap();
        let out = write_gridfm_batch(&snaps, dir.path(), &GridfmOptions::default()).unwrap();

        let bus = read(&out.dir.join("bus_data.parquet"));
        assert_eq!(bus.num_rows(), 2 * base.buses.len());
        let scen = col_i64(&bus, "scenario");
        assert_eq!(scen.value(0), 0);
        assert_eq!(scen.value(base.buses.len()), 1);

        let meta: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(out.dir.join("gridfm_meta.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(meta["n_scenarios"], 2);
        assert_eq!(meta["scenario"], 0);
    }

    #[test]
    fn empty_batch_errors() {
        let err = gridfm_record_batches_batch(&[], &GridfmOptions::default()).unwrap_err();
        assert!(matches!(err, Error::EmptyScenarioBatch), "got {err:?}");
    }

    #[test]
    fn shape_mismatch_across_snapshots_errors() {
        let big = case14();
        let small = Network::in_memory(
            "small",
            100.0,
            vec![bus(1, BusType::Ref), bus(2, BusType::Pq)],
            vec![branch(1, 2, 0.01, 0.1)],
        );
        let snaps = [
            GridfmSnapshot {
                net: &big,
                scenario: 0,
            },
            GridfmSnapshot {
                net: &small,
                scenario: 1,
            },
        ];
        let err = gridfm_record_batches_batch(&snaps, &GridfmOptions::default()).unwrap_err();
        assert!(
            matches!(
                err,
                Error::ScenarioShapeMismatch {
                    index: 1,
                    reason: ScenarioMismatch::Counts { .. }
                }
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn bus_order_mismatch_is_reported_distinctly() {
        // Same counts and the same bus-id set, but a different ordering: the dense
        // bus index would mean different buses across snapshots, so the batch is
        // rejected with the BusOrder reason (not the same-tuple Counts message).
        let base = case14();
        let mut reordered = base.clone();
        reordered.buses.swap(0, 1);
        let snaps = [
            GridfmSnapshot {
                net: &base,
                scenario: 0,
            },
            GridfmSnapshot {
                net: &reordered,
                scenario: 1,
            },
        ];
        let err = gridfm_record_batches_batch(&snaps, &GridfmOptions::default()).unwrap_err();
        assert!(
            matches!(
                err,
                Error::ScenarioShapeMismatch {
                    index: 1,
                    reason: ScenarioMismatch::BusOrder
                }
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn manifest_counts_sum_over_the_batch() {
        // Two snapshots on the same element set, but only the second has a zeroed
        // branch impedance. The manifest's dropped count describes the whole
        // dataset (a total of 1), not just the first snapshot — branch status and
        // impedance may legitimately differ per scenario.
        let base = case14();
        let mut perturbed = base.clone();
        perturbed.branches[0].r = 0.0;
        perturbed.branches[0].x = 0.0;
        let snaps = [
            GridfmSnapshot {
                net: &base,
                scenario: 0,
            },
            GridfmSnapshot {
                net: &perturbed,
                scenario: 1,
            },
        ];
        let dir = tempfile::tempdir().unwrap();
        let out = write_gridfm_batch(&snaps, dir.path(), &GridfmOptions::default()).unwrap();
        assert_eq!(out.dropped_zero_impedance, 1);
        let meta: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(out.dir.join("gridfm_meta.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(meta["dropped_zero_impedance"], 1);
    }

    #[test]
    fn y_bus_table_is_absent_when_disabled() {
        let net = case14();
        let opts = GridfmOptions {
            include_y_bus: false,
            ..Default::default()
        };
        let tables = gridfm_record_batches(&net, 0, &opts).unwrap();
        assert!(tables.y_bus.is_none(), "y_bus should not be built");

        let dir = tempfile::tempdir().unwrap();
        let out = write_gridfm_dataset(&net, 0, dir.path(), &opts).unwrap();
        assert!(
            !out.dir.join("y_bus_data.parquet").exists(),
            "y_bus_data.parquet should not be written"
        );
    }

    #[test]
    fn numbered_snapshots_stamps_base_plus_k_and_checks_overflow() {
        // The shared builder both bindings use: the k-th network is scenario
        // `base + k`, in order.
        let net = case14();
        let snaps = numbered_snapshots(&[&net, &net, &net], 5).unwrap();
        assert_eq!(snaps.len(), 3);
        assert_eq!(snaps[0].scenario, 5);
        assert_eq!(snaps[1].scenario, 6);
        assert_eq!(snaps[2].scenario, 7);

        // Overflow is checked (not wrapped to a negative id, not a panic) and names
        // the offending index.
        let err = numbered_snapshots(&[&net, &net], i64::MAX).unwrap_err();
        assert!(
            matches!(err, Error::ScenarioIdOverflow { index: 1, .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn out_of_service_generator_is_listed_but_excluded_from_bus_aggregate() {
        // Two paths react to `g.in_service`: gen_data emits an `in_service` column
        // for every generator (keeping its setpoint), while bus `Pg`/`Qg` aggregate
        // only in-service generation (`view.in_service_gens()`). Exercise the
        // `false` case on both.
        let mut net = Network::in_memory(
            "genoutage",
            100.0,
            vec![bus(1, BusType::Ref), bus(2, BusType::Pq)],
            vec![branch(1, 2, 0.01, 0.1)],
        );
        let mut g_on = gen_at(1, gencost(2, 3, vec![0.0, 1.0, 0.0]));
        g_on.pg = 50.0;
        let mut g_off = gen_at(2, gencost(2, 3, vec![0.0, 1.0, 0.0]));
        g_off.pg = 30.0;
        g_off.in_service = false;
        net.generators.push(g_on);
        net.generators.push(g_off);

        let tables = gridfm_record_batches(&net, 0, &GridfmOptions::default()).unwrap();

        // gen_data lists both gens in source order, flags the out-of-service one,
        // and keeps its setpoint.
        let g = &tables.generator;
        assert_eq!(g.num_rows(), 2);
        let in_service = col_i64(g, "in_service");
        assert_eq!(in_service.value(0), 1, "in-service gen flagged 1");
        assert_eq!(in_service.value(1), 0, "out-of-service gen flagged 0");
        assert!(
            (col_f64(g, "p_mw").value(1) - 30.0).abs() < 1e-12,
            "gen_data keeps the out-of-service setpoint"
        );

        // bus Pg aggregates only in-service generation: bus 1 (dense 0) gets 50,
        // bus 2 (dense 1) excludes the out-of-service gen's 30.
        let pg = col_f64(&tables.bus, "Pg");
        assert!(
            (pg.value(0) - 50.0).abs() < 1e-12,
            "in-service gen folded into bus Pg"
        );
        assert!(
            pg.value(1) == 0.0,
            "out-of-service gen excluded from bus Pg, got {}",
            pg.value(1)
        );
    }

    #[test]
    fn out_of_service_branch_zeros_flows_but_keeps_admittance() {
        // An out-of-service branch keeps its physical Y** admittances but carries
        // zero flows and `br_status = 0` — the path datakit's topology variants
        // exercise. Use non-flat voltages so an *in-service* branch carries real
        // flow, which makes the zero on the tripped branch meaningful (not just an
        // artifact of a flat start).
        let mut net = Network::in_memory(
            "outage",
            100.0,
            vec![
                bus(1, BusType::Ref),
                bus(2, BusType::Pq),
                bus(3, BusType::Pq),
            ],
            vec![branch(1, 2, 0.01, 0.1), branch(2, 3, 0.02, 0.2)],
        );
        net.buses[1].va = -3.0;
        net.buses[2].va = -6.0;
        net.branches[0].in_service = false; // trip branch 0

        let tables = gridfm_record_batches(&net, 0, &GridfmOptions::default()).unwrap();
        let br = &tables.branch;
        let status = col_i64(br, "br_status");
        assert_eq!(status.value(0), 0, "tripped branch reports br_status 0");
        assert_eq!(status.value(1), 1, "in-service branch reports br_status 1");

        for col in ["pf", "qf", "pt", "qt"] {
            let v = col_f64(br, col).value(0);
            assert!(
                v == 0.0,
                "{col} must be zero on the out-of-service branch, got {v}"
            );
        }
        // The in-service branch really carries flow at these voltages — guards
        // against a flat-start false pass that would zero every branch anyway.
        assert!(
            col_f64(br, "pf").value(1).abs() > 1e-6,
            "in-service branch should carry nonzero flow"
        );
        // Admittances are retained for the tripped branch (unlike a zero-impedance
        // branch, which zeroes them).
        assert!(
            col_f64(br, "Yff_i").value(0).abs() > 0.0,
            "out-of-service branch keeps its physical Y** admittances"
        );
    }

    fn bus(id: usize, kind: BusType) -> Bus {
        Bus {
            id: BusId(id),
            kind,
            vm: 1.0,
            va: 0.0,
            base_kv: 1.0,
            vmax: 1.1,
            vmin: 0.9,
            area: 1,
            zone: 1,
            name: None,
            extras: Extras::new(),
        }
    }

    fn branch(from: usize, to: usize, r: f64, x: f64) -> Branch {
        Branch {
            from: BusId(from),
            to: BusId(to),
            r,
            x,
            b: 0.0,
            rate_a: 0.0,
            rate_b: 0.0,
            rate_c: 0.0,
            tap: 0.0,
            shift: 0.0,
            in_service: true,
            angmin: -360.0,
            angmax: 360.0,
            extras: Extras::new(),
        }
    }

    fn gencost(model: u8, ncost: usize, coeffs: Vec<f64>) -> GenCost {
        GenCost {
            model,
            startup: 0.0,
            shutdown: 0.0,
            ncost,
            coeffs,
        }
    }

    fn gen_at(bus: usize, cost: GenCost) -> Generator {
        Generator {
            bus: BusId(bus),
            pg: 0.0,
            qg: 0.0,
            pmax: 100.0,
            pmin: 0.0,
            qmax: 50.0,
            qmin: -50.0,
            vg: 1.0,
            mbase: 100.0,
            in_service: true,
            cost: Some(cost),
            caps: [None; 11],
        }
    }

    #[test]
    fn degenerate_cost_gen_zeros_columns_and_is_counted() {
        // Counterpart to the zero-impedance test: a piecewise (model 1) cost row
        // gets zeroed cp* columns and is counted; a polynomial row is kept.
        let mut net = Network::in_memory(
            "degen",
            100.0,
            vec![bus(1, BusType::Ref), bus(2, BusType::Pq)],
            vec![branch(1, 2, 0.01, 0.1)],
        );
        net.generators
            .push(gen_at(1, gencost(1, 2, vec![0.0, 0.0, 1.0, 1.0]))); // piecewise
        net.generators
            .push(gen_at(2, gencost(2, 3, vec![0.01, 5.0, 0.0]))); // polynomial

        let tables = gridfm_record_batches(&net, 0, &GridfmOptions::default()).unwrap();
        let g = &tables.generator;
        let (cp0, cp1, cp2) = (
            col_f64(g, "cp0_eur"),
            col_f64(g, "cp1_eur_per_mw"),
            col_f64(g, "cp2_eur_per_mw2"),
        );
        assert_eq!((cp0.value(0), cp1.value(0), cp2.value(0)), (0.0, 0.0, 0.0));
        assert_eq!((cp0.value(1), cp1.value(1), cp2.value(1)), (0.0, 5.0, 0.01));

        let dir = tempfile::tempdir().unwrap();
        let out = write_gridfm_dataset(&net, 0, dir.path(), &GridfmOptions::default()).unwrap();
        assert_eq!(out.degenerate_cost_gens, 1);
        let meta: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(out.dir.join("gridfm_meta.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(meta["degenerate_cost_gens"], 1);
    }

    #[test]
    fn scenario_id_and_tap_toggle_take_effect() {
        let net = case14();

        // The scenario id (an explicit argument now) reaches both id columns.
        let bus = gridfm_record_batches(&net, 7, &GridfmOptions::default())
            .unwrap()
            .bus;
        assert_eq!(col_i64(&bus, "scenario").value(0), 7);
        assert_eq!(col_i64(&bus, "load_scenario_idx").value(0), 7);

        // Turning taps off changes a transformer's admittance columns.
        let on = gridfm_record_batches(&net, 0, &GridfmOptions::default())
            .unwrap()
            .branch;
        let off = gridfm_record_batches(
            &net,
            0,
            &GridfmOptions {
                include_taps: false,
                ..Default::default()
            },
        )
        .unwrap()
        .branch;
        let tap = col_f64(&on, "tap");
        let xfmr = (0..on.num_rows())
            .find(|&i| (tap.value(i) - 1.0).abs() > 1e-9)
            .expect("case14 has off-nominal transformers");
        // case14 transformers are lossless (r = 0), so compare the susceptance
        // (imaginary) part, which scales with 1/tap².
        assert!(
            (col_f64(&on, "Yff_i").value(xfmr) - col_f64(&off, "Yff_i").value(xfmr)).abs() > 1e-9,
            "taps off should change the transformer's Yff"
        );
    }
}
