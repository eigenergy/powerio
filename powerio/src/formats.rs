//! Facade format metadata.
//!
//! Component format enums select parsers and emitters inside their owning
//! crates. The facade exposes one small descriptor instead, so applications
//! can name an emitted artifact without depending on those implementation
//! enums or copying their alias tables.

use powerio_tx::format::routing::TransmissionFormat;

/// The canonical identity and destination shape of a PowerIO format.
///
/// `extension` is the conventional filename suffix without a leading dot; it
/// may be compound. It is `None` for directory formats with no primary case
/// file. `can_emit` reports whether a fresh universal emitter
/// exists for the format. It does not promise that every concrete module value can
/// emit that format, and it is not a build feature probe. A false value neither
/// promises nor forbids a same format retained source echo.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[non_exhaustive]
pub struct FormatInfo {
    /// Canonical stable token used by parse and emit operations when the
    /// current build and concrete value support the format.
    pub token: &'static str,
    /// Conventional filename suffix without a leading dot.
    pub extension: Option<&'static str>,
    /// Whether a path destination names an output directory rather than one
    /// file.
    pub is_directory: bool,
    /// Whether a fresh universal emitter exists for this format.
    pub can_emit: bool,
}

const fn info(
    token: &'static str,
    extension: Option<&'static str>,
    is_directory: bool,
    can_emit: bool,
) -> FormatInfo {
    FormatInfo {
        token,
        extension,
        is_directory,
        can_emit,
    }
}

/// Resolve a format token or common alias to facade owned metadata.
///
/// This includes transmission and distribution grid exchange formats and the
/// standalone geographic layer document. PowerIO IR and bare model JSON are
/// not grid exchange formats and therefore are not returned here.
///
#[must_use]
pub fn resolve_format(name: &str) -> Option<FormatInfo> {
    if crate::is_geo_layer_token(name) {
        return Some(info(
            "geo-json",
            Some(powerio_tx::geo::GEO_LAYER_EXTENSION),
            false,
            true,
        ));
    }
    if crate::is_pwd_display_token(name) {
        return Some(info("powerworld-pwd", Some("pwd"), false, false));
    }
    if let Some(format) = powerio_tx::format::parse_target_format(name) {
        let is_cgmes = format == powerio_tx::TargetFormat::Cgmes;
        return Some(info(
            format.token(),
            (!is_cgmes).then_some(format.extension()),
            is_cgmes,
            !matches!(format, powerio_tx::TargetFormat::DeepMindOpfDataJson),
        ));
    }

    if let Some(format) = powerio_dist::parse_dist_target_format(name) {
        return Some(match format.name() {
            "dss" => info("dss", Some("dss"), true, true),
            "pmd-json" => info("pmd-json", Some("json"), false, true),
            "bmopf-json" => info("bmopf-json", Some("json"), false, true),
            _ => return None,
        });
    }

    match powerio_tx::format::routing::parse_transmission_format(name) {
        Some(TransmissionFormat::PypsaCsv) => Some(info("pypsa-csv", None, true, true)),
        Some(TransmissionFormat::Pwb) => Some(info("pwb", Some("pwb"), false, false)),
        Some(TransmissionFormat::Gridfm) => Some(info("gridfm", None, true, true)),
        // The public IEEE archives name their CDF cases `.txt`; the reader also
        // recognizes `.cdf` and any name with the declared format.
        Some(TransmissionFormat::IeeeCdf) => Some(info("ieee-cdf", Some("txt"), false, false)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_resolve_to_one_canonical_descriptor() {
        assert_eq!(resolve_format("m"), resolve_format("MATPOWER"));
        assert_eq!(resolve_format("pm").unwrap().token, "powermodels-json");
        assert_eq!(resolve_format("engineering").unwrap().token, "pmd-json");
        assert_eq!(resolve_format("xiidm").unwrap().token, "xiidm");
        assert_eq!(resolve_format("jiidm").unwrap().token, "jiidm");
        assert_eq!(resolve_format("jiidm").unwrap().extension, Some("jiidm"));
        assert!(resolve_format("cgmes").unwrap().is_directory);
        assert_eq!(resolve_format("iidm"), None);
        assert_eq!(resolve_format("rawx"), None);
        assert_eq!(resolve_format("psse-rawx").unwrap().token, "psse-rawx");
    }

    #[test]
    fn destination_and_read_only_shapes_are_explicit() {
        let dss = resolve_format("opendss").unwrap();
        assert!(dss.is_directory);
        assert!(dss.can_emit);
        assert_eq!(dss.extension, Some("dss"));

        let pypsa = resolve_format("pypsa").unwrap();
        assert!(pypsa.is_directory);
        assert_eq!(pypsa.extension, None);

        let pwb = resolve_format("pwb").unwrap();
        assert!(!pwb.is_directory);
        assert!(!pwb.can_emit);
        assert_eq!(pwb.extension, Some("pwb"));

        let cdf = resolve_format("cdf").unwrap();
        assert_eq!(cdf.token, "ieee-cdf");
        assert!(!cdf.is_directory);
        assert!(!cdf.can_emit);
        assert_eq!(cdf.extension, Some("txt"));
    }

    #[test]
    fn nonformats_do_not_resolve() {
        assert_eq!(resolve_format("not-a-format"), None);
        assert_eq!(resolve_format("json"), None);
        assert_eq!(resolve_format("pio-json"), None);
        assert_eq!(resolve_format("model-json"), None);
    }
}
