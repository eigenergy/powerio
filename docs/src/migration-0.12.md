# From 0.11 to 0.12

PowerIO 0.12 starts a new release series. From 0.12 on, the release series
says what is compatible with what: within 0.12.x, any release reads the
PowerIO IR documents any other 0.12 release writes, and the C ABI only gains
symbols. This page lists what changes for code and data written against 0.11.

## PowerIO IR documents

The header's `version` was the integer `2` in every 0.11 document. From 0.12
it names the release whose schema the document follows, as a string:

```json
{
  "schema": "pio-ir",
  "version": "0.12.0",
  "producer": { "name": "powerio", "version": "0.12.0" }
}
```

`producer` is unchanged. It names the software that wrote the document,
which need not be PowerIO, and it does not decide whether a document is read.

**Reading.** PowerIO 0.12.x reads:

- a document whose `version` is any 0.12 release, `"0.12.0"` through
  `"0.12.z"`, including one later than the reader. A later release can add
  structural types; a reader refuses a type it does not implement and keeps
  reading the others.
- every 0.11 document, `"version": 2`. 0.12.0 changes no record layout, so
  saved 0.11 data reads with the same meaning. Serializing it again writes
  `"version": "0.12.0"`.

It refuses everything else with `READ.MODULE.UNSUPPORTED`, naming the version
and producer the document states. A later release series (`"0.13.0"`,
`"1.0.0"`) needs a newer PowerIO. An earlier release, the integer `1` of a
0.10 document, or a string that is not a `MAJOR.MINOR.PATCH` release has to be
regenerated from its source data.

**0.11 readers.** A 0.11 release refuses a 0.12 document. Its message names
`version "0.12.0"` and says to regenerate the document; read it with PowerIO
0.12 instead. A consumer that has to stay on 0.11 needs documents written by
0.11.

**Rust.** `powerio::IR_VERSION` is now a `&str`, `"0.12.0"`, and
`powerio::IR_MIN_VERSION` is gone. Code that compared the integer should
compare the release series instead, or leave the decision to `deserialize`.
`powerio::IR_SCHEMA_ID` is
`https://powerio.dev/schema/pio-ir/0.12.0/schema.json`.

**Python.** `powerio.versions()["powerio_ir"]` is now
`{"schema": "pio-ir", "version": "0.12.0"}`, with the version a string.

**C.** The JSON that `pio_schema_report` returns states `powerio_ir.version`
as the release string, `"0.12.0"`, instead of the integer `2`.

**Code that inspects documents.** To recognize PowerIO IR, check
`"schema": "pio-ir"` and let `deserialize` decide whether it reads the
version. A check for `"version": 2` no longer matches the documents 0.12
writes.

**Schema files.** Each release publishes its own schema at
`docs/schema/pio-ir/<release>/schema.json`, served from
`https://powerio.dev/schema/pio-ir/<release>/schema.json`, so a document's
`version` names its schema file directly. Validate a 0.12 document against
the file its `version` names. The 0.11 files under `pio-ir/2/` keep their
bytes and identifiers, and still describe 0.11 documents. See
[PowerIO IR](pio-json-schema.md) for the rule and `docs/schema/README.md`
for every file.

## C ABI

PowerIO 0.11 gave the C ABI its own number, 7, which a program compared with
the library at load. The number only ever changed when a new release series
began, so PowerIO 0.12 removes it and states the release instead.

Removed, with no alias:

- the `PIO_ABI_VERSION` macro;
- `pio_abi_version()`;
- the `abi` key of the `pio_schema_report` document.

Added:

- `PIO_VERSION_MAJOR`, `PIO_VERSION_MINOR`, and `PIO_VERSION_PATCH`, the
  release the header was generated from;
- `pio_version_numbers()`, which returns the release of the loaded library as
  a `PioVersionView` with `major`, `minor`, and `patch` fields.

`pio_version()` still returns the release as text.

A program compiled against the header of release X.Y.Z works with any library
that is in the same release series, as Cargo defines it (the same major and
minor numbers before 1.0, the same major number from 1.0 on), and is release
X.Y.Z or newer. Within a release series the C ABI only gains symbols: no
symbol is removed, and no signature or struct layout changes.

| 0.11 | 0.12 |
|---|---|
| `pio_abi_version() == PIO_ABI_VERSION` | `pio_version_numbers()` is in the header's release series, at the header's release or newer ([the check in C](capi.md#versions-and-compatibility)) |
| `#if PIO_ABI_VERSION != 7` | `#if PIO_VERSION_MAJOR != 0 \|\| PIO_VERSION_MINOR != 12` |
| `"abi"` in the `pio_schema_report` document | `"powerio_version"` |

A 0.11 library has no `pio_version_numbers`. A program that loads the library
with `dlopen` or `LoadLibrary` can look the symbol up first and treat its
absence as a library older than 0.12.

PowerIO.jl applies the same rule. It removes `abi_version()` and keeps
`library_version()`. Before its first call into a library, PowerIO.jl checks
that the library is in the release series of the header it was generated
from, at that release or a newer one. It refuses a library from another
series, an older library, or one without `pio_version_numbers`, and names
both releases in the error.
