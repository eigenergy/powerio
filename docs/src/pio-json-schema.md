# The `.pio.json` schema

`.pio.json` is the serialized form of `powerio_pkg::CompilerPackage`: a versioned
envelope around one PowerIO IR payload. The envelope shape and stability policy
are below. The crate is the implementation; `compiler-ir.md` is the architecture
note.

## Two stability tiers

A `.pio.json` file has two parts with different stability promises.

1. **The envelope** — every field except `model`. This is the versioned,
   documented surface: `schema`, `schema_version`, `producer`, `model_kind`,
   `origin`, `sources`, `source_maps`, `diagnostics`, `validation`, `summary`,
   `lowering_history`, `operating_points`, `derived`. Its shape changes only
   under the versioning policy below.

2. **The payload** — the `model` field's `balanced_network` /
   `multiconductor_network` object. This is a direct serde snapshot of the live
   PowerIO Rust IR (`powerio::Network` / `powerio_dist::DistNetwork`). It can
   grow whenever the IR grows. Adding a typed field to a model appears in the
   payload with no envelope version change. Until a v1 payload schema exists,
   tools that need a stable contract should read the envelope (model kind,
   summary, diagnostics, provenance) and treat the payload as opaque or pinned
   to a producer version.

`schema_version` versions the envelope only. The payload carries no separate
version yet; pin to `producer.version` if you depend on payload fields.

For distribution models, the payload follows the same multiconductor model used
by the BMOPF reader and writer. The surrounding `.pio.json` object is the
PowerIO package envelope: it adds model kind, provenance, source maps,
diagnostics, validation, and lowering metadata around that model. Use the
`bmopf-json` writer when a standalone BMOPF exchange file is needed.

## Versioning policy (envelope)

- `schema_version` is semver. The current value is `0.1.0`; the `schema` URL is
  `https://powerio.dev/schema/pio-package/0.1`.
- Optional additive envelope fields (a reader that ignores them loses nothing
  it relied on before) land without a version change; `operating_points` landed
  this way. The minor version bumps when a reader needs to depend on a field
  being present.
- Envelope field moves or removals bump the major version, or ship a migration.
- A reader tolerates unknown later top-level fields (they are ignored, not an
  error), so a package from a newer producer still loads. A later version can
  preserve them in an extras map instead of dropping them.
- A reader accepts same major `schema_version` values and rejects a different
  major version before using the payload.
- Every package states `producer.version` and `schema_version`.

## Explicit model kind

`model_kind` is a standalone top-level field and is authoritative. A reader
**must** branch on it and **must not** infer the payload kind from which field is
present. The payload is additionally self-describing: `model` is tagged by
`kind`, so `model.kind` and `model_kind` carry the same value.
`CompilerPackage::kind_is_consistent` asserts the two agree; a reader should
reject a package where they disagree.

```json
"model_kind": "balanced",
"model": { "kind": "balanced", "balanced_network": { "...": "..." } }
```

`model_kind` values: `balanced`, `multiconductor` (the enum is non-exhaustive;
later families can be added).

## Envelope reference

| field | type | required | notes |
|---|---|---|---|
| `schema` | string (URL) | yes | identifies the package format; defaults to the current URL on read |
| `schema_version` | string (semver) | yes | envelope version; defaults to current on read |
| `producer` | object | yes | `{tool, version, git_commit?, features[]}` |
| `package_id` | string | no | stable content id, e.g. `"sha256:..."`; unset by the scaffold |
| `created_at` | string (RFC 3339) | no | unset by default for deterministic output |
| `model_kind` | enum | yes | `balanced` \| `multiconductor`; authoritative |
| `model` | object | yes | `{kind, <kind>_network}`; follows the Rust model payload |
| `origin` | object | yes | tagged by `kind`: `in_memory` \| `file` \| `folder` \| `binary_file` \| `derived` \| `composite` |
| `sources` | array | no | declared source artifacts: `{id, kind, path?, format?, hash?}` |
| `source_maps` | array | no | `{element_path, source_ref, mapping_kind, confidence}` |
| `diagnostics` | array | no | structured findings (see below) |
| `validation` | object | yes | `{status, counts, passes[]}` |
| `summary` | object | yes | `{elements{}, topology?, units?}` |
| `lowering_history` | array | no | `LoweringRecord` per pass |
| `operating_points` | object | no | replayable updates over the one static payload |
| `derived` | object | no | optional matrix stats, normalized solver table metadata, and cache keys |

