# PSS SINCAL reader and writer proposal

Status: local reader implementation and validation in progress, 2026-10-07.
Both explicit public reader profiles are implemented. Five balanced cases and
one complete conductor-resolved native feeder have independent electrical
evidence, including C/Julia selections; broader corpus work remains unfinished.
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

The 2026-10-07 PR plan below supersedes earlier ordering that required a fresh
writer and full explicit-neutral/coupling coverage before public reader
integration. The reader targets broad corpus coverage; the fresh writer may
remain experimental. Unsupported electrical modes still fail explicitly.

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
- An unbalanced SINCAL case produces `MulticonductorNetwork` and preserves
  phase/neutral/source constraints through its existing consumers.
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

## Five-PR delivery plan: broad readers, experimental writer

Updated 2026-10-07 after review: dataset-specific work is grouped into one
balanced reader PR and one multiconductor reader PR. These five PRs supersede
the earlier nine-PR breakdown. Develop and verify commits and branches locally;
do not push branches or open PRs until the user reviews the result and
explicitly authorizes publication.

| PR | Local branch | Base | Scope |
| --- | --- | --- | --- |
| 1 | `codex/sincal-source-references` | Current `origin/main` | Shared source/reference model, matrices, PF, IR and C/Julia compatibility |
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

### Corpus ambition and acceptance accounting

| Corpus | Reader target | Evidence/data policy |
| --- | --- | --- |
| SimBench | Existing 15-node LV case, then available native LV/MV/HV and mixed-voltage cases representing distinct equipment modes | Balanced mappings compared with paired CSV. Inventory and pin additional archives before promising an exact count; retain ODbL/DbCL notices. |
| CSIRO 01–07 | Complete unbalanced reading of all seven feeders, led by 06 then 01, including applicable profiles/snapshots | Schema 11.5 Access through explicit optional import. CC BY 4.0, external corpus; resolve active native meanings rather than fitting historical results. |
| CSIRO 08–19 | Account for every selected family/variant, including inheritance in 10/11/14 and blank terminal fields in 13 | Target all 19 databases and all selectable variants. Three-phase connections and stored balanced results do not determine electrical family. |
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

Own all electrical mapping in `powerio-tx`. Include SimBench and other balanced
cases, schema adapters, applicable variants and public balanced integration in
this PR. Cover buses/voltage levels, positive-sequence lines, loads, external
sources, converter generation, transformer ratios/taps/shifts/losses, terminal
switching and service states. Preserve IDs/provenance; do not require inactive
zero-sequence fields for balanced inputs.

Use per-dataset commits/checklists inside this PR: initial SimBench mapping;
expanded SimBench modes; appropriate CSIRO balanced profiles via Access;
IEEE/student additional schemas; verified variant semantics. Unknown required
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
still outstanding; this does not complete PR 3.

Current balanced corpus progress: the initial public SimBench path is committed.
IEEE18, IEEE33 and the student study now map all 44/67/84 static equipment
records with schema-15.5/16.0 adapters and fresh independent checks below
4.5e-12 pu complex voltage. The student static state has zero demand; profile
snapshot coverage remains pending. Additional SimBench modes, applicable
Access cases, active profiles and inherited variants are still required work.

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

Own all conductor-resolved mapping in `powerio-dist`. Incorporate the existing
private component mappers, CSIRO 06 then 01 then 02–05/07, remaining applicable
CSIRO cases, LPC, profiles and variants in this PR. Preserve ordered phases,
phase pairs, unequal loads, neutral/reference behavior and open/inactive devices.
Resolve native load-star selection, active source impedance inputs, transformer
coil-to-conductor maps, autotransformer galvanic paths, core losses and nullable
tap modes. Neither missing inputs nor source values may be chosen to minimize
historical-result residuals. New generic model gaps require typed shared fixes,
not SINCAL-only physics hidden in metadata.

Implement per-phase absolute daily profiles/cyclic endpoints and selected
snapshots through existing time-series types where suitable, retaining one
network identity. Resolve effective variant inheritance/overrides/deletions,
with cycles and missing parents rejected. Cover the Representative 01 coupled
phase-pair counterexamples and Representative 13 blank terminal declarations
when version evidence establishes their meaning. Do not union stored variants.

Acceptance: complete original-MDB parsing of CSIRO 06 and 01 is the first
checkpoint; complete mappings of all seven unbalanced feeders and all other
applicable corpus cases are the goal. Report exact successful counts and every
remaining native-semantic blocker. Validate actual Rust reader output using
independent OpenDSS circuits/whole-network checks in addition to existing local
load/line equations. Isolate historical source/transformer disagreements and
missing native results; they cannot be quietly relabeled as passing or used
to calibrate inputs. Explicit-neutral/coupling modes absent from the corpus
remain unsupported until verified, without blocking cases that do not use them.

Complete two-family facade/CLI/binding integration here. Both authentic family
cases yield existing typed values; a phase-symmetric conductor model remains
multiconductor; ambiguity requires selection; unbalanced failure never retries
balanced. No implicit phase expansion/reduction. Source echo, edited modules
and IR-without-source have distinct tested behavior. Direct SQLite input and
optional Access import remain visibly different capabilities. Update diagnostic
and conversion baselines intentionally, then run full CI clippy and bindings.

