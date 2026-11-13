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

`case14_v577.pwb` and `case14_v483.pwb` are PowerWorld Simulator 24 (build
May 20, 2026) saves of MATPOWER's `case14` (`../case14.m`, BSD-3, see
`../LICENSE-MATPOWER.txt`), made 2026-11-13 for the `.pwb` reader's header
577 and 483 layouts. PowerIO's aux writer (the writer of eigenergy/powerio#869,
commit `b84ebb3e`) wrote `case14.m` as aux. The Gen block was then given a
`GenRegNum` column so that three machines regulate a remote bus (the machines
at buses 2, 6, and 8 regulate buses 4, 13, and 7; the others their own bus).
Simulator loaded the aux through SimAuto (`NewCase`, `ProcessAuxFile`) and
saved it with `SaveCase` as type `PWB` (header 577) and `PWB20` (header 483).

Each file is the save cut just before the record (tag `0x5529`, u32 length,
text) that lists the saving installation's licensed Simulator add-ons. The
cut keeps the header, the case description, and every table the reader
decodes; the reader decodes the cut file to the same records as the whole
save. The bytes are otherwise as Simulator wrote them.

| file | bytes | sha256 |
|---|---|---|
| `case14_v577.pwb` | 15246 | `e90a202c835e53cbc94bf4d727f08193cf7fc0eb3719d419ed3ae5015855780c` |
| `case14_v483.pwb` | 12504 | `926473f70b3bc5041a28c82fd8049034dc948b519170b0ab01ddf7f73885d44d` |

`powerio-tx/tests/powerworld_pwb.rs` checks both against `case14.m`. Simulator
gives a bus with no nominal voltage 138 kV, so the test expects 138 kV where
`case14.m` states 0.

## Simulator 24 saves of an edited case14

`case14_edits_v577.pwb`, `case14_edits_v425.pwb`, and `case14_edits_v338.pwb`
are PowerWorld Simulator 24 (build May 20, 2026) saves of MATPOWER's `case14`
(`../case14.m`, BSD-3, see `../LICENSE-MATPOWER.txt`), made 2026-11-13 for the
`.pwb` reader's shunt MW, phase shift, and per bus voltage limit decodes. The
aux was PowerIO's aux writer output for `case14.m` (the writer of
eigenergy/powerio#869, commit `b84ebb3e`) with these edits, each a later DATA
section or a split of the Bus section:

- buses 13 and 14 written without the `BusVoltLim`, `BusVoltLimHigh:1`, and
  `BusVoltLimLow:1` fields, then bus 14 given `BusVoltLim` YES alone;
- bus 1 given limit sets A to D of 0.91/1.01, 0.92/1.02, 0.93/1.03, and
  0.94/1.04, and bus 2 `BusVoltLim` NO with set A 0.95/1.05 and B to D
  0.9/1.1; bus 3 `BusVoltLim` NO;
- the bus 9 shunt's `SSNMW` set to 12.5;
- the 5-6 transformer's `LinePhase` set to -3.5;
- bus shunts (`BusG:1`, `BusB:1`) of 2.5/10 on bus 1, 0/-5 on bus 3, and
  1.5/0 on bus 13.

Simulator loaded it through SimAuto (`NewCase`, `ProcessAuxFile`) and saved it
with `SaveCase` as type `PWB` (header 577), `PWB19` (425), and `PWB17` (338).
Each file is cut before the add-on record exactly as the case14 saves above;
the bytes are otherwise as Simulator wrote them.

| file | bytes | sha256 |
|---|---|---|
| `case14_edits_v577.pwb` | 15238 | `4336fe94a539ff72ea477ee551eb4d0b694a5df443ea70337ac624d08416244b` |
| `case14_edits_v425.pwb` | 12215 | `1a1600b53c0b45ac9ab45e5f56c3c7188e992cb6952df6ff6eb6271c2f99d2c9` |
| `case14_edits_v338.pwb` | 11100 | `3ce1a213d0cb85c9f9c1dd2f5e3ebaed9de1857be6033feb6a8c64c31c5e837e` |

`powerio-tx/tests/powerworld_pwb.rs` checks all three against `case14.m` with
the edits applied and against the voltage limits Simulator reported for the
loaded case.
