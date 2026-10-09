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


Matrix parsing follows the pinned native `ParseAsSymMatrix` rules: start with
zeros and mirror supplied entries in row order. Later entries overwrite both
symmetric positions; missing cells retain earlier writes or zero. Extra numeric
rows beyond the conductor count are unread, and an empty property token keeps
the previous matrix. An oversized consumed row remains an error. The line and
linecode oracles include short, asymmetric, empty and partially specified rows.
`IMYD.dss` imports its four-wire capacitance matrix. Full-feeder comparisons are
reported separately; matching its matrix alone does not qualify the feeder.


Source resolution preserves native arithmetic order when positive and negative
sequence impedances agree. Diagonal impedance uses `(2 Z1 + Z0) / 3`;
sequence-specified mutual reactance uses `Xs - X1`, while short-circuit inputs
use `(X0 - X1) / 3`. The source-state oracle includes the modified secondary
source, and a bit-level regression pins the distinction.


## Compact native Thevenin emission

An isolated imported three-phase source can emit as native `Vsource.source`
when its current typed EMF phasors and series impedance have a representable
native sequence form. This avoids additional ideal-source cancellation circuits.
The writer derives the voltage, angle, sequence and impedance from typed values;
retained parameters never override edits. Matrix reconstruction must be exact
under the pinned engine's arithmetic. Phasor reconstruction allows only its
floating-point roundoff, not material unbalance.

The source must have a grounded return and no other attachment at its internal
EMF bus. Bounds, location, route, ratings, shunts, asymmetric impedance or
unrepresentable voltage changes retain the general equivalent. The source
branch provenance identifies the compiler-created component but supplies no
physics. This changes no network, IR or ABI type.

Short-circuit MVA inputs have a distinct mutual-reactance arithmetic path.
When the explicit sequence form cannot reconstruct the matrix exactly, the
writer derives MVA and X/R parameters from the current typed impedance. A
bounded search of nearby floating-point values must reproduce both native
sequence impedances exactly under shared reader/writer equations. It never
replays retained MVA parameters, and unsuccessful inversions keep the general
source equivalent.

```sh
cargo build -p powerio-cli
cargo build -p powerio --example dist_network
python evals/gridlabd/dss_thevenin_oracle.py \
  --bridge target/debug/examples/dist_network --powerio target/debug/powerio \
  --output /tmp/new-thevenin-results
```

Twenty-five native checks cover 50/60 Hz, positive/negative/zero sequences, phase
permutation, the secondary feeder source, short-circuit specification, unequal
sequence and explicit-return fallbacks. Public IR conversions verify edits to
voltage, resistance and branch length against separately changed native inputs.
The MVA path also verifies a doubled typed source voltage with its impedance
unchanged against an independently adjusted native short-circuit definition.
All original voltages, equipment quantities and source conductors pass both
canonical cycles. Changed-line, moved-source-port and unedited-model controls
must fail. The observer follows explicit compiler source terminals when a
fallback reimport splits the original port; it never substitutes bus KCL sums.

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
99 static cases for OpenDSS load models 1, 2, 3, 4, 5 and 8 across the voltage
band and dropout transition, including fractional and negative CVR exponents.
It parses through public IR, emits canonical DSS, repeats the import and output,
and compares native complex load voltages, currents and powers. Run the ignored
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

### OpenDSS overhead geometry calculation checkpoint

`dss_overhead_geometry_oracle.py` captures native series impedance and total
shunt susceptance for 150 synthetic cases: Carson, FullCarson and Deri; 40, 50,
60, 1000 and 3000 Hz; one and three phase arrangements, an explicit fourth
conductor, and four/five-conductor neutral reduction; two resistance/earth
resistivity profiles including strong skin effect. These are explicit SI
inputs to a private evaluator, not feeder-import qualification.

```sh
python evals/gridlabd/dss_overhead_geometry_oracle.py \
  --output /path/to/new-dss-overhead-matrices
POWERIO_GEOMETRY_ORACLE=/path/to/new-dss-overhead-matrices/matrices.json \
  cargo test -p powerio-dist --lib \
  dss::geometry::tests::pinned_overhead_matrices -- --ignored --nocapture
```

Both steps are required: native capture alone does not compare PowerIO. The
Rust comparison checks every matrix entry, with absolute tolerances of
`1e-12 ohm/m` and `1e-18 S/m`, plus `1e-10` relative tolerance. Generated decks,
SHA-256 hashes and native observations remain outside the repository.

