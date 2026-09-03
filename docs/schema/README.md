# PowerIO IR schema history

This directory records one PowerIO IR lineage. The names `pio-package` and
`powerio.module` identify successive representations of that IR; they are not
separate schema families. `pio-module` is the filesystem and URL spelling for
this single archive; current documents identify themselves as
`powerio.module`.

| PowerIO release | Document identity | Archived schema | Reader status |
|---|---|---|---|
| v0.6.1–v0.7.3 | `pio-package` lineage `0.1` | `pio-module/0.1/schema.json` | historical only |
| v0.8.0–v0.8.3 | `pio-package` lineage `0.2` | `pio-module/0.2/schema.json` | historical only |
| v0.9.0 | `pio-package` lineage `0.9` | `pio-module/0.9/schema.json` | historical only |
| v0.10.0 | `powerio.module`, version `1` | `pio-module/0.10.0/schema.json` | historical only |
| v0.11.0 | `powerio.module`, version `0.11.0` | `pio-module/0.11.0/schema.json` | current |

PowerIO v0.10.0 retired the `powerio-pkg` crate into the `powerio` facade and
replaced `NetworkPackage` with the serialization of `PioModule<PioValue>`.
That is when the document discriminator changed to `powerio.module`.

The v0.10.0 document used the integer version `1`. That choice suggested a
stable first major schema while the IR was still changing, so v0.11.0 replaces
it with the simpler rule that the PowerIO IR version is the `powerio` crate
version. ABI 7 remains independent.

The four historical files are exact copies of the schemas checked into their
releases. Their original `$id` values and field names are evidence of what
those releases produced. Their presence here does not mean that the current
deserializer accepts them. The retired payload schemas are not part of this
history: they described an older split representation that PowerIO no longer
uses.

The current document header is:

```json
{
  "schema": "powerio.module",
  "version": "0.11.0"
}
```

Generate the current schema with:

```text
cargo run -p powerio --example generate_schemas --features schema -- docs/schema
```

CI regenerates the current file and fails on a difference. A release preserves
its schema here; changing a historical file would erase evidence rather than
upgrade a document.

## Compatibility policy

The schema archive documents history. Compatibility exists only when the
deserializer and its tests explicitly implement it. PowerIO v0.11.0 accepts
the v0.11.0 document and rejects every other schema name or version.

This follows the useful distinction in LLVM and MLIR between a current
in-memory representation and supported serialized input. LLVM records released
bitcode compatibility fixtures and upgrades supported old input; MLIR's stable
bytecode lets a dialect record a version and provide an explicit upgrade hook.
PowerIO does not claim either guarantee merely because an old JSON Schema is
archived.

- [LLVM IR backwards compatibility](https://llvm.org/docs/DeveloperPolicy.html#ir-backwards-compatibility)
- [MLIR bytecode versioning](https://mlir.llvm.org/docs/BytecodeFormat/#versioning)
