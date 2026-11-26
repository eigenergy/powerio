# Benchmark results

Measured at commit [`ba36069c`](https://github.com/eigenergy/powerio/commit/ba36069c707d741937a97d0a646e2d73622ca3e5).

The catalog lists 39 cases. The run had 36 input cases and obtained 18 solutions directly from their original files.
The matrix includes every scheduled source and target combination and shows where a comparison could run.

## MATPOWER case14

Each cell gives the maximum voltage magnitude difference in per unit, then the relative admittance matrix error.
The limits for those two measurements are both 1e-9. Branch powers and voltage angles are checked separately in the JSON report.

| Output format and solver | Original vs. converted | Original vs. converted back | Converted vs. converted back |
| --- | --- | --- | --- |
| Egret JSON (Egret) | Pass<br>6.66e-16 pu, 6.39e-17 | Pass<br>0 pu, 0 | Pass<br>6.66e-16 pu, 6.39e-17 |
| PowerModels JSON (PowerModels) | Pass<br>1.55e-15 pu, 5.74e-17 | Pass<br>0 pu, 0 | Pass<br>1.55e-15 pu, 5.74e-17 |
| Surge JSON (Surge) | Difference<br>4.36e-07 pu, 1.18e-16 | Pass<br>0 pu, 0 | Difference<br>4.36e-07 pu, 1.18e-16 |
| XIIDM (PowSybl) | Some checks unavailable<br>4.44e-16 pu, admittance unavailable | Pass<br>4.44e-16 pu, 8.57e-17 | Some checks unavailable<br>4.44e-16 pu, admittance unavailable |
| PSS/E RAW 33 (PowSybl) | Some checks unavailable<br>4.44e-16 pu, admittance unavailable | Pass<br>2.22e-16 pu, 5.71e-17 | Some checks unavailable<br>4.44e-16 pu, admittance unavailable |
| pandapower JSON (pandapower) | Pass<br>6.2e-12 pu, 8.58e-17 | Pass<br>2.22e-16 pu, 7.21e-17 | Pass<br>6.2e-12 pu, 6.94e-17 |
| PyPSA CSV (PyPSA) | Pass<br>4.44e-16 pu, 9.55e-15 | Pass<br>4.44e-16 pu, 6.9e-17 | Pass<br>4.44e-16 pu, 9.53e-15 |

Through Egret, PowerModels, and Surge, MATPOWER returned exactly the same solved values for the original and converted-back files. The intermediate Surge solution differed from MATPOWER by up to 0.00394 MW on a branch, even though their admittance matrices agreed to 1.18e-16 relative error.

## IEEE 13-bus feeder

After PowerIO wrote a new OpenDSS circuit, OpenDSS solved both files on the same 41 conductors. At bus 680, phase 1, voltage magnitude changed from 2360.459 V to 2222.828 V. The OpenDSS system admittance matrices differed by 1.29e-5 in relative matrix 2-norm.

## How many comparisons ran

| Files compared | All checks passed | Measured difference | Some checks unavailable | Other outcomes |
| --- | ---: | ---: | ---: | ---: |
| Original and converted | 16 | 84 | 32 | 644 |
| Original and converted back | 100 | 100 | 61 | 515 |
| Converted and converted back | 12 | 49 | 45 | 670 |

Other outcomes include a failed write or solve, a missing input or solver, a return
conversion that does not apply, and cases whose bus labels could not be matched.

SiennaSchemas 0.1, 0.3, and 0.4 still need suitable reference cases. The MG-RAVENS input is present and its conversion results are included.
The benchmark does not yet have an independent reader and solver for the original MG-RAVENS JSON.

[Run the benchmark again](README.md).

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
| opendss-ieee13 | different | different |
| mg-ravens-case3 | agree | different |
| pmd-case3 | agree | agree |
| pmd-math-case3 | agree | different |
| bmopf-ieee13 | different | different |
| cim-distribution-tank | agree | agree |
| gridlabd-ieee4 | agree | conversion_failed |
| gridlabd-arras-ieee4 | agree | conversion_failed |
| pandapower-three-phase | agree | unavailable |
| sincal-distribution | unavailable | unavailable |

### Transmission

| Input | matpower | powermodels-json | psse | psse34 | psse35 | psse-rawx | powerworld | egret-json | pandapower-json | surge-json | powersystems-json | sienna-json | sienna-0-1-json | sienna-0-2-json | sienna-0-3-json | sienna-0-4-json | ravens-json | pslf | xiidm | jiidm | cgmes | ucte | pypsa-csv | gridfm |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| matpower-case14 | 🟢 / — / — | 🟢 / 🟢 / 🟢 | ◐ / 🟢 / ◐ | 🔴 / 🟢 / — | ◐ / 🟢 / ◐ | — / 🟢 / — | — / 🟢 / — | 🟢 / 🟢 / 🟢 | 🟢 / 🟢 / 🟢 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | — / 🟢 / — | — / 🟢 / — | — / 🟢 / — | — / 🟢 / — | — / 🟢 / — | — / 🔴 / — | 🔴 / 🟢 / — | ◐ / 🟢 / ◐ | ◐ / 🟢 / ◐ | — / 🟢 / — | — / 🔴 / — | 🟢 / 🟢 / 🟢 | — / 🔴 / 🔴 |
| psse-raw32-savnw | 🔴 / — / — | 🔴 / — / — | ◐ / — / — | 🔴 / — / — | ◐ / — / — | ◐ / — / — | — / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | 🔴 / — / — | ◐ / — / — | ◐ / — / — | — / — / — | — / — / — | 🔴 / — / — | — / — / — |
| psse-raw33-case14 | ◐ / ◐ / ◐ | — / ◐ / — | ◐ / — / — | 🔴 / ◐ / — | ◐ / ◐ / ◐ | ◐ / ◐ / ◐ | — / ◐ / — | 🔴 / ◐ / 🔴 | ◐ / ◐ / ◐ | 🔴 / ◐ / 🔴 | — / ◐ / — | — / ◐ / — | — / ◐ / — | — / ◐ / — | — / ◐ / — | — / ◐ / — | — / 🔴 / — | 🔴 / ◐ / — | ◐ / ◐ / ◐ | ◐ / ◐ / ◐ | — / ◐ / — | — / 🔴 / — | ◐ / ◐ / ◐ | — / 🔴 / 🔴 |
| psse-raw34-case14 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | 🔴 / — / — | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 |
| psse-raw35-case14 | 🔴 / 🔴 / ◐ | — / 🔴 / — | 🔴 / 🔴 / ◐ | 🔴 / 🔴 / — | 🔴 / — / — | 🔴 / 🔴 / ◐ | — / 🔴 / — | 🔴 / 🔴 / ◐ | 🔴 / 🔴 / ◐ | 🔴 / 🔴 / 🔴 | — / 🔴 / — | — / 🔴 / — | — / 🔴 / — | — / 🔴 / — | — / 🔴 / — | — / 🔴 / — | — / 🔴 / — | 🔴 / 🔴 / — | 🔴 / 🔴 / ◐ | 🔴 / 🔴 / ◐ | — / 🔴 / — | — / 🔴 / — | 🔴 / 🔴 / ◐ | — / 🔴 / 🔴 |
| psse-rawx-case14 | 🔴 / ◐ / 🔴 | — / ◐ / — | ◐ / ◐ / ◐ | 🔴 / ◐ / — | ◐ / ◐ / ◐ | ◐ / — / — | — / ◐ / — | 🔴 / ◐ / 🔴 | 🔴 / ◐ / 🔴 | 🔴 / ◐ / 🔴 | — / ◐ / — | — / ◐ / — | — / ◐ / — | — / ◐ / — | — / ◐ / — | — / ◐ / — | — / 🔴 / — | 🔴 / ◐ / — | 🔴 / ◐ / 🔴 | 🔴 / ◐ / 🔴 | — / — / — | — / 🔴 / — | 🔴 / 🔴 / ◐ | — / 🔴 / 🔴 |
| powermodels-case30 | 🟢 / 🟢 / 🟢 | 🟢 / — / — | ◐ / 🟢 / ◐ | 🔴 / 🟢 / — | ◐ / 🟢 / ◐ | — / 🟢 / — | — / 🟢 / — | 🟢 / 🟢 / 🟢 | 🟢 / 🟢 / 🟢 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | — / 🟢 / — | — / 🟢 / — | — / 🟢 / — | — / 🟢 / — | — / 🟢 / — | — / 🟢 / — | 🔴 / 🟢 / — | — / 🟢 / — | — / 🟢 / — | — / 🟢 / — | — / 🔴 / — | 🟢 / 🟢 / 🟢 | — / 🔴 / 🔴 |
| egret-case14 | 🟢 / 🟢 / 🟢 | 🟢 / 🟢 / 🟢 | ◐ / 🟢 / ◐ | 🔴 / 🟢 / — | ◐ / 🟢 / ◐ | — / 🟢 / — | — / 🟢 / — | 🟢 / — / — | 🟢 / 🟢 / 🟢 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | — / 🟢 / — | — / 🟢 / — | — / 🟢 / — | — / 🟢 / — | — / 🟢 / — | — / 🟢 / — | 🔴 / 🟢 / — | — / 🟢 / — | — / 🟢 / — | — / 🟢 / — | — / 🔴 / — | 🟢 / 🟢 / 🟢 | — / 🔴 / 🔴 |
| pandapower-example | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | — / — / — | 🔴 / — / — | — / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | — / — / — | 🔴 / — / — | 🔴 / 🔴 / 🔴 | 🔴 / — / — | 🔴 / 🔴 / 🔴 | 🔴 / — / — | 🔴 / — / — | — / 🔴 / 🔴 |
| pypsa-example | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / — | 🔴 / 🔴 / — | 🔴 / 🔴 / — | 🔴 / 🔴 / — | 🔴 / 🔴 / — | — / 🔴 / — | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | — / 🔴 / — | 🔴 / 🔴 / — | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | — / 🔴 / — | — / — / ◐ | 🔴 / — / — | — / 🔴 / 🔴 |
| surge-case14 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / — | 🔴 / 🟢 / 🔴 | — / 🟢 / — | — / 🟢 / — | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🟢 / — / — | 🔴 / 🟢 / 🔴 | — / 🟢 / — | — / 🟢 / — | — / 🟢 / — | — / 🟢 / — | — / 🟢 / — | — / 🔴 / — | 🔴 / 🟢 / — | — / 🟢 / — | — / 🟢 / — | 🔴 / 🟢 / — | — / 🔴 / — | 🔴 / 🔴 / 🔴 | — / 🔴 / 🔴 |
| powerworld-activsg200 | — / — / — | — / — / — | — / — / — | 🔴 / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | 🔴 / — / — | — / — / — | — / — / — | — / — / — | 🔴 / — / — | — / — / — | — / 🔴 / 🔴 |
| pslf-activsg200 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | 🔴 / — / — | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 |
| xiidm-eurostag | 🔴 / 🔴 / ◐ | — / 🔴 / — | 🔴 / 🔴 / ◐ | 🔴 / 🔴 / — | 🔴 / 🔴 / ◐ | — / 🔴 / ◐ | — / 🔴 / — | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / ◐ | 🔴 / 🔴 / ◐ | — / 🔴 / — | — / 🔴 / — | — / 🔴 / — | — / 🔴 / — | — / 🔴 / — | — / 🔴 / — | — / 🔴 / — | 🔴 / 🔴 / — | 🔴 / — / — | 🔴 / 🔴 / ◐ | ◐ / 🔴 / 🔴 | — / — / — | 🔴 / 🔴 / ◐ | — / 🔴 / 🔴 |
| jiidm-eurostag | 🔴 / 🔴 / ◐ | — / 🔴 / — | 🔴 / 🔴 / ◐ | 🔴 / 🔴 / — | 🔴 / 🔴 / ◐ | — / 🔴 / ◐ | — / 🔴 / — | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / ◐ | 🔴 / 🔴 / ◐ | — / 🔴 / — | — / 🔴 / — | — / 🔴 / — | — / 🔴 / — | — / 🔴 / — | — / 🔴 / — | — / 🔴 / — | 🔴 / 🔴 / — | 🔴 / 🔴 / ◐ | 🔴 / — / — | ◐ / 🔴 / 🔴 | — / — / — | 🔴 / 🔴 / ◐ | — / 🔴 / 🔴 |
| cgmes-ieee14 | 🔴 / — / — | — / ◐ / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | — / — / — | — / — / — | 🔴 / ◐ / 🔴 | 🔴 / — / — | 🔴 / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | 🔴 / — / — | 🔴 / ◐ / 🔴 | 🔴 / ◐ / 🔴 | ◐ / — / — | 🔴 / 🔴 / 🔴 | 🔴 / — / — | — / 🔴 / 🔴 |
| ucte-sn3-fr2 | 🔴 / 🔴 / 🔴 | — / ◐ / — | ◐ / ◐ / ◐ | 🔴 / ◐ / — | ◐ / ◐ / ◐ | — / ◐ / — | — / 🔴 / — | 🔴 / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | 🔴 / ◐ / 🔴 | 🔴 / ◐ / — | — / ◐ / — | — / ◐ / — | — / ◐ / — | — / ◐ / — | — / ◐ / — | — / 🔴 / — | 🔴 / 🔴 / — | 🔴 / ◐ / — | 🔴 / ◐ / — | — / ◐ / — | ◐ / — / — | 🔴 / ◐ / 🔴 | — / 🔴 / 🔴 |
| powersystems-case9 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / — | 🔴 / 🟢 / 🔴 | — / 🟢 / — | — / 🟢 / — | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | 🔴 / — / — | — / 🟢 / — | — / 🟢 / — | — / 🟢 / — | — / 🟢 / — | — / 🟢 / — | — / 🟢 / — | 🔴 / 🟢 / — | 🔴 / 🟢 / 🔴 | 🔴 / 🟢 / 🔴 | — / 🔴 / — | — / 🔴 / — | 🔴 / 🔴 / 🔴 | — / 🔴 / 🔴 |
| sienna-example | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | — / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — |
| sienna-0-1-reference | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — |
| sienna-0-2-example | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | — / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — |
| sienna-0-3-reference | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — |
| sienna-0-4-reference | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — |
| ieee-cdf-case14 | — / — / — | — / — / — | — / — / — | 🔴 / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | — / — / — | — / — / — | — / — / — | — / — / — |
| powerworld-binary-case14 | 🔴 / — / — | — / — / — | — / — / — | 🔴 / — / — | — / — / — | — / — / — | — / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | 🔴 / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | 🔴 / — / — |
| go-challenge-3 | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | — / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — |
| deepmind-opfdata | — / — / — | — / — / — | — / — / — | 🔴 / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | 🔴 / — / — | — / — / — | — / — / — | — / — / — | 🔴 / — / — | — / — / — | — / — / — |
| sincal-balanced | 🔴 / — / — | — / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | — / — / — | — / — / — | — / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | — / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | — / — / — |
| gridfm-case24 | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — |

🟢 All comparisons passed. ◐ Only some quantities could be compared.
🔴 A measured difference or a failed conversion or solve. — See the JSON for the reason.

### Distribution

| Input | dss | pmd-json | pmd-math-json | bmopf-json | ravens-json | cim-dist | gridlabd | gridlabd-arras |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| opendss-ieee13 | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | — / 🔴 / — | — / — / — | — / — / — | 🔴 / — / — | 🔴 / — / — |
| mg-ravens-case3 | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | — / — / — | 🔴 / — / — | 🔴 / — / — |
| pmd-case3 | — / 🔴 / 🔴 | ◐ / — / — | — / ◐ / — | — / 🔴 / 🔴 | — / ◐ / — | — / 🔴 / 🔴 | 🔴 / — / — | 🔴 / — / — |
| pmd-math-case3 | — / — / — | — / ◐ / — | ◐ / — / — | — / — / — | — / — / — | — / — / — | 🔴 / — / — | 🔴 / — / — |
| bmopf-ieee13 | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | — / — / — | — / — / — | — / — / — | 🔴 / — / — | 🔴 / — / — |
| cim-distribution-tank | — / — / — | 🔴 / — / — | 🔴 / — / — | — / — / — | — / — / — | — / — / — | 🔴 / — / — | 🔴 / — / — |
| gridlabd-ieee4 | — / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / — / — | — / 🔴 / 🔴 |
| gridlabd-arras-ieee4 | — / 🔴 / 🔴 | 🔴 / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / 🔴 / 🔴 | — / — / — |
| pandapower-three-phase | — / — / — | 🔴 / — / — | — / — / — | — / — / — | — / — / — | — / — / — | 🔴 / — / — | 🔴 / — / — |
| sincal-distribution | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — | 🔴 / — / — |

🟢 All comparisons passed. ◐ Only some quantities could be compared.
🔴 A measured difference or a failed conversion or solve. — See the JSON for the reason.
