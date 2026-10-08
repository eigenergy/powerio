# PSS SINCAL delivery roadmap

Updated 2026-10-08. Reader integration and validation are complete locally.
The five branches are ready for maintainer review; nothing has been pushed or
published. The [review packet](pss-sincal-review.md) owns the detailed PR
scopes, draft descriptions and evidence. The [user guide](../src/sincal.md)
owns public API usage and supported profiles. This roadmap records remaining
work and acceptance boundaries rather than a chronological implementation log.

## Scope and architecture

SINCAL can contain balanced and conductor-resolved networks. Callers select
`sincal-balanced` or `sincal-multiconductor` explicitly. The readers produce
`BalancedNetwork` in `powerio-tx` or `MulticonductorNetwork` in `powerio-dist`.
Neither electrical backend depends on the other. Shared bounded acquisition,
schema records and source retention live in `powerio-sincal`; the facade owns
public dispatch. Keep existing model, matrix, problem and binding boundaries.

A supported profile does not imply every SINCAL schema, equipment mode or
snapshot is supported. Required unsupported electrical mappings currently
reject the whole read. Any future partial reader must preserve identities and
ports, carry durable completeness state through IR, and block inappropriate
consumers. Merely retaining unknown electrical equipment in extras is unsafe.

Unchanged same-format source emission is byte exact. IR retains typed values
and provenance but omits source bytes. Fresh native writing remains a separate,
explicitly experimental profile; it is not a replacement for source echo.
Cross-format losses, defaults and compatibility assumptions must be reported.

## Five local PRs

| PR | Branch | Scope | Base |
| --- | --- | --- | --- |
| 1 | `codex/sincal-source-references` | Source references, consumers, IR and C/Julia compatibility | `d5f93763` |
| 2 | `codex/sincal-infrastructure` | Bounded native acquisition and shared storage | PR 1 |
| 3 | `codex/sincal-transmission-reader` | Balanced mappings and public integration | PR 2 |
| 4 | `codex/sincal-distribution-reader` | Multiconductor mappings and two-family binding integration | PR 3 |
| 5 | `codex/sincal-experimental-writer` | Fresh SQLite writing for a declared subset of both families | PR 4 |

Keep dataset work grouped into the two reader PRs. Matching local PowerIO.jl
companions cover source-reference access and reader options. The fresh remote
check on 2026-10-08 found no drift from `d5f937631a4f7c60460c610a02eb949e35812252`.
Check again before publication. Branch heads and local artifacts are recorded
in the session handoff manifest.

## Guideline audit and revised delivery priorities (2026-10-08)

The approved priorities are delivered:

- **Fidelity:** retained-table/selected-field inventory, affected counts,
  structured default and compatibility provenance, and loss reporting after
  IR restoration. Excluded Access tables have explicitly unknown row counts.
  This is not an exhaustive field-level audit of every native schema.
- **Bindings:** separate balanced and distribution selections across Rust,
  CLI, Python, C and Julia, including the narrow distribution compatibility
  option. Published ABI layouts remain intact; the selection additions are
  unpublished parts of this stack.
- **Trial workflows:** verified native echo, typed IR, calculation preparation
  and ordinary emission/reparse separately for each advertised case.
- **Integration:** completed CI-mirror stages (resumed after fixes), Python
  and Julia suites, acquisition tests and fresh independent numerical checks.
  The [verification report](../../evals/sincal/priority-verification.json)
  states exact coverage and exclusions. The legacy all-format external oracle
  harness was not rerun; SINCAL-specific oracles were.

PowerIO permits bounded profiles and documented approximations. It does not
permit silent loss of required electrical equipment or presenting a successful
parse as proof of solver/export support. Existing downstream readiness checks
remain authoritative. No general permissive-reading framework is required to
ship the narrow, explicitly selected source-control assumption.

## Complete-case evidence and limits

| Family / case | What is established | Remaining qualification |
| --- | --- | --- |
| Balanced: SimBench, IEEE18, IEEE33, student study, CSIRO19 | Five complete cases with independent parameter/numerical checks; CSIRO19 has seven snapshots | Student saved loads are zero; only declared schemas/modes are supported |
| Distribution: CSIRO09 | All 688 elements; five native snapshots plus five synthetic asymmetric stress cases | Native loads are symmetric; generic PF rejects unsourced islands; ordinary DSS/PMD/BMOPF export rejects referenced sources |
| Distribution: Truong12 | Complete native asymmetric 12-bus case with independent voltages and currents | Smaller than the requested additional substantial feeder; no redistribution license, so external only |
| Distribution: CSIRO12 | All 215 elements; five native snapshots plus five synthetic asymmetric stress cases; PF preparation passes | Explicit source-control assumptions; native NULL semantics unverified; native loads symmetric; ordinary referenced-source export rejects |

CSIRO12 supplies one additional substantial experimental distribution success
under the user's accepted assumption policy. Do not describe it as another
published asymmetric operating point or verified native NULL interpretation.
Keep that qualification in the PR description.

The [evidence index](../../evals/sincal/README.md) links source identities,
licenses, measured errors, negative controls and reproducible commands.
Independent OpenDSS construction uses native inputs; it does not establish
numerical equivalence of PowerIO's ordinary exported DSS models. PowerIO itself
has no solver. No unbalanced paper power-flow benchmark has been reproduced.

## Review and external acceptance gates

Next, review the five concrete diffs and declared profiles. Focus first on
source/reference semantics, acquisition boundaries, component mappings,
assumption visibility and consumer refusals, then inspect supporting evidence.
No broader dataset search or equipment expansion is required for this review.

Before publication, obtain explicit user authorization, recheck the remote
base, and rerun affected checks if review changes code. No push, PR creation,
merge, tag or release is authorized by this roadmap. The writer stays
experimental even when its internal round trips and independent circuit checks
pass. Opening and calculating generated files in SINCAL remains an external
acceptance gate, dependent on future tool access.

Further equipment, unresolved CSIRO cases, additional schema/profile modes,
verified native NULL semantics and ordinary referenced-source exports are
follow-up work driven by named use cases and adequate evidence. They are not
hidden prerequisites for reviewing the current reader.

## Evidence retention

Keep small licensed/synthetic fixtures, verification scripts, final numerical
reports and the source/licensing catalog. Keep original external models and
hash-matching acquisition records outside Git. Do not vendor models without
redistribution rights or bypass the fixture-size approval rule.

The cleanup replaces chronological prose and two overlapping historical corpus
reports with the current evidence index and a compact historical summary.
Earlier details remain recoverable at Git commit `1e95be55`; final source
identities, licenses, checks and limitations remain in the current tree.
Build caches, installed test environments and generated intermediates are
rebuildable. Preserve the original dirty research checkout and local branch
history until the work is reviewed and accounted for.
