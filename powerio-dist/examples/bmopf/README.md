# IEEE feeders in BMOPF JSON

Reference encodings of three IEEE distribution test feeders in the draft IEEE PES
BMOPF schema, produced by `powerio-dist`. They exist so the BMOPF task force has
canonical, schema-valid, fidelity-checked inputs for parsers and data profilers
(e.g. `BMOPFTools.jl`); regenerate them rather than editing by hand.

| Case | Source `.dss` | Size | Fidelity notes (parse + write) |
|---|---|---|---|
| IEEE 34 | `tests/data/dist/opendss/ieee34/ieee34Mod1.dss` (vendored) | 73 KB | 411 |
| IEEE 123 | `tests/data/dist/opendss/ieee123/IEEE123Master.dss` (vendored) | 110 KB | 607 |
| IEEE 37 | `37Bus/ieee37.dss` from the OpenDSS distribution (not vendored here) | 56 KB | 213 |

Each was generated with:

```
powerio convert <case>.dss --to bmopf-json -o <case>.json
```

Every emitted document validates against the vendored draft schema
(`tests/data/dist/bmopf/draft_bmopf_schema.json`), and the writer reports each
field the schema cannot carry as a fidelity warning on stderr (no silent drops).
The dss reader materializes every OpenDSS class default explicitly, so the BMOPF
output is fully explicit. Writing back to `.dss` reproduces the source byte for
byte, which fixes the source fidelity these encodings rest on.

The schema forbids sidecar data (`additionalProperties: false` throughout), so
these documents carry no `meta`/`$schema` block; the schema vintage is the
vendored copy. A namespaced extension hatch for self-identifying provenance is an
open task-force question (see the PR #82 discussion).

## Provenance and licensing

The IEEE 13/34/37/123 node test feeders ship with the OpenDSS distribution under
the BSD 3-Clause license (EPRI / DSS-Extensions). The `.dss` sources retain that
notice (`tests/data/dist/opendss/License.txt`); these derived BMOPF JSON encodings
are released under the same terms. IEEE 37 is regenerable from the OpenDSS
`Distrib/IEEETestCases/37Bus` directory (also mirrored at
`github.com/tshort/OpenDSS`).
