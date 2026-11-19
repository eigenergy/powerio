//! Collection transport and metadata; electrical values keep typed module ownership.
use super::*;

fn limits(max_bytes: u64, max_files: usize) -> Result<powerio::CollectionLimits, *mut PioError> {
    // The caller sets the totals; depth and ratio keep the facade defaults.
    let mut limits = powerio::CollectionLimits::default();
    limits.tree = powerio::SourceTreeLimits::new(
        max_files,
        max_bytes,
        powerio::SourceTreeLimits::default().max_depth(),
    )
    .map_err(|e| error_from_core(&e))?;
    limits.archive = powerio::collection::ArchiveLimits::new(
        max_bytes,
        max_files,
        max_bytes,
        powerio::collection::ArchiveLimits::default().max_ratio(),
    )
    .map_err(|e| error_from_core(&e))?;
    Ok(limits)
}
fn invalid(message: &str) -> *mut PioError {
    boundary_error(&codes::BIND_CAPI_INVALID_ARGUMENT, message)
}
unsafe fn collection<'a>(
    raw: *const PioCollection,
) -> Result<&'a powerio::Collection, *mut PioError> {
    unsafe { PioCollection::get(raw) }.ok_or_else(|| {
        boundary_error(
            &codes::BIND_CAPI_NULL_HANDLE,
            "PioCollection must not be NULL",
        )
    })
}

/// Acquire a folder or ZIP with explicit total byte and file limits.
/// # Safety
/// Pointers and handles must satisfy the crate-level safety requirements.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_collection_open(
    path: *const c_char,
    path_len: usize,
    max_bytes: u64,
    max_files: usize,
    error: *mut *mut PioError,
) -> *mut PioCollection {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let limits = limits(max_bytes, max_files)?;
            powerio::Collection::open(required_str(path, path_len, "path")?, limits)
                .map(PioCollection::new_raw)
                .map_err(|e| error_from_core(&e))
        })
    }
}

/// Copy and verify a ZIP, bounding compressed input before copying.
/// # Safety
/// Pointers and handles must satisfy the crate-level safety requirements.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_collection_from_zip(
    name: *const c_char,
    name_len: usize,
    data: *const u8,
    data_len: usize,
    max_bytes: u64,
    max_files: usize,
    error: *mut *mut PioError,
) -> *mut PioCollection {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let limits = limits(max_bytes, max_files)?;
            if data_len as u64 > max_bytes {
                return Err(invalid("collection compressed byte limit exceeded"));
            }
            let name = required_str(name, name_len, "name")?;
            let bytes = Arc::from(input_bytes(data, data_len, "data")?);
            powerio::Collection::from_zip(name, bytes, limits)
                .map(PioCollection::new_raw)
                .map_err(|e| error_from_core(&e))
        })
    }
}

/// Copy named buffers once into a shared immutable inventory. Checks count and
/// cumulative bytes before copying payloads. No host fallback is possible.
/// # Safety
/// Pointers and handles must satisfy the crate-level safety requirements.
/// buffers addresses buffers_len views; NULL is valid only at zero length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_collection_from_files(
    name: *const c_char,
    name_len: usize,
    buffers: *const PioNamedBufferView,
    buffers_len: usize,
    max_bytes: u64,
    max_files: usize,
    error: *mut *mut PioError,
) -> *mut PioCollection {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let limits = limits(max_bytes, max_files)?;
            if buffers_len > max_files {
                return Err(invalid("collection file limit exceeded"));
            }
            let name = required_str(name, name_len, "name")?;
            let buffers = if buffers_len == 0 {
                &[]
            } else if buffers.is_null() {
                return Err(invalid("buffers is NULL with a nonzero length"));
            } else {
                std::slice::from_raw_parts(buffers, buffers_len)
            };
            let total = buffers
                .iter()
                .try_fold(0u64, |n, b| n.checked_add(b.bytes.len as u64));
            if total.is_none_or(|n| n > max_bytes) {
                return Err(invalid("collection byte limit exceeded"));
            }
            let files = buffers
                .iter()
                .map(|b| {
                    let path = powerio::ArtifactPath::new(required_str(
                        b.name.data,
                        b.name.len,
                        "buffer name",
                    )?)
                    .map_err(|e| error_from_core(&e))?;
                    let bytes = Arc::from(input_bytes(b.bytes.data, b.bytes.len, "buffer bytes")?);
                    Ok((path, bytes))
                })
                .collect::<Result<Vec<_>, *mut PioError>>()?;
            powerio::Collection::from_files(name, files, limits)
                .map(PioCollection::new_raw)
                .map_err(|e| error_from_core(&e))
        })
    }
}

/// Inspect metadata and suggested selections as JSON, without electrical parsing.
/// Release the returned owned string with pio_string_release.
/// # Safety
/// Pointers and handles must satisfy the crate-level safety requirements.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_collection_inspect(
    raw: *const PioCollection,
    error: *mut *mut PioError,
) -> *mut PioString {
    unsafe {
        entry(error, std::ptr::null_mut(), || {
            let collection = collection(raw)?;
            let candidates = collection.inspect().map_err(|e| error_from_core(&e))?;
            Ok(PioString::new_raw(StringInner {
                text: serde_json::json!({
                    "entries":collection.entries(), "candidates":candidates,
                })
                .to_string(),
            }))
        })
    }
}

