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
//! These are the *raw* network fields, with EXTERNAL bus ids (the same id space
//! as `pio_bus_ids`), not the gridfm-datakit schema — no admittances or flows
//! (that schema needs the matrix layer; see issue #38).

use std::sync::Arc;

use arrow::array::{Array, ArrayRef, Float64Array, Int64Array, StructArray, UInt8Array};
use arrow::datatypes::{Field, Schema};
use arrow::error::ArrowError;
use arrow::ffi::{FFI_ArrowArray, FFI_ArrowSchema, to_ffi};
use arrow::record_batch::RecordBatch;
use powerio::{BusId, Network};

/// Table selectors for [`pio_to_arrow`](crate::pio_to_arrow); the C
/// header mirrors these as `PIO_ARROW_TABLE_*`.
pub const PIO_ARROW_TABLE_BUS: i32 = 0;
pub const PIO_ARROW_TABLE_BRANCH: i32 = 1;
pub const PIO_ARROW_TABLE_GEN: i32 = 2;
pub const PIO_ARROW_TABLE_LOAD: i32 = 3;
pub const PIO_ARROW_TABLE_SHUNT: i32 = 4;
pub const PIO_ARROW_TABLE_SWITCH: i32 = 5;

// These values are the ABI: the `PIO_ARROW_TABLE_*` macros in include/powerio.h
// are hand-synced to them. The set is append-only: these ids and each table's
// column order are frozen, a new table takes the next id (5, 6, ...) and extends
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
);

/// Build the requested table and export it over the C Data Interface. The
/// returned FFI structs own the columnar buffers until the consumer releases
/// them.
pub fn export(net: &Network, table: i32) -> Result<(FFI_ArrowArray, FFI_ArrowSchema), String> {
    let rb = match table {
        PIO_ARROW_TABLE_BUS => bus_batch(net),
        PIO_ARROW_TABLE_BRANCH => branch_batch(net),
        PIO_ARROW_TABLE_GEN => gen_batch(net),
        PIO_ARROW_TABLE_LOAD => load_batch(net),
        PIO_ARROW_TABLE_SHUNT => shunt_batch(net),
        PIO_ARROW_TABLE_SWITCH => switch_batch(net),
        other => return Err(format!("unknown Arrow table id {other}")),
    }
    .map_err(|e| e.to_string())?;

    // The C Data Interface represents a record batch as a struct array.
    let data = StructArray::from(rb).into_data();
    to_ffi(&data).map_err(|e| e.to_string())
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

fn batch(cols: Vec<(&str, ArrayRef)>) -> Result<RecordBatch, ArrowError> {
    let fields: Vec<Field> = cols
        .iter()
        .map(|(name, arr)| Field::new(*name, arr.data_type().clone(), false))
        .collect();
    let arrays: Vec<ArrayRef> = cols.into_iter().map(|(_, arr)| arr).collect();
    RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays)
}

/// External bus id as i64 (`-1` if it somehow overflows), matching `pio_branches`.
fn ext(id: BusId) -> i64 {
    i64::try_from(id.0).unwrap_or(-1)
}

fn usz(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(-1)
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
        use powerio::{
            Branch, BranchCharging, Bus, BusId, BusType, DEFAULT_BASE_FREQUENCY, Extras,
            SourceFormat,
        };

        Network {
            name: "terminal-projection".into(),
            base_mva: 100.0,
            base_frequency: DEFAULT_BASE_FREQUENCY,
            buses: vec![
                Bus {
                    id: BusId(1),
                    kind: BusType::Ref,
                    vm: 1.0,
                    va: 0.0,
                    base_kv: 230.0,
                    vmax: 1.1,
                    vmin: 0.9,
                    evhi: None,
                    evlo: None,
                    area: 1,
                    zone: 1,
                    name: None,
                    extras: Extras::new(),
                },
                Bus {
                    id: BusId(2),
                    kind: BusType::Pq,
                    vm: 1.0,
                    va: 0.0,
                    base_kv: 230.0,
                    vmax: 1.1,
                    vmin: 0.9,
                    evhi: None,
                    evlo: None,
                    area: 1,
                    zone: 1,
                    name: None,
                    extras: Extras::new(),
                },
            ],
            loads: Vec::new(),
            shunts: Vec::new(),
            branches: vec![Branch {
                from: BusId(1),
                to: BusId(2),
                r: 0.01,
                x: 0.1,
                b: 0.0,
                charging: Some(BranchCharging {
                    g_fr: 0.01,
                    b_fr: 0.02,
                    g_to: 0.03,
                    b_to: 0.05,
                }),
                rate_a: 100.0,
                rate_b: 0.0,
                rate_c: 0.0,
                current_ratings: None,
                tap: 0.0,
                shift: 0.0,
                in_service: true,
                angmin: -360.0,
                angmax: 360.0,
                control: None,
                solution: None,
                extras: Extras::new(),
            }],
            switches: Vec::new(),
            generators: Vec::new(),
            storage: Vec::new(),
            hvdc: Vec::new(),
            transformers_3w: Vec::new(),
            areas: Vec::new(),
            solver: None,
            source_format: SourceFormat::InMemory,
            source: None,
        }
    }

    fn round_trip(net: &Network, table: i32) -> StructArray {
        let (array, schema) = export(net, table).unwrap();
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
        assert!(export(&n, 99).is_err());
    }
}
