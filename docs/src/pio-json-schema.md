# PowerIO IR {#pio-ir}

A `.pio.json` file serializes one `PioModule<PioValue>`, which is one typed
value together with its diagnostics, producer, sources, source mappings,
history, and extensions. A current document begins like this:

```json
{
  "schema": "pio-ir",
  "version": 2,
  "producer": { "name": "powerio", "version": "0.11.0" },
  "value": {
    "type": "powerio.BalancedNetwork",
    "data": {}
  }
}
```

Use `serialize` to produce it and `deserialize` to read it. PowerIO IR is not
one of the grid exchange formats. MATPOWER, PSS/E, XIIDM, CGMES, OpenDSS, PMD
JSON, BMOPF, and the other formats in the format registry exist to exchange
power system data with other tools, and they enter through `parse` and leave
through `emit`. PowerIO IR preserves PowerIO types and module records instead,
and it is deliberately left out of grid exchange format discovery. Use it when
both sides consume PowerIO values, including calculation instances, solutions,
time series, and scenario sets.

The generated JSON Schema is checked in at
`docs/schema/pio-ir/2/schema.json` and served from
`https://powerio.dev/schema/pio-ir/2/schema.json`. That schema, the
serializer, and the deserializer are all tested from the same Rust types.
`docs/schema/README.md` lists the earlier `pio-package` and
`powerio.module` documents as one history under `pio-ir`.

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

Source bytes retained at runtime are not serialized. A source record names a
source without exposing a local absolute path; a parser may keep the bytes in
memory for byte exact same format emission while the process runs, but that
buffer is separate from the stored module.

The deserializer checks the cross references before it returns a module:
diagnostic source references and source mappings must name declared source
IDs, byte ranges must fit the declared source length, and history references
must name records in the same document. The MATPOWER and PSS/E readers attach
the byte range of the record a finding is about to every diagnostic they raise
at a known record, so a document serialized from one of their modules has
those spans; the other readers attach none yet.

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

`value.data` has the exact shape that type demands, and the deserializer
rejects a document where the two disagree, as it rejects a wrong schema name
or version, an unknown PowerIO type, duplicate IDs, invalid references,
nonfinite values in untyped positions, or a collection whose entries disagree
with its element type. [PowerIO IR reference](ir-reference.md) defines every
structural type field by field: type, unit, sign convention, invariant, and
the value a reader takes when a field is absent.

Typed floating point fields spell nonfinite values as `"Infinity"`,
`"-Infinity"`, and `"NaN"`. JSON `null` is not a floating point value.

## Collections and operating points

`TimeSeries<T>` stores ordered time points and values of `T`, and
`ScenarioSet<T>` stores named alternatives of `T` with optional probabilities
and no implied time order. Nested collections keep their structural type; the
document does not invent a flattened name for each composition.

An `OperatingPoint<N>` stores a shared base network and typed overrides keyed
by stable component ID. The serializer preserves that relationship rather
than expanding each entry into another complete static network.

## Determinism

`serialize` is a function of the module alone. Serializing one module twice
produces identical text, and serializing the module that text deserializes to
produces the same text again. Members are written in a fixed order, record
fields in declaration order and map keys (`extras`, `quantities`,
`extensions`, `details`) sorted. Diagnostic IDs are minted `d0`, `d1`, ... in
record order for records that have none, so the minted IDs depend only on
record order. Every float is written in the shortest decimal form that reads
back to the same value, and nonfinite values use the three string spellings
above. Equal modules therefore produce equal documents, which is what lets
you use a document as a cache key, a golden file, or the input of a content
digest.

## Resource limits

Before it retains large input data, deserialization applies explicit limits
on source count and byte lengths, diagnostics, source map and history
records, extension data, collection lengths, ID lengths, and nested value
depth. Hitting a limit produces a structured PowerIO diagnostic rather than
an allocation failure or a truncated result.

## Generations

The integer `version` is the generation of the serialized representation. It
is a property of the document alone, separate from the Rust memory layout,
the PowerIO release, any grid exchange format, and the C ABI, and it changes
only when the representation changes. `producer.version` records the release
that wrote the document; the reader reports it and ignores it when deciding
compatibility.

When a generation bumps inside one minor release line, the release ships with
a reader for the generation it replaces, so every 0.11.x release reads every
generation any 0.11.x release wrote. `powerio::IR_VERSION` is the generation
a build writes and `powerio::IR_MIN_VERSION` the oldest it reads; in 0.11
both are `2`. A refused document is reported with the schema name,
generation, and producer it claims and the remedy: a later generation needs a
newer PowerIO, and an earlier schema name or generation has to be regenerated
from its source data.
[`docs/schema/README.md`](https://github.com/eigenergy/powerio/blob/main/docs/schema/README.md)
is the ledger of every generation and the archive of every published schema.
