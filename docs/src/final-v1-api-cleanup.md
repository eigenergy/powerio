# Final 1.0 API

PowerIO 0.10 was the public 1.0 beta. The next release is the 1.0 candidate.
It removes the last source compatibility
aliases while keeping ABI 6, stored schema 1, format names, value kind strings,
diagnostic codes, matrix signs, and matrix orientations fixed.

The common path is:

```text
parse_file or parse_text
    -> PioModule { value, diagnostics, sources, history }
    -> transform or calc_*
    -> emit
```

The vocabulary follows the intersection of LLVM style operation naming and
the established PowerModels, PowerModelsDistribution, MATPOWER, pandapower,
PyPSA, PowerSystems, ExaModelsPower, and BMOPFTools vocabulary:

- Functions are verbs. A computed matrix, vector, report, or count uses
  `calc_*`.
- Nouns name stored values and fields: `value`, `diagnostics`, `sources`,
  `buses`, `branches`, `generators`, and `loads`.
- `to_*` transforms an in-memory value without choosing an external
  representation.
- `emit` produces a selected external representation in memory or at a
  destination.
- `parse_file` acquires a file or directory. `parse_text` accepts text already
  in memory and cannot resolve relative includes. Rust `parse(Source)` and C
  `pio_parse_bytes` remain the lower level binary input paths.
- `PioModule` owns parse and conversion diagnostics. A value does not expose a
  second diagnostics method that contradicts the module field.

## One concept in four languages

| Concept | Rust | Python | Julia | C ABI 6 |
|---|---|---|---|---|
| Read a path | `parse_file(...)` | `parse_file(...)` | `parse_file(...)` | `pio_parse_file(...)` |
| Parse text | `parse_text(...)` | `parse_text(...)` | `parse_text(...)` | `pio_parse_text(...)` |
| Parsed value | `module.value()` | `module.value` | `module.value` | typed handle taken from `PioModule` |
| Diagnostics | `module.diagnostics()` | `module.diagnostics` | `module.diagnostics` | `pio_module_diagnostics(...)` |
| In-memory transform | `to_*` | `to_*` | multiple-dispatch `to_*` | operation-specific `pio_*_to_*` |
| Calculate derived data | `calc_*` | `calc_*` | multiple-dispatch `calc_*` | operation-specific `pio_*_calc_*` |
| External representation | `emit(...)` | `module.emit(...)` | `emit(module, ...)` | `pio_module_emit_string/file(...)` |
| Format metadata | `resolve_format(...)` | `resolve_format(...)` | `resolve_format(...)` | `pio_resolve_format_json(...)` |

Language bindings keep their own idioms. Julia uses multiple dispatch over a
small set of Julia structs. Python uses properties, keyword arguments, and
path-like objects. Neither binding mirrors Rust ownership types or exposes the
C handle graph.

## Direct DC operations

There is no high level “DC data”, “DC branch coefficients”, or replacement
umbrella container. Those phrases do not identify a standard power system
quantity. Call the quantity required by the calculation:

- `calc_incidence_matrix`
- `calc_bus_susceptance_matrix`
- `calc_branch_susceptance_matrix`
- `calc_phase_shift_injection`
- `calc_branch_flow_dc`
- `calc_bus_injection_dc`

The incidence matrix `A` is branches by buses, with `+1` at the from bus and
`-1` at the to bus. The bus and branch matrices are
`B = A' * Diagonal(b) * A` and `Bf = Diagonal(b) * A`. Phase shift injection
stays separate. Branch flow is `-Bf * va + b .* shift`; bus injection is
`-B * va + p_shift`.

`BranchSusceptanceFormula` names the formula used to calculate `b`:
`series_susceptance`, `tap_adjusted_reactance`, or `reactance_only`. It is not
a data model or a generic DC convention.

ABI 6 retains `PioDcData` and its `pio_dc_data_*` symbols. This opaque handle
exists to own C arrays and their row mappings across FFI calls. It is not a
shared PowerIO domain type and is not surfaced by Rust, Python, or Julia.

## The final source break from 0.10

The high level source bindings remove the aliases that 0.10 carried for the
beta transition.

Python removes `parse`, the native `PioModule.from_file`, `from_str`, and
`from_bytes` constructor aliases, `to_format`,
`write_file`, the private native in-memory binary display parser, `dc_data`, callable
`diagnostics()`, `conversion.warnings`, noun matrix methods, and duplicate
count, graph, geography, selection, and GridFM spellings. PowerWorld `.pwd`
display data remains available through `parse_display_file` because the format
is binary rather than text. `BalancedNetwork` and `MulticonductorNetwork` also
remove their
value-level `diagnostics`, `read_warnings`, and `warnings` views; diagnostics
belong only to the owning `PioModule`. Use `parse_file` or `parse_text`, module
fields, `calc_*`, `to_*`, and `emit`. GridFM directories also use `parse_file`;
their scenarios are a `ScenarioSet` selected through `list_states` and
`export_state`, so the duplicate `GridfmRead` and `read_gridfm*` surface is
removed.

