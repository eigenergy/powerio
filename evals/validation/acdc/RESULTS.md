# Recorded MATPOWER-extension validation

Run on 2026-11-09 with the committed validation manifest:

- Julia 1.12.6; PowerModelsACDC 0.10.1 at `9ada4082ac087aed649f48fb5f977680b0fe06c7`.
- PowerModels 0.21.7; JuMP 1.32.1; Ipopt 1.16.0 with MUMPS.
- Four PGLib-HVDC cases at `f928f95cde06d035554d92854f03950e074850e0`; source hashes are enforced by `run.py`.

Reproduction command (the external directory contains the three pinned cases):

```sh
CARGO_TARGET_DIR=/tmp/p819-target CARGO_INCREMENTAL=0 OPENBLAS_NUM_THREADS=1 python3 evals/validation/acdc/run.py /tmp/p822-validation --pglib-dir /tmp/p819-corpus
```

All four whole-case input comparisons passed before and after preprocessing.
The native DC declarations also passed their independent parser comparisons.
All 26 negative-control/check assertions passed.

| Case | Calculation | Original/fresh objective | Maximum constraint/bound violation | Maximum state difference |
|---|---|---:|---:|---:|
| case5_3_he | OPF | 15693.105053824 | 3.141e-12 | 0.0 |
| case5_3_he | PF | 0.000000000 | 9.123e-14 | 0.0 |
| case24_7_jb | OPF | 150228.149248678 | 4.003e-11 | 0.0 |
| case24_7_jb | PF | 0.000000000 | 2.729e-13 | 0.0 |
| case39_10_he | OPF | 138063.517294866 | 4.564e-10 | 0.0 |
| case67 | OPF | 122253.023887493 | 6.821e-13 | 0.0 |

Each calculation ran twice: once on the original case and once on the fresh
export after PowerIO IR restoration. All twelve solves returned `LOCALLY_SOLVED`.
Both solved points were also evaluated in the other model. The violation column
is the largest violation across both solves and both cross-feasibility checks;
it is below the `1e-7` threshold. State differences include AC/DC voltages,
generator dispatch, branch flows, converter quantities and derived losses.
The observed OPF state equality is a result, not a requirement for nonunique optima.

The whole-case comparison excludes file provenance and previous solve duals,
with every excluded path/value recorded. In particular, case67 has `mu_pmax`
annotations which fresh output does not preserve. No electrical input mismatch
was excluded to obtain these results.

Preliminary PF solves of the original case39 and case67 inputs returned
`LOCALLY_INFEASIBLE` (maximum violations about 0.469 and 0.599 respectively).
Those are local solver outcomes, not global infeasibility proofs. They are not
counted as qualified PF cases and are not made passing tests by accepting failed
termination. Both cases are qualified for OPF.

The numerical evidence qualifies the PowerModelsACDC **MATPOWER-extension** IO
path. It does not validate a native PowerIO/Tellegen coupled solver. Independent
native-equation checks, parameter variations and the later PowerModelsACDC JSON
adapter remain in [FOLLOW_UP.md](FOLLOW_UP.md).