The calculation follows pinned DSS C-API
[`LineConstants.pas`](https://github.com/dss-extensions/dss_capi/blob/87d85c2622c8281b92255335bc7c09b11191b21d/src/General/LineConstants.pas),
including its rounded physical constants, frequency-boundary behavior and
Deri Bessel-series calculation. Neutral reduction eliminates the neutral
impedance block but retains the phase block of the full inverted potential
matrix for capacitance. The native C API reports capacitance in nF using a
different, full-precision `TwoPi`; the observation script converts that
quantity back to susceptance with the corresponding constant.

The public reader now uses this calculation together with ordered definitions
and native shared-cache history, as described below. The kernel comparison
alone does not qualify a feeder; whole-feeder qualification uses the public
reader, IR round trips and canonical output. Spacing and cable models remain
unresolved in the OpenDSS reader.

### Ordered OpenDSS overhead definitions

`dss_wiredata_oracle.py` checks 49 native conductor-definition cases, including
positional/abbreviated properties, RPN values, default backfilling, unit changes,
repeated radius assignments and historical/self `MakeLike`. A radius assignment
initializes capacitance radius only while the latter is negative; later radius
edits leave it alone. A GMR-only definition can leave capacitance radius negative,
in which case the geometry calculation uses the physical radius.

`dss_geometry_definition_oracle.py` checks 27 synthetic geometry-definition
cases. Conductor selectors, sticky units, equal-size and changed-size resets,
wire arrays and copied definitions follow native order. Wire assignment binds
an existing identity and copies its current scalar ratings, while calculation
reads the wire values visible at that later time. A later declaration does not
repair a missing reference. Both resolvers reuse bounded append-only property
and copy histories; no new public model or IR type is introduced. Seasonal
ratings remain source metadata. Assigning `LineSpacing` to `LineGeometry` copies
its coordinates and units at that point; later spacing edits do not change the
geometry until reassignment. The geometry keeps its own phase count and conductor
selector. Spacing copies, self-copies, partial coordinate arrays and size changes
follow the pinned engine. Missing references, mismatched conductor counts and
uninitialized coordinates remain errors. Direct `Line.spacing` and cable
conductor definitions remain unresolved.

```sh
python evals/gridlabd/dss_wiredata_oracle.py --output /tmp/new-wire-observations
POWERIO_WIRE_ORACLE=/tmp/new-wire-observations/wires.json \
  cargo test -p powerio-dist --lib \
  dss::geometry::wire::tests::pinned_wire_definitions -- --ignored
python evals/gridlabd/dss_geometry_definition_oracle.py \
  --output /tmp/new-geometry-definitions
POWERIO_GEOMETRY_DEFINITION_ORACLE=/tmp/new-geometry-definitions/definitions.json \
  cargo test -p powerio-dist --lib \
  dss::geometry::definition::tests::pinned_geometry_definitions -- --ignored
```

For public data, the same oracle checks the 32 geometry objects in the 17
CIMHub IEEE4 cases that declare `LineGeometry`. It verifies source byte counts
and Git blob hashes against `ieee-corpus.json` before execution. Keep the pinned
CIMHub checkout and generated observations outside the PowerIO repository.

```sh
python evals/gridlabd/dss_geometry_definition_oracle.py \
  --ieee4-base /path/to/CIMHub/ieee4/base --output /tmp/new-ieee4-geometries
POWERIO_GEOMETRY_DEFINITION_ORACLE=/tmp/new-ieee4-geometries/definitions.json \
  cargo test -p powerio-dist --lib \
  dss::geometry::definition::tests::pinned_geometry_definitions -- --ignored
```

This compares definition fields and the Carson impedance/admittance matrices
at 60 Hz, including unbalanced, single-phase and center-tapped case inputs.
Definition checks alone do **not** qualify transformer models or full power
flows. The following checks exercise shared-cache history and public import.

### Public OpenDSS overhead import and IEEE4 qualification

The reader lowers supported overhead `WireData`/`LineGeometry` lines to the
existing effective line matrices. It replays source-order assignments and
calculation commands because native OpenDSS shares geometry calculations while
each line caches its own primitive. Default and active earth models differ;
geometry edits, `MakeLike`, length/units changes, frequency changes and `Clear`
do not all invalidate the same caches. Unsupported geometry remains an explicit
electrical readiness blocker. This introduces no public model or IR type.

`dss_geometry_cache_oracle.py` captures twelve synthetic histories. The Rust
comparison checks native line primitives after each recorded state, with
`1e-10 S` absolute plus `1e-10` relative tolerance. Histories include shared
geometries, earth-model inheritance, definition edits, `MakeLike`, and replacing
a geometry binding with a `LineCode` or sequence parameters.

```sh
python evals/gridlabd/dss_geometry_cache_oracle.py \
  --output /tmp/new-geometry-cache
POWERIO_GEOMETRY_CACHE_ORACLE=/tmp/new-geometry-cache/observations.json \
  cargo test -p powerio-dist --lib \
  dss::geometry::timeline::tests::pinned_calculation_history -- --ignored
cargo build -p powerio --example dist_network
python evals/gridlabd/ieee4_import_oracle.py \
  --ieee4-base /path/to/CIMHub/ieee4/base \
  --bridge target/debug/examples/dist_network --output /tmp/new-ieee4-import
```

The IEEE4 harness checks all twenty-seven pinned CIMHub cases, including
transformer-code, autotransformer, four-wire and geometry variants. It also
verifies and stages the pinned `ieee4xy.dat` coordinate companion. Each qualification requires typed import, IR equality, native
versus canonical electrical comparison, repeated import/emission, and rejection
of a deliberately changed line. It compares every original bus voltage and
Line/Transformer/AutoTrans/Load/Capacitor/Reactor terminal current, power and loss, plus the physical
source feeder port. Source bytes and generated evidence stay external.

Twenty cases qualify: `DDBal`, `DDUnBal`, `DYBal`, `DYUnBal`, `GYDBal`,
`GYDUnBal`, `IMODOD`, `IMOYODlag`, `IMOYODlead`, `IMYD`, `OYODBal`,
`OYODUnBal`, `OnePh`, `SCTBal`, `SCTUnBal`, `YDBal`, `YDUnBal`, `YYBal`,
and `YYUnBal`, plus `YYDXn`. Together they provide 1,545 complex observables per
conversion pass. These are IEEE4 variants, not twenty distinct feeder families.
Seven remain unqualified:

- `YYD` retains delta-side voltage/power mismatches (maximum about
  `1.68e-5 V` and `0.00967 VA`).
- `YYD1Tank` currently fails canonical convergence at the pinned solve criterion.
  Earlier converged checkpoints also failed strict electrical comparisons.
- `Auto1bus`, `Auto3bus`, `AutoHLT`, and `AutoHLTCode` retain strict electrical
  mismatches, particularly at the very short connecting lines.
- `AutoAuto` retains all original observations and its typed autotransformer
  quantities match; the complete feeder still fails source/switch comparisons.

The four newly passing `IM*` cases use a native Thevenin source reconstructed
from current typed voltages and impedance. The same observer run against the
previous writer still qualifies 15/27, so the gain is not an observation change.
Every original bus node and source conductor remains observed. Source keys
include the original bus and node identities, and ambiguous interfaces fail.
Compiler-created auxiliary nodes are not original feeder observables.

`OYODUnBal` passes after the native source arithmetic correction described
above. Its 77 observations pass canonical output and reimport, with a failing
changed-line control; three additional fresh runs reproduce that result.
Direct line normalization and exact MVA source emission additionally qualify
`YYDXn` across all 111 observations. Fresh repeats confirm `GYDBal`, `YDBal`
and `YYDXn`; the current `GYDBal` first pass matches every observation exactly.
No threshold was relaxed or observation removed.

The broader external script audit recognizes both `New Circuit` and
`New object=Circuit` spellings: 56 of 61 entry points parse and retain typed
network equality through IR. The expanded inventory includes IEEE34, IEEE37
and IEEE123 master cases previously missed by that scan. It also includes
auxiliary and fault-study scripts; these counts are not qualified-feeder totals.

The reproducible inventory verifies all 351 acquired files against repository
commits in the catalog and their byte, SHA-256 and Git-blob hashes. Fifty-four
entries can emit canonical DSS; the variable-slip induction-machine case
remains blocked. Five parse refusals involve fault-study/direct-mode state.
The DG protection case now reads its fixed open transformer through IR, but
its enabled Fault remains unresolved and blocks electrical emission. None of
these counts proves a native electrical match. Canonical GLM remains unavailable
for this DSS inventory.

```sh
python evals/gridlabd/dss_corpus_audit.py --source-root /path/to/acquisition-with-manifest --bridge /path/to/dist_network --output /tmp/new-dss-audit
```

`dss_order_sensitivity.py` compares the native engine with itself after moving
unchanged transformer declarations or reversing unchanged line declarations.
It verifies exact element, terminal, bus/node and primitive-admittance
inventories before reporting numerical sensitivity. No electrical parameter
or comparison tolerance is changed.

```sh
python evals/gridlabd/dss_order_sensitivity.py --run-directory /path/to/ieee_distribution_import_oracle-output --output /tmp/new-order-diagnostic
```

All eight permutations of IEEE34mod2, IEEE13, IEEE123 and IEEE123Switches have
identical native primitives but exceed at least one current strict threshold.
Maximum native-to-native differences across the two permutations are:

| Case | Voltage (V) | Current (A) | Power/loss (VA) |
| --- | ---: | ---: | ---: |
| IEEE34mod2 | 2.06e-8 | 7.45e-9 | 1.84e-4 |
| IEEE13 | 7.07e-6 | 1.10e-5 | 1.70e-2 |
| IEEE123 | 7.30e-7 | 1.43e-6 | 4.10e-3 |
| IEEE123Switches | 7.31e-7 | 9.57e-7 | 4.48e-3 |

This establishes an ordering-dependent numerical effect; it does not prove
that every import residual is harmless. These feeders remain unqualified under
the unchanged strict comparison. The diagnostic does not choose a favorable
ordering, remove ports or replace native losses with inferred values.

The native and generated solves all use controls off, at most 1,000 iterations,
and tolerance `1e-11`, stricter than the original files' `1e-8`. The balanced
delta-delta native case stalls at `1e-12`; nonconverged runs never qualify.
Observable acceptance thresholds in `oracle.compare` remain unchanged.

### OpenDSS transformer-code assignment

`XfmrCode` properties use their native ordered table, including positional and
abbreviated input. A transformer binds the code state available at assignment;
later code edits do not change it. The reader uses bounded existing property
and copy histories, materializing winding connections, voltage/rating/tap,
resistance and neutral impedance, reactances, excitation and antifloat values.
Missing references and unresolved code properties produce an error instead of
a transformer with factory defaults. Code definitions remain retained metadata.
Seasonal ratings, tap bounds and DC resistance are not newly projected into
typed fields by this AC snapshot work.

Native `MakeLike` omits excitation and antifloat fields; self-copy resets winding
records. Scalar kVA setters propagate between windings, while arrays assign
individual winding values. End-of-edit scalar reactances can override an array.
Fifteen native state probes cover these distinctions and historical bindings.
The public IEEE4 checks above additionally qualify the balanced and unbalanced
center-tapped feeders, including excitation and unequal winding resistance.
Canonical output keeps typed winding resistances authoritative: replaying a
stored total load loss after them would replace unequal values with an equal
split. A mutation round-trip regression exercises this behavior.

```sh
python evals/gridlabd/dss_xfmrcode_oracle.py --output /tmp/new-xfmrcode-states
POWERIO_XFMRCODE_ORACLE=/tmp/new-xfmrcode-states/observations.json \
  cargo test -p powerio-dist --test dss_xfmrcode -- --include-ignored
```


## Public IEEE and low-voltage fixed-state profiles

`ieee_distribution_import_oracle.py` compares sixteen public feeder entries with
canonical output and repeated conversion. Sources and dependencies are pinned
in `ieee-corpus.json`; public bytes remain in an external checkout. Run:

```sh
python evals/gridlabd/ieee_distribution_import_oracle.py \
  --source-root /path/to/electricdss-tst \
  --bridge /path/to/dist_network --output /new/path
```

The profile inserts `Set ControlMode=Off` before every solve or voltage-base
calculation in the selected entry files. The report retains each insertion,
the original bytes and hashes, and the adjusted hashes. It compares the written
fixed device states; it does not reproduce actively regulated published results.
The common IEEE4 comparator checks complex bus voltages and device terminal
currents, powers and losses, source ports, IR equality, repeated conversion and
a changed-line negative control. Missing ports and nonconvergence fail. Each
case reports its result independently; a parser success is not qualification.


The suite includes both IEEE 8500 variants, the neutral-earth-voltage case,
three four-wire delta induction-machine cases, and the modified secondary test
circuit. Use `--case NAME` to select an entry; repeat the option for several.
The secondary circuit's 74 binary load profiles are pinned by size and Git blob
hash and remain external. Its explicit `sngfile`/`dblfile` path separators are
normalized for the POSIX engine, with every change recorded in the report.

Both IEEE 8500 variants now pass canonical output, IR and repeated conversion,
with 60,595 balanced and 64,126 unbalanced observations in each pass. Independent
fresh runs reproduce both qualifications, including changed-line negative
controls. The former line-loss mismatches disappear after direct normalization
of whole-line series impedance. The whole-case requirements now qualify seven
of sixteen entries: IEEE34mod1, IEEE37, both IEEE8500 variants, NEV and the
four-wire lagging/leading fixed-slip motor feeders.

NEV now reaches electrical comparison: canonical load records omit harmonic
spectrum references with a diagnostic. The pinned engine uses those spectra
only in non-fundamental harmonic mode; changing every native NEV load to the
default spectrum leaves all 3,044 native observations unchanged in the snapshot.
NEV now passes all 3,031 compared observations through canonical emission and
reimport. Its four wye capacitor banks retain their common neutral conductor
(`Bus2=.10.10.10`) in the existing shunt admittance matrix. Canonical emission
recognizes a uniform star, derives its kvar from the typed matrix and preserves
the original capacitor identity and both terminals. Edits to the matrix take
precedence over retained kvar. Other matrix shapes use the general admittance
writer. There is no new network or IR type.

`dss_capacitor_oracle.py` isolates this connection in twelve native cases:
one, two and three phases, phase permutations, and independent 50/60 Hz operating
and base frequencies. It checks typed and emitted primitives, all 660 electrical
observations, IR/reimport, and changed-line and grounded-return negative controls:

```sh
python evals/gridlabd/dss_capacitor_oracle.py \
  --bridge /path/to/dist_network --output /new/path
```

The newly qualified common-return profile is a single energized kvar/kV bank.
Filtered/stepped banks, capacitance specification, copy or repeated connection
history, separate return buses and terminal commands remain explicit gaps.
Untyped active capacitors block electrical readiness. Explicitly disabled
capacitors remain source-only unless a command could change that state.
The native equations and connection convention are pinned to DSS C-API
`87d85c2622c8281b92255335bc7c09b11191b21d`, `src/PDElements/Capacitor.pas`
(Git blob `4e5d4557fcb47394a03c3906ce50b55c93059c08`).

The modified secondary circuit now resolves its 74 binary load profiles and
static `UseActual` assignments. Scalar transformer kVA assignments also follow
the native winding synchronization rules. It still fails 20 of 953 observations
through canonical output and reimport. All bus voltages pass, with a maximum
voltage difference of `5.29e-9 V`. The remaining current and power differences
occur only on the two near-zero-impedance substation links; the maximum power
difference is about 927 VA. The feeder remains unqualified.

The reader normalizes resolved whole-line series impedance with `Z / length`,
avoiding the separately rounded reciprocal in `Z * (1 / length)`. On the short
substation link, this preserves native total impedance and its primitive
admittance exactly. Native source MVA reconstruction also avoids unnecessary
source-equivalent cancellation. These changes improve floating-point fidelity;
they do not round away impedance, replace equipment with ideal connections,
change acceptance thresholds, or omit difficult terminal observations.

The three four-wire induction-machine baselines now run with the actual upstream
`IndMach012a` library. `build_indmach_oracle.py` fetches the BSD-3 source at
`tshort/OpenDSS` commit `5005c668a72d20775f4c2d060feebb2866ba1d38`, verifies every
pinned Git blob, and builds it with Free Pascal 3.2.2 on macOS arm64. The only
source changes are local include paths, removal of the unused Windows resource,
and Darwin C export names. Original and patched hashes, compiler output and the
library hash remain in the external build directory. No motor equations change.
The generator/dynamics records use the same packed ABI as the pinned C-API engine.

```sh
python evals/gridlabd/build_indmach_oracle.py --output /tmp/new-indmach-oracle
DYLD_LIBRARY_PATH=/tmp/new-indmach-oracle/build \
python evals/gridlabd/ieee_distribution_import_oracle.py \
  --source-root /path/to/pinned/electricdss-tst \
  --bridge /path/to/dist_network \
  --indmach-manifest /tmp/new-indmach-oracle/manifest.json \
  --output /tmp/new-ieee-with-motors
```

The native motor, lagging and leading cases converge at the unchanged strict
settings and expose 109, 96 and 96 electrical observations. The runner saves
these original reference snapshots even when typed conversion is blocked.
The fixed-slip lagging/leading feeders now qualify all 96 observations each
through typed import, IR, canonical output and reimport. Their full phase
admittance is nonreciprocal and uses the existing `DistShunt` matrix; canonical
DSS writes the existing exact coupled-impedance decomposition. The comparator
sums only the explicitly declared equivalent's external terminal currents and
powers, validates every component's bus/node inventory, and follows the typed
source-impedance branch on reimport. It does not use unrelated bus flows or
internal negative-element losses as the motor's electrical observations.

The reader reproduces the pinned library's actual sequence transform: its
rotation constant is `0.866025403` and it explicitly inverts that matrix.
Replacing the inverse with conjugate/3 causes observable zero-slip errors.
Userdata before library creation is ignored; library reloads reset state;
successive userdata edits persist. Slip clamps when assigned, using the current
maximum slip. Changing the maximum afterward does not retroactively clamp.
The profile requires three-phase delta, explicit positive kV/kVA, and named
numeric userdata. Unqualified parameters, copied hidden state and variable slip
retain the explicit `READINESS.DSS.GENERATOR_USER_MODEL_UNRESOLVED` blocker.
Same-format byte echo remains available. Static variable-slip behavior remains
an unresolved compatibility requirement, not an excluded active control.

`dss_fixed_motor_oracle.py` checks nine parameter/state histories at 0.9, 1.0 and
1.1 pu. All 27 cases pass typed matrix current evaluation, canonical output,
reimport and changed-line negative controls (999 observations per conversion).
An ignored Rust test multiplies each actual typed matrix by 1.17 and emits from
a fresh module. All 27 edited circuits match independently increased native
motor kVA ratings and differ from their original circuits (another 999
observations). The maximum native current-law difference is below 2e-9 A.
No acceptance thresholds change.

```sh
DYLD_LIBRARY_PATH=/tmp/new-indmach-oracle/build \
python evals/gridlabd/dss_fixed_motor_oracle.py prepare \
  --bridge /path/to/dist_network \
  --indmach-manifest /tmp/new-indmach-oracle/manifest.json \
  --output /tmp/new-fixed-motor-sweep
POWERIO_FIXED_MOTOR_CASES=/tmp/new-fixed-motor-sweep \
  cargo test -p powerio-dist --test dss_user_model \
  export_edited_fixed_motor_matrices -- --ignored
DYLD_LIBRARY_PATH=/tmp/new-indmach-oracle/build \
python evals/gridlabd/dss_fixed_motor_oracle.py check-edits \
  --bridge /path/to/dist_network \
  --indmach-manifest /tmp/new-indmach-oracle/manifest.json \
  --output /tmp/new-fixed-motor-sweep
```

The earlier native baseline also exposed a writer ordering bug: `UserData` was
emitted before `UserModel`, leaving constructor defaults. The internal writer
now loads the library first. `export_generator_user_model_native_probe` remains
a diagnostic replay check for the unresolved variable-slip source, invoked
with `POWERIO_MOTOR_INPUT` and `POWERIO_MOTOR_OUTPUT`. Internal replay is not
public typed qualification.

### Static load-profile assignments

Assigning a `UseActual=yes` LoadShape to a Load's `daily`, `yearly`, or `duty`
property changes its nominal power even in snapshot mode. The reader applies
that change at the assignment, using the shape's state at that point in the
script. Later shape edits do not retroactively change an already assigned load.
The peak uses the first maximum absolute P sample, retaining its sign and Q at
that same index. An explicit Qmax overrides the sampled Q; absent Q data gives
zero. Explicit PF, kvar, ordered edits and MakeLike follow the pinned native
assignment rules. No time-series execution is performed.

Supported profile data includes inline arrays, CSV columns with an optional
header, direct CSV/PQCSV input, and little-endian single/double precision binary
files. Direct variable-interval binary files contain time/value pairs. The
reader preserves the native distinction between direct single-precision
storage and binary arrays promoted to double precision, including copied shapes.
Acquisition uses the existing source-root confinement and include budgets;
shapes are limited to 1,048,576 points. Unsupported transformations, memory
mapping, invalid data, or short Q files produce an error when an actual-power
assignment depends on them. They cannot silently leave the old load power.

`dss_useactual_oracle.py` compares nominal P/Q and native electrical results in
55 cases, including ordered references and copies, signed/tied peaks, PF
precedence, binary/CSV inputs, and scalar versus array transformer ratings.
All cases require canonical output, IR equality, reimport, and a changed-line
negative control.

LoadShape has a native ordered property table, so bounded `BatchEdit`,
abbreviations and positional assignments use the same stateful path as ordinary
edits. This restores the European LV master's `BatchEdit LoadShape..*
useactual=no` before load creation. Later shape edits affect future load
assignments; they do not retroactively change an already assigned nominal power.
Unsupported actions still produce explicit errors when an actual shape is used.
The complete European LV feeder again passes all 14,784 electrical observations
through canonical output and reimport.

```sh
python evals/gridlabd/dss_useactual_oracle.py \
  --bridge /path/to/dist_network --output /new/path
```

Same-format DSS project emission includes every acquired dependency, including
binary profiles, byte for byte under its original relative name. Root-level
projects also provide the established `case.dss` entrypoint; nested entrypoints
keep their relative location. Emission does not reopen source files. Absolute
dependencies, missing dependency context, or portable filename collisions are
refused because a byte-exact project cannot safely be relocated in those cases.
Canonical emission from the typed value remains available. Binary input is
never decoded as text or stripped of bytes that resemble a UTF-8 BOM.


### Public North American spacing definitions

`dss_spacing_oracle.py` verifies the pinned North American master before selecting
its conductor and geometry definitions. It puts the `Trans` overhead geometry,
`Pri` concentric-neutral cables and reduced four-conductor `Sec` bundle into
controlled, unbalanced circuits.
The report records the source line range and hashes; generated source files
show the complete circuit additions. These are geometry probes, not full-feeder
qualification.

```sh
python evals/gridlabd/dss_spacing_oracle.py \
  --north-american-master /path/to/LVTestCaseNorthAmerican/Master.dss \
  --bridge /path/to/dist_network --output /new/path
```

All three probes compare complex bus voltages, terminal currents, powers and losses,
source ports, IR equality and repeated canonical conversion. A changed line
length must fail the comparison. Public input bytes and generated results stay
outside the repository.


### Concentric-neutral cable geometry

The reader resolves `CNData` and `LineGeometry.CNCable`/`CNCables` through the
existing conductor histories. It preserves native unit interpretation, strand
GMR defaults, historical copies, live electrical edits and copied ratings.
Cable phases may have trailing bare neutrals; concentric strands are always
eliminated before optional reduction of those bare neutrals. The resulting
impedance and capacitance use the existing typed line model and IR layout.
Direct `Line.spacing` is covered below. Tape-shield cables and mixed bare/cable
phase positions remain unqualified. The pinned engine rejects `LineGeometry like=` when copying
a CN geometry with an invalid type cast; PowerIO reports that case explicitly.

```sh
python evals/gridlabd/dss_cn_geometry_oracle.py --output /tmp/new-cn-geometry
POWERIO_GEOMETRY_ORACLE=/tmp/new-cn-geometry/matrices.json \
  cargo test -p powerio-dist --lib pinned_overhead_matrices -- --ignored
POWERIO_GEOMETRY_DEFINITION_ORACLE=/tmp/new-cn-geometry/definitions.json \
  cargo test -p powerio-dist --lib pinned_geometry_definitions -- --ignored
python evals/gridlabd/dss_geometry_cache_oracle.py --output /tmp/new-geometry-cache
POWERIO_GEOMETRY_CACHE_ORACLE=/tmp/new-geometry-cache/observations.json \
  cargo test -p powerio-dist --lib pinned_calculation_history -- --ignored
```

The cable oracle checks 75 native matrices across three earth models, five
frequencies and five conductor/phase/reduction configurations. It records
convergence separately: the unloaded high-frequency circuits are matrix probes,
not feeder qualifications. Nine successful definition probes and the native
copy failure are recorded separately. Sixteen cache histories cover both
conductor families, including live cable edits and an overhead-to-cable change.

The complete North American master now imports with no electrical-readiness
blockers and survives IR and repeated canonical conversion. Native and emitted
solves converge, but the 21,671-observable comparison still fails: maximum
errors are approximately 26 microvolts, 57 microamps and 7.9 VA. Its `SecPar`
variant also remains unqualified. These residuals remain visible under the
unchanged acceptance thresholds. The three controlled public-geometry probes
pass independently; their success does not qualify the complete feeder.


### Direct Line.spacing and IEEE9500

Direct spacing retains live `LineSpacing`, `WireData` and `CNData` references.
The reader preserves assignment-time dimensions and ratings, later edits,
copied definitions and the native per-Line frequency cache. Unlike a
`LineGeometry.spacing` assignment, this path reads current coordinates when it
recalculates. Missing historical bindings and unsafe live conductor-count
changes remain explicit errors. The calculation also preserves the native
order of earth-model selection; the temporary geometry is first calculated
with the previous active earth model.

```sh
python evals/gridlabd/dss_direct_spacing_oracle.py --output /tmp/new-direct-spacing
POWERIO_GEOMETRY_CACHE_ORACLE=/tmp/new-direct-spacing/observations.json \
  cargo test -p powerio-dist --lib pinned_calculation_history -- --ignored
python evals/gridlabd/ieee9500_spacing_oracle.py \
  --master /path/to/ieee9500dss/ieee9500_base.dss \
  --bridge /path/to/dist_network --output /tmp/new-ieee9500-spacing
```

Twenty native histories compare line primitives and current ratings. The public
spacing oracle selects one original line for each overhead/cable and phase-count
combination, keeps its definitions and length, and replaces its endpoints with
a controlled source and unequal loads. All five selections pass: overhead
single-, two- and three-phase lines and single- and three-phase CN cables.
Together they compare 150 complex quantities, IR, repeated canonical output
and changed-line negative controls. They are line probes, not full-feeder
qualifications.

The `electricdss-tst` IEEE9500 master and its coordinate and UUID companions are
now pinned in the catalog. Its 2,622 unresolved geometry entries are eliminated;
import and IR equality succeed. The complete source does not converge in the
pinned native engine under the written fixed-state profile. Normal and Newton
methods both fail within 1,000 iterations at tolerances from `1e-11` to `1e-8`;
the original source with controls enabled also fails. Canonical convergence
alone cannot qualify that conversion. The separately pinned CIMHub balanced
and unbalanced archives remain distinct cases.
The full-feeder comparator also includes Generator, PVSystem and Storage ports;
missing converted ports are reported, not silently omitted. UUID resolution
uses the source directory while native automatic directory changes remain off.


Both CIMHub archives are exercised by:

```sh
python evals/gridlabd/ieee9500_archive_import_oracle.py \
  --archives /path/to/pinned/archives --bridge /path/to/dist_network \
  --output /tmp/new-ieee9500-archives
```

The archive bytes are checked against the catalog before extraction. Both
fixed-state native cases converge. Their source-path series reactor now lowers
to an existing conductor-resolved impedance branch, restoring the downstream
connection in canonical output. No public network or IR type is added.

The scalar series-reactor profile requires explicit `x`, permits `r`, preserves
phase and terminal order, and scales reactance from `basefreq` to the snapshot
frequency. Default and explicit current ratings are retained. Competing
impedance specifications, delta connections, damping/curves, copy histories,
non-default creation-frequency history and terminal/enable commands remain
explicitly unqualified. Active untyped reactors block electrical readiness;
a disabled property cannot hide an unresolved enable command. The comparator
uses the typed branch's original-element identity to compare the reactor's
physical ports even though its canonical class is Line.

```sh
python evals/gridlabd/dss_series_reactor_oracle.py \
  --bridge /path/to/dist_network --output /tmp/new-series-reactor
```

Fifteen native cases cover single-, two- and three-phase ports, resistance,
frequency scaling, assignment-time current ratings, IR, repeated canonical
conversion and changed-line negative controls. All pass. Both complete CIMHub
IEEE9500 canonical cases energize the feeder, but remain unqualified.

Rated wye shunt reactors with explicit series resistance or parallel damping
also use the existing shunt matrix. Native Reactor.R does not change the
kV/kvar specification mode: X still comes from the rating, with R retained.
This includes the R=0 declarations in the public IEEE118 case. Rp contributes
parallel conductance, including when series R is omitted. The reader evaluates
the inductance at the snapshot frequency and preserves normal/emergency
current ratings. Competing impedance specifications, copied state, explicit
return buses, delta resistance, curves and terminal commands remain outside
this added profile; existing series/grounding profiles are unchanged.

```sh
python evals/gridlabd/dss_rated_reactor_oracle.py --bridge /path/to/dist_network --output /tmp/new-rated-reactors
POWERIO_RATED_REACTOR_CASES=/tmp/new-rated-reactors cargo test -p powerio-dist --test dss_rated_reactor export_edited_rated_reactor_matrices -- --ignored
python evals/gridlabd/dss_rated_reactor_oracle.py --bridge /path/to/dist_network --output /tmp/new-rated-reactors --check-edits
```

Thirty cases cover one/two/three phases, 50/60 Hz, zero/nonzero resistance,
parallel damping and implicit R. Checks include canonical output, reimport,
current ratings and changed-line negative controls. Independent native edits
reduce resistance/damping and increase kvar to match actual edited typed
matrices; ratings are explicitly held fixed, as in the typed edit. Scalar
shunt emission reuses a retained kvar only when its projection exactly equals
the current matrix, so stale metadata cannot undo an edit.

IEEE118 now parses through IR and emits canonical DSS. Its original native
baseline does not converge at the pinned validation settings, so no whole-case
electrical qualification is claimed. Its PV-generator behavior and the broader
generator-model audit remain separate work.

### Fixed OpenDSS transformer terminals

Explicit Open/Close commands bind an existing Transformer and materialize its
final conductor states. A conductor index of zero changes phase conductors
only; the neutral remains in its current state. The unchanged typed winding
circuit moves open ports to internal zero-current nodes. Closed non-grounded
ports use ideal contacts; closed grounded returns stay directly on earth.
Every external conductor receives the pinned engine's 1e-12 S stabilization
when any port opens. Delta's unused conductor does not create an isolated
internal node. Two-phase delta retains all three hot terminals of its open
chain, including an explicit third bus node.

The native oracle measures every original terminal/conductor, including the
neutral and unused slots, plus aggregate external power and losses. It verifies
the declared winding/contact/stabilization inventory and observes only those
components. Internal element losses do not replace the original port loss.

```sh
python evals/gridlabd/dss_open_transformer_oracle.py --bridge /path/to/dist_network --output /tmp/new-open-transformers
POWERIO_OPEN_TRANSFORMER_CASES=/tmp/new-open-transformers cargo test -p powerio-dist --test dss_open_transformer export_edited_open_transformer_windings -- --ignored
python evals/gridlabd/dss_open_transformer_oracle.py --bridge /path/to/dist_network --output /tmp/new-open-transformers --check-edits
```

Eighty-four cases cover one/two/three phases, delta/wye connections, either
terminal, phase/neutral/unused-slot openings and three-winding units. All pass
canonical output, IR equality, reimport and changed-line negative controls.
All 84 actual typed winding-resistance/primary-tap edits also match independent
native edits, with 4,408 complex observations per pass. The unchanged input
fails each edited comparison as a negative control.

Layout/XfmrCode/copy changes after terminal commands and disabled transformers
with terminal commands still fail explicitly. Ordinary parameter edits preserve
the materialized state; reclosed ports use the ordinary transformer mapping.
Active protection execution is excluded. Enabled unconverted Fault objects now
block electrical use and canonical emission instead of being omitted. The
public DG protection input therefore gains parsing/IR support, without a
whole-case electrical qualification or canonical DSS claim.

### IEEE9500 PV phases and static BatchEdit commands

PVSystem and fixed-dispatch inverter output uses the typed per-phase rating
count. A two-phase wye inverter has two phases and a separate return terminal;
the return does not become a third generating phase. Thirteen native cases
cover wye/delta connections, phase permutations, power factors and a split-phase
service, with IR, repeated output and negative controls.

`BatchEdit` applies properties to the objects present at that point in the
script, in native visit order. Name matching is case-insensitive regular
expression search, without implicit anchoring. The reader uses the existing
ordered property/copy machinery and charges visits and assignments against its
work budgets. Supported ASCII expressions include anchors, alternation,
character classes and repetitions. Unsupported expressions such as lookarounds
or backreferences, non-ASCII matching and unknown classes report
`READ.DSS.BATCH_UNRESOLVED`; they are not ignored. A continuation following a
batch edits the last visited object, even when its name did not match. Load
copies preserve the destination's bus connection.

```sh
python evals/gridlabd/dss_pv_phase_oracle.py \
  --bridge /path/to/dist_network --output /tmp/new-pv-phases
python evals/gridlabd/dss_batch_edit_oracle.py \
  --bridge /path/to/dist_network --output /tmp/new-batch-edits
```

In the balanced CIMHub9500 case, fixing the PV phase count restores total PV
output from 2,303.11 kW to the native 2,954.66 kW. Applying the feeder's
`BatchEdit load..* vminpu=0.88` restores the native total load of 13,668.991 kW.
Maximum voltage differences fall from 88.8 V to 9.86 V after the PV fix, then to
0.638 V after the load edit. The unbalanced case reaches 0.638 V as well.
These are still failed comparisons under unchanged thresholds. Matched passive
admittance entries agree within `9.1e-13` S in the balanced diagnostic.

### IEEE9500 fixed battery power

The two idling Storage objects in each CIMHub IEEE9500 archive consume 2.5 kW
each. The reader represents their AC demand as typed `BoundedConstantPower` loads: constant power inside the
voltage band, impedance continuation outside it, and an optional current limit.
The law survives IR and typed edits. Canonical DSS writes a fixed Storage
equivalent, using discharging for negative demand and idling for positive or
zero demand; it does not reconstruct battery dispatch or energy capacity.

The qualified profile covers one-phase wye and two-/three-phase wye or delta,
voltage model 1, and fixed idling, charging or discharging P/Q setpoints within
inverter ratings. Voltage base, kW/rating and reactive-mode assignment order are
preserved. State-of-charge guards run at each native edit boundary: a full
battery requested to charge becomes idling, and a later SOC edit alone does not
restart it. The pinned constructor initially has zero absolute reserve despite
its displayed 20% setting; an explicit `kWhRated` or `%Reserve` setter updates
the absolute reserve. Charge/discharge percentages use the default rated-kW cap.
Efficiency settings without an inverter curve do not alter the fixed AC power.
Efficiency curves, forced balance, copied inverter state, one-phase delta,
negative charge/discharge percentages and inverter P/Q clipping remain explicit
electrical blockers. Untyped active storage reports `READINESS.DSS.STORAGE_UNRESOLVED`. PMD/BMOPF projections report
the loss of voltage bounds and current limits. Legacy C load views reject this
law rather than presenting it as unbounded constant power.

```sh
python evals/gridlabd/dss_storage_oracle.py \
  --bridge /path/to/dist_network --output /tmp/new-storage-sweeps
POWERIO_STORAGE_SAMPLES=/tmp/new-storage-sweeps/report.json \
  cargo test -p powerio --test load_voltage_ir \
  typed_static_storage_powers_match_native_voltage_sweeps -- --ignored
```

The 239-case sweep compares original, canonical and reimported bus voltages,
terminal currents, powers and losses with unchanged thresholds. It covers all
three states, five connection layouts, both current-limit settings, seven
voltage levels, reactive-only output, and assignment/edit histories. An altered
line negative control must fail. The Rust check separately evaluates typed branch powers at
the native voltages, including high-voltage current limiting and low-voltage
impedance continuation. Zero-power cases include an independent background load
so the changed-line control still exercises an energized line. Typed edits may
mix consuming, generating and reactive-only branches; canonical output splits
them without losing terminal mappings.

The separate electricdss-tst IEEE9500 entry now imports through IR and emits
canonical DSS without readiness blockers. Its two batteries default to 25 kW
ratings and each draws 0.25 kW at the written state, including the first battery's
full-SOC transition from charging to idling. The original native feeder still
does not converge at the pinned strict snapshot settings, so this entry remains
unqualified. Parser success is recorded separately from electrical matching.

Including the idle demand reduces maximum voltage differences to approximately
0.00111 V (balanced) and 0.00118 V (unbalanced), from 0.638 V. Both complete
IEEE9500 comparisons still fail. The current comparisons retain all 68,720
balanced and 72,545 unbalanced observations, including the original switch
ports through the mappings below. There are 26,368 and 30,706 strict failures,
respectively, in both canonical passes. Aggregate power and native convergence
do not qualify a feeder.

### External ports of fixed-open Line equivalents

Fixed-open Lines retain their original element identity and the names of their
two external contacts on the reduced-admittance shunt. The native comparator
checks those contacts against the original bus/conductor inventory and the
typed equivalent's internal terminals. It compares each external current and
power, and sums boundary power for total complex loss. It does not compare an
internal negative resistance's loss as if it were the whole original device.
Aliased reactors also retain original conductor identities when canonical DSS
moves a ground return to the last conductor. Ambiguous repeated-node
permutations are refused.

```sh
python evals/gridlabd/dss_open_ports_oracle.py \
  --bridge /path/to/dist_network --output /tmp/new-open-port-results
```

Ten native cases cover single phase, permuted three phase, explicit neutrals,
partial/whole terminal openings, generated-name collisions and reactor return
permutations. Wrong interface bus, node and element mappings must be rejected;
the altered-line electrical negative control must also fail. Both IEEE9500
archive comparisons now include all original observations, including the 85
previously missing switch observations in each case. Strict electrical residuals
remain failures.

### Native declaration-order sensitivity

The following diagnostic reorders only independent, explicitly parameterized
fixed-switch declarations in already acquired IEEE9500 profiles. It verifies
original profile hashes, component bus/node inventories and every native YPrim
entry before comparing solutions. It performs no PowerIO conversion and never
qualifies a feeder.

```sh
python evals/gridlabd/ieee9500_native_order_oracle.py \
  --profile-run /path/to/ieee9500-archive-results \
  --output /tmp/new-native-order-results
```

With identical native admittance entries, the balanced and unbalanced controls
show maximum voltage differences of about 0.00225 V and 0.00291 V. Their maximum
power differences are about 17.5 VA and 13.7 VA. This establishes native numerical
sensitivity under the existing strict thresholds. It does not establish that
all conversion residuals are numerical, widen tolerances, or replace missing
model validation with a passing classification.


### IEEE 4 AutoTrans coil equivalents

The DSS reader supports the three-phase series/common-wye AutoTrans profile
with two windings or a delta tertiary, grounded common returns and unity taps.
It requires explicit scalar leakage measurements and winding buses. Arrays,
MakeLike, terminal operations, non-unity taps and other connections remain
untyped with an electrical-readiness blocker. Disabled unsupported equipment
can remain source-only. This is a bounded static profile, not full AutoTrans
support.

The typed representation uses three existing single-phase transformers and
ideal contacts. Series-coil voltage and leakage parameters use the native
HV-minus-LV base correction. Creation frequency, core losses and antifloat
admittance are retained. Each coil uses the existing two-ended delta connection;
ordinary single-phase wye would add a neutral admittance absent from AutoTrans.

```sh
python evals/gridlabd/dss_autotrans_oracle.py \
  --bridge /path/to/dist_network --output /tmp/new-autotrans-results
```

Eight energized native cases check two/three windings, phase permutations,
unequal loading, core-loss settings and 50/60 Hz snapshots. They compare native
primitive admittances, voltages, all reported terminal currents and powers,
losses, IR round trips and repeated canonical output. Wrong contact nodes,
coil buses and missing banks are rejected. Altering a feeder line must fail the
electrical comparison.

Native AutoTrans reporting needs special treatment: common hot currents include
the series return, but the series-return slots remain in the API output.
`Powers` uses these adjusted currents; `Losses` uses the unadjusted primitive.
The observer reproduces both conventions without dropping the duplicated slots
or replacing losses with the sum of reported powers.

The public CIMHub IEEE 4 `AutoAuto.dss` now parses into typed equipment and
retains all 77 electrical observations. Its autotransformer observations match,
but the complete feeder still fails strict source/switch comparisons (about
0.000503 A and 100.23 VA maximum residual in the pinned run). It remains a failed
feeder qualification. The broader IEEE 4 suite is 20/27 qualified; no
thresholds have changed.
