# PowerIO 0.11 API

PowerIO 0.10 was the public beta. PowerIO 0.11 keeps the useful domain types
and removes the beta operations that named the same action several ways. It is
the candidate 1.0 API under a compatibility-focused stabilization cycle.

The public path is:

```text
Source -> parse -> PioModule<T> -> transform, update, or calculate -> emit
                              |
                              +-> serialize -> PowerIO IR
```

## Vocabulary

- `parse` acquires a grid exchange representation from one `Source`.
- `emit` produces a grid exchange representation in memory or at a
  destination.
- `serialize` and `deserialize` encode and decode PowerIO IR.
- `calc_*` calculates a derived matrix, vector, report, or count.
- `to_*` transforms an in-memory value to another semantic type.
- Nouns name stored values and fields: module, value, diagnostics, network,
  bus, branch, generator, load, operating point, time series, scenario set,
  calculation instance, and solution.

There is no extra catch-all vocabulary for a network, operating point,
collection entry, or conversion. Established terms such as `state_of_charge`,
dynamic model states, and the official CGMES SV profile keep their standard
names.

## One concept in four languages

| Meaning | Rust | Python | Julia | C ABI 7 |
|---|---|---|---|---|
| parse | `parse(source, format)` | `parse(source, format=...)` | `parse(source; format=...)` | `pio_parse` |
| value | `module.value` | `module.value` | `module.value` | `pio_module_value` |
| diagnostics | `module.diagnostics` | `module.diagnostics` | `module.diagnostics` | `pio_module_diagnostics` |
| emit | `emit(&module, format, destination)` | `emit(module, format, destination)` | `emit(module, format, destination)` | `pio_emit` |
| serialize | `serialize(&module, destination)` | `serialize(module, destination)` | `serialize(module, destination)` | `pio_module_serialize` |
| deserialize | `deserialize(source)` | `deserialize(source)` | `deserialize(source)` | `pio_module_deserialize` |

Rust matches the actual `PioValue` enum case. Python uses `isinstance`. Julia
uses multiple dispatch on `PioModule{T}`. C uses canonical structural type
names and exact typed accessors. `PioValueKind`, `.kind`, `try_into_typed`, and
binding specific narrowing wrappers are absent.

## Direct DC calculations

There is no high level “DC data” or “DC branch coefficients” type. Call the
quantity the calculation needs:

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

The incidence matrix `A` is branches by buses, with `+1` at the from bus and
`-1` at the to bus:

```text
Bf = Diagonal(b) * A
B  = A' * Diagonal(b) * A

p_branch = -Bf * va + b .* shift
p_shift  = A' * (b .* shift)
p_bus    = -B * va + p_shift
```

`BranchSusceptanceFormula` selects the documented series susceptance, tap
adjusted reactance, or reactance only equation.

## Collections and updates

`TimeSeries<T>` and `ScenarioSet<T>` expose ordinary collection operations.
Entries are typed values or owner rooted views; access does not serialize or
copy a complete network.

`OperatingPointUpdate`, `NetworkUpdate`, and `CalculationUpdate` target stable
component IDs and carry absolute values with explicit units. `apply_updates`
validates the complete batch before mutation. `UpdateReport` lists exact
changes and whether energized connectivity changed.

## PowerIO IR and C ABI

PowerIO IR has one shape per release: `"schema": "powerio.module"` and
`"version"` naming the release that wrote it. A build reads the compatible
0.11.x documents no newer than itself and has no reader for 0.10 documents and
no source alias.

C ABI 7 is the only exported ABI. It has no ABI 4, 5, or 6 symbol aliases. All
values use opaque reference counted handles, explicit buffer lengths, one
structured error output, canonical structural type names, and owner rooted
typed access.

## Release gate

The same candidate commits must pass Rust, C, Python, and Julia conformance;
source retention and same format fidelity tests; matrix sign and orientation
tests; C header and symbol checks; and the Tellegen, WebMCP, PowerMCP,
ExaModelsPower, and BMOPFTools consumer suites. No tag or package publication
occurs before the maintainer review of those exact commits and results.
