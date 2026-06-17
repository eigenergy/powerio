//! Readers and writers for supported case formats, all meeting at [`Network`].
//!
//! Each format module owns its reader and/or writer: MATPOWER `.m`,
//! PowerModels JSON, PSS/E `.raw`, PowerWorld `.aux`, egret `ModelData` JSON,
//! pandapower JSON, PyPSA CSV folders, and PSLF `.epc`. PowerWorld `.pwb`,
//! PSLF `.epc`, and PowerWorld `.pwd` displays are read only. Case input and
//! output formats meet here, so adding a writable format is one module plus
//! one hub registration.
//! [`parse_file`] reads Network cases, detecting the format from its extension;
//! [`parse_display_file`] reads display artifacts such as PowerWorld `.pwd`.
//! [`write_as`] serializes a `Network` to text targets. Directory formats,
//! such as PyPSA CSV folders, use explicit filesystem helpers. Non-finite
//! numeric values, such as MATPOWER `Inf`/`NaN` angle limits, are written as
//! JSON `null`.
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
//!   never dropped silently. On the read side, readers itemize what they ignore
//!   in [`Parsed`] `warnings`.

use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use serde_json::{Map, Value};

use crate::network::{Bus, BusId, BusType, Network, SourceFormat};
use crate::{Error, Result};

mod egret;
mod matpower;
mod pandapower;
mod powermodels;
pub mod powerworld;
mod pslf;
mod psse;
mod pypsa;

pub use egret::{parse_egret_json, write_egret_json};
pub use matpower::{parse_matpower, parse_matpower_file, write_matpower};
pub use pandapower::{parse_pandapower_json, write_pandapower_json};
pub use powermodels::{parse_powermodels_json, write_powermodels_json};
pub use powerworld::{PwdDisplay, PwdSubstation, parse_powerworld, write_powerworld};
pub use pslf::{parse_pslf, write_pslf};
pub use psse::{parse_psse, write_psse, write_psse_rev};
pub use pypsa::{PypsaCsvOutputs, read_pypsa_csv_folder, write_pypsa_csv_folder};

/// A target interchange format. See [`write_as`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TargetFormat {
    /// PowerModels.jl network data JSON.
    PowerModelsJson,
    /// egret `ModelData` JSON.
    EgretJson,
    /// PSS/E `.raw` at the given revision. `rev` selects the record layout the
    /// writer emits (33, 34, or 35); 33 is the historical default. The reader
    /// takes the revision from the file header, so this only affects writes.
    Psse { rev: u32 },
    /// PowerWorld auxiliary `.aux`.
    PowerWorld,
    /// pandapower `pandapowerNet` JSON.
    PandapowerJson,
    /// MATPOWER `.m` (round-trip; byte-exact when the case kept its source).
    Matpower,
    /// GE PSLF `.epc` (round-trip; byte-exact when the case kept its source).
    Pslf,
}

impl TargetFormat {
    /// Conventional file extension for this format (no leading dot).
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            TargetFormat::PowerModelsJson
            | TargetFormat::EgretJson
            | TargetFormat::PandapowerJson => "json",
            TargetFormat::Psse { .. } => "raw",
            TargetFormat::PowerWorld => "aux",
            TargetFormat::Matpower => "m",
            TargetFormat::Pslf => "epc",
        }
    }

    /// Human-readable format name for diagnostics.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            TargetFormat::PowerModelsJson => "PowerModels JSON",
            TargetFormat::EgretJson => "egret JSON",
            TargetFormat::Psse { .. } => "PSS/E .raw",
            TargetFormat::PowerWorld => "PowerWorld .aux",
            TargetFormat::PandapowerJson => "pandapower JSON",
            TargetFormat::Matpower => "MATPOWER .m",
            TargetFormat::Pslf => "PSLF .epc",
        }
    }

    /// Canonical API token for this format.
    #[must_use]
    pub fn token(self) -> &'static str {
        match self {
            TargetFormat::PowerModelsJson => "powermodels-json",
            TargetFormat::EgretJson => "egret-json",
            TargetFormat::Psse { rev: 34 } => "psse34",
            TargetFormat::Psse { rev: 35 } => "psse35",
            TargetFormat::Psse { .. } => "psse",
            TargetFormat::PowerWorld => "powerworld",
            TargetFormat::PandapowerJson => "pandapower-json",
            TargetFormat::Matpower => "matpower",
            TargetFormat::Pslf => "pslf",
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

/// A display artifact format. These files are not power network cases and do
/// not parse to [`Network`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DisplayFormat {
    /// PowerWorld oneline display `.pwd`.
    PowerWorld,
}

