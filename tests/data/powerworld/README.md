# PowerWorld fixtures

## ACTIVSg200

Sibling exports of the same synthetic 200 bus case, all produced from one
PowerWorld case by the creators of the ACTIVSg grids. Source: TAMU Electric
Grid Test Case Repository, <https://electricgrids.engr.tamu.edu/>, retrieved
2026-06-10. The case is synthetic, contains no CEII (stated in the case
header), and is published as:

> A. B. Birchfield, T. Xu, K. M. Gegner, K. S. Shetye, and T. J. Overbye,
> "Grid Structural Characteristics as Validation Criteria for Synthetic
> Networks," IEEE Transactions on Power Systems, vol. 32, no. 4, 2017.

Vendored bytes are pinned; do not reformat or re-export these files.

| file | format | sha256 |
|---|---|---|
| `ACTIVSg200.aux` | PowerWorld auxiliary (complete case export) | `72a17506e1cb26df6c76373613f6d1871c3b14da328556debaf755e8fe2650c1` |
| `ACTIVSg200.pwb` | PowerWorld binary case | `99b7512b92b3f9e897b15d579847b99c1ec626d220a12598018af12e0eee79b1` |
| `ACTIVSg200.RAW` | PSS/E raw (v33) | `fca4f71886a67c38c45979bc388476f6de044b64f426c7723afe1a025b988477` |
| `case_ACTIVSg200.m` | MATPOWER | `3c92cb217e1e04bb764d2566ccf01f3f2e2ac8af2d6b2907b0619ee335165c87` |
| `ACTIVSg200.pwd` | PowerWorld display (diagram sibling) | `be67278b62b474ece7750e5d548ffc64a192a9377e2a082547e738a1375672f8` |

The case files describe the same network, so they serve as cross format
oracles for each other (the `.pwd` carries the diagram, not the case; its
substation coordinates are checked against the `.aux` geography): counts and values parsed from one format are checked
against the others in `powerio/tests/` parity tests. The `.aux` and `.pwb`
are a same day export pair; the `.RAW` and `.m` are earlier revisions of the
case, so parity against them is structural (identities, impedances, limits)
rather than solved state.

Larger ACTIVSg cases (2000 bus and up) are fetched, not vendored: see
`evals/validation/fetch_powerworld.sh` (ACTIVSg2000 sibling sets from TAMU and from
PowerWorld Corporation's synthetic case page,
<https://www.powerworld.com/new-synthetic-power-flow-cases>) and
`evals/validation/fetch_cases.sh` (MATPOWER exports of the larger grids).

## Simulator 24 saves of case14

The `case14_v*.pwb` files are PowerWorld Simulator 24 (build May 20, 2026)
saves of MATPOWER's `case14` (`../case14.m`, BSD-3, see
`../LICENSE-MATPOWER.txt`), made 2026-11-13 for the `.pwb` reader's layouts,
one file per header word. PowerIO's aux writer (the writer of
eigenergy/powerio#869, commit `b84ebb3e`) wrote `case14.m` as aux. The Gen
block was then given a `GenRegNum` column so that three machines regulate a
remote bus (the machines at buses 2, 6, and 8 regulate buses 4, 13, and 7;
the others their own bus). Simulator loaded the aux through SimAuto
(`NewCase`, `ProcessAuxFile`) and saved it with `SaveCase` as type `PWB`
(header 577), `PWB20` (483), `PWB17` (338), `PWB18` (368), `PWB19` (425),
`PWB21` (508), and `PWB22` (537). The `PWB20` save of a second SimAuto
session reproduces `case14_v483.pwb` byte for byte.

Each file is the save cut just before the record (tag `0x5529`, u32 length,
text) that lists the saving installation's licensed Simulator add-ons. The
cut keeps the header, the case description, and every table the reader
decodes; the reader decodes the cut file to the same records as the whole
save. The bytes are otherwise as Simulator wrote them.

| file | bytes | sha256 |
|---|---|---|
| `case14_v577.pwb` | 15246 | `e90a202c835e53cbc94bf4d727f08193cf7fc0eb3719d419ed3ae5015855780c` |
| `case14_v483.pwb` | 12504 | `926473f70b3bc5041a28c82fd8049034dc948b519170b0ab01ddf7f73885d44d` |
| `case14_v338.pwb` | 11108 | `74e5bc4d556de0ba289b394f1e607d691cb054980ecbd1a621a6860a991ad74a` |
| `case14_v368.pwb` | 11119 | `005717480c4cd5db7e6abf89fec154fa4bccad2cfb819a3bef034180be5a87fc` |
| `case14_v425.pwb` | 12223 | `e89bac6f3d78a1592a98b6adc3ef935f65b00aaf54b06a29ed008c14f14f3a37` |
| `case14_v508.pwb` | 12602 | `4dd66db5a2e59be4128866b14859fe5daf7c007515fb87d141448db8322f1939` |
| `case14_v537.pwb` | 13017 | `270260f6d41a0e3a0fef21da4c27ad91f7cb4e97e40c11fa844f1622045dcb87` |

`powerio-tx/tests/powerworld_pwb.rs` checks every file against `case14.m`. Simulator
gives a bus with no nominal voltage 138 kV, so the test expects 138 kV where
`case14.m` states 0.
