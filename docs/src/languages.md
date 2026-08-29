# Rust, Python, Julia, and C

One set of operations, one set of stable strings, four idioms. The table maps each concept to its spelling per surface; semantics, kinds, format names, diagnostic codes, signs, and units are identical everywhere, and each surface follows its own language's conventions for errors, ownership, and dispatch.

| Concept | Rust | Python | Julia | C ABI |
|---|---|---|---|---|
| parse a source | `parse(Source::open(path)?)` → `PioModule<PioValue>` | `powerio.parse(path)` → `PioModule` | `parse_file(path)` → `PioModule{T}` | `pio_parse_file` → `PioModule *` |
| parse named bytes | `parse(Source::from_bytes(name, bytes)?)` | `powerio.parse(bytes)` | `parse_bytes(bytes; name)` | `pio_parse_bytes` |
| the value kind | `module.value().kind().as_str()` | `module.kind` | `kind(m)` (or the type parameter) | `pio_module_kind` |
| typed narrowing | `try_into_typed::<T>(module)` (consuming, recoverable) | `value_type=` assertion + `module.value` | the `PioModule{T}` type itself; `m.value` | `pio_module_balanced_network`, `pio_module_multiconductor_network` |
| diagnostics | `module.diagnostics()` → `&[Diagnostic]` | `module.diagnostics()` → native records | `diagnostics(m)` → `Vector{Diagnostic}` | `pio_module_diagnostics` → `PioDiagnostics *` |
| failure | `Err(Error)` carrying diagnostics | raised `PowerIOError` family | thrown `PowerIOCError` with records | NULL/-1 + `PioError *` out |
| same format write | `write_module_str(&module, fmt)` | `module.value.to_format(to)` / `.write_file(path, to)` | `write_str(m; format)` / `write_file` | `pio_module_write_str` / `_write_file` |
| convert one call | `convert_file` | `powerio.convert_file` | `convert_file` | `pio_convert_file` |
| stored document | `stored::write_module` / facade parse | `PioModule.from_json` / `to_json` | `write_json(m)` / `parse_file` | `pio_module_write_json` / `_read_json` |
| state selection | `select` module ops | `module.export_state(...)` (zero based) | `select_state(m; time=…)` (one based) | `pio_module_export_state` (zero based) |
| balanced lowering | `transform::lower_module_to_balanced` | `module.to_balanced()` | `lower_to_balanced(m)` | `pio_module_lower_to_balanced` |
| DC branch data | `powerio_tx::dc_network_data()` | `net.dc_data()` | `dc_data` + `BorrowedVector` views | `pio_dc_data_*` spans |
| feature probe | Cargo features | `powerio.features()` | `has_feature`, `features()` | `pio_has_feature`, `pio_build_info` |

The differences are deliberate, and small:

- Rust owns and moves: narrowing consumes the dynamic module and hands it back on mismatch. Python and Julia share: the typed value and the module co-own the same native data, and finalizers or reference counts release it.
- Index bases follow the language: element identifiers are the source's own everywhere, dense positions are zero based at the C and Python boundary and one based in Julia's `select_state`, each stated in its documentation.
- Errors follow the language: `Result` in Rust, exceptions in Python and Julia, status plus `PioError` handles in C. All four carry the same coded records, except Python's `FileNotFoundError` for a missing file, which carries none (the deliberate Python idiom for a bad path).
- Borrowed numerical views (`C` spans, Julia `BorrowedVector`, Python buffer views) stay valid until their owner releases; `copy` produces an ordinary mutable array.

The C page ([C ABI](capi.md)) and the Python page ([Python API](python.md)) carry each surface's full story; PowerIO.jl documents Julia at [eigenergy.github.io/PowerIO.jl](https://eigenergy.github.io/PowerIO.jl). The CLI and MCP server expose the same operations over their own boundaries: [CLI and MCP](cli-mcp.md).
