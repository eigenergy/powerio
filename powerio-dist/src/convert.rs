//! Cross format conversion output and the format dispatcher.

use crate::model::{DistNetwork, DistSourceFormat};

/// Extra files a writer generated beside the primary text payload.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ConversionSidecar {
    /// Relative file path the primary text refers to.
    pub path: String,
    /// File content.
    pub text: String,
}

impl ConversionSidecar {
    /// The fidelity warning for a surface that could not write this sidecar,
    /// with `reason` saying why in that surface's terms.
    ///
    /// One formatter on the owning type so every surface — the CLI's stdout
    /// carve-out, the text-only C entry points, any future binding — phrases
    /// the same event the same way. Downstream consumers classify fidelity
    /// losses by matching warning text, so two phrasings of one event means a
    /// classifier keyed on either misses the other.
    #[must_use]
    pub fn dropped_warning(&self, reason: &str) -> String {
        format!(
            "fidelity: sidecar `{}` was not written: {reason}",
            self.path
        )
    }
}

/// Text in the target format plus every fidelity loss the writer took.
/// Nothing drops silently: a field the target cannot represent appears
/// here as a warning naming the element and field.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Conversion {
    pub text: String,
    /// Extra files referenced by `text`, such as OpenDSS `Buscoords` CSV.
    pub sidecars: Vec<ConversionSidecar>,
    pub warnings: Vec<String>,
    /// Structured diagnostics for warning paths with stable codes.
    ///
    /// The legacy `warnings` strings remain the compatibility surface for C,
    /// Python, Julia, and CLI callers. New code should prefer this field when
    /// it needs stable assertions.
    pub diagnostics: Vec<crate::diagnostics::StructuredDiagnostic>,
}

/// A writable distribution format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DistTargetFormat {
    Dss,
    BmopfJson,
    PmdJson,
}

/// Resolves common names and file extensions to a target format.
pub fn dist_target_from_name(name: &str) -> Option<DistTargetFormat> {
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

    /// [`dist_target_from_name`] as a `Result`, matching the transmission
    /// hub's `TargetFormat: FromStr`.
    fn from_str(s: &str) -> crate::Result<Self> {
        dist_target_from_name(s).ok_or_else(|| crate::Error::UnknownFormat(s.to_string()))
    }
}

impl DistTargetFormat {
    /// The canonical format name (`dss`, `pmd-json`, `bmopf-json`), accepted
    /// back by [`dist_target_from_name`].
    pub fn name(self) -> &'static str {
        match self {
            DistTargetFormat::Dss => "dss",
            DistTargetFormat::PmdJson => "pmd-json",
            DistTargetFormat::BmopfJson => "bmopf-json",
        }
    }
}

