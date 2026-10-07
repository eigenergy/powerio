# SINCAL local review packet

Updated 2026-10-08 against the approved [delivery roadmap](pss-sincal.md).
These are local PR drafts and an evidence index. User review is deferred until
one or two additional substantial distribution cases pass complete parsing and
independent end-to-end validation. That gate is not yet met. Preserve the five
stacked splits below; no PR is published.

## Base and review status

A fresh `git fetch origin main` confirmed `origin/main` is still
`d5f937631a4f7c60460c610a02eb949e35812252`, the stack's base. No base drift was
present at this check. Verify again before eventual publication. Local branch
heads and test logs are recorded in the session handoff manifest; branch names
below remain the review identities across local rebases.

- [x] Five coherent branches exist with the intended linear ancestry.
- [x] Current remote base checked; no merge/rebase onto newer main needed.
- [x] Draft descriptions, declared profiles and evidence indexed below.
- [ ] One or two additional substantial native distribution cases pass complete
  parsing and independent end-to-end validation (required before user review).
- [ ] Add each passing case's pinned source, counts, selection, command, oracle
  errors/tolerances and licensing disposition to PR 4.
- [ ] Final maintainability/correctness review of the complete per-PR diffs.
- [x] Balanced CLI selections: native CSIRO19 snapshots, source echo and failures.
- [ ] Balanced Access/snapshot selection integration across Python/C/Julia.
- [ ] Final affected regression packet after integration changes.
- [ ] User discussion and explicit permission before pushing or opening PRs.

The current distribution PR is the largest. Much of its diff is synthetic tests,
external validation harnesses and measured reports. Review its acquisition/API
boundaries and component mappers first, then the supporting tests and evidence.
Keep dataset work organized within this PR; do not split it into more dataset
PRs. Add modes needed by selected whole-case candidates only when independently
verified. Do not expand equipment coverage merely to improve partial counts.

## PR 1 — Preserve source references in multiconductor models and consumers

Branch: `codex/sincal-source-references`; base: reviewed `origin/main`.
Julia companion: the local branch of the same name.

**Draft description.** Multiconductor inputs need to distinguish floating,
grounded and earth-referenced sources without changing their electrical circuit.
Add typed source references and carry them through admittance assembly,
power-flow readiness, IR and C/Julia access. Preserve existing DSS/PMD/BMOPF
semantics and reject unsourced islands when constructing calculation instances.

**Review boundary.** Shared distribution model and consumers only; no SINCAL
acquisition or electrical mapper. Existing published IR meanings and ABI layouts
must remain compatible. Review source/neutral constraints, Wye current balance,
owner-rooted C access and the matching Julia calls before convenience helpers.

**Evidence.** The extracted branch's recorded packet has 1,042 Rust tests,
1,842 Julia assertions, header/symbol coverage and full CI clippy. Source and
multiconductor consumer tests in this branch are the reproducible checks; these
counts describe that recorded run, not a fresh run performed by this document.

## PR 2 — Acquire native SINCAL projects through bounded shared storage

Branch: `codex/sincal-infrastructure`; base: PR 1.

**Draft description.** SINCAL stores electrical inputs in native archives,
SQLite databases and Access projects. Add a model-neutral support crate for
bounded acquisition, schema/identity decoding and source retention, plus an
explicit optional MDB Tools importer. Preserve absent fields, NULL, zero and
original MDB identity without running external tools during library parsing.

**Supported profile.** Native SQLite/archive transport and the declared acquired-
record schemas. Structural admission is not electrical support or automatic
family selection. Original sources remain distinct from caller-supplied tables;
matching a source digest does not attest the table transcription.

**Evidence.** [Access acquisition report](../../evals/sincal/access-acquisition.json),
shared-crate malformed/budget/path/identity tests and Python acquisition tests.
All 19 original CSIRO MDBs have structural acquisition evidence. The electrical
backends must independently validate their admitted schema/profile meanings.

