# Corpus harness

`powerio corpus` runs the conversion matrix's properties against a private
case corpus without letting a byte of it into the repository. The vendored
fixtures cover the readers and writers on a handful of public cases, and real
corpora hold the tail: dialect quirks, fields no fixture uses, and records at
scales no fixture reaches. Much of that data is confidential, so it cannot
become a fixture, appear in a warning quoted in a commit, or fingerprint
itself through a numeric constant in a test.

```text
powerio corpus ingest <corpus-dir> --work <scratch-dir> [--max-bytes N]
powerio corpus compare --work <scratch-dir>
powerio corpus walk --work <scratch-dir> [--walks N] [--hops N] [--seed N] [--settle N]
powerio corpus report --work <scratch-dir> -o findings.jsonl --summary summary.md
```

The corpus directory is only ever read. The work directory holds raw values
and is disposable, so keep it on the machine that owns the corpus.
`findings.jsonl` is the boundary: before `report` writes a byte, it audits its
own output against every string the corpus taught it. The tool and the
conversion matrix test in `powerio-cli/tests/conversion_matrix_report.rs`
run the same code, the `invariants` module of `powerio-cli`, so the CI gate
and the harness cannot drift apart.

## Ingest and bucketing

Every readable file is parsed into the typed model and given an electrical
fingerprint: element counts, base MVA, the sorted degree sequence of the bus
graph, and quantized multisets of impedances and injections. Siblings of one
case in different formats share a fingerprint even when all their names
differ, so they land in one bucket, and every other file gets a bucket of its
own. Bucket identifiers are ordinals assigned in fingerprint order, so nothing
about a bucket identifier, path, or report row comes from a source file name.
An unparseable file is a finding rather than an error, and a parser panic is
reported by code location plus a tool minimized token level mutation
distance, not by file content.

## Compare

`compare` runs every leg from the pristine case: the four matrix properties
in both directions for every sibling pair, and as a PowerIO round trip for
every sibling on its own. The properties are warning accounting on read and
write, core survival, `Y_bus` entry for entry and per bus injections, and the
canonicalized typed model diff. Each typed model diff path is paired against
the warnings that leg emitted, and a diff with no covering warning becomes an
undeclared loss finding.

## Walk

`walk` converts each bucket's case through random cycles of formats and
grades the chain properties: the route must not change the destination
(converting A to B to C must land where A to C does), conversion must settle
(a second pass through a format changes nothing), and an emptied table must
stay empty (rows reappearing means a writer made up data no reader gave it).
`compare` already grades each hop's own loss, so walk findings carry only the
chain properties, each with the format path and the seed that replays it.

The run learns as it goes. A ledger in the work directory keeps, per directed
format pair, every distinct signature that edge has produced, and the next
path is drawn toward the pairs that have taught the least. The run ends when
`--settle` consecutive walks teach the ledger nothing. The ledger persists
across runs; delete it to start over.

## The anonymization boundary

The tool enforces the boundary itself rather than relying on the care of
whoever reads the report:

- Identifiers: every name in a case is replaced with its element class
  ordinal (`bus#12`, `gen#3`) in every emitted string, warning texts included.
- Values: findings give relative deltas, ratios, and quantized magnitudes.
  A field's exact value appears only when it is a format constant such as a
  mode flag or a column count, and not when it is grid data.
- Text: no line of a source file is echoed, and comments in decks are not
  echoed either.
- Findings are property shaped: format, record type, field path, expected
  behavior, observed behavior, and structural preconditions. That is enough
  to reproduce the problem without any of the case.

## From a finding to a fix

1. Triage by severity: crash, silent value change, silent drop, undeclared
   loss, miscounted warning, declared loss confirmed.
2. Restate the finding as a falsifiable sentence about the format, using the
   findings file alone.
3. Build a minimal synthetic reproducer from an existing vendored fixture,
   from `powerio gen`, or by hand as a case of two to four buses with
   canonical values. The test must fail before the fix and pass after. A
   finding you cannot reproduce synthetically is not actionable; it goes into
   the report as open and does not go into a commit.
4. Fix under the existing decision order: carry the data, stop retaining
   restatements, warn only on losses. Then run the full gate, including the
   conversion matrix with rederived baselines.
5. Commit one property per commit, and describe the property rather than the
   run that found it. Rerun the tool to confirm the finding class closed and
   no other bucket regressed.

A fixture added this way comes with its generation script or handwritten
provenance, recorded in the fixture README. Corpus paths do not belong in a
diff.
