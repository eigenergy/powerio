# From 0.11 to 1.0

PowerIO 1.0 starts the 1.x release series. From 1.0 on, the release series
says what is compatible with what: within 1.x, any release reads the PowerIO
IR documents any other 1.x release writes, the C ABI only gains symbols, and
the public Rust, Python, and Julia APIs change only additively. This page
lists what changes for code and data written against 0.11.

## PowerIO IR documents

The header's `version` was the integer `2` in every 0.11 document. From 1.0
it names the release whose schema the document follows, as a string:

```json
{
  "schema": "pio-ir",
  "version": "1.0.0",
  "producer": { "name": "powerio", "version": "1.0.0" }
}
```

`producer` is unchanged. It names the software that wrote the document,
which need not be PowerIO, and it does not decide whether a document is read.

**Reading.** PowerIO 1.x reads:

- a document whose `version` is any 1.x release, `"1.0.0"` through
  `"1.y.z"`, including one later than the reader. A later release can add
  structural types; a reader refuses a type it does not implement and keeps
  reading the others.
- every 0.11 document, `"version": 2`, with the same meaning. Serializing it
  again writes `"version": "1.0.0"`.

It refuses everything else with `READ.MODULE.UNSUPPORTED`, naming the version
and producer the document states. A later release series (`"2.0.0"`) needs
a newer PowerIO. An earlier release, the integer `1` of a
0.10 document, or a string that is not a `MAJOR.MINOR.PATCH` release has to be
regenerated from its source data.

**0.11 readers.** A 0.11 release refuses a 1.0 document. Its message names
`version "1.0.0"` and says to regenerate the document; read it with PowerIO
1.0 instead. A consumer that has to stay on 0.11 needs documents written by
0.11.

**Rust.** `powerio::IR_VERSION` is now a `&str`, `"1.0.0"`, and
`powerio::IR_MIN_VERSION` is gone. Code that compared the integer should
compare the release series instead, or leave the decision to `deserialize`.
`powerio::IR_SCHEMA_ID` is
`https://powerio.dev/schema/pio-ir/1.0.0/schema.json`.

**Python.** `powerio.versions()["powerio_ir"]` is now
`{"schema": "pio-ir", "version": "1.0.0"}`, with the version a string.

**C.** The JSON that `pio_schema_report` returns states `powerio_ir.version`
as the release string, `"1.0.0"`, instead of the integer `2`.

**Code that inspects documents.** To recognize PowerIO IR, check
`"schema": "pio-ir"` and let `deserialize` decide whether it reads the
version. A check for `"version": 2` no longer matches the documents 1.0
writes.

**Schema files.** Each release publishes its own schema at
`docs/schema/pio-ir/<release>/schema.json`, served from
`https://powerio.dev/schema/pio-ir/<release>/schema.json`, so a document's
`version` names its schema file directly. Validate a 1.0 document against
the file its `version` names. The 0.11 files under `pio-ir/2/` keep their
bytes and identifiers, and still describe 0.11 documents. See
[PowerIO IR](pio-json-schema.md) for the rule and `docs/schema/README.md`
for every file.

## C ABI

PowerIO 0.11 gave the C ABI its own number, 7, which a program compared with
the library at load. The number only ever changed when a new release series
began, so PowerIO 1.0 removes it and states the release instead.

Removed, with no alias:

- the `PIO_ABI_VERSION` macro;
- `pio_abi_version()`;
- the `abi` key of the `pio_schema_report` document.

Added:

- `PIO_VERSION_MAJOR`, `PIO_VERSION_MINOR`, and `PIO_VERSION_PATCH`, the
  release the header was generated from;
- `pio_version_numbers()`, which returns the release of the loaded library as
  a `PioVersionView` with `major`, `minor`, and `patch` fields.

`pio_version()` still returns the release as text. A prerelease such as
`1.0.0-rc.1` states its core release, `1.0.0`, in the numbers and the whole
version in `pio_version()`.

A program compiled against the header of release X.Y.Z works with any library
that is in the same release series, as Cargo defines it (the same major
number from 1.0 on), and is release X.Y.Z or newer. Within a release series the C ABI only gains symbols: no
symbol is removed, and no signature or struct layout changes.

