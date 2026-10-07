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
