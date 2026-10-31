//! MG-RAVENS JSON import and export for the multiconductor
//! [`MulticonductorNetwork`](crate::model::MulticonductorNetwork).
//!
//! MG-RAVENS (<https://github.com/lanl-ansi/MG-RAVENS>, Apache-2.0) is the
//! LANL/Triad microgrid data schema: one JSON document of name-keyed hash
//! tables nested under the CIM class hierarchy, with `Ravens.cimObjectType`
//! naming each object's concrete class and `Class::'name'` strings pointing
//! between tables. Values are SI (volts, watts, ohms, meters) and radians.
//!
//! This is the multiconductor sibling of the balanced MG-RAVENS support in
//! the `powerio-tx` crate: that reader takes the MATPOWER-derived transmission
//! profile and refuses documents with per-phase detail; this one takes the
//! OpenDSS-derived distribution profile — `ACLineSegmentPhase`,
//! `EnergyConsumerPhase`, `PerLengthPhaseImpedance` matrices,
//! `TransformerTank` ends with catalog `TransformerEndInfo` records,
//! `EnergySource` Thevenin sources — and refuses the balanced profile's
//! markers, pointing back. Because it lands in `MulticonductorNetwork`, a
//! RAVENS feeder converts to OpenDSS `.dss`, PowerModelsDistribution
//! ENGINEERING JSON, BMOPF JSON, and distribution CIM through the same
//! [`DistTargetFormat`](crate::DistTargetFormat) machinery as every other
//! distribution source, and any of those convert into RAVENS.
//!
//! The writer emits deterministic UUID-shaped mRIDs derived from class and
//! name (imported mRIDs pass through the `ravens_mrid` extras), so write,
//! read, write is byte stable; a parse retains its source on the module for
//! the byte-exact echo tier.
//!
//! The reader's findings use the `READ.RAVENS_DIST.*` codes and the writer's the
//! `EMIT.RAVENS_DIST.*` codes of [`crate::diagnostics`]. The vocabulary the two
//! profiles share (the profile markers, deterministic mRIDs, and the phase
//! kinds) is [`powerio_core::cim`]; the balanced profile is read and written
//! by `powerio-tx`, and `powerio-tnd` routes a document to its profile.
//!
//! # Phase convention
//!
//! CIM `SinglePhaseKind`/`PhaseCode` letters map to the wire-coordinate
//! terminal names (OpenDSS node numbers) the rest of the crate uses:
//! `A`→`1`, `B`→`2`, `C`→`3`, `N`→`4`, and the split-phase secondary codes
//! `s1`/`s2`→`1`/`2`.

pub(crate) mod read;
mod write;

pub(crate) use write::emit_ravens_json_text;

/// Extras key carrying an imported `IdentifiedObject.mRID`. Prefixed like
/// `pmd_*` so the other writers treat it as converter bookkeeping, not data.
pub(crate) const MRID_KEY: &str = "ravens_mrid";

/// Extras key carrying `Switch.normalOpen` when it differs from the live
/// state (the typed model only has `open`).
pub(crate) const NORMAL_OPEN_KEY: &str = "ravens_normal_open";

/// A deterministic mRID for an element with no imported id: the shared CIM
/// derivation under the MG-RAVENS namespace, the same one the balanced
/// profile writer in `powerio-tx` uses. The upstream converters write a
/// fresh random UUID per run; a name-derived id keeps canonical output
/// idempotent (write, parse, write is byte stable) and diffs meaningful.
/// When the element carries a round-tripped source mRID in its extras, the
/// caller passes that through instead.
pub(crate) fn det_mrid(kind: &str, name: &str) -> String {
    powerio_core::cim::deterministic_mrid(powerio_core::cim::ravens::NAMESPACE, kind, name)
}

/// Split a trailing digit run so hash keys order numerically (`line10` after
/// `line2`); the upstream converters number elements, and hash tables carry
/// no order of their own.
pub(crate) fn natural_key(name: &str) -> (String, u64, String) {
    let stem_len = name.len() - name.bytes().rev().take_while(u8::is_ascii_digit).count();
    let (stem, digits) = name.split_at(stem_len);
    (
        stem.to_owned(),
        digits.parse().unwrap_or(0),
        name.to_owned(),
    )
}

// The phase vocabulary is the shared CIM one, so a terminal named by a BMOPF
// convention (`a`, `b`, `c`, `n`) and one named by an OpenDSS node number
// write the same letters, and the cim-dist writer agrees.
pub(crate) use powerio_core::cim::{expand_phase_code, phase_terminal, terminal_phase};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn det_mrid_is_a_stable_version_5_uuid() {
        let a = det_mrid("ACLineSegment", "ohline");
        assert_eq!(a, det_mrid("ACLineSegment", "ohline"));
        assert_ne!(a, det_mrid("ACLineSegment", "quad"));
        assert_eq!(a.len(), 36);
        assert_eq!(a.as_bytes()[14], b'5', "version nibble: {a}");
    }

    #[test]
    fn natural_keys_order_numerically() {
        let mut names = vec!["line10", "line2", "line1"];
        names.sort_by_key(|n| natural_key(n));
        assert_eq!(names, ["line1", "line2", "line10"]);
    }
}
