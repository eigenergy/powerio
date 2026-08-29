//! Readers and writers for supported case formats, all meeting at [`BalancedNetwork`].
//!
//! Each format module owns its reader and/or writer: MATPOWER `.m`,
//! PowerModels JSON, PSS/E `.raw`, PowerWorld `.aux`, egret `ModelData` JSON,
//! pandapower JSON, PyPSA CSV folders, PSLF `.epc`, GO Challenge 3 JSON, and
//! Surge JSON, and DeepMind OPFData JSON. PowerWorld `.pwb` cases, GO Challenge
//! 3 and OPFData JSON canonical output, and PowerWorld `.pwd` displays are read
//! only. Case input and
//! output formats meet here, so adding a writable format is one module plus
//! one hub registration.
//! [`parse`] reads a retained source into a typed module, detecting the
//! format from the source name and content; [`parse_display_file`] reads
//! display artifacts such as PowerWorld `.pwd`. [`write_as`] writes a parsed
//! module, echoing the retained source on a same format target, and
//! [`write_network`] is the semantic write for bare typed networks.
//! Non-finite numeric values, such as MATPOWER `Inf`/`NaN` angle limits, are
//! written as JSON `null`.
//!
//! # Fidelity behavior
//!
//! Conversion is two-tier:
//!
//! - **Same format writes of an unchanged parsed module return the original
//!   bytes.** The module retains its source, so [`write_as`] back to the same
//!   format returns every field, comment, and numeric token.
//! - **Cross-format keeps maximal fidelity with itemized loss.** Whatever the
//!   target format cannot represent is reported in the [`Conversion`]
//!   findings, never dropped silently. On the read side, readers itemize what
//!   they ignore on the module's diagnostics.

use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::str::FromStr;

use serde_json::{Map, Value};

use powerio_core::{PioModule, SourceDescriptor};

use crate::diagnostics::{Diagnostic, DiagnosticInfo, Diagnostics, EmitFamily, codes};
use crate::gen_cost::{GenCostPatch, MissingGenCostPolicy};
use crate::network::{BalancedNetwork, Branch, BranchRatingSet, Bus, BusId, BusType, SourceFormat};
use crate::{Error, Result};
use routing::{Detection, JsonClass, SourceFormat as DetectedFormat, TransmissionFormat};

mod egret;
#[doc(hidden)]
pub mod goc3;
mod matpower;
mod opfdata;
mod pandapower;
mod powermodels;
pub mod powerworld;
mod pslf;
mod psse;
mod pypsa;
pub mod routing;
mod surge;

pub use egret::write_egret_json;
pub use goc3::parse_goc3_json;
pub use matpower::write_matpower;
pub use pandapower::write_pandapower_json;
pub use powermodels::write_powermodels_json;
pub use powerworld::{PwdDisplay, PwdSubstation, write_powerworld};
pub use pslf::write_pslf;
pub use psse::{write_psse, write_psse_rev};
pub use pypsa::{PypsaCsvOutputs, write_pypsa_csv_folder};
pub use surge::write_surge_json;

/// A target case format. See [`write_as`].
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
    /// ARPA-E GO Challenge 3 JSON input data. This is read only except for
    /// same format source echo when the parsed network still carries its source.
    Goc3Json,
    /// Surge native JSON network document.
    SurgeJson,
    /// One JSON document from a DeepMind OPFData release. Read only except for
    /// an exact write back to the retained source format.
    DeepMindOpfDataJson,
}

impl TargetFormat {
    /// Conventional file extension for this format (no leading dot).
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            TargetFormat::PowerModelsJson
            | TargetFormat::EgretJson
            | TargetFormat::PandapowerJson
            | TargetFormat::Goc3Json
            | TargetFormat::SurgeJson
            | TargetFormat::DeepMindOpfDataJson => "json",
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
            TargetFormat::Goc3Json => "GO Challenge 3 JSON",
            TargetFormat::SurgeJson => "Surge JSON",
            TargetFormat::DeepMindOpfDataJson => "DeepMind OPFData JSON",
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
            TargetFormat::Goc3Json => "goc3-json",
            TargetFormat::SurgeJson => "surge-json",
            TargetFormat::DeepMindOpfDataJson => "opfdata-json",
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
/// not parse to [`BalancedNetwork`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DisplayFormat {
    /// PowerWorld oneline display `.pwd`.
    PowerWorld,
    /// The standalone geographic document ([`crate::geo::GeoLayer`]):
    /// canonical `.geo.json`, read tolerantly from GeoJSON, aliased CSV/JSON
    /// records, and headerless buscoords CSV.
    GeoJson,
}

impl DisplayFormat {
    /// Conventional file extension for this display format (no leading dot).
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            DisplayFormat::PowerWorld => "pwd",
            DisplayFormat::GeoJson => crate::geo::GEO_LAYER_EXTENSION,
        }
    }

    /// Human-readable format name for diagnostics.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            DisplayFormat::PowerWorld => "PowerWorld .pwd",
            DisplayFormat::GeoJson => "geo layer",
        }
    }

    /// Canonical API token for this format.
    #[must_use]
    pub fn token(self) -> &'static str {
        match self {
            DisplayFormat::PowerWorld => "powerworld-display",
            DisplayFormat::GeoJson => "geojson",
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
/// Accepts `pwd`, `powerworld-pwd`, and `powerworld-display`; `geojson`,
/// `geo-json`, and `geo` name the geographic layer.
#[must_use]
pub fn display_format_from_name(name: &str) -> Option<DisplayFormat> {
    Some(match name.to_ascii_lowercase().as_str() {
        "pwd" | "powerworld-pwd" | "powerworld-display" => DisplayFormat::PowerWorld,
        "geojson" | "geo-json" | "geo" => DisplayFormat::GeoJson,
        _ => return None,
    })
}

/// Map a format name (with the common aliases) to a [`TargetFormat`], or `None`
/// if unrecognized. Accepts `matpower`/`m`, `powermodels-json`/`powermodels`/`pm`,
/// `egret-json`/`egret`, `pandapower-json`/`pandapower`/`pp`, `psse`/`raw`,
/// `powerworld`/`aux`, `pslf`/`epc`, `goc3-json`/`goc3`, and
/// `surge-json`/`surge`, and `opfdata-json`/`opfdata`/`gridopt`.
/// Case-insensitive. The one place the bindings (Python, C ABI) share, so a new
/// text format means one new arm here, not three. PyPSA CSV folders, GridFM
/// datasets, and PowerWorld `.pwb` are directory or read only inputs with no
/// text target; they are routed by [`crate::format::routing`].
///
/// [`SourceFormat`]'s reported token is [`SourceFormat::name`], which resolves
/// here directly, so `net.to_format(other.source_format)` works for every
/// format. The `powermodelsjson`/`egretjson`/`pandapowerjson` aliases keep the
/// pre-0.9 camel-case spellings (`"PowerModelsJson"` lowercased) resolving for
/// callers that stored them.
#[must_use]
pub fn target_format_from_name(name: &str) -> Option<TargetFormat> {
    Some(match routing::transmission_format_from_name(name)? {
        TransmissionFormat::Matpower => TargetFormat::Matpower,
        TransmissionFormat::PowerModelsJson => TargetFormat::PowerModelsJson,
        TransmissionFormat::EgretJson => TargetFormat::EgretJson,
        TransmissionFormat::Psse => TargetFormat::Psse { rev: 33 },
        TransmissionFormat::Psse34 => TargetFormat::Psse { rev: 34 },
        TransmissionFormat::Psse35 => TargetFormat::Psse { rev: 35 },
        TransmissionFormat::PowerWorld => TargetFormat::PowerWorld,
        TransmissionFormat::PandapowerJson => TargetFormat::PandapowerJson,
        TransmissionFormat::Pslf => TargetFormat::Pslf,
        TransmissionFormat::Goc3Json => TargetFormat::Goc3Json,
        TransmissionFormat::SurgeJson => TargetFormat::SurgeJson,
        TransmissionFormat::DeepMindOpfDataJson => TargetFormat::DeepMindOpfDataJson,
        TransmissionFormat::PypsaCsv | TransmissionFormat::Pwb | TransmissionFormat::Gridfm => {
            return None;
        }
    })
}

/// Output of a display parse. PowerWorld `.pwd` produces
/// [`DisplayData::PowerWorld`]; a geographic sidecar produces
/// [`DisplayData::Geo`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum DisplayData {
    /// PowerWorld oneline display data.
    PowerWorld(PwdDisplay),
    /// A standalone geographic layer.
    Geo(crate::geo::GeoLayer),
}

impl DisplayData {
    /// The display format represented by this value.
    #[must_use]
    pub fn format(&self) -> DisplayFormat {
        match self {
            DisplayData::PowerWorld(_) => DisplayFormat::PowerWorld,
            DisplayData::Geo(_) => DisplayFormat::GeoJson,
        }
    }
}

