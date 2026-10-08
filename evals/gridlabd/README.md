# GridLAB-D and OpenDSS oracle calibration

These tools calibrate independent engines against analytic static circuits,
then validate specific PowerIO conversion profiles. Electrical reader and IR
checks cover the initial profile described below, including canonical GLM
output. They do not establish full format compatibility or OpenDSS conversion
equivalence.

`engines.json` pins the PNNL 5.3.0 macOS archive and runtime files, the Arras
4.3.17 Linux amd64 container by digest, and OpenDSSDirect 0.9.4 with DSS C-API
0.14.5 (OpenDSS SVN 3723). Keep the engines outside the repository. The PNNL
binary is x86_64; an Apple Silicon host needs x86_64 execution support. The
Arras container likewise requires amd64 execution support. This initial PNNL
pin is platform-specific; an independently built binary needs its own verified
profile rather than bypassing the checksum check.

Create a Python 3.14 virtual environment and install `requirements.txt`. Download
the PNNL archive from `engines.json`, verify `archive_sha256`, and unpack it into
`PNNL_ROOT`. Pull the exact Arras image named in that file. No system-wide engine
installation is required. The Python packages are evaluation dependencies only.

From the repository root, using that environment:

```sh
python -m unittest discover -s evals/gridlabd -v
python evals/gridlabd/oracle.py --pnnl-root "$PNNL_ROOT" --output /tmp/gld-calibration
python evals/gridlabd/neutral_oracle.py --output /tmp/gld-neutral
python evals/gridlabd/transformer_oracle.py --pnnl-root "$PNNL_ROOT" --output /tmp/gld-transformers
python evals/gridlabd/upstream.py --pnnl-root "$PNNL_ROOT" --output /tmp/gld-upstream
```

Each output path must be new. Engine failures cannot leave stale recorder data
that a later run mistakes for success. The analytic engine checks run offline
after provisioning; `upstream.py` downloads small hash-pinned public models.
Docker uses file copying instead of host bind mounts so Colima and remote
Docker daemons receive the actual inputs. Arras runs with networking disabled;
its launcher shell mode initializes the runtime without its post-run GitHub
version check. PNNL requires shared-memory access that some shell sandboxes
deny. Record this execution requirement when running in such an environment.

## Electrical checks

`oracle.py` solves a single-phase circuit and an asymmetrically loaded,
mutually coupled three-phase circuit. Every engine receives explicit constant
impedances, zero line charging, and no active controllers. The reference solves
`(Z_line + diag(Z_load)) I = V_source`. It compares complex bus voltages,
currents and powers at both line ends, total line losses, and the full series
impedance matrix. Each output includes input hashes, engine versions, expected
and actual snapshots, quantity-specific errors, and command logs.

Voltages are conductor-to-earth volts. Currents and powers point **into** each
link end, so GridLAB-D's outgoing values change sign. Powers are VA, impedances
ohms. No phase permutation, angle alignment, magnitude-only comparison, or
intersection of mismatching identity sets is allowed. Rectangular and polar
GridLAB-D output have explicit angle conventions. Recorders use 15 significant
digits and the final static sample. GridLAB-D's `b` matrix equals series
impedance for these series-only lines; its general impedance dump is not a
nodal admittance matrix. OpenDSS impedance is recovered from its line `YPrim`,
not from echoed input properties.

OpenDSS voltage sources have a declared finite impedance of `1e-6 + j1e-6`
ohms. The analytic reference uses their measured terminal phasors, so the
source impedance is not confused with an ideal GridLAB-D swing bus. This
calibration does not validate source-model equivalence.

`neutral_oracle.py` adds two unreduced four-wire OpenDSS circuits. A separate
nodal reference solves both neutral potentials, including finite source-end
grounding and optional sink-end grounding. All four conductors' voltages,
currents, powers and matrix entries are checked. Removing the sink ground is
a physical negative control and must be detected. This does not establish an
equivalent GridLAB-D neutral model.

The unit tests additionally reject phase swaps, current polarity reversal,
V/kV errors, missing or extra quantities, nonfinite values and altered earth
references. Empty comparisons cannot pass. Absolute-plus-relative tolerances
are stated per physical quantity in `oracle.py`; they are not the broad
tolerances used by the published feeder regressions.

