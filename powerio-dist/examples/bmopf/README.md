# IEEE cases in BMOPF JSON

Reference encodings of IEEE distribution test cases in the draft IEEE PES BMOPF
schema (v0.0.1), produced by `powerio-dist`. They give the BMOPF task force
canonical inputs that validate against the schema and pass the fidelity gates,
for parsers and data profilers such as `BMOPFTools.jl`. Regenerate them rather
than editing by hand.

| Case | Source `.dss` | Size | Fidelity notes (parse + write) |
|---|---|---|---|
| IEEE 34 | `tests/data/dist/opendss/ieee34/ieee34Mod1.dss` (vendored) | 73 KB | 411 |
| IEEE 123 | `tests/data/dist/opendss/ieee123/IEEE123Master.dss` (vendored) | 111 KB | 607 |
| 4 bus delta wye | `4Bus-DY-Bal/4Bus-DY-Bal.DSS` from the OpenDSS distribution | 7 KB | 17 |

34 and 123 are recognizable feeders. The Kersting 4 bus case isolates a single
delta to wye service transformer, the four wire winding that the BMOPF schema's
transformer subtypes exist to model and that the feeders' regulator transformers
do not show on their own. Each was generated with:

```
powerio convert <case>.dss --to bmopf-json -o <case>.json
```

Every document validates against the vendored schema
(`tests/data/dist/bmopf/draft_bmopf_schema.json`), and the writer reports each
field the schema cannot carry as a fidelity warning on stderr, so nothing drops
silently. The dss reader materializes every OpenDSS class default explicitly, so
the output is fully explicit, and writing back to `.dss` reproduces the source
byte for byte.

Each document carries a top level `meta` block (BMOPF v0.0.1): `meta.generator`
names the writing tool and version, and `meta.$schema` pins the schema vintage.
The block is deterministic (no timestamp) so output stays byte stable. Per phase
generator `cost` is an array, as v0.0.1 requires.

## Provenance and licensing

The IEEE node test feeders and the Kersting 4 bus transformer cases ship with the
OpenDSS distribution under the BSD 3-Clause license (EPRI / DSS-Extensions). The
vendored `.dss` sources retain that notice
(`tests/data/dist/opendss/License.txt`); these derived BMOPF JSON encodings are
released under the same terms. The 4 bus case is regenerable from the OpenDSS
`Distrib/IEEETestCases/4Bus-DY-Bal` directory (also mirrored at
`github.com/tshort/OpenDSS`).