fn display_file_guidance() -> Error {
    Error::UnknownFormat(
        "a PowerWorld .pwd is display data, not a BalancedNetwork case; \
         use parse_display_file(path, None)"
            .into(),
    )
}

/// Parse display bytes in the named display format `from`.
///
/// # Errors
/// [`Error::UnknownFormat`] if `from` is not a display format; otherwise the
/// reader's own [`Error`] on malformed input.
pub fn parse_display_bytes(bytes: &[u8], from: &str) -> Result<DisplayData> {
    let fmt =
        display_format_from_name(from).ok_or_else(|| Error::UnknownFormat(from.to_string()))?;
    match fmt {
        DisplayFormat::PowerWorld => Ok(DisplayData::PowerWorld(powerworld::parse_pwd_display(
            bytes,
        )?)),
        // The tolerant reader's own notes are available through
        // `GeoLayer::parse_bytes` for callers that want them.
        DisplayFormat::GeoJson => Ok(DisplayData::Geo(
            crate::geo::GeoLayer::parse_bytes(bytes, None)?.layer,
        )),
    }
}

/// Render a file extension for a user-facing message: `` extension `xyz` ``
/// when present, `no extension` otherwise.
fn describe_extension(extension: Option<&str>) -> String {
    match extension {
        Some(ext) => format!("extension `{ext}`"),
        None => "no extension".to_owned(),
    }
}

/// Parse the display file at `path`, choosing the reader from `from` or, when
/// `None`, from the extension. A `.pwd` extension selects PowerWorld display
/// data.
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
            Some("geojson") => DisplayFormat::GeoJson,
            // `.geo.json` is the canonical layer name; a bare `.json` stays
            // ambiguous (it is usually a case file).
            Some("json")
                if path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.to_ascii_lowercase()
                            .ends_with(crate::geo::GEO_LAYER_EXTENSION)
                    }) =>
            {
                DisplayFormat::GeoJson
            }
            other => {
                return Err(Error::UnknownFormat(format!(
                    "cannot infer display format from file with {}; \
                     pass an explicit display format",
                    describe_extension(other)
                )));
            }
        },
    };
    let bytes = read_file_bytes(path)?;
    match fmt {
        DisplayFormat::PowerWorld => Ok(DisplayData::PowerWorld(powerworld::parse_pwd_display(
            &bytes,
        )?)),
        DisplayFormat::GeoJson => Ok(DisplayData::Geo(
            crate::geo::GeoLayer::parse_bytes(&bytes, path.file_name().and_then(|n| n.to_str()))?
                .layer,
        )),
    }
}

/// An I/O failure naming the path it happened on. The bare OS message ("No
/// such file or directory") reaches callers who cannot see which path the
/// library resolved, so every read here names the file.
pub(crate) fn named_io_error(path: &std::path::Path, e: &std::io::Error) -> Error {
    Error::Io(std::io::Error::new(
        e.kind(),
        format!("cannot read {}: {e}", path.display()),
    ))
}

fn read_file_bytes(path: &std::path::Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|e| named_io_error(path, &e))
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

/// Whether a source format name means PSLF EPC.
fn is_pslf_name(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().replace(['-', '_'], "").as_str(),
        "pslf" | "epc" | "pslfepc"
    )
}

/// Parse the case file at `path`, choosing the reader from `from` (the
/// [`target_format_from_name`] names plus `pypsa-csv`/`pypsa`, `pwb`, `pslf`,
/// and `epc`) or, when `None`, from the path: a directory containing
/// `network.csv` parses as a PyPSA CSV folder (any other directory is refused
/// as a directory with [`Error::UnknownFormat`], before extension inference),
/// and a file maps by extension (`m`/`json`/`raw`/`aux`/`pwb`/`epc`),
/// case insensitively (issue #97: `.RAW` is as common as `.raw` in the wild). A
/// `.json` file is classified by top level shape markers: pandapower
/// (`"_class": "pandapowerNet"`), egret (`elements` and `system`), GO Challenge
/// 3 (`network` plus `time_series_input`/`reliability`), Surge JSON
/// (`format: "surge-json"`), OPFData (`grid`, `solution`, and `metadata`), and
/// PowerModels JSON (`baseMVA`, `branch`, `gen`, or `gencost`). JSON matching
/// model JSON markers (`buses` plus a network key), distribution markers,
/// ambiguous markers, or no known markers returns [`Error::UnknownFormat`].
/// Declare a format on the source to force a parser. PowerWorld `.pwb` is a
/// binary read only format; PSLF `.epc` is text and has a writer. Returns the
/// typed module: the network value, the reader's findings, and the retained
/// source.
///
/// The one parser the CLI and the Python/C/Julia bindings share, so adding a
/// source format is one edit here, not one per binding.
///
/// # Errors
/// A `Request` failure when the format cannot be determined or is refused, an
/// `Io` failure when acquisition fails, and the reader's own failure on
/// malformed input. Findings collected before a failure ride the returned
/// error.
///
pub fn parse(
    source: powerio_core::Source,
) -> std::result::Result<PioModule<BalancedNetwork>, powerio_core::Error> {
    let mut warnings = Diagnostics::new();
    match parse_to_network(&source, &mut warnings) {
        Ok(network) => {
            let mut module = PioModule::new(network);
            for buffer in source.acquired_buffers() {
                let descriptor = match SourceDescriptor::new(
                    buffer.id().clone(),
                    buffer.name(),
                    buffer.bytes().len() as u64,
                ) {
                    Ok(descriptor) => descriptor,
                    Err(error) => return Err(error.with_source(source)),
                };
                if let Err(error) = module.add_source_descriptor(descriptor) {
                    return Err(error.with_source(source));
                }
            }
            let mut module = module.with_source(source);
            for record in warnings.into_records() {
                module.add_diagnostic(record)?;
            }
            Ok(module)
        }
        Err(error) => {
            let core = powerio_core::Error::new(error.code(), error.to_string());
            Err(core
                .with_diagnostics(warnings.into_records())
                .with_cause(error)
                .with_source(source))
        }
    }
}

/// The format dispatch behind [`parse`]: name and content detection, then the
/// one reader map.
fn parse_to_network(
    source: &powerio_core::Source,
    warnings: &mut Diagnostics,
) -> Result<BalancedNetwork> {
    let from = source.format().map(powerio_core::FormatId::as_str);
    let path = std::path::Path::new(source.name());
    // The file stem is the name hint for formats that don't carry their own
    // name. An angle bracketed source name is the conventional non-file
    // spelling an anonymous in-memory caller uses and carries no hint; a name
    // with an extension contributes its stem, and any other name is the hint
    // itself.
    let stem = if source.name().starts_with('<') {
        None
    } else if path.extension().is_some() {
        path.file_stem().and_then(|stem| stem.to_str())
    } else {
        Some(source.name())
    };
    // PyPSA CSV folders are directories, not files; dispatch them before any
    // extension logic. `from` accepts the pypsa aliases, and a bare directory
    // source with a `network.csv` auto-detects.
    if source.is_directory() {
        let marker = powerio_core::ArtifactPath::new("network.csv")
            .expect("static name is a valid artifact path");
        if from.is_some_and(is_pypsa_csv_name) || (from.is_none() && source.buffer(&marker).is_ok())
        {
            return pypsa::read_pypsa_csv_source(source, warnings);
        }
        // Any other directory has no reader; refuse it as a directory before
        // the extension logic reads ".07" off a name like `pglib-opf-23.07`.
        return Err(Error::UnknownFormat(format!(
            "{} is a directory, and the only directory case format is a PyPSA CSV \
             folder (one holding a network.csv); pass a case file",
            path.display()
        )));
    }
    if from.is_some_and(is_pypsa_csv_name) {
        return Err(Error::UnknownFormat(
            "a PyPSA CSV case is a directory holding a network.csv; open the folder as the source"
                .into(),
        ));
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
        // Binary input: the exact bytes go to the reader, byte order mark
        // handling included, since the mark is a text concept.
        let buffer = primary(source)?;
        return powerworld::parse_pwb_collecting(buffer.bytes(), stem, warnings);
    }
    if from.is_some_and(is_pslf_name) || (from.is_none() && ext.as_deref() == Some("epc")) {
        let buffer = primary(source)?;
        let network = pslf::parse_pslf_source(source_text(&buffer)?, stem, warnings)?;
        reject_empty_case(&network, "PSLF .epc")?;
        return Ok(network);
    }
    if from
        .and_then(target_format_from_name)
        .is_some_and(|format| format == TargetFormat::DeepMindOpfDataJson)
        && matches!(ext.as_deref(), Some("pt" | "gz"))
    {
        return Err(Error::UnknownFormat(
            "OPFData .pt tensor caches and .tar.gz archives are not case files; extract and parse an example_N.json source file"
                .into(),
        ));
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
            Some(target_format_from_name(f).ok_or_else(|| unknown_source_format(f))?)
        }
        None => {
            // Everything but `.json` (sniffed below) resolves without the text.
            match ext.as_deref() {
                Some("m") => Some(TargetFormat::Matpower),
                Some("raw") => Some(TargetFormat::Psse { rev: 33 }),
                Some("aux") => Some(TargetFormat::PowerWorld),
                Some("json") => None,
                Some("dss") => return Err(unknown_source_format("dss")),
                other => {
                    // A nameless or oddly named source can still carry a JSON
                    // document (in-memory text has no extension to state);
                    // sniff it like a `.json` before refusing. The primary
                    // buffer is already retained, so peeking is free.
                    let jsonish = source.primary_buffer().is_ok_and(|buffer| {
                        source_text(&buffer)
                            .is_ok_and(|text| text.trim_start().starts_with(['{', '[']))
                    });
                    if jsonish {
                        None
                    } else {
                        return Err(Error::UnknownFormat(format!(
                            "cannot infer from source name with {}; \
                             declare a source format",
                            describe_extension(other)
                        )));
                    }
                }
            }
        }
    };
    // The parser decodes a byte order mark free slice of the one retained
    // buffer; the module keeps the exact original bytes for same format
    // writing. Sniffing a `.json` borrows the same slice.
    let buffer = primary(source)?;
    let text = source_text(&buffer)?;
    let fmt = match fmt_hint {
        Some(fmt) => fmt,
        None => match routing::classify_json_text(text) {
            // The network serialization is not a case format, but it parses:
            // a bare model JSON document decodes through `from_json` and
            // routes to `BalancedNetwork` like any other balanced source.
            JsonClass::ModelJson => return BalancedNetwork::from_json(text),
            class => json_target_from_class(class)?,
        },
    };
    read_source(text, fmt, stem, warnings)
}

