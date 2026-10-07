# SINCAL acquisition and corpus evidence

This branch supplies model-neutral SQLite/archive acquisition and schema
validation. The balanced reader branch additionally maps the complete small SimBench case
the pinned IEEE18, IEEE33 and student SQLite static inputs, and CSIRO19 Access snapshots
through the explicit `sincal-balanced` public parser profile. Acquisition-only reports
remain distinct from electrical reader validation; none establishes fresh
writer acceptance. The [delivery plan](../../docs/design/pss-sincal.md) records the
five local PR scopes and dataset targets. The [research catalog](research-catalog.md)
records prior inspections and data rights; its deeper electrical investigations
belong to the reader branches and are not all implemented in this branch.

Run the small licensed archive and paired SimBench evidence checks with:

```sh
python3 -m unittest discover -s evals/sincal -v
python3 evals/sincal/inspect_native.py tests/data/sincal/1-LV-rural1--0-sw.sinx
cargo test -p powerio-sincal
```

The fixture's provenance and ODbL/DbCL notices live in
[tests/data/sincal](../../tests/data/sincal/README.md). The 88,876-byte archive
is unmodified; its larger unpacked database is not vendored. Original synthetic
unit tests exercise malformed inputs without copying additional native models.

The [CSIRO inventory](csiro-inventory.json) contains hashes, counts and raw
observations for 19 external CC BY 4.0 databases. It contains no model payloads.
Access acquisition is explicit and optional, as described below. These
historical counts are all stored rows, not effective selected-variant sizes.
External corpora remain outside the repository, and a source-code license
alone does not establish model redistribution rights.

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
independent checking. Broader profile and variant delivery remains in scope.

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

## Conductor-resolved reader development

The distribution branch extracts the existing family-local component adapters
and their synthetic tests, with an explicit internal harness:

```sh
cargo build -p powerio-dist --example sincal_multiconductor
# Complete mapping: rejects the entire network when any required mode fails.
target/debug/examples/sincal_multiconductor records read /tmp/records.json 1
# Diagnostic audit: component reports are not successful network parses.
target/debug/examples/sincal_multiconductor records audit /tmp/records.json 1
```

`native` instead of `records` selects SQLite/archive acquisition explicitly.
The electrical adapter independently admits its observed 11.5/14.8 schemas;
shared structural support for 15.5/16.0 does not automatically extend it.
The schema-11.5 adapter now interprets NULL `VoltageLevel.Flag_Volt` using
Siemens' legacy line-to-line input convention (General Input Data, April 2014,
printed pp. 18 and 28, including single-phase networks). A NULL `Flag_Tap`
selects the common tap in this legacy profile; the Database Description's
transformer table documents zero as the common-tap default. These rules remain
version-specific interpretations, not proof from running SINCAL. Missing rows,
missing selector columns, malformed values and newer-schema NULLs still fail.
The original acquisition records remain unchanged. Successful typed networks
record the applied defaults; node metadata also records the legacy voltage
basis. Controls absent from the older layout have an explicit field whitelist;
this does not turn arbitrary missing or NULL numeric data into zero.

Selected standard types use the materialized electrical values in the equipment
row. The adapter validates the local/global selector and records its identity;
it neither follows library paths nor substitutes catalog data for missing
stored parameters. Both values and type references are checked in synthetic
regressions through the typed Access acquisition boundary.

Reproduce the development coverage report with all 19 external, hash-checked
original MDBs and the already verified acquired records:

```sh
python3 evals/sincal/audit_distribution.py /tmp/native-models /tmp/acquired-records \
  /tmp/distribution-csiro.json --reader target/debug/examples/sincal_multiconductor
```

The committed `distribution-csiro.json` records context errors, component
coverage and complete parsing separately. It is a development audit, not an
acceptance test, and currently claims zero complete native unbalanced networks.
The source and acquired-record hashes must match `access-acquisition.json`;
missing required files, changed records or tool failures abort the audit.
Schema acquisition itself adds bounded nonunique lookup indexes, preserving
NULLs and duplicates while avoiding full-table scans for every component.

At this checkpoint, CSIRO 06 maps all 113 lines and 27 of 51 transformers;
its source and 18 phase-pair loads now map as well (159/218 total components),
while its 35 remaining loads and 24 remaining transformers still reject. The full audit
also reaches individual components in 04, 07, 09 and 19. Indexing removes the
query-budget failures previously observed in 02, 04 and 09 without increasing
the SQL instruction allowance. Remaining context-level failures include
autotransformers, other NULL selectors, regulation modes and unmapped machine
or capacitor types. Three-phase connectivity alone is not evidence that a
case should be routed to the balanced backend.

Further known work includes load-star selection, source impedance, transformer
nullable taps/core/partial connections, selected profiles and inherited variants.
The original research component evidence is retained; component success is not
presented as complete CSIRO parsing. Fresh writer packaging is reserved for its
separate branch and has not been pulled into this reader extraction.


### Finite source zero sequence

The schema-11.5 current-data profile now supports an ideal positive/negative-
sequence source with directly specified finite `R0+jX0`. It uses the existing
referenced voltage source and a shunt from its local star to earth, with
admittance `3/Z0`. This preserves the rotating-sequence voltage constraints,
zero-sequence voltage drop and grounding losses without adding electrical
behavior to metadata. The source star is not connected to an external neutral.
Minimum/maximum short-circuit selection, ambiguous settings, unresolved neutral
impedances and other source impedance modes still reject.

