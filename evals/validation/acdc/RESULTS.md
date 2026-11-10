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

## Coupled OPF types and preparation (separate model branch)

Branch `codex/819-acdc-models` starts at reader commit `49e8eacb`; it does not
change the reader PR. It adds explicit coupled intent, identity-keyed physical
points/results, IR 2 structural types, and numerical preparation. Tellegen's
case5 native branch consumes these APIs and retains its independent Julia
checks. The existing case5 source fixture is reused; no new fixture is vendored.

Validation on 2026-11-09:

- Default workspace `cargo test`: 3,629 passed, 0 failed, 41 ignored. The final
  focused matrix-enabled run adds/executes five coupled tests and all eleven
  reader tests, all passing. `cargo test -p powerio-capi`: 55 passed, 1 ignored.
- Frozen 0.11.0 and 0.11.9 wheels reject coupled instance/result structural types
  with and without the capability marker; raw/nested reader checks still pass.
- Changed-library and new-test clippy, formatting, schema/type parity,
  diagnostic parity, terminology, mdBook build/test and warnings-denied
  all-feature workspace rustdoc pass. Only the unreleased schema is regenerated.
- The full CI clippy matrix was attempted: C ABI no-default/release/dist and
  Python pass. Workspace, matrix-gridfm and no-sincal fail on existing exact
  float comparisons in unchanged tests (including PSSE, PMD math and
  `synth_facade_parity`); this is not a claim of green full CI. No lint gate or
  unrelated test was weakened.

The result constructor checks data completeness and finiteness. Solver feasibility
and local convergence remain the producer's responsibility. Reference compatibility
is not a qualification of physical rebasing, directional loss models, native PF,
LCC, wider-case native solves, or binding calculation parity.

## Numerical angle-reference policy — 2026-11-10

The coupled model branch adds an explicit `AcDcAngleReferencePolicy` on intent,
with `Declared` preserving the previous default/refusal and
`LowestBusIdPerIsland` adding zero-angle numerical anchors only on uncovered
active AC islands. Preparation records all declared/added anchors and component
source bus IDs; source bus roles, PF dispatch, and ordinary AC-only guards stay
unchanged. Row reordering and DC connectivity do not change anchor selection.
The policy survives typed instance/result IR; the default field is omitted for
old-shape compatibility and unknown policies refuse. Only the unreleased schema
changed. The IR reference tables now also document both coupled structural types.

Validation: the complete default workspace test run had one failure in the
previously missing coupled IR-reference field tables; that documentation was
fixed and its test rerun successfully. All other workspace tests passed. The
seven focused coupled tests (including matrix preparation) pass, as do changed
library and focused-test clippy checks. The full seven-way CI clippy matrix was
run: C ABI variants and Python pass; workspace/no-SINCAL/matrix-gridfm still
hit the previously recorded unchanged exact-float tests in `psse.rs` and
`synth_facade_parity.rs`. These baseline warnings are not relaxed here.

Case67 native validation and independent phase alignment are performed on the
existing Tellegen native-validation branch. Reader PR #822 and its commit
49e8eacb are unchanged; this extends only the separate model branch.

## Declared physical-current bound — 2026-11-10

`DeclaredCurrentAcpVscV1` is a separate required profile token. It keeps the
existing ACP VSC equations, inverter loss coefficient and auxiliary bounds,
but honours the declared AC RMS valve-current limit on
`S/(sqrt(3)*V_converter)` kA. The compatibility profile and its nominal-1-pu
current repair are unchanged. `AcDcCurrentLimitPolicy` is derived from the
profile and recorded with each station's declared/effective values; there is
no policy override underneath the compatibility token.

Zero, undersized and generous declared limits remain exact. Nine coupled tests
cover preparation, source preservation, explicit policy selection, instance and
nested-solution IR round trips, unknown-profile/override refusal and legacy
guards. Only the unreleased schema gains the new enum alternative; published
schemas and existing compatibility document layouts remain unchanged. A frozen
552fc413 profile decoder was also run: it accepts the compatibility token and
rejects `declared_current_acp_vsc_v1` as an unknown variant.

Validation: 472 problem/matrix tests plus 21 focused facade, schema and IR-reference
tests pass. Changed libraries and coupled-test Clippy checks pass. All seven
CI Clippy configurations were attempted: the four C ABI/Python configurations
pass; workspace, matrix-gridfm and no-SINCAL encounter the previously documented
unchanged float-comparison warnings in psse.rs, synth_facade_parity.rs and
pmd_math.rs. No warnings were suppressed to make those configurations pass.

