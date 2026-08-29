//! The module write dispatcher: the write twin of [`crate::parse`]. One
//! dynamic module writes to a named target format; the kind routes to the
//! family writer that owns it, and `pio-json` serializes any kind through the
//! stored document.
//!
//! Same format writing keeps the family writers' echo tier: an unchanged
//! parsed module writes its retained source bytes exactly. Cross format
//! writing serializes the typed value and reports what the target cannot
//! represent through the returned diagnostics.

use powerio_core::{Destination, Diagnostic, Error, PioModule, WriteResult};

use crate::PioValue;

pub mod codes {
    powerio_core::diagnostic_codes! {
        REQUEST_WRITE_UNKNOWN_FORMAT = "REQUEST.WRITE.UNKNOWN_FORMAT", Error,
            "the requested target format name is not recognized", category = Request;
        REQUEST_WRITE_UNSUPPORTED_VALUE_KIND = "REQUEST.WRITE.UNSUPPORTED_VALUE_KIND", Error,
            "the module's value kind has no writer for the requested format", category = Request;
    }
}

/// True when `format` names the stored module document.
fn is_pio_json(format: &str) -> bool {
    matches!(format, "pio-json" | "pio_json" | "pio.json")
}

/// True when `format` names the PyPSA CSV folder target.
fn is_pypsa_dir(format: &str) -> bool {
    matches!(format, "pypsa-csv" | "pypsa")
}

/// A typed sibling module over `value` carrying the dynamic module's source
/// descriptors, diagnostics, and retained source. Source descriptors are
/// added first because diagnostic spans validate against them.
fn typed_sibling<T>(module: &PioModule<PioValue>, value: T) -> Result<PioModule<T>, Error> {
    let mut out = PioModule::new(value);
    for descriptor in module.sources() {
        out.add_source_descriptor(descriptor.clone())?;
    }
    for diagnostic in module.diagnostics() {
        out.add_diagnostic(diagnostic.clone())?;
    }
    Ok(match module.source() {
        Some(source) => out.with_source(source.clone()),
        None => out,
    })
}

/// The module's retained source's exact original text, when that source's
/// content is already `format`: the byte exact echo tier every kind should
/// get, not just [`PioValue::BalancedNetwork`] and
/// [`PioValue::MulticonductorNetwork`] (whose family writers already carry
/// it through `powerio_tx::format::write_as` and `powerio_dist::write_as`).
/// `None` when there is no retained source, or its content is not `format`.
///
/// A source declared with an explicit format token (the C ABI and bindings
/// route this way) is trusted directly; a source routed here by content (a
/// bare `.json` file with no declared format, the common case for the CLI
/// and a plain `Source::open`) is reclassified the same way `powerio::parse`'s
/// own routing already did once to land on this kind in the first place.
fn echo_retained_source(module: &PioModule<PioValue>, format: &str) -> Option<String> {
    use powerio_tx::format::routing::{
        Detection, JsonClass, classify_format_name, classify_json_text,
    };

    let source = module.source()?;
    let requested = classify_format_name(format).known()?;
    let buffer = source.primary_buffer().ok()?;
    let actual = if let Some(declared) = source.format() {
        classify_format_name(declared.as_str()).known()?
    } else {
        let text = std::str::from_utf8(buffer.content_bytes()).ok()?;
        match classify_json_text(text) {
            JsonClass::Case(Detection::Known(found)) => found,
            _ => return None,
        }
    };
    if requested != actual {
        return None;
    }
    // The raw bytes, not the decoded/BOM stripped `content_bytes` used only
    // to reclassify above: an echo must reproduce the source exactly.
    std::str::from_utf8(buffer.bytes()).ok().map(str::to_owned)
}

fn unsupported_kind(module: &PioModule<PioValue>, format: &str) -> Error {
    Error::new(
        &codes::REQUEST_WRITE_UNSUPPORTED_VALUE_KIND,
        format!(
            "a {} module has no {format} writer; pio-json stores any kind, and the \
             network kinds write their family's case formats",
            module.value().kind().as_str()
        ),
    )
}

fn unknown_format(format: &str) -> Error {
    Error::new(
        &codes::REQUEST_WRITE_UNKNOWN_FORMAT,
        format!("{format} is not a recognized target format name"),
    )
}

