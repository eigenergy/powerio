# PSS/E RAW fixtures

## From PowSybl Core (MPL-2.0)

`ExampleVersion32_exported.raw` and `IEEE_30_bus.raw` are unmodified copies of
two PSS/E RAW revision 32 test resources in
[PowSybl Core](https://github.com/powsybl/powsybl-core) at commit
`0939bfcc2c0c094de907dc818dd688b4cbfb7281` (the 7.3.0 release, the same
commit the PowSybl interoperability gate in `evals/powsybl` checks out).
PowSybl Core is distributed under the Mozilla Public License 2.0; the files
carry no license header of their own.

| File | Source file in PowSybl Core | SHA-256 |
| --- | --- | --- |
| `ExampleVersion32_exported.raw` | `psse/psse-converter/src/test/resources/ExampleVersion32_exported.raw` | `4769a4c1a39f4441f508285b2828f35adae70a1f13f3f1507cd30590d34b9394` |
| `IEEE_30_bus.raw` | `psse/psse-converter/src/test/resources/IEEE_30_bus.raw` | `0e281169ed4370ca6f59f2cba788cc6c6da152a251e4c006c1f93e83ff3ac2d7` |

No edits were made. `ExampleVersion32_exported.raw` is the eight bus case
PowSybl writes to check its own revision 32 export: four lines (one a self
loop on bus 3), four two winding transformers with fixed windings, one
generator, two loads, a fixed shunt and a switched shunt on bus 7, and an area,
zone, and owner record. `IEEE_30_bus.raw` is the University of Washington
archive IEEE 30 bus test case as PSS/E writes it at revision 32.

## Original to this repository

Every other file here is original to this repository: `case5.raw`,
`case14.raw`, `case3_3w_v33.raw`, and `case7_v32.raw` are hand written
minimal cases whose title lines say what each exercises, and `case14_v34.raw`
and `case14_v35.raw` were written from `../case14.m` with
`powerio convert --to psse34` and `--to psse35`. `case7_v32.raw` states every
record type the reader maps in the revision 32 layout: bus, load, fixed shunt,
generator, branch, two and three winding transformer, area, two-terminal DC
line, and switched shunt, plus zone, owner, inter-area transfer, and impedance
correction records the reader skips and reports.
