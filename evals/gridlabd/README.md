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

## Overhead conductor geometry calibration

`overhead_geometry_oracle.py` checks synthetic A/AN, BC/BCN and ABC/ABCN
cross sections at 60 Hz, plus ABCN at 50 Hz. These conductor positions are
cross-section dimensions in feet; they are unrelated to geographic component
latitude/longitude. The harness does not yet qualify PowerIO geometry import.

```sh
python evals/gridlabd/overhead_geometry_oracle.py --pnnl-root "$PNNL_ROOT" \
  --output /tmp/gld-overhead
```

The seven circuits exercise both pinned GridLAB-D forks using NR with/without
capacitance and FBS without capacitance: 42 native runs. Independent modified
Carson and image-potential calculations check series impedance, total shunt
admittance, complex voltages, both-end currents/powers and losses. The shunt
readback comes from the native `a` and `b` matrices, without inserting expected
capacitances into the observations. Neutral-removal negative controls expose
the effect of the return conductor.

Fourteen OpenDSS checks cover full and explicitly reduced conductor matrices
under SimpleCarson. The engine's rounded SI constants differ from GridLAB-D's
rounded imperial constants; each profile is checked against its own equations.
Setting the OpenDSS default earth model alone does not activate that model for
the geometry API. The harness first solves a line explicitly selecting Carson,
then reads its geometry matrices. Deri and FullCarson remain outside this
calibration, rather than being relabeled equivalent.

For both pinned GridLAB-D forks, series neutral reduction is the Schur
complement and shunt reduction takes the phase block of the full admittance.
An independent four-conductor calculation verifies the corresponding
zero-neutral-voltage boundary condition, including nonzero neutral current.
This does not establish arbitrary floating-neutral or finite-grounding support.

All 42 native matrix/voltage cases match, but only 28 match every physical port
quantity. Fourteen retain reported-current or loss discrepancies: coupled
capacitance uses `d=a=I+ZY/2` in native link-current reporting where a physical
pi circuit requires `d=I+YZ/2`; NR's matching bus voltages do not validate those
reported currents. Native total loss also sums the absolute real loss of each
phase, overstating dissipation when mutual coupling transfers real power
between phases. Separate `native_readback_equations` comparisons reproduce
these behaviors in all 42 cases. They never replace the physical comparisons.
The complete calibration intentionally exits nonzero while those physical
discrepancies remain. FBS with coupled capacitance has separate earlier
qualification and is not covered by these 42 runs.

`--native-evidence /tmp/gld-overhead` rechecks acquired native results after
verifying every generated input hash and pinned version; OpenDSS matrix checks
still execute. Both the original evidence hash and each native output directory
are recorded. No failed physical comparison becomes a successful conversion.

The equations and readback behavior were audited in the commits pinned by
`engines.json`: GridLAB-D `overhead_line.cpp`, `line.h`, `line.cpp` and
`link.cpp`; DSS `LineConstants.pas`, `LineGeometry.pas` and
`CAPI_LineGeometries.pas`. Geometry-derived neutral reduction, native readback
discrepancies and cross-engine earth-model differences require explicit
provenance or diagnostics when conversion support is added.

## Overhead geometry conversion

The reader materializes `overhead_line_conductor` and `line_spacing` references
as linecode phase matrices, retaining the native reduced-neutral calculation
in linecode provenance and reporting `READ.GLM.GEOMETRY_MATERIALIZED`.
GMR, resistance, diameter and spacing use native units with normal unit
conversion. Explicit nonzero diagonal configuration impedances select the
native matrix path before geometry. Active geometry requires positive finite
GMR/resistance/distances; enabled capacitance additionally requires diameters
and heights. Unsupported native invalid-value fallbacks fail explicitly.

Ordinary 50/60-Hz module globals survive import, IR and canonical output.
Canonical geometry output uses current matrices, with a diagnostic for the
original conductor/spacing provenance that cannot be reconstructed. Map points
on the line/configuration retain their typed attachment. Points on conductor
or spacing objects without a typed attachment still fail explicitly.

```sh
python evals/gridlabd/overhead_roundtrip_oracle.py --pnnl-root "$PNNL_ROOT" \
  --bridge target/debug/examples/glm_roundtrip --output /tmp/gld-geometry-roundtrip
```

Seven phase/frequency cases, each with capacitance on/off, run in both forks.
Fresh, IR-restored, edited, voltage-rebased and grounded-source-reference output
is checked against independent physical circuit equations. Edits change source
voltage, load admittance and line length (both series and shunt quantities).
Noncommuting shunts use exact endpoint load equivalents in NR as well as FBS.
The oracle aggregates native series currents and native end-shunt coefficient
readback, then separately checks an independent root-meter source current.
Total physical loss is the signed sum of observed physical port powers; native
`power_losses` remains recorded separately. Geometry neutral reduction is
verified for its stated source boundary condition, not claimed as full
explicit-neutral or arbitrary-grounding support.

The companion cross-format audit uses the existing CLI and preserves the
original fixed-source reference:

```sh
python evals/gridlabd/overhead_dss_oracle.py --powerio target/debug/powerio \
  --output /tmp/gld-geometry-dss
```

All 14 line-matrix comparisons pass, but all 14 complete-circuit comparisons
currently fail. The DSS writer leaves unspecified source impedance at native
finite defaults, and B/C sources emitted as two-phase acquire a 180-degree
phase separation instead of the original 120 degrees. Actual source settings
and voltages are recorded. No tolerance relaxation or substitution of observed
source voltage is used to declare cross-format success. These source-export
gaps remain unresolved and prevent an OpenDSS compatibility claim.

## Electrical reader and IR

`powerio_dist::glm::parse` returns `PioModule<MulticonductorNetwork>` for an
initial static profile: phase-to-earth node/meter/load objects, fixed SWING
phasors, independent constant P/I/Z load terms (legacy A/B/C and explicit
AB/BC/CA/AN/BN/CN connections), and overhead lines with explicit
phase impedance matrices. It materializes an earth return on buses; this is
not an explicit line-neutral model. Phase subsets retain their A/B/C identities.
Parented loads share their root electrical bus. Line impedances become ohms
per meter; enabled line capacitance becomes half the shunt admittance at each
end. Quoted unit suffixes retain their native interpretation and diagnostics.
Numeric setters are validated in source order even when later overwritten or
assigned to an inactive phase.