`transformer_oracle.py` checks a two-winding single-phase transformer and a
center-tapped service transformer against all three engines. The latter has
unequal secondary leakage impedances and simultaneous unequal leg-neutral
loads and a leg-leg load. Its independent reference stamps ideal coil
incidences and solves the internal core potential. Complex terminal currents,
voltages, powers, neutral current and losses are compared. The OpenDSS checks
also compare the complete winding primitive admittance, including return
conductor rows. Reversing a secondary winding is a physical negative control.

GridLAB-D stores the second split-phase leg voltage with reversed polarity,
while its link current uses physical conductor polarity. The adapter negates
that voltage, preserves current polarity, and checks the separately reported
line-line voltage and powers. GridLAB-D's declared finite core shunt is included
in its reference; OpenDSS uses a separately evaluated zero-core-loss reference.
This calibration does not equate those two core models. OpenDSS fixtures contain
a regulator requesting a different voltage, but control mode is off. The
declared tap positions must remain unchanged before and after solving; outputs
record both states and identify them as source-declared, not engine-settled.

A reduced GridLAB-D triplex-line matrix is not an explicit-neutral model. Both
the adapter and future conversion checks must respect the pinned engine's
warning that a supplied 2×2 matrix cannot determine neutral/ground currents.
The center-tapped tests establish grounded service-terminal conventions; they
do not establish floating-neutral or triplex-cable equivalence.

## Electrical reader and IR

`powerio_dist::glm::parse` returns `PioModule<MulticonductorNetwork>` for an
initial static profile: phase-to-earth node/meter/load objects, fixed SWING
phasors, independent constant P/I/Z load terms, and overhead lines with explicit
phase impedance matrices. It materializes an earth return on buses; this is
not an explicit line-neutral model. Phase subsets retain their A/B/C identities.
Parented loads share their root electrical bus. Line impedances become ohms
per meter; enabled line capacitance becomes half the shunt admittance at each
end. Quoted unit suffixes retain their native interpretation and diagnostics.
Numeric setters are validated in source order even when later overwritten or
assigned to an inactive phase.

The reader rejects unmodeled physical properties, unknown globals, active
device classes, geometry-dependent lines, delta/split-phase flags, conflicting
child nominal voltages and names that collide under the current canonical
case-insensitive lookup. Coordinates attach to root buses, parented loads,
lines and used line configurations. Unmapped coordinate-bearing objects still
fail explicitly rather than inheriting an unrelated parent bus's location.
Instrumentation remains in retained source with a remark. No recorder, assertion
or controller executes during parsing. Facade dispatch accepts `gridlabd`
(PNNL 5.3, also inferred from `.glm`) and `gridlabd-arras` (Arras 4.3).

Build and run the reader qualification with:

```sh
cargo build -p powerio --example glm_network
python evals/gridlabd/network_oracle.py --pnnl-root "$PNNL_ROOT" \
  --reader target/debug/examples/glm_network --output /tmp/gld-network-reader
```

The bridge serializes the imported network to PowerIO IR, deserializes it and
requires identical network data. The evaluator uses the restored typed tables
to independently calculate branch currents, powers, losses and load-current
balance at each engine's solved voltages. It covers eight circuits per engine:
single-phase and coupled three-phase constant impedance, phase C alone, mixed
P/I/Z, a shifted source phasor, enabled line charging, and parented loads with
NR and FBS. The original constant-impedance circuits additionally match the
independent linear solve. Deliberate resistance, load-power, source-angle and
capacitance errors must fail. Reports include engine identity, input hashes,
typed network data, IR equality and each residual's absolute-plus-relative
bound. This qualifies reader physics and IR preservation; fresh/edited GLM
output and cross-format engine validation need separate checks.

## Canonical GLM output

`powerio_dist::glm::emit_canonical` takes a `MulticonductorNetwork` and a
destination. It generates one fresh GLM file from the current typed value;
there is no retained-source path in this API. It supports the reader's initial
phase-to-earth P/I/Z and explicit line-matrix profile. An unsupported electrical
feature fails before any artifact is written. Non-electrical metadata losses,
including legacy drawing coordinates, optimization bounds, ratings and source
cost data, are reported. Typed component geographic points are emitted on the
mapped objects; unavailable datum/origin fields are diagnosed. Facade emission
uses the same canonical writer for fresh, edited,
IR-restored and cross-profile modules.

