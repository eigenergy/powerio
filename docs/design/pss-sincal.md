# PSS SINCAL reader and writer proposal

Status: reader integration and validation complete locally, 2026-10-08.
Both explicit public reader profiles are implemented. Five balanced cases and
two complete conductor-resolved native cases have strict-reader electrical
evidence: CSIRO09 and an asymmetric 12-bus model. The latter is smaller than
the additional substantial feeders requested. Following the user's 2026-10-08
acceptance of labelled inaccuracies for trial use, CSIRO12 now supplies one
additional substantial distribution example: all 215 elements pass public
parsing and independent numerical checks under explicit source-control
assumptions. This is experimental coverage, not verified native NULL behavior
or another published asymmetric operating point. Keep those qualifications in
the PR description; do not redefine distribution support to require every
published snapshot to be asymmetric. Both families now expose selections in Rust, CLI, Python, C and Julia;
the narrow distribution compatibility option reaches all five entry points.
The [review packet](pss-sincal-review.md) records the completed integration
checks and the five local PR scopes. No PR has been published.
Nothing is published. Research base: `c8184eba` (PowerIO 0.11.4); the local PR
stack starts at `d5f93763`. The current user-facing capabilities are documented
in [the guide](../src/sincal.md). Earlier rationale below is dated context,
not current API authority.

The expanded search found authentic licensed unbalanced CSIRO data and the
Siemens database/input manuals. The earlier blanket lack-of-evidence blocker
is therefore narrowed: implementation can progress from documented phase,
load and grounding meanings. Linked explicit-neutral/coupling examples and
native writer acceptance remain open. See the detailed
[research catalog](../../evals/sincal/research-catalog.md).

## Scope

SINCAL is one exchange format with balanced and conductor-resolved electrical
profiles. Balanced profiles use `BalancedNetwork` in `powerio-tx`; unbalanced
profiles use `MulticonductorNetwork` in `powerio-dist`. The first public
milestone must include unbalanced steady-state support, but this priority does
not make every SINCAL project a distribution model. Do not lower an unbalanced
profile through a balanced equivalent. Native SQLite projects and SQLite-backed `.sinx` archives are the
initial input candidates. A `.sin` file alone is not the electrical database.
The expanded reader target includes Access through an explicit optional import
path, because all 19 CSIRO cases use that transport. Direct SQLite/archive
reading and tool-assisted Access import must be documented separately.
Server databases, dynamics, protection calculations, and network-state XML
patches remain outside the initial electrical profile. An Access-to-table or
Access-to-SQLite transcription is not an authentic native SQLite export.

The latest 2026-10-07 decision supersedes the earlier supported-profile-first
review sequence: earn one or two more substantial complete distribution cases
before asking the user to review. This is a bounded reader milestone, not a
requirement to finish the entire CSIRO collection. Keep the five PR splits and
the existing experimental writer subset. Unsupported electrical modes still
fail explicitly. The 2026-10-08 user decision additionally permits labelled
parsing approximations to encourage trials. Prioritize small, case-backed
compatibility improvements with explicit opt-in, diagnostics and IR-persistent
assumption metadata. Do not silently invent impedances, omit equipment or change
network family. Report strict and assumed-circuit evidence separately.

The user does not currently have SINCAL access. We can establish structural,
electrical, and cross-tool evidence now, but successful import and calculation
in SINCAL remain separate acceptance checks when access becomes available.

## Two-backend architecture contract

This contract corrects the earlier distribution-only placement language and
governs both reader and writer delivery. PowerIO already has two network
families; SINCAL adds adapters to those families, not another network type.

| Selected electrical profile | Reader value and component owner | Fresh writer input |
| --- | --- | --- |
| Balanced positive sequence | `PioModule<BalancedNetwork>` from `powerio-tx` | `BalancedNetwork`, using its existing units, references, taps and component semantics |
| Conductor-resolved, including unbalanced networks | `PioModule<MulticonductorNetwork>` from `powerio-dist` | `MulticonductorNetwork`, preserving ordered conductors, neutral/reference constraints and full supported circuit data |

The facade owns the conversion to the existing `PioValue` variant. Its SINCAL
route must classify the selected electrical profile before choosing a backend;
registering `sincal` unconditionally as `RoutedFamily::Distribution` would
violate this contract. Fresh emission dispatches from the typed value through
the corresponding family emitter. One format identity may accept both value
families; there is no `SincalNetwork` public value or alternate binding type.

Selection must use version-verified native model/profile declarations and the
requested interpretation, not voltage level, a transmission/distribution
label, equal P/Q values, all-L123 connections, zero-filled inactive columns,
or the presence of historical balanced/unbalanced result tables. A conductor
model remains conductor-resolved even at a balanced operating point. A native
project can retain several studies; its last calculation is not by itself a
model-family declaration. If the interpretation remains ambiguous, report
that and require an explicit profile selection rather than guess. The exact
selection API and schema fields must be established before public routing.

Apply validation within the selected profile. A balanced positive-sequence
reader must not demand zero-sequence or explicit-neutral inputs that its
model does not need. Conversely, failed multiconductor validation must never
trigger a balanced fallback. Symmetrical-component reduction or phase
expansion is an explicit conversion with diagnostics for assumptions and
losses; parsing and ordinary emission do not perform it implicitly. If a
selected profile excludes active source semantics, report that exclusion or
reject it according to the existing fidelity contract.

Keep the electrical mappers in their respective component crates. Neither
`powerio-tx` nor `powerio-dist` may depend on the other or on the facade.
The existing private `powerio-dist/src/sincal/` implementation is the
multiconductor adapter and research staging location, not the final owner of
all SINCAL support. Before adding the balanced adapter, factor shared archive,
SQLite/schema, identity and source-retention mechanics behind a model-neutral
boundary, without moving either network type into `powerio-core`. If this
requires a shared internal format-support crate, include its dependency,
packaging and release changes in that implementation; do not route the
balanced backend through `powerio-dist` merely to reuse acquisition code.
Family-specific field interpretation remains with each mapper.

Shared support is implemented in `powerio-sincal`: bounded archive acquisition,
source/companion retention and SQLite schema/identity decoding depend only on
core, ZIP and SQLite. Electrical adapters are developed on the subsequent
reader branches, and own electrical methods and family diagnostics. Structural validation
requires no phase/zero-sequence inputs and does not select a family from stored
result tables. Neither a balanced mapper nor automatic profile selection is
implied by sharing this layer. The support crate is included
in workspace testing/clippy, publish order and the checked architecture map.