**Review focus.** Archive traversal, resource limits, source/companion lifetimes,
variant identity, NULL fidelity, importer subprocess bounds, package/CI graph.
No electrical equations should move into `powerio-sincal`.

## PR 3 — Read verified balanced SINCAL profiles into BalancedNetwork

Branch: `codex/sincal-transmission-reader`; base: PR 2.

**Draft description.** Add an explicitly selected balanced SINCAL reader that
produces the existing `BalancedNetwork`. Map the verified SQLite and Access
profiles, retain native source bytes, and reject phase-resolved or unsupported
active modes without balancing fallback. Support selected absolute daily
snapshots for schema-11.5 acquired Access records through the Rust API.

| Verified native case | Scope and evidence |
| --- | --- |
| SimBench 15-node LV | All 32 elements; paired CSV parameters and independent pandapower solution, including derived fixed-tap checks |
| IEEE18 / IEEE33 | All 44 / 67 elements; independently constructed balanced calculations |
| Student study | All 84 static elements; saved loads are zero, so this is weaker electrical evidence and not profile-result validation |
| CSIRO19 | All 33 elements at seven explicit times; public Rust parsing, MDB echo and IR preservation; independent voltage errors below 1.98e-11 pu |

**Limits.** Only the documented schema/equipment profiles are claimed.
Inherited variants, further active profile modes, additional SimBench/Access
cases and native SINCAL execution remain unsupported or follow-up. The combined
reader stack must complete balanced selection bindings before merge readiness.

**Evidence.** [SimBench](../../evals/sincal/balanced-simbench.json),
[additional SQLite cases](../../evals/sincal/balanced-external.json),
[CSIRO19](../../evals/sincal/balanced-csiro19.json), their adjacent `check_*`
harnesses, mapper tests and `powerio/tests/sincal.rs`. The balanced Access
milestone recorded 1,035 transmission/shared regression tests, 14 facade/schema/
graph tests, 19 Python acquisition tests and full CI clippy.

The combined reader stack also checks [balanced CLI snapshots](../../evals/sincal/balanced-csiro19-cli.json)
against the same independent oracle. This is interface coverage for CSIRO19,
not another supported native case.

**Review focus.** Declared-family routing, units and conversion base, terminal
states, fixed taps, active-control refusal, snapshot interpolation and factors,
legacy defaults with provenance, atomic rejection and source-versus-IR fidelity.

## PR 4 — Read verified conductor-resolved SINCAL profiles and expose both families

Branch: `codex/sincal-distribution-reader`; base: PR 3.
Julia companion: local `codex/sincal-distribution-reader`.

**Draft description.** Add conductor-resolved SINCAL mapping into the existing
`MulticonductorNetwork`, preserving supported phase circuits, source references,
open devices and explicitly selected snapshots. Integrate both SINCAL families
with the facade, CLI and typed bindings. Unsupported required electrical modes
reject the whole network; partial audit counts never become returned feeders.

| Supported evidence | What it establishes |
| --- | --- |
| Original CSIRO09 | All 688 elements, five snapshots, independent OpenDSS comparison on 617 energized nodes; four isolated nodes retained |
| Published Truong 12-bus | All 45 elements, 33 unequal single-phase loads; public native SQLite parsing, source echo, IR, matrix and PF-instance checks; independent OpenDSS voltages and terminal currents/powers |
| Five asymmetric stress cases | Unequal-load conductor behavior; these are synthetic modifications, not additional authentic native feeders |
| Independent component checks | Verified source, line, load, selected transformer, shunt and switch profiles; see the evidence index for exact modes and limitations |
| Public interfaces | Explicit family/variant/time/acquisition selection, typed access, original-source echo and IR preservation |

**New review gate.** This table now contains two complete native
distribution cases. The additional 12-bus case is smaller than the requested
feeders; add one or two substantial published cases with whole-network
validation before requesting user review; component reports and synthetic stress
cases cannot satisfy that gate. See the roadmap for the exact evidence packet.

