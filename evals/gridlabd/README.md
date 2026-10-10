# GridLAB-D and OpenDSS electrical validation

These tools calibrate pinned engines against independent circuits and validate
specific PowerIO static conversion profiles. They exercise canonical output,
edits, IR round trips and cross-format conversion. A parser pass, an unchanged
source echo or an engine exit code alone does not establish electrical support.
The user-facing format guide is [Distribution formats](../../docs/src/distribution.md).

## Provision the engines

[engines.json](engines.json) pins the following independent runtimes:

| Profile | Runtime | Pin |
| --- | --- | --- |
| PNNL | GridLAB-D 5.3.0 macOS | Archive and runtime-file SHA-256 hashes |
| Arras | Arras Energy 4.3.17 Linux amd64 | Container digest and reported version |
| DSS | OpenDSSDirect 0.9.4 / DSS C-API 0.14.5 | Package versions and engine revision, based on SVN 3723 |

Keep engine installations, public feeders and generated results outside git.
Create a Python 3.14 virtual environment and install [requirements.txt](requirements.txt).
Download the PNNL archive named in the lock, verify its archive hash, unpack it,
and set `PNNL_ROOT` to the directory containing `bin/`, `lib/` and `share/`.
Pull the exact Arras image from the lock. The two GridLAB-D binaries require
x86_64/amd64 execution support, including on Apple Silicon hosts. An independently
built engine needs a separately verified profile; do not bypass the checksum checks.

Arras runs with networking disabled. Its launcher shell mode initializes the
runtime without a post-run GitHub version check. Docker file copying works with
remote and Colima daemons; the scripts do not depend on host bind mounts.
PNNL requires shared-memory access that some shell sandboxes deny. Record that
execution requirement in the validation evidence.

Run commands from the repository root with the evaluation environment active.
Every result directory must be new: stale recordings must never qualify a failed
rerun. Preserve command failures and explicit retry evidence.
If other Cargo checks run concurrently, copy the built validation executables
to a separate run directory and pass those copies as `--bridge` and `--powerio`.
Their recorded hashes must remain stable throughout the run. Each script's
`--help` lists its required bridges, inputs and subset selectors.

## Calibrate before converting

```sh
python -m unittest discover -s evals/gridlabd -v
python evals/gridlabd/oracle.py --pnnl-root "$PNNL_ROOT" --output /tmp/gld-calibration
python evals/gridlabd/neutral_oracle.py --output /tmp/gld-neutral
python evals/gridlabd/transformer_oracle.py --pnnl-root "$PNNL_ROOT" --output /tmp/gld-transformers
python evals/gridlabd/upstream.py --pnnl-root "$PNNL_ROOT" --output /tmp/gld-upstream
```

`oracle.py` compares single-phase and coupled unbalanced three-phase circuits
with the independent solution `(Z_line + diag(Z_load)) I = V_source`.
`neutral_oracle.py` solves unreduced four-wire DSS circuits with finite grounding
and a removed-ground negative control. `transformer_oracle.py` stamps independent
single-phase and grounded center-tapped winding equations, unequal secondary
leakages and leg-neutral/leg-leg loads. It also checks complete DSS winding
primitives and a reversed-secondary negative control.

These calibrations preserve differences between engine models. The basic DSS
source has finite impedance and uses measured terminal voltage in its reference;
that check does not establish ideal-source conversion. The transformer reference
includes GridLAB-D's declared core shunt and separately evaluates DSS's zero-core
case. An explicit-neutral DSS pass does not establish a floating-neutral GLM
mapping. Native split-phase leg-2 voltage has reversed polarity; the adapter
preserves physical current polarity and verifies line-line voltage and power.

[upstream.json](upstream.json) pins the unmodified PNNL IEEE13 NR/FBS and IEEE37 NR
regressions, their input hashes, license source and 70 original complex assertions.
IEEE13 includes 2, 8 and 10 V tolerances; IEEE37 checks six line-line voltages within
0.5 V. These tests retain native controls and establish engine sanity. They are
separate from fixed-state conversion qualification.

## Build the conversion bridges

```sh
cargo build -p powerio --example glm_network --example glm_roundtrip --example dist_network
cargo build -p powerio-dist --example glm_objects --example glm_preprocess --example glm_quantities
```