/// The primary buffer of a file or memory source.
fn primary(source: &powerio_core::Source) -> Result<powerio_core::SourceBuffer> {
    source.primary_buffer().map_err(|error| Error::FormatRead {
        format: "source",
        message: error.to_string(),
    })
}

/// The text a reader decodes: the buffer's byte order mark free slice,
/// validated as UTF-8.
fn source_text(buffer: &powerio_core::SourceBuffer) -> Result<&str> {
    std::str::from_utf8(buffer.content_bytes()).map_err(|e| Error::FormatRead {
        format: "case text",
        message: format!("not valid UTF-8: {e}"),
    })
}

/// Read decoded `text` as `fmt`, using `name_hint` (e.g. the file stem) when
/// the format carries no name of its own. The single format to reader map:
/// every parse route funnels through it, so every format is dispatched the
/// same way. Readers borrow the text; the module retains the source bytes.
fn read_source(
    text: &str,
    fmt: TargetFormat,
    name_hint: Option<&str>,
    warnings: &mut Diagnostics,
) -> Result<BalancedNetwork> {
    let net = match fmt {
        TargetFormat::Matpower => matpower::parse_matpower_source(text, name_hint),
        TargetFormat::PowerModelsJson => {
            powermodels::parse_powermodels_json_source(text, name_hint, warnings)
        }
        TargetFormat::Psse { .. } => psse::parse_psse_source(text, name_hint, warnings),
        TargetFormat::PowerWorld => {
            powerworld::map::parse_powerworld_source(text, name_hint, warnings)
        }
        TargetFormat::EgretJson => egret::parse_egret_source(text, name_hint),
        TargetFormat::PandapowerJson => {
            pandapower::parse_pandapower_source(text, name_hint, warnings)
        }
        // PSLF read normally enters through the `is_pslf_name`/`.epc` fast
        // path in the dispatch; this arm keeps the funnel total.
        TargetFormat::Pslf => pslf::parse_pslf_source(text, name_hint, warnings),
        // The general parse takes the network and drops the typed problem
        // document; the calculation instance this source declares arrives
        // with the instance types.
        TargetFormat::Goc3Json => {
            goc3::parse_goc3_source(text, name_hint, warnings).map(|(net, _goc3)| net)
        }
        TargetFormat::SurgeJson => surge::parse_surge_source(text, name_hint, warnings),
        TargetFormat::DeepMindOpfDataJson => {
            opfdata::parse_opfdata_source(text, name_hint, warnings)
        }
    }?;
    reject_empty_case(&net, fmt.label())?;
    Ok(net)
}

/// Geographic metadata for a reader that harvested longitude/latitude
/// coordinates: `Some` once any bus carries a location, so a case without
/// coordinates serializes exactly as before. The space is stamped geographic
/// only when every point fits longitude/latitude bounds; a source that
/// violates its format's own convention (projected meters in a pandapower
/// `geo` column) reads as unknown instead of claiming WGS84.
pub(crate) fn geographic_meta(buses: &[Bus]) -> Option<crate::geo::GeoMeta> {
    let mut located = buses.iter().filter_map(|bus| bus.location).peekable();
    located.peek()?;
    let in_bounds = located.all(|location| location.x.abs() <= 180.0 && location.y.abs() <= 90.0);
    Some(crate::geo::GeoMeta {
        space: if in_bounds {
            crate::geo::CoordinateSpace::Geographic { crs: None }
        } else {
            crate::geo::CoordinateSpace::Unknown
        },
        kind: None,
    })
}

/// A source id from an f64: an in range value truncates the way the readers
/// always have; a negative, non-finite, or over-ceiling value is refused with
/// a message naming `column`, instead of letting the `as usize` cast saturate.
/// The ceiling is [`crate::network::BusId::MAX`] (`i64::MAX`, the C ABI id
/// bound), applied to every id column so a non-bus id gets the same policy.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub(crate) fn id_from_f64(
    value: f64,
    column: impl std::fmt::Display,
) -> std::result::Result<usize, String> {
    // Strict `<`: `i64::MAX as f64` rounds up to 2^63, so `<=` would admit
    // values the cast saturates past `BusId::MAX`.
    if value >= 0.0 && value < i64::MAX as f64 {
        Ok(value as usize)
    } else {
        // Debug keeps the shortest float form ("1e300", never 301 digits).
        Err(format!(
            "`{column}` value {value:?} is outside the id range 0..2^63"
        ))
    }
}

/// A case with no buses is content-free for every consumer. Most readers
/// already reject it on a missing required table, but a JSON carrying only
/// `baseMVA` would otherwise parse to a hollow network; reject it in the
/// [`read_source`] funnel so every parse path (file and in-memory) is guarded,
/// and in the PyPSA folder reader, which bypasses the funnel.
pub(crate) fn reject_empty_case(net: &BalancedNetwork, format: &'static str) -> Result<()> {
    if net.buses().is_empty() {
        return Err(Error::FormatRead {
            format,
            message: "case has no buses".into(),
        });
    }
    Ok(())
}

/// The source format names [`parse`] accepts as a declared format, each with
/// its aliases. The unknown format error prints this list, and a test walks
/// every alias through [`routing::transmission_format_from_name`] so it
/// cannot drift from the matcher. `pypsa-csv` names a directory source and
/// `pwb` a binary one; every other name reads file and memory sources alike.
pub const SOURCE_FORMAT_NAMES: &str = "matpower/m, powermodels-json/powermodels/pm, \
     egret-json/egret, psse/raw, psse34, psse35, powerworld/aux, \
     pandapower-json/pandapower/pp, pslf/epc, pypsa-csv/pypsa, pwb, goc3-json/goc3, \
     surge-json/surge, opfdata-json/opfdata/gridopt";

/// An unrecognized source format token. When the token names a distribution
/// format (`dss`, `pmd`, `bmopf`), the error points at the distribution
/// surface instead of echoing the token: this parser reads only balanced
/// transmission formats. Otherwise the refusal enumerates the accepted names.
fn unknown_source_format(name: &str) -> Error {
    if name.eq_ignore_ascii_case("powerio-json") {
        return Error::UnknownFormat(
            "the `powerio-json` token was retired in 0.9.0: model JSON is not a case \
             format or a conversion target; write it with `to_json` \
             (`pio_balanced_network_to_json` in C, `json_format model-json` on the MCP \
             server), store the case as `.pio.json`, and classify a JSON document with \
             `classify_json_text` (family `model-json`)"
                .into(),
        );
    }
    if let Some(dist) = routing::distribution_format_from_name(name) {
        return Error::UnknownFormat(format!(
            "`{}` is a distribution format, and this parser reads only balanced \
             transmission formats; parse it through the one module family \
             (powerio::parse in Rust, pio_parse_file in C, powerio.parse in Python, \
             parse_file in Julia), which routes distribution formats",
            dist.name()
        ));
    }
    Error::UnknownFormat(format!("{name}; accepted names: {SOURCE_FORMAT_NAMES}"))
}

