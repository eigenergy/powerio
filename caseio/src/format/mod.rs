//! The format hub: readers and writers for every supported file format, all
//! meeting at the shared [`Network`](crate::Network).
//!
//! Each format is one module here, owning its reader and/or writer — MATPOWER
//! `.m`, PowerModels JSON, PSS/E `.raw`, PowerWorld `.aux`, plus the write-only
//! EGRET sink. Every input and output format meets at the hub, so adding a
//! format is one module, not a change to any other. [`parse`] reads a file,
//! detecting the format from its extension; [`write_as`] serializes a `Network`
//! to a target. Non-finite numeric values (a MATPOWER `Inf`/`NaN` angle limit,
//! say) are written as JSON `null`.

use std::collections::BTreeSet;

use serde_json::{Map, Value};

use crate::network::{Network, SourceFormat};
use crate::{Error, Result};

mod egret;
mod matpower;
mod powermodels;
mod powerworld;
mod psse;

pub use egret::write_egret_json;
pub use matpower::{parse_matpower, parse_matpower_file, write_matpower};
pub use powermodels::{parse_powermodels_json, write_powermodels_json};
pub use powerworld::{parse_powerworld, write_powerworld};
pub use psse::{parse_psse, write_psse};

/// A target interchange format. See [`write_as`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetFormat {
    /// PowerModels.jl network data JSON.
    PowerModelsJson,
    /// EGRET `ModelData` JSON.
    EgretJson,
    /// PSS/E `.raw` (v33).
    Psse,
    /// PowerWorld auxiliary `.aux`.
    PowerWorld,
    /// MATPOWER `.m` (round-trip; byte-exact when the case kept its source).
    Matpower,
}

impl TargetFormat {
    /// Conventional file extension for this format (no leading dot).
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            TargetFormat::PowerModelsJson | TargetFormat::EgretJson => "json",
            TargetFormat::Psse => "raw",
            TargetFormat::PowerWorld => "aux",
            TargetFormat::Matpower => "m",
        }
    }
}

/// Map a format name (with the common aliases) to a [`TargetFormat`], or `None`
/// if unrecognized. Accepts `matpower`/`m`, `powermodels-json`/`powermodels`/`pm`,
/// `egret-json`/`egret`, `psse`/`raw`, `powerworld`/`aux`. Case-insensitive. The
/// one place the bindings (Python, C ABI) share, so a new format means one new
/// arm here, not three.
#[must_use]
pub fn target_format_from_name(name: &str) -> Option<TargetFormat> {
    Some(match name.to_ascii_lowercase().as_str() {
        "matpower" | "m" => TargetFormat::Matpower,
        "powermodels-json" | "powermodels" | "pm" => TargetFormat::PowerModelsJson,
        "egret-json" | "egret" => TargetFormat::EgretJson,
        "psse" | "raw" => TargetFormat::Psse,
        "powerworld" | "aux" => TargetFormat::PowerWorld,
        _ => return None,
    })
}

/// Read the case at `path` into a [`Network`], choosing the reader from `from`
/// (a format name, see [`target_format_from_name`]) or, when `None`, from the
/// file extension (`m`/`json`/`raw`/`aux`). EGRET JSON is write-only. The one
/// reader the CLI and the Python/C bindings share, so adding a source format is
/// one edit here, not one per binding.
///
/// # Errors
/// [`Error::UnknownFormat`] if `from` is unrecognized or the extension can't be
/// mapped (and for the write-only EGRET format); [`Error::Io`] if the file
/// can't be read; the reader's own [`Error`] on malformed input.
pub fn read_path(path: &std::path::Path, from: Option<&str>) -> Result<Network> {
    let fmt = match from {
        Some(f) => target_format_from_name(f).ok_or_else(|| Error::UnknownFormat(f.to_string()))?,
        None => match path.extension().and_then(|e| e.to_str()) {
            Some("m") => TargetFormat::Matpower,
            Some("json") => TargetFormat::PowerModelsJson,
            Some("raw") => TargetFormat::Psse,
            Some("aux") => TargetFormat::PowerWorld,
            other => {
                return Err(Error::UnknownFormat(format!(
                    "cannot infer from file extension {other:?}; pass an explicit source format"
                )));
            }
        },
    };
    // MATPOWER reads the file itself so the network name comes from the file
    // stem and the buffer moves straight into the retained source (byte-exact
    // round-trip); every other reader takes the file contents through the
    // shared dispatch.
    match fmt {
        TargetFormat::Matpower => crate::parse_matpower_file(path),
        _ => read_text(&std::fs::read_to_string(path)?, fmt),
    }
}