`glm_network` reads a source project through the facade and exposes typed data
and IR. `glm_roundtrip` emits from current typed values, including restored,
edited, rebased and grounded-reference stages. `dist_network` exercises DSS
import, IR and canonical output. Bridge binaries are hashed in the reports.
The frontend examples expose source object identities, coordinates,
preprocessing and electrical quantities without claiming network conversion.

The format frontend preserves original buffers and source spans, follows the
bounded local include policy, and selects PNNL/Arras behavior explicitly.
Quoted and unquoted native units can differ. Do not replace ordered native
setters, explicit defaults or profile-dependent unit interpretation with guessed
host state. Unsupported executable or electrical semantics remain explicit gaps.

## Choose a qualification suite

| Suite | Scope |
| --- | --- |
| `preprocess_oracle.py`, `object_oracle.py`, `coordinate_oracle.py`, `quantity_oracle.py` | Source expansion, explicit references, declared/initialized coordinates, quantities and unit-converted circuits |
| `roundtrip_oracle.py`, `parent_roundtrip_oracle.py`, `location_roundtrip_oracle.py` | Static phase networks, parent chains and geographic attachments |
| `switch_roundtrip_oracle.py`, `ideal_switch_oracle.py`, `pair_load_oracle.py`, `line_shunt_oracle.py` | Fixed contacts, phase-pair laws and physical line-end shunts |
| `overhead_roundtrip_oracle.py`, `overhead_dss_oracle.py`, `cable_oracle.py` | Conductor geometry, reduced return paths, fork-specific cable laws and physical ports |
| `transformer_roundtrip_oracle.py`, `glm_service_write_oracle.py`, `glm_coil_bank_write_oracle.py`, `glm_ideal_regulator_write_oracle.py` | Winding connections, grounded services, coherent coil banks and fixed ideal ratios |
| `glm_injection_write_oracle.py`, `current_form_write_oracle.py` | Fixed injections and exact finite-impedance current variables |
| `feeder_oracle.py` | Selected pinned IEEE4/13/34/37/123 states, including documented IEEE13/37 edits |
| `ieee8500_import_oracle.py`, `ieee8500_write_oracle.py` | Complete pinned IEEE8500 import and canonical/repeated conversion |
| `ieee9500_import_oracle.py`, `ieee9500_write_oracle.py` | Balanced/unbalanced IEEE9500 import, IR, repeated DSS/GLM and physical-port balance |
| `dss_corpus_audit.py`, `ieee_distribution_import_oracle.py`, `ieee4_import_oracle.py` | Public DSS inventory and selected electrical qualifications |

Suites that audit native discrepancies or unfinished conversion profiles may
intentionally fail. A passed subset does not qualify omitted cases or stages.
Use per-case reports and the compatibility matrix to distinguish native support,
verified equivalents, excluded behavior and unresolved gaps.

For a current-form analytic circuit:

```sh
python evals/gridlabd/current_form_write_oracle.py \
  --bridge target/debug/examples/glm_roundtrip \
  --dss-bridge target/debug/examples/dist_network \
  --pnnl-root "$PNNL_ROOT" --output /tmp/gld-current-form
```

For IEEE9500, acquire the hash-pinned CIMHub `ieee9500` source tree outside git.
The import suite verifies each source file's hash. Its optional round-trip bridge
adds DSS reimport, IR equality and canonical re-emission checks.

```sh
python evals/gridlabd/ieee9500_import_oracle.py \
  --source /path/to/CIMHub/ieee9500 \
  --bridge target/debug/examples/glm_network \
  --roundtrip-bridge target/debug/examples/dist_network \
  --pnnl-root "$PNNL_ROOT" --output /tmp/gld-ieee9500-import

python evals/gridlabd/ieee9500_write_oracle.py \
  --source /path/to/CIMHub/ieee9500 \
  --bridge target/debug/examples/glm_roundtrip \
  --reference /tmp/gld-ieee9500-import \
  --pnnl-root "$PNNL_ROOT" --output /tmp/gld-ieee9500-canonical
```

Canonical qualification requires a passed import reference, identical engine
pins and all original measurement files. `--case`, `--profile` and `--stage`
select explicit subsets. `--execution-timeout` must be positive and finite;
a timeout remains a failed execution with retained stdout/stderr.
[ieee-corpus.json](ieee-corpus.json) records upstream case provenance.
Redistribution and fixture-size requirements apply before vendoring any input.