Analytical feasible/binding/violating base-conversion cases and explicitly
modified Julia checks are maintained on Tellegen's `codex/acdc-native-case5`
branch. The reference modification restores only source Imax after preprocessing;
it is not a claim of unmodified PowerModelsACDC equivalence. Generic rebasing,
unequal-base bypasses, physical pole/return topology and native coupled PF remain
separate gates. Reader PR #822 remains unchanged and draft.

## Physical local-coordinate contract — 2026-11-10

`PhysicalCoordinatesAcpVscV1` is a separate required profile, deriving declared
current limits and `PhysicalLocalBases`. Preparation records PCC/filter/valve
and converter-side impedance bases, physical transformer turns, the from-node
DC resistance reference side and endpoint voltage ratios. Missing active AC
voltage bases and nonrepresentable ratios refuse. Existing source-coordinate
profiles retain their equations and policies; no generic rebase is enabled.

Twelve coupled tests include profile/point/result IR persistence, source
preservation, explicit bases, ignored bypass tap, invalid-base refusal and
unknown override/profile rejection. The frozen 896fef02 decoder accepts both
old profiles and rejects the new token. Only the unreleased schema changes.
All 472 problem/matrix and 24 focused facade/schema/reference tests pass.
Changed-library and coupled-test Clippy checks pass. All seven CI Clippy
configurations were run: three C ABI configurations and Python pass; the three
broad configurations retain unchanged float-comparison warnings in psse.rs,
pmd_math.rs and synth_facade_parity.rs. No warning is relaxed here.

Weighted bypass/DC equations and independent SI-equation qualification belong
to Tellegen's existing `codex/acdc-native-case5` branch. The new profile retains
LossCinv in both directions and reference auxiliary bounds. General physical
rebasing, physical pole/return topology, native coupled PF and directional loss
selection remain separate gates. Reader PR #822 is unchanged and draft.

### Fixed valve-loss modes (2026-11-10)

The separately selected `FixedModePhysicalAcpVscV1` stores a complete,
identity-keyed mode list. Rectifier selects Crec and valve AC withdrawal P >= 0;
inverter selects Cinv and P <= 0. At P = 0 the declared branch remains selected.
Existing profiles retain their curves, limits, tokens and omitted-field IR shape.
Only the unreleased IR schema changed; published snapshots are byte-identical.

Validation: 14 coupled model/IR tests, 11 MATPOWER-extension tests, 146 matrix
library tests and 46 problem library tests pass. Changed library Clippy with
`schema,matrix` passes. All seven CI Clippy targets were run: all C ABI variants
and Python pass; workspace/no-SINCAL remain blocked by existing `float_cmp`
findings in PSS/E/PMD tests, and matrix-gridfm by `synth_facade_parity.rs`.
No suppressions were added to these unrelated tests.

The exact profile enum extracted from frozen `ba1f9fc9` was compiled with its
Serde snake-case decoder: all three existing tokens decode, and the new token
is rejected. Extracted enum SHA256:
`d20d020207594ecf56c33f8ac493539e19d952357a04866f0cf84b90a038499a`.
This is a profile-token refusal check; current full instance and solution IR
round trips also test missing/unknown/null modes and downgrade refusal.
Numerical branch qualification belongs to the Tellegen validation harness.

### Coupled PF intent and control gate (2026-11-10)

`AcDcPfInstance` / `AcDcPfSolution` add an explicit required PF profile for the
pinned source-pu/Cinv equations and reference controls. Equal increments from
declared generator outputs make per-generator allocation explicit; controls
require one AC reference per AC island and one DC voltage controller per DC
island. Duplicate AC voltage authority and unsupported droop refuse. PF has no
objective or implicit dispatch/rating constraints.

The full frozen `27aae3b3` PowerIO decoder was compiled and run on current
case5 IR: the existing OPF instance is accepted; both new PF instance and PF
solution are rejected as unknown structural variants. Current full IR round trips,
required capability, source preservation, point shape, unknown/missing profile
and unsupported control combinations pass. Only the unreleased schema changed.

Validation: 16 coupled model tests, 11 reader tests, 146 matrix library tests and
46 problem library tests pass (219 total). Changed-library Clippy passes. All
seven CI Clippy targets were attempted: C ABI variants and Python pass; existing
PSS/E/PMD/synthetic-parity float comparisons still block the other targets.
Numerical PF qualification is recorded in Tellegen's independent reference gate.
