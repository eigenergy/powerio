# The PowerIO compiler IR

PowerIO is organized as a compiler for power system data: frontends parse source
formats into typed IR, passes normalize and lower it, and backends emit target
artifacts. This document defines the IR boundaries and the `.pio.json` compiler
package that ties them together. The field-level reference for the package is in
[the PIO JSON schema guide](https://eigenergy.github.io/powerio/guide/pio-json-schema.html).

The governing decision is that there is no single flattened universal `Network`
mega-struct. There are concrete model families, and a package object that wraps
one payload at a time with the metadata that makes a compiler artifact
trustworthy.

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

## The compiler package (`.pio.json`)

`powerio_pkg::CompilerPackage` is the readable envelope. It is the object that
records how a source was interpreted, and it is the interchange layer for
language bindings. Binary `.pio` is out of scope until the JSON package
stabilizes.

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
- optional `derived` metadata.

### Explicit model kind

`model_kind` is a standalone field. A reader must never infer whether the payload
is balanced or multiconductor from which field is present. The payload enum is
also tagged by `kind`, so the payload is self-describing too;
`CompilerPackage::kind_is_consistent` asserts the two agree, and a reader should
reject a package where they do not.

### Payload stability

The envelope is the versioned, documented surface. The nested `balanced_network`
/ `multiconductor_network` payloads are direct serde snapshots of the live
PowerIO Rust IR and are experimental until a v1 payload schema is declared; they
grow whenever the IR grows. See
[the PIO JSON schema guide](https://eigenergy.github.io/powerio/guide/pio-json-schema.html).

### Provenance and source maps

`Origin` distinguishes an in-memory model, a single file (with or without
retained source), a folder dataset, a partially decoded binary, a derived
product, or a composite. A `SourceMapEntry` answers "where did this canonical
field come from?" with an `element_path`, a `SourceRef` into a declared source, a
`mapping_kind` (`exact`, `defaulted`, `inferred`, `converted_units`, `lowered`,
`aggregated`, `split`, `synthetic`, `retained_extra`), and a `confidence`. Parser
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

Every pass that transforms one model into another appends a `LoweringRecord`
(input and output kind, options, assumptions, approximations, dropped fields,
diagnostics, validation status) to `lowering_history`, so the transformation is
auditable rather than implicit. This change set defines the record shape; the
passes themselves are later work. The v0.4.0 design direction for
`MulticonductorNetwork` to `BalancedNetwork` lowering is in
[the v0.4 release direction](https://eigenergy.github.io/powerio/guide/v0.4-release-direction.html); the implementation is
tracked in #145.

## Versioning

`schema_version` is semver. Additive envelope fields bump the minor; field moves
bump the major or ship a migration. Unknown future top-level fields are tolerated
on read (ignored), so a package from a newer producer still deserializes when
the `schema_version` major version matches. A different major version is
rejected before payload use.
