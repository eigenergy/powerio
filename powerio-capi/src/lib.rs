//! C ABI for `powerio` — the polyglot substrate.
//!
//! Parse any supported power-system case format into an opaque handle, query it,
//! convert losslessly to another format, and pull out the numeric tables a
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

use powerio::{IndexCore, IndexedNetwork, Network};

/// Opaque parsed case handle. Carries the parsed [`Network`] plus the
/// [`IndexCore`] derived from it once at parse time, so every indexed query
/// reuses the same bus-id map and nodal aggregates instead of rebuilding them.
pub struct PioCase {
    net: Network,
    core: IndexCore,
}

/// Copy `msg` (truncated to fit) into a caller `char[len]` buffer, always
/// NUL-terminated. Shared by the error and warning outputs.
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

fn into_cstring(s: String) -> *mut c_char {
    match CString::new(s) {
        Ok(c) => c.into_raw(),
        // A NUL in the text (shouldn't happen for `.m`/JSON) can't go in a C
        // string; hand back an empty string rather than fail.
        Err(_) => CString::default().into_raw(),
    }
}

/// Run `f` at the FFI boundary, catching any panic so it can't unwind across
/// `extern "C"` (UB). Returns `fallback` if `f` panics.
unsafe fn guard<R>(fallback: R, f: impl FnOnce() -> R) -> R {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(fallback)
}

/// Parse `path` (format from extension, or `from` if non-NULL) into a case
/// handle. Returns `NULL` on error and writes the message into `errbuf`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_parse(
    path: *const c_char,
    from: *const c_char,
    errbuf: *mut c_char,
    errlen: usize,
) -> *mut PioCase {
    unsafe {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let path = cstr(path).ok_or_else(|| "path is NULL or not UTF-8".to_string())?;
            let from = if from.is_null() { None } else { cstr(from) };
            powerio::read_path(std::path::Path::new(path), from)
                .map_err(|e| e.to_string())
                .map(|net| {
                    let core = IndexCore::build(&net);
                    Box::into_raw(Box::new(PioCase { net, core }))
                })
        }));
        match r {
            Ok(Ok(ptr)) => ptr,
            Ok(Err(msg)) => {
                copy_to_buf(errbuf, errlen, &msg);
                std::ptr::null_mut()
            }
            Err(_) => {
                copy_to_buf(errbuf, errlen, "panic while parsing");
                std::ptr::null_mut()
            }
        }
    }
}

/// Free a case handle from [`pio_parse`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_case_free(case: *mut PioCase) {
    unsafe {
        if !case.is_null() {
            drop(Box::from_raw(case));
        }
    }
}

unsafe fn case_ref<'a>(case: *const PioCase) -> Option<&'a PioCase> {
    unsafe { case.as_ref() }
}

