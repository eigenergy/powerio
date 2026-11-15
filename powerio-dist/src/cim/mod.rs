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
//! CIM16 documents may state an enumeration as text rather than as a
//! reference, and draw the network in a Diagram Layout; the reader reads
//! both.
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
//! # Transformer forms
//!
//! A `PowerTransformer` states its windings either as `PowerTransformerEnd`
//! records with their own ratings and impedances, or as `TransformerTank`s,
//! each with `TransformerTankEnd`s and catalog data (`TransformerTankInfo`,
//! `TransformerEndInfo`, `ShortCircuitTest`, `NoLoadTest`) that an `Asset`
//! links. Each tank reads as one transformer, with its parent and catalog
//! entry in extras, and is written back as a tank; every other transformer is
//! written in the `PowerTransformerEnd` form. No-load data from either form
//! lands in the `%noloadloss` and `%imag` extras.
//!
//! [`MulticonductorNetwork`]: crate::MulticonductorNetwork

mod read;
pub(crate) mod wire_info;
mod write;
mod xml;

use std::f64::consts::PI;

pub(crate) use read::parse_cim_collecting;
pub(crate) use write::emit_cim_dist_text;

/// Per phase and distribution only classes: CGMES never uses them, so one of
/// them in a header-less CIM document marks distribution CIM. The IEC 61968
/// customer and metering classes (`ServiceLocation`, `UsagePoint`) mark a
/// distribution feeder stated with sequence impedances and no per phase
/// class.
const DISTRIBUTION_MARKERS: [&[u8]; 12] = [
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
    b"ServiceLocation",
    b"UsagePoint",
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

/// The standard CIM leakage magnitude. The alternative is test-only evidence
/// of why producer-specific X values cannot be used as standard magnitudes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LeakageReading {
    Magnitude,
    #[cfg(test)]
    Reactance,
}

const LEAKAGE_READING: LeakageReading = LeakageReading::Magnitude;

/// The phase clock of winding `winding` against the first, as CIM states it
/// (`phaseAngleClock`: the thirty degree steps by which the end lags end
/// 1). A polyphase unit's clock is its vector group shift
/// ([`crate::transformer::calc_winding_phase_shift`], the delta orientation
/// the physics stamps), so a delta–wye pair is clock 1 when the high voltage
/// winding leads (OpenDSS's default `leadlag`) and clock 11 when the low
/// voltage winding leads (`leadlag` `lead` or `euro`). A single phase
/// winding whose neutral pair is reversed against the first is clock 6. The
/// CIM and MG-RAVENS readers and writers all check and write clocks through
/// this.
///
/// # Errors
/// What [`crate::transformer::calc_winding_phase_shift`] refuses: a winding
/// outside the unit or an unknown `leadlag` convention.
pub(crate) fn tank_phase_clock(
    transformer: &crate::DistTransformer,
    winding: usize,
    three_phase: bool,
) -> Result<f64, powerio_core::Error> {
    if three_phase {
        let shift = crate::transformer::calc_winding_phase_shift(transformer, winding)?;
        return Ok((shift / 30.0).rem_euclid(12.0));
    }
    let neutral_first = |w: Option<&crate::DistWinding>| {
        w.and_then(|w| w.terminal_map.first())
            .is_some_and(|t| t == "4")
    };
    if neutral_first(transformer.windings.first())
        == neutral_first(transformer.windings.get(winding))
    {
        Ok(0.0)
    } else {
        Ok(6.0)
    }
}

/// Refuse a network holding a transformer whose phase clocks are unknown
/// (an unknown `leadlag` convention on a delta–wye unit): the CIM and
/// MG-RAVENS writers state every winding's clock, and dropping the unit
/// would disconnect the feeder.
pub(crate) fn require_phase_clocks(
    net: &crate::MulticonductorNetwork,
) -> Result<(), powerio_core::Error> {
    for transformer in net.transformers() {
        for winding in 1..transformer.windings.len() {
            crate::transformer::calc_winding_phase_shift(transformer, winding)?;
        }
    }
    Ok(())
}

/// A stated clock the winding layout cannot represent under either
/// `leadlag` convention.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ClockMismatch {
    /// The winding, zero based.
    pub winding: usize,
    /// The clock the source states.
    pub stated: f64,
    /// The clock the layout represents under OpenDSS's default convention.
    pub represented: f64,
}

