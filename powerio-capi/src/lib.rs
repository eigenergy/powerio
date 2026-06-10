//! C ABI for `powerio`.
//!
//! Parse any supported power system case format into an opaque handle, query it,
//! convert it to another format, and pull out the numeric tables a
//! downstream solver needs to assemble matrices. Every entry point is `extern
//! "C"`, catches panics at the boundary, and returns error text into a
//! caller-provided buffer. Strings handed back are owned by the library; free
//! them with [`pio_string_free`]. Array extractors fill caller-allocated
//! buffers (length = the matching `pio_n_*` count); pass `NULL` to skip one.
//!
//! Naming: every symbol is prefixed `pio_`. The header is `include/powerio.h`.

#![allow(clippy::missing_safety_doc)]

use std::ffi::{CStr, CString, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};

use powerio::{IndexCore, IndexedNetwork, Network, TargetFormat};

#[cfg(feature = "arrow")]
mod arrow_export;
#[cfg(feature = "arrow")]
pub use arrow_export::{
    PIO_ARROW_TABLE_BRANCH, PIO_ARROW_TABLE_BUS, PIO_ARROW_TABLE_GEN, PIO_ARROW_TABLE_LOAD,
    PIO_ARROW_TABLE_SHUNT,
};

/// Opaque parsed network handle. Carries the parsed [`Network`] plus the
/// [`IndexCore`] derived from it once at parse time, so every indexed query
/// reuses the same bus-id map and nodal aggregates instead of rebuilding them.
pub struct PioNetwork {
    net: Network,
    core: IndexCore,
}

/// Copy `msg` (truncated to fit) into a caller `char[len]` buffer, always
/// NUL-terminated. Shared by the error and warning outputs.
///
/// # Safety
/// A non-NULL `buf` must point to at least `len` writable bytes; the write
/// stays within `len` (at most `len - 1` message bytes plus the terminating
/// NUL). NULL or `len == 0` is a no-op.
unsafe fn copy_to_buf(buf: *mut c_char, len: usize, msg: &str) {
    unsafe {
        if buf.is_null() || len == 0 {
            return;
        }
        let bytes = msg.as_bytes();
        let n = bytes.len().min(len - 1);
        std::ptr::copy_nonoverlapping(bytes.as_ptr().cast::<c_char>(), buf, n);
        *buf.add(n) = 0;
    }
}

unsafe fn cstr<'a>(p: *const c_char) -> Option<&'a str> {
    unsafe {
        if p.is_null() {
            return None;
        }
        CStr::from_ptr(p).to_str().ok()
    }
}

/// Move `s` into an owned C string, or `None` if it holds an interior NUL byte
/// (which can't cross as a C string). Callers surface the `None` as a real error
/// rather than silently handing back an empty string.
fn into_cstring(s: String) -> Option<*mut c_char> {
    CString::new(s).ok().map(CString::into_raw)
}

/// Finish a `*mut c_char` entry point: hand back the owned C string, or on an
/// interior NUL write the error into `errbuf` (NULL/0 to skip) and return NULL.
/// The shared tail of the string-returning functions.
fn finish_cstring(s: String, errbuf: *mut c_char, errlen: usize) -> *mut c_char {
    match into_cstring(s) {
        Some(p) => p,
        None => {
            unsafe { copy_to_buf(errbuf, errlen, "output contained an interior NUL byte") };
            std::ptr::null_mut()
        }
    }
}

/// Run `f` at the FFI boundary, catching any panic so it can't unwind across
/// `extern "C"` (UB). Returns `fallback` if `f` panics.
unsafe fn guard<R>(fallback: R, f: impl FnOnce() -> R) -> R {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(fallback)
}

/// Box a `Network` into an owned network handle, building its [`IndexCore`] once so
/// every indexed query reuses it. The one constructor for `*mut PioNetwork`.
fn make_network(net: Network) -> *mut PioNetwork {
    let core = IndexCore::build(&net);
    Box::into_raw(Box::new(PioNetwork { net, core }))
}

/// Finish a `*mut PioNetwork` entry point: run `f` (producing a `Network` or an
/// error message) under the panic guard, hand back an owned handle, or write the
/// error, `panic_msg` if `f` panicked, into `errbuf` and return NULL. The
/// shared tail of every handle-returning function (`pio_parse_file`,
/// `pio_parse_str`, `pio_to_normalized`, `pio_from_json`).
unsafe fn finish_network(
    errbuf: *mut c_char,
    errlen: usize,
    panic_msg: &str,
    f: impl FnOnce() -> Result<Network, String>,
) -> *mut PioNetwork {
    unsafe {
        match catch_unwind(AssertUnwindSafe(f)) {
            Ok(Ok(net)) => make_network(net),
            Ok(Err(msg)) => {
                copy_to_buf(errbuf, errlen, &msg);
                std::ptr::null_mut()
            }
            Err(_) => {
                copy_to_buf(errbuf, errlen, panic_msg);
                std::ptr::null_mut()
            }
        }
    }
}

