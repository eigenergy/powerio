//! Distribution CIM: IEC 61968 and 61970 CIM100 RDF/XML for unbalanced
//! feeders, read into and written from the [`MulticonductorNetwork`]
//! (format token `cim-dist`).
//!
//! The profile is the one the GridAPPS-D and CIMHub tools exchange: one
//! RDF/XML document in the `http://iec.ch/TC57/CIM100#` namespace (the
//! iec61970cim17v40 and iec61968cim13v13 release CGMES 3.0 also uses), with
//! `ConnectivityNode` connectivity and per phase equipment
//! (`ACLineSegmentPhase`, `EnergyConsumerPhase`, `SwitchPhase`, …). The
//! reader also accepts the older versioned `CIM-schema-cimNN` namespaces.
//! A directory of `.xml` documents reads as one case, with `rdf:about`
//! objects extending the ones another document defines.
//!
//! Same format emission returns the retained source bytes of a single
//! document. Canonical output is one CIM100 document with deterministic
//! mRIDs: imported ids pass through the `cim_mrid` extras, and everything
//! else takes a UUIDv5, so write, read, write is byte stable.
//!
//! # Phase convention
//!
//! CIM `SinglePhaseKind` values map to the wire coordinate terminal names
//! (OpenDSS node numbers) the rest of the crate uses: `A`→`1`, `B`→`2`,
//! `C`→`3`, `N`→`4`, and the split phase `s1`/`s2`→`1`/`2`. A conductor
//! beyond those four has no `SinglePhaseKind` and is reported on output.
//!
//! [`MulticonductorNetwork`]: crate::MulticonductorNetwork

mod read;
mod write;
mod xml;

use std::f64::consts::PI;

use crate::model::ConductorMatrix;

pub(crate) use read::parse_cim_collecting;
pub(crate) use write::emit_cim_dist_text;

/// Per phase and distribution only classes: CGMES never uses them, so one of
/// them in a header-less CIM document marks distribution CIM.
const DISTRIBUTION_MARKERS: [&[u8]; 10] = [
    b"ACLineSegmentPhase",
    b"EnergyConsumerPhase",
    b"EnergySourcePhase",
    b"SwitchPhase",
    b"ShuntCompensatorPhase",
    b"PerLengthPhaseImpedance",
    b"PhaseImpedanceData",
    b"TransformerTank",
    b"PowerElectronicsConnectionPhase",
    b"ConcentricNeutralCableInfo",
];

/// How much of a document the header checks read.
pub const CIM_DIST_HEAD_BYTES: usize = 16 * 1024;

/// Whether a document opens like distribution CIM: RDF/XML in a CIM namespace
/// with no `md:FullModel` header (which marks a CGMES profile) in its first
/// [`CIM_DIST_HEAD_BYTES`]. A caller holding only a file's head can rule it
/// out with this before reading the whole file for [`looks_like_cim_dist`].
#[must_use]
pub fn looks_like_cim_dist_header(head: &[u8]) -> bool {
    let head = &head[..head.len().min(CIM_DIST_HEAD_BYTES)];
    has(head, b"22-rdf-syntax-ns#")
        && (has(head, b"CIM100#") || has(head, b"CIM-schema-cim"))
        && !has(head, b"FullModel")
}

/// Whether a document reads as distribution CIM: its header passes
/// [`looks_like_cim_dist_header`], and it holds at least one per phase or
/// distribution only class anywhere.
///
/// The check scans bytes, so it never requires the document to be UTF-8.
#[must_use]
pub fn looks_like_cim_dist(bytes: &[u8]) -> bool {
    looks_like_cim_dist_header(bytes)
        && DISTRIBUTION_MARKERS.iter().any(|marker| has(bytes, marker))
}

fn has(haystack: &[u8], needle: &[u8]) -> bool {
    memchr::memmem::find(haystack, needle).is_some()
}

/// Whether a directory entry is a document of the case: an `.xml` file at
/// the directory's top level. Subdirectories (archives, schemas, other
/// cases) are not part of it.
fn is_case_document(entry: &str) -> bool {
    !entry.contains('/')
        && std::path::Path::new(entry)
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("xml"))
}

