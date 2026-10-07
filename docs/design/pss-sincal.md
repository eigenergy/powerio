# PSS SINCAL reader and writer proposal

Status: local reader implementation and validation in progress, 2026-10-07.
Both explicit public reader profiles are implemented. Five balanced cases and
one complete conductor-resolved native feeder have independent electrical
evidence. Distribution selections include C/Julia; the new balanced Access
selections currently use Rust. The user-approved delivery decision below prioritizes
review of the supported profiles; broader corpus coverage is follow-up work.
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

The user-approved 2026-10-07 delivery decision below supersedes earlier
requirements to complete the broad corpus before opening the five PRs. Deliver
the verified reader profiles and the existing experimental writer subset first;
retain broader coverage as follow-up work. Unsupported electrical modes still
fail explicitly. This changes delivery scope, not the evidence for any case.

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
  feeder is the next coverage milestone, not an initial PR-opening gate.
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

### Approved delivery priorities and review gates

Decision agreed with the user on 2026-10-07: prepare the existing implementation
for review instead of treating corpus completion as a prerequisite. Keep the five
PRs and the two-backend contract. This section supersedes earlier broad-corpus
acceptance language in this proposal; outstanding cases are not relabeled as
supported. Updating this roadmap does not authorize pushing or opening PRs.

| Priority | Deliverable | Completion evidence |
| --- | --- | --- |
| 1 | Prepare all five local PRs for review | Reviewed base drift and diffs, coherent commits, per-PR supported-profile/limitation tables, concise descriptions and reproducible validation packet; discuss with the user before publication |
| 2 | Finish balanced Access/variant/snapshot selections in CLI, Python, C and Julia | Same explicit-family behavior as Rust and the existing distribution interfaces; native snapshot/echo/IR checks, wrong-family rejection and relevant binding regressions |
| 3 | Freeze writer expansion | Keep the existing experimental subset, explicit opt-in, unsupported-mode rejection and read/write/edit/IR tests; add no equipment modes to this delivery |
| 4 | Validate one additional authentic unbalanced feeder | Follow-up work on partial mixed-winding transformers and Wye loads affecting CSIRO05/06; independently check new circuits and the resulting whole network; account separately for source-input conflicts |
| 5 | Expand remaining corpus coverage | Follow-up variants, profiles, uncommon equipment and additional cases, selected to answer named gaps rather than to grow counts |

Opening gates: complete the final base/diff review and validation packet, then
obtain the user's publication authorization. No further dataset search or new
complete unbalanced feeder is required to open useful draft PRs. PRs 1–2 are
ready for final review preparation; PRs 3–5 can enter draft review with explicit
limits. The balanced selection-binding gap can remain visible during draft review
but must be closed before reader merge readiness is claimed.

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
Undocumented electrical meanings still gate support for the affected reader modes,
not delivery of independently verified profiles. Broader unbalanced support must
not be advertised on the strength of component counts or synthetic cases alone.

### Follow-up corpus ambition and evidence accounting

This inventory retains the broader ambition. Unfinished rows are follow-up
coverage, not additional acceptance requirements for the initial five PRs.

| Corpus | Reader target | Evidence/data policy |
| --- | --- | --- |
| SimBench | Existing 15-node LV case, then available native LV/MV/HV and mixed-voltage cases representing distinct equipment modes | Balanced mappings compared with paired CSV. Inventory and pin additional archives before promising an exact count; retain ODbL/DbCL notices. |
| CSIRO 01–07 | Complete unbalanced reading of all seven feeders; next target is one additional authentic case through CSIRO05/06 semantics, including applicable profiles/snapshots | Schema 11.5 Access through explicit optional import. CC BY 4.0, external corpus; resolve active native meanings rather than fitting historical results. |
| CSIRO 08–19 | Account for every selected family/variant, including inheritance in 10/11/14 and blank terminal fields in 13 | Follow-up target: all 19 databases and applicable selectable variants. Three-phase connections and stored balanced results do not determine electrical family. |
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

The broad reader corpus above remains a follow-up objective, not a prerequisite
for the initial five PRs or a promise that more coding alone can make every
original database pass. Native SINCAL execution is an external acceptance gate.
Missing electrical meanings and conflicting native inputs are tracked separately
because they also limit what can be implemented faithfully without that access.

