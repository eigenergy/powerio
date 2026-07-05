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
//! normalized solver table rules: per unit/radian values and dense zero based
//! row ids. Matrix table ids after that carry COO triplets in the same dense bus
//! index space, with matrix dimensions stored in Arrow schema metadata.

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
pub const PIO_ARROW_TABLE_MATRIX_BUS: i32 = 19;
pub const PIO_ARROW_TABLE_MATRIX_BRANCH: i32 = 20;

const ARROW_SCHEMA_VERSION: &str = "1";

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
        && PIO_ARROW_TABLE_MATRIX_BUS == 19
        && PIO_ARROW_TABLE_MATRIX_BRANCH == 20
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
        PIO_ARROW_TABLE_MATRIX_BUS => matrix_bus_batch(net, core)?,
        PIO_ARROW_TABLE_MATRIX_BRANCH => matrix_branch_batch(net, core)?,
        other => return Err(format!("unknown Arrow table id {other}")),
    };

    // The C Data Interface represents a record batch as a struct array. Build
    // the schema from the RecordBatch, not from ArrayData, so table metadata
    // such as matrix dimensions survives the FFI boundary.
    let schema = FFI_ArrowSchema::try_from(rb.schema().as_ref()).map_err(|e| e.to_string())?;
    let data = StructArray::from(rb).into_data();
    Ok((FFI_ArrowArray::new(&data), schema))
}

/// Return the Arrow table catalog as compact JSON.
pub fn catalog_json() -> String {
    serde_json::to_string(&catalog_value()).expect("Arrow catalog JSON is serializable")
}