/// Whether a source holds distribution CIM: its own buffer reads as
/// distribution CIM ([`looks_like_cim_dist`]), or, for a directory, one of the
/// `.xml` documents at its top level does. The reads go through the source,
/// so they count against its acquisition budget and are cached for the parse
/// that follows.
#[must_use]
pub fn source_looks_like_cim_dist(source: &powerio_core::Source) -> bool {
    if !source.is_directory() {
        return source
            .primary_buffer()
            .is_ok_and(|buffer| looks_like_cim_dist(buffer.content_bytes()));
    }
    let Ok(entries) = source.entry_names() else {
        return false;
    };
    entries.iter().any(|entry| {
        is_case_document(entry.as_str())
            && source
                .root_buffer(entry.as_str())
                .is_ok_and(|buffer| looks_like_cim_dist(buffer.content_bytes()))
    })
}

// The reader and the writer share these so the writer computes exactly what
// the reader does: write, read, write stays byte stable.

/// Impedance base `V²/S` (ohm), or None when degenerate.
fn z_base(v_ref: f64, s_rating: f64) -> Option<f64> {
    (v_ref > 0.0 && s_rating > 0.0).then_some(v_ref * v_ref / s_rating)
}

/// Expand zero and positive sequence scalars into an `n×n` phase matrix:
/// self `(z0 + 2·z1)/3` on the diagonal, mutual `(z0 − z1)/3` off it.
fn sequence_to_phase(z0: f64, z1: f64, n: usize) -> ConductorMatrix {
    let self_z = (z0 + 2.0 * z1) / 3.0;
    let mutual = (z0 - z1) / 3.0;
    let mut m = vec![vec![mutual; n]; n];
    for (i, row) in m.iter_mut().enumerate() {
        row[i] = self_z;
    }
    m
}

/// An angle in radians wrapped to `(-π, π]`.
fn wrap_angle(angle: f64) -> f64 {
    let mut a = angle % (2.0 * PI);
    if a <= -PI {
        a += 2.0 * PI;
    } else if a > PI {
        a -= 2.0 * PI;
    }
    a
}

#[cfg(test)]
mod tests {
    use std::f64::consts::PI;

    use super::{is_case_document, looks_like_cim_dist, sequence_to_phase, wrap_angle};

    #[test]
    #[allow(clippy::float_cmp)]
    fn sequence_expands_to_a_symmetric_phase_matrix() {
        let m = sequence_to_phase(6.0, 3.0, 3);
        // self = (6 + 6)/3 = 4, mutual = (6 - 3)/3 = 1.
        assert_eq!(m[0][0], 4.0);
        assert_eq!(m[0][1], 1.0);
        assert_eq!(m[1][0], 1.0);
    }

    #[test]
    fn angles_wrap_into_the_half_open_interval() {
        assert!((wrap_angle(-4.0 * PI / 3.0) - 2.0 * PI / 3.0).abs() < 1e-12);
        assert!((wrap_angle(PI) - PI).abs() < 1e-12);
        assert!((wrap_angle(-PI) - PI).abs() < 1e-12);
    }

    #[test]
    fn a_case_reads_only_the_top_level_xml_documents() {
        assert!(is_case_document("feeder.xml"));
        assert!(is_case_document("feeder.XML"));
        assert!(!is_case_document("archive/feeder.xml"));
        assert!(!is_case_document("notes.txt"));
    }

    const RDF: &str = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:cim="http://iec.ch/TC57/CIM100#">"#;

    #[test]
    fn a_distribution_document_is_recognized_by_its_per_phase_classes() {
        let doc = format!("{RDF}<cim:ACLineSegmentPhase rdf:ID=\"_p\"/></rdf:RDF>");
        assert!(looks_like_cim_dist(doc.as_bytes()));
    }

    #[test]
    fn a_cgmes_profile_or_a_document_without_markers_is_not() {
        let header = format!(
            "{RDF}<md:FullModel rdf:about=\"urn:uuid:1\"/><cim:ACLineSegmentPhase rdf:ID=\"_p\"/></rdf:RDF>"
        );
        assert!(!looks_like_cim_dist(header.as_bytes()));
        let balanced = format!("{RDF}<cim:ACLineSegment rdf:ID=\"_l\"/></rdf:RDF>");
        assert!(!looks_like_cim_dist(balanced.as_bytes()));
        assert!(!looks_like_cim_dist(
            b"<network xmlns=\"http://www.powsybl.org/schema/iidm/1_0\"/>"
        ));
    }
}
