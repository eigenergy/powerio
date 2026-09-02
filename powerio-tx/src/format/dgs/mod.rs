//! DIgSILENT PowerFactory DGS V5 ASCII exports.
//!
//! PowerFactory projects live in encrypted `.pfd` files that only the
//! application decodes. Its DGS export writes the same objects as plain
//! class tables, one row per object, which this module reads:
//! [`tokens`] decodes the tables into an object index, [`route`] decides
//! which network family the objects justify, and [`balanced`] builds the
//! positive sequence [`BalancedNetwork`] when the export carries sequence
//! data only. The conductor level route belongs to the `powerio` facade,
//! which owns both network families.
//!
//! The reference mapping is the PowSybl PowerFactory converter, which reads
//! the same DGS V5 export definitions.

mod balanced;
pub mod route;
pub mod tokens;

pub use balanced::SpanContext;
pub use route::{DgsRoute, route};
pub use tokens::{ClassHeader, DgsDocument, DgsObject, DgsValue, RefKey};

use crate::diagnostics::Diagnostics;
use crate::network::BalancedNetwork;
use crate::{Error, Result};

/// Whether `bytes` open like a DGS export: the first table header is
/// `$$General` or another `$$Class` line after optional comments.
#[must_use]
pub fn looks_like_dgs(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(4096)];
    let text = String::from_utf8_lossy(head);
    text.trim_start_matches('\u{feff}')
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('*'))
        .is_some_and(|line| line.starts_with("$$") && line.contains(';'))
}

/// The refusal for an encrypted `.pfd` project file: the bytes carry no
/// readable tables, and the way out is a DGS export.
#[must_use]
pub fn encrypted_project_message(name: &str) -> String {
    format!(
        "{name} is a DIgSILENT PowerFactory .pfd project file, which is encrypted and \
         readable only by the PowerFactory application; export the study case as \
         DGS V5 ASCII (File > Export > DGS) and read the .dgs file"
    )
}

/// Whether a declared format name means a `.pfd` project file.
#[must_use]
pub fn is_project_name(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().replace(['-', '_'], "").as_str(),
        "pfd" | "powerfactorypfd" | "powerfactoryproject"
    )
}

/// Decode `text` and decide its family without building a network.
///
/// # Errors
/// [`Error::FormatRead`] on malformed text.
pub fn route_text(text: &str) -> Result<DgsRoute> {
    let document = DgsDocument::parse(text)?;
    Ok(route(&document))
}

/// Read a DGS export that carries sequence data only into a balanced
/// network. An export that justifies the multiconductor model is refused
/// with guidance to the facade, which owns that route.
///
/// # Errors
/// [`Error::FormatRead`] on malformed text or an export with no topology;
/// [`Error::UnknownFormat`] when the export needs the multiconductor model.
pub(crate) fn parse_dgs_source(
    text: &str,
    name_hint: Option<&str>,
    warnings: &mut Diagnostics,
    spans: Option<SpanContext>,
) -> Result<BalancedNetwork> {
    let document = DgsDocument::parse(text)?;
    match route(&document) {
        DgsRoute::Balanced => balanced::build_balanced(&document, name_hint, warnings, spans),
        DgsRoute::Multiconductor(markers) => Err(Error::UnknownFormat(format!(
            "the DGS export carries conductor level data ({}), which the balanced \
             transmission parser does not read; parse it through the one module family \
             (`powerio::parse` in Rust and `parse` in the language bindings), which routes \
             it to the multiconductor network",
            summarize_markers(&markers)
        ))),
        DgsRoute::Undecided(reason) => Err(Error::FormatRead {
            format: tokens::FMT,
            message: reason,
        }),
    }
}

/// The first markers and a count of the rest, for one line of guidance.
#[must_use]
pub fn summarize_markers(markers: &[String]) -> String {
    const SHOWN: usize = 3;
    let shown = markers
        .iter()
        .take(SHOWN)
        .cloned()
        .collect::<Vec<_>>()
        .join("; ");
    if markers.len() > SHOWN {
        format!("{shown}; and {} more", markers.len() - SHOWN)
    } else {
        shown
    }
}

/// Build the balanced network of an already decoded document.
///
/// # Errors
/// [`Error::FormatRead`] when the export states no usable terminal.
#[doc(hidden)]
pub fn __build_balanced(
    document: &DgsDocument,
    name_hint: Option<&str>,
    spans: Option<SpanContext>,
) -> Result<(BalancedNetwork, Vec<powerio_core::Diagnostic>)> {
    let mut warnings = Diagnostics::new();
    let network = balanced::build_balanced(document, name_hint, &mut warnings, spans)?;
    Ok((network, warnings.into_records()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_reads_the_first_table_header() {
        assert!(looks_like_dgs(b"$$General;ID(a:40);Descr(a:40);Val(a:40)\n1;Version;5.0\n"));
        assert!(looks_like_dgs(
            b"* exported\n\n$$ElmNet;ID(a:40);loc_name(a:40)\n2;Net\n"
        ));
        assert!(!looks_like_dgs(b"function mpc = case9\n"));
        assert!(!looks_like_dgs(b"{\"bus\": {}}"));
    }

    #[test]
    fn project_names_are_recognized() {
        assert!(is_project_name("pfd"));
        assert!(is_project_name("PowerFactory-PFD"));
        assert!(!is_project_name("dgs"));
    }
}
