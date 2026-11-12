# From 0.11 to 0.12

PowerIO 0.12 starts a new release series. From 0.12 on, the release series
says what is compatible with what: within 0.12.x, newer readers keep reading
older PowerIO IR documents, and the C ABI only gains symbols. Older readers
accept newer documents for supported types and refuse unknown structural types. This page lists what changes for code and data written against 0.11.

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
- every 0.11 document, `"version": 2`, with the same meaning. Serializing it
  again writes `"version": "0.12.0"`.

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

`pio_version()` still returns the release as text. A prerelease such as
`0.12.0-rc.1` states its core release, `0.12.0`, in the numbers and the whole
version in `pio_version()`.

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

## Graph data

`IndexedNetwork::to_petgraph` is removed, and `powerio-tx` no longer depends
on petgraph, a crate before its own 1.0 whose type was in PowerIO's public
API. `IndexedNetwork::branch_edges()` yields the same topology, one
`(branch, from, to)` per in-service branch over dense bus indices. To keep a
petgraph graph, add `petgraph` to your application's dependencies. With an
existing `IndexedNetwork` named `view`, add `view.n()` nodes and one edge per
item:

```rust,ignore
let mut graph = petgraph::graph::UnGraph::<usize, usize>::with_capacity(view.n(), 0);
let nodes: Vec<_> = (0..view.n()).map(|bus| graph.add_node(bus)).collect();
for (branch, from, to) in view.branch_edges() {
    graph.add_edge(nodes[from], nodes[to], branch);
}
```

## Removed compatibility shims

PowerIO 0.11 kept these only so that 0.11.x code kept working; 0.12 removes
them with no alias.

| 0.11 | 0.12 |
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

## Numeric thermal ratings

Branch and winding ratings remain ordinary numbers in Rust, Python, C, and
Julia. In 0.12, zero means a zero apparent-power bound and positive infinity
means no bound. Neither changes equipment status, impedance, or connectivity.
The existing thermal-rating update accepts these same values. Normalization
converts MVA to per unit; zero and infinity keep their meaning.

Readers and writers translate format conventions. MATPOWER uses zero for no
bound; Egret uses null or an absent field. Egret numeric zero stays zero.
Writers report values a target cannot represent rather than silently changing
the constraint. Sienna and PowerSystems JSON, which need a number, state no
bound as PowerSystems' `INFINITE_BOUND` (see [Sienna exchange](sienna.md)).
NaN is not accepted as a usable thermal bound.

PowerIO IR uses its existing nonfinite encoding: `"Infinity"` in JSON.
The document version distinguishes 0.11 files, where zero meant no bound,
from 0.12 files. No thermal-specific record wrappers are needed. See also
[DC OPF bundles](dcopf-bundle.md) for the corresponding numeric output change.

PowerIO 0.11.10 and earlier could interpret an Egret zero as no bound.
Regenerate affected data from the original Egret input; an old IR document
cannot recover a distinction its producer discarded.

Egret files that PowerIO 0.11.10 or earlier wrote may have a `0` in a mixed
rating series where the model meant unlimited; 0.12 reads that `0` as a zero
bound. Regenerate them from the original model with 0.11.11 or later, which
writes `null` there.

## BMOPF output

There is one BMOPF. PowerIO 0.12 reads and writes the schema proposed in
[dsopt-schema#2](https://github.com/distribution-system-opt/dsopt-schema/pull/2),
vendored from the Task Force repository at commit `6796394`. Its pin moves to
the release tag the schema's `$id` names once the Task Force ratifies it.

| 0.11 | 0.12 |
|---|---|
| `--to bmopf-json@<version>`, `emit(module, "bmopf-json@<version>")` | refused; write `bmopf-json` |
| `BmopfSchemaVersion`, `BmopfEmitOptions::with_schema_version` | removed; there is nothing to select |
| `BMOPF_SCHEMA_VERSION` | removed; a written document's `meta.schema_version` is the release the schema's `$id` names |
| `bmopf::BMOPF_PROPOSAL_URL` | the function `bmopf::bmopf_proposal_url()` |
| `bmopf_schema` in `pio_schema_report` and `powerio.versions()`, a version number | the schema's `$id` |
| `BmopfEmitOptions::sideload_coordinates` | removed; geometry is in the typed `bus.*.geo`, `line.*.geo` and `meta.crs` slots |
| bus bounds written as `v_min`/`v_max` | written as `vpg_min`/`vpg_max` |
| bus neutral to ground cap written as `vn_max` | written as `vng_max` |
| `meta.$schema` stating a PowerIO archive URL | no `meta.$schema`; `meta.schema_version`, which the schema requires, is the one identifier, and the schema's `$id` is in `meta.provenance` |
| bus `vn_max` | `vng_max` |
| regulator `tap_ratio`, `tap_ratio_min`, `tap_ratio_max` | `voltage_ratio`, `voltage_ratio_min`, `voltage_ratio_max` |
| `prime_mover: "DSTATCOM"`, `p_unit`/`q_unit` `VA_FRACTION`, `dc_control: "droop"` | `STATCOM`, `PU`, `DROOP` |
| line `s_max` per phase | one entry per conductor; a per-phase array is dropped on write |
| an absent `meta.crs` read as `EPSG:4326` | read as `OGC:CRS84`; both are WGS84 longitude then latitude |

Multiconductor IVR preparation changes three results. A fixed inverter power
factor follows the OpenDSS sign convention, `pf > 0` putting Q on the same
side as P, so `McOpfCoil::power_factor_coefficients` is `[a, -pf]` where it
was `[a, pf]`, and `reactive_slope` changes sign. A type-A regulator prepares
with the regulated over source ratio and `tap_inverse = true`, as type B
does. An inverter's `energy_cost_rate` sets its dispatch cost.

Documents from older BMOPF drafts, PowerIO 0.11 output among them, still
read, as BMOPF, with one `READ.BMOPF.SCHEMA_LEGACY` warning. A consumer that
still needs an older draft's document has to keep its source file: writing
such a document, even unchanged, produces the current schema.

## Conversion output

No action is needed unless a consumer compares output byte for byte or
validates it against a closed schema.

- PowerModels JSON and egret JSON documents carry more PowerIO keys, which
  both tools ignore: areas, case metadata, the detailed connectivity
  hierarchy, the coordinate reference, element identities and rating sets,
  and typed fields such as transformer and switched shunt controls.
  [Format fidelity](format-fidelity.md) lists them. A consumer that rejects
  unknown keys has to allow them.
- A MATPOWER branch matrix with the solved flow columns `PF`, `QF`, `PT`,
  and `QT` reads them as the branch solution, and the MATPOWER writer states
  them when every branch has a solution, so such a branch matrix has 17
  columns instead of 13.
- The PMD ENGINEERING writer writes a generator `control_mode` the source
  stated.

## Toolchain

The Rust MSRV is 1.89 (it was 1.88), because rusqlite 0.41 and uuid 1.27
require it. Python wheels need Python 3.11 or later, as they did in 0.11.9.
