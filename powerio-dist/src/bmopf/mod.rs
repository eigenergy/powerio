//! The BMOPF task force JSON schema
//! (<https://github.com/distribution-system-opt/dsopt-schema>).
//!
//! Everything is explicit SI: volts, watts, vars, ohms, siemens, meters,
//! radians, string bus ids and terminal names. Both schema versions set
//! `additionalProperties: false` on electrical elements and permit free-form
//! top-level `extras` and `meta.provenance` objects, so the strict emitter drops what the target
//! version cannot carry and says so per field.
//!
//! # Two schema versions
//!
//! [`BmopfSchemaVersion`] names the version. Schema 0.1.0 is what the task force
//! accepts: ten element classes and four transformer subtypes. Schema 0.2.0
//! is the proposal that declares the classes 0.1.0 has no table for, and
//! gives the transformer taps, winding neutral impedance, and no load
//! admittance their own subtype slots.
//!
//! The reader accepts both, whatever a document states, because every class
//! 0.2.0 declares at the top level also reads from the `extras` slot 0.1.0
//! files it under. It resolves the version from `meta.$schema` and reports
//! `READ.BMOPF.SCHEMA_ABSENT` when the document states none and
//! `READ.BMOPF.SCHEMA_UNKNOWN` when the value names no version: the class
//! layout of such a document is stated nowhere, and a consumer that assumes
//! one reads a different network on the other.
//!
//! The writer targets one version, 0.2.0 by default. Writing 0.2.0 keeps supported proposal fields in place. Writing 0.1.0 relocates what that version has no slot for, listed
//! below, and reports each move.
//!
//! # What a 0.1.0 write parks under `extras`
//!
//! Some data has a physical meaning schema 0.1.0 cannot express in place but
//! can carry in the free form `extras` object, and that data is relocated
//! rather than dropped, with a warning per element. A consumer that reads the
//! document and ignores `extras` gets a network that differs physically from
//! the source and no error saying so: a tapped transformer reads as nominal
//! tap, an impedance grounded neutral loses its internal grounding branch, and the
//! magnetizing branch disappears.
//!
//! ## Transformer fields
//!
//! Schema 0.1.0 has no slot for these nine fields in the `single_phase`,
//! `center_tap`, `wye_delta`, and `delta_wye` subtype definitions, so a
//! transformer carrying any of them emits them under
//! `extras.transformer.<subtype>.<name>`, keyed by the same transformer name
//! the subtype table uses:
//!
//! `tap`, `tap_min`, `tap_max`, `r_neutral_from`, `x_neutral_from`,
//! `r_neutral_to`, `x_neutral_to`, `g_no_load`, `b_no_load`.
//!
//! Schema 0.2.0 declares all nine on those subtypes, so they stay in place
//! and only the three tap names change, to `tap_ratio`, `tap_ratio_min`, and
//! `tap_ratio_max`, the spelling its regulator subtypes use.
//!
//! Neither version defines untyped `transformer.<subtype>` passthrough
//! objects, so those keep every field in place and nothing moves. The BMOPF
//! reader folds a 0.1.0 overlay back onto the subtype objects; on a key
//! collision the field already in the subtype object wins.
//!
//! ## Whole tables
//!
//! Schema 0.1.0 has no top level slot for these classes, so under that
//! version each emits at `extras.<class>.<name>` with the keying its top
//! level table used:
//!
//! - `ibr` and `control_profile`, emitted from the typed model
//! - `dc_bus`, `dc_branch`, `dc_grounding`, `dc_load`, `dc_source`,
//!   `time_series`, emitted from untyped objects of that class
//! - `capacitor`, only for a capacitor too malformed to type; a typed
//!   capacitor goes to the strict top level `capacitor` table
//!
//! Schema 0.2.0 declares every one of them but `capacitor`, whose top level
//! table stays strict there too, so a malformed capacitor keeps its raw
//! properties under `extras` under either version.
//!
//! A source document's own `extras` object is stashed on read and re-emitted
//! verbatim, so consumer keys beside these survive a write and read back.
//!
//! # Regulator subtypes
//!
//! Schema 0.1.0 defines no regulator subtype; the task force's BMOPFTools
//! toolchain extends it with `single_phase_autotransformer` and
//! `open_delta_regulator`, and schema 0.2.0 declares both. This emitter
//! writes them at top level under 0.2.0 and in `extras.transformer` under 0.1.0.
//! An OpenDSS transformer a RegControl targets emits as
//! `transformer.single_phase_autotransformer` when it reads as a series
//! regulator (one phase, two windings of equal connection, voltage, and
//! rating on distinct buses), and two identical line to line legs spelling
//! one open delta connection (ABBC/BCAC/CABA) merge into one
//! `transformer.open_delta_regulator` object named after the first leg. A
//! BMOPF document that already carries either subtype re-emits it verbatim.
//!
//! # `terminal_conventions` goes stale on a rename
//!
//! The emitted `terminal_conventions` block is the source document's own
//! block re-emitted verbatim, or, absent that, one authored from the terminal
//! names in the model: `n` and `N` are neutral, and `4` in the complete
//! `1,2,3,4` convention is neutral; other names are phase labels. Either way the block describes the terminal naming
//! of the document that carries it and nothing else. A consumer that renames
//! terminals must delete the block and recompute it from the renamed
//! terminals; carried across a rename it sorts names no bus has any more, and
//! nothing else in the document contradicts it.

mod profile;
pub(crate) mod read;
mod write;

pub use profile::{
    BMOPF_PROPOSAL_COMMIT, BMOPF_PROPOSAL_SHA256, BMOPF_PROPOSAL_URL, BmopfSchemaVersion,
};
pub(crate) use write::emit_bmopf_json_text_with_options;
pub use write::{BMOPF_SCHEMA_ID, BMOPF_SCHEMA_VERSION, BmopfEmitOptions};

mod validate;