Current distribution progress: the local branch includes verified schema-11.5
voltage-basis defaults, finite direct source zero-sequence mapping, explicit
phase-pair and single-phase-earth loads, selected absolute daily load snapshots,
and ideal connection lines represented by typed switches. CSIRO 06 maps 159/218
components (113 lines, 27 transformers, one source and 18 loads); its remaining
loads and transformers still reject. CSIRO 09 now maps all 688 native elements
at an explicitly selected daily snapshot. Five authentic snapshots and five
separately labelled unequal-delta-load stress cases agree with independently
constructed OpenDSS networks to below 0.000372 V across all 617 energized nodes.
Four native nodes behind open connections remain isolated. The source's finite
OpenDSS approximation is measured separately; no native SINCAL run is claimed.
The selected authentic loads are phase-symmetric, so asymmetric evidence comes
from the explicit stress cases, not an assertion about their original powers.
Independent component checks additionally cover 484 native loads at 2,420
selected snapshots and all 144 declared CSIRO 09 ideal connections. Four exactly
zero ordinary line primitives now also map to exact typed switches, preserving
ratings and terminal states. All 19 base variants remain audited, with one
complete native conductor-resolved parse at an explicit snapshot. Transformer topology now separates winding declarations
from operating-state resolution. Two verified Y0 profiles cover finite nominal
full windings and exact neutral-tap same-voltage connections; active controllers,
off-neutral same-voltage regulators, YN0 and D0 electrical circuits remain work.
The explicit-midnight audit maps 781/1033 CSIRO01 and 1162/2329 CSIRO02
components. These are component counts, not whole-feeder validation.
Nominal partial delta–delta windings now map through the documented coil
incidence, without renormalizing ratings by the number of installed coils.
All 518 newly mapped native devices across CSIRO02/04/06/07 pass independent
OpenDSS component checks. At 0h their mapped totals are now 1525/2329,
575/861, 163/218 and 283/456 respectively. No additional complete feeder is
claimed. Wye star/sequence semantics, partial mixed-winding transformers,
inconsistent core parameters and broader profiles/variants remain work. The public facade now
selects `sincal-multiconductor` explicitly, retaining original native bytes and
the existing network type. Native SQLite/archive routing is exercised through
CLI/Python/C; explicit Access/variant/snapshot options now use Rust, Python, C, Julia and
the CLI's summary/convert/serialize commands.
CSIRO09 public-facade results reproduce the independent electrical checks, MDB
echo and IR restoration. Generic admittance assembly succeeds with no omission
diagnostics. Generic PF-instance construction correctly refuses the full native
network's four unsourced isolated buses; connected synthetic input constructs
successfully. Python wheel and CLI checks preserve explicit snapshot choices
and original MDB echo. C/Julia selection plumbing now passes the complete C
and Julia suites, header/entry-point parity, and an external CSIRO09 comparison
at 0h/6h against the CLI, including typed IR restoration and original MDB echo.
Broader corpus coverage and the remainder of the integration packet remain work.
Schema-11.5 optional transformer defaults now cover the sparse CSIRO03 records
with explicit provenance; its context now advances to an unmapped ShuntReactor
conductor declaration. Its 297 limited-P/Q loads additionally require an exact native
voltage-reduction curve and a corresponding generic typed representation.
See `evals/sincal/` for exact scope and independently reproduced evidence.

Both LPC Access files are acquired into external typed records, with schema
12.8 admitted only through that explicit acquisition boundary. The conductor
reader preserves its distinct control layout and records legacy line-line
voltage defaults. The European LV case maps 260/262 components: all 205 lines
and 55 single-phase constant-power loads. Their actual Rust mappings agree
with independent OpenDSS line primitives and load currents. Unspecified line
thermal ratings stay absent rather than becoming invented limits.
The two remaining components, source and transformer, lack declared
zero-sequence input while automatic completion is disabled. No complete LPC
parse or LPC whole-network solver agreement is claimed. S1a maps 54 loads; its 54 DC
infeeds, 27 library-referenced lines, source and inconsistent transformer core
inputs still reject. Both files remain external research inputs with unresolved
redistribution rights; no version marker or electrical input is rewritten.

### PR 5: experimental fresh writer

Implement deterministic fresh schema-14.8 SQLite generation and packaging for
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
open/save/calculate remains E2. `FormatInfo.can_emit` must not imply universal
fresh output when only a separate experimental path exists.

### Local development and review handoff

Preserve the research worktree and use an isolated local checkout. Refresh
remote references, review base drift, extract commits and run tests on each
branch. Keep the original research/evidence available until all changes are
accounted for. Review untracked files as well as `git diff --stat`. Do not copy
large temporary models or tool environments into commits. Record branch heads,
parent commits, changed-file scopes, reproducible tests and corpus coverage in
a local handoff report. No empty branch or plan-only commit represents an
implemented reader/writer PR.

Once all five local branches meet their scopes, return for discussion with
that report. No push, PR creation, merge or release is authorized by this goal.
Reader-native semantics still gate claims for affected cases; SINCAL native
writer acceptance does not gate local reader delivery. Broader corpus searches
should serve named mapping gaps rather than delaying implementation.
