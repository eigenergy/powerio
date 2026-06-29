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
   `lowering_history`, `derived`. Its shape changes only under the versioning
   policy below.

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
- Additive envelope fields bump the minor version.
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
| `derived` | object | no | optional matrix stats / cache keys |

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
marked as exact source fields; MATPOWER `base_frequency` has no source map. When
a multiconductor network is packaged, its `defaulted` fields lift into source
maps with `mapping_kind = defaulted`, and its retained source becomes
`origin.retained_source`. Validation diagnostics attach the matching
`source_ref` when the package has a source map for the reported field.

## Example

```json
{
  "schema": "https://powerio.dev/schema/pio-package/0.1",
  "schema_version": "0.1.0",
  "producer": { "tool": "powerio", "version": "0.3.3" },
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