impl DisplayFormat {
    /// Conventional file extension for this display format (no leading dot).
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            DisplayFormat::PowerWorld => "pwd",
        }
    }

    /// Human-readable format name for diagnostics.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            DisplayFormat::PowerWorld => "PowerWorld .pwd",
        }
    }

    /// Canonical API token for this format.
    #[must_use]
    pub fn token(self) -> &'static str {
        match self {
            DisplayFormat::PowerWorld => "powerworld-display",
        }
    }
}

impl fmt::Display for DisplayFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.token())
    }
}

impl FromStr for DisplayFormat {
    type Err = Error;

    fn from_str(name: &str) -> Result<Self> {
        display_format_from_name(name).ok_or_else(|| Error::UnknownFormat(name.to_string()))
    }
}

/// Map a display format name to a [`DisplayFormat`], or `None` if unrecognized.
/// Accepts `pwd`, `powerworld-pwd`, and `powerworld-display`.
#[must_use]
pub fn display_format_from_name(name: &str) -> Option<DisplayFormat> {
    Some(match name.to_ascii_lowercase().as_str() {
        "pwd" | "powerworld-pwd" | "powerworld-display" => DisplayFormat::PowerWorld,
        _ => return None,
    })
}

/// Map a format name (with the common aliases) to a [`TargetFormat`], or `None`
/// if unrecognized. Accepts `matpower`/`m`, `powermodels-json`/`powermodels`/`pm`,
/// `egret-json`/`egret`, `pandapower-json`/`pandapower`/`pp`, `psse`/`raw`,
/// `powerworld`/`aux`. Case-insensitive. The one place the bindings (Python, C
/// ABI) share, so a new text format means one new arm here, not three. PyPSA
/// CSV folders are directory inputs with no text target; their aliases are
/// matched by the private `is_pypsa_csv_name` next to this.
///
/// The `powermodelsjson`/`egretjson`/`pandapowerjson` aliases let a
/// [`SourceFormat`]'s string form (`{:?}` lowercased, e.g. `"PowerModelsJson"`)
/// round-trip back to a target, so `net.to_format(other.source_format)` works
/// for every format.
#[must_use]
pub fn target_format_from_name(name: &str) -> Option<TargetFormat> {
    Some(match name.to_ascii_lowercase().as_str() {
        "matpower" | "m" => TargetFormat::Matpower,
        "powermodels-json" | "powermodels" | "powermodelsjson" | "pm" => {
            TargetFormat::PowerModelsJson
        }
        "egret-json" | "egret" | "egretjson" => TargetFormat::EgretJson,
        "psse" | "raw" | "psse33" | "raw33" => TargetFormat::Psse { rev: 33 },
        "psse34" | "raw34" => TargetFormat::Psse { rev: 34 },
        "psse35" | "raw35" => TargetFormat::Psse { rev: 35 },
        "powerworld" | "aux" => TargetFormat::PowerWorld,
        "pandapower-json" | "pandapower" | "pandapowerjson" | "pp" => TargetFormat::PandapowerJson,
        "pslf" | "epc" | "pslf-epc" => TargetFormat::Pslf,
        _ => return None,
    })
}

/// Output of a display parse. v0.2.2 supports PowerWorld `.pwd`; future display
/// formats can add variants without changing the parse entry point.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum DisplayData {
    /// PowerWorld oneline display data.
    PowerWorld(PwdDisplay),
}

