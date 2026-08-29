//! The gridfm-datakit Parquet dataset reader: rebuild [`BalancedNetwork`]
//! values, and every scenario as one shared identity
//! [`ScenarioSet`](powerio_core::ScenarioSet), from the four table schema
//! `gridfm-datakit` writes (`bus_data`, `gen_data`, `branch_data`,
//! `y_bus_data`). `y_bus_data` is ignored on read; branches carry raw
//! `r/x/b`. The write side, which derives `y_bus_data` and the branch flows,
//! lives in `powerio-matrix` behind its `gridfm` feature.
//!
//! The read is lossy but power flow complete; each rebuilt network reports
//! what the gridfm schema could not round trip (synthesized bus ids, folded
//! per bus load and shunt, relabeled unity ratio transformers) as structured
//! diagnostics. Units follow datakit: `Pd, Qd, Pg, Qg` MW/MVAr, `Vm` per
//! unit, `Va` degrees, `r, x, b` per unit, `GS, BS` divided by `base_mva`.
//! `bus`, `from_bus`, `to_bus` are dense `[0, n)` indices.

use std::path::Path;

use arrow::array::{Array, ArrayRef, Float64Array, Int64Array};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

use powerio_tx::network::{Branch, Bus, BusId, BusType, Generator, Load, Shunt, SourceFormat};
use powerio_tx::{BalancedNetwork, GenCost};

use crate::collect::Diagnostics;

type Error = powerio_tx::Error;
type Result<T> = std::result::Result<T, Error>;

/// The reader's fidelity notes: what the gridfm schema cannot round trip.
pub mod codes {
    powerio_core::diagnostic_codes! {
        READ_GRIDFM_FIELD_DROPPED = "READ.GRIDFM.FIELD_DROPPED", Warning,
            "a field the gridfm schema does not carry is absent from the network";
        READ_GRIDFM_VALUE_DEFAULTED = "READ.GRIDFM.VALUE_DEFAULTED", Warning,
            "a manifest value the reader needs was absent and was defaulted";
        READ_GRIDFM_VALUE_INFERRED = "READ.GRIDFM.VALUE_INFERRED", Warning,
            "an identity the gridfm schema does not store was synthesized";
        READ_GRIDFM_VALUE_COLLAPSED = "READ.GRIDFM.VALUE_COLLAPSED", Warning,
            "nodal totals were folded into synthetic per bus elements";
        READ_GRIDFM_ELEMENT_RELABELED = "READ.GRIDFM.ELEMENT_RELABELED", Warning,
            "a unity ratio transformer is indistinguishable from a line and reads as one";
        /// Retired in 0.9.0: every gridfm read finding now carries its own
        /// code, so the package no longer wraps them under one catch-all.
        READ_GRIDFM_FIDELITY_WARNING = "READ.GRIDFM.FIDELITY_WARNING", Warning,
            "a gridfm read finding with no identity of its own", retired = "0.9.0";
    }
}
/// One rebuilt scenario of a gridfm dataset.
#[derive(Debug, Clone)]
pub struct GridfmRead {
    /// The reconstructed network (`source_format = SourceFormat::Gridfm`).
    pub network: BalancedNetwork,
    /// The scenario id these rows came from.
    pub scenario: i64,
    /// What the gridfm schema couldn't round-trip — synthesized bus ids, folded
    /// per-bus load/shunt, dropped HVDC/storage, etc., as structured records.
    pub diagnostics: Vec<powerio_core::Diagnostic>,
    /// The same findings as `CODE: message` lines, rendered from
    /// `diagnostics`.
    pub warnings: Vec<String>,
}

/// Build one [`BalancedNetwork`] from in-memory gridfm tables, selecting
/// `scenario`'s rows. The pure inverse of the writer's single snapshot
/// batches: `base_mva` and `name` come from the caller (the disk path reads
/// them from `gridfm_meta.json`).
///
/// # Errors
/// [`powerio_tx::Error::FormatRead`] if a required column is missing or mistyped, a column
/// carries nulls, a dense index is negative, or `scenario` isn't present; plus
/// whatever [`BalancedNetwork::validate`] rejects (duplicate / dangling bus ids).
pub fn read_gridfm_network(
    bus_table: &RecordBatch,
    generator_table: &RecordBatch,
    branch_table: &RecordBatch,
    scenario: i64,
    base_mva: f64,
    name: &str,
) -> Result<GridfmRead> {
    let bus = bus_columns(std::slice::from_ref(bus_table))?;
    let gens = gen_columns(std::slice::from_ref(generator_table))?;
    let branch = branch_columns(std::slice::from_ref(branch_table))?;
    build_network_from_columns(
        &bus,
        &gens,
        &branch,
        scenario,
        base_mva,
        name,
        Diagnostics::new(),
    )
}

