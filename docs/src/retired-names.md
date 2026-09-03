# Retired crates and API names

0.10 removes the pre-1.0 surfaces the architecture retired. Each row names the replacement; the [migration guide](migration-v0.10.md) walks the mechanical changes.

## Crates

| Retired | Replacement |
|---|---|
| `powerio-pkg` | dissolved into the `powerio` facade: `PioModule`, `PioValue`, the stored document |
| `powerio-diag` | `powerio-core`'s diagnostics |

The old crates stay on crates.io at their final 0.x versions; nothing publishes them again.

## Types and operations

| Retired | Replacement |
|---|---|
| `NetworkPackage`, `Package` (Python), `read_package` (Julia) | `PioModule` and the one parse |
| `StoredModule` as a public Julia type | `PioModule{T}` |
| `ScopfInstance` | `AcScucInstance` (DOE GO Challenge 3) and the settled instance families |
| `OperatingPointSeries` | `TimeSeries<OperatingPoint<N>>` as a module value |
| `parse_as`, type marker parse forms | one parse with automatic detection; a format name overrides detection only |
| public solver row tables (`NormalizedSolverTables`, dense row arrays) | hidden from the documented surface; the frozen 0.9 upgrade reader is the one remaining consumer, and the rows leave entirely when it retires |
| `pio_package_*`, `pio_scopf_*` C entry points | the module surface: `pio_parse_*`, `pio_module_*` |
| the network returning C parse family and its error buffers | module handles and structured `PioError` |
| `pio_dist_abi_version`, `pio_dist_capabilities_json`, `pio_matrix_available` | one ABI handshake plus `pio_has_feature` |
| the `pkg` cargo feature | gone; the release feature set is `arrow,matrix,gridfm,dist,prob` |
| `powerio-json` as a format token | none: model JSON is a network serialization, never a case format |
| the `study` stored field | refused by the upgrade reader with the 0.9 materialize instruction |

## Vocabulary

`package` no longer names anything current: the JSON classification family for a stored document is `module`, the CLI subcommand is `powerio module`, and the MCP transport speaks `module_json`. `warnings` as a separate channel is gone; a warning is a diagnostic severity and every finding carries a stable code.