impl DisplayData {
    /// The display format represented by this value.
    #[must_use]
    pub fn format(&self) -> DisplayFormat {
        match self {
            DisplayData::PowerWorld(_) => DisplayFormat::PowerWorld,
        }
    }
}

fn display_file_guidance() -> Error {
    Error::UnknownFormat(
        "a PowerWorld .pwd is display data, not a Network case; \
         use parse_display_file(path, None)"
            .into(),
    )
}

/// Parse display bytes in the named display `format`.
///
/// # Errors
/// [`Error::UnknownFormat`] if `format` is not a display format; otherwise the
/// reader's own [`Error`] on malformed input.
pub fn parse_display_bytes(bytes: &[u8], format: &str) -> Result<DisplayData> {
    let fmt =
        display_format_from_name(format).ok_or_else(|| Error::UnknownFormat(format.to_string()))?;
    match fmt {
        DisplayFormat::PowerWorld => Ok(DisplayData::PowerWorld(powerworld::parse_pwd_display(
            bytes,
        )?)),
    }
}

/// Parse the display file at `path`, choosing the reader from `from` or, when
/// `None`, from the extension. v0.2.2 infers PowerWorld `.pwd`.
///
/// # Errors
/// [`Error::UnknownFormat`] if `from` is unrecognized or the extension cannot
/// be mapped; [`Error::Io`] if the file cannot be read; the reader's own
/// [`Error`] on malformed input.
pub fn parse_display_file(
    path: impl AsRef<std::path::Path>,
    from: Option<&str>,
) -> Result<DisplayData> {
    let path = path.as_ref();
    let fmt = match from {
        Some(f) => {
            display_format_from_name(f).ok_or_else(|| Error::UnknownFormat(f.to_string()))?
        }
        None => match path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("pwd") => DisplayFormat::PowerWorld,
            other => {
                return Err(Error::UnknownFormat(format!(
                    "cannot infer display format from file extension {other:?}; \
                     pass an explicit display format"
                )));
            }
        },
    };
    let bytes = std::fs::read(path)?;
    match fmt {
        DisplayFormat::PowerWorld => Ok(DisplayData::PowerWorld(powerworld::parse_pwd_display(
            &bytes,
        )?)),
    }
}

/// Whether a format name means a PyPSA CSV folder. PyPSA folders are directory
/// inputs, not text targets, so they have no [`TargetFormat`] arm; this is the
/// companion alias matcher to [`target_format_from_name`] and the one place the
/// PyPSA aliases live.
fn is_pypsa_csv_name(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().replace(['-', '_'], "").as_str(),
        "pypsacsv" | "pypsa"
    )
}

/// Whether a source format name means PSLF EPC. PSLF is read only and
/// deliberately stays out of [`TargetFormat`].
fn is_pslf_name(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().replace(['-', '_'], "").as_str(),
        "pslf" | "epc" | "pslfepc"
    )
}

