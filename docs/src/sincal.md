# PSS SINCAL

SINCAL is one native project format capable of holding both balanced and
conductor-resolved networks. Select the electrical profile explicitly. The
current balanced reader produces the existing `BalancedNetwork`; it does not
infer balance from equal phase values or automatically reduce unbalanced data.
The explicit multiconductor profile produces `MulticonductorNetwork` through
`powerio-dist`; neither profile silently retries the other family.

## Balanced reader

The `sincal-balanced` input token selects positive-sequence load flow. The
current profile accepts native SQLite schemas 14.8, 15.5 and 16.0 and `.sinx` archives,
with one base variant. Rust also accepts explicitly acquired schema-11.5 Access
records and selected absolute daily snapshots, as described below. It covers
buses, positive-sequence lines, static loads,
external sources, converter injections and two-winding transformers with fixed
common taps, plus fixed capacitor banks. Ideal external sources support absolute
and relative source/terminal voltage prescriptions and voltage-only limits.
Open terminals and inactive equipment remain in the network.
Unknown required modes fail with table, native record ID and field context.

```rust,ignore
let options = powerio::ParseOptions::default().format("sincal-balanced")?;
let module = powerio::parse_with_options("case.sinx", &options)?;
assert!(matches!(module.value(), powerio::PioValue::BalancedNetwork(_)));
powerio::emit(&module, "matpower", "case.m")?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

```sh
powerio summary case.sinx --from sincal-balanced
powerio convert case.sinx --from sincal-balanced --to matpower -o case.m
powerio serialize case.sinx --from sincal-balanced -o case.pio.json
```

Python uses `powerio.parse(path, format="sincal-balanced")`. The C ABI uses the
existing `pio_parse` format argument and `pio_value_balanced_network` typed
accessor. No new network type or binding ABI is introduced.

Bare `sincal` and an undeclared `.sinx` require profile selection. Explicit
selection does not authorize discarding incompatible phase-resolved data:
unsupported terminal or load modes are errors, with no fallback to another
family. The native model's source-format identity is `sincal`; the retained
source records the selected input profile as `sincal-balanced`.

## Multiconductor reader

Use `sincal-multiconductor` to select the conductor-resolved reader. A symmetric
operating point still produces `MulticonductorNetwork`. The verified electrical
profiles include schema-14.8 native SQLite/archive inputs and schema-11.5/12.8
Access acquisition records. Shared structural admission of other versions does
not imply their electrical support. Unsupported equipment or missing required
sequence data rejects the complete parse; no partial feeder is returned.
Nominal delta–delta transformers support one, two or three installed coils,
including reversed polarity and open terminals. Winding selectors expand to
actual phase pairs; no unused phase is invented. Partial mixed-winding
transformers and inconsistent native core-loss inputs still reject.

```sh
powerio summary case.db --from sincal-multiconductor
powerio convert case.db --from sincal-multiconductor --to pmd-json -o case.json
powerio convert case.db --from sincal-multiconductor --to sincal-multiconductor -o copy.db
```

Python's existing `parse(..., format="sincal-multiconductor")` and the C ABI's
existing `pio_parse` route native SQLite/archive inputs to their established
multiconductor typed accessors. No ABI entry point or network family is added.

The Rust facade exposes explicit variant/snapshot/acquisition selection through
`ParseOptions.sincal_multiconductor`. Python uses `dist.SincalReadOptions`, and
the CLI exposes the same selections on `summary`, `convert` and `serialize`.
Access parsing requires
the original MDB plus the optional helper's acquired tables, supplied as a
relative source companion. Parsing does not run MDB Tools. The recorded original
length and SHA-256 must match the MDB; this catches mismatched input files, but
is not an attestation that caller-supplied table contents are authentic.

```rust,no_run
let mut selection = powerio::dist::SincalReadOptions::default();
selection.variant = Some(1);
selection.snapshot_hours = Some(12.0);
selection.acquired_tables = Some("acquired.json".into());
let mut options = powerio::ParseOptions::default().format("sincal-multiconductor")?;
options.sincal_multiconductor = Some(selection);
let module = powerio::parse_with_options("project/original.mdb", &options)?;
assert!(matches!(module.value(), powerio::PioValue::MulticonductorNetwork(_)));
# Ok::<(), Box<dyn std::error::Error>>(())
```

Create `project/acquired.json` with `evals/sincal/import_access.py`. In-memory
sources supply the companion with `Source::with_named_buffer`; file companions
stay under the source's acquisition root. For example:

```python
import powerio
from powerio.dist import SincalReadOptions

