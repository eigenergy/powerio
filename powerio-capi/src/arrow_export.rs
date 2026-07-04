//! Raw network tables over the Arrow C Data Interface.
//!
//! Builds the parsed [`Network`] element tables (bus/branch/gen/load/shunt) as
//! Arrow record batches and lends them across the C ABI zero-copy via
//! [`arrow::ffi::to_ffi`]. This is the in-memory, self-describing sibling of
//! the `powerio-json` snapshot and the `pio_branches`-style numeric
//! extractors: any Arrow consumer (pyarrow, Arrow.jl, Arrow C++, polars, DuckDB)
//! can pull a whole table without a copy or a temp file. The schema is the
//! ABI's evolution valve: richer columns arrive here, never as new C
//! signatures.
//!
//! Tables 0..5 are the *raw* network fields, with EXTERNAL bus ids (the same id
//! space as `pio_bus_ids`), not the gridfm-datakit schema. Tables 6..14 are the
//! normalized solver table contract: per unit/radian values and dense zero based
//! row ids. Matrix table ids after that carry COO triplets in the same dense bus
//! index space, with matrix dimensions stored in Arrow schema metadata.

#[cfg(feature = "matrix")]
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;

use arrow::array::{Array, ArrayRef, Float64Array, Int64Array, StructArray, UInt8Array};
use arrow::datatypes::{Field, Schema};
use arrow::error::ArrowError;
use arrow::ffi::{FFI_ArrowArray, FFI_ArrowSchema};
use arrow::record_batch::RecordBatch;
#[cfg(feature = "matrix")]
use powerio::IndexedNetwork;
use powerio::{BusId, IndexCore, Network, NormalizedSolverTables, SolverArcTerminal};

/// Table selectors for [`pio_to_arrow`](crate::pio_to_arrow); the C
/// header mirrors these as `PIO_ARROW_TABLE_*`.
pub const PIO_ARROW_TABLE_BUS: i32 = 0;
pub const PIO_ARROW_TABLE_BRANCH: i32 = 1;
pub const PIO_ARROW_TABLE_GEN: i32 = 2;
pub const PIO_ARROW_TABLE_LOAD: i32 = 3;
pub const PIO_ARROW_TABLE_SHUNT: i32 = 4;
pub const PIO_ARROW_TABLE_SWITCH: i32 = 5;
pub const PIO_ARROW_TABLE_SOLVER_BUS: i32 = 6;
pub const PIO_ARROW_TABLE_SOLVER_LOAD: i32 = 7;
pub const PIO_ARROW_TABLE_SOLVER_SHUNT: i32 = 8;
pub const PIO_ARROW_TABLE_SOLVER_BRANCH: i32 = 9;
pub const PIO_ARROW_TABLE_SOLVER_SWITCH: i32 = 10;
pub const PIO_ARROW_TABLE_SOLVER_ARC: i32 = 11;
pub const PIO_ARROW_TABLE_SOLVER_GEN: i32 = 12;
pub const PIO_ARROW_TABLE_SOLVER_STORAGE: i32 = 13;
pub const PIO_ARROW_TABLE_SOLVER_HVDC: i32 = 14;
pub const PIO_ARROW_TABLE_YBUS: i32 = 15;
pub const PIO_ARROW_TABLE_INCIDENCE: i32 = 16;
pub const PIO_ARROW_TABLE_BPRIME: i32 = 17;
pub const PIO_ARROW_TABLE_BDOUBLEPRIME: i32 = 18;

// These values are the ABI: the `PIO_ARROW_TABLE_*` macros in include/powerio.h
// are hand-synced to them. The set is append-only: these ids and each table's
// column order are frozen, a new table takes the next id and extends
// this assert, and new columns append (nullable) at the end so consumers read by
// name. Pin them so a Rust-side edit that drifts from the header (a renumber, a
// reorder, a dropped table) fails the build instead of silently exporting the
// wrong table.
const _: () = assert!(
    PIO_ARROW_TABLE_BUS == 0
        && PIO_ARROW_TABLE_BRANCH == 1
        && PIO_ARROW_TABLE_GEN == 2
        && PIO_ARROW_TABLE_LOAD == 3
        && PIO_ARROW_TABLE_SHUNT == 4
        && PIO_ARROW_TABLE_SWITCH == 5
        && PIO_ARROW_TABLE_SOLVER_BUS == 6
        && PIO_ARROW_TABLE_SOLVER_LOAD == 7
        && PIO_ARROW_TABLE_SOLVER_SHUNT == 8
        && PIO_ARROW_TABLE_SOLVER_BRANCH == 9
        && PIO_ARROW_TABLE_SOLVER_SWITCH == 10
        && PIO_ARROW_TABLE_SOLVER_ARC == 11
        && PIO_ARROW_TABLE_SOLVER_GEN == 12
        && PIO_ARROW_TABLE_SOLVER_STORAGE == 13
        && PIO_ARROW_TABLE_SOLVER_HVDC == 14
        && PIO_ARROW_TABLE_YBUS == 15
        && PIO_ARROW_TABLE_INCIDENCE == 16
        && PIO_ARROW_TABLE_BPRIME == 17
        && PIO_ARROW_TABLE_BDOUBLEPRIME == 18
);

