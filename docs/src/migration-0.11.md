# From 0.10 to 0.11

PowerIO 0.11 breaks the 0.10 source API and the C ABI once, to remove
duplicate ways of doing the same thing and to expose the same concepts in
Rust, Python, Julia, and C. After that the 0.11.x line stays compatible, and
any further unavoidable public break waits for 0.12.

## One `parse`, one `emit`

Every grid exchange format now goes through `parse`, which opens the input
itself. You can hand it a file or directory name, content already in memory,
or a `Source` of named buffers, and all of them reach the same call.

```rust,ignore
let module = powerio::parse("case9.m")?;
let module = powerio::parse(case_directory)?;
let module = powerio::parse(bytes)?;

// Content in memory carries the name `<memory>`, which identifies no format,
// so a format read from a file extension is declared or named.
let module = powerio::parse_with_options(
    bytes,
    &powerio::ParseOptions::default().format("matpower")?,
)?;
let module = powerio::parse(powerio::Source::from_memory("case9.m", bytes)?)?;
```

| 0.10 | 0.11 |
|---|---|
| `parse(Source::open(path)?, None)` | `parse(path)` |
| `parse(source, Some(format))` | `parse_with_options(source, &ParseOptions::default().format(format)?)` |
| `emit(&module, format, Destination::path(path))` | `emit(&module, format, path)` |
| `serialize(&module, Destination::path(path))` | `serialize(&module, path)` |
| `deserialize(Source::open(path)?)` | `deserialize(path)` |
| `network.to_json()` | `serialize(&PioModule::new(network), destination)` |
| `BalancedNetwork::from_json(text)` | `deserialize(Source::from_memory("module.pio.json", text)?)?.into_value()` |
| `parse_display(source, from)` | `parse(input)`, which returns `PioValue::GeoLayer`; `PwdDisplay` remains for the raw display record |
| `DisplayData`, `DisplayFormat` | removed from Rust; Python keeps `parse_display` and `DisplayData` for the raw PowerWorld display record |
| a layer written by hand | `emit(&module, "geo-json", path)` |

Python accepts a path, a file object, or a bytes-like object. A `str` is
taken as a path, so wrap text already in memory in `io.StringIO`.

```python
module = powerio.parse("case9.m")
module = powerio.parse(io.StringIO(text), format="matpower", name="case9.m")
module = powerio.parse(binary_data, format="pwb", name="case.pwb")
```

Julia uses multiple dispatch on a path string, `IO`, or
`AbstractVector{UInt8}`.

```julia
module_ = parse("case9.m")
module_ = parse(IOBuffer(text); format="matpower", name="case9.m")
module_ = parse(bytes; format="pwb", name="case.pwb")
```

In C you construct a `PioSource` with `pio_source_open` or
`pio_source_from_memory` and then call `pio_parse`.

These 0.10 names are removed:

- `parse_file`
- `parse_text`
- `parse_str`
- `parse_bytes`

## Read the module fields

`PioModule<T>` contains the typed value and its diagnostics, producer, sources,
source mappings, history, and extensions. Rust, Python, and Julia expose
`value` and `diagnostics` as fields or properties.

In Rust, dynamic parsing returns `PioModule<PioValue>`, so match on
`module.value()` directly. In Python, use
`isinstance(module.value, BalancedNetwork)`; in Julia, dispatch on
`PioModule{BalancedNetwork}` or another concrete parameter.

`PioValueKind`, `.kind`, `try_into_typed`, `IntoTypedModule`, and the binding
wrappers that duplicated each language's own type inspection are removed. The
C ABI uses structural names such as `powerio.BalancedNetwork` and exact type
predicates instead of an ordinal value kind enum.

Diagnostics belong to the module, so Python and Julia no longer have a
callable `diagnostics()`.

## Emit formats; serialize PowerIO IR

`emit` is now the one way to produce a grid exchange format.

```rust,ignore
powerio::emit(&module, "matpower", "copy.m")?;
```

```python
memory_result = powerio.emit(module, "matpower")
file_result = powerio.emit(module, "psse", "case.raw")
```

```julia
memory_result = emit(module_, "matpower")
file_result = emit(module_, "psse", "case.raw")
```

The result lists every artifact it wrote, the output layout, the fidelity, and
the emission diagnostics, and the same call handles text, binary, single file,
and directory formats.

