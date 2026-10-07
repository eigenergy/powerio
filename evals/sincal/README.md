# SINCAL validation evidence

Use the [user guide](../../docs/src/sincal.md) for API usage and supported
profiles, and the [review packet](../../docs/design/pss-sincal-review.md) for
PR scope. This directory contains reproducible acquisition and electrical
checks, source hashes, licensing provenance and measured results. PowerIO has
no solver; numerical checks use independent downstream calculations.

## Evidence to review first

| Evidence | What it establishes |
| --- | --- |
| [Final integration](priority-verification.json) | CI stages, Python/Julia checks, fresh numerical reruns and explicit exclusions |
| [Trial workflows](trial-workflows.json) | Per-case echo, IR, PF preparation and emission/reparse; not export numerical equivalence |
| [Balanced SimBench](balanced-simbench.json), [other SQLite cases](balanced-external.json), [CSIRO19](balanced-csiro19.json) | Five complete balanced cases, with case-specific independent references |
| [CSIRO09 public reader](csiro09-public.json) | All 688 elements at five native snapshots, plus five synthetic asymmetric stress cases |
| [Truong12](truong12-public.json) | Complete native asymmetric 12-bus case |
| [CSIRO12](csiro12-compatibility.json) | All 215 elements under explicit compatibility assumptions; native NULL semantics remain unverified |
| [Historical corpus summary](corpus-history.md) | Earlier partial-coverage audits; not current reader acceptance results |
| [Source catalog](research-catalog.md) and [CSIRO inventory](csiro-inventory.json) | Source URLs, licensing disposition and identities; not support claims |

Retain the small licensed SimBench fixture and synthetic unit tests. External
MDB/SQLite models, paper PDFs, decoded records and solver environments are not
fixtures. No license is inferred from a repository's code license. Source
rights and notices are recorded in the catalog and
[fixture README](../../tests/data/sincal/README.md).

Run acquisition regressions without external model downloads:

```sh
python3 -m unittest discover -s evals/sincal -v
cargo test -p powerio-sincal
```

Component checks remain available in the adjacent scripts; their original
commands are indexed in [component checks](component-checks.md). Historical
investigations explain unsupported modes but do not enlarge the advertised
profile. The full pre-cleanup narrative is recoverable at Git commit `1e95be55`.

## Explicit Access acquisition

Install MDB Tools separately and invoke the adapter explicitly; ordinary Rust
library parsing does not launch it. Python 3.11+ and MDB Tools 1.0.1 were tested
on macOS. Other tool releases/platforms require their own verification.

```sh
python3 evals/sincal/import_access.py /external/database.mdb /tmp/records.json \
  --tool-directory /path/to/mdbtools/bin
cargo run -p powerio-sincal --example inspect_records -- /tmp/records.json 1
```

The helper copies the original into private temporary storage, hashes that
snapshot, and collects declared columns plus typed JSON rows. It restores
omitted NULLs from schema declarations without treating missing columns as
NULL or text zero as a number. No exported SQL is executed. By default it
acquires 21 requested input/profile tables where present; use repeated
`--table` arguments to choose another explicit set. It lists excluded tables
and absent requested tables separately. Blob/object cells and unknown schema
declaration syntax fail explicitly. An electrical mapper must request every
table its active input modes require; successful structural decoding alone
cannot justify omitting a referenced type/library table.

Limits are 256 MiB original input, 64 MiB record/tool output, 1 MiB per text
field, 100,000 rows per table, four million acquired cells and bounded tool
runtime. Original MDB bytes remain external; record documents are internal
acquisition artifacts, not native SQLite exports or lossless MDB archives.
The tool never overwrites a destination or emits partial output on validation
failure. Keep the original MDB for native source echo and provenance.

Reproduce all 19 required corpus cases, with a fresh external record directory:

```sh
cargo build -p powerio-sincal --example inspect_records
python3 evals/sincal/check_access_corpus.py /external/csiro \
  /tmp/sincal-acquired /tmp/sincal-acquisition-report.json \
  --inspector target/debug/examples/inspect_records \
  --tool-directory /path/to/mdbtools/bin
```