/// Build the requested table and export it over the C Data Interface. The
/// returned FFI structs own the columnar buffers until the consumer releases
/// them.
pub fn export(
    net: &Network,
    core: &IndexCore,
    table: i32,
) -> Result<(FFI_ArrowArray, FFI_ArrowSchema), String> {
    let rb = match table {
        PIO_ARROW_TABLE_BUS => bus_batch(net).map_err(|e| e.to_string())?,
        PIO_ARROW_TABLE_BRANCH => branch_batch(net).map_err(|e| e.to_string())?,
        PIO_ARROW_TABLE_GEN => gen_batch(net).map_err(|e| e.to_string())?,
        PIO_ARROW_TABLE_LOAD => load_batch(net).map_err(|e| e.to_string())?,
        PIO_ARROW_TABLE_SHUNT => shunt_batch(net).map_err(|e| e.to_string())?,
        PIO_ARROW_TABLE_SWITCH => switch_batch(net).map_err(|e| e.to_string())?,
        PIO_ARROW_TABLE_SOLVER_BUS => {
            solver_bus_batch(&solver_tables(net)?).map_err(|e| e.to_string())?
        }
        PIO_ARROW_TABLE_SOLVER_LOAD => {
            solver_load_batch(&solver_tables(net)?).map_err(|e| e.to_string())?
        }
        PIO_ARROW_TABLE_SOLVER_SHUNT => {
            solver_shunt_batch(&solver_tables(net)?).map_err(|e| e.to_string())?
        }
        PIO_ARROW_TABLE_SOLVER_BRANCH => {
            solver_branch_batch(&solver_tables(net)?).map_err(|e| e.to_string())?
        }
        PIO_ARROW_TABLE_SOLVER_SWITCH => {
            solver_switch_batch(&solver_tables(net)?).map_err(|e| e.to_string())?
        }
        PIO_ARROW_TABLE_SOLVER_ARC => {
            solver_arc_batch(&solver_tables(net)?).map_err(|e| e.to_string())?
        }
        PIO_ARROW_TABLE_SOLVER_GEN => {
            solver_gen_batch(&solver_tables(net)?).map_err(|e| e.to_string())?
        }
        PIO_ARROW_TABLE_SOLVER_STORAGE => {
            solver_storage_batch(&solver_tables(net)?).map_err(|e| e.to_string())?
        }
        PIO_ARROW_TABLE_SOLVER_HVDC => {
            solver_hvdc_batch(&solver_tables(net)?).map_err(|e| e.to_string())?
        }
        PIO_ARROW_TABLE_YBUS => matrix_ybus_batch(net, core)?,
        PIO_ARROW_TABLE_INCIDENCE => matrix_incidence_batch(net, core)?,
        PIO_ARROW_TABLE_BPRIME => matrix_bprime_batch(net, core)?,
        PIO_ARROW_TABLE_BDOUBLEPRIME => matrix_bdoubleprime_batch(net, core)?,
        other => return Err(format!("unknown Arrow table id {other}")),
    };

    // The C Data Interface represents a record batch as a struct array. Build
    // the schema from the RecordBatch, not from ArrayData, so table metadata
    // such as matrix dimensions survives the FFI boundary.
    let schema = FFI_ArrowSchema::try_from(rb.schema().as_ref()).map_err(|e| e.to_string())?;
    let data = StructArray::from(rb).into_data();
    Ok((FFI_ArrowArray::new(&data), schema))
}

fn solver_tables(net: &Network) -> Result<NormalizedSolverTables, String> {
    net.to_normalized_solver_tables().map_err(|e| e.to_string())
}

fn bus_batch(net: &Network) -> Result<RecordBatch, ArrowError> {
    let b = &net.buses;
    batch(vec![
        ("id", i64s(b.iter().map(|x| ext(x.id)).collect())),
        (
            "kind",
            i64s(b.iter().map(|x| i64::from(x.kind as u8)).collect()),
        ),
        ("vm", f64s(b.iter().map(|x| x.vm).collect())),
        ("va", f64s(b.iter().map(|x| x.va).collect())),
        ("base_kv", f64s(b.iter().map(|x| x.base_kv).collect())),
        ("vmax", f64s(b.iter().map(|x| x.vmax).collect())),
        ("vmin", f64s(b.iter().map(|x| x.vmin).collect())),
        ("area", i64s(b.iter().map(|x| usz(x.area)).collect())),
        ("zone", i64s(b.iter().map(|x| usz(x.zone)).collect())),
    ])
}

fn branch_batch(net: &Network) -> Result<RecordBatch, ArrowError> {
    let br = &net.branches;
    batch(vec![
        ("from", i64s(br.iter().map(|x| ext(x.from)).collect())),
        ("to", i64s(br.iter().map(|x| ext(x.to)).collect())),
        ("r", f64s(br.iter().map(|x| x.r).collect())),
        ("x", f64s(br.iter().map(|x| x.x).collect())),
        (
            "b",
            f64s(br.iter().map(|x| x.legacy_total_charging_b()).collect()),
        ),
        ("rate_a", f64s(br.iter().map(|x| x.rate_a).collect())),
        ("rate_b", f64s(br.iter().map(|x| x.rate_b).collect())),
        ("rate_c", f64s(br.iter().map(|x| x.rate_c).collect())),
        ("tap", f64s(br.iter().map(|x| x.tap).collect())),
        ("shift", f64s(br.iter().map(|x| x.shift).collect())),
        (
            "in_service",
            u8s(br.iter().map(|x| u8::from(x.in_service)).collect()),
        ),
        ("angmin", f64s(br.iter().map(|x| x.angmin).collect())),
        ("angmax", f64s(br.iter().map(|x| x.angmax).collect())),
        (
            "g_fr",
            f64s(br.iter().map(|x| x.terminal_charging().g_fr).collect()),
        ),
        (
            "b_fr",
            f64s(br.iter().map(|x| x.terminal_charging().b_fr).collect()),
        ),
        (
            "g_to",
            f64s(br.iter().map(|x| x.terminal_charging().g_to).collect()),
        ),
        (
            "b_to",
            f64s(br.iter().map(|x| x.terminal_charging().b_to).collect()),
        ),
        (
            "c_rating_a",
            f64s(
                br.iter()
                    .map(|x| x.current_ratings.map_or(0.0, |r| r.c_rating_a))
                    .collect(),
            ),
        ),
        (
            "c_rating_b",
            f64s(
                br.iter()
                    .map(|x| x.current_ratings.map_or(0.0, |r| r.c_rating_b))
                    .collect(),
            ),
        ),
        (
            "c_rating_c",
            f64s(
                br.iter()
                    .map(|x| x.current_ratings.map_or(0.0, |r| r.c_rating_c))
                    .collect(),
            ),
        ),
        (
            "pf",
            f64s(
                br.iter()
                    .map(|x| x.solution.map_or(0.0, |s| s.pf))
                    .collect(),
            ),
        ),
        (
            "qf",
            f64s(
                br.iter()
                    .map(|x| x.solution.map_or(0.0, |s| s.qf))
                    .collect(),
            ),
        ),
        (
            "pt",
            f64s(
                br.iter()
                    .map(|x| x.solution.map_or(0.0, |s| s.pt))
                    .collect(),
            ),
        ),
        (
            "qt",
            f64s(
                br.iter()
                    .map(|x| x.solution.map_or(0.0, |s| s.qt))
                    .collect(),
            ),
        ),
    ])
}

