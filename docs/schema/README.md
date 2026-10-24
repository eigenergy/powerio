# PowerIO IR schema history

PowerIO IR has one document lineage. The IR version identifies incompatible
changes to existing record layouts or meanings. A release can add structural
types without changing the IR version. Schema snapshots list the types
implemented by a release and keep their published bytes and identifiers.

| IR version | First release | Document identity | Schema snapshot | Read by 0.11.6 |
|---|---|---|---|---|
| none | v0.6.1 | `pio-package` lineage `0.1` | `pio-ir/0.1/schema.json` | no |
| none | v0.8.0 | `pio-package` lineage `0.2` | `pio-ir/0.2/schema.json` | no |
| none | v0.9.0 | `pio-package` lineage `0.9` | `pio-ir/0.9/schema.json` | no |
| 1 | v0.10.0 | `powerio.module`, version `1` | `pio-ir/1/schema.json` | no |
| 2 | v0.11.0 | `pio-ir`, version `2` | `pio-ir/2/schema.json` | yes |
| 2 | v0.11.1 | `pio-ir`, version `2` | `pio-ir/2/0.11.1/schema.json` | yes |
| 2 | v0.11.3 | `pio-ir`, version `2` | `pio-ir/2/0.11.3/schema.json` | yes |
| 2 | v0.11.4 | `pio-ir`, version `2` | `pio-ir/2/0.11.4/schema.json` | yes |
| 2 | v0.11.5 | `pio-ir`, version `2` | `pio-ir/2/0.11.5/schema.json` | yes |
| 2 | v0.11.6 | `pio-ir`, version `2` | `pio-ir/2/0.11.7/schema.json` | yes |

The current document begins:

```json
{
  "schema": "pio-ir",
  "version": 2,
  "producer": { "name": "powerio", "version": "0.11.5" }
}
```

## Compatibility

PowerIO 0.11.6 keeps IR version 2 and every existing record layout. The
LinDist3Flow OPF instance and solution types added in 0.11.1, the three PSS/E
contingency analysis files added in 0.11.3, and the fixed-dispatch LinDist3Flow
instance and solution types added in 0.11.4 use distinct structural type names.
A reader accepts the types it implements; an older reader rejects an
unknown type without losing the ability to read familiar types. A list of
allowed names can gain a name the same way: 0.11.6 adds `cim-dist` to the
source formats a distribution network records. SINCAL adds `sincal` to both
network families' source-format lists. An older reader rejects a document that
uses an unknown format while still reading documents using known formats. A version
bump requires an incompatible change to an existing representation and an
explicit release decision. Adding fields to a record is not automatically
compatible: readers can reject unknown fields, so existing records retain
their layout throughout the 0.11.x line.

`powerio::IR_VERSION` is the IR version a build writes and
`powerio::IR_MIN_VERSION` the oldest it reads. Both remain 2 in 0.11.6.
`producer.version` records the producing release for diagnostics; it does not
determine whether a document can be read. The C ABI remains independently
versioned at 7. [The PowerIO IR chapter](../src/pio-json-schema.md) sets this
versioning scheme beside LLVM bitcode and MLIR bytecode.

The 0.11.0 schema at `pio-ir/2/schema.json` is a frozen snapshot of its 32
structural types. The 0.11.1 snapshot adds two structural types and their
supporting definitions, the 0.11.3 snapshot adds three more:
`powerio.ContingencySet`, `powerio.SubsystemSet`, and `powerio.MonitoredSet`,
the 0.11.4 snapshot adds `powerio.LinDist3FlowPfInstance`,
`powerio.LinDist3FlowPfSolution`, and their limit-report definitions, the
0.11.5 snapshot republishes the 0.11.4 catalog under its own identifier, and the
0.11.6 snapshot adds `cim-dist` and `sincal` to `DistSourceFormat`, `sincal`
to `SourceFormat`, and the distinctly tagged `powerio.ReferencedVoltageSource`
nested record. Ordinary voltage sources retain their original representation.
It also extends monitor scopes and statements with kV-qualified selection and
pre/post voltage bands while preserving previously accepted monitor records.

The release name in the snapshot path does not create a new IR version.
A later release without catalog changes can reuse that snapshot (0.11.2
reused 0.11.1). CI preserves existing definitions and document rules, with
narrow checks for added format names, the distinctly tagged voltage-source
and partial-transformer alternatives, and additive monitor fields and variants.
The monitor checks preserve existing property schemas and required fields and require distinct
constant tags for added alternatives; they reject new restrictions and
ambiguous variants. Every earlier published snapshot keeps its exact bytes.

Three-winding transformers with any nondefault per-winding status use the
nested `powerio.PartialTransformer3W` record. Its `value` holds the complete
physical transformer, including all three windings, impedances, units, identity,
and original extras. Readers that predate this type reject it instead of
silently energizing an off winding. Ordinary all-on and whole-transformer-off
records with default winding flags retain their exact historical body. This
also preserves nondefault flags while the whole transformer is off, so a later
status change cannot lose the individual winding state. A bare legacy body with
nondefault winding flags is rejected with a diagnostic requiring the new type.

## Served identifiers

Every published snapshot keeps its `$id` and original archive path. The
historical identifiers are `pio-package/0.1`, `pio-package/0.2`,
`pio-package/0.9/schema.json`, `pio-module/1/schema.json`,
`pio-ir/2/schema.json`, `pio-ir/2/0.11.1/schema.json`,
`pio-ir/2/0.11.3/schema.json`, `pio-ir/2/0.11.4/schema.json`, and
`pio-ir/2/0.11.5/schema.json` beneath `https://powerio.dev/schema/`.
The current schema uses `pio-ir/2/0.11.7/schema.json` under that same root.
The documentation site serves the archive paths and published identifiers.

## Regenerating the current catalog

```text
cargo run -p powerio --example generate_schemas --features schema -- docs/schema
```

The generator writes the path named by `powerio::IR_SCHEMA_ID`, currently
`pio-ir/2/0.11.7/schema.json`. It leaves earlier snapshots untouched. CI fails
if the generated catalog differs from the committed file.
