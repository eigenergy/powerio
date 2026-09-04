# PowerIO IR schema history

One PowerIO IR lineage lives here. Each directory is named for the version its
document declared: the `pio-package` lineages `0.1`, `0.2`, and `0.9`, then
the integer generations that replaced them. The current generation is the only
document the generator writes. The others are frozen copies of what earlier
releases published.

| Generation | PowerIO release | Document identity | Archived schema | Read by 0.11 |
|---|---|---|---|---|
| none | v0.6.1 to v0.7.3 | `pio-package` lineage `0.1` | `pio-ir/0.1/schema.json` | no |
| none | v0.8.0 to v0.8.3 | `pio-package` lineage `0.2` | `pio-ir/0.2/schema.json` | no |
| none | v0.9.0 | `pio-package` lineage `0.9` | `pio-ir/0.9/schema.json` | no |
| 1 | v0.10.0 | `powerio.module`, version `1` | `pio-ir/1/schema.json` | no |
| 2 | v0.11.0 | `pio-ir`, version `2` | `pio-ir/2/schema.json` | yes |

The current document begins:

```json
{
  "schema": "pio-ir",
  "version": 2,
  "producer": { "name": "powerio", "version": "0.11.0" }
}
```

## The version rule

`version` is the generation of the serialized representation. It advances only
when that representation changes. A bump inside one minor release line ships
with a reader for the generation it replaces, so every 0.11.x release reads
every generation any 0.11.x release wrote. `powerio::IR_VERSION` is the
generation a build writes and `powerio::IR_MIN_VERSION` the oldest generation
it reads; the floor rises only at a minor release boundary.

`producer.version` records the release that wrote a document. The reader
reports it and never consults it for compatibility. The C ABI is versioned
separately.

A refused document names the identity, generation, and producer it states, and
the remedy. A later generation needs a newer PowerIO. An earlier identity or
generation has to be regenerated from its source data.

This split follows LLVM bitcode, whose identification block carries a producer
string and an epoch, and MLIR bytecode, whose header carries an integer format
version and a producer string. PowerIO makes no LLVM or MLIR compatibility
claim.

## Served identifiers

Every archived document keeps the `$id` its release published. The
documentation site serves each file at its archive path and at that `$id`
path (`pio-package/0.1`, `pio-package/0.2`, `pio-package/0.9`, and
`pio-module/1`), so a document quoting the identifier still resolves. Keep the
archived files byte for byte. `powerio/tests/frozen_schemas.rs` pins the
directory listing and the identifiers.

## Regenerating the current schema

```text
cargo run -p powerio --example generate_schemas --features schema -- docs/schema
```

CI regenerates `pio-ir/2/schema.json` on every pull request and fails on a
difference.