The reader rejects unmodeled physical properties, unknown globals, active
device classes, unqualified geometry/cable models, split-phase flags, conflicting
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

## Electrical parent chains

`parent_oracle.py` separately calibrates electrical parent chains through
node/meter/load ancestors. It retains a direct sibling demand while testing
two- and three-level chains, phase subsets, leaf-first declarations and
independent P/I/Z demands at intermediate levels. Across both forks and NR/FBS,
43 of 56 native inputs match the full independent circuit. Arras NR omits deeper
loads in six inputs and crashes in seven; these outcomes remain visible and
the full calibration exits nonzero. The reader explicitly refuses those
Arras NR hierarchies rather than silently flattening a different circuit.

```sh
python evals/gridlabd/parent_oracle.py --pnnl-root "$PNNL_ROOT" \
  --output /tmp/gld-parents
python evals/gridlabd/parent_roundtrip_oracle.py --pnnl-root "$PNNL_ROOT" \
  --native-report /tmp/gld-parents/report.json \
  --bridge target/debug/examples/glm_roundtrip --output /tmp/gld-parent-output
```

The second harness rechecks native records and input hashes, verifies all
thirteen source refusals occur before any IR/GLM artifact is written, and
checks five canonical variants for each of the 43 accepted inputs. Identical
canonical bytes share one native execution per fork; this is reported as
215 output/reference checks over twelve distinct canonical engine runs, not
215 separate executions. Omitting the nested demand must fail the reference.
Fresh/IR output equality and edited coefficients are checked independently of
retained-source echo. The native Arras NR limitation is not resolved by these
canonical results. `--case` selects named inputs for focused investigation.

## Canonical GLM output

`powerio_dist::glm::emit_canonical` takes a `MulticonductorNetwork` and a
destination. It generates one fresh GLM file from the current typed value;
there is no retained-source path in this API. It supports the reader's initial
phase-to-earth and explicit phase-pair P/I/Z and line-matrix profile. An unsupported electrical
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
floating references are refused. Source zero phasors, nonreciprocal end-shunt
equivalents, frequencies outside 50/60 Hz, uninterpreted electrical extras, additional
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

## Explicit phase-pair loads

The explicit `constant_power_*`, `constant_current_*` and
`constant_impedance_*` properties for AB/BC/CA/AN/BN/CN map to ordered
two-terminal loads. Legacy A/B/C and explicit terms are additive. Current
coefficients follow the actual connection voltage angle; they are not fixed
current phasors. The writer reconstructs each connection's physical coefficient
from its own nominal voltage. It also emits complete three-channel typed delta
loads as explicit pairs, diagnosing the identity split on subsequent import.
Traditional GLM D flags and scalar real/reactive aliases use the mappings below.
Incomplete/open-delta typed channel layouts still require additional mapping.

```sh
cargo build -p powerio --example glm_roundtrip
python evals/gridlabd/pair_load_oracle.py --pnnl-root "$PNNL_ROOT" \
  --bridge target/debug/examples/glm_roundtrip --output /tmp/gld-pairs
```

Eight circuits exercise pure pair impedances, angle-following currents, mixed
legacy/explicit P/I/Z and capacitive pairs. Both pinned forks run NR and FBS;
FBS cases include an exact ideal contact. Each native baseline and five canonical
variants (fresh, IR-restored, edited, rebased and grounded-reference) must match
an independent nonlinear circuit solve: 192 runs and 5,376 complex comparisons.
The reference checks KCL to 1e-9 A. Every case must reject a changed connection;
current cases must also reject a fixed-phasor interpretation. Merely reversing
an impedance's terminals is electrically identical and is not a valid negative
control. This qualifies explicit connection laws, not split-phase
or cross-format compatibility.

### Traditional delta flags and scalar setters

On a `load`, the `D` flag connects legacy A/B/C demand terms to AB/BC/CA.
`nominal_voltage` remains the phase-to-earth base; each pair uses √3 times
that voltage. Legacy and explicit properties remain additive, even on the
same pair. `D` on node/meter/load objects preserves ordinary source phasor
references and does not introduce a line-neutral conductor. Link `D` flags
are not yet qualified. A stated load channel with an absent endpoint fails,
including an explicitly stated zero channel.

The `_real` and `_reac` setters mutate the corresponding complex property in
source order for all nine legacy/explicit suffixes and all three P/I/Z laws.
Power parts use W/VAr, currents use A and impedances use Ohm; supported scaled
units convert before assignment. A later full complex setter replaces both
parts. Every setter is validated, including values subsequently overwritten.

```sh
python evals/gridlabd/delta_load_oracle.py --pnnl-root "$PNNL_ROOT" \
  --bridge target/debug/examples/glm_roundtrip --output /tmp/gld-delta
```

Twelve synthetic circuits exercise individual legacy delta channels, phase
subsets, mixed legacy/explicit laws, a root delta load and both setter orders
in wye and delta. Both pinned forks run NR and FBS. Each native input and five
canonical variants must match the independent nonlinear KCL reference. A
wye/delta substitution must fail; current cases also reject fixed phasors.
The generator asserts its root-object substitution so a missing injection
cannot accidentally qualify the circuit. This covers these static demand
laws; it does not establish transformer, split-phase or full feeder support.

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
round trips. Ideal FBS contacts import as ideal typed switches and are qualified
by the separate FBS harness below. Split-phase switches, explicit neutral contacts,
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
parameter is changed to hide it.

`ideal_switch_oracle.py` checks ideal closed-contact output on a source-oriented
forest. Ten supported circuits per fork cover single A/C phases, AC subsets, three-phase
and mixed phase states, coupled impedances, mixed P/I/Z loads and reversed input
links and commuting line end-shunt matrices. The baseline uses native ideal contacts; fresh, IR-restored, edited,
nominally rebased and grounded-reference output are solved independently.
Linear solves and scalar ZIP equations provide the reference. A `1e-4` ohm
contact replacement must disagree, so a finite NR approximation cannot pass.

```sh
python evals/gridlabd/ideal_switch_oracle.py \
  --output /tmp/ideal-switches --bridge target/debug/examples/glm_roundtrip \
  --pnnl-root "$GLD_ROOT"
```