/// The JSON formats share the `.json` extension, so an explicit source format
/// isn't always given. Classification lives here so the CLI and bindings use
/// the same top level markers as the Rust parsers.
#[cfg(test)]
fn sniff_json(text: &str) -> Result<TargetFormat> {
    json_target_from_class(routing::classify_json_text(text))
}

/// The case format a JSON classification selects; the shapes that are not
/// case formats are refused with the surface that reads them named. Model
/// JSON never reaches this from `parse`, which decodes it directly.
fn json_target_from_class(class: JsonClass) -> Result<TargetFormat> {
    match class {
        JsonClass::Module => Err(Error::UnknownFormat(
            "JSON is a .pio.json stored module; read it with the module surface \
             (powerio::parse in Rust, pio_parse_str in C, powerio.parse in \
             Python, parse_bytes in Julia)"
                .into(),
        )),
        JsonClass::ModelJson => Err(Error::UnknownFormat(
            "JSON is bare powerio model JSON, which is not a case format; read it with \
             BalancedNetwork::from_json (pio_balanced_network_from_json in C, \
             powerio.from_json in Python, from_json in Julia)"
                .into(),
        )),
        JsonClass::Case(Detection::Known(DetectedFormat::Transmission(format))) => {
            transmission_json_target(format)
        }
        JsonClass::Case(Detection::Known(DetectedFormat::Distribution(format))) => {
            Err(Error::UnknownFormat(format!(
                "JSON looks like distribution `{}`; use the distribution parser or pass an explicit transmission format",
                format.name()
            )))
        }
        JsonClass::Case(Detection::Ambiguous) => Err(Error::UnknownFormat(
            "ambiguous JSON markers; pass an explicit source format".into(),
        )),
        JsonClass::Case(Detection::Unknown) => Err(Error::UnknownFormat(
            "cannot infer JSON format; pass an explicit source format".into(),
        )),
    }
}

fn transmission_json_target(format: TransmissionFormat) -> Result<TargetFormat> {
    match format {
        TransmissionFormat::PowerModelsJson => Ok(TargetFormat::PowerModelsJson),
        TransmissionFormat::EgretJson => Ok(TargetFormat::EgretJson),
        TransmissionFormat::PandapowerJson => Ok(TargetFormat::PandapowerJson),
        TransmissionFormat::Goc3Json => Ok(TargetFormat::Goc3Json),
        TransmissionFormat::SurgeJson => Ok(TargetFormat::SurgeJson),
        TransmissionFormat::DeepMindOpfDataJson => Ok(TargetFormat::DeepMindOpfDataJson),
        other => Err(Error::UnknownFormat(format!(
            "JSON classifier returned non-JSON transmission format `{}`",
            other.name()
        ))),
    }
}

/// Output of a conversion: the serialized text plus the fidelity findings:
/// data the target can't represent, defaults synthesized, or blocks mapped
/// best effort. Empty `diagnostics` means a faithful conversion. For
/// [`convert_file`] and [`convert_str`], `diagnostics` carries the read side
/// findings ahead of the write side. Warning is one diagnostic severity;
/// rendered text lines come from [`crate::diagnostics::render_diagnostics`].
///
/// `#[non_exhaustive]`: a returns-only type, so downstream code reads it but
/// never constructs it, leaving room to add fidelity metadata without a breaking
/// change.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Conversion {
    pub text: String,
    /// The findings as structured records: a stable code, a severity, and a
    /// message.
    pub diagnostics: Vec<Diagnostic>,
}

impl Conversion {
    pub(crate) fn new(text: String, diagnostics: Diagnostics) -> Self {
        Self {
            text,
            diagnostics: diagnostics.into_records(),
        }
    }

    /// A conversion that dropped nothing, e.g. a same-format echo.
    pub(crate) fn faithful(text: String) -> Self {
        Self::new(text, Diagnostics::new())
    }

    /// The findings as `CODE: message` lines, rendered on request. Warning is
    /// one diagnostic severity; there is no separately stored text channel.
    #[must_use]
    pub fn rendered_diagnostics(&self) -> Vec<String> {
        crate::diagnostics::render_diagnostics(&self.diagnostics)
    }

    /// Record one finding after the writer has run.
    pub(crate) fn push(&mut self, info: &'static DiagnosticInfo, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic::of(info, message));
    }

    /// Put the read side's findings ahead of the write side's.
    pub(crate) fn prepend(&mut self, read: Vec<Diagnostic>) {
        let mut records = read;
        records.append(&mut self.diagnostics);
        self.diagnostics = records;
    }
}

/// Optional write-time policies layered on top of the neutral [`BalancedNetwork`].
///
/// The default is a no-op and preserves the old `write_as` / `convert_*`
/// behavior. Non-default options work on a cloned network and never mutate the
/// caller's case.
#[derive(Debug, Clone, Default)]
pub struct WriteOptions {
    pub missing_gen_cost: MissingGenCostPolicy,
    pub gen_cost_patches: Vec<GenCostPatch>,
}

impl WriteOptions {
    #[must_use]
    pub fn is_default(&self) -> bool {
        self.missing_gen_cost.is_preserve() && self.gen_cost_patches.is_empty()
    }
}

/// Write a parsed module to `format`. Writing back to the source format of an
/// unchanged parsed module returns the retained source bytes exactly,
/// including a byte order mark; any other target serializes the typed value.
///
/// # Errors
/// [`Error::WriteUnsupported`] for a read only target, and the writer's own
/// [`Error`] on a case it cannot state.
pub fn write_as(
    module: &PioModule<BalancedNetwork>,
    format: TargetFormat,
) -> std::result::Result<Conversion, powerio_core::Error> {
    if let Some(text) = echo_text(module, format) {
        return Ok(Conversion::faithful(text));
    }
    let mut conv = write_conversion(module.value(), format).map_err(core_error)?;
    warn_psse_downgrade(module, format, &mut conv);
    Ok(conv)
}

/// Project a crate failure onto the common operation failure type.
pub(crate) fn core_error(error: Error) -> powerio_core::Error {
    let message = error.to_string();
    powerio_core::Error::new(error.code(), message).with_cause(error)
}

/// The retained source text when writing `module` back to its source format:
/// the echo that reproduces the input byte for byte. `None` sends the write
/// down the semantic path.
fn echo_text(module: &PioModule<BalancedNetwork>, target: TargetFormat) -> Option<String> {
    let source = module.source()?;
    let buffer = source.primary_buffer().ok()?;
    if !same_format(target, module.value().source_format()) {
        return None;
    }
    let text = std::str::from_utf8(buffer.bytes()).ok()?;
    // A PSS/E source echoes only when the requested revision equals the
    // source's own; any other revision goes through write_psse_rev so the
    // caller gets the layout it asked for instead of the original bytes.
    if let TargetFormat::Psse { rev } = target
        && psse::header_rev(text.trim_start_matches('\u{feff}')) != rev
    {
        return None;
    }
    Some(text.to_owned())
}

/// Serialize a typed network to `format` with no source echo: the semantic
/// write used for values constructed in memory or severed from their module.
///
/// # Errors
/// As [`write_as`].
pub fn write_network(
    net: &BalancedNetwork,
    format: TargetFormat,
) -> std::result::Result<Conversion, powerio_core::Error> {
    write_conversion(net, format).map_err(core_error)
}

pub(crate) fn write_conversion(net: &BalancedNetwork, format: TargetFormat) -> Result<Conversion> {
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
        TargetFormat::SurgeJson => write_surge_json(net),
        TargetFormat::Goc3Json => {
            return Err(Error::WriteUnsupported {
                format: "goc3-json",
            });
        }
        TargetFormat::DeepMindOpfDataJson => {
            return Err(Error::WriteUnsupported {
                format: "opfdata-json",
            });
        }
    };
    warn_normalized_tap(net, format, &mut conv);
    warn_missing_reference(net, format, &mut conv);
    warn_dropped_frequency(net, format, &mut conv);
    warn_dropped_locations(net, format, &mut conv);
    warn_dropped_transformer_charging(net, format, &mut conv);
    Ok(conv)
}

/// Write a parsed module to `format` through a destination: the one write
/// operation over file, memory, and (for the directory formats) folder
/// output. Every text target commits a single artifact — a path destination
/// names the exact file, a memory destination names the artifact — staged
/// and renamed into place so a failed write never exposes a partial target.
/// The result carries the complete artifact inventory and the writer's
/// findings. PyPSA CSV folders write through [`write_pypsa_csv`].
///
/// # Errors
/// As [`write_as`], plus the destination's own collision and staging
/// failures.
pub fn write(
    module: &PioModule<BalancedNetwork>,
    format: TargetFormat,
    destination: powerio_core::Destination,
) -> std::result::Result<powerio_core::WriteResult, powerio_core::Error> {
    write_with_options(module, format, &WriteOptions::default(), destination)
}