The writer derives bus nominal voltages from connected fixed sources and
reconstructs each load's coefficients using its own stated nominal voltage.
Nonuniform load bases therefore retain the same physical law. Matrix indices
and source/load properties follow explicit phase identities even when model
terminal order differs from native A/B/C order. A source reference on an
already perfectly grounded terminal has the exact earth-referenced equivalent;
floating references are refused. Source zero phasors, noncapacitive or unequal
line end shunts, non-60-Hz cases, uninterpreted electrical extras, additional
device families and unqualified object-name escaping remain explicit gaps.

```sh
cargo build -p powerio --example glm_roundtrip
python evals/gridlabd/roundtrip_oracle.py --pnnl-root "$PNNL_ROOT" \
  --bridge target/debug/examples/glm_roundtrip --output /tmp/gld-canonical
```

For each of the eight reader cases and both engines, this bridge emits five
variants: fresh, IR-restored, edited, electrically equivalent load-base changes
and grounded source references. The harness runs the bare canonical file as
well as a separately instrumented copy. Fresh and IR-restored files must be
byte-identical; native electrical results of equivalent variants must match the
original case. Edits to loads, line length and source voltage must change the
result and satisfy the edited network equations. Constant-impedance cases also
match a separately solved analytic circuit. These checks qualify 80 canonical
engine solves, not just parser acceptance or retained-source echo. OpenDSS
cross-format equivalence remains unqualified.

## Facade profiles and source-project replay

Normal `powerio::parse` / `emit`, distribution dispatch and CLI conversion use
the explicit profiles above. GLM targets emit directory inventories: retained
same-profile projects keep their entrypoint and acquired includes under their
original relative names; canonical output is `case.glm`. The retained project
is never reopened from disk. Literal absolute includes cannot be relocated
byte exactly and are refused. Explicit preprocessing inputs are stored in the
module's `org.powerio.gridlabd` extension and replay reports when those external
inputs are required. Profile identity is module source metadata, preserving the
frozen IR network record and its existing `source_format` enum.

```sh
cargo build -p powerio --example glm_dispatch
python evals/gridlabd/dispatch_oracle.py --pnnl-root "$PNNL_ROOT" \
  --bridge target/debug/examples/glm_dispatch --output /tmp/gld-dispatch
```

This checks an included conditional that selects different load impedances
under the two native profiles. Same-profile replay must preserve both files
exactly. IR-restored, edited and cross-profile output must independently solve
with the declared target engine and satisfy the typed equations and analytic
circuit solution. The cross-profile result preserves the interpreted circuit,
not the source conditional's different meaning in the other fork. Rust tests
also cover deleting original files before replay, memory-only buffers, BOMs,
a widened acquisition root, external definitions, and atomic refusal of
nonportable projects. These checks add no device-family coverage to the
initial electrical profile.

## Typed geographic points and canonical output

`MulticonductorNetwork::component_locations` borrows typed geographic points
with exact component identities, optional axes and an explicitly optional
datum. Absent datum means unknown. This is separate from legacy drawings,
route vertices and conductor cross-section geometry. IR uses an additive
`powerio.LocatedMulticonductorNetwork` wrapper only when points are present;
the nested electrical body and all previous network records keep their original
schema. The streaming decoder does not buffer a second entire JSON network.
It checks duplicate/stale attachments, finite axis ranges and a 1,048,576 point
bound. Legacy readers reject the new mandatory type tag.

```sh
cargo build -p powerio --example glm_locations
python evals/gridlabd/location_roundtrip_oracle.py --pnnl-root "$PNNL_ROOT" \
  --bridge target/debug/examples/glm_locations --output /tmp/gld-locations
```

For both engines, this checks fresh, IR-restored, edited and geography-free
networks. Coordinates include DMS, assignment-time references, explicit zeros,
partial pairs, independent load points and a deliberately different line point.
Native saves independently confirm five object states per run; the initialized
line midpoint is checked separately from the retained declared point. Native
save rounding permits 1.4e-6 degrees; typed/source checks use 1e-12 degrees.
Swapped axes and identities must fail. All eight electrical snapshots must
remain equivalent and satisfy typed circuit residuals and an independent
constant-impedance solve. Coordinate edits never set electrical line length.

Mapped buses, loads, lines and used line configurations are covered. Additional
object-only attachments, coordinate cross-format projection, geographic layer
representation and dedicated C/Python/Julia point views remain unfinished.
Other format writers report point loss; GLM reports unsupported point origins
and declared datums without changing numeric angles.

## Published references and limits