FBS output recombines disjoint phase groups and orients every link away from its
source. Native probes showed that separate mixed groups can fail and reversed
links can produce wrong voltages even with exit zero. The writer checks the
whole link forest, including open ties, and requires one source per component
and incoming phase coverage. Cycles, overlapping parallel contacts, conflicting
grouped points and disconnected phase attachments remain explicit output gaps.
The writer does not modify the network or replace ideal constraints by epsilon
impedances. This qualification does not resolve the NR numerical failure above.

The full run deliberately includes an additional noncommuting capacitance case
and exits nonzero. Both native FBS engines produce correct voltages but wrong
source-end currents and losses. The discrepancy matches `(Z Y - Y Z) I_out`,
where `Y` is the one-end shunt admittance: the native line uses `d=a`, while an
exact π line needs `a=I+Z Y` and `d=I+Y Z`. The report retains that quantitative
witness. Canonical emission uses exact end-shunt loads and passes the physical
pi-line reference in both forks, while the original native failures remain
visible. `--case` selects named circuits for
focused investigation, but a selected passing subset is not full qualification.

`line_shunt_oracle.py` qualifies conductive, inductive and unequal end shunts,
including source-only, sink-only and reversed-line cases under NR and FBS.
The `glm_roundtrip` validation bridge accepts an optional fourth argument with
typed per-metre `g_from`, `b_from`, `g_to` and `b_to` matrices, applied before IR
serialization. Five variants in each of eight solver/case combinations run in
both forks. For example:

```sh
python evals/gridlabd/line_shunt_oracle.py --pnnl-root "$PNNL_ROOT" \
  --bridge target/debug/examples/glm_roundtrip --output /tmp/gld-end-shunts
```

The reference independently solves the full pi circuit and checks KCL. The
observable line ports aggregate the generated shunts and native series link.
FBS uses zero-drop parented meters for each shunt. Arras NR silently ignores
grandchild loads below a parented meter, so NR keeps the original hierarchy,
reads native impedance coefficients and solved voltages, and checks an
independent root-meter source-current witness. The witness accounts for native
meter accumulation of outgoing links when a line is reversed. Expected shunt
currents are never inserted into observed totals. Omitted shunts, swapped ends
and bare series-link port comparisons must all fail.
Each fresh case also runs without the shunt-meter instrumentation; all bus
voltages and bare series-link currents, powers and losses must stay unchanged.

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

## Underground cable conversion

`cable_oracle.py` compares conductor geometry with native impedance dumps and an
independent block-matrix reduction. It checks voltages, currents, powers and
charging admittances for native inputs, canonical output and edited networks.
IR-restored output must match fresh canonical bytes. Source root meters witness
physical current independently of native link reporting; removing return paths
(or perturbing bare-cable resistance) must fail the comparison.

```sh
cargo build -p powerio --example glm_roundtrip
python evals/gridlabd/cable_oracle.py --bridge target/debug/examples/glm_roundtrip --pnnl-root "$PNNL_ROOT" --output /tmp/gld-cables
```

The cases include IEEE13's CN606/CN607 parameters, phase subsets, a separate
neutral, tape shields, capacitance and 50/60 Hz. PNNL and Arras have different
neutral reductions, so each uses its own reference profile. PNNL unshielded
geometry and Arras shield geometry without N are checked as native-invalid
inputs, separately from supported electrical cases. These results do not yet
qualify triplex cables, OpenDSS CNData/TSData decoding or a complete feeder.

## Bounded load laws

`load_voltage_oracle.py --powerio /path/to/powerio --output /new/path` checks
55 static cases for OpenDSS load models 1, 2, 5 and 8 across the voltage band
and dropout transition. It parses through public IR, emits canonical DSS and
compares native complex load voltages, currents and powers. Run the ignored
`typed_laws_match_native_load_currents` test in `powerio/tests/load_voltage_ir.rs`
with `POWERIO_LOAD_VOLTAGE_SAMPLES` set to the resulting `report.json` to verify
the Rust evaluator against the same independent native samples. The report
records the pinned engine version and CLI/input hashes. These microcases do
not by themselves qualify a complete feeder or the remaining load models.

## Ordinary transformer conversion

`transformer_roundtrip_oracle.py` uses independent ideal-coil and series-impedance
equations to check loaded single-phase A/C and unbalanced three-phase grounded
wye transformers. Both pinned GridLAB-D forks run the original, fresh canonical
and edited network; OpenDSS runs fresh and edited conversions. Edits change
source voltage, load demand, winding voltage, resistance and reactance. A wrong
turns ratio must fail, and IR-restored GLM must equal fresh canonical output.

```sh
cargo build -p powerio --example glm_roundtrip
python evals/gridlabd/transformer_roundtrip_oracle.py --bridge target/debug/examples/glm_roundtrip --pnnl-root "$PNNL_ROOT" --output /tmp/gld-transformers
```

This qualifies ordinary NR series-coil behavior, not FBS, magnetizing branches,
regulators, split-phase services or a complete public feeder. The earlier
`transformer_oracle.py` calibrates engine coil conventions without PowerIO.

## Fixed regulators and IEEE13

`regulator_oracle.py` compares Type A/B phase ratios and series resistance with
independent coil equations. It exercises unequal taps, phase subsets, object tap
overrides, native default resistance, edited winding values and OpenDSS output.
Every case must detect a deliberately wrong turns ratio.

`feeder_oracle.py` defaults to the hash-pinned IEEE13 NR input in `upstream.json`;
its IEEE37 profile is described below.
It compares 257 complex quantities under both pinned GridLAB-D forks and the
pinned OpenDSS engine: all 38 bus-phase voltages, three source currents,
156 line/transformer terminal current, power and loss quantities, and 60
per-phase current/power quantities for the ten original load objects. Canonical
load pieces are aggregated by their original object identity. Every bus also
passes Kirchhoff current balance, including the independently metered root.

Fresh and IR-restored GLM must be byte-identical. Edited output is compared
with independently edited original GLM, without reading the converted electrical
parameters: loads change by 25%, line lengths/contact impedances by 10%,
winding resistances by 20%, reactances by -10%, secondary ratings by 5%,
regulator primary taps by 2%, and source phase A by 3%. The edited native
regulator reference uses Type A; canonical output uses Type B. Removing a
regulator boost and substituting stale unedited output must both be detected.

