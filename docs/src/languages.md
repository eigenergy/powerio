# Rust, Python, Julia, and C

Rust, Python, Julia, and C expose the same operations on the same power system
types:

```text
source -> parse -> PioModule<T> -> calculation or update -> emit
                         |
                         +-> serialize -> PowerIO IR
```

`parse` and `emit` handle grid exchange formats, while `serialize` and
`deserialize` handle PowerIO IR. A `calc_*` function returns a derived matrix,
vector, report, or count, and a `to_*` function transforms a value in memory
into another semantic type. The nouns are stored values and fields: module,
value, diagnostics, network, bus, branch, generator, load, operating point,
time series, scenario set, instance, solution.

| Meaning | Rust | Python | Julia | C ABI 7 |
|---|---|---|---|---|
| name a file | pass the name to `parse` | pass a path to `parse` | pass a path string to `parse` | `pio_source_open` |
| acquire memory | `Source::from_memory(name, bytes)` | pass a file or bytes-like object | pass `IO` or `AbstractVector{UInt8}` | `pio_source_from_memory` |
| parse | `parse(input)`, `parse_with_options(input, &options)` | `parse(source, format=..., name=...)` | `parse(source; format=..., name=...)` | `pio_parse` |
| module value | `module.value()` | `module.value` | `module.value` | `pio_module_value` |
| module diagnostics | `module.diagnostics()` | `module.diagnostics` | `module.diagnostics` | `pio_module_diagnostics` |
| emit a format | `emit(&module, format, destination)` | `emit(module, format, destination=None)` | `emit(module, format, destination=nothing)` | `pio_emit` |
| serialize IR | `serialize(&module, destination)` | `serialize(module, destination=None)` | `serialize(module, destination=nothing)` | `pio_module_serialize` |
| deserialize IR | `deserialize(source)` | `deserialize(source)` | `deserialize(source)` | `pio_module_deserialize` |
| apply updates | `apply_updates` | `apply_updates` | `apply_updates!` | `pio_apply_updates` |

To find out what you parsed, Rust matches on the `PioValue` case that
`module.value()` returns, Python uses `isinstance`, Julia dispatches on
`PioModule{T}` and on the concrete value types, and C compares canonical
structural type names with `pio_value_is_type` before calling the typed
accessor for the type it handles.

Only Rust and C have a `Source` type, because both need an explicit owner for
acquired bytes. A Python path, open file, or bytes-like object, or a Julia
path, `IO`, or byte vector, already says where the bytes come from, and the
interpreter owns them, so `parse` takes them directly and reads the source name
from the path or from the `name` argument.

A format made of related files goes through the same `parse`. For GO Challenge
3, a directory holding the problem file returns `AcScucInstance`, and once the
matching solution file sits beside it the same call returns `AcScucSolution`.

## Member access

Member names match across the four languages; what differs is the syntax, which
follows what each language's users expect.

| Language | A member | A table's length |
|---|---|---|
| Rust | accessor method: `module.value()`, `module.diagnostics()`, `network.buses()` | `network.buses().len()` |
| Python | read only property: `module.value`, `module.diagnostics`, `network.buses` | `len(network.buses)` or `network.n_buses` |
| Julia | property: `module.value`, `module.diagnostics`, `net.buses` | `length(net.buses)` |
| C | one function per member: `pio_module_value`, `pio_module_diagnostics` | a `_count` function |

Rust containers (`PioModule`, `BalancedNetwork`, `MulticonductorNetwork`,
instances, and solutions) keep their fields private because they maintain
invariants; `value_mut` severs retained source bytes, for example, and
`add_diagnostic` rejects a duplicate identifier. Rust element records (`Bus`,
`Branch`, `Generator`, `Load`, and the rest) are plain public structs, because
struct literals and pattern matching are how Rust users build and read them.
Python keeps the `n_*` counts because a table property builds one dict per row.
A Python element is a dict keyed by the Rust field names, while a Julia element
is an immutable struct whose field names are the C ABI names, which spell out
the quantity and unit (`vm_pu`, `active_power_mw`). Unifying those two sets of
field names is listed under [Known limits](scope-0.11.md).

## Collections

`TimeSeries<T>` and `ScenarioSet<T>` contain typed entries, and each language
reaches them its own way.

| Language | Operations |
|---|---|
| Rust | `len`, `iter`, checked `get` |
| Python | `len`, iteration, `series[index]`, `scenarios[id]`; `TimeSeries` is a `Sequence` and `ScenarioSet` a `Mapping` |
| Julia | `length`, iteration, 1-based `getindex` |
| C | zero-based length and entry access; scenario lookup by identifier |

An entry is either the contained value or a view rooted in the owning module,
so indexing does not serialize, expand, or copy a complete network.

## Calculations

The DC matrix and vector functions have the same names in each language:

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

Rust and C use zero based sparse matrix positions, Python sparse matrices use
SciPy's zero based positions, and Julia presents one based indices. Stable
component identities and source identifiers do not change at a language
boundary. [Matrices and graphs](matrices.md) gives the signs and equations.

## Errors and ownership

Rust returns `Result`, Python raises `PowerIOError` subclasses, Julia throws a
`PowerIOError` with structured diagnostics, and C returns a documented failure
value and writes one `PioError *` through its error output. Each failure has a
stable diagnostic code.

Python and Julia keep the native owner alive behind the borrowed typed views
they hand you, whereas C callers retain and release opaque handles themselves.
Ownership differs; the data types and the operation names do not.
