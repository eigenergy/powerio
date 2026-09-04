# PowerIO IR schema history

This directory records one PowerIO IR lineage. `pio-package` and
`powerio.module` are historical document identities, not separate schema
families. The current identity and the archive root are both `pio-ir`.

| PowerIO release | Document identity | Archived schema | Reader status |
|---|---|---|---|
| v0.6.1–v0.7.3 | `pio-package` lineage `0.1` | `pio-ir/0.1/schema.json` | historical only |
| v0.8.0–v0.8.3 | `pio-package` lineage `0.2` | `pio-ir/0.2/schema.json` | historical only |
| v0.9.0 | `pio-package` lineage `0.9` | `pio-ir/0.9/schema.json` | historical only |
| v0.10.0 | `powerio.module`, version `1` | `pio-ir/0.10.0/schema.json` | historical only |
| v0.11.0 | `pio-ir`, version `2` | `pio-ir/2/schema.json` | current |

PowerIO v0.10.0 retired the `powerio-pkg` crate into the `powerio` facade and
replaced `NetworkPackage` with the serialization of `PioModule<PioValue>`.
That release used the document identity `powerio.module` and integer version
`1`. PowerIO v0.11.0 gives the IR one durable identity, `pio-ir`, and advances
the representation to generation `2`.

The current document begins:

```json
{
  "schema": "pio-ir",
  "version": 2,
  "producer": { "name": "powerio", "version": "0.11.0" }
}
```

The integer `version` identifies the PowerIO IR representation. It changes
only when that representation changes. `producer.version` separately records
the PowerIO release that wrote a document. The C ABI remains independently
versioned. A reader accepts generation `2`; support for any other generation
requires an explicit reader or upgrade path.

The `0.1`, `0.2`, and `0.9` files are the exact schemas frozen in the v0.9.0
tag. The `0.10.0` file is the exact `powerio.module` schema from the v0.10.0
tag. Their original `$id` values and field names remain as evidence of what
those releases produced. Their presence does not make them readable by the
current deserializer. Retired payload schemas described an older split
representation and are intentionally excluded.

Generate the current schema with:

```text
cargo run -p powerio --example generate_schemas --features schema -- docs/schema
```

CI regenerates `pio-ir/2/schema.json` and fails on a difference. Released
schemas remain frozen. This separation follows MLIR bytecode's use of an
independent integer format version and a producer string, without claiming
LLVM or MLIR compatibility guarantees for PowerIO.

- [LLVM IR backwards compatibility](https://llvm.org/docs/DeveloperPolicy.html#ir-backwards-compatibility)
- [MLIR bytecode versioning](https://mlir.llvm.org/docs/BytecodeFormat/#versioning)