fn gen_batch(net: &Network) -> Result<RecordBatch, ArrowError> {
    let g = &net.generators;
    batch(vec![
        ("bus", i64s(g.iter().map(|x| ext(x.bus)).collect())),
        ("pg", f64s(g.iter().map(|x| x.pg).collect())),
        ("qg", f64s(g.iter().map(|x| x.qg).collect())),
        ("pmax", f64s(g.iter().map(|x| x.pmax).collect())),
        ("pmin", f64s(g.iter().map(|x| x.pmin).collect())),
        ("qmax", f64s(g.iter().map(|x| x.qmax).collect())),
        ("qmin", f64s(g.iter().map(|x| x.qmin).collect())),
        ("vg", f64s(g.iter().map(|x| x.vg).collect())),
        ("mbase", f64s(g.iter().map(|x| x.mbase).collect())),
        (
            "in_service",
            u8s(g.iter().map(|x| u8::from(x.in_service)).collect()),
        ),
    ])
}

fn load_batch(net: &Network) -> Result<RecordBatch, ArrowError> {
    let l = &net.loads;
    batch(vec![
        ("bus", i64s(l.iter().map(|x| ext(x.bus)).collect())),
        ("p", f64s(l.iter().map(|x| x.p).collect())),
        ("q", f64s(l.iter().map(|x| x.q).collect())),
        (
            "in_service",
            u8s(l.iter().map(|x| u8::from(x.in_service)).collect()),
        ),
    ])
}

fn shunt_batch(net: &Network) -> Result<RecordBatch, ArrowError> {
    let s = &net.shunts;
    batch(vec![
        ("bus", i64s(s.iter().map(|x| ext(x.bus)).collect())),
        ("g", f64s(s.iter().map(|x| x.g).collect())),
        ("b", f64s(s.iter().map(|x| x.b).collect())),
        (
            "in_service",
            u8s(s.iter().map(|x| u8::from(x.in_service)).collect()),
        ),
    ])
}

fn switch_batch(net: &Network) -> Result<RecordBatch, ArrowError> {
    let s = &net.switches;
    batch(vec![
        ("from", i64s(s.iter().map(|x| ext(x.from)).collect())),
        ("to", i64s(s.iter().map(|x| ext(x.to)).collect())),
        (
            "closed",
            u8s(s.iter().map(|x| u8::from(x.closed)).collect()),
        ),
        (
            "thermal_rating",
            f64s(s.iter().map(|x| x.thermal_rating.unwrap_or(0.0)).collect()),
        ),
        (
            "current_rating",
            f64s(s.iter().map(|x| x.current_rating.unwrap_or(0.0)).collect()),
        ),
        ("pf", f64s(s.iter().map(|x| x.pf.unwrap_or(0.0)).collect())),
        ("qf", f64s(s.iter().map(|x| x.qf.unwrap_or(0.0)).collect())),
        ("pt", f64s(s.iter().map(|x| x.pt.unwrap_or(0.0)).collect())),
        ("qt", f64s(s.iter().map(|x| x.qt.unwrap_or(0.0)).collect())),
    ])
}

fn solver_bus_batch(t: &NormalizedSolverTables) -> Result<RecordBatch, ArrowError> {
    batch(vec![
        (
            "index",
            i64s(t.buses.iter().map(|x| usz(x.index)).collect()),
        ),
        (
            "bus_id",
            i64s(t.buses.iter().map(|x| ext(x.bus_id)).collect()),
        ),
        (
            "source_row",
            i64s(t.buses.iter().map(|x| opt_usz(x.source_row)).collect()),
        ),
        (
            "kind",
            i64s(t.buses.iter().map(|x| i64::from(x.kind as u8)).collect()),
        ),
        ("vm", f64s(t.buses.iter().map(|x| x.vm).collect())),
        ("va", f64s(t.buses.iter().map(|x| x.va).collect())),
        ("base_kv", f64s(t.buses.iter().map(|x| x.base_kv).collect())),
        ("vmax", f64s(t.buses.iter().map(|x| x.vmax).collect())),
        ("vmin", f64s(t.buses.iter().map(|x| x.vmin).collect())),
        ("pd", f64s(t.buses.iter().map(|x| x.pd).collect())),
        ("qd", f64s(t.buses.iter().map(|x| x.qd).collect())),
        ("gs", f64s(t.buses.iter().map(|x| x.gs).collect())),
        ("bs", f64s(t.buses.iter().map(|x| x.bs).collect())),
        (
            "component_label",
            i64s(t.index.component_labels.iter().map(|&x| usz(x)).collect()),
        ),
        (
            "is_reference",
            u8s(t
                .buses
                .iter()
                .map(|x| u8::from(t.index.reference_bus_indices.contains(&x.index)))
                .collect()),
        ),
    ])
}

fn solver_load_batch(t: &NormalizedSolverTables) -> Result<RecordBatch, ArrowError> {
    batch(vec![
        (
            "index",
            i64s(t.loads.iter().map(|x| usz(x.index)).collect()),
        ),
        (
            "source_row",
            i64s(t.loads.iter().map(|x| opt_usz(x.source_row)).collect()),
        ),
        (
            "bus_index",
            i64s(t.loads.iter().map(|x| usz(x.bus_index)).collect()),
        ),
        ("p", f64s(t.loads.iter().map(|x| x.p).collect())),
        ("q", f64s(t.loads.iter().map(|x| x.q).collect())),
    ])
}

fn solver_shunt_batch(t: &NormalizedSolverTables) -> Result<RecordBatch, ArrowError> {
    batch(vec![
        (
            "index",
            i64s(t.shunts.iter().map(|x| usz(x.index)).collect()),
        ),
        (
            "source_row",
            i64s(t.shunts.iter().map(|x| opt_usz(x.source_row)).collect()),
        ),
        (
            "bus_index",
            i64s(t.shunts.iter().map(|x| usz(x.bus_index)).collect()),
        ),
        ("g", f64s(t.shunts.iter().map(|x| x.g).collect())),
        ("b", f64s(t.shunts.iter().map(|x| x.b).collect())),
    ])
}

