//! ABI v6: opaque owned handles with `retain`/`release`, structured
//! [`PioError`] handles, the stored module surface, and the DC branch data
//! a consumer needs to assemble or differentiate its own formulation.
//!
//! Ownership rules, stated once and repeated in `powerio.h`:
//!
//! - Every handle is an independently owned reference. `retain` mints a new
//!   handle over the same immutable value; `release` drops one handle.
//!   `release(NULL)` is a no-op. Releasing a parent never invalidates a
//!   child or a retained sibling.
//! - Accessors on a result handle return immutable spans (pointer and
//!   length) that stay valid until that handle's last `release`.
//! - Concurrent immutable calls on one handle are allowed. Releasing a raw
//!   handle concurrently with any call on that same raw handle is caller
//!   error.
//! - Fallible entry points take a `PioError**` out parameter (NULL to
//!   ignore). On failure they return NULL (or false) and, when the out
//!   parameter is non NULL, store a new error handle the caller releases
//!   with `pio_error_release`. Panics never unwind across the boundary;
//!   they become `BIND.CAPI.PANIC` errors.

use std::ffi::{CStr, CString, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use powerio::{BalancedNetwork, DcConvention, IndexedNetwork};

use crate::diagnostics::codes;

// ---- handle machinery -------------------------------------------------------

/// One raw C handle: a boxed strong reference. The box is the handle the C
/// caller owns; the [`Arc`] owns the shared immutable value. These helpers
/// are the whole unsafe ownership core, kept dependency free so `cargo miri
/// test` can drive them directly. Each C-visible opaque type wraps one
/// `HandleBox` through the `arc_handle!` macro, so every type shares this exact
/// lifecycle.
#[repr(transparent)]
pub struct HandleBox<T> {
    pub(crate) inner: Arc<T>,
}

pub(crate) fn handle_new<T>(value: T) -> *mut HandleBox<T> {
    Box::into_raw(Box::new(HandleBox {
        inner: Arc::new(value),
    }))
}

/// Mint an independent handle over the same value. NULL stays NULL.
pub(crate) unsafe fn handle_retain<T>(raw: *const HandleBox<T>) -> *mut HandleBox<T> {
    let Some(handle) = (unsafe { raw.as_ref() }) else {
        return std::ptr::null_mut();
    };
    Box::into_raw(Box::new(HandleBox {
        inner: Arc::clone(&handle.inner),
    }))
}

/// Drop one handle. NULL is a no-op.
pub(crate) unsafe fn handle_release<T>(raw: *mut HandleBox<T>) {
    if !raw.is_null() {
        drop(unsafe { Box::from_raw(raw) });
    }
}

/// Borrow the shared value behind a handle.
pub(crate) unsafe fn handle_ref<'a, T>(raw: *const HandleBox<T>) -> Option<&'a T> {
    unsafe { raw.as_ref() }.map(|handle| handle.inner.as_ref())
}

/// Declare one C-visible opaque handle type over an inner value type, with
/// the shared new/retain/release/borrow lifecycle. `#[repr(transparent)]`
/// over [`HandleBox`] keeps the raw pointer casts sound.
macro_rules! arc_handle {
    ($(#[$doc:meta])* $name:ident, $inner:ty) => {
        $(#[$doc])*
        #[repr(transparent)]
        pub struct $name($crate::v6::HandleBox<$inner>);

        impl $name {
            /// The four lifecycle entries are generated for every handle
            /// type; a given type may construct or borrow through another
            /// path.
            #[allow(dead_code)]
            pub(crate) fn new_raw(value: $inner) -> *mut $name {
                $crate::v6::handle_new(value).cast()
            }

            /// Wrap a value for a caller that boxes the handle itself.
            #[allow(dead_code)]
            pub(crate) fn wrap(value: $inner) -> $name {
                $name($crate::v6::HandleBox {
                    inner: std::sync::Arc::new(value),
                })
            }

            #[allow(dead_code)]
            pub(crate) unsafe fn retain_raw(raw: *const $name) -> *mut $name {
                unsafe { $crate::v6::handle_retain(raw.cast::<$crate::v6::HandleBox<$inner>>()) }
                    .cast()
            }

            #[allow(dead_code)]
            pub(crate) unsafe fn release_raw(raw: *mut $name) {
                unsafe { $crate::v6::handle_release(raw.cast::<$crate::v6::HandleBox<$inner>>()) }
            }

            #[allow(dead_code)]
            pub(crate) unsafe fn get<'a>(raw: *const $name) -> Option<&'a $inner> {
                unsafe { $crate::v6::handle_ref(raw.cast::<$crate::v6::HandleBox<$inner>>()) }
            }
        }

        impl std::ops::Deref for $name {
            type Target = $inner;

            fn deref(&self) -> &$inner {
                self.0.inner.as_ref()
            }
        }
    };
}

pub(crate) use arc_handle;

// ---- pio_error --------------------------------------------------------------

/// A structured failure: stable code, rendered message, and the structured
/// diagnostics as JSON. Strings are NUL terminated and live as long as the
/// handle.
pub struct ErrorInner {
    code: CString,
    message: CString,
    diagnostics_json: CString,
    /// The typed records behind [`pio_error_diagnostics`].
    records: Vec<powerio_core::Diagnostic>,
}

arc_handle!(
    /// The opaque C error type.
    PioError,
    ErrorInner
);

fn lossy_cstring(text: &str) -> CString {
    CString::new(text.replace('\0', "\u{fffd}")).expect("interior NULs replaced")
}

/// Build an error handle from its structured findings. `records` becomes both
/// `pio_error_diagnostics` (the typed channel) and `pio_error_diagnostics_json`
/// (serialized through `Diagnostic`'s own serde form, the C ABI's documented
/// JSON channel), so the two always agree; `code` is the first record's code,
/// or the uncoded fallback when `records` is empty.
fn error_from_diagnostics(message: &str, records: Vec<powerio_core::Diagnostic>) -> *mut PioError {
    let code = records
        .first()
        .map_or(codes::BIND_CAPI_UNCODED_FAILURE.code, |diagnostic| {
            diagnostic.code()
        });
    let diagnostics_json = serde_json::to_string(&records).unwrap_or_else(|_| "[]".to_owned());
    PioError::new_raw(ErrorInner {
        code: lossy_cstring(code),
        message: lossy_cstring(message),
        diagnostics_json: lossy_cstring(&diagnostics_json),
        records,
    })
}

/// Build an error handle from a boundary-detected code and message, with no
/// richer diagnostic behind it: builds the one `Diagnostic` record itself, so
/// `pio_error_diagnostics`/`pio_error_diagnostics_json` carry the same finding
/// `pio_error_message` renders instead of coming back empty. `pio_error_message`
/// reads `CODE: message`, the shape [`error_from_core`] also reports. An `code`
/// that fails the code grammar (never true of a registered constant, but not
/// statically guaranteed) falls back to the uncoded failure code and folds the
/// original code and message into the fallback's message text.
pub(crate) fn error_from_parts(code: &str, message: &str) -> *mut PioError {
    let diagnostic = match powerio_core::DiagnosticCode::new(code) {
        Ok(diagnostic_code) => powerio_core::Diagnostic::new(
            diagnostic_code,
            powerio_core::DiagnosticSeverity::Error,
            message,
        ),
        Err(_) => powerio_core::Diagnostic::new(
            powerio_core::DiagnosticCode::new(codes::BIND_CAPI_UNCODED_FAILURE.code)
                .expect("static code is well formed"),
            powerio_core::DiagnosticSeverity::Error,
            format!("{code}: {message}"),
        ),
    };
    let rendered = powerio_core::render_diagnostic(&diagnostic);
    error_from_diagnostics(&rendered, vec![diagnostic])
}

/// Build an error handle from one rendered `CODE: message` line, the shape
/// the internal helpers report. A line with no code prefix keeps the whole
/// text as the message under the uncoded fallback.
pub(crate) fn error_from_line(line: &str) -> *mut PioError {
    let (code, message) = match line.split_once(": ") {
        Some((code, message)) if powerio_core::code_is_well_formed(code) => (code, message),
        _ => ("BIND.CAPI.UNCODED_FAILURE", line),
    };
    error_from_parts(code, message)
}

pub(crate) fn error_from_core(error: &powerio_core::Error) -> *mut PioError {
    error_from_diagnostics(&error.to_string(), error.diagnostics().to_vec())
}

fn error_panic() -> *mut PioError {
    error_from_parts(
        codes::BIND_CAPI_PANIC.code,
        "the operation panicked; the library state is unchanged",
    )
}

/// Store `error` through the out parameter, or release it when the caller
/// passed NULL.
unsafe fn store_error(out: *mut *mut PioError, error: *mut PioError) {
    if out.is_null() {
        unsafe { PioError::release_raw(error) };
    } else {
        unsafe { *out = error };
    }
}

/// Run one fallible v6 entry point: NULL out the error slot, catch panics,
/// and store the failure. `f` returns the success value or an owned error
/// handle.
pub(crate) unsafe fn v6_entry<R>(
    out: *mut *mut PioError,
    fallback: R,
    f: impl FnOnce() -> Result<R, *mut PioError>,
) -> R {
    if !out.is_null() {
        unsafe { *out = std::ptr::null_mut() };
    }
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => {
            unsafe { store_error(out, error) };
            fallback
        }
        Err(_) => {
            unsafe { store_error(out, error_panic()) };
            fallback
        }
    }
}

/// The failure's stable diagnostic code, valid until the handle's release.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_error_code(error: *const PioError) -> *const c_char {
    unsafe {
        crate::guard(std::ptr::null(), || {
            PioError::get(error).map_or(std::ptr::null(), |inner| inner.code.as_ptr())
        })
    }
}

/// The rendered failure message, valid until the handle's release.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_error_message(error: *const PioError) -> *const c_char {
    unsafe {
        crate::guard(std::ptr::null(), || {
            PioError::get(error).map_or(std::ptr::null(), |inner| inner.message.as_ptr())
        })
    }
}

/// The structured diagnostics as a JSON array, valid until the handle's
/// release.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_error_diagnostics_json(error: *const PioError) -> *const c_char {
    unsafe {
        crate::guard(std::ptr::null(), || {
            PioError::get(error).map_or(std::ptr::null(), |inner| inner.diagnostics_json.as_ptr())
        })
    }
}

/// Mint an independent handle to the same error. NULL stays NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_error_retain(error: *const PioError) -> *mut PioError {
    unsafe { crate::guard(std::ptr::null_mut(), || PioError::retain_raw(error)) }
}

/// Release one error handle. NULL is a no-op.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_error_release(error: *mut PioError) {
    unsafe { crate::guard((), || PioError::release_raw(error)) }
}

// ---- pio_module -------------------------------------------------------------

/// The runtime module: one typed value with its common records, plus the
/// rendered kind name so `pio_module_kind` can return a borrowed span.
pub struct ModuleInner {
    module: powerio_core::PioModule<powerio::PioValue>,
    kind: CString,
}

arc_handle!(
    /// The opaque C module type.
    PioModule,
    ModuleInner
);

fn module_handle(module: powerio_core::PioModule<powerio::PioValue>) -> *mut PioModule {
    let kind = lossy_cstring(module.value().kind().as_str());
    PioModule::new_raw(ModuleInner { module, kind })
}

