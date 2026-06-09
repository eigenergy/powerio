//! The format hub: readers and writers for every supported file format, all
//! meeting at the shared [`Network`].
//!
//! Each format is one module here, owning its reader and/or writer: MATPOWER
//! `.m`, PowerModels JSON, PSS/E `.raw`, PowerWorld `.aux`, and egret
//! `ModelData` JSON. Every input and output format meets at the hub, so adding a
//! format is one module, not a change to any other. [`parse`] reads a file,
//! detecting the format from its extension; [`write_as`] serializes a `Network`
//! to a target. Non-finite numeric values (a MATPOWER `Inf`/`NaN` angle limit,
//! say) are written as JSON `null`.
//!
//! # Fidelity contract
//!
//! Conversion is two-tier:
//!
//! - **Same format writes return the original text.** A reader keeps its source
//!   text (see [`Network`]), so writing back to the same format returns every
//!   field, comment, and numeric token.
//! - **Cross-format keeps maximal fidelity with itemized loss.** Whatever the
//!   target format cannot represent is reported in the [`Conversion`] `warnings`,
//!   never dropped silently.

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use serde_json::{Map, Value};

use crate::network::{Network, SourceFormat};
use crate::{Error, Result};

mod egret;
mod matpower;
mod powermodels;
mod powerworld;
mod psse;

pub use egret::{parse_egret_json, write_egret_json};
pub use matpower::{parse_matpower, parse_matpower_file, write_matpower};
pub use powermodels::{parse_powermodels_json, write_powermodels_json};
pub use powerworld::{parse_powerworld, write_powerworld};
pub use psse::{parse_psse, write_psse};

/// A target interchange format. See [`write_as`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TargetFormat {
    /// PowerModels.jl network data JSON.
    PowerModelsJson,
    /// egret `ModelData` JSON.
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

    /// Human-readable format name for diagnostics.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            TargetFormat::PowerModelsJson => "PowerModels JSON",
            TargetFormat::EgretJson => "egret JSON",
            TargetFormat::Psse => "PSS/E .raw",
            TargetFormat::PowerWorld => "PowerWorld .aux",
            TargetFormat::Matpower => "MATPOWER .m",
        }
    }

    /// Canonical API token for this format.
    #[must_use]
    pub fn token(self) -> &'static str {
        match self {
            TargetFormat::PowerModelsJson => "powermodels-json",
            TargetFormat::EgretJson => "egret-json",
            TargetFormat::Psse => "psse",
            TargetFormat::PowerWorld => "powerworld",
            TargetFormat::Matpower => "matpower",
        }
    }
}

impl fmt::Display for TargetFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.token())
    }
}

impl FromStr for TargetFormat {
    type Err = Error;