/// Dispatch in-memory `content` to the reader for `fmt` — the single
/// format→reader map, shared by [`read_path`] and [`parse_str`]. EGRET JSON is
/// write-only.
fn read_text(content: &str, fmt: TargetFormat) -> Result<Network> {
    match fmt {
        TargetFormat::Matpower => parse_matpower(content),
        TargetFormat::PowerModelsJson => parse_powermodels_json(content),
        TargetFormat::Psse => parse_psse(content),
        TargetFormat::PowerWorld => parse_powerworld(content),
        TargetFormat::EgretJson => Err(Error::UnknownFormat(
            "EGRET JSON is write-only and cannot be read".to_string(),
        )),
    }
}

/// Parse the case file at `path` into a [`Network`], detecting the format from
/// the file extension (`m`/`json`/`raw`/`aux`). The single high-level read
/// entry point; use [`read_path`] to force a specific source format, or
/// [`parse_str`] for in-memory text.
///
/// # Errors
/// As [`read_path`] with `from = None`.
pub fn parse(path: impl AsRef<std::path::Path>) -> Result<Network> {
    read_path(path.as_ref(), None)
}

/// Parse in-memory case `text` of the named `format` (see
/// [`target_format_from_name`]) into a [`Network`]. EGRET JSON is write-only.
///
/// # Errors
/// [`Error::UnknownFormat`] if `format` is unrecognized or write-only; the
/// reader's own [`Error`] on malformed input.
pub fn parse_str(text: &str, format: &str) -> Result<Network> {
    let fmt =
        target_format_from_name(format).ok_or_else(|| Error::UnknownFormat(format.to_string()))?;
    read_text(text, fmt)
}

/// Output of a conversion: the serialized text plus any fidelity warnings —
/// data the target can't represent, defaults synthesized, or blocks mapped
/// best-effort. An empty `warnings` means a faithful conversion.
#[derive(Debug, Clone)]
pub struct Conversion {
    pub text: String,
    pub warnings: Vec<String>,
}

/// Convert a [`Network`] to `format`. Writing back to the source format echoes
/// the retained source byte-for-byte (the same-format leg of the fidelity
/// contract); otherwise the network is serialized into the target.
#[must_use]
pub fn write_as(net: &Network, format: TargetFormat) -> Conversion {
    if same_format(format, net.source_format) {
        if let Some(src) = &net.source {
            return Conversion {
                text: src.to_string(),
                warnings: Vec::new(),
            };
        }
    }
    match format {
        TargetFormat::PowerModelsJson => write_powermodels_json(net),
        TargetFormat::EgretJson => write_egret_json(net),
        TargetFormat::Psse => write_psse(net),
        TargetFormat::PowerWorld => write_powerworld(net),
        // From another source (or no retained source): canonical MATPOWER from
        // the folded model.
        TargetFormat::Matpower => Conversion {
            text: crate::write_matpower(net),
            warnings: Vec::new(),
        },
    }
}

/// Whether a write target is the same format the network was read from.
fn same_format(target: TargetFormat, source: SourceFormat) -> bool {
    matches!(
        (target, source),
        (TargetFormat::Matpower, SourceFormat::Matpower)
            | (TargetFormat::PowerModelsJson, SourceFormat::PowerModelsJson)
            | (TargetFormat::EgretJson, SourceFormat::EgretJson)
            | (TargetFormat::Psse, SourceFormat::Psse)
            | (TargetFormat::PowerWorld, SourceFormat::PowerWorld)
    )
}

/// JSON number for a finite `f64`; `Value::Null` for `NaN`/`±Inf`.
pub(crate) fn jnum(x: f64) -> Value {
    serde_json::Number::from_f64(x).map_or(Value::Null, Value::Number)
}

/// Serialize a built JSON tree into a [`Conversion`], appending one warning that
/// names every field where a non-finite `f64` was written as `null` (JSON has no
/// `±Inf`/`NaN`). Shared by the JSON writers.
pub(crate) fn finish(root: Map<String, Value>, mut warnings: Vec<String>) -> Conversion {
    let value = Value::Object(root);
    let mut nulls = BTreeSet::new();
    collect_null_keys(&value, &mut nulls);
    if !nulls.is_empty() {
        warnings.push(format!(
            "non-finite numeric values written as JSON null in field(s): {}",
            nulls.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    let text = serde_json::to_string_pretty(&value).expect("a serde_json::Value always serializes");
    Conversion { text, warnings }
}

/// Collect the names of object keys whose value is `null`, anywhere in the tree.
fn collect_null_keys(value: &Value, out: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            for (key, val) in map {
                if val.is_null() {
                    out.insert(key.clone());
                } else {
                    collect_null_keys(val, out);
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|v| collect_null_keys(v, out)),
        _ => {}
    }
}
