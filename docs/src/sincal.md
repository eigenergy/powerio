# PSS SINCAL

SINCAL is one native project format capable of holding both balanced and
conductor-resolved networks. Select the electrical profile explicitly. The
current balanced reader produces the existing `BalancedNetwork`; it does not
infer balance from equal phase values or automatically reduce unbalanced data.
The multiconductor reader is being developed separately in `powerio-dist`.

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

## Fidelity and output

Native physical quantities use a declared 100 MVA internal conversion base.
Disabled native generator capability limits become unbounded typed limits,
not zero capability. Diagnostics identify this and data outside the chosen
snapshot. Disabled modern profile references do not replace static powers.
Active profiles require a supported adapter and explicit snapshot selection;
currently only the declared schema-11.5 absolute daily profile is accepted.
Fault, dynamic, protection, economic, diagram and stored-result
data remain in the retained source. Cross-format output reports their omission.

An unchanged module can emit `sincal` or `sincal-balanced` to reproduce its
primary native bytes exactly. A SQLite input echoes SQLite bytes; an archive
input echoes archive bytes; an acquired MDB input echoes the original MDB.
Use an appropriate destination filename:

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