/// Parse one JSON selection (at most 1 MiB) into an owned typed module. Optional
/// options have the same semantics as pio_parse_with_options, but cannot acquire
/// host files. If report is non-NULL, success also returns an owned JSON string
/// containing observed dependencies and the incomplete flag. Release it with
/// pio_string_release. False incomplete is not a completeness certificate.
/// # Safety
/// Pointers and handles must satisfy the crate-level safety requirements.
/// Non-NULL options and report must be valid for their declared types. Report
/// is cleared before work; its prior value must already have been released.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_collection_parse(
    raw: *const PioCollection,
    selection: *const c_char,
    selection_len: usize,
    options: *const PioParseOptions,
    report: *mut *mut PioString,
    error: *mut *mut PioError,
) -> *mut PioModule {
    unsafe {
        if !report.is_null() {
            *report = std::ptr::null_mut();
        }
        entry(error, std::ptr::null_mut(), || {
            let collection = collection(raw)?;
            if selection_len > 1 << 20 {
                return Err(invalid("selection metadata exceeds 1 MiB"));
            }
            let selected =
                serde_json::from_str(required_str(selection, selection_len, "selection")?)
                    .map_err(|e| invalid(&format!("invalid collection selection: {e}")))?;
            let mut parsed_options = powerio::ParseOptions::default();
            apply_parse_selections(&mut parsed_options, options)?;
            let result = collection
                .parse(&selected, &parsed_options)
                .map_err(|e| error_from_core(&e))?;
            let metadata = serde_json::json!({"dependencies":result.dependencies,"incomplete":result.incomplete}).to_string();
            let module = module_handle(result.module);
            if !report.is_null() {
                *report = PioString::new_raw(StringInner { text: metadata });
            }
            Ok(module)
        })
    }
}

/// Retain an independently releasable collection handle.
/// # Safety
/// Pointers and handles must satisfy the crate-level safety requirements.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_collection_retain(raw: *const PioCollection) -> *mut PioCollection {
    unsafe { PioCollection::retain_raw(raw) }
}
/// Release one owned collection handle. Parsed modules retain their own source.
/// # Safety
/// Pointers and handles must satisfy the crate-level safety requirements.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pio_collection_release(raw: *mut PioCollection) {
    unsafe { PioCollection::release_raw(raw) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ownership_observations_and_failed_selection_are_independent() {
        unsafe {
            let primary =
                b"New Circuit.day basekv=12.47 bus1=source\nRedirect ../shared/topology.inc\n";
            let topology =
                b"New Line.l bus1=source bus2=load phases=3 r1=0.1 x1=0.2 r0=0.3 x0=0.4 length=1\n";
            let files = [
                PioNamedBufferView {
                    name: PioStringView::new("masters/day.dss"),
                    bytes: PioByteView::new(primary),
                },
                PioNamedBufferView {
                    name: PioStringView::new("masters/night.dss"),
                    bytes: PioByteView::new(primary),
                },
                PioNamedBufferView {
                    name: PioStringView::new("shared/topology.inc"),
                    bytes: PioByteView::new(topology),
                },
            ];
            let mut error = std::ptr::null_mut();
            let collection = pio_collection_from_files(
                b"study".as_ptr().cast(),
                5,
                files.as_ptr(),
                files.len(),
                4096,
                10,
                &mut error,
            );
            assert!(error.is_null());
            assert!(!collection.is_null());
            let retained = pio_collection_retain(collection);
            pio_collection_release(collection);
            let inventory = pio_collection_inspect(retained, &mut error);
            assert!(error.is_null());
            assert!(!inventory.is_null());
            pio_string_release(inventory);
            let mut report = std::ptr::null_mut();
            let bad = br#"{"primary":"../escape.dss"}"#;
            assert!(
                pio_collection_parse(
                    retained,
                    bad.as_ptr().cast(),
                    bad.len(),
                    std::ptr::null(),
                    &mut report,
                    &mut error
                )
                .is_null()
            );
            assert!(!error.is_null());
            assert!(report.is_null());
            pio_error_release(error);
            let selection = br#"{"primary":"masters/night.dss","format":"dss"}"#;
            let module = pio_collection_parse(
                retained,
                selection.as_ptr().cast(),
                selection.len(),
                std::ptr::null(),
                &mut report,
                &mut error,
            );
            assert!(error.is_null());
            assert!(!module.is_null());
            let metadata: serde_json::Value =
                serde_json::from_str(&PioString::get(report).unwrap().text).unwrap();
            assert_eq!(
                metadata["dependencies"],
                serde_json::json!(["shared/topology.inc"])
            );
            assert_eq!(metadata["incomplete"], false);
            pio_string_release(report);
            pio_collection_release(retained);
            let value = pio_module_value(module);
            pio_module_release(module);
            assert!(pio_value_is_type(
                value,
                b"powerio.MulticonductorNetwork".as_ptr().cast(),
                29
            ));
            pio_value_release(value);
        }
    }
    #[test]
    fn rejects_null_and_oversize_inputs_before_copy() {
        unsafe {
            let mut error = std::ptr::null_mut();
            assert!(
                pio_collection_from_zip(
                    b"x.zip".as_ptr().cast(),
                    5,
                    std::ptr::null(),
                    100,
                    10,
                    10,
                    &mut error
                )
                .is_null()
            );
            assert!(!error.is_null());
            pio_error_release(error);
            assert!(
                pio_collection_from_files(
                    b"x".as_ptr().cast(),
                    1,
                    std::ptr::null(),
                    1,
                    10,
                    10,
                    &mut error
                )
                .is_null()
            );
            assert!(!error.is_null());
            pio_error_release(error);
            assert!(pio_collection_inspect(std::ptr::null(), &mut error).is_null());
            assert!(!error.is_null());
            pio_error_release(error);
            pio_collection_release(std::ptr::null_mut());
        }
    }
}
