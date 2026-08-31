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
| transform to balanced | `transform::to_balanced` | `module.to_balanced()` | `to_balanced(module)` | `pio_module_to_balanced` |
| inspect the transform first | `transform::to_balanced_report` | `module.to_balanced_report()` | `to_balanced_report(module)` | `pio_module_to_balanced_report_json` |
| parse text in memory | `parse_text(name, text, format)` | `powerio.parse_text(text, name=..., format=...)` | `parse_text(text; name=..., format=...)` | `pio_parse_text` -> `PioModule *` |
| incidence matrix | `DcOperators::calc_incidence_matrix()` | `net.calc_incidence_matrix()` | `calc_incidence_matrix(module)` | assemble from `pio_dc_data_from_indices` and `pio_dc_data_to_indices` |
| bus susceptance matrix | `DcOperators::calc_bus_susceptance_matrix()` | `net.calc_bus_susceptance_matrix()` | `calc_bus_susceptance_matrix(module)` | assemble from the endpoint and susceptance arrays on `PioDcData` |
| branch susceptance matrix | `DcOperators::calc_branch_susceptance_matrix()` | `net.calc_branch_susceptance_matrix()` | `calc_branch_susceptance_matrix(module)` | assemble from the endpoint and susceptance arrays on `PioDcData` |
| phase shift injection | `DcOperators::calc_phase_shift_injection()` | `net.calc_phase_shift_injection()` | `calc_phase_shift_injection(module)` | `pio_dc_data_shift_injection` low level accessor |
| DC branch flow | `DcOperators::calc_branch_flow_dc(va)` | `net.calc_branch_flow_dc(va)` | `calc_branch_flow_dc(module, va)` | `pio_dc_data_calc_branch_flow` |
| DC bus injection | `DcOperators::calc_bus_injection_dc(va)` | `net.calc_bus_injection_dc(va)` | `calc_bus_injection_dc(module, va)` | assemble `-B * va + p_shift` from the frozen `PioDcData` arrays |
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
`PioDcData` arrays remain a low level FFI exception rather than a second high
level matrix API.

Dense positions are zero based in Rust, Python, and C. Julia's public positions
and collection indices are one based. Source element identifiers keep the
source's own spelling in every language.

## Physical input and C ABI compatibility

The names above are the 1.0 path. The released 0.10 high level Python, Julia,
and Rust aliases are removed; the [1.0 API surface](final-v1-api-cleanup.md)
lists their replacements. C ABI 6 keeps all released symbols. Its new
`pio_parse_text` name forwards to the released `pio_parse_str` operation, and
`pio_parse_bytes` remains available for binary input.

Rust `parse(Source)` remains the lower level acquisition API for applications
that already own bytes or need a custom include root. These physical input
operations are FFI and parser building blocks, not a second high level object
model.

The C page ([C ABI](capi.md)) and Python page ([Python API](python.md)) describe
their complete surfaces. PowerIO.jl documents Julia at
[eigenergy.github.io/PowerIO.jl](https://eigenergy.github.io/PowerIO.jl). The
command line tool and MCP server expose the same value kinds and conventions at
their own boundaries: [CLI and MCP](cli-mcp.md).
