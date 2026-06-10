//! Cross format conversion output and the format dispatcher.

use crate::model::{DistNetwork, DistSourceFormat};

/// Text in the target format plus every fidelity loss the writer took.
/// Nothing drops silently: a field the target cannot represent appears
/// here as a warning naming the element and field.
#[derive(Debug, Clone)]
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
    match name.to_ascii_lowercase().as_str() {
        "dss" | "opendss" => Some(DistTargetFormat::Dss),
        "bmopf" | "bmopf-json" | "bmopf_json" => Some(DistTargetFormat::BmopfJson),
        "pmd" | "pmd-json" | "pmd_json" | "engineering" => Some(DistTargetFormat::PmdJson),
        _ => None,
    }
}

/// [`dist_target_from_name`] as a `Result`, for the dispatchers that must
/// reject an unknown name before doing any work.
fn target(name: &str) -> crate::Result<DistTargetFormat> {
    dist_target_from_name(name).ok_or_else(|| crate::Error::UnknownFormat(name.to_string()))
}

fn read(path: &std::path::Path) -> crate::Result<String> {
    std::fs::read_to_string(path).map_err(|source| crate::Error::Io {
        path: path.display().to_string(),
        source,
    })
}

/// PMD ENGINEERING JSON carries a top level `data_model` key; the BMOPF
/// layout has none. Deserializing into an [`IgnoredAny`](serde::de::IgnoredAny)
/// field finds the key at the top level only (a nested or quoted occurrence
/// doesn't count) without building the value tree.
fn is_pmd_json(text: &str) -> bool {
    #[derive(serde::Deserialize)]
    struct Shape {
        data_model: Option<serde::de::IgnoredAny>,
    }
    serde_json::from_str::<Shape>(text).is_ok_and(|s| s.data_model.is_some())
}

/// Parses `text` in the named format (see [`dist_target_from_name`]).
pub fn parse_str(text: &str, format: &str) -> crate::Result<DistNetwork> {
    match target(format)? {
        DistTargetFormat::Dss => Ok(crate::dss::parse_dss_str(text)),
        DistTargetFormat::BmopfJson => crate::bmopf::parse_bmopf_str(text),
        DistTargetFormat::PmdJson => crate::pmd::parse_pmd_str(text),
    }
}

/// Parses `path`, taking the format from `from` when given, the `.dss`
/// extension otherwise, and for `.json` the presence of the top level PMD
/// ENGINEERING `data_model` key against the BMOPF layout.
pub fn parse_file(
    path: impl AsRef<std::path::Path>,
    from: Option<&str>,
) -> crate::Result<DistNetwork> {
    let path = path.as_ref();
    // Dss goes through the path-based parser (Redirect/Compile resolve
    // against the file's directory); the JSON readers take text.
    let format = if let Some(from) = from {
        target(from)?
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
                return if is_pmd_json(&text) {
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

/// Parses `text` as `from` and writes it as `to` in one call. The warnings
/// carry both the parse warnings and the writer's fidelity losses.
pub fn convert_str(text: &str, from: &str, to: &str) -> crate::Result<Conversion> {
    let to = target(to)?;
    Ok(convert(&parse_str(text, from)?, to))
}

/// Parses `path` (format from `from` or the file itself) and writes it as
/// `to` in one call. The warnings carry both the parse warnings and the
/// writer's fidelity losses.
pub fn convert_file(
    path: impl AsRef<std::path::Path>,
    to: &str,
    from: Option<&str>,
) -> crate::Result<Conversion> {
    let to = target(to)?;
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
    /// model and reports each fidelity loss in the warnings.
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
    fn sniff_requires_top_level_data_model() {
        assert!(is_pmd_json(r#"{"data_model": "ENGINEERING"}"#));
        // Nested or quoted occurrences are not the marker.
        assert!(!is_pmd_json(r#"{"bus": {"data_model": {}}}"#));
        assert!(!is_pmd_json(r#"{"name": "data_model"}"#));
        assert!(!is_pmd_json("{not json"));
    }

    #[test]
    fn unknown_format_names_fail_before_any_work() {
        assert!(matches!(
            parse_str("", "matpower"),
            Err(crate::Error::UnknownFormat(_))
        ));
        assert!(matches!(
            convert_str("clear\n", "dss", "matpower"),
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
        let conv = convert_str(dss, "dss", "bmopf").unwrap();
        assert!(
            conv.warnings.iter().any(|w| w.contains("furlong")),
            "parse warnings must surface through the one-shot converter: {:?}",
            conv.warnings
        );
    }
}