/// ABI version of this C interface. Bump on any breaking change to an existing
/// `pio_*` signature or to the JSON transport schema (new additive symbols don't
/// require a bump). A consumer compares [`pio_abi_version`] against the value it
/// was built against (the `PIO_ABI_VERSION` macro in `powerio.h`) and refuses a
/// mismatched library instead of calling in blind.
pub const PIO_ABI_VERSION: u32 = 3;

/// A comfortable error-buffer size: pass a `char[PIO_ERRBUF_MIN]` to any
/// `errbuf`/`warnbuf` parameter and a message always fits without truncation.
pub const PIO_ERRBUF_MIN: usize = 256;

/// The ABI version the library was built with (see [`PIO_ABI_VERSION`]). Lets a
/// consumer detect a stale or incompatible library at load time. Infallible.
#[unsafe(no_mangle)]
pub extern "C" fn pio_abi_version() -> u32 {
    PIO_ABI_VERSION
}

/// The crate version string (e.g. `"0.0.1"`), `'static` and NUL-terminated. Do
/// NOT free it. Informational; pair it with [`pio_abi_version`] for the actual
/// compatibility check.
#[unsafe(no_mangle)]
pub extern "C" fn pio_version() -> *const c_char {
    // env! is resolved at compile time; the trailing NUL makes it a valid C
    // string and the 'static lifetime means the pointer is always valid and
    // never owned by the caller.
    concat!(env!("CARGO_PKG_VERSION"), "\0")
        .as_ptr()
        .cast::<c_char>()
}

fn target_format_from_c(to: *const c_char) -> Result<TargetFormat, String> {
    let to = unsafe { cstr(to) }.ok_or_else(|| "to is NULL or not UTF-8".to_string())?;
    to.parse::<TargetFormat>().map_err(|e| e.to_string())
}

fn optional_cstr<'a>(p: *const c_char, name: &str) -> Result<Option<&'a str>, String> {
    if p.is_null() {
        Ok(None)
    } else {
        unsafe { cstr(p) }
            .map(Some)
            .ok_or_else(|| format!("{name} is not UTF-8"))
    }
}

/// Parse `path` (format from extension, or `from` if non-NULL) into a case
/// handle. Returns `NULL` on error and writes the message into `errbuf`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_parse_file(
    path: *const c_char,
    from: *const c_char,
    errbuf: *mut c_char,
    errlen: usize,
) -> *mut PioNetwork {
    unsafe {
        finish_network(errbuf, errlen, "panic while parsing", || {
            let path = cstr(path).ok_or_else(|| "path is NULL or not UTF-8".to_string())?;
            let from = optional_cstr(from, "from")?;
            powerio::parse_file(std::path::Path::new(path), from).map_err(|e| e.to_string())
        })
    }
}

/// Parse in-memory case `text` of the named `format` into a network handle. Unlike
/// [`pio_parse_file`] there is no path to infer from, so `format` is required: one of
/// `matpower`/`m`, `powermodels`/`pm`, `egret`, `psse`/`raw`, `powerworld`/`aux`
/// (see `TargetFormat::from_str`). Returns `NULL` on error and writes the
/// message into `errbuf`. Free the handle with [`pio_network_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_parse_str(
    text: *const c_char,
    format: *const c_char,
    errbuf: *mut c_char,
    errlen: usize,
) -> *mut PioNetwork {
    unsafe {
        finish_network(errbuf, errlen, "panic while parsing", || {
            let text = cstr(text).ok_or_else(|| "text is NULL or not UTF-8".to_string())?;
            let format = cstr(format).ok_or_else(|| "format is NULL or not UTF-8".to_string())?;
            powerio::parse_str(text, format).map_err(|e| e.to_string())
        })
    }
}

/// Free a network handle from [`pio_parse_file`], [`pio_parse_str`],
/// [`pio_to_normalized`], or [`pio_from_json`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_network_free(net: *mut PioNetwork) {
    unsafe {
        if !net.is_null() {
            drop(Box::from_raw(net));
        }
    }
}

unsafe fn network_ref<'a>(net: *const PioNetwork) -> Option<&'a PioNetwork> {
    unsafe { net.as_ref() }
}

/// View `net` through its cached [`IndexCore`] with no per-call rebuild.
unsafe fn view<'a>(net: *const PioNetwork) -> Option<IndexedNetwork<'a>> {
    unsafe {
        net.as_ref()
            .map(|c| IndexedNetwork::with_core(&c.net, &c.core))
    }
}

