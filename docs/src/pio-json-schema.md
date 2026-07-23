# `.pio.json` format

A `.pio.json` file stores one typed network model payload and the record of how
it was produced. The `model` field contains the JSON representation of either
`powerio::Network` (balanced) or
`powerio_dist::DistNetwork` (multiconductor). The document metadata records
provenance, source maps, structured diagnostics, validation results, lowering
history, optional operating points, and optional study commits.
`powerio_pkg::NetworkPackage` is the
implementation type; [Compiler model layers](compiler-ir.md) describes the
payload types.

## Purpose

A MATPOWER or OpenDSS file
states the case; it cannot state how a parser read it: which fields were
defaulted or inferred, what validation found, or how a multiconductor model was
lowered to a balanced one. The metadata records that work next to the model, so
a downstream tool can audit a conversion instead of trusting it.

The `.pio.json` document is also the handoff object between PowerIO consumers:
one artifact whose model kind is explicit, with provenance intact.

## `.pio.json` is not a case format {#not-a-case-format}

Case formats move cases between tools: MATPOWER, PSS/E, OpenDSS, PMD JSON,
BMOPF, GOC3, and the other rows in the conversion tables. PowerIO reads and
writes those formats at converter boundaries. A `.pio.json` document is
PowerIO's compiled artifact: the model plus the record of how that model was
produced.

Pick a case format by what the receiving tool reads. Use `.pio.json` when the
receiving consumer is PowerIO or a binding that wants provenance, diagnostics,
operating points, and the explicit model kind. Use BMOPF, OpenDSS, PMD JSON, or
another supported case format when the next tool expects that format.

`powerio-json` is bare balanced `Network` JSON, without package metadata or
source maps. Version 0.7 removes it from advertised CLI file formats. Use
`Network::to_json` and `Network::from_json` for model JSON. The C ABI exposes
the same operations as `pio_to_json` and `pio_from_json`.

ABI v4 continues to accept `powerio-json` in `pio_parse_str` and
`pio_to_format`. Those format tokens are compatibility aliases. Removing them requires a future C ABI version change.

## Two stability tiers

A `.pio.json` file has two parts with different stability promises.

1. **Metadata** — every field except `model`. This is the versioned,
   documented surface; the metadata section below gives the policy and the
   field table.

2. **Model JSON** — the `model` field's `balanced_network` /
   `multiconductor_network` object. The model JSON is a declared schema of its
   own, named by the top-level `payload_schema` URL and versioned by
   `payload_schema_version`. A consumer that computes on model fields pins the
   model JSON version; a tool that routes or audits documents pins the metadata
   version and can keep treating the model JSON as opaque.

The two versions are independent because they change at different rates and
break different consumers: the model JSON grows whenever the IR grows (a minor
`payload_schema_version` bump), while the metadata bookkeeping barely moves.

The schema URLs are JSON Schema `$id` identifiers. The docs site also serves a
generated schema at each identifier path under `schema.json`, so consumers can
fetch a machine readable view of the serde model shape. These generated schemas
validate the model fields inside `.pio.json` documents and let consumers pin a
payload major; they do not define standalone case formats.

## The metadata: `pio-package/0.1` {#pio-package}

The `schema` field on every `.pio.json` document names the metadata schema:
`https://powerio.dev/schema/pio-package/0.1`. `schema_version` is semver; the
current value is `0.1.1`.
The generated schema is served at
`https://powerio.dev/schema/pio-package/0.1/schema.json`.

- Optional additive metadata fields (a reader that ignores them loses nothing
  it relied on before) land without a version change; `operating_points` landed
  this way. The minor version bumps when a reader needs to depend on a field
  being present.
- Metadata field moves or removals bump the major version, or ship a migration.
- A reader tolerates unknown later top-level fields (they are ignored without
  error), so a document from a newer producer still loads. A later version can
  preserve them in an extras map instead of dropping them.
- A reader accepts same major `schema_version` values and rejects a different
  major version before using the model JSON.

### Metadata Reference

