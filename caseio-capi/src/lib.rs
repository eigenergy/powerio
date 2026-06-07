//! C ABI for `caseio` — the polyglot substrate.
//!
//! Parse any supported power-system case format into an opaque handle, query it,
//! convert losslessly to another format, and pull out the numeric tables a
//! downstream solver needs to assemble matrices. Every entry point is `extern
//! "C"`, catches panics at the boundary, and returns error text into a
//! caller-provided buffer. Strings handed back are owned by the library; free
//! them with [`cio_string_free`]. Array extractors fill caller-allocated
//! buffers (length = the matching `cio_n_*` count); pass `NULL` to skip one.
//!
//! Naming: every symbol is prefixed `cio_`. The header is `include/caseio.h`.

#![allow(clippy::missing_safety_doc)]

use std::ffi::{c_char, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};

use caseio::{IndexedNetwork, Network, TargetFormat};

/// Opaque parsed case handle.
pub struct CioCase {
    net: Network,
}

/// Copy `msg` (truncated to fit) into a caller `char[len]` buffer, always
/// NUL-terminated. Shared by the error and warning outputs.
unsafe fn copy_to_buf(buf: *mut c_char, len: usize, msg: &str) {
    if buf.is_null() || len == 0 {
        return;
    }
    let bytes = msg.as_bytes();
    let n = bytes.len().min(len - 1);
    std::ptr::copy_nonoverlapping(bytes.as_ptr().cast::<c_char>(), buf, n);
    *buf.add(n) = 0;
}

unsafe fn cstr<'a>(p: *const c_char) -> Option<&'a str> {
    if p.is_null() {
        return None;
    }
    CStr::from_ptr(p).to_str().ok()
}

fn into_cstring(s: String) -> *mut c_char {
    match CString::new(s) {
        Ok(c) => c.into_raw(),
        // A NUL in the text (shouldn't happen for `.m`/JSON) can't go in a C
        // string; hand back an empty string rather than fail.
        Err(_) => CString::default().into_raw(),
    }
}

fn read_network(path: &str, from: Option<&str>) -> Result<Network, String> {
    let p = std::path::Path::new(path);
    let fmt = match from {
        Some(f) => caseio::target_format_from_name(f)
            .ok_or_else(|| format!("unknown source format: {f}"))?,
        None => match p.extension().and_then(|e| e.to_str()) {
            Some("m") => TargetFormat::Matpower,
            Some("json") => TargetFormat::PowerModelsJson,
            Some("raw") => TargetFormat::Psse,
            Some("aux") => TargetFormat::PowerWorld,
            other => return Err(format!("cannot infer input format from extension {other:?}")),
        },
    };
    let read = || std::fs::read_to_string(p).map_err(|e| e.to_string());
    match fmt {
        TargetFormat::Matpower => caseio::parse_matpower_file(path).map_err(|e| e.to_string()),
        TargetFormat::PowerModelsJson => {
            caseio::parse_powermodels_json(&read()?).map_err(|e| e.to_string())
        }
        TargetFormat::Psse => caseio::parse_psse(&read()?).map_err(|e| e.to_string()),
        TargetFormat::PowerWorld => caseio::parse_powerworld(&read()?).map_err(|e| e.to_string()),
        TargetFormat::EgretJson => {
            Err("reading EGRET JSON is not supported yet (write-only)".into())
        }
    }
}

