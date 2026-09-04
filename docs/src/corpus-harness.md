# Corpus harness

`powerio corpus` runs the conversion matrix's properties against a private
case corpus without letting a byte of it into the repository. The vendored
fixtures cover the readers and writers on a handful of public cases; real
corpora hold the tail: dialect quirks, fields no fixture states, records at
scales no fixture reaches. Much of that data is confidential, so it can never
become a fixture, appear in a warning quoted in a commit, or fingerprint
itself through a numeric constant in a test.

```text
powerio corpus ingest <corpus-dir> --work <scratch-dir> [--max-bytes N]
powerio corpus compare --work <scratch-dir>
powerio corpus walk --work <scratch-dir> [--walks N] [--hops N] [--seed N] [--settle N]
powerio corpus report --work <scratch-dir> -o findings.jsonl --summary summary.md
```

The corpus directory is only read. The work directory holds raw values and is
disposable, so it stays on the machine that owns the corpus. `findings.jsonl`
is the boundary: `report` audits its own output against every string the
corpus taught it before writing a byte. The tool and the conversion matrix
test in `powerio-cli/tests/conversion_matrix_report.rs` run the same code,
the `invariants` module of `powerio-cli`, so the CI gate and the harness
cannot drift apart.

## Ingest and bucketing

Every readable file parses into the typed model and gets an electrical
fingerprint: element counts, base MVA, the sorted degree sequence of the bus
graph, and quantized multisets of impedances and injections. Siblings of one
case in different formats share a fingerprint even when every name differs,
so they land in one bucket; every other file gets its own. Bucket identifiers
are ordinals assigned in fingerprint order. Nothing about a bucket
identifier, path, or report row derives from a source file name. An
unparseable file is a finding rather than an error, and a parser panic is
reported by code location plus a tool minimized token level mutation
distance, never by file content.

## Compare

`compare` runs every leg from the pristine case: the four matrix properties
in both directions for every sibling pair and as a PowerIO round trip for
every sibling alone. The properties are warning accounting on read and write,
core survival, `Y_bus` entry for entry and per bus injections, and the
canonicalized typed model diff. Every typed model diff path is paired against
the warnings that leg emitted, and a diff with no covering warning is an
undeclared loss finding.

## Walk

`walk` converts each bucket's case through random cycles of formats and
grades three chain properties: the route must not change the destination
(converting A to B to C must land where A to C does), conversion must settle
(a second pass through a format is a no-op), and an emptied table must stay
empty (rows reappearing means a writer stated data no reader gave it). A hop's
own loss is graded by `compare` already, so walk findings carry only the
chain properties, each with the format path and the seed that replays it.

The run learns as it goes. A ledger in the work directory records, per
directed format pair, every distinct signature that edge has produced, and
the next path is drawn toward the pairs that have taught the least. The run
ends when `--settle` consecutive walks teach the ledger nothing. The ledger
persists across runs; delete it to start over.

## The anonymization boundary

The tool enforces the boundary rather than relying on the care of whoever
reads the report:

- Identifiers: every name in a case is replaced with its element class
  ordinal (`bus#12`, `gen#3`) in every emitted string, warning texts included.
- Values: findings state relative deltas, ratios, and quantized magnitudes.
  A field's exact value appears only when it is a format constant such as a
  mode flag or a column count, never when it is grid data.
- Text: no line of a source file is echoed. Comments in decks are never
  surfaced.
- Findings are property shaped: format, record type, field path, expected
  behavior, observed behavior, and structural preconditions. That is enough
  to reproduce without any of the case.

## From a finding to a fix

1. Triage by severity: crash, silent value change, silent drop, undeclared
   loss, miscounted warning, declared loss confirmed.
2. Restate the finding as a falsifiable sentence about the format, from the
   findings file alone.
3. Build a minimal synthetic reproducer from an existing vendored fixture,
   from `powerio gen`, or by hand as a case of two to four buses with
   canonical values. The test must fail before the fix and pass after. A
   finding with no synthetic reproducer is not actionable; it goes to the
   report as open, never to a commit.
4. Fix under the existing decision order: carry the data, stop retaining
   restatements, warn only on losses. Then run the full gate, including the
   conversion matrix with rederived baselines.
5. Commit one property per commit, stating the property rather than the run
   that surfaced it. Rerun the tool to confirm the finding class closed and no
   other bucket regressed.

A fixture added this way comes with its generation script or handwritten
provenance, recorded in the fixture README. Corpus paths never appear in a
diff.