| field | type | required | notes |
|---|---|---|---|
| `schema` | string (URL) | yes | identifies the `.pio.json` document metadata; defaults to the current URL on read |
| `schema_version` | string (semver) | yes | metadata version; defaults to current on read |
| `producer` | object | yes | `{tool, version, git_commit?, features[]}` |
| `package_id` | string | no | stable content id, e.g. `"sha256:..."`; unset by the scaffold |
| `created_at` | string (RFC 3339) | no | unset by default for deterministic output |
| `model_kind` | enum | yes | `balanced` \| `multiconductor`; authoritative |
| `payload_schema` | string (URL) | no | declared model JSON schema for `model_kind`; absent on pre-0.1.1 documents |
| `payload_schema_version` | string (semver) | no | model JSON version; a different major is rejected on read |
| `model` | object | yes | `{kind, <kind>_network}`; the serialized Rust model JSON |
| `origin` | object | yes | tagged by `kind`: `in_memory` \| `file` \| `folder` \| `binary_file` \| `derived` \| `composite` |
| `sources` | array | no | declared source artifacts: `{id, kind, path?, format?, hash?}` |
| `source_maps` | array | no | `{element_path, source_ref, mapping_kind, confidence}` |
| `diagnostics` | array | no | structured findings (see below) |
| `validation` | object | yes | `{status, counts, passes[]}` |
| `summary` | object | yes | `{elements{}, topology?, units?}` |
| `lowering_history` | array | no | `LoweringRecord` per pass |
| `operating_points` | object | no | replayable updates over the one static model JSON |
| `study` | object | no | ordered cumulative edits over the base payload |
| `derived` | object | no | optional matrix stats, normalized solver table metadata, and cache keys |

## Explicit model kind

`model_kind` is a standalone top-level field and is authoritative. A reader
**must** branch on it and **must not** infer the model kind from which field is
present. The model JSON is additionally self-describing: `model` is tagged by
`kind`, so `model.kind` and `model_kind` carry the same value.
`NetworkPackage::kind_is_consistent` asserts the two agree; a reader should
reject a document where they disagree.

```json
"model_kind": "balanced",
"model": { "kind": "balanced", "balanced_network": { "...": "..." } }
```

`model_kind` values: `balanced`, `multiconductor` (the enum is non-exhaustive;
later families can be added).

## The Model JSON

`payload_schema` names the model JSON schema per model kind and
`payload_schema_version` versions it, currently `1.1.0` for both kinds.
Additive optional fields bump the minor; field moves or removals bump the
major. A reader rejects a different major (or a version that does not parse as
semver) before computing on model fields. Both fields are absent on documents
written before metadata version 0.1.1; such model JSON predates the declared
schema and is accepted.

Each payload is what its Rust model serializes. The generated JSON Schema is
derived from those serde models and checked in CI against the committed
`docs/schema/**/schema.json` files. The model's rustdoc is the field reference,
and the balanced payload's wire form is additionally held to a committed
golden file by `powerio/tests/snapshot_schema.rs`.

### The Balanced Model JSON: `pio-payload-balanced/1` {#pio-payload-balanced}

`https://powerio.dev/schema/pio-payload-balanced/1` names the serde form of
`powerio::Network` under `model.balanced_network`, stamped when `model_kind`
is `balanced`: the scalar positive sequence transmission model. The tables are
`buses`, `loads`, `shunts`, `branches`, `switches`, `generators`, `storage`,
`hvdc`, `transformers_3w`, and `areas`, alongside `name`, `base_mva`,
`base_frequency`, `source_format`, and optional solver metadata. Units follow
the MATPOWER conventions: MW and MVAr power, per unit voltage magnitudes and
impedances on the system base, degree angles. Every element carries an `extras`
map for source format fields the model does not name. The field reference is the
[`powerio::Network` rustdoc](../powerio/network/struct.Network.html).
The generated schema is served at
`https://powerio.dev/schema/pio-payload-balanced/1/schema.json`.

### The Multiconductor Model JSON: `pio-payload-multiconductor/2` {#pio-payload-multiconductor}