Current end-to-end evidence is five balanced cases (including CSIRO19's seven
Access snapshots) and one conductor-resolved case, CSIRO09. Its original loads
are symmetric; five separate synthetic stress cases exercise asymmetry. None of
the seven priority unbalanced CSIRO01–07 feeders parses completely. Their
5,472/7,359 mapped components are component coverage, not a completion estimate.
The 1,887 first rejections comprise 798 coupled reduced-phase charging lines,
297 limited-P/Q loads, 271 partial mixed-winding transformers, 217 Wye loads,
ten other transformer/regulator modes, 291 inconsistent core-loss inputs and
three conflicting profile timestamps. Further failures may follow a resolved
first rejection.

Prioritize verified partial mixed-winding and Wye circuits for CSIRO05/06.
CSIRO05 has seven transformers and two profile conflicts left; CSIRO06 has
35 Wye loads, nine partial transformers and eleven nameplate conflicts. Resolve
input-conflict dispositions explicitly; never silently alter an original model
or call a corrected derivative an original-case pass. The original mandatory
CSIRO06/01 checkpoint is superseded by the approved supported-profile delivery
gates above; those original cases remain unsupported.
Investigations must address named gaps using new evidence; repeated rejected
hypotheses and broad dataset searches do not advance acceptance.

Prioritize public integration and review packets for supported profiles before
resuming these follow-up investigations. Balanced-compatible Access inputs belong
in the balanced PR with explicit family selection, not in the distribution completion
count. The experimental writer retains its declared subset and separate native
acceptance gate. Publication still requires the user's later permission.

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
complete unbalanced feeder. CLI/Python/C/Julia access to these new balanced
selection options remains an integration task.

### PR 4: distribution / unbalanced reader

Own all conductor-resolved mapping in `powerio-dist`. Deliver the existing verified
component profiles and public multiconductor integration. Preserve ordered phases,
phase pairs, unequal loads, supported neutral/reference behavior and open/inactive
devices. Keep verified absolute and common-factor daily snapshots. Unknown active
modes reject atomically; neither missing inputs nor source values may be chosen
to minimize historical-result residuals. New generic model gaps require typed
shared fixes, not SINCAL-only physics hidden in metadata.

Initial PR acceptance: all 688 CSIRO09 elements parse through the public facade
at the five verified snapshots; independent OpenDSS comparisons pass on the
energized network and isolated nodes remain faithfully represented. Five labelled
asymmetric stress cases and the existing independently checked component circuits
establish the declared unbalanced profile. Preserve source echo, IR fidelity,
explicit selection, unsupported-mode diagnostics and existing consumer behavior.
Report that native CSIRO09 powers are symmetric and that zero of the seven
priority genuinely unbalanced original feeders currently parses completely.
These limits must appear in the PR description and supported-profile table.

Follow-up acceptance: one additional authentic mixed-phase feeder, prioritizing
CSIRO05/06 partial mixed-winding and Wye semantics, followed by broader coverage.
All seven priority feeders, remaining applicable CSIRO/LPC cases, additional
profiles, inherited variants/deletions, Representative 01 coupled charging and
Representative 13 blank terminals stay on that backlog. Resolve meanings from
evidence, test actual Rust output independently, never union stored variants,
and distinguish source-input conflicts from mapper gaps. Full explicit-neutral
or coupling modes remain unsupported until verified. None of these expansions
is a gate to opening the initial distribution PR for its declared profile.

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
| 12 | 214 / 215 | No |
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
are not counted as missing native data. CSIRO09 is the one complete conductor-resolved
native feeder. Five authentic snapshots and five labelled unequal-delta stress
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
CSIRO12 now maps all lines, loads and its capacitor; its source's NULL controls
remain unresolved. A balanced stored source result confirms its specified
positive-sequence voltage but does not establish the unknown control field's
conductor-domain behavior. No source-control default is inferred from that result.

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
validation. Broader corpus coverage is follow-up work; the final integration
and review packet remains part of the initial delivery.

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

Return for discussion when the five branches have reviewable diffs, tables of
supported profiles, concise PR descriptions and a reproducible validation packet.
List the balanced selection-binding task explicitly if draft review starts
before it is finished. Do not wait for follow-up corpus completion or expand
the writer to make the review packet. No push, PR creation, merge or release is
authorized by this roadmap update. Native semantics still gate claims for
affected cases; native writer acceptance remains a separate external gate.
