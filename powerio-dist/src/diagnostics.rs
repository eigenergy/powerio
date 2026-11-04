//! The codes this crate emits.
//!
//! The record itself lives in `powerio-core`, below this crate and below the
//! `.pio.json` document model, so a distribution finding reaches a package
//! without a translation step. What lives here is the distribution side
//! registry: one [`DiagnosticInfo`] per code, declared once, so an emission
//! site names an entry rather than a loose string.
//!
//! Codes are families, not one per site: what differs between two sites of a
//! family is which object or property it was, which belongs in `details`.

// The collector is crate-private implementation support, not API: each
// emitting crate carries its own copy (src/collect.rs) and never exports it.
pub(crate) use crate::collect::Diagnostics;

pub use powerio_core::{
    Diagnostic, DiagnosticCode, DiagnosticInfo, DiagnosticSeverity, DiagnosticStage,
    check_registry, render_diagnostic, render_diagnostics,
};

pub mod codes {
    powerio_core::diagnostic_codes! {
        PARSE_SINCAL_MULTICONDUCTOR = "PARSE.DIST.SINCAL", Error,
            "the selected conductor-resolved SINCAL profile cannot be parsed", category = Parse;
        REQUEST_SINCAL_MULTICONDUCTOR_FEATURE_DISABLED = "REQUEST.DIST_FORMAT.SINCAL_FEATURE_DISABLED", Error,
            "conductor-resolved SINCAL is not available through this build or entry point", category = Request;
        READ_SINCAL_MULTICONDUCTOR_RETAINED_SOURCE_ONLY = "READ.DIST.SINCAL_RETAINED_SOURCE_ONLY", Remark,
            "native data outside the selected conductor-resolved profile remain only in source";
        READ_SINCAL_GEOMETRY = "READ.DIST.SINCAL_GEOMETRY", Warning,
            "optional SINCAL drawing geometry retains native view metadata and reports mapping limits";
        READ_SINCAL_UNCONNECTED_NODES = "READ.DIST.SINCAL_UNCONNECTED_NODES", Remark,
            "native nodes with no declared conductors remain in network extras";
        READ_SINCAL_ASSUMED_INACTIVE_SOURCE_CONTROLS = "READ.DIST.SINCAL_ASSUMED_INACTIVE_SOURCE_CONTROLS", Warning,
            "legacy NULL source controls were explicitly assumed inactive";
        EMIT_SINCAL_MULTICONDUCTOR_RETAINED_SOURCE_OMITTED = "EMIT.DIST.SINCAL_RETAINED_SOURCE_OMITTED", Warning,
            "native SINCAL source-only data are omitted from cross-format emission";
        EMIT_SINCAL_MULTICONDUCTOR_UNSUPPORTED = "EMIT.DIST.SINCAL_UNSUPPORTED", Error,
            "invalid or unsupported candidate multiconductor SINCAL output", category = Output;
        EMIT_SINCAL_MULTICONDUCTOR_EXPERIMENTAL = "EMIT.DIST.SINCAL_EXPERIMENTAL", Remark,
            "candidate multiconductor output has no native SINCAL acceptance evidence";
        EMIT_SINCAL_MULTICONDUCTOR_LOSS = "EMIT.DIST.SINCAL_LOSS", Warning,
            "candidate multiconductor output canonicalizes topology or omits ancillary data";
        // PARSE: the source text could not be decoded as given.
        PARSE_DSS_SOURCE_MALFORMED = "PARSE.DSS.SOURCE_MALFORMED", Warning,
            "a dss command, object spec, or property assignment does not parse";
        PARSE_DIST_MALFORMED = "PARSE.DIST.MALFORMED", Error,
            "a distribution document is not valid JSON for its format", category = Parse;
        PARSE_DIST_SOURCE_MALFORMED = "PARSE.DIST.SOURCE_MALFORMED", Error,
            "a distribution reader refused the source it was given", category = Parse;
        PARSE_GLM_SYNTAX = "PARSE.GLM.SYNTAX", Error,
            "a GLM source contains invalid or incomplete syntax", category = Parse;
        PARSE_GLM_LIMIT_EXCEEDED = "PARSE.GLM.LIMIT_EXCEEDED", Error,
            "a GLM source exceeds the bounded parser resources", category = Parse;
        PARSE_GLM_PREPROCESSING = "PARSE.GLM.PREPROCESSING", Error,
            "a GLM preprocessing operation is invalid or requires unsupported external semantics", category = Parse;
        PARSE_GLM_OBJECT = "PARSE.GLM.OBJECT", Error,
            "a GLM object identity, declaration or reference cannot be resolved", category = Parse;
        PARSE_GLM_QUANTITY = "PARSE.GLM.QUANTITY", Error,
            "a GLM quantity or unit cannot be interpreted in the static profile", category = Parse;
        READ_GLM_UNIT_IGNORED = "READ.GLM.UNIT_IGNORED", Warning,
            "native quantity conversion ignores a stated unit suffix";
        READ_GLM_ELECTRICAL_UNSUPPORTED = "READ.GLM.ELECTRICAL_UNSUPPORTED", Error,
            "a GLM electrical feature has no verified static mapping", category = Data;
        READ_GLM_IMPLICIT_SOURCE = "READ.GLM.IMPLICIT_SOURCE", Remark,
            "a connected directed FBS tree has one implicit upstream voltage source";
        READ_GLM_SOURCE_ONLY = "READ.GLM.SOURCE_ONLY", Remark,
            "non-electrical GLM data remains in the retained source";
        READ_GLM_SWITCH_STATE_NORMALIZED = "READ.GLM.SWITCH_STATE_NORMALIZED", Remark,
            "static switch status and banked initialization determine contact states";
        READ_GLM_SWITCH_EQUIVALENT = "READ.GLM.SWITCH_EQUIVALENT", Remark,
            "finite closed switch contacts use an exact static line equivalent";
        READ_GLM_REDUCED_TRIPLEX = "READ.GLM.REDUCED_TRIPLEX", Warning,
            "a direct triplex matrix does not specify physical neutral or ground current";
        READ_GLM_LOAD_NORMALIZED = "READ.GLM.LOAD_NORMALIZED", Remark,
            "native ZIP initialization defines the typed load voltage-law terms";
        READ_GLM_BATTERY_IDLE = "READ.GLM.BATTERY_IDLE", Remark,
            "Internal battery has zero fixed injection in constant-PF mode; energy and inactive control metadata remain in source";
        READ_GLM_SOLAR_EQUIVALENT = "READ.GLM.SOLAR_EQUIVALENT", Remark,
            "a static solar and inverter snapshot uses fixed typed inverter injections";
        READ_GLM_REACTOR_EQUIVALENT = "READ.GLM.REACTOR_EQUIVALENT", Remark,
            "a static reactor uses an exact line or ideal-contact equivalent";
        READ_GLM_SUBSTATION_EQUIVALENT = "READ.GLM.SUBSTATION_EQUIVALENT", Remark,
            "a standalone substation supplies initialized sequence-derived phase voltages";
        READ_GLM_GENERATOR_EQUIVALENT = "READ.GLM.GENERATOR_EQUIVALENT", Remark,
            "initialized static generator powers and ratings use phase-resolved records";
        READ_GLM_REGULATOR_EQUIVALENT = "READ.GLM.REGULATOR_EQUIVALENT", Remark,
            "fixed regulator phases use equivalent winding records on explicit normalization bases";
        READ_GLM_TRANSFORMER_EQUIVALENT = "READ.GLM.TRANSFORMER_EQUIVALENT", Warning,
            "native NR delta-delta transformer equations use an equivalent grounded-wye circuit";
        READ_GLM_CONTROL_FROZEN = "READ.GLM.CONTROL_FROZEN", Warning,
            "active control behavior is omitted while declared fixed electrical states are preserved";
        READ_GLM_LINE_EQUIVALENT = "READ.GLM.LINE_EQUIVALENT", Remark,
            "native FBS noncommuting line equations use an exact admittance equivalent";
        READ_GLM_NATIVE_LINE_DISCREPANCY = "READ.GLM.NATIVE_LINE_DISCREPANCY", Warning,
            "native line reporting and FBS solved states can differ from the physical pi-line model for noncommuting matrices";
        READ_GLM_GEOMETRY_MATERIALIZED = "READ.GLM.GEOMETRY_MATERIALIZED", Remark,
            "overhead conductor geometry is materialized as phase matrices with explicit reduction provenance";
        EMIT_GLM_UNSUPPORTED = "EMIT.GLM.UNSUPPORTED", Error,
            "the GLM writer cannot yet represent this electrical feature", category = Data;
        EMIT_GLM_FIELD_DROPPED = "EMIT.GLM.FIELD_DROPPED", Warning,
            "non-electrical metadata has no expression in canonical GLM output";
        EMIT_GLM_VALUE_REBASED = "EMIT.GLM.VALUE_REBASED", Remark,
            "a GLM nominal voltage or identity was materialized without changing electrical behavior";
        EMIT_GLM_IDEAL_CONTACTS = "EMIT.GLM.IDEAL_CONTACTS", Remark,
            "ideal contacts use source-oriented FBS with exact zero impedance and explicit phase grouping";
        EMIT_GLM_TRANSFORMER_SHUNT_EQUIVALENT = "EMIT.GLM.TRANSFORMER_SHUNT_EQUIVALENT", Remark,
            "a transformer terminal core shunt uses an exact admittance load";
        EMIT_GLM_LINE_SHUNT_EQUIVALENT = "EMIT.GLM.LINE_SHUNT_EQUIVALENT", Remark,
            "line end shunts use exact phase-pair and earth-return admittance loads";
        EMIT_GLM_PROJECT_UNREPRESENTABLE = "EMIT.GLM.PROJECT_UNREPRESENTABLE", Error,
            "retained GLM project bytes cannot be replayed as a portable output inventory", category = Output;
        EMIT_GLM_REPLAY_CONTEXT_REQUIRED = "EMIT.GLM.REPLAY_CONTEXT_REQUIRED", Remark,
            "exact GLM project replay requires the original explicit preprocessing inputs";
        VALIDATE_DIST_COORDINATES_INVALID = "VALIDATE.DIST.COORDINATES_INVALID", Error,
            "component geographic coordinates or their attachment identities are invalid", category = Data;
        EMIT_DIST_COMPONENT_COORDINATES_DROPPED = "EMIT.DIST.COMPONENT_COORDINATES_DROPPED", Warning,
            "the target cannot carry typed component geographic coordinates";

        // READ.DSS: decoded, but not representable in the multiconductor model.
        /// A `Redirect`/`Compile`/`Buscoords` include the reader refused
        /// because it escapes the case directory, or the include root when
        /// the caller widened one. Severity `Error`: the parse continued,
        /// but the network is incomplete.
        READ_DSS_INCLUDE_REFUSED = "READ.DSS.INCLUDE_REFUSED", Error,
            "an include escaping the case directory or the configured include root was refused";
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
        READ_DSS_SOURCE_DISABLED = "READ.DSS.SOURCE_DISABLED", Remark,
            "a disabled source is retained without electrical contributions";
        EMIT_DSS_PROJECT_UNREPRESENTABLE = "EMIT.DSS.PROJECT_UNREPRESENTABLE", Error,
            "retained DSS dependencies cannot be replayed as a portable project", category = Output;
        EMIT_DSS_PROJECT_COMPANION_KEPT = "EMIT.DSS.PROJECT_COMPANION_KEPT", Remark,
            "a supplied file the deck never read was copied into the retained project";
        READ_DSS_LOADSHAPE_UNRESOLVED = "READ.DSS.LOADSHAPE_UNRESOLVED", Error,
            "static UseActual loadshape power could not be resolved";
        READ_DSS_SOURCE_UNRESOLVED = "READ.DSS.SOURCE_UNRESOLVED", Error,
            "a voltage source is invalid or outside the qualified static profile", category = Data;
        READ_DSS_FAULT_UNRESOLVED = "READ.DSS.FAULT_UNRESOLVED", Error,
            "a fault switching state could not be resolved into the static electrical snapshot", category = Data;
        READ_DSS_TRANSFORMER_UNRESOLVED = "READ.DSS.TRANSFORMER_UNRESOLVED", Error,
            "a transformer definition could not be resolved into the static electrical snapshot";
        READ_DSS_LINE_UNRESOLVED = "READ.DSS.LINE_UNRESOLVED", Error,
            "a line is invalid or outside the qualified static electrical profile", category = Data;
        READ_DSS_BATCH_UNRESOLVED = "READ.DSS.BATCH_UNRESOLVED", Error,
            "a BatchEdit selector could not be executed faithfully", category = Data;
        READ_DSS_PROPERTY_UNKNOWN = "READ.DSS.PROPERTY_UNKNOWN", Warning,
            "a property this reader does not model is kept as written";
        READ_DSS_REFERENCE_DROPPED = "READ.DSS.REFERENCE_DROPPED", Warning,
            "a control or element reference names an object the case does not declare";
        READ_DSS_RETAINED_SOURCE_ONLY = "READ.DSS.RETAINED_SOURCE_ONLY", Warning,
            "a field survives in extras or the retained source rather than in a typed field";
        READ_DSS_COORDINATE_SPACE_UNKNOWN = "READ.DSS.COORDINATE_SPACE_UNKNOWN", Remark,
            "buscoords declare no coordinate reference system";
        READ_DSS_INCLUDE_LOAD_FAILED = "READ.DSS.INCLUDE_LOAD_FAILED", Warning,
            "an include the case names could not be loaded";
        READ_DSS_INCLUDE_DEPTH_LIMIT = "READ.DSS.INCLUDE_DEPTH_LIMIT", Warning,
            "the reader stopped following includes at the nesting depth limit";
        READ_DSS_GEOMETRY_UNRESOLVED = "READ.DSS.GEOMETRY_UNRESOLVED", Error,
            "line geometry remains source data without calculated conductor impedances";
        BUILD_DIST_ELECTRICAL_INCOMPLETE = "BUILD.DIST.ELECTRICAL_INCOMPLETE", Error,
            "distribution equipment lacks the electrical data required for numerical use", category = Data;
        TRANSFORM_DIST_LINDIST3FLOW_PREPARATION_FAILED =
            "TRANSFORM.DIST.LINDIST3FLOW_PREPARATION_FAILED", Error,
            "a distribution component cannot be prepared for LinDist3Flow", category = Data;
        READ_DSS_LINECODE_UNKNOWN = "READ.DSS.LINECODE_UNKNOWN", Warning,
            "a line names a linecode the case does not declare";

        // EMIT.DSS: what the canonical dss writer cannot state.
        EMIT_DSS_IDEAL_SWITCH_EQUIVALENT = "EMIT.DSS.IDEAL_SWITCH_EQUIVALENT", Remark,
            "an ideal closed switch is emitted using exact impedance cancellation";
        EMIT_DSS_IDEAL_TRANSFORMER_EQUIVALENT = "EMIT.DSS.IDEAL_TRANSFORMER_EQUIVALENT", Remark,
            "an ideal transformer is emitted using exact leakage cancellation";
        EMIT_DSS_SOURCE_EQUIVALENT = "EMIT.DSS.SOURCE_EQUIVALENT", Remark,
            "an ideal source is emitted as an exact finite-impedance circuit equivalent";
        EMIT_DSS_IMPEDANCE_EQUIVALENT = "EMIT.DSS.IMPEDANCE_EQUIVALENT", Remark,
            "a conductor impedance is emitted as an exact zero-EMF source equivalent";
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
        /// OpenDSS reads a node list positionally, with a grounded return
        /// conductor last. A uniquely grounded return in any other position
        /// makes the emitted element address different conductors.
        EMIT_DSS_TERMINAL_ORDER_UNREPRESENTABLE = "EMIT.DSS.TERMINAL_ORDER_UNREPRESENTABLE", Error,
            "a grounded return conductor is not last in a positional dss node list";

        // READ.PMD.
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
        READ_PMD_VALUE_INLINED = "READ.PMD.VALUE_INLINED", Remark,
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
        READ_BMOPF_CAPACITOR_LOWERED = "READ.BMOPF.CAPACITOR_LOWERED", Remark,
            "a per-coil capacitor was represented exactly by a terminal shunt";
        READ_BMOPF_VALUE_COLLAPSED = "READ.BMOPF.VALUE_COLLAPSED", Warning,
            "a per phase or per terminal BMOPF value was collapsed to one entry";
        READ_BMOPF_TRANSFORMER_OPEN_DELTA_SPLIT = "READ.BMOPF.TRANSFORMER_OPEN_DELTA_SPLIT", Warning,
            "one open_delta_regulator object reads as its two regulating legs, the second under a synthesized name";
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
        /// The document states no `meta.$schema`, so the schema version it was
        /// written against is unknown and the reader accepts both versions.
        READ_BMOPF_SCHEMA_ABSENT = "READ.BMOPF.SCHEMA_ABSENT", Warning,
            "a BMOPF document states no schema, so its version is unknown";
        /// The document states a `meta.$schema` that names no known version.
        READ_BMOPF_SCHEMA_UNKNOWN = "READ.BMOPF.SCHEMA_UNKNOWN", Warning,
            "a BMOPF document states a schema naming no known version";
        READ_BMOPF_SEMANTIC_INVALID = "READ.BMOPF.SEMANTIC_INVALID", Error,
            "the BMOPF document has inconsistent dimensions, references, or bounds";
        READ_BMOPF_SCHEMA_MISMATCH = "READ.BMOPF.SCHEMA_MISMATCH", Error,
            "the schema URI and explicit schema version disagree";

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


        // EMIT.BMOPF: the writer's element specific codes, registered
        // entries rather than loose string literals.
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
        EMIT_BMOPF_TRANSFORMER_OPEN_DELTA_MERGED = "EMIT.BMOPF.TRANSFORMER_OPEN_DELTA_MERGED",
            Warning, "two regulator legs merged into one open_delta_regulator object";
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

        // READ.CIM_DIST: distribution CIM decoded, but not representable in
        // the multiconductor model.
        READ_CIM_DIST_CLASS_UNMAPPED = "READ.CIM_DIST.CLASS_UNMAPPED", Warning,
            "objects of a CIM class the distribution mapping does not read were not carried";
        READ_CIM_DIST_CONTEXT_UNMAPPED = "READ.CIM_DIST.CONTEXT_UNMAPPED", Remark,
            "containment, naming, or base voltage context the model has no field for was not carried";
        READ_CIM_DIST_FIELD_UNMAPPED = "READ.CIM_DIST.FIELD_UNMAPPED", Warning,
            "a property of a mapped CIM object has no model field and was not carried";
        READ_CIM_DIST_RECORD_DROPPED = "READ.CIM_DIST.RECORD_DROPPED", Warning,
            "a CIM equipment record could not be mapped and was dropped";
        READ_CIM_DIST_VALUE_DEFAULTED = "READ.CIM_DIST.VALUE_DEFAULTED", Warning,
            "a value the model needs was absent or unusable and was defaulted";
        READ_CIM_DIST_VALUE_UNSUPPORTED = "READ.CIM_DIST.VALUE_UNSUPPORTED", Warning,
            "a CIM value outside the mapped set was read as the nearest one";
        READ_CIM_DIST_VALUE_COLLAPSED = "READ.CIM_DIST.VALUE_COLLAPSED", Warning,
            "CIM structure was reduced to what one model field can carry";
        READ_CIM_DIST_VALUE_CONFLICT = "READ.CIM_DIST.VALUE_CONFLICT", Warning,
            "two statements of one CIM property disagree and the first was kept";
        READ_CIM_DIST_REFERENCE_DROPPED = "READ.CIM_DIST.REFERENCE_DROPPED", Warning,
            "a CIM reference names an object the document set does not define";
        READ_CIM_DIST_IDENTIFIER_SUBSTITUTED = "READ.CIM_DIST.IDENTIFIER_SUBSTITUTED", Warning,
            "a CIM name that is empty or repeats another was replaced by the object's mRID";
        /// A segment states no impedance (no `PerLengthImpedance` and no
        /// series values), typically because it is described by wire
        /// geometry, which the reader does not calculate. Severity `Error`:
        /// the parse continued, but the segment has no series impedance.
        READ_CIM_DIST_IMPEDANCE_UNRESOLVED = "READ.CIM_DIST.IMPEDANCE_UNRESOLVED", Error,
            "a line segment states no impedance the reader can carry";

        // READ.PMD_MATH: the MATHEMATICAL model decoded, but not representable
        // as stated.
        READ_PMD_MATH_VALUE_DEFAULTED = "READ.PMD_MATH.VALUE_DEFAULTED", Warning,
            "a value the model needs is absent from the MATHEMATICAL document and was defaulted";
        READ_PMD_MATH_VALUE_SUBSTITUTED = "READ.PMD_MATH.VALUE_SUBSTITUTED", Warning,
            "a MATHEMATICAL value was restated on a base or form the model carries";
        READ_PMD_MATH_VALUE_COLLAPSED = "READ.PMD_MATH.VALUE_COLLAPSED", Warning,
            "MATHEMATICAL structure was reduced to what one model field can carry";
        READ_PMD_MATH_RECORD_DROPPED = "READ.PMD_MATH.RECORD_DROPPED", Warning,
            "a MATHEMATICAL object could not be mapped and was dropped";
        READ_PMD_MATH_RETAINED_SOURCE_ONLY = "READ.PMD_MATH.RETAINED_SOURCE_ONLY", Warning,
            "a MATHEMATICAL object or field survives untyped or in extras rather than in a typed field";
        READ_PMD_MATH_SOURCE_MALFORMED = "READ.PMD_MATH.SOURCE_MALFORMED", Warning,
            "a MATHEMATICAL value is not the shape its key declares";
        READ_PMD_MATH_REFERENCE_DROPPED = "READ.PMD_MATH.REFERENCE_DROPPED", Warning,
            "a MATHEMATICAL object names a bus or map target the document does not define";

        // EMIT.PMD_MATH: what the MATHEMATICAL writer cannot state.
        EMIT_PMD_MATH_FIELD_DROPPED = "EMIT.PMD_MATH.FIELD_DROPPED", Warning,
            "a field the MATHEMATICAL model has no key for was dropped";
        EMIT_PMD_MATH_RECORD_DROPPED = "EMIT.PMD_MATH.RECORD_DROPPED", Warning,
            "an element the MATHEMATICAL writer does not lower was dropped";
        EMIT_PMD_MATH_VALUE_DEFAULTED = "EMIT.PMD_MATH.VALUE_DEFAULTED", Warning,
            "a value the MATHEMATICAL model requires was synthesized";
        EMIT_PMD_MATH_VALUE_SUBSTITUTED = "EMIT.PMD_MATH.VALUE_SUBSTITUTED", Warning,
            "a stated value was replaced by one the MATHEMATICAL model can hold";

        // READ.RAVENS: the multiconductor MG-RAVENS profile decoded, but
        // not representable in the model as stated.
        READ_RAVENS_DIST_CLASS_UNMAPPED = "READ.RAVENS_DIST.CLASS_UNMAPPED", Warning,
            "MG-RAVENS objects of a class the distribution mapping does not read were kept untyped";
        READ_RAVENS_DIST_FIELD_UNMAPPED = "READ.RAVENS_DIST.FIELD_UNMAPPED", Warning,
            "a property of a mapped MG-RAVENS object has no model field and was not carried";
        READ_RAVENS_DIST_RECORD_DROPPED = "READ.RAVENS_DIST.RECORD_DROPPED", Warning,
            "an MG-RAVENS equipment record could not be mapped and was kept untyped";
        READ_RAVENS_DIST_VALUE_DEFAULTED = "READ.RAVENS_DIST.VALUE_DEFAULTED", Warning,
            "a value the model needs is absent from the MG-RAVENS document and was defaulted";
        READ_RAVENS_DIST_VALUE_SUBSTITUTED = "READ.RAVENS_DIST.VALUE_SUBSTITUTED", Warning,
            "an MG-RAVENS value outside the mapped set was read as the nearest one";
        READ_RAVENS_DIST_VALUE_COLLAPSED = "READ.RAVENS_DIST.VALUE_COLLAPSED", Warning,
            "MG-RAVENS structure was reduced to what one model field can carry";
        READ_RAVENS_DIST_VALUE_CONFLICT = "READ.RAVENS_DIST.VALUE_CONFLICT", Warning,
            "two MG-RAVENS statements of one value disagree and the first was kept";
        READ_RAVENS_DIST_RETAINED_SOURCE_ONLY = "READ.RAVENS_DIST.RETAINED_SOURCE_ONLY", Warning,
            "an MG-RAVENS object survives untyped: it round-trips to MG-RAVENS and is dropped elsewhere";

        // EMIT.RAVENS: what the multiconductor MG-RAVENS writer cannot state.
        EMIT_RAVENS_DIST_FIELD_DROPPED = "EMIT.RAVENS_DIST.FIELD_DROPPED", Warning,
            "a field MG-RAVENS has no property for was dropped";
        EMIT_RAVENS_DIST_RECORD_DROPPED = "EMIT.RAVENS_DIST.RECORD_DROPPED", Warning,
            "an element the MG-RAVENS writer does not emit was dropped";
        EMIT_RAVENS_DIST_EXTRAS_DROPPED = "EMIT.RAVENS_DIST.EXTRAS_DROPPED", Warning,
            "a passthrough extra with no MG-RAVENS property was dropped";
        EMIT_RAVENS_DIST_VALUE_COLLAPSED = "EMIT.RAVENS_DIST.VALUE_COLLAPSED", Warning,
            "structure was reduced to what one MG-RAVENS property can carry";
        EMIT_RAVENS_DIST_VALUE_SUBSTITUTED = "EMIT.RAVENS_DIST.VALUE_SUBSTITUTED", Warning,
            "a stated value was replaced by one MG-RAVENS can hold";
        EMIT_RAVENS_DIST_NOT_A_NUMBER = "EMIT.RAVENS_DIST.NOT_A_NUMBER", Warning,
            "a non-finite value was written as 0, because JSON has no Inf or NaN";
        EMIT_RAVENS_DIST_SOURCE_SPLIT = "EMIT.RAVENS_DIST.SOURCE_SPLIT", Warning,
            "an unbalanced source is written as one single phase EnergySource per phase conductor, with the same terminal phasors";

        // EMIT.CIM_DIST: what the distribution CIM writer cannot state.
        EMIT_CIM_DIST_FIELD_DROPPED = "EMIT.CIM_DIST.FIELD_DROPPED", Warning,
            "a field distribution CIM has no property for was dropped";
        EMIT_CIM_DIST_RECORD_DROPPED = "EMIT.CIM_DIST.RECORD_DROPPED", Warning,
            "an element the distribution CIM writer does not emit was dropped";
        EMIT_CIM_DIST_EXTRAS_DROPPED = "EMIT.CIM_DIST.EXTRAS_DROPPED", Warning,
            "a passthrough extra with no distribution CIM property was dropped";
        EMIT_CIM_DIST_VALUE_COLLAPSED = "EMIT.CIM_DIST.VALUE_COLLAPSED", Warning,
            "structure was reduced to what one CIM property can carry";
        EMIT_CIM_DIST_VALUE_SUBSTITUTED = "EMIT.CIM_DIST.VALUE_SUBSTITUTED", Warning,
            "an identifier or value was replaced by one distribution CIM can state";

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

        // Same-family distribution network projections.
        TRANSFORM_DIST_NEUTRAL_KRON_REDUCED = "TRANSFORM.DIST.NEUTRAL_KRON_REDUCED", Remark,
            "an explicit grounded neutral was eliminated by Kron reduction";
        TRANSFORM_DIST_NEUTRAL_KRON_FORCED_GROUND =
            "TRANSFORM.DIST.NEUTRAL_KRON_FORCED_GROUND", Warning,
            "a neutral was treated as ideally grounded before Kron reduction";
        TRANSFORM_DIST_KRON_UNTYPED_RETAINED = "TRANSFORM.DIST.KRON_UNTYPED_RETAINED", Warning,
            "untyped source objects were retained unchanged through neutral reduction";

        // Failures.
        READ_DIST_IO_FAILED = "READ.DIST.IO_FAILED", Error,
            "a distribution case file could not be read", category = Io;
        REQUEST_DIST_FORMAT_UNKNOWN = "REQUEST.DIST_FORMAT.UNKNOWN", Error,
            "the named distribution format is not one powerio reads",
            category = Request;
        TRANSFORM_DIST_KRON_REDUCTION_FAILED = "TRANSFORM.DIST.KRON_REDUCTION_FAILED", Error,
            "a neutral Kron projection could not preserve the network semantics",
            category = Data;
    }
}

/// Every code this crate declares.
#[must_use]
pub fn registry() -> Vec<&'static DiagnosticInfo> {
    codes::ALL.to_vec()
}

/// Attach an input-derived locator, or record its loss on the finding when it
/// cannot be stored. A locator is identity, so it is never shortened; when the
/// input supplies one past the record bounds, the finding says a locator
/// existed and how long it was instead of silently changing identity.
pub(crate) fn attach_target(diagnostic: &mut Diagnostic, target: String) {
    let byte_length = target.len();
    if diagnostic.set_target(target).is_err() {
        let marker =
            diagnostic.insert_detail("dropped_target_bytes", serde_json::Value::from(byte_length));
        debug_assert!(marker.is_ok(), "the drop marker fits an emitted finding");
    }
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
