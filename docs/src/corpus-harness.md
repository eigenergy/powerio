# Corpus harness

Status: the tool ships in 0.9.0 as `powerio corpus`. This page is the
architecture for running the conversion matrix's properties against private
case corpora without leaking a byte of them. Read
`powerio-cli/tests/conversion_matrix_report.rs`'s module doc first: the
matrix's four cell properties (warning parity, core survival, `Y_bus` +
injection survival, typed-model parity on lossless claims) are the engine
this harness generalizes from seven vendored cases to any corpus. Both run
the same code, `powerio-cli`'s `invariants` module, so the CI gate and the
harness cannot drift apart.

```text
powerio corpus ingest <corpus-dir> --work <scratch-dir> [--max-bytes N]
powerio corpus compare --work <scratch-dir>
powerio corpus walk --work <scratch-dir> [--walks N] [--hops N] [--seed N] [--settle N]
powerio corpus report --work <scratch-dir> -o findings.jsonl --summary summary.md
```

The corpus directory is only read. The work directory holds raw values and is
disposable, so it stays on the machine that owns the corpus. `findings.jsonl`
is the boundary, and `report` audits its own output against every string the
corpus taught it before writing a byte. `compare` and `walk` are independent
analyses over one ingest; `report` takes whichever ran.

## Walks

`compare` runs every leg from the pristine case, which is the right shape for
a gate and blind to what only a chain can state. `walk` converts each bucket's
case through random cycles of formats and grades three chain properties: the
route must not change the destination (converting A→B→C must land where A→C
does), conversion must settle (a second pass through a format is a no-op), and
an emptied table must stay empty (rows reappearing means a writer stated data
no reader gave it). A hop's own loss is graded by `compare` already, so walk
findings carry only the chain properties, each with the format path and the
seed that replays it.

The run learns as it goes. A ledger in the work directory records, per
directed format pair, every distinct signature that edge has produced —
warning shapes, changed model path classes, chain findings — and the next path
is drawn toward the pairs that have taught the least: an edge's weight is its
novelty rate, so an unwalked edge scores 1 and a mined-out one decays toward
zero without ever being excluded. The run ends when `--settle` consecutive
walks teach the ledger nothing. The ledger persists across runs; delete it to
start over.

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

**The tool is deterministic Rust**: ingest, fingerprint, bucket, compare,
report. It is the only layer that reads corpus bytes, and its output is
already anonymized (below), so everything downstream of it is safe to read,
quote, and commit.

**The workflow around it is a convention**, not code. A finding is triaged,
restated as a property, reproduced synthetically, fixed, and verified. The
convention is what keeps the boundary: whoever acts on a finding works from
the report rather than from the cases, so nothing enters the repository that
was not rederived from the property.

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
parser panics are reported by code location plus a tool-minimized
token-level mutation distance, never by file content.

**Comparisons.** Within a bucket, the tool runs the same four
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
already carries — `evals/validation/validate_opendss.py` (opendssdirect) for dss,
`evals/validation/validate_psse.jl` (PowerModels) and pandapower `runpp` on the
transmission side — and the tool compares outcomes across siblings of
one bucket: convergence disagreement, voltage-magnitude spread beyond
tolerance, injection residuals. A case whose formats disagree about
convergence is a conversion defect until proven otherwise.

**Variants.** From each bucket the tool derives deterministic
perturbations (load scaling, status toggles, quantized impedance jitter,
per-format token mutations for grammar fuzzing) and reruns the properties.
Variants inherit the bucket's isolation; their seeds derive from the bucket
ordinal, not the content.

## The anonymization boundary

The findings file is the boundary, and the tool enforces it rather than
relying on the care of whoever reads the report:

- **Identifiers**: the tool knows every name in a case, so it replaces
  each with its element-class ordinal (`bus#12`, `gen#3`) in every string it
  emits, warning texts included.
- **Values**: findings state relative deltas, ratios, and quantized
  magnitudes (order of magnitude, not the mantissa). A field's exact value
  appears only when it is a format constant (a mode flag, a column count),
  never when it is grid data.
- **Text**: no line of a source file is ever echoed. Comments in decks are
  data rather than instructions, and are never surfaced.
- **Findings are property-shaped**: format, record type, field path,
  expected behavior, observed behavior, structural preconditions
  (`three-winding, tertiary in service, v33`). That is enough to reproduce
  without any of the case.

## From a finding to a fix

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
5. **Commit** one property per commit, stating the property rather than the
   run that surfaced it. Re-run the tool to confirm the finding class closed
   and no other bucket regressed.

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
  should land with the tool.
- **Sibling ground truth.** The ACTIVSg parity suite is the model: several
  formats, one case, cross-checked. The harness generalizes exactly that
  pattern, which is also why bucketing is fingerprint-based — sibling
  grouping is the whole game.
- **Format vocabulary limits.** PowerWorld aux HVDC stayed unimplemented
  because no vendored export states its vocabulary. A corpus containing one
  changes that: the tool can report the *field names* of an unmodeled
  DATA section (format vocabulary is not case data), which is precisely the
  resource the matrix work wished for.

## Non-goals

The tool does not phone anywhere, does not persist bucket contents
past the run (`--work` is disposable; the report is the only artifact worth
keeping), and does not attempt statistical anonymization of published
aggregates — the boundary is categorical (properties, ordinals, deltas),
not differential.
