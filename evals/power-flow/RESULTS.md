# Conversion benchmark results

This local run used the files and solver versions in [the benchmark instructions](README.md).
The [Conversion Matrix comment on PR #964](https://github.com/eigenergy/powerio/pull/964#issuecomment-7178483745)
links to the GitHub run with the full results and machine-readable evidence.

## Original files and new files in the same format

| Input | Original files recovered exactly | Grid data after writing and reading new files |
| --- | --- | --- |
| matpower-case14 | agree | different |
| opendss-ieee13 | agree | different |
| mg-ravens-case3 | agree | different |

## AC power flow and admittance

36 of 39 source profiles have fixtures. 25 original cases were solved. 873 comparisons measured bus or conductor voltages, including 593 with independent readers for both files. 788 comparisons measured admittance.

Compared with `ba36069c707d`, 0 checks lost coverage. See `changes.json` for changes in measurements, inputs, and solver versions.

Maximum absolute voltage difference and relative admittance 2-norm

| Input | Output | Original vs converted voltage | Original vs converted admittance | Original vs converted back voltage | Converted vs converted back voltage |
| --- | --- | --- | --- | --- | --- |
| matpower-case14 | matpower | 0 p.u. (pass) | 0 (pass) | — | — |
| matpower-case14 | powermodels-json | 1.55e-15 p.u. (pass) | 5.74e-17 (pass) | 0 p.u. (pass) | 1.55e-15 p.u. (pass) |
| matpower-case14 | psse | 4.44e-16 p.u. (pass) | 1.04e-16 (pass) | 2.22e-16 p.u. (pass) | 4.44e-16 p.u. (pass) |
| matpower-case14 | psse34 | — | — | 2.22e-16 p.u. (pass) | — |
| matpower-case14 | psse35 | 4.44e-16 p.u. (pass) | 1.04e-16 (pass) | 2.22e-16 p.u. (pass) | 4.44e-16 p.u. (pass) |
| matpower-case14 | psse-rawx | 4.44e-16 p.u. (pass) | 1.04e-16 (pass) | 2.22e-16 p.u. (pass) | 4.44e-16 p.u. (pass) |
| matpower-case14 | powerworld | 1.55e-15 p.u. (pass) | 5.74e-17 (pass) | 0 p.u. (pass) | 1.55e-15 p.u. (pass) |
| matpower-case14 | egret-json | 6.66e-16 p.u. (pass) | 6.39e-17 (pass) | 0 p.u. (pass) | 6.66e-16 p.u. (pass) |
| matpower-case14 | pandapower-json | 6.2e-12 p.u. (pass) | 8.58e-17 (pass) | 2.22e-16 p.u. (pass) | 6.2e-12 p.u. (pass) |
| matpower-case14 | surge-json | 4.36e-07 p.u. (differs) | 1.18e-16 (pass) | 0 p.u. (pass) | 4.36e-07 p.u. (differs) |
| matpower-case14 | powersystems-json | 9.67e-08 p.u. (differs) | 2.39e-08 (differs) | 2.22e-16 p.u. (pass) | 9.67e-08 p.u. (differs) |
| matpower-case14 | sienna-json | 1.11e-15 p.u. (pass) | 1.18e-16 (pass) | 2.22e-16 p.u. (pass) | 1.11e-15 p.u. (pass) |
| matpower-case14 | sienna-0-1-json | 1.11e-15 p.u. (pass) | 1.18e-16 (pass) | 2.22e-16 p.u. (pass) | 1.11e-15 p.u. (pass) |
| matpower-case14 | sienna-0-2-json | 1.11e-15 p.u. (pass) | 1.18e-16 (pass) | 2.22e-16 p.u. (pass) | 1.11e-15 p.u. (pass) |
| matpower-case14 | sienna-0-3-json | 1.11e-15 p.u. (pass) | 1.18e-16 (pass) | 2.22e-16 p.u. (pass) | 1.11e-15 p.u. (pass) |
| matpower-case14 | sienna-0-4-json | 1.11e-15 p.u. (pass) | 1.18e-16 (pass) | 2.22e-16 p.u. (pass) | 1.11e-15 p.u. (pass) |
| matpower-case14 | ravens-json | 8.88e-16 p.u. (pass) | 8.58e-17 (pass) | 0.09 p.u. (differs) | 0.09 p.u. (differs) |
| matpower-case14 | pslf | — | — | 2.22e-16 p.u. (pass) | — |
| matpower-case14 | xiidm | 4.44e-16 p.u. (pass) | 9.07e-17 (pass) | 4.44e-16 p.u. (pass) | 4.44e-16 p.u. (pass) |
| matpower-case14 | jiidm | 4.44e-16 p.u. (pass) | 9.07e-17 (pass) | 4.44e-16 p.u. (pass) | 4.44e-16 p.u. (pass) |
| matpower-case14 | cgmes | 1.01e-06 p.u. (differs) | 1.27e-16 (pass) | 4.44e-16 p.u. (pass) | 1.01e-06 p.u. (differs) |
| matpower-case14 | ucte | — | — | 0.0222 p.u. (differs) | — |
| matpower-case14 | pypsa-csv | 4.44e-16 p.u. (pass) | 9.55e-15 (pass) | 4.44e-16 p.u. (pass) | 4.44e-16 p.u. (pass) |
| opendss-ieee13 | dss | 138 V (differs) | 1.29e-05 (differs) | — | — |
| opendss-ieee13 | bmopf-json | — | — | 161 V (differs) | — |
| mg-ravens-case3 | dss | 1.58e-08 V (pass) | — | 2.27e-13 V (pass) | 1.58e-08 V (pass) |
| mg-ravens-case3 | pmd-json | 4.86e-10 V (pass) | — | 0 V (pass) | 4.86e-10 V (pass) |
| mg-ravens-case3 | pmd-math-json | 4.86e-10 V (pass) | — | 0 V (pass) | 4.86e-10 V (pass) |
| mg-ravens-case3 | bmopf-json | 0 V (pass) | — | 0 V (pass) | 0 V (pass) |
| mg-ravens-case3 | ravens-json | 0 V (pass) | — | — | — |
| mg-ravens-case3 | cim-dist | — | — | 3.41e-12 V (pass) | — |

- MG-RAVENS nominal load in Tellegen vs upstream OpenDSS example — 3.33 V (differs) across 9 matched nodes.

- IEEE 13-bus source solved in OpenDSS and PowerModelsDistribution — 155 V (differs) across 41 matched nodes.

- pandapower source solved in pandapower and LightSim2Grid — Voltage controlling elements, i.e. generators, external grids, or DC lines, at the same bus have different setpoints.

- Deliberate error test passed. Doubling branch 1–2 reactance gave a relative admittance difference of 0.255.

- MATPOWER and LightSim2Grid on the converted case14 — 6.2e-12 p.u. across 14 buses.

— means that quantity was not measured. The full matrix gives all three comparisons for every route. The JSON gives each metric, tolerance, solver, and reason for missing coverage.

The earlier local run at `ba36069c707d` measured voltage in 499 comparisons and
admittance in 196. The new run records checks that used independent file
readers and checks that used PowerIO to parse a file before solving.

The original IEEE 13-bus feeder and the new OpenDSS files both solved on
41 conductors. The largest voltage change was 137.631 V at bus 680, phase 1.
PowerModelsDistribution also solved the original feeder with its neutral
modeled explicitly. Its result differed from OpenDSS by 154.649 V across
the same 41 nodes.

PowerModelsDistribution could not load the new IEEE 13-bus ENGINEERING
JSON. The full report records the parser's error. The PNNL GridLAB-D
IEEE four-node case solved in its original format and after conversion
to OpenDSS, PMD MATHEMATICAL JSON, and both GLM profiles. Voltage magnitude
after writing a new GLM differed by about 0.000406 V, within the 0.001 V
limit. The maximum angle difference exceeded its separate limit.

PSS/E RAW revision 34 could not be solved in PowSybl, PowerModels, or
Surge. SiennaSchemas 0.1, 0.3, and 0.4 have no pinned original source file
in this benchmark. Converted files in those formats are included in the
matrix.

## Full conversion matrix

The three marks in each cell compare original with converted, original with converted-back,
and converted with converted-back, in that order.
The reference is the result of solving the original file. See the JSON for individual failures,
numerical differences, and reasons a comparison could not run.

| Input | Original files recovered | Grid data in new files written in the original format |
| --- | --- | --- |
| matpower-case14 | agree | different |
| psse-raw32-savnw | different | agree |
| psse-raw33-case14 | agree | agree |
| psse-raw34-case14 | agree | agree |
| psse-raw35-case14 | agree | agree |
| psse-rawx-case14 | agree | different |
| powermodels-case30 | agree | different |
| egret-case14 | agree | different |
| pandapower-example | agree | different |
| pypsa-example | different | different |
| surge-case14 | agree | agree |
| powerworld-activsg200 | agree | different |
| pslf-activsg200 | agree | different |
| xiidm-eurostag | agree | agree |
| jiidm-eurostag | agree | agree |
| cgmes-ieee14 | agree | different |
| ucte-sn3-fr2 | agree | different |
| powersystems-case9 | different | conversion_failed |
| sienna-example | agree | agree |
| sienna-0-1-reference | unavailable | unavailable |
| sienna-0-2-example | agree | agree |
| sienna-0-3-reference | unavailable | unavailable |
| sienna-0-4-reference | unavailable | unavailable |
| ieee-cdf-case14 | unavailable | unavailable |
| powerworld-binary-case14 | unavailable | unavailable |
| go-challenge-3 | agree | unavailable |
| deepmind-opfdata | agree | unavailable |
| sincal-balanced | agree | unavailable |
| gridfm-case24 | not_applicable | unavailable |
| opendss-ieee13 | agree | different |
| mg-ravens-case3 | agree | different |
| pmd-case3 | agree | agree |
| pmd-math-case3 | agree | different |
| bmopf-ieee13 | different | different |
| cim-distribution-tank | agree | agree |
| gridlabd-ieee4 | agree | different |
| gridlabd-arras-ieee4 | agree | different |
| pandapower-three-phase | agree | unavailable |
| sincal-distribution | unavailable | unavailable |

## Transmission

| Input | matpower | powermodels-json | psse | psse34 | psse35 | psse-rawx | powerworld | egret-json | pandapower-json | surge-json | powersystems-json | sienna-json | sienna-0-1-json | sienna-0-2-json | sienna-0-3-json | sienna-0-4-json | ravens-json | pslf | xiidm | jiidm | cgmes | ucte | pypsa-csv | gridfm |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| matpower-case14 | 🟢 / — / — | 🟢 / 🟢 / 🟢 | 🟢 / 🟢 / 🟢 | 🔴 / 🟢 / — | 🟢 / 🟢 / 🟢 | 🟢 / 🟢 / 🟢 | 🟢 / 🟢 / 🟢 | 🟢 / 🟢 / 🟢 | 🟢 / 🟢 / 🟢 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🟢 / 🟢 / 🟢 | 🟢 / 🟢 / 🟢 | 🟢 / 🟢 / 🟢 | 🟢 / 🟢 / 🟢 | 🟢 / 🟢 / 🟢 | 🟢 / 🔴 / 🔴 | 🔴 / 🟢 / — | 🟢 / 🟢 / 🟢 | 🟢 / 🟢 / 🟢 | 🔴 / 🟢 / 🔴 | — / 🔴 / — | 🟢 / 🟢 / 🟢 | — / 🔴 / 🔴 |
| psse-raw32-savnw | 🔴 / — / — | 🔴 / — / — | 🟢 / — / — | 🔴 / — / — | 🟢 / — / — | 🟢 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🟢 / — / — | 🟢 / — / — | 🔴 / — / — | — / — / — | 🔴 / — / — | — / — / — |
| psse-raw33-case14 | 🟢 / 🟢 / 🟢 | 🔴 / 🟢 / 🔴 | 🟢 / — / — | 🔴 / 🟢 / — | 🟢 / 🟢 / 🟢 | 🟢 / 🟢 / 🟢 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🟢 / 🟢 / 🟢 | 🔴 / 🟢 / 🔴 | — / 🟢 / — | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🟢 / — | 🟢 / 🟢 / 🟢 | 🟢 / 🟢 / 🟢 | 🔴 / 🟢 / 🔴 | — / 🔴 / — | 🟢 / 🟢 / 🟢 | — / 🔴 / 🔴 |
| psse-raw34-case14 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | 🔴 / — / — | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 |
| psse-raw35-case14 | 🔴 / 🔴 / 🟢 | 🔴 / 🔴 / 🟢 | 🔴 / 🔴 / 🟢 | 🔴 / 🔴 / — | 🔴 / — / — | 🔴 / 🔴 / 🟢 | 🔴 / 🔴 / 🟢 | 🔴 / 🔴 / 🟢 | 🔴 / 🔴 / 🟢 | 🔴 / 🔴 / 🔴 | — / 🔴 / — | 🔴 / 🔴 / 🟢 | 🔴 / 🔴 / 🟢 | 🔴 / 🔴 / 🟢 | 🔴 / 🔴 / 🟢 | 🔴 / 🔴 / 🟢 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / — | 🔴 / 🔴 / 🟢 | 🔴 / 🔴 / 🟢 | 🔴 / 🔴 / 🔴 | — / 🔴 / — | 🔴 / 🔴 / 🟢 | — / 🔴 / 🔴 |
| psse-rawx-case14 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🟢 / 🟢 / 🟢 | 🔴 / 🟢 / — | 🟢 / 🟢 / 🟢 | 🟢 / — / — | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | — / 🟢 / — | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🟢 / — | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / — / — | — / 🔴 / — | 🔴 / 🔴 / 🟢 | — / 🔴 / 🔴 |
| powermodels-case30 | 🟢 / 🟢 / 🟢 | 🟢 / — / — | 🟢 / 🟢 / 🟢 | 🔴 / 🟢 / — | 🟢 / 🟢 / 🟢 | 🟢 / 🟢 / 🟢 | 🟢 / 🟢 / 🟢 | 🟢 / 🟢 / 🟢 | 🟢 / 🟢 / 🟢 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🟢 / 🟢 / 🟢 | 🟢 / 🟢 / 🟢 | 🟢 / 🟢 / 🟢 | 🟢 / 🟢 / 🟢 | 🟢 / 🟢 / 🟢 | 🟢 / 🟢 / 🟢 | 🔴 / 🟢 / — | — / 🟢 / — | — / 🟢 / — | — / 🟢 / — | — / 🔴 / — | 🟢 / 🟢 / 🟢 | — / 🔴 / 🔴 |
| egret-case14 | 🟢 / 🟢 / 🟢 | 🟢 / 🟢 / 🟢 | 🟢 / 🟢 / 🟢 | 🔴 / 🟢 / — | 🟢 / 🟢 / 🟢 | 🟢 / 🟢 / 🟢 | 🟢 / 🟢 / 🟢 | 🟢 / — / — | 🟢 / 🟢 / 🟢 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🟢 / 🟢 / 🟢 | 🟢 / 🟢 / 🟢 | 🟢 / 🟢 / 🟢 | 🟢 / 🟢 / 🟢 | 🟢 / 🟢 / 🟢 | 🟢 / 🟢 / 🟢 | 🔴 / 🟢 / — | — / 🟢 / — | — / 🟢 / — | 🔴 / 🟢 / 🔴 | — / 🔴 / — | 🟢 / 🟢 / 🟢 | — / 🔴 / 🔴 |
| pandapower-example | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | — / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / 🔴 / 🔴 | 🔴 / — / — | 🔴 / 🔴 / 🔴 | 🔴 / — / — | 🔴 / — / — | — / 🔴 / 🔴 |
| pypsa-example | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / — | 🔴 / 🔴 / — | 🔴 / 🔴 / — | 🔴 / 🔴 / — | 🔴 / 🔴 / — | 🔴 / 🔴 / 🟢 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / 🔴 / 🟢 | 🔴 / 🔴 / — | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | — / — / 🟢 | 🔴 / — / — | — / 🔴 / 🔴 |
| surge-case14 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / — | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🟢 / — / — | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🟢 / — | — / 🟢 / — | — / 🟢 / — | 🔴 / 🟢 / — | — / 🔴 / — | 🔴 / 🔴 / 🔴 | — / 🔴 / 🔴 |
| powerworld-activsg200 | 🔴 / 🟢 / 🔴 | 🟢 / 🟢 / 🟢 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / — | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🟢 / — / — | 🟢 / 🟢 / 🟢 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🟢 / 🟢 / 🟢 | 🟢 / 🟢 / 🟢 | 🟢 / 🟢 / 🟢 | 🟢 / 🟢 / 🟢 | 🟢 / 🟢 / 🟢 | 🟢 / 🟢 / 🟢 | 🔴 / 🟢 / — | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | — / 🟢 / — | 🔴 / 🔴 / — | 🔴 / 🟢 / 🔴 | — / 🔴 / 🔴 |
| pslf-activsg200 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | 🔴 / — / — | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 |
| xiidm-eurostag | 🔴 / 🔴 / ◐ | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / ◐ | 🔴 / 🔴 / — | 🔴 / 🔴 / ◐ | 🔴 / 🔴 / ◐ | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / ◐ | 🔴 / 🔴 / ◐ | — / 🔴 / — | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / — | 🔴 / — / — | 🔴 / 🔴 / ◐ | ◐ / 🔴 / 🔴 | — / — / — | 🔴 / 🔴 / ◐ | — / 🔴 / 🔴 |
| jiidm-eurostag | 🔴 / 🔴 / ◐ | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / ◐ | 🔴 / 🔴 / — | 🔴 / 🔴 / ◐ | 🔴 / 🔴 / ◐ | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / ◐ | 🔴 / 🔴 / ◐ | — / 🔴 / — | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / — | 🔴 / 🔴 / ◐ | 🔴 / — / — | ◐ / 🔴 / 🔴 | — / — / — | 🔴 / 🔴 / ◐ | — / 🔴 / 🔴 |
| cgmes-ieee14 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / — | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | — / 🟢 / — | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / — | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🟢 / — / — | 🔴 / 🔴 / 🔴 | 🔴 / 🟢 / 🔴 | — / 🔴 / 🔴 |
| ucte-sn3-fr2 | 🔴 / 🔴 / 🔴 | ◐ / ◐ / ◐ | ◐ / ◐ / ◐ | 🔴 / ◐ / — | ◐ / ◐ / ◐ | ◐ / ◐ / ◐ | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / ◐ / 🔴 | 🔴 / ◐ / — | ◐ / ◐ / ◐ | ◐ / ◐ / ◐ | ◐ / ◐ / ◐ | ◐ / ◐ / ◐ | ◐ / ◐ / ◐ | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / — | 🔴 / ◐ / — | 🔴 / ◐ / — | 🔴 / ◐ / 🔴 | ◐ / — / — | 🔴 / ◐ / 🔴 | — / 🔴 / 🔴 |
| powersystems-case9 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / — | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / — / — | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / — | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🔴 / 🔴 | — / 🔴 / — | 🔴 / 🔴 / 🔴 | — / 🔴 / 🔴 |
| sienna-example | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / — / — | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / — / — |
| sienna-0-1-reference | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — |
| sienna-0-2-example | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / — / — | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / — / — |
| sienna-0-3-reference | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — |
| sienna-0-4-reference | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — |
| ieee-cdf-case14 | — / — / — | — / — / — | — / — / — | 🔴 / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | — / — / — | — / — / — | — / — / — | — / — / — |
| powerworld-binary-case14 | 🔴 / — / — | 🟢 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🟢 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🟢 / — / — | 🟢 / — / — | 🟢 / — / — | 🟢 / — / — | 🟢 / — / — | 🟢 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | — / — / — | 🔴 / — / — | 🔴 / — / — |
| go-challenge-3 | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — |
| deepmind-opfdata | 🟢 / — / — | 🟢 / — / — | — / — / — | 🔴 / — / — | — / — / — | — / — / — | 🟢 / — / — | 🟢 / — / — | — / — / — | 🟢 / — / — | 🔴 / — / — | 🟢 / — / — | 🟢 / — / — | 🟢 / — / — | 🟢 / — / — | 🟢 / — / — | 🟢 / — / — | 🔴 / — / — | — / — / — | — / — / — | — / — / — | 🔴 / — / — | 🟢 / — / — | — / — / — |
| sincal-balanced | 🔴 / — / — | 🟢 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | — / — / — |
| gridfm-case24 | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — |

🟢 All comparisons passed. ◐ Only some quantities could be compared.
🔴 A measured difference or a failed conversion or solve. — See the JSON for the reason.

## Distribution

| Input | dss | pmd-json | pmd-math-json | bmopf-json | ravens-json | cim-dist | gridlabd | gridlabd-arras |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| opendss-ieee13 | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | — / 🔴 / — | — / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — |
| mg-ravens-case3 | ◐ / ◐ / ◐ | ◐ / ◐ / ◐ | ◐ / ◐ / ◐ | ◐ / ◐ / ◐ | ◐ / — / — | 🔴 / ◐ / — | 🔴 / — / — | 🔴 / — / — |
| pmd-case3 | ◐ / 🔴 / 🔴 | ◐ / — / — | ◐ / ◐ / ◐ | 🔴 / 🔴 / 🔴 | ◐ / ◐ / ◐ | 🔴 / 🔴 / 🔴 | 🔴 / — / — | 🔴 / — / — |
| pmd-math-case3 | 🔴 / 🔴 / ◐ | ◐ / ◐ / ◐ | ◐ / — / — | 🔴 / 🔴 / — | 🔴 / 🔴 / — | 🔴 / 🔴 / — | 🔴 / — / — | 🔴 / — / — |
| bmopf-ieee13 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / — / — | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / — / — | 🔴 / — / — |
| cim-distribution-tank | — / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — |
| gridlabd-ieee4 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / — / — | 🔴 / 🔴 / ◐ |
| gridlabd-arras-ieee4 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / ◐ | 🔴 / — / — |
| pandapower-three-phase | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — |
| sincal-distribution | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — |

🟢 All comparisons passed. ◐ Only some quantities could be compared.
🔴 A measured difference or a failed conversion or solve. — See the JSON for the reason.
