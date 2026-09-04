# IEEE cases in BMOPF JSON

Reference encodings of IEEE distribution test cases in BMOPF schema 0.2.0,
produced by `powerio-dist`. They validate against that schema and
exercise parsers and data profilers such as `BMOPFTools.jl`. Regenerate them
rather than editing by hand.

| Case | Source `.dss` | Size | Write diagnostics |
|---|---|---|---|
| IEEE 34 | `tests/data/dist/opendss/ieee34/ieee34Mod1.dss` (vendored) | 80,069 bytes | 306 |
| IEEE 123 | `tests/data/dist/opendss/ieee123/IEEE123Master.dss` (vendored) | 119,902 bytes | 453 |
| 4 bus delta wye | `4Bus-DY-Bal/4Bus-DY-Bal.DSS` from the OpenDSS distribution | 6,913 bytes | 0 |

34 and 123 are recognizable feeders. The Kersting 4 bus case isolates a single
delta to wye service transformer, the four wire winding that the BMOPF schema's
transformer subtypes exist to model and that the feeders' regulator transformers
do not show on their own. Regenerate or check all three with:

```
cargo run -p powerio-dist --example regen_bmopf_examples
cargo run -p powerio-dist --example regen_bmopf_examples -- --check
```

Every document validates against the vendored schema
(`tests/data/dist/bmopf/bmopf-0.2.0.schema.json`), and the writer reports each
field the schema cannot carry as a fidelity warning on stderr, so nothing drops
silently. The dss reader materializes every OpenDSS class default explicitly, so
the output is fully explicit, and writing back to `.dss` reproduces the source
byte for byte.

Each document carries a top level `meta` block. `meta.case_study_generator`
names the writing tool and version, and `meta.$schema` pins the schema identity.
The block is deterministic (no timestamp) so output stays byte stable. Per phase
generator `cost` is an array, as schema 0.2.0 requires.

## Source provenance

The IEEE node test feeders and the Kersting 4 bus transformer cases ship with the
OpenDSS distribution under its BSD 3-Clause terms (EPRI / DSS-Extensions). This
is the license of the source case data, not the BMOPF format. The vendored `.dss`
sources retain the upstream notice, and these converted JSON representations
carry it in
[`NOTICE-OPENDSS-BSD-3-CLAUSE`](NOTICE-OPENDSS-BSD-3-CLAUSE). The 4 bus case is
regenerable from the OpenDSS
`Distrib/IEEETestCases/4Bus-DY-Bal` directory (also mirrored at
`github.com/tshort/OpenDSS`).