/// Parse `path` (format from extension, or `from` if non-NULL) into a case
/// handle. Returns `NULL` on error and writes the message into `errbuf`.
#[no_mangle]
pub unsafe extern "C" fn cio_parse(
    path: *const c_char,
    from: *const c_char,
    errbuf: *mut c_char,
    errlen: usize,
) -> *mut CioCase {
    let r = catch_unwind(AssertUnwindSafe(|| {
        let path = cstr(path).ok_or_else(|| "path is NULL or not UTF-8".to_string())?;
        let from = if from.is_null() { None } else { cstr(from) };
        read_network(path, from).map(|net| Box::into_raw(Box::new(CioCase { net })))
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

/// Free a case handle from [`cio_parse`].
#[no_mangle]
pub unsafe extern "C" fn cio_case_free(case: *mut CioCase) {
    if !case.is_null() {
        drop(Box::from_raw(case));
    }
}

unsafe fn case_ref<'a>(case: *const CioCase) -> Option<&'a Network> {
    case.as_ref().map(|c| &c.net)
}

#[no_mangle]
pub unsafe extern "C" fn cio_n_buses(case: *const CioCase) -> usize {
    case_ref(case).map_or(0, |n| n.buses.len())
}

#[no_mangle]
pub unsafe extern "C" fn cio_n_branches(case: *const CioCase) -> usize {
    case_ref(case).map_or(0, |n| n.branches.len())
}

#[no_mangle]
pub unsafe extern "C" fn cio_n_gens(case: *const CioCase) -> usize {
    case_ref(case).map_or(0, |n| n.generators.len())
}

#[no_mangle]
pub unsafe extern "C" fn cio_base_mva(case: *const CioCase) -> f64 {
    case_ref(case).map_or(0.0, |n| n.base_mva)
}

/// Dense `[0, n)` index of the single reference bus, or `-1` if not exactly one.
#[no_mangle]
pub unsafe extern "C" fn cio_reference_bus(case: *const CioCase) -> isize {
    match case_ref(case) {
        Some(net) => IndexedNetwork::new(net)
            .reference_bus_index()
            .map_or(-1, |i| i as isize),
        None => -1,
    }
}

#[no_mangle]
pub unsafe extern "C" fn cio_n_components(case: *const CioCase) -> usize {
    case_ref(case).map_or(0, |net| IndexedNetwork::new(net).n_connected_components())
}

/// `1` if the in-service topology is a forest, else `0`.
#[no_mangle]
pub unsafe extern "C" fn cio_is_radial(case: *const CioCase) -> i32 {
    case_ref(case).map_or(0, |net| i32::from(IndexedNetwork::new(net).is_radial()))
}

/// Serialize back to MATPOWER `.m` (byte-exact echo when parsed from MATPOWER).
/// Returns an owned C string; free with [`cio_string_free`].
#[no_mangle]
pub unsafe extern "C" fn cio_write_matpower(case: *const CioCase) -> *mut c_char {
    match case_ref(case) {
        Some(net) => into_cstring(caseio::write_matpower(net)),
        None => std::ptr::null_mut(),
    }
}

/// Convert `path` to format `to` (optionally forcing the source via `from`).
/// Returns the converted text as an owned C string (free with
/// [`cio_string_free`]), `NULL` on error. Fidelity warnings, if any, are written
/// `\n`-joined into `warnbuf`.
#[no_mangle]
pub unsafe extern "C" fn cio_convert(
    path: *const c_char,
    to: *const c_char,
    from: *const c_char,
    warnbuf: *mut c_char,
    warnlen: usize,
    errbuf: *mut c_char,
    errlen: usize,
) -> *mut c_char {
    let r = catch_unwind(AssertUnwindSafe(|| {
        let path = cstr(path).ok_or_else(|| "path is NULL or not UTF-8".to_string())?;
        let to = cstr(to).ok_or_else(|| "to is NULL or not UTF-8".to_string())?;
        let from = if from.is_null() { None } else { cstr(from) };
        let target = caseio::target_format_from_name(to)
            .ok_or_else(|| format!("unknown target format: {to}"))?;
        let net = read_network(path, from)?;
        let conv = caseio::write_as(&net, target);
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

/// Free a string returned by [`cio_write_matpower`] or [`cio_convert`].
#[no_mangle]
pub unsafe extern "C" fn cio_string_free(s: *mut c_char) {
    if !s.is_null() {
        drop(CString::from_raw(s));
    }
}

unsafe fn fill<T: Copy>(ptr: *mut T, vals: impl Iterator<Item = T>) {
    if ptr.is_null() {
        return;
    }
    for (i, v) in vals.enumerate() {
        *ptr.add(i) = v;
    }
}

/// Fill `out` (length `cio_n_buses`) with the 1-based bus ids in dense order.
#[no_mangle]
pub unsafe extern "C" fn cio_bus_ids(case: *const CioCase, out: *mut i64) {
    if let Some(net) = case_ref(case) {
        fill(out, net.buses.iter().map(|b| b.id as i64));
    }
}

/// Fill the branch tables (each length `cio_n_branches`, dense bus indices for
/// `from`/`to` resolved against the case). Any pointer may be `NULL` to skip.
#[no_mangle]
pub unsafe extern "C" fn cio_branches(
    case: *const CioCase,
    from: *mut i64,
    to: *mut i64,
    r: *mut f64,
    x: *mut f64,
    b: *mut f64,
    tap: *mut f64,
    shift: *mut f64,
    in_service: *mut u8,
) {
    let Some(net) = case_ref(case) else { return };
    let view = IndexedNetwork::new(net);
    let idx = |id: usize| view.bus_index(id).map_or(-1i64, |i| i as i64);
    fill(from, net.branches.iter().map(|br| idx(br.from)));
    fill(to, net.branches.iter().map(|br| idx(br.to)));
    fill(r, net.branches.iter().map(|br| br.r));
    fill(x, net.branches.iter().map(|br| br.x));
    fill(b, net.branches.iter().map(|br| br.b));
    fill(tap, net.branches.iter().map(|br| br.tap));
    fill(shift, net.branches.iter().map(|br| br.shift));
    fill(in_service, net.branches.iter().map(|br| u8::from(br.in_service)));
}

/// Fill the generator tables (each length `cio_n_gens`; `bus` is a dense index).
/// Any pointer may be `NULL` to skip.
#[no_mangle]
pub unsafe extern "C" fn cio_gens(
    case: *const CioCase,
    bus: *mut i64,
    pg: *mut f64,
    pmax: *mut f64,
    pmin: *mut f64,
    in_service: *mut u8,
) {
    let Some(net) = case_ref(case) else { return };
    let view = IndexedNetwork::new(net);
    let idx = |id: usize| view.bus_index(id).map_or(-1i64, |i| i as i64);
    fill(bus, net.generators.iter().map(|g| idx(g.bus)));
    fill(pg, net.generators.iter().map(|g| g.pg));
    fill(pmax, net.generators.iter().map(|g| g.pmax));
    fill(pmin, net.generators.iter().map(|g| g.pmin));
    fill(in_service, net.generators.iter().map(|g| u8::from(g.in_service)));
}

/// Fill nodal aggregates (each length `cio_n_buses`, dense order): active and
/// reactive demand summed per bus. Any pointer may be `NULL`.
#[no_mangle]
pub unsafe extern "C" fn cio_nodal_demand(case: *const CioCase, pd: *mut f64, qd: *mut f64) {
    if let Some(net) = case_ref(case) {
        let view = IndexedNetwork::new(net);
        fill(pd, view.pd().iter().copied());
        fill(qd, view.qd().iter().copied());
    }
}

/// Fill nodal shunt aggregates (each length `cio_n_buses`, dense order).
#[no_mangle]
pub unsafe extern "C" fn cio_nodal_shunt(case: *const CioCase, gs: *mut f64, bs: *mut f64) {
    if let Some(net) = case_ref(case) {
        let view = IndexedNetwork::new(net);
        fill(gs, view.gs().iter().copied());
        fill(bs, view.bs().iter().copied());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    fn case9() -> *mut CioCase {
        let path = CString::new(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../tests/data/case9.m")
                .to_str()
                .unwrap(),
        )
        .unwrap();
        let mut err = [0 as c_char; 256];
        let c = unsafe { cio_parse(path.as_ptr(), std::ptr::null(), err.as_mut_ptr(), err.len()) };
        assert!(!c.is_null(), "parse returned null");
        c
    }

    #[test]
    fn parse_query_free() {
        let c = case9();
        unsafe {
            assert_eq!(cio_n_buses(c), 9);
            assert_eq!(cio_n_branches(c), 9);
            assert_eq!(cio_n_gens(c), 3);
            assert_eq!(cio_base_mva(c), 100.0);
            assert_eq!(cio_n_components(c), 1);
            assert!(cio_reference_bus(c) >= 0);
            cio_case_free(c);
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
            let s = cio_write_matpower(c);
            assert!(!s.is_null());
            let got = CStr::from_ptr(s).to_str().unwrap();
            assert_eq!(got, src);
            cio_string_free(s);
            cio_case_free(c);
        }
    }

    #[test]
    fn extract_branch_tables() {
        let c = case9();
        unsafe {
            let nb = cio_n_branches(c);
            let mut from = vec![0i64; nb];
            let mut x = vec![0f64; nb];
            cio_branches(
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
            cio_case_free(c);
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
            let s = cio_convert(
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
            cio_string_free(s);
        }
    }

    #[test]
    fn parse_error_sets_message_not_null_handle() {
        let path = CString::new("/no/such/case.m").unwrap();
        let mut err = [0 as c_char; 256];
        let c = unsafe { cio_parse(path.as_ptr(), std::ptr::null(), err.as_mut_ptr(), err.len()) };
        assert!(c.is_null());
        let msg = unsafe { CStr::from_ptr(err.as_ptr()) }.to_str().unwrap();
        assert!(!msg.is_empty(), "expected an error message");
    }
}
