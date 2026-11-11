# Conversion evidence and incremental improvement

The PR matrix and its JSON evidence come from the same test cells and grading
rules. Run:

```sh
POWERIO_CONVERSION_MATRIX_JSON=target/conversion-matrix.json \
  cargo test -p powerio-cli --test conversion_matrix_report
python3 scripts/conversion-matrix-plan.py target/conversion-matrix.json \
  --output target/conversion-backlog.json
```

The Conversion Matrix workflow uploads `conversion-matrix.json`,
`conversion-backlog.json`, and the detailed findings in the
`conversion-matrix-warning-details` artifact. The main-branch Docs deployment
publishes [matrix evidence](assets/conversion-matrix.json) and the
[ranked backlog](assets/conversion-backlog.json). A PR artifact describes its
exact tested commit, which can be GitHub's synthetic merge commit; `head_commit`
and `base_commit` identify the PR inputs separately. Local evidence also states
whether the working tree was dirty. Missing provenance is unknown, not success.

The JSON evidence format is `schema: "powerio-conversion-matrix"`, `schema_version: 1`.
It includes `provenance`, `passed`, `failures`, grade `counts`, and unique `cells`.
Each cell identifies its source, target (null for source parsing), fixture
`cases`, grade, findings, model differences, baseline, and check failures.
The grades are `lossless`, `exact`, `restated`, `approximate`, `dropped`, and
`failed`. `verification.independent_consumer` is explicitly
`not_run_by_this_suite`: self round trips do not establish native tool acceptance.
These are CI artifact versions, independent of PowerIO IR document versions.

A website can display this evidence, filter by format pair and diagnostic, and
link to the exact workflow run. Imported JSON is evidence to inspect, not proof
of its origin. Match repository, tested commit, run and artifact identity before
calling it authenticated CI. Display failed or stale runs explicitly; preserve
source-only findings and distinguish native fields from retained extensions.
Do not replace a failed run with the last green run without showing both states.

## A bounded repair loop

1. Run the matrix on a known commit. Rank failing checks first, then dropped,
   approximate and restated fields by the number of affected cells. The planner's
   code groups are triage hints; one diagnostic can describe several unrelated
   fields, so group size is not a promised improvement count.
2. Pick one bounded, representable gap. Find the target format's authoritative
   semantics and add the smallest reproducer. Prefer an independent target
   consumer; otherwise identify the specification and the remaining uncertainty.
3. Implement the mapping and check native units, signs, IDs, optional values,
   and source retention. Run relevant Rust, Python, C and Julia checks, the
   matrix, and the repository's required lint gates.
4. Compare evidence with `--previous previous-matrix.json`. Review changed
   grades and findings **and** removed cells or fixtures. A smaller test set is
   not a fidelity improvement. Unsupported target features remain reported.
5. Only after checking the observed delta, update an intentional baseline with
   `POWERIO_MATRIX_BLESS=1`. Open a reviewable PR describing the reproducer,
   independent evidence, exact delta, limitations and credit. Merge requires
   review; the planner never edits code, weakens grades or accepts a baseline.
6. Once the change lands, use the new main evidence to select the next gap.

A scheduled coding agent can perform this bounded cycle with at most one open
repair PR. It must stop on ambiguous semantics, failing independent checks or a
concurrent branch update. It must not modify its own acceptance rules to obtain
more green cells. The goal is preserved electrical meaning, not a target color.
