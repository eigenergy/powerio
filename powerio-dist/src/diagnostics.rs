//! The codes this crate emits.
//!
//! The record itself lives in `powerio-diag`, below this crate and below the
//! `.pio.json` document model, so a distribution finding reaches a package
//! without a translation step. What lives here is the distribution side
//! registry: one [`DiagnosticInfo`] per code, declared once, so an emission
//! site names an entry rather than a loose string.
//!
//! Codes are families, not one per site: what differs between two sites of a
//! family is which object or property it was, which belongs in `details`.

pub use powerio_diag::{
    DiagnosticCode, DiagnosticInfo, DiagnosticSeverity, DiagnosticStage, Diagnostics, SourceRef,
    StructuredDiagnostic, check_registry, render_line, render_lines,
};

pub mod codes {
    powerio_diag::diagnostic_codes! {
        // PARSE: the source text could not be decoded as given.
        PARSE_DSS_BOM_STRIPPED = "PARSE.DSS.BOM_STRIPPED", Info,
            "a leading UTF-8 byte order mark was removed before the reader ran";
        PARSE_DSS_SOURCE_MALFORMED = "PARSE.DSS.SOURCE_MALFORMED", Warning,
            "a dss command, object spec, or property assignment does not parse";
        PARSE_DIST_MALFORMED = "PARSE.DIST.MALFORMED", Fatal,
            "a distribution document is not valid JSON for its format", category = Parse;
        PARSE_DIST_SOURCE_MALFORMED = "PARSE.DIST.SOURCE_MALFORMED", Fatal,
            "a distribution reader refused the source it was given", category = Parse;

        // READ.DSS: decoded, but not representable in the multiconductor model.
        /// A `Redirect`/`Compile`/`Buscoords` include the reader refused
        /// because it escapes the case directory. Severity `Error`: the parse
        /// continued, but the network is incomplete.
        READ_DSS_INCLUDE_REFUSED = "READ.DSS.INCLUDE_REFUSED", Error,
            "an include escaping the case directory was refused";
        /// The reader stopped following includes because the case exceeded the
        /// include budget. Severity `Error` for the same reason.
        READ_DSS_INCLUDE_BUDGET = "READ.DSS.INCLUDE_BUDGET", Error,
            "the reader stopped following includes at the case's include budget";
        READ_DSS_VALUE_CLAMPED = "READ.DSS.VALUE_CLAMPED", Warning,
            "a count or dimension beyond the supported maximum was clamped";
        READ_DSS_VALUE_DEFAULTED = "READ.DSS.VALUE_DEFAULTED", Warning,
            "a value the model needs was absent or unusable and was defaulted";
        READ_DSS_VALUE_UNSUPPORTED = "READ.DSS.VALUE_UNSUPPORTED", Warning,
            "a property value outside the modeled set was read as the nearest one";
        READ_DSS_OBJECT_UNTYPED = "READ.DSS.OBJECT_UNTYPED", Warning,
            "an object shape the model does not type yet is kept untyped";
        READ_DSS_PROPERTY_UNKNOWN = "READ.DSS.PROPERTY_UNKNOWN", Warning,
            "a property this reader does not model is kept as written";
        READ_DSS_REFERENCE_DROPPED = "READ.DSS.REFERENCE_DROPPED", Warning,
            "a control or element reference names an object the case does not declare";
        READ_DSS_RETAINED_SOURCE_ONLY = "READ.DSS.RETAINED_SOURCE_ONLY", Warning,
            "a field survives in extras or the retained source rather than in a typed field";
        READ_DSS_COORDINATE_SPACE_UNKNOWN = "READ.DSS.COORDINATE_SPACE_UNKNOWN", Info,
            "buscoords declare no coordinate reference system";
        READ_DSS_INCLUDE_LOAD_FAILED = "READ.DSS.INCLUDE_LOAD_FAILED", Warning,
            "an include the case names could not be loaded";
        READ_DSS_INCLUDE_DEPTH_LIMIT = "READ.DSS.INCLUDE_DEPTH_LIMIT", Warning,
            "the reader stopped following includes at the nesting depth limit";
        READ_DSS_LINECODE_UNKNOWN = "READ.DSS.LINECODE_UNKNOWN", Warning,
            "a line names a linecode the case does not declare";

        // EMIT.DSS: what the canonical dss writer cannot state.
        EMIT_DSS_FIELD_DROPPED = "EMIT.DSS.FIELD_DROPPED", Warning,
            "a field the dss object model has no property for was dropped";
        EMIT_DSS_RECORD_DROPPED = "EMIT.DSS.RECORD_DROPPED", Warning,
            "an element the dss writer does not emit was dropped";
        EMIT_DSS_VALUE_COLLAPSED = "EMIT.DSS.VALUE_COLLAPSED", Warning,
            "structure was reduced to what one dss property can carry";
        EMIT_DSS_VALUE_DEFAULTED = "EMIT.DSS.VALUE_DEFAULTED", Warning,
            "a value the dss engine requires was synthesized";
        EMIT_DSS_VALUE_SUBSTITUTED = "EMIT.DSS.VALUE_SUBSTITUTED", Warning,
            "a stated value was replaced by one the dss engine reads back the same way";
        EMIT_DSS_EXTRAS_DROPPED = "EMIT.DSS.EXTRAS_DROPPED", Warning,
            "a passthrough extra the canonical dss writer does not regenerate was dropped";

        // READ.PMD.
        READ_PMD_FIELD_DROPPED = "READ.PMD.FIELD_DROPPED", Warning,
            "a PMD field the multiconductor model cannot state was dropped";
        READ_PMD_VALUE_CLAMPED = "READ.PMD.VALUE_CLAMPED", Warning,
            "a PMD dimension beyond the supported maximum was clamped";
        READ_PMD_VALUE_COLLAPSED = "READ.PMD.VALUE_COLLAPSED", Warning,
            "a per terminal or per phase PMD value was collapsed to one entry";
        READ_PMD_VALUE_DEFAULTED = "READ.PMD.VALUE_DEFAULTED", Warning,
            "a PMD value the model needs was absent or unusable and was defaulted";
        READ_PMD_RETAINED_SOURCE_ONLY = "READ.PMD.RETAINED_SOURCE_ONLY", Warning,
            "a PMD field survives in extras rather than in a typed field";
        READ_PMD_SOURCE_MALFORMED = "READ.PMD.SOURCE_MALFORMED", Warning,
            "a PMD value is not the shape its key declares";
        READ_PMD_RECORD_DROPPED = "READ.PMD.RECORD_DROPPED", Warning,
            "a PMD object beyond the modeled set was dropped";
        READ_PMD_VALUE_INLINED = "READ.PMD.VALUE_INLINED", Info,
            "an inline PMD impedance was materialized as a named linecode";

        // EMIT.PMD.
        EMIT_PMD_FIELD_DROPPED = "EMIT.PMD.FIELD_DROPPED", Warning,
            "a field the PMD schema has no key for was dropped";
        EMIT_PMD_RECORD_DROPPED = "EMIT.PMD.RECORD_DROPPED", Warning,
            "an element the ENGINEERING document does not model was dropped";
        EMIT_PMD_VALUE_CLAMPED = "EMIT.PMD.VALUE_CLAMPED", Warning,
            "a conductor count beyond the supported maximum was clamped";
        EMIT_PMD_VALUE_DEFAULTED = "EMIT.PMD.VALUE_DEFAULTED", Warning,
            "a value the ENGINEERING schema requires was synthesized";
        EMIT_PMD_VALUE_SUBSTITUTED = "EMIT.PMD.VALUE_SUBSTITUTED", Warning,
            "a stated value was replaced by one the PMD schema can hold";

        // READ.BMOPF.
        /// A BMOPF field the schema types as a number holds something else.
        /// Severity `Error`: the field reads as `NaN`, which serializes on as
        /// an unbounded limit, so the parse states a fact the source never gave.
        READ_BMOPF_FIELD_NOT_A_NUMBER = "READ.BMOPF.FIELD_NOT_A_NUMBER", Error,
            "a BMOPF field the schema types as a number holds something else";
        READ_BMOPF_FIELD_DROPPED = "READ.BMOPF.FIELD_DROPPED", Warning,
            "a BMOPF field with no canonical home was dropped";
        READ_BMOPF_RECORD_DROPPED = "READ.BMOPF.RECORD_DROPPED", Warning,
            "a BMOPF object or winding beyond the modeled set was dropped";
        READ_BMOPF_VALUE_COLLAPSED = "READ.BMOPF.VALUE_COLLAPSED", Warning,
            "a per phase or per terminal BMOPF value was collapsed to one entry";
        READ_BMOPF_VALUE_DEFAULTED = "READ.BMOPF.VALUE_DEFAULTED", Warning,
            "a BMOPF value the model needs was absent or unusable and was defaulted";
        READ_BMOPF_VALUE_UNSUPPORTED = "READ.BMOPF.VALUE_UNSUPPORTED", Warning,
            "a BMOPF enumeration value outside the schema was read as the nearest one";
        READ_BMOPF_VALUE_INFERRED = "READ.BMOPF.VALUE_INFERRED", Warning,
            "a BMOPF structure the schema does not name was reconstructed";
        READ_BMOPF_RETAINED_SOURCE_ONLY = "READ.BMOPF.RETAINED_SOURCE_ONLY", Warning,
            "a BMOPF field outside the schema survives in extras or untyped";
        READ_BMOPF_SOURCE_MALFORMED = "READ.BMOPF.SOURCE_MALFORMED", Warning,
            "a BMOPF value is not the shape its key declares";

        // EMIT.BMOPF: the general families beside the nineteen transformer
        // codes the writer already publishes.
        EMIT_BMOPF_FIELD_DROPPED = "EMIT.BMOPF.FIELD_DROPPED", Warning,
            "a field the BMOPF schema has no slot for was dropped";
        EMIT_BMOPF_RECORD_DROPPED = "EMIT.BMOPF.RECORD_DROPPED", Warning,
            "an object the emitted BMOPF document does not reference was dropped";
        EMIT_BMOPF_VALUE_DEFAULTED = "EMIT.BMOPF.VALUE_DEFAULTED", Warning,
            "a value the BMOPF schema requires was synthesized";
        EMIT_BMOPF_RETAINED_SOURCE_ONLY = "EMIT.BMOPF.RETAINED_SOURCE_ONLY", Warning,
            "a field with no schema slot was written under extras";
        EMIT_BMOPF_VALUE_CLAMPED = "EMIT.BMOPF.VALUE_CLAMPED", Warning,
            "a matrix dimension beyond the supported maximum was clamped";
        EMIT_BMOPF_VALUE_SUBSTITUTED = "EMIT.BMOPF.VALUE_SUBSTITUTED", Warning,
            "a stated value was replaced by one the BMOPF schema can hold";
        EMIT_BMOPF_SOURCE_COUNT = "EMIT.BMOPF.SOURCE_COUNT", Warning,
            "the BMOPF formulation expects exactly one voltage source";


        // EMIT.BMOPF: the nineteen codes the writer already publishes, now
        // registered entries rather than loose string literals.
        EMIT_BMOPF_AUTOTRANSFORMER_DROPPED = "EMIT.BMOPF.AUTOTRANSFORMER_DROPPED", Warning,
            "an autotransformer the BMOPF schema cannot state was dropped";
        EMIT_BMOPF_BUS_LOCATION_DROPPED = "EMIT.BMOPF.BUS_LOCATION_DROPPED", Warning,
            "a bus location the BMOPF schema cannot state was dropped";
        EMIT_BMOPF_REGCONTROL_DROPPED = "EMIT.BMOPF.REGCONTROL_DROPPED", Warning,
            "a regulator control the BMOPF schema cannot state was dropped";
        EMIT_BMOPF_TRANSFORMER_CENTER_TAP_LEAKAGE_UNREPRESENTABLE =
            "EMIT.BMOPF.TRANSFORMER_CENTER_TAP_LEAKAGE_UNREPRESENTABLE", Warning,
            "a centre tap transformer's leakage split has no BMOPF spelling";
        EMIT_BMOPF_TRANSFORMER_CENTER_TAP_NEUTRAL_COLLAPSED =
            "EMIT.BMOPF.TRANSFORMER_CENTER_TAP_NEUTRAL_COLLAPSED", Warning,
            "a centre tap transformer's neutral was collapsed to one BMOPF winding";
        EMIT_BMOPF_TRANSFORMER_CENTER_TAP_RATING_COLLAPSED =
            "EMIT.BMOPF.TRANSFORMER_CENTER_TAP_RATING_COLLAPSED", Warning,
            "a centre tap transformer's per leg ratings were collapsed to one";
        EMIT_BMOPF_TRANSFORMER_CENTER_TAP_TAP_COLLAPSED =
            "EMIT.BMOPF.TRANSFORMER_CENTER_TAP_TAP_COLLAPSED", Warning,
            "a centre tap transformer's per leg taps were collapsed to one";
        EMIT_BMOPF_TRANSFORMER_CONNECTION_LOSSY = "EMIT.BMOPF.TRANSFORMER_CONNECTION_LOSSY",
            Warning, "a transformer connection reads back differently through BMOPF";
        EMIT_BMOPF_TRANSFORMER_EXTRA_DROPPED = "EMIT.BMOPF.TRANSFORMER_EXTRA_DROPPED", Warning,
            "a transformer passthrough extra with no BMOPF slot was dropped";
        EMIT_BMOPF_TRANSFORMER_MISSING_XSC = "EMIT.BMOPF.TRANSFORMER_MISSING_XSC", Warning,
            "a transformer states no short circuit reactance for BMOPF to carry";
        EMIT_BMOPF_TRANSFORMER_N_WINDING_RATING_COLLAPSED =
            "EMIT.BMOPF.TRANSFORMER_N_WINDING_RATING_COLLAPSED", Warning,
            "an n winding transformer's per winding ratings were collapsed to one";
        EMIT_BMOPF_TRANSFORMER_NEUTRAL_DROPPED = "EMIT.BMOPF.TRANSFORMER_NEUTRAL_DROPPED",
            Warning, "a transformer neutral the BMOPF schema cannot state was dropped";
        EMIT_BMOPF_TRANSFORMER_NO_LOAD_SHUNT_DROPPED =
            "EMIT.BMOPF.TRANSFORMER_NO_LOAD_SHUNT_DROPPED", Warning,
            "a transformer no load shunt the BMOPF schema cannot state was dropped";
        EMIT_BMOPF_TRANSFORMER_NO_LOAD_SHUNT_UNCONVERTIBLE =
            "EMIT.BMOPF.TRANSFORMER_NO_LOAD_SHUNT_UNCONVERTIBLE", Warning,
            "a transformer no load shunt could not be converted to the BMOPF form";
        EMIT_BMOPF_TRANSFORMER_PER_PHASE_TAP_COLLAPSED =
            "EMIT.BMOPF.TRANSFORMER_PER_PHASE_TAP_COLLAPSED", Warning,
            "a transformer's per phase taps were collapsed to one BMOPF tap";
        EMIT_BMOPF_TRANSFORMER_TAP_DROPPED = "EMIT.BMOPF.TRANSFORMER_TAP_DROPPED", Warning,
            "a transformer tap the BMOPF schema cannot state was dropped";
        EMIT_BMOPF_TRANSFORMER_UNSUPPORTED = "EMIT.BMOPF.TRANSFORMER_UNSUPPORTED", Warning,
            "a transformer shape the BMOPF schema has no subtype for";
        EMIT_BMOPF_TRANSFORMER_WINDINGS_CLAMPED = "EMIT.BMOPF.TRANSFORMER_WINDINGS_CLAMPED",
            Warning, "a transformer's winding count was clamped to what BMOPF carries";
        EMIT_BMOPF_TRANSFORMER_WYE_WYE_DECOMPOSED = "EMIT.BMOPF.TRANSFORMER_WYE_WYE_DECOMPOSED",
            Warning, "a wye-wye transformer was decomposed into BMOPF pairs";

        // The multiconductor model itself.
        READ_MULTICONDUCTOR_VALUE_DEFAULTED = "READ.MULTICONDUCTOR.VALUE_DEFAULTED", Warning,
            "a value the document never states was defaulted while reading";
        VALIDATE_MULTICONDUCTOR_REFERENCE_UNDEFINED =
            "VALIDATE.MULTICONDUCTOR.REFERENCE_UNDEFINED", Warning,
            "an element references a bus or linecode the document does not declare";
        EMIT_MULTICONDUCTOR_ROUTE_DROPPED = "EMIT.MULTICONDUCTOR.ROUTE_DROPPED", Warning,
            "a line polyline was dropped because the target has no polyline field";
        EMIT_MULTICONDUCTOR_SIDECAR_DROPPED = "EMIT.MULTICONDUCTOR.SIDECAR_DROPPED", Warning,
            "a companion file the case text refers to was not written";

        // Failures.
        READ_DIST_IO_FAILED = "READ.DIST.IO_FAILED", Fatal,
            "a distribution case file could not be read", category = Io;
        /// Retired in 0.9.0: every distribution read finding now carries its
        /// own code, so the package no longer wraps them under one catch-all.
        READ_DIST_PARSE_WARNING = "READ.DIST.PARSE_WARNING", Warning,
            "a distribution parse finding with no identity of its own", retired = "0.9.0";
        REQUEST_DIST_FORMAT_UNKNOWN = "REQUEST.DIST_FORMAT.UNKNOWN", Fatal,
            "the named distribution format is not one powerio reads",
            category = UnknownFormat;
    }
}

/// Every code this crate declares.
#[must_use]
pub fn registry() -> Vec<&'static DiagnosticInfo> {
    codes::ALL.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_is_sound() {
        let problems = check_registry(registry());
        assert!(problems.is_empty(), "{problems:#?}");
    }
}
