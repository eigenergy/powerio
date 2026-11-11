//! The BMOPF task force JSON schema
//! (<https://github.com/distribution-system-opt/dsopt-schema>).
//!
//! Everything is explicit SI: volts, watts, vars, ohms, siemens, meters,
//! radians, string bus ids and terminal names. The schema sets
//! `additionalProperties: false` on electrical elements and permits free-form
//! top-level `extras` and `meta.provenance` objects, so the strict emitter
//! drops what the schema cannot carry and says so per field.
//!
//! # One schema version
//!
//! PowerIO writes BMOPF 0.2.0, as proposed in
//! [dsopt-schema#2](https://github.com/distribution-system-opt/dsopt-schema/pull/2)
//! and vendored from its commit [`BMOPF_PROPOSAL_COMMIT`]. 0.2.0 is schema
//! 0.1.1 plus the classes the task force's draft carried (IBR, control
//! profiles, time series, the DC network, the n-winding and regulator
//! transformers, typed geometry). The pin moves to the `schema-v0.2.0` tag
//! once the Task Force ratifies the proposal.
//!
//! A 0.2.x document names its release in `meta.schema_version`, its only
//! version identifier; any 0.2.x release reads as the 0.2 line. A document
//! written before 0.2.0 states no release and is identified by its
//! `meta.$schema`, as 0.1.x defined.
//!
//! The reader reads every document as 0.2.0. A document that names an earlier
//! version (0.1.0, 0.1.1, the `bmopf-report` drafts, or the 0.2.0 draft
//! PowerIO 0.11 wrote) is read through the legacy paths that
//! accept the older layouts, and reported once with
//! `READ.BMOPF.SCHEMA_LEGACY`, which names the version it stated. The older
//! layouts are: the IBR, control profile, DC, time series, and overflow
//! transformer tables under `extras`; the transformer `tap` spelling; the
//! `cost` spelling of `energy_cost_rate`; the bus bounds `v_min`/`v_max` and
//! `vn_max`; `meta.$schema`; the regulators' `tap_ratio`; `DSTATCOM`,
//! `VA_FRACTION` and the lowercase enum values; and the `extras.geojson`
//! geometry layer. A document stating 0.2.x that uses those spellings gets the
//! same single finding, and one stating both bus bound spellings with
//! different values is `READ.BMOPF.SEMANTIC_INVALID`. A document stating no
//! version is reported with `READ.BMOPF.SCHEMA_ABSENT`, and one naming no
//! known version with `READ.BMOPF.SCHEMA_UNKNOWN`; both are read the same way.
//!
//! Output always states 0.2.0, even for an unchanged document read from an
//! earlier version: the retained source bytes are echoed only when the
//! source itself states a 0.2.x release and no `meta.$schema`. The writer
//! never states a value schema 0.2.0 rejects; it drops it and reports it.
//!
//! The phase to ground bounds are `vpg_min`/`vpg_max`, and the neutral to
//! ground cap is `vng_max`. Geometry goes in the
//! typed `bus.*.geo` (a GeoJSON Point) and `line.*.geo` (a GeoJSON
//! LineString) slots, with the coordinate system in `meta.crs` (absent means
//! OGC:CRS84, longitude then latitude, which the writer leaves implicit); a coordinate
//! space the schema cannot name is also recorded as `extras.powerio_geo`.
//! Component locations (`MulticonductorNetwork::component_locations`) travel
//! as the `geo` Point of the voltage source, load, generator, IBR, shunt,
//! capacitor, switch or transformer they attach to, in a WGS84 document; a
//! switch or transformer `geo` LineString reads as the midpoint of its end
//! positions. `ibr` and `control_profile` are top-level tables, the two-winding
//! transformer subtypes spell the ratio `tap_ratio`, `tap_ratio_min` and
//! `tap_ratio_max`, the regulators `voltage_ratio`, `voltage_ratio_min` and
//! `voltage_ratio_max`, and costs are `energy_cost_rate`. Line and linecode
//! `s_max` have one entry per conductor.
//!
//! The schema's `$id` ([`BmopfSchemaVersion::schema_id`]), where the exact
//! schema bytes were retrieved from, and their digest are recorded under
//! `meta.provenance.powerio_bmopf` ([`BmopfSchemaVersion::retrieval_url`],
//! [`BMOPF_PROPOSAL_SHA256`]).
//!
//! # What stays under `extras`
//!
//! The top-level `capacitor` table is strict, so a capacitor too malformed to
//! type keeps its raw properties under `extras.capacitor`, and a time series
//! profile BMOPF's multiplicative semantics cannot represent stays verbatim
//! under `extras.time_series`; each is reported. A source document's own
//! `extras` object is stashed on read and re-emitted verbatim, so consumer
//! keys beside these survive a write and read back.
//!
//! # Regulator subtypes
//!
//! Schema 0.1.0 defines no regulator subtype; the task force's BMOPFTools
//! toolchain extends it with `single_phase_autotransformer` and
//! `open_delta_regulator`, and schema 0.2.0 declares both. This emitter
//! writes them in the top-level `transformer` table.
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

mod capacitor;
mod geo;
mod profile;
pub(crate) mod read;
mod write;

pub use profile::{
    BMOPF_PROPOSAL_COMMIT, BMOPF_PROPOSAL_SHA256, BMOPF_PROPOSAL_URL, BmopfSchemaVersion,
};
pub(crate) use write::emit_bmopf_json_text_with_options;
pub use write::{BMOPF_SCHEMA_ID, BMOPF_SCHEMA_VERSION, BmopfEmitOptions};

/// Whether a BMOPF document identifies itself as the version PowerIO
/// writes: a `meta.schema_version` of the 0.2 line and no earlier
/// `meta.$schema`. A same-format emission echoes retained source bytes only
/// then; a document stating an earlier version, or none, is written fresh as
/// 0.2.0. Shared with the facade, which echoes calculation values itself.
#[doc(hidden)]
#[must_use]
pub fn __states_written_version(text: &str) -> bool {
    #[derive(serde::Deserialize)]
    struct Document {
        meta: Option<Meta>,
    }
    #[derive(serde::Deserialize)]
    struct Meta {
        schema_version: Option<String>,
        #[serde(rename = "$schema")]
        schema: Option<serde::de::IgnoredAny>,
    }
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    serde_json::from_str::<Document>(text)
        .ok()
        .and_then(|document| document.meta)
        .is_some_and(|meta| {
            meta.schema.is_none()
                && meta
                    .schema_version
                    .as_deref()
                    .and_then(BmopfSchemaVersion::from_release)
                    == Some(BmopfSchemaVersion::default())
        })
}

mod validate;
