# The PowerIO compiler IR

PowerIO is organized as a compiler for power system data: frontends parse source
formats into typed IR, passes normalize and lower it, and backends emit target
artifacts. The IR boundaries and the `.pio.json` package are below. The field
reference for the package is in
[the PIO JSON schema guide](https://eigenergy.github.io/powerio/guide/pio-json-schema.html).

There is no flattened universal `Network` mega-struct. PowerIO keeps concrete
model families separate. The package wraps one payload at a time with source,
diagnostic, validation, and lowering metadata.

## Model families

PowerIO keeps two concrete static-grid IR families distinct. They share
conventions, not types; code that needs both holds a package, not a union struct.

### `BalancedNetwork`

`powerio::BalancedNetwork` (an alias of `powerio::Network`) is the scalar
positive-sequence model for transmission power flow, OPF, matrices, and graph
analysis. Every electrical quantity is a single `f64`, with no phase or conductor
dimension. External bus ids are not dense matrix indices; the dense solver view
is derived separately and preserves external ids. Loads and shunts are
first class records, not folded onto bus rows.

### `MulticonductorNetwork`

`powerio_dist::MulticonductorNetwork` (an alias of `powerio_dist::DistNetwork`)
is the wire-coordinate model for conductor-level distribution. Bus ids are
strings; terminals are ordered string names; every element carries a terminal
map; grounding is explicit; units are SI and radians. A neutral is not just
another phase; it carries grounding and reduction semantics. Format defaults and
inferred facts are tracked, and unsupported objects are preserved rather than
dropped.

A balanced model cannot represent conductor-level asymmetry; a multiconductor
model carries terminal and grounding data that has no place in a positive
sequence struct. The two families never merge into one struct.

BMOPF JSON is the strict exchange format for the distribution family. The
`.pio.json` package uses the same `MulticonductorNetwork` model and wraps it
with compiler metadata: model kind, provenance, source maps, diagnostics,
validation, and lowering history.

## The compiler package (`.pio.json`)

`powerio_pkg::CompilerPackage` is the readable envelope. It is the object that
records how a source was interpreted. Language bindings can pass the package
without guessing whether it holds balanced or multiconductor data. Binary `.pio`
is out of scope until the JSON package settles.

A package always carries:

- `schema` (URL) and `schema_version` (semver);
- `producer` metadata;
- `model_kind`, explicit and authoritative;
- `model`, the one typed payload, tagged by `kind`;
- `origin` and `sources`;
- `source_maps`;
- `diagnostics`;
- `validation`;
- `summary`;
- `lowering_history`;
- optional `operating_points`;
- optional `derived` metadata.

`operating_points` is a format neutral series of replayable field updates over
the package's single static payload. Materializing one point returns a static
package with those updates applied and the series cleared.
GO Challenge 3 package construction fills this block from `time_series_input`:
the balanced payload holds the first interval, while every interval is available
as an operating point.

For balanced payloads, `CompilerPackage::attach_normalized_solver_table_metadata`
records the compact contract for
`powerio::Network::to_normalized_solver_tables()`: pass name, units, row counts,
dense bus ids, reference/component indices, branch to arc indices, and source row
provenance. The package does not duplicate the full table rows; it records enough
metadata for a compiler cache or sidecar artifact to verify table identity.

### Explicit model kind

`model_kind` is a standalone field. A reader must never infer whether the payload
is balanced or multiconductor from which field is present. The payload enum is
also tagged by `kind`, so the payload is self-describing too;
`CompilerPackage::kind_is_consistent` asserts the two agree, and a reader should
reject a package where they do not.

### Payload stability

The envelope is the versioned, documented surface. The nested `balanced_network`
/ `multiconductor_network` payloads are direct serde snapshots of the live
PowerIO Rust IR. They can change whenever the Rust models change, until a v1
payload schema is declared. See
[the PIO JSON schema guide](https://eigenergy.github.io/powerio/guide/pio-json-schema.html).

### Provenance and source maps

`Origin` distinguishes an in-memory model, a single file (with or without
retained source), a folder dataset, a partially decoded binary, a derived
product, or a composite. A `SourceMapEntry` points from a payload field to its
source with an `element_path`, a `SourceRef` into a declared source, a
`mapping_kind` (`exact`, `defaulted`, `inferred`, `converted_units`, `lowered`,
`aggregated`, `split`, `synthetic`, `retained_extra`), and a `confidence`.
Balanced `source_ref.field` values use canonical payload field names. Parser
bookkeeping that should not live in the IR payload (retained source text,
default-materialization records) is lifted into this layer rather than the raw
payload.

### Structured diagnostics

Every finding carries a stable dotted `code`, a `severity` (`debug`, `info`,
`warning`, `error`, `fatal`; worst-last so a set's dominant severity is its max),
the `stage` it came from, a human `message`, and where known an `element_path`, a
`source_ref`, a `details` object, and a `suggested_action`. Human-readable
warnings are rendered from these, not the other way around. Codes are namespaced
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
return structured `LOWER.MULTI_TO_BALANCED.*` diagnostics. The package method
`CompilerPackage::lower_multiconductor_to_balanced` returns a derived balanced
package and appends the record. This pass is explicit only; readers, writers,
matrix builders, bindings, and MCP operations do not run it implicitly. The
v0.4.0 direction is in
[the v0.4 release direction](https://eigenergy.github.io/powerio/guide/v0.4-release-direction.html).

### Operating point materialization

`CompilerPackage::materialize_operating_point(index)` clones the package, applies
one point's field updates to the typed payload, clears `operating_points`, drops
stale source maps and diagnostics for changed fields, recomputes validation, and
records a `LoweringRecord` with `pass = "materialize-operating-point"`. If the
package already carried normalized solver table metadata, the metadata is
rebuilt for the updated static payload.

## Versioning

`schema_version` is semver. Optional additive envelope fields land without a
version change (`operating_points` did); the minor bumps when a reader needs to
depend on a field being present; field moves bump the major or ship a migration. Unknown future top-level fields are tolerated
on read (ignored), so a package from a newer producer still deserializes when
the `schema_version` major version matches. A different major version is
rejected before payload use.