Both paths retain source bytes on `PioModule`, preserve unchanged same-format
echo, and distinguish fresh emission after edits or IR deserialization. Reuse
existing diagnostics, output destinations and typed Rust/Python/C/Julia
access. No new ABI layout or IR meaning is justified merely by the format
supporting two families. Each family keeps its existing matrix and solver
consumers; SINCAL introduces no solver dependency into either model crate.

Public reader integration requires evidence for these cases:

- A declared balanced SINCAL case produces `BalancedNetwork` and supports
  paired SimBench parameter checks plus balanced cross-format/matrix checks.
- The native conductor-resolved case and independently checked asymmetric
  synthetic circuits produce `MulticonductorNetwork` and preserve supported
  phase/neutral/source constraints through its existing consumers. Record the
  synthetic origin of asymmetric evidence. An additional authentic mixed-phase
  feeder is now a prerequisite for user review; aim for two additional cases.
- A balanced operating point in a conductor model retains its multiconductor
  type; ambiguous profiles and conflicting declarations do not silently select
  a family; unsupported unbalanced inputs cannot fall back to balanced parsing.
- Balanced input without zero-sequence data succeeds where that profile
  permits it, while multiconductor input still refuses missing required data.
- Source echo preserves the original acquired source; edits and IR-without-source
  cannot silently reuse obsolete bytes. Existing format routing and binding
  regressions pass, and the crate dependency graph stays acyclic.

The separately experimental writer must read fresh writes from both supported
typed families back to the intended family. Its smaller supported profile and
native acceptance status do not restrict the reader's supported corpus.

The current SimBench archive is a candidate for the balanced path, subject to
profile-selection evidence; failure of its private multiconductor mapping
does not establish that it is invalid as a balanced network. The external
CSIRO cases remain the priority for unbalanced validation. Neither family is
advertised as public SINCAL support until its required implementation exists.

## Evidence and example files