`object_oracle.py` checks the explicit object index against both native engines.
Build `cargo build -p powerio-dist --example glm_objects` and pass that executable
as `--indexer`, along with `--pnnl-root` and a new `--output` directory. The case
uses sparse explicit IDs, a wildcard-class numeric reference, nested recorder
parenting, an inline line configuration, and range/count declarations. Named
parents and class inventories are compared with saved native objects. Native
equality assertions verify full line references, with a wrong-configuration
negative control; PNNL saved GLM omits inherited line fields and its recorder
truncates object-reference strings. Electrical results must also match the
independent analytic reference.

`GlmProject` maps syntax/reference errors back to original project buffers.
`ObjectIndex` borrows its properties and keeps explicit numeric IDs separate
from local ordinals. It retains repeated scalar properties and uninterpreted
syntax. The index alone does not establish electrical support: dynamic object creation,
namespaces, relative/wildcard-selection references, reassigned identities and
deferred repeated-reference ordering remain unqualified and are not silently
resolved by guessing. Duplicate numeric IDs are rejected, including Arras
declarations that could otherwise shadow an earlier index entry.

`preprocess_oracle.py` compares native include/define/conditional projects with
PowerIO-preprocessed text in both engines, and checks both against the analytic
single-phase reference. Build its driver with
`cargo build -p powerio-dist --example glm_preprocess`, then pass the resulting
executable using `--preprocessor`, along with `--pnnl-root` and a new `--output`
directory. This verifies preprocessing and source syntax, not canonical network
conversion. The identical input selects a 20+j5 ohm load in PNNL and a 40+j5 ohm
load in Arras: PNNL dereferences the left operand, while Arras defaults to a
literal left operand. Both compare strings, not numbers. Select `pnnl` or
`arras` when invoking the Rust driver directly. The case also shares definitions
across nested includes and excludes deliberately invalid inactive syntax.

The frontend resolves includes relative to the primary file, matching an engine
launched in that directory. Native GridLAB-D parses includes as declarations;
they are not arbitrary property fragments. Includes inside unfinished
declarations are currently rejected to avoid changing an independently created
object into a nested child. Environment variables, implicit `GLPATH` searches,
general typed global evaluation through `#set`, generated directives, executable macros
and remote includes remain unsupported and require further profile work.
The original project bytes and expansion source map remain separate from the
expanded text. The boolean preprocessing switches `reinclude`, `literal_if` and
`relax_undefined_if` are handled explicitly where the dialect supports them.
Line-comment stripping follows native preprocessing, including
its handling of comment markers before quoted-string syntax is parsed.

`coordinate_oracle.py` qualifies common object-header latitude and longitude in
both pinned GridLAB-D forks. Run it with `--output <new-directory>` and
`--pnnl-root <installation-directory>` and optionally
`--indexer /path/to/glm_objects` to compare PowerIO's declared coordinates.
It checks signed decimal, hemisphere
decimal-minute and degree-minute-second literals, coordinate references, missing
and partial points, and explicit zero coordinates. The comparison decodes native
saved GLM; its tolerance accounts for the engine writer's hundredth-of-an-arcsecond
rounding. The two forks use different saved object and quoting syntax. References
copy coordinates at assignment time; a nested reference before a later assignment
must retain the earlier value. Copying an absent axis keeps it absent.

Both engines replace the test line's declared location with its endpoint
midpoint during initialization. Thus the saved engine location is derived state,
not evidence that an importer preserved the declared location. The case records
both. It does not establish a geographic datum, a route polyline, a conductor
spacing, or an electrical line length. `ObjectIndex::declared_coordinates`
evaluates the supported source profile without assigning a CRS or inventing a
missing axis. Invalid ranges, nonfinite values and trailing junk fail explicitly
with original-source spans; the native engine's permissive numeric-prefix parsing
is not silently reproduced. Typed network attachment and fresh/edited/IR
conversion are qualified separately by `location_roundtrip_oracle.py`; this
source-coordinate harness alone does not establish them.

`switch_oracle.py` qualifies native closed-switch impedances independently of
PowerIO conversion. Run it with `--output <new-directory>` and
`--pnnl-root <installation-directory>`; `--profiles pnnl arras` is the default.
It uses a single-phase constant-impedance circuit, explicit module
`default_resistance 0.0001`, and the following base variants per engine:

| Solver and property | Effective series impedance (ohms) |
|---|---|
| NR, unspecified switch impedance | `0.0001 - 0.0001j` |
| NR, explicit `switch_impedance 0+0j` | `0.0001 + 0.0001j` |
| NR, explicit `switch_impedance 1+0.5j` | `1 + 0.5j` |
| FBS, unspecified switch impedance | `0` |
| FBS, explicit `switch_impedance 1+0.5j` | `1 + 0.5j` |

