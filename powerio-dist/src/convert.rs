//! Cross format conversion output and the format dispatcher.

use crate::model::{DistNetwork, DistSourceFormat};
use powerio_format::DistributionFormat;

/// Text in the target format plus every fidelity loss the writer took.
/// Nothing drops silently: a field the target cannot represent appears
/// here as a warning naming the element and field.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Conversion {
    pub text: String,
    pub warnings: Vec<String>,
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
    match powerio_format::distribution_format_from_name(name)? {
        DistributionFormat::Dss => Some(DistTargetFormat::Dss),
        DistributionFormat::BmopfJson => Some(DistTargetFormat::BmopfJson),
        DistributionFormat::PmdJson => Some(DistTargetFormat::PmdJson),
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

/// Parses `text` in the named format (see [`dist_target_from_name`]).
pub fn parse_str(text: &str, format: &str) -> crate::Result<DistNetwork> {
    match format.parse::<DistTargetFormat>()? {
        DistTargetFormat::Dss => Ok(crate::dss::parse_dss_str(text)),
        DistTargetFormat::BmopfJson => crate::bmopf::parse_bmopf_str(text),
        DistTargetFormat::PmdJson => crate::pmd::parse_pmd_str(text),
    }
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
                return if powerio_format::infer_distribution_json_format(&text)
                    == DistributionFormat::PmdJson
                {
                    crate::pmd::parse_pmd_str(&text)
                } else {
                    crate::bmopf::parse_bmopf_str(&text)
                };
            }
            other => return Err(crate::Error::UnknownFormat(other.to_string())),
        }
    };
    match format {
        DistTargetFormat::Dss => crate::dss::parse_dss_file(path),
        DistTargetFormat::BmopfJson => crate::bmopf::parse_bmopf_str(&read(path)?),
        DistTargetFormat::PmdJson => crate::pmd::parse_pmd_str(&read(path)?),
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
        warnings,
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
                    warnings: Vec::new(),
                };
            }
        }
        match format {
            DistTargetFormat::Dss => crate::dss::write_dss(self),
            DistTargetFormat::BmopfJson => crate::bmopf::write_bmopf_json(self),
            DistTargetFormat::PmdJson => crate::pmd::write_pmd_json(self),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distribution_json_classifier_preserves_pmd_marker_and_bmopf_fallback() {
        assert_eq!(
            powerio_format::infer_distribution_json_format(r#"{"data_model": "ENGINEERING"}"#),
            DistributionFormat::PmdJson
        );
        assert_eq!(
            powerio_format::infer_distribution_json_format(r#"{"bus": {"data_model": {}}}"#),
            DistributionFormat::BmopfJson
        );
        assert_eq!(
            powerio_format::infer_distribution_json_format(r#"{"name": "data_model"}"#),
            DistributionFormat::BmopfJson
        );
        assert_eq!(
            powerio_format::infer_distribution_json_format("{not json"),
            DistributionFormat::BmopfJson
        );
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
}