fn solver_branch_batch(t: &NormalizedSolverTables) -> Result<RecordBatch, ArrowError> {
    batch(vec![
        (
            "index",
            i64s(t.branches.iter().map(|x| usz(x.index)).collect()),
        ),
        (
            "source_row",
            i64s(t.branches.iter().map(|x| opt_usz(x.source_row)).collect()),
        ),
        (
            "from_bus_index",
            i64s(t.branches.iter().map(|x| usz(x.from_bus_index)).collect()),
        ),
        (
            "to_bus_index",
            i64s(t.branches.iter().map(|x| usz(x.to_bus_index)).collect()),
        ),
        ("r", f64s(t.branches.iter().map(|x| x.r).collect())),
        ("x", f64s(t.branches.iter().map(|x| x.x).collect())),
        ("b", f64s(t.branches.iter().map(|x| x.b).collect())),
        ("g_fr", f64s(t.branches.iter().map(|x| x.g_fr).collect())),
        ("b_fr", f64s(t.branches.iter().map(|x| x.b_fr).collect())),
        ("g_to", f64s(t.branches.iter().map(|x| x.g_to).collect())),
        ("b_to", f64s(t.branches.iter().map(|x| x.b_to).collect())),
        (
            "rate_a",
            f64s(t.branches.iter().map(|x| x.rate_a).collect()),
        ),
        (
            "rate_b",
            f64s(t.branches.iter().map(|x| x.rate_b).collect()),
        ),
        (
            "rate_c",
            f64s(t.branches.iter().map(|x| x.rate_c).collect()),
        ),
        ("tap", f64s(t.branches.iter().map(|x| x.tap).collect())),
        ("shift", f64s(t.branches.iter().map(|x| x.shift).collect())),
        (
            "angmin",
            f64s(t.branches.iter().map(|x| x.angmin).collect()),
        ),
        (
            "angmax",
            f64s(t.branches.iter().map(|x| x.angmax).collect()),
        ),
    ])
}

fn solver_switch_batch(t: &NormalizedSolverTables) -> Result<RecordBatch, ArrowError> {
    batch(vec![
        (
            "index",
            i64s(t.switches.iter().map(|x| usz(x.index)).collect()),
        ),
        (
            "source_row",
            i64s(t.switches.iter().map(|x| opt_usz(x.source_row)).collect()),
        ),
        (
            "from_bus_index",
            i64s(t.switches.iter().map(|x| usz(x.from_bus_index)).collect()),
        ),
        (
            "to_bus_index",
            i64s(t.switches.iter().map(|x| usz(x.to_bus_index)).collect()),
        ),
        (
            "closed",
            u8s(t.switches.iter().map(|x| u8::from(x.closed)).collect()),
        ),
        (
            "thermal_rating",
            f64s(
                t.switches
                    .iter()
                    .map(|x| x.thermal_rating.unwrap_or(0.0))
                    .collect(),
            ),
        ),
        (
            "current_rating",
            f64s(
                t.switches
                    .iter()
                    .map(|x| x.current_rating.unwrap_or(0.0))
                    .collect(),
            ),
        ),
        (
            "pf",
            f64s(t.switches.iter().map(|x| x.pf.unwrap_or(0.0)).collect()),
        ),
        (
            "qf",
            f64s(t.switches.iter().map(|x| x.qf.unwrap_or(0.0)).collect()),
        ),
        (
            "pt",
            f64s(t.switches.iter().map(|x| x.pt.unwrap_or(0.0)).collect()),
        ),
        (
            "qt",
            f64s(t.switches.iter().map(|x| x.qt.unwrap_or(0.0)).collect()),
        ),
    ])
}

fn solver_arc_batch(t: &NormalizedSolverTables) -> Result<RecordBatch, ArrowError> {
    batch(vec![
        ("index", i64s(t.arcs.iter().map(|x| usz(x.index)).collect())),
        (
            "branch_index",
            i64s(t.arcs.iter().map(|x| usz(x.branch_index)).collect()),
        ),
        (
            "terminal",
            i64s(
                t.arcs
                    .iter()
                    .map(|x| match x.terminal {
                        SolverArcTerminal::From => 0,
                        SolverArcTerminal::To => 1,
                    })
                    .collect(),
            ),
        ),
        (
            "from_bus_index",
            i64s(t.arcs.iter().map(|x| usz(x.from_bus_index)).collect()),
        ),
        (
            "to_bus_index",
            i64s(t.arcs.iter().map(|x| usz(x.to_bus_index)).collect()),
        ),
        ("tap", f64s(t.arcs.iter().map(|x| x.tap).collect())),
        ("shift", f64s(t.arcs.iter().map(|x| x.shift).collect())),
        ("g_shunt", f64s(t.arcs.iter().map(|x| x.g_shunt).collect())),
        ("b_shunt", f64s(t.arcs.iter().map(|x| x.b_shunt).collect())),
        ("rate_a", f64s(t.arcs.iter().map(|x| x.rate_a).collect())),
    ])
}

fn solver_gen_batch(t: &NormalizedSolverTables) -> Result<RecordBatch, ArrowError> {
    batch(vec![
        (
            "index",
            i64s(t.generators.iter().map(|x| usz(x.index)).collect()),
        ),
        (
            "source_row",
            i64s(t.generators.iter().map(|x| opt_usz(x.source_row)).collect()),
        ),
        (
            "bus_index",
            i64s(t.generators.iter().map(|x| usz(x.bus_index)).collect()),
        ),
        ("pg", f64s(t.generators.iter().map(|x| x.pg).collect())),
        ("qg", f64s(t.generators.iter().map(|x| x.qg).collect())),
        ("pmax", f64s(t.generators.iter().map(|x| x.pmax).collect())),
        ("pmin", f64s(t.generators.iter().map(|x| x.pmin).collect())),
        ("qmax", f64s(t.generators.iter().map(|x| x.qmax).collect())),
        ("qmin", f64s(t.generators.iter().map(|x| x.qmin).collect())),
        ("vg", f64s(t.generators.iter().map(|x| x.vg).collect())),
        (
            "mbase",
            f64s(t.generators.iter().map(|x| x.mbase).collect()),
        ),
        (
            "regulated_bus_index",
            i64s(
                t.generators
                    .iter()
                    .map(|x| opt_usz(x.regulated_bus_index))
                    .collect(),
            ),
        ),
    ])
}