/// Write one dynamic module as `format` into `destination`. The kind routes
/// to its family writer; `pio-json` serializes any kind as the stored module
/// document. The result carries the complete artifact inventory and the
/// writer's findings.
///
/// # Errors
/// [`codes::REQUEST_WRITE_UNKNOWN_FORMAT`] for a format name nothing
/// recognizes, [`codes::REQUEST_WRITE_UNSUPPORTED_VALUE_KIND`] for a kind the
/// named format cannot state, and the family writer's own failure otherwise.
///
/// # Panics
/// Never on external input: the stored document's fixed artifact name is
/// valid by construction.
pub fn write_module_as(
    module: &PioModule<PioValue>,
    format: &str,
    destination: Destination,
) -> Result<WriteResult, Error> {
    if is_pio_json(format) {
        let text = crate::stored::write_module(module)?;
        let artifact = powerio_core::MemoryArtifact::new(
            powerio_core::ArtifactPath::new("case.pio.json")
                .expect("static name is a valid artifact path"),
            text.into_bytes(),
        );
        return destination.__commit_artifacts(false, vec![artifact], Vec::new());
    }
    match module.value() {
        PioValue::BalancedNetwork(net) => {
            let typed = typed_sibling(module, net.clone())?;
            if is_pypsa_dir(format) {
                return powerio_tx::format::write_pypsa_csv(&typed, destination);
            }
            let Some(target) = powerio_tx::format::target_format_from_name(format) else {
                return Err(unknown_format(format));
            };
            powerio_tx::format::write(&typed, target, destination)
        }
        PioValue::MulticonductorNetwork(net) => {
            let Some(target) = powerio_dist::dist_target_from_name(format) else {
                return Err(unknown_format(format));
            };
            let typed = typed_sibling(module, net.clone())?;
            powerio_dist::write(&typed, target, destination)
        }
        _ => {
            if let Some(text) = echo_retained_source(module, format) {
                let artifact = powerio_core::MemoryArtifact::new(
                    powerio_core::ArtifactPath::new("case")
                        .expect("static name is a valid artifact path"),
                    text.into_bytes(),
                );
                return destination.__commit_artifacts(false, vec![artifact], Vec::new());
            }
            if known_format_name(format) {
                Err(unsupported_kind(module, format))
            } else {
                Err(unknown_format(format))
            }
        }
    }
}

/// [`write_module_as`] for a single text artifact: the converted text and the
/// writer's findings, without touching the filesystem. Directory targets
/// (PyPSA CSV) are refused; write them through a path destination. A
/// multiconductor target with a sidecar (georeferenced DSS and its buscoords
/// CSV) returns the primary text and reports the dropped sidecar as a
/// finding.
///
/// # Errors
/// As [`write_module_as`].
pub fn write_module_str(
    module: &PioModule<PioValue>,
    format: &str,
) -> Result<(String, Vec<Diagnostic>), Error> {
    write_module_str_with_options(module, format, &powerio_tx::WriteOptions::default())
}

/// [`write_module_str`] with the balanced write-time cost policies applied.
/// The policies are a balanced network concern; a multiconductor or stored
/// document target ignores them.
///
/// # Errors
/// As [`write_module_as`].
pub fn write_module_str_with_options(
    module: &PioModule<PioValue>,
    format: &str,
    options: &powerio_tx::WriteOptions,
) -> Result<(String, Vec<Diagnostic>), Error> {
    if is_pypsa_dir(format) {
        return Err(Error::new(
            &codes::REQUEST_WRITE_UNSUPPORTED_VALUE_KIND,
            "pypsa-csv is a directory target; write it through a path destination",
        ));
    }
    if is_pio_json(format) {
        let text = crate::stored::write_module(module)?;
        return Ok((text, Vec::new()));
    }
    match module.value() {
        PioValue::BalancedNetwork(net) => {
            let typed = typed_sibling(module, net.clone())?;
            let Some(target) = powerio_tx::format::target_format_from_name(format) else {
                return Err(unknown_format(format));
            };
            let conv = powerio_tx::format::write_as_with_options(&typed, target, options)?;
            Ok((conv.text, conv.diagnostics))
        }
        PioValue::MulticonductorNetwork(net) => {
            let Some(target) = powerio_dist::dist_target_from_name(format) else {
                return Err(unknown_format(format));
            };
            let typed = typed_sibling(module, net.clone())?;
            let conv = powerio_dist::write_as(&typed, target);
            let mut diagnostics = conv.diagnostics;
            for sidecar in &conv.sidecars {
                diagnostics.push(sidecar.dropped_diagnostic("the text form carries one file"));
            }
            Ok((conv.text, diagnostics))
        }
        _ => {
            if let Some(text) = echo_retained_source(module, format) {
                return Ok((text, Vec::new()));
            }
            if known_format_name(format) {
                Err(unsupported_kind(module, format))
            } else {
                Err(unknown_format(format))
            }
        }
    }
}

/// True when `format` is a name some family recognizes, used to tell "wrong
/// kind for this format" apart from "no such format".
fn known_format_name(format: &str) -> bool {
    is_pio_json(format)
        || is_pypsa_dir(format)
        || powerio_tx::format::target_format_from_name(format).is_some()
        || powerio_dist::dist_target_from_name(format).is_some()
}