/// Open one dataset directory through the pinned acquisition and extract
/// every table the reader uses: the same entry walk and buffer reads
/// [`parse_gridfm_source`] performs, so the path entry points share its
/// symbolic link and escape refusals.
struct DatasetTables {
    base_mva: f64,
    name: String,
    meta_warnings: Diagnostics,
    bus: BusColumns,
    gens: GenColumns,
    branch: BranchColumns,
}

fn open_dataset_tables(dir: &Path) -> Result<DatasetTables> {
    let source =
        powerio_core::Source::open(dir).map_err(|error| powerio_tx::Error::FormatRead {
            format: "gridfm",
            message: format!("opening {}: {error}", dir.display()),
        })?;
    let entries = source
        .entry_names()
        .map_err(|error| powerio_tx::Error::FormatRead {
            format: "gridfm",
            message: format!("listing {}: {error}", dir.display()),
        })?;
    let prefix = resolve_raw_prefix(&entries)?;
    let (base_mva, name, meta_warnings) = read_meta_source(&source, &prefix);
    let bus = bus_columns(&read_parquet_buffer(&source, &prefix, "bus_data.parquet")?)?;
    let gens = gen_columns(&read_parquet_buffer(&source, &prefix, "gen_data.parquet")?)?;
    let branch = branch_columns(&read_parquet_buffer(
        &source,
        &prefix,
        "branch_data.parquet",
    )?)?;
    Ok(DatasetTables {
        base_mva,
        name,
        meta_warnings,
        bus,
        gens,
        branch,
    })
}

/// Read one `scenario` from a gridfm dataset on disk and rebuild a [`BalancedNetwork`].
/// The inverse of `powerio_matrix::write_gridfm_dataset`.
///
/// `dir` is resolved leniently: the leaf `raw/` directory holding the parquet
/// files, a `<case>/` directory with a `raw/` child, or a parent directory with
/// exactly one `*/raw/` child all work. `base_mva` and the case name come from
/// `gridfm_meta.json` (a missing manifest defaults `base_mva` to 100 and warns).
///
/// # Errors
/// Propagates [`read_gridfm_network`] plus any filesystem / Parquet read error.
pub fn read_gridfm_dataset(dir: impl AsRef<Path>, scenario: i64) -> Result<GridfmRead> {
    let tables = open_dataset_tables(dir.as_ref())?;
    build_network_from_columns(
        &tables.bus,
        &tables.gens,
        &tables.branch,
        scenario,
        tables.base_mva,
        &tables.name,
        tables.meta_warnings,
    )
}

/// Read every scenario from a gridfm dataset, one [`BalancedNetwork`] per `scenario` id
/// (sorted ascending) over the shared topology — the read side of the scenario
/// batch (#57). Each scenario is rebuilt independently, so two scenarios may
/// differ in branch status, bus types, and reference bus.
///
/// # Errors
/// Propagates [`read_gridfm_dataset`]'s filesystem / Parquet / build errors.
pub fn read_gridfm_scenarios(dir: impl AsRef<Path>) -> Result<Vec<GridfmRead>> {
    // Extract every column once and reuse across scenarios; rebuilding each
    // scenario from the raw batches would re-concatenate each table n_scenarios
    // times (O(n_scenarios × table_size)).
    let tables = open_dataset_tables(dir.as_ref())?;
    distinct_sorted(&tables.bus.scenario)
        .into_iter()
        .map(|s| {
            build_network_from_columns(
                &tables.bus,
                &tables.gens,
                &tables.branch,
                s,
                tables.base_mva,
                &tables.name,
                tables.meta_warnings.clone(),
            )
        })
        .collect()
}

/// The distinct scenario ids in a gridfm dataset, ascending — the keys
/// [`read_gridfm_scenarios`] rebuilds a [`BalancedNetwork`] for. Reads only `bus_data`'s
/// scenario column, so it enumerates a dataset's scenarios without rebuilding
/// every network; the C ABI's `pio_scenario_ids` is a thin wrapper over it.
///
/// # Errors
/// Propagates the directory resolution and `bus_data.parquet` read errors.
pub fn gridfm_scenario_ids(dir: impl AsRef<Path>) -> Result<Vec<i64>> {
    let source = powerio_core::Source::open(dir.as_ref()).map_err(|error| {
        powerio_tx::Error::FormatRead {
            format: "gridfm",
            message: format!("opening {}: {error}", dir.as_ref().display()),
        }
    })?;
    let entries = source
        .entry_names()
        .map_err(|error| powerio_tx::Error::FormatRead {
            format: "gridfm",
            message: format!("listing {}: {error}", dir.as_ref().display()),
        })?;
    let prefix = resolve_raw_prefix(&entries)?;
    let bus = bus_columns(&read_parquet_buffer(&source, &prefix, "bus_data.parquet")?)?;
    Ok(distinct_sorted(&bus.scenario))
}