/// [`write()`] with write-time cost policies, as [`write_as_with_options`].
///
/// # Errors
/// As [`write()`].
///
/// # Panics
/// Never on external input: the fixed artifact name is valid by
/// construction.
pub fn write_with_options(
    module: &PioModule<BalancedNetwork>,
    format: TargetFormat,
    options: &WriteOptions,
    destination: powerio_core::Destination,
) -> std::result::Result<powerio_core::WriteResult, powerio_core::Error> {
    let conv = write_as_with_options(module, format, options)?;
    let artifact = powerio_core::MemoryArtifact::new(
        powerio_core::ArtifactPath::new("case").expect("static name is a valid artifact path"),
        conv.text.into_bytes(),
    );
    destination.__commit_artifacts(false, vec![artifact], conv.diagnostics)
}

/// Write a parsed module as a PyPSA CSV folder through a destination: the
/// directory form of [`write()`]. Either destination names the output root and
/// every returned artifact sits below it; the whole inventory commits
/// atomically.
///
/// # Errors
/// The destination's collision and staging failures.
///
/// # Panics
/// Never on external input: the writer's fixed artifact names are valid by
/// construction.
pub fn write_pypsa_csv(
    module: &PioModule<BalancedNetwork>,
    destination: powerio_core::Destination,
) -> std::result::Result<powerio_core::WriteResult, powerio_core::Error> {
    let (artifacts, diagnostics) = pypsa::pypsa_csv_artifacts(module.value());
    let artifacts = artifacts
        .into_iter()
        .map(|(name, text)| {
            powerio_core::MemoryArtifact::new(
                powerio_core::ArtifactPath::new(name).expect("the writer emits fixed valid names"),
                text.into_bytes(),
            )
        })
        .collect();
    destination.__commit_artifacts(true, artifacts, diagnostics)
}

/// Write a parsed module with write-time cost policies. The plain
/// [`write_as`] behavior is preserved when `options` is default; a non-default
/// policy edits a copy of the typed value, so its write never echoes source
/// bytes the policy no longer matches.
pub fn write_as_with_options(
    module: &PioModule<BalancedNetwork>,
    format: TargetFormat,
    options: &WriteOptions,
) -> std::result::Result<Conversion, powerio_core::Error> {
    if options.is_default() {
        return write_as(module, format);
    }
    let (working, policy_warnings) =
        apply_write_cost_policy(module.value(), options).map_err(core_error)?;
    let mut conv = write_conversion(&working, format).map_err(core_error)?;
    conv.prepend(policy_warnings);
    Ok(conv)
}

/// Apply the write-time cost policy to a copy of `net` and report what it did.
///
/// Shared by the text and directory writers so both surfaces run one policy and
/// describe it with the same findings. The caller's network is never mutated.
pub(crate) fn apply_write_cost_policy(
    net: &BalancedNetwork,
    options: &WriteOptions,
) -> Result<(BalancedNetwork, Vec<Diagnostic>)> {
    let mut working = net.clone();
    let report =
        working.apply_gen_cost_policy(&options.gen_cost_patches, options.missing_gen_cost)?;
    let mut policy_warnings = Diagnostics::new();
    if report.patched > 0 {
        policy_warnings.push(
            &codes::TRANSFORM_GEN_COST_POLICY_APPLIED,
            format!(
                "generator cost patch applied to {} generator(s)",
                report.patched
            ),
        );
    }
    if report.synthesized > 0 {
        policy_warnings.push(
            &codes::TRANSFORM_GEN_COST_POLICY_APPLIED,
            match options.missing_gen_cost {
                MissingGenCostPolicy::Fill {
                    c2,
                    c1,
                    c0,
                    startup,
                    shutdown,
                } => format!(
                    "generator cost synthesized for {} generator(s): model 2, ncost 3, \
                 coeffs [{c2}, {c1}, {c0}], startup {startup}, shutdown {shutdown}",
                    report.synthesized
                ),
                _ => unreachable!("only Fill synthesizes costs"),
            },
        );
    }
    Ok((working, policy_warnings.into_records()))
}

/// Allocate a circuit id for an element keyed by `key` — a bus for loads/shunts,
/// or a `(from, to)` pair for branches: reuse the source-supplied `preferred` id
/// when it is still free on this key, else the lowest free positional id. Keeps
/// parallel devices distinct so the `(key, id)` uniqueness rule the PSS/E and
/// PSLF records require holds even when the source supplies colliding ids.
pub(super) fn allocate_circuit_id<K: Ord + Clone>(
    preferred: Option<&str>,
    key: K,
    used: &mut std::collections::BTreeMap<K, std::collections::BTreeSet<String>>,
) -> String {
    let taken = used.entry(key).or_default();
    if let Some(id) = preferred {
        if taken.insert(id.to_owned()) {
            return id.to_owned();
        }
    }
    let mut n = 1u32;
    loop {
        let candidate = n.to_string();
        if taken.insert(candidate.clone()) {
            return candidate;
        }
        n += 1;
    }
}

/// Warn when a PSS/E source is re-serialized at an older revision than its own.
/// `parse_file` maps every `.raw` to revision 33 and the `psse`/`raw` aliases
/// resolve to 33, so writing a v34/v35 source through the default target skips
/// the echo path (revisions differ) and re-emits the v33 layout, dropping the
/// modern records (12 named ratings, load DG/LOADTYPE columns, the system-wide
/// block) and any unmodeled section the echo would have preserved. Name the
/// downgrade instead of performing it silently.
fn warn_psse_downgrade(
    module: &PioModule<BalancedNetwork>,
    format: TargetFormat,
    conv: &mut Conversion,
) {
    let source_text = module
        .source()
        .and_then(|source| source.primary_buffer().ok())
        .and_then(|buffer| String::from_utf8(buffer.content_bytes().to_vec()).ok());
    if let (TargetFormat::Psse { rev }, SourceFormat::Psse, Some(src)) = (
        format,
        module.value().source_format(),
        source_text.as_deref(),
    ) {
        let src_rev = psse::header_rev(src);
        if src_rev > rev {
            conv.push(
                &codes::EMIT_PSSE_DOWNGRADED,
                format!(
                    "PSS/E source is revision {src_rev} but the write target is revision {rev}; \
                     the older layout drops fields the source carried (write to psse{src_rev} to keep them)"
                ),
            );
        }
    }
}

/// Warn when a non-default system frequency writes to a format with no frequency
/// field. PSS/E (`BASFRQ`) and pandapower (`f_hz`) carry it; MATPOWER,
/// PowerModels, egret, and PowerWorld have nowhere to put it, so a 50 Hz case
/// would silently read back as the 60 Hz default. Report the loss instead.
fn warn_dropped_frequency(net: &BalancedNetwork, format: TargetFormat, conv: &mut Conversion) {
    let carries_frequency = matches!(
        format,
        TargetFormat::Psse { .. } | TargetFormat::PandapowerJson
    );
    if carries_frequency {
        return;
    }
    if (net.base_frequency() - crate::network::DEFAULT_BASE_FREQUENCY).abs() > 1e-9 {
        conv.push(
            &format.emit_family().field_dropped,
            format!(
                "system base frequency {} Hz dropped: {} has no frequency field (reads back as {} Hz)",
                net.base_frequency(),
                format.label(),
                crate::network::DEFAULT_BASE_FREQUENCY
            ),
        );
    }
}

/// Warn when the case carries bus locations and the target has no geometry
/// concept. PowerWorld aux (`Latitude:1`/`Longitude:1`) and pandapower
/// (`geo`) carry them, and the PyPSA folder writer (`x`/`y`) has its own
/// path; MATPOWER, PSS/E, PowerModels, egret, PSLF, and Surge have nowhere to
/// put them, matching the `base_frequency` behavior. `powerio geo extract`
/// writes the sidecar as the escape hatch.
fn warn_dropped_locations(net: &BalancedNetwork, format: TargetFormat, conv: &mut Conversion) {
    let carries_locations = matches!(
        format,
        TargetFormat::PowerWorld | TargetFormat::PandapowerJson
    );
    if carries_locations {
        return;
    }
    let n = net.buses().iter().filter(|b| b.location.is_some()).count();
    let routed = net.branches().iter().filter(|b| b.route.is_some()).count();
    if n > 0 || routed > 0 {
        conv.push(
            &format.emit_family().field_dropped,
            format!(
                "{n} bus location(s) and {routed} branch route(s) dropped: {} has no \
                 coordinate field (write a .geo.json sidecar to keep them)",
                format.label()
            ),
        );
    }
}