`https://powerio.dev/schema/pio-payload-multiconductor/2` names the serde form
of `powerio_dist::DistNetwork` under `model.multiconductor_network`, stamped
when `model_kind` is `multiconductor`: the wire coordinate distribution model,
in SI units with radian angles. [Compiler IR](compiler-ir.md) describes the
model family. The field reference is the
[`powerio_dist::DistNetwork` rustdoc](../powerio_dist/model/struct.DistNetwork.html).
The generated schema is served at
`https://powerio.dev/schema/pio-payload-multiconductor/2/schema.json`; `/1`
(the vintage before the BMOPF schema 0.1.0 alignment renamed the bus
symmetrical component bounds) stays served for documents that declare it.
Do not extract this object as a distribution case file. Use `.pio.json` for
PowerIO artifacts; when a receiving tool expects BMOPF, PMD JSON, or OpenDSS,
write that case format through `powerio convert`.

## Row identity

Every row of every balanced model table except `areas`
carries a `uid` string: the source record uid where the format defines one
(GOC3), and a `{table}:{row}` value synthesized at document build otherwise. A
synthesized uid records the row the element had when the document was built and
sticks to the element from then on. Uids are unique per table; a duplicate is a
validation error. Operating point updates resolve against these identities
(below). Rows in documents written before 0.1.1 carry no `uid`, which is what
keeps their row-addressed operating points valid.

## Operating points

`operating_points` records a time axis and an ordered list of model field
updates. A point names a table, a row identity and/or a zero based row, and the
fields to overwrite. Materializing a point clones the static model, applies
those field updates, and clears `operating_points` in the returned document.

Updates resolve by identity first. When the referenced table carries `uid`
values, `element.source_uid` is authoritative: it selects the row, a present
`element.row` must agree with the resolved row, and an unknown or duplicated
uid is an error (reported by validation and fatal to materialization). A
producer that knows the identity can omit `row` entirely. When the table
carries no uids (documents written before 0.1.1), `source_uid` is advisory and
`row` addresses the update alone. An update may not overwrite `uid` itself, and
an element ref with neither `row` nor `source_uid` does not parse.

The block shape is:

| field | type | notes |
|---|---|---|
| `time_axis.periods` | integer | number of available operating points |
| `time_axis.duration_hours` | array of numbers | optional per period duration |
| `time_axis.labels` | array of strings | optional labels, such as `"1"`, `"2"`, ... |
| `points[]` | array | one replayable state |
| `points[].index` | integer | zero based period index; addresses `time_axis.duration_hours` and `time_axis.labels` |
| `points[].updates[]` | array | row field updates to apply for this point |
| `updates[].element.table` | string | model table name, such as `generators`, `loads`, `branches`, or `hvdc` |
| `updates[].element.row` | integer | zero based row; optional when `source_uid` is present, then a consistency check |
| `updates[].element.source_uid` | string | the target row's model identity (`uid`); authoritative when the table carries uids |
| `updates[].fields` | object | field names and JSON values to overwrite |
| `metadata` | object | optional series or point metadata |

GO Challenge 3 documents use this block for the scheduling time series. The
static `model` reflects the first interval that can be represented by
`Network`; `operating_points` carries replayable updates for every interval.
`NetworkPackage::materialize_operating_point(index)` returns a new static
document with `origin.kind = "derived"` and
`origin.pass = "materialize-operating-point"`.

```json
"operating_points": {
  "time_axis": { "periods": 2, "duration_hours": [1.0, 1.0], "labels": ["1", "2"] },
  "points": [
    { "index": 0, "updates": [] },
    { "index": 1,
      "updates": [
        { "element": { "table": "loads", "row": 0, "source_uid": "device_1" },
          "fields": { "p": 12.5, "q": 3.2 } }
      ] }
  ],
  "metadata": { "source_format": "goc3-json" }
}
```

## Study commits

`study` stores ordered cumulative edits to a balanced model payload.
Materializing commit `k` applies commits 0 through `k`, clears the study and
operating point blocks, and returns a static package. Study commits differ from
operating points, which are independent overlays. See [Study blocks](study-block.md) for edit kinds,
identity resolution, materialization, and language APIs.

## Derived metadata

`derived.normalized_solver_tables` records the compact identity metadata for
`powerio::Network::to_normalized_solver_tables()` without embedding every table
row in the document. The full tables are a derived artifact; this metadata lets a
compiler cache prove it was built from the same lowering pass and row order.

The block carries:

- `pass`: `"balanced-to-normalized-solver-tables"`;
- `units`: per unit power, per unit voltage, radian angles, per unit impedance
  and admittance, zero based dense indices;
