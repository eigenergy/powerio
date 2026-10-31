# MG-RAVENS and PMD MATHEMATICAL review handoff

This records the review of [Rust #746](https://github.com/eigenergy/powerio/pull/746)
and its [Julia companion #197](https://github.com/eigenergy/PowerIO.jl/pull/197).
The format and fidelity chapters describe the supported behavior.

## What this PR adds and where it belongs

MG-RAVENS can describe a balanced network or list individual phase and neutral
conductors. PowerIO reads the data produced by the upstream MATPOWER converter
into `BalancedNetwork`; it reads the data produced by the upstream OpenDSS
converter into `MulticonductorNetwork`. Both use the format name
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
