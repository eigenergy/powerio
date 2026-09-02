# From the 1.0 beta to 1.0

PowerIO 0.10.0 was the public 1.0 beta. PowerIO 1.0 makes the final source and
ABI break. The
changes remove duplicate operations and expose the same concepts in Rust,
Python, Julia, and C.

## Parse one `Source`

Every grid exchange format enters through `parse`.

```rust,ignore
let source = powerio::Source::open("case9.m")?;
let module = powerio::parse(source, None)?;

let source = powerio::Source::from_memory("case9.m", bytes)?;
let module = powerio::parse(source, Some("matpower"))?;
# Ok::<(), powerio::Error>(())
```

Python accepts a path, file object, or bytes-like object. A `str` names a
path; use `io.StringIO` for text already in memory.

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

C constructs a `PioSource` with `pio_source_open` or
`pio_source_from_memory`, then calls `pio_parse`.

The following 0.10 names are removed:

- `parse_file`
- `parse_text`
- `parse_str`
- `parse_bytes`

## Read the module fields

`PioModule<T>` contains the typed value and its diagnostics, producer, sources,
source mappings, history, and extensions. Rust, Python, and Julia expose
`value` and `diagnostics` as fields or properties.

Rust dynamic parsing returns `PioModule<PioValue>`. Match `module.value`
directly. Python uses `isinstance(module.value, BalancedNetwork)`. Julia
dispatches on `PioModule{BalancedNetwork}` or another concrete parameter.

`PioValueKind`, `.kind`, `try_into_typed`, `IntoTypedModule`, and binding
wrappers that duplicate language type inspection are removed. The C ABI uses
structural names such as `powerio.BalancedNetwork` and exact type predicates;
it has no ordinal value kind enum.

Diagnostics belong to the module. Python and Julia no longer provide a
callable `diagnostics()` operation.

## Emit formats; serialize PowerIO IR

`emit` is the only operation that produces a grid exchange format.

```rust,ignore
powerio::emit(&module, "matpower", powerio::Destination::path("copy.m"))?;
# Ok::<(), powerio::Error>(())
```

```python
memory_result = powerio.emit(module, "matpower")
file_result = powerio.emit(module, "psse", "case.raw")
```

```julia
memory_result = emit(module_, "matpower")
file_result = emit(module_, "psse", "case.raw")
```

The result records every artifact, the output layout, fidelity, and emission
diagnostics. The same call handles text, binary, one file, and directory
formats.

Use `serialize` and `deserialize` for PowerIO IR. `.pio.json` is not a grid
exchange format and is absent from format discovery. Both operations use the
single PowerIO 1.0 document shape: `"schema": "powerio.module"` and
`"version": 1`. Documents produced by the beta are not PowerIO 1.0 IR and
must be regenerated from their original power system data.

The following 0.10 names are removed:

- `write_to`
- `write_string`
- `write_file`
- `to_format`
- module JSON read and write names
- the `pio-json` format token

`to_*` remains available for a genuine in-memory semantic transformation.

## Use ordinary collection operations

`TimeSeries<T>` and `ScenarioSet<T>` contain actual typed values. Rust uses
`len`, `iter`, and checked `get`; Python uses iteration and indexing; Julia
uses `length`, iteration, and 1-based `getindex`; C provides opaque length and
element access.

Remove calls to `StateInventory`, `StateSelector`, `SelectedState`,
`list_states`, `select_state`, and `export_state`. Index the collection
instead. A returned entry is the contained typed value or an owner rooted
typed view. It is not encoded and reparsed.

## Apply typed updates

PowerIO defines `OperatingPointUpdate`, `NetworkUpdate`,
`CalculationUpdate`, `apply_updates`, and `UpdateReport`. Updates identify a
component by stable `ComponentId`, carry absolute values with explicit units,
validate the whole batch, and apply atomically. Julia mutation operations end
in `!`.

`UpdateReport` lists the component IDs and fields changed and reports whether
energized connectivity changed. A bus demand update must name a load or an
explicit allocation rule. PowerIO never assigns aggregate demand to an
arbitrary load. `LoadAllocation::ProportionalToCurrentActivePower` preserves
the current shares and refuses an all zero basis. `LoadAllocation::Equal`
divides the replacement equally, so a caller can explicitly restore demand
after setting every participating load to zero.

## Use calculation names that state the result

The public DC bundle types and the phrase “DC branch coefficients” are gone.
Use the named calculations:

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

`BranchSusceptanceFormula` selects the documented series susceptance,
tap adjusted reactance, or reactance only equation.

## Calculation instances and solutions

PowerIO 1.0 registers the following calculation pairs:

```text
DcPfInstance       DcPfSolution
AcPfInstance       AcPfSolution
DcOpfInstance      DcOpfSolution
AcOpfInstance      AcOpfSolution
McAcPfInstance     McAcPfSolution
McAcOpfInstance    McAcOpfSolution
AcScucInstance     AcScucSolution
```

`SocwrOpfSolution` records a PowerModels SOCWR relaxation, including its
W-space values and objective lower bound. It is not an `AcOpfSolution` unless
voltage recovery and AC residual checks support that claim.

Instance fields use `initial_point`, not the catch-all beta name.

## C ABI 7

PowerIO 1.0 replaces ABI 6 with ABI 7. ABI 7 has no ABI 4, 5, or 6 aliases and
does not reuse removed table IDs. Sources, destinations, modules, typed values,
collections, diagnostics, artifacts, sparse matrices, and vectors use opaque
reference counted handles. Every buffer carries an explicit length. Borrowed
typed handles keep their module owner alive.

PowerIO.jl moves to ABI 7 in the same release. Julia packages should depend on
PowerIO.jl rather than call the C ABI directly.

## Inputs accepted by 1.0

PowerIO accepts documented aliases for names defined by external formats, such
as `rawx` for the canonical `psse-rawx` format token. Those aliases identify
the same third party format; they do not preserve a prerelease PowerIO API or
IR shape. There are no prerelease source or document aliases in 1.0.