fn catalog_value() -> serde_json::Value {
    let matrix_available = cfg!(feature = "matrix");
    let table_spec = |id: i32,
                      name: &str,
                      format: &str,
                      feature_requirements: &[&str],
                      available: bool,
                      row_axis: Option<&str>,
                      col_axis: Option<&str>,
                      units: serde_json::Value,
                      columns: &[(&str, &str)]| {
        serde_json::json!({
            "id": id,
            "name": name,
            "schema_version": ARROW_SCHEMA_VERSION,
            "format": format,
            "feature_requirements": feature_requirements,
            "available": available,
            "row_axis": row_axis,
            "col_axis": col_axis,
            "units": units,
            "columns": columns.iter().map(|(name, dtype)| {
                serde_json::json!({"name": name, "type": dtype, "nullable": false})
            }).collect::<Vec<_>>(),
        })
    };
    serde_json::json!({
        "schema_version": ARROW_SCHEMA_VERSION,
        "producer": "powerio-capi",
        "tables": [
            table_spec(PIO_ARROW_TABLE_BUS, "bus", "record_batch", &["arrow"], true, None, None, units_source(), &[
                ("id", "int64"), ("kind", "int64"), ("vm", "float64"), ("va", "float64"),
                ("base_kv", "float64"), ("vmax", "float64"), ("vmin", "float64"),
                ("area", "int64"), ("zone", "int64"),
            ]),
            table_spec(PIO_ARROW_TABLE_BRANCH, "branch", "record_batch", &["arrow"], true, None, None, units_source(), &[
                ("from", "int64"), ("to", "int64"), ("r", "float64"), ("x", "float64"),
                ("b", "float64"), ("rate_a", "float64"), ("rate_b", "float64"),
                ("rate_c", "float64"), ("tap", "float64"), ("shift", "float64"),
                ("in_service", "uint8"), ("angmin", "float64"), ("angmax", "float64"),
                ("g_fr", "float64"), ("b_fr", "float64"), ("g_to", "float64"),
                ("b_to", "float64"), ("c_rating_a", "float64"), ("c_rating_b", "float64"),
                ("c_rating_c", "float64"), ("pf", "float64"), ("qf", "float64"),
                ("pt", "float64"), ("qt", "float64"),
            ]),
            table_spec(PIO_ARROW_TABLE_GEN, "gen", "record_batch", &["arrow"], true, None, None, units_source(), &[
                ("bus", "int64"), ("pg", "float64"), ("qg", "float64"),
                ("pmax", "float64"), ("pmin", "float64"), ("qmax", "float64"),
                ("qmin", "float64"), ("vg", "float64"), ("mbase", "float64"),
                ("in_service", "uint8"),
            ]),
            table_spec(PIO_ARROW_TABLE_LOAD, "load", "record_batch", &["arrow"], true, None, None, units_source(), &[
                ("bus", "int64"), ("p", "float64"), ("q", "float64"), ("in_service", "uint8"),
            ]),
            table_spec(PIO_ARROW_TABLE_SHUNT, "shunt", "record_batch", &["arrow"], true, None, None, units_source(), &[
                ("bus", "int64"), ("g", "float64"), ("b", "float64"), ("in_service", "uint8"),
            ]),
            table_spec(PIO_ARROW_TABLE_SWITCH, "switch", "record_batch", &["arrow"], true, None, None, units_source(), &[
                ("from", "int64"), ("to", "int64"), ("closed", "uint8"),
                ("thermal_rating", "float64"), ("current_rating", "float64"),
                ("pf", "float64"), ("qf", "float64"), ("pt", "float64"), ("qt", "float64"),
            ]),
            table_spec(PIO_ARROW_TABLE_SOLVER_BUS, "solver_bus", "record_batch", &["arrow"], true, Some("solver_bus"), None, units_solver(), &[
                ("index", "int64"), ("bus_id", "int64"), ("source_row", "int64"),
                ("kind", "int64"), ("vm", "float64"), ("va", "float64"),
                ("base_kv", "float64"), ("vmax", "float64"), ("vmin", "float64"),
                ("pd", "float64"), ("qd", "float64"), ("gs", "float64"),
                ("bs", "float64"), ("component_label", "int64"), ("is_reference", "uint8"),
            ]),
            table_spec(PIO_ARROW_TABLE_SOLVER_LOAD, "solver_load", "record_batch", &["arrow"], true, Some("solver_load"), None, units_solver(), &[
                ("index", "int64"), ("source_row", "int64"), ("bus_index", "int64"),
                ("p", "float64"), ("q", "float64"),
            ]),
            table_spec(PIO_ARROW_TABLE_SOLVER_SHUNT, "solver_shunt", "record_batch", &["arrow"], true, Some("solver_shunt"), None, units_solver(), &[
                ("index", "int64"), ("source_row", "int64"), ("bus_index", "int64"),
                ("g", "float64"), ("b", "float64"),
            ]),
            table_spec(PIO_ARROW_TABLE_SOLVER_BRANCH, "solver_branch", "record_batch", &["arrow"], true, Some("solver_branch"), None, units_solver(), &[
                ("index", "int64"), ("source_row", "int64"), ("from_bus_index", "int64"),
                ("to_bus_index", "int64"), ("r", "float64"), ("x", "float64"),
                ("b", "float64"), ("g_fr", "float64"), ("b_fr", "float64"),
                ("g_to", "float64"), ("b_to", "float64"), ("rate_a", "float64"),
                ("rate_b", "float64"), ("rate_c", "float64"), ("tap", "float64"),
                ("shift", "float64"), ("angmin", "float64"), ("angmax", "float64"),
            ]),
            table_spec(PIO_ARROW_TABLE_SOLVER_SWITCH, "solver_switch", "record_batch", &["arrow"], true, Some("solver_switch"), None, units_solver(), &[
                ("index", "int64"), ("source_row", "int64"), ("from_bus_index", "int64"),
                ("to_bus_index", "int64"), ("closed", "uint8"), ("thermal_rating", "float64"),
                ("current_rating", "float64"), ("pf", "float64"), ("qf", "float64"),
                ("pt", "float64"), ("qt", "float64"),
            ]),
            table_spec(PIO_ARROW_TABLE_SOLVER_ARC, "solver_arc", "record_batch", &["arrow"], true, Some("solver_arc"), None, units_solver(), &[
                ("index", "int64"), ("branch_index", "int64"), ("terminal", "int64"),
                ("from_bus_index", "int64"), ("to_bus_index", "int64"), ("tap", "float64"),
                ("shift", "float64"), ("g_shunt", "float64"), ("b_shunt", "float64"),
                ("rate_a", "float64"),
            ]),
            table_spec(PIO_ARROW_TABLE_SOLVER_GEN, "solver_gen", "record_batch", &["arrow"], true, Some("solver_gen"), None, units_solver(), &[
                ("index", "int64"), ("source_row", "int64"), ("bus_index", "int64"),
                ("pg", "float64"), ("qg", "float64"), ("pmax", "float64"),
                ("pmin", "float64"), ("qmax", "float64"), ("qmin", "float64"),
                ("vg", "float64"), ("mbase", "float64"), ("regulated_bus_index", "int64"),
            ]),
            table_spec(PIO_ARROW_TABLE_SOLVER_STORAGE, "solver_storage", "record_batch", &["arrow"], true, Some("solver_storage"), None, units_solver(), &[
                ("index", "int64"), ("source_row", "int64"), ("bus_index", "int64"),
                ("ps", "float64"), ("qs", "float64"), ("energy", "float64"),
                ("energy_rating", "float64"), ("charge_rating", "float64"),
                ("discharge_rating", "float64"), ("thermal_rating", "float64"),
                ("qmin", "float64"), ("qmax", "float64"), ("r", "float64"),
                ("x", "float64"), ("p_loss", "float64"), ("q_loss", "float64"),
            ]),
            table_spec(PIO_ARROW_TABLE_SOLVER_HVDC, "solver_hvdc", "record_batch", &["arrow"], true, Some("solver_hvdc"), None, units_solver(), &[
                ("index", "int64"), ("source_row", "int64"), ("from_bus_index", "int64"),
                ("to_bus_index", "int64"), ("pf", "float64"), ("pt", "float64"),
                ("qf", "float64"), ("qt", "float64"), ("vf", "float64"), ("vt", "float64"),
                ("pmin", "float64"), ("pmax", "float64"), ("qminf", "float64"),
                ("qmaxf", "float64"), ("qmint", "float64"), ("qmaxt", "float64"),
                ("loss0", "float64"), ("loss1", "float64"),
            ]),
            table_spec(PIO_ARROW_TABLE_YBUS, "ybus", "coo", &["arrow", "matrix"], matrix_available, Some("matrix_bus"), Some("matrix_bus"), units_matrix(), &[
                ("row_index", "int64"), ("col_index", "int64"), ("g", "float64"), ("b", "float64"),
            ]),
            table_spec(PIO_ARROW_TABLE_INCIDENCE, "incidence", "coo", &["arrow", "matrix"], matrix_available, Some("matrix_bus"), Some("matrix_branch"), units_matrix(), &[
                ("row_index", "int64"), ("col_index", "int64"), ("value", "float64"),
            ]),
            table_spec(PIO_ARROW_TABLE_BPRIME, "bprime", "coo", &["arrow", "matrix"], matrix_available, Some("matrix_bus"), Some("matrix_bus"), units_matrix(), &[
                ("row_index", "int64"), ("col_index", "int64"), ("value", "float64"),
            ]),
            table_spec(PIO_ARROW_TABLE_BDOUBLEPRIME, "bdoubleprime", "coo", &["arrow", "matrix"], matrix_available, Some("matrix_bus"), Some("matrix_bus"), units_matrix(), &[
                ("row_index", "int64"), ("col_index", "int64"), ("value", "float64"),
            ]),
            table_spec(PIO_ARROW_TABLE_MATRIX_BUS, "matrix_bus", "axis_map", &["arrow", "matrix"], matrix_available, Some("matrix_bus"), None, units_axis(), &[
                ("index", "int64"), ("bus_id", "int64"), ("source_row", "int64"),
                ("is_reference", "uint8"), ("component", "int64"),
            ]),
            table_spec(PIO_ARROW_TABLE_MATRIX_BRANCH, "matrix_branch", "axis_map", &["arrow", "matrix"], matrix_available, Some("matrix_branch"), None, units_axis(), &[
                ("index", "int64"), ("source_row", "int64"), ("from_bus_id", "int64"),
                ("to_bus_id", "int64"),
            ]),
        ]
    })
}

