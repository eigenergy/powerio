//! Distribution format dispatch.

use crate::model::{DistSourceFormat, MulticonductorNetwork};

/// Extra files an emitter generated beside the primary text payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextSidecar {
    /// Relative file path the primary text refers to.
    pub(crate) path: String,
    /// File content.
    pub(crate) text: String,
}

/// One format serializer's internal output before it commits to a destination.
#[derive(Debug, Clone)]
pub(crate) struct TextEmission {
    pub(crate) text: String,
    pub(crate) sidecars: Vec<TextSidecar>,
    pub(crate) diagnostics: Vec<crate::diagnostics::Diagnostic>,
    pub(crate) fidelity: powerio_core::Fidelity,
}

impl TextEmission {
    pub(crate) fn new(
        text: String,
        sidecars: Vec<TextSidecar>,
        diagnostics: crate::diagnostics::Diagnostics,
    ) -> Self {
        Self {
            text,
            sidecars,
            diagnostics: diagnostics.into_records(),
            fidelity: powerio_core::Fidelity::Canonical,
        }
    }

    /// An emission that dropped nothing, e.g. a same format echo.
    pub(crate) fn faithful(text: String) -> Self {
        let mut emission = Self::new(text, Vec::new(), crate::diagnostics::Diagnostics::new());
        emission.fidelity = powerio_core::Fidelity::ExactSameFormat;
        emission
    }

    /// Record one finding after the writer has run.
    pub(crate) fn push(
        &mut self,
        info: &'static crate::diagnostics::DiagnosticInfo,
        message: impl Into<String>,
    ) {
        self.diagnostics
            .push(crate::diagnostics::Diagnostic::of(info, message));
    }

    #[cfg(test)]
    pub(crate) fn render_diagnostics(&self) -> Vec<String> {
        crate::diagnostics::render_diagnostics(&self.diagnostics)
    }
}

/// A writable distribution format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DistTargetFormat {
    Dss,
    BmopfJson,
    PmdJson,
}

/// Format specific policies for distribution network emission.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct EmitOptions {
    pub dss: crate::dss::DssEmitOptions,
    pub bmopf: crate::bmopf::BmopfEmitOptions,
}

impl EmitOptions {
    fn is_default_for(&self, format: DistTargetFormat) -> bool {
        match format {
            DistTargetFormat::Dss => self.dss == crate::dss::DssEmitOptions::default(),
            DistTargetFormat::BmopfJson => self.bmopf == crate::bmopf::BmopfEmitOptions::default(),
            DistTargetFormat::PmdJson => true,
        }
    }
}

/// Resolves common names and file extensions to a target format.
pub fn parse_dist_target_format(name: &str) -> Option<DistTargetFormat> {
    let key = canonical_key(name);
    match key.as_str() {
        "dss" | "opendss" => Some(DistTargetFormat::Dss),
        "pmd" | "pmdjson" | "engineering" => Some(DistTargetFormat::PmdJson),
        "bmopf" | "bmopfjson" => Some(DistTargetFormat::BmopfJson),
        _ => None,
    }
}

impl std::str::FromStr for DistTargetFormat {
    type Err = crate::Error;

    /// [`parse_dist_target_format`] as a `Result`, matching the transmission
    /// hub's `TargetFormat: FromStr`.
    fn from_str(s: &str) -> crate::Result<Self> {
        parse_dist_target_format(s).ok_or_else(|| crate::Error::UnknownFormat(s.to_string()))
    }
}

impl DistTargetFormat {
    /// The canonical format name (`dss`, `pmd-json`, `bmopf-json`), accepted
    /// back by [`parse_dist_target_format`].
    pub fn name(self) -> &'static str {
        match self {
            DistTargetFormat::Dss => "dss",
            DistTargetFormat::PmdJson => "pmd-json",
            DistTargetFormat::BmopfJson => "bmopf-json",
        }
    }
}

fn canonical_key(name: &str) -> String {
    name.to_ascii_lowercase()
        .chars()
        .filter(|c| *c != '-' && *c != '_')
        .collect()
}