fn read(path: &std::path::Path) -> crate::Result<String> {
    std::fs::read_to_string(path).map_err(|source| crate::Error::Io {
        path: path.display().to_string(),
        source,
    })
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

/// The warning every parse path pushes when it removes a byte order mark.
pub(crate) const BOM_WARNING: &str =
    "leading UTF-8 byte order mark removed; a same-format write returns the text without it";

/// Parse `text` as `format`, stripping a leading UTF-8 byte order mark first:
/// Windows tooling saves case files with one, and both serde_json and the DSS
/// tokenizer treat it as garbage in the first token. The retained source loses
/// the mark, so a same-format echo differs by exactly those three bytes; the
/// warning itemizes that.
fn parse_text(text: &str, format: DistTargetFormat) -> crate::Result<DistNetwork> {
    let stripped = text.trim_start_matches('\u{feff}');
    let mut net = match format {
        DistTargetFormat::Dss => crate::dss::parse_dss_str(stripped),
        DistTargetFormat::BmopfJson => crate::bmopf::parse_bmopf_str(stripped)?,
        DistTargetFormat::PmdJson => crate::pmd::parse_pmd_str(stripped)?,
    };
    if stripped.len() != text.len() {
        net.warnings.push(BOM_WARNING.to_owned());
    }
    Ok(net)
}

/// Parses `text` in the named format (see [`dist_target_from_name`]).
pub fn parse_str(text: &str, format: &str) -> crate::Result<DistNetwork> {
    parse_text(text, format.parse::<DistTargetFormat>()?)
}

/// Parses `path`, taking the format from `from` when given, the `.dss`
/// extension otherwise, and for `.json` the shared distribution classifier.
pub fn parse_file(
    path: impl AsRef<std::path::Path>,
    from: Option<&str>,
) -> crate::Result<DistNetwork> {
    let path = path.as_ref();
    // Dss goes through the path-based parser (Redirect/Compile resolve
    // against the file's directory); the JSON readers take text.
    let format = if let Some(from) = from {
        from.parse::<DistTargetFormat>()?
    } else {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        match ext.as_str() {
            "dss" => DistTargetFormat::Dss,
            "json" => {
                let text = read(path)?;
                return parse_text(&text, classify_distribution_json(&text)?);
            }
            other => return Err(crate::Error::UnknownFormat(other.to_string())),
        }
    };
    match format {
        DistTargetFormat::Dss => crate::dss::parse_dss_file(path),
        DistTargetFormat::BmopfJson | DistTargetFormat::PmdJson => parse_text(&read(path)?, format),
    }
}

/// Prepend the reader's parse warnings to the writer's fidelity warnings: the
/// one-shot converters return no handle to query, so this is the only place
/// the loud half of the parse can surface.
fn convert(net: &DistNetwork, target: DistTargetFormat) -> Conversion {
    let conv = net.to_format(target);
    let mut warnings = net.warnings.clone();
    warnings.extend(conv.warnings);
    Conversion {
        text: conv.text,
        sidecars: conv.sidecars,
        warnings,
        diagnostics: conv.diagnostics,
    }
}

/// Parses `text` as `format` and writes it as `to` in one call. The warnings
/// carry both the parse warnings and the writer's fidelity losses.
pub fn convert_str(text: &str, to: DistTargetFormat, format: &str) -> crate::Result<Conversion> {
    Ok(convert(&parse_str(text, format)?, to))
}

/// Parses `path` (format from `from` or the file itself) and writes it as
/// `to` in one call. The warnings carry both the parse warnings and the
/// writer's fidelity losses.
pub fn convert_file(
    path: impl AsRef<std::path::Path>,
    to: DistTargetFormat,
    from: Option<&str>,
) -> crate::Result<Conversion> {
    Ok(convert(&parse_file(path, from)?, to))
}

impl DistTargetFormat {
    fn matches(self, source: DistSourceFormat) -> bool {
        matches!(
            (self, source),
            (DistTargetFormat::Dss, DistSourceFormat::Dss)
                | (DistTargetFormat::BmopfJson, DistSourceFormat::BmopfJson)
                | (DistTargetFormat::PmdJson, DistSourceFormat::PmdJson)
        )
    }
}

impl DistNetwork {
    /// Writes the network in `format`, bypassing byte exact source echo.
    pub fn to_canonical_format(&self, format: DistTargetFormat) -> Conversion {
        let mut conv = match format {
            DistTargetFormat::Dss => crate::dss::write_dss(self),
            DistTargetFormat::BmopfJson => crate::bmopf::write_bmopf_json(self),
            DistTargetFormat::PmdJson => crate::pmd::write_pmd_json(self),
        };
        // No distribution format carries line routes; report the loss the
        // way bus locations already do (`.pio.json` keeps them).
        let routed = self
            .lines
            .iter()
            .filter(|line| line.route.is_some())
            .count();
        if routed > 0 {
            conv.warnings.push(format!(
                "{routed} line route(s) dropped: {} has no polyline field",
                format.name()
            ));
        }
        conv
    }

    /// Writes the network in `format`.
    ///
    /// Writing back to the source format echoes the retained source text
    /// byte for byte; every cross format write regenerates from the typed
    /// model and reports each fidelity loss in the warnings. The returned
    /// warnings hold only the writer's losses: parse warnings stay on
    /// [`DistNetwork::warnings`] (the one-shot [`convert_str`]/[`convert_file`]
    /// merge the two). After mutating a parsed model, set `source = None`
    /// (and `source_format`), or the echo tier returns the original text
    /// and silently discards the edits.
    pub fn to_format(&self, format: DistTargetFormat) -> Conversion {
        if let (Some(source), Some(source_format)) = (&self.source, self.source_format) {
            if format.matches(source_format) {
                return Conversion {
                    text: source.as_ref().clone(),
                    sidecars: Vec::new(),
                    warnings: Vec::new(),
                    diagnostics: Vec::new(),
                };
            }
        }
        self.to_canonical_format(format)
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
    /// near-empty `DistNetwork`.
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
            let err = crate::parse_str(&doc, format.name())
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
    fn byte_order_mark_is_stripped_and_warned() {
        let dss = "\u{feff}clear\nnew circuit.c basekv=12.47 bus1=src\n";
        let net = parse_str(dss, "dss").unwrap();
        assert!(
            net.warnings.iter().any(|w| w.contains("byte order mark")),
            "warnings: {:?}",
            net.warnings
        );
        assert!(
            net.source
                .as_ref()
                .is_some_and(|s| !s.starts_with('\u{feff}'))
        );
    }

    #[test]
    fn parse_file_rejects_unclassifiable_json() {
        // A PowerModels document used to fall through to the BMOPF reader
        // and parse into a bogus near-empty network.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("case.json");
        std::fs::write(
            &path,
            r#"{"bus": {}, "branch": {}, "gen": {}, "baseMVA": 100.0}"#,
        )
        .unwrap();
        let err = parse_file(&path, None).unwrap_err();
        assert!(
            err.to_string()
                .contains("not a recognized distribution document"),
            "{err}"
        );
        // An explicit format still overrides the classifier.
        assert!(parse_file(&path, Some("bmopf-json")).is_ok());
    }

    #[test]
    fn unknown_format_names_fail_before_any_work() {
        assert!(matches!(
            parse_str("", "matpower"),
            Err(crate::Error::UnknownFormat(_))
        ));
        assert!(matches!(
            "matpower".parse::<DistTargetFormat>(),
            Err(crate::Error::UnknownFormat(_))
        ));
        assert!(matches!(
            parse_file("missing.dss", Some("matpower")),
            Err(crate::Error::UnknownFormat(_))
        ));
    }

    #[test]
    fn one_shot_convert_carries_parse_warnings() {
        let dss = "clear\nnew circuit.w basekv=12.47 bus1=src\n\
                   new line.l1 bus1=src bus2=b2 length=1 units=furlong\n";
        let conv = convert_str(dss, DistTargetFormat::BmopfJson, "dss").unwrap();
        assert!(
            conv.warnings.iter().any(|w| w.contains("furlong")),
            "parse warnings must surface through the one-shot converter: {:?}",
            conv.warnings
        );
    }

    #[test]
    fn canonical_format_bypasses_same_format_dss_echo() {
        let src = "Clear\n\
                   New Circuit.c basekv=12.47 bus1=sourcebus\n\
                   New Load.l1 bus1=sourcebus.1 phases=1 conn=wye kv=7.2 kw=10 kvar=2\n";
        let net = parse_str(src, "dss").unwrap();
        assert_eq!(net.to_format(DistTargetFormat::Dss).text, src);

        let canonical = net.to_canonical_format(DistTargetFormat::Dss);
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
