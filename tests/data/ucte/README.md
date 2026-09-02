# UCTE-DEF fixtures

Small `.uct` cases for the UCTE-DEF reader and writer tests.

## Vendored from PowSybl Core

Both files are copied verbatim (no edits) from the `powsybl/powsybl-core`
repository at commit `0939bfcc2c0c094de907dc818dd688b4cbfb7281` (v7.3.0),
which is licensed under the Mozilla Public License 2.0.

| File | PowSybl source path | Bytes |
| --- | --- | --- |
| `20170322_1844_SN3_FR2.uct` | `ucte/ucte-network/src/test/resources/20170322_1844_SN3_FR2.uct` | 1134 |
| `elementName.uct` | `ucte/ucte-converter/src/test/resources/elementName.uct` | 1531 |

`20170322_1844_SN3_FR2.uct` is a five node French case with two parallel
lines, two transformers, and one phase regulation; its file name follows the
UCTE naming convention, so the reader dates the case from it. `elementName.uct`
adds Belgian nodes, three cross border nodes, a busbar coupler, tie lines with
element names, and a regulation record that names a transformer the `##T`
block does not declare.

The remaining PowSybl `.uct` fixtures are not copied; the PowSybl
interoperability gate (`evals/powsybl/run.sh`) reads them from a sparse
checkout of the same commit.

## Synthetic

`synthetic_all_blocks.uct` is written for these tests and carries every block
of revision 2007.05.01: `##C`, `##N` with three `##Z` country groups including
the cross border group `##ZXX`, `##L` with a real line, an out of operation
parallel line, a busbar coupler, an equivalent element under the reactance
floor, and a tie line pair, `##T` with two transformers, `##R` with one phase
regulation and one symmetrical angle regulation, and the `##TT` and `##E`
blocks the reader retains in the source text. The generation record of the
slack node exercises every optional node column, including the primary control
and short circuit fields and the power plant type.