fn units_source() -> serde_json::Value {
    serde_json::json!({
        "power": "source",
        "voltage": "source",
        "angle": "degree",
        "index_base": "external_bus_id"
    })
}

fn units_solver() -> serde_json::Value {
    serde_json::json!({
        "power": "per_unit",
        "voltage": "per_unit",
        "angle": "radian",
        "impedance": "per_unit",
        "admittance": "per_unit",
        "index_base": "zero"
    })
}

fn units_matrix() -> serde_json::Value {
    serde_json::json!({
        "matrix_index_base": "zero",
        "value": "per_unit"
    })
}

fn units_axis() -> serde_json::Value {
    serde_json::json!({
        "index_base": "zero",
        "source_row_base": "zero",
        "missing_source_row": -1
    })
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
    ($table_name:expr, $matrix:expr, $row_axis:expr, $col_axis:expr) => {{
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
            MatrixShape {
                rows: matrix.rows(),
                cols: matrix.cols(),
            },
            row_index,
            col_index,
            value,
            MatrixAxes {
                row: $row_axis,
                col: $col_axis,
            },
        )
    }};
}

#[cfg(feature = "matrix")]
fn matrix_bus_batch(net: &Network, core: &IndexCore) -> Result<RecordBatch, String> {
    let view = IndexedNetwork::with_core(net, core);
    let refs = view.reference_bus_indices();
    let components = view.connected_component_labels();
    let source_rows: HashMap<BusId, usize> = net
        .buses
        .iter()
        .enumerate()
        .map(|(idx, bus)| (bus.id, idx))
        .collect();

    let buses = &view.network().buses;
    batch_with_metadata(
        vec![
            ("index", i64s((0..buses.len()).map(usz).collect::<Vec<_>>())),
            (
                "bus_id",
                i64s(buses.iter().map(|bus| ext(bus.id)).collect()),
            ),
            (
                "source_row",
                i64s(
                    buses
                        .iter()
                        .map(|bus| source_rows.get(&bus.id).copied().map_or(-1, usz))
                        .collect(),
                ),
            ),
            (
                "is_reference",
                u8s((0..buses.len())
                    .map(|idx| u8::from(refs.contains(&idx)))
                    .collect()),
            ),
            (
                "component",
                i64s(components.iter().map(|&label| usz(label)).collect()),
            ),
        ],
        axis_metadata("matrix_bus"),
    )
    .map_err(|e| e.to_string())
}

