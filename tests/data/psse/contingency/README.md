# PSS/E contingency description fixtures

Every file here is original to this repository. Siemens does not publish the
normative `.con` grammar, so the statements these files state were drawn from
public contingency files and from question threads about them. The fixtures
carry no content from those sources: each one is written here, and the sources
below are cited as evidence that a spelling occurs in real files.

| File | Bytes | What it covers |
| --- | --- | --- |
| `psse35_generated.con` | 141 | The header PSS/E 35 writes (`/PSS(R)E` stamp, `COM` banner), one automatic specification, a file `END`. |
| `explicit_mixed.con` | 934 | Explicit cases: quoted, bare, and apostrophe names; every action the grammar reads; synonyms, lowercase keywords, tab indentation, a mid-file `COM` line. |
| `no_file_end.con` | 203 | The corpus layout: `BUS   1001` column spacing, no file `END`. |
| `automatic_skip.con` | 175 | Automatic specifications for branches, units, and ties, `3WLOWVOLTAGE`, a `SKIP` block, and one line after the file `END`. |
| `tara_extensions.con` | 367 | Statements only the TARA solver reads: a `//` comment, `BUSNUMBERS`, `BRANCHNAMES`, bus-name mode, a padded quoted circuit, a trailing `/` comment, a nested dispatch block, and the `DEFAULT DISPATCH` and `DEFAULT DISPATCH DOWN` block openers. |

## Evidence

| Source | What it shows |
| --- | --- |
| [IdahoLabUnsupported/PSSE_for_Human_factor_research](https://github.com/IdahoLabUnsupported/PSSE_for_Human_factor_research) `WOA.con` | The `/PSS(R)E` and `COM` header PSS/E 35 writes, and `SINGLE BRANCH IN SUBSYSTEM` with a quoted subsystem name. |
| [jbarberia/DespachoDeSeguridadPSSE](https://github.com/jbarberia/DespachoDeSeguridadPSSE) `IEEE14.con` | Explicit cases with bare names and `OPEN BRANCH FROM BUS i TO BUS j`. |
| [Guzman-Dufrechou/Flujo_DC-uy](https://github.com/Guzman-Dufrechou/Flujo_DC-uy) `IEEE5bus.con` | `REMOVE MACHINE id FROM BUS i` beside branch openings in one file. |
| [GOCompetition/Validation](https://github.com/GOCompetition/Validation) `case14.con` | Machine and branch cases generated per element, one action per case. |
| [ORNL/ExaGO](https://github.com/ORNL/ExaGO) `case9_pw.con` | A generated file whose case names encode the element the case outages. |
| The PSS/E example set `savnw.con`, via [Power-Agent/PowerMCP](https://github.com/Power-Agent/PowerMCP) | The example file Siemens ships: `CIRCUIT` on a branch line, a file `END`. |
| [PowerGem/VSCodeExtension](https://github.com/PowerGem/VSCodeExtension) `examples/Contingency.con` | The TARA statements: `BUSNUMBERS`, `BUSNAMES`, `BRANCHNAMES`, bus names in place of numbers, `DEFAULT DISPATCH` blocks, `//` comments. |
| [psspy.org question 9168](https://psspy.org/psse-help-forum/question/9168/) | `SKIP` blocks and the `i TO j CKT c` lines inside them. |
| `ACTIVSg2000.con` (machine-local corpus, see `tests/data/local_psse_contingency_corpus.tsv`) | The column spacing `BUS   1001`, case names holding an apostrophe, a file with no trailing `END`, and an empty case. |
