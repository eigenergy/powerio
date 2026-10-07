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
acceptance test. Without an explicit snapshot, active profiles still reject.
The separate snapshot audit and complete CSIRO09 electrical check below now
establish one complete conductor-resolved case.
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

`load-daily-profiles.json` validates 484 profiled loads (482 phase-pair and
two phase-to-earth) across CSIRO 01/04/07 at five times (2,420 snapshots), including interpolation and
the cyclic boundary. NumPy independently interpolates the publisher's input
powers; OpenDSS independently constructs each load's primitive. Maximum
power error is below 7.3e-12 W/var and admittance error below 1.5e-17 S.
All 485 selected loads are accounted for: one load in CSIRO 04 (Element 543,
OpSer 598) rejects because two different native values share time 7.5 hours.
The checker verifies that rejection against the original acquired records;
it does not choose or average a conflicting row. The source files and generated
component exports remain external, with pinned hashes in the compact report.

`distribution-csiro-profiles.json` separately audits all 19 cases at time zero.
This does not establish a complete parsed network or a whole-feeder solve;
unresolved grounding, transformers and other modes remain explicit failures.

### Ideal connection lines

For the steady-state profile, `Line.Flag_LineTyp=3` is an ideal connection,
represented by an existing `DistSwitch`. Siemens General Input Data (April
2014), printed p. 153, specifies node fusion for power flow and separately
states that stored R/X/C apply to dynamics, distance protection and data export.
The reader therefore does not approximate these connections with a small
impedance or require their inactive zero-sequence inputs. Original source
bytes retain those other-study inputs for same-format echo.

The first supported connection profile has two full three-phase ports. It
preserves A/B/C identity, both native terminal states, service state and any
positive ampacity (including parallel/rating factors). If either port is open
or the element is out of service, the typed switch is open. A native neutral
conductor is neither joined nor grounded. Reduced/neutral-only ports, segments,
coupling references and active unresolved controls reject explicitly. Ordinary
cables and overhead lines continue through the existing finite circuit mapper.

```sh
POWERIO_SINCAL_CONNECTION_RECORDS=/tmp/acquired-records/representative09.json \
POWERIO_SINCAL_CONNECTION_EXPORT=/tmp/connection-components.json \
  cargo test -p powerio-dist --lib \
  sincal::connection_tests::export_csiro_connections -- --ignored
python3 evals/sincal/check_connections.py /tmp/native-models/csiro-representative09.mdb \
  /tmp/acquired-records/representative09.json /tmp/connection-components.json \
  --report /tmp/connections-csiro09.json
```

`connections-csiro09.json` accounts for all 144 native connections: 140 closed
and four open, with 420 active phase constraints. An independent union-find
of the original native ports agrees with SciPy connected components of the
actual Rust switch/graph output. Each port, phase map, rating and native state
is also checked directly. Synthetic tests cover all eight combinations of
service and terminal states, fresh PMD transport, existing graph consumers,
input immutability, inactive sequence fields and malformed/unsupported inputs.
This is topology/component evidence, not a complete feeder or native solver
acceptance. CSIRO 09 now maps all 620 line elements; its source and 66 profiled
loads still require further work.


### Explicit single-phase earth loads

Siemens General Input Data (April 2014), printed p. 97, defines L1/L2/L3
load connections as phase-to-earth. With no declared zero-sequence input,
these explicit two-terminal connections now map to a `SinglePhase` load and
an internal grounded terminal. The native bus neutral remains untouched;
only the selected phase crosses the native terminal switch. Nominal branch
voltage is the native line-to-line base divided by the square root of three.
Three-phase Wye loads still require their own star/sequence resolution, and
active unresolved sequence or star-point impedances continue to reject.

Synthetic tests exercise every single phase and P/I/Z voltage model, open
terminals, floating native neutrals, rejection of active unresolved inputs,
and PMD transport. The daily profile oracle additionally checks both authentic
CSIRO 01 single-phase loads at all five snapshot times against independently
constructed OpenDSS phase-to-earth primitives. It verifies the explicit earth
and phase-switch maps, as well as powers and admittances. This adds ten native
component checks; it does not establish a complete parsed feeder.

