# Final 1.0 API cleanup

PowerIO 0.10 is the 1.0 beta. Its maintenance releases add the preferred
surface without changing ABI 6, stored schema 1, format names, value kind
strings, or existing call semantics. The common path is:

```text
parse_file -> PioModule { value, diagnostics } -> transform or named matrix operation -> emit
```

`to_*` names are reserved for transformations that return an in-memory value.
`emit` writes an external representation. Named matrix operations expose the
matrix or vector a caller asked for instead of requiring an intermediate DC
coefficient container.

## 0.10 compatibility

Every 0.10.x release keeps the earlier names without warnings. Python retains
`parse`, `PioModule.from_file`, `from_str`, `from_bytes`, `to_format`,
`write_file`, the network write and conversion helpers, `dc_data`, and the
released noun matrix methods plus `MulticonductorNetwork.graph`. Julia
retains `parse_bytes`, `to_format`, the `write_*` family, and raw `DcData`
access. Rust retains its released noun DC calculation methods plus
`MulticonductorNetwork::graph` and `write_module_*`. C retains
`pio_parse_bytes`, `pio_module_write_*`, `pio_dc_data_*`, and
`pio_multiconductor_network_graph_json`.

These names make patch releases source compatible while users move to the
common path. They are not a second object model. Rust `parse(Source)` and C
`pio_parse_bytes` also remain the physical input layer for callers that already
own bytes; neither is promoted as the ordinary path API.

## The final source break

The 1.0 release removes the compatibility spellings from the high level source
surfaces:

- Python removes the names listed above, the callable `diagnostics()` alias,
  `conversion.warnings`, `n_gens`, `n_connected_components`,
  `to_balanced_inspect`, and `select_state`. The replacements are
  `parse_file`, the `value` and `diagnostics` properties, `emit`, the named DC
  methods `calc_incidence_matrix`, `calc_bus_susceptance_matrix`,
  `calc_branch_susceptance_matrix`, `calc_phase_shift_injection`, and
  `calc_branch_flow_dc`, `conversion.diagnostics`, `n_generators`,
  `n_islands`, `to_balanced_report`, and `inspect_state` or `export_state`.
  The noun matrix methods `bprime`, `bdoubleprime`, `ybus`, `ybus_parts`,
  `adjacency`, `ptdf`, `lodf`, `lacpf`, `weighted_laplacian`, and `incidence`
  leave in favor of their `calc_*` spellings. `calc_incidence_factors`
  replaces the low level factor form of `incidence`, and `to_graph` replaces
  the multiconductor `graph` method.
- Julia stops exporting the compatibility names above, `PowerIOCError`,
  `n_gens`, `n_components`, rendered `warnings`, ambiguous `sources`,
  `release_c_data`, the old balanced conversion names, and zero based DC
  endpoint accessors. The replacements are `PowerIOError`, `module_sources`,
  `voltage_sources`, `close`, `to_balanced`, `to_balanced_report`, and the one
  based position accessors. The five `calc_*` matrix and vector operations
  replace raw `DcData` functions in ordinary Julia code.
- Rust replaces the facade's broad `powerio_tx` glob with explicit exports.
  Indexed views, normalized compiler data, solver tables, and component error
  internals stay available from their component crates but stop appearing at
  the facade root. The old multiconductor conversion root exports and
  `write_module_*` facade aliases leave in favor of `transform` and `emit`.
  `MulticonductorToBalancedLowering` leaves in favor of
  `MulticonductorToBalancedTransformation`.
  The noun `DcOperators` calculation methods leave in favor of
  `calc_incidence_matrix`, `calc_bus_susceptance_matrix`,
  `calc_branch_susceptance_matrix`, `calc_phase_shift_injection`,
  `calc_branch_flow_dc`, and `calc_reference_constrained_system`.
  `MulticonductorNetwork::graph` leaves in favor of `to_graph`.

PowerIO.jl 0.10 has one released incidence name collision. The canonical
`calc_incidence_matrix(::PioModule{BalancedNetwork})` result is PowerModels
branch by bus, while the legacy `BalancedNetwork` and path overloads are bus by
branch. The final 1.0 cleanup reconciles those legacy overloads to the
branch by bus orientation; code that depends on their old orientation must
transpose explicitly during migration.

`try_into_typed` remains the advanced owned conversion unless a technically
sound standard trait replacement becomes possible; 1.0 does not promise its
removal.

The C ABI does not take this source break. ABI 6 keeps every existing symbol,
ownership rule, error convention, and index base. The `pio_module_emit_*`
and `pio_multiconductor_network_to_graph_json` functions are additive names
over the existing output machinery and graph projection. Old C names remain
callable so a library update does not require recompiling or rewriting an
existing binding.

## What stays fixed

The cleanup does not restore `NetworkPackage`, `DistNetwork`, `ScopfInstance`,
`OperatingPointSeries`, solver row tables, a generic `Network` type, or a
format token for PowerIO model JSON. Calculation kinds, diagnostic codes,
source ownership, same format byte echo, and matrix signs and orientations do
not change.

The release gate checks that only this inventory breaks. It also runs the Rust,
C, Python, and Julia API conformance tests, source retention tests, same format
echo tests, and DC matrix sign and orientation tests before the version changes
to 1.0.0.
