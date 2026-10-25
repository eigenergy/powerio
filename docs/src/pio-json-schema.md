# PowerIO IR {#pio-ir}

A `.pio.json` file serializes one `PioModule<PioValue>`, which is one typed
value together with its diagnostics, producer, sources, source mappings,
history, and extensions. A current document begins like this:

```json
{
  "schema": "pio-ir",
  "version": "0.12.0",
  "producer": { "name": "powerio", "version": "0.12.0" },
  "value": {
    "type": "powerio.BalancedNetwork",
    "data": {}
  }
}
```

Use `serialize` to produce it and `deserialize` to read it. PowerIO IR is not
one of the grid exchange formats. MATPOWER, PSS/E, XIIDM, CGMES, OpenDSS, PMD
JSON, BMOPF, distribution CIM, and the other formats in the format registry exist to exchange
power system data with other tools, and they enter through `parse` and leave
through `emit`. PowerIO IR preserves PowerIO types and module records instead,
and it is deliberately left out of grid exchange format discovery. Use it when
both sides consume PowerIO values, including calculation instances, solutions,
time series, and scenario sets.

The generated JSON Schema is checked in at
`docs/schema/pio-ir/0.12.0/schema.json` and served from
`https://powerio.dev/schema/pio-ir/0.12.0/schema.json`. That schema, the
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
must name records in the same document. The MATPOWER, PSS/E RAW, UCTE-DEF,
and IEEE CDF readers attach the byte range of the record a finding is about to
every diagnostic they raise at a known record, so a document serialized from
one of their modules has those spans; the other readers attach none yet.

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

A transformer with nondefault per-winding service flags is stored as a nested
record in its physical table, with `type` equal to `"powerio.PartialTransformer3W"`
and the complete transformer body under `value`.
The nested body preserves every original field and all three winding rows; it
is not a star expansion or an altered electrical network. Earlier readers reject
this new record because its required transformer fields are not at the outer
level. Default winding states retain the historical unwrapped representation,
including when the whole transformer is off. Malformed wrappers and unwrapped
nondefault winding states are rejected, never silently treated as all-on.

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

Stored collections have two entry bounds. Operating point and solution
vectors follow the network's dimensions: each `StoredQuantity`'s `identities`
and `values`, and every bus, branch, generator, terminal, source, and
conductor array of a DC, AC, SOCWR, multiconductor, or LinDist3Flow solution,
optional arrays included, carries at most 4,194,304 (2^22) entries. A
multiconductor solution has one entry per bus terminal, so solutions over up
to 4,194,304 bus terminals round trip. The reader refuses the first entry
past the bound with `READ.MODULE.INVALID`, and `serialize` refuses a longer
vector with `EMIT.MODULE.RECORD_CAP` rather than write a document the reader
refuses. Time series and scenario sets carry at most 65,536 entries each, as
do the three winding transformer power records of a solution. A stored
operating point names at most 64 quantities.