/// Parse the case file at `path`.
///
/// `from` can force a reader by name: [`target_format_from_name`] names plus
/// `pypsa-csv`/`pypsa`, `pwb`, `pslf`, and `epc`. With `from = None`, a
/// directory containing `network.csv` parses as PyPSA CSV and a file maps by
/// extension (`m`/`json`/`raw`/`aux`/`pwb`/`epc`), case insensitively. JSON is
/// sniffed as pandapower, egret, or PowerModels. PowerWorld `.pwb` is binary
/// and read only; PSLF `.epc` is text and read only. Returns [`Parsed`]: the
/// network plus reader warnings.
///
/// The one path-based parser the CLI and the Python/C/Julia bindings share (each
/// exposes the same `parse_file(path, from)` shape), so adding a source format is
/// one edit here, not one per binding. For in-memory text use [`parse_str`].
///
/// # Errors
/// [`Error::UnknownFormat`] if `from` is unrecognized or the extension can't be
/// mapped; [`Error::Io`] if the file can't be read; the reader's own [`Error`]
/// on malformed input.
pub fn parse_file(path: impl AsRef<std::path::Path>, from: Option<&str>) -> Result<Parsed> {
    let path = path.as_ref();
    // PyPSA CSV folders are directories, not files; dispatch them before any
    // extension logic. `from` accepts the pypsa aliases, and a bare directory
    // with a `network.csv` auto-detects.
    if from.is_some_and(is_pypsa_csv_name)
        || (from.is_none() && path.is_dir() && path.join("network.csv").is_file())
    {
        return pypsa::read_pypsa_csv_folder(path);
    }
    // PowerWorld `.pwb` is binary and read only; dispatch it before the text
    // read. `from` accepts "pwb" for files with a different extension.
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    if from.is_some_and(|f| f.eq_ignore_ascii_case("pwb"))
        || (from.is_none() && ext.as_deref() == Some("pwb"))
    {
        let bytes = std::fs::read(path)?;
        let stem = path.file_stem().and_then(|s| s.to_str());
        // The binary reader is total (no fidelity warnings); wrap its network
        // in the shared [`Parsed`] shape.
        let network = powerworld::parse_pwb(&bytes, stem)?;
        return Ok(Parsed {
            network,
            warnings: Vec::new(),
        });
    }
    if from.is_some_and(is_pslf_name) || (from.is_none() && ext.as_deref() == Some("epc")) {
        let text = std::fs::read_to_string(path)?;
        let stem = path.file_stem().and_then(|s| s.to_str());
        let mut warnings = Vec::new();
        let network = pslf::parse_pslf_source(Arc::new(text), stem, &mut warnings)?;
        reject_empty_case(&network, "PSLF .epc")?;
        return Ok(Parsed { network, warnings });
    }
    // Settle the format before touching the file: an unmapped or binary
    // extension must surface as UnknownFormat, not as the UTF-8 read error
    // the text formats' loader would hit first. `.pwd` gets its own arm
    // because the display sibling ships next to every case file in the wild
    // and carries no case data.
    if from.is_none() && ext.as_deref() == Some("pwd") {
        return Err(display_file_guidance());
    }
    let fmt_hint = match from {
        Some(f) => {
            if display_format_from_name(f).is_some() {
                return Err(display_file_guidance());
            }
            Some(target_format_from_name(f).ok_or_else(|| Error::UnknownFormat(f.to_string()))?)
        }
        None => {
            // Everything but `.json` (sniffed below) resolves without the text.
            match ext.as_deref() {
                Some("m") => Some(TargetFormat::Matpower),
                Some("raw") => Some(TargetFormat::Psse { rev: 33 }),
                Some("aux") => Some(TargetFormat::PowerWorld),
                Some("json") => None,
                other => {
                    return Err(Error::UnknownFormat(format!(
                        "cannot infer from file extension {other:?}; \
                         pass an explicit source format"
                    )));
                }
            }
        }
    };
    // Read the file once into an owned buffer; the reader moves it straight into
    // the retained source (byte-exact round-trip) with no copy. Sniffing a
    // `.json` borrows the text before the move.
    let text = std::fs::read_to_string(path)?;
    let fmt = fmt_hint.unwrap_or_else(|| sniff_json(&text));
    // The file stem is the name hint for formats that don't carry their own name.
    let stem = path.file_stem().and_then(|s| s.to_str());
    read_source(Arc::new(text), fmt, stem)
}