Three additional variants per engine check scalar-alias unit conversion,
complex/scalar assignment order and the fallback for negative components.

The harness compares complex terminal voltages, currents, powers and losses
against independent analytic solutions. It infers impedance from solved voltage
drop and current, and verifies that an ideal-switch assumption fails for every
nonzero case. The pinned PNNL and Arras implementations agree on these cases.
NR's zero-component fallback must not silently become an ideal canonical switch;
finite contacts need their actual impedance or a verified equivalent. This
calibration does not establish per-phase or banked switching, open contacts,
protection, controls, or PowerIO switch read/write support.

`switch_state_oracle.py` separately qualifies twelve NR phase-state cases per
fork: individual and banked contacts, majority decisions, two-phase ties,
overall status precedence, phase subsets and open contacts. Its coupled bypass
uses two series sections and an intermediate bus: the pinned solver rejects
a directly parallel switch/link as a duplicate admittance entry. The two-section
circuit retains the same analytic end-to-end impedance. The harness compares
32 complex electrical quantities per case, native saved phase states, and a
deliberately reversed contact-state reference. Arras omits default `CLOSED`
fields from its save; these omissions are recorded explicitly and conduction
is independently checked through the electrical measurements.

`switch_roundtrip_oracle.py` consumes the hashed successful state calibration
and the Rust `glm_roundtrip` bridge to check fresh, IR-restored and edited
canonical GLM. It aggregates contact measurements when mixed states have
become separate typed groups. Its independent reference applies the bridge's
explicit source, line-impedance and load-admittance edits. For example:

```sh
python evals/gridlabd/switch_state_oracle.py --output /tmp/switch-states --pnnl-root "$GLD_ROOT"
python evals/gridlabd/switch_roundtrip_oracle.py \
  --calibration /tmp/switch-states --impedance-calibration /tmp/switch-impedance \
  --output /tmp/switch-roundtrip --bridge target/debug/examples/glm_roundtrip \
  --pnnl-root "$GLD_ROOT"
```

The optional `--impedance-calibration` directory must come from a successful
`switch_oracle.py` run; it adds NR default/zero and explicit NR/FBS finite-contact
round trips. Ideal FBS contacts import as ideal typed switches, but their
canonical NR output equivalent remains unresolved and is explicitly excluded
from successful writer claims. Split-phase switches, explicit neutral contacts,
active controls/protection and additional global profiles remain unqualified.

The expanded impedance run currently exits nonzero: both pinned engines return
NaNs for the edited `nr_complex_alias_last` case under the emitted NR settings.
Its source is 247.2 V, series impedance is `6.6+7.7j` ohm, and load impedance is
`16+4j` ohm. The report retains each failed primary run and diagnostic runs
using a native switch under NR and the unchanged electrical circuit under FBS.
The native switch also fails; FBS matches all seven analytic electrical
quantities. A successful alternate-solver check never changes the primary NR
result to passed. This engine limitation is distinct from successful fresh
and restored runs and from the 72 state round trips. No tolerance or electrical
parameter is changed to hide it. The separate ideal-contact writer gap remains.

`quantity_oracle.py` compares `GlmProject::quantity` with native property
readbacks and the analytic single-phase circuit. Build the `glm_quantities`
example and supply it with `--reader`, alongside `--pnnl-root` and a new
`--output` directory. This validation schema covers selected properties only;
it is not a network reader or a canonical writer.

Quantity interpretation requires the native property kind and unit. It supports
finite decimal/exponent literals, rectangular and polar phasors, and a bounded
electrical/geometry unit profile with SI prefixes, lengths, powers, per-length
parameters and pinned native temperature/angle definitions. Unsupported units,
expressions and property references fail with original-source spans. Unit
expressions are bounded to 256 bytes, 32 factors and integer powers 2 through 16.

Quoting is significant: native GLM converts `5 kVA` in a complex property to
5000 VA, but the quoted string `"5 kVA"` takes a different conversion path and
retains 5 VA. PowerIO preserves this distinction and emits
`READ.GLM.UNIT_IGNORED` for the ignored suffix. Consumers must carry quantity
diagnostics into the resulting module. Arras also accepts quoted parenthesized
complex strings; unquoted parentheses require other expression semantics.
The harness uses 15-digit output and each complex value's own notation: forcing
rectangular output in the pinned Arras build can mislabel polar properties.
Source quantities and native readbacks use a relative tolerance of `1e-12`
with an absolute floor of `1e-12`; electrical checks retain the analytic
harness's quantity-specific tolerances.

