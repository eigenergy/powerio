# PowerIO IR schema history

A PowerIO IR document names, in its header, the release whose schema it
follows. Compatibility follows the release series as Cargo defines it: one
minor release before 1.0 (0.12.x), then one major release (1.x, 2.x). Within
a release series, any release reads the documents written by any other
release in it. A reader refuses a structural type it does not implement, one
type at a time, and keeps reading the types it does. A change that the
readers of a series could not read starts a new release series.

## The document header

A document begins:

```json
{
  "schema": "pio-ir",
  "version": "0.12.0",
  "producer": { "name": "powerio", "version": "0.12.0" }
}
```

- `schema` is always `pio-ir`.
- `version` is the release whose schema file describes the document,
  `pio-ir/<version>/schema.json` in this directory. `powerio::IR_VERSION` is
  the release a build writes, and `powerio::IR_SCHEMA_ID` is its file's
  `$id`.
- `producer` names the software that wrote the document, which need not be
  PowerIO. A refusal reports it; it does not decide whether a document is
  read.

PowerIO 0.12.x reads a document whose `version` is any 0.12.z release,
including a later one than its own, and the integer `version` 2 that every
0.11 release wrote. It refuses every other version and names the version it
found, the producer, and the remedy: a later release series needs a newer
PowerIO, and anything older has to be regenerated from its source data.

## The schema files

From 0.12.0 on, every release publishes `pio-ir/<release>/schema.json`, and
no release reuses an earlier file. Every file already published keeps its
exact bytes, archive path, and `$id`.

| Schema file | First release | Document header | Read by 0.12.x |
|---|---|---|---|
| `pio-ir/0.1/schema.json` | v0.6.1 | `pio-package` lineage `0.1` | no |
| `pio-ir/0.2/schema.json` | v0.8.0 | `pio-package` lineage `0.2` | no |
| `pio-ir/0.9/schema.json` | v0.9.0 | `pio-package` lineage `0.9` | no |
| `pio-ir/1/schema.json` | v0.10.0 | `powerio.module`, version `1` | no |
| `pio-ir/2/schema.json` | v0.11.0 | `pio-ir`, version `2` | yes |
| `pio-ir/2/0.11.1/schema.json` | v0.11.1 | `pio-ir`, version `2` | yes |
| `pio-ir/2/0.11.3/schema.json` | v0.11.3 | `pio-ir`, version `2` | yes |
| `pio-ir/2/0.11.4/schema.json` | v0.11.4 | `pio-ir`, version `2` | yes |
| `pio-ir/2/0.11.5/schema.json` | v0.11.5 | `pio-ir`, version `2` | yes |
| `pio-ir/2/0.11.6/schema.json` | v0.11.6 | `pio-ir`, version `2` | yes |
| `pio-ir/0.12.0/schema.json` | v0.12.0 | `pio-ir`, version `"0.12.0"` | yes |

The 0.11 releases wrote the integer 2 in every document and published their
schema files beneath `pio-ir/2/`; 0.11.2 reused the 0.11.1 file. A 0.11
document states which release wrote it only in `producer`.

## Compatibility

PowerIO 0.12.0 keeps every record layout 0.11.6 had. Its schema describes the
same structural types with the same definitions; only the header's
`version` changes, from the integer 2 to the release string. That is why it
reads 0.11 documents.

Within a release series a release can add structural types, add names to a
list of allowed values, and add a distinctly tagged alternative to a record.
Every existing definition keeps its exact text. An older reader in the series
refuses a document that uses a type or name it does not implement, and still
reads documents that use only the ones it knows. Adding a field to an
existing record is not compatible, because readers refuse unknown fields.

The 0.11 releases added types the same way. The 0.11.0 schema has 32
structural types. 0.11.1 added the LinDist3Flow OPF instance and solution;
0.11.3 added `powerio.ContingencySet`, `powerio.SubsystemSet`, and
`powerio.MonitoredSet`; 0.11.4 added `powerio.LinDist3FlowPfInstance`,
`powerio.LinDist3FlowPfSolution`, and their limit report definitions; 0.11.5
republished the 0.11.4 definitions under its own `$id`; and 0.11.6 added
`cim-dist` and `sincal` to `DistSourceFormat`, `sincal` to `SourceFormat`,
the distinctly tagged `powerio.ReferencedVoltageSource` nested record, and
kV-qualified selection and pre/post voltage bands in monitor scopes and
statements. Ordinary voltage sources and previously accepted monitor records
keep their original representation.

Three-winding transformers with any nondefault per-winding status use the
nested `powerio.PartialTransformer3W` record. Its `value` holds the complete
physical transformer, including all three windings, impedances, units, identity,
and original extras. Readers that predate this type reject it instead of
silently energizing an off winding. Ordinary all-on and whole-transformer-off
records with default winding flags retain their exact historical body. This
also preserves nondefault flags while the whole transformer is off, so a later
status change cannot lose the individual winding state. A bare legacy body with
nondefault winding flags is rejected with a diagnostic requiring the new type.

## Checks

- `powerio/tests/frozen_schemas.rs` compares the current schema with every
  0.11 file and with the file of every earlier release in its own release
  series. A definition an earlier file has keeps its exact text, except that a
  tagged union may gain alternatives and a list of allowed values may gain
  values. The header's `version` is the one document rule that differs
  between files.
- `scripts/check-release-versions.sh` checks that `IR_VERSION` is at or after
  the workspace version, that `IR_SCHEMA_ID` names
  `pio-ir/<IR_VERSION>/schema.json`, and that the file is checked in. With
  `--release`, which the tag and publish workflows pass, `IR_VERSION` must
  equal the release.
- CI regenerates the current schema and fails if it differs from the
  committed file.
- `scripts/check-ir-compatibility.py` writes documents with the published
  0.11.0 and 0.11.6 Python packages and reads them with the current build.

## Served identifiers

Every published file keeps its `$id` and original archive path. The
historical identifiers are `pio-package/0.1`, `pio-package/0.2`,
`pio-package/0.9/schema.json`, `pio-module/1/schema.json`,
`pio-ir/2/schema.json`, `pio-ir/2/0.11.1/schema.json`,
`pio-ir/2/0.11.3/schema.json`, `pio-ir/2/0.11.4/schema.json`,
`pio-ir/2/0.11.5/schema.json`, and `pio-ir/2/0.11.6/schema.json` beneath
`https://powerio.dev/schema/`. From 0.12.0 on, a release's file is served at
`pio-ir/<release>/schema.json` under that same root. The documentation site
serves the archive paths and published identifiers.

## Regenerating the current schema

```text
cargo run -p powerio --example generate_schemas --features schema -- docs/schema
```

The generator writes the path named by `powerio::IR_SCHEMA_ID`,
`pio-ir/<IR_VERSION>/schema.json`, currently `pio-ir/0.12.0/schema.json`. It
leaves every other file untouched.

`IR_VERSION` runs ahead of the workspace version between releases, and a
release's version bump sets it to the release. Set `IR_VERSION` and
`IR_SCHEMA_ID` to the new release, regenerate, and point `CURRENT_SCHEMA` in
`powerio/tests/frozen_schemas.rs` and the schema path in
`powerio/tests/ir_reference.rs` at the new file.
