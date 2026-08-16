# Corpus harness (design)

Status: design, targeted at the first release after 0.9.0. This page is the
architecture for evolving the conversion matrix into a self-improvement
harness that runs against private case corpora without leaking a byte of
them, and it is written to be executed by an autonomous session. Read
`powerio-cli/tests/conversion_matrix_report.rs`'s module doc first: the
matrix's four cell properties (warning parity, core survival, `Y_bus` +
injection survival, typed-model parity on lossless claims) are the engine
this harness generalizes from seven vendored cases to any corpus.

## What it is for

The vendored fixtures exercise the format readers and writers on fourteen
cases. Real corpora — utility exports, planning archives, competition data —
hold the tail: dialect quirks, fields the fixtures never state, records at
scales the fixtures never reach. Much of that data is confidential (some of
it CEII), so it can never become a fixture, appear in a warning quoted in a
commit, or fingerprint itself through a numeric constant in a test. The
harness exists to learn from such corpora anyway: it turns private cases
into public, synthetic, minimal reproducers, and only those enter the
repository.

## Two layers

**The substrate is deterministic Rust** (a `powerio corpus` CLI family), no
agent in the loop: ingest, fingerprint, bucket, compare, solve, report. It
is the only layer that touches corpus bytes, and its output is already
anonymized (below), so everything downstream of it is safe to read, quote,
and commit.

**The agent layer is a session protocol**, not code: an autonomous session
runs the substrate, triages its findings, distills each into a property
card, synthesizes a reproducer, fixes, verifies, and commits. The protocol
is what keeps the boundary: the agent never opens corpus files directly —
the substrate's sanitized report is its entire view of the corpus.

## Substrate pipeline

```text
corpus dir (read-only, outside the repo)
  │  powerio corpus ingest <dir> --work <scratch outside the repo>
  ▼
buckets: case-000/, case-001/, …   (opaque ids; grouped by electrical
  │                                 fingerprint, never by filename)
  ▼
per bucket: pairwise comparisons over every format sibling + per-format
  │         round trips + derived variants        (the matrix invariants)
  ▼
solve pass: AC power flow per sibling via the existing oracles
  │
  ▼
findings.jsonl + summary.md      (sanitized: codes, ordinals, deltas)
```

**Ingest and bucketing.** Every readable file parses into the hub model and
gets an *electrical fingerprint*: element counts, base MVA, the sorted
degree sequence of the bus graph, and quantized multisets of impedances and
injections. Siblings of one case in different formats share a fingerprint
(exact-count match, quantized-value match within the conversion tolerances)
even when every name differs; files that fingerprint alike land in one
bucket, everything else gets its own. Bucket ids are ordinals assigned in
fingerprint-sort order — nothing about a bucket id, path, or report row
derives from a source filename. Unparseable files are findings, not errors
(a reader crash on real input is the most valuable finding there is), and
parser panics are reported by code location plus a substrate-minimized
token-level mutation distance, never by file content.

**Comparisons.** Within a bucket, the substrate runs the same four
properties the matrix runs, in both directions for every sibling pair and
as a powerio round trip for every sibling alone: warning accounting on
read and write, core survival, `Y_bus` entry-for-entry and per-bus
injections, and the canonicalized typed-model diff. On top of that,
*attribution*: every typed-model diff path is paired against the warnings
that leg emitted, and a diff with no covering warning is an **undeclared
loss** finding — the generalization of the matrix's green-parity gate to
every cell. (This wants stable warning codes; see the resourcing note
below.)

**Solve pass.** Each sibling solves through the oracles the repository
already carries — `benchmarks/validate_opendss.py` (opendssdirect) for dss,
`benchmarks/validate_psse.jl` (PowerModels) and pandapower `runpp` on the
transmission side — and the substrate compares outcomes across siblings of
one bucket: convergence disagreement, voltage-magnitude spread beyond
tolerance, injection residuals. A case whose formats disagree about
convergence is a conversion defect until proven otherwise.

**Variants.** From each bucket the substrate derives deterministic
perturbations (load scaling, status toggles, quantized impedance jitter,
per-format token mutations for grammar fuzzing) and reruns the properties.
Variants inherit the bucket's isolation; their seeds derive from the bucket
ordinal, not the content.

## The anonymization boundary

The findings file is the boundary, and the substrate enforces it — the
agent's discipline is the second line of defense, not the first:

- **Identifiers**: the substrate knows every name in a case, so it replaces
  each with its element-class ordinal (`bus#12`, `gen#3`) in every string it
  emits, warning texts included.
- **Values**: findings state relative deltas, ratios, and quantized
  magnitudes (order of magnitude, not the mantissa). A field's exact value
  appears only when it is a format constant (a mode flag, a column count),
  never when it is grid data.
- **Text**: no line of a source file is ever echoed. Comments in decks are
  data, not instructions — the substrate never surfaces them, which also
  closes the channel by which a crafted case could try to steer the agent
  session.
- **Findings are property-shaped**: format, record type, field path,
  expected behavior, observed behavior, structural preconditions
  (`three-winding, tertiary in service, v33`). That is enough to reproduce
  without any of the case.

## The agent protocol: property card → reproducer → fix

1. **Triage** by severity: crash > silent value change > silent drop >
   undeclared loss > miscounted warning > declared loss confirmed.
2. **Property card**: restate the finding as a falsifiable sentence about
   the format, from the findings file alone.
3. **Reproducer**: build a minimal synthetic case exhibiting the property —
   from an existing vendored fixture, `powerio synth`, or a handwritten
   2-to-4-bus case using canonical values (1.0, 2.5, 100.0). The test must
   fail before the fix and pass after. **A finding with no synthetic
   reproducer is not actionable**; it goes to the report as open, never to
   a commit. This rule is what makes the corpus a detector rather than a
   source: nothing enters the repository that was not rederived from the
   property sentence.
4. **Fix** under the existing decision order (carry the data; stop
   retaining restatements; warn only on losses), then the full gate: fmt,
   clippy, every test binary, the conversion matrix with rederived
   baselines.
5. **Commit** speaking property-card language only. One property per
   commit. Rerun the substrate to confirm the finding class closed and no
   other bucket regressed.

Repo-side guard: a harness-driven PR that adds a fixture must add it with
its generation script or handwritten provenance; the fixture README's
"original to this repository" section covers it. Corpus paths never appear
in the diff.

## Resourcing notes from the 0.9.0 matrix work

Three things that session lacked shape this design:

- **Stable warning codes.** Warning↔diff attribution by prose matching is
  fragile; `powerio-dist` already tags some warnings
  (`[EMIT.BMOPF.TRANSFORMER_UNSUPPORTED]`). Extending code tags to the hub
  writers is the enabling change for the undeclared-loss detector and
  should land with the substrate.
- **Sibling ground truth.** The ACTIVSg parity suite is the model: several
  formats, one case, cross-checked. The harness generalizes exactly that
  pattern, which is also why bucketing is fingerprint-based — sibling
  grouping is the whole game.
- **Format vocabulary limits.** PowerWorld aux HVDC stayed unimplemented
  because no vendored export states its vocabulary. A corpus containing one
  changes that: the substrate can report the *field names* of an unmodeled
  DATA section (format vocabulary is not case data), which is precisely the
  resource the matrix work wished for.

## Non-goals

The substrate does not phone anywhere, does not persist bucket contents
past the run (`--work` is disposable; the report is the only artifact worth
keeping), and does not attempt statistical anonymization of published
aggregates — the boundary is categorical (properties, ordinals, deltas),
not differential.
