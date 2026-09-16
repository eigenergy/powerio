# PowerIO IR schema history

PowerIO IR has one document lineage. Generations identify incompatible changes
to existing record layouts or meanings. A release can add structural types
without changing the generation. Schema snapshots list the types implemented
by a release and keep their published bytes and identifiers.

| Generation | First release | Document identity | Schema snapshot | Read by 0.11.3 |
|---|---|---|---|---|
| none | v0.6.1 | `pio-package` lineage `0.1` | `pio-ir/0.1/schema.json` | no |
| none | v0.8.0 | `pio-package` lineage `0.2` | `pio-ir/0.2/schema.json` | no |
| none | v0.9.0 | `pio-package` lineage `0.9` | `pio-ir/0.9/schema.json` | no |
| 1 | v0.10.0 | `powerio.module`, version `1` | `pio-ir/1/schema.json` | no |
| 2 | v0.11.0 | `pio-ir`, version `2` | `pio-ir/2/schema.json` | yes |
| 2 | v0.11.1, additive type catalog | `pio-ir`, version `2` | `pio-ir/2/0.11.1/schema.json` | yes |
| 2 | v0.11.3, additive type catalog | `pio-ir`, version `2` | `pio-ir/2/0.11.3/schema.json` | yes |

The current document begins:

```json
{
  "schema": "pio-ir",
  "version": 2,
  "producer": { "name": "powerio", "version": "0.11.3" }
}
```

## Compatibility

PowerIO 0.11.3 keeps IR generation 2 and every existing record layout. The
LinDist3Flow instance and solution types added in 0.11.1 and the three PSS/E
contingency analysis files added in 0.11.3 use distinct structural type names.
A reader accepts the types it implements; an older reader rejects an
unknown type without losing the ability to read familiar types. A generation
bump requires an incompatible change to an existing representation and an
explicit release decision. Adding fields to a record is not automatically
compatible: readers can reject unknown fields, so existing records retain
their layout throughout the 0.11.x line.

`powerio::IR_VERSION` is the generation a build writes and
`powerio::IR_MIN_VERSION` the oldest it reads. Both remain 2 in 0.11.3.
`producer.version` records the producing release for diagnostics; it does not
determine whether a document can be read. The C ABI remains independently
versioned at 7.

The 0.11.0 schema at `pio-ir/2/schema.json` is a frozen snapshot of its 32
structural types. The 0.11.1 snapshot adds two structural types and their
supporting definitions, and the 0.11.3 snapshot adds three more:
`powerio.ContingencySet`, `powerio.SubsystemSet`, and `powerio.MonitoredSet`.
The release name in the snapshot path does not create
a new IR generation. A later release without catalog changes can reuse that
snapshot. CI checks that all existing definitions and document rules remain
identical and that every earlier published snapshot keeps its exact bytes.

## Served identifiers

Every published snapshot keeps its `$id` and original archive path. The
historical identifiers are `pio-package/0.1`, `pio-package/0.2`,
`pio-package/0.9/schema.json`, `pio-module/1/schema.json`,
`pio-ir/2/schema.json`, and `pio-ir/2/0.11.1/schema.json` beneath
`https://powerio.dev/schema/`.
The current catalog uses `pio-ir/2/0.11.3/schema.json` under that same root.
The documentation site serves the archive paths and published identifiers.

## Regenerating the current catalog

```text
cargo run -p powerio --example generate_schemas --features schema -- docs/schema
```

The generator writes the path named by `powerio::IR_SCHEMA_ID`, currently
`pio-ir/2/0.11.3/schema.json`. It leaves earlier snapshots untouched. CI fails
if the generated catalog differs from the committed file.
