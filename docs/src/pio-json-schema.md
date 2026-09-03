# PowerIO IR {#pio-module}

A `.pio.json` file serializes one `PioModule<PioValue>`: one typed value with
its diagnostics, producer, sources, source mappings, history, and extensions.
Beginning with PowerIO v0.11.0, the document version equals the `powerio` crate
version. The current document shape begins:

```json
{
  "schema": "powerio.module",
  "version": "0.11.0",
  "producer": { "name": "powerio", "version": "0.11.0" },
  "value": {
    "type": "powerio.BalancedNetwork",
    "data": {}
  }
}
```

Use `serialize` to produce it and `deserialize` to read it. PowerIO v0.11.0
reads the v0.11.0 document shape.

The generated JSON Schema is checked in at
`docs/schema/pio-module/0.11.0/schema.json` and served from
`https://powerio.dev/schema/pio-module/0.11.0/schema.json`. The checked-in schema,
the serializer, and the deserializer are tested from the same Rust types.
`docs/schema/README.md` records the earlier `pio-package` and
`powerio.module` documents as one history.

## PowerIO IR is not a grid exchange format

MATPOWER, PSS/E, XIIDM, CGMES, OpenDSS, PMD JSON, BMOPF, and the other formats
in the format registry exchange power system data with other tools. They enter
through `parse` and leave through `emit`.

PowerIO IR preserves PowerIO types and module records. It is deliberately
absent from grid exchange format discovery. Use it when both sides consume
PowerIO values, including calculation instances, solutions, time series, and
scenario sets.

## Module records

The document stores these common records when present:

| field | meaning |
|---|---|
| `producer` | the software operation that created this module |
| `sources` | source names, sizes, declared formats, and digests |
| `source_map` | JSON Pointer paths in `value.data` mapped to source byte ranges |
| `diagnostics` | structured findings with stable codes and severities |
| `history` | ordered derivations that produced the current value |
| `extensions` | namespaced data outside the PowerIO core schema |

Runtime retained source bytes are not serialized. A source record names a
source without exposing a local absolute path. A parser can retain bytes in
memory for byte exact same format emission while the process is running; that
buffer is separate from the stored module.

Diagnostic source references and source mappings must name declared source
IDs. Byte ranges must fit the declared source length. History references must
name records present in the same document. The deserializer validates those
relationships before returning a module. The MATPOWER and PSS/E readers
attach the byte range of the record a finding is about to every diagnostic
they raise at a known record, so a document serialized from their modules
carries those spans; the other readers attach none yet.

## Typed values

`value.type` is the canonical structural type name used by Rust, C, Python,
Julia, and the document schema. Examples include:

```text
powerio.BalancedNetwork
powerio.MulticonductorNetwork
powerio.OperatingPoint<powerio.BalancedNetwork>
powerio.TimeSeries<powerio.MulticonductorNetwork>
powerio.ScenarioSet<powerio.TimeSeries<powerio.BalancedNetwork>>
powerio.DcOpfInstance
powerio.SocwrOpfSolution
```

`value.data` has the exact shape for that type. A structural type name and its
data must agree. An incorrect schema name or version, an unknown PowerIO type,
duplicate identities, invalid references, nonfinite values in untyped
positions, or a collection whose entries disagree with its element type is
rejected. [PowerIO IR reference](ir-reference.md) defines every structural
type field by field: type, unit, sign convention, invariant, and the value a
reader takes when a field is absent.

Typed floating point fields spell nonfinite values as `"Infinity"`,
`"-Infinity"`, and `"NaN"`. JSON `null` is not a floating point value.

## Collections and operating points

`TimeSeries<T>` stores ordered time points and values of `T`.
`ScenarioSet<T>` stores named alternatives of `T` with optional probabilities
and no implied time order. Nested collections keep their structural type; the
document does not invent flattened names for each composition.

An `OperatingPoint<N>` stores a shared base network and typed overrides keyed
by stable component identity. The serializer preserves that relationship
without expanding each entry into another complete static network.

## Determinism

`serialize` is a function of the module alone. Serializing one module twice
produces identical text, and serializing the module that text deserializes to
produces the same text again. Members are written in a fixed order: record
fields in declaration order and map keys (`extras`, `quantities`,
`extensions`, `details`) sorted. Diagnostic identities are minted `d0`, `d1`,
... in record order for records that carry none, so the minted identities
depend only on record order. Every float is written in the shortest decimal
form that reads back to the same value, and nonfinite values use the three
string spellings above. Equal modules therefore produce equal documents,
which lets a document serve as a cache key, a golden file, or the input of a
content digest.

## Resource limits

Deserialization applies explicit limits before retaining large input data.
Those limits cover source count and byte lengths, diagnostics, source map and
history records, extension data, collection lengths, identity lengths, and
nested value depth. Limit failures use structured PowerIO diagnostics rather
than allocation failures or truncated results.

## Schema changes

The version belongs to the PowerIO IR document, not the Rust memory layout, a
grid exchange format, or the C ABI. PowerIO v0.11.0 writes and reads document
version `0.11.0`. Historical schemas document what earlier releases produced;
they do not add implicit compatibility to the deserializer.