/// View `case` through its cached [`IndexCore`] — no per-call rebuild.
unsafe fn view<'a>(case: *const PioCase) -> Option<IndexedNetwork<'a>> {
    unsafe {
        case.as_ref()
            .map(|c| IndexedNetwork::with_core(&c.net, &c.core))
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_n_buses(case: *const PioCase) -> usize {
    unsafe { guard(0, || case_ref(case).map_or(0, |c| c.net.buses.len())) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_n_branches(case: *const PioCase) -> usize {
    unsafe { guard(0, || case_ref(case).map_or(0, |c| c.net.branches.len())) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_n_gens(case: *const PioCase) -> usize {
    unsafe { guard(0, || case_ref(case).map_or(0, |c| c.net.generators.len())) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_base_mva(case: *const PioCase) -> f64 {
    unsafe { guard(0.0, || case_ref(case).map_or(0.0, |c| c.net.base_mva)) }
}

/// Dense `[0, n)` index of the single reference bus, or `-1` if not exactly one
/// (also `-1` if the index is too large for `isize`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_reference_bus(case: *const PioCase) -> isize {
    unsafe {
        guard(-1, || match view(case) {
            Some(v) => v
                .reference_bus_index()
                .map_or(-1, |i| isize::try_from(i).unwrap_or(-1)),
            None => -1,
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_n_components(case: *const PioCase) -> usize {
    unsafe { guard(0, || view(case).map_or(0, |v| v.n_connected_components())) }
}

/// `1` if the in-service topology is a forest, else `0`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_is_radial(case: *const PioCase) -> i32 {
    unsafe { guard(0, || view(case).map_or(0, |v| i32::from(v.is_radial()))) }
}

/// Serialize back to MATPOWER `.m` (byte-exact echo when parsed from MATPOWER).
/// Returns an owned C string; free with [`pio_string_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_write_matpower(case: *const PioCase) -> *mut c_char {
    unsafe {
        guard(std::ptr::null_mut(), || match case_ref(case) {
            Some(c) => into_cstring(powerio::write_matpower(&c.net)),
            None => std::ptr::null_mut(),
        })
    }
}

/// Convert `path` to format `to` (optionally forcing the source via `from`).
/// Returns the converted text as an owned C string (free with
/// [`pio_string_free`]), `NULL` on error. Fidelity warnings, if any, are written
/// `\n`-joined into `warnbuf`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_convert(
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
            let to = cstr(to).ok_or_else(|| "to is NULL or not UTF-8".to_string())?;
            let from = if from.is_null() { None } else { cstr(from) };
            let target = powerio::target_format_from_name(to)
                .ok_or_else(|| format!("unknown target format: {to}"))?;
            let net =
                powerio::read_path(std::path::Path::new(path), from).map_err(|e| e.to_string())?;
            let conv = powerio::write_as(&net, target);
            Ok::<_, String>((conv.text, conv.warnings))
        }));
        match r {
            Ok(Ok((text, warnings))) => {
                copy_to_buf(warnbuf, warnlen, &warnings.join("\n"));
                into_cstring(text)
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

/// Free a string returned by [`pio_write_matpower`], [`pio_convert`], or
/// [`pio_to_json`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_string_free(s: *mut c_char) {
    unsafe {
        if !s.is_null() {
            drop(CString::from_raw(s));
        }
    }
}

/// Serialize the case to JSON — the structured-table transport every Julia
/// bridge consumes. Carries the whole [`Network`] (buses, loads, shunts,
/// branches, generators, storage, HVDC, extras) but not the retained source
/// text, so it is structured data, not the byte-exact echo. Returns an owned C
/// string (free with [`pio_string_free`]), `NULL` on error (message into
/// `errbuf`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_to_json(
    case: *const PioCase,
    errbuf: *mut c_char,
    errlen: usize,
) -> *mut c_char {
    unsafe {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let c = case_ref(case).ok_or_else(|| "case is NULL".to_string())?;
            c.net.to_json().map_err(|e| e.to_string())
        }));
        match r {
            Ok(Ok(json)) => into_cstring(json),
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

/// Rebuild a case handle from JSON produced by [`pio_to_json`]. Returns a new
/// handle (free with [`pio_case_free`]), or `NULL` on error (message into
/// `errbuf`). The handle has no retained source, so [`pio_write_matpower`]
/// reformats it rather than echoing a byte-exact original.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_from_json(
    json: *const c_char,
    errbuf: *mut c_char,
    errlen: usize,
) -> *mut PioCase {
    unsafe {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let json = cstr(json).ok_or_else(|| "json is NULL or not UTF-8".to_string())?;
            Network::from_json(json)
                .map_err(|e| e.to_string())
                .map(|net| {
                    let core = IndexCore::build(&net);
                    Box::into_raw(Box::new(PioCase { net, core }))
                })
        }));
        match r {
            Ok(Ok(ptr)) => ptr,
            Ok(Err(msg)) => {
                copy_to_buf(errbuf, errlen, &msg);
                std::ptr::null_mut()
            }
            Err(_) => {
                copy_to_buf(errbuf, errlen, "panic while parsing JSON");
                std::ptr::null_mut()
            }
        }
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
pub unsafe extern "C" fn pio_bus_ids(case: *const PioCase, out: *mut i64) {
    unsafe {
        guard((), || {
            if let Some(c) = case_ref(case) {
                fill(
                    out,
                    c.net
                        .buses
                        .iter()
                        .map(|b| i64::try_from(b.id).unwrap_or(-1)),
                );
            }
        })
    }
}

/// Fill the branch tables (each length `pio_n_branches`, dense bus indices for
/// `from`/`to` resolved against the case). Any pointer may be `NULL` to skip.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_branches(
    case: *const PioCase,
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
            let Some(c) = case_ref(case) else { return };
            let view = IndexedNetwork::with_core(&c.net, &c.core);
            let idx = |id: usize| {
                view.bus_index(id)
                    .map_or(-1, |i| i64::try_from(i).unwrap_or(-1))
            };
            let net = &c.net;
            fill(from, net.branches.iter().map(|br| idx(br.from)));
            fill(to, net.branches.iter().map(|br| idx(br.to)));
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

/// Fill the generator tables (each length `pio_n_gens`; `bus` is a dense index).
/// Any pointer may be `NULL` to skip.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_gens(
    case: *const PioCase,
    bus: *mut i64,
    pg: *mut f64,
    pmax: *mut f64,
    pmin: *mut f64,
    in_service: *mut u8,
) {
    unsafe {
        guard((), || {
            let Some(c) = case_ref(case) else { return };
            let view = IndexedNetwork::with_core(&c.net, &c.core);
            let idx = |id: usize| {
                view.bus_index(id)
                    .map_or(-1, |i| i64::try_from(i).unwrap_or(-1))
            };
            let net = &c.net;
            fill(bus, net.generators.iter().map(|g| idx(g.bus)));
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
pub unsafe extern "C" fn pio_nodal_demand(case: *const PioCase, pd: *mut f64, qd: *mut f64) {
    unsafe {
        guard((), || {
            if let Some(v) = view(case) {
                fill(pd, v.pd().iter().copied());
                fill(qd, v.qd().iter().copied());
            }
        })
    }
}

/// Fill nodal shunt aggregates (each length `pio_n_buses`, dense order).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_nodal_shunt(case: *const PioCase, gs: *mut f64, bs: *mut f64) {
    unsafe {
        guard((), || {
            if let Some(v) = view(case) {
                fill(gs, v.gs().iter().copied());
                fill(bs, v.bs().iter().copied());
            }
        })
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

    fn case9() -> *mut PioCase {
        let path = CString::new(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../tests/data/case9.m")
                .to_str()
                .unwrap(),
        )
        .unwrap();
        let mut err = [0 as c_char; 256];
        let c = unsafe { pio_parse(path.as_ptr(), std::ptr::null(), err.as_mut_ptr(), err.len()) };
        assert!(!c.is_null(), "parse returned null");
        c
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
            pio_case_free(c);
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
            let s = pio_write_matpower(c);
            assert!(!s.is_null());
            let got = CStr::from_ptr(s).to_str().unwrap();
            assert_eq!(got, src);
            pio_string_free(s);
            pio_case_free(c);
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
            // case9 is connected: every branch resolves to real dense indices.
            assert!(from.iter().all(|&f| f >= 0));
            assert!(x.iter().all(|&xx| xx > 0.0));
            pio_case_free(c);
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
            let s = pio_convert(
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
    fn parse_error_sets_message_not_null_handle() {
        let path = CString::new("/no/such/case.m").unwrap();
        let mut err = [0 as c_char; 256];
        let c = unsafe { pio_parse(path.as_ptr(), std::ptr::null(), err.as_mut_ptr(), err.len()) };
        assert!(c.is_null());
        let msg = unsafe { CStr::from_ptr(err.as_ptr()) }.to_str().unwrap();
        assert!(!msg.is_empty(), "expected an error message");
    }

    #[test]
    fn extract_gen_and_nodal_tables() {
        // case30 carries generators, loads, and shunts: cross-check the table
        // extractors against known counts and aggregate signs (a column swap in
        // pio_gens/pio_nodal_* would otherwise ship silently).
        let path = data_path("case30.m");
        let mut err = [0 as c_char; 256];
        let c = unsafe { pio_parse(path.as_ptr(), std::ptr::null(), err.as_mut_ptr(), err.len()) };
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

            pio_case_free(c);
        }
    }

    #[test]
    fn null_handle_and_null_out_are_safe() {
        // Every query tolerates a NULL handle (the documented safe default), and
        // a NULL output pointer on a valid case is skipped, not dereferenced.
        unsafe {
            let nil: *const PioCase = std::ptr::null();
            assert_eq!(pio_n_buses(nil), 0);
            assert_eq!(pio_n_branches(nil), 0);
            assert_eq!(pio_n_gens(nil), 0);
            assert_eq!(pio_base_mva(nil), 0.0);
            assert_eq!(pio_reference_bus(nil), -1);
            assert_eq!(pio_is_radial(nil), 0);
            assert_eq!(pio_n_components(nil), 0);

            let c = case9();
            pio_bus_ids(c, std::ptr::null_mut());
            pio_nodal_demand(c, std::ptr::null_mut(), std::ptr::null_mut());
            pio_gens(
                c,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            pio_case_free(c);
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
            let s = pio_convert(
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
            let h =
                unsafe { pio_parse(path.as_ptr(), std::ptr::null(), err.as_mut_ptr(), err.len()) };
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
            pio_case_free(back);
            pio_case_free(c);
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
        let c = unsafe { pio_parse(path.as_ptr(), std::ptr::null(), err.as_mut_ptr(), err.len()) };
        assert!(c.is_null());
        let nul = err
            .iter()
            .position(|&b| b == 0)
            .expect("buffer must be NUL-terminated");
        assert!(nul <= 15);
    }
}