/// The distinct values of `scenario`, ascending.
fn distinct_sorted(scenario: &[i64]) -> Vec<i64> {
    let mut ids = scenario.to_vec();
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// Every scenario of a gridfm dataset as one [`ScenarioSet`] over shared
/// element identities: each scenario's network reuses the first scenario's
/// table allocation wherever the rebuilt table is equal, so unchanged
/// topology and parameters are stored once and only the tables a scenario
/// actually changes are held per scenario. Scenario ids are the dataset's
/// `scenario` values, ascending; the diagnostics are every scenario's read
/// findings in that order.
///
/// The current gridfm profile is raw snapshot data that names no solved
/// calculation, so the set is network data — never a solution set.
///
/// [`ScenarioSet`]: powerio_core::ScenarioSet
///
/// # Errors
/// Propagates [`read_gridfm_scenarios`], plus a scenario identity the set
/// rejects.
pub fn read_gridfm_scenario_set(
    dir: impl AsRef<Path>,
) -> std::result::Result<
    (
        powerio_core::ScenarioSet<BalancedNetwork>,
        Vec<powerio_core::Diagnostic>,
    ),
    powerio_core::Error,
> {
    let reads = read_gridfm_scenarios(dir)
        .map_err(|error| powerio_core::Error::new(error.code(), error.to_string()))?;
    let mut diagnostics = Vec::new();
    let mut scenarios = Vec::with_capacity(reads.len());
    let mut donor: Option<BalancedNetwork> = None;
    for read in reads {
        let mut network = read.network;
        match &donor {
            Some(base) => network.share_equal_tables(base),
            None => donor = Some(network.clone()),
        }
        diagnostics.extend(read.diagnostics);
        let id = powerio_core::ScenarioId::new(read.scenario.to_string())?;
        scenarios.push(powerio_core::Scenario::new(id, None, network));
    }
    let set = powerio_core::ScenarioSet::new(scenarios)?;
    Ok((set, diagnostics))
}

/// The unperturbed base case: [`read_gridfm_dataset`] at `scenario = 0` (datakit's
/// convention). There is no single "shared base" beyond a chosen scenario — bus
/// types, branch status, and reference bus all vary per scenario — so the base
/// case is just scenario 0.
///
/// # Errors
/// Propagates [`read_gridfm_dataset`].
pub fn gridfm_base_case(dir: impl AsRef<Path>) -> Result<GridfmRead> {
    read_gridfm_dataset(dir, 0)
}

/// Every `bus_data` column the reader uses, concatenated across all batches once.
/// Extracting columns up front lets a multi-scenario read reuse them rather than
/// re-concatenating the whole table for each scenario.
struct BusColumns {
    scenario: Vec<i64>,
    bus: Vec<i64>,
    pv: Vec<i64>,
    refc: Vec<i64>,
    vm: Vec<f64>,
    va: Vec<f64>,
    vn_kv: Vec<f64>,
    min_vm: Vec<f64>,
    max_vm: Vec<f64>,
    pd: Vec<f64>,
    qd: Vec<f64>,
    gs: Vec<f64>,
    bs: Vec<f64>,
}

fn bus_columns(batches: &[RecordBatch]) -> Result<BusColumns> {
    Ok(BusColumns {
        scenario: i64_col(batches, "scenario")?,
        bus: i64_col(batches, "bus")?,
        pv: i64_col(batches, "PV")?,
        refc: i64_col(batches, "REF")?,
        vm: f64_col(batches, "Vm")?,
        va: f64_col(batches, "Va")?,
        vn_kv: f64_col(batches, "vn_kv")?,
        min_vm: f64_col(batches, "min_vm_pu")?,
        max_vm: f64_col(batches, "max_vm_pu")?,
        pd: f64_col(batches, "Pd")?,
        qd: f64_col(batches, "Qd")?,
        gs: f64_col(batches, "GS")?,
        bs: f64_col(batches, "BS")?,
    })
}

/// Every `gen_data` column the reader uses (cost is `cp0`/`cp1`/`cp2`).
struct GenColumns {
    scenario: Vec<i64>,
    bus: Vec<i64>,
    p_mw: Vec<f64>,
    q_mvar: Vec<f64>,
    min_p: Vec<f64>,
    max_p: Vec<f64>,
    min_q: Vec<f64>,
    max_q: Vec<f64>,
    cp0: Vec<f64>,
    cp1: Vec<f64>,
    cp2: Vec<f64>,
    in_service: Vec<i64>,
}

fn gen_columns(batches: &[RecordBatch]) -> Result<GenColumns> {
    Ok(GenColumns {
        scenario: i64_col(batches, "scenario")?,
        bus: i64_col(batches, "bus")?,
        p_mw: f64_col(batches, "p_mw")?,
        q_mvar: f64_col(batches, "q_mvar")?,
        min_p: f64_col(batches, "min_p_mw")?,
        max_p: f64_col(batches, "max_p_mw")?,
        min_q: f64_col(batches, "min_q_mvar")?,
        max_q: f64_col(batches, "max_q_mvar")?,
        cp0: f64_col(batches, "cp0_eur")?,
        cp1: f64_col(batches, "cp1_eur_per_mw")?,
        cp2: f64_col(batches, "cp2_eur_per_mw2")?,
        in_service: i64_col(batches, "in_service")?,
    })
}

/// Every `branch_data` column the reader uses (`Y**` / flow columns are ignored).
struct BranchColumns {
    scenario: Vec<i64>,
    from_bus: Vec<i64>,
    to_bus: Vec<i64>,
    r: Vec<f64>,
    x: Vec<f64>,
    b: Vec<f64>,
    tap: Vec<f64>,
    shift: Vec<f64>,
    ang_min: Vec<f64>,
    ang_max: Vec<f64>,
    rate_a: Vec<f64>,
    status: Vec<i64>,
}

fn branch_columns(batches: &[RecordBatch]) -> Result<BranchColumns> {
    Ok(BranchColumns {
        scenario: i64_col(batches, "scenario")?,
        from_bus: i64_col(batches, "from_bus")?,
        to_bus: i64_col(batches, "to_bus")?,
        r: f64_col(batches, "r")?,
        x: f64_col(batches, "x")?,
        b: f64_col(batches, "b")?,
        tap: f64_col(batches, "tap")?,
        shift: f64_col(batches, "shift")?,
        ang_min: f64_col(batches, "ang_min")?,
        ang_max: f64_col(batches, "ang_max")?,
        rate_a: f64_col(batches, "rate_a")?,
        status: i64_col(batches, "br_status")?,
    })
}

/// The shared core: rebuild one scenario's [`BalancedNetwork`] from already-extracted
/// columns. The columns are concatenated once by the caller and reused across
/// scenarios, so a multi-scenario read doesn't re-copy each table per scenario.
/// `warnings` is seeded with any manifest-level notes (e.g. a defaulted
/// `base_mva`) and extended with the per-read fidelity notes.
// tap == 1.0 / != 0.0 reads are exact, not approximate; the builder is one long
// linear pass over the three tables, so the length is inherent.
#[allow(clippy::float_cmp, clippy::too_many_lines)]
fn build_network_from_columns(
    bus: &BusColumns,
    gens: &GenColumns,
    branch: &BranchColumns,
    scenario: i64,
    base_mva: f64,
    name: &str,
    mut warnings: Diagnostics,
) -> Result<GridfmRead> {
    // --- buses, loads, shunts (bus_data) ---
    let bus_rows = scenario_rows(&bus.scenario, scenario);
    if bus_rows.is_empty() {
        let mut avail = bus.scenario.clone();
        avail.sort_unstable();
        avail.dedup();
        return Err(powerio_tx::Error::FormatRead {
            format: "gridfm",
            message: format!("scenario {scenario} not present; available: {avail:?}"),
        });
    }

    let bus_id = &bus.bus;
    let pv = &bus.pv;
    let refc = &bus.refc;
    let vm = &bus.vm;
    let va = &bus.va;
    let vn_kv = &bus.vn_kv;
    let min_vm = &bus.min_vm;
    let max_vm = &bus.max_vm;
    let pd = &bus.pd;
    let qd = &bus.qd;
    let gs = &bus.gs;
    let bs = &bus.bs;

    let mut buses = Vec::with_capacity(bus_rows.len());
    let mut loads = Vec::new();
    let mut shunts = Vec::new();
    // Dense bus index -> voltage magnitude, so a generator recovers its `vg`
    // setpoint from its bus (gridfm has no separate gen voltage column, but a
    // generator's setpoint is its bus's regulated `Vm`).
    let mut bus_vm: std::collections::HashMap<i64, f64> =
        std::collections::HashMap::with_capacity(bus_rows.len());
    for &r in &bus_rows {
        let id = dense_bus_id(bus_id[r])?;
        bus_vm.insert(bus_id[r], vm[r]);
        // REF / PV / PQ one-hot; the writer guarantees exactly one set, but read
        // defensively (REF wins, then PV, else PQ).
        let kind = if refc[r] != 0 {
            BusType::Ref
        } else if pv[r] != 0 {
            BusType::Pv
        } else {
            BusType::Pq
        };
        let mut bus = Bus::new(id, kind, vn_kv[r]);
        bus.vm = vm[r];
        bus.va = va[r];
        bus.vmax = max_vm[r];
        bus.vmin = min_vm[r];
        bus.area = 0;
        bus.zone = 0;
        buses.push(bus);
        if pd[r] != 0.0 || qd[r] != 0.0 {
            loads.push(Load::new(id, pd[r], qd[r]));
        }
        // Undo the writer's `/ base_mva` (powerio-matrix/src/io/gridfm.rs) to recover MW/MVAr at V=1.
        if gs[r] != 0.0 || bs[r] != 0.0 {
            shunts.push(Shunt::new(id, gs[r] * base_mva, bs[r] * base_mva));
        }
    }

    // --- generators (gen_data) ---
    let gen_rows = scenario_rows(&gens.scenario, scenario);
    require_scenario_block(&gens.scenario, scenario, &gen_rows, "gen_data")?;
    let g_bus = &gens.bus;
    let p_mw = &gens.p_mw;
    let q_mvar = &gens.q_mvar;
    let min_p = &gens.min_p;
    let max_p = &gens.max_p;
    let min_q = &gens.min_q;
    let max_q = &gens.max_q;
    let cp0 = &gens.cp0;
    let cp1 = &gens.cp1;
    let cp2 = &gens.cp2;
    let g_in = &gens.in_service;

    let mut generators = Vec::with_capacity(gen_rows.len());
    for &r in &gen_rows {
        // Any nonzero coefficient is a real polynomial cost. An all-zero triple is
        // ambiguous in the schema — a generator with no cost, a genuine zero
        // polynomial cost, or a piecewise/cubic+ cost the writer couldn't represent
        // (all written as `(0, 0, 0)`) — so it reads back as `None` (see warnings).
        let cost = if cp0[r] != 0.0 || cp1[r] != 0.0 || cp2[r] != 0.0 {
            Some(GenCost::new(2, 0.0, 0.0, vec![cp2[r], cp1[r], cp0[r]]))
        } else {
            None
        };
        let mut generator = Generator::new(dense_bus_id(g_bus[r])?);
        generator.pg = p_mw[r];
        generator.qg = q_mvar[r];
        generator.pmax = max_p[r];
        generator.pmin = min_p[r];
        generator.qmax = max_q[r];
        generator.qmin = min_q[r];
        // The schema has no gen vg; recover the setpoint from the bus's Vm
        // (falls back to 1.0 only if the gen references an absent bus, which
        // `validate()` below then rejects).
        generator.vg = bus_vm.get(&g_bus[r]).copied().unwrap_or(1.0);
        generator.mbase = base_mva;
        generator.in_service = g_in[r] != 0;
        generator.cost = cost;
        generators.push(generator);
    }

    // --- branches (branch_data); Y** and pf/qf/pt/qt are ignored ---
    let br_rows = scenario_rows(&branch.scenario, scenario);
    require_scenario_block(&branch.scenario, scenario, &br_rows, "branch_data")?;
    let from_bus = &branch.from_bus;
    let to_bus = &branch.to_bus;
    let r_col = &branch.r;
    let x_col = &branch.x;
    let b_col = &branch.b;
    let tap = &branch.tap;
    let shift = &branch.shift;
    let ang_min = &branch.ang_min;
    let ang_max = &branch.ang_max;
    let rate_a = &branch.rate_a;
    let br_status = &branch.status;

    let mut branches = Vec::with_capacity(br_rows.len());
    // The writer stores the *effective* tap (`Branch::effective_tap`), so a line
    // (raw tap 0) lands as 1.0. Map unit tap + no shift back to the raw `tap == 0`
    // line convention, otherwise every line reads as a transformer
    // (`is_transformer` keys off `tap != 0`) and a read→write to a format that
    // splits lines from transformers (PSS/E, PowerWorld) misclassifies them. A
    // genuine unity-ratio, zero-shift transformer is electrically identical to a
    // line and is (unavoidably) read as one.
    let mut unit_tap_lines = 0usize;
    for &row in &br_rows {
        let shift_v = shift[row];
        let tap_out = if tap[row] == 1.0 && shift_v == 0.0 {
            unit_tap_lines += 1;
            0.0
        } else {
            tap[row]
        };
        let mut branch = Branch::new(
            dense_bus_id(from_bus[row])?,
            dense_bus_id(to_bus[row])?,
            r_col[row],
            x_col[row],
        );
        branch.b = b_col[row];
        branch.rate_a = rate_a[row];
        branch.tap = tap_out;
        branch.shift = shift_v;
        branch.in_service = br_status[row] != 0;
        branch.angmin = ang_min[row];
        branch.angmax = ang_max[row];
        branches.push(branch);
    }

    let mut net = BalancedNetwork::new(name, base_mva);
    *net.buses_mut() = buses;
    *net.loads_mut() = loads;
    *net.shunts_mut() = shunts;
    *net.branches_mut() = branches;
    *net.generators_mut() = generators;
    *net.source_format_mut() = SourceFormat::Gridfm;
    net.validate()?;

    // --- fidelity warnings: what the gridfm schema couldn't carry back ---
    warnings.push(
        &codes::READ_GRIDFM_VALUE_INFERRED,
        format!(
            "synthesized bus ids 1..={}; original bus ids are not stored in a gridfm dataset, \
         so a written case is renumbered",
            net.buses().len()
        ),
    );
    if !net.loads().is_empty() {
        warnings.push(
            &codes::READ_GRIDFM_VALUE_COLLAPSED,
            format!(
                "folded nodal load into {} synthetic per-bus Load(s); per-load granularity is \
             not recoverable",
                net.loads().len()
            ),
        );
    }
    if !net.shunts().is_empty() {
        warnings.push(
            &codes::READ_GRIDFM_VALUE_COLLAPSED,
            format!(
                "folded nodal shunts into {} synthetic per-bus Shunt(s); per-shunt granularity \
             is not recoverable",
                net.shunts().len()
            ),
        );
    }
    if unit_tap_lines > 0 {
        warnings.push(&codes::READ_GRIDFM_ELEMENT_RELABELED, format!(
            "{unit_tap_lines} branch(es) had unit effective tap and no phase shift and were read \
             as lines (raw tap 0); a unity-ratio, zero-shift transformer in the source is \
             indistinguishable from a line and is read as one (the power flow is identical)"
        ));
    }
    let no_cost_gens = net.generators().iter().filter(|g| g.cost.is_none()).count();
    if no_cost_gens > 0 {
        warnings.push(&codes::READ_GRIDFM_FIELD_DROPPED, format!(
            "{no_cost_gens} generator(s) read with no cost: an all-zero cost triple in the dataset \
             is the writer's encoding for a generator with no cost, a genuine zero polynomial \
             cost, or a piecewise/cubic+ cost it couldn't represent — indistinguishable on read"
        ));
    }
    warnings.push(
        &codes::READ_GRIDFM_FIELD_DROPPED,
        "HVDC, storage, areas/zones, bus names, rate_b/rate_c, generator mbase/ramp limits, \
         and startup/shutdown costs are absent from the gridfm schema",
    );

    Ok(GridfmRead {
        network: net,
        scenario,
        warnings: warnings.lines(),
        diagnostics: warnings.into_records(),
    })
}

// --- reader helpers --------------------------------------------------------

/// One `READ.GRIDFM.VALUE_DEFAULTED` note, for a manifest the reader could not
/// use.
fn defaulted_meta(message: impl Into<String>) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();
    diagnostics.push(&codes::READ_GRIDFM_VALUE_DEFAULTED, message);
    diagnostics
}

