//! The module emission dispatcher. One dynamic module emits a named target
//! format; the kind routes to the
//! family writer that owns it, and `pio-json` serializes any kind through the
//! stored document.
//!
//! Same format emission keeps the family writers' echo tier: an unchanged
//! parsed module emits its retained source bytes exactly. Cross format
//! emission serializes the typed value and reports what the target cannot
//! represent through the returned diagnostics.

use powerio_core::{Destination, EmitResult, Error, PioModule};

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
    format == "pio-json"
}

/// True when `format` names the PyPSA CSV folder target.
fn is_pypsa_dir(format: &str) -> bool {
    powerio_tx::format::is_pypsa_csv_name(format)
}

/// True when two names identify the same supported directory format. The
/// retained source carries the format selected by the parser; this comparison
/// admits the public aliases at emission without treating an arbitrary
/// declared directory token as an echoable format.
fn same_directory_format(source: &str, requested: &str) -> bool {
    if is_pypsa_dir(source) && is_pypsa_dir(requested) {
        return true;
    }
    #[cfg(feature = "gridfm")]
    if source.eq_ignore_ascii_case("gridfm") && requested.eq_ignore_ascii_case("gridfm") {
        return true;
    }
    false
}

/// The exact artifact inventory of an unchanged directory source when the
/// requested format is that source's format. Entry names come from the
/// source's bounded portable directory walk, and every byte is acquired
/// through the source's confined, no-symlink path rather than by reopening a
/// caller-controlled path here.
fn echo_retained_directory(
    module: &PioModule<PioValue>,
    format: &str,
) -> Result<Option<Vec<powerio_core::MemoryArtifact>>, Error> {
    let Some(source) = module.source().filter(|source| source.is_directory()) else {
        return Ok(None);
    };
    let Some(source_format) = source.format() else {
        return Ok(None);
    };
    if !same_directory_format(source_format.as_str(), format) {
        return Ok(None);
    }

    let mut artifacts = Vec::new();
    for name in source.entry_names()? {
        let buffer = source.buffer(&name)?;
        artifacts.push(powerio_core::MemoryArtifact::new(
            name,
            buffer.bytes().to_vec(),
        ));
    }
    Ok(Some(artifacts))
}

/// A typed sibling module over `value` carrying every common record and the
/// retained source from the dynamic module. Source descriptors are added
/// first because source map and diagnostic spans validate against them.
fn typed_sibling<T>(module: &PioModule<PioValue>, value: T) -> Result<PioModule<T>, Error> {
    let mut out = PioModule::new(value).with_producer(module.producer().clone());
    for descriptor in module.sources() {
        out.add_source_descriptor(descriptor.clone())?;
    }
    for entry in module.source_map() {
        out.add_source_map_entry(entry.clone())?;
    }
    for diagnostic in module.diagnostics() {
        out.add_diagnostic(diagnostic.clone())?;
    }
    for entry in module.history() {
        out.add_history_entry(entry.clone())?;
    }
    for (namespace, value) in module.extensions() {
        out.insert_extension(namespace.clone(), value.clone())?;
    }
    Ok(match module.source() {
        Some(source) => out.with_source(source.clone()),
        None => out,
    })
}

/// The module's retained source's exact original text, when that source's
/// content is already `format`: the byte exact echo tier every kind should
/// get, not just [`PioValue::BalancedNetwork`] and
/// [`PioValue::MulticonductorNetwork`] (whose family emitters already carry
/// it through `powerio_tx::emit` and `powerio_dist::emit`).
/// `None` when there is no retained source, or its content is not `format`.
///
/// A source declared with an explicit format token (the C ABI and bindings
/// route this way) is trusted directly; a source routed here by content (a
/// bare `.json` file with no declared format, the common case for the CLI
/// and a plain `Source::open`) is reclassified the same way `powerio::parse`'s
/// own routing already did once to land on this kind in the first place.
fn retained_source_matches_case_format(module: &PioModule<PioValue>, format: &str) -> bool {
    use powerio_tx::format::routing::{
        Detection, JsonClass, classify_format_name, classify_json_text,
    };

    let Some(source) = module.source() else {
        return false;
    };
    let Some(requested) = classify_format_name(format).known() else {
        return false;
    };
    let actual = if let Some(declared) = source.format() {
        let Some(actual) = classify_format_name(declared.as_str()).known() else {
            return false;
        };
        actual
    } else {
        let Ok(buffer) = source.primary_buffer() else {
            return false;
        };
        let Ok(text) = std::str::from_utf8(buffer.content_bytes()) else {
            return false;
        };
        match classify_json_text(text) {
            JsonClass::Case(Detection::Known(found)) => found,
            _ => return false,
        }
    };
    requested == actual
}

fn echo_retained_source(module: &PioModule<PioValue>, format: &str) -> Option<String> {
    if !retained_source_matches_case_format(module, format) {
        return None;
    }
    let source = module.source()?;
    let buffer = source.primary_buffer().ok()?;
    // The raw bytes, not the decoded/BOM stripped `content_bytes` used only
    // to reclassify above: an echo must reproduce the source exactly.
    std::str::from_utf8(buffer.bytes()).ok().map(str::to_owned)
}

