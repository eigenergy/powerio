//! GridFM interchange: a parsed case as the gridfm-datakit Parquet schema.
//!
//! [`gridfm-datakit`](https://github.com/gridfm) writes per-scenario Parquet
//! tables that [`gridfm-graphkit`](https://github.com/gridfm)'s
//! `HeteroGridDatasetDisk` trains a GNN on. This module emits the same four
//! tables — `bus_data`, `gen_data`, `branch_data`, `y_bus_data` — from one
//! parsed [`Network`], so graphkit can train on powerio output directly and the
//! scenario-batch path (issue #14) has its on-disk format.
//!
//! # The snapshot contract
//!
//! powerio has no power flow solver. One parsed case is one snapshot
//! (`scenario = 0`): voltages and generator dispatch are the case's stored
//! values, and branch flows `pf/qf/pt/qt` are computed from those voltages and
//! the branch admittances ([`branch_flows`]). For a solved MATPOWER case the
//! stored voltages are the converged operating point, so the flows match what a
//! solver would report to float tolerance; for an unsolved/flat start case they
//! are the flows at the stored voltages, not a re-solved dispatch.
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
use crate::network::{Branch, Bus, BusType};
use crate::{Error, GenCost, Network, Result};

/// Options for the gridfm export.
#[derive(Debug, Clone)]
pub struct GridfmOptions {
    /// Scenario id stamped into the `scenario` and `load_scenario_idx` columns.
    /// A parsed case is one snapshot, so the default is `0`.
    pub scenario: i64,
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
            scenario: 0,
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

/// The gridfm-datakit tables as Arrow record batches. The Parquet writer builds
/// from these; a deferred gridfm-schema Arrow C Data Interface export (issue #38)
/// would reuse them. (The raw network Arrow export that ships in powerio-capi is
/// a different, lighter schema.)
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct GridfmTables {
    pub bus: RecordBatch,
    pub generator: RecordBatch,
    pub branch: RecordBatch,
    pub y_bus: RecordBatch,
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
    scenario: i64,
    schema: &'static str,
    n_buses: usize,
    n_branches: usize,
    n_branches_in_service: usize,
    n_gens: usize,
    reference_bus: usize,
    /// Branches with `r² + x² = 0`: their admittance/flow columns are zeroed.
    dropped_zero_impedance: usize,
    /// Generators whose cost row gridfm can't represent (piecewise, missing,
    /// malformed, or cubic and higher): their `cp*` columns are zeroed.
    degenerate_cost_gens: usize,
    files: Vec<String>,
    powerio_version: String,
}

/// Build the four gridfm tables for one network. Pure (no I/O).
///
/// # Errors
/// [`Error::ReferenceBusCount`] unless the case has exactly one reference bus
/// (graphkit needs a slack), [`Error::NonFiniteSusceptance`] for a branch with
/// NaN/Inf impedance, and [`Error::UnknownBus`] if a generator or branch
/// references a bus the network doesn't define.
pub fn gridfm_record_batches(net: &Network, opts: &GridfmOptions) -> Result<GridfmTables> {
    let view = IndexedNetwork::new(net);
    let ref_bus = view.reference_bus_index()?;
    tables_from_view(&view, ref_bus, opts)
}

/// The four tables from an already-built view and resolved reference bus, so the
/// writer doesn't re-index the network just to fill the manifest.
fn tables_from_view(
    view: &IndexedNetwork,
    ref_bus: usize,
    opts: &GridfmOptions,
) -> Result<GridfmTables> {
    Ok(GridfmTables {
        bus: bus_batch(view, opts.scenario)?,
        generator: gen_batch(view, opts.scenario, ref_bus)?,
        branch: branch_batch(view, opts)?,
        y_bus: y_bus_batch(view, opts)?,
    })
}

/// Write the gridfm-datakit Parquet dataset for one case under
/// `out_dir/<network_name>/raw/`, matching datakit's directory layout. Writes
/// `bus_data.parquet`, `gen_data.parquet`, `branch_data.parquet`, optionally
/// `y_bus_data.parquet`, and a `gridfm_meta.json` manifest.
///
/// # Errors
/// Propagates [`gridfm_record_batches`] and any filesystem/Parquet error.
pub fn write_gridfm_dataset(
    net: &Network,
    out_dir: impl AsRef<Path>,
    opts: &GridfmOptions,
) -> Result<GridfmOutputs> {
    let view = IndexedNetwork::new(net);
    let ref_bus = view.reference_bus_index()?;
    let tables = tables_from_view(&view, ref_bus, opts)?;

    let dir = out_dir.as_ref().join(&net.name).join("raw");
    std::fs::create_dir_all(&dir)?;

    let mut files = Vec::new();
    put_parquet(&dir, "bus_data.parquet", &tables.bus, &mut files)?;
    put_parquet(&dir, "gen_data.parquet", &tables.generator, &mut files)?;
    put_parquet(&dir, "branch_data.parquet", &tables.branch, &mut files)?;
    if opts.include_y_bus {
        put_parquet(&dir, "y_bus_data.parquet", &tables.y_bus, &mut files)?;
    }

    let dropped_zero_impedance = net
        .branches
        .iter()
        .filter(|br| br.r * br.r + br.x * br.x == 0.0)
        .count();
    let degenerate_cost_gens = net
        .generators
        .iter()
        .filter(|g| !cost_representable(g.cost.as_ref()))
        .count();

    let meta = GridfmMeta {
        case_name: net.name.clone(),
        base_mva: net.base_mva,
        scenario: opts.scenario,
        schema: "gridfm-datakit",
        n_buses: net.buses.len(),
        n_branches: net.branches.len(),
        n_branches_in_service: net.branches.iter().filter(|b| b.in_service).count(),
        n_gens: net.generators.len(),
        reference_bus: ref_bus,
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

fn bus_batch(view: &IndexedNetwork, scenario: i64) -> Result<RecordBatch> {
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

    let bus = i64_range(n);
    batch(vec![
        ("scenario", const_i64(scenario, n)),
        ("load_scenario_idx", const_i64(scenario, n)),
        ("bus", bus),
        ("Pd", f64s(view.pd().to_vec())),
        ("Qd", f64s(view.qd().to_vec())),
        ("Pg", f64s(pg)),
        ("Qg", f64s(qg)),
        ("Vm", f64s(buses.iter().map(|b| b.vm).collect())),
        ("Va", f64s(buses.iter().map(|b| b.va).collect())),
        ("PQ", one_hot(buses, BusType::Pq)),
        ("PV", one_hot(buses, BusType::Pv)),
        ("REF", one_hot(buses, BusType::Ref)),
        ("vn_kv", f64s(buses.iter().map(|b| b.base_kv).collect())),
        ("min_vm_pu", f64s(buses.iter().map(|b| b.vmin).collect())),
        ("max_vm_pu", f64s(buses.iter().map(|b| b.vmax).collect())),
        ("GS", f64s(view.gs().iter().map(|g| g / base).collect())),
        ("BS", f64s(view.bs().iter().map(|b| b / base).collect())),
    ])
}

fn gen_batch(view: &IndexedNetwork, scenario: i64, ref_bus: usize) -> Result<RecordBatch> {
    let gens = view.generators();
    let m = gens.len();

    let mut bus = Vec::with_capacity(m);
    let mut is_slack = Vec::with_capacity(m);
    let (mut cp0, mut cp1, mut cp2) = (
        Vec::with_capacity(m),
        Vec::with_capacity(m),
        Vec::with_capacity(m),
    );
    for (row, g) in gens.iter().enumerate() {
        let i = view.bus_index(g.bus).ok_or(Error::UnknownBus {
            bus_id: g.bus,
            element_index: row,
        })?;
        bus.push(i as i64);
        is_slack.push(i64::from(i == ref_bus));
        let (c0, c1, c2) = gridfm_cost(g.cost.as_ref());
        cp0.push(c0);
        cp1.push(c1);
        cp2.push(c2);
    }

    batch(vec![
        ("scenario", const_i64(scenario, m)),
        ("load_scenario_idx", const_i64(scenario, m)),
        ("idx", i64_range(m)),
        ("bus", i64s(bus)),
        ("p_mw", f64s(gens.iter().map(|g| g.pg).collect())),
        ("q_mvar", f64s(gens.iter().map(|g| g.qg).collect())),
        ("min_p_mw", f64s(gens.iter().map(|g| g.pmin).collect())),
        ("max_p_mw", f64s(gens.iter().map(|g| g.pmax).collect())),
        ("min_q_mvar", f64s(gens.iter().map(|g| g.qmin).collect())),
        ("max_q_mvar", f64s(gens.iter().map(|g| g.qmax).collect())),
        ("cp0_eur", f64s(cp0)),
        ("cp1_eur_per_mw", f64s(cp1)),
        ("cp2_eur_per_mw2", f64s(cp2)),
        (
            "in_service",
            i64s(gens.iter().map(|g| i64::from(g.in_service)).collect()),
        ),
        ("is_slack_gen", i64s(is_slack)),
    ])
}

#[allow(clippy::too_many_lines, clippy::many_single_char_names)]
fn branch_batch(view: &IndexedNetwork, opts: &GridfmOptions) -> Result<RecordBatch> {
    let base = view.base_mva();
    let branches = view.branches();
    let m = branches.len();
    let buses = &view.network().buses;

    // Same flags the Y_bus builder derives, so the branch admittance columns and
    // y_bus_data come from one kernel.
    let flags = YbusFlags {
        unity_taps: !opts.include_taps,
        zero_shifts: !opts.include_shifts,
        ..Default::default()
    };
    // Complex bus voltages `vm·e^{jθ}`, dense order, for the flow evaluation.
    let v: Vec<Complex64> = buses
        .iter()
        .map(|b| Complex64::from_polar(b.vm, b.va.to_radians()))
        .collect();

    let mut from_bus = Vec::with_capacity(m);
    let mut to_bus = Vec::with_capacity(m);
    let (mut pf, mut qf, mut pt, mut qt) = (
        Vec::with_capacity(m),
        Vec::with_capacity(m),
        Vec::with_capacity(m),
        Vec::with_capacity(m),
    );
    let (mut yff_r, mut yff_i) = (Vec::with_capacity(m), Vec::with_capacity(m));
    let (mut yft_r, mut yft_i) = (Vec::with_capacity(m), Vec::with_capacity(m));
    let (mut ytf_r, mut ytf_i) = (Vec::with_capacity(m), Vec::with_capacity(m));
    let (mut ytt_r, mut ytt_i) = (Vec::with_capacity(m), Vec::with_capacity(m));

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
        let block = branch_admittance(br, flags, row)?;
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
    }

    batch(vec![
        ("scenario", const_i64(opts.scenario, m)),
        ("load_scenario_idx", const_i64(opts.scenario, m)),
        ("idx", i64_range(m)),
        ("from_bus", i64s(from_bus)),
        ("to_bus", i64s(to_bus)),
        ("pf", f64s(pf)),
        ("qf", f64s(qf)),
        ("pt", f64s(pt)),
        ("qt", f64s(qt)),
        ("r", f64s(branches.iter().map(|b| b.r).collect())),
        ("x", f64s(branches.iter().map(|b| b.x).collect())),
        ("b", f64s(branches.iter().map(|b| b.b).collect())),
        ("Yff_r", f64s(yff_r)),
        ("Yff_i", f64s(yff_i)),
        ("Yft_r", f64s(yft_r)),
        ("Yft_i", f64s(yft_i)),
        ("Ytf_r", f64s(ytf_r)),
        ("Ytf_i", f64s(ytf_i)),
        ("Ytt_r", f64s(ytt_r)),
        ("Ytt_i", f64s(ytt_i)),
        (
            "tap",
            f64s(branches.iter().map(Branch::effective_tap).collect()),
        ),
        ("shift", f64s(branches.iter().map(|b| b.shift).collect())),
        ("ang_min", f64s(branches.iter().map(|b| b.angmin).collect())),
        ("ang_max", f64s(branches.iter().map(|b| b.angmax).collect())),
        ("rate_a", f64s(branches.iter().map(|b| b.rate_a).collect())),
        (
            "br_status",
            i64s(branches.iter().map(|b| i64::from(b.in_service)).collect()),
        ),
    ])
}

fn y_bus_batch(view: &IndexedNetwork, opts: &GridfmOptions) -> Result<RecordBatch> {
    let parts = build_ybus(view, &opts.build_options())?;
    // G and B don't share a sparsity pattern: a lossless branch (r = 0) is a pure
    // reactance, so its G entries are structurally zero where B's aren't. datakit
    // keys y_bus rows on the complex value being nonzero, i.e. the union of the G
    // and B positions. Merge into a sorted (row, col) map so the output is
    // row-major like `np.nonzero`, then drop any all-zero position.
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

    let mut index1 = Vec::with_capacity(entries.len());
    let mut index2 = Vec::with_capacity(entries.len());
    let mut g_vals = Vec::with_capacity(entries.len());
    let mut b_vals = Vec::with_capacity(entries.len());
    for ((row, col), (gv, bv)) in entries {
        if gv == 0.0 && bv == 0.0 {
            continue;
        }
        index1.push(row as i64);
        index2.push(col as i64);
        g_vals.push(gv);
        b_vals.push(bv);
    }
    let len = index1.len();
    batch(vec![
        ("scenario", const_i64(opts.scenario, len)),
        ("load_scenario_idx", const_i64(opts.scenario, len)),
        ("index1", i64s(index1)),
        ("index2", i64s(index2)),
        ("G", f64s(g_vals)),
        ("B", f64s(b_vals)),
    ])
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

fn const_i64(value: i64, len: usize) -> ArrayRef {
    Arc::new(Int64Array::from(vec![value; len]))
}

fn i64_range(len: usize) -> ArrayRef {
    Arc::new(Int64Array::from((0..len as i64).collect::<Vec<_>>()))
}

fn i64s(v: Vec<i64>) -> ArrayRef {
    Arc::new(Int64Array::from(v))
}

fn f64s(v: Vec<f64>) -> ArrayRef {
    Arc::new(Float64Array::from(v))
}

fn one_hot(buses: &[Bus], kind: BusType) -> ArrayRef {
    i64s(buses.iter().map(|b| i64::from(b.kind == kind)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::{BusId, Extras, Generator};
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
        let tables = gridfm_record_batches(&net, &GridfmOptions::default()).unwrap();

        assert_eq!(names(&tables.bus), BUS_COLS);
        assert_eq!(names(&tables.generator), GEN_COLS);
        assert_eq!(names(&tables.branch), BRANCH_COLS);
        assert_eq!(names(&tables.y_bus), YBUS_COLS);

        assert_eq!(tables.bus.num_rows(), net.buses.len()); // 14
        assert_eq!(tables.generator.num_rows(), net.generators.len()); // 5
        assert_eq!(tables.branch.num_rows(), net.branches.len()); // 20
    }

    #[test]
    fn parquet_round_trips_through_reader() {
        let net = case14();
        let dir = tempfile::tempdir().unwrap();
        let out = write_gridfm_dataset(&net, dir.path(), &GridfmOptions::default()).unwrap();

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
        let tables = gridfm_record_batches(&net, &GridfmOptions::default()).unwrap();
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
        let tables = gridfm_record_batches(&net, &GridfmOptions::default()).unwrap();
        let br = &tables.branch;

        let yff_r = col_f64(br, "Yff_r");
        let yff_i = col_f64(br, "Yff_i");
        for (row, branch) in net.branches.iter().enumerate() {
            if let Some(block) = branch_admittance(branch, YbusFlags::default(), row).unwrap() {
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
        let tables = gridfm_record_batches(&net, &GridfmOptions::default()).unwrap();
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
        let tables = gridfm_record_batches(&net, &GridfmOptions::default()).unwrap();
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
        let tables = gridfm_record_batches(&net, &GridfmOptions::default()).unwrap();
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
        let out = write_gridfm_dataset(&net, dir.path(), &GridfmOptions::default()).unwrap();
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
        let err = gridfm_record_batches(&net, &GridfmOptions::default()).unwrap_err();
        assert!(
            matches!(err, Error::ReferenceBusCount { .. }),
            "got {err:?}"
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

        let tables = gridfm_record_batches(&net, &GridfmOptions::default()).unwrap();
        let g = &tables.generator;
        let (cp0, cp1, cp2) = (
            col_f64(g, "cp0_eur"),
            col_f64(g, "cp1_eur_per_mw"),
            col_f64(g, "cp2_eur_per_mw2"),
        );
        assert_eq!((cp0.value(0), cp1.value(0), cp2.value(0)), (0.0, 0.0, 0.0));
        assert_eq!((cp0.value(1), cp1.value(1), cp2.value(1)), (0.0, 5.0, 0.01));

        let dir = tempfile::tempdir().unwrap();
        let out = write_gridfm_dataset(&net, dir.path(), &GridfmOptions::default()).unwrap();
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

        // The scenario id reaches both id columns.
        let opts = GridfmOptions {
            scenario: 7,
            ..Default::default()
        };
        let bus = gridfm_record_batches(&net, &opts).unwrap().bus;
        assert_eq!(col_i64(&bus, "scenario").value(0), 7);
        assert_eq!(col_i64(&bus, "load_scenario_idx").value(0), 7);

        // Turning taps off changes a transformer's admittance columns.
        let on = gridfm_record_batches(&net, &GridfmOptions::default())
            .unwrap()
            .branch;
        let off = gridfm_record_batches(
            &net,
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