/// Read an owned `source` buffer as `fmt`, using `name_hint` (e.g. the file
/// stem) when the format carries no name of its own. The single format→reader
/// map: [`parse_file`] and [`parse_str`] both funnel through it, so every format
/// is dispatched the same way. Each reader takes the owned `Arc` so
/// it moves the buffer straight into the retained source (no copy) and is free
/// to specialize its parse internally. Owns the [`Parsed`] warnings vector;
/// readers that report fidelity loss append to it.
fn read_source(source: Arc<String>, fmt: TargetFormat, name_hint: Option<&str>) -> Result<Parsed> {
    let mut warnings = Vec::new();
    let net = match fmt {
        TargetFormat::Matpower => matpower::parse_matpower_source(source, name_hint),
        TargetFormat::PowerModelsJson => {
            powermodels::parse_powermodels_json_source(source, name_hint)
        }
        TargetFormat::Psse { .. } => psse::parse_psse_source(source, name_hint),
        TargetFormat::PowerWorld => powerworld::parse_powerworld_source(source, name_hint),
        TargetFormat::EgretJson => egret::parse_egret_source(source, name_hint),
        TargetFormat::PandapowerJson => {
            pandapower::parse_pandapower_source(source, name_hint, &mut warnings)
        }
        // PSLF read normally enters through the `is_pslf_name`/`.epc` fast path in
        // parse_file / parse_str; this arm keeps the funnel total.
        TargetFormat::Pslf => pslf::parse_pslf_source(source, name_hint, &mut warnings),
    }?;
    reject_empty_case(&net, fmt.label())?;
    Ok(Parsed {
        network: net,
        warnings,
    })
}

/// A case with no buses is content-free for every consumer. Most readers
/// already reject it on a missing required table, but a JSON carrying only
/// `baseMVA` would otherwise parse to a hollow network; reject it in the
/// [`read_source`] funnel so every parse path (file and in-memory) is guarded,
/// and in the PyPSA folder reader, which bypasses the funnel.
pub(crate) fn reject_empty_case(net: &Network, format: &'static str) -> Result<()> {
    if net.buses.is_empty() {
        return Err(Error::FormatRead {
            format,
            message: "case has no buses".into(),
        });
    }
    Ok(())
}

/// The interchange JSON formats share the `.json` extension, so an explicit
/// source format isn't always given. Sniff three ways: pandapower declares
/// itself (`"_class": "pandapowerNet"`); egret `ModelData` has top level
/// `elements` and `system`; else fall back to PowerModels (the more common
/// input).
///
/// Deserializing into [`IgnoredAny`] fields scans the JSON to find the
/// top level keys without building the whole `Value` tree, so a large
/// PowerModels file isn't fully allocated here only to be parsed again by its
/// reader.
fn sniff_json(text: &str) -> TargetFormat {
    use serde::de::IgnoredAny;
    #[derive(serde::Deserialize)]
    struct Shape {
        #[serde(rename = "_class")]
        class: Option<String>,
        elements: Option<IgnoredAny>,
        system: Option<IgnoredAny>,
    }
    match serde_json::from_str::<Shape>(text) {
        Ok(Shape {
            class: Some(class), ..
        }) if class == "pandapowerNet" => TargetFormat::PandapowerJson,
        Ok(Shape {
            elements: Some(_),
            system: Some(_),
            ..
        }) => TargetFormat::EgretJson,
        _ => TargetFormat::PowerModelsJson,
    }
}

/// Parse in-memory case `text` of the named `format` (see
/// [`target_format_from_name`]). Returns [`Parsed`]: the network plus the
/// reader's fidelity warnings.
///
/// # Errors
/// [`Error::UnknownFormat`] if `format` is unrecognized; the reader's own
/// [`Error`] on malformed input.
pub fn parse_str(text: &str, format: &str) -> Result<Parsed> {
    if is_pslf_name(format) {
        let mut warnings = Vec::new();
        let network = pslf::parse_pslf_source(Arc::new(text.to_owned()), None, &mut warnings)?;
        reject_empty_case(&network, "PSLF .epc")?;
        return Ok(Parsed { network, warnings });
    }
    let fmt =
        target_format_from_name(format).ok_or_else(|| Error::UnknownFormat(format.to_string()))?;
    read_source(Arc::new(text.to_owned()), fmt, None)
}