/// Warn when a transformer carries line charging and the target's
/// transformer record has no susceptance column to hold it. The PSLF `.epc`
/// transformer record is the one such target; PSS/E writes representable
/// magnetizing admittance and the MATPOWER shaped writers keep the legacy total
/// projection on the branch row, so neither drops it.
fn warn_dropped_transformer_charging(
    net: &BalancedNetwork,
    format: TargetFormat,
    conv: &mut Conversion,
) {
    if !matches!(format, TargetFormat::Pslf) {
        return;
    }
    let n = net
        .branches()
        .iter()
        .filter(|b| b.is_transformer() && b.total_charging_b() != 0.0)
        .count();
    if n > 0 {
        conv.push(
            &codes::EMIT_PSLF.field_dropped,
            format!(
                "{n} transformer(s) carry line charging that the PSLF .epc transformer \
                 record cannot represent; the charging was dropped"
            ),
        );
    }
}

pub(super) fn branch_rating_set_drop_warning(
    target: &str,
    branch_index: usize,
    branch: &Branch,
    rating: &BranchRatingSet,
) -> String {
    format!(
        "branch {} ({} to {}) rating set {}={} MVA dropped: {} has no field for branch rating sets beyond rate_a, rate_b, and rate_c",
        branch_index + 1,
        branch.from,
        branch.to,
        rating.name,
        rating.rate_mva,
        target
    )
}

/// Warn once when elements carry passthrough extras `target`'s writer does not
/// replay. `consumed` is the writer's own rule: the keys it reads back into a
/// record. Everything else was retained by a reader because the source stated
/// more than a rewrite would synthesize, so dropping it without saying so is
/// an undeclared loss (#330). One line, a count and the reason, matching the
/// granularity of the other writer warnings.
pub(super) fn warn_dropped_extras(
    family: &'static EmitFamily,
    target: &str,
    net: &BalancedNetwork,
    consumed: impl Fn(&str) -> bool,
    warnings: &mut Diagnostics,
) {
    let carries = |extras: &crate::network::Extras| extras.keys().any(|k| !consumed(k));
    let dropped = net.buses().iter().filter(|e| carries(&e.extras)).count()
        + net.branches().iter().filter(|e| carries(&e.extras)).count()
        + net.loads().iter().filter(|e| carries(&e.extras)).count()
        + net.shunts().iter().filter(|e| carries(&e.extras)).count()
        + net.switches().iter().filter(|e| carries(&e.extras)).count()
        + net.storage().iter().filter(|e| carries(&e.extras)).count()
        + net.hvdc().iter().filter(|e| carries(&e.extras)).count()
        + net
            .transformers_3w()
            .iter()
            .filter(|e| carries(&e.extras))
            .count();
    if dropped > 0 {
        warnings.push(
            &family.extras_dropped,
            format!(
                "{dropped} element(s) carry source-format passthrough fields (extras) the {target} \
                 writer does not replay; dropped"
            ),
        );
    }
}

/// Warn when a writer drops the area table. Its own line rather than the
/// extras count: `areas` is a typed field, not a passthrough (#330).
pub(super) fn warn_dropped_areas(
    family: &'static EmitFamily,
    target: &str,
    net: &BalancedNetwork,
    warnings: &mut Diagnostics,
) {
    if !net.areas().is_empty() {
        warnings.push(
            &family.areas_dropped,
            format!(
                "{} area record(s) dropped: the {target} writer emits no area table",
                net.areas().len()
            ),
        );
    }
}

pub(super) fn warn_extra_branch_rating_sets(
    family: &'static EmitFamily,
    target: &str,
    net: &BalancedNetwork,
    warnings: &mut Diagnostics,
) {
    for (branch_index, branch) in net.branches().iter().enumerate() {
        for rating in &branch.rating_sets {
            warnings.push(
                &family.rating_set_dropped,
                branch_rating_set_drop_warning(target, branch_index, branch, rating),
            );
        }
    }
}

/// The declared format ID for a caller-supplied token. Tokens are matched
/// case insensitively and accept the historical underscore spelling of a
/// hyphenated alias; the ID itself keeps the stable lower case hyphen
/// grammar.
pub fn format_id_for(
    token: &str,
) -> std::result::Result<powerio_core::FormatId, powerio_core::Error> {
    powerio_core::FormatId::new(token.to_ascii_lowercase().replace('_', "-"))
}

/// Attach a caller-named source format to a source.
fn with_declared_format(
    source: powerio_core::Source,
    from: Option<&str>,
) -> std::result::Result<powerio_core::Source, powerio_core::Error> {
    match from {
        None => Ok(source),
        Some(token) => Ok(source.with_format(format_id_for(token)?)),
    }
}

/// Convert a case file to `to`, optionally forcing the source format with
/// `from`.
///
/// This is the canonical file-conversion helper shared by the bindings. It
/// parses `path` once, writes the parsed module to `to`, and returns the
/// converted text plus any fidelity findings, read side first. An echo
/// (writing back to the source format) returns the retained text with no
/// findings.
///
/// # Errors
/// As [`parse`].
pub fn convert_file(
    path: impl AsRef<std::path::Path>,
    to: TargetFormat,
    from: Option<&str>,
) -> std::result::Result<Conversion, powerio_core::Error> {
    let source = with_declared_format(powerio_core::Source::open(path.as_ref())?, from)?;
    convert_source(source, to, &WriteOptions::default())
}

/// Convert a case file with write-time cost policies.
pub fn convert_file_with_options(
    path: impl AsRef<std::path::Path>,
    to: TargetFormat,
    from: Option<&str>,
    options: &WriteOptions,
) -> std::result::Result<Conversion, powerio_core::Error> {
    let source = with_declared_format(powerio_core::Source::open(path.as_ref())?, from)?;
    convert_source(source, to, options)
}

/// Convert in-memory case `text` of the named source format `from` (see
/// [`target_format_from_name`]) to `to`.
///
/// Parses `text` once and writes the parsed module to `to` without a
/// temporary file. Findings are ordered read side first, as in
/// [`convert_file`].
///
/// # Errors
/// As [`parse`].
pub fn convert_str(
    text: &str,
    to: TargetFormat,
    from: &str,
) -> std::result::Result<Conversion, powerio_core::Error> {
    convert_str_with_options(text, to, from, &WriteOptions::default())
}

/// Convert in-memory case text with write-time cost policies.
pub fn convert_str_with_options(
    text: &str,
    to: TargetFormat,
    from: &str,
    options: &WriteOptions,
) -> std::result::Result<Conversion, powerio_core::Error> {
    let source = with_declared_format(
        powerio_core::Source::from_bytes("<memory>", text.as_bytes().to_vec())?,
        Some(from),
    )?;
    convert_source(source, to, options)
}

fn convert_source(
    source: powerio_core::Source,
    to: TargetFormat,
    options: &WriteOptions,
) -> std::result::Result<Conversion, powerio_core::Error> {
    let module = parse(source)?;
    let echoed = options.is_default() && echo_text(&module, to).is_some();
    let mut conv = write_as_with_options(&module, to, options)?;
    if !echoed {
        conv.prepend(module.diagnostics().to_vec());
    }
    Ok(conv)
}

/// Write `net` into `out_dir` as the named directory format. This function
/// dispatches directory format names for the bindings. PyPSA CSV
/// (`pypsa-csv`/`pypsa`) is the one such
/// format today; a text format name is rejected by name, pointing at
/// [`write_as`]. Returns the write's findings as structured records; render
/// them with `diagnostics::render_diagnostics` for a text channel.
///
/// # Errors
/// [`Error::UnknownFormat`] for a non-directory format name; the writer's own
/// [`Error`] otherwise.
pub fn write_dir(
    net: &BalancedNetwork,
    to: &str,
    out_dir: impl AsRef<std::path::Path>,
) -> std::result::Result<Vec<Diagnostic>, powerio_core::Error> {
    if is_pypsa_csv_name(to) {
        return write_pypsa_csv_folder(net, out_dir.as_ref()).map(|o| o.diagnostics);
    }
    Err(core_error(unknown_directory_format(to)))
}

fn unknown_directory_format(to: &str) -> Error {
    Error::UnknownFormat(format!(
        "{to} is not a directory format (directory targets: pypsa-csv/pypsa); \
         text formats serialize through write_as / to_format"
    ))
}