The harness removes only the incompatible output-only `voltdump`, replaces the
root node by an electrically equivalent meter, and instruments the existing
parent relationships. Native original voltage assertions remain enabled;
assertions are removed for deliberate edits. Both original and canonical GLM
use a 1e-9 V convergence threshold. OpenDSS uses controls off, tolerance 1e-12
and at most 100 iterations; its default-solve measurements remain separately
recorded, not used as strict acceptance evidence.

For original lines, native a/b impedance dumps and solved voltages determine
physical pi-circuit ports. GridLAB-D's published input-current calculation uses
a different matrix order for noncommuting shunts; its raw output is preserved
and independently checked against native a/b/c/d equations. Canonical generated
end shunts and P/I/Z loads use native coefficient readback and solved voltages,
without inserting meters that change NR parent topology. Loss means the sum of
terminal powers; published phase-clamped `power_losses` is retained separately.

```sh
cargo build -p powerio --example glm_roundtrip
python evals/gridlabd/regulator_oracle.py --bridge target/debug/examples/glm_roundtrip --pnnl-root "$PNNL_ROOT" --output /tmp/gld-regulators
python evals/gridlabd/feeder_oracle.py --bridge target/debug/examples/glm_roundtrip --source /path/to/autotest/test_IEEE_13_NR.glm --pnnl-root "$PNNL_ROOT" --output /tmp/gld-ieee13
```

This qualifies the pinned IEEE13 NR case and these edits, including all its
static device ports. It does not qualify other feeders, FBS, explicit-neutral
networks, split-phase services or the remaining transformer/load profiles.


## IEEE37 and native NR delta declarations

The same full-feeder harness accepts `--case ieee37`, with the pinned IEEE37
input and its configuration file kept in the `upstream.py` directory layout.
The bridge passes that directory as an explicit `Source` acquisition root;
ordinary parsing never widens containment automatically. The profiler flag is
retained as an output-only setting. Native instrumentation flattens only the
hash-checked configuration include and overrides the source's polar output
setting to match the recorder's rectangular format.

```sh
python evals/gridlabd/feeder_oracle.py --case ieee37 --bridge target/debug/examples/glm_roundtrip --source /path/to/autotest/test_IEEE_37_NR.glm --pnnl-root "$PNNL_ROOT" --output /tmp/gld-ieee37
python evals/gridlabd/transformer_roundtrip_oracle.py --connection DELTA_DELTA --bridge target/debug/examples/glm_roundtrip --pnnl-root "$PNNL_ROOT" --output /tmp/gld-nr-delta
```

IEEE37 acceptance covers 732 complex quantities: 111 bus-phase voltages, three
source currents, 468 branch terminal current/power/loss quantities, and 150
phase current/power quantities across 25 original load objects. The same
independent edits, every-bus current balance and native reporting-law checks
apply. A wrong transformer ratio and stale unedited output must fail. Original
input retains all six published line-line voltage assertions; the feeder begins
downstream of its regulator, so this is not open-delta regulator qualification.