/// Output of a parse: the network plus the reader's fidelity warnings —
/// tables and columns the model cannot carry, reported instead of dropped
/// silently. Empty for readers that don't report read warnings (currently
/// every format except pandapower JSON and PyPSA CSV; the PSS/E and
/// PowerWorld reductions are documented in docs/format-fidelity.md, not
/// reported here yet).
///
/// `#[non_exhaustive]`: a returns-only type, so downstream code reads it but
/// never constructs it, leaving room to add parse metadata without a breaking
/// change.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Parsed {
    pub network: Network,
    pub warnings: Vec<String>,
}

/// Output of a conversion: the serialized text plus any fidelity warnings:
/// data the target can't represent, defaults synthesized, or blocks mapped best
/// effort. An empty `warnings` means a faithful conversion. For [`convert_file`]
/// and [`convert_str`], `warnings` carries the read side ([`Parsed`] warnings)
/// too, ahead of the write side.
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
    if is_echo(net, format) {
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
        TargetFormat::Psse { rev } => write_psse_rev(net, rev),
        TargetFormat::PowerWorld => write_powerworld(net),
        TargetFormat::PandapowerJson => write_pandapower_json(net),
        // From another source (or no retained source): canonical MATPOWER from
        // the folded model, which itemizes what it can't carry (HVDC, gen caps,
        // extras, a partial-cost case).
        TargetFormat::Matpower => matpower::write_matpower_conversion(net),
        TargetFormat::Pslf => write_pslf(net),
    };
    warn_normalized_tap(net, format, &mut conv);
    warn_missing_reference(net, format, &mut conv);
    warn_dropped_frequency(net, format, &mut conv);
    conv
}

/// Warn when a non-default system frequency writes to a format with no frequency
/// field. PSS/E (`BASFRQ`) and pandapower (`f_hz`) carry it; MATPOWER,
/// PowerModels, egret, and PowerWorld have nowhere to put it, so a 50 Hz case
/// would silently read back as the 60 Hz default. Report the loss instead.
fn warn_dropped_frequency(net: &Network, format: TargetFormat, conv: &mut Conversion) {
    let carries_frequency = matches!(
        format,
        TargetFormat::Psse { .. } | TargetFormat::PandapowerJson
    );
    if carries_frequency {
        return;
    }
    if (net.base_frequency - crate::network::DEFAULT_BASE_FREQUENCY).abs() > 1e-9 {
        conv.warnings.push(format!(
            "system base frequency {} Hz dropped: {} has no frequency field (reads back as {} Hz)",
            net.base_frequency,
            format.label(),
            crate::network::DEFAULT_BASE_FREQUENCY
        ));
    }
}

/// Convert a case file to `to`, optionally forcing the source format with
/// `from`.
///
/// This is the canonical file-conversion helper shared by the bindings. It
/// parses `path` once, writes the resulting [`Network`] to `to`, and returns the
/// converted text plus any fidelity warnings, read side first. An echo (writing
/// back to the source format) returns the retained text with no warnings.
///
/// # Errors
/// As [`parse_file`].
pub fn convert_file(
    path: impl AsRef<std::path::Path>,
    to: TargetFormat,
    from: Option<&str>,
) -> Result<Conversion> {
    let parsed = parse_file(path, from)?;
    let mut conv = write_as(&parsed.network, to);
    if !is_echo(&parsed.network, to) {
        conv.warnings.splice(0..0, parsed.warnings);
    }
    Ok(conv)
}

/// Convert in-memory case `text` of the named `format` (see
/// [`target_format_from_name`]) to `to`.
///
/// The in-memory sibling of [`convert_file`], shared by the bindings: parses
/// `text` once and writes the resulting [`Network`] to `to`, with no file
/// staging in between. Warnings are read side first, as in [`convert_file`].
///
/// # Errors
/// As [`parse_str`].
pub fn convert_str(text: &str, to: TargetFormat, format: &str) -> Result<Conversion> {
    let parsed = parse_str(text, format)?;
    let mut conv = write_as(&parsed.network, to);
    if !is_echo(&parsed.network, to) {
        conv.warnings.splice(0..0, parsed.warnings);
    }
    Ok(conv)
}