| 0.11 | 1.0 |
|---|---|
| `pio_abi_version() == PIO_ABI_VERSION` | `pio_version_numbers()` is in the header's release series, at the header's release or newer ([the check in C](capi.md#versions-and-compatibility)) |
| `#if PIO_ABI_VERSION != 7` | `#if PIO_VERSION_MAJOR != 1` |
| `"abi"` in the `pio_schema_report` document | `"powerio_version"` |

A 0.11 library has no `pio_version_numbers`. A program that loads the library
with `dlopen` or `LoadLibrary` can look the symbol up first and treat its
absence as a library older than 1.0.

PowerIO.jl applies the same rule. It removes `abi_version()` and keeps
`library_version()`. Before its first call into a library, PowerIO.jl checks
that the library is in the release series of the header it was generated
from, at that release or a newer one. It refuses a library from another
series, an older library, or one without `pio_version_numbers`, and names
both releases in the error.

## Three winding transformer magnetizing admittance

`Transformer3W` gains `magnetizing_connection`, which says where `mag_g` and
`mag_b` connect: `StarPoint`, between the star point and ground, or
`Winding1Terminal`, between winding 1's bus and ground outside its ratio.
0.11 placed every three winding magnetizing admittance at the star point,
which is what IIDM and CGMES state but not what PSS/E states (#741). For a
unit with an off-nominal winding 1 ratio the difference reached `|Ym|` in
`Y_bus`.

- The PSS/E RAW and RAWX readers state `Winding1Terminal`; the XIIDM, JIIDM,
  and CGMES readers state `StarPoint`.
- The star lowering, and so every matrix, puts a `Winding1Terminal`
  admittance on winding 1's branch as from side charging `Ym * tap1²`, so
  bus 1 sees `Ym`.
- A PowerIO IR document states the field only when it is not the star point,
  so every 0.11 document reads with the meaning it had.
- PSS/E written from star point data, and XIIDM, JIIDM, or CGMES written from
  PSS/E data, keep the value and report the moved placement with
  `VALUE_SUBSTITUTED`.
- C: `PioBalancedThreeWindingTransformerView` ends with a
  `magnetizing_connection` string view. Julia: `ThreeWindingTransformer` ends
  with `magnetizing_connection::String`. Python rows carry
  `magnetizing_connection` when it is not `"star_point"`.

## Renamed for consistency

- `SocwrOpfSolution` names its branch flows as the other solutions do:
  `branch_from_active_flow`, `branch_from_reactive_flow`,
  `branch_to_active_flow`, and `branch_to_reactive_flow` (they ended in
  `_power`), in the Rust values and accessors, the PowerIO IR keys, and the
  quantity names the C ABI's solution value accessor takes. The reader still
  accepts the 0.11 keys. Its `bus_order` and `branch_order` return vectors,
  as every other solution's do.
- `to_balanced` and `to_balanced_report` are also exported at the facade
  root, beside the other transformations; `powerio::transform` keeps them.

## Removed compatibility shims

PowerIO 0.11 kept these only so that 0.11.x code kept working; 1.0 removes
them with no alias.

| 0.11 | 1.0 |
|---|---|
| Python `powerio.parse_display(path)`, which returned `DisplayData(kind, PwdDisplay(...))` | `powerio.parse(path)`, which returns a `GeoLayer` with diagnostics and reads under the input byte limits |
| Python `DisplayData`, `PwdDisplay`, `PwdSubstation` | the `GeoLayer` features; Rust keeps `PwdDisplay` and `to_geo_layer_from_pwd` |
| the MCP server's `display` tool | the `parse` tool, which returns the `GeoLayer` as PowerIO IR |
| `powerio-capi`'s inert `arrow` Cargo feature | omit it: `cargo build -p powerio-capi --release --features matrix,gridfm,dist,prob` |

`parse` refuses a `.pwd` file whose bus identity tables are ambiguous or
repeated, or whose header decodes but places nothing, with
`PARSE.SOURCE.MALFORMED`; `parse_display` returned the substations of such a
file. The display's save stamp, an undocumented header number the reader uses
to recognize drawing records, is not part of the `GeoLayer`.

## Toolchain

The Rust MSRV is 1.89 (it was 1.88), because rusqlite 0.41 and uuid 1.27
require it. Python wheels need Python 3.11 or later, as they did in 0.11.9.
