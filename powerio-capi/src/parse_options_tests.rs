//! Original synthetic cases for option presence, companion ownership and errors.
use super::*;
use sha2::{Digest, Sha256};

const ORIGINAL: &[u8] = b"\0\x01\0\0Standard Jet DB\0PowerIO synthetic source, not a real MDB";
const FORMAT: &str = "sincal-multiconductor";

unsafe fn view_text(view: PioStringView) -> String {
    if view.len == 0 {
        return String::new();
    }
    unsafe {
        std::str::from_utf8(std::slice::from_raw_parts(view.data.cast(), view.len))
            .unwrap()
            .to_owned()
    }
}

fn records() -> Vec<u8> {
    let db = rusqlite::Connection::open_in_memory().unwrap();
    db.execute_batch(include_str!(
        "../../tests/data/sincal/synthetic-multiconductor.sql"
    ))
    .unwrap();
    db.execute_batch("UPDATE Version SET Version_No=11.5;
        UPDATE Load SET Flag_Lf=15, fP=1, fQ=1, fS=1, DayOpSer_ID=7;
        CREATE TABLE OpSer (OpSer_ID INTEGER, Variant_ID INTEGER, Flag_Variant INTEGER,
          Flag_Typ INTEGER, Flag_Ser INTEGER, BaseT REAL, Power_a1 REAL, Power_b1 REAL, Reduce_a2 REAL, Reduce_b2 REAL);
        INSERT INTO OpSer VALUES (7,1,1,3,1,0,0,0,0,0);
        CREATE TABLE OpSerVal (OpSerVal_ID INTEGER, OpSer_ID INTEGER, Variant_ID INTEGER,
          Flag_Variant INTEGER, OpTime REAL, Flag_Curve INTEGER, Factor REAL, P REAL, Q REAL, Op_ID INTEGER);
        INSERT INTO OpSerVal VALUES (1,7,1,1,0,1,NULL,6,-3,NULL),(2,7,1,1,12,2,NULL,18,9,NULL);").unwrap();
    let names: Vec<String> = db
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let tables: Vec<_> = names
        .iter()
        .map(|name| {
            let mut statement = db.prepare(&format!("SELECT * FROM {name}")).unwrap();
            let columns: Vec<_> = statement
                .column_names()
                .iter()
                .map(|name| serde_json::json!({"name": name, "native_type": "synthetic"}))
                .collect();
            let rows: Vec<Vec<serde_json::Value>> = statement
                .query_map([], |row| {
                    (0..columns.len())
                        .map(|i| {
                            Ok(match row.get_ref(i)? {
                                rusqlite::types::ValueRef::Null => serde_json::Value::Null,
                                rusqlite::types::ValueRef::Integer(n) => n.into(),
                                rusqlite::types::ValueRef::Real(n) => serde_json::json!(n),
                                rusqlite::types::ValueRef::Text(s) => {
                                    std::str::from_utf8(s).unwrap().into()
                                }
                                rusqlite::types::ValueRef::Blob(_) => {
                                    panic!("no blobs in original synthetic schema")
                                }
                            })
                        })
                        .collect()
                })
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            serde_json::json!({"name": name, "columns": columns, "rows": rows})
        })
        .collect();
    serde_json::to_vec(&serde_json::json!({
        "format": "powerio-sincal-tables", "version": 1, "transport": "access-mdbtools",
        "source": {"name": "synthetic.mdb", "bytes": ORIGINAL.len(), "sha256": format!("{:x}", Sha256::digest(ORIGINAL))},
        "tools": {"mdb-json": "synthetic", "mdb-schema": "synthetic", "mdb-tables": "synthetic"},
        "tables": tables, "excluded_tables": [], "absent_requested_tables": [],
    })).unwrap()
}

fn selection() -> PioSincalReadOptions {
    PioSincalReadOptions {
        has_variant: true,
        variant: 1,
        has_snapshot_hours: true,
        snapshot_hours: 6.0,
        acquired_tables: PioStringView::new("records.json"),
    }
}

unsafe fn source(error: *mut *mut PioError) -> *mut PioSource {
    let mut records = records();
    let buffer = PioNamedBufferView {
        name: PioStringView::new("records.json"),
        bytes: PioByteView::new(&records),
    };
    let source = unsafe {
        pio_source_from_memory_with_buffers(
            b"synthetic.mdb".as_ptr().cast(),
            13,
            ORIGINAL.as_ptr(),
            ORIGINAL.len(),
            &buffer,
            1,
            error,
        )
    };
    // A caller may free or overwrite every input buffer once construction returns.
    records.fill(0);
    source
}

#[test]
fn sincal_capi_selections_preserve_snapshot_presence_and_source_ownership() {
    unsafe {
        let mut error = std::ptr::null_mut();
        let source = source(&mut error);
        assert!(!source.is_null());
        assert!(error.is_null());
        for (hours, expected) in [(0.0, 2000.0), (6.0, 4000.0), (12.0, 6000.0)] {
            let mut selected = selection();
            selected.snapshot_hours = hours;
            let options = PioParseOptions {
                acquisition_root: PioStringView::EMPTY,
                sincal_multiconductor: &selected,
            };
            let module = pio_parse_with_options(
                source,
                FORMAT.as_ptr().cast(),
                FORMAT.len(),
                &options,
                &mut error,
            );
            assert!(!module.is_null(), "{}", view_text(pio_error_message(error)));
            let value = pio_module_value(module);
            let network = pio_value_multiconductor_network(value, &mut error);
            pio_value_release(value);
            pio_module_release(module);
            let mut load = std::mem::MaybeUninit::<PioMulticonductorLoadView>::uninit();
            assert!(pio_multiconductor_network_load_at(
                network,
                0,
                load.as_mut_ptr(),
                &mut error
            ));
            let load = load.assume_init();
            assert_eq!(
                std::slice::from_raw_parts(
                    load.active_power_nominal_w.data,
                    load.active_power_nominal_w.len
                ),
                &[expected; 3]
            );
            pio_multiconductor_network_release(network);
        }
        pio_source_release(source);
    }
}

#[test]
fn sincal_capi_refuses_missing_snapshot_bad_family_and_bad_borrowed_views() {
    unsafe {
        let mut error = std::ptr::null_mut();
        let source = source(&mut error);
        let mut selected = selection();
        selected.has_snapshot_hours = false;
        let options = PioParseOptions {
            acquisition_root: PioStringView::EMPTY,
            sincal_multiconductor: &selected,
        };
        assert!(
            pio_parse_with_options(
                source,
                FORMAT.as_ptr().cast(),
                FORMAT.len(),
                &options,
                &mut error
            )
            .is_null()
        );
        assert!(!error.is_null());
        pio_error_release(error);
        selected.has_snapshot_hours = true;
        let options = PioParseOptions {
            acquisition_root: PioStringView::EMPTY,
            sincal_multiconductor: &selected,
        };
        for format in ["sincal-balanced", "dss"] {
            assert!(
                pio_parse_with_options(
                    source,
                    format.as_ptr().cast(),
                    format.len(),
                    &options,
                    &mut error
                )
                .is_null()
            );
            assert_eq!(
                view_text(pio_error_code(error)),
                "REQUEST.PARSE.SINCAL_OPTIONS_PROFILE"
            );
            pio_error_release(error);
        }
        selected.acquired_tables = PioStringView {
            data: std::ptr::null(),
            len: 1,
        };
        let options = PioParseOptions {
            acquisition_root: PioStringView::EMPTY,
            sincal_multiconductor: &selected,
        };
        assert!(
            pio_parse_with_options(
                source,
                FORMAT.as_ptr().cast(),
                FORMAT.len(),
                &options,
                &mut error
            )
            .is_null()
        );
        assert_eq!(view_text(pio_error_code(error)), "BIND.CAPI.NULL_ARGUMENT");
        pio_error_release(error);
        pio_source_release(source);
        assert!(
            pio_source_from_memory_with_buffers(
                b"x".as_ptr().cast(),
                1,
                ORIGINAL.as_ptr(),
                ORIGINAL.len(),
                std::ptr::null(),
                1,
                &mut error
            )
            .is_null()
        );
        assert_eq!(view_text(pio_error_code(error)), "BIND.CAPI.NULL_ARGUMENT");
        pio_error_release(error);
    }
}

#[test]
#[ignore = "export original synthetic acquired tables for the Julia companion tests"]
fn export_sincal_binding_records() {
    std::fs::write(
        std::env::var_os("POWERIO_SINCAL_BINDING_RECORDS").unwrap(),
        records(),
    )
    .unwrap();
}