/// Row indices whose `scenario` column equals `scenario`, in table order.
fn scenario_rows(scen: &[i64], scenario: i64) -> Vec<usize> {
    scen.iter()
        .enumerate()
        .filter_map(|(i, &s)| (s == scenario).then_some(i))
        .collect()
}

/// Guard a gen/branch table: empty `rows` is fine when the whole table is empty
/// (a case legitimately has no generators, or no branches), but if the table
/// holds rows for *other* scenarios yet none for this one, the dataset is partial
/// or corrupt and would silently yield a wrong-but-valid network — error instead.
fn require_scenario_block(
    scen_col: &[i64],
    scenario: i64,
    rows: &[usize],
    table: &str,
) -> Result<()> {
    if rows.is_empty() && !scen_col.is_empty() {
        return Err(powerio_tx::Error::FormatRead {
            format: "gridfm",
            message: format!(
                "scenario {scenario} has no {table} rows, but the table holds {} row(s) for other \
                 scenarios — a partial or corrupt dataset",
                scen_col.len()
            ),
        });
    }
    Ok(())
}

/// A dense `[0, n)` parquet index → 1-based [`BusId`]. Errors on a negative index.
fn dense_bus_id(v: i64) -> Result<BusId> {
    let idx = usize::try_from(v).map_err(|_| powerio_tx::Error::FormatRead {
        format: "gridfm",
        message: format!("negative dense bus index {v}"),
    })?;
    Ok(BusId(idx + 1))
}