unsafe fn required_str<'a>(raw: *const c_char, what: &str) -> Result<&'a str, *mut PioError> {
    if raw.is_null() {
        return Err(error_from_parts(
            codes::BIND_CAPI_NULL_ARGUMENT.code,
            &format!("{what} must not be NULL"),
        ));
    }
    unsafe { CStr::from_ptr(raw) }.to_str().map_err(|_| {
        error_from_parts(
            codes::BIND_CAPI_INVALID_UTF8.code,
            &format!("{what} is not valid UTF-8"),
        )
    })
}

unsafe fn optional_str<'a>(
    raw: *const c_char,
    what: &str,
) -> Result<Option<&'a str>, *mut PioError> {
    if raw.is_null() {
        return Ok(None);
    }
    unsafe { required_str(raw, what) }.map(Some)
}

unsafe fn required_module<'a>(raw: *const PioModule) -> Result<&'a ModuleInner, *mut PioError> {
    unsafe { PioModule::get(raw) }.ok_or_else(|| {
        error_from_parts(
            codes::BIND_CAPI_NULL_HANDLE.code,
            "module handle must not be NULL",
        )
    })
}

fn owned_string(text: String) -> Result<*mut c_char, *mut PioError> {
    CString::new(text).map(CString::into_raw).map_err(|_| {
        error_from_parts(
            codes::BIND_CAPI_INTERIOR_NUL.code,
            "output contained an interior NUL byte",
        )
    })
}

fn parse_source(
    source: powerio_core::Source,
    format: Option<&str>,
) -> Result<*mut PioModule, *mut PioError> {
    let source = match format {
        Some(name) => {
            let id = powerio_core::FormatId::new(name.to_ascii_lowercase().replace('_', "-"))
                .map_err(|error| error_from_core(&error))?;
            source.with_format(id)
        }
        None => source,
    };
    powerio::parse(source)
        .map(module_handle)
        .map_err(|error| error_from_core(&error))
}

/// Read stored `.pio.json` text: version 1, or a released 0.9 document
/// upgraded one way. Returns a new module handle, or NULL with `error` set.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_read_json(
    text: *const c_char,
    error: *mut *mut PioError,
) -> *mut PioModule {
    unsafe {
        v6_entry(error, std::ptr::null_mut(), || {
            let text = required_str(text, "text")?;
            powerio::stored::read_module(text)
                .map(module_handle)
                .map_err(|error| error_from_core(&error))
        })
    }
}

/// Parse a case file into a module of whichever family claims it. `format`
/// may be NULL for detection by name and content.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_parse_file(
    path: *const c_char,
    format: *const c_char,
    error: *mut *mut PioError,
) -> *mut PioModule {
    unsafe {
        v6_entry(error, std::ptr::null_mut(), || {
            let path = required_str(path, "path")?;
            let format = optional_str(format, "format")?;
            let source = powerio_core::Source::open(std::path::Path::new(path))
                .map_err(|error| error_from_core(&error))?;
            parse_source(source, format)
        })
    }
}

/// Parse in-memory case text into a module. `name` labels the buffer for
/// diagnostics and format detection; NULL uses `<memory>`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_parse_str(
    name: *const c_char,
    text: *const c_char,
    format: *const c_char,
    error: *mut *mut PioError,
) -> *mut PioModule {
    unsafe {
        v6_entry(error, std::ptr::null_mut(), || {
            let name = optional_str(name, "name")?.unwrap_or("<memory>");
            let text = required_str(text, "text")?;
            let format = optional_str(format, "format")?;
            let source = powerio_core::Source::from_bytes(name, text.as_bytes().to_vec())
                .map_err(|error| error_from_core(&error))?;
            parse_source(source, format)
        })
    }
}

/// Parse in-memory case bytes into a module: the only in-memory way to read
/// a binary format. Text formats must be UTF-8. `name` labels the buffer for
/// diagnostics and format detection; NULL uses `<memory>`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_parse_bytes(
    name: *const c_char,
    data: *const u8,
    len: usize,
    format: *const c_char,
    error: *mut *mut PioError,
) -> *mut PioModule {
    unsafe {
        v6_entry(error, std::ptr::null_mut(), || {
            let name = optional_str(name, "name")?.unwrap_or("<memory>");
            if data.is_null() {
                return Err(error_from_parts(
                    codes::BIND_CAPI_NULL_ARGUMENT.code,
                    "data must not be NULL",
                ));
            }
            let bytes = std::slice::from_raw_parts(data, len).to_vec();
            let format = optional_str(format, "format")?;
            let source = powerio_core::Source::from_bytes(name, bytes)
                .map_err(|error| error_from_core(&error))?;
            parse_source(source, format)
        })
    }
}

/// Rebuild a typed module around one value with the source module's
/// provenance threaded on: sources first (a diagnostic's span validates
/// against them), then the findings, then the retained source, so the byte
/// exact same format echo survives the module surface.
fn provenanced<T>(
    module: &powerio_core::PioModule<powerio::PioValue>,
    value: T,
) -> Result<powerio_core::PioModule<T>, *mut PioError> {
    let mut out = powerio_core::PioModule::new(value);
    for descriptor in module.sources() {
        out.add_source_descriptor(descriptor.clone())
            .map_err(|error| error_from_core(&error))?;
    }
    for diagnostic in module.diagnostics() {
        out.add_diagnostic(diagnostic.clone())
            .map_err(|error| error_from_core(&error))?;
    }
    Ok(match module.source() {
        Some(source) => out.with_source(source.clone()),
        None => out,
    })
}

/// The module's balanced network value as an owned network handle, provenance
/// included. Any other value kind is refused with the kind named.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_balanced_network(
    module: *const PioModule,
    error: *mut *mut PioError,
) -> *mut crate::PioBalancedNetwork {
    unsafe {
        v6_entry(error, std::ptr::null_mut(), || {
            let inner = required_module(module)?;
            let powerio::PioValue::BalancedNetwork(network) = inner.module.value() else {
                return Err(error_from_parts(
                    powerio::codes::REQUEST_PACKAGE_WRONG_MODEL_KIND.code,
                    &format!(
                        "the module carries a {} value; pio_module_balanced_network takes a \
                         balanced network",
                        inner.module.value().kind().as_str()
                    ),
                ));
            };
            Ok(crate::make_network_module(provenanced(
                &inner.module,
                network.clone(),
            )?))
        })
    }
}

/// The module's multiconductor network value as an owned distribution
/// handle, provenance included. Any other value kind is refused.
#[cfg(feature = "dist")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_multiconductor_network(
    module: *const PioModule,
    error: *mut *mut PioError,
) -> *mut crate::PioMulticonductorNetwork {
    unsafe {
        v6_entry(error, std::ptr::null_mut(), || {
            let inner = required_module(module)?;
            let powerio::PioValue::MulticonductorNetwork(network) = inner.module.value() else {
                return Err(error_from_parts(
                    powerio::codes::REQUEST_PACKAGE_WRONG_MODEL_KIND.code,
                    &format!(
                        "the module carries a {} value; pio_module_multiconductor_network takes \
                         a multiconductor network",
                        inner.module.value().kind().as_str()
                    ),
                ));
            };
            Ok(crate::PioMulticonductorNetwork::from_module_raw(
                provenanced(&inner.module, network.clone())?,
            ))
        })
    }
}

/// A module over one balanced network handle's value, sharing that handle's
/// records: the wrap for semantic writing of a network built in memory (for
/// example through `pio_balanced_network_from_json`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_of_balanced_network(
    network: *const crate::PioBalancedNetwork,
    error: *mut *mut PioError,
) -> *mut PioModule {
    unsafe {
        v6_entry(error, std::ptr::null_mut(), || {
            let module =
                crate::balanced_network_module(network).map_err(|line| error_from_line(&line))?;
            Ok(module_handle(
                module.map_value(powerio::PioValue::BalancedNetwork),
            ))
        })
    }
}

/// A module over one multiconductor network handle's value, sharing that
/// handle's records: the wrap for semantic writing.
#[cfg(feature = "dist")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_of_multiconductor_network(
    network: *const crate::PioMulticonductorNetwork,
    error: *mut *mut PioError,
) -> *mut PioModule {
    unsafe {
        v6_entry(error, std::ptr::null_mut(), || {
            let module = crate::multiconductor_network_module(network)
                .map_err(|line| error_from_line(&line))?;
            Ok(module_handle(
                module.map_value(powerio::PioValue::MulticonductorNetwork),
            ))
        })
    }
}

/// Write the module as the named target format and return the text: the one
/// write operation over the C surface. Writing an unchanged parsed module
/// back to its source format returns the retained bytes exactly; any other
/// target serializes the typed value. The writer's findings cross through
/// `out_diagnostics` as a structured handle (NULL discards them). Free the
/// text with `pio_string_release`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_write_str(
    module: *const PioModule,
    format: *const c_char,
    out_diagnostics: *mut *mut PioDiagnostics,
    error: *mut *mut PioError,
) -> *mut c_char {
    unsafe {
        if !out_diagnostics.is_null() {
            *out_diagnostics = std::ptr::null_mut();
        }
        v6_entry(error, std::ptr::null_mut(), || {
            let inner = required_module(module)?;
            let format = required_str(format, "format")?;
            let (text, diagnostics) = powerio::write_module_str(&inner.module, format)
                .map_err(|error| error_from_core(&error))?;
            let out = owned_string(text)?;
            if !out_diagnostics.is_null() {
                *out_diagnostics = diagnostics_handle(&diagnostics);
            }
            Ok(out)
        })
    }
}

/// Write the module as the named target format into `path`: the filesystem
/// form of [`pio_module_write_str`], covering the directory targets (PyPSA
/// CSV) a single text cannot state. The destination must not already exist.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_write_file(
    module: *const PioModule,
    format: *const c_char,
    path: *const c_char,
    out_diagnostics: *mut *mut PioDiagnostics,
    error: *mut *mut PioError,
) -> i32 {
    unsafe {
        if !out_diagnostics.is_null() {
            *out_diagnostics = std::ptr::null_mut();
        }
        v6_entry(error, -1, || {
            let inner = required_module(module)?;
            let format = required_str(format, "format")?;
            let path = required_str(path, "path")?;
            let result = powerio::write_module_as(
                &inner.module,
                format,
                powerio_core::Destination::path(path),
            )
            .map_err(|error| error_from_core(&error))?;
            if !out_diagnostics.is_null() {
                *out_diagnostics = diagnostics_handle(result.diagnostics());
            }
            Ok(0)
        })
    }
}

/// The stored version 1 document. Free with `pio_string_release`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_write_json(
    module: *const PioModule,
    error: *mut *mut PioError,
) -> *mut c_char {
    unsafe {
        v6_entry(error, std::ptr::null_mut(), || {
            let inner = required_module(module)?;
            let text = powerio::stored::write_module(&inner.module)
                .map_err(|error| error_from_core(&error))?;
            owned_string(text)
        })
    }
}

/// The module's diagnostics as a JSON array, each entry `Diagnostic`'s own
/// serde form (code, severity, message, and, when carried, id, target,
/// spans, related, details, and suggested_action). Free with
/// `pio_string_release`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_diagnostics_json(
    module: *const PioModule,
    error: *mut *mut PioError,
) -> *mut c_char {
    unsafe {
        v6_entry(error, std::ptr::null_mut(), || {
            let inner = required_module(module)?;
            let text = serde_json::to_string(inner.module.diagnostics()).map_err(|error| {
                error_from_parts(codes::EMIT_CAPI_SERIALIZE_FAILED.code, &error.to_string())
            })?;
            owned_string(text)
        })
    }
}

