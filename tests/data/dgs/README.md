# DGS fixtures

DIgSILENT PowerFactory DGS plaintext exports, used by `powerio/tests/dgs_parity.rs`
to validate the `dgs` reader against the MATPOWER companion of the same case.

DGS is the format PowerFactory writes for data exchange; the `.pfd` project
export is encrypted and has no public decoder (see `docs/powerfactory.md`).

## Files

| file | case | DGS schema | ids | decimals | source |
| --- | --- | --- | --- | --- | --- |
| `IEEE_39.dgs` | IEEE 39 New England | V5.0 | integer | dot | VeraGrid |
| `IEEE118_v2_test.dgs` | IEEE 118 | V7.0 | string/mixed | comma | VeraGrid |

The two span the format axes the reader must handle: V5 vs V7 schema, integer
`ID` vs string `FID`, and dot vs comma decimal separator.

## Provenance and license

`SPDX-License-Identifier: MPL-2.0` (for `IEEE_39.dgs` and `IEEE118_v2_test.dgs`).

Both `.dgs` files come from VeraGrid (https://github.com/SanPen/VeraGrid), which
is MPL-2.0. The full license text is vendored alongside them in
`VeraGrid-LICENSE.md`. The `.dgs` files carry no embedded SPDX header; what is
inside each is DIgSILENT's exporter banner (`DIgSILENT (R) DGS Export ...
Copyright (C) DIgSILENT GmbH ... All rights reserved`), which is boilerplate
about the export tool, not a license grant on the network data. The MPL-2.0
grant is VeraGrid's, applying to the files as distributed in their repository.

- `IEEE_39.dgs`: `Grids_and_profiles/grids/IEEE_39.dgs`
- `IEEE118_v2_test.dgs`: `src/tests/data/grids/DGS/IEEE118_v2_test.dgs`

Retrieved 2026-06-16. The IEEE 39 file is a PowerFactory V15.1.7 export; the
IEEE 118 file is a PowerFactory V25.0.3.0 export.

sha256:

```
fccf0d0087ba4b69c5e3dfb8b2cfaf49e6a47f11d7582903bbc99139bb0d090c  IEEE_39.dgs
5222e5d6e13877ca7a7b7ca19e884127b8e1b9f64050eb85f403d3f1a98dc2db  IEEE118_v2_test.dgs
fab3dd6bdab226f1c08630b1dd917e11fcb4ec5e1e020e2c16f83a0a13863e85  VeraGrid-LICENSE.md
```

The MATPOWER companions are `tests/data/case39.m` and the existing
`tests/data/case118.m`. These are public IEEE benchmark cases from the MATPOWER
project; MATPOWER's code is BSD-3 but MATPOWER states its case files are not
covered by that BSD license, the same status as the other `case*.m` fixtures
already in `tests/data/`.

## Parity scope

Topology round-trips exactly: the IEEE 39 DGS bus set and branch endpoint set
match `case39.m` (39 buses, 46 branches, 10 generators); the IEEE 118 DGS bus
set and generator count match `case118.m`.

Electrical parameters are not compared. A DGS export stores ohmic type
impedances scaled by line length against its own voltage bases; a MATPOWER case
stores reduced per-unit values from a different parameter source. The IEEE 118
export is also a 179-branch variant (170 lines + 9 transformers) against
case118's 186, a difference in the export's line set. These are provenance
differences, not parser defects.