/// Look up a named column, erroring if absent.
fn column<'a>(b: &'a RecordBatch, name: &str) -> Result<&'a ArrayRef> {
    b.column_by_name(name)
        .ok_or_else(|| powerio_tx::Error::FormatRead {
            format: "gridfm",
            message: format!("missing column `{name}`"),
        })
}

/// Concatenate a named non-null `Int64` column across all batches.
fn i64_col(batches: &[RecordBatch], name: &str) -> Result<Vec<i64>> {
    let mut out = Vec::with_capacity(batches.iter().map(RecordBatch::num_rows).sum());
    for b in batches {
        let arr = column(b, name)?;
        let col = arr.as_any().downcast_ref::<Int64Array>().ok_or_else(|| {
            powerio_tx::Error::FormatRead {
                format: "gridfm",
                message: format!("column `{name}` is not Int64"),
            }
        })?;
        if col.null_count() > 0 {
            return Err(powerio_tx::Error::FormatRead {
                format: "gridfm",
                message: format!("column `{name}` has nulls"),
            });
        }
        out.extend_from_slice(col.values());
    }
    Ok(out)
}

/// Concatenate a named non-null `Float64` column across all batches.
fn f64_col(batches: &[RecordBatch], name: &str) -> Result<Vec<f64>> {
    let mut out = Vec::with_capacity(batches.iter().map(RecordBatch::num_rows).sum());
    for b in batches {
        let arr = column(b, name)?;
        let col = arr.as_any().downcast_ref::<Float64Array>().ok_or_else(|| {
            powerio_tx::Error::FormatRead {
                format: "gridfm",
                message: format!("column `{name}` is not Float64"),
            }
        })?;
        if col.null_count() > 0 {
            return Err(powerio_tx::Error::FormatRead {
                format: "gridfm",
                message: format!("column `{name}` has nulls"),
            });
        }
        out.extend_from_slice(col.values());
    }
    Ok(out)
}