/// Write `net` into `out_dir` with write-time cost policies: the directory twin
/// of [`write_as_with_options`]. Default options are [`write_dir`] exactly.
/// The policy's own findings come back ahead of the writer's.
///
/// # Errors
/// As [`write_dir`], plus the cost policy's own [`Error`].
pub fn write_dir_with_options(
    net: &BalancedNetwork,
    to: &str,
    out_dir: impl AsRef<std::path::Path>,
    options: &WriteOptions,
) -> std::result::Result<Vec<Diagnostic>, powerio_core::Error> {
    // Refuse an unknown target before the policy runs, so a bad format name is
    // reported as one rather than as whatever the cost pass hits first.
    if !is_pypsa_csv_name(to) {
        return Err(core_error(unknown_directory_format(to)));
    }
    if options.is_default() {
        return write_dir(net, to, out_dir);
    }
    let (working, mut diagnostics) = apply_write_cost_policy(net, options).map_err(core_error)?;
    diagnostics.extend(write_dir(&working, to, out_dir)?);
    Ok(diagnostics)
}

/// Warn when a network with no reference (slack) bus converts to a format
/// whose solvers require one. PowerWorld `.pwb` is the one source that
/// systematically lacks the designation (the binary does not store it), so
/// the silent case would be common; `to_normalized` synthesizes a slack at
/// the largest pmax in service generator bus for consumers that need one.
fn warn_missing_reference(net: &BalancedNetwork, format: TargetFormat, conv: &mut Conversion) {
    let needs_ref = matches!(
        format,
        TargetFormat::Matpower
            | TargetFormat::Psse { .. }
            | TargetFormat::PowerModelsJson
            | TargetFormat::PandapowerJson
            | TargetFormat::Pslf
            | TargetFormat::SurgeJson
    );
    if needs_ref {
        if let Some(message) = missing_reference_warning(net) {
            conv.push(&format.emit_family().reference_missing, message);
        }
    }
}