The mapping follows the direct zero-sequence and grounding definitions in the
Siemens General Input Data manual (April 2014, printed pp. 50–51). Do not infer
legacy defaults from later versions: [Siemens 14.5 release information, p. 13](https://sincal.s3.amazonaws.com/14.5/ReleaseNotes-Eng.pdf)
explicitly changed handling of activated zero-sequence inputs containing zeros.
The finite profile is therefore independently version-gated.

Reproduce the independent circuit check (NumPy/OpenDSSDirect are optional
evaluation dependencies):

```sh
POWERIO_SINCAL_SOURCE_ORACLE_DIR=/tmp/source-oracle cargo test -p powerio-dist \
  sincal::legacy_tests::export_source_zero_sequence_oracle --lib -- --ignored
python3 evals/sincal/check_source_zero_sequence.py /tmp/source-oracle \
  --report /tmp/source-zero-sequence.json
```

`source-zero-sequence.json` records three original synthetic cases (resistive,
inductive and capacitive zero sequence). A dense MNA solve uses the actual Rust
reader output; a separate OpenDSS circuit uses the original source/load inputs.
The maximum complex-voltage difference is below 7.1e-7 V. OpenDSS's required
small positive/negative impedance approximation is recorded explicitly.
Rust tests also cover current balance, losses, numerical extremes, typed
transport and calculation-mode rejection. These are component/circuit checks,
not complete CSIRO validation. The previously observed discrepancies with
historical source results remain unresolved; no stored result is used to fit
or replace an input impedance. Native SINCAL execution remains unperformed.


### Native phase-pair loads

An L12/L23/L31 or explicitly delta-connected load needs no inferred star or
connection to earth when zero-sequence input is undeclared: its branch
incidence already enforces zero total phase current. The mapper still rejects
active unresolved grounding/sequence inputs, and this rule does not establish
a default for Wye loads. Siemens General Input Data (April 2014, printed
pp. 97–98) defines the phase-pair connections and explicit delta power modes.

The external CSIRO 06 check exercises all 18 native phase-pair loads through
the production Rust mapper. OpenDSS independently constructs constant-impedance
loads from the original apparent power, power factor and voltage; the checker
compares full two-conductor admittances and verifies the absence of a ground
path. A wrong-grounding counterexample conducts 5 A under common-mode voltage.
The maximum admittance difference is below 2e-18 S against a 1e-12 S tolerance.
This is component evidence, not whole-feeder parsing or native SINCAL execution.

```sh
POWERIO_SINCAL_LOAD_RECORDS=/tmp/acquired-records/representative06.json \
POWERIO_SINCAL_LOAD_EXPORT=/tmp/csiro06-load-components.json \
  cargo test -p powerio-dist --lib \
  sincal::load_mapping_tests::export_csiro_phase_pair_loads -- --ignored
python3 evals/sincal/check_phase_pair_loads.py \
  /tmp/acquired-records/representative06.json /tmp/native-models/csiro-representative06.mdb \
  /tmp/csiro06-load-components.json --report /tmp/load-phase-pairs.json
```

`load-phase-pairs.json` records input/export hashes, all selected element IDs,
engine versions and measured errors. Source models remain external; the
checker verifies their identity against the licensed acquisition manifest.

### Explicit daily load snapshots

The internal distribution reader/audit accepts an optional snapshot time in
hours. Without it, active profiles still reject instead of silently selecting
midnight or using base powers. Selection currently supports schema-11.5 local
absolute-power daily profiles (`OpSer.Flag_Typ=3`, `Flag_Ser=1`) with no power
or diversity corrections and unity load P/Q factors. Profile kW/kvar replace
aggregate input powers; the base apparent-power factor `fS` does not scale the
replacement. Connections and constant-P/I/Z voltage behavior are preserved.

`BaseT=0` means 24 hours. Continuous segments interpolate linearly; discrete
segments retain the preceding value. The last segment wraps to the first
sample. A time-zero sample is required; an explicit sample at the period must
agree with it. Duplicate IDs/times, conflicting endpoints, malformed values,
unsupported selectors, weekly/yearly composition and allocation over explicit
per-phase input arrays reject. Original inputs remain unchanged, and selected
profile/time/period provenance accompanies the mapped load. These are selected
static snapshots; automatic `TimeSeries` construction remains future work.

The definitions are in Siemens General Input Data (April 2014), printed
pp. 292–295; Database Description pp. 84–85; and Load Flow pp. 48–50.

```sh
POWERIO_SINCAL_PROFILE_RECORDS=/tmp/acquired-records \
POWERIO_SINCAL_PROFILE_EXPORT=/tmp/profile-components.json \
  cargo test -p powerio-dist --lib \
  sincal::load_profile_tests::export_csiro_daily_loads -- --ignored
python3 evals/sincal/check_daily_profiles.py /tmp/native-models /tmp/acquired-records \
  /tmp/profile-components.json --report /tmp/load-daily-profiles.json
cargo build -p powerio-dist --example sincal_multiconductor
python3 evals/sincal/audit_distribution.py /tmp/native-models /tmp/acquired-records \
  /tmp/distribution-csiro-profiles.json --reader target/debug/examples/sincal_multiconductor \
  --snapshot-hours 0
```

`load-daily-profiles.json` validates 482 profiled phase-pair loads across
CSIRO 01/04/07 at five times (2,410 snapshots), including interpolation and
the cyclic boundary. NumPy independently interpolates the publisher's input
powers; OpenDSS independently constructs each load's primitive. Maximum
power error is below 1.9e-12 W/var and admittance error below 1.5e-17 S.
All 483 selected loads are accounted for: one load in CSIRO 04 (Element 543,
OpSer 598) rejects because two different native values share time 7.5 hours.
The checker verifies that rejection against the original acquired records;
it does not choose or average a conflicting row. The source files and generated
component exports remain external, with pinned hashes in the compact report.

`distribution-csiro-profiles.json` separately audits all 19 cases at time zero.
This does not establish a complete parsed network or a whole-feeder solve;
unresolved grounding, transformers and other modes remain explicit failures.