/// The value's permanent kind identifier, valid until the handle's release.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_kind(module: *const PioModule) -> *const c_char {
    unsafe {
        crate::guard(std::ptr::null(), || {
            PioModule::get(module).map_or(std::ptr::null(), |inner| inner.kind.as_ptr())
        })
    }
}

/// Value inspection and supported operation discovery, as JSON. Free with
/// `pio_string_release`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_inspect_json(
    module: *const PioModule,
    error: *mut *mut PioError,
) -> *mut c_char {
    unsafe {
        v6_entry(error, std::ptr::null_mut(), || {
            let inner = required_module(module)?;
            owned_string(inspect_json(&inner.module))
        })
    }
}

/// The typed time or scenario inventory as JSON. Free with `pio_string_release`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_state_inventory_json(
    module: *const PioModule,
    error: *mut *mut PioError,
) -> *mut c_char {
    unsafe {
        v6_entry(error, std::ptr::null_mut(), || {
            let inner = required_module(module)?;
            let inventory = powerio::select::state_inventory(inner.module.value())
                .map_err(|error| error_from_core(&error))?;
            owned_string(inventory_json(&inventory))
        })
    }
}

unsafe fn selector<'a>(
    time_position: i64,
    scenario: *const c_char,
) -> Result<powerio::select::StateSelector<'a>, *mut PioError> {
    let scenario = unsafe { optional_str(scenario, "scenario")? };
    match (time_position, scenario) {
        (position, None) if position >= 0 => Ok(powerio::select::StateSelector::TimePosition(
            usize::try_from(position).expect("checked nonnegative"),
        )),
        (position, Some(id)) if position < 0 => Ok(powerio::select::StateSelector::Scenario(id)),
        _ => Err(error_from_parts(
            codes::REQUEST_CAPI_SELECTOR_CONFLICT.code,
            "pass exactly one key: time_position >= 0 with scenario NULL, or \
             time_position < 0 with scenario set",
        )),
    }
}

/// Export one selected time point or scenario as an independent static
/// module. `time_position >= 0` selects by position (scenario must be NULL);
/// `scenario` non NULL selects by ID (time_position must be negative).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_export_state(
    module: *const PioModule,
    time_position: i64,
    scenario: *const c_char,
    error: *mut *mut PioError,
) -> *mut PioModule {
    unsafe {
        v6_entry(error, std::ptr::null_mut(), || {
            let inner = required_module(module)?;
            let selector = selector(time_position, scenario)?;
            powerio::select::export_state(inner.module.value(), selector)
                .map(module_handle)
                .map_err(|error| error_from_core(&error))
        })
    }
}

/// Readiness of the multiconductor value for the balanced lowering, as JSON.
/// Free with `pio_string_release`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_lowering_readiness_json(
    module: *const PioModule,
    base_mva: f64,
    error: *mut *mut PioError,
) -> *mut c_char {
    unsafe {
        v6_entry(error, std::ptr::null_mut(), || {
            let inner = required_module(module)?;
            let readiness = powerio::transform::check_module_lowering(
                &inner.module,
                powerio::transform::MulticonductorToBalancedOptions {
                    base_mva,
                    ..Default::default()
                },
            )
            .map_err(|error| error_from_core(&error))?;
            // Publish the 1.0 record shape: the readiness struct's own
            // diagnostics field is the frozen 0.9 form its internal checks
            // build, so the report swaps it for the module records.
            let mut value = serde_json::to_value(&readiness).map_err(|error| {
                error_from_parts(codes::EMIT_CAPI_SERIALIZE_FAILED.code, &error.to_string())
            })?;
            let records = readiness.diagnostics_as_module_records();
            if let serde_json::Value::Object(map) = &mut value {
                if records.is_empty() {
                    map.remove("diagnostics");
                } else {
                    map.insert(
                        "diagnostics".to_owned(),
                        serde_json::to_value(&records).map_err(|error| {
                            error_from_parts(
                                codes::EMIT_CAPI_SERIALIZE_FAILED.code,
                                &error.to_string(),
                            )
                        })?,
                    );
                }
            }
            let text = serde_json::to_string(&value).map_err(|error| {
                error_from_parts(codes::EMIT_CAPI_SERIALIZE_FAILED.code, &error.to_string())
            })?;
            owned_string(text)
        })
    }
}

/// Explicitly lower the multiconductor value to a balanced module. Records
/// and source ownership carry over; the pass appends its findings and one
/// Transform history entry.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_lower_to_balanced(
    module: *const PioModule,
    base_mva: f64,
    error: *mut *mut PioError,
) -> *mut PioModule {
    unsafe {
        v6_entry(error, std::ptr::null_mut(), || {
            let inner = required_module(module)?;
            // The transform consumes a module; rebuilding from the stored
            // form preserves every serializable record. The runtime retained
            // source does not cross this copy.
            let text = powerio::stored::write_module(&inner.module)
                .map_err(|error| error_from_core(&error))?;
            let owned =
                powerio::stored::read_module(&text).map_err(|error| error_from_core(&error))?;
            powerio::transform::lower_module_to_balanced(
                owned,
                powerio::transform::MulticonductorToBalancedOptions {
                    base_mva,
                    ..Default::default()
                },
            )
            .map(module_handle)
            .map_err(|(_, boxed)| {
                // The error's records are already 1.0 `Diagnostic` rows, so
                // both channels carry them whole: targets, ids, spans, and
                // details all survive instead of being projected away.
                let records = boxed.diagnostics.clone();
                let message = records
                    .first()
                    .map_or_else(|| boxed.to_string(), powerio_core::render_diagnostic);
                error_from_diagnostics(&message, records)
            })
        })
    }
}

/// Mint an independent handle to the same module. NULL stays NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_retain(module: *const PioModule) -> *mut PioModule {
    unsafe { crate::guard(std::ptr::null_mut(), || PioModule::retain_raw(module)) }
}

/// Release one module handle. NULL is a no-op.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_release(module: *mut PioModule) {
    unsafe { crate::guard((), || PioModule::release_raw(module)) }
}

fn inspect_json(module: &powerio_core::PioModule<powerio::PioValue>) -> String {
    use powerio::PioValue as V;
    let value = module.value();
    let summary = match value {
        V::BalancedNetwork(network) => serde_json::json!({
            "buses": network.buses().len(),
            "branches": network.branches().len(),
            "generators": network.generators().len(),
            "loads": network.loads().len(),
        }),
        V::MulticonductorNetwork(network) => serde_json::json!({
            "buses": network.buses().len(),
            "lines": network.lines().len(),
            "transformers": network.transformers().len(),
            "switches": network.switches().len(),
        }),
        V::BalancedNetworkTimeSeries(series) => serde_json::json!({"points": series.len()}),
        V::BalancedOperatingPointTimeSeries(series) => {
            serde_json::json!({"points": series.len()})
        }
        V::BalancedNetworkScenarioSet(set) => serde_json::json!({"scenarios": set.len()}),
        _ => serde_json::json!({}),
    };
    let operations: Vec<&str> = match value {
        V::BalancedNetwork(_) => vec!["inspect", "diagnostics", "write", "dc_data"],
        V::MulticonductorNetwork(_) => vec![
            "inspect",
            "diagnostics",
            "write",
            "lowering_readiness",
            "lower_to_balanced",
        ],
        V::BalancedNetworkTimeSeries(_)
        | V::BalancedOperatingPointTimeSeries(_)
        | V::BalancedNetworkScenarioSet(_) => vec![
            "inspect",
            "diagnostics",
            "write",
            "state_inventory",
            "export_state",
        ],
        _ => vec!["inspect", "diagnostics", "write"],
    };
    // The value's own source format when the value kind carries one
    // (balanced and multiconductor networks always or optionally do); every
    // other value kind has none of its own, so the first source descriptor's
    // declared format stands in. Omitted entirely when neither is known, so
    // PowerIO.jl's `source_format(m)` (`get(inspect(m), :source_format,
    // nothing)`) and `write_file(m, path)` with no format see `nothing`
    // rather than an empty string.
    let source_format: Option<&str> = match value {
        V::BalancedNetwork(network) => Some(network.source_format().name()),
        V::MulticonductorNetwork(network) => network.source_format().map(|format| format.name()),
        _ => module
            .sources()
            .first()
            .and_then(|source| source.format())
            .map(|format| format.as_str()),
    };
    let mut report = serde_json::json!({
        "kind": value.kind().as_str(),
        "value": summary,
        "records": {
            "sources": module.sources().len(),
            "source_map": module.source_map().len(),
            "diagnostics": module.diagnostics().len(),
            "history": module.history().len(),
            "extensions": module.extensions().len(),
        },
        "operations": operations,
    });
    if let (Some(format), serde_json::Value::Object(map)) = (source_format, &mut report) {
        map.insert(
            "source_format".to_owned(),
            serde_json::Value::String(format.to_owned()),
        );
    }
    report.to_string()
}

fn inventory_json(inventory: &powerio::select::StateInventory) -> String {
    match inventory {
        powerio::select::StateInventory::TimePoints(points) => serde_json::json!({
            "keyed_by": "time_position",
            "time_points": points
                .iter()
                .map(|point| {
                    serde_json::json!({
                        "position": point.position,
                        "label": point.label,
                        "duration_seconds": point.duration.map(|d| d.as_secs_f64()),
                    })
                })
                .collect::<Vec<_>>(),
        }),
        powerio::select::StateInventory::Scenarios(scenarios) => serde_json::json!({
            "keyed_by": "scenario",
            "scenarios": scenarios
                .iter()
                .map(|scenario| {
                    serde_json::json!({
                        "id": scenario.id,
                        "probability": scenario.probability,
                    })
                })
                .collect::<Vec<_>>(),
        }),
        _ => serde_json::json!({}),
    }
    .to_string()
}

// ---- pio_diagnostics --------------------------------------------------------

/// One rendered diagnostic row: every string is a `CString` built once at
/// construction so the borrowed pointers the accessors return stay valid
/// until the handle's last release.
struct DiagnosticRow {
    code: CString,
    severity: CString,
    message: CString,
    id: Option<CString>,
    target: Option<CString>,
    suggested_action: Option<CString>,
    details_json: Option<CString>,
    spans: Vec<(CString, u64, u64)>,
    related: Vec<CString>,
}

impl DiagnosticRow {
    fn of(diagnostic: &powerio_core::Diagnostic) -> Self {
        let details = diagnostic.details();
        Self {
            code: lossy_cstring(diagnostic.code()),
            severity: lossy_cstring(diagnostic.severity().as_str()),
            message: lossy_cstring(diagnostic.message()),
            id: diagnostic.id().map(|id| lossy_cstring(id.as_str())),
            target: diagnostic.target().map(lossy_cstring),
            suggested_action: diagnostic.suggested_action().map(lossy_cstring),
            details_json: if details.is_empty() {
                None
            } else {
                Some(lossy_cstring(
                    &serde_json::to_string(details).unwrap_or_else(|_| "{}".to_owned()),
                ))
            },
            spans: diagnostic
                .spans()
                .iter()
                .map(|span| {
                    (
                        lossy_cstring(span.source().as_str()),
                        span.byte_start(),
                        span.byte_end(),
                    )
                })
                .collect(),
            related: diagnostic
                .related()
                .iter()
                .map(|id| lossy_cstring(id.as_str()))
                .collect(),
        }
    }
}

/// The rendered rows of one diagnostics list.
pub struct DiagnosticsInner {
    rows: Vec<DiagnosticRow>,
}

impl DiagnosticsInner {
    fn of(records: &[powerio_core::Diagnostic]) -> Self {
        Self {
            rows: records.iter().map(DiagnosticRow::of).collect(),
        }
    }

