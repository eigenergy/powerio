# CLI and MCP

## The powerio command

The `powerio` binary drives the same operations from a shell. With no subcommand it opens the interactive TUI.

```sh
powerio convert case14.m --to psse -o case14.raw   # parse + emit, findings on stderr
powerio summary case14.m                           # the canonical network summary JSON
powerio serialize case14.m -o case14.pio.json      # PowerIO IR
powerio verify case30.m --kind bdoubleprime        # matrix stats and the SDDM check
powerio batch -i tests/data -o out --matrices bprime,bdoubleprime
powerio sensitivities case30.m -o out              # PTDF and LODF
powerio dcopf case30.m -o out                      # the static DC OPF bundle
powerio gridfm case14.m -o out                     # a GridFM Parquet dataset
powerio geo extract case.aux -o layer.geo.json     # standalone geographic layers
```

Format names, structural value types, and diagnostic codes are the same strings the language APIs use. Exit status is nonzero on failure; diagnostics render one per line as `CODE: message`.

## The MCP server

`powerio-mcp` (from the Python package) calls the public Python API directly.
Its tools are `parse`, `emit`, `summarize`, `diagnostics`, `to_normalized`,
`calc_matrix`, `to_balanced_report`, `to_balanced`, `display`, and `about`.
`parse` returns serialized PowerIO IR. The tools that accept `powerio_ir`
deserialize that same IR; they do not define another network, collection,
calculation, update, or solution representation.

Collection entries use an ordinary zero-based `time_index` or a
`scenario_id`. No tool exports or expands an entry into a static network.
`emit(format, destination)` writes a file or directory; omit `destination` to
return in-memory artifacts. Diagnostics remain structured records with code,
severity, message, target, and source spans.

```sh
pip install 'powerio[mcp]'
powerio-mcp            # stdio transport
```

The server accepts exactly one input source: serialized PowerIO IR, a grid
exchange path, or in-memory grid exchange content. Filesystem access is
disabled unless allowed roots are configured; remote URI schemes are rejected.