/// Warn when a network with no reference (slack) bus converts to a format
/// whose solvers require one. PowerWorld `.pwb` is the one source that
/// systematically lacks the designation (the binary does not store it), so
/// the silent case would be common; `to_normalized` synthesizes a slack at
/// the largest pmax in service generator bus for consumers that need one.
fn warn_missing_reference(net: &Network, format: TargetFormat, conv: &mut Conversion) {
    let needs_ref = matches!(
        format,
        TargetFormat::Matpower
            | TargetFormat::Psse { .. }
            | TargetFormat::PowerModelsJson
            | TargetFormat::PandapowerJson
            | TargetFormat::Pslf
    );
    if needs_ref {
        conv.warnings.extend(missing_reference_warning(net));
    }
}

/// The slackless-network warning itself, shared with the PyPSA folder writer
/// (which produces `PypsaCsvOutputs`, not a [`Conversion`], so it cannot go
/// through [`warn_missing_reference`]).
pub(super) fn missing_reference_warning(net: &Network) -> Option<String> {
    (!net.buses.iter().any(|b| b.kind == BusType::Ref)).then(|| {
        "no reference (slack) bus in the source network; power flow tools \
         reject such cases — to_normalized synthesizes a slack at the \
         largest pmax in service generator bus"
            .to_string()
    })
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
    if matches!(format, TargetFormat::Matpower) {
        return;
    }
    conv.warnings.extend(normalized_tap_warning(net));
}

/// The normalized-label warning itself, shared with the PyPSA folder writer.
// `tap == 1.0` / `shift == 0.0` are exact by construction (see
// `warn_normalized_tap`), so an epsilon compare would be wrong here.
#[allow(clippy::float_cmp)]
pub(super) fn normalized_tap_warning(net: &Network) -> Option<String> {
    if !net.is_normalized() {
        return None;
    }
    // After normalization a line (raw tap 0) and a unity-ratio transformer (raw
    // tap 1) both read as tap 1.0 / shift 0.0, so they cannot be told apart. Count
    // them together as the branches whose line/transformer label is now ambiguous.
    let ambiguous = net
        .branches
        .iter()
        .filter(|b| b.tap == 1.0 && b.shift == 0.0)
        .count();
    (ambiguous > 0).then(|| {
        format!(
            "normalized network: {ambiguous} branch(es) have unit tap and no phase \
             shift, so the line/transformer label is not preserved (the power flow \
             is identical)"
        )
    })
}

/// True when `value` is set and deviates from `reference`: the shared test for
/// "does this rating column carry information the target cannot" used by the
/// rate_b/rate_c drop warnings.
fn nonzero_differs(value: f64, reference: f64) -> bool {
    value.abs() > f64::EPSILON && (value - reference).abs() > f64::EPSILON
}

/// Set a bus's kind through the `bus_pos` index, leaving Isolated buses alone.
/// Shared by the readers that derive bus kinds from generator/slack tables.
pub(crate) fn set_bus_kind(
    buses: &mut [Bus],
    bus_pos: &HashMap<BusId, usize>,
    bus: BusId,
    kind: BusType,
) {
    if let Some(&idx) = bus_pos.get(&bus) {
        if buses[idx].kind != BusType::Isolated {
            buses[idx].kind = kind;
        }
    }
}

/// `base_kv` of a bus through the `bus_pos` index; 0.0 for an unknown bus.
pub(crate) fn bus_kv(buses: &[Bus], bus_pos: &HashMap<BusId, usize>, bus: BusId) -> f64 {
    bus_pos
        .get(&bus)
        .and_then(|&i| buses.get(i))
        .map_or(0.0, |b| b.base_kv)
}