/// Normalize `net` into a NEW per-unit network handle: per unit, radians,
/// out-of-service filtered, densely reindexed, bus types canonicalized (see
/// `Network::to_normalized`). The result is independent of `net`; free both
/// with [`pio_network_free`]. Every extractor and [`pio_to_json`] works on it
/// unchanged (the handle is per unit, not MW). Returns `NULL` on error (no
/// reference bus can be chosen, or a non-positive base MVA) and writes the
/// message into `errbuf`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_to_normalized(
    net: *const PioNetwork,
    errbuf: *mut c_char,
    errlen: usize,
) -> *mut PioNetwork {
    unsafe {
        finish_network(errbuf, errlen, "panic while normalizing", || {
            let c = network_ref(net).ok_or_else(|| "network handle is NULL".to_string())?;
            c.net.to_normalized().map_err(|e| e.to_string())
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_n_buses(net: *const PioNetwork) -> usize {
    unsafe { guard(0, || network_ref(net).map_or(0, |c| c.net.buses.len())) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_n_branches(net: *const PioNetwork) -> usize {
    unsafe { guard(0, || network_ref(net).map_or(0, |c| c.net.branches.len())) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_n_gens(net: *const PioNetwork) -> usize {
    unsafe { guard(0, || network_ref(net).map_or(0, |c| c.net.generators.len())) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_base_mva(net: *const PioNetwork) -> f64 {
    unsafe { guard(0.0, || network_ref(net).map_or(0.0, |c| c.net.base_mva)) }
}

/// Dense `[0, n)` index of the single reference bus, or `-1` if not exactly one
/// (also `-1` if the index is too large for `isize`). A network may carry
/// several references (one per island, or a normalized case that kept the file's
/// multiple `REF` buses); use [`pio_n_reference_buses`] to tell zero from many,
/// and [`pio_reference_buses`] to read them all.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_reference_bus(net: *const PioNetwork) -> isize {
    unsafe {
        guard(-1, || match view(net) {
            Some(v) => v
                .reference_bus_index()
                .map_or(-1, |i| isize::try_from(i).unwrap_or(-1)),
            None => -1,
        })
    }
}

/// Number of reference (slack) buses. `0` means none; `> 1` means one reference
/// per island or several fixed reference buses in one island. A normalized case
/// always reports `>= 1`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_n_reference_buses(net: *const PioNetwork) -> usize {
    unsafe {
        guard(0, || {
            view(net).map_or(0, |v| v.reference_bus_indices().len())
        })
    }
}

/// Fill `out` (length [`pio_n_reference_buses`]) with the dense `[0, n)` indices
/// of the reference buses, ascending.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_reference_buses(net: *const PioNetwork, out: *mut i64) {
    unsafe {
        guard((), || {
            if let Some(v) = view(net) {
                fill(
                    out,
                    v.reference_bus_indices()
                        .into_iter()
                        .map(|i| i64::try_from(i).unwrap_or(-1)),
                );
            }
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_n_components(net: *const PioNetwork) -> usize {
    unsafe { guard(0, || view(net).map_or(0, |v| v.n_connected_components())) }
}

/// `1` if the in-service topology is a forest, else `0`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_is_radial(net: *const PioNetwork) -> i32 {
    unsafe { guard(0, || view(net).map_or(0, |v| i32::from(v.is_radial()))) }
}

/// Serialize `net` to MATPOWER `.m` text (byte-exact echo when parsed from
/// MATPOWER). Returns an owned C string; free with [`pio_string_free`]. Returns
/// `NULL` on error and writes the message into `errbuf`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_to_matpower(
    net: *const PioNetwork,
    errbuf: *mut c_char,
    errlen: usize,
) -> *mut c_char {
    unsafe {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let c = network_ref(net).ok_or_else(|| "network handle is NULL".to_string())?;
            Ok::<_, String>(c.net.to_matpower())
        }));
        match r {
            Ok(Ok(text)) => finish_cstring(text, errbuf, errlen),
            Ok(Err(msg)) => {
                copy_to_buf(errbuf, errlen, &msg);
                std::ptr::null_mut()
            }
            Err(_) => {
                copy_to_buf(errbuf, errlen, "panic while serializing to MATPOWER");
                std::ptr::null_mut()
            }
        }
    }
}

/// Serialize `net` to format `to`.
///
/// Returns the converted text as an owned C string (free with
/// [`pio_string_free`]), `NULL` on error. Fidelity warnings, if any, are written
/// `\n`-joined into `warnbuf`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_to_format(
    net: *const PioNetwork,
    to: *const c_char,
    warnbuf: *mut c_char,
    warnlen: usize,
    errbuf: *mut c_char,
    errlen: usize,
) -> *mut c_char {
    unsafe {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let c = network_ref(net).ok_or_else(|| "network handle is NULL".to_string())?;
            let target = target_format_from_c(to)?;
            let conv = c.net.to_format(target);
            Ok::<_, String>((conv.text, conv.warnings))
        }));
        match r {
            Ok(Ok((text, warnings))) => {
                copy_to_buf(warnbuf, warnlen, &warnings.join("\n"));
                finish_cstring(text, errbuf, errlen)
            }
            Ok(Err(msg)) => {
                copy_to_buf(errbuf, errlen, &msg);
                std::ptr::null_mut()
            }
            Err(_) => {
                copy_to_buf(errbuf, errlen, "panic while converting");
                std::ptr::null_mut()
            }
        }
    }
}

/// Convert `path` to format `to` (optionally forcing the source via `from`).
/// Returns the converted text as an owned C string (free with
/// [`pio_string_free`]), `NULL` on error. Fidelity warnings, if any, are written
/// `\n`-joined into `warnbuf`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_convert_file(
    path: *const c_char,
    to: *const c_char,
    from: *const c_char,
    warnbuf: *mut c_char,
    warnlen: usize,
    errbuf: *mut c_char,
    errlen: usize,
) -> *mut c_char {
    unsafe {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let path = cstr(path).ok_or_else(|| "path is NULL or not UTF-8".to_string())?;
            let from = optional_cstr(from, "from")?;
            let target = target_format_from_c(to)?;
            let conv = powerio::convert_file(std::path::Path::new(path), target, from)
                .map_err(|e| e.to_string())?;
            Ok::<_, String>((conv.text, conv.warnings))
        }));
        match r {
            Ok(Ok((text, warnings))) => {
                copy_to_buf(warnbuf, warnlen, &warnings.join("\n"));
                finish_cstring(text, errbuf, errlen)
            }
            Ok(Err(msg)) => {
                copy_to_buf(errbuf, errlen, &msg);
                std::ptr::null_mut()
            }
            Err(_) => {
                copy_to_buf(errbuf, errlen, "panic while converting");
                std::ptr::null_mut()
            }
        }
    }
}