fn echo_retained_pio_json(module: &PioModule<PioValue>) -> Option<String> {
    let source = module.source()?;
    let buffer = source.primary_buffer().ok()?;
    let content = std::str::from_utf8(buffer.content_bytes()).ok()?;
    let is_stored = match source.format() {
        Some(format) => format.as_str() == "pio-json",
        None => {
            matches!(
                powerio_tx::format::routing::classify_json_text(content),
                powerio_tx::format::routing::JsonClass::Module
            )
        }
    };
    if !is_stored {
        return None;
    }
    // A released 0.9 NetworkPackage is an upgrade input, not a version 1
    // document eligible for exact echo. Retain it for provenance, but emit
    // the decoded module through the current one-way writer.
    let header: serde_json::Value = serde_json::from_str(content).ok()?;
    if header.get("schema").and_then(serde_json::Value::as_str) != Some(crate::stored::SCHEMA_NAME)
        || header.get("version").and_then(serde_json::Value::as_u64)
            != Some(u64::from(crate::stored::SCHEMA_VERSION))
    {
        return None;
    }
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

/// Emit one dynamic module as `format` into `destination`. The kind routes
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
pub fn emit(
    module: &PioModule<PioValue>,
    format: &str,
    destination: Destination,
) -> Result<EmitResult, Error> {
    if is_pio_json(format) {
        let (text, exact) = match echo_retained_pio_json(module) {
            Some(text) => (text, true),
            None => (crate::stored::emit_module(module)?, false),
        };
        let artifact = powerio_core::MemoryArtifact::new(
            powerio_core::ArtifactPath::new("case.pio.json")
                .expect("static name is a valid artifact path"),
            text.into_bytes(),
        );
        return destination.__commit_artifacts(exact, vec![artifact], Vec::new());
    }
    if let Some(artifacts) = echo_retained_directory(module, format)? {
        return destination.__commit_artifacts(true, artifacts, Vec::new());
    }
    match module.value() {
        PioValue::BalancedNetwork(net) => {
            let typed = typed_sibling(module, net.clone())?;
            let typed = if retained_source_matches_case_format(module, format) {
                typed
            } else {
                typed.sever_source()
            };
            if is_pypsa_dir(format) {
                return powerio_tx::__emit_pypsa_csv(&typed, destination);
            }
            let Some(target) = powerio_tx::format::parse_target_format(format) else {
                return Err(unknown_format(format));
            };
            powerio_tx::emit(&typed, target, destination)
        }
        PioValue::MulticonductorNetwork(net) => {
            let Some(target) = powerio_dist::parse_dist_target_format(format) else {
                return Err(unknown_format(format));
            };
            let typed = typed_sibling(module, net.clone())?;
            let typed = if retained_source_matches_case_format(module, format) {
                typed
            } else {
                typed.sever_source()
            };
            powerio_dist::emit(&typed, target, destination)
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

/// True when `format` is a name some family recognizes, used to tell "wrong
/// kind for this format" apart from "no such format".
fn known_format_name(format: &str) -> bool {
    is_pio_json(format)
        || is_pypsa_dir(format)
        || powerio_tx::format::parse_target_format(format).is_some()
        || powerio_dist::parse_dist_target_format(format).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use powerio_core::{
        DiagnosticCode, DiagnosticSeverity, HistoryEntry, HistoryId, HistoryKind, Producer,
        SourceDescriptor, SourceId, SourceMapEntry, SourceRelation, SourceSpan,
    };
    use powerio_tx::{BalancedNetwork, Bus, BusId, BusType};

    #[test]
    fn a_typed_writer_sibling_preserves_every_common_record() {
        let network = BalancedNetwork::in_memory(
            "records",
            100.0,
            vec![Bus::new(BusId(1), BusType::Ref, 230.0)],
            vec![],
        );
        let source = powerio_core::Source::from_bytes("case.m", b"case bytes".to_vec())
            .unwrap()
            .with_format(powerio_core::FormatId::new("matpower").unwrap());
        let source_id = SourceId::new("source-1").unwrap();
        let mut module = PioModule::new(PioValue::BalancedNetwork(network.clone()))
            .with_producer(Producer::new("records-test", "1").unwrap())
            .with_source(source);
        module
            .add_source_descriptor(SourceDescriptor::new(source_id.clone(), "case.m", 10).unwrap())
            .unwrap();
        module
            .add_source_map_entry(
                SourceMapEntry::new(
                    "/buses/0",
                    SourceRelation::Exact,
                    vec![SourceSpan::new(source_id, 0, 4).unwrap()],
                )
                .unwrap(),
            )
            .unwrap();
        module
            .add_diagnostic(powerio_core::Diagnostic::new(
                DiagnosticCode::new("READ.TEST.RECORD").unwrap(),
                DiagnosticSeverity::Remark,
                "record carried to the family writer",
            ))
            .unwrap();
        module
            .add_history_entry(
                HistoryEntry::new(
                    HistoryId::new("history-1").unwrap(),
                    HistoryKind::Parse,
                    "parse_file",
                )
                .unwrap(),
            )
            .unwrap();
        module
            .insert_extension("test.writer", serde_json::json!({"kept": true}))
            .unwrap();

        let sibling = typed_sibling(&module, network).unwrap();
        assert_eq!(sibling.producer(), module.producer());
        assert_eq!(sibling.sources(), module.sources());
        assert_eq!(sibling.source_map(), module.source_map());
        assert_eq!(sibling.diagnostics(), module.diagnostics());
        assert_eq!(sibling.history(), module.history());
        assert_eq!(sibling.extensions(), module.extensions());
        let sibling_source = sibling.source().unwrap();
        assert_eq!(sibling_source.name(), "case.m");
        assert_eq!(
            sibling_source.primary_buffer().unwrap().bytes(),
            b"case bytes"
        );
    }
}