- `row_counts`: counts for buses, loads, shunts, branches, switches, arcs,
  generators, storage, and HVDC rows;
- `bus_ids`, `reference_bus_indices`, and `component_labels`;
- `branch_from_arc_indices` and `branch_to_arc_indices`;
- `source_rows`: source row indices for rows that survived normalization, with
  `null` for synthetic rows such as 3-winding star buses and branches.

## Diagnostics

Each diagnostic carries a stable dotted `code`, a `severity` (`debug`, `info`,
`warning`, `error`, `fatal`; ordered worst-last), the `stage` it came from
(`parse`, `read`, `canonicalize`, `validate`, `lower`, `emit`, `bind`,
`partner`), a human `message`, and where known an `element_path`, a `source_ref`,
a `details` object, a `suggested_action`, and a `safe_to_ignore` list. Code
namespaces by leading segment: `PARSE`, `READ`, `IR`, `VALIDATE`, `FIDELITY`,
`LOWER`, `EMIT`, `BINDING`, `PARTNER`, `PERF`.

## Source maps

A `source_map` entry records where a canonical field came from: an `element_path`
(a JSON pointer, or a best-effort locator in v0.1), a `source_ref` into a declared
source, a `mapping_kind` (`exact`, `defaulted`, `inferred`, `converted_units`,
`lowered`, `aggregated`, `split`, `synthetic`, `retained_extra`), and a
`confidence` (`exact`, `high`, `medium`, `low`). Balanced documents emit source
maps for stable bus, load, shunt, branch, and generator fields. Balanced
`source_ref.field` values use the same canonical field names as the model JSON, so
they can be compared directly with `element_path`. When a source format folds
several canonical elements into one source row, the source map records that
relation with another mapping kind; MATPOWER load and shunt fields use
`mapping_kind = split` and point to the bus record while keeping fields such as
`p`, `q`, `g`, and `b`. Values that the source format does not carry are not
mapped as exact; MATPOWER `base_frequency` has no source map. When a
multiconductor network is written as `.pio.json`, its `defaulted` fields lift into source maps
with `mapping_kind = defaulted`, and its retained source becomes
`origin.retained_source`. Validation diagnostics attach the matching `source_ref`
when the document has a source map for the reported field.

`NetworkPackage::lower_multiconductor_to_balanced(options)` returns a new
balanced document with `origin.kind = derived` and
`origin.pass = "multiconductor-to-balanced"`. It preserves the parent
`lowering_history` and appends a `LoweringRecord` whose options, assumptions,
approximations, dropped fields, diagnostics, and validation status describe the
pass. Lowered balanced source maps use `lowered`, `aggregated`,
`converted_units`, `synthetic`, and `defaulted` mapping kinds. The pass is never
implicit during `.pio.json` readback, format conversion, matrix construction,
bindings, or MCP operations.

## Example

```json
{
  "schema": "https://powerio.dev/schema/pio-package/0.1",
  "schema_version": "0.1.1",
  "producer": { "tool": "powerio", "version": "0.7.0" },
  "model_kind": "multiconductor",
  "payload_schema": "https://powerio.dev/schema/pio-payload-multiconductor/2",
  "payload_schema_version": "2.0.0",
  "model": {
    "kind": "multiconductor",
    "multiconductor_network": {
      "base_frequency": 60.0,
      "loads": [
        { "name": "l1", "bus": "b1", "configuration": "wye",
          "voltage_model": { "model": "zip", "v_nom": [230.0], "alpha_z": [0.5], "...": "..." } }
      ]
    }
  },
  "origin": { "kind": "file", "format": "dss", "retained_source": true },
  "sources": [ { "id": "src0", "kind": "file", "format": "dss" } ],
  "source_maps": [
    { "element_path": "/model/multiconductor_network/vsource.source#basekv",
      "source_ref": { "source_id": "src0", "field": "basekv" },
      "mapping_kind": "defaulted", "confidence": "high" }
  ],
  "validation": { "status": "ok", "counts": { "fatal": 0, "error": 0, "warning": 0, "info": 0, "debug": 0 } },
  "summary": { "elements": { "buses": 1, "loads": 1 }, "units": { "power": "W/var", "angle": "radians" } }
}
```