/// Free a string returned by [`pio_to_matpower`], [`pio_to_format`],
/// [`pio_convert_file`], or
/// [`pio_to_json`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_string_free(s: *mut c_char) {
    unsafe {
        if !s.is_null() {
            drop(CString::from_raw(s));
        }
    }
}

/// Serialize the case to JSON: the structured-table transport every Julia
/// bridge consumes. Carries the whole [`Network`] (buses, loads, shunts,
/// branches, generators, storage, HVDC, extras) but not the retained source
/// text, so it is structured data, not the byte-exact echo. Returns an owned C
/// string (free with [`pio_string_free`]), `NULL` on error (message into
/// `errbuf`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_to_json(
    net: *const PioNetwork,
    errbuf: *mut c_char,
    errlen: usize,
) -> *mut c_char {
    unsafe {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let c = network_ref(net).ok_or_else(|| "network handle is NULL".to_string())?;
            c.net.to_json().map_err(|e| e.to_string())
        }));
        match r {
            Ok(Ok(json)) => finish_cstring(json, errbuf, errlen),
            Ok(Err(msg)) => {
                copy_to_buf(errbuf, errlen, &msg);
                std::ptr::null_mut()
            }
            Err(_) => {
                copy_to_buf(errbuf, errlen, "panic while serializing to JSON");
                std::ptr::null_mut()
            }
        }
    }
}

/// Rebuild a network handle from JSON produced by [`pio_to_json`]. Returns a new
/// handle (free with [`pio_network_free`]), or `NULL` on error (message into
/// `errbuf`). The handle has no retained source, so [`pio_to_matpower`]
/// reformats it rather than echoing a byte-exact original.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_from_json(
    json: *const c_char,
    errbuf: *mut c_char,
    errlen: usize,
) -> *mut PioNetwork {
    unsafe {
        finish_network(errbuf, errlen, "panic while parsing JSON", || {
            let json = cstr(json).ok_or_else(|| "json is NULL or not UTF-8".to_string())?;
            Network::from_json(json).map_err(|e| e.to_string())
        })
    }
}

unsafe fn fill<T: Copy>(ptr: *mut T, vals: impl Iterator<Item = T>) {
    unsafe {
        if ptr.is_null() {
            return;
        }
        for (i, v) in vals.enumerate() {
            *ptr.add(i) = v;
        }
    }
}

/// Fill `out` (length `pio_n_buses`) with the 1-based bus ids in dense order.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_bus_ids(net: *const PioNetwork, out: *mut i64) {
    unsafe {
        guard((), || {
            if let Some(c) = network_ref(net) {
                fill(
                    out,
                    c.net
                        .buses
                        .iter()
                        .map(|b| i64::try_from(b.id.0).unwrap_or(-1)),
                );
            }
        })
    }
}

/// Fill the branch tables (each length `pio_n_branches`). `from`/`to` are the
/// 1-based bus ids (the same id space as [`pio_bus_ids`], not dense indices);
/// map them to dense matrix rows with the [`pio_bus_ids`] ordering. Any pointer
/// may be `NULL` to skip.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_branches(
    net: *const PioNetwork,
    from: *mut i64,
    to: *mut i64,
    r: *mut f64,
    x: *mut f64,
    b: *mut f64,
    tap: *mut f64,
    shift: *mut f64,
    in_service: *mut u8,
) {
    unsafe {
        guard((), || {
            let Some(c) = network_ref(net) else { return };
            let net = &c.net;
            fill(
                from,
                net.branches
                    .iter()
                    .map(|br| i64::try_from(br.from.0).unwrap_or(-1)),
            );
            fill(
                to,
                net.branches
                    .iter()
                    .map(|br| i64::try_from(br.to.0).unwrap_or(-1)),
            );
            fill(r, net.branches.iter().map(|br| br.r));
            fill(x, net.branches.iter().map(|br| br.x));
            fill(b, net.branches.iter().map(|br| br.b));
            fill(tap, net.branches.iter().map(|br| br.tap));
            fill(shift, net.branches.iter().map(|br| br.shift));
            fill(
                in_service,
                net.branches.iter().map(|br| u8::from(br.in_service)),
            );
        })
    }
}