    fn row(&self, index: usize) -> Option<&DiagnosticRow> {
        self.rows.get(index)
    }
}

arc_handle!(
    /// The opaque structured diagnostics list: index addressed rows whose
    /// borrowed strings stay valid until the handle's last release.
    PioDiagnostics,
    DiagnosticsInner
);

/// Mint a diagnostics handle over rendered rows. The crate's conversion
/// entry points use this for their finding channel.
pub(crate) fn diagnostics_handle(records: &[powerio_core::Diagnostic]) -> *mut PioDiagnostics {
    PioDiagnostics::new_raw(DiagnosticsInner::of(records))
}

/// The module's diagnostics as a structured list handle. This is the binding
/// inspection path; [`pio_module_diagnostics_json`] stays as the explicit
/// serialization helper.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_module_diagnostics(
    module: *const PioModule,
    error: *mut *mut PioError,
) -> *mut PioDiagnostics {
    unsafe {
        v6_entry(error, std::ptr::null_mut(), || {
            let inner = required_module(module)?;
            Ok(PioDiagnostics::new_raw(DiagnosticsInner::of(
                inner.module.diagnostics(),
            )))
        })
    }
}

/// The failure's diagnostics as a structured list handle. NULL error yields
/// an empty list.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_error_diagnostics(error: *const PioError) -> *mut PioDiagnostics {
    unsafe {
        crate::guard(std::ptr::null_mut(), || {
            let records = PioError::get(error).map_or(&[][..], |inner| inner.records.as_slice());
            PioDiagnostics::new_raw(DiagnosticsInner::of(records))
        })
    }
}

/// The number of rows in the list. NULL yields 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_diagnostics_len(diagnostics: *const PioDiagnostics) -> usize {
    unsafe {
        crate::guard(0, || {
            PioDiagnostics::get(diagnostics).map_or(0, |inner| inner.rows.len())
        })
    }
}

/// Mint an independent handle to the same list. NULL stays NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_diagnostics_retain(
    diagnostics: *const PioDiagnostics,
) -> *mut PioDiagnostics {
    unsafe {
        crate::guard(std::ptr::null_mut(), || {
            PioDiagnostics::retain_raw(diagnostics)
        })
    }
}

/// Release one list handle. NULL is a no-op.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_diagnostics_release(diagnostics: *mut PioDiagnostics) {
    unsafe { crate::guard((), || PioDiagnostics::release_raw(diagnostics)) }
}

/// The row's stable diagnostic code. NULL handle or an out of range index yields NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_diagnostic_code(
    diagnostics: *const PioDiagnostics,
    index: usize,
) -> *const c_char {
    unsafe {
        crate::guard(std::ptr::null(), || {
            PioDiagnostics::get(diagnostics)
                .and_then(|inner| inner.row(index))
                .map_or(std::ptr::null(), |row| row.code.as_ptr())
        })
    }
}

/// The row's severity name: `error`, `warning`, `remark`, or `note`. NULL handle or an out of range index yields NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_diagnostic_severity(
    diagnostics: *const PioDiagnostics,
    index: usize,
) -> *const c_char {
    unsafe {
        crate::guard(std::ptr::null(), || {
            PioDiagnostics::get(diagnostics)
                .and_then(|inner| inner.row(index))
                .map_or(std::ptr::null(), |row| row.severity.as_ptr())
        })
    }
}

/// The row's rendered message. Explanatory text, not a stable identifier. NULL handle or an out of range index yields NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_diagnostic_message(
    diagnostics: *const PioDiagnostics,
    index: usize,
) -> *const c_char {
    unsafe {
        crate::guard(std::ptr::null(), || {
            PioDiagnostics::get(diagnostics)
                .and_then(|inner| inner.row(index))
                .map_or(std::ptr::null(), |row| row.message.as_ptr())
        })
    }
}

/// The row's identifier when one was assigned, else NULL. NULL handle or an out of range index yields NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_diagnostic_id(
    diagnostics: *const PioDiagnostics,
    index: usize,
) -> *const c_char {
    unsafe {
        crate::guard(std::ptr::null(), || {
            PioDiagnostics::get(diagnostics)
                .and_then(|inner| inner.row(index))
                .and_then(|row| row.id.as_ref())
                .map_or(std::ptr::null(), |value| value.as_ptr())
        })
    }
}

/// The row's target locator when one exists, else NULL. NULL handle or an out of range index yields NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_diagnostic_target(
    diagnostics: *const PioDiagnostics,
    index: usize,
) -> *const c_char {
    unsafe {
        crate::guard(std::ptr::null(), || {
            PioDiagnostics::get(diagnostics)
                .and_then(|inner| inner.row(index))
                .and_then(|row| row.target.as_ref())
                .map_or(std::ptr::null(), |value| value.as_ptr())
        })
    }
}

/// The row's suggested action when one exists, else NULL. NULL handle or an out of range index yields NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_diagnostic_suggested_action(
    diagnostics: *const PioDiagnostics,
    index: usize,
) -> *const c_char {
    unsafe {
        crate::guard(std::ptr::null(), || {
            PioDiagnostics::get(diagnostics)
                .and_then(|inner| inner.row(index))
                .and_then(|row| row.suggested_action.as_ref())
                .map_or(std::ptr::null(), |value| value.as_ptr())
        })
    }
}

/// The row's details as one JSON object, or NULL when it has none. NULL handle or an out of range index yields NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_diagnostic_details_json(
    diagnostics: *const PioDiagnostics,
    index: usize,
) -> *const c_char {
    unsafe {
        crate::guard(std::ptr::null(), || {
            PioDiagnostics::get(diagnostics)
                .and_then(|inner| inner.row(index))
                .and_then(|row| row.details_json.as_ref())
                .map_or(std::ptr::null(), |value| value.as_ptr())
        })
    }
}

/// The number of source spans on one row. NULL or out of range yields 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_diagnostic_n_spans(
    diagnostics: *const PioDiagnostics,
    index: usize,
) -> usize {
    unsafe {
        crate::guard(0, || {
            PioDiagnostics::get(diagnostics)
                .and_then(|inner| inner.row(index))
                .map_or(0, |row| row.spans.len())
        })
    }
}

/// One source span: writes the byte range and returns the span's source
/// identifier. NULL handle or an out of range index yields NULL and leaves
/// the out parameters unwritten.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_diagnostic_span(
    diagnostics: *const PioDiagnostics,
    index: usize,
    span: usize,
    byte_start: *mut u64,
    byte_end: *mut u64,
) -> *const c_char {
    unsafe {
        crate::guard(std::ptr::null(), || {
            let Some(row) = PioDiagnostics::get(diagnostics).and_then(|inner| inner.row(index))
            else {
                return std::ptr::null();
            };
            let Some((source, start, end)) = row.spans.get(span) else {
                return std::ptr::null();
            };
            if !byte_start.is_null() {
                *byte_start = *start;
            }
            if !byte_end.is_null() {
                *byte_end = *end;
            }
            source.as_ptr()
        })
    }
}

/// The number of related diagnostic identifiers on one row.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_diagnostic_n_related(
    diagnostics: *const PioDiagnostics,
    index: usize,
) -> usize {
    unsafe {
        crate::guard(0, || {
            PioDiagnostics::get(diagnostics)
                .and_then(|inner| inner.row(index))
                .map_or(0, |row| row.related.len())
        })
    }
}

/// One related diagnostic identifier. NULL or out of range yields NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_diagnostic_related(
    diagnostics: *const PioDiagnostics,
    index: usize,
    related: usize,
) -> *const c_char {
    unsafe {
        crate::guard(std::ptr::null(), || {
            PioDiagnostics::get(diagnostics)
                .and_then(|inner| inner.row(index))
                .and_then(|row| row.related.get(related))
                .map_or(std::ptr::null(), |id| id.as_ptr())
        })
    }
}

// ---- pio_dc_data ------------------------------------------------------------

/// The DC branch data of one balanced network under one susceptance formula,
/// with the stable element mappings that interpret every row. Arrays are
/// owned by the handle; spans stay valid until its last release.
///
/// Rows and columns describe the analysis network after three winding
/// transformer expansion: each in-service three winding transformer
/// contributes one synthetic star bus (appended after the declared buses)
/// and three winding branches, and every table here indexes that expanded
/// form consistently.
pub struct DcDataInner {
    /// Signed incidence rows: `A[e, from] = +1`, `A[e, to] = -1`.
    from_indices: Vec<i64>,
    to_indices: Vec<i64>,
    /// Branch susceptance per included row, PowerModels sign.
    susceptance: Vec<f64>,
    /// Phase shift angle per included row, radians; `0` for an unshifted
    /// branch or a formula that excludes shifts.
    shift: Vec<f64>,
    /// Phase shift bus injection `p_shift = A' * (b .* shift)`, per bus,
    /// the MATPOWER `makeBdc` sign.
    shift_injection: Vec<f64>,
    /// Stable module element ID per included row. The `_ids` vectors own the
    /// bytes the pointer tables alias, so they are read through the pointers.
    #[allow(dead_code)]
    row_ids: Vec<CString>,
    row_id_pointers: Vec<*const c_char>,
    /// Stable bus element ID per incidence column.
    #[allow(dead_code)]
    bus_ids: Vec<CString>,
    bus_id_pointers: Vec<*const c_char>,
    /// Omitted branches: stable ID plus the diagnostic reason.
    #[allow(dead_code)]
    omitted_ids: Vec<CString>,
    omitted_id_pointers: Vec<*const c_char>,
    #[allow(dead_code)]
    omitted_reasons: Vec<CString>,
    omitted_reason_pointers: Vec<*const c_char>,
    /// The selected branch susceptance formula's stable name.
    formula: CString,
}

// The raw pointer tables alias the CStrings owned by the same struct and are
// only read through `&self`.
unsafe impl Send for DcDataInner {}
unsafe impl Sync for DcDataInner {}

arc_handle!(
    /// The opaque C DC data type.
    PioDcData,
    DcDataInner
);

fn dc_formula(name: &str) -> Result<DcConvention, *mut PioError> {
    DcConvention::from_formula_name(name).ok_or_else(|| {
        error_from_parts(
            codes::REQUEST_CAPI_UNKNOWN_FORMULA.code,
            &format!(
                "unknown branch susceptance formula `{name}`; expected series_susceptance, \
                 tap_adjusted_reactance, or reactance_only"
            ),
        )
    })
}

fn pointer_table(strings: &[CString]) -> Vec<*const c_char> {
    strings.iter().map(|string| string.as_ptr()).collect()
}

