# PSS SINCAL

SINCAL is one native project format capable of holding both balanced and
conductor-resolved networks. Select the electrical profile explicitly. The
current balanced reader produces the existing `BalancedNetwork`; it does not
infer balance from equal phase values or automatically reduce unbalanced data.
The multiconductor reader is being developed separately in `powerio-dist`.

## Balanced reader

The `sincal-balanced` input token selects positive-sequence load flow. The
current profile accepts schema-14.8 native SQLite files and `.sinx` archives,
with one base variant. It covers buses, positive-sequence lines, static loads,
external sources, converter injections and two-winding transformers with fixed
common taps. Open terminals and inactive equipment remain in the network.
Unknown required modes fail with table, native record ID and field context.

```rust,no_run
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
static profile. Fault, dynamic, protection, economic, diagram and stored-result
data remain in the retained source. Cross-format output reports their omission.

An unchanged module can emit `sincal` or `sincal-balanced` to reproduce its
primary native bytes exactly. A SQLite input echoes SQLite bytes; an archive
input echoes archive bytes. Use an appropriate destination filename:

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
internal typed records are not native SQLite output and are not yet accepted
by this public balanced profile.

## Validation

The reader maps all 15 nodes and 32 equipment records of the small licensed
SimBench native case. Rust tests cover malformed/unsupported input, service
states, open terminals, typed routing, binary echo and IR behavior. The
`evals/sincal/check_balanced_simbench.py` harness checks actual Rust output
against paired publisher CSV inputs and a fresh pandapower 3.2.2 calculation,
including derived nonzero taps on either winding. Stored native results are
separate evidence; no SINCAL desktop execution has been performed.

Other schemas, profiles and corpus cases remain under development. Native
fixtures require redistribution rights; external research models are not
silently copied into the test suite.