/// Fill the generator tables (each length `pio_n_gens`; `bus` is the 1-based bus
/// id, the same id space as [`pio_bus_ids`]). Any pointer may be `NULL` to skip.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_gens(
    net: *const PioNetwork,
    bus: *mut i64,
    pg: *mut f64,
    pmax: *mut f64,
    pmin: *mut f64,
    in_service: *mut u8,
) {
    unsafe {
        guard((), || {
            let Some(c) = network_ref(net) else { return };
            let net = &c.net;
            fill(
                bus,
                net.generators
                    .iter()
                    .map(|g| i64::try_from(g.bus.0).unwrap_or(-1)),
            );
            fill(pg, net.generators.iter().map(|g| g.pg));
            fill(pmax, net.generators.iter().map(|g| g.pmax));
            fill(pmin, net.generators.iter().map(|g| g.pmin));
            fill(
                in_service,
                net.generators.iter().map(|g| u8::from(g.in_service)),
            );
        })
    }
}

/// Fill nodal aggregates (each length `pio_n_buses`, dense order): active and
/// reactive demand summed per bus. Any pointer may be `NULL`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_nodal_demand(net: *const PioNetwork, pd: *mut f64, qd: *mut f64) {
    unsafe {
        guard((), || {
            if let Some(v) = view(net) {
                fill(pd, v.pd().iter().copied());
                fill(qd, v.qd().iter().copied());
            }
        })
    }
}

/// Fill nodal shunt aggregates (each length `pio_n_buses`, dense order).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_nodal_shunt(net: *const PioNetwork, gs: *mut f64, bs: *mut f64) {
    unsafe {
        guard((), || {
            if let Some(v) = view(net) {
                fill(gs, v.gs().iter().copied());
                fill(bs, v.bs().iter().copied());
            }
        })
    }
}

