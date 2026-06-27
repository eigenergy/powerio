# The PowerIO compiler IR

PowerIO is organized as a compiler for power system data: frontends parse source
artifacts into typed IR, passes normalize and lower it, and backends emit target
artifacts. This document defines the v1 IR boundaries and the `.pio.json`
compiler package that ties them together.

The governing decision is that there is no single flattened universal `Network`
mega-struct. There are concrete model families, and a package object that wraps
one payload at a time with the metadata that makes a compiler artifact
trustworthy.

## Layers

1. **Frontends** parse a source format (MATPOWER, PSS/E, PowerWorld, PSLF,
   OpenDSS, PowerModels JSON, PMD JSON, BMOPF JSON, egret, pandapower, PyPSA,
   GridFM) into a typed payload plus, where available, retained source, source
   maps, structured diagnostics, and origin metadata.
2. **Concrete electrical IRs** are the typed payloads. v1 has two, and they stay
   distinct.
3. **The compiler package** (`.pio.json`) wraps one payload with explicit model
   kind, provenance, diagnostics, validation, and lowering history.
4. **Passes** validate, normalize, and lower the IR. Lowering is recorded.
5. **Backends** emit target artifacts (a source format, matrices, Arrow tables,
   a CIM-like export) from the IR.

## Model families

### `BalancedNetwork`

`powerio::BalancedNetwork` is the scalar positive-sequence model for transmission
power flow, OPF, matrices, and graph analysis. It is an alias of the historical
`powerio::Network`; the struct is unchanged. Every electrical quantity is a
single `f64`, with no phase or conductor dimension. External bus ids are not
dense matrix indices; the dense solver view (`IndexedNetwork`) is derived
separately and preserves external ids. Loads and shunts are first-class records,
not folded onto bus rows.

### `MulticonductorNetwork`

`powerio_dist::MulticonductorNetwork` is the wire-coordinate model for OpenDSS,
PMD, and conductor-level distribution OPF, including BMOPF's up-to-four-wire
semantics. It is an alias of the historical `powerio_dist::DistNetwork`; the
struct is unchanged. Bus ids are strings; terminals are ordered string names;
every element carries a terminal map; grounding is explicit; units are SI and
radians. A neutral is not just another phase: it carries grounding and reduction
semantics. OpenDSS defaults and inferred facts are tracked (`defaulted`), and
unsupported objects are preserved (`untyped`) rather than dropped.

The two families never merge into one struct. A balanced model cannot represent
conductor-level asymmetry; a multiconductor model carries terminal and grounding
data that has no place in a positive-sequence struct. Code that needs both holds
a `CompilerPackage`, not a union type.

## The compiler package (`.pio.json`)

`powerio_pkg::CompilerPackage` is the readable envelope. It is not just another
case format; it is the object that records how a source was interpreted, and it
is the interchange layer for language bindings and PowerMCP. Binary `.pio` is out
of scope until the JSON package stabilizes.

A package always carries:

- `schema` (URL) and `schema_version` (semver);
- `producer` (tool, version, optional git commit, features);
- `model_kind`, explicit and authoritative;
- `model`, the one typed payload, tagged by `kind`;
- `origin` and `sources`;
- `source_maps`;
- `diagnostics`;
- `validation`;
- `summary`;
- `lowering_history`;
- optional `derived` metadata (matrix stats, cache keys).

### Explicit model kind

`model_kind` is a standalone field. A reader must never infer whether the payload
is balanced or multiconductor from which field is present. The payload enum is
also tagged by `kind`, so the payload is self-describing too;
`CompilerPackage::kind_is_consistent` asserts the two agree, and a reader should
reject a package where they do not.

```json
{
  "schema": "https://powerio.dev/schema/pio-package/0.1",
  "schema_version": "0.1.0",
  "producer": { "tool": "powerio", "version": "0.3.3" },
  "model_kind": "balanced",
  "model": {
    "kind": "balanced",
    "balanced_network": { "name": "case", "base_mva": 100.0, "...": "..." }
  },
  "origin": { "kind": "file", "path": "case.raw", "format": "psse", "retained_source": true },
  "sources": [ { "id": "src0", "kind": "file", "path": "case.raw", "format": "psse" } ],
  "source_maps": [],
  "diagnostics": [],
  "validation": { "status": "ok", "counts": { "fatal": 0, "error": 0, "warning": 0, "info": 0, "debug": 0 } },
  "summary": { "elements": { "buses": 118, "branches": 186, "generators": 54 } }
}
```

### Provenance and source maps

`Origin` is an internally tagged enum: `InMemory`, `File`, `Folder`,
`BinaryFile`, `Derived` (a lowering product), `Composite`. It distinguishes
retained source from canonical regenerated output, folder datasets from single
files, and derived models from parsed ones.