/// Read one scenario from a dataset directory in the named `from` format.
/// This function dispatches dataset format names; the C ABI's `pio_read_dir`
/// wraps it. `gridfm` is the currently supported dataset format; `scenario`
/// selects within it. PyPSA CSV directories are case inputs rather than
/// datasets and parse through the ordinary parse.
///
/// # Errors
/// [`powerio_tx::Error::UnknownFormat`] for a non-dataset format name;
/// otherwise as [`read_gridfm_dataset`].
pub fn read_dataset_dir(
    dir: impl AsRef<std::path::Path>,
    from: &str,
    scenario: i64,
) -> Result<GridfmRead> {
    require_dataset_format(from)?;
    read_gridfm_dataset(dir, scenario)
}

/// Return the distinct scenario IDs in ascending order for dataset directory
/// `dir` in the named `from` format. The C ABI exposes the same query through
/// `pio_scenario_ids`.
///
/// # Errors
/// As [`read_dataset_dir`].
pub fn dataset_scenario_ids(dir: impl AsRef<std::path::Path>, from: &str) -> Result<Vec<i64>> {
    require_dataset_format(from)?;
    gridfm_scenario_ids(dir)
}

fn require_dataset_format(from: &str) -> Result<()> {
    if from.eq_ignore_ascii_case("gridfm") {
        return Ok(());
    }
    Err(powerio_tx::Error::UnknownFormat(format!(
        "{from} is not a dataset directory format (dataset formats: gridfm); \
         PyPSA CSV directories parse through the ordinary parse"
    )))
}