/// Export one raw network table over the Arrow C Data Interface.
///
/// `table` is one of the `PIO_ARROW_TABLE_*` selectors (bus/branch/gen/load/
/// shunt); the columns are the parsed network fields with EXTERNAL bus ids (the
/// `pio_bus_ids` id space), not the gridfm schema. On success (returns `0`),
/// `out_array` and `out_schema` are populated with owned C Data Interface
/// structs: ownership of the Arrow buffers transfers to the caller, both
/// `release` callbacks are non-NULL, and the caller MUST invoke each exactly
/// once when done (skipping one leaks; the structs outlive `pio_network_free`).
/// On error (returns `-1`) the message is written into `errbuf` and the
/// out-params are left untouched. Only built with the `arrow` cargo feature.
#[cfg(feature = "arrow")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_export_arrow(
    net: *const PioNetwork,
    table: i32,
    out_array: *mut arrow::ffi::FFI_ArrowArray,
    out_schema: *mut arrow::ffi::FFI_ArrowSchema,
    errbuf: *mut c_char,
    errlen: usize,
) -> i32 {
    unsafe {
        let r = catch_unwind(AssertUnwindSafe(|| {
            if out_array.is_null() || out_schema.is_null() {
                return Err("out_array or out_schema is NULL".to_string());
            }
            let c = network_ref(net).ok_or_else(|| "network handle is NULL".to_string())?;
            arrow_export::export(&c.net, table)
        }));
        match r {
            Ok(Ok((array, schema))) => {
                // Move the FFI structs into caller memory: ptr::write does not
                // drop the (caller-zeroed) destination and does not run Drop on
                // `array`/`schema`, so the producer release callbacks transfer to
                // the caller. Exactly one owner.
                std::ptr::write(out_array, array);
                std::ptr::write(out_schema, schema);
                0
            }
            Ok(Err(msg)) => {
                copy_to_buf(errbuf, errlen, &msg);
                -1
            }
            Err(_) => {
                copy_to_buf(errbuf, errlen, "panic while exporting Arrow");
                -1
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    fn data_path(name: &str) -> CString {
        CString::new(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../tests/data")
                .join(name)
                .to_str()
                .unwrap(),
        )
        .unwrap()
    }

    fn case9() -> *mut PioNetwork {
        let path = CString::new(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../tests/data/case9.m")
                .to_str()
                .unwrap(),
        )
        .unwrap();
        let mut err = [0 as c_char; 256];
        let c =
            unsafe { pio_parse_file(path.as_ptr(), std::ptr::null(), err.as_mut_ptr(), err.len()) };
        assert!(!c.is_null(), "parse returned null");
        c
    }

    #[test]
    fn version_surface() {
        // The ABI version is the compatibility contract a consumer checks at
        // load; the version string is static, NUL-terminated, and non-empty.
        assert_eq!(pio_abi_version(), PIO_ABI_VERSION);
        let v = unsafe { CStr::from_ptr(pio_version()) }.to_str().unwrap();
        assert_eq!(v, env!("CARGO_PKG_VERSION"));
        assert!(!v.is_empty());
    }

    #[test]
    fn parse_query_free() {
        let c = case9();
        unsafe {
            assert_eq!(pio_n_buses(c), 9);
            assert_eq!(pio_n_branches(c), 9);
            assert_eq!(pio_n_gens(c), 3);
            assert_eq!(pio_base_mva(c), 100.0);
            assert_eq!(pio_n_components(c), 1);
            assert!(pio_reference_bus(c) >= 0);
            pio_network_free(c);
        }
    }

    #[test]
    fn write_is_byte_exact() {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data/case9.m"),
        )
        .unwrap();
        let c = case9();
        unsafe {
            let mut err = [0 as c_char; 256];
            let s = pio_to_matpower(c, err.as_mut_ptr(), err.len());
            assert!(!s.is_null());
            let got = CStr::from_ptr(s).to_str().unwrap();
            assert_eq!(got, src);
            pio_string_free(s);

            let null = pio_to_matpower(std::ptr::null(), err.as_mut_ptr(), err.len());
            assert!(null.is_null());
            assert_eq!(
                CStr::from_ptr(err.as_ptr()).to_str().unwrap(),
                "network handle is NULL"
            );
            pio_network_free(c);
        }
    }

    #[test]
    fn extract_branch_tables() {
        let c = case9();
        unsafe {
            let nb = pio_n_branches(c);
            let mut from = vec![0i64; nb];
            let mut x = vec![0f64; nb];
            pio_branches(
                c,
                from.as_mut_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                x.as_mut_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            // `from` carries the 1-based bus ids (case9 buses are 1..=9), the
            // same id space as pio_bus_ids, not dense indices.
            assert!(from.iter().all(|&f| f >= 1));
            assert!(x.iter().all(|&xx| xx > 0.0));
            pio_network_free(c);
        }
    }

    #[test]
    fn convert_matpower_echo() {
        let path = CString::new(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../tests/data/case14.m")
                .to_str()
                .unwrap(),
        )
        .unwrap();
        let to = CString::new("matpower").unwrap();
        let mut warn = [0 as c_char; 256];
        let mut err = [0 as c_char; 256];
        unsafe {
            let s = pio_convert_file(
                path.as_ptr(),
                to.as_ptr(),
                std::ptr::null(),
                warn.as_mut_ptr(),
                warn.len(),
                err.as_mut_ptr(),
                err.len(),
            );
            assert!(!s.is_null());
            let got = CStr::from_ptr(s).to_str().unwrap();
            let src = std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data/case14.m"),
            )
            .unwrap();
            assert_eq!(got, src);
            pio_string_free(s);
        }
    }

    #[test]
    fn to_format_converts_live_handle() {
        let c = case9();
        let to = CString::new("powermodels-json").unwrap();
        let mut warn = [0 as c_char; 256];
        let mut err = [0 as c_char; 256];
        unsafe {
            let s = pio_to_format(
                c,
                to.as_ptr(),
                warn.as_mut_ptr(),
                warn.len(),
                err.as_mut_ptr(),
                err.len(),
            );
            assert!(!s.is_null());
            let text = CStr::from_ptr(s).to_str().unwrap();
            assert!(text.contains("\"bus\""));
            pio_string_free(s);
            pio_network_free(c);
        }
    }

    #[test]
    fn parse_error_sets_message_not_null_handle() {
        let path = CString::new("/no/such/case.m").unwrap();
        let mut err = [0 as c_char; 256];
        let c =
            unsafe { pio_parse_file(path.as_ptr(), std::ptr::null(), err.as_mut_ptr(), err.len()) };
        assert!(c.is_null());
        let msg = unsafe { CStr::from_ptr(err.as_ptr()) }.to_str().unwrap();
        assert!(!msg.is_empty(), "expected an error message");
    }

    #[test]
    fn non_utf8_from_hint_errors_instead_of_falling_back() {
        let path = data_path("case9.m");
        let to = CString::new("matpower").unwrap();
        let bad_from = [0xff_u8, 0];
        let mut err = [0 as c_char; 256];
        let c = unsafe {
            pio_parse_file(
                path.as_ptr(),
                bad_from.as_ptr().cast::<c_char>(),
                err.as_mut_ptr(),
                err.len(),
            )
        };
        assert!(c.is_null());
        assert_eq!(
            unsafe { CStr::from_ptr(err.as_ptr()) }.to_str().unwrap(),
            "from is not UTF-8"
        );

        let mut warn = [0 as c_char; 256];
        err.fill(0);
        let s = unsafe {
            pio_convert_file(
                path.as_ptr(),
                to.as_ptr(),
                bad_from.as_ptr().cast::<c_char>(),
                warn.as_mut_ptr(),
                warn.len(),
                err.as_mut_ptr(),
                err.len(),
            )
        };
        assert!(s.is_null());
        assert_eq!(
            unsafe { CStr::from_ptr(err.as_ptr()) }.to_str().unwrap(),
            "from is not UTF-8"
        );
    }

    #[test]
    fn extract_gen_and_nodal_tables() {
        // case30 carries generators, loads, and shunts: cross-check the table
        // extractors against known counts and aggregate signs (a column swap in
        // pio_gens/pio_nodal_* would otherwise ship silently).
        let path = data_path("case30.m");
        let mut err = [0 as c_char; 256];
        let c =
            unsafe { pio_parse_file(path.as_ptr(), std::ptr::null(), err.as_mut_ptr(), err.len()) };
        assert!(!c.is_null());
        unsafe {
            let nb = pio_n_buses(c);
            let ng = pio_n_gens(c);
            assert_eq!(nb, 30);
            assert!(ng > 0);

            let mut gbus = vec![-9i64; ng];
            let mut pmax = vec![0f64; ng];
            pio_gens(
                c,
                gbus.as_mut_ptr(),
                std::ptr::null_mut(),
                pmax.as_mut_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            assert!(gbus.iter().all(|&b| b >= 0 && (b as usize) < nb));
            assert!(pmax.iter().any(|&p| p > 0.0));

            let mut ids = vec![0i64; nb];
            pio_bus_ids(c, ids.as_mut_ptr());
            assert!(ids.iter().all(|&id| id >= 1)); // MATPOWER bus ids are 1-based

            let mut pd = vec![0f64; nb];
            let mut qd = vec![0f64; nb];
            pio_nodal_demand(c, pd.as_mut_ptr(), qd.as_mut_ptr());
            assert!(pd.iter().sum::<f64>() > 0.0, "case30 has active demand");

            let mut gs = vec![0f64; nb];
            let mut bs = vec![0f64; nb];
            pio_nodal_shunt(c, gs.as_mut_ptr(), bs.as_mut_ptr());
            assert!(gs.iter().chain(bs.iter()).all(|x| x.is_finite()));

            pio_network_free(c);
        }
    }

    #[test]
    fn null_handle_and_null_out_are_safe() {
        // Every query tolerates a NULL handle (the documented safe default), and
        // a NULL output pointer on a valid case is skipped, not dereferenced.
        unsafe {
            let nil: *const PioNetwork = std::ptr::null();
            assert_eq!(pio_n_buses(nil), 0);
            assert_eq!(pio_n_branches(nil), 0);
            assert_eq!(pio_n_gens(nil), 0);
            assert_eq!(pio_base_mva(nil), 0.0);
            assert_eq!(pio_reference_bus(nil), -1);
            assert_eq!(pio_n_reference_buses(nil), 0);
            assert_eq!(pio_is_radial(nil), 0);
            assert_eq!(pio_n_components(nil), 0);

            // The two FFI constructors reject a NULL input rather than crash.
            let mut err = [0 as c_char; 128];
            assert!(pio_to_normalized(nil, err.as_mut_ptr(), err.len()).is_null());
            let fmt = CString::new("matpower").unwrap();
            assert!(
                pio_parse_str(std::ptr::null(), fmt.as_ptr(), err.as_mut_ptr(), err.len())
                    .is_null()
            );

            let c = case9();
            pio_bus_ids(c, std::ptr::null_mut());
            pio_reference_buses(c, std::ptr::null_mut());
            pio_nodal_demand(c, std::ptr::null_mut(), std::ptr::null_mut());
            pio_gens(
                c,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            pio_network_free(c);
        }
    }

    #[test]
    fn normalized_multi_ref_is_legible() {
        // A two-slack case (both gen-backed file REF buses) normalizes to a
        // handle that keeps both references. `pio_reference_bus` can't name a
        // single slack (returns -1), but the reference-set accessors do, so a C
        // consumer can tell "two slacks, you pick" from "no slack, broken".
        let src = "\
function mpc = tworef
mpc.version = '2';
mpc.baseMVA = 100;
mpc.bus = [
\t1\t3\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
\t2\t3\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
\t3\t1\t50\t10\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
];
mpc.gen = [
\t1\t0\t0\t100\t-100\t1\t100\t1\t100\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0;
\t2\t0\t0\t100\t-100\t1\t100\t1\t300\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0;
];
mpc.branch = [
\t1\t2\t0.01\t0.1\t0\t0\t0\t0\t0\t0\t1\t-360\t360;
\t2\t3\t0.01\t0.1\t0\t0\t0\t0\t0\t0\t1\t-360\t360;
];
";
        let text = CString::new(src).unwrap();
        let fmt = CString::new("matpower").unwrap();
        let mut err = [0 as c_char; 256];
        unsafe {
            let cs = pio_parse_str(text.as_ptr(), fmt.as_ptr(), err.as_mut_ptr(), err.len());
            assert!(!cs.is_null(), "parse_str returned null");
            let cn = pio_to_normalized(cs, err.as_mut_ptr(), err.len());
            assert!(!cn.is_null(), "to_normalized returned null");

            assert_eq!(pio_n_reference_buses(cn), 2);
            // Multiple references: the single-slack query reports -1, by design.
            assert_eq!(pio_reference_bus(cn), -1);
            let mut refs = vec![0i64; pio_n_reference_buses(cn)];
            pio_reference_buses(cn, refs.as_mut_ptr());
            assert_eq!(refs, vec![0, 1]);

            pio_network_free(cn);
            pio_network_free(cs);
        }
    }

    #[test]
    fn convert_emits_warning_into_buffer() {
        // t_case9_dcline carries an HVDC dcline PSS/E can't represent; the drop
        // must reach the caller's warning buffer, not vanish.
        let path = data_path("t_case9_dcline.m");
        let to = CString::new("psse").unwrap();
        let mut warn = [0 as c_char; 512];
        let mut err = [0 as c_char; 256];
        unsafe {
            let s = pio_convert_file(
                path.as_ptr(),
                to.as_ptr(),
                std::ptr::null(),
                warn.as_mut_ptr(),
                warn.len(),
                err.as_mut_ptr(),
                err.len(),
            );
            assert!(!s.is_null());
            let w = CStr::from_ptr(warn.as_ptr()).to_str().unwrap();
            assert!(
                w.contains("dcline"),
                "expected an HVDC/dcline warning, got {w:?}"
            );
            pio_string_free(s);
        }
    }

    #[test]
    fn json_round_trip_preserves_structure() {
        // to_json -> from_json must reproduce the structured tables. case30
        // carries loads, shunts, and gen costs, so a dropped field shows up.
        let c = {
            let path = data_path("case30.m");
            let mut err = [0 as c_char; 256];
            let h = unsafe {
                pio_parse_file(path.as_ptr(), std::ptr::null(), err.as_mut_ptr(), err.len())
            };
            assert!(!h.is_null());
            h
        };
        unsafe {
            let mut err = [0 as c_char; 256];
            let json = pio_to_json(c, err.as_mut_ptr(), err.len());
            assert!(!json.is_null(), "to_json returned null");
            let text = CStr::from_ptr(json).to_str().unwrap().to_owned();
            assert!(text.contains("\"buses\""));

            let back = pio_from_json(json.cast_const(), err.as_mut_ptr(), err.len());
            assert!(!back.is_null(), "from_json returned null");
            // Counts and base survive the round trip through JSON.
            assert_eq!(pio_n_buses(back), pio_n_buses(c));
            assert_eq!(pio_n_branches(back), pio_n_branches(c));
            assert_eq!(pio_n_gens(back), pio_n_gens(c));
            assert_eq!(pio_base_mva(back), pio_base_mva(c));
            assert_eq!(pio_reference_bus(back), pio_reference_bus(c));

            pio_string_free(json);
            pio_network_free(back);
            pio_network_free(c);
        }
    }

    #[test]
    fn from_json_rejects_garbage() {
        let bad = CString::new("{ not json").unwrap();
        let mut err = [0 as c_char; 256];
        let h = unsafe { pio_from_json(bad.as_ptr(), err.as_mut_ptr(), err.len()) };
        assert!(h.is_null());
        let msg = unsafe { CStr::from_ptr(err.as_ptr()) }.to_str().unwrap();
        assert!(!msg.is_empty(), "expected a JSON parse error message");
    }

    #[test]
    fn to_json_null_handle_is_safe() {
        let mut err = [0 as c_char; 256];
        let s = unsafe { pio_to_json(std::ptr::null(), err.as_mut_ptr(), err.len()) };
        assert!(s.is_null());
    }

    #[test]
    fn error_buffer_truncates_and_nul_terminates() {
        // copy_to_buf must truncate an oversized message to fit and keep the
        // trailing NUL (the one piece of pointer arithmetic in the file).
        let path = CString::new("/no/such/directory/deeply/nested/missing/case.m").unwrap();
        let mut err = [0x7f as c_char; 16]; // prefill nonzero so the NUL is visible
        let c =
            unsafe { pio_parse_file(path.as_ptr(), std::ptr::null(), err.as_mut_ptr(), err.len()) };
        assert!(c.is_null());
        let nul = err
            .iter()
            .position(|&b| b == 0)
            .expect("buffer must be NUL-terminated");
        assert!(nul <= 15);
    }

    #[cfg(feature = "arrow")]
    #[test]
    fn export_arrow_null_out_params_return_error() {
        // A NULL out_array/out_schema must be reported (-1), not dereferenced.
        let c = case9();
        let mut err = [0 as c_char; 256];
        let rc = unsafe {
            pio_export_arrow(
                c,
                PIO_ARROW_TABLE_BUS,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                err.as_mut_ptr(),
                err.len(),
            )
        };
        assert_eq!(rc, -1);
        let msg = unsafe { CStr::from_ptr(err.as_ptr()) }.to_str().unwrap();
        assert!(!msg.is_empty(), "expected an error message");
        unsafe { pio_network_free(c) };
    }
}
