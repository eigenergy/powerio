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