#[cfg(not(feature = "matrix"))]
fn matrix_bus_batch(_net: &Network, _core: &IndexCore) -> Result<RecordBatch, String> {
    Err(matrix_feature_error())
}

#[cfg(feature = "matrix")]
fn matrix_branch_batch(net: &Network, core: &IndexCore) -> Result<RecordBatch, String> {
    let view = IndexedNetwork::with_core(net, core);
    let mut branch_cols = Vec::new();
    for (idx, br) in view.in_service_branches() {
        let i = view
            .bus_index(br.from)
            .ok_or_else(|| format!("unknown from bus {} on branch row {idx}", br.from.0))?;
        let j = view
            .bus_index(br.to)
            .ok_or_else(|| format!("unknown to bus {} on branch row {idx}", br.to.0))?;
        if i == j || br.x == 0.0 {
            continue;
        }
        let b_e = 1.0 / br.x;
        if !b_e.is_finite() {
            return Err(format!("non-finite branch susceptance at row {idx}"));
        }
        branch_cols.push((idx, br.from, br.to));
    }

    let mut index = Vec::with_capacity(branch_cols.len());
    let mut source_row = Vec::with_capacity(branch_cols.len());
    let mut from_bus_id = Vec::with_capacity(branch_cols.len());
    let mut to_bus_id = Vec::with_capacity(branch_cols.len());
    for (col, &(idx, from, to)) in branch_cols.iter().enumerate() {
        index.push(usz(col));
        source_row.push((idx < net.branches.len()).then_some(idx).map_or(-1, usz));
        from_bus_id.push(ext(from));
        to_bus_id.push(ext(to));
    }

    batch_with_metadata(
        vec![
            ("index", i64s(index)),
            ("source_row", i64s(source_row)),
            ("from_bus_id", i64s(from_bus_id)),
            ("to_bus_id", i64s(to_bus_id)),
        ],
        axis_metadata("matrix_branch"),
    )
    .map_err(|e| e.to_string())
}

#[cfg(not(feature = "matrix"))]
fn matrix_branch_batch(_net: &Network, _core: &IndexCore) -> Result<RecordBatch, String> {
    Err(matrix_feature_error())
}