[SimBench's dataset catalog](https://simbench.de/de/datensaetze/) provides
native SINCAL models alongside CSV, PowerFactory, and INTEGRAL versions.
The following file was downloaded and inspected outside the repository:

- Case: `1-LV-rural1--0-sw.sinx`.
- [Original download](https://daks.uni-kassel.de/bitstreams/1e89ef81-8a51-4412-8795-5cdc1e285db9/download).
- Archive size: 88,876 bytes.
- SHA-256: `019f52397b4bc5673abed79a5fe1ed0d484d9674ffca04d4102806cd29ccc659`.
- Contains `SIArchive.ini`, a binary compound-document `.sin` file,
  `database.ini`, a 2,289,664-byte SQLite database, and diagram/log sidecars.
- Database: 250 tables; 15 nodes, 32 elements, 46 terminals, 13 lines,
  13 loads, four converter generators, one two-winding transformer, and one infeeder.
- The `Version` row includes `14.8`; do not interpret this as the product
  release without the schema documentation.
- The inspected load has aggregate P/Q and zero phase-specific P/Q fields.
  This is a balanced regression candidate, not evidence of unbalanced coverage.
- `database.ini` contains an original absolute Windows path. Acquisition must
  resolve the packaged database safely, not follow that path on the host.

SimBench states that its data uses ODbL 1.0 and individual database contents
use DbCL 1.0. See the [project license](https://simbench.readthedocs.io/en/v1.5.3/about/license.html)
and [ODbL terms](https://opendatacommons.org/licenses/odbl/1.0/).
Any vendored case needs provenance, attribution, both applicable notices, and
clear separation from PowerIO's code license. Adapted databases must retain
the applicable ODbL obligations. Inspect archive contents for additional
notices before adding a fixture. The unmodified archive and ten small paired
CSV files are now under `tests/data/sincal/`, with the upstream license
notice and source links. No additional notices occur inside the archive.
The archive is below the 100 KiB fixture limit, but its unpacked database is
above it; do not vendor that database without the exact-file approval required
by AGENTS.md. Record `wc -lc` and the diff statistics before committing fixtures.

For unit tests, prefer small original synthetic data covering each supported
electrical behavior. A synthetic file derived from an unverified schema is not
an independent SINCAL interoperability oracle. Keep one authentic native case
for acquisition and byte-exact echo; use larger cases in an optional external
validation corpus.

## Other implementations and documentation

- [CSIRO's Australian feeder collection](https://doi.org/10.4225/08/5631B1DF6F1A0)
  provides 19 native Access projects under CC BY 4.0. Representative 01 has
  mixed phase connections, 252 transformers and historical unbalanced results.
  All 19 databases have now been inventoried outside the repository; none was
  vendored. Seven have mixed terminal phases and unbalanced results. The
  smallest of these is Representative 06 (4,820,992 bytes). No database has
  populated coupling or neutral-point tables; the detailed catalog distinguishes
  this inventory from the deeper numerical checks of Representative 01.
- [Benedikt Schmidt's LoadFlow](https://github.com/benediktibk/loadflow)
  includes an Access connector, 67 native test databases and April 2014
  Siemens manuals. Its applications/libraries are GPL; do not assume this
  licenses the vendor manuals or every inherited test model. The manuals
  resolve several enum and neutral-point questions, subject to version checks.

- [pandapower's sincal_converter branch](https://github.com/e2nIEE/pandapower/tree/sincal_converter)
  includes readers, writers, and synthetic-network test generators. Its root
  license is BSD-3-Clause. The authors say it was developed for SINCAL 17.0
  in [discussion 2907](https://github.com/e2nIEE/pandapower/discussions/2907).
  The converter disclaimer limits supported elements and versions. Its writer
  initialization uses Windows COM `Sincal.DatabaseManager.CreateDB` and
  `Sincal.Simulation`, so it is not a standalone cross-platform writer oracle.
- [Sigrid](https://github.com/stianchris/sigrid) is GPL-3.0-or-later and imports
  SINCAL CIM XML into PyPSA/TESPy. It is an exchange-profile reference, not a
  native database writer or a demonstrated unbalanced oracle.
- [Zepben exporter FAQ](https://zepben.github.io/evolve/docs/sincal-exporter/2.18.3/common/faq/)
  documents a writer that uses `.sin` and version-specific blank database
  templates. Public documentation was found; an open-source license for the
  exporter or redistribution rights for its templates were not established.
- Siemens' [15.0 release notes](https://sincal.s3.amazonaws.com/15.0/ReleaseNotes-Eng.pdf)
  describe SQLite support; its [7.5 release notes](https://sincal.simtec.cc/7.5/ReleaseNotes-Eng.pdf)
  describe native and database-independent XML payloads in `.sinx` archives.
  XML archives are a potential alternative transport, pending actual samples
  and schema evidence. They are distinct from CIM and network-state XML.
- [Official product downloads](https://www.simtec.cc/en/downloads.aspx) require
  access credentials. No redistribution license was established for the
  bundled Siemens examples.
- Siemens' April 2015 database-interface manual documents `SinDBCreate.exe`
  for generating blank electrical databases and an optional `.sin` companion
  without the graphical interface. It is supplied with SINCAL; that revision
  lists Access and server backends, not SQLite. Use it as a future seed-project
  acquisition route when access is available, not as evidence that our
  standalone SQLite writer already has a verified construction contract.
  See [§3.2, printed pp. 37–39](https://manualzz.com/doc/2231110/pss-sincal-database-interface-and-automation).

## Five-PR delivery plan: supported profiles first

Updated 2026-10-07 after review: dataset-specific work is grouped into one
balanced reader PR and one multiconductor reader PR. These five PRs supersede
the earlier nine-PR breakdown. Develop and verify commits and branches locally;
do not push branches or open PRs until the user reviews the result and
explicitly authorizes publication.

| PR | Local branch | Base | Scope |
| --- | --- | --- | --- |
| 1 | `codex/sincal-source-references` | Reviewed base `d5f93763`; check `origin/main` drift | Shared source/reference model, matrices, PF, IR and C/Julia compatibility |
| 2 | `codex/sincal-infrastructure` | PR 1 | Shared archive/SQLite/Access acquisition, structural schema records and corpus harness |
| 3 | `codex/sincal-transmission-reader` | PR 2 | Balanced mappings across datasets/schemas and balanced public integration |
| 4 | `codex/sincal-distribution-reader` | PR 3 | Multiconductor mappings across datasets/schemas and two-family public integration |
| 5 | `codex/sincal-experimental-writer` | PR 4 | Fresh SQLite writing for a declared subset of both network families |

This is a linear local review stack. PR 4's technical implementation primarily
needs PRs 1–2; stacking it after PR 3 makes combined dispatch tests and eventual
review bases unambiguous. It does not lower unbalanced work's priority. Each
branch must have coherent commits and pass its scoped tests; moving a branch
pointer without implementing its deliverable does not complete it. PR 1 also
has a local PowerIO.jl companion with the same branch name.

### Guideline audit and revised delivery priorities (2026-10-08)

This update supersedes the earlier case-search-first sequence and the informal
recommendation to make conversion the default trial workflow. It reconciles
`AGENTS.md`, the current guide, actual reader/writer/consumer code and the user's
acceptance of labelled approximations. Design notes are dated evidence, not
current API authority. The original dirty research checkout was inspected but
not modified; implementation status below refers to the local reader stack.

#### What PowerIO permits and what this branch actually does

| Repository contract / precedent | Actual SINCAL state | Delivery consequence |
| --- | --- | --- |
| [Bounded format profiles](../src/concepts.md#values) and [known limits](../src/scope-0.11.md): full source-format coverage is not required | Both families have bounded readers; native fresh writing is experimental | Publish precise schema/equipment/selection support, without promising the full CSIRO corpus or native writer acceptance |
| [Distribution checks](../src/distribution.md) separate parsing, semantic validity and computational support; DSS retains unresolved geometry and blocks analysis (`powerio-dist/tests/electrical_readiness.rs`); BMOPF retains unknown fields with warnings | SINCAL currently rejects unsupported required electrical mappings atomically; this is its profile policy, not a universal PowerIO parsing requirement | Keep malformed identities/dimensions/references fatal. Permit documented defaults and explicitly selected assumptions. Do not make every future readable case contingent on a solver or writer supporting it |
| The existing model has `untyped`, extras, diagnostics and readiness audits | The generic readiness audit specifically blocks DSS geometry, not every arbitrary untyped electrical object | Simply putting failed SINCAL equipment into `untyped` would be unsafe. A future retained-but-incomplete read needs preserved identities/ports, durable IR state and explicit downstream guards; it is not implemented by this plan |
| [Format fidelity](../src/format-fidelity.md#format-notes) calls for named table/field losses and affected counts; registered diagnostic details carry machine-readable context | Both SINCAL readers currently issue broad source-only remarks; the compatibility warning names fields in prose and extras | Itemize relevant retained-only tables/fields and counts, plus structured component/field/assumption details. Audit default provenance through IR and cross-format loss reporting; do not treat a generic warning as complete fidelity accounting |
| [Same-format fidelity](../../AGENTS.md) preserves retained bytes; IR omits those bytes and edits invalidate echo | Native echo and IR value preservation are checked; ordinary fresh SINCAL emission remains unavailable, with `can_emit=false` | Keep echo, IR serialization and experimental fresh emission distinct in commands, examples and capability metadata |
| Numerical and writer support are separate (`powerio-dist/src/convert.rs::emit_text_with_options`) | Ordinary DSS/PMD/BMOPF emission rejects terminal-referenced voltage sources; CSIRO09/12 use them. Their OpenDSS oracle builds its reference directly from native inputs | Advertise parse → inspect → IR/matrix/PF where supported. Do not advertise generic conversion for these cases. An independent oracle is not evidence of PowerIO export support |
| [Typed/lazy bindings](../../AGENTS.md), stable names and matching C/Julia layouts | Distribution variant/snapshot/acquisition options already exist in Rust, CLI, Python, C and Julia; balanced options exist in Rust/CLI; the new source-control assumption exists only in Rust/CLI | Complete the specific option gaps instead of rebuilding binding infrastructure. Finalize unpublished C selection additions with the same-named Julia companion; preserve published ABI layouts |
| [Contributor checks](../src/contributor-workflow.md) require the CI mirror before pushing and surface-specific gates | Recent Rust/Python checks and focused clippy pass, but are not a fresh full CI-mirror packet | Run the required integrated checks on the final stack before publication; record unavailable external tools as missing checks, not passes |

The format-fidelity guide already documents reported substitutions/defaults in
other readers. The user's tolerance for approximations is compatible with that
practice. It does not authorize a false completeness claim or the silent removal
of electrical equipment. Nor does it require a general strict/permissive policy
framework: the existing format-specific option is sufficient for CSIRO12.
`LinDist3FlowBuildOptions::unsupported` is a calculation-preparation policy;
its Reject/Lower/Approximate/Permissive modes are not SINCAL parse options.

#### Delivery update (2026-10-08)

Priorities 1–3 are implemented locally: bounded source inventory with structured
loss details; versioned default and compatibility provenance; separate balanced
and distribution option types in Python/C/Julia; and case-specific public trial
checks. The [user guide](../src/sincal.md#fidelity-findings-and-working-trial-routes)
and [reproducible trial harness](../../evals/sincal/check_trial_workflows.py)
distinguish source echo, IR, PF preparation and ordinary target emission.
CSIRO09/12 do not acquire unsupported export capability from their passing
independent OpenDSS oracles. No new native fixture is added.

Integration uncovered and repaired the missing rusqlite browser-backend feature;
WebAssembly component compilation now passes. The matching Julia companion
passes 1,895 assertions (two existing broken tests), and its native CSIRO19/12
checks pass 18 assertions. The CI mirror stages pass, including installed-wheel checks (265 tests passed,
one skipped); the writer's scoped tests pass and its ten implementation patches
are preserved through restacking. [The verification record](../../evals/sincal/priority-verification.json)
and [trial workflow report](../../evals/sincal/trial-workflows.json) record the
scope and limitations. Priorities 1–4 are complete locally; maintainer review and
publication permission are the next steps.
The fetched remote base remains `d5f937631a4f7c60460c610a02eb949e35812252`.

#### Next implementation sequence

| Order | Bounded deliverable | Done when |
| --- | --- | --- |
| 1 | Close fidelity/reporting gaps for the existing supported profiles | Relevant retained-only tables/fields and counts are reported; assumptions/defaults have structured context and survive IR as required; echo/edit/IR behavior and cross-format losses have focused tests |
| 2 | Finish two-family option integration | Python gets the compatibility flag; balanced Access/variant/snapshot options reach Python/C/Julia; decide and document the compatibility flag's C/Julia exposure in the same unpublished selection-layout pass; typed/lazy access and explicit family selection remain unchanged |
| 3 | Deliver an accurate trial workflow and capability table | One command sequence per named case reaches its actually supported output: native echo, IR, matrix/PF construction, or an independently tested target format. CSI09's unsourced-island PF refusal and CSI09/12's referenced-source export refusal are explicit |
| 4 | Assemble the existing five PRs and run integrated gates | Supported profiles, qualified CSIRO12 evidence, known losses, bindings and tests are reviewable together; final base drift and per-PR diffs checked; CI mirror and applicable external checks recorded |
| Follow-up | Expand retention or equipment only for a named user case | A retained-but-incomplete read is distinguished from an electrical success and blocked at unsupported consumers. Any new approximation has provenance and independent checks. No new broad search, paper parameter fitting or general permissive parser is required for these PRs |

For order 3, start with the working native-source/IR path; a new referenced-source
DSS/PMD/BMOPF lowering is not a hidden prerequisite. If export becomes the chosen
trial deliverable, implement and validate one explicit target/profile rather
than removing its reference-source guard. Keep the experimental SINCAL writer
subset frozen; reader export to another format is a distinct boundary.

#### Evidence and review gates

The user's earlier request requires one or two additional substantial published
distribution cases before review. The later acceptance of labelled approximations
now gives us one such experimental trial case, CSIRO12, with all 215 components
mapped under five explicit NULL-source-control assumptions. Its native loads are
symmetric; that does not make its conductor-resolved model a balanced-backend
case. Native NULL semantics remain unverified. Existing CSIRO09 and asymmetric
Truong12 evidence remains useful. Do not make another arbitrary case count or
reproduction of an underspecified paper a prerequisite to the bounded work above.
The [review packet](pss-sincal-review.md) remains a draft until integration and
fidelity checks are complete; this update does not request review or publication.

Whole-case electrical validation remains the standard for claiming a complete
numerically checked feeder. Source retention/inspection, defaulted interpretation,
consumer readiness, external solver agreement and native SINCAL acceptance must
be reported separately. No new public value type or status framework is needed.

For a case to count, use an unmodified published native source and its explicitly
selected variant/snapshot. Acquire every table/sidecar needed by active inputs;
parse the entire selected model through the public conductor-resolved profile;
account for every node, component, phase and switch state; and compare the actual
PowerIO output against an independently constructed circuit and solve. Report
complex voltages, terminal currents/powers, reference constraints, topology and
load totals as applicable, with justified tolerances and solver convergence.
Require zero unreported electrical omissions. Preserve inactive equipment and
identify unsourced islands explicitly rather than silently dropping them.
Source echo and IR checks must use the same native case. A balanced-profile
parse alone cannot replace this distribution milestone. A symmetric operating
point explicitly parsed as MulticonductorNetwork remains distribution evidence;
report the limits of its phase-asymmetry coverage.
Component-only checks, tiny isolated circuits, different times of one feeder,
writer-generated models and repaired derivatives do not count as additional
published complete cases. Native SINCAL execution remains a separate claim.

Published paper results are an acceptable additional numerical reference when
case inputs, selected state and reported quantities can be matched. Use table
rounding or figure resolution to justify tolerances; distinguish fundamental
phasors from harmonic RMS/THD and component losses from all-frequency totals.
Paper aggregates supplement the whole-network phase-resolved checks above;
they do not alone satisfy that gate. The existing balanced IEEE18 now has a
[paper cross-check](../../evals/sincal/ieee18-paper.json) independent of native
result dumps. No additional unbalanced case is claimed from this evidence.
The subsequent [unbalanced-paper audit](../../evals/sincal/unbalanced-paper-constraints.json)
reconstructs output phasors where possible, but neither Arif 2013 nor Vinayagam
2015 provides enough matched input/state detail for an exact load-flow replication.
Do not spend further effort fitting missing parameters to these output figures.

Merge gates: maintainer agreement on each declared profile; completion of its
bounded integration work; passing relevant regressions, fidelity and diagnostic
checks; and no unresolved correctness defect within that advertised profile.
Perform a focused maintainability review of shared versus family-owned logic,
public API footprint and unnecessary abstractions. Do not invent new framework
work solely to prepare these PRs. Existing evidence can be reused when its code
and inputs are unchanged; rerun checks affected by edits or base drift.

External acceptance gates: native SINCAL open/save/calculate for generated files
and any claims of agreement with native execution. These remain unverified and
are not gates to reviewing or merging the explicitly experimental writer.
Undocumented electrical meanings still prevent a verified-native-semantics claim,
but the user-authorized explicit compatibility assumptions may support an
experimental profile. Broader unbalanced support must not be advertised on the
strength of partial component counts or synthetic cases alone.

### Follow-up corpus ambition and evidence accounting

This inventory retains the broader ambition. One or two additional complete
distribution cases are the immediate review gate; the rest remains follow-up.

| Corpus | Reader target | Evidence/data policy |
| --- | --- | --- |
| SimBench | Existing 15-node LV case, then available native LV/MV/HV and mixed-voltage cases representing distinct equipment modes | Balanced mappings compared with paired CSV. Inventory and pin additional archives before promising an exact count; retain ODbL/DbCL notices. |
| CSIRO 01–07 | Longer-term complete unbalanced reading of all seven feeders; CSIRO05 remains a candidate for the bounded additional-case gate | Schema 11.5 Access through explicit optional import. CC BY 4.0, external corpus; resolve active native meanings rather than fitting historical results. |
| CSIRO 08–19 | Account for every selected family/variant, including inheritance in 10/11/14 and blank terminal fields in 13 | Follow-up target: all 19 databases and applicable selectable variants. Three-phase connections and stored balanced results do not determine electrical family. |
| Truong 12-bus | Complete schema-15.0 native SQLite, 45 elements with unequal single-phase loads | New full public-reader/OpenDSS success; no redistribution license, external only; too small to close the larger-case review gate. |
| MATLAB LPC European LV/S1a | Mixed-phase schema-12.8 Access compatibility | External local checks while inherited model rights remain unresolved. No stored native results; independently check mapped electrical behavior. |
| IEEE18/33 and student study | Schema-15.5/16.0 SQLite compatibility | External cases, no fixtures without redistribution rights. Orphan coupling sidecar is not evidence of active coupling semantics. |
| LoadFlow's 67 databases | Inventory modes and select distinct regressions, including inspected schema-11.2 examples | Stretch coverage, not 67 promised passes; review model-specific rights; no copied GPL implementation or vendor manuals. |

Every report distinguishes acquisition, structural decoding, complete electrical
mapping, consumer readiness, independent checking and native-result agreement.
Record source hashes, schema, selected family/variant/snapshot, tool versions,
component totals and diagnostics. Rejections and partial component reports do
not count as successful parsed networks. Preserve inactive/islanded equipment;
a particular solver's readiness is separate from faithful parsing. Compare
historical results only against aligned inputs, never use those results to
fill missing input physics.

Default CI uses original synthetic cases and the existing small licensed
archive. External jobs use supplied hash-checked files and publish compact
reports; missing required acceptance data fails the job rather than becoming
a silent skip. Record optional cases as not run. No fixture over 100 KiB is
committed without exact-file approval of bytes, lines, source, license and PR
impact. Unclear data rights prohibit vendoring regardless of code licensing.

### Delivery feasibility and remaining validation

Completing the entire reader corpus remains a follow-up objective, not a
prerequisite for the five PRs or a promise that more coding alone can make every
original database pass. The additional distribution-case gate does apply.
Native SINCAL execution is an external acceptance gate.
Missing electrical meanings and conflicting native inputs are tracked separately
because they also limit what can be implemented faithfully without that access.

Current end-to-end evidence is five balanced cases (including CSIRO19's seven
Access snapshots) and two conductor-resolved cases. CSIRO09
has symmetric original loads; five separate synthetic stress cases exercise
asymmetry. The new schema-15.0 12-bus case has 33 authentically unequal
single-phase loads and maps all 45 elements. Public parsing, source echo, IR,
generic matrix/PF-instance construction, independent OpenDSS complex voltages
and line-terminal currents/powers pass. Its smaller size does not satisfy the
requested additional substantial-feeder gate. See
[the report](../../evals/sincal/truong12-public.json). None of
the seven priority unbalanced CSIRO01–07 feeders parses completely. Their
5,472/7,359 mapped components are component coverage, not a completion estimate.
The 1,887 first rejections comprise 798 coupled reduced-phase charging lines,
297 limited-P/Q loads, 271 partial mixed-winding transformers, 217 Wye loads,
ten other transformer/regulator modes, 291 inconsistent core-loss inputs and
three conflicting profile timestamps. Further failures may follow a resolved
first rejection.

The initial [candidate search](../../evals/sincal/research-catalog.md#larger-distribution-case-search-2026-10-07)
downloaded six additional LoadFlow databases and inspected another European LV
repository. It found no new ready unbalanced native case: five LoadFlow models
have MDB decoder errors, the reliable rural model is small and has only code-7
ports, and the new European LV database is empty. Related author repositories
and forks did not reveal another populated native release. Do not count these
as acceptance successes or continue the same broad searches without a new lead.

The populated MATLAB LPC European LV case remains 260/262 components, with
168 nodes and 55 single-phase loads. Newly inspected upstream construction code
corroborates the source/transformer category omissions; it does not resolve them.
Prioritize a concrete new model release or documented semantics over bypassing
those declarations. CSIRO05 is structurally close, with seven partial DYN11
transformers and two profile conflicts, but it is not a verified implementation
route yet. Its 343 saved transformer snapshots are nearly unloaded and do not
validate the proposed leakage model. A separate sequence-aware CSIRO06 circuit
hypothesis also failed all nine historical component comparisons. Stop extending
these hypotheses without new documented semantics or matched reference evidence.
CSIRO06 still has 35 Wye loads, nine partial transformers and eleven nameplate
conflicts. CSIRO12 now has a bounded experimental route: assume five schema-11.5 NULL
source controls inactive, with warnings. Its 215 components, five snapshots,
three unused node records and independent OpenDSS comparisons are covered in
[the compatibility report](../../evals/sincal/csiro12-compatibility.json).
Native NULL semantics remain unverified. Its native loads are symmetric, so it
does not alone demonstrate another published mixed-phase operating point.

A second MDB decoder failed on the larger LoadFlow candidate. The paired CSIRO
PowerFactory project was acquired unchanged under CC BY 4.0, but its binary PFD
has not been decoded and the user has no PowerFactory access. A usable export
is an optional external evidence route, not a required implementation milestone
for all SINCAL support. The reviewed vendor download route requires an access
key; no new public native model was acquired there. Those earlier findings added zero complete-case successes. A subsequent
GitHub code search found the complete 12-bus asymmetric model described above.
Its documented global missing-zero-sequence policy is now supported for the
verified sequence-line and ideal-source profile, with explicit inputs retaining
precedence and defaults recorded. It does not repair the global-input-only LPC
or CSIRO sources. Jackcess 5.1.7 now decodes the 294-node LoadFlow candidate, but
it lacks the required zero-sequence declarations and has no stored ULF results.
Cross-checking CSIRO05/06/12 with Jackcess confirms the previously observed
NULLs and nameplates. Two further public converters describe ordinary sequence
impedances, but do not resolve partial winding semantics. Resume a larger candidate
when a specific missing meaning, reliable export or independent reference is
available; retain the additional-case review gate without inventing a delivery
estimate from component counts.

Resolve source conflicts explicitly. Never alter an original model silently or
call a corrected derivative an original-case pass. Work on a candidate only when
there is a concrete route to whole-case verification; report failed hypotheses
and unresolved source meanings separately. Balanced-compatible Access inputs
remain in the balanced family and never inflate distribution completion counts.
Keep integration work bounded behind the new whole-case priority. Publication
still requires the user's later permission.

### PR 1: source-reference model and consumers

Extract typed floating/grounded source references, reference-aware sparse
constraints, prescribed PF boundaries, instance readiness, IR compatibility
and additive C/Julia access. Include the ideal WYE neutral-current corrections
needed by the same electrical consumers. No SINCAL acquisition or parser code.

Acceptance: unequal-loading voltage differences and current balance; relevant
DSS/PMD/BMOPF regressions; unchanged legacy layouts and earth-source semantics;
IR schema and rejection behavior; matrix/PF/C/Julia tests, header parity,
entry-point coverage, formatting and the complete CI clippy matrix. Validate
on the extracted current-main branch, not only the combined research checkout.
The Julia companion stays local under the user's publication restriction.

### PR 2: shared infrastructure and acquisition

Extract `powerio-sincal` transport, archive bounds, query-only SQLite decoding,
source retention, versioned structural identity/terminal records and the corpus
manifest. Integrate workspace publication/CI/architecture guards. No dependency
on either electrical backend. Shared fixture ownership must not imply that all
SINCAL data is distribution data.

Add explicit optional MDB Tools import with a version-tagged typed record
boundary shared with SQLite. Preserve absent fields, SQL NULL, numeric zero,
text and provenance. Keep original MDB source bytes/identity distinct from
intermediate tables. A transcription is not a native SQLite export, and source
echo does not imply Access writing after edits. Ordinary library parsing must
not silently spawn tools; expose tool requirements and platform capability.
Use shell-free invocation, bounded temporary output/time/rows and clear missing-
tool errors. Do not fall back to another family or reuse stale decoded data.

Target schema 11.5 alongside 14.8 structural decoding. Several MDBs exceed the
existing 64 MiB SQLite budget (largest inspected: 136,704,000 bytes); separate
transport/input/result budgets and measure memory rather than lifting all
limits. The full 19-case corpus is roughly 871 MB. Avoid eagerly loading result
tables for an input-only request, without discarding active electrical inputs
or profiles. Version-specific electrical meanings remain backend-owned.

Acceptance: authentic archive acquisition and unchanged-source echo; malformed
input/path/variant rejection; all 19 original CSIRO MDBs reproducibly acquired
with pinned counts/identities; missing/NULL/zero fidelity and failure/timeout
checks. Acquisition alone does not establish electrical support.

### PR 3: transmission / balanced reader

Own all electrical mapping in `powerio-tx`. Deliver the five currently verified
balanced cases, their checked schema/profile adapters and public balanced
integration in this PR. Cover buses/voltage levels, positive-sequence lines, loads,
external sources, converter generation, transformer ratios/taps/shifts/losses, terminal
switching and service states. Preserve IDs/provenance; do not require inactive
zero-sequence fields for balanced inputs.

Retain per-dataset commits/checklists for SimBench, CSIRO19 Access and
IEEE/student inputs. Additional SimBench modes, Access cases, active profiles
and inherited variants are follow-up scope. Unknown required
modes fail atomically with component/field diagnostics. A widened accepted
version range without checked field meanings is not schema support.

Acceptance: all 32 components of the licensed small SimBench case map, paired
CSV parameters match recorded tolerances, and independent balanced calculations
and MATPOWER/pandapower projections validate electrical meaning. Publish exact
achieved corpus/variant counts and unresolved cases. Public balanced dispatch
requires explicit or verified unambiguous profile selection; it must not make
SINCAL an unconditional transmission format. Combined public release follows
PR 4's two-family checks. Include CLI/typed binding/IR/echo tests applicable to
this path; a fresh writer is not required.

Current local progress (2026-10-07): the internal schema-14.8 balanced mapper
now covers all 15 nodes and 32 equipment records of the licensed small SimBench
archive. The actual Rust output passes paired-CSV parameter checks and a fresh
pandapower 3.2.2 pi-transformer comparison (maximum complex voltage difference
6.3e-14 pu). Derived nonzero primary/secondary tap cases also pass, below
5.1e-11 pu. Open-terminal and inactive-device retention and refusal of unsupported
active modes have focused Rust tests. The reproducible harness and exact report
are under `evals/sincal/`. Public `sincal-balanced` dispatch now reaches the existing typed model through
Rust, CLI, C and Python. Registered diagnostics distinguish profile selection,
malformed input, conversion assumptions and source-only data. Binary archive
and direct-SQLite echo are byte exact; edited and IR-restored modules refuse
fresh output. Additional schemas, variants/profiles and corpus coverage are
follow-up work. Balanced Access selection bindings and the final review packet
remain the initial PR deliverables.

Current balanced corpus progress: the initial public SimBench path is committed.
IEEE18, IEEE33 and the student study now map all 44/67/84 static equipment
records with schema-15.5/16.0 adapters and fresh independent checks below
4.5e-12 pu complex voltage. The student static state has zero demand; profile
snapshot coverage remains pending. Additional SimBench modes, applicable
Access cases, active profiles and inherited variants remain follow-up work.

CSIRO19 now adds a schema-11.5 balanced Access case: all 26 nodes and 33 elements
(25 lines, seven loads, one source), checked at seven explicit daily snapshots.
The public Rust facade retains the MDB, checks its identity against the caller's
acquired records, and preserves the typed value through IR. An independently
constructed pandapower model agrees within 1.98e-11 pu complex voltage; four
intentional unit/base/charging/source mutations fail. Exact MDB echo and rejection
of conflicting family options, missing time, invalid variant and altered original
bytes are exercised at every snapshot. Native SINCAL execution is not claimed.
See `evals/sincal/balanced-csiro19.json`. This is balanced coverage and adds no
complete unbalanced feeder. CLI selection now covers native CSIRO19 at two times with original MDB echo;
see `evals/sincal/balanced-csiro19-cli.json`. Python/C/Julia access to these new
balanced selection options remains an integration task.

### PR 4: distribution / unbalanced reader

Own all conductor-resolved mapping in `powerio-dist`. Deliver the existing verified
component profiles and public multiconductor integration. Preserve ordered phases,
phase pairs, unequal loads, supported neutral/reference behavior and open/inactive
devices. Keep verified absolute and common-factor daily snapshots. Unknown active
modes reject atomically; neither missing inputs nor source values may be chosen
to minimize historical-result residuals. New generic model gaps require typed
shared fixes, not SINCAL-only physics hidden in metadata.

Existing baseline: all 688 CSIRO09 elements parse through the public facade
at the five verified snapshots; independent OpenDSS comparisons pass on the
energized network and isolated nodes remain faithfully represented. Five labelled
asymmetric stress cases and the existing independently checked component circuits
establish the declared unbalanced profile. Preserve source echo, IR fidelity,
explicit selection, unsupported-mode diagnostics and existing consumer behavior.
Report that native CSIRO09 powers are symmetric and that zero of the seven
priority genuinely unbalanced original feeders currently parses completely.
These limits must appear in the PR description and supported-profile table.

User-review acceptance now additionally requires one or two substantial published
distribution feeders with complete end-to-end validation as defined above. New
non-CSIRO candidates are welcome; CSIRO05 remains a concrete existing candidate.
All seven priority feeders, remaining applicable CSIRO/LPC cases, additional
profiles, inherited variants/deletions, Representative 01 coupled charging and
Representative 13 blank terminals stay on that backlog. Resolve meanings from
evidence, test actual Rust output independently, never union stored variants,
and distinguish source-input conflicts from mapper gaps. Full explicit-neutral
or coupling modes remain unsupported until verified. Broader completion beyond
the additional-case review gate remains follow-up work.

Complete two-family facade/CLI/binding integration here. Both authentic family
cases yield existing typed values; a phase-symmetric conductor model remains
multiconductor; ambiguity requires selection; unbalanced failure never retries
balanced. No implicit phase expansion/reduction. Source echo, edited modules
and IR-without-source have distinct tested behavior. Direct SQLite input and
optional Access import remain visibly different capabilities. Update diagnostic
and conversion baselines intentionally, then run full CI clippy and bindings.

Current distribution progress: both public electrical families are integrated.
The conductor reader returns the existing `MulticonductorNetwork`; the balanced
reader returns `BalancedNetwork`. Rust, Python, CLI, C and Julia support explicit
family, Access acquisition, variant and daily-snapshot selection. MDB echo and
IR restoration are verified. No unbalanced failure falls back to a balanced parse.

The implemented conductor profiles cover directly supplied sequence lines,
verified reduced-phase circuits, ideal connections, phase-earth and phase-pair
loads, selected absolute and common-factor daily load profiles, ideal positive-sequence sources
with explicit zero-sequence circuits, several full-winding transformer groups,
nominal partial delta-delta windings, verified Y0 autotransformer profiles, and
fixed reactor/capacitor banks. Open ports preserve their electrical primitives
behind switches. Exact-zero ordinary lines become exact typed connections.
Documented schema-11.5 defaults cover sparse voltage bases, selected transformer
fields, optional shunt fields, line temperatures, line-model flags, dielectric
losses, temperature coefficients, parallel counts, rating factors and rated
frequency. Applied defaults retain provenance; source bytes and acquired NULLs
are unchanged. Required impedances, ratings and sequence declarations are not
filled in merely to achieve a successful parse.

At midnight, current component coverage is:

| CSIRO case | Mapped / total | Complete parse |
|---|---:|:---:|
| 01 | 781 / 1033 | No |
| 02 | 1525 / 2329 | No |
| 03 | 776 / 1084 | No |
| 04 | 575 / 861 | No |
| 05 | 1369 / 1378 | No |
| 06 | 163 / 218 | No |
| 07 | 283 / 456 | No |
| 08 | 265 / 309 | No |
| 09 | 688 / 688 | Yes |
| 10 | 99 / 124 | No |
| 11 | 87 / 104 | No |
| 12 | 214 / 215 strict; 215 / 215 opt-in | Experimental assumptions only |
| 14 | 55 / 65 | No |
| 15 | 89 / 102 | No |
| 16 | 90 / 103 | No |
| 17 | 130 / 142 | No |
| 18 | 169 / 295 | No |
| 19 | 1 / 33 | No |

All 19 base variants remain audited. Eighteen now have component dispositions;
CSIRO13's unresolved terminal declaration still prevents a topology-level audit.
Synchronous-machine port declarations follow the ordinary conductor selectors
documented in Input Data (April 2014), p.60. This lets the audit evaluate the other
components in CSIRO08/10/11/16/17/18; it does not implement those machines, create
source constraints or ground their stars. Complete parsing still rejects them.
CSIRO08/11 acquisition now includes their four capacitor banks, independently
checked alongside the existing five native banks. Missing acquisition tables
are not counted as missing native data. CSIRO09 remains the complete conductor-resolved
native feeder in this CSIRO audit; the separate 12-bus success is recorded above. Five authentic snapshots and five labelled unequal-delta stress
cases agree with independently constructed OpenDSS circuits within 0.000372 V
across 617 energized nodes; four native nodes remain isolated behind open
connections. Authentic snapshot loads are phase-symmetric; the stress cases
supply asymmetric evidence. Generic admittance assembly has no omissions.
Generic PF-instance construction rejects the four unsourced isolated buses,
as required, while connected synthetic input constructs successfully.

Additional independent evidence covers 484 native loads at 2,420 snapshots,
518 partial delta-delta transformers, all 144 CSIRO09 ideal connections, nine
rated shunt banks, 186 lines with temperature defaults, and 1,658 further finite
line circuits with sparse inputs. The latter includes 157 coupled single-phase
series-only lines checked by eliminating absent currents from OpenDSS's full
three-phase admittance. Their 29 exact-zero ordinary lines and 33 declared
connections retain exact topology and ratings. The CSIRO03 reactor also matches
49 historical local records at measured terminal voltages without fitting.
These component checks do not establish additional complete feeder solutions.

LPC European LV maps 260/262 components: all 205 lines and 55 single-phase loads,
with independent OpenDSS checks. Its source and transformer lack declared
zero-sequence inputs while automatic completion is disabled. LPC S1a maps 54
loads; its DC infeeds, library-referenced lines and source/transformer modes
remain work. Both files stay external because redistribution rights are unresolved.

Follow-up implementation work includes Wye star/sequence semantics, coupled
reduced-phase charging, partial mixed-winding transformers, additional regulators,
CSIRO03's 297 limited-P/Q loads, synchronous
machines, further sparse source/transformer fields, and broader variants/profiles.
CSIRO12 maps all lines, loads and its capacitor in strict mode. The explicit
`assume_inactive_source_controls` option additionally admits its source while
recording five NULL-to-zero assumptions. Original source bytes remain unchanged.
Three nodes with no declared equipment conductors are preserved in extras;
no actual unsourced electrical island is removed. Public parse, source echo,
IR, generic matrix/PF construction and five independent OpenDSS snapshots pass;
unequal-load stress tests are labelled synthetic. This does not verify native
NULL semantics or use a stored balanced result to infer them.

CSIRO05 now maps 456 additional loads with stored UI-manipulator references.
Independent interpolation and OpenDSS terminal-current checks cover all 2,280
snapshots (368 three-phase delta loads and 88 phase-pair loads). The stored
power factors apply once; manipulator IDs remain provenance. Two further loads
have conflicting native profile timestamps and still reject, alongside seven
transformers with unresolved partial mixed-winding circuits. This is 1,369/1,378 component
coverage, not another complete feeder. See `evals/sincal/materialized-loads.json`.
All seven transformers use DYN11 (`VecGrp=59`) with L1 selected at both terminals;
they therefore need verified partial mixed-winding circuits. The April 2014
database manual p.46 documents fixed status as the `Flag_roh` default. That default
is now implemented for schema-11.5 NULLs, with source preservation and provenance;
explicit controllers, invalid fields, missing columns and other-schema NULLs
remain errors. Admitting that default does not establish the partial circuits.

`evals/sincal/partial-mixed-history.json` audits all nine CSIRO06 partial YNd1
transformers against their aligned historical terminal records. An independent
single-coil OpenDSS hypothesis (Sn/3 per installed coil, nominal pi excitation,
without the separate native zero-sequence impedance) disagrees in every case:
maximum terminal power discrepancies range from 1.95 to 4.75 kVA. This is a rejected
acceptance hypothesis, not validation of reader output. Historical records do not
attest that the current input revision produced them. Distinct zero-sequence,
excitation and rating treatment must be resolved before expanding the mapper;
no parameter fitting or source repair is used.

CSIRO06's 55 remaining rejections are explicitly classified: 35 Wye loads,
nine partial YNd1 transformers and eleven inconsistent core-loss nameplates.
The excitation formula confirms the latter cannot produce a real reactive core
component, even though all 51 transformers satisfy the short-circuit `ur <= uk`
check. Input-data conflicts need separate acceptance dispositions; no tolerance
expansion or input repair is applied. Native SINCAL execution remains an external
acceptance gate, separate from continued reader implementation and independent
validation. One or two additional whole-case successes now gate user review;
broader corpus coverage beyond that remains follow-up. Final integration and
review checks remain part of initial delivery.

Derived-variant research now establishes complete-row override selection against
all 9,942 acquired native active input rows across CSIRO10/11/14. All 44 variants
have structurally consistent terminal/profile references; only the three stored
active selections have independent cache agreement. See
`evals/sincal/variant-inheritance.json`. This does not yet enable derived parsing.
As follow-up work, implement bounded effective-row materialization in the shared
storage crate, retain original row origins and source bytes, and test both electrical families
against these selections. Resolve and test the database deletion encoding first:
`Flag_Variant=0` is a cached inactive selection, not a tombstone, and absence of
a child override means inheritance. The documented scenario export of deletion
as out-of-order does not establish the original database encoding. Cycles,
missing parents, duplicate identities and unknown deletion forms must reject.

### PR 5: experimental fresh writer

Expansion is frozen for this delivery. Review and stabilize the existing subset;
additional transformer modes, external neutrals, general primitive shunts and
other unsupported equipment are follow-up work. Do not add them as merge gates.

Retain deterministic fresh schema-14.8 SQLite generation and packaging for
a declared static subset of both typed families. Source/line/load/transformer
support is the starting profile, not a promise to write every parsed network.
Select electrical validation by family; share only structural writer mechanics.
Define canonical native representations for auxiliary buses/switches rather
than trying to reconstruct native rows from arbitrary typed topology.

Exclude Access output, inherited variants, time-series output, diagrams and
undocumented desktop `.sin` generation initially. Require no source/template
database, copied results or host paths. Reject unsupported required physics
before creating a partial artifact; report permitted losses explicitly.
Acceptance: typed construction -> fresh write -> read; read -> change load ->
fresh write -> read; IR -> fresh write, for both families. Assert family,
electrical equivalence, deterministic bytes, integrity, diagnostics and
independent circuit checks. Keep opt-in experimental labeling; native
open/save/calculate remains an external acceptance gate. `FormatInfo.can_emit`
must not imply universal fresh output when only a separate experimental path exists.

### Local development and review handoff

Preserve the research worktree and use an isolated local checkout. Refresh
remote references, review base drift, extract commits and run tests on each
branch. Keep the original research/evidence available until all changes are
accounted for. Review untracked files as well as `git diff --stat`. Do not copy
large temporary models or tool environments into commits. Record branch heads,
parent commits, changed-file scopes, reproducible tests and corpus coverage in
a local handoff report. No empty branch or plan-only commit represents an
implemented reader/writer PR.

Return for user review only after the additional complete-distribution-case gate
passes and the five branches have reviewable diffs, supported-profile tables,
concise PR descriptions and a reproducible validation packet. Record the remaining
balanced selection-binding work honestly; do not let it displace whole-case
validation. No push, PR creation, merge or release is authorized by this roadmap
update. Native writer acceptance remains a separate external gate.