/// Element tables that identify a distribution document beside its `bus`
/// table.
///
/// `load`, `shunt`, and `switch` are shared with PowerModels, so on their own
/// they cannot tell the two apart. They stay in the list all the same, because
/// [`NOT_BMOPF_KEYS`] is what refuses a PowerModels document, and it refuses it
/// whatever else the document holds. Dropping the shared names instead would
/// refuse a real BMOPF feeder built only from them, which the reader parses
/// and which this classifier used to accept.
///
/// These names do not separate BMOPF from PMD: the two share most of their
/// element vocabulary. `data_model` does that, and it is checked first.
const DISTRIBUTION_ELEMENT_TABLES: &[&str] = &[
    "capacitor",
    // `control_profile` and `ibr` are typed dispatch tables of the reader,
    // though schema 0.1.0 moved them under `extras`: a pre-0.1.0 document
    // still declares them at the top level, and what the reader accepts the
    // classifier must identify.
    "control_profile",
    "generator",
    "ibr",
    "line",
    "linecode",
    "load",
    "meta",
    "shunt",
    "switch",
    "terminal_conventions",
    "transformer",
    "voltage_source",
];

/// Top level keys no BMOPF document carries, and a PowerModels or MATPOWER
/// derived document does. One of these refuses the BMOPF reading whatever
/// else the document holds, so a name that a future BMOPF revision adds
/// cannot make a PowerModels file classify.
const NOT_BMOPF_KEYS: &[&str] = &[
    "baseMVA",
    "branch",
    "dcline",
    "gen",
    "per_unit",
    "source_type",
    "source_version",
    "storage",
];

/// The PMD marker. Neither BMOPF nor PowerModels carries this key, so it
/// identifies the ENGINEERING and MATHEMATICAL models on its own; the PMD
/// reader then rejects MATHEMATICAL with its own message.
const PMD_MARKER: &str = "data_model";

/// What the top level of a document holds, for classification only.
///
/// The probe reads the top level keys and skips every value, so it never
/// materializes the document. The old classifier built a whole
/// `serde_json::Value` and dropped it, which doubled the peak memory of a
/// parse and did the tokenizing work twice: the chosen reader parses the
/// same text again. A case file is attacker controlled input, so a reader
/// sized allocation that serves no purpose is worth removing.
// Five independent presence flags, which is what a marker probe is. An
// enum or a builder, the shapes this lint steers toward, would model a
// choice; these are not exclusive.
#[allow(clippy::struct_excessive_bools)]
#[derive(Default)]
struct TopLevel {
    /// The document is a JSON object.
    is_object: bool,
    pmd_marker: bool,
    bus: bool,
    /// A key from [`DISTRIBUTION_ELEMENT_TABLES`].
    dist_table: bool,
    /// A key from [`NOT_BMOPF_KEYS`].
    not_bmopf: bool,
}

impl<'de> serde::Deserialize<'de> for TopLevel {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Probe;

        impl<'de> serde::de::Visitor<'de> for Probe {
            type Value = TopLevel;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a JSON document")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<TopLevel, A::Error> {
                let mut out = TopLevel {
                    is_object: true,
                    ..TopLevel::default()
                };
                // Keys arrive as borrowed or owned strings. Nothing is kept:
                // each key sets a flag and each value is discarded, so the
                // probe holds a constant amount of memory whatever the size
                // of the document.
                while let Some(key) = map.next_key::<std::borrow::Cow<'_, str>>()? {
                    let key = key.as_ref();
                    out.pmd_marker |= key == PMD_MARKER;
                    out.bus |= key == "bus";
                    out.dist_table |= DISTRIBUTION_ELEMENT_TABLES.contains(&key);
                    out.not_bmopf |= NOT_BMOPF_KEYS.contains(&key);
                    map.next_value::<serde::de::IgnoredAny>()?;
                }
                Ok(out)
            }

            // A valid document that is not an object cannot be either
            // format. Record that and let the caller report it, rather than
            // failing as a parse error, which would send it to the BMOPF
            // fallback.
            fn visit_bool<E>(self, _: bool) -> Result<TopLevel, E> {
                Ok(TopLevel::default())
            }
            fn visit_i64<E>(self, _: i64) -> Result<TopLevel, E> {
                Ok(TopLevel::default())
            }
            fn visit_u64<E>(self, _: u64) -> Result<TopLevel, E> {
                Ok(TopLevel::default())
            }
            fn visit_f64<E>(self, _: f64) -> Result<TopLevel, E> {
                Ok(TopLevel::default())
            }
            fn visit_str<E>(self, _: &str) -> Result<TopLevel, E> {
                Ok(TopLevel::default())
            }
            fn visit_unit<E>(self) -> Result<TopLevel, E> {
                Ok(TopLevel::default())
            }
            fn visit_none<E>(self) -> Result<TopLevel, E> {
                Ok(TopLevel::default())
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<TopLevel, A::Error> {
                while seq.next_element::<serde::de::IgnoredAny>()?.is_some() {}
                Ok(TopLevel::default())
            }
        }

        deserializer.deserialize_any(Probe)
    }
}