Julia removes `parse_bytes`, `to_format`, the `write_*` family, raw `DcData`,
`PowerIOCError`, rendered `warnings`, ambiguous `sources`, zero based endpoint
accessors, and duplicate balanced conversion names. Use `parse_file` or
`parse_text`, `PowerIOError`, module fields, one based Julia positions,
multiple-dispatch `calc_*` and `to_*`, and `emit`.

Rust removes the 0.10 forwarding aliases, the broad facade glob, and the high
level `DcNetworkData`/`dc_network_data` container. The facade exports the
stable entry types and operations explicitly. Compiler data remains available
from the component crates but is not duplicated at the facade root. Ordinary
callers read or match `module.value()`; owned generic narrowing, where needed,
is an advanced operation rather than the main parse path.

The following Rust name groups use their final verb forms:

- DC and matrix calculations: `calc_*`
- validation: `check_*`
- component format name parsing: `parse_*`
- facade artifact metadata: `resolve_format`
- collection discovery: `list_*` or `find_*`
- algorithm choice: `select_*`
- in-memory projection: `to_*` or `map_*`
- external representation: `emit*`

The public matrix calculation replacements are:

| 0.10 Rust name | 1.0 Rust name |
|---|---|
| `build_bprime` | `calc_bprime_matrix` |
| `build_bdoubleprime` | `calc_bdoubleprime_matrix` |
| `build_ybus` | `calc_admittance_matrix` |
| `build_lacpf` | `calc_lacpf_matrix` |
| `build_adjacency` | `calc_adjacency_matrix` |
| `build_ptdf`, `build_lodf` | `calc_ptdf`, `calc_lodf` |
| `build_ptdf_lodf` | `calc_ptdf_lodf` |
| `build_ptdf_lodf_with_options` | `calc_ptdf_lodf_with_options` |
| `build_weighted_laplacian` | `calc_weighted_laplacian` |
| `build_flow_map` | `calc_branch_flow_matrix` |
| `build_multiconductor_admittance` | `calc_multiconductor_admittance_matrix` |
| `build_kind` | `calc_matrix` |
| `build_dc_opf_matrices` | `calc_dc_opf_matrices` |

`build_dc_opf_preparation` and `build_ac_opf_preparation` keep `build_*`: they
construct solver preparation records rather than calculate one derived matrix
or vector.

DC OPF preparation also makes its distinct positive solver quantities
explicit. `DcBranchData`, `DcGeneratorData`, and `NodalGeneratorData` become
`DcBranchParameters`, `DcGeneratorParameters`, and
`NodalGeneratorParameters`. `branches.b` becomes
`branches.susceptance_magnitude`. `DcOpfMatrices.incidence` and
`DcOpfMatrices.flow_map` become `bus_branch_incidence` and
`branch_flow_matrix`. These are not aliases for the signed, branch by bus
PowerModels operators.

PowerIO.jl 0.10 had one incidence name collision. The final
`calc_incidence_matrix` result is always the PowerModels branch by bus
orientation. Code that used the old bus by branch overload must transpose
explicitly during migration.

## C ABI stability

The C ABI does not take the source break. ABI 6 keeps every released symbol,
ownership rule, error convention, and zero based index. Preferred additive
symbols include `pio_parse_text`, `pio_module_emit_*`,
`pio_balanced_network_to_normalized`, the geography and module `to_*`
operations, `pio_dc_data_calc_branch_flow`,
`pio_module_list_states_json`, and
`pio_multiconductor_network_to_graph_json`.

Released spellings such as `pio_parse_str`, `pio_parse_bytes`,
`pio_module_write_*`, `pio_module_of_*`, `pio_convert_*`, and the complete
`pio_dc_data_*` family remain callable. A library update does not require an
existing ABI 6 consumer to recompile or rewrite its binding.

`pio_module_inspect_json.operations` also remains unchanged. Its additive
`preferred_operations` and `compatibility_operations` arrays distinguish the
final grammar from retained ABI names.

## What stays fixed

The final cleanup does not restore `NetworkPackage`, `DistNetwork`,
`ScopfInstance`, `OperatingPointSeries`, solver row tables, a generic
`Network`, a PowerIO case format, or a source-level DC container. It does not
rename standard buses, branches, generators, loads, shunts, transformers,
operating points, problem instances, solutions, time series, or scenario
sets.

Release readiness requires the same candidate commit to pass the Rust, C,
Python, and Julia API tests; source retention and same format echo tests; DC
sign and orientation tests; C header and symbol checks; documentation and
terminology checks; and the Tellegen and PowerMCP consumer suites. The release
intent remains draft until all of those checks pass.