These bounds count entries, not bytes. Byte limits apply when PowerIO reads
a source. A file named by path, whether a raw format (including BMOPF) or
stored IR, may hold up to 1 GiB; `POWERIO_MAX_PRIMARY_BYTES` sets a different
positive decimal byte count. The files a case references and the files of a
directory input share a cumulative limit, `POWERIO_MAX_REFERENCED_BYTES`, also
1 GiB by default. Both settings come from the process environment. The MCP
server sets both to 256 MiB when its operator leaves them unset; see
[The MCP server](cli-mcp.md#the-mcp-server).

CGMES XML counts against the referenced limit wherever it comes from. The
profiles of one CGMES parse, read from files or expanded from ZIP archives,
total at most `POWERIO_MAX_REFERENCED_BYTES`, and a CGMES XML document or
archive larger than that limit is refused whether it comes from a path or
from caller-owned memory. A CGMES file above 1 GiB named by path therefore
needs both settings raised. An archive may hold at most 4,096 entries, a
count read from its end records before the archive is indexed, and at most
eight end records. Entries may not share a name or any bytes, and each must
lie inside the archive's entry data. Each file entry may declare at most the
referenced limit and at most 200 times its compressed size. An XML entry may
declare no more than the budget the profiles before it left, and it must
inflate to exactly the size it declares. Nested archives, symbolic link
entries, unsafe entry names, document type and entity declarations, and
invalid UTF-8 are refused. These archive checks are fixed; only the byte limit
is configurable.

Some readers keep fixed limits below these defaults. An XIIDM or JIIDM
document holds at most 64 MiB, from a path or from memory. An OpenDSS case
follows at most 4,096 includes and stops following them once they have loaded
64 MiB of script text, counting a file again each time the case runs it.

Python's `parse_display`, kept only for 0.11 compatibility, reads a display
file whole without these limits; `parse` reads the same `.pwd` file under them.

Apart from CGMES, XIIDM, and JIIDM, caller-owned memory has no whole-document limit; format and
record limits still apply. Stored solutions embed their instance and network,
so they can exceed the raw input's size. Consumers should check the actual
serialized byte length against their reload limit before saving, and choose a
smaller limit for memory-constrained applications. The byte limits do not
bound process memory.

## Versions and release series

The header's `version` names the release whose schema the document follows:
`"0.12.0"` for a document this build writes, described by
`pio-ir/0.12.0/schema.json`. `powerio::IR_VERSION` is that release and
`powerio::IR_SCHEMA_ID` the `$id` of its schema file. `producer` names the
software that wrote the document, which need not be PowerIO; the reader reports
it and ignores it when deciding whether to read the document.

Compatibility follows the release series as Cargo defines it: one minor
release before 1.0 (0.12.x), then one major release (1.x, 2.x). Within a
release series, any release reads PowerIO IR documents written by any other
release in it, including a later one. A later release in the series can add
structural types, add names to a list of allowed values, and add a distinctly
tagged alternative to a record; every existing definition keeps its exact
text. A reader refuses a document that uses a type or name it does not
implement and keeps reading documents that use only the ones it knows. A
change that existing readers could not read, such as a new field on an
existing record or a different meaning for an existing one, starts a new
release series.

PowerIO 0.12 also reads the documents every 0.11 release wrote. Those state
the integer `"version": 2`, and 0.12.0 keeps every record layout they use, so
they decode as they did in 0.11. A 0.11 reader refuses a 0.12 document by its
string version, whatever its value holds. Its message names the version and
says to regenerate the document; upgrading the reader to 0.12 also works.

The reader refuses every other version and reports the schema name, version,
and producer the document states, with the remedy: a string from a later
release series (`"0.13.0"`, `"1.0.0"`) needs a newer PowerIO, and an earlier
release, the integer 1 of a 0.10 document, or a string that is not a
`MAJOR.MINOR.PATCH` release has to be regenerated from its source data.

Every release from 0.12.0 on publishes its own schema file at
`pio-ir/<release>/schema.json`, and no release reuses an earlier one. The 0.11
files under `pio-ir/2/`, and every earlier file, keep their exact bytes and
identifiers. A test compares the current schema with every 0.11 file and with
the file of every earlier release in its series, so a definition that a
reader of the series understands cannot change.
[`docs/schema/README.md`](https://github.com/eigenergy/powerio/blob/main/docs/schema/README.md)
lists every schema file, what each release added, and the document header
each describes.

Other intermediate representations answer the versioning question
differently. LLVM bitcode records a format version in its
`MODULE_CODE_VERSION` record (0, 1, or 2), which tells the reader how some
records are encoded, such as whether instruction operands are absolute or
relative value IDs. That number describes the encoding rather than a
compatibility window: a newer LLVM reads any bitcode since LLVM 3.0 and
upgrades it on load, and an older LLVM is not promised to read newer bitcode.
MLIR bytecode carries an explicit version number with per-dialect upgrade
hooks. StableHLO ties its compatibility guarantees to its own release
version, as PowerIO IR does with the release series. A 0.10 document
is refused rather than upgraded; an upgrade path for those documents would be
a later addition.
