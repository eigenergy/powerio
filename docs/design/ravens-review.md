# MG-RAVENS and PMD MATHEMATICAL review handoff

This records the review of [Rust #746](https://github.com/eigenergy/powerio/pull/746)
and its [Julia companion #197](https://github.com/eigenergy/PowerIO.jl/pull/197).
The format and fidelity chapters describe the supported behavior.

## What this PR adds and where it belongs

MG-RAVENS can describe a balanced network or list individual phase and neutral
conductors. PowerIO reads the data produced by the upstream MATPOWER converter
into `BalancedNetwork`; it reads the upstream OpenDSS converter's linecode-based
profile into `MulticonductorNetwork`. Wire/cable asset impedance remains
unsupported and diagnosed. Both use the format name
`ravens-json`. The code calls these choices `Profile::Balanced` and
`Profile::Multiconductor`; callers do not choose a separate format or flag.

| Operation | Owner |
|---|---|
| Read and write balanced RAVENS data | `powerio-tx::format::ravens` |
| Read and write RAVENS data with individual conductors, and PMD MATHEMATICAL JSON | `powerio-dist` |
| Inspect a RAVENS document and call the matching reader | `powerio-tnd::formats::ravens::parse` |
| Public `parse` and `emit`, supported-format information, and PowerIO IR serialization | `powerio` |
| Shared CIM identifiers, phase names, and detection of RAVENS record types | `powerio-core::cim` |

The readers and writers stay with the network types they use. A program using
only `powerio-tx` can use balanced RAVENS without depending on
`powerio-dist`; distribution callers can use RAVENS without `powerio-tx`.
The small selection function belongs in `powerio-tnd`, which already
depends on both crates and also contains the SINCAL adapter.

**Current limit:** the RAVENS function in `powerio-tnd` returns one of the
two network types. It does not construct an `IntegratedNetwork`. A document
containing record types that require both readers returns
`PARSE.RAVENS.MIXED_PROFILE`. Supporting a combined transmission and
distribution model in one RAVENS document remains future work.

For review, check the numerical conversion in each reader and writer, and
the documented losses below. The placement in `powerio-tnd` should not be
read as evidence that combined-network import has been implemented.

Retained source bytes belong to a parsed module. Writing that module back to
its original format can reproduce the original bytes; writing a network
recovered from PowerIO IR exercises the writer and reports data the target
format cannot represent. Tests cover both operations.

PowerIO IR stays at version 2. The schema at
`pio-ir/2/0.11.8/schema.json` adds the new source-format names, while the
published 0.11.7 schema stays unchanged. Existing public enum positions and
serialized format information remain compatible. The existing C ABI 7 is
sufficient; this PR adds no generated C entry points or Julia runtime API.

## Verified findings and regression coverage

Six independent reviews began after both companion branches had green CI.
They covered the complete original diff, then cross-checked the fixes.

| Finding | Regression evidence |
|---|---|
| A fixed-Q generator became a voltage controller | Balanced reader/writer tests and two converged native PowSybl AC solutions |
| An unequipped bus lost its voltage base | Native `Connector` metadata, unchanged electrical component counts, schema validation and IR round trip |
| Sparse polynomial terms and reactive-only ZIP terms changed meaning | Balanced cost and voltage-law tests |
| Distribution source impedance disappeared | Typed Thevenin branch, coupled sequence-to-phase matrix checks, repeated canonical writes and independent OpenDSS voltage comparison |
| Disabled equipment became active; finite neutral resistance became perfect ground | Distribution status and grounding regressions; native OpenDSS neutral-voltage comparison |
| Neutral poles, phase order, delta branch labels or load voltage bases changed | Distribution physical regression suite and native RAVENS schema gate |
| Kron-reduced transformer neutrals, angle units, switch shunts and zero-resistance leakage were recovered incorrectly | PMD reader/writer regressions and native PMD exports |
| Unbounded transformer ratings became zero capacity and erased impedance | Positive computational impedance base with explicit imputation, while the capacity remains unbounded |
| Center-tap polarity and asymmetric leakage changed during canonical conversion | Native PMD-derived winding-orientation regression and BMOPF explicit-zero leakage cases |
| Tiny malformed inputs caused panics or unbounded dimension allocation | Direct reader/writer malformed-input regressions and seeded `ravens_pmd_math` fuzz target |
| Unrelated warnings concealed electrical changes in the conversion matrix | Independent physical comparisons and a mutation test that corrupts an impedance despite unchanged metadata warnings |
| Report truncation hid field differences and fixture identity | Uncapped matrix detail output with fixture labels; bounded corpus reporting retains its existing API |
| The matrix compared an eliminated grounded return with earth as different nodes | Restored ground identity in the expected projection, with a mutation check that still rejects reversed winding orientation |
| CGMES voltage limits remained only in detailed connectivity | Apply the limits to member calculation buses and check both supported CGMES versions |
| Identifier substitutions and omitted area memberships were silent in exchange routes | Report actual old/new bus identifiers and lost areas; unchanged default IIDM area 1 does not trigger a loss finding |

## Independent oracles

`evals/validation/ravens/` pins MG-RAVENS commit
`094ad1163cb111df676884635aa03faa0538b688`. Distribution output uses its
unmodified generated schema. The upstream MATPOWER converter itself writes
two fields missing from that schema: `ConnectivityNode.SvVoltage` and
`MySettings`. The balanced check adds only those two typed shapes, runs the
native converter to prove that discrepancy, and includes rejection controls.
It does not claim that the unmodified schema accepts the balanced profile.
Every emitted test document passes through IR to prevent source-byte echo.
The archived ENWL example's precise source-semantic refusal is reported
separately from successfully validated output.

`evals/powsybl/check_ravens.py` compares direct and RAVENS-mediated routes
through IR for MATPOWER cases 9, 14, 30, 57 and 118. It checks native tables
after PSS/E 33, PSS/E 35, RAWX, XIIDM and JIIDM imports. The two documented
profile losses, generator voltage setpoints and third branch ratings, have
explicit diagnostic and recovered-value assertions. Other table fields
remain compared. The fixed-Q case additionally compares converged AC bus
voltages, angles and generator powers. The existing full PowSybl suite
continues to cover its broader PSS/E, IIDM, CGMES and UCTE corpus.

`powerio-dist/tests/pmd_math.rs` compares the writer against five independent
PMD v0.16.0 exports. The regeneration commands are in
`tests/data/dist/README.md`, with the oracle environment under
`powerio-dist/tools/pmd/`. Vendored examples retain their upstream licenses;
every added fixture is smaller than 100 KiB.

## Reproducing the gates

Run `bash scripts/ci-mirror.sh` for the repository's Linux CI configurations.
Additional focused commands and oracle setup are documented in:

- `evals/validation/ravens/README.md` — schema and converter-profile gate.
- `evals/powsybl/README.md` — native import and electrical comparisons.
- `tests/data/dist/README.md` — PMD export regeneration and fixture provenance.
- `fuzz/README.md` — the direct RAVENS/PMD parser and writer target.
- `docs/src/contributor-workflow.md` — binding and feature-matrix gates.

The conversion matrix's detailed artifact records individual fixtures,
diagnostics and typed differences. Its baseline pins observable grades and
finding codes; it is not an independent numerical oracle. Electrical checks
do not receive a blanket waiver because a conversion reports metadata loss.
Unsupported conductor projections require the affected device's own finding.
Two RAVENS transformer projections are checked against exact recovered values:
the center-tap fixture loses its additional leakage pairs, and the PMD
two-winding fixture loses nonunity winding taps. Their named diagnostics and
electrical changes remain visible; these routes are not called electrically
equivalent. The archived BMOPF IEEE 13 fixture declares `xfm2` as single phase
but gives each winding three unidentified terminals. Its transformer comparison
is explicitly unverified, scoped only to that component; other transformers,
lines and injections still undergo the physical checks.

Schema acceptance, canonical fixed points and matching finite test cases do
not prove universal electrical equivalence. Unsupported controls and fields
remain explicit fidelity findings. Input dimension limits prevent the
demonstrated allocation amplification; they are not a global process-memory
budget for arbitrarily large networks.

## Frederik Geth review follow-up, 2026-10-29

Fred's [follow-up review](https://github.com/eigenergy/powerio/pull/746#pullrequestreview-5690845743)
confirmed that neutral restoration, switched neutrals and inactive equipment
were addressed. Its two remaining reproductions failed on `8e30071` exactly
as reported: the five tests returned three passes and two failures.

- The PMD MATHEMATICAL source writer now emits the Thevenin branch when
  either resistance or reactance is nonzero. An absent component becomes a
  zero matrix; an entirely ideal source still fixes its bus voltage directly.
- The multiconductor RAVENS writer gives `DistLine.i_max` precedence over
  its linecode default. The reader keeps distinct segment limits as line
  overrides when several segments share a linecode. Unequal per-conductor
  overrides receive the existing scalar-collapse diagnostic.

`powerio-dist/tests/review_followup.rs` commits Fred's five reproductions and
four additional tests. All nine pass. The additional cases check resistance
and reactance separately, absent components, an ideal-source control, every
emitted matrix entry in physical ohms, source readback, lower/higher/zero
current overrides, the linecode fallback, shared-linecode limits, and the
nonuniform-limit diagnostic. Every reproduction bypasses retained-source echo.
No new fixtures, public APIs, IR layouts or Julia changes are needed. Fred
[approved the Julia companion](https://github.com/eigenergy/PowerIO.jl/pull/197#pullrequestreview-5690878535)
at `e5c714d126d8c5548bc944feb0c189540c9fe58e`.

Reproduce the focused review checks from the repository root:

```sh
cargo test -p powerio-dist --test review_followup -- --nocapture
cargo test -p powerio-dist
cargo test -p powerio-cli --test conversion_matrix_report
cargo fmt --all --check
bash scripts/ci-clippy.sh
```

Local validation of this follow-up passed: 896 distribution tests (18
ignored), all 14 conversion-matrix tests without a baseline change, all seven
Clippy configurations, formatting, whitespace and terminology checks.
The complete cross-platform, native-oracle and paired Julia workflows run on
GitHub for the follow-up commit; consult that commit's checks for their final
status rather than treating the previous green head as current evidence.

### Prompt for a fresh session

Continue the MG-RAVENS handoff from GitHub, not an old local checkout.
Read eigenergy/powerio#746, its current head and checks, this document, and
Frederik Geth's latest review. The two remaining requested changes have fixes
and nine passing regression tests in `powerio-dist/tests/review_followup.rs`.
The previous six independent reviews and their verified fixes are already
recorded above; do not restart that work. PowerIO.jl#197 is approved at
`e5c714d126d8c5548bc944feb0c189540c9fe58e` and needs no follow-up code.
Inspect CI for the current Rust head, fix any attributable failure, and
respond to any new review findings. Preserve all existing commits and push
any further work to `feat/ravens-json-pmd-math` with a fast-forward update.
Keep the PR ready for review and request Fred's re-review if it is not already
requested. Do not dismiss his review or merge/release without an explicit
instruction. All necessary source, regression tests and handoff context are
on GitHub; no files from the previous session are required.

## Electrical follow-up, 2026-11-04

The two boundary fixes and CIM interoperability fixes from `ba0bfca3` and
`c331ddbc` are included. The refresh retains main's per-snapshot BMOPF
validation, verbatim foreign-profile retention, and deletion of the obsolete
Julia companion pin.

The next electrical corrections cover non-Kron transformer loss-branch
terminal identities, the line-to-line/phase voltage-limit conversion, balanced
bus voltage anchors, and coupled Thevenin impedance on split sources. The new
regressions in `review_electrical.rs` and `ravens_voltage_bases.rs` failed before
the corrections. Split sources now use one shared explicit impedance branch;
malformed/asymmetric matrices are refused, and generated identities avoid
case-insensitive collisions with existing model records.

The distribution corpus always checks the physical circuit and source
constraints. Its expected transformer projections require each affected
field's own diagnostic; the archived BMOPF IEEE13 `xfm2` exception remains
limited to that ambiguous component. These checks do not waive other model
differences or settle the remaining items in Mohamed Numair's review. In
particular, wire/cable asset impedance is still outside the supported import
profile. Consult CI on the exact pushed head before approving any merge.

Local validation passed the full distribution suite, the balanced RAVENS
regressions, all seven Clippy profiles, and the 14 conversion-report tests
without a baseline change. Native PMD 0.16.0 lowered independent phase-B/C
DSS cases to the same transformer loss-branch terminal maps and R/X/G/B
matrices. The pinned upstream RAVENS schema gate checked 36 cases with no
failures; additional fresh split-source output passed the unmodified schema
after crossing PowerIO IR. Native comparisons and schema acceptance cover
these cases, rather than asserting general solver equivalence.


## Neutral, bounds and load-law follow-up, 2026-11-04

`review_round_two.rs` covers the next requested corrections. Switch neutral
elimination now uses the complex series Schur complement, reading shared
linecodes before those codes change. Ratings and conductor maps follow the
retained phases; the report includes the eliminated position and recovery
coefficients. A zero, uncoupled neutral remains an ideal return rather than
triggering a singular inverse.

Fresh RAVENS output restores earth returns for implicit wye loads, generators
and transformer windings. When a bus already has a floating neutral, implicit
earth-return devices use a private grounded node joined through an ideal
closed phase switch. The floating neutral stays separate. Corpus comparison
contracts such a switch only after checking its electrical conditions; open,
rated or impedance-bearing switches are negative controls.

PMD pairwise bounds retain terminal IDs and their original voltage scale.
Fresh MATHEMATICAL output rebases only the bound value. Constraints referring
to the neutral prevent Kron reduction rather than disappearing. ENGINEERING
and RAVENS emission report unsupported pairwise bounds. Nonuniform ZIP laws
use separate ENGINEERING phase loads with collision-safe names, preserving
reactive coefficients, delta pairs and retained cutoff voltage. The split is
reported; it changes record identity rather than the nominal phase laws.

The phase decoder accepts both `s12` and `s21`, and switch-only documents
select the multiconductor route. The reader accepts the CIM switch subclasses
listed by the adapter. Emission keeps each class accepted by the pinned
upstream schema. That schema has no `Disconnector` slot: this class emits as
`Switch` with an explicit class-loss diagnostic; it is not relabeled as
`GroundDisconnector`.

Independent checks validated 13 fresh documents against the unmodified pinned
upstream schema, including restored earth returns, mixed floating/ground
returns, and every supported switch class. Coupled switch R/X matrices and
terminal maps match native PMD 0.16.0 on a proportional R/X example; its complex
series-reduction helper also confirms the purely resistive example. PMD's
older `apply_kron_reduction!` path reduces R and X separately and produces
nonfinite X for an all-zero X matrix, so that case uses its complex helper.
These checks establish the stated examples, not universal solver equivalence.


The expanded conversion matrix also exposed two ideal-switch paths:
ENGINEERING emission supplied a tiny default resistance, and identity extras
prevented OpenDSS's exact cancelled-series equivalent. Ideal contacts now stay
ideal in both writers. Test-only contraction requires opposite finite matrices,
exclusive internal nodes, no shunts and no lost voltage constraints; changing
one resistance prevents contraction. This is an electrical-circuit comparison,
not a waiver of metadata or current-limit diagnostics.

The previous Linux conversion failure was a per-phase voltage-bound diagnostic
on an ideal source bus: platform libm rounding made its equal phase magnitudes
differ by one ULP. RAVENS now omits that loss finding only when an ideal
Earth-referenced source fixes every stated phase bound exactly. Actual unequal
or unfixed bounds still report their loss; no numeric tolerance was widened.

The existing Julia companion at `e5c714d` has two stale assertions in
`test/test_dist.jl` expecting the synthesized `_pio_line_feeder` linecode name.
The corrected reader retains the declared `lc3` identity. All other 3,836 Julia
assertions passed on the previous Rust head; those two companion expectations
need updating before this PR's binding gate can clear. No companion changes or
merge are part of this follow-up.


Two conversion baseline rows change for the IEEE13 private ground-return node:
RAVENS to OpenDSS adds the exact-switch equivalent, identity/terminal diagnostics
and two retained generated-line property records; RAVENS to ENGINEERING adds
the private bus's unsupported-name finding. All physical-circuit checks pass
before accepting those diagnostic-count changes. No new test-data files were
vendored.


Local validation of this round passed 939 distribution tests (18 ignored),
all 14 conversion-matrix tests, all seven CI Clippy configurations, facade
routing/format checks, formatting and whitespace checks. The 13 new focused
regressions pass; the first six reproductions failed before the fixes.
The native/schema checks above ran on fresh emissions without retained-source
echo. Consult GitHub checks for the pushed head; local success does not clear
the separate Julia companion expectations or authorize merging.


## Final-review corrections, 2026-11-04

The final review of `3defe1b9` was performed after all 13 workflows cleared.
The Julia companion's two linecode assertions now expect the retained `lc3`
identity at `ca7c80d`; the binding rerun checked that commit and passed all
3,838 assertions. No merge was performed.

Four remaining reproductions are addressed by this follow-up:

- A balanced RAVENS `baseMVA` that is explicitly zero, negative, null or
  nonnumeric is refused before calculating per-unit impedances. An absent
  setting keeps the documented 100 MVA default and its finding.
- Balanced equipment with missing or malformed terminals receives its own
  named `READ.RAVENS.RECORD_DROPPED` diagnostic. This covers loads, shunts,
  synchronous/asynchronous machines, sources, lines and transformer ends;
  unknown-node pointers keep the existing reference-loss diagnostic.
- Nonzero RAVENS transformer impedance requires positive finite winding
  voltage and power bases. When these are undefined, emission fails with
  `BUILD.DIST.ELECTRICAL_INCOMPLETE` naming the transformer and winding.
  It neither invents a computational rating nor silently writes an ideal
  equivalent. An already ideal unit needs no impedance base.
- PMD MATHEMATICAL delta loads on a reduced three-conductor basis append the
  unused conductor and zero-fill its other branch powers. The stated pair
  remains first, including BC, CA and reversed pairs. Each ZIP component
  follows the same projection, and bus conductor arrays grow consistently.
  Networks without a suitable basis keep their explicit conductors and report
  that Kron reduction was skipped. An already reduced pure-delta document
  retains its state when no explicit neutral needs restoring.

`powerio-tx/tests/review_final.rs` and `powerio-dist/tests/review_final.rs`
exercise fresh emission, invalid-value controls, all six pair orders, ZIP
parts, partial bus terminals, canonical fixed points and the non-Kron policy.
No new fixture files are needed.


Validation passed 942 distribution tests (18 ignored) and 1,321 transmission
tests (one ignored), including the seven new regression tests. The conversion
matrix's only baseline change is the center-tap fixture on the MATHEMATICAL
self-conversion: its two-conductor delta basis now retains the explicit model,
adding one skipped-Kron finding and removing three neutral-elimination
findings. Physical-circuit comparisons still pass.

Native PMD 0.16.0 constructed 18 models from fresh fixed output: all six pair
orders in ACR and SOC for constant-power loads, and all six in ACR for ZIP
loads. Its SOC ZIP path has an upstream `constraint_pqw` method error; a
separate native OpenDSS-to-PMD control reproduces the same error. The six ZIP
SOC cases are recorded as matching that limitation, not as successful model
constructions. These are construction checks, not power-flow solves.

All seven Clippy configurations, formatting and whitespace checks passed on
the final code. The Rust changes are split into balanced-reader validation and
distribution-emission commits on the existing PR branch. CI must clear on the
new head before any merge is considered.

## Transformer input-base correction, 2026-11-04

Final review of `e0d842d6` found the corresponding reader boundary still open:
nonzero catalog winding resistance and short-circuit leakage with zero
`ratedS` / `basePower` parsed as zero R/X. Fresh RAVENS output then removed
those fields without an impedance-loss diagnostic. The earlier writer guard
could not catch a value already made ideal by parsing.

The active-tank mapper now refuses nonzero impedance without positive finite
winding voltage and power bases, including bases that overflow or underflow
when calculated. Nonzero short-circuit and no-load loss tests also require
usable test bases. A missing short-circuit `basePower` keeps the existing
`ratedS` fallback; an explicitly invalid value is refused. Errors identify
the tank, end and affected field. Ideal equipment needs no fabricated rating,
and inactive native equipment remains retained outside the typed equations.

The new `review_final.rs` regressions cover the original reproduction,
independent resistance and test-base failures, each winding, malformed and
missing bases, valid controls and inactive equipment. No fixtures are added.
The Julia companion is refreshed against main, retaining main's compatible
shared-linecode assertions and the RAVENS tests. Neither PR is merged.

Local validation passed 944 distribution tests (18 ignored), all 14
conversion-matrix checks without a baseline change, the eight facade RAVENS
tests and all seven CI Clippy configurations on Rust 1.99.0. Formatting and
whitespace checks pass. The companion refresh is `4e67c67`; consult the checks
on the new Rust head for the paired binding result before approving a merge.

## Main refresh after #771, 2026-11-04

The refresh onto `baadf749` retains #783's frequency metadata fix and #771's
supported PMD-to-BMOPF profile activation. The changelog keeps every entry.
The BMOPF writer uses #771's per-emission profile index, with #746's retained
PMD provenance checks applied while preparing that index: clearing the source
format hint must neither reinterpret PMD hours as native BMOPF seconds nor
activate an omitted PMD `replace` default. Supported ENGINEERING profiles still
activate in schema 0.2.0; schema 0.1.0, MATHEMATICAL and unknown PMD profiles
remain source-only with diagnostics.

The regression checks cover source-format reset, explicit and omitted
`replace`, both schema versions, foreign PMD markers and a second canonical
BMOPF read/write without multiplying seconds by 3,600 again. No fixtures or
schema snapshots are added. The Julia companion remains `4e67c67`.

Local validation of the merged tree passed 960 distribution tests (18
ignored), all 14 conversion checks with no additional baseline edit, the 14
focused transmission frequency/RAVENS tests and all seven Clippy
configurations on Rust 1.99.0. Formatting and whitespace checks passed.
Consult CI on the pushed refresh, including its paired Julia binding gate,
before approving any merge.