**Limits that must appear in the PR.** CSIRO09's original loads are symmetric.
Zero of the seven priority genuinely unbalanced original CSIRO feeders parses
completely. Mixed partial transformers, Wye star/sequence modes, coupled reduced-
phase charging, limited-P/Q loads and other unsupported modes remain follow-up.
Conflicting native nameplates/profile samples require separate dispositions.
Generic PF-instance construction correctly rejects CSIRO09's unsourced isolated
nodes; successful parsing does not imply solver readiness for every retained island.

**Evidence.** [Public asymmetric 12-bus validation](../../evals/sincal/truong12-public.json),
[public CSIRO09 validation](../../evals/sincal/csiro09-public.json),
[component audit](../../evals/sincal/distribution-csiro-profiles.json),
[Python/CLI checks](../../evals/sincal/csiro09-bindings.json) and the
[detailed evidence index](../../evals/sincal/README.md). Current reports describe
successful profiles and explicit rejections separately. Preserve the license
boundary: native research files stay external unless redistribution is authorized.

**Review focus.** No tx/dist dependency or family fallback; grounded/floating
circuit semantics; no guessed missing physics; consistent controls and defaults;
component-to-model fidelity; selection plumbing and C/Julia ownership/layout.
The C selection structs are new in this unpublished PR, absent from current
`origin/main`; finalize their two-family shape with Julia before publication.

**Experimental compatibility evidence (2026-10-08).** CSIRO12 now parses all 215
elements through the public multiconductor reader at five snapshots, with
original MDB echo, IR, generic matrix and PF-instance checks. Native-input
OpenDSS comparisons cover 188 connected native nodes and all 153 nonideal lines;
three conductor-free node records remain in extras. Maximum complex-voltage
difference across native snapshots and synthetic unequal-load stress is
0.000370 V. The Rust/CLI opt-in assumes five NULL source controls inactive,
emits warnings and retains assumptions in IR. This is a usable experimental
example, not verified native NULL behavior or a new published asymmetric case.
See [the report](../../evals/sincal/csiro12-compatibility.json).

## PR 5 — Add opt-in experimental fresh SINCAL output

Branch: `codex/sincal-experimental-writer`; base: PR 4.

**Draft description.** Generate deterministic schema-14.8 SQLite databases and
archives from a declared subset of either existing network family, without
requiring native source bytes or a template database. Expose fresh writing as
an explicit experimental Rust option, separate from unchanged-source echo.
Reject unsupported physics before creating a partial output.

**Supported subset.** Existing checked source/line/load profiles, selected
balanced and multiconductor transformer representations, and canonical open
phase switches. Construction, editing and IR-restored values have readback
checks. Freeze this subset for the initial PR; no new equipment modes are needed.

**Limits.** No Access writing, native project UI/diagram reconstruction, inherited
variants, time-series writing, general external-neutral/shunt circuits or arbitrary
transformer primitives. Native SINCAL open/save/calculate remains unverified.
`can_emit=false` continues to prevent advertising unrestricted ordinary emission.

**Evidence.** Writer roundtrip tests and the independent writer reports linked
from [the evidence index](../../evals/sincal/README.md). After the balanced Access
integration, 244 focused Rust tests, 26 Python evidence tests and full CI clippy
passed on the combined stack. CSIRO19's public-reader measurements remained
unchanged, and the SimBench fresh-writer oracle reproduced its prior report.

**Review focus.** Declared subset checks, deterministic construction, no hidden
source/template dependency, transformer/switch circuit equivalence, diagnostics,
atomic output and separation of experimental fresh writing from exact source echo.

## Reproducing checks without expanding scope

Use each branch's applicable unit/integration tests and the exact commands in
`evals/sincal/README.md`. External jobs require their hash-pinned source files;
missing required data is a failure, not an implicit pass. Run affected checks
after integration edits, and the full CI clippy matrix before final handoff.
Do not rerun unchanged expensive oracles merely to increase a test count.

The original research checkout remains untouched. This packet does not authorize
publication. The additional distribution-case gate now precedes user review;
completion of the entire corpus and native writer acceptance remain separate.
