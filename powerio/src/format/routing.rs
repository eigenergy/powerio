//! Shared format alias and JSON shape routing for the `powerio` crate.
//!
//! It maps format names and top level JSON markers without parsing a document.

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
    #[doc(hidden)]
    PowerioJson,
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
            Self::PowerioJson => "powerio-json",
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
    if let Some(format) = transmission_format_from_name(name) {
        return Detection::Known(SourceFormat::Transmission(format));
    }
    if let Some(format) = distribution_format_from_name(name) {
        return Detection::Known(SourceFormat::Distribution(format));
    }
    Detection::Unknown
}

pub fn transmission_format_from_name(name: &str) -> Option<TransmissionFormat> {
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
        "poweriojson" | "powerio" | "json" => Some(TransmissionFormat::PowerioJson),
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

pub fn distribution_format_from_name(name: &str) -> Option<DistributionFormat> {
    let key = canonical_key(name);
    match key.as_str() {
        "dss" | "opendss" => Some(DistributionFormat::Dss),
        "pmd" | "pmdjson" | "engineering" => Some(DistributionFormat::PmdJson),
        "bmopf" | "bmopfjson" => Some(DistributionFormat::BmopfJson),
        _ => None,
    }
}

/// Top level classification of bare JSON text: a `.pio.json` package envelope
/// or a case document with its format detection. The envelope outcome lives in
/// the classifier's result rather than a separate predicate, so every consumer
/// handles it, and one parse answers both questions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JsonClass {
    /// A `.pio.json` package envelope. A package is not a converter boundary
    /// format, so it stays out of [`SourceFormat`]; callers route it to the
    /// package reader instead of a case parser.
    Package,
    /// A case document and its format detection.
    Case(Detection<JsonFormat>),
}

/// Classify a JSON document: a `.pio.json` package envelope, or a case
/// document across the transmission and distribution domains.
///
/// An envelope is recognized by a top level `model_kind` of `"balanced"` or
/// `"multiconductor"` plus a `model` key; the value check keeps a case
/// document that happens to carry those key names from being misrouted.
/// For a case, Unknown means there is no recognized top level marker, and
/// Ambiguous means the document contains strong markers from both domains, so
/// the caller must ask the user for an explicit format.
pub fn classify_json_text(text: &str) -> JsonClass {
    let Ok(shape) = JsonShape::try_from(text) else {
        return JsonClass::Case(Detection::Unknown);
    };
    if matches!(
        shape.string("model_kind"),
        Some("balanced" | "multiconductor")
    ) && shape.has("model")
    {
        return JsonClass::Package;
    }
    JsonClass::Case(shape.classify())
}

fn canonical_key(name: &str) -> String {
    name.to_ascii_lowercase()
        .chars()
        .filter(|c| *c != '-' && *c != '_')
        .collect()
}

struct JsonShape {
    object: serde_json::Map<String, serde_json::Value>,
}

impl TryFrom<&str> for JsonShape {
    type Error = ();

    fn try_from(text: &str) -> Result<Self, Self::Error> {
        let value = serde_json::from_str::<serde_json::Value>(text).map_err(|_| ())?;
        let serde_json::Value::Object(object) = value else {
            return Err(());
        };
        Ok(Self { object })
    }
}

impl JsonShape {
    fn has(&self, key: &str) -> bool {
        self.object.contains_key(key)
    }

    fn string(&self, key: &str) -> Option<&str> {
        self.object.get(key).and_then(serde_json::Value::as_str)
    }

    fn classify(&self) -> Detection<JsonFormat> {
        let is_pandapower = self.string("_class") == Some("pandapowerNet");
        let is_egret = self.has("elements") && self.has("system");
        let is_goc3 = self.has("network")
            && (self.has("time_series_input") || self.has("reliability"))
            && self.object.get("network").is_some_and(|network| {
                network.as_object().is_some_and(|obj| {
                    obj.contains_key("simple_dispatchable_device")
                        || obj.contains_key("ac_line")
                        || obj.contains_key("two_winding_transformer")
                })
            });
        let is_surge = self.string("format") == Some("surge-json")
            && self.has("schema_version")
            && self.has("network");
        let is_opfdata = self
            .object
            .get("grid")
            .and_then(serde_json::Value::as_object)
            .is_some_and(|grid| {
                grid.contains_key("nodes")
                    && grid.contains_key("edges")
                    && grid.contains_key("context")
            })
            && self
                .object
                .get("solution")
                .and_then(serde_json::Value::as_object)
                .is_some_and(|solution| {
                    solution.contains_key("nodes") && solution.contains_key("edges")
                })
            && self
                .object
                .get("metadata")
                .and_then(serde_json::Value::as_object)
                .is_some_and(|metadata| metadata.contains_key("objective"));
        let is_powerio = self.has("buses")
            && (self.has("branches")
                || self.has("base_mva")
                || self.has("loads")
                || self.has("generators"));
        let is_power_models =
            self.has("baseMVA") || self.has("branch") || self.has("gen") || self.has("gencost");
        let transmission = is_pandapower
            || is_egret
            || is_goc3
            || is_surge
            || is_opfdata
            || is_powerio
            || is_power_models;

        let is_pmd = self.has("data_model");
        let strong_bmopf = self.has("line")
            || self.has("linecode")
            || self.has("transformer")
            || self.has("voltage_source");
        let weak_bmopf = self.has("bus")
            || self.has("load")
            || self.has("generator")
            || self.has("shunt")
            || self.has("switch");
        let distribution = is_pmd || strong_bmopf || (weak_bmopf && !transmission);

        match (transmission, distribution) {
            (true, true) => Detection::Ambiguous,
            (true, false) => Detection::Known(SourceFormat::Transmission(if is_pandapower {
                TransmissionFormat::PandapowerJson
            } else if is_egret {
                TransmissionFormat::EgretJson
            } else if is_goc3 {
                TransmissionFormat::Goc3Json
            } else if is_surge {
                TransmissionFormat::SurgeJson
            } else if is_opfdata {
                TransmissionFormat::DeepMindOpfDataJson
            } else if is_powerio {
                TransmissionFormat::PowerioJson
            } else {
                TransmissionFormat::PowerModelsJson
            })),
            (false, true) => Detection::Known(SourceFormat::Distribution(if is_pmd {
                DistributionFormat::PmdJson
            } else {
                DistributionFormat::BmopfJson
            })),
            (false, false) => Detection::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Detection, DistributionFormat, JsonClass, SourceFormat, TransmissionFormat,
        classify_json_text,
    };

    #[test]
    fn classifies_package_envelope() {
        assert_eq!(
            classify_json_text(
                r#"{"model_kind":"multiconductor","model":{"kind":"multiconductor"}}"#
            ),
            JsonClass::Package
        );
        assert_eq!(
            classify_json_text(r#"{"model_kind":"balanced","model":{}}"#),
            JsonClass::Package
        );
        // A payload alone is not an envelope, and neither is a case document,
        // even one that carries the envelope key names with case-file values.
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
    fn classifies_powerio_json() {
        assert_eq!(
            classify_json_text(r#"{"base_mva":100.0,"buses":[],"branches":[]}"#),
            JsonClass::Case(Detection::Known(SourceFormat::Transmission(
                TransmissionFormat::PowerioJson
            )))
        );
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
                super::transmission_format_from_name(alias),
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
                super::transmission_format_from_name(alias),
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
                super::transmission_format_from_name(alias),
                Some(TransmissionFormat::DeepMindOpfDataJson),
                "{alias}"
            );
        }
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
}