## Compare physical quantities

Voltages are conductor-to-earth volts. Currents and powers enter each device
terminal. Powers are VA, impedances ohms and admittances siemens. Loss is the
signed sum of physical terminal powers. Never fit an angle rotation, permute
phases implicitly, compare magnitudes alone or intersect mismatched identity sets.
Missing/extra quantities, wrong units, nonfinite values and empty snapshots fail.

GridLAB-D's line `b` matrix is a series impedance only for the qualified
series-only profile. DSS line impedance is recovered from `YPrim`. Native
readbacks can differ from the actual nodal equations: coupled shunts, transformer
connections and FBS auxiliary laws need their own profile-specific checks.
Raw readbacks remain evidence alongside physical circuit observations.

Exact equivalent circuits can have internal loads, branches and virtual sources.
Their boundary current includes all contributing series and endpoint-shunt
currents. Physical power uses the physical endpoint voltage times conjugate
boundary current. Internal branch losses or virtual-source powers must not be
substituted for the original device's loss or terminal power.

The shared scalar current-form plan preserves finite impedance through
`u = scale * I`; it does not replace a small impedance with an ideal contact or
an epsilon. Its analytic suite independently checks the finite stamp, auxiliary
KCL and original physical ports. Impedance and primitive-matrix tolerances stay
tight even when complete feeders use their separate flow budgets.

## Accuracy budgets and negative controls

[oracle.py](oracle.py) is the authority for absolute-plus-relative complex error:
`|actual - reference| <= atol + rtol * |reference|`.

| Quantity | Analytic calibration: absolute / relative | `feeder-1ppm-v1`: absolute / relative |
| --- | --- | --- |
| Voltage | 1e-6 V / 1e-9 | 1e-4 V / 1e-6 |
| Current | 1e-7 A / 1e-9 | 1e-5 A / 1e-6 |
| Power and loss | 1e-4 VA / 1e-9 | 1e-2 VA / 1e-6 |
| Impedance/admittance | 1e-10 ohm/S / 1e-10 | Unchanged |

Raw analytic-calibration comparisons remain recorded when a suite applies the
feeder profile. Near-zero quantities use the absolute floor. Current-form
microcases additionally request absolute 1e-9 V native stopping for the analytic
comparison; only stopping precision changes, not electrical coefficients.

[feeder_balance.py](feeder_balance.py) independently evaluates typed external
P/I/Z loads, fixed injections and shunts. Non-reference conductor KCL uses a
10 microampere floor plus one part per million of total incident current
magnitude. Unsupported external laws fail explicitly. Private transformer core
contributions stay at measured winding boundaries. Changed loads, wrong taps,
missing active contacts and altered grounds must remain detectable under the
same budgets. An absent inactive contact can receive zero flow only with explicit
retained inactive-state evidence; its omission remains diagnosed.

## Current limitations

Qualification applies to the stated source, engine and fixed-state profiles.
General explicit/floating neutrals, additional transformer connections and coil
banks, arbitrary GLM preprocessing/reference forms, further static device laws,
and complete geographic/binding coverage remain unfinished. Active controls,
protection sequences and behavioral simulation are excluded; relevant fixed
states still need preservation or an explicit missing-state diagnostic.

The IEEE9500 baseline, IR-restored, rebased and grounded-reference canonical
profiles qualify on both forks. The source-phase edit remains unqualified:
native NR DELTA_GWYE stamps extra primary zero-sequence current that its
reported transformer currents omit. The upstream voltage discrepancy and KCL
residual are electrical mapping failures. A balanced stiff-source microcase
does not expose this gap, and numerical tolerance must not hide it.

Canonical NR stopping scales with nominal voltage at 1e-9 per unit. FBS retains
its absolute stopping setting and 1,000-iteration budget. These convergence
settings do not change the electrical circuit or comparison budgets. Zero
capacitor admittances emit exact zero-current loads with preserved points and
class-rebasing diagnostics because Arras rejects zero-rated capacitor objects.

Qualification is determined by complete per-case reports and documented profile
limits. Historical pass counts and parser inventories do not establish electrical
compatibility. Keep failed execution and comparison evidence alongside successful
results.
