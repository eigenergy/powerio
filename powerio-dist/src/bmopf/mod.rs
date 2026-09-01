//! The draft BMOPF task force JSON schema (frederikgeth/bmopf-report).
//!
//! Everything is explicit SI: volts, watts, vars, ohms, siemens, meters,
//! radians, string bus ids and terminal names. The schema sets
//! `additionalProperties: false` on every element, so the strict emitter
//! drops what the schema cannot carry and says so per field; the dropped
//! data stays in the model's `extras`, never in the emitted JSON.
//!
//! # What the emitter parks under `extras`
//!
//! Some data has a physical meaning the schema cannot express in place but
//! can carry in the free form `extras` object, and that data is relocated
//! rather than dropped, with a warning per element. A consumer that reads the
//! document and ignores `extras` gets a network that differs physically from
//! the source and no error saying so: a tapped transformer reads as nominal
//! tap, an impedance grounded neutral as solidly grounded, and the
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
//! The schema defines neither the `n_winding` subtype nor untyped
//! `transformer.<subtype>` passthrough objects, so those keep every field in
//! place and nothing moves. The BMOPF reader folds the overlay back onto
//! the subtype objects, so a powerio round trip is lossless; on a key
//! collision the field already in the subtype object wins.
//!
//! ## Whole tables
//!
//! Schema 0.1.0 has no top level slot for these classes, so each emits at
//! `extras.<class>.<name>` with the keying its top level table used:
//!
//! - `ibr` and `control_profile`, emitted from the typed model
//! - `dc_bus`, `dc_branch`, `dc_load`, `dc_source`, `time_series`, emitted from
//!   untyped objects of that class
//! - `capacitor`, only for a capacitor too malformed to type; a typed
//!   capacitor goes to the strict top level `capacitor` table
//!
//! A source document's own `extras` object is stashed on read and re-emitted
//! verbatim, so consumer keys beside these survive a round trip.
//!
//! # Regulator subtypes track the BMOPFTools schema extension
//!
//! Schema 0.1.0 defines no regulator subtype; the BMOPF authors' BMOPFTools
//! toolchain extends the schema with `single_phase_autotransformer` and
//! `open_delta_regulator`, and this emitter includes both for interoperation
//! with that toolchain, the same standing as the `voltage_source.cost`
//! passthrough, pending an upstream schema proposal. An OpenDSS transformer
//! a RegControl targets emits as `transformer.single_phase_autotransformer`
//! when it reads as a series regulator (one phase, two windings of equal
//! connection, voltage, and rating on distinct buses), and two identical
//! line to line legs spelling one open delta connection (ABBC/BCAC/CABA)
//! merge into one `transformer.open_delta_regulator` object named after the
//! first leg. A BMOPF document that already carries either subtype
//! re-emits it verbatim.
//!
//! # `terminal_conventions` goes stale on a rename
//!
//! The emitted `terminal_conventions` block is the source document's own
//! block re-emitted verbatim, or, absent that, one authored from the terminal
//! names in the model: a terminal named `n` in any case is a neutral, every
//! other name is a phase. Either way the block describes the terminal naming
//! of the document that carries it and nothing else. A consumer that renames
//! terminals must delete the block and recompute it from the renamed
//! terminals; carried across a rename it sorts names no bus has any more, and
//! nothing else in the document contradicts it.

pub(crate) mod read;
mod write;

pub(crate) use write::emit_bmopf_json_text_with_options;
pub use write::{BMOPF_SCHEMA_ID, BMOPF_SCHEMA_VERSION, BmopfEmitOptions};
