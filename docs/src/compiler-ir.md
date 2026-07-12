# Compiler model layers

Readers parse source formats into typed models. Passes normalize or lower those
models, and writers emit target artifacts. The `.pio.json` field reference is
in
[the `.pio.json` format chapter](pio-json-schema.md).

PowerIO keeps balanced and multiconductor models as separate types. A
`.pio.json` document stores one model payload with provenance, diagnostics,
validation results, and lowering history.

## Model families

PowerIO keeps two concrete static-grid IR families distinct. They share
conventions while keeping separate types; code that needs both holds a
`.pio.json` document rather than a union struct.

### `BalancedNetwork`

`powerio::BalancedNetwork` (an alias of `powerio::Network`) is the scalar
positive sequence model for transmission power flow, OPF, matrices, and graph
analysis. Every electrical quantity is a single `f64`, with no phase or conductor
dimension. Source bus IDs are not dense matrix indices; the dense solver view
is derived separately and preserves source IDs. Loads and shunts have
separate records rather than fields folded onto bus rows.

### `MulticonductorNetwork`

`powerio_dist::MulticonductorNetwork` (an alias of `powerio_dist::DistNetwork`)
is the wire coordinate model for conductor level distribution. Bus IDs are
strings; terminals are ordered string names; every element carries a terminal
map; grounding is explicit; units are SI and radians. A neutral carries
grounding and reduction semantics beyond a phase label. Format defaults and
inferred facts are tracked, and unsupported objects are preserved rather than
dropped.

A balanced model cannot represent conductor-level asymmetry; a multiconductor
model carries terminal and grounding data that has no place in a positive
sequence struct. The two families never merge into one struct.

BMOPF JSON is a strict case format for the distribution family. The `.pio.json`
document uses the same `MulticonductorNetwork` model and wraps it with
metadata: model kind, provenance, source maps, diagnostics, validation, and
lowering history. The `.pio.json` chapter explains why the document is not a
case format.

## The `.pio.json` document

`powerio_pkg::NetworkPackage` is the implementation type for a `.pio.json`
document. It records how a source was interpreted. Language bindings can pass
the document without guessing whether it holds balanced or multiconductor data.

A `.pio.json` document always carries:

- `schema` (URL) and `schema_version` (semver);
- `producer` metadata;
- `model_kind`, explicit and authoritative;
- `model`, the typed model payload, tagged by `kind`;
- `origin` and `sources`;
- `source_maps`;
- `diagnostics`;
- `validation`;
- `summary`;
- `lowering_history`;
- optional `operating_points`;
- optional `study` commits;
- optional `derived` metadata.

`operating_points` is a format neutral series of replayable field updates over
the document's single static model payload. Materializing one point returns
a static document with those updates applied and the series cleared. GO
Challenge 3 document construction fills this block from `time_series_input`:
the balanced model JSON holds the first interval, while every interval is
available as an operating point.

For balanced model JSON, `NetworkPackage::attach_normalized_solver_table_metadata`
records compact metadata for
`powerio::Network::to_normalized_solver_tables()`: pass name, units, row counts,
dense bus ids, reference/component indices, branch to arc indices, and source row
provenance. The document does not duplicate the full table rows; it records enough
metadata for a compiler cache or sidecar artifact to verify table identity.

### Explicit model kind

`model_kind` is a standalone, authoritative field: a reader branches on it
rather than inferring the model kind from which field is present. The reader
requirements are in [the `.pio.json` format chapter](pio-json-schema.md).

### Model JSON stability

The metadata and the model JSON are versioned independently, declared by the
document's `payload_schema` / `payload_schema_version` fields. Model rows
carry stable `uid` identities that operating point updates resolve against.
The bump rules are in [the `.pio.json` format chapter](pio-json-schema.md).

### Provenance and source maps

`Origin` distinguishes an in-memory model, a single file (with or without
retained source), a folder dataset, a partially decoded binary, a derived
product, or a composite. A `SourceMapEntry` points from a model field to its
source with an `element_path`, a `SourceRef` into a declared source, a
`mapping_kind` (`exact`, `defaulted`, `inferred`, `converted_units`, `lowered`,
`aggregated`, `split`, `synthetic`, `retained_extra`), and a `confidence`.
Balanced `source_ref.field` values use canonical model field names. Parser
bookkeeping that should not live in the model JSON (retained source text,
default-materialization records) is lifted into this layer rather than the raw
model JSON.

### Structured diagnostics

Every finding carries a stable dotted `code`, a `severity` (`debug`, `info`,
`warning`, `error`, `fatal`; worst-last so a set's dominant severity is its max),
the `stage` it came from, a human `message`, and where known an `element_path`, a
`source_ref`, a `details` object, and a `suggested_action`. The structured
record is primary; human-readable warnings are rendered from it. Codes are namespaced
by leading segment (`PARSE`, `READ`, `IR`, `VALIDATE`, `FIDELITY`, `LOWER`,
`EMIT`, `BINDING`, `PARTNER`, `PERF`), with the conventional shape
`NAMESPACE.SOURCE_OR_TARGET.SPECIFIC`.

### Lowering

Each pass that transforms one model into another appends a `LoweringRecord`
(input and output kind, options, assumptions, approximations, dropped fields,
diagnostics, validation status) to `lowering_history`. The record makes the
transformation explicit.

`powerio_pkg::lower_multiconductor_to_balanced` lowers transparent three phase
`MulticonductorNetwork` values into `BalancedNetwork` using the
`FortescuePowerInvariant` sequence convention. Neutral conductors are Kron
reduced before the sequence transform. One wire and two wire inputs,
transformers, untyped objects, missing phase references, and closed switches
return structured `LOWER.MULTI_TO_BALANCED.*` diagnostics.
`NetworkPackage::lower_multiconductor_to_balanced` returns a derived balanced
document and appends the record. This pass is explicit only; readers, writers,
matrix builders, bindings, and MCP operations do not run it implicitly.

### Operating point materialization

`NetworkPackage::materialize_operating_point(index)` clones the document, applies
one point's field updates to the typed model JSON, clears `operating_points`, drops
stale source maps and diagnostics for changed fields, recomputes validation, and
records a `LoweringRecord` with `pass = "materialize-operating-point"`. If the
document already carried normalized solver table metadata, the metadata is
rebuilt for the updated static model JSON.

## Versioning

The metadata and model JSON versioning policies are in
[the `.pio.json` format chapter](pio-json-schema.md#pio-package).