The source directory can contain the publisher's `DataRelease/...` layout or
the research filenames `csiro-representative01.mdb` through `...19.mdb`.
Missing cases, changed source hashes, mismatched counts or Rust inspection
failures fail the run. The checks compare acquired row counts with the pinned
inventory, then compare Rust's selected-variant identities with independently
selected records. The [acquisition report](access-acquisition.json) records all 19 passing
base-variant snapshots; derived variants and
electrical mappings remain outside this claim. On this macOS run the maximum
Rust inspector child RSS across the 19 cases was about 80.2 MB. Memory and
execution time are observations for this corpus, not portability guarantees.

## Balanced reader: first complete authentic case

The Rust `sincal_balanced` example explicitly selects positive sequence and
exports the actual typed reader output for validation. It is not automatic
family detection. The facade accepts the same case with the explicit
`sincal-balanced` format argument; see [the user guide](../../docs/src/sincal.md). The current schema adapter accepts
14.8 and maps all 32 equipment records of the licensed 15-node SimBench case.
Unknown active modes fail with native table, record and field context.

```sh
cargo test -p powerio-tx --lib format::sincal
cargo build -p powerio-tx --example sincal_balanced
python evals/sincal/check_balanced_simbench.py target/debug/examples/sincal_balanced /tmp/simbench-check.json
```

The independent check requires `numpy` and `pandapower==3.2.2`. It compares
Rust network parameters with the original CSV files, constructs a pandapower
network solely from those CSV inputs, and compares a fresh pi-transformer
load-flow solution with independently solved nodal equations for the mapped
network. Stored rounded CSV results are reported separately and never supply
input physics. Temporary derived cases additionally test nonzero fixed taps
on each winding against independently configured pandapower transformers.
See [the recorded report](balanced-simbench.json).

The mapper uses a declared 100 MVA conversion base, native bus IDs and stable
element identities. Open terminals become explicit switches on auxiliary buses;
inactive equipment is retained. Absent generator capability limits remain
unbounded, rather than becoming zero capability. Fault impedance and historical
result tables do not affect load-flow mapping. Voltage-dependent loads are
normalized to demand at the nominal bus voltage.

Still required for the complete transmission PR: additional SimBench modes
and corpus cases, verified Access/other-schema adapters, profile/variant
selection and expanded public input options. Registered diagnostics,
facade/CLI/C/Python parsing, retained binary-source echo, edited-module refusal
and IR-without-native-source behavior now have focused integration tests. This first mapped case does not establish
those capabilities, unbalanced reading, or native SINCAL writer acceptance.

## Additional balanced SQLite cases

`check_balanced_external.py` requires all three original databases identified in
[research-catalog.md](research-catalog.md), supplied externally with exact hashes.
It invokes the actual Rust reader, checks every native equipment identity,
terminal topology, service state and load power, builds pandapower independently
from native physical inputs, and compares a fresh calculation with separate
nodal equations over the mapped network. Capacitor admittance and closed
terminal switches participate in those equations. No historical result is used
as input. Missing or mismatched files fail this check.

```sh
cargo build -p powerio-tx --example sincal_balanced
python3 evals/sincal/check_balanced_external.py \
  target/debug/examples/sincal_balanced /external/sqlite-cases /tmp/balanced-external.json
```

The compact measured report is [balanced-external.json](balanced-external.json).
All 44/67/84 native elements map for IEEE18/IEEE33/student respectively. Their
fresh voltage errors are below 4.5e-12 pu. IEEE18 has ten fixed capacitor banks;
IEEE18/33 each retain one explicit closed terminal switch. Schema 15.5 and 16.0
use the same checked electrical columns as the admitted 14.8 subset; newly
observed columns are recorded in the catalog, not guessed from version numbers.

The student model's static P/Q values are zero and saved profile enables are
off. Its 960 historical node rows represent different states. Historical
comparison is explicitly non-gating and is not presented as aligned native
validation. The two IEEE comparisons are also reported separately from fresh
independent checking. Broader profiles and variants are follow-up work.

## Balanced schema-11.5 Access snapshots

`check_balanced_access.py` checks the original external CSIRO19 MDB through the
public Rust facade example `sincal_balanced_access`. It maps all 33 components
at seven times and compares actual mapped output with independently constructed
pandapower inputs, including native topology, service states, daily interpolation,
power factors, charging and source voltage. The report is `balanced-csiro19.json`.
Its maximum complex voltage difference is below 1.98e-11 pu; four deliberate
parameter mutations are detected. Each invocation also checks byte-exact MDB echo,
IR typed-value preservation, and wrong-family/missing-time/invalid-variant/changed
MDB refusal. Native SINCAL acceptance and further unbalanced cases are not claimed.

