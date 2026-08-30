# CLI and MCP

## The powerio command

The `powerio` binary drives the same operations from a shell. With no subcommand it opens the interactive TUI.

```sh
powerio convert case14.m --to psse -o case14.raw   # parse + write, findings on stderr
powerio summary case14.m                           # the canonical network summary JSON
powerio module case14.m -o case14.pio.json         # the stored module document
powerio module gridfm_case14/ --scenario 7 -o one.pio.json  # one scenario as a static module
powerio verify case30.m --kind bdoubleprime        # matrix stats and the SDDM check
powerio batch -i tests/data -o out --matrices bprime,bdoubleprime
powerio sensitivities case30.m -o out              # PTDF and LODF
powerio dcopf case30.m -o out                      # the static DC OPF bundle
powerio gridfm case14.m -o out                     # a GridFM Parquet dataset
powerio geo extract case.aux -o layer.geo.json     # standalone geographic layers
```

Format names, value kinds, and diagnostic codes are the same stable strings the language APIs use, so a script can branch on them. Exit status is nonzero on failure; findings render one per line as `CODE: message`.

## The MCP server

`powerio-mcp` (from the Python package) serves the same operations to MCP clients: parse, inspect, diagnostics, conversion, writing, state inventory and selection, conversion to a balanced network, and DC matrix data. Tool inputs and outputs use the stable kind and format strings, and diagnostics cross as structured records with code, severity, message, and target. Every tool response also carries `schema` (a dotted name such as `powerio.parse`) and `powerio_version` (the release that produced it), so a client can identify a response's shape before reading further.

```sh
pip install 'powerio[mcp]'
powerio-mcp            # stdio transport
```

The server operates on modules the way every other surface does; a stored document, a case file, or in-memory content are equivalent inputs. Local filesystem access is explicit: the server reads the paths the client names and nothing else.
