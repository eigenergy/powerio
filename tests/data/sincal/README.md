# SINCAL native fixture

`1-LV-rural1--0-sw.sinx` is an unmodified SimBench electrical network archive,
downloaded on 2026-10-06 from the University of Kassel data repository through
the [SimBench catalog](https://simbench.de/de/datensaetze/).

- [Source file](https://daks.uni-kassel.de/bitstreams/1e89ef81-8a51-4412-8795-5cdc1e285db9/download)
- SHA-256: `019f52397b4bc5673abed79a5fe1ed0d484d9674ffca04d4102806cd29ccc659`
- Size: 88,876 bytes (binary ZIP archive; 250 newline bytes).
- Original contents and paths are preserved, including diagrams and logs.
- The native database has 15 nodes, 13 lines, 13 loads, one two-winding
  transformer, one infeeder, and four converter generators (`DCInfeeder`).
  It is a balanced case, not an unbalanced oracle.

The SimBench database is licensed under the
[Open Database License 1.0](https://opendatacommons.org/licenses/odbl/1.0/).
Rights in individual database contents use the
[Database Contents License 1.0](https://opendatacommons.org/licenses/dbcl/1.0/).
See `LICENSE-SimBench.txt` for the upstream copyright and licensing notice,
copied from [SimBench revision 54cc5bad](https://github.com/e2nIEE/simbench/blob/54cc5badc420b7c0d82e734158a35ac61986b07c/LICENSE).
Its BSD section applies to upstream code, not this dataset. This fixture is
not relicensed under PowerIO's MIT/Apache code license.

No license/readme files occur inside the supplied archive. There are no
modified or stripped archive members. The extracted 2,289,664-byte database
is deliberately not vendored separately.

The research harness in `evals/sincal/` verifies the digest, package layout,
schema identity and variant-local topology. Parser and writer interoperability
tests remain to be implemented. Byte-exact echo alone is not writer validation.

## Paired CSV evidence

`simbench-csv/` contains ten unmodified members from the paired
[SimBench CSV archive](https://daks.uni-kassel.de/bitstreams/d25bf5ad-a2a3-4b40-92aa-91012be56332/download).
These retain the same ODbL/DbCL licensing. Only topology, element parameters,
and stored load-flow results are included; the large load profiles remain
external. The checks in `evals/sincal/` compare the native parameter mappings
against this independent representation, with explicit units and tolerances.
The CSV representation includes bus sections joined by switches; the native
model has already collapsed those connections. It must not be mistaken for
byte-exact preservation of the CSV topology.