    fn from_str(name: &str) -> Result<Self> {
        target_format_from_name(name).ok_or_else(|| Error::UnknownFormat(name.to_string()))
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
/// file extension (`m`/`json`/`raw`/`aux`). A `.json` file is sniffed for the
/// egret vs PowerModels shape; pass `from` to force one.
/// The one reader the CLI and the Python/C bindings share, so adding a source
/// format is one edit here, not one per binding.
///
/// # Errors
/// [`Error::UnknownFormat`] if `from` is unrecognized or the extension can't be
/// mapped; [`Error::Io`] if the file can't be read; the reader's own [`Error`]
/// on malformed input.
pub fn read_path(path: &std::path::Path, from: Option<&str>) -> Result<Network> {
    // Read the file once into an owned buffer; the reader moves it straight into
    // the retained source (byte-exact round-trip) with no copy. Sniffing a
    // `.json` borrows the text before the move.
    let text = std::fs::read_to_string(path)?;
    let fmt = match from {
        Some(f) => target_format_from_name(f).ok_or_else(|| Error::UnknownFormat(f.to_string()))?,
        None => match path.extension().and_then(|e| e.to_str()) {
            Some("m") => TargetFormat::Matpower,
            Some("json") => sniff_json(&text),
            Some("raw") => TargetFormat::Psse,
            Some("aux") => TargetFormat::PowerWorld,
            other => {
                return Err(Error::UnknownFormat(format!(
                    "cannot infer from file extension {other:?}; pass an explicit source format"
                )));
            }
        },
    };
    // The file stem is the name hint for formats that don't carry their own name.
    let stem = path.file_stem().and_then(|s| s.to_str());
    read_source(Arc::new(text), fmt, stem)
}

/// Read an owned `source` buffer as `fmt`, using `name_hint` (e.g. the file
/// stem) when the format carries no name of its own. The single format→reader
/// map: [`parse`], [`parse_str`], and [`read_path`] all funnel through it, so
/// every format is dispatched the same way. Each reader takes the owned `Arc` so
/// it moves the buffer straight into the retained source (no copy) and is free
/// to specialize its parse internally.
fn read_source(source: Arc<String>, fmt: TargetFormat, name_hint: Option<&str>) -> Result<Network> {
    let net = match fmt {
        TargetFormat::Matpower => matpower::parse_matpower_source(source, name_hint),
        TargetFormat::PowerModelsJson => {
            powermodels::parse_powermodels_json_source(source, name_hint)
        }
        TargetFormat::Psse => psse::parse_psse_source(source, name_hint),
        TargetFormat::PowerWorld => powerworld::parse_powerworld_source(source, name_hint),
        TargetFormat::EgretJson => egret::parse_egret_source(source, name_hint),
    }?;
    // A case with no buses is content-free for every consumer. Most readers
    // already reject it on a missing required table, but a JSON carrying only
    // `baseMVA` would otherwise parse to a hollow network; reject it here so the
    // one funnel guards every parse path (file and in-memory).
    if net.buses.is_empty() {
        return Err(Error::FormatRead {
            format: fmt.label(),
            message: "case has no buses".into(),
        });
    }
    Ok(net)
}

/// Both interchange JSON formats use the `.json` extension, so an explicit
/// source format isn't always given. egret `ModelData` has top-level `elements`
/// and `system`; PowerModels network data does not. Sniff that and fall back to
/// PowerModels (the more common input) when the text is not egret shaped.
///
/// Deserializing into [`IgnoredAny`] fields scans the JSON to find the two
/// top-level keys without building the whole `Value` tree, so a large
/// PowerModels file isn't fully allocated here only to be parsed again by its
/// reader.
fn sniff_json(text: &str) -> TargetFormat {
    use serde::de::IgnoredAny;
    #[derive(serde::Deserialize)]
    struct Shape {
        elements: Option<IgnoredAny>,
        system: Option<IgnoredAny>,
    }
    match serde_json::from_str::<Shape>(text) {
        Ok(Shape {
            elements: Some(_),
            system: Some(_),
        }) => TargetFormat::EgretJson,
        _ => TargetFormat::PowerModelsJson,
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

/// Parse the case file at `path` into a [`Network`].
///
/// This is the canonical path-based parser name shared by the language
/// bindings. It is equivalent to [`parse`].
///
/// # Errors
/// As [`read_path`] with `from = None`.
pub fn parse_file(path: impl AsRef<std::path::Path>) -> Result<Network> {
    read_path(path.as_ref(), None)
}

/// Parse in-memory case `text` of the named `format` (see
/// [`target_format_from_name`]) into a [`Network`].
///
/// # Errors
/// [`Error::UnknownFormat`] if `format` is unrecognized; the reader's own
/// [`Error`] on malformed input.
pub fn parse_str(text: &str, format: &str) -> Result<Network> {
    let fmt =
        target_format_from_name(format).ok_or_else(|| Error::UnknownFormat(format.to_string()))?;
    read_source(Arc::new(text.to_owned()), fmt, None)
}

/// Output of a conversion: the serialized text plus any fidelity warnings:
/// data the target can't represent, defaults synthesized, or blocks mapped best
/// effort. An empty `warnings` means a faithful conversion.
///
/// `#[non_exhaustive]`: a returns-only type, so downstream code reads it but
/// never constructs it, leaving room to add fidelity metadata without a breaking
/// change.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Conversion {
    pub text: String,
    pub warnings: Vec<String>,
}

/// Convert a [`Network`] to `format`. Writing back to the source format returns
/// the retained source text; otherwise the network is serialized into the target.
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
    let mut conv = match format {
        TargetFormat::PowerModelsJson => write_powermodels_json(net),
        TargetFormat::EgretJson => write_egret_json(net),
        TargetFormat::Psse => write_psse(net),
        TargetFormat::PowerWorld => write_powerworld(net),
        // From another source (or no retained source): canonical MATPOWER from
        // the folded model, which itemizes what it can't carry (HVDC, gen caps,
        // extras, a partial-cost case).
        TargetFormat::Matpower => matpower::write_matpower_conversion(net),
    };
    warn_normalized_tap(net, format, &mut conv);
    conv
}

/// Convert a case file to `to`, optionally forcing the source format with
/// `from`.
///
/// This is the canonical file-conversion helper shared by the bindings. It
/// parses `path` once, writes the resulting [`Network`] to `to`, and returns the
/// converted text plus any fidelity warnings.
///
/// # Errors
/// As [`read_path`].
pub fn convert_file(
    path: impl AsRef<std::path::Path>,
    to: TargetFormat,
    from: Option<&str>,
) -> Result<Conversion> {
    let net = read_path(path.as_ref(), from)?;
    Ok(write_as(&net, to))
}

/// A normalized network has its tap canonicalized to `1.0` on every line (the
/// `0 → 1` rule), but [`Branch::is_transformer`](crate::network::Branch::is_transformer),
/// the test these writers use to split lines from transformers, keys off
/// `tap != 0`. So a normalized line is written into the transformer section/type.
/// The power flow is identical (a unity-ratio, zero-shift transformer equals a
/// line), but the label is not, so report the fidelity loss rather than relabel
/// it silently. MATPOWER has no separate transformer representation (just a `TAP`
/// column), so it is exempt.
// `tap == 1.0` / `shift == 0.0` are exact by construction: normalization sets a
// line's tap from `effective_tap()` (the literal `1.0`) and its shift from
// `0.0 * DEG_TO_RAD` (exactly `0.0`), so an epsilon compare would be wrong here.
#[allow(clippy::float_cmp)]
fn warn_normalized_tap(net: &Network, format: TargetFormat, conv: &mut Conversion) {
    if !net.is_normalized() || matches!(format, TargetFormat::Matpower) {
        return;
    }
    // After normalization a line (raw tap 0) and a unity-ratio transformer (raw
    // tap 1) both read as tap 1.0 / shift 0.0, so they cannot be told apart. Count
    // them together as the branches whose line/transformer label is now ambiguous.
    let ambiguous = net
        .branches
        .iter()
        .filter(|b| b.tap == 1.0 && b.shift == 0.0)
        .count();
    if ambiguous > 0 {
        conv.warnings.push(format!(
            "normalized network: {ambiguous} branch(es) have unit tap and no phase \
             shift and are written as transformers; a normalized line is indistinguishable \
             from a transformer whose tap is exactly 1, so the line/transformer label is \
             not preserved (the power flow is identical)"
        ));
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
