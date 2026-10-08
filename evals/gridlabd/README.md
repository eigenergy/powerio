# GridLAB-D and OpenDSS oracle calibration

These tools check independent engines against analytic static circuits before
using those engines to validate PowerIO conversion. They currently make **no
claim of PowerIO GLM support or full format compatibility**.

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

## Published references and limits

`coordinate_oracle.py` qualifies common object-header latitude and longitude in
both pinned GridLAB-D forks. Run it with `--output <new-directory>` and
`--pnnl-root <installation-directory>`. It checks signed decimal, hemisphere
decimal-minute and degree-minute-second literals, coordinate references, missing
and partial points, and explicit zero coordinates. The comparison decodes native
saved GLM; its tolerance accounts for the engine writer's hundredth-of-an-arcsecond
rounding. The two forks use different saved object and quoting syntax.

Both engines replace the test line's declared location with its endpoint
midpoint during initialization. Thus the saved engine location is derived state,
not evidence that an importer preserved the declared location. The case records
both. It does not establish a geographic datum, a route polyline, a conductor
spacing, or an electrical line length. These checks calibrate the oracle only;
PowerIO coordinate mapping and fresh/edited/IR conversion remain to be verified.

`upstream.json` records original PNNL source URLs, commit, hashes, license and
assertion inventories. `upstream.py` runs the unmodified IEEE 13 NR and FBS
regressions (32 complex assertions each) and IEEE 37 NR (six). The 13-node
tests include 27 tolerances of 2 V, four of 8 V, and one of 10 V. The 37-node
test begins downstream of the regulator and checks line-line voltages within
0.5 V. These native regression passes establish engine sanity; their original
controls and solver settings remain intact. They are not evidence of frozen
state conversion or high-precision feeder equivalence.

Only generated, original microcases are used for analytic calibration. Public
feeder inputs and generated engine outputs remain outside git. Other transformer
connections, floating split-phase services, load-law sweeps, explicit-neutral GLM
equivalents and larger public feeders require additional qualification. The
existing OpenDSS passive-admittance harness in `evals/validation/` remains a
separate check; it intentionally excludes transformer physics.