/// The `leadlag` convention a tank's stated clocks need ([`tank_phase_clock`]):
/// `None` when the default (the high voltage winding leads) represents
/// every stated clock, `Some("euro")` when only the European convention
/// does, else the first clock neither represents. An absent clock takes the
/// represented one.
#[allow(clippy::float_cmp)] // clock numbers are discrete
pub(crate) fn clocks_leadlag(
    windings: &[crate::DistWinding],
    stated: &[Option<f64>],
    three_phase: bool,
) -> Result<Option<&'static str>, ClockMismatch> {
    let mut probe = crate::DistTransformer::new(
        "",
        windings.to_vec(),
        Vec::new(),
        if three_phase { 3 } else { 1 },
    );
    let mismatch = |probe: &crate::DistTransformer| {
        stated.iter().enumerate().find_map(|(winding, clock)| {
            let clock = (*clock)?;
            // The probe states no unknown convention, so the clock exists.
            let represented = tank_phase_clock(probe, winding, three_phase).unwrap_or(f64::NAN);
            (clock != represented).then_some(ClockMismatch {
                winding,
                stated: clock,
                represented,
            })
        })
    };
    let Some(default) = mismatch(&probe) else {
        return Ok(None);
    };
    probe.extras.insert("leadlag".into(), "euro".into());
    match mismatch(&probe) {
        None => Ok(Some("euro")),
        Some(_) => Err(default),
    }
}

/// One `ShortCircuitTest` as percentages on winding 1's power base.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ShortCircuitPct {
    /// The pair reactance.
    x: f64,
    /// The pair resistance from the load loss.
    r: f64,
}

/// A short circuit test's leakage impedance (ohm, referred to the energised
/// end of rated voltage `rated_u`) and load loss (kW), on a test base of
/// `base_power` VA, as percentages on winding 1's rating `s_1`. `None` when
/// a voltage or power is not positive.
fn short_circuit_test_pct(
    leakage: f64,
    loss_kw: f64,
    rated_u: f64,
    base_power: f64,
    s_1: f64,
    reading: LeakageReading,
) -> Option<ShortCircuitPct> {
    let base = crate::electrical::calc_impedance_base(rated_u, base_power)?;
    if s_1 <= 0.0 {
        return None;
    }
    let z = 100.0 * leakage / base;
    let r = 100.0 * 1000.0 * loss_kw / base_power;
    let x = match reading {
        LeakageReading::Magnitude => (z * z - r * r).max(0.0).sqrt(),
        #[cfg(test)]
        LeakageReading::Reactance => z,
    };
    let scale = s_1 / base_power;
    Some(ShortCircuitPct {
        x: x * scale,
        r: r * scale,
    })
}

/// A no-load test's loss (kW) and exciting current (percent of rated
/// current at `base_power`) as the OpenDSS `(%noloadloss, %imag)` on winding
/// 1's rating `s_1`. `None` when a power is not positive.
///
/// The one reading of a `NoLoadTest`, for distribution CIM and MG-RAVENS
/// alike. CIM100 (and the MG-RAVENS profile built on it) types
/// `NoLoadTest.loss` as `KiloActivePower`, so the loss is in kW, and
/// `excitingCurrent` is the whole exciting current, whose in-phase part is
/// the core loss: OpenDSS's magnetizing `%imag` is the quadrature part,
/// `sqrt(iexc² − loss²)`, as the MG-RAVENS exporter writes
/// `excitingCurrent = sqrt(%imag² + %noloadloss²)`. A loss above the
/// exciting current keeps the exciting current.
pub(crate) fn no_load_test_pct(
    loss_kw: f64,
    exciting: f64,
    base_power: f64,
    s_1: f64,
) -> Option<(f64, f64)> {
    if base_power <= 0.0 || s_1 <= 0.0 {
        return None;
    }
    let loss = 100.0 * 1000.0 * loss_kw / s_1;
    let iexc = exciting * base_power / s_1;
    let imag = if loss > 0.0 && loss <= iexc {
        (iexc * iexc - loss * loss).sqrt()
    } else {
        iexc
    };
    Some((loss, imag))
}

/// The `NoLoadTest` (loss in kW, exciting current in percent) on a test
/// base of winding 1's rating `s_1` that [`no_load_test_pct`] reads back as
/// `(loss_pct, imag)` exactly: the inverse both writers state.
pub(crate) fn no_load_test_values(loss_pct: f64, imag: f64, s_1: f64) -> (f64, f64) {
    let loss = exact_preimage(loss_pct, loss_pct * s_1 / 100.0 / 1000.0, |l| {
        no_load_test_pct(l, 0.0, s_1, s_1).map_or(f64::NAN, |p| p.0)
    });
    let read_loss = no_load_test_pct(loss, 0.0, s_1, s_1).map_or(0.0, |p| p.0);
    let forward = if read_loss > 0.0 {
        imag.hypot(read_loss)
    } else {
        imag
    };
    let exciting = exact_preimage(imag, forward, |e| {
        no_load_test_pct(loss, e, s_1, s_1).map_or(f64::NAN, |p| p.1)
    });
    (loss, exciting)
}