/// The slackless-network warning itself, shared with the PyPSA folder writer
/// (which produces `PypsaCsvOutputs`, not a [`Conversion`], so it cannot go
/// through [`warn_missing_reference`]).
pub(super) fn missing_reference_warning(net: &BalancedNetwork) -> Option<String> {
    (!net.buses().iter().any(|b| b.kind == BusType::Ref)).then(|| {
        "no reference (slack) bus in the source network; power flow tools \
         reject such cases; to_normalized synthesizes a slack at the \
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
fn warn_normalized_tap(net: &BalancedNetwork, format: TargetFormat, conv: &mut Conversion) {
    if matches!(format, TargetFormat::Matpower) {
        return;
    }
    if let Some(message) = normalized_tap_warning(net) {
        conv.push(&format.emit_family().element_relabeled, message);
    }
}

/// The normalized-label warning itself, shared with the PyPSA folder writer.
// `tap == 1.0` / `shift == 0.0` are exact by construction (see
// `warn_normalized_tap`), so an epsilon compare would be wrong here.
#[allow(clippy::float_cmp)]
pub(super) fn normalized_tap_warning(net: &BalancedNetwork) -> Option<String> {
    if !net.is_normalized() {
        return None;
    }
    // After normalization a line (raw tap 0) and a unity-ratio transformer (raw
    // tap 1) both read as tap 1.0 / shift 0.0, so they cannot be told apart. Count
    // them together as the branches whose line/transformer label is now ambiguous.
    let ambiguous = net
        .branches()
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
/// name both interpolate a `BalancedNetwork` name straight into a quoted field, where an
/// embedded quote (or, for PSS/E, the `/` inline-comment delimiter) would shift
/// every later column of the record.
/// A line terminator is always replaced, whatever `forbidden` holds: no text
/// record format can carry one inside a field, so an embedded `\n` does not
/// shift a column, it ends the record and makes everything after it parse as
/// a new one. A crafted name could otherwise forge whole records in the
/// written file.
pub(crate) fn sanitize_quoted<'a>(
    value: &'a str,
    forbidden: &[char],
    replacement: char,
) -> std::borrow::Cow<'a, str> {
    let breaks = |c: char| c == '\n' || c == '\r' || forbidden.contains(&c);
    if value.contains(breaks) {
        value
            .chars()
            .map(|c| if breaks(c) { replacement } else { c })
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
            | (TargetFormat::Goc3Json, SourceFormat::Goc3Json)
            | (TargetFormat::SurgeJson, SourceFormat::SurgeJson)
            | (
                TargetFormat::DeepMindOpfDataJson,
                SourceFormat::DeepMindOpfDataJson,
            )
    )
}

/// JSON number for a finite `f64`; `Value::Null` for `NaN`/`±Inf`.
pub(crate) fn jnum(x: f64) -> Value {
    serde_json::Number::from_f64(x).map_or(Value::Null, Value::Number)
}

/// Serialize a built JSON tree into a [`Conversion`], appending one warning that
/// names every field where a non-finite `f64` was written as `null` (JSON has no
/// `±Inf`/`NaN`). Shared by the JSON writers.
pub(crate) fn finish(
    family: &'static EmitFamily,
    root: Map<String, Value>,
    mut warnings: Diagnostics,
) -> Conversion {
    let value = Value::Object(root);
    let mut nulls = BTreeSet::new();
    collect_null_keys(&value, &mut nulls);
    if !nulls.is_empty() {
        warnings.push(
            &family.not_a_number,
            format!(
                "non-finite numeric values written as JSON null in field(s): {}",
                nulls.into_iter().collect::<Vec<_>>().join(", ")
            ),
        );
    }
    let text = serde_json::to_string_pretty(&value).expect("a serde_json::Value always serializes");
    Conversion::new(text, warnings)
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

/// Test-only compatibility parse shapes; production code goes through
/// [`parse`] and the module type.
#[cfg(test)]
pub(crate) mod test_parse {
    use super::*;

    #[derive(Debug)]
    pub(crate) struct TestParsed {
        pub network: BalancedNetwork,
        pub diagnostics: Vec<Diagnostic>,
    }

    impl TestParsed {
        pub(crate) fn rendered_diagnostics(&self) -> Vec<String> {
            crate::diagnostics::render_diagnostics(&self.diagnostics)
        }
    }

    fn declared(
        source: powerio_core::Source,
        from: Option<&str>,
    ) -> std::result::Result<powerio_core::Source, powerio_core::Error> {
        match from {
            None => Ok(source),
            Some(token) => Ok(source.with_format(powerio_core::FormatId::new(
                token.to_ascii_lowercase().replace('_', "-"),
            )?)),
        }
    }

    pub(crate) fn parse_file(
        path: impl AsRef<std::path::Path>,
        from: Option<&str>,
    ) -> std::result::Result<TestParsed, powerio_core::Error> {
        let source = declared(powerio_core::Source::open(path.as_ref())?, from)?;
        parse(source).map(|module| TestParsed {
            diagnostics: module.diagnostics().to_vec(),
            network: module.into_value(),
        })
    }

    pub(crate) fn parse_str(
        text: &str,
        from: &str,
    ) -> std::result::Result<TestParsed, powerio_core::Error> {
        let source = declared(
            powerio_core::Source::from_bytes("<memory>", text.as_bytes().to_vec())?,
            Some(from),
        )?;
        parse(source).map(|module| TestParsed {
            diagnostics: module.diagnostics().to_vec(),
            network: module.into_value(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::test_parse::{parse_file, parse_str};
    use super::*;
    use crate::network::SourceFormat;

    #[test]
    fn sanitize_quoted_always_replaces_line_terminators() {
        // A terminator ends the record, so it is replaced whatever the
        // caller's delimiter set holds: a name carrying one could otherwise
        // forge whole records in a written .raw/.aux/.epc.
        for forbidden in [&[][..], &['\''][..], &['"'][..]] {
            let out = sanitize_quoted("A\n42, 'X'\r\nB", forbidden, ' ');
            assert!(
                !out.contains('\n') && !out.contains('\r'),
                "terminator survived with forbidden={forbidden:?}: {out:?}"
            );
        }
        // A clean value is still borrowed, not copied.
        assert!(matches!(
            sanitize_quoted("clean name", &['\''], ' '),
            std::borrow::Cow::Borrowed(_)
        ));
    }

    #[test]
    fn dss_extension_error_names_the_distribution_surface() {
        let path = std::env::temp_dir().join(format!(
            "powerio-dss-surface-{}-feeder.dss",
            std::process::id()
        ));
        std::fs::write(&path, "New Circuit.feeder\n").unwrap();
        let err = parse_file(&path, None).unwrap_err();
        let _ = std::fs::remove_file(&path);
        assert!(err.to_string().contains("distribution"), "got: {err}");
    }

    #[test]
    fn io_error_names_the_path() {
        let path =
            std::env::temp_dir().join(format!("powerio-no-such-case-{}.m", std::process::id()));
        let err = parse_file(&path, None).unwrap_err();
        assert_eq!(err.category(), powerio_core::ErrorCategory::Io);
        let msg = err.to_string();
        assert!(
            msg.contains(&path.display().to_string()),
            "the io failure must name the path: {msg}"
        );
    }

    #[test]
    fn a_directory_is_refused_as_a_directory() {
        // A versioned dataset directory: extension inference would read ".07"
        // off the name and misdiagnose the mistake as a format problem.
        let dir = std::env::temp_dir().join(format!("pglib-opf-23.07-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let err = parse_file(&dir, None).unwrap_err();
        std::fs::remove_dir_all(&dir).unwrap();
        let msg = err.to_string();
        assert!(msg.contains("is a directory"), "got: {msg}");
        assert!(msg.contains(&dir.display().to_string()), "got: {msg}");
        assert!(msg.contains("PyPSA CSV folder"), "got: {msg}");
    }

    #[test]
    fn unknown_format_error_lists_the_accepted_names() {
        let err = parse_str("anything", "not-a-format").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("not-a-format"), "got: {msg}");
        assert!(msg.contains("accepted names:"), "got: {msg}");
        assert!(msg.contains(SOURCE_FORMAT_NAMES), "got: {msg}");
    }

    #[test]
    fn the_accepted_name_list_matches_the_matcher() {
        use routing::TransmissionFormat as TF;
        // Every alias in the printed list resolves.
        let mut canonical = Vec::new();
        for clause in SOURCE_FORMAT_NAMES.split(", ") {
            for (i, alias) in clause.split('/').enumerate() {
                let resolved = routing::transmission_format_from_name(alias);
                assert!(
                    resolved.is_some(),
                    "listed alias `{alias}` does not resolve"
                );
                if i == 0 {
                    canonical.push(resolved.unwrap());
                }
            }
        }
        // Every parseable format is listed. Gridfm is the one matcher entry
        // with no parse_file arm (datasets go through the read_dir surface).
        for format in [
            TF::Matpower,
            TF::PowerModelsJson,
            TF::EgretJson,
            TF::Psse,
            TF::Psse34,
            TF::Psse35,
            TF::PowerWorld,
            TF::PandapowerJson,
            TF::PypsaCsv,
            TF::Pslf,
            TF::Pwb,
            TF::Goc3Json,
            TF::SurgeJson,
            TF::DeepMindOpfDataJson,
        ] {
            assert!(
                canonical.contains(&format),
                "{} is missing from SOURCE_FORMAT_NAMES",
                format.name()
            );
        }
    }

    #[test]
    fn a_case_with_generators_and_no_cost_data_warns() {
        let costless = "\
function mpc = nocost
mpc.version = '2';
mpc.baseMVA = 100;
mpc.bus = [
\t1\t3\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
\t2\t1\t50\t10\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
];
mpc.gen = [
\t1\t60\t0\t100\t-100\t1\t100\t1\t100\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0;
];
mpc.branch = [
\t1\t2\t0.01\t0.1\t0\t0\t0\t0\t0\t0\t1\t-360\t360;
];
";
        // The parse itself stays silent: whether a case carries costs is the
        // case's business, and a conversion leg must not count it. The
        // solver-ready copy is where a zero objective becomes real.
        let parsed = parse_str(costless, "matpower").unwrap();
        assert!(
            parsed.rendered_diagnostics().is_empty(),
            "{:?}",
            parsed.rendered_diagnostics()
        );
        let normalized = parsed
            .network
            .to_normalized_with_options(&crate::NormalizeOptions::default())
            .unwrap();
        let absent: Vec<_> = normalized
            .diagnostics
            .iter()
            .filter(|d| d.code() == "CANONICALIZE.NORMALIZE.GEN_COST_ABSENT")
            .collect();
        assert_eq!(absent.len(), 1, "{:?}", normalized.warnings);
        assert!(absent[0].message().contains("no cost data"), "{absent:?}");
        assert!(absent[0].message().contains("1 in-service"), "{absent:?}");

        // The same case with a gencost table is silent.
        let costed = format!("{costless}mpc.gencost = [\n\t2\t0\t0\t3\t0.01\t40\t0;\n];\n");
        let parsed = parse_str(&costed, "matpower").unwrap();
        let normalized = parsed
            .network
            .to_normalized_with_options(&crate::NormalizeOptions::default())
            .unwrap();
        assert!(
            normalized
                .diagnostics
                .iter()
                .all(|d| d.code() != "CANONICALIZE.NORMALIZE.GEN_COST_ABSENT"),
            "{:?}",
            normalized.warnings
        );
    }

    #[test]
    fn distribution_from_token_error_names_the_distribution_surface() {
        for token in ["dss", "pmd", "bmopf"] {
            let err = parse_str("anything", token).unwrap_err();
            assert!(
                err.to_string().contains("one module family"),
                "{token}: {err}"
            );
        }
        // A genuinely unknown token still echoes plainly.
        let err = parse_str("anything", "nonesuch").unwrap_err();
        assert!(err.to_string().contains("nonesuch"));
    }

    #[test]
    fn byte_order_mark_is_retained_and_echoed() {
        // Windows tooling saves case files with a UTF-8 byte order mark. The
        // parser decodes a mark free slice of the one retained buffer, and an
        // unchanged same format write reproduces the original bytes, mark
        // included.
        let case = "\u{feff}function mpc = t\n\
                    mpc.version = '2';\n\
                    mpc.baseMVA = 100;\n\
                    mpc.bus = [1 3 0 0 0 0 1 1.0 0 345 1 1.1 0.9;];\n\
                    mpc.gen = [];\n\
                    mpc.branch = [];\n";
        let source = powerio_core::Source::from_bytes("case.m", case.as_bytes().to_vec()).unwrap();
        let module = parse(source.with_format(format_id_for("matpower").unwrap())).unwrap();
        assert_eq!(module.value().buses().len(), 1);
        assert!(
            module.diagnostics().is_empty(),
            "{:?}",
            module.diagnostics()
        );
        let echo = write_as(&module, TargetFormat::Matpower).unwrap();
        assert_eq!(echo.text, case, "the echo reproduces the mark exactly");
    }

    #[test]
    fn canonical_format_bypasses_same_format_matpower_echo() {
        let case = "function mpc = t\n\
                    % a comment the canonical writer does not keep\n\
                    mpc.version = '2';\n\
                    mpc.baseMVA = 100;\n\
                    mpc.bus = [1 3 0 0 0 0 1 1.0 0 345 1 1.1 0.9;];\n\
                    mpc.gen = [];\n\
                    mpc.branch = [];\n";
        let source = powerio_core::Source::from_bytes("case.m", case.as_bytes().to_vec()).unwrap();
        let module =
            parse(source.with_format(powerio_core::FormatId::new("matpower").unwrap())).unwrap();
        assert_eq!(
            write_as(&module, TargetFormat::Matpower).unwrap().text,
            case
        );

        let net = module.into_value();
        let canonical = net.to_canonical_format(TargetFormat::Matpower).unwrap();
        assert_ne!(canonical.text, case);
        let reparsed = parse_str(&canonical.text, "matpower").unwrap();
        assert_eq!(reparsed.network.buses().len(), 1);
    }

    #[test]
    fn package_json_error_names_the_package_reader() {
        let err = sniff_json(r#"{"model_kind":"balanced","model":{}}"#).unwrap_err();
        assert!(err.to_string().contains(".pio.json"), "got: {err}");
    }

    #[test]
    fn source_format_strings_round_trip_to_a_target() {
        // The bindings expose `source_format` as its `name()` token, and
        // `to_format` routes that string back through `target_format_from_name`.
        // Every writable source format must resolve; the legacy `{:?}` spelling
        // (the pre-0.9 property value, issue #75) must keep resolving for
        // callers that stored it.
        for (sf, want) in [
            (SourceFormat::Matpower, TargetFormat::Matpower),
            (SourceFormat::PowerModelsJson, TargetFormat::PowerModelsJson),
            (SourceFormat::EgretJson, TargetFormat::EgretJson),
            (SourceFormat::Psse, TargetFormat::Psse { rev: 33 }),
            (SourceFormat::PowerWorld, TargetFormat::PowerWorld),
            (SourceFormat::PandapowerJson, TargetFormat::PandapowerJson),
            (SourceFormat::Pslf, TargetFormat::Pslf),
            (SourceFormat::Goc3Json, TargetFormat::Goc3Json),
            (SourceFormat::SurgeJson, TargetFormat::SurgeJson),
            (
                SourceFormat::DeepMindOpfDataJson,
                TargetFormat::DeepMindOpfDataJson,
            ),
        ] {
            let token = sf.name();
            assert_eq!(
                target_format_from_name(token),
                Some(want),
                "source_format {token:?} did not round-trip"
            );
            let legacy = format!("{sf:?}");
            assert_eq!(
                target_format_from_name(&legacy),
                Some(want),
                "legacy spelling {legacy:?} did not round-trip"
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