Acquire the 21 default tables plus `NetworkGroup` and `NetworkGroupTrans`, as
interchange controls must be visible. The report pins both original and acquired
hashes; acquisition records are supplied by the caller, not independently attested.
The CC BY 4.0 original stays external; no new model fixture is vendored.

```sh
cargo build -p powerio --example sincal_balanced_access
python evals/sincal/check_balanced_access.py \
  target/debug/examples/sincal_balanced_access \
  /external/csiro-representative19.mdb /external/csiro19-balanced-records.json \
  evals/sincal/balanced-csiro19.json
```

## Complete CSIRO09 public reader

The public oracle checks all 688 native elements at five daily snapshots and
five separately labelled synthetic asymmetric stress cases. The native loads
are symmetric. Four isolated native nodes remain in the typed network; generic
PF preparation refuses the unsourced islands. Ordinary DSS/PMD/BMOPF export
refuses terminal-referenced sources. These refusals are expected, not full
conversion successes.

The checker solves the actual typed network and independently constructs
OpenDSS from native input tables. The report records tolerances, ideal-source
approximation, hashes and negative controls. OpenDSS is not fed a PowerIO DSS
export. See [the detailed public report](csiro09-public.json) and the latest
[numerical rerun](priority-verification.json).

```sh
cargo build -p powerio --example sincal_public
POWERIO_MAX_PRIMARY_BYTES=69181440 python3 evals/sincal/check_csiro09_corpus.py \
  /external/representative09.json /external/csiro-representative09.mdb \
  target/debug/examples/sincal_public /tmp/csiro09-public.json --public-reader
```

Use an environment with NumPy, SciPy and OpenDSSDirect.py; tool versions are
pinned by the recorded reports. Keep original MDBs and hash-matching acquired
records outside the repository. No native SINCAL execution is claimed.

### Complete asymmetric native 12-bus case (2026-10-08)

