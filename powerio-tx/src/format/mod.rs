//! Parsing and emission for supported case formats, all meeting at [`BalancedNetwork`].
//!
//! Each format module owns its parser and/or serializer: MATPOWER `.m`,
//! PowerModels JSON, PSS/E `.raw`, PowerWorld `.aux`, egret `ModelData` JSON,
//! pandapower JSON, PyPSA CSV folders, PSLF `.epc`, PSS/E RAWX 35, PowSybl
//! XIIDM and JIIDM 1.0 through 1.17, CIM CGMES 2.4.15 and 3.0, GO Challenge 3 JSON,
//! Surge JSON, and
//! DeepMind OPFData JSON. PowerWorld `.pwb` cases, OPFData JSON, and the
//! IEEE Common Data Format are input
//! only. GO Challenge 3 defines a calculation rather than a bare network, so
//! its implementation is private to the `powerio` facade's typed parser.
//! PowerWorld `.pwd` displays
//! use the display API. Case input and
//! output formats meet here, so adding a format that supports emission is one module plus
//! one hub registration.
//! [`parse`] compiles a retained source into a typed module, detecting the
//! format from the source name and content; [`parse_display`] parses
//! display artifacts such as PowerWorld `.pwd`. [`emit`] emits a parsed
//! module through a destination and echoes the retained source for a same
//! format target.
//! Non-finite numeric values, such as MATPOWER `Inf`/`NaN` angle limits, are
//! emitted as JSON `null`.
//!
//! # Fidelity behavior
//!
//! Emission has two fidelity tiers:
//!
//! - **Same format emission of an unchanged parsed module returns the original
//!   bytes.** The module retains its source, so [`emit`] back to the same
//!   format returns every field, comment, and numeric token.
//! - **Cross-format keeps maximal fidelity with itemized loss.** Whatever the
//!   target format cannot represent is reported by
//!   [`EmitResult::diagnostics`](powerio_core::EmitResult::diagnostics), never
//!   dropped silently. During parsing, parsers itemize what
//!   they ignore on the module's diagnostics.

use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::str::FromStr;

use serde_json::{Map, Value};

use powerio_core::PioModule;

use crate::diagnostics::{Diagnostic, DiagnosticInfo, Diagnostics, EmitFamily, codes};
use crate::gen_cost::{GenCostPatch, MissingGenCostPolicy};
use crate::network::{BalancedNetwork, Branch, BranchRatingSet, Bus, BusId, BusType, SourceFormat};
use crate::{Error, Result};
use routing::{Detection, JsonClass, SourceFormat as DetectedFormat, TransmissionFormat};

mod cgmes;
mod decode;
mod egret;
pub(crate) mod goc3;
mod ieee_cdf;
mod matpower;
mod opfdata;
mod pandapower;
mod powermodels;
pub mod powerworld;
mod pslf;
mod psse;
mod pypsa;
mod rawx;
pub mod routing;
mod surge;
mod ucte;
mod union_find;
mod xiidm;

pub use powerworld::{PwdDisplay, PwdSubstation};

#[doc(hidden)]
pub use egret::{
    egret_declares_time_series as __egret_declares_time_series,
    parse_egret_time_series as __parse_egret_time_series,
};
#[doc(hidden)]
pub use opfdata::{OpfDataSolution, parse_opfdata_json as __parse_opfdata_json};
#[doc(hidden)]
pub use pypsa::{
    PypsaAxis, PypsaCsvSequence, parse_pypsa_csv_time_series as __parse_pypsa_csv_time_series,
    pypsa_axis as __pypsa_axis,
};

pub use cgmes::CgmesVersion;
pub(crate) use egret::write_egret_json;
pub(crate) use pandapower::write_pandapower_json;
pub(crate) use powermodels::write_powermodels_json;
pub(crate) use powerworld::write_powerworld;
pub(crate) use pslf::write_pslf;
pub(crate) use psse::write_psse_rev;
pub(crate) use rawx::write_rawx;
pub(crate) use surge::write_surge_json;
pub(crate) use xiidm::{write_jiidm, write_xiidm};

/// A target case format. See [`emit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TargetFormat {
    /// PowerModels.jl network data JSON.
    PowerModelsJson,
    /// egret `ModelData` JSON.
    EgretJson,
    /// PSS/E `.raw` at the given revision. `rev` selects the record layout the
    /// serializer emits (33, 34, or 35); 33 is the historical default. The parser
    /// takes the revision from the file header, so this only affects emission.
    Psse { rev: u32 },
    /// PSS/E Extensible Power Flow Data File, revision 35 JSON.
    PsseRawx,
    /// PowerWorld auxiliary `.aux`.
    PowerWorld,
    /// pandapower `pandapowerNet` JSON.
    PandapowerJson,
    /// MATPOWER `.m` (round-trip; byte-exact when the case kept its source).
    Matpower,
    /// GE PSLF `.epc` (round-trip; byte-exact when the case kept its source).
    Pslf,
    /// DOE GO Challenge 3 JSON problem or solution data. The `powerio` facade
    /// owns its typed problem and solution handling; direct `powerio-tx`
    /// parsing refuses this calculation format.
    Goc3Json,
    /// Surge native JSON network document.
    SurgeJson,
    /// One JSON document from a DeepMind OPFData release. Read only except for
    /// an exact emission back to the retained source format.
    DeepMindOpfDataJson,
    /// PowSybl XIIDM XML, version 1.17.
    Xiidm,
    /// PowSybl JIIDM JSON, version 1.17.
    Jiidm,
    /// IEC CIM Common Grid Model Exchange Specification profile set.
    Cgmes,
    /// ENTSO-E UCTE-DEF `.uct`; fresh output uses revision 2007.05.01.
    Ucte,
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
            TargetFormat::PsseRawx => "rawx",
            TargetFormat::PowerWorld => "aux",
            TargetFormat::Matpower => "m",
            TargetFormat::Pslf => "epc",
            TargetFormat::Xiidm => "xiidm",
            TargetFormat::Jiidm => "jiidm",
            TargetFormat::Cgmes => "xml",
            TargetFormat::Ucte => "uct",
        }
    }

    /// Human-readable format name for diagnostics.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            TargetFormat::PowerModelsJson => "PowerModels JSON",
            TargetFormat::EgretJson => "egret JSON",
            TargetFormat::Psse { .. } => "PSS/E .raw",
            TargetFormat::PsseRawx => "PSS/E RAWX 35",
            TargetFormat::PowerWorld => "PowerWorld .aux",
            TargetFormat::PandapowerJson => "pandapower JSON",
            TargetFormat::Matpower => "MATPOWER .m",
            TargetFormat::Pslf => "PSLF .epc",
            TargetFormat::Goc3Json => "GO Challenge 3 JSON",
            TargetFormat::SurgeJson => "Surge JSON",
            TargetFormat::DeepMindOpfDataJson => "DeepMind OPFData JSON",
            TargetFormat::Xiidm => "XIIDM 1.17 XML",
            TargetFormat::Jiidm => "JIIDM 1.17 JSON",
            TargetFormat::Cgmes => "CGMES 3.0 profile set",
            TargetFormat::Ucte => "UCTE-DEF .uct",
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
            TargetFormat::PsseRawx => "psse-rawx",
            TargetFormat::PowerWorld => "powerworld",
            TargetFormat::PandapowerJson => "pandapower-json",
            TargetFormat::Matpower => "matpower",
            TargetFormat::Pslf => "pslf",
            TargetFormat::Goc3Json => "goc3-json",
            TargetFormat::SurgeJson => "surge-json",
            TargetFormat::DeepMindOpfDataJson => "opfdata-json",
            TargetFormat::Xiidm => "xiidm",
            TargetFormat::Jiidm => "jiidm",
            TargetFormat::Cgmes => "cgmes",
            TargetFormat::Ucte => "ucte",
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
        parse_target_format(name).ok_or_else(|| Error::UnknownFormat(name.to_string()))
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
        parse_display_format(name).ok_or_else(|| Error::UnknownFormat(name.to_string()))
    }
}