fn solver_storage_batch(t: &NormalizedSolverTables) -> Result<RecordBatch, ArrowError> {
    batch(vec![
        (
            "index",
            i64s(t.storage.iter().map(|x| usz(x.index)).collect()),
        ),
        (
            "source_row",
            i64s(t.storage.iter().map(|x| opt_usz(x.source_row)).collect()),
        ),
        (
            "bus_index",
            i64s(t.storage.iter().map(|x| usz(x.bus_index)).collect()),
        ),
        ("ps", f64s(t.storage.iter().map(|x| x.ps).collect())),
        ("qs", f64s(t.storage.iter().map(|x| x.qs).collect())),
        ("energy", f64s(t.storage.iter().map(|x| x.energy).collect())),
        (
            "energy_rating",
            f64s(t.storage.iter().map(|x| x.energy_rating).collect()),
        ),
        (
            "charge_rating",
            f64s(t.storage.iter().map(|x| x.charge_rating).collect()),
        ),
        (
            "discharge_rating",
            f64s(t.storage.iter().map(|x| x.discharge_rating).collect()),
        ),
        (
            "thermal_rating",
            f64s(t.storage.iter().map(|x| x.thermal_rating).collect()),
        ),
        ("qmin", f64s(t.storage.iter().map(|x| x.qmin).collect())),
        ("qmax", f64s(t.storage.iter().map(|x| x.qmax).collect())),
        ("r", f64s(t.storage.iter().map(|x| x.r).collect())),
        ("x", f64s(t.storage.iter().map(|x| x.x).collect())),
        ("p_loss", f64s(t.storage.iter().map(|x| x.p_loss).collect())),
        ("q_loss", f64s(t.storage.iter().map(|x| x.q_loss).collect())),
    ])
}

fn solver_hvdc_batch(t: &NormalizedSolverTables) -> Result<RecordBatch, ArrowError> {
    batch(vec![
        ("index", i64s(t.hvdc.iter().map(|x| usz(x.index)).collect())),
        (
            "source_row",
            i64s(t.hvdc.iter().map(|x| opt_usz(x.source_row)).collect()),
        ),
        (
            "from_bus_index",
            i64s(t.hvdc.iter().map(|x| usz(x.from_bus_index)).collect()),
        ),
        (
            "to_bus_index",
            i64s(t.hvdc.iter().map(|x| usz(x.to_bus_index)).collect()),
        ),
        ("pf", f64s(t.hvdc.iter().map(|x| x.pf).collect())),
        ("pt", f64s(t.hvdc.iter().map(|x| x.pt).collect())),
        ("qf", f64s(t.hvdc.iter().map(|x| x.qf).collect())),
        ("qt", f64s(t.hvdc.iter().map(|x| x.qt).collect())),
        ("vf", f64s(t.hvdc.iter().map(|x| x.vf).collect())),
        ("vt", f64s(t.hvdc.iter().map(|x| x.vt).collect())),
        ("pmin", f64s(t.hvdc.iter().map(|x| x.pmin).collect())),
        ("pmax", f64s(t.hvdc.iter().map(|x| x.pmax).collect())),
        ("qminf", f64s(t.hvdc.iter().map(|x| x.qminf).collect())),
        ("qmaxf", f64s(t.hvdc.iter().map(|x| x.qmaxf).collect())),
        ("qmint", f64s(t.hvdc.iter().map(|x| x.qmint).collect())),
        ("qmaxt", f64s(t.hvdc.iter().map(|x| x.qmaxt).collect())),
        ("loss0", f64s(t.hvdc.iter().map(|x| x.loss0).collect())),
        ("loss1", f64s(t.hvdc.iter().map(|x| x.loss1).collect())),
    ])
}

#[cfg(feature = "matrix")]
macro_rules! real_matrix_batch {
    ($table_name:expr, $matrix:expr) => {{
        let matrix = $matrix;
        let mut row_index = Vec::with_capacity(matrix.nnz());
        let mut col_index = Vec::with_capacity(matrix.nnz());
        let mut value = Vec::with_capacity(matrix.nnz());
        for (row, vec) in matrix.outer_iterator().enumerate() {
            for (col, &entry) in vec.iter() {
                row_index.push(usz(row));
                col_index.push(usz(col));
                value.push(entry);
            }
        }
        matrix_real_batch(
            $table_name,
            matrix.rows(),
            matrix.cols(),
            row_index,
            col_index,
            value,
        )
    }};
}

#[cfg(feature = "matrix")]
fn matrix_ybus_batch(net: &Network, core: &IndexCore) -> Result<RecordBatch, String> {
    let view = IndexedNetwork::with_core(net, core);
    let parts = powerio_matrix::build_ybus(&view, &powerio_matrix::BuildOptions::default())
        .map_err(|e| e.to_string())?;
    let mut entries: BTreeMap<(usize, usize), (f64, f64)> = BTreeMap::new();
    for (row, vec) in parts.g.outer_iterator().enumerate() {
        for (col, &value) in vec.iter() {
            entries.entry((row, col)).or_default().0 = value;
        }
    }
    for (row, vec) in parts.b.outer_iterator().enumerate() {
        for (col, &value) in vec.iter() {
            entries.entry((row, col)).or_default().1 = value;
        }
    }

    let mut row_index = Vec::with_capacity(entries.len());
    let mut col_index = Vec::with_capacity(entries.len());
    let mut g = Vec::with_capacity(entries.len());
    let mut b = Vec::with_capacity(entries.len());
    for ((row, col), (g_value, b_value)) in entries {
        row_index.push(usz(row));
        col_index.push(usz(col));
        g.push(g_value);
        b.push(b_value);
    }
    matrix_ybus_record_batch(parts.g.rows(), parts.g.cols(), row_index, col_index, g, b)
        .map_err(|e| e.to_string())
}

#[cfg(not(feature = "matrix"))]
fn matrix_ybus_batch(_net: &Network, _core: &IndexCore) -> Result<RecordBatch, String> {
    Err(matrix_feature_error())
}