### Operating points

`operating_points` records a time axis and an ordered list of payload field
updates. A point names a table, zero based row, optional source UID, and the
fields to overwrite. Materializing a point clones the static payload, applies
those field updates, and clears `operating_points` in the returned package.

The block shape is:

| field | type | notes |
|---|---|---|
| `time_axis.periods` | integer | number of available operating points |
| `time_axis.duration_hours` | array of numbers | optional per period duration |
| `time_axis.labels` | array of strings | optional labels, such as `"1"`, `"2"`, ... |
| `points[]` | array | one replayable state |
| `points[].index` | integer | zero based period index |
| `points[].label` | string | optional point label |
| `points[].duration_hours` | number | optional point duration |
| `points[].updates[]` | array | row field updates to apply for this point |
| `updates[].element.table` | string | payload table name, such as `generators`, `loads`, `branches`, or `hvdc` |
| `updates[].element.row` | integer | zero based row in that table |
| `updates[].element.source_uid` | string | optional source record UID |
| `updates[].fields` | object | field names and JSON values to overwrite |
| `metadata` | object | optional series or point metadata |

GO Challenge 3 packages use this block for the scheduling time series. The
static `model` reflects the first interval that can be represented by
`Network`; `operating_points` carries replayable updates for every interval.
`CompilerPackage::materialize_operating_point(index)` returns a new static
package with `origin.kind = "derived"` and
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

### Derived metadata

`derived.normalized_solver_tables` records the compact identity metadata for
`powerio::Network::to_normalized_solver_tables()` without embedding every table
row in the package. The full tables are a derived artifact; this metadata lets a
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

### Diagnostics

Each diagnostic carries a stable dotted `code`, a `severity` (`debug`, `info`,
`warning`, `error`, `fatal`; ordered worst-last), the `stage` it came from
(`parse`, `read`, `canonicalize`, `validate`, `lower`, `emit`, `bind`,
`partner`), a human `message`, and where known an `element_path`, a `source_ref`,
a `details` object, a `suggested_action`, and a `safe_to_ignore` list. Code
namespaces by leading segment: `PARSE`, `READ`, `IR`, `VALIDATE`, `FIDELITY`,
`LOWER`, `EMIT`, `BINDING`, `PARTNER`, `PERF`.

### Source maps

A `source_map` entry records where a canonical field came from: an `element_path`
(a JSON pointer, or a best-effort locator in v0.1), a `source_ref` into a declared
source, a `mapping_kind` (`exact`, `defaulted`, `inferred`, `converted_units`,
`lowered`, `aggregated`, `split`, `synthetic`, `retained_extra`), and a
`confidence` (`exact`, `high`, `medium`, `low`). Balanced packages emit source
maps for stable bus, load, shunt, branch, and generator fields. Balanced
`source_ref.field` values use the same canonical field names as the payload, so
they can be compared directly with `element_path`. When a source format folds
several canonical elements into one source row, the source map records that
relation with another mapping kind; MATPOWER load and shunt fields use
`mapping_kind = split` and point to the bus record while keeping fields such as
`p`, `q`, `g`, and `b`. Values that the source format does not carry are not
mapped as exact; MATPOWER `base_frequency` has no source map. When a
multiconductor network is packaged, its `defaulted` fields lift into source maps
with `mapping_kind = defaulted`, and its retained source becomes
`origin.retained_source`. Validation diagnostics attach the matching `source_ref`
when the package has a source map for the reported field.

`CompilerPackage::lower_multiconductor_to_balanced(options)` returns a new
balanced package with `origin.kind = derived` and
`origin.pass = "multiconductor-to-balanced"`. It preserves the parent
`lowering_history` and appends a `LoweringRecord` whose options, assumptions,
approximations, dropped fields, diagnostics, and validation status describe the
pass. Lowered balanced source maps use `lowered`, `aggregated`,
`converted_units`, `synthetic`, and `defaulted` mapping kinds. The pass is never
implicit during package readback, format conversion, matrix construction,
bindings, or MCP operations.

## Example

```json
{
  "schema": "https://powerio.dev/schema/pio-package/0.1",
  "schema_version": "0.1.0",
  "producer": { "tool": "powerio", "version": "0.4.0" },
  "model_kind": "multiconductor",
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