`upstream.json` records original PNNL source URLs, commit, hashes, license and
assertion inventories. `upstream.py` runs the unmodified IEEE 13 NR and FBS
regressions (32 complex assertions each) and IEEE 37 NR (six). The 13-node
tests include 27 tolerances of 2 V, four of 8 V, and one of 10 V. The 37-node
test begins downstream of the regulator and checks line-line voltages within
0.5 V. These native regression passes establish engine sanity; their original
controls and solver settings remain intact. They are not evidence of frozen
state conversion or high-precision feeder equivalence.
Passing `--preprocessor <glm_preprocess-executable>` runs the assertions against
PowerIO-preprocessed copies instead. The report records the executed hash and
marks that mode explicitly; this still checks only preprocessing and syntax,
without electrical network conversion or frozen controls.

Only generated, original microcases are used for analytic calibration. Public
feeder inputs and generated engine outputs remain outside git. Other transformer
connections, floating split-phase services, load-law sweeps, explicit-neutral GLM
equivalents and larger public feeders require additional qualification. The
existing OpenDSS passive-admittance harness in `evals/validation/` remains a
separate check; it intentionally excludes transformer physics.

## Ordered OpenDSS source and line snapshots

The OpenDSS adapter resolves the static electrical state in native definition
and edit order. Circuit/Vsource and Line/LineCode copying, unit changes,
frequency selection and solve-time primitive invalidation are qualified against
DSS C-API 0.14.5. Unsupported source settings or an unmaterializable primitive
fail explicitly. Disabled sources retain their declared data without adding
live voltage constraints or impedance branches.

A typed voltage source supplies an ideal EMF constraint. Imported finite source
impedance is a separate coupled branch; floating returns use explicit reference
terminals. Canonical DSS writes ideal constraints with exact cancelling-resistance
circuits, preserving phase-specific phasors and source returns. It never turns a
finite impedance into an epsilon or assumes that a native default source is ideal.
The generated helper ports must be aggregated when measuring original source
current and power.

Each Line owns its ordered effective matrix state. LineCode definitions remain
retained provenance rather than live electrical dependencies. Native `like`
operations can retain different cached and current matrices; the snapshot
resolver follows those rules. Unresolved geometry remains source data with a
readiness blocker instead of acquiring fabricated conductor counts or default
impedance. This prerequisite does not implement the GLM network reader or its
geometry conversion.

The source suites generate synthetic inputs outside git, then evaluate public
import, IR and canonical output through the CLI:

```sh
cargo build -p powerio-cli --bin powerio
python evals/gridlabd/dss_source_lowering_oracle.py prepare --output /tmp/dss-source-import
python evals/gridlabd/dss_source_lowering_oracle.py evaluate \
  --output /tmp/dss-source-import --public --powerio target/debug/powerio
python evals/gridlabd/dss_source_writer_oracle.py prepare --output /tmp/dss-source-output
python evals/gridlabd/dss_source_writer_oracle.py evaluate \
  --output /tmp/dss-source-output --powerio target/debug/powerio
```

The loaded references use independent coupled-source MNA and native terminal
observations. Source voltage, current, power, loss and impedance comparisons keep
the calibration budgets from `oracle.py`; floating/earth returns, phase patterns,
P/I/Z loads, IR reconstruction and repeated canonical output are separate cases.
Bad impedance, incorrect grounding and changed source state must not pass.

`dss_source_state_oracle.py`, `dss_source_enabled_oracle.py`,
`dss_source_validity_oracle.py` and `dss_source_profile_oracle.py` qualify their
named property and state profiles. `dss_line_history_oracle.py`,
`dss_linecode_oracle.py` and `dss_line_oracle.py` qualify ordered line definitions
and native primitive state. Their `--help` describes input/export requirements.
Ignored Rust exporters for these suites accept DSS projects; GLM-specific
exporters and overhead-geometry cross-format checks belong to the later GLM
reader/geometry work.

A pass applies to its named snapshot profile and engine pin. Active controllers,
protection sequences, load schedules and behavioral simulation are not executed.
The complete static compatibility target still includes additional neutral,
geometry, transformer and device profiles; these are not completed by source and
line snapshot qualification alone.