#[cfg(feature = "matrix")]
fn matrix_incidence_batch(net: &Network, core: &IndexCore) -> Result<RecordBatch, String> {
    let view = IndexedNetwork::with_core(net, core);
    let parts = powerio_matrix::build_incidence(
        &view,
        powerio_matrix::DcConvention::PaperPure,
        &powerio_matrix::BuildOptions::default(),
    )
    .map_err(|e| e.to_string())?;
    real_matrix_batch!("incidence", parts.a).map_err(|e| e.to_string())
}

#[cfg(not(feature = "matrix"))]
fn matrix_incidence_batch(_net: &Network, _core: &IndexCore) -> Result<RecordBatch, String> {
    Err(matrix_feature_error())
}

#[cfg(feature = "matrix")]
fn matrix_bprime_batch(net: &Network, core: &IndexCore) -> Result<RecordBatch, String> {
    let view = IndexedNetwork::with_core(net, core);
    let matrix = powerio_matrix::build_bprime(&view, &powerio_matrix::BuildOptions::default())
        .map_err(|e| e.to_string())?;
    real_matrix_batch!("bprime", matrix).map_err(|e| e.to_string())
}

#[cfg(not(feature = "matrix"))]
fn matrix_bprime_batch(_net: &Network, _core: &IndexCore) -> Result<RecordBatch, String> {
    Err(matrix_feature_error())
}

#[cfg(feature = "matrix")]
fn matrix_bdoubleprime_batch(net: &Network, core: &IndexCore) -> Result<RecordBatch, String> {
    let view = IndexedNetwork::with_core(net, core);
    let matrix =
        powerio_matrix::build_bdoubleprime(&view, &powerio_matrix::BuildOptions::default())
            .map_err(|e| e.to_string())?;
    real_matrix_batch!("bdoubleprime", matrix).map_err(|e| e.to_string())
}

#[cfg(not(feature = "matrix"))]
fn matrix_bdoubleprime_batch(_net: &Network, _core: &IndexCore) -> Result<RecordBatch, String> {
    Err(matrix_feature_error())
}

#[cfg(not(feature = "matrix"))]
fn matrix_feature_error() -> String {
    "matrix Arrow tables require the matrix cargo feature".to_owned()
}

#[cfg(feature = "matrix")]
fn matrix_real_batch(
    table: &str,
    rows: usize,
    cols: usize,
    row_index: Vec<i64>,
    col_index: Vec<i64>,
    value: Vec<f64>,
) -> Result<RecordBatch, ArrowError> {
    batch_with_metadata(
        vec![
            ("row_index", i64s(row_index)),
            ("col_index", i64s(col_index)),
            ("value", f64s(value)),
        ],
        matrix_metadata(table, rows, cols),
    )
}

#[cfg(feature = "matrix")]
fn matrix_ybus_record_batch(
    rows: usize,
    cols: usize,
    row_index: Vec<i64>,
    col_index: Vec<i64>,
    g: Vec<f64>,
    b: Vec<f64>,
) -> Result<RecordBatch, ArrowError> {
    batch_with_metadata(
        vec![
            ("row_index", i64s(row_index)),
            ("col_index", i64s(col_index)),
            ("g", f64s(g)),
            ("b", f64s(b)),
        ],
        matrix_metadata("ybus", rows, cols),
    )
}

fn batch(cols: Vec<(&str, ArrayRef)>) -> Result<RecordBatch, ArrowError> {
    batch_with_metadata(cols, HashMap::new())
}

fn batch_with_metadata(
    cols: Vec<(&str, ArrayRef)>,
    metadata: HashMap<String, String>,
) -> Result<RecordBatch, ArrowError> {
    let fields: Vec<Field> = cols
        .iter()
        .map(|(name, arr)| Field::new(*name, arr.data_type().clone(), false))
        .collect();
    let arrays: Vec<ArrayRef> = cols.into_iter().map(|(_, arr)| arr).collect();
    RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(fields, metadata)),
        arrays,
    )
}

#[cfg(feature = "matrix")]
fn matrix_metadata(table: &str, rows: usize, cols: usize) -> HashMap<String, String> {
    HashMap::from([
        ("powerio.table".to_owned(), table.to_owned()),
        ("powerio.index_space".to_owned(), "solver_bus".to_owned()),
        ("powerio.row_count".to_owned(), rows.to_string()),
        ("powerio.col_count".to_owned(), cols.to_string()),
    ])
}

/// External bus id as i64 (`-1` if it somehow overflows), matching `pio_branches`.
fn ext(id: BusId) -> i64 {
    i64::try_from(id.0).unwrap_or(-1)
}

fn usz(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(-1)
}

fn opt_usz(n: Option<usize>) -> i64 {
    n.map_or(-1, usz)
}

fn i64s(v: Vec<i64>) -> ArrayRef {
    Arc::new(Int64Array::from(v))
}

fn f64s(v: Vec<f64>) -> ArrayRef {
    Arc::new(Float64Array::from(v))
}

