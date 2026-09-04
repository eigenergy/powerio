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
stable first major schema while the IR was still changing, and it said nothing
about which builds could read a document. From v0.11.0 the PowerIO IR version
is the `powerio` release that wrote the document, which makes the reader's
window a statement about releases rather than about an opaque counter: see
[Compatibility policy](#compatibility-policy). ABI 7 remains independent, and
the "Reader status" column above names the shape a build writes, not the whole
set it reads.

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

CI regenerates the current file and fails on a difference. A release adds its
generated directory and its row in the table above; `powerio/tests/frozen_schemas.rs`
holds the directory, the table, and this build's version to one another, and
the documentation site serves every directory listed. A release preserves its
schema here; changing a historical file would erase evidence rather than
upgrade a document.

## Compatibility policy

The schema archive documents history. Compatibility exists only where the
deserializer and its tests implement it, and the deserializer implements one
rule, stated once on `powerio::IR_SCHEMA_VERSION`: a build reads every
document a SemVer compatible build no newer than itself wrote. On the `0.y`
line that is the same minor version with a patch no later than the reader's,
so PowerIO 0.11.3 reads the documents of 0.11.0 through 0.11.3 and refuses
0.10.0, 0.12.0, and 0.11.4, naming the version it found and whether a newer
PowerIO or a regenerated document is the remedy. The compatible line is
therefore additive: a 0.11.x release may add a field with a default and may
not remove or redefine one, which is what lets the newer reader read the older
document without an upgrade pass.

This follows the useful distinction in LLVM and MLIR between a current
in-memory representation and supported serialized input. LLVM reads the
bitcode of the releases inside its compatibility window and upgrades it on
load; MLIR's stable bytecode lets a dialect record a version and provide an
explicit upgrade hook. PowerIO's window is the compatible release line, and it
claims nothing for an earlier line merely because that line's JSON Schema is
archived here.

- [LLVM IR backwards compatibility](https://llvm.org/docs/DeveloperPolicy.html#ir-backwards-compatibility)
- [MLIR bytecode versioning](https://mlir.llvm.org/docs/BytecodeFormat/#versioning)