/// Map a display format name to a [`DisplayFormat`], or `None` if unrecognized.
/// Accepts `pwd`, `powerworld-pwd`, and `powerworld-display`; `geojson`,
/// `geo-json`, and `geo` name the geographic layer.
#[must_use]
pub fn parse_display_format(name: &str) -> Option<DisplayFormat> {
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
/// `surge-json`/`surge`, `opfdata-json`/`opfdata`/`gridopt`, `xiidm`, `jiidm`,
/// `cgmes`, and `ucte`/`uct`.
/// Case-insensitive. The one place the bindings (Python, C ABI) share, so a new
/// format means one new arm here, not three. CGMES emits a profile directory;
/// PyPSA CSV folders, GridFM datasets, PowerWorld `.pwb`, and IEEE CDF cases
/// are routed by [`crate::format::routing`].
///
/// [`SourceFormat`]'s reported token is [`SourceFormat::name`], which resolves
/// here directly, so a module can emit to another module's source format
/// token for every supported case format. Compact spellings such as
/// `powermodelsjson` are accepted as format name aliases.
#[must_use]
pub fn parse_target_format(name: &str) -> Option<TargetFormat> {
    // `iidm` and `rawx` are accepted input spellings. Output metadata and
    // requests use the unambiguous grid exchange format names.
    if name.eq_ignore_ascii_case("iidm") || name.eq_ignore_ascii_case("rawx") {
        return None;
    }
    Some(match routing::parse_transmission_format(name)? {
        TransmissionFormat::Matpower => TargetFormat::Matpower,
        TransmissionFormat::PowerModelsJson => TargetFormat::PowerModelsJson,
        TransmissionFormat::EgretJson => TargetFormat::EgretJson,
        TransmissionFormat::Psse => TargetFormat::Psse { rev: 33 },
        TransmissionFormat::Psse34 => TargetFormat::Psse { rev: 34 },
        TransmissionFormat::Psse35 => TargetFormat::Psse { rev: 35 },
        TransmissionFormat::PsseRawx => TargetFormat::PsseRawx,
        TransmissionFormat::PowerWorld => TargetFormat::PowerWorld,
        TransmissionFormat::PandapowerJson => TargetFormat::PandapowerJson,
        TransmissionFormat::Pslf => TargetFormat::Pslf,
        TransmissionFormat::Goc3Json => TargetFormat::Goc3Json,
        TransmissionFormat::SurgeJson => TargetFormat::SurgeJson,
        TransmissionFormat::DeepMindOpfDataJson => TargetFormat::DeepMindOpfDataJson,
        TransmissionFormat::Xiidm => TargetFormat::Xiidm,
        TransmissionFormat::Jiidm => TargetFormat::Jiidm,
        TransmissionFormat::Cgmes => TargetFormat::Cgmes,
        TransmissionFormat::Ucte => TargetFormat::Ucte,
        TransmissionFormat::PypsaCsv
        | TransmissionFormat::Pwb
        | TransmissionFormat::Gridfm
        | TransmissionFormat::IeeeCdf => {
            return None;
        }
    })
}

/// Parse a declared input format. `iidm` and `rawx` are accepted here only and
/// normalized to the canonical `xiidm` and `psse-rawx` tokens on the resulting
/// module.
fn parse_source_target_format(name: &str) -> Option<TargetFormat> {
    match routing::parse_transmission_format(name) {
        Some(TransmissionFormat::Xiidm) => Some(TargetFormat::Xiidm),
        Some(TransmissionFormat::PsseRawx) => Some(TargetFormat::PsseRawx),
        _ => parse_target_format(name),
    }
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
         use parse_display(Source::open(path)?, None)"
            .into(),
    )
}

/// Render a file extension for a user-facing message: `` extension `xyz` ``
/// when present, `no extension` otherwise.
fn describe_extension(extension: Option<&str>) -> String {
    match extension {
        Some(ext) => format!("extension `{ext}`"),
        None => "no extension".to_owned(),
    }
}