/// Project the shared [`powerio::dc_network_data`] assembly into the owned C
/// spans: the same values Rust and Python read, with the strings pinned as
/// NUL terminated copies the pointer tables alias. Every table describes the
/// analysis network after three winding transformer expansion, so `bus_ids`
/// has exactly `n_buses` entries by construction.
fn build_dc_data(
    network: &BalancedNetwork,
    convention: DcConvention,
) -> Result<DcDataInner, *mut PioError> {
    let view = IndexedNetwork::new(network);
    let data = powerio::dc_network_data(&view, convention);
    let row_ids: Vec<CString> = data.row_ids.iter().map(|id| lossy_cstring(id)).collect();
    let bus_ids: Vec<CString> = data.bus_ids.iter().map(|id| lossy_cstring(id)).collect();
    let (omitted_ids, omitted_reasons): (Vec<CString>, Vec<CString>) = data
        .omitted
        .iter()
        .map(|(id, reason)| (lossy_cstring(id), lossy_cstring(reason)))
        .unzip();
    let row_id_pointers = pointer_table(&row_ids);
    let bus_id_pointers = pointer_table(&bus_ids);
    let omitted_id_pointers = pointer_table(&omitted_ids);
    let omitted_reason_pointers = pointer_table(&omitted_reasons);
    Ok(DcDataInner {
        from_indices: data
            .from_indices
            .iter()
            .map(|&index| i64::try_from(index).expect("bus count fits i64"))
            .collect(),
        to_indices: data
            .to_indices
            .iter()
            .map(|&index| i64::try_from(index).expect("bus count fits i64"))
            .collect(),
        susceptance: data.susceptance,
        shift: data.shift,
        shift_injection: data.shift_injection,
        row_ids,
        row_id_pointers,
        bus_ids,
        bus_id_pointers,
        omitted_ids,
        omitted_id_pointers,
        omitted_reasons,
        omitted_reason_pointers,
        formula: lossy_cstring(data.formula),
    })
}

/// Build the DC branch data of a module's balanced network value under the
/// named branch susceptance formula (`series_susceptance`,
/// `tap_adjusted_reactance`, or `reactance_only`). The result is an
/// independently owned handle: releasing the module never invalidates it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_dc_data_build(
    module: *const PioModule,
    formula: *const c_char,
    error: *mut *mut PioError,
) -> *mut PioDcData {
    unsafe {
        v6_entry(error, std::ptr::null_mut(), || {
            let inner = required_module(module)?;
            let formula = dc_formula(required_str(formula, "formula")?)?;
            let powerio::PioValue::BalancedNetwork(network) = inner.module.value() else {
                return Err(error_from_parts(
                    codes::REQUEST_CAPI_NOT_A_BALANCED_NETWORK.code,
                    &format!(
                        "the module carries a {} value; DC data takes a balanced network",
                        inner.module.value().kind().as_str()
                    ),
                ));
            };
            build_dc_data(network, formula).map(PioDcData::new_raw)
        })
    }
}

/// Included incidence row count (`m`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_dc_data_n_rows(data: *const PioDcData) -> usize {
    unsafe {
        crate::guard(0, || {
            PioDcData::get(data).map_or(0, |inner| inner.susceptance.len())
        })
    }
}

/// Incidence column count (`n`, the bus count).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_dc_data_n_buses(data: *const PioDcData) -> usize {
    unsafe {
        crate::guard(0, || {
            PioDcData::get(data).map_or(0, |inner| inner.shift_injection.len())
        })
    }
}

/// From bus column per included row (`A[e, from] = +1`), length `n_rows`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_dc_data_from_indices(data: *const PioDcData) -> *const i64 {
    unsafe {
        crate::guard(std::ptr::null(), || {
            PioDcData::get(data).map_or(std::ptr::null(), |inner| inner.from_indices.as_ptr())
        })
    }
}

/// To bus column per included row (`A[e, to] = -1`), length `n_rows`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_dc_data_to_indices(data: *const PioDcData) -> *const i64 {
    unsafe {
        crate::guard(std::ptr::null(), || {
            PioDcData::get(data).map_or(std::ptr::null(), |inner| inner.to_indices.as_ptr())
        })
    }
}

/// Branch susceptance per included row, PowerModels sign, length `n_rows`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_dc_data_susceptance(data: *const PioDcData) -> *const f64 {
    unsafe {
        crate::guard(std::ptr::null(), || {
            PioDcData::get(data).map_or(std::ptr::null(), |inner| inner.susceptance.as_ptr())
        })
    }
}

/// Phase shift angle per included row, radians, length `n_rows`. `0` for an
/// unshifted branch or a formula that excludes shifts.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_dc_data_shift(data: *const PioDcData) -> *const f64 {
    unsafe {
        crate::guard(std::ptr::null(), || {
            PioDcData::get(data).map_or(std::ptr::null(), |inner| inner.shift.as_ptr())
        })
    }
}

/// Phase shift bus injection `p_shift = A' * (b .* shift)` (the MATPOWER
/// `makeBdc` sign), length `n_buses`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_dc_data_shift_injection(data: *const PioDcData) -> *const f64 {
    unsafe {
        crate::guard(std::ptr::null(), || {
            PioDcData::get(data).map_or(std::ptr::null(), |inner| inner.shift_injection.as_ptr())
        })
    }
}

/// Stable module element ID per included row, length `n_rows`. Both the
/// table and the strings stay valid until the handle's release.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_dc_data_row_ids(data: *const PioDcData) -> *const *const c_char {
    unsafe {
        crate::guard(std::ptr::null(), || {
            PioDcData::get(data).map_or(std::ptr::null(), |inner| inner.row_id_pointers.as_ptr())
        })
    }
}

/// Stable bus element ID per incidence column, length `n_buses`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_dc_data_bus_ids(data: *const PioDcData) -> *const *const c_char {
    unsafe {
        crate::guard(std::ptr::null(), || {
            PioDcData::get(data).map_or(std::ptr::null(), |inner| inner.bus_id_pointers.as_ptr())
        })
    }
}

/// Count of branches the selected formula cannot represent.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_dc_data_n_omitted(data: *const PioDcData) -> usize {
    unsafe {
        crate::guard(0, || {
            PioDcData::get(data).map_or(0, |inner| inner.omitted_ids.len())
        })
    }
}

/// Stable element IDs of the omitted branches, length `n_omitted`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_dc_data_omitted_ids(data: *const PioDcData) -> *const *const c_char {
    unsafe {
        crate::guard(std::ptr::null(), || {
            PioDcData::get(data)
                .map_or(std::ptr::null(), |inner| inner.omitted_id_pointers.as_ptr())
        })
    }
}

/// Diagnostic reason per omitted branch, length `n_omitted`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_dc_data_omitted_reasons(
    data: *const PioDcData,
) -> *const *const c_char {
    unsafe {
        crate::guard(std::ptr::null(), || {
            PioDcData::get(data).map_or(std::ptr::null(), |inner| {
                inner.omitted_reason_pointers.as_ptr()
            })
        })
    }
}

/// The selected branch susceptance formula's stable name.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_dc_data_formula(data: *const PioDcData) -> *const c_char {
    unsafe {
        crate::guard(std::ptr::null(), || {
            PioDcData::get(data).map_or(std::ptr::null(), |inner| inner.formula.as_ptr())
        })
    }
}

/// Fill `out` with the complete affine branch flow
/// `p_branch = -b .* (va_from - va_to) + b .* shift`: given bus voltage
/// angles `va` (radians, length `n_buses`), writes
/// `-b[e] * (va[from] - va[to]) + b[e] * shift[e]` per included row into
/// `out` (length `n_rows`), so `A' * p_branch` equals the bus injection
/// including `shift_injection`. Returns false on a NULL argument or a length
/// mismatch. No temporary vector is allocated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_dc_data_fill_branch_flow(
    data: *const PioDcData,
    va: *const f64,
    va_len: usize,
    out: *mut f64,
    out_len: usize,
) -> bool {
    unsafe {
        crate::guard(false, || {
            let Some(inner) = PioDcData::get(data) else {
                return false;
            };
            if va.is_null() || out.is_null() {
                return false;
            }
            if va_len != inner.shift_injection.len() || out_len != inner.susceptance.len() {
                return false;
            }
            let va = std::slice::from_raw_parts(va, va_len);
            let out = std::slice::from_raw_parts_mut(out, out_len);
            for (row, slot) in out.iter_mut().enumerate() {
                let from = usize::try_from(inner.from_indices[row]).expect("stored nonnegative");
                let to = usize::try_from(inner.to_indices[row]).expect("stored nonnegative");
                *slot = -inner.susceptance[row] * (va[from] - va[to])
                    + inner.susceptance[row] * inner.shift[row];
            }
            true
        })
    }
}

/// Mint an independent handle to the same DC data. NULL stays NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_dc_data_retain(data: *const PioDcData) -> *mut PioDcData {
    unsafe { crate::guard(std::ptr::null_mut(), || PioDcData::retain_raw(data)) }
}