/// Zero copy `bytes::Bytes` over an acquired source buffer, for the parquet
/// reader.
struct BufferBytes(powerio_core::SourceBuffer);

impl AsRef<[u8]> for BufferBytes {
    fn as_ref(&self) -> &[u8] {
        self.0.bytes()
    }
}

fn core_error(error: &powerio_tx::Error) -> powerio_core::Error {
    powerio_core::Error::new(error.code(), error.to_string())
}

/// The raw table prefix within the entry listing, resolved as leniently as
/// the path reader resolves directories: the tables at the root, under
/// `raw/`, or under exactly one `<case>/raw/`.
fn resolve_raw_prefix(entries: &[powerio_core::ArtifactPath]) -> Result<String> {
    let holds = |prefix: &str| {
        entries
            .iter()
            .any(|entry| entry.as_str() == format!("{prefix}bus_data.parquet"))
    };
    if holds("") {
        return Ok(String::new());
    }
    if holds("raw/") {
        return Ok("raw/".to_owned());
    }
    let mut nested: Vec<String> = entries
        .iter()
        .filter_map(|entry| {
            entry
                .as_str()
                .strip_suffix("/raw/bus_data.parquet")
                .filter(|case| !case.contains('/'))
                .map(|case| format!("{case}/raw/"))
        })
        .collect();
    nested.sort();
    nested.dedup();
    match nested.len() {
        0 => Err(powerio_tx::Error::FormatRead {
            format: "gridfm",
            message: "no gridfm dataset found: no bus_data.parquet at the root, under raw/, \
                      or under a single <case>/raw/"
                .to_owned(),
        }),
        1 => Ok(nested.remove(0)),
        n => Err(powerio_tx::Error::FormatRead {
            format: "gridfm",
            message: format!(
                "{n} <case>/raw/ dataset directories found; open one of them directly"
            ),
        }),
    }
}

fn read_parquet_buffer(
    source: &powerio_core::Source,
    prefix: &str,
    file: &str,
) -> Result<Vec<RecordBatch>> {
    let name = format!("{prefix}{file}");
    let path = powerio_core::ArtifactPath::new(name.clone()).map_err(|error| {
        powerio_tx::Error::FormatRead {
            format: "gridfm",
            message: error.to_string(),
        }
    })?;
    let buffer = source
        .buffer(&path)
        .map_err(|error| powerio_tx::Error::FormatRead {
            format: "gridfm",
            message: format!("acquiring {name}: {error}"),
        })?;
    let bytes = bytes::Bytes::from_owner(BufferBytes(buffer));
    let reader = ParquetRecordBatchReaderBuilder::try_new(bytes)
        .and_then(ParquetRecordBatchReaderBuilder::build)
        .map_err(|e| powerio_tx::Error::FormatRead {
            format: "gridfm",
            message: format!("reading {name}: {e}"),
        })?;
    reader
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| powerio_tx::Error::FormatRead {
            format: "gridfm",
            message: format!("decoding {name}: {e}"),
        })
}

