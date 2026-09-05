# IEEE cases in BMOPF JSON

Reference encodings of IEEE distribution test cases in draft BMOPF 0.2,
produced by `powerio-dist`. They validate against that schema and
exercise parsers and data profilers such as `BMOPFTools.jl`. Regenerate them
rather than editing by hand.

| Case | Source `.dss` | Size | Write diagnostics |
|---|---|---|---|
| IEEE 34 | `tests/data/dist/opendss/ieee34/ieee34Mod1.dss` (vendored) | 80,822 bytes | 23 |
| IEEE 123 | `tests/data/dist/opendss/ieee123/IEEE123Master.dss` (vendored) | 120,747 bytes | 43 |
| 4 bus delta wye | `4Bus-DY-Bal/4Bus-DY-Bal.DSS` from the OpenDSS distribution | 8,980 bytes | 0 |

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
field or conversion the selected target cannot represent. Diagnostic counts
count aggregated records; each record retains its occurrence count. These are
canonical conversions. Re-emitting them as OpenDSS does not reconstruct the
source file bytes. Byte-exact preservation applies only to an unchanged module
emitted in the same format in which that module was parsed.


Each document carries a top level `meta` block. `meta.case_study_generator`
names the writing tool and version. `meta.$schema` gives an immutable proposal
retrieval URL, while `meta.provenance.powerio_bmopf` records its canonical
identity, commit, schema digest and proposal status.
The block is deterministic (no timestamp) so output stays byte stable. Generator `energy_cost_rate` is a per-phase array in $/kWh.

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
