# Rust, Python, Julia, and C

The four language surfaces use the same operations and power system types:

```text
source -> parse -> PioModule<T> -> calculation or update -> emit
                         |
                         +-> serialize -> PowerIO IR
```

`parse` and `emit` handle grid exchange representations. `serialize` and
`deserialize` handle PowerIO IR. `calc_*` names a derived matrix or vector.
`to_*` is reserved for an in-memory transformation to another semantic type.

| Meaning | Rust | Python | Julia | C ABI 7 |
|---|---|---|---|---|
| name a file | pass the name to `parse` | pass a path object to `parse` | pass a path string to `parse` | `pio_source_open` |
| acquire memory | `Source::from_memory(name, bytes)` | pass a file or bytes-like object | pass `IO` or `AbstractVector{UInt8}` | `pio_source_from_memory` |
| parse | `parse(input)`, `parse_with_options(input, &options)` | `parse(source, format=..., name=...)` | `parse(source; format=..., name=...)` | `pio_parse` |
| module value | `module.value` | `module.value` | `module.value` | `pio_module_value` |
| module diagnostics | `module.diagnostics` | `module.diagnostics` | `module.diagnostics` | `pio_module_diagnostics` |
| emit a format | `emit(&module, format, destination)` | `emit(module, format, destination=None)` | `emit(module, format, destination=nothing)` | `pio_emit` |
| serialize IR | `serialize(&module, destination)` | `serialize(module, destination=None)` | `serialize(module, destination=nothing)` | `pio_module_serialize` |
| deserialize IR | `deserialize(source)` | `deserialize(source)` | `deserialize(source)` | `pio_module_deserialize` |
| apply updates | `apply_updates` | `apply_updates` | `apply_updates!` | `pio_apply_updates` |

Rust matches `PioValue` enum cases through `module.value`. Python uses
`isinstance`. Julia dispatches on `PioModule{T}` and concrete value types. C
uses canonical structural type names, an exact type predicate, and owner
rooted typed accessors. None of the four surfaces has an ordinal value kind
enum or a typed narrowing wrapper.

Only Rust and C have a `Source` type. Both need an explicit owner for acquired
bytes: a Rust caller must say who holds the buffers a parse reads and when they
are released, and a C caller must pair every acquisition with a release call,
so `Source` makes that ownership visible at the call site. Python and Julia
values already carry the same information. A path, an open file object, or a
bytes-like object states where the bytes come from, and the interpreter owns
and frees them, so `parse` takes those values directly and derives the source
name from the path or the `name` argument. A `Source` class in Python or Julia
would restate Rust ownership mechanics without adding information.

Formats made of related files use the same `parse` operation. For GO Challenge
3, a directory containing only the problem returns `AcScucInstance`; adding
the matching solution file makes it return `AcScucSolution` in all four
languages.

## Collections

`TimeSeries<T>` and `ScenarioSet<T>` contain typed entries.

| Language | Operations |
|---|---|
| Rust | `len`, `iter`, checked `get` |
| Python | `len`, iteration, `series[index]`, `scenarios[id]` |
| Julia | `length`, iteration, 1-based `getindex` |
| C | zero-based length and entry access; scenario ID lookup |

An entry is the contained value or an owner rooted view. Collection indexing
does not serialize, expand, or copy a complete network.

## Calculations

The matrix and vector names agree across languages:

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

Rust and C use zero-based sparse matrix positions. Python sparse matrices use
SciPy's zero-based positions. Julia presents 1-based indices. Stable PowerIO
component IDs and source identifiers do not change at a language boundary.

## Errors and ownership

Rust returns `Result`. Python raises `PowerIOError` subclasses. Julia throws a
`PowerIOError` carrying structured diagnostics. C returns a documented failure
value and writes one `PioError *` through its error output.

Python and Julia wrappers keep native owners alive for borrowed typed views. C
callers retain and release opaque handles explicitly. These ownership details
do not change the data types or operation names.
