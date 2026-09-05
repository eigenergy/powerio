# CLI and MCP

## The powerio command

The `powerio` binary runs the same operations from a shell, and with no subcommand it opens the interactive TUI.

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

The other subcommands are `batch`, which writes matrix families for every
case in a directory, `gen`, which writes synthetic cases, `geo apply` and
`geo convert`, `corpus`, the private corpus harness described in
[Corpus harness](corpus-harness.md), and `tui`. `dcopf` accepts the alias
`dc-opf`. `--from` and `--to` take the format tokens and aliases of the
[format table](format-fidelity.md); `iidm` and `rawx` are accepted as
input spellings only.

Format names, structural value types, and diagnostic codes are the same
strings the language APIs use. Diagnostics print one per line on stderr as
`CODE: message`.

### Standard input and output

`convert`, `summary`, `serialize`, `verify`, `dcopf`, and `sensitivities` read
the case from standard input when the input is `-`. A stream has no file name
to infer a format from, so `--from` is required, and a gridfm dataset is a
directory, so it cannot arrive on a stream at all. `convert` and `serialize`
write a single text result to standard output when `-o` is `-` or omitted; a
directory format such as `pypsa-csv` or `cgmes` needs an output directory.

```sh
cat case9.m | powerio convert - --from matpower --to psse -o - > case9.raw
powerio serialize - --from psse < case9.raw
```

### Exit status

| Status | Meaning |
|---|---|
| 0 | success |
| 1 | a failure without a PowerIO error category |
| 2 | `request`: the arguments name something the request cannot satisfy (a format the writer cannot produce, a missing `--from` for standard input); clap usage errors also exit 2 |
| 3 | `io`: a path could not be read or written |
| 4 | `parse`: the input is malformed, or a refused include left the output incomplete |
| 5 | `data`: valid input that the operation cannot satisfy |
| 6 | `output`: the writer could not produce the requested format |

The categories are the `ErrorCategory` values the Rust and Python APIs report
on every failure, so a shell script and a Python caller branch on the same
five names.

Every failure the binary reports is a PowerIO error with a registered
diagnostic code, including the ones the command line raises itself
(`REQUEST.CLI.FORMAT_REQUIRED`, `REQUEST.CLI.TARGET_UNSUPPORTED`,
`REQUEST.CLI.OUTPUT_REQUIRED`, `REQUEST.CLI.FAMILY_MISMATCH`,
`REQUEST.CLI.OPTION_INVALID`, `REQUEST.CLI.NO_CASES`,
`PARSE.CLI.ERRORS_REPORTED`, `VALIDATE.CLI.INPUT_LACKS_DATA`,
`EMIT.CLI.SIDECAR_PATH`, `EMIT.CLI.ERRORS_REPORTED`). A reader that reports
errors ends the run with `PARSE.CLI.ERRORS_REPORTED` and status 4 after the
output is written; a writer that reports errors ends it with
`EMIT.CLI.ERRORS_REPORTED` and status 6. A failure that reaches the top of
the program without a registered code is reported as `BIND.CLI.UNCLASSIFIED`
and exits 1.

### Diagnostics format

`--diagnostics-format text` (the default) prints one `CODE: message` line per
diagnostic as the command runs, `wrote <path>` when a file lands, and a failure
as `Error:` and `Caused by:` lines.

`--diagnostics-format json` (accepted before or after the subcommand) prints
one JSON array on stderr when the command ends, and nothing else. The array
holds every diagnostic the run produced, warnings and errors alike, as PowerIO
IR diagnostic records: the same encoding a module's `diagnostics` field carries
in a `.pio.json` document, with the same fields (`id`, `severity`, `code`,
`message`, `target`, `spans`, `related`, `details`, `suggested_action`). A
failure adds its record, and each further reason in its cause chain becomes a
`note` record whose `related` names the failure record:

```json
[{"id": "failure", "severity": "error", "code": "READ.IO.OPEN",
  "message": "cannot open source `case9.m`"},
 {"id": "d0", "severity": "note", "code": "READ.IO.OPEN",
  "message": "No such file or directory (os error 2)", "related": ["failure"]}]
```

The exit status stays the process exit status and is not repeated inside the
records.

## The MCP server

`powerio-mcp` (from the Python package) calls the public Python API directly.
Its tools are `parse`, `emit`, `summarize`, `diagnostics`, `to_normalized`,
`calc_matrix`, `to_balanced_report`, `to_balanced`, `display`, and `about`.
`parse` returns serialized PowerIO IR, and the tools that accept `powerio_ir`
deserialize that same IR rather than defining a network, collection,
calculation, update, or solution representation of their own.

You address a collection entry with a plain zero based `time_index` or a
`scenario_id`; no tool exports or expands an entry into a static network.
`emit(format, destination)` writes a file or directory, and if you omit
`destination` it returns the artifacts in memory. Diagnostics stay structured
records with code, severity, message, target, and source spans.

```sh
pip install 'powerio[mcp]'   # Python 3.10 or later
powerio-mcp                  # stdio transport
```

The server accepts exactly one input source: serialized PowerIO IR, a grid
exchange path, or grid exchange content in memory. Filesystem access is off
unless `POWERIO_MCP_ALLOWED_ROOTS` names the directories the server may read,
and remote URI schemes are rejected.