For the phase-to-earth comparison, both terminal-current rows are checked
over all unconstrained voltage columns with earth fixed at zero. OpenDSS
adds a small numerical neutral-diagonal shunt in
[`Load.pas`, `CalcYPrimMatrix`](https://github.com/dss-extensions/dss_capi/blob/0.14.5/src/PCElements/Load.pas#L1206-L1210).
That diagonal multiplies the constrained zero voltage; it is not an input
impedance to copy into PowerIO. The oracle retains its original 1e-12 S
acceptance tolerance and checks earth connectivity separately.


### Schema-11.5 transformer defaults and limited-load gap

The April 2014 Database Description (printed pp.45–46) lists zero defaults
for optional transformer rotation, common tap position/midpoint/increments,
core loss and no-load current, and selects winding 1 as the default tap side.
The schema-11.5 reader applies only these enumerated defaults to explicit NULL
cells. Inactive common-tap values are not reported as defaults when individual
taps are selected. Applied fields appear under the native transformer ID in
`network.defaulted`; source cells remain NULL. Required voltage/power ratings,
series measurements, vector group and regulation mode never take this path.
Missing columns, malformed values, inconsistent core parameters and modern
schema NULLs still reject. A NULL centre-tap selector additionally requires
absent/zero centre-tap measurements and no centre-tap neutral reference.

CSIRO03 contains eight transformers using these legacy omissions. Synthetic
regressions compare their default profile with fully specified circuits, check
provenance and source immutability, and exercise each missing/invalid field.
The external audit passes these NULL-field guards. With transformer topology
now decoded independently of operating state, CSIRO03 next rejects the
unmapped ShuntReactor conductor declaration (element 3766). It still reports a
context failure, not a whole-network parse or a complete component audit.

All 297 CSIRO03 loads use native `Flag_LoadType=4` (limited/scaled P/Q).
Siemens Load Flow (April 2014), printed pp.11–12, shows a smooth reduction
curve below a voltage threshold; it is not constant power or a simple ZIP
polynomial over the full domain. The public
[6.0 release notes](https://sincal.simtec.cc/6.0/ReleaseNotes-Eng.pdf) describe
its introduction, but neither source establishes the exact curve equation.
The [12.5 release notes](https://sincal.s3.amazonaws.com/12.5/ReleaseNotes-Eng.pdf)
add slack-power-driven correction of limited loads, so future version support
must also distinguish that behavior. No limited load is relabeled as constant
power, no curve is fitted to the illustration/results, and no generic voltage
model is added until its exact semantics are established. This is a reader
implementation gap; native writer acceptance is a separate external gate.

### Ungrounded Y0 autotransformers

Transformer topology now reads the ordered winding/port declarations separately
from regulator state. Y0, YN0 and D0 remain explicitly marked autotransformers;
accepting their conductor declarations does not accept their electrical modes.
Unsupported controls and circuits fail atomic assembly with component identity,
and the corpus audit can now account for other components in those networks.

Two Y0 electrical profiles are implemented:

- Full windings at different rated voltages, nominal fixed taps, zero excitation,
  no extra rotation/grounding, and explicit R0/R1 and X0/X1 inputs. Positive and
  negative sequences use the rated two-port circuit. Zero sequence is a physical
  longitudinal branch: `I01=(U01-U02)/Z0`, `I02=-I01`, without the turns ratio.
- Equal rated voltages at the exact neutral tap, zero excitation, a valid
  characteristic tap range and resolved sequence ratios. The specified
  `Zact=Zchar*((1-uact)/(1-uchar))^2` is exactly zero. An existing typed switch
  connects the selected phases; no tiny impedance or ground is introduced.
  Element/terminal states and native identities are retained.

These rules follow Siemens General Input Data (April 2014), printed pp.180–182
and 189. Non-neutral same-voltage regulation, active controllers, finite partial
windings, YN0 and D0 circuits remain unsupported; their topology is not an
ordinary isolated YY/DD approximation. Ordinary transformer paths are unchanged.

`autotransformers.json` separates its evidence: two synthetic finite Y0
primitives compare rotating sequences against OpenDSS isolated floating-YY
circuits (internal star references eliminated), and zero sequence against the
manual's longitudinal-branch equation. This is **not** a native OpenDSS AutoTrans
or SINCAL acceptance test. Both voltage directions agree below 5e-16 relative
primitive error. The two authentic CSIRO01 neutral-tap devices (2451, 2453)
are checked against the original hash-pinned identities, phases and topology.
Mutations removing zero-sequence transfer, reversing transfer signs, changing
a native phase, opening a connection or omitting a device all fail the checker.

At the autotransformer checkpoint the all-19-case audits contained zero complete
native parses; the later CSIRO09 snapshot milestone below supersedes that count.
At explicit midnight, CSIRO01 maps 781/1033 components and CSIRO02 maps
1162/2329. Their remaining failures include partial transformers, inconsistent
core parameters, Wye load zero-sequence semantics, reduced-phase lines and
off-neutral regulators. Cases 05, 14 and 15 now reach component-level audits
instead of stopping on transformer operating-mode fields during topology.
CSIRO06 remains at 159/218. These counts are mappings, not independent
whole-feeder electrical validation.

```sh
POWERIO_SINCAL_AUTO_EXPORT=/tmp/y0-primitives.json cargo test -p powerio-dist --lib \
  sincal::autotransformer_tests::export_y0_primitives -- --ignored
POWERIO_SINCAL_AUTO_RECORDS=/tmp/acquired-records/representative01.json \
POWERIO_SINCAL_AUTO_NATIVE_EXPORT=/tmp/y0-native.json cargo test -p powerio-dist --lib \
  sincal::autotransformer_tests::export_native_neutral_y0 -- --ignored
python3 evals/sincal/check_autotransformers.py /tmp/y0-primitives.json \
  /tmp/acquired-records/representative01.json /tmp/native-models/csiro-representative01.mdb \
  /tmp/y0-native.json --report /tmp/autotransformers.json
```


### Schema-12.8 LPC component compatibility

`DatabaseSnapshot::decode_records` now structurally admits 12.8 acquired Access
records while direct native SQLite admission stays unchanged. The distribution
reader independently admits the verified static field layout. Absent later
`Flag_Lf`, `Flag_Pctrl`, `Rlf`/`Xlf`, `C01`/`C02` and `ElemLoading_ID` fields
retain the older circuit. In contrast, 12.8 boost and controller-reference
columns already exist and cannot be silently omitted. Explicit NULL controls
and required electrical data still reject; the schema-11.5 transformer NULL
defaults are not extended to 12.8.

The NULL voltage-kind selector retains line-line voltage with provenance. This
uses the General Input Data (April 2014), printed pp.18,28 convention;
[14.0 release notes, pp.19–20](https://sincal.s3.amazonaws.com/14.0/ReleaseNotes-Eng.pdf)
document the later voltage-kind selection. Schema numbers are retained as found
and are not treated as product release numbers. Synthetic tests compare explicit
and historical layouts and reject missing 12.8 controls and modern NULLs.

The Database Description (April 2014), printed p.19, gives zero as the default
thermal line rating `Ith`. Such an unspecified rating now maps to `i_max=None`;
it does not change the conductor matrix or create an arbitrary current limit.
Negative, NULL, nonfinite and overflowing ratings, nonpositive lengths and
invalid parallel/rating factors still reject.

`lpc-european-components.json` checks **all 205 lines and 55 loads** in the
original European LV model against independently built OpenDSS circuits. Every
conductor matrix entry, endpoint and missing thermal rating is checked. The
load check covers native P/power-factor input, selected phase, explicit earth,
constant-power behavior and both terminal currents. Line relative error is
below 7e-16; load current error is below 4e-13 A. Wrong mutual coupling, omitted
line, invented rating, wrong load phase, missing earth and wrong load power
mutations all fail the checker. Source, acquisition and export hashes are pinned.

This is **260/262 component coverage, not a complete network parse**. The source
and transformer have `Flag_Input=3`, omitting the zero-sequence category, with
`CalcParameter.Flag_LFZ0=1` (input data only). Input Data pp.219 and Database
Description p.3 distinguish missing sequence input from entered zero values.
The reader still rejects those components; no ratios are guessed. S1a maps its
54 loads but still rejects DC-infeed schema differences, library-selected lines,
missing source sequence input and inconsistent transformer core parameters.
All 19 CSIRO audits were rerun; their component counts are unchanged.

The LPC models are **external research inputs only**. Repository code licensing
does not establish redistribution rights for inherited network models. No MDB,
acquired table file or mapped component export is committed.

```sh
python3 evals/sincal/import_access.py /tmp/matlab-lpc-eu.mdb /tmp/lpc-eu-records.json
POWERIO_SINCAL_LPC_RECORDS=/tmp/lpc-eu-records.json \
POWERIO_SINCAL_LPC_EXPORT=/tmp/lpc-eu-components.json cargo test -p powerio-dist --lib \
  export_lpc_european_components -- --ignored
python3 evals/sincal/check_lpc_components.py /tmp/lpc-eu-records.json \
  /tmp/matlab-lpc-eu.mdb /tmp/lpc-eu-components.json --report /tmp/lpc-components.json
```


### Complete CSIRO09 snapshots and asymmetric stress validation

`csiro09-network.json` records five complete native snapshots (0, 0.25, 12,
23.75 and 24 hours) and five separately labelled synthetic stress cases. The
reader maps all **688 native elements**: 620 line/connection records, 67 loads
and one source. Of the 621 native nodes, 617 are energized and four remain
isolated behind open connections. None is silently discarded from the model.
The native loads in this case are phase-symmetric; the stress run multiplies
its three delta-branch powers by 2, 0.5 and 1 in both independent constructions.
This deliberately tests asymmetry but is not presented as authentic source data.

Three documented input selections unblock the source and daily loads:

- Database Description (April 2014), Infeeder/Line/Transformer type selectors:
  `Flag_Typ_ID=0` means no selected type even when a retained `Typ_ID` is nonzero.
  Materialized values remain authoritative and the inactive ID remains provenance.
- Input Data (April 2014), pp.50–51: source zero-sequence ratios use the selected
  short-circuit R/X input, independently of the ideal load-flow voltage boundary.
  This profile requires current explicit R/X and refuses unresolved min/max modes.
- Input Data p.94 and pp.292–295: absolute daily P/Q samples are multiplied by
  their respective native fP/fQ factors once. Interpolation, phase allocation and
  factors are independently checked; the unrelated fS factor is not applied.

Four ordinary native lines have exactly zero series and charging matrices.
After all ordinary-line validation/corrections they become typed ideal switches,
with current limits, length/provenance and separate terminal switching retained.
The conversion uses exact zero tests; small nonzero impedances, nonzero charging
and other singular matrices are not silently fused.

The checker solves the actual Rust network using sparse MNA and constant-power
current iteration, then independently builds OpenDSS from the hash-pinned acquired
native records. It compares every energized native phase, isolated-node identity
and load power. Maximum voltage difference is below **0.000372 V**, with a fixed
0.001 V tolerance; maximum power difference is below 2e-10 VA. OpenDSS uses explicit
1e-6 ohm positive/negative-sequence R/X for the ideal source. A second MNA solve
with that same finite boundary measures numerical agreement separately from the
physical approximation to the ideal source; matched-boundary error must remain
below 1e-4 V. Exploratory smaller impedances worsened numerical conditioning,
so they are not treated as stronger evidence. The voltage tolerance was not relaxed.
The source zero-sequence impedance is unchanged in all these comparisons.

Four deliberately corrupted mappings (profile scaling, mutual impedance, an open
connection and a missing load) are rejected. Both all-19-case audits are refreshed;
only CSIRO09 currently completes at an explicit snapshot. This is external OpenDSS
validation, not native SINCAL acceptance or a claim of complete corpus coverage.
Original MDBs and acquired/mapped model files remain external; only derived
validation evidence is committed, with CSIRO CC BY 4.0 attribution.

```sh
cargo build -p powerio-dist --example sincal_multiconductor
python3 evals/sincal/check_csiro09_corpus.py \
  /tmp/acquired-records/representative09.json \
  /tmp/native-models/csiro-representative09.mdb \
  target/debug/examples/sincal_multiconductor /tmp/csiro09-network.json
```


### Public multiconductor facade validation

`csiro09-public.json` repeats the complete five-snapshot/five-stress comparison
through `powerio::parse_with_options` with the explicit `sincal-multiconductor`
profile and caller-supplied Access tables. The `sincal_public` example checks
byte-exact original MDB emission, typed IR restoration, and refusal to echo
native bytes after IR restoration before exporting the actual typed network.
No intermediate JSON is relabelled as native SQLite or MDB. The independent
checker and its original electrical tolerances are unchanged.

The example also runs the generic multiconductor admittance assembly with zero
omission diagnostics. The generic power-flow instance constructor correctly
rejects the full native network: four isolated native buses have no source
(the first reported bus is `2178`). The report pins that diagnostic at every
snapshot. Parsing retains those buses; no reference or energization is invented.
The independent electrical comparison covers the 617 energized native nodes.
The connected synthetic public-reader test additionally verifies successful
construction through the same generic power-flow API. These are separate
claims: complete parsing and matrix assembly do not imply that every retained
native bus belongs in an energized calculation instance.

The current public Rust options select a variant, daily snapshot and relative
acquired-table companion. Ordinary native parsing is also available through
the CLI, Python and C's existing format argument and typed network accessors.
Python, CLI, C and Julia selection options are now checked as described below.
The generic C ABI remains version 7 with no new symbols. The current 0.11.4 IR
schema adds the SINCAL source-format enum spelling; older schema snapshots are
unchanged, and no electrical type layout or IR version changes.

```sh
cargo build -p powerio --example sincal_public
POWERIO_MAX_PRIMARY_BYTES=69181440 python3 evals/sincal/check_csiro09_corpus.py \
  /tmp/acquired-records/representative09.json \
  /tmp/native-models/csiro-representative09.mdb \
  target/debug/examples/sincal_public /tmp/csiro09-public.json --public-reader
```

The explicit primary bound equals the original case's size; the normal 64 MiB
file limit and the 64 MiB acquired-table limit are unchanged. Acquired origin
checks verify the original length/hash and Jet header, not the external tool's
faithfulness. Original synthetic unit tests additionally reject wrong origins,
missing/escaping companions, unsupported modes and conflicting family options.
The small original SQL fixture (7,367 bytes, 88 lines) constructs native SQLite
at test runtime and contains no third-party model payload.

### Python and CLI selection validation

`check_public_bindings.py` uses an installed wheel and the built CLI, with the
hash-pinned original CSIRO09 MDB and acquired records kept outside the repo.
At 0h and 6h, it compares complete serialized typed values, requires changed
load powers across snapshots, and checks original MDB echo through both APIs.
`csiro09-bindings.json` records these binding checks separately from the
independent OpenDSS evidence. It does not execute native SINCAL.

```sh
POWERIO_MAX_PRIMARY_BYTES=69181440 python3 evals/sincal/check_public_bindings.py \
  /tmp/models/representative09.mdb /tmp/records/representative09.json \
  target/debug/powerio /tmp/csiro09-bindings.json --acquisition-root /tmp
```

The CLI exposes selections on `summary`, `serialize` and `convert`. The Python
wrapper supplies typed `dist.SincalReadOptions` and either file acquisition or
named memory buffers. Original synthetic Python tests cover four profile times,
missing snapshots, wrong variants, NaN times, mismatched source hashes, missing
companions, root confinement, conflicting families and invalid option types.
The model and matrix APIs remain unchanged. No C ABI symbols are added here.

### C and Julia selection validation

`check_julia_bindings.jl` uses the matching Julia companion and C library to
parse the external, hash-pinned CSIRO09 MDB at 0h and 6h. It compares the complete
typed value with CLI serialization, verifies different native load powers,
checks original MDB echo, and verifies typed IR restoration. The derived
`csiro09-julia-bindings.json` report contains no native model. These are binding
checks; independent electrical evidence remains in `csiro09-public.json`.

```sh
POWERIO_MAX_PRIMARY_BYTES=69181440 POWERIO_CAPI=/path/to/libpowerio_capi.dylib \
  julia --project=/path/to/PowerIO.jl evals/sincal/check_julia_bindings.jl \
  /tmp/models/representative09.mdb /tmp/records/representative09.json \
  target/debug/powerio /tmp /tmp/csiro09-julia-bindings.json
```

The complete C suite passes 48 tests (one optional synthetic exporter ignored).
The Julia suite passes 1,896 assertions with one skipped check, including 41 new
selection assertions. The full CI Clippy matrix, regenerated header parity and
Julia coverage of all 511 C entry points pass. The two additive C functions
preserve the existing ABI 7 layouts and ordinary `pio_parse` signature. The
small Julia acquisition fixture is generated from original PowerIO SQL and
explicitly licensed under MIT; it is not a redistributed native feeder.

### Partial delta–delta transformer validation

The reader now assembles the installed delta coil pairs for vector groups Dd0
and Dd6, including one or two coils, nominal fixed taps, and open native ports.
The native winding selector identifies coils, so W1 connects L1–L2 on both
sides; it is not a one-conductor connection. Compact four- or six-coordinate
shunts preserve those circuits through existing multiconductor model types.
No neutral or absent third phase is introduced. Ratings keep the documented
three-phase base; they are not multiplied by the number of selected coils.

The basis is Siemens *Load Flow*, April 2014, printed pp.34–35/39 (installed
coil connections), and *Input Data*, p.180 (nominal impedance/excitation).
`check_partial_delta.py` independently builds OpenDSS single-phase transformers
with the appropriate per-coil ratings and separate symmetric excitation
branches. Twelve original synthetic cases cover all partial winding selections
and both polarities. Every newly supported native device is checked against
its original, hash-pinned input rows:

| Case | Newly mapped, independently checked | Rejected inconsistent core inputs | Total mapped components at 0h |
| --- | ---: | ---: | ---: |
| CSIRO02 | 363 | 23 | 1525/2329 |
| CSIRO04 | 95 | 83 | 575/861 |
| CSIRO06 | 4 | 4 | 163/218 |
| CSIRO07 | 56 | 25 | 283/456 |

Reports are `partial-delta.json` (06) and `partial-delta-csiro02/04/07.json`.
Across all 518 accepted native devices, maximum relative primitive error is
1.883e-13 against a 1e-11 tolerance. Four negative controls check factor-three
scaling, wrong conductor endpoints, invented grounding and wrong polarity.
The 135 rejected devices have core real power exceeding declared no-load VA;
no input is repaired. This is component evidence, not native SINCAL execution
or complete-feeder acceptance. Partial mixed-winding circuits remain work.

```sh
POWERIO_SINCAL_PARTIAL_RECORDS=/tmp/records/representative06.json \
POWERIO_SINCAL_PARTIAL_EXPORT=/tmp/partial-delta-export.json \
  cargo test -p powerio-dist --lib export_partial_delta_circuits -- --ignored
python3 evals/sincal/check_partial_delta.py /tmp/partial-delta-export.json \
  /tmp/records/representative06.json /tmp/models/csiro-representative06.mdb \
  /tmp/partial-delta.json --case 6
```

Repeat with cases 2, 4 and 7 for the other reports. The exports and original
models stay external; only compact derived reports are committed. Both all-19
base-variant audits were refreshed. Complete native parsing remains one case,
CSIRO09 with an explicit snapshot. Distribution regressions (592 tests) and
the full CI Clippy matrix pass.

### CSIRO06 remaining-input audit

`check_csiro06_blockers.py` classifies all 55 rejected components at the explicit
midnight snapshot against the original hash-pinned MDB and acquired records.
The current reader maps 163/218 components. This audit does not turn a rejection
into a successful network or make missing-input assumptions:

- 35 three-phase Wye loads have no active zero-sequence category and no explicit
  star-point reference. Native `Flag_LFZ0=1` disables sequence completion. Their
  connection semantics still require resolution.
- Nine partial YNd1 transformers declare direct zero-sequence ohms on the
  grounded primary. Their positive-sequence magnitudes are approximately
  1607–2280 ohms, while zero-sequence magnitudes are 2.20–3.59 ohms. The existing
  independent delta-coil model cannot simply be reused while dropping these
  inputs. The report does not classify this difference as invalid data.
- Eleven transformers have core loss greater than apparent no-load power.
  Decimal arithmetic confirms three repeated conflicts: 32 W versus 30.72 VA,
  726 W versus 725.76 VA, and 30 W versus 27 VA. These exceed floating-point
  roundoff. Siemens General Input Data (April 2014), printed p.180, uses
  `Pcore = Vfe * 1000` W and `S0 = i0 / 100 * Sn * 1e6` VA. No real reactive
  core component can satisfy those nameplates. All 51 transformers separately
  satisfy `ur <= uk`, so the current conflicts are excitation inputs, not
  short-circuit impedance inputs.

The reader now names `ur/uk` or `Vfe/i0/Sn` in the corresponding error. The
core-loss tolerance and accepted electrical profiles are unchanged. A complete
strict read of the unchanged case needs a documented native handling rule for
the inconsistent inputs, or an explicitly identified corrected derivative;
completing the missing mappings alone is insufficient.

`csiro06-blockers.json` records component IDs, derived quantities, identities,
attribution and four negative controls against missing accounting, accepted
invalid excitation, wrong error classification and ignored load failures.
The report is a development blocker audit, not independent electrical or native
SINCAL acceptance. No native model, manual or corrected fixture is vendored.

```sh
cargo build -p powerio-dist --example sincal_multiconductor
python3 evals/sincal/check_csiro06_blockers.py \
  /tmp/models/csiro-representative06.mdb /tmp/records/representative06.json \
  target/debug/examples/sincal_multiconductor /tmp/csiro06-blockers.json
```

### Rated reactor and capacitor banks

The distribution reader maps fixed `ShuntReactor` and `ShuntCondensator` banks
to conductor shunts and port switches. Nominal ratings and losses produce
fundamental-frequency admittances; fixed steps scale both. Ratings are total
installed power: a phase-earth bank uses the full rating on that phase, a
phase-pair bank between its two phases, and a three-phase bank divides it across
three branches. Floating three-phase stars retain zero common-mode admittance.
Grounded three-phase banks retain their distinct sequence impedances. Explicit
neutral impedances, automatic regulation, nontrivial single-phase zero sequence,
and stepped direct-ohm zero sequence remain rejected. Inactive elements and open
terminals retain their passive primitive behind an open typed switch.

Only schema 11.5 admits documented NULL zero defaults for optional losses,
grounding, macro and regulator flags (Database Description, April 2014 pp.29–32).
Applied fields enter `network.defaulted`; required ratings, active step inputs,
missing columns and modern NULLs still reject. Capacitor input mode3 is not
inferred from the reactor enum. Materialized type references keep their source
provenance. The existing `MulticonductorNetwork` model needs no new public type.

All nine native banks in CSIRO03/08/11/12/16/17 and 14 original synthetic phase cases
pass independent OpenDSS constant-impedance checks. Maximum relative primitive
error is 2.52e-16 against 1e-10; eight mutations detect factor-three, reactive-sign,
grounding and service-state errors. The oracle reconstructs the neutral return
row from phase-current KCL instead of interpreting OpenDSS's numerical neutral
regularizer as a physical grounding path. `rated-shunts.json` records identities,
versions, tolerances and component results; it does not claim a feeder solve.

`rated-shunt-history.json` independently checks the CSIRO03 reactor against all
49 aligned historical node/branch snapshots at their recorded terminal voltages.
Maximum local power error is 2.44e-11 VA and current error 8.89e-16 A. The wrong
factor-three interpretation differs by at least 13,136 VA. Twelve node records
have limit violations; result `Flag_State` is a limit-status flag, not equipment
service state, and is not used as a node/branch join key. No input is calibrated
to results, and no historical result rows or native models are vendored.

The importer now includes bank tables by default. `check_access_corpus.py`
explicitly retains the original `BASE_TABLES` profile so its existing identities
remain reproducible. `acquire_shunt_corpus.py` adds the two tables, verifies every
base table unchanged, and records the new acquired-document identities in
`shunt-acquisition.json`. The distribution audit takes
`--shunt-record-directory` to use those extended cases and marks the profile
per case. At the bank checkpoint, both all-19 audits used those extensions. CSIRO03
mapped 2/1084 components (source and reactor); CSIRO12 mapped 61/215 (34 lines, 26 loads
and capacitor). Cases 16/17 still stop at synchronous-machine topology, although
their three inactive capacitors pass component checks. Complete native parsing
remains one feeder, CSIRO09 with an explicit snapshot.

```sh
cargo build -p powerio-dist --example sincal_multiconductor
python3 evals/sincal/acquire_shunt_corpus.py /tmp/models /tmp/records \
  /tmp/shunt-records target/debug/examples/sincal_multiconductor \
  /tmp/shunt-acquisition.json
POWERIO_SINCAL_SHUNT_RECORDS_DIR=/tmp/shunt-records \
POWERIO_SINCAL_SHUNT_EXPORT=/tmp/rated-shunt-circuits.json \
  cargo test -p powerio-dist --lib export_rated_shunt_circuits -- --ignored
python3 evals/sincal/check_rated_shunts.py /tmp/rated-shunt-circuits.json \
  /tmp/models /tmp/shunt-records /tmp/records /tmp/rated-shunts.json
python3 evals/sincal/check_shunt_history.py /tmp/models/csiro-representative03.mdb \
  /tmp/rated-shunt-circuits.json /tmp/rated-shunt-history.json
```

### Legacy line temperatures

The schema-11.5 reader now applies the documented 20 C `VoltageLevel.Temp_Line`
or `Temp_Cable` default only when the field selected by the line kind is NULL.
The Database Description (April 2014), VoltageLevel table, assigns 20 C to both.
Explicit temperatures still apply their resistance correction; the other line
kind's temperature is inactive. Missing columns, invalid/nonfinite values and
modern NULLs remain errors. Applied defaults are recorded under `Line.<id>` in
`network.defaulted`, while retained source/acquisition data keep the original NULL.

`legacy-temperatures.json` checks all 186 newly mapped native line circuits:
25 in CSIRO03, eight in CSIRO05 and 153 in CSIRO12. Independent OpenDSS circuits
agree within 9.55e-16 relative admittance error (tolerance 1e-10), including 28
single-phase lines and three open terminals. Using 70 C instead of 20 C produces
at least 0.0898 relative error in every checked case. The oracle pins source and
acquisition identities, exact candidate sets, connectivity, ratings and defaults.
No native files or result rows are redistributed.

The refreshed midnight component totals are 27/1084 for CSIRO03, 86/1378 for
CSIRO05 and 214/215 for CSIRO12. The latter maps every line, load and capacitor;
its source still rejects nullable control fields. CSIRO03/05 now expose additional
NULL line flags, loss and parallel-count fields that need separate documented
interpretation; load and transformer gaps also remain. This is component evidence,
not another complete feeder or native SINCAL acceptance. The complete-feeder count
remains one (CSIRO09 at a selected snapshot).

```sh
POWERIO_SINCAL_TEMPERATURE_RECORDS=/tmp/records \
POWERIO_SINCAL_TEMPERATURE_EXPORT=/tmp/line-temperature-circuits.json \
  cargo test -p powerio-dist --lib export_legacy_temperature_lines -- --ignored
python3 evals/sincal/check_legacy_temperatures.py /tmp/models /tmp/records \
  /tmp/line-temperature-circuits.json /tmp/legacy-temperatures.json
```

### Sparse legacy line inputs

The schema-11.5 profile now reads additional optional NULL fields using the
Database Description (April 2014), pp.18–19: `Flag_Ll`, `Flag_Ground`, `Flag_Macro`
and dielectric losses `va` default to zero; `ParSys` and `fr` to one; `alpha` to
0.004 per C; and rated frequency `fn` to 50 Hz. Explicit values remain authoritative.
The temperature coefficient is read only when a temperature correction is active.
Modern NULLs in active fields, missing columns, malformed values, required
impedances/length/voltage and unknown active modes still reject. Applied defaults
enter `network.defaulted` for both finite lines and ideal connections. The original
source and typed acquisition remain unchanged.

`sparse-lines.json` verifies all 1,720 newly mapped native line elements across
CSIRO03/05/14/15. Of these, 1,658 finite circuits agree with independent OpenDSS
primitives within 8.30e-16 relative error (tolerance 1e-10). This includes 157
coupled single-phase series-only lines: the oracle eliminates absent currents
from OpenDSS's full three-phase admittance, fixing one unused endpoint per absent
conductor as a zero-current gauge. It does not project the full admittance by
simply deleting rows. Reduced-phase charging remains limited to independent
phases. OpenDSS's numerically regularized open-conductor primitive is not used as
a physical grounding circuit.

The remaining 29 exact-zero ordinary lines and 33 declared connections retain
exact switches, native ratings and port states. Declared connection partitions
are checked through native union-find versus SciPy graph components. All exports
pin native identities, candidate sets, phases, endpoints, limits and default
provenance. The earlier 186 temperature-only line exports remain byte-identical.
No native files or result rows are vendored.

Current midnight totals are 776/1084 for CSIRO03, 913/1378 for CSIRO05, 55/65 for
CSIRO14 and 89/102 for CSIRO15; every line in those four cases now maps. Their
remaining loads, sources and transformers still reject. CSIRO12 remains 214/215,
and CSIRO09 is still the only complete conductor-resolved native parse. Local
inspection found no unbalanced stored results for CSIRO12; its single balanced
source result alone cannot establish the unresolved source-control semantics.

```sh
POWERIO_SINCAL_SPARSE_LINE_RECORDS=/tmp/records \
POWERIO_SINCAL_SPARSE_LINE_EXPORT=/tmp/sparse-line-circuits.json \
  cargo test -p powerio-dist --lib export_sparse_legacy_lines -- --ignored
python3 evals/sincal/check_sparse_lines.py /tmp/models /tmp/records \
  /tmp/sparse-line-circuits.json /tmp/sparse-lines.json
```

### Materialized load manipulators: CSIRO05

`materialized-loads.json` verifies 456 native loads at five selected times
(2,280 snapshots): 368 three-phase delta loads and 88 phase-pair loads. Siemens
[Release Notes 21.0, pp. 4–6](https://sincal.s3.amazonaws.com/21.0/ReleaseNotes-Eng.pdf)
distinguish permanent UI edits from runtime operating-point factors. The reader
therefore uses the stored electrical fields, applies each load's `fP`/`fQ` once,
and retains `Mpl_ID` as provenance. It neither requires the UI definition nor
reapplies it. Synthetic tests cover all supported electrical schema profiles,
conflicting saved UI factors, unchanged source bytes, invalid references,
required inputs, daily selection and typed serialization.

The independent checker derives profile interpolation and connections from
hash-pinned acquired native tables and builds OpenDSS constant-power loads.
It compares every mapped branch power and terminal current at the independently
solved voltage. Maximum errors are 1.82e-12 W/var and 2.98e-13 A, respectively.
Applying the stored factor twice gives a counterexample at every nonzero-power
snapshot (2,265); the remaining 15 snapshots have exactly zero power.

Two native loads still reject: element 841/profile 853 has conflicting values
at 4h, and element 1007/profile 656 at 2h. The checker independently inventories
these conflicts instead of skipping unspecified failures. CSIRO05 now maps
1,369/1,378 components; those two loads and seven transformers remain unresolved.
This is component evidence, not a complete feeder solve or native acceptance.

```sh
POWERIO_SINCAL_MANIPULATOR_RECORDS="$records/representative05.json" \
POWERIO_SINCAL_MANIPULATOR_EXPORT="$output/csiro05-loads.json" \
cargo test -p powerio-dist --lib export_csiro05_materialized_loads -- --ignored
python evals/sincal/check_materialized_loads.py \
  "$sources/csiro-representative05.mdb" "$records/representative05.json" \
  "$output/csiro05-loads.json" --report "$output/materialized-loads.json"
```

Native files and acquired tables remain external; only the small derived report
is committed. The original CSIRO data is CC BY 4.0, with attribution in the report.

### Partial mixed-winding investigation and fixed tap-status defaults

`partial-mixed-history.json` audits all nine native CSIRO06 partial YNd1
transformers using the original MDB's aligned node and branch results. The
independent OpenDSS candidate uses one coil rated at Sn/3, phase-earth voltage
on the Wye side, phase-pair voltage on the Delta side, and split nominal pi
excitation. It deliberately does not model the distinct native zero-sequence
impedance. The candidate disagrees with every historical case: maximum terminal
power errors range from 1.95 to 4.75 kVA and current-magnitude errors from 4.19
to 9.62 A. The selected phase and return connections cover W1, W2 and W3.

This is evidence against accepting that simple hypothesis, **not** a passing
transformer mapping or proof that the stored results used the current inputs.
The report records `candidate_accepted=false`, `production_mapping_added=false`
and `stored_results_attest_current_inputs=false`. No native input is edited,
no value is fitted, and no SINCAL process is run. The next circuit work must
resolve the distinct sequence impedances, excitation and rating conventions.

```sh
python evals/sincal/audit_partial_mixed_history.py \
  "$sources/csiro-representative06.mdb" "$records/representative06.json" \
  "$output/partial-mixed-history.json"
```

Separately, Database Description (April 2014), p.46 documents `Flag_roh=1`
(fixed tap) as the default. The schema-11.5 reader now accepts a stored NULL
with that interpretation, preserves the original cell and records it in
`network.defaulted`. Synthetic tests verify electrical equivalence to explicit
fixed status and reject controllers, invalid values, missing fields and NULLs
in other schema profiles. CSIRO05's seven transformers now reach the explicit
partial mixed-winding rejection. Component coverage and complete-feeder counts
are unchanged; there is no new feeder acceptance claim.

## Machine terminal topology and broader corpus accounting

The General Input Data manual (April 2014), p.60, declares the ordinary
phase/phase-pair port connections for synchronous machines. The topology reader
now collects those conductors without pretending to implement a machine's
positive-, negative- or zero-sequence circuit. No source, ground or partial
network is returned for an unsupported machine. Synthetic tests cover all seven
phase selections, isolated machine nodes, source-byte preservation and rejection
of unknown or missing terminal declarations.

This removes the audit's early stop in CSIRO08/10/11/16/17/18. All 19 cases are
still audited, with component dispositions for 18; CSIRO13's unresolved terminal
selector remains a global error. The new midnight counts are 265/309, 99/124,
87/104, 90/103, 130/142 and 169/295 respectively. These are newly measured
component counts, not newly supported complete feeders. Every machine remains
an explicit component rejection, and only CSIRO09 parses completely.

`acquire_shunt_corpus.py` now includes CSIRO08/11 in addition to 03/12/16/17.
Its source hashes and every original acquired table must match the base manifest.
The extended acquisition adds the four capacitor banks that the old table set
omitted. The rated-bank export/checker verifies all nine native banks against
independently constructed OpenDSS circuits, retaining the 14 synthetic cases and
eight mutation controls. Re-run both distribution audits with this extended
record directory; do not use the older four-case directory with the new manifest.
Original models and acquired tables remain external research data.

## Relative daily profiles with unequal branch powers

Schema-11.5 daily common-factor profiles (`OpSer.Flag_Typ=1`) now scale the
already decoded base powers. The mapper retains unequal Wye/delta branch powers,
phase order, voltage dependence and the selected input mode's power factors.
General Input Data (April 2014), pp.292–295, defines the common factor, cyclic
period and direct-scaling coefficient profile. This path requires zero
`Power_a1`, `Power_b1` and `Reduce_a2`, with `Reduce_b2=1`; it does not infer
energy conversion or topology-dependent coincidence. Factors must be finite and
nonnegative. Function 2 (separate P/Q factors) remains unsupported because its
legacy field storage is not established by the inspected database manual.

`relative-profiles.json` records 42 original synthetic snapshots: unequal Wye
and delta branches, all three admitted voltage models, interpolation, zero
power, peak factors and cyclic repetition. NumPy samples the independently
specified curve; OpenDSS supplies branch currents for separate phase loads.
Ten corruption controls detect duplicate scaling, phase averaging, wrong ground,
wrong voltage model and incorrect provenance. This adds no native-corpus or
native SINCAL acceptance claim: all audited CSIRO load profiles are absolute.

```sh
POWERIO_SINCAL_RELATIVE_EXPORT="$output/relative-profiles.json" \
cargo test -p powerio-dist --lib export_relative_daily_profiles -- --ignored
python evals/sincal/check_relative_profiles.py \
  "$output/relative-profiles.json" "$output/relative-profile-check.json"
```

The public acquired-source path requires an explicit time and retains the
original bytes. Unit tests also reject malformed factors, unsupported coefficient
settings, conflicting cyclic endpoints, unknown functions and arithmetic
underflow/overflow. Absolute-profile output keeps its existing provenance layout;
its 484 native loads/2,420 snapshots were independently rechecked. The older
absolute checker now explicitly verifies the already-present native `power_factors`
provenance instead of rejecting that additional field. The CSIRO05 export of
456 loads/2,280 snapshots remains byte-exact against its prior export.