/// Distribution parser policy for a `.json` input.
///
/// Classification is by positive marker, never by "everything else is
/// BMOPF": an unmarked document used to fall through to the BMOPF reader
/// and parse into a bogus near-empty network.
///
/// - PMD carries `data_model`, which no other family here carries. The marker
///   decides on its own: every real PMD ENGINEERING document also carries the
///   element tables BMOPF uses, so a document holding both is the normal case
///   and not a contradiction.
/// - BMOPF carries a `bus` table beside at least one distribution element
///   table, and no key that marks a PowerModels or MATPOWER derived document.
/// - Anything else is refused with a message naming both rules.
///
/// Malformed JSON still routes to BMOPF so its reader reports the parse
/// error, which names the byte offset. The probe never materializes the
/// document, so classification costs one pass and constant memory.
pub fn classify_distribution_json(text: &str) -> crate::Result<DistTargetFormat> {
    // A leading byte order mark would fail the parse here and silently send
    // a PMD document down the BMOPF fallback; classify without it.
    let text = text.trim_start_matches('\u{feff}');
    let unrecognized = |detail: &str| crate::Error::Json {
        format: "distribution",
        message: format!(
            "not a recognized distribution document: {detail}. PMD ENGINEERING JSON \
             carries `data_model`; BMOPF JSON carries a `bus` table beside one of \
             {DISTRIBUTION_ELEMENT_TABLES:?}. Pass the format explicitly to override."
        ),
    };

    let Ok(top) = serde_json::from_str::<TopLevel>(text) else {
        return Ok(DistTargetFormat::BmopfJson);
    };
    if !top.is_object {
        return Err(unrecognized("the top level is not an object"));
    }

    // `data_model` is authoritative. PMD ENGINEERING and BMOPF share most
    // of their element table names (`line`, `linecode`, `transformer`, and
    // the rest), so the marker separates them, not the table set. The PMD
    // reader is what judges the marker's value, and it rejects the
    // MATHEMATICAL model with its own message.
    if top.pmd_marker {
        return Ok(DistTargetFormat::PmdJson);
    }
    if top.bus && top.dist_table && !top.not_bmopf {
        return Ok(DistTargetFormat::BmopfJson);
    }
    Err(if top.bus && top.not_bmopf {
        unrecognized(
            "it carries a `bus` table with PowerModels keys beside it, so it is a \
             transmission document; read it through the transmission hub",
        )
    } else if top.bus {
        unrecognized("its `bus` table has no distribution element table beside it")
    } else {
        unrecognized("it carries no marker of either format")
    })
}