fn u8s(v: Vec<u8>) -> ArrayRef {
    Arc::new(UInt8Array::from(v))
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::ffi::from_ffi;

    fn net(name: &str) -> Network {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/data")
            .join(name);
        powerio::parse_file(&path, None).unwrap().network
    }

    fn terminal_projection_net() -> Network {
        use powerio::{Branch, BranchCharging, Bus, BusId, BusType};

        let mut branch = Branch::new(BusId(1), BusId(2), 0.01, 0.1);
        branch.charging = Some(BranchCharging::new(0.01, 0.02, 0.03, 0.05));
        branch.rate_a = 100.0;
        Network::in_memory(
            "terminal-projection",
            100.0,
            vec![
                Bus::new(BusId(1), BusType::Ref, 230.0),
                Bus::new(BusId(2), BusType::Pq, 230.0),
            ],
            vec![branch],
        )
    }

    fn round_trip(net: &Network, table: i32) -> StructArray {
        let core = IndexCore::build(net);
        let (array, schema) = export(net, &core, table).unwrap();
        // from_ffi consumes the array and borrows the schema (zero-copy import).
        let data = unsafe { from_ffi(array, &schema) }.unwrap();
        StructArray::from(data)
    }

    fn f64_col<'a>(sa: &'a StructArray, name: &str) -> &'a Float64Array {
        sa.column_by_name(name)
            .unwrap()
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
    }

    fn i64_col<'a>(sa: &'a StructArray, name: &str) -> &'a Int64Array {
        sa.column_by_name(name)
            .unwrap()
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap()
    }

    #[cfg(feature = "matrix")]
    fn rb_f64_col<'a>(rb: &'a RecordBatch, name: &str) -> &'a Float64Array {
        rb.column_by_name(name)
            .unwrap()
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
    }

    #[cfg(feature = "matrix")]
    fn rb_i64_col<'a>(rb: &'a RecordBatch, name: &str) -> &'a Int64Array {
        rb.column_by_name(name)
            .unwrap()
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap()
    }

    #[cfg(feature = "matrix")]
    fn matrix_record_batch(net: &Network, table: i32) -> RecordBatch {
        let core = IndexCore::build(net);
        match table {
            PIO_ARROW_TABLE_YBUS => matrix_ybus_batch(net, &core).unwrap(),
            PIO_ARROW_TABLE_INCIDENCE => matrix_incidence_batch(net, &core).unwrap(),
            PIO_ARROW_TABLE_BPRIME => matrix_bprime_batch(net, &core).unwrap(),
            PIO_ARROW_TABLE_BDOUBLEPRIME => matrix_bdoubleprime_batch(net, &core).unwrap(),
            _ => panic!("not a matrix table id: {table}"),
        }
    }

    #[cfg(feature = "matrix")]
    fn f64_bits(values: &Float64Array) -> Vec<String> {
        values
            .values()
            .iter()
            .map(|value| format!("0x{:016x}", value.to_bits()))
            .collect()
    }

    #[cfg(feature = "matrix")]
    fn matrix_table_json(table_name: &str, rb: &RecordBatch) -> serde_json::Value {
        let metadata = rb.schema();
        let metadata = metadata.metadata();
        let mut obj = serde_json::Map::new();
        obj.insert("table".to_owned(), serde_json::json!(table_name));
        obj.insert(
            "row_count".to_owned(),
            serde_json::json!(
                metadata
                    .get("powerio.row_count")
                    .unwrap()
                    .parse::<usize>()
                    .unwrap()
            ),
        );
        obj.insert(
            "col_count".to_owned(),
            serde_json::json!(
                metadata
                    .get("powerio.col_count")
                    .unwrap()
                    .parse::<usize>()
                    .unwrap()
            ),
        );
        obj.insert(
            "row_index".to_owned(),
            serde_json::json!(rb_i64_col(rb, "row_index").values().to_vec()),
        );
        obj.insert(
            "col_index".to_owned(),
            serde_json::json!(rb_i64_col(rb, "col_index").values().to_vec()),
        );
        if table_name == "ybus" {
            obj.insert(
                "g_bits".to_owned(),
                serde_json::json!(f64_bits(rb_f64_col(rb, "g"))),
            );
            obj.insert(
                "b_bits".to_owned(),
                serde_json::json!(f64_bits(rb_f64_col(rb, "b"))),
            );
        } else {
            obj.insert(
                "value_bits".to_owned(),
                serde_json::json!(f64_bits(rb_f64_col(rb, "value"))),
            );
        }
        serde_json::Value::Object(obj)
    }

    #[cfg(feature = "matrix")]
    fn matrix_golden_json(case_file: &str) -> serde_json::Value {
        let n = net(case_file);
        let tables = [
            ("ybus", PIO_ARROW_TABLE_YBUS),
            ("incidence", PIO_ARROW_TABLE_INCIDENCE),
            ("bprime", PIO_ARROW_TABLE_BPRIME),
            ("bdoubleprime", PIO_ARROW_TABLE_BDOUBLEPRIME),
        ];
        let mut table_obj = serde_json::Map::new();
        for (name, table) in tables {
            let rb = matrix_record_batch(&n, table);
            table_obj.insert(name.to_owned(), matrix_table_json(name, &rb));
        }
        serde_json::json!({
            "case": case_file,
            "tables": table_obj,
        })
    }

    #[cfg(feature = "matrix")]
    fn assert_ffi_matches_record_batch(table: i32, value_cols: &[&str]) {
        let n = net("case9.m");
        let rb = matrix_record_batch(&n, table);
        let sa = round_trip(&n, table);

        assert_eq!(sa.len(), rb.num_rows());
        assert_eq!(
            i64_col(&sa, "row_index").values(),
            rb_i64_col(&rb, "row_index").values()
        );
        assert_eq!(
            i64_col(&sa, "col_index").values(),
            rb_i64_col(&rb, "col_index").values()
        );
        for &name in value_cols {
            assert_eq!(
                f64_bits(f64_col(&sa, name)),
                f64_bits(rb_f64_col(&rb, name)),
                "{name} column changed through FFI"
            );
        }
    }

    #[test]
    fn bus_table_round_trips_with_external_ids() {
        let n = net("case9.m");
        let sa = round_trip(&n, PIO_ARROW_TABLE_BUS);
        assert_eq!(sa.len(), n.buses.len());
        let ids = sa
            .column_by_name("id")
            .unwrap()
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        // The whole id column survives, in order (a reversed/offset column would
        // pass a single-cell check).
        let expected: Vec<i64> = n
            .buses
            .iter()
            .map(|b| i64::try_from(b.id.0).unwrap())
            .collect();
        assert_eq!(ids.values(), expected.as_slice());
    }

    #[test]
    fn empty_table_exports_zero_rows() {
        // case9 has no shunts: a length-0 table must cross the C Data Interface
        // and import back without faulting (a common producer mishandling).
        let n = net("case9.m");
        assert_eq!(n.shunts.len(), 0);
        assert_eq!(round_trip(&n, PIO_ARROW_TABLE_SHUNT).len(), 0);
    }

    #[test]
    fn every_table_has_the_expected_row_count() {
        // case30 carries buses, branches, gens, loads, and shunts.
        let n = net("case30.m");
        assert_eq!(round_trip(&n, PIO_ARROW_TABLE_BUS).len(), n.buses.len());
        assert_eq!(
            round_trip(&n, PIO_ARROW_TABLE_BRANCH).len(),
            n.branches.len()
        );
        assert_eq!(
            round_trip(&n, PIO_ARROW_TABLE_GEN).len(),
            n.generators.len()
        );
        assert_eq!(round_trip(&n, PIO_ARROW_TABLE_LOAD).len(), n.loads.len());
        assert_eq!(round_trip(&n, PIO_ARROW_TABLE_SHUNT).len(), n.shunts.len());
    }

    #[test]
    fn normalized_solver_tables_export_dense_per_unit_rows() {
        let n = net("case14.m");
        let tables = n.to_normalized_solver_tables().unwrap();

        assert_eq!(
            round_trip(&n, PIO_ARROW_TABLE_SOLVER_BUS).len(),
            tables.buses.len()
        );
        assert_eq!(
            round_trip(&n, PIO_ARROW_TABLE_SOLVER_BRANCH).len(),
            tables.branches.len()
        );
        assert_eq!(
            round_trip(&n, PIO_ARROW_TABLE_SOLVER_ARC).len(),
            tables.arcs.len()
        );
        assert_eq!(
            round_trip(&n, PIO_ARROW_TABLE_SOLVER_GEN).len(),
            tables.generators.len()
        );

        let bus = round_trip(&n, PIO_ARROW_TABLE_SOLVER_BUS);
        assert_eq!(i64_col(&bus, "index").value(1), 1);
        assert_eq!(i64_col(&bus, "bus_id").value(1), 2);
        assert_eq!(i64_col(&bus, "source_row").value(1), 1);
        assert!((f64_col(&bus, "pd").value(1) - 21.7 / 100.0).abs() < 1e-12);

        let branch = round_trip(&n, PIO_ARROW_TABLE_SOLVER_BRANCH);
        assert_eq!(i64_col(&branch, "from_bus_index").value(0), 0);
        assert_eq!(i64_col(&branch, "to_bus_index").value(0), 1);

        let arc = round_trip(&n, PIO_ARROW_TABLE_SOLVER_ARC);
        assert_eq!(i64_col(&arc, "branch_index").value(0), 0);
        assert_eq!(i64_col(&arc, "terminal").value(0), 0);
        assert_eq!(i64_col(&arc, "branch_index").value(1), 0);
        assert_eq!(i64_col(&arc, "terminal").value(1), 1);
    }

    #[test]
    fn branch_table_b_is_legacy_projection() {
        let n = terminal_projection_net();
        let sa = round_trip(&n, PIO_ARROW_TABLE_BRANCH);
        assert_eq!(sa.len(), 1);
        assert!((f64_col(&sa, "b").value(0) - 0.07).abs() < 1e-12);
        assert!((f64_col(&sa, "g_fr").value(0) - 0.01).abs() < 1e-12);
        assert!((f64_col(&sa, "b_fr").value(0) - 0.02).abs() < 1e-12);
        assert!((f64_col(&sa, "g_to").value(0) - 0.03).abs() < 1e-12);
        assert!((f64_col(&sa, "b_to").value(0) - 0.05).abs() < 1e-12);
    }

    #[test]
    fn unknown_table_id_errors() {
        let n = net("case9.m");
        let core = IndexCore::build(&n);
        assert!(export(&n, &core, 99).is_err());
    }

    #[cfg(not(feature = "matrix"))]
    #[test]
    fn matrix_table_requires_matrix_feature() {
        let n = net("case9.m");
        let core = IndexCore::build(&n);
        let err = export(&n, &core, PIO_ARROW_TABLE_BPRIME).unwrap_err();
        assert!(err.contains("matrix cargo feature"), "{err}");
    }

    #[cfg(feature = "matrix")]
    #[test]
    fn matrix_tables_round_trip_through_ffi() {
        assert_ffi_matches_record_batch(PIO_ARROW_TABLE_YBUS, &["g", "b"]);
        assert_ffi_matches_record_batch(PIO_ARROW_TABLE_INCIDENCE, &["value"]);
        assert_ffi_matches_record_batch(PIO_ARROW_TABLE_BPRIME, &["value"]);
        assert_ffi_matches_record_batch(PIO_ARROW_TABLE_BDOUBLEPRIME, &["value"]);
    }

    #[cfg(feature = "matrix")]
    #[test]
    fn matrix_tables_carry_schema_dimensions() {
        let n = net("case9.m");
        let rb = matrix_record_batch(&n, PIO_ARROW_TABLE_BPRIME);
        let metadata = rb.schema();
        let metadata = metadata.metadata();
        assert_eq!(metadata.get("powerio.table").unwrap(), "bprime");
        assert_eq!(metadata.get("powerio.index_space").unwrap(), "solver_bus");
        assert_eq!(metadata.get("powerio.row_count").unwrap(), "9");
        assert_eq!(metadata.get("powerio.col_count").unwrap(), "9");

        let core = IndexCore::build(&n);
        let (array, schema) = export(&n, &core, PIO_ARROW_TABLE_BPRIME).unwrap();
        let imported_schema = Schema::try_from(&schema).unwrap();
        let metadata = imported_schema.metadata();
        assert_eq!(metadata.get("powerio.table").unwrap(), "bprime");
        assert_eq!(metadata.get("powerio.index_space").unwrap(), "solver_bus");
        assert_eq!(metadata.get("powerio.row_count").unwrap(), "9");
        assert_eq!(metadata.get("powerio.col_count").unwrap(), "9");
        let _data = unsafe { from_ffi(array, &schema) }.unwrap();
    }

    #[cfg(feature = "matrix")]
    #[test]
    fn matrix_arrow_golden_fixtures_match() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data/capi_matrix");
        for case_file in ["case9.m", "case30.m"] {
            let fixture = dir.join(case_file.replace(".m", "_arrow_coo.json"));
            let expected: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&fixture).unwrap()).unwrap();
            assert_eq!(matrix_golden_json(case_file), expected, "{case_file}");
        }
    }

    #[cfg(feature = "matrix")]
    #[ignore = "rewrites committed matrix Arrow COO fixtures"]
    #[test]
    fn rewrite_matrix_arrow_golden_fixtures() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data/capi_matrix");
        std::fs::create_dir_all(&dir).unwrap();
        for case_file in ["case9.m", "case30.m"] {
            let fixture = dir.join(case_file.replace(".m", "_arrow_coo.json"));
            let text = serde_json::to_string_pretty(&matrix_golden_json(case_file)).unwrap();
            std::fs::write(fixture, format!("{text}\n")).unwrap();
        }
    }
}