/// Release one DC data handle. NULL is a no-op.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_dc_data_release(data: *mut PioDcData) {
    unsafe { crate::guard((), || PioDcData::release_raw(data)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pure handle core under Miri: new, retain, cross release orders,
    /// NULL no-ops.
    #[test]
    fn handle_lifecycle_is_sound() {
        let first = handle_new(String::from("shared"));
        let second = unsafe { handle_retain(first) };
        assert_ne!(first, second);
        assert_eq!(unsafe { handle_ref(second) }.unwrap(), "shared");
        // Parent releases first; the child stays valid.
        unsafe { handle_release(first) };
        assert_eq!(unsafe { handle_ref(second) }.unwrap(), "shared");
        unsafe { handle_release(second) };
        // NULL is a no-op everywhere.
        unsafe { handle_release(std::ptr::null_mut::<HandleBox<String>>()) };
        assert!(unsafe { handle_retain(std::ptr::null::<HandleBox<String>>()) }.is_null());
        assert!(unsafe { handle_ref(std::ptr::null::<HandleBox<String>>()) }.is_none());
    }

    /// Test reads through accessor pointers, with the null case failed
    /// explicitly rather than dereferenced.
    unsafe fn checked<'a, T>(pointer: *const T) -> &'a T {
        unsafe { pointer.as_ref() }.expect("accessor returned NULL")
    }

    unsafe fn checked_slice<'a, T>(pointer: *const T, len: usize) -> &'a [T] {
        assert!(!pointer.is_null(), "accessor returned NULL");
        unsafe { std::slice::from_raw_parts(pointer, len) }
    }

    fn case_text() -> CString {
        CString::new(
            "function mpc = case\n\
             mpc.version = '2';\n\
             mpc.baseMVA = 100;\n\
             mpc.bus = [1 3 0 0 0 0 1 1.0 0 230 1 1.1 0.9; 2 1 30 10 0 0 1 1.0 0 230 1 1.1 0.9; 3 1 0 0 0 0 1 1.0 0 230 1 1.1 0.9;];\n\
             mpc.gen = [1 40 0 30 -30 1.0 100 1 100 0;];\n\
             mpc.branch = [1 2 0.01 0.1 0 250 250 250 0 0 1 -30 30; 2 3 0.02 0.2 0 250 250 250 0 0 0 -30 30;];\n",
        )
        .unwrap()
    }

    #[test]
    fn module_parse_bytes_reads_memory_and_refuses_null() {
        unsafe {
            let mut error = std::ptr::null_mut();
            let matpower = CString::new("matpower").unwrap();
            let text = case_text();
            let bytes = text.as_bytes();
            let module = pio_parse_bytes(
                std::ptr::null(),
                bytes.as_ptr(),
                bytes.len(),
                matpower.as_ptr(),
                &raw mut error,
            );
            assert!(error.is_null());
            assert_eq!(
                CStr::from_ptr(pio_module_kind(module)).to_str().unwrap(),
                "balanced_network"
            );
            pio_module_release(module);

            let module = pio_parse_bytes(
                std::ptr::null(),
                std::ptr::null(),
                0,
                matpower.as_ptr(),
                &raw mut error,
            );
            assert!(module.is_null());
            assert!(!error.is_null());
            assert_eq!(
                CStr::from_ptr(pio_error_code(error)).to_str().unwrap(),
                "BIND.CAPI.NULL_ARGUMENT"
            );
            pio_error_release(error);
        }
    }

    #[test]
    fn module_as_network_threads_provenance_and_refuses_other_kinds() {
        unsafe {
            let mut error = std::ptr::null_mut();
            let matpower = CString::new("matpower").unwrap();
            let module = pio_parse_str(
                std::ptr::null(),
                case_text().as_ptr(),
                matpower.as_ptr(),
                &raw mut error,
            );
            assert!(error.is_null());
            let net = pio_module_balanced_network(module, &raw mut error);
            assert!(error.is_null());
            assert!(!net.is_null());
            // The retained source threads through: wrapping the network back
            // into a module and writing the same format echoes the exact
            // source bytes.
            let net_module = pio_module_of_balanced_network(net, &raw mut error);
            assert!(error.is_null());
            let to = CString::new("matpower").unwrap();
            let mut diag: *mut PioDiagnostics = std::ptr::null_mut();
            let text = pio_module_write_str(net_module, to.as_ptr(), &raw mut diag, &raw mut error);
            assert!(!text.is_null());
            let echoed = CStr::from_ptr(text).to_str().unwrap().to_owned();
            assert_eq!(echoed, case_text().to_str().unwrap());
            crate::pio_string_release(text);
            if !diag.is_null() {
                pio_diagnostics_release(diag);
            }
            pio_module_release(net_module);
            crate::pio_balanced_network_release(net);

            #[cfg(feature = "dist")]
            {
                let wrong = pio_module_multiconductor_network(module, &raw mut error);
                assert!(wrong.is_null());
                assert!(!error.is_null());
                assert_eq!(
                    CStr::from_ptr(pio_error_code(error)).to_str().unwrap(),
                    "REQUEST.PACKAGE.WRONG_MODEL_KIND"
                );
                pio_error_release(error);
            }
            pio_module_release(module);
        }
    }

    #[test]
    fn module_round_trips_and_survives_parent_release() {
        unsafe {
            let mut error = std::ptr::null_mut();
            let matpower = CString::new("matpower").unwrap();
            let module = pio_parse_str(
                std::ptr::null(),
                case_text().as_ptr(),
                matpower.as_ptr(),
                &raw mut error,
            );
            assert!(
                error.is_null(),
                "{:?}",
                CStr::from_ptr(pio_error_message(error))
            );
            assert_eq!(
                CStr::from_ptr(pio_module_kind(module)).to_str().unwrap(),
                "balanced_network"
            );

            let text = pio_module_write_json(module, &raw mut error);
            assert!(!text.is_null());
            let reread = pio_module_read_json(text, &raw mut error);
            assert!(error.is_null());
            crate::pio_string_release(text);

            // A child handle outlives its parent.
            let retained = pio_module_retain(reread);
            pio_module_release(reread);
            assert_eq!(
                CStr::from_ptr(pio_module_kind(retained)).to_str().unwrap(),
                "balanced_network"
            );
            pio_module_release(retained);
            pio_module_release(module);
            pio_module_release(std::ptr::null_mut());
        }
    }

    #[test]
    fn dc_data_matches_powermodels_and_maps_omissions() {
        unsafe {
            let mut error = std::ptr::null_mut();
            let matpower = CString::new("matpower").unwrap();
            let module = pio_parse_str(
                std::ptr::null(),
                case_text().as_ptr(),
                matpower.as_ptr(),
                &raw mut error,
            );
            assert!(error.is_null());
            let formula = CString::new("series_susceptance").unwrap();
            let data = pio_dc_data_build(module, formula.as_ptr(), &raw mut error);
            assert!(error.is_null());
            // The DC data is independently owned.
            pio_module_release(module);

            assert_eq!(pio_dc_data_n_rows(data), 1);
            assert_eq!(pio_dc_data_n_buses(data), 3);
            let b = *checked(pio_dc_data_susceptance(data));
            // series, PowerModels sign: imag(1/(r+ix)) = -x/(r^2+x^2)
            let expected = -0.1 / (0.01_f64 * 0.01 + 0.1 * 0.1);
            assert!((b - expected).abs() < 1e-12, "{b}");
            assert_eq!(*checked(pio_dc_data_from_indices(data)), 0);
            assert_eq!(*checked(pio_dc_data_to_indices(data)), 1);
            let row_id = CStr::from_ptr(*checked(pio_dc_data_row_ids(data)))
                .to_str()
                .unwrap();
            assert_eq!(row_id, "branches:0");
            let bus_ids = pio_dc_data_bus_ids(data);
            assert_eq!(CStr::from_ptr(*checked(bus_ids)).to_str().unwrap(), "1");

            // The out of service branch is an omitted mapping, by stable ID.
            assert_eq!(pio_dc_data_n_omitted(data), 1);
            let omitted = CStr::from_ptr(*checked(pio_dc_data_omitted_ids(data)))
                .to_str()
                .unwrap();
            assert_eq!(omitted, "branches:1");
            let reason = CStr::from_ptr(*checked(pio_dc_data_omitted_reasons(data)))
                .to_str()
                .unwrap();
            assert!(reason.contains("out of service"), "{reason}");
            assert_eq!(
                CStr::from_ptr(pio_dc_data_formula(data)).to_str().unwrap(),
                "series_susceptance"
            );

            // p_branch = -b (va_from - va_to), filled without a temporary.
            let va = [0.05_f64, 0.0, 0.0];
            let mut flow = [0.0_f64];
            assert!(pio_dc_data_fill_branch_flow(
                data,
                va.as_ptr(),
                3,
                flow.as_mut_ptr(),
                1
            ));
            assert!((flow[0] - (-expected * 0.05)).abs() < 1e-12, "{}", flow[0]);
            // Length mismatches are refused.
            assert!(!pio_dc_data_fill_branch_flow(
                data,
                va.as_ptr(),
                2,
                flow.as_mut_ptr(),
                1
            ));

            let kept = pio_dc_data_retain(data);
            pio_dc_data_release(data);
            assert_eq!(pio_dc_data_n_rows(kept), 1);
            pio_dc_data_release(kept);
            pio_dc_data_release(std::ptr::null_mut());
        }
    }

    fn shifted_case_text() -> CString {
        CString::new(
            "function mpc = case\n\
             mpc.version = '2';\n\
             mpc.baseMVA = 100;\n\
             mpc.bus = [1 3 0 0 0 0 1 1.0 0 230 1 1.1 0.9; 2 1 30 10 0 0 1 1.0 0 230 1 1.1 0.9; 3 1 20 5 0 0 1 1.0 0 230 1 1.1 0.9;];\n\
             mpc.gen = [1 40 0 30 -30 1.0 100 1 100 0;];\n\
             mpc.branch = [1 2 0.01 0.1 0 250 250 250 0 0 1 -30 30; 1 3 0.02 0.2 0 250 250 250 0 10 1 -30 30;];\n",
        )
        .unwrap()
    }

    /// The complete affine flow: `p_branch = -b (va_from - va_to) + b shift`,
    /// and the KCL identity `A' * p_branch == -B va + shift_injection` holds
    /// for a case with a nonzero phase shift branch.
    #[test]
    fn branch_flow_carries_the_phase_shift_term() {
        unsafe {
            let mut error = std::ptr::null_mut();
            let matpower = CString::new("matpower").unwrap();
            let module = pio_parse_str(
                std::ptr::null(),
                shifted_case_text().as_ptr(),
                matpower.as_ptr(),
                &raw mut error,
            );
            assert!(error.is_null());
            let formula = CString::new("series_susceptance").unwrap();
            let data = pio_dc_data_build(module, formula.as_ptr(), &raw mut error);
            assert!(error.is_null());
            pio_module_release(module);

            let m = pio_dc_data_n_rows(data);
            let n = pio_dc_data_n_buses(data);
            assert_eq!((m, n), (2, 3));
            let b = checked_slice(pio_dc_data_susceptance(data), m);
            let shift = checked_slice(pio_dc_data_shift(data), m);
            let from = checked_slice(pio_dc_data_from_indices(data), m);
            let to = checked_slice(pio_dc_data_to_indices(data), m);
            let injection = checked_slice(pio_dc_data_shift_injection(data), n);
            assert!((shift[0]).abs() < 1e-15);
            assert!(
                (shift[1] - 10.0_f64.to_radians()).abs() < 1e-12,
                "{}",
                shift[1]
            );
            // p_shift = A' (b .* shift): b*shift at the from bus, -b*shift
            // at the to bus.
            assert!((injection[0] - (b[1] * shift[1])).abs() < 1e-12);
            assert!((injection[2] - (-b[1] * shift[1])).abs() < 1e-12);

            // Flat start pin: the shifted row alone carries b * shift, the
            // MATPOWER (1/x)(0 - shift) fixed term, negative for a positive
            // shift on an inductive branch.
            let flat = [0.0_f64; 3];
            let mut flow_flat = [0.0_f64; 2];
            assert!(pio_dc_data_fill_branch_flow(
                data,
                flat.as_ptr(),
                3,
                flow_flat.as_mut_ptr(),
                2
            ));
            assert!(flow_flat[0].abs() < 1e-15);
            assert!((flow_flat[1] - b[1] * shift[1]).abs() < 1e-12);
            assert!(flow_flat[1] < 0.0, "{}", flow_flat[1]);

            let va = [0.03_f64, 0.01, -0.02];
            let mut flow = [0.0_f64; 2];
            assert!(pio_dc_data_fill_branch_flow(
                data,
                va.as_ptr(),
                3,
                flow.as_mut_ptr(),
                2
            ));
            for row in 0..m {
                let f = usize::try_from(from[row]).unwrap();
                let t = usize::try_from(to[row]).unwrap();
                let expected = -b[row] * (va[f] - va[t]) + b[row] * shift[row];
                assert!((flow[row] - expected).abs() < 1e-12, "row {row}");
            }
            // KCL: A' * p_branch equals the angle terms plus shift_injection.
            let mut bus_from_flows = [0.0_f64; 3];
            for row in 0..m {
                let f = usize::try_from(from[row]).unwrap();
                let t = usize::try_from(to[row]).unwrap();
                bus_from_flows[f] += flow[row];
                bus_from_flows[t] -= flow[row];
            }
            for bus in 0..n {
                let mut angle_term = 0.0;
                for row in 0..m {
                    let f = usize::try_from(from[row]).unwrap();
                    let t = usize::try_from(to[row]).unwrap();
                    let sign = if f == bus {
                        1.0
                    } else if t == bus {
                        -1.0
                    } else {
                        0.0
                    };
                    angle_term += sign * (-b[row] * (va[f] - va[t]));
                }
                assert!(
                    (bus_from_flows[bus] - (angle_term + injection[bus])).abs() < 1e-12,
                    "bus {bus}"
                );
            }
            pio_dc_data_release(data);
        }
    }

    /// Every table describes the analysis network after three winding
    /// expansion: the bus ID table has exactly `n_buses` entries (declared
    /// buses plus the synthetic star bus) and the winding branches appear as
    /// included or omitted rows.
    #[test]
    fn three_winding_expansion_keeps_the_tables_aligned() {
        unsafe {
            let mut error = std::ptr::null_mut();
            let path = CString::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../tests/data/psse/case3_3w_v33.raw"
            ))
            .unwrap();
            let psse = CString::new("psse").unwrap();
            let module = pio_parse_file(path.as_ptr(), psse.as_ptr(), &raw mut error);
            assert!(
                error.is_null(),
                "{:?}",
                CStr::from_ptr(pio_error_message(error))
            );
            let formula = CString::new("series_susceptance").unwrap();
            let data = pio_dc_data_build(module, formula.as_ptr(), &raw mut error);
            assert!(error.is_null());
            pio_module_release(module);

            let n = pio_dc_data_n_buses(data);
            let m = pio_dc_data_n_rows(data);
            let omitted = pio_dc_data_n_omitted(data);
            // Three declared buses plus the synthetic star bus.
            assert_eq!(n, 4);
            // The three winding branches all appear, included or omitted.
            assert!(m + omitted >= 3, "m {m} omitted {omitted}");
            let bus_ids = checked_slice(pio_dc_data_bus_ids(data), n);
            for (column, id) in bus_ids.iter().enumerate() {
                assert!(!id.is_null(), "bus column {column}");
                assert!(!CStr::from_ptr(*id).to_bytes().is_empty());
            }
            let row_ids = checked_slice(pio_dc_data_row_ids(data), m);
            for (row, id) in row_ids.iter().enumerate() {
                assert!(!id.is_null(), "row {row}");
            }
            pio_dc_data_release(data);
        }
    }

    /// Every refusal carries its own registered code: NULL handles, an
    /// unrecognized formula, conflicting selection keys, a value kind DC data
    /// does not accept, and a static value's selection refusal all differ.
    #[test]
    fn refusals_carry_distinct_registered_codes() {
        unsafe {
            let code_of = |error: *mut PioError| {
                let code = CStr::from_ptr(pio_error_code(error))
                    .to_str()
                    .unwrap()
                    .to_owned();
                pio_error_release(error);
                code
            };
            let mut error = std::ptr::null_mut();

            // NULL module handle.
            let text = pio_module_write_json(std::ptr::null(), &raw mut error);
            assert!(text.is_null());
            assert_eq!(code_of(error), "BIND.CAPI.NULL_HANDLE");

            let matpower = CString::new("matpower").unwrap();
            let mut error = std::ptr::null_mut();
            let module = pio_parse_str(
                std::ptr::null(),
                case_text().as_ptr(),
                matpower.as_ptr(),
                &raw mut error,
            );
            assert!(error.is_null());

            // An unrecognized formula string.
            let mut error = std::ptr::null_mut();
            let bogus = CString::new("nodal_admittance").unwrap();
            let data = pio_dc_data_build(module, bogus.as_ptr(), &raw mut error);
            assert!(data.is_null());
            assert_eq!(code_of(error), "REQUEST.CAPI.UNKNOWN_FORMULA");

            // Both rejected selection key combinations.
            let mut error = std::ptr::null_mut();
            let scenario = CString::new("s1").unwrap();
            let exported = pio_module_export_state(module, 0, scenario.as_ptr(), &raw mut error);
            assert!(exported.is_null());
            assert_eq!(code_of(error), "REQUEST.CAPI.SELECTOR_CONFLICT");
            let mut error = std::ptr::null_mut();
            let exported = pio_module_export_state(module, -1, std::ptr::null(), &raw mut error);
            assert!(exported.is_null());
            assert_eq!(code_of(error), "REQUEST.CAPI.SELECTOR_CONFLICT");

            // A static value's selection refusal comes from the library and
            // differs from the DC data kind refusal below.
            let mut error = std::ptr::null_mut();
            let exported = pio_module_export_state(module, 0, std::ptr::null(), &raw mut error);
            assert!(exported.is_null());
            let selection_code = code_of(error);
            assert_eq!(selection_code, "REQUEST.STATE.NOT_A_COLLECTION");

            // DC data against a value kind it does not accept.
            let dss = CString::new("dss").unwrap();
            let circuit = CString::new(
                "Clear\nNew Circuit.tiny basekv=12.47 bus1=src\n\
                 New Line.l1 bus1=src bus2=a length=1\nSet VoltageBases=[12.47]\n",
            )
            .unwrap();
            let mut error = std::ptr::null_mut();
            let mc = pio_parse_str(
                std::ptr::null(),
                circuit.as_ptr(),
                dss.as_ptr(),
                &raw mut error,
            );
            assert!(error.is_null(), "dss parse failed");
            let formula = CString::new("series_susceptance").unwrap();
            let mut error = std::ptr::null_mut();
            let data = pio_dc_data_build(mc, formula.as_ptr(), &raw mut error);
            assert!(data.is_null());
            let dc_kind_code = code_of(error);
            assert_eq!(dc_kind_code, "REQUEST.CAPI.NOT_A_BALANCED_NETWORK");
            assert_ne!(dc_kind_code, selection_code);

            pio_module_release(mc);
            pio_module_release(module);
        }
    }

    /// The panic guard on the direct accessors: NULL handles fall back, and a
    /// value that panics on drop leaves release returning normally.
    #[test]
    fn direct_accessors_fall_back_and_release_survives_a_drop_panic() {
        unsafe {
            assert!(pio_module_kind(std::ptr::null()).is_null());
            assert!(pio_module_diagnostics_json(std::ptr::null(), std::ptr::null_mut()).is_null());
            assert!(pio_error_code(std::ptr::null()).is_null());
            assert!(pio_error_message(std::ptr::null()).is_null());
            assert!(pio_error_diagnostics_json(std::ptr::null()).is_null());
            assert!(pio_error_retain(std::ptr::null()).is_null());
            assert_eq!(pio_dc_data_n_rows(std::ptr::null()), 0);
            assert_eq!(pio_dc_data_n_buses(std::ptr::null()), 0);
            assert!(pio_dc_data_from_indices(std::ptr::null()).is_null());
            assert!(pio_dc_data_to_indices(std::ptr::null()).is_null());
            assert!(pio_dc_data_susceptance(std::ptr::null()).is_null());
            assert!(pio_dc_data_shift(std::ptr::null()).is_null());
            assert!(pio_dc_data_shift_injection(std::ptr::null()).is_null());
            assert!(pio_dc_data_row_ids(std::ptr::null()).is_null());
            assert!(pio_dc_data_bus_ids(std::ptr::null()).is_null());
            assert_eq!(pio_dc_data_n_omitted(std::ptr::null()), 0);
            assert!(pio_dc_data_omitted_ids(std::ptr::null()).is_null());
            assert!(pio_dc_data_omitted_reasons(std::ptr::null()).is_null());
            assert!(pio_dc_data_formula(std::ptr::null()).is_null());
            assert!(pio_dc_data_retain(std::ptr::null()).is_null());
            assert!(!pio_dc_data_fill_branch_flow(
                std::ptr::null(),
                std::ptr::null(),
                0,
                std::ptr::null_mut(),
                0
            ));
            // NULL release no-ops.
            pio_error_release(std::ptr::null_mut());
            pio_module_release(std::ptr::null_mut());
            pio_dc_data_release(std::ptr::null_mut());

            // A drop panic stays behind the same guard release uses.
            struct PanicOnDrop;
            impl Drop for PanicOnDrop {
                fn drop(&mut self) {
                    panic!("drop panicked");
                }
            }
            let handle = handle_new(PanicOnDrop);
            crate::guard((), || handle_release(handle));
        }
    }

    /// `pio_diagnostics_*` and `pio_diagnostic_*`: NULL handles and
    /// out-of-range indices fall back safely everywhere, and a row's
    /// required fields (code, severity, message) are always readable while
    /// its optional fields (id, target, suggested_action, details_json) are
    /// NULL when the finding carries none.
    #[test]
    fn diagnostics_accessors_fall_back_on_null_and_out_of_range() {
        unsafe {
            // A NULL diagnostics handle: every accessor reports the safe
            // empty default rather than dereferencing.
            let nil: *const PioDiagnostics = std::ptr::null();
            assert_eq!(pio_diagnostics_len(nil), 0);
            assert!(pio_diagnostics_retain(nil).is_null());
            assert!(pio_diagnostic_code(nil, 0).is_null());
            assert!(pio_diagnostic_severity(nil, 0).is_null());
            assert!(pio_diagnostic_message(nil, 0).is_null());
            assert!(pio_diagnostic_id(nil, 0).is_null());
            assert!(pio_diagnostic_target(nil, 0).is_null());
            assert!(pio_diagnostic_suggested_action(nil, 0).is_null());
            assert!(pio_diagnostic_details_json(nil, 0).is_null());
            assert_eq!(pio_diagnostic_n_spans(nil, 0), 0);
            assert!(
                pio_diagnostic_span(nil, 0, 0, std::ptr::null_mut(), std::ptr::null_mut())
                    .is_null()
            );
            assert_eq!(pio_diagnostic_n_related(nil, 0), 0);
            assert!(pio_diagnostic_related(nil, 0, 0).is_null());
            pio_diagnostics_release(std::ptr::null_mut());

            // A real list with at least one row: index 0 is live, an
            // out-of-range index falls back the same way NULL does. The
            // pandapower fixture carries switches the model ignores, so its
            // module has findings to index (see
            // module_diagnostics_expose_every_finding_untruncated in lib.rs,
            // which reads the same fixture's findings by message content).
            let mut error = std::ptr::null_mut();
            let path = CString::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../tests/data/pandapower/example.json"
            ))
            .unwrap();
            let module = pio_parse_file(path.as_ptr(), std::ptr::null(), &raw mut error);
            assert!(
                error.is_null(),
                "{:?}",
                CStr::from_ptr(pio_error_message(error))
            );
            let list = pio_module_diagnostics(module, &raw mut error);
            assert!(error.is_null());
            let n = pio_diagnostics_len(list);
            assert!(
                n >= 1,
                "the pandapower fixture's ignored switches are findings"
            );

            // Required fields never return NULL for a live row.
            assert!(!pio_diagnostic_code(list, 0).is_null());
            assert!(!pio_diagnostic_severity(list, 0).is_null());
            assert!(!pio_diagnostic_message(list, 0).is_null());
            let code = CStr::from_ptr(pio_diagnostic_code(list, 0))
                .to_str()
                .unwrap();
            assert!(code.contains('.'), "{code}");

            // Out of range on a live handle matches the NULL-handle fallback.
            assert!(pio_diagnostic_code(list, n).is_null());
            assert!(pio_diagnostic_severity(list, n).is_null());
            assert!(pio_diagnostic_message(list, n).is_null());
            assert!(pio_diagnostic_id(list, n).is_null());
            assert!(pio_diagnostic_target(list, n).is_null());
            assert!(pio_diagnostic_suggested_action(list, n).is_null());
            assert!(pio_diagnostic_details_json(list, n).is_null());
            assert_eq!(pio_diagnostic_n_spans(list, n), 0);
            assert!(
                pio_diagnostic_span(list, n, 0, std::ptr::null_mut(), std::ptr::null_mut())
                    .is_null()
            );
            assert_eq!(pio_diagnostic_n_related(list, n), 0);
            assert!(pio_diagnostic_related(list, n, 0).is_null());
            // A span index out of range on a live row is NULL too, whether or
            // not the row itself carries any spans.
            let spans = pio_diagnostic_n_spans(list, 0);
            assert!(
                pio_diagnostic_span(list, 0, spans, std::ptr::null_mut(), std::ptr::null_mut())
                    .is_null()
            );

            // Retaining a live list mints an independent handle the parent's
            // release does not invalidate.
            let retained = pio_diagnostics_retain(list);
            pio_diagnostics_release(list);
            assert_eq!(pio_diagnostics_len(retained), n);
            pio_diagnostics_release(retained);
            pio_module_release(module);
        }
    }

    /// Every operation `pio_module_inspect_json` advertises maps to an
    /// exported symbol in the committed header, for each value family the
    /// operations table branches on.
    #[test]
    fn every_inspect_operation_maps_to_an_exported_symbol() {
        let header = include_str!("../include/powerio.h");
        let symbol_of = |op: &str| match op {
            "inspect" => "pio_module_inspect_json",
            "diagnostics" => "pio_module_diagnostics_json",
            "write" => "pio_module_write_json",
            "dc_data" => "pio_dc_data_build",
            "state_inventory" => "pio_module_state_inventory_json",
            "export_state" => "pio_module_export_state",
            "lowering_readiness" => "pio_module_lowering_readiness_json",
            "lower_to_balanced" => "pio_module_lower_to_balanced",
            other => panic!("operation `{other}` has no symbol mapping"),
        };
        let balanced = powerio::parse(
            powerio_core::Source::from_bytes("case.m", case_text().into_bytes())
                .unwrap()
                .with_format(powerio_core::FormatId::new("matpower").unwrap()),
        )
        .unwrap();
        let network = match balanced.value() {
            powerio::PioValue::BalancedNetwork(network) => network.clone(),
            _ => panic!("wrong kind"),
        };
        let series = powerio_core::TimeSeries::new(
            vec![powerio_core::TimePoint::new("h0", None).unwrap()],
            vec![network],
        )
        .unwrap();
        let series_module =
            powerio_core::PioModule::new(powerio::PioValue::BalancedNetworkTimeSeries(series));
        let dss_module = powerio::parse(
            powerio_core::Source::from_bytes(
                "tiny.dss",
                b"Clear\nNew Circuit.tiny basekv=12.47 bus1=src\nNew Line.l1 bus1=src bus2=a length=1\nSet VoltageBases=[12.47]\n".to_vec(),
            )
            .unwrap()
            .with_format(powerio_core::FormatId::new("dss").unwrap()),
        )
        .unwrap();
        for module in [&balanced, &series_module, &dss_module] {
            let rendered = inspect_json(module);
            let parsed: serde_json::Value = serde_json::from_str(&rendered).unwrap();
            let operations = parsed["operations"].as_array().unwrap();
            assert!(!operations.is_empty());
            for operation in operations {
                let symbol = symbol_of(operation.as_str().unwrap());
                assert!(
                    header.contains(&format!("{symbol}(")),
                    "{symbol} is not in the committed header"
                );
            }
        }
    }

    /// `inspect_json`'s `source_format` key: present with the parsed
    /// source's token for a value kind that carries its own (balanced and
    /// multiconductor networks), and absent for a kind that carries none of
    /// its own and no source descriptor either. This is the field
    /// `PowerIO.jl`'s `source_format(m)` (`get(inspect(m), :source_format,
    /// nothing)`) reads, and that `write_file(m, path)` with no `format`
    /// falls back to.
    #[test]
    fn inspect_json_carries_source_format_when_the_value_or_a_source_declares_one() {
        let balanced = powerio::parse(
            powerio_core::Source::from_bytes("case.m", case_text().into_bytes())
                .unwrap()
                .with_format(powerio_core::FormatId::new("matpower").unwrap()),
        )
        .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&inspect_json(&balanced)).unwrap()["source_format"],
            "matpower"
        );
        let dss_module = powerio::parse(
            powerio_core::Source::from_bytes(
                "tiny.dss",
                b"Clear\nNew Circuit.tiny basekv=12.47 bus1=src\nNew Line.l1 bus1=src bus2=a length=1\nSet VoltageBases=[12.47]\n".to_vec(),
            )
            .unwrap()
            .with_format(powerio_core::FormatId::new("dss").unwrap()),
        )
        .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&inspect_json(&dss_module)).unwrap()["source_format"],
            "dss"
        );
        // A value kind with no source_format accessor of its own, built with
        // no source descriptor either (assembled in memory from an
        // already-parsed network, not through a `Source`): the key is
        // omitted rather than emitted as `null`.
        let network = match balanced.value() {
            powerio::PioValue::BalancedNetwork(network) => network.clone(),
            _ => panic!("wrong kind"),
        };
        let series = powerio_core::TimeSeries::new(
            vec![powerio_core::TimePoint::new("h0", None).unwrap()],
            vec![network],
        )
        .unwrap();
        let series_module =
            powerio_core::PioModule::new(powerio::PioValue::BalancedNetworkTimeSeries(series));
        let report: serde_json::Value =
            serde_json::from_str(&inspect_json(&series_module)).unwrap();
        assert!(report.get("source_format").is_none(), "{report}");
    }

    /// The end to end path Julia's `write_file(m, path)` with no `format`
    /// depends on: `pio_parse_file` on a real MATPOWER file reports
    /// `source_format` through the full C ABI surface, and it survives a
    /// `.pio.json` save and reload — the exact round trip a script doing
    /// `m = parse_file(...); write_json(m, "case.pio.json")`, later reopened
    /// with `PowerIO.parse_file("case.pio.json")`, then `write_file(m2,
    /// "out.m")` with no format, depends on.
    #[test]
    fn source_format_reaches_pio_parse_file_and_survives_a_pio_json_round_trip() {
        unsafe {
            let path = CString::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../tests/data/case9.m"
            ))
            .unwrap();
            let mut error = std::ptr::null_mut();
            let module = pio_parse_file(path.as_ptr(), std::ptr::null(), &raw mut error);
            assert!(
                error.is_null(),
                "{:?}",
                CStr::from_ptr(pio_error_message(error))
            );
            let report = pio_module_inspect_json(module, &raw mut error);
            assert!(error.is_null());
            let parsed: serde_json::Value =
                serde_json::from_str(CStr::from_ptr(report).to_str().unwrap()).unwrap();
            assert_eq!(parsed["source_format"], "matpower");
            crate::pio_string_release(report);

            let stored = pio_module_write_json(module, &raw mut error);
            assert!(error.is_null());
            let stored_text = CStr::from_ptr(stored).to_str().unwrap().to_owned();
            crate::pio_string_release(stored);
            pio_module_release(module);

            let tmp = tempfile::tempdir().unwrap();
            let saved_path = tmp.path().join("case.pio.json");
            std::fs::write(&saved_path, &stored_text).unwrap();
            let saved_path_c = CString::new(saved_path.to_str().unwrap()).unwrap();
            let mut error = std::ptr::null_mut();
            let reloaded = pio_parse_file(saved_path_c.as_ptr(), std::ptr::null(), &raw mut error);
            assert!(
                error.is_null(),
                "{:?}",
                CStr::from_ptr(pio_error_message(error))
            );
            let report = pio_module_inspect_json(reloaded, &raw mut error);
            assert!(error.is_null());
            let parsed: serde_json::Value =
                serde_json::from_str(CStr::from_ptr(report).to_str().unwrap()).unwrap();
            assert_eq!(parsed["source_format"], "matpower");
            crate::pio_string_release(report);
            pio_module_release(reloaded);
        }
    }

    #[test]
    fn a_failure_returns_a_structured_error_handle() {
        unsafe {
            let mut error = std::ptr::null_mut();
            let bad = CString::new("not a case at all").unwrap();
            let module = pio_parse_str(
                std::ptr::null(),
                bad.as_ptr(),
                std::ptr::null(),
                &raw mut error,
            );
            assert!(module.is_null());
            assert!(!error.is_null());
            let code = CStr::from_ptr(pio_error_code(error)).to_str().unwrap();
            assert!(code.contains('.'), "{code}");
            pio_error_release(error);
        }
    }

    #[test]
    fn error_handles_carry_code_message_and_diagnostics() {
        let error = error_from_parts("READ.TEST.CODE", "message text");
        unsafe {
            assert_eq!(
                CStr::from_ptr(pio_error_code(error)).to_str().unwrap(),
                "READ.TEST.CODE"
            );
            // `pio_error_message` reads `CODE: message`, the one shape both
            // `error_from_parts` and `error_from_core` report.
            assert_eq!(
                CStr::from_ptr(pio_error_message(error)).to_str().unwrap(),
                "READ.TEST.CODE: message text"
            );
            let retained = pio_error_retain(error);
            pio_error_release(error);
            // The typed channel carries the same one finding the JSON channel
            // does, in `Diagnostic`'s own serde form.
            let diagnostics = pio_error_diagnostics(retained);
            assert_eq!(pio_diagnostics_len(diagnostics), 1);
            pio_diagnostics_release(diagnostics);
            assert_eq!(
                CStr::from_ptr(pio_error_diagnostics_json(retained))
                    .to_str()
                    .unwrap(),
                r#"[{"code":"READ.TEST.CODE","severity":"error","message":"message text"}]"#
            );
            pio_error_release(retained);
            pio_error_release(std::ptr::null_mut());
        }
    }

    /// The readiness report publishes the 1.0 record shape: `target` keys,
    /// the four 1.0 severities, and no legacy `element_path`.
    #[cfg(feature = "dist")]
    #[test]
    fn lowering_readiness_reports_module_records() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../tests/data/dist/opendss/ieee13/IEEE13Nodeckt.dss"
        );
        let c_path = std::ffi::CString::new(path).unwrap();
        let mut error: *mut PioError = std::ptr::null_mut();
        let module = unsafe { pio_parse_file(c_path.as_ptr(), std::ptr::null(), &raw mut error) };
        assert!(!module.is_null());
        let text = unsafe { pio_module_lowering_readiness_json(module, 100.0, &raw mut error) };
        assert!(!text.is_null());
        let json = unsafe { std::ffi::CStr::from_ptr(text) }.to_str().unwrap();
        let value: serde_json::Value = serde_json::from_str(json).unwrap();
        let records = value["diagnostics"].as_array().expect("records");
        assert!(!records.is_empty());
        for record in records {
            assert!(record.get("element_path").is_none(), "{record}");
            let severity = record["severity"].as_str().unwrap();
            assert!(
                ["error", "warning", "remark", "note"].contains(&severity),
                "{severity}"
            );
            if let Some(target) = record.get("target").and_then(|t| t.as_str()) {
                assert!(target.starts_with('/'), "{target}");
            }
        }
        unsafe { crate::pio_string_release(text) };
        unsafe { pio_module_release(module) };
    }

    /// A refused lowering's typed records keep their pointer targets on the
    /// C channel; the projection that dropped them is gone.
    #[cfg(feature = "dist")]
    #[test]
    fn refused_lowering_keeps_targets_on_the_typed_channel() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../tests/data/dist/opendss/ieee13/IEEE13Nodeckt.dss"
        );
        let c_path = std::ffi::CString::new(path).unwrap();
        let mut error: *mut PioError = std::ptr::null_mut();
        let module = unsafe { pio_parse_file(c_path.as_ptr(), std::ptr::null(), &raw mut error) };
        assert!(!module.is_null());
        let lowered = unsafe { pio_module_lower_to_balanced(module, 100.0, &raw mut error) };
        assert!(lowered.is_null(), "IEEE13 must refuse the lowering");
        assert!(!error.is_null());
        let list = unsafe { pio_error_diagnostics(error) };
        assert!(!list.is_null());
        let len = unsafe { pio_diagnostics_len(list) };
        assert!(len > 0);
        let mut saw_target = false;
        for index in 0..len {
            let target = unsafe { pio_diagnostic_target(list, index) };
            if !target.is_null() {
                let text = unsafe { std::ffi::CStr::from_ptr(target) }
                    .to_str()
                    .unwrap();
                assert!(text.starts_with('/'), "{text}");
                saw_target = true;
            }
        }
        assert!(
            saw_target,
            "at least one record must carry its pointer target"
        );
        unsafe { pio_diagnostics_release(list) };
        unsafe { pio_error_release(error) };
        unsafe { pio_module_release(module) };
    }
}