/// Parse display data from one [`powerio_core::Source`], choosing the parser
/// from `from`, the source's declared format, or its name. A `.pwd` extension
/// selects PowerWorld display data.
///
/// # Errors
/// [`Error::UnknownFormat`] if `from` is unrecognized or the extension cannot
/// be mapped; [`Error::Io`] if the file cannot be read; the parser's own
/// [`Error`] on malformed input.
#[allow(
    clippy::needless_pass_by_value,
    reason = "display parsing takes ownership of Source like the main parse operation"
)]
pub fn parse_display(source: powerio_core::Source, from: Option<&str>) -> Result<DisplayData> {
    let path = std::path::Path::new(source.name());
    let declared = from.or_else(|| source.format().map(powerio_core::FormatId::as_str));
    let fmt = match declared {
        Some(f) => parse_display_format(f).ok_or_else(|| Error::UnknownFormat(f.to_string()))?,
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
    let buffer = source
        .primary_buffer()
        .map_err(|error| Error::Io(std::io::Error::other(error)))?;
    let bytes = buffer.content_bytes();
    match fmt {
        DisplayFormat::PowerWorld => Ok(DisplayData::PowerWorld(powerworld::__parse_pwd_display(
            bytes,
        )?)),
        DisplayFormat::GeoJson => {
            let text = std::str::from_utf8(bytes).map_err(|error| Error::FormatRead {
                format: "geo layer",
                message: format!("not valid UTF-8: {error}"),
            })?;
            Ok(DisplayData::Geo(
                crate::geo::GeoLayer::parse(text, path.file_name().and_then(|n| n.to_str()))?.layer,
            ))
        }
    }
}

/// Whether a format name means a PyPSA CSV folder. PyPSA folders are directory
/// inputs, not text targets, so they have no [`TargetFormat`] arm; this is the
/// companion alias matcher to [`parse_target_format`] and the one place the
/// PyPSA aliases live.
pub fn is_pypsa_csv_name(name: &str) -> bool {
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

/// Whether a source format name means the IEEE Common Data Format.
fn is_ieee_cdf_name(name: &str) -> bool {
    routing::parse_transmission_format(name) == Some(TransmissionFormat::IeeeCdf)
}

/// Parse the case file at `path`, choosing the parser from `from` (the
/// [`parse_target_format`] names plus `pypsa-csv`/`pypsa`, `pwb`, `pslf`,
/// and `epc`) or, when `None`, from the path: a directory containing
/// `network.csv` parses as a PyPSA CSV folder (any other directory is refused
/// as a directory with [`Error::UnknownFormat`], before extension inference),
/// and a file maps by extension (`m`/`json`/`raw`/`aux`/`pwb`/`epc`),
/// case insensitively (issue #97: `.RAW` is as common as `.raw` in the wild);
/// a `.txt` or `.cdf` file whose first card is an IEEE CDF title card reads
/// as `ieee-cdf`. A
/// `.json` file is classified by top level shape markers: pandapower
/// (`"_class": "pandapowerNet"`), egret (`elements` and `system`), GO Challenge
/// 3 (`network` plus `time_series_input`/`reliability`, refused here with
/// guidance to the typed facade parser), Surge JSON
/// (`format: "surge-json"`), OPFData (`grid`, `solution`, and `metadata`), and
/// PowerModels JSON (`baseMVA`, `branch`, `gen`, or `gencost`). JSON matching
/// model JSON markers (`buses` plus a network key), distribution markers,
/// ambiguous markers, or no known markers returns [`Error::UnknownFormat`].
/// Declare a format on the source to force a parser. PowerWorld `.pwb` is a
/// binary input only format; PSLF `.epc` is text and supports emission. Returns
/// the typed module: the network value, the parser's findings, and the retained
/// source.
///
/// The balanced network parser used by the top level facade. The CLI and
/// language bindings call that facade so calculation formats such as GO
/// Challenge 3 keep their typed values.
///
/// # Errors
/// A `Request` failure when the format cannot be determined or is refused, an
/// `Io` failure when acquisition fails, and the parser's own failure on
/// malformed input. Findings collected before a failure ride the returned
/// error.
///
pub fn parse(
    source: powerio_core::Source,
) -> std::result::Result<PioModule<BalancedNetwork>, powerio_core::Error> {
    parse_with_json_class(source, None)
}

/// [`parse`], given a JSON classification the caller already computed on the
/// same bytes. The `powerio` facade routes a source by its own call to
/// [`routing::classify_json_text`] before it ever reaches this crate; when
/// that routing lands on the balanced hub, passing the result here skips the
/// second classification [`parse`] would otherwise run over the identical
/// text. `None` reproduces [`parse`] exactly, classifying inline only if and
/// when [`parse_to_network`] needs to.
///
/// Not part of this crate's public reading surface — the facade is the one
/// caller with a classification already in hand — so this stays out of the
/// rendered docs.
///
/// # Errors
/// Same as [`parse`].
#[doc(hidden)]
pub fn parse_with_json_class(
    source: powerio_core::Source,
    json_class: Option<routing::JsonClass>,
) -> std::result::Result<PioModule<BalancedNetwork>, powerio_core::Error> {
    // Resolve the physical JSON transport once. In particular, model JSON
    // carries the network's semantic origin inside the serialized value, but
    // those bytes are model JSON rather than that origin's case format.
    let json_class = json_class.or_else(|| {
        let buffer = source.primary_buffer().ok()?;
        let text = std::str::from_utf8(buffer.content_bytes()).ok()?;
        Some(routing::classify_json_text(text))
    });
    let is_model_json = matches!(json_class, Some(routing::JsonClass::ModelJson));
    let is_rawx = source
        .format()
        .and_then(|format| parse_target_format(format.as_str()))
        == Some(TargetFormat::PsseRawx)
        || std::path::Path::new(source.name())
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("rawx"))
        || matches!(
            json_class,
            Some(routing::JsonClass::Case(routing::Detection::Known(
                routing::SourceFormat::Transmission(TransmissionFormat::PsseRawx)
            )))
        );
    let is_xiidm = source.format().is_some_and(|format| {
        routing::parse_transmission_format(format.as_str()) == Some(TransmissionFormat::Xiidm)
    }) || std::path::Path::new(source.name())
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("xiidm"))
        || source
            .primary_buffer()
            .is_ok_and(|buffer| xiidm::looks_like_xiidm(buffer.content_bytes()));
    let is_jiidm = source.format().is_some_and(|format| {
        routing::parse_transmission_format(format.as_str()) == Some(TransmissionFormat::Jiidm)
    }) || std::path::Path::new(source.name())
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("jiidm"))
        || matches!(
            json_class,
            Some(routing::JsonClass::Case(routing::Detection::Known(
                routing::SourceFormat::Transmission(TransmissionFormat::Jiidm)
            )))
        );
    let is_cgmes = source.format().is_some_and(|format| {
        routing::parse_transmission_format(format.as_str()) == Some(TransmissionFormat::Cgmes)
    }) || cgmes::looks_like_profile_set(&source);
    let mut warnings = Diagnostics::new();
    match parse_to_network(&source, &mut warnings, json_class) {
        Ok(mut network) => {
            network.assign_missing_component_ids();
            // Record the detected format on the retained source before the
            // common constructor builds descriptors and the coarse root
            // source map. RAWX aliases normalize to the one public token.
            let source = if is_rawx {
                source.with_format(
                    powerio_core::FormatId::new("psse-rawx")
                        .expect("the canonical RAWX token is valid"),
                )
            } else if is_jiidm {
                source.with_format(
                    powerio_core::FormatId::new("jiidm")
                        .expect("the canonical JIIDM token is valid"),
                )
            } else if is_xiidm {
                source.with_format(
                    powerio_core::FormatId::new("xiidm")
                        .expect("the canonical XIIDM token is valid"),
                )
            } else if is_cgmes {
                source.with_format(
                    powerio_core::FormatId::new("cgmes")
                        .expect("the canonical CGMES token is valid"),
                )
            } else if source.format().is_some() {
                source
            } else {
                let format = if is_model_json {
                    "model-json"
                } else {
                    network.source_format().name()
                };
                match powerio_core::FormatId::new(format) {
                    Ok(format) => source.with_format(format),
                    Err(_) => source,
                }
            };
            PioModule::parsed(network, source, warnings.into_records())
        }
        Err(error) => {
            // A reader that failed on a located record leaves that record's
            // byte range on the collector; the failure carries it as a span.
            let mut core = powerio_core::Error::new(error.code(), error.to_string());
            if let Some(span) = warnings.record_span() {
                core = core.with_span(span);
            }
            Err(core
                .with_diagnostics(warnings.into_records())
                .with_cause(error)
                .with_source(source))
        }
    }
}