module = powerio.parse(
    "project/original.mdb", format="sincal-multiconductor",
    sincal_multiconductor=SincalReadOptions(
        variant=1, snapshot_hours=12, acquired_tables="acquired.json"),
)
```

```sh
powerio summary project/original.mdb --from sincal-multiconductor \
  --sincal-variant 1 --sincal-snapshot-hours 12 \
  --sincal-acquired-tables acquired.json
```

Python memory inputs supply `named_buffers={"acquired.json": records_bytes}`.
They never read companions from disk. File inputs may explicitly widen the
root with `acquisition_root=...` in Python or `--acquisition-root` in the CLI;
the root must contain the primary and referenced files. The normal root is the
primary file's parent directory. Julia uses the same keywords with its exported
`SincalReadOptions`:

```julia
using PowerIO
module_ = parse("project/original.mdb"; format="sincal-multiconductor",
    sincal_multiconductor=SincalReadOptions(
        variant=1, snapshot_hours=12, acquired_tables="acquired.json"))
```

Julia memory/IO inputs take `named_buffers=Dict("acquired.json" => records_bytes)`.
C uses `pio_parse_with_options` with borrowed `PioParseOptions` and
`PioSincalReadOptions`; zero initialization leaves optional values absent, and
presence flags distinguish an omitted snapshot from explicit midnight. The
existing `pio_parse` remains unchanged. For memory inputs,
`pio_source_from_memory_with_buffers` copies the primary and named companions
before returning; caller buffers may then be released. These additions require
the matching development library and Julia companion. Existing ABI 7 layouts
and typed network accessors are unchanged.

Legacy UI-manipulator references on loads retain their IDs as provenance. Their
edits are already stored in the load input fields, so stored power factors apply
once; the UI definition is not evaluated again. This follows
[Siemens Release Notes 21.0, pp. 4–6](https://sincal.s3.amazonaws.com/21.0/ReleaseNotes-Eng.pdf).
Conflicting daily timestamps remain errors.

Active daily profiles require an explicit snapshot; no midnight default is
assumed. In schema 11.5, absolute P/Q profiles replace aggregate powers, while
common relative-factor profiles scale the defined powers, preserving unequal
Wye or delta branches and their voltage dependence. Relative profiles currently
require the documented direct-scaling coefficient settings: `Power_a1=0`,
`Power_b1=0`, `Reduce_a2=0`, `Reduce_b2=1`. They accept finite nonnegative factors,
linear or step interpolation and cyclic repetition. Separate P/Q-factor modes,
topology-dependent coincidence and absolute-profile allocation to unequal
branches remain unsupported. Generic time-series and inherited variants remain
under development.

Schema-11.5 NULL transformer tap status uses the documented fixed-status default
and records that interpretation. Active controllers and unresolved partial
mixed-winding circuits still reject; the default does not supply missing physics.

Fixed reactor and capacitor banks retain their phase connections, losses,
grounding and open/inactive states as typed shunts and switches. Supported
three-phase grounded banks retain separate positive- and zero-sequence inputs.
Explicit neutral impedances, automatic regulators and stepped direct
zero-sequence impedances still require additional mapping. Documented optional
schema-11.5 defaults are recorded; required ratings are never filled in.
The Access acquisition helper includes both bank tables by default.
For schema 11.5, a NULL voltage-level line/cable temperature uses its documented
20 C default. The selected field is recorded in `network.defaulted`; explicit
temperatures retain their correction, and missing columns or modern NULLs fail.
The same legacy profile admits documented defaults for optional line flags,
dielectric losses, parallel counts, rating factors, rated frequency and active
temperature coefficients. Required impedances and sequence declarations remain
strict. Ideal connections record their applied defaults as well.

PowerIO's primary-file limit remains 64 MiB by default. For a known larger input,
use its existing explicit `POWERIO_MAX_PRIMARY_BYTES` setting. CSIRO09 is
69,181,440 bytes; its validation uses that exact bound. Acquired table documents
retain their separate 64 MiB limit. The library does not silently raise either.

## Fidelity and output

In the balanced profile, native physical quantities use a declared 100 MVA
internal conversion base. The multiconductor model uses its usual SI units.
Disabled native generator capability limits become unbounded typed limits,
not zero capability. Diagnostics identify this and data outside the chosen
snapshot. Disabled modern profile references do not replace static powers.
Active profiles require a supported adapter and explicit snapshot selection;
currently only the declared schema-11.5 absolute daily profile is accepted.
Fault, dynamic, protection, economic, diagram and stored-result
data remain in the retained source. Cross-format output reports their omission.

An unchanged module can emit `sincal` or its matching explicit profile token
to reproduce its primary native bytes exactly. A SQLite input echoes SQLite
bytes; an archive echoes archive bytes; an explicitly acquired Access case
echoes the original MDB, never the intermediate tables. An explicit output
profile for the other network family is refused. Use an appropriate destination filename:

```sh
powerio convert case.sinx --from sincal-balanced --to sincal-balanced -o copy.sinx
```

This is source echo, not a fresh writer: format metadata reports `can_emit=false`.
Editing the value removes its retained native source. PowerIO IR retains typed
values and provenance, not the original native project bytes. Edited,
constructed and IR-restored modules therefore refuse native output without
creating a partial file. The planned experimental writer has a separate
acceptance scope.

Access/MDB acquisition currently uses the explicit optional helper under
`evals/sincal`; ordinary library parsing never starts external programs. Its
internal typed records are not native SQLite output. The Rust balanced selection
API accepts them as a companion to their original MDB.

## Validation

The reader maps all 15 nodes and 32 equipment records of the small licensed
SimBench native case. Rust tests cover malformed/unsupported input, service
states, open terminals, typed routing, binary echo and IR behavior. The
`evals/sincal/check_balanced_simbench.py` harness checks actual Rust output
against paired publisher CSV inputs and a fresh pandapower 3.2.2 calculation,
including derived nonzero taps on either winding. Stored native results are
separate evidence; no SINCAL desktop execution has been performed.

Three external cases also map completely: IEEE18 (18 native nodes, 44 elements),
IEEE33 (33 nodes, 67 elements), and the student study (40 nodes, 84 elements).
Their fresh independent pandapower comparisons agree within 4.5e-12 pu in
complex bus voltage. The student file's saved static load powers are zero with
profiles disabled; its historical profile results are not the state parsed here.
The external validation report records this distinction. No external native
model files are vendored.

Additional schemas, active profiles, variants and corpus cases remain under development. Native
fixtures require redistribution rights; external research models are not
silently copied into the test suite.

The public multiconductor facade maps all 688 CSIRO09 elements at five selected
daily snapshots. Its original powers are symmetric; five separately labelled
unequal-delta-branch stress cases exercise asymmetric behavior. Independent
OpenDSS comparisons cover every phase at 617 energized native nodes and four
isolated nodes, with maximum voltage difference below 0.000372 V. The finite
OpenDSS source approximation is checked separately. The same public path checks
original MDB echo and IR value preservation; see `evals/sincal/csiro09-public.json`.
This establishes one complete conductor-resolved case, not complete corpus
coverage or native SINCAL desktop acceptance.

### Explicit balanced Access snapshots in Rust

The balanced reader also accepts schema-11.5 Access acquisition records through
`powerio_tx::format::SincalBalancedReadOptions`, carried by
`powerio::ParseOptions::sincal_balanced`. Select `sincal-balanced` explicitly,
set `variant` and `snapshot_hours` when required, and set `acquired_tables` to
the relative companion containing `import_access.py` output. Memory sources attach
that companion with `Source::with_named_buffer`; file sources use the acquisition
root. Parsing verifies the original MDB header, byte count and SHA-256 against
those records and never invokes MDB Tools. Include `NetworkGroup` and
`NetworkGroupTrans` in acquisition so balanced interchange controls are checked.

CSIRO19 is checked at seven daily snapshots through this public path. Profiles
currently admit absolute daily P/Q with linear or stepped interpolation. Modern
active profiles, inherited variants and other profile modes still reject.
Legacy voltage/temperature defaults are narrowly scoped and retained in component
metadata. Balanced Access selection through CLI/Python/C/Julia is not yet exposed.