#[cfg(feature = "matrix")]
fn matrix_ybus_batch(net: &Network, core: &IndexCore) -> Result<RecordBatch, String> {
    let view = IndexedNetwork::with_core(net, core);
    let parts = powerio_matrix::build_ybus(&view, &powerio_matrix::BuildOptions::default())
        .map_err(|e| e.to_string())?;
    let mut cols = YbusColumns {
        row_index: Vec::with_capacity(parts.g.nnz() + parts.b.nnz()),
        col_index: Vec::with_capacity(parts.g.nnz() + parts.b.nnz()),
        g: Vec::with_capacity(parts.g.nnz() + parts.b.nnz()),
        b: Vec::with_capacity(parts.g.nnz() + parts.b.nnz()),
    };
    for row in 0..parts.g.rows() {
        match (parts.g.outer_view(row), parts.b.outer_view(row)) {
            (Some(g_row), Some(b_row)) => push_ybus_row(
                row,
                g_row.indices(),
                g_row.data(),
                b_row.indices(),
                b_row.data(),
                &mut cols,
            ),
            (Some(g_row), None) => {
                push_ybus_row(row, g_row.indices(), g_row.data(), &[], &[], &mut cols);
            }
            (None, Some(b_row)) => {
                push_ybus_row(row, &[], &[], b_row.indices(), b_row.data(), &mut cols);
            }
            (None, None) => {}
        }
    }
    matrix_ybus_record_batch(
        MatrixShape {
            rows: parts.g.rows(),
            cols: parts.g.cols(),
        },
        cols.row_index,
        cols.col_index,
        cols.g,
        cols.b,
        MatrixAxes {
            row: "matrix_bus",
            col: "matrix_bus",
        },
    )
    .map_err(|e| e.to_string())
}

#[cfg(not(feature = "matrix"))]
fn matrix_ybus_batch(_net: &Network, _core: &IndexCore) -> Result<RecordBatch, String> {
    Err(matrix_feature_error())
}

#[cfg(feature = "matrix")]
struct YbusColumns {
    row_index: Vec<i64>,
    col_index: Vec<i64>,
    g: Vec<f64>,
    b: Vec<f64>,
}

#[cfg(feature = "matrix")]
fn push_ybus_row(
    row: usize,
    g_indices: &[usize],
    g_data: &[f64],
    b_indices: &[usize],
    b_data: &[f64],
    cols: &mut YbusColumns,
) {
    let mut gi = 0;
    let mut bi = 0;
    while gi < g_indices.len() || bi < b_indices.len() {
        let (col, g_value, b_value) = match (g_indices.get(gi), b_indices.get(bi)) {
            (Some(&g_col), Some(&b_col)) => match g_col.cmp(&b_col) {
                std::cmp::Ordering::Less => {
                    gi += 1;
                    (g_col, g_data[gi - 1], 0.0)
                }
                std::cmp::Ordering::Greater => {
                    bi += 1;
                    (b_col, 0.0, b_data[bi - 1])
                }
                std::cmp::Ordering::Equal => {
                    gi += 1;
                    bi += 1;
                    (g_col, g_data[gi - 1], b_data[bi - 1])
                }
            },
            (Some(&g_col), None) => {
                gi += 1;
                (g_col, g_data[gi - 1], 0.0)
            }
            (None, Some(&b_col)) => {
                bi += 1;
                (b_col, 0.0, b_data[bi - 1])
            }
            (None, None) => unreachable!(),
        };
        cols.row_index.push(usz(row));
        cols.col_index.push(usz(col));
        cols.g.push(g_value);
        cols.b.push(b_value);
    }
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
    real_matrix_batch!("incidence", parts.a, "matrix_bus", "matrix_branch")
        .map_err(|e| e.to_string())
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
    real_matrix_batch!("bprime", matrix, "matrix_bus", "matrix_bus").map_err(|e| e.to_string())
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
    real_matrix_batch!("bdoubleprime", matrix, "matrix_bus", "matrix_bus")
        .map_err(|e| e.to_string())
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
struct MatrixShape {
    rows: usize,
    cols: usize,
}

#[cfg(feature = "matrix")]
struct MatrixAxes<'a> {
    row: &'a str,
    col: &'a str,
}

#[cfg(feature = "matrix")]
fn matrix_real_batch(
    table: &str,
    shape: MatrixShape,
    row_index: Vec<i64>,
    col_index: Vec<i64>,
    value: Vec<f64>,
    axes: MatrixAxes<'_>,
) -> Result<RecordBatch, ArrowError> {
    batch_with_metadata(
        vec![
            ("row_index", i64s(row_index)),
            ("col_index", i64s(col_index)),
            ("value", f64s(value)),
        ],
        matrix_metadata(table, shape.rows, shape.cols, axes.row, axes.col),
    )
}

