# ACTIVSg200 fixtures

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
`benchmarks/fetch_powerworld.sh` (ACTIVSg2000 sibling sets from TAMU and from
PowerWorld Corporation's synthetic case page,
<https://www.powerworld.com/new-synthetic-power-flow-cases>) and
`benchmarks/fetch_cases.sh` (MATPOWER exports of the larger grids).
