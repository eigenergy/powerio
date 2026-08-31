//! Shared format alias and JSON shape routing for the `powerio` crate.
//!
//! It maps format name aliases with no parsing at all, and classifies JSON
//! content by deserializing a typed header instead of materializing the
//! whole document into a generic value tree.

use serde::Deserialize;

/// A classification result that can be known, absent, or unsafe to choose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detection<T> {
    Known(T),
    Unknown,
    Ambiguous,
}

impl<T> Detection<T> {
    pub fn known(self) -> Option<T> {
        match self {
            Self::Known(value) => Some(value),
            Self::Unknown | Self::Ambiguous => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Domain {
    Transmission,
    Distribution,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TransmissionFormat {
    Matpower,
    PowerModelsJson,
    EgretJson,
    Psse,
    Psse34,
    Psse35,
    PowerWorld,
    PandapowerJson,
    PypsaCsv,
    Pslf,
    Pwb,
    Gridfm,
    Goc3Json,
    SurgeJson,
    DeepMindOpfDataJson,
}

impl TransmissionFormat {
    pub fn name(self) -> &'static str {
        match self {
            Self::Matpower => "matpower",
            Self::PowerModelsJson => "powermodels-json",
            Self::EgretJson => "egret-json",
            Self::Psse => "psse",
            Self::Psse34 => "psse34",
            Self::Psse35 => "psse35",
            Self::PowerWorld => "powerworld",
            Self::PandapowerJson => "pandapower-json",
            Self::PypsaCsv => "pypsa-csv",
            Self::Pslf => "pslf",
            Self::Pwb => "pwb",
            Self::Gridfm => "gridfm",
            Self::Goc3Json => "goc3-json",
            Self::SurgeJson => "surge-json",
            Self::DeepMindOpfDataJson => "opfdata-json",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DistributionFormat {
    Dss,
    PmdJson,
    BmopfJson,
}

impl DistributionFormat {
    pub fn name(self) -> &'static str {
        match self {
            Self::Dss => "dss",
            Self::PmdJson => "pmd-json",
            Self::BmopfJson => "bmopf-json",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SourceFormat {
    Transmission(TransmissionFormat),
    Distribution(DistributionFormat),
}

impl SourceFormat {
    pub fn domain(self) -> Domain {
        match self {
            Self::Transmission(_) => Domain::Transmission,
            Self::Distribution(_) => Domain::Distribution,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Transmission(format) => format.name(),
            Self::Distribution(format) => format.name(),
        }
    }
}

pub type JsonFormat = SourceFormat;

/// Resolve a source format name or common alias.
pub fn classify_format_name(name: &str) -> Detection<SourceFormat> {
    if let Some(format) = parse_transmission_format(name) {
        return Detection::Known(SourceFormat::Transmission(format));
    }
    if let Some(format) = parse_distribution_format(name) {
        return Detection::Known(SourceFormat::Distribution(format));
    }
    Detection::Unknown
}

pub fn parse_transmission_format(name: &str) -> Option<TransmissionFormat> {
    let key = canonical_key(name);
    match key.as_str() {
        "matpower" | "m" => Some(TransmissionFormat::Matpower),
        "powermodelsjson" | "powermodels" | "pm" => Some(TransmissionFormat::PowerModelsJson),
        "egretjson" | "egret" => Some(TransmissionFormat::EgretJson),
        "psse" | "psse33" | "raw" | "raw33" => Some(TransmissionFormat::Psse),
        "psse34" | "raw34" => Some(TransmissionFormat::Psse34),
        "psse35" | "raw35" => Some(TransmissionFormat::Psse35),
        "powerworld" | "aux" => Some(TransmissionFormat::PowerWorld),
        "pandapowerjson" | "pandapower" | "pp" => Some(TransmissionFormat::PandapowerJson),
        "pypsacsv" | "pypsa" => Some(TransmissionFormat::PypsaCsv),
        "pslf" | "epc" | "pslfepc" => Some(TransmissionFormat::Pslf),
        "pwb" => Some(TransmissionFormat::Pwb),
        "gridfm" => Some(TransmissionFormat::Gridfm),
        "goc3" | "goc3json" | "go3" | "gochallenge3" | "c3" => Some(TransmissionFormat::Goc3Json),
        "surge" | "surgejson" => Some(TransmissionFormat::SurgeJson),
        "opfdata"
        | "opfdatajson"
        | "deepmindopfdata"
        | "deepmindopfdatajson"
        | "gridopt"
        | "gridoptjson" => Some(TransmissionFormat::DeepMindOpfDataJson),
        _ => None,
    }
}

pub fn parse_distribution_format(name: &str) -> Option<DistributionFormat> {
    let key = canonical_key(name);
    match key.as_str() {
        "dss" | "opendss" => Some(DistributionFormat::Dss),
        "pmd" | "pmdjson" | "engineering" => Some(DistributionFormat::PmdJson),
        "bmopf" | "bmopfjson" => Some(DistributionFormat::BmopfJson),
        _ => None,
    }
}

/// Top level classification of bare JSON text: a `.pio.json` package, bare
/// model JSON, or a case document with its format detection. The package and
/// model JSON outcomes live in the classifier's result rather than in separate
/// predicates, so every consumer handles them, and one header read answers
/// every question instead of a full document parse per question.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JsonClass {
    /// A `.pio.json` stored module document. The stored document is not a
    /// converter boundary format, so it stays out of [`SourceFormat`];
    /// callers route it to the stored module reader instead of a case parser.
    Module,
    /// Bare [`BalancedNetwork`](crate::BalancedNetwork) model JSON, written by
    /// `to_json` and read by `from_json`. powerio authors it, so it is not a
    /// case format and stays out of [`SourceFormat`]; callers route it to
    /// those two methods instead of a case parser.
    ModelJson,
    /// A case document and its format detection.
    Case(Detection<JsonFormat>),
}

/// The closed set of classification families, in the one spelling every
/// surface uses: the Rust [`JsonClass`], the C `pio_classify_str` label before
/// its optional `:<format>` tail, the Python `classify_json_text` status, and
/// the Julia family symbol.
///
/// Spellings are permanent and a family is never removed or redefined. A new
/// family appends to this list and gets a changelog line, so a consumer that
/// dispatches a file picker on it keeps working.
pub const JSON_CLASSES: [&str; 6] = [
    "transmission",
    "distribution",
    "module",
    "model-json",
    "ambiguous",
    "unknown",
];

impl JsonClass {
    /// This classification's family token, one of [`JSON_CLASSES`]. The
    /// detected format, where there is one, is [`SourceFormat::name`].
    #[must_use]
    pub fn family(self) -> &'static str {
        match self {
            Self::Module => "module",
            Self::ModelJson => "model-json",
            Self::Case(Detection::Known(format)) => match format.domain() {
                Domain::Transmission => "transmission",
                Domain::Distribution => "distribution",
            },
            Self::Case(Detection::Ambiguous) => "ambiguous",
            Self::Case(Detection::Unknown) => "unknown",
        }
    }
}

/// Classify a JSON document: a `.pio.json` stored module, bare model JSON, or a case
/// document across the transmission and distribution domains.
///
/// A package is recognized by a top level `model_kind` of `"balanced"` or
/// `"multiconductor"` plus a `model` key (the released 0.9 shape), or by the
/// version 1 stored module's own `schema: "powerio.module"` header; the value
/// check keeps a case document that happens to carry those key names from
/// being misrouted.
/// Model JSON is recognized by `buses` beside another network key, which the
/// case formats spell differently (PowerModels writes `bus`, not `buses`).
/// For a case, Unknown means there is no recognized top level marker, and
/// Ambiguous means the document contains strong markers from both domains, so
/// the caller must ask the user for an explicit format.
pub fn classify_json_text(text: &str) -> JsonClass {
    // Windows tooling saves JSON with a UTF-8 byte order mark, which
    // serde_json rejects; strip it so a BOM never hides the format.
    let Ok(header) = serde_json::from_str::<JsonHeader>(text.trim_start_matches('\u{feff}')) else {
        return JsonClass::Case(Detection::Unknown);
    };
    if matches!(
        header.model_kind.as_deref(),
        Some("balanced" | "multiconductor")
    ) && header.model
    {
        return JsonClass::Module;
    }
    // The version 1 stored module names itself in its header.
    if header.schema.as_deref() == Some("powerio.module") {
        return JsonClass::Module;
    }
    header.classify()
}

/// Classify JSON bytes without performing lossy text replacement.
///
/// Invalid UTF-8 has no trustworthy JSON markers and returns the same
/// [`Detection::Unknown`] classification as malformed JSON. A leading UTF-8
/// byte order mark is accepted through [`classify_json_text`].
#[must_use]
pub fn classify_json_bytes(bytes: &[u8]) -> JsonClass {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return JsonClass::Case(Detection::Unknown);
    };
    classify_json_text(text)
}

fn canonical_key(name: &str) -> String {
    name.to_ascii_lowercase()
        .chars()
        .filter(|c| *c != '-' && *c != '_')
        .collect()
}

/// Reads `true` when a key is present, for whatever value it holds — a JSON
/// `null` included, unlike a plain `Option<T>` field, which would treat a
/// `null` value the same as a missing key. [`serde::de::IgnoredAny`] walks
/// and discards the value without materializing it, so a marker key whose
/// value is a large array or object (an unrelated top level table in the
/// same document) costs one scan, not an allocation per element.
fn present<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    serde::de::IgnoredAny::deserialize(deserializer)?;
    Ok(true)
}

/// The key's string value, or `None` for a missing key, a `null`, or any
/// other non-string value — matching `serde_json::Value::as_str`'s
/// permissive read, so a marker key of the wrong type carries no marker
/// instead of failing the whole header.
fn maybe_str<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(serde_json::Value::deserialize(deserializer)?
        .as_str()
        .map(str::to_owned))
}

/// `Some(header)` when the key's value is a JSON object, `None` for a
/// missing key or any non-object value — matching the object guard the
/// nested markers (`grid`, `solution`, `metadata`) read before checking a
/// key within them.
fn maybe_object<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    if value.is_object() {
        T::deserialize(value)
            .map(Some)
            .map_err(serde::de::Error::custom)
    } else {
        Ok(None)
    }
}

/// The `network` key's presence, independent of its shape (the surge marker
/// only needs presence), paired with its nested markers when it does turn
/// out to be an object (the GO Challenge 3 markers). One key answers both,
/// since a `maybe_object` field alone would lose the presence signal for a
/// non-object value.
#[derive(Default)]
struct NetworkField {
    present: bool,
    header: NetworkHeader,
}

impl<'de> Deserialize<'de> for NetworkField {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        let header = if value.is_object() {
            NetworkHeader::deserialize(value).map_err(serde::de::Error::custom)?
        } else {
            NetworkHeader::default()
        };
        Ok(Self {
            present: true,
            header,
        })
    }
}

#[derive(Default, Deserialize)]
struct NetworkHeader {
    #[serde(default, deserialize_with = "present")]
    simple_dispatchable_device: bool,
    #[serde(default, deserialize_with = "present")]
    ac_line: bool,
    #[serde(default, deserialize_with = "present")]
    two_winding_transformer: bool,
}

#[derive(Deserialize)]
struct GridHeader {
    #[serde(default, deserialize_with = "present")]
    nodes: bool,
    #[serde(default, deserialize_with = "present")]
    edges: bool,
    #[serde(default, deserialize_with = "present")]
    context: bool,
}

#[derive(Deserialize)]
struct SolutionHeader {
    #[serde(default, deserialize_with = "present")]
    nodes: bool,
    #[serde(default, deserialize_with = "present")]
    edges: bool,
}

#[derive(Deserialize)]
struct MetadataHeader {
    #[serde(default, deserialize_with = "present")]
    objective: bool,
}

/// The top level markers [`JsonClass`] needs, read in one pass over the
/// document: unlisted keys (every case format's actual data rows) are
/// skipped by serde's derived `Deserialize` without being materialized, and
/// the listed keys cost at most a presence check or one short string.
// Every bool here is an independent named field a derived `Deserialize`
// fills from its own JSON key, never a positional constructor argument, so
// the excessive-bools lint's mix-up concern does not apply.
#[allow(clippy::struct_excessive_bools)]
#[derive(Default, Deserialize)]
struct JsonHeader {
    #[serde(default, deserialize_with = "maybe_str")]
    model_kind: Option<String>,
    #[serde(default, deserialize_with = "present")]
    model: bool,
    #[serde(default, deserialize_with = "maybe_str")]
    schema: Option<String>,
    #[serde(default, deserialize_with = "maybe_str", rename = "_class")]
    pandapower_class: Option<String>,
    #[serde(default, deserialize_with = "present")]
    elements: bool,
    #[serde(default, deserialize_with = "present")]
    system: bool,
    #[serde(default)]
    network: NetworkField,
    #[serde(default, deserialize_with = "present")]
    time_series_input: bool,
    #[serde(default, deserialize_with = "present")]
    reliability: bool,
    #[serde(default, deserialize_with = "maybe_str")]
    format: Option<String>,
    #[serde(default, deserialize_with = "present")]
    schema_version: bool,
    #[serde(default, deserialize_with = "maybe_object")]
    grid: Option<GridHeader>,
    #[serde(default, deserialize_with = "maybe_object")]
    solution: Option<SolutionHeader>,
    #[serde(default, deserialize_with = "maybe_object")]
    metadata: Option<MetadataHeader>,
    #[serde(default, deserialize_with = "present")]
    buses: bool,
    #[serde(default, deserialize_with = "present")]
    branches: bool,
    #[serde(default, deserialize_with = "present")]
    base_mva: bool,
    #[serde(default, deserialize_with = "present")]
    loads: bool,
    #[serde(default, deserialize_with = "present")]
    generators: bool,
    #[serde(default, deserialize_with = "present", rename = "baseMVA")]
    base_mva_camel: bool,
    #[serde(default, deserialize_with = "present")]
    branch: bool,
    #[serde(default, deserialize_with = "present")]
    r#gen: bool,
    #[serde(default, deserialize_with = "present")]
    gencost: bool,
    #[serde(default, deserialize_with = "present")]
    data_model: bool,
    #[serde(default, deserialize_with = "present")]
    line: bool,
    #[serde(default, deserialize_with = "present")]
    linecode: bool,
    #[serde(default, deserialize_with = "present")]
    transformer: bool,
    #[serde(default, deserialize_with = "present")]
    voltage_source: bool,
    #[serde(default, deserialize_with = "present")]
    bus: bool,
    #[serde(default, deserialize_with = "present")]
    load: bool,
    #[serde(default, deserialize_with = "present")]
    generator: bool,
    #[serde(default, deserialize_with = "present")]
    shunt: bool,
    #[serde(default, deserialize_with = "present")]
    switch: bool,
}

impl JsonHeader {
    fn classify(&self) -> JsonClass {
        let is_pandapower = self.pandapower_class.as_deref() == Some("pandapowerNet");
        let is_egret = self.elements && self.system;
        let is_goc3 = (self.time_series_input || self.reliability)
            && (self.network.header.simple_dispatchable_device
                || self.network.header.ac_line
                || self.network.header.two_winding_transformer);
        let is_surge = self.format.as_deref() == Some("surge-json")
            && self.schema_version
            && self.network.present;
        let is_opfdata = self
            .grid
            .as_ref()
            .is_some_and(|grid| grid.nodes && grid.edges && grid.context)
            && self
                .solution
                .as_ref()
                .is_some_and(|solution| solution.nodes && solution.edges)
            && self
                .metadata
                .as_ref()
                .is_some_and(|metadata| metadata.objective);
        let is_model_json =
            self.buses && (self.branches || self.base_mva || self.loads || self.generators);
        let is_power_models = self.base_mva_camel || self.branch || self.r#gen || self.gencost;
        let transmission = is_pandapower
            || is_egret
            || is_goc3
            || is_surge
            || is_opfdata
            || is_model_json
            || is_power_models;

        let is_pmd = self.data_model;
        let strong_bmopf = self.line || self.linecode || self.transformer || self.voltage_source;
        let weak_bmopf = self.bus || self.load || self.generator || self.shunt || self.switch;
        let distribution = is_pmd || strong_bmopf || (weak_bmopf && !transmission);

        match (transmission, distribution) {
            (true, true) => JsonClass::Case(Detection::Ambiguous),
            // Model JSON is answered inside the transmission arm rather than
            // ahead of it, so a document carrying distribution markers too is
            // still reported as ambiguous instead of being claimed here.
            (true, false)
                if is_model_json
                    && !is_pandapower
                    && !is_egret
                    && !is_goc3
                    && !is_surge
                    && !is_opfdata =>
            {
                JsonClass::ModelJson
            }
            (true, false) => JsonClass::Case(Detection::Known(SourceFormat::Transmission(
                if is_pandapower {
                    TransmissionFormat::PandapowerJson
                } else if is_egret {
                    TransmissionFormat::EgretJson
                } else if is_goc3 {
                    TransmissionFormat::Goc3Json
                } else if is_surge {
                    TransmissionFormat::SurgeJson
                } else if is_opfdata {
                    TransmissionFormat::DeepMindOpfDataJson
                } else {
                    TransmissionFormat::PowerModelsJson
                },
            ))),
            (false, true) => {
                JsonClass::Case(Detection::Known(SourceFormat::Distribution(if is_pmd {
                    DistributionFormat::PmdJson
                } else {
                    DistributionFormat::BmopfJson
                })))
            }
            (false, false) => JsonClass::Case(Detection::Unknown),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Detection, DistributionFormat, JsonClass, SourceFormat, TransmissionFormat,
        classify_json_bytes, classify_json_text,
    };

    #[test]
    fn classifies_package() {
        assert_eq!(
            classify_json_text(
                r#"{"model_kind":"multiconductor","model":{"kind":"multiconductor"}}"#
            ),
            JsonClass::Module
        );
        assert_eq!(
            classify_json_text(r#"{"model_kind":"balanced","model":{}}"#),
            JsonClass::Module
        );
        // A payload alone is not a package, and neither is a case document,
        // even one that carries the package key names with case-file values.
        assert_eq!(
            classify_json_text(r#"{"buses":[],"linecodes":[]}"#),
            JsonClass::Case(Detection::Unknown)
        );
        assert_eq!(
            classify_json_text(r#"{"baseMVA":100.0,"bus":{},"model":"ACP","model_kind":"opf"}"#),
            JsonClass::Case(Detection::Known(SourceFormat::Transmission(
                TransmissionFormat::PowerModelsJson
            )))
        );
        assert_eq!(
            classify_json_text("not json"),
            JsonClass::Case(Detection::Unknown)
        );
    }

    #[test]
    fn classifies_pmd_json() {
        assert_eq!(
            classify_json_text(r#"{"data_model":"ENGINEERING","bus":{}}"#),
            JsonClass::Case(Detection::Known(SourceFormat::Distribution(
                DistributionFormat::PmdJson
            )))
        );
    }

    #[test]
    fn classifies_full_bmopf_json() {
        assert_eq!(
            classify_json_text(r#"{"bus":{},"linecode":{},"voltage_source":{}}"#),
            JsonClass::Case(Detection::Known(SourceFormat::Distribution(
                DistributionFormat::BmopfJson
            )))
        );
    }

    #[test]
    fn classifies_minimal_bmopf_json() {
        assert_eq!(
            classify_json_text(r#"{"bus":{"a":{"terminal_names":["1"]}}}"#),
            JsonClass::Case(Detection::Known(SourceFormat::Distribution(
                DistributionFormat::BmopfJson
            )))
        );
    }

    #[test]
    fn classifies_power_models_with_bus_and_base_mva_as_transmission() {
        assert_eq!(
            classify_json_text(
                r#"{"baseMVA":100.0,"bus":{},"branch":{},"gen":{},"load":{},"switch":{}}"#
            ),
            JsonClass::Case(Detection::Known(SourceFormat::Transmission(
                TransmissionFormat::PowerModelsJson
            )))
        );
    }

    #[test]
    fn classifies_model_json() {
        assert_eq!(
            classify_json_text(r#"{"base_mva":100.0,"buses":[],"branches":[]}"#),
            JsonClass::ModelJson
        );
        assert_eq!(JsonClass::ModelJson.family(), "model-json");
        // Distribution markers beside the model keys are still ambiguous:
        // the model JSON arm must not claim a document it cannot read.
        assert_eq!(
            classify_json_text(r#"{"base_mva":100.0,"buses":[],"linecode":{}}"#),
            JsonClass::Case(Detection::Ambiguous)
        );
    }

    #[test]
    fn every_family_is_in_the_closed_set() {
        for class in [
            JsonClass::Module,
            JsonClass::ModelJson,
            JsonClass::Case(Detection::Ambiguous),
            JsonClass::Case(Detection::Unknown),
            JsonClass::Case(Detection::Known(SourceFormat::Transmission(
                TransmissionFormat::Matpower,
            ))),
            JsonClass::Case(Detection::Known(SourceFormat::Distribution(
                DistributionFormat::Dss,
            ))),
        ] {
            assert!(
                super::JSON_CLASSES.contains(&class.family()),
                "{class:?} answers with a family outside the closed set"
            );
        }
    }

    #[test]
    fn classifies_pandapower_json() {
        assert_eq!(
            classify_json_text(r#"{"_class":"pandapowerNet","_object":{}}"#),
            JsonClass::Case(Detection::Known(SourceFormat::Transmission(
                TransmissionFormat::PandapowerJson
            )))
        );
    }

    #[test]
    fn classifies_egret_json() {
        assert_eq!(
            classify_json_text(r#"{"elements":{},"system":{}}"#),
            JsonClass::Case(Detection::Known(SourceFormat::Transmission(
                TransmissionFormat::EgretJson
            )))
        );
    }

    #[test]
    fn classifies_goc3_json() {
        assert_eq!(
            classify_json_text(
                r#"{"network":{"bus":[],"simple_dispatchable_device":[]},"time_series_input":{}}"#
            ),
            JsonClass::Case(Detection::Known(SourceFormat::Transmission(
                TransmissionFormat::Goc3Json
            )))
        );
    }

    #[test]
    fn resolves_goc3_aliases() {
        for alias in ["goc3-json", "goc3", "go3", "go-challenge-3", "c3"] {
            assert_eq!(
                super::parse_transmission_format(alias),
                Some(TransmissionFormat::Goc3Json),
                "{alias}"
            );
        }
    }

    #[test]
    fn classifies_surge_json() {
        assert_eq!(
            classify_json_text(
                r#"{"format":"surge-json","schema_version":"0.1.0","network":{"buses":[]}}"#
            ),
            JsonClass::Case(Detection::Known(SourceFormat::Transmission(
                TransmissionFormat::SurgeJson
            )))
        );
    }

    #[test]
    fn resolves_surge_aliases() {
        for alias in ["surge-json", "surge", "surgejson"] {
            assert_eq!(
                super::parse_transmission_format(alias),
                Some(TransmissionFormat::SurgeJson),
                "{alias}"
            );
        }
    }

    #[test]
    fn classifies_opfdata_json() {
        assert_eq!(
            classify_json_text(
                r#"{
                    "grid":{"nodes":{},"edges":{},"context":[]},
                    "solution":{"nodes":{},"edges":{}},
                    "metadata":{"objective":0.0}
                }"#
            ),
            JsonClass::Case(Detection::Known(SourceFormat::Transmission(
                TransmissionFormat::DeepMindOpfDataJson
            )))
        );
        assert_eq!(
            classify_json_text(r#"{"grid":{},"solution":{},"metadata":{}}"#),
            JsonClass::Case(Detection::Unknown)
        );
    }

    #[test]
    fn resolves_opfdata_aliases() {
        for alias in [
            "opfdata-json",
            "opfdata",
            "OPFData",
            "deepmind-opfdata-json",
            "deepmind-opfdata",
            "gridopt-json",
            "gridopt",
        ] {
            assert_eq!(
                super::parse_transmission_format(alias),
                Some(TransmissionFormat::DeepMindOpfDataJson),
                "{alias}"
            );
        }
    }

    #[test]
    fn classifies_json_with_leading_byte_order_mark() {
        assert_eq!(
            classify_json_text("\u{feff}{\"baseMVA\":100.0,\"bus\":{},\"branch\":{}}"),
            JsonClass::Case(Detection::Known(SourceFormat::Transmission(
                TransmissionFormat::PowerModelsJson
            )))
        );
    }

    #[test]
    fn classifies_json_bytes_without_lossy_utf8_replacement() {
        assert_eq!(
            classify_json_bytes(b"\xef\xbb\xbf{\"baseMVA\":100.0,\"bus\":{},\"branch\":{}}"),
            JsonClass::Case(Detection::Known(SourceFormat::Transmission(
                TransmissionFormat::PowerModelsJson
            )))
        );
        assert_eq!(
            classify_json_bytes(b"{\"base_mva\":100.0,\"buses\":[],\"branches\":[]}"),
            JsonClass::ModelJson
        );
        assert_eq!(
            classify_json_bytes(b"{\"baseMVA\":100.0,\"bus\":{}\xff}"),
            JsonClass::Case(Detection::Unknown)
        );
    }

    #[test]
    fn unknown_json_has_no_signal() {
        assert_eq!(
            classify_json_text(r#"{"name":"case"}"#),
            JsonClass::Case(Detection::Unknown)
        );
    }

    #[test]
    fn mixed_transmission_and_distribution_markers_are_ambiguous() {
        assert_eq!(
            classify_json_text(r#"{"baseMVA":100.0,"voltage_source":{}}"#),
            JsonClass::Case(Detection::Ambiguous)
        );
    }

    /// A key's presence check is true even when its value is JSON `null`;
    /// only a genuinely absent key is absent. Pins the header deserializer
    /// against the shortcut that would otherwise treat a `null` value the
    /// same as a missing key.
    #[test]
    fn a_null_valued_marker_key_still_counts_as_present() {
        assert_eq!(
            classify_json_text(r#"{"model_kind":"balanced","model":null}"#),
            JsonClass::Module
        );
    }

    /// A string marker (`schema`, `model_kind`, `_class`, `format`) whose
    /// value is not a string carries no marker from that key, matching
    /// `serde_json::Value::as_str`'s permissive read; classification still
    /// proceeds over the document's other markers instead of erroring.
    #[test]
    fn a_non_string_value_at_a_string_marker_key_is_read_as_absent() {
        assert_eq!(
            classify_json_text(r#"{"schema":123,"baseMVA":100.0,"bus":{},"branch":{}}"#),
            JsonClass::Case(Detection::Known(SourceFormat::Transmission(
                TransmissionFormat::PowerModelsJson
            )))
        );
    }

    /// A `network` value that is not a JSON object carries no GO Challenge 3
    /// marker, but does not stop the surge marker (also keyed off `network`,
    /// as a presence check rather than a shape check) from resolving.
    #[test]
    fn a_non_object_network_value_does_not_stop_the_surge_marker() {
        assert_eq!(
            classify_json_text(
                r#"{"format":"surge-json","schema_version":"0.1.0","network":"opaque"}"#
            ),
            JsonClass::Case(Detection::Known(SourceFormat::Transmission(
                TransmissionFormat::SurgeJson
            )))
        );
    }
}