fn read_meta_source(source: &powerio_core::Source, prefix: &str) -> (f64, String, Diagnostics) {
    let fallback_name = || {
        prefix.strip_suffix("/raw/").map_or_else(
            || {
                std::path::Path::new(source.name())
                    .file_name()
                    .and_then(|s| s.to_str())
                    .map_or_else(|| "gridfm".to_owned(), str::to_owned)
            },
            str::to_owned,
        )
    };
    let Ok(meta_path) = powerio_core::ArtifactPath::new(format!("{prefix}gridfm_meta.json")) else {
        return (
            100.0,
            fallback_name(),
            defaulted_meta("gridfm_meta.json name did not validate; base_mva defaulted to 100"),
        );
    };
    let Ok(buffer) = source.buffer(&meta_path) else {
        return (
            100.0,
            fallback_name(),
            defaulted_meta("gridfm_meta.json could not be acquired; base_mva defaulted to 100"),
        );
    };
    let Ok(text) = std::str::from_utf8(buffer.content_bytes()) else {
        return (
            100.0,
            fallback_name(),
            defaulted_meta("gridfm_meta.json is not UTF-8; base_mva defaulted to 100"),
        );
    };
    let Ok(meta) = serde_json::from_str::<serde_json::Value>(text) else {
        return (
            100.0,
            fallback_name(),
            defaulted_meta("gridfm_meta.json is not valid JSON; base_mva defaulted to 100"),
        );
    };
    let name = meta
        .get("case_name")
        .and_then(serde_json::Value::as_str)
        .map_or_else(fallback_name, str::to_string);
    let mut warnings = Diagnostics::new();
    let base = match meta.get("base_mva").and_then(serde_json::Value::as_f64) {
        Some(b) if b.is_finite() && b > 0.0 => b,
        _ => {
            warnings.push(
                &codes::READ_GRIDFM_VALUE_DEFAULTED,
                "gridfm_meta.json has no usable base_mva (absent or not a positive number); \
                 defaulted to 100",
            );
            100.0
        }
    };
    (base, name, warnings)
}

/// Parse a gridfm dataset from the retained directory source: every scenario
/// as one scenario set over shared element identities, with each scenario's
/// read findings in ascending scenario order.
pub(crate) fn parse_gridfm_source(
    source: &powerio_core::Source,
) -> std::result::Result<
    (
        powerio_core::ScenarioSet<BalancedNetwork>,
        Vec<powerio_core::Diagnostic>,
    ),
    powerio_core::Error,
> {
    let entries = source.entry_names()?;
    let prefix = resolve_raw_prefix(&entries).map_err(|error| core_error(&error))?;
    let (base_mva, name, meta_warnings) = read_meta_source(source, &prefix);
    let bus = bus_columns(
        &read_parquet_buffer(source, &prefix, "bus_data.parquet")
            .map_err(|error| core_error(&error))?,
    )
    .map_err(|error| core_error(&error))?;
    let gens = gen_columns(
        &read_parquet_buffer(source, &prefix, "gen_data.parquet")
            .map_err(|error| core_error(&error))?,
    )
    .map_err(|error| core_error(&error))?;
    let branch = branch_columns(
        &read_parquet_buffer(source, &prefix, "branch_data.parquet")
            .map_err(|error| core_error(&error))?,
    )
    .map_err(|error| core_error(&error))?;

    let mut diagnostics = Vec::new();
    let mut scenarios = Vec::new();
    let mut donor: Option<BalancedNetwork> = None;
    for scenario in distinct_sorted(&bus.scenario) {
        let read = build_network_from_columns(
            &bus,
            &gens,
            &branch,
            scenario,
            base_mva,
            &name,
            meta_warnings.clone(),
        )
        .map_err(|error| core_error(&error))?;
        let mut network = read.network;
        match &donor {
            Some(base) => network.share_equal_tables(base),
            None => donor = Some(network.clone()),
        }
        diagnostics.extend(read.diagnostics);
        let id = powerio_core::ScenarioId::new(read.scenario.to_string())?;
        scenarios.push(powerio_core::Scenario::new(id, None, network));
    }
    let set = powerio_core::ScenarioSet::new(scenarios)?;
    Ok((set, diagnostics))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn require_scenario_block_flags_partial_tables() {
        // Empty table → ok (a legitimately element-less case). Present → ok.
        // Absent from a non-empty table → error (a partial or corrupt
        // dataset would otherwise silently yield a wrong but valid network).
        assert!(require_scenario_block(&[], 0, &[], "gen_data").is_ok());
        assert!(require_scenario_block(&[0, 0, 1], 0, &[0, 1], "gen_data").is_ok());
        let err = require_scenario_block(&[0, 0], 1, &[], "branch_data").unwrap_err();
        assert!(
            matches!(
                err,
                powerio_tx::Error::FormatRead {
                    format: "gridfm",
                    ..
                }
            ),
            "got {err:?}"
        );
    }
}