[`truong12-public.json`](truong12-public.json) records a new complete public
parse of [Truong812001/Unbalance-Power-Flow](https://github.com/Truong812001/Unbalance-Power-Flow/tree/1459d2be39b3c00aac195b1d6c01bcde16ee357f),
`Update_finalV1_database/12bus/12busbc_files/database.db`. Pin revision and
SHA-256 as recorded in the report. The original is 2,363,392 bytes and has no
published redistribution license; keep it outside the repository. No fixture
or upstream solver code is incorporated.

The schema-15.0 variant-1 static model has 12 native nodes, 11 sequence lines,
33 unequal single-phase constant-PQ loads and one ideal source. All 45 elements
map through `sincal-multiconductor`; original SQLite echo, typed IR round-trip,
generic matrix construction without omissions and generic PF-instance construction
pass. Structural admission of 15.0 does not enable the balanced electrical
adapter: that boundary has its own rejection regression.

The native calculation explicitly selects `Flag_LFZ0=2` (Input Data, April 2014,
printed p.219): supply missing zero sequence from positive sequence. The reader
implements that selected policy for the verified line/ideal-source profile and
records `zero_sequence_from_positive` provenance. Explicit component declarations
win. This does not turn arbitrary NULLs into defaults, extend transformer or
finite-source support, or change native global-input-only cases into this mode.

```sh
cargo build -p powerio --example sincal_native_multiconductor
# Use an environment with NumPy and OpenDSSDirect.py installed.
python3 evals/sincal/validate_truong12.py /external/12busbc_files/database.db \
  target/debug/examples/sincal_native_multiconductor /tmp/truong12-public.json
```

The wrapper calls the public facade example and solves its actual typed output
with independent dense MNA. OpenDSS is built separately from native input tables.
All 36 complex voltages and 66 line-terminal currents and powers are compared;
tolerances are 0.001 V, 0.001 A and 0.1 VA, with typed KCL residual below 1e-6 A.
The report records measured maxima, unequal phase demand totals, tool versions,
source/checker/reader hashes and the ideal-source approximation. Removing a load,
doubling a line impedance or moving a load to the wrong phase must fail.

Stored ULF voltages agree within 0.087 V after a fixed +30-degree reference
rotation. Their separate tolerance is the native VDN=.01% of nominal phase
voltage, not a fitted threshold. Historical results do not attest the current
input revision; fresh independent solver agreement is the primary evidence.
No native SINCAL execution is claimed. This is a new authentic asymmetric success,
but its 12-bus size does **not** satisfy the requested larger-feeder review gate.

## Paper-based electrical cross-check (2026-10-08)

The [IEEE18 paper report](ieee18-paper.json) adds a result-dump-independent
check of the existing balanced case. It matches the reader's complete line,
load and shunt inputs against Table A.I of
[Milovanović et al., DOI 10.7251/IJEEC1801011M](https://doi.org/10.7251/IJEEC1801011M),
then compares a fresh solve of the typed model with the fundamental-frequency
loss bars in Figure 8. Both source files are hash-pinned and remain external.

```sh
cargo build -p powerio-tx --example sincal_balanced
# Research environment: numpy, pandapower, pdfplumber==0.11.9
python3 evals/sincal/check_ieee18_paper.py /external/IEEE18.db \
  /external/milovanovic2018.pdf target/debug/examples/sincal_balanced \
  /tmp/ieee18-paper.json
```

All 17 plotted groups pass a 0.15 kW figure-resolution tolerance; the largest
observed difference is 0.00156 kW. A 10% load perturbation fails by 8.43 kW.
Vector extraction avoids manual pixel estimates but does not turn a plot into
an exact numerical oracle. The two parallel 25–26 lines form one plotted group.
This validates balanced fundamental-frequency behavior only. It neither adds
an unbalanced case nor establishes fresh SINCAL execution or harmonic support.
No PDF, extracted reference table/plot or native model is vendored.

## Experimental CSIRO12 compatibility and paper audit (2026-10-08)

The user authorized explicitly labelled parsing approximations for trial use.
The `assume_inactive_source_controls` option (Rust, CLI, Python, C and Julia)
admits five NULL
schema-11.5 source controls as inactive: `Flag_LfLimit`, `Flag_LfCtrl`,
`Flag_Qctrl`, `Flag_Macro`, `Kr`. Siemens' April 2014 Database Description,
Infeeder table pp.15–16, lists zero defaults for four of these; it does not
establish SQL NULL behavior, and its Infeeder table does not define `Flag_LfCtrl`.
All five interpretations are therefore explicit compatibility assumptions,
never an unconditional manual-backed claim. Nonzero controls, missing columns,
newer-schema NULLs and missing required electrical values still reject.

[CSIRO12 compatibility evidence](csiro12-compatibility.json) covers the original
CC BY 4.0 MDB with unmodified, hash-pinned acquired tables. All 215 components
are accounted for: 187 lines/connections, 26 loads, one capacitor and one source.
Of 191 native nodes, 188 have equipment conductors and three unused records
(35, 153, 173) remain in `extras.sincal_unconnected_nodes`. No electrical island
is silently removed or supplied with invented phases.

Five native snapshots (0, 0.25, 12, 23.75, 24 h) pass public facade parsing,
byte-exact MDB echo, IR value preservation including assumption metadata,
generic matrix construction and generic PF-instance construction. Independently
constructed native-input OpenDSS circuits agree with sparse MNA solves of the
actual typed output. Five additional unequal-delta-load stress cases are
**synthetic**. Across all ten checks, 564 native complex phase voltages and
918 line terminal currents/powers per check give maxima of 0.000370 V,
8.27e-8 A and 0.00309 VA. Current/power comparison matches the small finite
OpenDSS source impedance; the report separates that approximation's voltage
impact. Four negative controls detect missing loads/capacitor, missing mutual
impedance and incorrect load factors. Native loads are symmetric: this is one
additional substantial experimental distribution case, but does not independently
validate a published asymmetric operating point or prove native NULL semantics.

```sh
cargo build -p powerio --example sincal_public
python3 evals/sincal/check_csiro12_compatibility.py \
  /external/representative12-complete-records.json \
  /external/csiro-representative12.mdb target/debug/examples/sincal_public \
  /tmp/csiro12-compatibility.json
```

Use acquisition including `ShuntCondensator`; the earlier acquisition manifest
omitted that table. The checker pins the complete record digest separately.
The native model remains external; no additional native fixture is vendored.
The existing CSIRO09 five-snapshot/five-stress public oracle was rerun after
shared harness changes and still passes (maximum 0.000372 V).

[The unbalanced-paper audit](unbalanced-paper-constraints.json) is deliberately
weaker evidence: output reconstruction, not a reproduced power flow. For
[Arif et al. 2013](https://file.scirp.org/Html/14-6401233_32197.htm), Table 5 supplies
phase magnitudes and angle differences. These imply approximately 1.079% and
1.444% negative/positive sequence voltage ratios in the PV/storage scenarios;
those ratios are derived here, not reported benchmarks. Figure 4 and the text do
not supply the line/neutral impedances and full transformer equivalent needed
for independent network reconstruction.

[Vinayagam et al. 2015](https://www.atlantis-press.com/article/25841476.pdf) reports
about 1.48% VUF. Interpreting Figure 12's magnitudes as phase voltages with exact
120-degree spacing gives 0.9954%; interpreting them as a closed line-line set
gives 1.9913%. Neither assumption reproduces 1.48%. Missing phase angles and
precise voltage basis/state prevent a unique reconstruction; this is not a
claim that the paper is wrong. No model parameters were fitted to reported outputs.

```sh
python3 evals/sincal/check_unbalanced_paper_constraints.py /tmp/paper-audit.json
```

Arif is CC BY; Vinayagam's article is CC BY-NC 4.0. No paper PDF, figure or
native model was added as a fixture. Only attributed numerical observations,
our calculations and their explicit limits are recorded.

## Binding and trial workflow verification (2026-10-08)

[The trial report](trial-workflows.json) and [integration packet](priority-verification.json)
record the final run. `check_trial_workflows.py` uses an installed wheel and external native sources.
It tests source echo, typed IR, PF preparation and ordinary target emission
separately. It does not solve the exported targets; numerical evidence remains
in the independent native-input oracles above. Both families use explicit
selection objects. The distribution compatibility flag is available in Python,
C and Julia as well as Rust/CLI; balanced Access/snapshot selections are now
available across those same entry points.

```sh
POWERIO_MAX_PRIMARY_BYTES=69181440 python3 evals/sincal/check_trial_workflows.py \
  /external/csiro-representative19.mdb /external/csiro19-balanced-records.json \
  /external/csiro-representative09.mdb /external/representative09.json \
  /external/csiro-representative12.mdb /external/representative12-with-shunts.json \
  /external/truong12/database.db /tmp/trial-workflows.json
```

The checked noon snapshots yield these distinct outcomes:

| Case | Echo and typed IR | PF instance | Ordinary emission/reparse |
| --- | --- | --- | --- |
| CSIRO19 balanced | Pass | Pass | MATPOWER pass, with loss diagnostics |
| Truong12 multiconductor | Pass | Pass | DSS, PMD and BMOPF pass, with loss diagnostics |
| CSIRO09 multiconductor | Pass | Unsourced-island refusal | Reference-terminal source refusal |
| CSIRO12 multiconductor, compatibility opt-in | Pass | Pass | Reference-terminal source refusal |

The per-case matrix/oracle commands are listed above and in component-checks.md. In particular,
CSIRO09/12's OpenDSS references are generated independently from native tables;
they are not output from the ordinary DSS writer. No native SINCAL files were
added to fixtures by this integration work. Source inventory findings enumerate
retained tables and selected source-only fields, and explicitly mark excluded
Access tables as having unknown row counts; the inventory is not a complete
field-level schema audit.

`check_julia_trial_workflows.jl` checks three CSIRO19 balanced snapshots and
CSIRO12's strict refusal/opt-in success against the same original inputs. Point
`POWERIO_CAPI` at the current library and `--project` at the matching companion:

```sh
POWERIO_CAPI=/path/to/libpowerio_capi.dylib \
  julia --project=/path/to/PowerIO.jl evals/sincal/check_julia_trial_workflows.jl \
  /external/csiro-representative19.mdb /external/csiro19-balanced-records.json \
  /external/csiro-representative12.mdb /external/representative12-with-shunts.json
```

Use the platform's corresponding `.so` or `.dll` on Linux or Windows. These
are reader/binding checks; fresh SINCAL desktop acceptance remains external.

## Experimental writer branch

The local `codex/sincal-experimental-writer` branch starts with shared source-free
SQLite construction and deterministic candidate packaging in `powerio-sincal`.
The tests create original small schemas at runtime. They check byte determinism,
parameterized text (including quotes and newlines), strict integer/real/text
cells, NULL preservation, bad identity/reference rejection, unsafe archive
names and rejection of results/views/additional variants. They establish
storage construction only, not a complete network writer or native acceptance.

The balanced backend now authors fresh schema-14.8 rows for an explicitly
selected static positive-sequence profile: ideal reference sources, fixed-PQ
converters, PQ/current/impedance loads, lines and fixed two-winding transformers.
It uses physical native units, preserves non-100-MVA electrical quantities and
service states, and checks recovered electrical values before returning bytes.
Unknown active controls, asymmetric branch shunts, PV buses and components
outside the declared subset fail atomically. Unsupported metadata, capability
limits and stored source/branch results produce explicit loss diagnostics.
The staging API is hidden in `powerio-tx`; universal fresh emission stays off.

Runtime-generated tests cover construction, deterministic bytes/packaging,
source-free serde restoration and edits, numerical-loss rejection, fixed
transformer loss/ratio/rotation and service states. The existing licensed
SimBench fixture also passes fresh write/read without a template. No additional
native model fixtures were added.

```sh
cargo test -p powerio-tx --lib sincal::write_tests
cargo build -p powerio-tx --example sincal_balanced
python3 evals/sincal/check_balanced_simbench.py target/debug/examples/sincal_balanced \
  /tmp/writer-balanced-simbench.json --fresh
```

`writer-balanced-simbench.json` records an independent comparison of the actual
fresh-write/read result with publisher CSV inputs and pandapower 3.2.2, including
separate HV/LV fixed-tap perturbations. Baseline complex voltage error is below
6.3e-14 pu; tap cases are below 5.1e-11 pu (2e-8 pu acceptance tolerance).
These are electrical interoperability checks, not native desktop acceptance.

An integration test restores a balanced module through the real PowerIO IR,
edits its load, invokes the candidate backend and parses fresh bytes through
the public balanced reader. It separately verifies byte-exact echo of the new
output; the old native project cannot enter the backend.

Complete multiconductor circuit coverage and broader writer subset coverage
remain unfinished. The explicit Rust facade option is described below. The
multiconductor staging implementation and IR evidence are described below. Canonical balanced transformer rows do not claim conductor/neutral
semantics, and must not be used as an implicit multiconductor writer.

### Multiconductor candidate writer

The family-local hidden staging API now constructs fresh schema-14.8 inputs
from `MulticonductorNetwork` for ideal positive-sequence sources (earth referenced
or an isolated floating star), phase-resolved static loads, and sequence-
representable lines. Unequal Wye and delta branch powers, phase pairs and single
phases remain unbalanced. Loads split into native one-branch elements, preserving
individual nominal voltages and PQ/current/impedance behavior. No balanced
backend is called. Line matrices must fit the declared native sequence profile;
unsupported conductor matrices are rejected, never approximated as balanced.

`ExperimentalMulticonductorOptions.nominal_ll_volts` explicitly supplies a
positive line-line voltage for every typed bus; the distribution model does not
have a bus nominal-voltage field. No operating source voltage, bound or `extras`
value is silently reinterpreted as nominal. Native node IDs are assigned
canonically and returned as a bus-ID map. Closed switches collapse only when
that does not connect an additional conductor. External neutral paths, open
switches and attached floating references require further native circuit work.
Native readback introduces explicit device-local buses/switches as needed.
The candidate explicitly writes `Flag_LFmet=8`, `Flag_UsymElm=3` and
`Flag_DIType=0`: phase-domain unbalanced calculation, asymmetric elements
retained, no automatic method fallback. The balanced candidate separately
writes `2/1/0`. These documented fields are present in the authentic 14.8
catalog; their construction is tested, but native execution is still unverified.

```sh
cargo test -p powerio-dist --lib sincal::write_tests
POWERIO_SINCAL_WRITER_ORACLE_DIR=/tmp/dist-writer-oracle cargo test -p powerio-dist \
  --lib sincal::write_tests::export_writer_oracle -- --ignored
python3 evals/sincal/check_multiconductor_writer.py /tmp/dist-writer-oracle \
  --report /tmp/writer-multiconductor.json
cargo test -p powerio --test sincal
```

`writer-multiconductor.json` records two original synthetic unequal-load
circuits, with earth-referenced and floating-star sources. Dense MNA evaluates
the typed input and actual fresh-write/read result, independently of PowerIO
matrix code. A separate OpenDSS circuit uses the original electrical inputs.
The fresh-readback voltage difference is below 4e-13 V; OpenDSS differs by less
than 1.9e-5 V with an explicit 1e-6-ohm source-impedance approximation (1e-3 V
acceptance tolerance). Grounding the floating source is an intentional
counterexample that the oracle must detect. Neither input nor reference uses
stored native results. Tests also cover actual IR restoration and editing
while preserving the floating reference, malformed typed vectors and unsupported
circuit rejection, deterministic output, and rewriting through closed device
switches. Test circuits are generated at runtime; no native fixtures were added.

This is incremental PR5 implementation. Selected typed transformer output is
implemented below. Other shunt/generator output,
external-neutral and general shunt authoring remain unfinished. Canonical open
phase switches now have the verified writer profile described below. The Rust experimental
facade option is now implemented as described below. The complete authentic unbalanced reader corpus targets in
PR4 are unchanged; these writer tests do not establish their completion. Native
SINCAL desktop acceptance remains a separate external gate.


### Multiconductor transformer writer evidence

The candidate now writes finite, three-phase, two-winding delta/delta and
solidly grounded delta/Wye or Wye/delta transformers with equal VA ratings,
nonnegative leakage resistance/reactance, and positive fixed taps. Winding
resistances combine on their common base. Effective winding voltages include
both taps, preserving the terminal-referred impedance as well as the turns
ratio. Nominal bus voltage remains independently supplied by the caller.

The generic winding convention follows the existing OpenDSS writer: mixed
windings default to ANSI/lag; `leadlag` selects lead/Euro when present. The
higher rated winding determines the direction, including when the primary
winding is the lower-voltage side. This follows the
[OpenDSS transformer property definition](https://dss-extensions.org/dss-format/Transformer.html).
Nonzero core/anti-float/neutral physics and unmapped transformer extras fail
explicitly. Wye/Wye, floating Wye stars, ideal zero-leakage transformers,
autotransformers and partial windings still require separate representations.

Every authored transformer is read back and checked against a direct coil-
incidence primitive, independently of the reader's symmetrical-component
construction. This checks all six phase coordinates and both terminal-switch
connections, including negative- and zero-sequence behavior. The generic
PowerIO matrix builder's ideal-Wye transformer support is not used to validate
these finite transformer circuits.

```sh
cargo test -p powerio-dist --lib sincal::write_transformer_tests
POWERIO_SINCAL_WRITER_ORACLE_DIR=/tmp/transformer-writer-oracle cargo test \
  -p powerio-dist --lib sincal::write_transformer_tests::export_transformer_writer_oracle -- --ignored
python3 evals/sincal/check_transformer_writer.py /tmp/transformer-writer-oracle \
  --report /tmp/writer-transformer.json
```

`writer-transformer.json` records 24 original synthetic combinations of three
connection arrangements, step-up/down winding order, lead/lag rotation and
fixed taps. The oracle constructs OpenDSS from the original typed winding
values, extracts its full six-phase YPrim, and compares the actual Rust
fresh-write/read result. Maximum relative primitive error is 6.21e-16.
Unequal-load solves with positive, negative and zero-sequence excitation agree
within 1.18e-10 V. Reversed-rotation counterexamples must fail the mixed-winding
cases. All 24 expected files are required; missing cases fail the harness.

This establishes typed construction and source-free serde restoration/editing
for the selected transformer subset. The report is small derived validation
metadata; no native model fixtures were added.

The writer also recognizes equivalent six-conductor transformer primitives
from native reading. It requires an exclusive auxiliary bus and exactly two
closed three-phase terminal switches. Switch direction and coordinate names
do not supply winding physics. Candidate scalar parameters come from the
matrix, and acceptance compares every conductor entry in each port block,
including negative/zero sequence and sequence coupling. Incompatible edits,
core terms, open ports and additional attached equipment are rejected. Native
provenance is never used to reconstruct electrical values.

Primitive shunts have no recoverable VA nameplate or split of winding losses.
The candidate reports its canonical 1 MVA parameter base and equal resistance
allocation explicitly; these choices reproduce the circuit and do not claim
original thermal ratings. Eliminated auxiliary buses span two voltage levels,
so their nominal-voltage option entries may be omitted and they have no single
entry in the returned native node-ID map. All remaining bus levels are explicit.

All 24 oracle cases now also check read/write/read and edited-primitives against
OpenDSS. Multiplying typed admittance by 1.2 is independently checked by reducing
the original OpenDSS winding resistances/reactance by that factor. Maximum
relative primitive error is 6.00e-16 for rewriting and 1.17e-15 after editing;
the edited unequal-load voltage difference is below 1.31e-10 V. A facade test
separately serializes the reader-produced primitive through real PowerIO IR,
removes provenance, edits the matrices and verifies fresh write/read. The
ordinary family dispatch and source-echo API remain unchanged.


### Explicit experimental facade option

The Rust facade now offers `emit_with_options(module, format, options, destination)`.
`EmitOptions::default()` is equivalent to `emit`. Setting `sincal_experimental`
to `Some(SincalExperimentalOptions)` requests a fresh candidate and bypasses
retained-source echo. Both typed and dynamic modules dispatch through their
owning electrical backend; the generic `sincal` output target accepts either
network family, while `sincal-balanced` rejects multiconductor values. Other
value types and options used with other formats are rejected before writing.

SQLite versus candidate archive packaging is an explicit container option.
Distribution nominal line-line voltages are mandatory for every non-eliminated
bus; balanced values reject that map because their bus nominal kV is already
typed. Both emitters finish validation and packaging before committing output.
Existing destination files retain the core destination collision behavior.
The result is always `Fidelity::Canonical`, with experimental and loss warnings.
The ordinary API still echoes unchanged native sources exactly and refuses
fresh native output; format metadata intentionally remains `can_emit=false`.

```sh
cargo test -p powerio --test sincal_emit --test sincal --test crate_graph
```

Public tests cover both families and containers, deterministic output, actual
IR edits, typed modules, non-network rejection, explicit family mismatch,
missing/inapplicable voltage options, invalid archive names, no partial output
or overwrite, preserved source echo, and unchanged default MATPOWER emission.
The facade depends on the shared SINCAL crate only for model-neutral packaging;
backend dependencies remain independent. CLI/Python/C experimental writer
options remain separate work.

### Experimental open-switch output

Open switches now write for canonical phase selections L1/L2/L3/L12/L23/L31/L123.
Their endpoints stay separate even when they carry unequal declared voltages.
The canonical native representation is a one-metre ordinary line with exactly
zero series and shunt parameters and an explicitly open first terminal. The
reader represents it with an auxiliary bus, a closed ideal connection and an
open terminal switch; it introduces no finite impedance or charging. A supplied
uniform positive ampacity is retained on the ideal connection. No ampacity is
invented when absent. Closed-switch collapse, including collapse of a readback
carrier on a subsequent write, still reports omitted closed-switch ratings.
Neutral switching, conductor permutations, nonuniform ampacities and unsafe
partial closed-switch collapse remain explicit errors.

Original synthetic tests cover all seven phase selections, optional ratings,
deterministic bytes, read/edit/write, malformed limits, and source-free IR
restoration followed by fresh SQLite/archive emission. `check_open_switch_writer.py`
compares the actual original and fresh-readback typed circuits through independent
dense MNA, then compares them with OpenDSS on the physical feeder with its tie
open. Maximum fresh-readback voltage error is 6.36e-14 V; OpenDSS error is
1.89e-5 V with the documented 1e-6-ohm ideal-source approximation. Closing each
recovered tie changes voltage by at least 0.117 V. `writer-open-switches.json`
records the seven cases, negative controls, tool versions and hashes. This
establishes electrical writer/readback equivalence, not native SINCAL acceptance.

```sh
POWERIO_SINCAL_OPEN_SWITCH_EXPORT=/tmp/open-switch-circuits \
  cargo test -p powerio-dist --lib export_open_switch_writer_oracle -- --ignored
python3 evals/sincal/check_open_switch_writer.py /tmp/open-switch-circuits \
  /tmp/writer-open-switches.json
```

No third-party model or new fixture is required; synthetic exports stay external.