A `SourceMapEntry` answers "where did this canonical field come from?" with an
`element_path` (a JSON pointer, or a best-effort locator in v0.1), a `SourceRef`
into a declared source, a `mapping_kind`, and a `confidence`. The `mapping_kind`
vocabulary is the key provenance distinction: `exact`, `defaulted`, `inferred`,
`converted_units`, `lowered`, `aggregated`, `split`, `synthetic`,
`retained_extra`. A field materialized from an OpenDSS default is `defaulted`; a
positive-sequence equivalent produced by lowering is `lowered`; ohms converted to
per unit are `converted_units`.

When a `MulticonductorNetwork` is wrapped, its `defaulted` map lifts into source
maps with `mapping_kind = defaulted`, and its parse `warnings` lift into
structured diagnostics. That is why those two fields are skipped in the IR
payload (see ADR 0001): they are parser bookkeeping that belongs in the
envelope's provenance layer, not in the raw IR.

### Structured diagnostics

Every finding carries a stable dotted `code`, a `severity`, the `stage` it came
from, a human `message`, and where known an `element_path`, a `source_ref`, a
`details` object, and a `suggested_action`. Human-readable warnings are rendered
from these, not the other way around.

Severity, worst-last so a set's dominant severity is its max: `debug`, `info`,
`warning`, `error`, `fatal`. `fatal` means the package could not be produced;
`error` means it exists but is not valid for the intended use without repair;
`warning` means semantics were defaulted, approximated, or lost; `info` is a
provenance event worth recording.

Code namespaces, by leading segment: `PARSE`, `READ`, `IR`, `VALIDATE`,
`FIDELITY`, `LOWER`, `EMIT`, `BINDING`, `PARTNER`, `PERF`. The conventional shape
is `NAMESPACE.SOURCE_OR_TARGET.SPECIFIC`, e.g. `EMIT.BMOPF.UNSUPPORTED_STORAGE`,
`LOWER.MULTI.REJECT_SINGLE_PHASE_EQUIVALENT`, `VALIDATE.MULTI.TERMINAL_ARITY`.

### Lowering

Lowering is where PowerIO is a compiler rather than a parser. Every pass that
transforms one model into another appends a `LoweringRecord` (input and output
kind, options, assumptions, approximations, dropped fields, diagnostics,
validation status) to `lowering_history`.

The multiconductor-to-balanced reduction is the most consequential lowering and
must be explicit and diagnostic-rich, never a silent positive-sequence
projection. The contract: never assume every feeder has a unique
positive-sequence equivalent; apply Kron reduction before a sequence transform
when a neutral is present; reject or warn on ambiguous one-wire and two-wire
cases; record every reduction, approximation, and dropped conductor-level
constraint. This change set defines the `LoweringRecord` shape; the pass itself
is later work.

## Versioning

`schema_version` is semver. Additive fields bump the minor; field moves bump the
major or ship a migration pass. Unknown future top-level fields are tolerated on
read (ignored), so a package from a newer producer still deserializes; a future
version may preserve them in an extras map instead.

## Naming

The historical names are kept; the v1 names `BalancedNetwork` and
`MulticonductorNetwork` are aliases (introduced by PR #143, ahead of any breaking
rename). See `migration-v1.md` for the full table across Rust, Python, C ABI,
Julia, MCP, and CLI. The top-level `Network` identifier is not repurposed as the
envelope yet, because `powerio::Network` still means the balanced model and
repurposing it would break callers. The envelope is `powerio_pkg::CompilerPackage`
until that rename can be staged safely.

## Payload stability

The `.pio.json` envelope is versioned and documented; the nested
`balanced_network` / `multiconductor_network` payloads are direct serde snapshots
of the live PowerIO Rust IR and are experimental until a v1 payload schema is
declared. They grow with the IR (PR #143 enlarged both). See
`pio-json-schema.md` for the field reference and the stability policy.

## Adapter seams

The package is the consumption boundary for partners:

- **PowerMCP** traffics in `.pio.json` packages and summaries rather than ad hoc
  JSON strings: `parse(path) -> {package, summary, diagnostics}`, then
  `summary` / `normalize` / `matrix` / `save` accept a package.
- **BMOPFTools** consumes BMOPF JSON emitted by PowerIO and keeps its own schema
  mirror at the boundary; `MulticonductorNetwork -> BMOPF JSON` is an emission
  pass with diagnostics (see the reconciliation table).
- **ExaModelsPower** consumes normalized balanced tables / Arrow / named tuples
  through the hot path, not JSON, and does not duplicate the parser.
- **MG-RAVENS** is an export target (`BalancedNetwork -> RAVENS JSON`), not the
  internal IR; CIM stays external.