/// The format dispatch behind [`parse`]: name and content detection, then the
/// one reader map. `json_class` is a classification the caller already
/// computed on this source's own text ([`parse_with_json_class`]); when it is
/// `None`, this classifies inline at the point a `.json` source needs it,
/// exactly as [`parse`] always has.
#[allow(clippy::too_many_lines)]
fn parse_to_network(
    source: &powerio_core::Source,
    warnings: &mut Diagnostics,
    json_class: Option<routing::JsonClass>,
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
        if from.is_some_and(|format| {
            routing::parse_transmission_format(format) == Some(TransmissionFormat::Cgmes)
        }) || (from.is_none() && cgmes::looks_like_profile_set(source))
        {
            return cgmes::parse_source(source, warnings);
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
    if from.is_some_and(|format| {
        routing::parse_transmission_format(format) == Some(TransmissionFormat::Cgmes)
    }) || (from.is_none() && cgmes::looks_like_profile_set(source))
    {
        return cgmes::parse_source(source, warnings);
    }
    if from.is_some_and(|format| format == "model-json") {
        let buffer = primary(source)?;
        return BalancedNetwork::from_json(source_text(&buffer)?);
    }
    // PowerWorld `.pwb` is binary and read only; dispatch it before the text
    // read. `from` accepts "pwb" for files with a different extension.
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    let looks_like_xiidm = source
        .primary_buffer()
        .is_ok_and(|buffer| xiidm::looks_like_xiidm(buffer.content_bytes()));
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
    // An IEEE CDF case has no fixed extension: the public archives use
    // `.txt` and some tools `.cdf`, so those two are inferred from the
    // title card layout and any other name needs the declared format.
    if from.is_some_and(is_ieee_cdf_name)
        || (from.is_none()
            && matches!(ext.as_deref(), Some("txt" | "cdf"))
            && source
                .primary_buffer()
                .is_ok_and(|buffer| ieee_cdf::looks_like_ieee_cdf(buffer.content_bytes())))
    {
        let buffer = primary(source)?;
        let text = source_text(&buffer)?;
        // Record spans refer to the whole retained buffer, so the decoded
        // text's offset past a byte order mark is part of every span.
        let origin = ieee_cdf::TextOrigin::new(
            buffer.id().clone(),
            (buffer.bytes().len() - buffer.content_bytes().len()) as u64,
        );
        let network = ieee_cdf::parse_ieee_cdf_source(text, stem, Some(origin), warnings)?;
        reject_empty_case(&network, ieee_cdf::FMT)?;
        return Ok(network);
    }
    if from
        .and_then(parse_source_target_format)
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
            if parse_display_format(f).is_some() {
                return Err(display_file_guidance());
            }
            Some(parse_source_target_format(f).ok_or_else(|| unknown_source_format(f))?)
        }
        None => {
            // Everything but `.json` (sniffed below) resolves without the text.
            match ext.as_deref() {
                Some("m") => Some(TargetFormat::Matpower),
                Some("raw") => Some(TargetFormat::Psse { rev: 33 }),
                Some("rawx") => Some(TargetFormat::PsseRawx),
                Some("aux") => Some(TargetFormat::PowerWorld),
                Some("xiidm") => Some(TargetFormat::Xiidm),
                Some("jiidm") => Some(TargetFormat::Jiidm),
                Some("xml") if looks_like_xiidm => Some(TargetFormat::Xiidm),
                Some("xml" | "zip") => Some(TargetFormat::Cgmes),
                Some("uct") => Some(TargetFormat::Ucte),
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
    if fmt_hint == Some(TargetFormat::Xiidm) {
        let network = xiidm::parse_xiidm_bytes(buffer.content_bytes(), warnings)?;
        reject_empty_case(&network, TargetFormat::Xiidm.label())?;
        return Ok(network);
    }
    let text = source_text(&buffer)?;
    // Readers that locate records mark them as byte ranges of `text`; the
    // retained buffer starts with the byte order mark `text` omits.
    warnings.locate_in(
        buffer.id().clone(),
        (buffer.bytes().len() - buffer.content_bytes().len()) as u64,
    );
    let fmt = match fmt_hint {
        Some(fmt) => fmt,
        // A caller ahead of this (the `powerio` facade's own routing) may
        // already have classified this exact text; trust that answer instead
        // of running the same classification a second time. `unwrap_or_else`
        // only classifies here when nothing did yet, so a caller with no
        // hint (every direct `parse` caller) behaves exactly as before.
        None => match json_class.unwrap_or_else(|| routing::classify_json_text(text)) {
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
/// same way. Readers borrow the text; the module retains the source bytes,
/// and `warnings` is located in that buffer for readers that attach record
/// spans to their findings.
fn read_source(
    text: &str,
    fmt: TargetFormat,
    name_hint: Option<&str>,
    warnings: &mut Diagnostics,
) -> Result<BalancedNetwork> {
    let net = match fmt {
        TargetFormat::Matpower => matpower::parse_matpower_source(text, name_hint, warnings),
        TargetFormat::PowerModelsJson => {
            powermodels::parse_powermodels_json_source(text, name_hint, warnings)
        }
        TargetFormat::Psse { .. } => psse::parse_psse_source(text, name_hint, warnings),
        TargetFormat::PsseRawx => rawx::parse_rawx_source(text, name_hint, warnings),
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
        TargetFormat::Goc3Json => {
            return Err(Error::UnknownFormat(
                "goc3-json defines a GO Challenge 3 calculation; use powerio::parse to obtain AcScucInstance or AcScucSolution"
                    .into(),
            ));
        }
        TargetFormat::SurgeJson => surge::parse_surge_source(text, name_hint, warnings),
        TargetFormat::DeepMindOpfDataJson => {
            opfdata::parse_opfdata_source(text, name_hint, warnings)
        }
        TargetFormat::Xiidm => xiidm::parse_xiidm_source(text, warnings),
        TargetFormat::Jiidm => xiidm::parse_jiidm_source(text, warnings),
        TargetFormat::Cgmes => {
            cgmes::parse_text(name_hint.unwrap_or("profile.xml"), text, warnings)
        }
        TargetFormat::Ucte => ucte::parse_ucte_source(text, name_hint, warnings),
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

/// Reject a case with neither an AC calculation view nor physical DC equipment.
/// XIIDM 1.17 permits a network containing only DC nodes and equipment, while
/// the other balanced formats still need at least one bus.
pub(crate) fn reject_empty_case(net: &BalancedNetwork, format: &'static str) -> Result<()> {
    let detailed = net.detailed_connectivity();
    let has_dc_equipment = detailed.as_ref().is_some_and(|detailed| {
        !detailed.dc_nodes.is_empty()
            || !detailed.dc_grounds.is_empty()
            || !detailed.dc_lines.is_empty()
            || !detailed.dc_switches.is_empty()
    });
    let has_empty_xiidm_voltage_level = matches!(
        net.source_format(),
        SourceFormat::Xiidm | SourceFormat::Jiidm
    ) && detailed
        .as_ref()
        .is_some_and(|detailed| !detailed.voltage_levels.is_empty());
    if net.buses().is_empty() && !has_dc_equipment && !has_empty_xiidm_voltage_level {
        return Err(Error::FormatRead {
            format,
            message: "case has no buses or DC equipment".into(),
        });
    }
    Ok(())
}

/// The source format names this crate recognizes, each with its aliases. A
/// recognized calculation format can still be refused with guidance to the
/// top level facade. The unknown format error prints this list, and a test walks
/// every alias through [`routing::parse_transmission_format`] so it
/// cannot drift from the matcher. `pypsa-csv` names a directory source and
/// `pwb` a binary one; every other name reads file and memory sources alike.
pub const SOURCE_FORMAT_NAMES: &str = "matpower/m, powermodels-json/powermodels/pm, \
     egret-json/egret, psse/raw, psse34, psse35, psse-rawx/rawx, powerworld/aux, \
     pandapower-json/pandapower/pp, pslf/epc, pypsa-csv/pypsa, pwb, goc3-json/goc3, \
     surge-json/surge, opfdata-json/opfdata/gridopt, xiidm/iidm, jiidm, cgmes, ucte/uct, \
     ieee-cdf/cdf";

/// An unrecognized source format token. When the token names a distribution
/// format (`dss`, `pmd`, `bmopf`), the error points at the distribution
/// surface instead of echoing the token: this parser reads only balanced
/// transmission formats. Otherwise the refusal enumerates the accepted names.
fn unknown_source_format(name: &str) -> Error {
    if let Some(dist) = routing::parse_distribution_format(name) {
        return Error::UnknownFormat(format!(
            "`{}` is a distribution format, and this parser reads only balanced \
             transmission formats; parse it through the one module family \
             (`powerio::parse` in Rust and `parse` in the language bindings), \
             which routes distribution formats",
            dist.name()
        ));
    }
    Error::UnknownFormat(format!("{name}; accepted names: {SOURCE_FORMAT_NAMES}"))
}

/// The case format a JSON classification selects; the shapes that are not
/// case formats are refused with the surface that reads them named. Model
/// JSON never reaches this from `parse`, which decodes it directly.
fn json_target_from_class(class: JsonClass) -> Result<TargetFormat> {
    match class {
        JsonClass::Module => Err(Error::UnknownFormat(
            "JSON is PowerIO IR; decode it with `deserialize` rather than the \
             grid exchange format parser"
                .into(),
        )),
        JsonClass::ModelJson => Err(Error::UnknownFormat(
            "JSON is bare powerio model JSON, which is not a case format; read it with \
             `BalancedNetwork::from_json` in Rust or serialize a complete PowerIO module"
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
        TransmissionFormat::PsseRawx => Ok(TargetFormat::PsseRawx),
        TransmissionFormat::Jiidm => Ok(TargetFormat::Jiidm),
        other => Err(Error::UnknownFormat(format!(
            "JSON classifier returned non-JSON transmission format `{}`",
            other.name()
        ))),
    }
}

/// One text serializer's internal output before it commits to a destination.
#[derive(Debug, Clone)]
pub(crate) struct TextEmission {
    pub(crate) text: String,
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) fidelity: powerio_core::Fidelity,
}

impl TextEmission {
    pub(crate) fn new(text: String, diagnostics: Diagnostics) -> Self {
        Self {
            text,
            diagnostics: diagnostics.into_records(),
            fidelity: powerio_core::Fidelity::Canonical,
        }
    }

    /// An emission that dropped nothing, e.g. a same format echo.
    pub(crate) fn faithful(text: String) -> Self {
        let mut emission = Self::new(text, Diagnostics::new());
        emission.fidelity = powerio_core::Fidelity::ExactSameFormat;
        emission
    }

    #[cfg(test)]
    pub(crate) fn render_diagnostics(&self) -> Vec<String> {
        crate::diagnostics::render_diagnostics(&self.diagnostics)
    }

    /// Record one finding after the serializer has run.
    pub(crate) fn push(&mut self, info: &'static DiagnosticInfo, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic::of(info, message));
    }

    /// Put parse diagnostics ahead of emission diagnostics.
    pub(crate) fn prepend(&mut self, read: Vec<Diagnostic>) {
        let mut records = read;
        records.append(&mut self.diagnostics);
        self.diagnostics = records;
    }
}

/// Optional emission policies layered on top of the neutral [`BalancedNetwork`].
///
/// The default preserves the module as stated. Other options work on a cloned
/// network and never mutate the caller's case.
#[derive(Debug, Clone, Default)]
pub struct EmitOptions {
    pub missing_gen_cost: MissingGenCostPolicy,
    pub gen_cost_patches: Vec<GenCostPatch>,
}

impl EmitOptions {
    #[must_use]
    pub fn is_default(&self) -> bool {
        self.missing_gen_cost.is_preserve() && self.gen_cost_patches.is_empty()
    }
}

/// Prepare a parsed module for emission to `format`. Emitting to the source format of an
/// unchanged parsed module returns the retained source bytes exactly,
/// including a byte order mark; any other target serializes the typed value.
///
/// # Errors
/// [`Error::WriteUnsupported`] for a read only target, and the serializer's own
/// [`Error`] on a case it cannot state.
fn emit_text(
    module: &PioModule<BalancedNetwork>,
    format: TargetFormat,
) -> std::result::Result<TextEmission, powerio_core::Error> {
    if let Some(text) = echo_text(module, format) {
        return Ok(TextEmission::faithful(text));
    }
    let mut conv = emit_value_text(module.value(), format).map_err(core_error)?;
    warn_psse_downgrade(module, format, &mut conv);
    Ok(conv)
}

/// Project a crate failure onto the common operation failure type.
pub(crate) fn core_error(error: Error) -> powerio_core::Error {
    let message = error.to_string();
    powerio_core::Error::new(error.code(), message).with_cause(error)
}

/// The retained source text when emitting `module` back to its source format:
/// the echo that reproduces the input byte for byte. `None` sends the emission
/// down the semantic serialization path.
fn echo_text(module: &PioModule<BalancedNetwork>, target: TargetFormat) -> Option<String> {
    let source = module.source()?;
    let buffer = source.primary_buffer().ok()?;
    let source_format = source
        .format()
        .and_then(|format| parse_target_format(format.as_str()))?;
    if !same_target_format(target, source_format) {
        return None;
    }
    let text = std::str::from_utf8(buffer.bytes()).ok()?;
    // A PSS/E source echoes only when the requested revision equals the
    // source's own; any other revision goes through write_psse_rev so the
    // caller gets the layout it asked for instead of the original bytes.
    if let TargetFormat::Psse { rev } = target
        && psse::header_rev(text.trim_start_matches('\u{feff}')).ok()? != rev
    {
        return None;
    }
    Some(text.to_owned())
}

/// Serialize a typed network to `format` with no source echo.
pub(crate) fn emit_value_text(net: &BalancedNetwork, format: TargetFormat) -> Result<TextEmission> {
    let mut conv = match format {
        TargetFormat::PowerModelsJson => write_powermodels_json(net),
        TargetFormat::EgretJson => write_egret_json(net),
        TargetFormat::Psse { rev } => {
            if !matches!(rev, 33..=35) {
                return Err(Error::Emit {
                    format: "PSS/E .raw",
                    message: format!(
                        "unsupported revision {rev}; emission supports only revisions 33, 34, and 35"
                    ),
                });
            }
            net.check_base_mva()?;
            write_psse_rev(net, rev)
        }
        TargetFormat::PsseRawx => {
            net.check_base_mva()?;
            write_rawx(net)?
        }
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
        TargetFormat::Xiidm => write_xiidm(net)?,
        TargetFormat::Jiidm => write_jiidm(net)?,
        TargetFormat::Cgmes => {
            return Err(Error::WriteUnsupported { format: "cgmes" });
        }
        TargetFormat::Ucte => ucte::write_ucte(net)?,
    };
    warn_normalized_tap(net, format, &mut conv);
    warn_missing_reference(net, format, &mut conv);
    warn_dropped_frequency(net, format, &mut conv);
    warn_dropped_locations(net, format, &mut conv);
    warn_dropped_transformer_charging(net, format, &mut conv);
    Ok(conv)
}

/// Emit a parsed module to `format` through a destination: the one output
/// operation over file, memory, and (for the directory formats) folder
/// output. Every text target commits a single artifact — a path destination
/// names the exact file, a memory destination names the artifact — staged
/// and renamed into place so a failed emission never exposes a partial target.
/// The result carries the complete artifact inventory and the serializer's
/// findings.
///
/// # Errors
/// The format serializer or destination refused the operation.
/// failures.
pub fn emit(
    module: &PioModule<BalancedNetwork>,
    format: TargetFormat,
    destination: powerio_core::Destination,
) -> std::result::Result<powerio_core::EmitResult, powerio_core::Error> {
    emit_with_options(module, format, &EmitOptions::default(), destination)
}

/// [`emit()`] with generator cost policies.
///
/// # Errors
/// As [`emit()`].
///
/// # Panics
/// Never on external input: the fixed artifact name is valid by
/// construction.
pub fn emit_with_options(
    module: &PioModule<BalancedNetwork>,
    format: TargetFormat,
    options: &EmitOptions,
    destination: powerio_core::Destination,
) -> std::result::Result<powerio_core::EmitResult, powerio_core::Error> {
    if format == TargetFormat::Cgmes
        && options.is_default()
        && let Some(source) = module.source()
        && source
            .format()
            .is_some_and(|value| value.as_str() == "cgmes")
    {
        if source.is_directory() {
            let mut artifacts = Vec::new();
            for name in source.entry_names()? {
                let buffer = source.buffer(&name)?;
                artifacts.push(powerio_core::MemoryArtifact::new(
                    name,
                    buffer.bytes().to_vec(),
                ));
            }
            return destination.__commit_artifacts(
                true,
                powerio_core::Fidelity::ExactSameFormat,
                artifacts,
                Vec::new(),
            );
        }
        if source.acquired_buffers().len() == 1 {
            let buffer = source.primary_buffer()?;
            let file_name = std::path::Path::new(buffer.name())
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| powerio_core::ArtifactPath::new(name).ok())
                .unwrap_or_else(|| {
                    powerio_core::ArtifactPath::new("case.zip")
                        .expect("static name is a valid artifact path")
                });
            let artifact = powerio_core::MemoryArtifact::new(file_name, buffer.bytes().to_vec());
            return destination.__commit_artifacts(
                false,
                powerio_core::Fidelity::ExactSameFormat,
                vec![artifact],
                Vec::new(),
            );
        }
    }
    if matches!(format, TargetFormat::Xiidm | TargetFormat::Jiidm)
        && options.is_default()
        && let Some(source) = module.source()
        && source
            .format()
            .and_then(|value| parse_target_format(value.as_str()))
            == Some(format)
        && source.acquired_buffers().len() == 1
    {
        let buffer = source.primary_buffer()?;
        let artifact = powerio_core::MemoryArtifact::new(
            powerio_core::ArtifactPath::new(format!("case.{}", format.extension()))
                .expect("the format extension names a valid artifact path"),
            buffer.bytes().to_vec(),
        );
        return destination.__commit_artifacts(
            false,
            powerio_core::Fidelity::ExactSameFormat,
            vec![artifact],
            Vec::new(),
        );
    }
    if format == TargetFormat::Cgmes {
        let (working, mut diagnostics) = if options.is_default() {
            (module.value().clone(), Vec::new())
        } else {
            apply_emit_cost_policy(module.value(), options).map_err(core_error)?
        };
        let (artifacts, format_diagnostics) = cgmes::artifacts(&working).map_err(core_error)?;
        diagnostics.extend(format_diagnostics.into_records());
        return destination.__commit_artifacts(
            true,
            powerio_core::Fidelity::Canonical,
            artifacts,
            diagnostics,
        );
    }
    let conv = emit_text_with_options(module, format, options)?;
    let artifact = powerio_core::MemoryArtifact::new(
        powerio_core::ArtifactPath::new("case").expect("static name is a valid artifact path"),
        conv.text.into_bytes(),
    );
    destination.__commit_artifacts(false, conv.fidelity, vec![artifact], conv.diagnostics)
}

/// Emit a parsed module as a PyPSA CSV folder through a destination. Either
/// destination names the output root and
/// every returned artifact sits below it; the whole inventory commits
/// atomically.
///
/// # Errors
/// The destination's collision and staging failures.
///
/// # Panics
/// Never on external input: the serializer's fixed artifact names are valid by
/// construction.
#[doc(hidden)]
pub fn __emit_pypsa_csv(
    module: &PioModule<BalancedNetwork>,
    destination: powerio_core::Destination,
) -> std::result::Result<powerio_core::EmitResult, powerio_core::Error> {
    __emit_pypsa_csv_with_options(module, &EmitOptions::default(), destination)
}

/// Internal bridge for the universal facade's PyPSA directory dispatch.
#[doc(hidden)]
pub fn __emit_pypsa_csv_with_options(
    module: &PioModule<BalancedNetwork>,
    options: &EmitOptions,
    destination: powerio_core::Destination,
) -> std::result::Result<powerio_core::EmitResult, powerio_core::Error> {
    let (working, mut diagnostics) = if options.is_default() {
        (None, Vec::new())
    } else {
        let (network, diagnostics) =
            apply_emit_cost_policy(module.value(), options).map_err(core_error)?;
        (Some(network), diagnostics)
    };
    let (artifacts, format_diagnostics) =
        pypsa::pypsa_csv_artifacts(working.as_ref().unwrap_or(module.value()));
    diagnostics.extend(format_diagnostics);
    let artifacts = artifacts
        .into_iter()
        .map(|(name, text)| {
            powerio_core::MemoryArtifact::new(
                powerio_core::ArtifactPath::new(name).expect("the writer emits fixed valid names"),
                text.into_bytes(),
            )
        })
        .collect();
    destination.__commit_artifacts(
        true,
        powerio_core::Fidelity::Canonical,
        artifacts,
        diagnostics,
    )
}

/// Prepare a parsed module with emission policies. The plain
/// emission behavior is preserved when `options` is default; a non-default
/// policy edits a copy of the typed value, so its emission never echoes source
/// bytes the policy no longer matches.
fn emit_text_with_options(
    module: &PioModule<BalancedNetwork>,
    format: TargetFormat,
    options: &EmitOptions,
) -> std::result::Result<TextEmission, powerio_core::Error> {
    if options.is_default() {
        return emit_text(module, format);
    }
    let (working, policy_warnings) =
        apply_emit_cost_policy(module.value(), options).map_err(core_error)?;
    let mut conv = emit_value_text(&working, format).map_err(core_error)?;
    conv.prepend(policy_warnings);
    Ok(conv)
}

/// Apply the emission cost policy to a copy of `net` and report what it did.
///
/// Shared by the text and directory writers so both surfaces run one policy and
/// describe it with the same findings. The caller's network is never mutated.
pub(crate) fn apply_emit_cost_policy(
    net: &BalancedNetwork,
    options: &EmitOptions,
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

/// Warn when a PSS/E source is emitted at an older revision than its own.
/// The `psse` and `raw` emission aliases resolve to revision 33, so emitting a
/// v34/v35 source through the default target skips
/// the echo path (revisions differ) and re-emits the v33 layout, dropping the
/// modern records (12 named ratings, load DG/LOADTYPE columns, the system-wide
/// block) and any unmodeled section the echo would have preserved. Name the
/// downgrade instead of performing it silently.
fn warn_psse_downgrade(
    module: &PioModule<BalancedNetwork>,
    format: TargetFormat,
    conv: &mut TextEmission,
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
        if let Ok(src_rev) = psse::header_rev(src)
            && src_rev > rev
        {
            conv.push(
                &codes::EMIT_PSSE_DOWNGRADED,
                format!(
                    "PSS/E source is revision {src_rev} but the emission target is revision {rev}; \
                     the older layout drops fields the source carried (emit as psse{src_rev} to keep them)"
                ),
            );
        }
    }
}

/// Warn when a non-default system frequency is emitted to a format with no frequency
/// field. PSS/E (`BASFRQ`) and pandapower (`f_hz`) carry it; MATPOWER,
/// PowerModels, egret, and PowerWorld have nowhere to put it, so a 50 Hz case
/// would parse again as the 60 Hz default. Report the loss instead.
fn warn_dropped_frequency(net: &BalancedNetwork, format: TargetFormat, conv: &mut TextEmission) {
    let carries_frequency = matches!(
        format,
        TargetFormat::Psse { .. } | TargetFormat::PsseRawx | TargetFormat::PandapowerJson
    );
    if carries_frequency {
        return;
    }
    // UCTE-DEF has no frequency field either, but it describes the 50 Hz
    // synchronous area, so a 50 Hz case loses nothing and reads back as 50.
    if format == TargetFormat::Ucte {
        if (net.base_frequency() - 50.0).abs() > 1e-9 {
            conv.push(
                &format.emit_family().field_dropped,
                format!(
                    "system base frequency {} Hz dropped: UCTE-DEF describes the 50 Hz synchronous area and has no frequency field (reads back as 50 Hz)",
                    net.base_frequency()
                ),
            );
        }
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
/// (`geo`) carry them, and PyPSA folder emission (`x`/`y`) has its own
/// path; MATPOWER, PSS/E, PowerModels, egret, PSLF, and Surge have nowhere to
/// put them, matching the `base_frequency` behavior. `powerio geo extract`
/// emits the sidecar as the escape hatch.
fn warn_dropped_locations(net: &BalancedNetwork, format: TargetFormat, conv: &mut TextEmission) {
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
                 coordinate field (emit a .geo.json sidecar to keep them)",
                format.label()
            ),
        );
    }
}

/// Warn when a transformer carries line charging and the target's
/// transformer record has no susceptance column to hold it. The PSLF `.epc`
/// transformer record is the one such target; PSS/E emits representable
/// magnetizing admittance and the MATPOWER serializers keep the legacy total
/// projection on the branch row, so neither drops it.
fn warn_dropped_transformer_charging(
    net: &BalancedNetwork,
    format: TargetFormat,
    conv: &mut TextEmission,
) {
    if !matches!(format, TargetFormat::Pslf) {
        return;
    }
    let n = net
        .branches()
        .iter()
        .filter(|b| b.is_transformer() && b.calc_total_charging_b() != 0.0)
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
pub fn parse_format_id(
    token: &str,
) -> std::result::Result<powerio_core::FormatId, powerio_core::Error> {
    powerio_core::FormatId::new(token.to_ascii_lowercase().replace('_', "-"))
}

/// Warn when a network with no reference (slack) bus converts to a format
/// whose solvers require one. PowerWorld `.pwb` is the one source that
/// systematically lacks the designation (the binary does not store it), so
/// the silent case would be common; `to_normalized` synthesizes a slack at
/// the largest pmax in service generator bus for consumers that need one.
fn warn_missing_reference(net: &BalancedNetwork, format: TargetFormat, conv: &mut TextEmission) {
    let needs_ref = matches!(
        format,
        TargetFormat::Matpower
            | TargetFormat::Psse { .. }
            | TargetFormat::PsseRawx
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

/// The slackless network warning itself, shared with the PyPSA folder emitter.
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
// line's tap from `calc_effective_tap()` (the literal `1.0`) and its shift from
// `0.0 * DEG_TO_RAD` (exactly `0.0`), so an epsilon compare would be wrong here.
#[allow(clippy::float_cmp)]
fn warn_normalized_tap(net: &BalancedNetwork, format: TargetFormat, conv: &mut TextEmission) {
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
pub(super) fn nonzero_differs(value: f64, reference: f64) -> bool {
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

/// Whether two case targets identify the same physical format. PSS/E revisions
/// share a family here; the retained header check above decides whether the
/// requested revision is byte exact.
fn same_target_format(requested: TargetFormat, source: TargetFormat) -> bool {
    requested == source
        || matches!(
            (requested, source),
            (TargetFormat::Psse { .. }, TargetFormat::Psse { .. })
        )
}

/// JSON number for a finite `f64`; `Value::Null` for `NaN`/`±Inf`.
pub(crate) fn jnum(x: f64) -> Value {
    serde_json::Number::from_f64(x).map_or(Value::Null, Value::Number)
}

/// Serialize a built JSON tree into a [`TextEmission`], appending one warning that
/// names every field where a non-finite `f64` was written as `null` (JSON has no
/// `±Inf`/`NaN`). Shared by the JSON writers.
pub(crate) fn finish(
    family: &'static EmitFamily,
    root: Map<String, Value>,
    mut warnings: Diagnostics,
) -> TextEmission {
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
    TextEmission::new(text, warnings)
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

/// Test harness for parser and emitter fixtures.
#[cfg(test)]
pub(crate) mod test_parse {
    use super::*;

    #[derive(Debug)]
    pub(crate) struct TestParsed {
        pub network: BalancedNetwork,
        pub diagnostics: Vec<Diagnostic>,
    }

    impl TestParsed {
        pub(crate) fn render_diagnostics(&self) -> Vec<String> {
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
            diagnostics: module.diagnostics.clone(),
            network: module.into_value(),
        })
    }

    pub(crate) fn parse_str(
        text: &str,
        from: &str,
    ) -> std::result::Result<TestParsed, powerio_core::Error> {
        let source = declared(
            powerio_core::Source::from_memory("<memory>", text.as_bytes().to_vec())?,
            Some(from),
        )?;
        parse(source).map(|module| TestParsed {
            diagnostics: module.diagnostics.clone(),
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
                let resolved = routing::parse_transmission_format(alias);
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
            TF::Ucte,
            TF::Xiidm,
            TF::Jiidm,
            TF::Cgmes,
            TF::IeeeCdf,
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
            parsed.render_diagnostics().is_empty(),
            "{:?}",
            parsed.render_diagnostics()
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
        let source = powerio_core::Source::from_memory("case.m", case.as_bytes().to_vec()).unwrap();
        let module = parse(source.with_format(parse_format_id("matpower").unwrap())).unwrap();
        assert_eq!(module.value().buses().len(), 1);
        assert!(module.diagnostics.is_empty(), "{:?}", module.diagnostics);
        let echo = emit_text(&module, TargetFormat::Matpower).unwrap();
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
        let source = powerio_core::Source::from_memory("case.m", case.as_bytes().to_vec()).unwrap();
        let module =
            parse(source.with_format(powerio_core::FormatId::new("matpower").unwrap())).unwrap();
        assert_eq!(
            emit_text(&module, TargetFormat::Matpower).unwrap().text,
            case
        );

        let net = module.into_value();
        let canonical = emit_value_text(&net, TargetFormat::Matpower).unwrap();
        assert_ne!(canonical.text, case);
        let reparsed = parse_str(&canonical.text, "matpower").unwrap();
        assert_eq!(reparsed.network.buses().len(), 1);
    }

    #[test]
    fn source_format_strings_round_trip_to_a_target() {
        // The bindings expose `source_format` as its `name()` token, and
        // `emit` routes that string back through `parse_target_format`.
        // Every writable source format must resolve.
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
            (SourceFormat::Ucte, TargetFormat::Ucte),
            (
                SourceFormat::DeepMindOpfDataJson,
                TargetFormat::DeepMindOpfDataJson,
            ),
        ] {
            let token = sf.name();
            assert_eq!(
                parse_target_format(token),
                Some(want),
                "source_format {token:?} did not round-trip"
            );
        }
        // The derived/in-memory source formats have no writer target, and
        // neither do the read only .pwb binary and the IEEE CDF text.
        for sf in [
            SourceFormat::InMemory,
            SourceFormat::Normalized,
            SourceFormat::Gridfm,
            SourceFormat::PypsaCsv,
            SourceFormat::PowerWorldBinary,
            SourceFormat::IeeeCdf,
        ] {
            assert_eq!(parse_target_format(sf.name()), None);
        }
    }
}