/// Parse one source into a compiled distribution module. The typed
/// [`MulticonductorNetwork`] is the module's value; the reader's findings are
/// the module's diagnostics; the source itself is retained on the module, so
/// a same format emission echoes the input bytes exactly.
///
/// The format comes from the source's declared format when one was selected,
/// the `.dss` extension otherwise, and for `.json` from
/// [`classify_distribution_json`].
///
/// # Errors
/// The operation failure carries the reader's findings up to the failure and
/// retains the source for span interpretation.
///
pub fn parse(
    source: powerio_core::Source,
) -> std::result::Result<powerio_core::PioModule<MulticonductorNetwork>, powerio_core::Error> {
    let mut warnings = crate::collect::Diagnostics::new();
    match parse_to_network(&source, &mut warnings) {
        Ok(network) => {
            let format = *network.source_format();
            // Record the detected format before the common constructor builds
            // descriptors and the coarse `/value` source map.
            let source =
                match format.and_then(|format| powerio_core::FormatId::new(format.name()).ok()) {
                    Some(format) => source.with_format(format),
                    None => source,
                };
            powerio_core::PioModule::parsed(network, source, warnings.into_records())
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

/// The format dispatch behind [`parse`].
fn parse_to_network(
    source: &powerio_core::Source,
    warnings: &mut crate::collect::Diagnostics,
) -> crate::Result<MulticonductorNetwork> {
    if source.is_directory() {
        return Err(crate::Error::UnknownFormat(format!(
            "{} is a directory, and no distribution case format is a directory; pass a case file",
            source.name()
        )));
    }
    let declared = source
        .format()
        .map(powerio_core::FormatId::as_str)
        .map(|name| {
            parse_dist_target_format(name)
                .ok_or_else(|| crate::Error::UnknownFormat(name.to_string()))
        })
        .transpose()?;
    let format = if let Some(format) = declared {
        format
    } else {
        let ext = std::path::Path::new(source.name())
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        match ext.as_str() {
            "dss" => DistTargetFormat::Dss,
            "json" => {
                let buffer = primary(source)?;
                classify_distribution_json(source_text(&buffer)?)?
            }
            other => return Err(crate::Error::UnknownFormat(other.to_string())),
        }
    };
    match format {
        DistTargetFormat::Dss => crate::dss::read::parse_dss_collecting(source, warnings),
        DistTargetFormat::BmopfJson => {
            let buffer = primary(source)?;
            crate::bmopf::read::parse_bmopf_collecting(source_text(&buffer)?, warnings)
        }
        DistTargetFormat::PmdJson => {
            let buffer = primary(source)?;
            crate::pmd::read::parse_pmd_collecting(source_text(&buffer)?, warnings)
        }
    }
}

/// The retained primary buffer of a file or in-memory source.
fn primary(source: &powerio_core::Source) -> crate::Result<powerio_core::SourceBuffer> {
    source
        .primary_buffer()
        .map_err(|error| crate::Error::FormatRead {
            format: "case text",
            message: error.to_string(),
        })
}

/// The buffer's decode slice as UTF-8 text: the retained bytes with a leading
/// byte order mark skipped, so the mark survives for the echo and never
/// reaches a reader.
fn source_text(buffer: &powerio_core::SourceBuffer) -> crate::Result<&str> {
    std::str::from_utf8(buffer.content_bytes()).map_err(|e| crate::Error::FormatRead {
        format: "case text",
        message: format!("not valid UTF-8: {e}"),
    })
}

impl DistTargetFormat {
    fn matches(self, source: Option<DistSourceFormat>) -> bool {
        matches!(
            (self, source),
            (DistTargetFormat::Dss, Some(DistSourceFormat::Dss))
                | (
                    DistTargetFormat::BmopfJson,
                    Some(DistSourceFormat::BmopfJson)
                )
                | (DistTargetFormat::PmdJson, Some(DistSourceFormat::PmdJson))
        )
    }
}

/// Prepare a parsed module for emission to `format`. Emitting an unchanged
/// parsed module to its source format returns the retained source bytes
/// exactly, including a byte order mark; any other target serializes the typed
/// value.
#[must_use]
pub(crate) fn emit_text_with_options(
    module: &powerio_core::PioModule<MulticonductorNetwork>,
    format: DistTargetFormat,
    options: &EmitOptions,
) -> TextEmission {
    if options.is_default_for(format)
        && let Some(text) = echo_text(module, format)
    {
        return TextEmission::faithful(text);
    }
    emit_value_text_with_options(module.value(), format, options)
}

/// The retained source text when emitting `module` back to its source format:
/// the echo that reproduces the input byte for byte. `None` sends the emission
/// down the semantic path.
fn echo_text(
    module: &powerio_core::PioModule<MulticonductorNetwork>,
    target: DistTargetFormat,
) -> Option<String> {
    let source = module.source()?;
    let buffer = source.primary_buffer().ok()?;
    // Both the retained source's declared format and the value's own must be
    // the target: a deserialized module retains the PowerIO IR document, not
    // the case its value came from.
    let declared = source
        .format()
        .and_then(|format| parse_dist_target_format(format.as_str()))?;
    if declared != target || !target.matches(*module.value().source_format()) {
        return None;
    }
    let text = std::str::from_utf8(buffer.bytes()).ok()?;
    Some(text.to_owned())
}

/// Serialize a typed network to `format` with no source echo.
pub(crate) fn emit_value_text_with_options(
    net: &MulticonductorNetwork,
    format: DistTargetFormat,
    options: &EmitOptions,
) -> TextEmission {
    let mut conv = match format {
        DistTargetFormat::Dss => crate::dss::emit_dss_text_with_options(net, &options.dss),
        DistTargetFormat::BmopfJson => {
            crate::bmopf::emit_bmopf_json_text_with_options(net, options.bmopf)
        }
        DistTargetFormat::PmdJson => crate::pmd::emit_pmd_json_text(net),
    };
    // No distribution format carries line routes; report the loss the
    // way bus locations already do (`.pio.json` keeps them).
    let routed = net
        .lines()
        .iter()
        .filter(|line| line.route.is_some())
        .count();
    if routed > 0 {
        conv.push(
            &crate::diagnostics::codes::EMIT_MULTICONDUCTOR_ROUTE_DROPPED,
            format!(
                "{routed} line route(s) dropped: {} has no polyline field",
                format.name()
            ),
        );
    }
    conv
}

#[cfg(test)]
pub(crate) fn emit_value_text(
    net: &MulticonductorNetwork,
    format: DistTargetFormat,
) -> TextEmission {
    emit_value_text_with_options(net, format, &EmitOptions::default())
}

/// Emit a parsed module to `format` through a destination.
///
/// The JSON targets commit a single artifact — a path destination names the
/// exact file, a memory destination names the artifact. The dss target is a
/// directory inventory: the destination names the output root, the case text
/// commits as `case.dss`, and every companion file the emitter produced
/// (OpenDSS `Buscoords` CSV) commits beside it, so nothing the case text
/// refers to is missing from the output.
///
/// # Errors
/// The destination's own collision and staging failures.
///
/// # Panics
/// Never on external input: the fixed artifact names are valid by
/// construction.
pub fn emit(
    module: &powerio_core::PioModule<MulticonductorNetwork>,
    format: DistTargetFormat,
    destination: powerio_core::Destination,
) -> std::result::Result<powerio_core::EmitResult, powerio_core::Error> {
    emit_with_options(module, format, &EmitOptions::default(), destination)
}

/// [`emit`] with format specific policies.
///
/// # Errors
/// The destination refused the output inventory.
///
/// # Panics
/// Never on external input: the fixed artifact names are valid by
/// construction.
pub fn emit_with_options(
    module: &powerio_core::PioModule<MulticonductorNetwork>,
    format: DistTargetFormat,
    options: &EmitOptions,
    destination: powerio_core::Destination,
) -> std::result::Result<powerio_core::EmitResult, powerio_core::Error> {
    let conv = emit_text_with_options(module, format, options);
    match format {
        DistTargetFormat::Dss => {
            let mut artifacts = vec![powerio_core::MemoryArtifact::new(
                powerio_core::ArtifactPath::new("case.dss")
                    .expect("static name is a valid artifact path"),
                conv.text.into_bytes(),
            )];
            for sidecar in conv.sidecars {
                artifacts.push(powerio_core::MemoryArtifact::new(
                    powerio_core::ArtifactPath::new(sidecar.path)?,
                    sidecar.text.into_bytes(),
                ));
            }
            destination.__commit_artifacts(true, conv.fidelity, artifacts, conv.diagnostics)
        }
        DistTargetFormat::BmopfJson | DistTargetFormat::PmdJson => {
            let artifact = powerio_core::MemoryArtifact::new(
                powerio_core::ArtifactPath::new("case")
                    .expect("static name is a valid artifact path"),
                conv.text.into_bytes(),
            );
            destination.__commit_artifacts(false, conv.fidelity, vec![artifact], conv.diagnostics)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distribution_json_classifier_preserves_pmd_marker_and_bmopf_fallback() {
        for doc in [
            r#"{"data_model": "ENGINEERING"}"#,
            r#"{"data_model": "MATHEMATICAL"}"#,
            // The marker identifies the family whatever its value is; the
            // PMD reader is what judges the value.
            r#"{"data_model": 7}"#,
            r#"{"data_model": null}"#,
        ] {
            assert_eq!(
                classify_distribution_json(doc).unwrap(),
                DistTargetFormat::PmdJson,
                "{doc}"
            );
        }
        for doc in [
            r#"{"bus": {}, "voltage_source": {}}"#,
            // A pre-0.1.0 feeder fragment: no `voltage_source`, but the
            // reader accepts it, so the classifier must too.
            r#"{"bus": {}, "line": {}, "linecode": {}}"#,
            r#"{"bus": {}, "transformer": {}}"#,
            r#"{"bus": {}, "capacitor": {}}"#,
            r#"{"bus": {}, "generator": {}}"#,
            // The pre-0.1.0 top-level spellings of the tables 0.1.0 moved
            // under `extras`; the reader dispatches both.
            r#"{"bus": {}, "ibr": {}}"#,
            r#"{"bus": {}, "control_profile": {}}"#,
        ] {
            assert_eq!(
                classify_distribution_json(doc).unwrap(),
                DistTargetFormat::BmopfJson,
                "{doc}"
            );
        }
        // Malformed JSON still routes to BMOPF so its reader reports the
        // parse error, which names the byte offset.
        assert_eq!(
            classify_distribution_json("{not json").unwrap(),
            DistTargetFormat::BmopfJson
        );
        // A byte order mark must not push a PMD document down the BMOPF
        // fallback.
        assert_eq!(
            classify_distribution_json("\u{feff}{\"data_model\": \"ENGINEERING\"}").unwrap(),
            DistTargetFormat::PmdJson
        );
    }

    /// A PowerModels document shares `bus`, `load`, `shunt`, `switch`, and
    /// `name` with BMOPF, so none of those can be the discriminator. This
    /// is the exact document family that used to parse into a bogus
    /// near-empty `MulticonductorNetwork`.
    #[test]
    fn a_powermodels_document_never_classifies_as_bmopf() {
        // The real key set a powerio PowerModels write produces.
        let powermodels = r#"{"baseMVA": 100.0, "branch": {}, "bus": {}, "dcline": {},
            "gen": {}, "load": {}, "name": "case14", "per_unit": true, "shunt": {},
            "source_type": "matpower", "source_version": "2", "storage": {},
            "switch": {}}"#;
        assert!(classify_distribution_json(powermodels).is_err());

        // Each PowerModels marker refuses the BMOPF reading on its own, even
        // beside a real BMOPF table. A future BMOPF revision that adds a
        // colliding table name therefore cannot make this document classify.
        for marker in NOT_BMOPF_KEYS {
            let doc = format!("{{\"bus\": {{}}, \"linecode\": {{}}, \"{marker}\": 1}}");
            assert!(
                classify_distribution_json(&doc).is_err(),
                "`{marker}` must refuse the BMOPF reading: {doc}"
            );
        }
    }

    /// The two rules pull in opposite directions and this classifier has
    /// swung both ways: dropping the shared table names refuses real BMOPF
    /// feeders, and keeping them without the veto reads PowerModels as BMOPF.
    /// Pin both ends together so neither correction can undo the other.
    #[test]
    fn shared_table_names_classify_as_bmopf_and_the_veto_still_refuses_powermodels() {
        // A BMOPF feeder built only from names PowerModels also uses. No
        // veto key is present, so the distribution reading stands.
        for doc in [
            r#"{"bus": {}, "load": {}}"#,
            r#"{"bus": {}, "shunt": {}}"#,
            r#"{"bus": {}, "switch": {}}"#,
            r#"{"bus": {}, "meta": {"frequency": 60}}"#,
        ] {
            assert_eq!(
                classify_distribution_json(doc).unwrap(),
                DistTargetFormat::BmopfJson,
                "{doc}"
            );
        }
        // The same shared names beside one veto key stay transmission.
        for doc in [
            r#"{"bus": {}, "load": {}, "baseMVA": 100.0}"#,
            r#"{"bus": {}, "shunt": {}, "branch": {}}"#,
            r#"{"bus": {}, "switch": {}, "per_unit": true}"#,
        ] {
            assert!(classify_distribution_json(doc).is_err(), "{doc}");
        }
    }

    /// PMD ENGINEERING and BMOPF share most element table names, so a real
    /// PMD document carries `data_model` beside `line` and `linecode`. The
    /// marker must win, or every PMD file would be read as BMOPF.
    #[test]
    fn the_pmd_marker_wins_over_shared_element_tables() {
        let both = r#"{"data_model": "ENGINEERING", "bus": {}, "line": {}, "linecode": {}}"#;
        assert_eq!(
            classify_distribution_json(both).unwrap(),
            DistTargetFormat::PmdJson
        );
    }

    #[test]
    fn unclassifiable_documents_are_refused_with_a_reason() {
        for (doc, needle) in [
            (
                r#"{"bus": {"data_model": {}}}"#,
                "no distribution element table",
            ),
            (r#"{"name": "data_model"}"#, "no marker of either format"),
            ("{}", "no marker of either format"),
            ("[]", "not an object"),
            ("null", "not an object"),
            ("3", "not an object"),
            (r#""a string""#, "not an object"),
            ("true", "not an object"),
        ] {
            let err = classify_distribution_json(doc).unwrap_err().to_string();
            assert!(err.contains(needle), "{doc}: got {err}");
        }
    }

    /// The probe reads top level keys and skips values, so neither the size
    /// of a value nor the number of keys can make it allocate the document.
    /// A deeply nested value hits serde_json's own recursion limit, which
    /// surfaces as the malformed-JSON route rather than a stack overflow.
    #[test]
    fn the_probe_is_bounded_on_adversarial_shapes() {
        // A huge value under an ignored key: skipped, not materialized.
        let big = format!(
            r#"{{"bus": {{}}, "linecode": {{}}, "junk": [{}]}}"#,
            "0,".repeat(200_000) + "0"
        );
        assert_eq!(
            classify_distribution_json(&big).unwrap(),
            DistTargetFormat::BmopfJson
        );

        // Many distinct top level keys: one flag per key, nothing stored.
        let mut keys = String::new();
        for i in 0..50_000 {
            use std::fmt::Write as _;
            let _ = write!(keys, "\"k{i}\":0,");
        }
        let many = format!(r#"{{{keys}"bus":{{}},"linecode":{{}}}}"#);
        assert_eq!(
            classify_distribution_json(&many).unwrap(),
            DistTargetFormat::BmopfJson
        );

        // Deep nesting under an ignored key: `IgnoredAny` skips a value
        // without recursion, so depth costs no stack. The old classifier
        // built a `serde_json::Value`, whose recursive descent refuses past
        // 128 levels, so a legitimate document nested deeper than that used
        // to take the malformed route.
        let deep = format!(
            r#"{{"bus":{{}},"linecode":{{}},"junk":{}{}}}"#,
            "[".repeat(20_000),
            "]".repeat(20_000)
        );
        assert_eq!(
            classify_distribution_json(&deep).unwrap(),
            DistTargetFormat::BmopfJson
        );

        // A duplicate marker key is still one marker.
        assert_eq!(
            classify_distribution_json(
                r#"{"data_model":"ENGINEERING","data_model":"ENGINEERING"}"#
            )
            .unwrap(),
            DistTargetFormat::PmdJson
        );
    }

    /// The probe skips a value without recursion, so it accepts a document
    /// nested far deeper than the reader will take. The reader must then
    /// refuse that document with an error, never with a crash: the
    /// classifier is what decides which reader sees untrusted input.
    #[test]
    fn a_document_the_probe_accepts_is_refused_by_the_reader_not_a_crash() {
        for depth in [200usize, 20_000, 500_000] {
            let doc = format!(
                "{{\"bus\":{{}},\"linecode\":{{}},\"junk\":{}{}}}",
                "[".repeat(depth),
                "]".repeat(depth)
            );
            let format = classify_distribution_json(&doc).expect("markers are present");
            assert_eq!(format, DistTargetFormat::BmopfJson);
            let err = crate::testkit::parse_str(&doc, format.name())
                .expect_err("the reader refuses past its recursion limit");
            assert!(
                err.to_string().contains("recursion limit"),
                "depth {depth}: {err}"
            );
        }
    }

    /// JSON keys are case sensitive and both formats are machine written,
    /// so a near miss must not classify. It would pick a reader that then
    /// fails on every table.
    #[test]
    fn marker_matching_is_case_sensitive() {
        for doc in [
            r#"{"Data_Model": "ENGINEERING"}"#,
            r#"{"DATA_MODEL": "ENGINEERING"}"#,
            r#"{"Bus": {}, "Linecode": {}}"#,
        ] {
            assert!(classify_distribution_json(doc).is_err(), "{doc}");
        }
    }

    #[test]
    fn byte_order_mark_is_retained_and_echoed() {
        let dss = "\u{feff}clear\nnew circuit.c basekv=12.47 bus1=src\n";
        let net = crate::testkit::parse_dss_str(dss);
        assert_eq!(net.name().as_deref(), Some("c"));
        assert!(
            !net.warnings.iter().any(|w| w.contains("byte order mark")),
            "retaining the mark is not a loss: {:?}",
            net.warnings
        );
        // The echo returns the input bytes exactly, mark included; the
        // decode slice the reader saw was mark free.
        assert_eq!(net.emit(DistTargetFormat::Dss).text, dss);
        assert!(
            net.source
                .as_ref()
                .is_some_and(|s| !s.starts_with('\u{feff}'))
        );
    }

    #[test]
    fn memory_and_file_sources_parse_the_same_fixture_alike() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../tests/data/dist/micro/onephase_zip_load.dss"
        );
        let bytes = std::fs::read(path).unwrap();
        let from_bytes =
            crate::testkit::parse_str(std::str::from_utf8(&bytes).unwrap(), "dss").unwrap();
        let from_file = crate::testkit::parse_file(path, None).unwrap();
        assert_eq!(from_bytes.buses().len(), from_file.buses().len());
        assert_eq!(from_bytes.loads().len(), from_file.loads().len());
        assert_eq!(from_bytes.source, from_file.source);
    }

    #[test]
    fn non_utf8_bytes_are_refused_and_name_the_encoding() {
        // 0xE9 is CP1252 é, the classic single byte a Windows editor leaves.
        let bytes: &[u8] = b"clear\nnew circuit.caf\xE9 basekv=12.47 bus1=src\n";
        let source = powerio_core::Source::from_memory("<memory>", bytes.to_vec())
            .unwrap()
            .with_format(powerio_core::FormatId::new("dss").unwrap());
        let err = parse(source).unwrap_err();
        assert!(err.to_string().contains("UTF-8"), "{err}");
    }

    #[test]
    fn parse_rejects_unclassifiable_json() {
        // A PowerModels document used to fall through to the BMOPF reader
        // and parse into a bogus near-empty network.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("case.json");
        std::fs::write(
            &path,
            r#"{"bus": {}, "branch": {}, "gen": {}, "baseMVA": 100.0}"#,
        )
        .unwrap();
        let err = crate::testkit::parse_file(&path, None).unwrap_err();
        assert!(
            err.to_string()
                .contains("not a recognized distribution document"),
            "{err}"
        );
        // An explicit format still overrides the classifier.
        assert!(crate::testkit::parse_file(&path, Some("bmopf-json")).is_ok());
    }

    #[test]
    fn unknown_format_names_fail_before_any_work() {
        assert!(matches!(
            crate::testkit::parse_str("", "matpower"),
            Err(crate::Error::UnknownFormat(_))
        ));
        assert!(matches!(
            "matpower".parse::<DistTargetFormat>(),
            Err(crate::Error::UnknownFormat(_))
        ));
        assert!(matches!(
            crate::testkit::parse_file("missing.dss", Some("matpower")),
            Err(crate::Error::UnknownFormat(_))
        ));
    }

    #[test]
    fn parse_diagnostics_remain_on_the_module_when_it_is_emitted() {
        let dss = "clear\nnew circuit.w basekv=12.47 bus1=src\n\
                   new line.l1 bus1=src bus2=b2 length=1 units=furlong\n";
        let source = powerio_core::Source::from_memory("<memory>", dss.as_bytes().to_vec())
            .unwrap()
            .with_format(powerio_core::FormatId::new("dss").unwrap());
        let module = parse(source).unwrap();
        let lines = crate::diagnostics::render_diagnostics(module.diagnostics());
        assert!(
            lines.iter().any(|w| w.contains("furlong")),
            "parse diagnostics stay on PioModule: {lines:?}"
        );
        emit(
            &module,
            DistTargetFormat::BmopfJson,
            powerio_core::Destination::memory("case.json").unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn canonical_format_bypasses_same_format_dss_echo() {
        let src = "Clear\n\
                   New Circuit.c basekv=12.47 bus1=sourcebus\n\
                   New Load.l1 bus1=sourcebus.1 phases=1 conn=wye kv=7.2 kw=10 kvar=2\n";
        let net = crate::testkit::parse_dss_str(src);
        assert_eq!(net.emit(DistTargetFormat::Dss).text, src);

        let canonical = net.emit_value(DistTargetFormat::Dss);
        assert_ne!(canonical.text, src);
        assert!(
            canonical
                .text
                .lines()
                .any(|l| l.contains("Load.l1") && l.contains("vminpu=0")),
            "{}",
            canonical.text
        );
    }
}