#[cfg(feature = "matrix")]
fn matrix_ybus_record_batch(
    shape: MatrixShape,
    row_index: Vec<i64>,
    col_index: Vec<i64>,
    g: Vec<f64>,
    b: Vec<f64>,
    axes: MatrixAxes<'_>,
) -> Result<RecordBatch, ArrowError> {
    batch_with_metadata(
        vec![
            ("row_index", i64s(row_index)),
            ("col_index", i64s(col_index)),
            ("g", f64s(g)),
            ("b", f64s(b)),
        ],
        matrix_metadata("ybus", shape.rows, shape.cols, axes.row, axes.col),
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
fn matrix_metadata(
    table: &str,
    rows: usize,
    cols: usize,
    row_axis: &str,
    col_axis: &str,
) -> HashMap<String, String> {
    HashMap::from([
        ("powerio.table".to_owned(), table.to_owned()),
        (
            "powerio.schema_version".to_owned(),
            ARROW_SCHEMA_VERSION.to_owned(),
        ),
        ("powerio.format".to_owned(), "coo".to_owned()),
        ("powerio.index_space".to_owned(), "solver_bus".to_owned()),
        ("powerio.row_axis".to_owned(), row_axis.to_owned()),
        ("powerio.col_axis".to_owned(), col_axis.to_owned()),
        ("powerio.row_count".to_owned(), rows.to_string()),
        ("powerio.col_count".to_owned(), cols.to_string()),
    ])
}

#[cfg(feature = "matrix")]
fn axis_metadata(table: &str) -> HashMap<String, String> {
    HashMap::from([
        ("powerio.table".to_owned(), table.to_owned()),
        (
            "powerio.schema_version".to_owned(),
            ARROW_SCHEMA_VERSION.to_owned(),
        ),
        ("powerio.format".to_owned(), "axis_map".to_owned()),
        ("powerio.row_axis".to_owned(), table.to_owned()),
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
    fn u8_col<'a>(sa: &'a StructArray, name: &str) -> &'a UInt8Array {
        sa.column_by_name(name)
            .unwrap()
            .as_any()
            .downcast_ref::<UInt8Array>()
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
            PIO_ARROW_TABLE_MATRIX_BUS => matrix_bus_batch(net, &core).unwrap(),
            PIO_ARROW_TABLE_MATRIX_BRANCH => matrix_branch_batch(net, &core).unwrap(),
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
            "schema_version".to_owned(),
            serde_json::json!(metadata.get("powerio.schema_version").unwrap()),
        );
        obj.insert(
            "format".to_owned(),
            serde_json::json!(metadata.get("powerio.format").unwrap()),
        );
        obj.insert(
            "row_axis".to_owned(),
            serde_json::json!(metadata.get("powerio.row_axis").unwrap()),
        );
        obj.insert(
            "col_axis".to_owned(),
            serde_json::json!(metadata.get("powerio.col_axis").unwrap()),
        );
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
    fn axis_table_json(table_name: &str, rb: &RecordBatch) -> serde_json::Value {
        let metadata = rb.schema();
        let metadata = metadata.metadata();
        let mut obj = serde_json::Map::new();
        obj.insert("table".to_owned(), serde_json::json!(table_name));
        obj.insert(
            "schema_version".to_owned(),
            serde_json::json!(metadata.get("powerio.schema_version").unwrap()),
        );
        obj.insert(
            "format".to_owned(),
            serde_json::json!(metadata.get("powerio.format").unwrap()),
        );
        obj.insert(
            "row_axis".to_owned(),
            serde_json::json!(metadata.get("powerio.row_axis").unwrap()),
        );
        obj.insert(
            "index".to_owned(),
            serde_json::json!(rb_i64_col(rb, "index").values().to_vec()),
        );
        obj.insert(
            "source_row".to_owned(),
            serde_json::json!(rb_i64_col(rb, "source_row").values().to_vec()),
        );
        if table_name == "matrix_bus" {
            obj.insert(
                "bus_id".to_owned(),
                serde_json::json!(rb_i64_col(rb, "bus_id").values().to_vec()),
            );
            obj.insert(
                "is_reference".to_owned(),
                serde_json::json!(
                    rb.column_by_name("is_reference")
                        .unwrap()
                        .as_any()
                        .downcast_ref::<UInt8Array>()
                        .unwrap()
                        .values()
                        .to_vec()
                ),
            );
            obj.insert(
                "component".to_owned(),
                serde_json::json!(rb_i64_col(rb, "component").values().to_vec()),
            );
        } else {
            obj.insert(
                "from_bus_id".to_owned(),
                serde_json::json!(rb_i64_col(rb, "from_bus_id").values().to_vec()),
            );
            obj.insert(
                "to_bus_id".to_owned(),
                serde_json::json!(rb_i64_col(rb, "to_bus_id").values().to_vec()),
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
        let axes = [
            ("matrix_bus", PIO_ARROW_TABLE_MATRIX_BUS),
            ("matrix_branch", PIO_ARROW_TABLE_MATRIX_BRANCH),
        ];
        let mut axis_obj = serde_json::Map::new();
        for (name, table) in axes {
            let rb = matrix_record_batch(&n, table);
            axis_obj.insert(name.to_owned(), axis_table_json(name, &rb));
        }
        serde_json::json!({
            "case": case_file,
            "axes": axis_obj,
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

    #[test]
    fn arrow_table_ids_are_append_only() {
        assert_eq!(PIO_ARROW_TABLE_BUS, 0);
        assert_eq!(PIO_ARROW_TABLE_BRANCH, 1);
        assert_eq!(PIO_ARROW_TABLE_GEN, 2);
        assert_eq!(PIO_ARROW_TABLE_LOAD, 3);
        assert_eq!(PIO_ARROW_TABLE_SHUNT, 4);
        assert_eq!(PIO_ARROW_TABLE_SWITCH, 5);
        assert_eq!(PIO_ARROW_TABLE_SOLVER_BUS, 6);
        assert_eq!(PIO_ARROW_TABLE_SOLVER_LOAD, 7);
        assert_eq!(PIO_ARROW_TABLE_SOLVER_SHUNT, 8);
        assert_eq!(PIO_ARROW_TABLE_SOLVER_BRANCH, 9);
        assert_eq!(PIO_ARROW_TABLE_SOLVER_SWITCH, 10);
        assert_eq!(PIO_ARROW_TABLE_SOLVER_ARC, 11);
        assert_eq!(PIO_ARROW_TABLE_SOLVER_GEN, 12);
        assert_eq!(PIO_ARROW_TABLE_SOLVER_STORAGE, 13);
        assert_eq!(PIO_ARROW_TABLE_SOLVER_HVDC, 14);
        assert_eq!(PIO_ARROW_TABLE_YBUS, 15);
        assert_eq!(PIO_ARROW_TABLE_INCIDENCE, 16);
        assert_eq!(PIO_ARROW_TABLE_BPRIME, 17);
        assert_eq!(PIO_ARROW_TABLE_BDOUBLEPRIME, 18);
        assert_eq!(PIO_ARROW_TABLE_MATRIX_BUS, 19);
        assert_eq!(PIO_ARROW_TABLE_MATRIX_BRANCH, 20);
    }

    #[test]
    fn arrow_catalog_lists_ids_columns_axes_and_features() {
        let catalog: serde_json::Value = serde_json::from_str(&catalog_json()).unwrap();
        assert_eq!(catalog["schema_version"], ARROW_SCHEMA_VERSION);
        let tables = catalog["tables"].as_array().unwrap();
        let find = |name: &str| {
            tables
                .iter()
                .find(|table| table["name"] == name)
                .unwrap_or_else(|| panic!("missing catalog table {name}"))
        };

        let bus = find("bus");
        assert_eq!(bus["id"], PIO_ARROW_TABLE_BUS);
        assert_eq!(bus["feature_requirements"], serde_json::json!(["arrow"]));
        assert_eq!(bus["columns"][0]["name"], "id");

        let bprime = find("bprime");
        assert_eq!(bprime["id"], PIO_ARROW_TABLE_BPRIME);
        assert_eq!(bprime["format"], "coo");
        assert_eq!(bprime["row_axis"], "matrix_bus");
        assert_eq!(bprime["col_axis"], "matrix_bus");
        assert_eq!(
            bprime["feature_requirements"],
            serde_json::json!(["arrow", "matrix"])
        );
        assert_eq!(bprime["available"], cfg!(feature = "matrix"));

        let incidence = find("incidence");
        assert_eq!(incidence["row_axis"], "matrix_bus");
        assert_eq!(incidence["col_axis"], "matrix_branch");

        let axis = find("matrix_bus");
        assert_eq!(axis["id"], PIO_ARROW_TABLE_MATRIX_BUS);
        assert_eq!(axis["format"], "axis_map");
        assert_eq!(axis["columns"][1]["name"], "bus_id");
    }

    #[cfg(not(feature = "matrix"))]
    #[test]
    fn matrix_table_requires_matrix_feature() {
        let n = net("case9.m");
        let core = IndexCore::build(&n);
        let err = export(&n, &core, PIO_ARROW_TABLE_BPRIME).unwrap_err();
        assert!(err.contains("matrix cargo feature"), "{err}");
        let err = export(&n, &core, PIO_ARROW_TABLE_MATRIX_BUS).unwrap_err();
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
        assert_eq!(metadata.get("powerio.schema_version").unwrap(), "1");
        assert_eq!(metadata.get("powerio.format").unwrap(), "coo");
        assert_eq!(metadata.get("powerio.index_space").unwrap(), "solver_bus");
        assert_eq!(metadata.get("powerio.row_axis").unwrap(), "matrix_bus");
        assert_eq!(metadata.get("powerio.col_axis").unwrap(), "matrix_bus");
        assert_eq!(metadata.get("powerio.row_count").unwrap(), "9");
        assert_eq!(metadata.get("powerio.col_count").unwrap(), "9");

        let core = IndexCore::build(&n);
        let (array, schema) = export(&n, &core, PIO_ARROW_TABLE_BPRIME).unwrap();
        let imported_schema = Schema::try_from(&schema).unwrap();
        let metadata = imported_schema.metadata();
        assert_eq!(metadata.get("powerio.table").unwrap(), "bprime");
        assert_eq!(metadata.get("powerio.schema_version").unwrap(), "1");
        assert_eq!(metadata.get("powerio.format").unwrap(), "coo");
        assert_eq!(metadata.get("powerio.index_space").unwrap(), "solver_bus");
        assert_eq!(metadata.get("powerio.row_axis").unwrap(), "matrix_bus");
        assert_eq!(metadata.get("powerio.col_axis").unwrap(), "matrix_bus");
        assert_eq!(metadata.get("powerio.row_count").unwrap(), "9");
        assert_eq!(metadata.get("powerio.col_count").unwrap(), "9");
        let _data = unsafe { from_ffi(array, &schema) }.unwrap();
    }

    #[cfg(feature = "matrix")]
    #[test]
    fn incidence_uses_branch_axis_metadata() {
        let n = net("case9.m");
        let rb = matrix_record_batch(&n, PIO_ARROW_TABLE_INCIDENCE);
        let metadata = rb.schema();
        let metadata = metadata.metadata();
        assert_eq!(metadata.get("powerio.table").unwrap(), "incidence");
        assert_eq!(metadata.get("powerio.format").unwrap(), "coo");
        assert_eq!(metadata.get("powerio.row_axis").unwrap(), "matrix_bus");
        assert_eq!(metadata.get("powerio.col_axis").unwrap(), "matrix_branch");
        assert_eq!(metadata.get("powerio.row_count").unwrap(), "9");
        assert_eq!(metadata.get("powerio.col_count").unwrap(), "9");
    }

    #[cfg(feature = "matrix")]
    #[test]
    fn matrix_axis_maps_export_dense_rows() {
        let n = net("case14.m");
        let bus = round_trip(&n, PIO_ARROW_TABLE_MATRIX_BUS);
        assert_eq!(bus.len(), 14);
        assert_eq!(i64_col(&bus, "index").value(0), 0);
        assert_eq!(i64_col(&bus, "bus_id").value(0), 1);
        assert_eq!(i64_col(&bus, "source_row").value(0), 0);
        assert_eq!(u8_col(&bus, "is_reference").value(0), 1);
        assert_eq!(i64_col(&bus, "component").value(0), 0);
        assert_eq!(i64_col(&bus, "index").value(13), 13);
        assert_eq!(i64_col(&bus, "bus_id").value(13), 14);

        let branch = round_trip(&n, PIO_ARROW_TABLE_MATRIX_BRANCH);
        let incidence = matrix_record_batch(&n, PIO_ARROW_TABLE_INCIDENCE);
        let col_count = incidence
            .schema()
            .metadata()
            .get("powerio.col_count")
            .unwrap()
            .parse::<usize>()
            .unwrap();
        assert_eq!(branch.len(), col_count);
        assert_eq!(i64_col(&branch, "index").value(0), 0);
        assert_eq!(i64_col(&branch, "source_row").value(0), 0);
        assert_eq!(i64_col(&branch, "from_bus_id").value(0), 1);
        assert_eq!(i64_col(&branch, "to_bus_id").value(0), 2);
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