Both pinned engines implement NR `DELTA_DELTA` with grounded-wye port equations.
The reader records that effective circuit and diagnoses the equivalent; the
canonical writer emits `WYE_WYE`. `transformer_roundtrip_oracle.py` additionally
checks this behavior with unequal ground-referenced impedances, witnessing
nonzero zero-sequence current. A physical OpenDSS delta/delta winding is an
explicit negative control. Matching this native profile does not establish
physical delta isolation or the separate FBS transformer equations. The pinned
[PNNL transformer implementation](https://github.com/gridlab-d/gridlab-d/blob/1b2e463d3c149e999a2c5f04213477f545e01dc6/powerflow/transformer.cpp)
and [link stamps](https://github.com/gridlab-d/gridlab-d/blob/1b2e463d3c149e999a2c5f04213477f545e01dc6/powerflow/link.cpp)
provide the native equations; the engine checks independently verify the mapping.

### IEEE feeder corpus

`ieee-corpus.json` catalogs pinned public feeder inputs and published result
sources, including IEEE4/13/34/37/123/8500/9500 and low-voltage/neutral cases.
It is an acquisition inventory, not a compatibility claim. Keep downloaded
models outside the repository; review the source license before redistributing.

The `upstream.py` downloader includes IEEE4 and the IEEE34/IEEE123 NR cases and
their configuration includes. Files without assertions are acquired only;
the feeder oracle adds bounded recorder runs. Run a fixed-state conversion check with:

```sh
python evals/gridlabd/feeder_oracle.py --case ieee34 --baseline-only \
  --source /path/to/upstream/autotest/test_IEEE_34_NR.glm \
  --bridge /path/to/glm_roundtrip --pnnl-root /path/to/GridLAB-D \
  --output /path/to/new-report-directory
```

For IEEE4 use `--case ieee4 --baseline-only` and source
`models/powerflow_IEEE_4node.glm`. Its default FBS solver, implicit upstream
source, numeric grounded-wye connection and declared phase ratings are retained.
Instrumentation names its three numeric-ID links for measurement. The legacy
clock parses its timestamp before its EST timezone; the recorder stops one
second after that actual start. The native default core admittance remains in
fresh output. Omitting it must fail, as must a wrong transformer ratio.

For IEEE123 use `--case ieee123 --baseline-only` and source
`autotest/test_IEEE123_zero_voltages_NR.glm`. The original IEEE123 assertions
check absent-phase zero voltages; the oracle measures the energized circuit too.

| Fixed-state profile | Buses | Lines | Winding equivalents | Loads | Capacitor coils | Complex quantities |
|---|---:|---:|---:|---:|---:|---:|
| IEEE4 FBS grounded-wye step-down | 4 | 2 | 1 | 1 | 0 | 60 |
| IEEE34 NR | 55 | 51 | 7 | 70 | 6 | 1,000 |
| IEEE123 NR | 130 | 124 | 10 | 89 | 6 | 2,029 |

Each fork compares the complete inventory with canonical GLM and OpenDSS and
checks current balance at every bus. These profiles do not yet qualify edited
IEEE4/34/123 states or other IEEE4 transformer variants. IEEE13/37 retain their edited-state checks. Fresh and
IR-restored canonical GLM must be identical. IEEE123's unused center-tapped and
triplex equipment definitions stay in source; no service circuit is instantiated.

IEEE34 needs a native stopping tolerance of `1e-6 V`; tighter settings failed
convergence on the pinned engine. Voltage/current acceptance limits remain those
in `oracle.py`. Its absolute power budget is twice the maximum source voltage
multiplied by the absolute current tolerance, to account for subtracting two
terminal powers at roughly 15 kV. The report also retains results against the
stricter microcase power threshold. Native source-meter current, which follows
GridLAB-D's noncommuting line-shunt reporting law, is retained separately;
compared source current sums physical native ports. Recorder runs are bounded
to one second and use 17 significant digits.


### IEEE8500 static reference

`ieee8500_reference.py` pins the IEEE8500 input catalogued from
`GRIDAPPSD/Powergrid-Models/archive/GLM/IEEE_8500node.glm`. It compares the
original PNNL circuit with explicit static versions on PNNL and Arras. Three
VARVOLT capacitor banks use their declared switch states; the fourth bank and
all regulator taps are already manual. All four banks and all regulator taps
are recorded. The 1,177 loaded `triplex_node` objects become `triplex_load`
objects with `constant_power_1/constant_power_2`, because Arras hides the older
node `power_1/power_2` properties. Source bytes must match the pinned digest.

The original lowercase voltage/current dump modes are not portable: PNNL uses
`RECT` for voltage, Arras uses `rect`, and both use `RECT` for current. Correcting
these output settings changes no electrical fields. Runs stop after one second;
recorders use one-second intervals. The three controlled banks are frozen only
in the static variants. Complete inventories contain 4,879 voltage rows and
4,876 current rows, giving 29,265 complex values including absent phases.

```sh
python evals/gridlabd/ieee8500_reference.py --source /path/to/IEEE_8500node.glm \
  --pnnl-root /path/to/GridLAB-D --output /path/to/new-reference-directory
```

This is native reference preparation at six-decimal dump precision. It is not
PowerIO conversion qualification, a full-precision port/loss comparison, or an
edited-model check. Original engine failures and source adaptations must remain
distinct from converter results. Dataset contents remain outside git.


### Center-tapped service import

`split_phase_import_oracle.py` validates actual GLM import, IR restoration and
OpenDSS emission for the grounded service circuit used by IEEE8500. Six small
synthetic circuits cover primary phases A/B/C, NR and FBS, unequal secondary
impedances, mixed P/I/Z terms, default/copy rules and IEEE8500 service parameters.
Both pinned forks supply native reference measurements. The linear cases also
compare against the independent ideal-core circuit equations. Deliberately
reversing a secondary, removing core loss or changing the rating must fail.

```sh
cargo build -p powerio --example glm_network
python evals/gridlabd/split_phase_import_oracle.py \
  --bridge target/debug/examples/glm_network --pnnl-root /path/to/GridLAB-D \
  --output /path/to/new-service-import-directory
```

The typed equivalent uses four windings and a constant-impedance load at an
auxiliary core bus. Its zero-leakage fourth winding exposes the internal core;
placing that load at an external terminal changes primary copper losses.
Loss measurements aggregate the three external ports, including the separate
core load. Secondary leg 2 has reversed terminal order, matching the native
`voltage_2` convention. The comparison uses the existing full-precision V/A/VA
budgets, not the IEEE8500 dump precision.

This qualifies grounded service import and OpenDSS emission only. Triplex geometry
has separate qualification below. Direct triplex matrices, floating neutrals,
canonical GLM service writing and complete IEEE8500 feeder conversion remain
unfinished. PNNL's legacy `triplex_node.power_*` inputs are
accepted only by the PNNL import profile; Arras requires `triplex_load` fields.


### Connected triplex geometry import

`triplex_import_oracle.py` extends the grounded service circuit with a triplex
line. It compares 30 complex observables per case: transformer ports, both line
ends including neutral current, load voltages and losses. The six cases cover
A/B/C primaries, NR/FBS, unequal wires, 50/60 Hz, mixed P/I/Z loads, IEEE8500
parameters and an upstream overhead line with omitted mutual spacing. Two
linear cases also use independent ideal-core and three-conductor circuit
solutions. Floating the receiving neutral deliberately must fail.

```sh
cargo build -p powerio --example glm_network
python evals/gridlabd/triplex_import_oracle.py \
  --bridge target/debug/examples/glm_network --pnnl-root /path/to/GridLAB-D \
  --output /path/to/new-triplex-import-directory
```

The typed line keeps the full primitive impedance matrix and both neutral
terminals are grounded. The native hot-port reduction is its Schur complement;
the primitive also recovers the native neutral current. Ground current can flow
at a bus independently of current in the neutral conductor. These conventions
must not be replaced with a floating-neutral series circuit.

The missing-spacing case preserves the pinned engine's zero mutual impedance,
with an import diagnostic. Seasonal ampacities remain in source with an explicit
projection diagnostic. This evidence does not qualify direct triplex 2×2
matrices, floating-neutral service import, canonical GLM service writing or the
complete IEEE8500 feeder.


### Delta–grounded-wye import

`delta_grounded_import_oracle.py` validates the native `DELTA_GWYE` connection
through actual import, IR restoration and OpenDSS emission. Seven circuits
cover NR/FBS step-down, step-up, equal line-voltage ratings and IEEE8500's
115/12.47 kV, 27.5 MVA, 0.01344+j0.1551 pu substation parameters. Each uses
unbalanced impedances and source phasors with a common-mode component.

```sh
cargo build -p powerio --example glm_network
python evals/gridlabd/delta_grounded_import_oracle.py \
  --bridge target/debug/examples/glm_network --pnnl-root /path/to/GridLAB-D \
  --output /path/to/new-delta-grounded-import-directory
```

Each case compares 19 complex V/A/VA observables: both buses' phase voltages,
all physical phase-port currents and powers, and total transformer losses.
An independent ideal-coil circuit provides a linear reference. A deliberately
grounded primary coil must fail, guarding against loss of delta isolation.
The typed equivalent consists of three ordinary phase-pair primary coils with
grounded secondary coils. It adds no internal buses or new model types.
Native step-down and step-up orientations differ; equal line-voltage ratings
use the step-up orientation. Neither solver adds a core branch for this connection.

This qualifies the reader and OpenDSS projection, not canonical GLM writing of
phase-pair coils or the complete IEEE8500 feeder. The full feeder now has separate import qualification below.


### FBS regulator and complete IEEE8500 import

`fbs_regulator_import_oracle.py` checks Type A/B taps, phase subsets, object tap
overrides and ignored FBS resistance against both pinned forks and independent
ideal-ratio equations. The reader stores zero leakage. OpenDSS's directly zero
transformer impedance is singular, so emission uses one-ohm secondary leakage
and an equal negative series resistance. At the unloaded internal node, KCL
cancels them exactly. Physical to-port powers and total losses include the added
line. The oracle removes cancellation deliberately and requires a mismatch.

```sh
cargo build -p powerio --example glm_network
python evals/gridlabd/fbs_regulator_import_oracle.py \
  --bridge target/debug/examples/glm_network --pnnl-root /path/to/GridLAB-D \
  --output /path/to/new-fbs-regulator-directory
python evals/gridlabd/ieee8500_import_oracle.py \
  --source /path/to/IEEE_8500node.glm \
  --bridge target/debug/examples/glm_network --pnnl-root /path/to/GridLAB-D \
  --output /path/to/new-ieee8500-import-directory
```

The IEEE8500 harness uses the external file pinned by `ieee8500_reference.py`;
no dataset contents are vendored. It retains the documented portable static
adaptations, compares the original PNNL import with that circuit, and checks
actual IR-restored OpenDSS emission against both native engines. It compares
82,901 complex observations and checks current balance at 8,525 non-source,
non-ground bus terminals in each solution. These include all 4,879 native bus
objects and 4,876 links, with explicit aggregation of transformer and switch
circuit equivalents. A 1% error in one feeder-regulator tap must fail.

The native default voltage error is a fraction of nominal voltage: the source
file's `1e-4` permits much larger differences than the comparison budget. This
harness uses `1e-12` and full-precision rectangular recordings. OpenDSS uses
`tolerance=1e-12`, `maxiterations=1000`, and disabled controls. The shared V/A/VA
comparison budgets are unchanged.

Physical losses are the signed sum of external port powers. Native GridLAB-D
ordinary links instead take the absolute real loss on each phase before summing;
mutual coupling can make that reported value differ from dissipation. Both
physical losses and that native reported quantity are checked separately. The
delta-grounded-wye bank reports the signed total. No phase-power differences are
discarded to make aggregate losses agree.

PNNL records groups of properties. The pinned Arras group recorder truncates
rows because its write uses the pointer size as the buffer bound. Arras instead
uses bounded multi-recorder property lists under 900 characters, in a temporary
output directory, with the validation container's file-descriptor limit set to
65,536. Complete identity and column inventories are checked before comparison.

Plain ideal closed contacts emit exact cancelling resistances, avoiding the
OpenDSS `switch=y` dummy impedance. Plain open contacts are disabled and do not
introduce line charging or numerical leakage. Broader source-specific switch
profiles remain separate compatibility work.
This qualifies fixed-state feeder import and OpenDSS emission; canonical GLM
service writing and edited IEEE8500 feeder qualification remain unfinished.

`ideal_transformer_oracle.py --powerio /path/to/powerio --output /path/to/new-directory`
checks grounded and floating secondary returns, nominal and unequal winding taps,
through public DSS import, IR, emission and reimport. Analytic resistive circuits
check return displacement, voltage ratios, currents, powers and total losses.
Deliberately incomplete cancellation must fail. The floating return uses two
negative half-ohm conductors to cancel the one-ohm differential leakage without
adding a ground connection. Core and neutral shunts remain outside this profile.


### IEEE13/34 FBS native-equation audit

`fbs_feeder_import_oracle.py` now delegates to the common original-feeder
harness, preserving its command-line entry point. The obsolete audit assumed
that imported lines were all physical pi circuits. Native FBS instead sets
`d=a` in its hybrid transfer equations, and import now preserves that law using
existing admittance records. NR still solves a physical circuit and uses the
noncommuting equation only for reporting.

```sh
python evals/gridlabd/fbs_feeder_import_oracle.py \
  --case ieee34 --source /path/to/autotest/test_IEEE_34_FBS.glm \
  --bridge /path/to/glm_network --pnnl-root /path/to/GridLAB-D \
  --output /path/to/new-fbs-audit-directory
```

IEEE13 FBS passes the raw bus and physical external-port comparisons on both
forks. IEEE34 currently retains two failed line-loss comparisons, about
0.00011 VA each, with unchanged common tolerances. It is not declared a complete
raw match. Both retain deliberately incorrect output checks. Canonical writing
of the auxiliary-node native FBS line equivalent remains unfinished.


### PNNL constant-PQ generators

`diesel_import_oracle.py` checks seven circuits with both NR and FBS: unequal
signed phase powers, a parent-meter alias, native per-phase clipping, scalar
initialization overrides, the native default rating, and low/high voltages.
Each circuit compares nine complex voltage/current/power observations against
an independent diagonal-line solution, then against actual PowerIO import,
IR restoration and OpenDSS emission. A deliberately wrong phase power must
fail the comparison. Arras's different diesel class is not substituted.

```sh
python evals/gridlabd/diesel_import_oracle.py \
  --bridge /path/to/glm_network --pnnl-root /path/to/GridLAB-D \
  --output /path/to/new-diesel-validation-directory
```

The high/low cases exercise voltages outside OpenDSS's default generator band.
Format-neutral generators emit a diagnosed 0..2 pu band; parsed OpenDSS
generators retain their own bounds. This does not promise constant power above
2 pu or qualify the complete IEEE9500 feeder. Canonical GLM generator writing
and OpenDSS generator rating-limit mapping remain unfinished.

### Static reactors and standalone substations

`reactor_source_oracle.py` checks eight diagonal circuits under NR and FBS on
both pinned forks. It covers unequal reactor impedances, mixed/all-zero phases,
single-phase and phase-pair links, sequence voltages, reference-phase rotations,
and the IEEE9500 source/reactor parameter values (66,395.28 V nominal,
69,715.045 V positive-sequence input, and 14.7983 ohm per-phase reactance). Each phase compares complex
source/sink voltages, both-end currents and powers against an independent
analytic circuit and actual import, IR and OpenDSS output. Deliberately changed
load power must fail. The IEEE9500 parameter probe is not a full-feeder result.

```sh
python evals/gridlabd/reactor_source_oracle.py \
  --bridge /path/to/glm_network --pnnl-root /path/to/GridLAB-D \
  --output /path/to/new-reactor-import-results
python evals/gridlabd/reactor_canonical_oracle.py \
  --powerio /path/to/powerio --bridge /path/to/glm_network \
  --pnnl-root /path/to/GridLAB-D \
  --import-results /path/to/new-reactor-import-results \
  --output /path/to/new-reactor-canonical-results
```

The second harness takes the qualified IR for the reference-B and IEEE9500
parameter circuits, emits fresh GLM, solves it natively, reimports it and solves
its OpenDSS output. A second pass edits source angles and reactor impedances in
the public IR, then requires agreement with changed analytic results and
disagreement with the original circuit. Both forks are checked independently.

Reactor current ratings remain source-only with diagnostics. Open reactor states,
parented substations and external transmission coupling need further mapping.

### Static solar-fed inverters

`solar_import_oracle.py` checks eight circuits on each pinned fork. The cases
cover signed power factors, pure reactive output, clipping, offline state,
static weather, all three split-phase anchors, and the IEEE9500 PV parameters
on a service transformer. The harness independently evaluates the solar
thermal equation and inverter dispatch, compares native complex electrical
results with actual PowerIO import, IR and OpenDSS emission, and requires a
wrong-power negative control to fail.

```sh
python evals/gridlabd/solar_import_oracle.py \
  --bridge /path/to/glm_network --pnnl-root /path/to/GridLAB-D \
  --output /path/to/new-solar-import-results
```

The high-output service case checks the 240 V hot-to-hot base: a 120 V base
would incorrectly trigger OpenDSS's generator voltage-law transition. The
IEEE9500 solar block produces 13,989.345858367315 W under its static climate,
which differs from the declared 13,990 W inverter schedule. Comparisons use
native inverter `VA_Out`; Arras does not update solar `P_Out` in this path.
The profile covers `BASEEFFICIENT` solar with default orientation and simple
inverter efficiency in `FOUR_QUADRANT CONSTANT_PF` mode. Other control modes, weather files and canonical GLM inverter output remain unfinished. The idle battery profile and full IEEE9500 audit are described below.


### IEEE9500 prerequisites and complete feeder audit

`battery_import_oracle.py` qualifies the internal lithium-ion battery's idle
`CONSTANT_PF` state: eight native/import comparisons across both solvers and
forks, including different SOC and scheduled powers, with wrong-power negatives.
`triplex_zip_import_oracle.py` checks sixteen service circuits covering the
IEEE9500 load parameters, mixed ZIP terms, signed power factors, native fraction
normalization, explicit-term overrides and zero base power.
`direct_triplex_import_oracle.py` checks eight direct-matrix services, including
the IEEE9500 matrix and a reciprocal physical variant. It compares physical hot
ports and signed losses through IR and DSS; undefined neutral currents stay
outside that comparison. Native absolute-real loss telemetry is recorded
separately. Every wrong-load negative must fail.

`fixed_state_import_oracle.py` checks inherited and explicit capacitor rating
voltages and single-phase manual banks on both forks, plus the connected PNNL NR
`SWING_PQ`/future-outage case. Five native comparisons and wrong-impedance
negatives qualify these device states. It does not execute active controls.

Each device harness takes `--bridge`, `--pnnl-root`, and a new `--output` directory.
For the complete externally cached CIMHub projects:

```sh
python evals/gridlabd/ieee9500_import_oracle.py \
  --source /path/to/CIMHub/ieee9500 \
  --bridge /path/to/glm_network --pnnl-root /path/to/GridLAB-D \
  --output /path/to/new-ieee9500-results
```

The harness pins each original base, wrapper and edits file by SHA-256, parses
both complete projects, verifies IR equality and runs their DSS outputs. It
records every native bus and both ends of every native link at full precision.
Native instrumentation extends the zero-duration clock by one second and uses
`default_maximum_voltage_error=1e-8`; electrical declarations remain unchanged.
Direct triplex neutral currents are unspecified and are explicitly excluded.
Source bytes remain external; this change vendors no public feeder files.

**The strict full-feeder comparison currently fails and the harness exits 1.**
Initial whole-feeder voltages agree within tens of microvolts; reported branch
currents/powers have additional discrepancies. The largest observed current
difference is explained by pinned native `line.cpp` using `d=a=I+ZY/2`, while
physical pi ports require `d=I+YZ/2`. The independently evaluated difference
`(ZY-YZ) Iout / 2` predicts the observed reporting offset. The audit records that
explanation separately and never overwrites measurements, loosens tolerances,
or converts a failed comparison into a pass. Near-zero impedances and native
convergence floors still need qualification. Deliberate wrong-tap output must
fail. No full matching claim, edited-feeder qualification, or Arras IEEE9500
portability is implied; the original Arras input also has unsupported properties
and different diesel semantics.

### FBS delta-delta imports and remaining IEEE variants

`delta_delta_import_oracle.py` validates the native FBS `DELTA_DELTA` equations
against an independent analytic solve and the actual PowerIO → IR → DSS path.
Its twelve cases cover both pinned forks, step-up/down/equal ratios, wye/delta
loads, and unbalanced source phasors. Deliberately damaged output must fail.
The equivalent uses existing admittance and ideal-contact records, including
three scaled-current auxiliary variables. It does not claim physical delta
zero-sequence isolation or canonical GLM output support.

```sh
python evals/gridlabd/delta_delta_import_oracle.py \
  --bridge /path/to/glm_network --pnnl-root /path/to/GridLAB-D \
  --output /path/to/new-delta-results
python evals/gridlabd/ieee_fbs_delta_import_oracle.py \
  --case ieee37 --source /path/to/autotest/test_IEEE_37_FBS.glm \
  --bridge /path/to/glm_network --pnnl-root /path/to/GridLAB-D \
  --output /path/to/new-ieee37-results
```

The feeder harness also accepts `--case ieee123` with either
`test_IEEE-123_FBS.glm` or `test_IEEE123_zero_voltages_FBS.glm`. It inventories
objects with numeric IDs and nested assertions, resolves source aliases, and
records every node and both ends of every link. Unnamed objects receive stable
recorder names. It preserves electrical declarations, fixed states and native
assertions, tightens the FBS stopping fraction to `1e-12`, and supplies one
recorder timestep. Arras's `voltdump` formatting keyword is spelled `rect`;
this telemetry-only spelling is adjusted explicitly. Both forks use pinned
engines, and no feeder bytes are vendored.

IEEE37 FBS passes all 579 raw complex measurements on both forks, with a
wrong-line-length negative. The two IEEE123 variants parse, preserve IR and
solve in DSS, but their complete raw comparisons remain **failed**:

- The standard case has 2,157 measurements. Four isolated native nodes retain
  initialized voltages and have no DSS bus; these remain missing-measurement
  failures. A regulator reports an undeclared phase voltage. An open switch
  reports downstream load current/power despite zero input; DSS reports zero
  at both open ports. These sixteen raw failures remain visible.
- The zero-voltage variant has 2,079 measurements. With the native FBS line
  equivalent, only its undeclared-phase voltage remains a raw failure on each
  fork. All line/port comparisons pass. The inconsistent voltage observable
  remains in the comparison, so the full case is not declared a pass.

Together with earlier support, thirteen of fourteen cached original GLM entry
points now parse with IR equality. This is an entry-point count, not fourteen
feeder families or thirteen electrical qualifications. The remaining
`test_IEEE123_run.glm` is a switching-schedule simulation; it still needs explicit
static-snapshot handling rather than silently executing or stripping controls.

### Analytic FBS line equivalents

`fbs_line_import_oracle.py` exercises eight combinations of two/three phases,
line length, and pinned engine fork. It compares an independent analytic hybrid
solve with native outputs and the actual import → IR → DSS path. Wrong contact
impedance must fail. The native equations are `Vfrom=a Vto+Z Iout` and
`Iin=c Vto+a Iout`, with `a=I+ZY/2`, `c=Y+YZY/4`, and Y the total shunt.
No matrix symmetrization, epsilon impedance, or tolerance widening is used.
Configuration coefficients/coordinates remain on a linecode; line coordinates
follow the equivalent's shunt and contacts. FBS import uses these equations only
when Z/Y do not commute; NR retains the physical circuit.

```sh
python evals/gridlabd/fbs_line_import_oracle.py \
  --bridge /path/to/glm_network --pnnl-root /path/to/GridLAB-D \
  --output /path/to/new-fbs-line-results
```

The OpenDSS source writer also distinguishes its native symmetric (`Z1=Z2`)
and asymmetric sequence-source formulas. Only the latter needs compensation
for the pinned engine's truncated rotation constants. Using that compensation
for symmetric blocks introduced a measurable projection error; the line
microcases exercise both branches and catch it.

### IEEE European low-voltage snapshot

`european_lv_import_oracle.py` validates the original European LV `Master.dss`
from the pinned `dss-extensions/electricdss-tst` revision in `ieee-corpus.json`.
The catalog pins all 109 companion files, including the load profiles, by byte
count and Git blob hash. Keep this checkout outside the PowerIO repository;
no public dataset bytes are vendored. The original master selects snapshot
mode and sets `useactual=no` on the shapes before creating its loads.

```sh
cargo build -p powerio --example dist_network
python evals/gridlabd/european_lv_import_oracle.py \
  --source /path/to/electricdss-tst/Version8/Distrib/IEEETestCases/LVTestCase \
  --bridge target/debug/examples/dist_network \
  --output /path/to/new-european-lv-results
```

The bridge imports the original, serializes/deserializes its IR with full typed
network equality, and emits canonical output from a fresh module. This prevents
retained-source replay from being mistaken for writer validation. The oracle
compares all 907 original buses, all 906 lines, the transformer, all 55 loads,
and the source's physical feeder port: 14,784 complex voltage/current/power/loss
quantities. It repeats the comparison after a second import and canonical
write, and requires an intentionally changed line length to fail. Both native
and canonical snapshots use the pinned DSS engine, controls off and solver
tolerance `1e-12`; acceptance tolerances remain those in `oracle.py`.

Canonical static DSS output omits `daily`, `yearly`, and `duty` loadshape
references on loads, generators and voltage sources, with an explicit loss
diagnostic, because their definitions are not regenerated. The original
references remain retained on import; same-format source replay is unaffected.
This result qualifies the feeder's static DSS/IR/DSS path. It does not qualify
its time series or canonical GridLAB-D emission, which still reports unsupported
additional tables and transformer details.

### Fixed OpenDSS Line terminal states

`dss_terminal_state_oracle.py` checks 38 synthetic native cases through import,
IR and first/repeated canonical DSS output. It covers one through four
conductors, partial and whole-terminal opens, both ends, reclosure, equal and
changed conductor counts, `LineCode` assignment, `MakeLike`, explicit-neutral
opens, and zero charging. Every opened typed matrix is compared directly with
native `YPrim`; bus voltages and physical line-port currents and powers are
compared after both canonical cycles. A separately initialized native circuit
with the wrong fixed state must fail the electrical comparison.

```sh
cargo build -p powerio --example dist_network
python evals/gridlabd/dss_terminal_state_oracle.py \
  --bridge target/debug/examples/dist_network \
  --output /path/to/new-terminal-state-results
```

Ordinary Line `Open`/`Close` commands bind at execution time and replay in
property/copy/solve order. Native conductor-count changes reset the contacts;
a same-size `MakeLike` keeps the destination's contacts. Fixed open ports use
the native zero-current Schur complement, including the pinned engine's
`1e-12 S` diagonal stabilization, represented by an existing typed shunt and
ideal external contacts. Effective linecodes remain available, contact ratings
are retained, and unsupported line metadata is diagnosed as source-only.
No new IR type or solver dependency is introduced.

The DSS writer preserves reciprocal coupled/unequal shunts with scalar
impedance branches and diagonal corrections; nonreciprocal matrices use the
existing coupled-source equivalent. This avoids collapsing a symmetric matrix
to one scalar and avoids unnecessary cancellation in reciprocal circuits.
Already-exact scalar delta shunts keep their compact native expression.
Geometry-derived lines still require calculated impedances before conversion.
Commands on other device classes, omitted/invalid terminal indices, and
out-of-range conductor indices remain explicit refusals. Active switching and
protection execution remain outside this static profile.