/// A value near `forward` that the reader's `inverse` maps back to `target`
/// exactly, so a write, read, write cycle is byte stable. Unit conversions
/// round, so the plain forward value can come back one bit away; the search
/// steps outward a float at a time and keeps `forward` when no nearby value
/// is exact.
#[allow(clippy::float_cmp)] // exact equality is the property sought
pub(crate) fn exact_preimage(target: f64, forward: f64, inverse: impl Fn(f64) -> f64) -> f64 {
    if !forward.is_finite() || inverse(forward) == target {
        return forward;
    }
    let (mut below, mut above) = (forward, forward);
    for _ in 0..32 {
        below = below.next_down();
        above = above.next_up();
        if inverse(below) == target {
            return below;
        }
        if inverse(above) == target {
            return above;
        }
    }
    forward
}

#[cfg(test)]
mod tests {
    use std::f64::consts::PI;

    use super::{
        LeakageReading, is_case_document, looks_like_cim_dist, no_load_test_pct,
        short_circuit_test_pct, wrap_angle,
    };

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
    }

    /// The IEEE 13 center tap tank `ct5` (2400/120/120 V, 5 kVA): its tests
    /// state 23.5008, 23.5008 and 0.039168 ohm, exactly 2.04%, 2.04% and
    /// 1.36% of the energised end's base, and only the 1 to 2 test states a
    /// loss (0.09 kW, 1.8%). Read as a magnitude the 1 to 2 pair loses its
    /// resistive part; read as a reactance every pair keeps the source value.
    #[test]
    fn leakage_impedance_interpretations() {
        let tests = [
            (23.5008, 0.09, 2400.0),
            (23.5008, 0.0, 2400.0),
            (0.039_168, 0.0, 120.0),
        ];
        let read = |reading| -> Vec<f64> {
            tests
                .iter()
                .map(|&(z, loss, u)| {
                    short_circuit_test_pct(z, loss, u, 5000.0, 5000.0, reading)
                        .unwrap()
                        .x
                })
                .collect()
        };
        let magnitude = read(LeakageReading::Magnitude);
        assert!(close(
            magnitude[0],
            (2.04_f64.powi(2) - 1.8_f64.powi(2)).sqrt()
        ));
        assert!(close(magnitude[0], 0.96));
        assert!(close(magnitude[1], 2.04));
        assert!(close(magnitude[2], 1.36));
        let reactance = read(LeakageReading::Reactance);
        assert!(close(reactance[0], 2.04));
        assert!(close(reactance[1], 2.04));
        assert!(close(reactance[2], 1.36));
        // The resistance comes from the loss either way.
        let pair = short_circuit_test_pct(
            23.5008,
            0.09,
            2400.0,
            5000.0,
            5000.0,
            LeakageReading::Reactance,
        )
        .unwrap();
        assert!(close(pair.r, 1.8));
    }

    /// The test base rescales to winding 1's rating, and a magnitude smaller
    /// than its resistive part reads as no reactance.
    #[test]
    fn short_circuit_tests_rescale_and_flag_short_magnitudes() {
        // 2.04% on a 5 kVA test base is 4.08% on a 10 kVA winding 1.
        let pct = short_circuit_test_pct(
            23.5008,
            0.0,
            2400.0,
            5000.0,
            10_000.0,
            LeakageReading::Magnitude,
        )
        .unwrap();
        assert!(close(pct.x, 4.08));
        let short =
            short_circuit_test_pct(1.0, 0.09, 2400.0, 5000.0, 5000.0, LeakageReading::Magnitude)
                .unwrap();
        assert!(close(short.x, 0.0));
        assert!(
            short_circuit_test_pct(1.0, 0.0, 0.0, 5000.0, 5000.0, LeakageReading::Magnitude)
                .is_none()
        );
    }

    /// 25 kVA with 0.05 kW of no-load loss and 0.538516% exciting current is
    /// 0.2% loss and 0.5% magnetizing current.
    #[test]
    fn no_load_tests_become_percentages() {
        let (loss, imag) = no_load_test_pct(0.05, 0.538_516, 25_000.0, 25_000.0).unwrap();
        assert!(close(loss, 0.2));
        assert!((imag - 0.5).abs() < 1e-6);
        // A loss above the exciting current keeps the exciting current.
        let (_, imag) = no_load_test_pct(0.05, 0.1, 25_000.0, 25_000.0).unwrap();
        assert!(close(imag, 0.1));
        // A test base other than winding 1's rating rescales the current.
        let (_, imag) = no_load_test_pct(0.0, 0.5, 5000.0, 10_000.0).unwrap();
        assert!(close(imag, 0.25));
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn sequence_expands_to_a_symmetric_phase_matrix() {
        let m = crate::sequence::calc_phase_from_sequence(
            3,
            3.0,
            6.0,
            crate::sequence::SequenceThird::Divide,
        );
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
    fn a_feeder_with_usage_points_is_distribution_cim() {
        let cim16 = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:cim="http://iec.ch/TC57/2013/CIM-schema-cim16#">"#;
        let doc = format!(
            "{cim16}<cim:ACLineSegment rdf:ID=\"_l\"/><cim:UsagePoint rdf:ID=\"_u\"/></rdf:RDF>"
        );
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
