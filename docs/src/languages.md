# Rust, Python, Julia, and C

The four language surfaces use the same operations and the same power system
types:

```text
source -> parse -> PioModule<T> -> calculation or update -> emit
                         |
                         +-> serialize -> PowerIO IR
```

`parse` and `emit` handle grid exchange formats. `serialize` and
`deserialize` handle PowerIO IR. `calc_*` names a derived matrix, vector,
report, or count. `to_*` names an in memory transformation to another
semantic type. Nouns name stored values and fields: module, value,
diagnostics, network, bus, branch, generator, load, operating point, time
series, scenario set, instance, solution.

| Meaning | Rust | Python | Julia | C ABI 7 |
|---|---|---|---|---|
| name a file | pass the name to `parse` | pass a path to `parse` | pass a path string to `parse` | `pio_source_open` |
| acquire memory | `Source::from_memory(name, bytes)` | pass a file or bytes-like object | pass `IO` or `AbstractVector{UInt8}` | `pio_source_from_memory` |
| parse | `parse(input)`, `parse_with_options(input, &options)` | `parse(source, format=..., name=...)` | `parse(source; format=..., name=...)` | `pio_parse` |
| module value | `module.value()` | `module.value` | `module.value` | `pio_module_value` |
| module diagnostics | `module.diagnostics` | `module.diagnostics` | `module.diagnostics` | `pio_module_diagnostics` |
| emit a format | `emit(&module, format, destination)` | `emit(module, format, destination=None)` | `emit(module, format, destination=nothing)` | `pio_emit` |
| serialize IR | `serialize(&module, destination)` | `serialize(module, destination=None)` | `serialize(module, destination=nothing)` | `pio_module_serialize` |
| deserialize IR | `deserialize(source)` | `deserialize(source)` | `deserialize(source)` | `pio_module_deserialize` |
| apply updates | `apply_updates` | `apply_updates` | `apply_updates!` | `pio_apply_updates` |

Rust matches the `PioValue` case that `module.value()` returns. Python uses
`isinstance`. Julia dispatches on `PioModule{T}` and on the concrete value
types. C compares canonical structural type names with `pio_value_is_type`
and calls the typed accessor for the type it handles.

Only Rust and C have a `Source` type, because both need an explicit owner
for acquired bytes. A Python path, open file, or bytes-like object, and a
Julia path, `IO`, or byte vector, already say where the bytes come from and
are owned by the interpreter, so `parse` takes them directly and reads the
source name from the path or the `name` argument.

A format made of related files uses the same `parse`. For GO Challenge 3, a
directory holding the problem file returns `AcScucInstance`; the matching
solution file beside it makes the same call return `AcScucSolution`.

## Collections

`TimeSeries<T>` and `ScenarioSet<T>` hold typed entries.

| Language | Operations |
|---|---|
| Rust | `len`, `iter`, checked `get` |
| Python | `len`, iteration, `series[index]`, `scenarios[id]` |
| Julia | `length`, iteration, 1-based `getindex` |
| C | zero-based length and entry access; scenario lookup by identifier |

An entry is the contained value or a view rooted in the owning module.
Indexing does not serialize, expand, or copy a complete network.

## Calculations

The DC matrix and vector names agree across the languages:

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

Rust and C use zero-based sparse matrix positions. Python sparse matrices
use SciPy's zero-based positions. Julia presents 1-based indices. Stable
component identities and source identifiers do not change at a language
boundary. [Matrices and graphs](matrices.md) states the signs and equations.

## Errors and ownership

Rust returns `Result`. Python raises `PowerIOError` subclasses. Julia throws
a `PowerIOError` carrying structured diagnostics. C returns a documented
failure value and writes one `PioError *` through its error output. Every
failure carries a stable diagnostic code.

Python and Julia keep native owners alive behind borrowed typed views. C
callers retain and release opaque handles explicitly. Ownership details do
not change the data types or the operation names.