PowerIO IR goes through `serialize` and `deserialize` instead. `.pio.json` is
not a grid exchange format, so format discovery does not list it. Both calls
use the PowerIO IR document shape, `"schema": "pio-ir"` with integer
`"version": 2`, and the producer record names the PowerIO release separately.
Documents from earlier generations have to be regenerated from their original
power system data.

These 0.10 names are removed:

- `write_to`
- `write_string`
- `write_file`
- `to_format`
- module JSON read and write names
- `BalancedNetwork::to_json`, `from_json`, and `to_json_with_diagnostics`
- the `model-json` JSON classification family
- the `pio-json` format token

`to_*` is still there for a genuine semantic transformation in memory.

## Use ordinary collection operations

`TimeSeries<T>` and `ScenarioSet<T>` hold real typed values, so you use them
like any other collection. Rust has `len`, `iter`, and checked `get`; Python
has iteration and indexing; Julia has `length`, iteration, and 1-based
`getindex`; C has opaque length and element access.

Remove calls to `StateInventory`, `StateSelector`, `SelectedState`,
`list_states`, `select_state`, and `export_state` and index the collection
instead. What you get back is the contained typed value itself, or an owner
rooted typed view of it; nothing is encoded and reparsed on the way out.

## Apply typed updates

PowerIO 0.11 adds `OperatingPointUpdate`, `NetworkUpdate`,
`CalculationUpdate`, `apply_updates`, and `UpdateReport`. An update identifies
its component by stable `ComponentId` and gives an absolute value with an
explicit unit; the whole batch is validated first and then applied
atomically. In Julia the mutating functions end in `!`.

`UpdateReport` lists the component IDs and fields that changed and says
whether energized connectivity changed. A bus demand update has to name a load
or an explicit allocation rule, because PowerIO will not pick an arbitrary
load to receive aggregate demand. `LoadAllocation::ProportionalToCurrentActivePower`
keeps the current shares and refuses an all zero basis;
`LoadAllocation::Equal` splits the replacement evenly, which is how you
restore demand after setting every participating load to zero.

## Use calculation names that state the result

The DC data types `PioDcData`, `DcNetworkData`, and `dc_data`, and the phrase
"DC branch coefficients", are gone, though the DC OPF bundle files written by
`powerio dcopf` and `emit_dcopf_bundle` remain. Use the named calculations
instead:

```text
calc_incidence_matrix
calc_branch_susceptances
calc_bus_susceptance_matrix
calc_branch_flow_matrix
calc_branch_phase_shift_injection
calc_bus_phase_shift_injection
calc_branch_flow_dc
calc_bus_injection_dc
```

The public orientation and signs are:

```text
A[e, from] = +1
A[e, to]   = -1

Bf = Diagonal(b) * A
B  = A' * Diagonal(b) * A

p_branch = -Bf * va + b .* shift
p_shift  = A' * (b .* shift)
p_bus    = -B * va + p_shift
```

`BranchSusceptanceFormula` picks between the documented series susceptance,
tap adjusted reactance, and reactance only equations.

## Calculation instances and solutions

PowerIO 0.11 registers these calculation pairs:

```text
DcPfInstance       DcPfSolution
AcPfInstance       AcPfSolution
DcOpfInstance      DcOpfSolution
AcOpfInstance      AcOpfSolution
McAcPfInstance     McAcPfSolution
McAcOpfInstance    McAcOpfSolution
AcScucInstance     AcScucSolution
```

`SocwrOpfSolution` holds a PowerModels SOCWR relaxation, including its
W-space values and objective lower bound. PowerIO does not call it an
`AcOpfSolution` unless voltage recovery and AC residual checks support that
claim.

The initial assignment of an instance is its `initial_point` field.

## C ABI 7

PowerIO 0.11 replaces ABI 6 with ABI 7, which drops the ABI 4, 5, and 6
aliases and the Arrow export. Sources, destinations, modules, typed values,
collections, diagnostics, artifacts, sparse matrices, and vectors are all
opaque reference counted handles, every buffer comes with an explicit length,
and a borrowed typed handle keeps its module owner alive.

PowerIO.jl moves to ABI 7 in the same release. Julia packages should depend on
PowerIO.jl rather than call the C ABI directly.

## Inputs accepted by 0.11

PowerIO still accepts documented aliases for names that external formats
define, such as `rawx` for the canonical `psse-rawx` format token. An alias
like that names the same third party format; it does not keep a prerelease
PowerIO API or IR shape alive, and 0.11 has no beta source or document
aliases at all.
