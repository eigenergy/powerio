# Rust, Python, Julia, and C

The public path has the same shape in every language:

```text
parse_file or parse_text -> PioModule { value, diagnostics } -> to_* or calc_* -> emit
```

Value kinds, format names, diagnostic codes, units, and matrix signs do not
change at a language boundary. Ownership, dispatch, errors, and index bases
follow each language.

Surface specific operations and limits are listed in
[1.0 Scope and Known Limits](scope-v1.md).

| Concept | Rust | Python | Julia | C ABI |
|---|---|---|---|---|
| parse a path | `parse_file(path)` -> `PioModule<PioValue>` | `powerio.parse_file(path)` -> `PioModule` | `parse_file(path)` -> `PioModule{T}` | `pio_parse_file` -> `PioModule *` |
| read the value | match on `module.value()` | `module.value` | `module.value` | `pio_module_kind`, then the typed module accessor |
| diagnostics | `module.diagnostics()` -> `&[Diagnostic]` | `module.diagnostics` -> native records | `module.diagnostics` -> `Vector{Diagnostic}` | `pio_module_diagnostics` -> `PioDiagnostics *` |
| failure | `Err(Error)` carrying diagnostics | raised `PowerIOError` family | thrown `PowerIOError` with records | NULL/-1 plus `PioError *` out |
| transform to balanced | `transform::module_to_balanced` | `module.to_balanced()` | `to_balanced(module)` | `pio_module_to_balanced` |
| inspect the transform first | `transform::module_to_balanced_report` | `module.to_balanced_report()` | `to_balanced_report(module)` | `pio_module_to_balanced_report_json` |
| incidence matrix | `DcOperators::calc_incidence_matrix()` | `net.calc_incidence_matrix()` | `calc_incidence_matrix(module)` | assemble from the ABI 6 DC coefficient spans |
| bus susceptance matrix | `DcOperators::calc_bus_susceptance_matrix()` | `net.calc_bus_susceptance_matrix()` | `calc_bus_susceptance_matrix(module)` | assemble from the ABI 6 DC coefficient spans |
| branch susceptance matrix | `DcOperators::calc_branch_susceptance_matrix()` | `net.calc_branch_susceptance_matrix()` | `calc_branch_susceptance_matrix(module)` | assemble from the ABI 6 DC coefficient spans |
| phase shift injection | `DcOperators::calc_phase_shift_injection()` | `net.calc_phase_shift_injection()` | `calc_phase_shift_injection(module)` | `pio_dc_data_shift_injection` low level accessor |
| DC branch flow | `DcOperators::calc_branch_flow_dc(va)` | `net.calc_branch_flow_dc(va)` | `calc_branch_flow_dc(module, va)` | `pio_dc_data_fill_branch_flow` |
| emit text | `emit(&module, format, Destination::memory(name)?)` | `module.emit(format)` | `emit(module, format)` | `pio_module_emit_string` |
| emit files | `emit(&module, format, Destination::path(path))` | `module.emit(format, path)` | `emit(module, format, path)` | `pio_module_emit_file` |
| feature probe | Cargo features | `powerio.features()` | `has_feature`, `features()` | `pio_has_feature`, `pio_build_info` |

Rust narrows the dynamic value with ordinary enum matching:

```rust,ignore
let module = powerio::parse_file("case9.m")?;
match module.value() {
    powerio::PioValue::BalancedNetwork(network) => {
        println!("{} buses", network.buses().len());
    }
    other => println!("value kind: {}", other.kind().as_str()),
}
```

Python and Julia keep the module and typed value over the same native data.
The C typed accessors mint retained handles. Releasing the module does not
invalidate a retained child.

The `calc_*` prefix marks an operation that computes a new matrix or vector.
Noun spellings are reserved for stored fields and accessors. The fixed ABI 6
coefficient spans remain a low level exception rather than a second high level
matrix API.

Dense positions are zero based in Rust, Python, and C. Julia's public positions
and collection indices are one based. Source element identifiers keep the
source's own spelling in every language.

## 0.10 compatibility and low level input

The names above are the documented 1.0 path. Until the final 1.0 cleanup, the
released 0.10 entry points remain available without warnings: Python `parse`,
`to_format`,
`write_file`, `dc_data`, and the released noun matrix methods; Julia
`parse_bytes`, `to_format`, the `write_*` family, and the raw `DcData`
functions; Rust `write_module_*` and the released noun DC calculation
methods; and the original C `pio_module_write_*` and
`pio_dc_data_*` functions. They are listed in the
[final 1.0 cleanup](final-v1-api-cleanup.md) instead of mixed into examples.

Rust `parse(Source)` remains the lower level acquisition API for applications
that already own bytes or need a custom include root. The C ABI likewise keeps
`pio_parse_bytes` so bindings and applications can pass physical input without
a temporary file. These physical input operations are compatibility and FFI
building blocks, not a second high level object model.

The C page ([C ABI](capi.md)) and Python page ([Python API](python.md)) describe
their complete surfaces. PowerIO.jl documents Julia at
[eigenergy.github.io/PowerIO.jl](https://eigenergy.github.io/PowerIO.jl). The
command line tool and MCP server expose the same value kinds and conventions at
their own boundaries: [CLI and MCP](cli-mcp.md).