/// Replace characters that would corrupt a quoted or delimited field with
/// `replacement`, so a free-form name can't shift or truncate the record it sits
/// in. `forbidden` lists the destination's quote, delimiter, and comment chars.
/// Returns the value borrowed unchanged when it holds none of them, so the common
/// clean-name path allocates nothing.
///
/// Each text writer calls this at its quoting seam and warns when the result
/// differs from the input (the substitution silently alters operator-facing
/// names): the PSS/E single-quoted bus name and the PowerWorld double-quoted bus
/// name both interpolate a `Network` name straight into a quoted field, where an
/// embedded quote (or, for PSS/E, the `/` inline-comment delimiter) would shift
/// every later column of the record.
pub(crate) fn sanitize_quoted<'a>(
    value: &'a str,
    forbidden: &[char],
    replacement: char,
) -> std::borrow::Cow<'a, str> {
    if value.contains(forbidden) {
        value
            .chars()
            .map(|c| {
                if forbidden.contains(&c) {
                    replacement
                } else {
                    c
                }
            })
            .collect::<String>()
            .into()
    } else {
        std::borrow::Cow::Borrowed(value)
    }
}

/// Impedance base `v_kv² / base_mva`; 1.0 when either base is missing, so a
/// per-unit ↔ ohm conversion on it is the identity.
pub(crate) fn zbase(v_kv: f64, base_mva: f64) -> f64 {
    if v_kv > 0.0 && base_mva > 0.0 {
        v_kv * v_kv / base_mva
    } else {
        1.0
    }
}

/// Whether writing `net` to `target` echoes the retained source text: the
/// target is the source format and the source is still attached. An echo
/// reproduces the input byte for byte, so read fidelity warnings don't apply.
fn is_echo(net: &Network, target: TargetFormat) -> bool {
    same_format(target, net.source_format) && net.source.is_some()
}

/// Whether a write target is the same format the network was read from.
fn same_format(target: TargetFormat, source: SourceFormat) -> bool {
    matches!(
        (target, source),
        (TargetFormat::Matpower, SourceFormat::Matpower)
            | (TargetFormat::PowerModelsJson, SourceFormat::PowerModelsJson)
            | (TargetFormat::EgretJson, SourceFormat::EgretJson)
            | (TargetFormat::Psse { .. }, SourceFormat::Psse)
            | (TargetFormat::PowerWorld, SourceFormat::PowerWorld)
            | (TargetFormat::PandapowerJson, SourceFormat::PandapowerJson)
            | (TargetFormat::Pslf, SourceFormat::Pslf)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::SourceFormat;

    #[test]
    fn source_format_strings_round_trip_to_a_target() {
        // The bindings expose `source_format` as its `{:?}` form, and
        // `to_format` routes that string back through `target_format_from_name`.
        // Every writable source format must resolve — including PowerModelsJson /
        // EgretJson, whose camel-case names need the `powermodelsjson` /
        // `egretjson` aliases (issue #75).
        for (sf, want) in [
            (SourceFormat::Matpower, TargetFormat::Matpower),
            (SourceFormat::PowerModelsJson, TargetFormat::PowerModelsJson),
            (SourceFormat::EgretJson, TargetFormat::EgretJson),
            (SourceFormat::Psse, TargetFormat::Psse { rev: 33 }),
            (SourceFormat::PowerWorld, TargetFormat::PowerWorld),
            (SourceFormat::PandapowerJson, TargetFormat::PandapowerJson),
            (SourceFormat::Pslf, TargetFormat::Pslf),
        ] {
            let token = format!("{sf:?}");
            assert_eq!(
                target_format_from_name(&token),
                Some(want),
                "source_format {token:?} did not round-trip"
            );
        }
        // The derived/in-memory source formats have no writer target, and
        // neither does the read only .pwb binary.
        for sf in [
            SourceFormat::InMemory,
            SourceFormat::Normalized,
            SourceFormat::Gridfm,
            SourceFormat::PypsaCsv,
            SourceFormat::PowerWorldBinary,
        ] {
            assert_eq!(target_format_from_name(&format!("{sf:?}")), None);
        }
    }
}
